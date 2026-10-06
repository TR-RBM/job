use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::model::{Job, State};
use crate::objects::{Graph, Kind, ROOT};

use super::{Ledger, message};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Counter(pub u128);

impl Serialize for Counter {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for Counter {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let value: u128 = text.parse().map_err(serde::de::Error::custom)?;
        if value.to_string() != text {
            return Err(serde::de::Error::custom(message(
                "invalid fair-share accounting",
            )));
        }
        Ok(Self(value))
    }
}

fn overflow() -> String {
    message("fair-share accounting overflow; admission stopped")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Resource {
    CpuRequestTime,
    MemoryRequestTime,
}

impl Resource {
    pub fn name(self) -> &'static str {
        match self {
            Self::CpuRequestTime => "cpu-request-time",
            Self::MemoryRequestTime => "memory-request-time",
        }
    }

    fn rate(self, job: &Job) -> u64 {
        match self {
            Self::CpuRequestTime => job.reservation.vector.cores_milli,
            Self::MemoryRequestTime => job.reservation.vector.memory,
        }
    }
}

pub fn mode(graph: &Graph, id: u64) -> Option<Resource> {
    match graph.nodes.get(&id)?.config.get("fair_share")?.as_str()? {
        "cpu-request-time" => Some(Resource::CpuRequestTime),
        "memory-request-time" => Some(Resource::MemoryRequestTime),
        _ => None,
    }
}

pub fn scopes(graph: &Graph, queue: u64) -> Vec<(u64, u64, Resource)> {
    graph
        .ancestors(queue)
        .windows(2)
        .filter_map(|pair| mode(graph, pair[1]).map(|resource| (pair[1], pair[0], resource)))
        .collect()
}

pub fn validate_job(graph: &Graph, job: &Job) -> Result<(), String> {
    for (scope, _, resource) in scopes(graph, job.queue_id.unwrap()) {
        if resource.rate(job) == 0 {
            return Err(format!(
                "{}: {} ({})",
                message("fair share requires a positive resource request"),
                graph.path(scope),
                resource.name()
            ));
        }
    }
    Ok(())
}

fn scope_key(id: u64, resource: Resource) -> String {
    format!("{id}:{}", resource.name())
}

fn weight(graph: &Graph, id: u64) -> u64 {
    graph.nodes[&id]
        .config
        .get("share_weight")
        .and_then(|v| v.as_u64())
        .unwrap_or(1)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Branch {
    pub service: Counter,
    pub remainder: u64,
    pub weight: u64,
    pub competing: bool,
    pub rate: Counter,
}

impl Branch {
    fn new(weight: u64) -> Self {
        Self {
            service: Counter(0),
            remainder: 0,
            weight,
            competing: false,
            rate: Counter(0),
        }
    }

    fn charge(&mut self, raw: u128) -> Result<(), String> {
        let quotient = raw / self.weight as u128;
        let remainder = (raw % self.weight as u128) as u64 + self.remainder;
        self.service.0 = self
            .service
            .0
            .checked_add(quotient)
            .and_then(|v| v.checked_add((remainder / self.weight) as u128))
            .ok_or_else(overflow)?;
        self.remainder = remainder % self.weight;
        Ok(())
    }

    fn ceiling(&self) -> Result<u128, String> {
        self.service
            .0
            .checked_add(u128::from(self.remainder != 0))
            .ok_or_else(overflow)
    }

    fn compare(&self, other: &Self) -> Ordering {
        self.service
            .cmp(&other.service)
            .then_with(|| (self.remainder * other.weight).cmp(&(other.remainder * self.weight)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub object_id: u64,
    pub resource: Resource,
    pub watermark: Counter,
    pub branches: BTreeMap<u64, Branch>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accounting {
    pub scopes: BTreeMap<String, Scope>,
}

impl Accounting {
    pub fn validate(&self) -> Result<(), String> {
        for (key, scope) in &self.scopes {
            if scope.object_id == 0
                || *key != scope_key(scope.object_id, scope.resource)
                || scope.branches.iter().any(|(id, b)| {
                    *id == 0 || !(1..=10000).contains(&b.weight) || b.remainder >= b.weight
                })
            {
                return Err(message("invalid fair-share accounting"));
            }
        }
        Ok(())
    }

    pub fn active(&self) -> bool {
        self.scopes
            .values()
            .any(|s| s.branches.values().any(|b| b.competing || b.rate.0 != 0))
    }

    pub fn advance(
        &mut self,
        graph: &Graph,
        jobs: &BTreeMap<u64, Job>,
        delta: u64,
    ) -> Result<(), String> {
        for scope in self.scopes.values_mut() {
            for branch in scope.branches.values_mut() {
                branch.charge(
                    branch
                        .rate
                        .0
                        .checked_mul(delta as u128)
                        .ok_or_else(overflow)?,
                )?;
            }
            let minimum = scope
                .branches
                .values()
                .filter(|b| b.competing)
                .map(Branch::ceiling)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .min();
            if let Some(minimum) = minimum {
                scope.watermark.0 = scope.watermark.0.max(minimum);
            }
        }
        let mut participants: BTreeMap<String, BTreeMap<u64, u128>> = BTreeMap::new();
        for job in jobs.values() {
            let Some(queue) = job.queue_id else { continue };
            let participates = job.state.active()
                || (job.state == State::Queued
                    && graph.view(queue).paused_by.is_empty()
                    && super::bounds(graph, queue, job.spec.declared.priority.unwrap_or(0))
                        .is_ok()
                    && validate_job(graph, job).is_ok());
            if !participates {
                continue;
            }
            for (id, child, resource) in scopes(graph, queue) {
                let rate = participants
                    .entry(scope_key(id, resource))
                    .or_default()
                    .entry(child)
                    .or_default();
                if job.state.active() {
                    *rate = rate
                        .checked_add(resource.rate(job) as u128)
                        .ok_or_else(overflow)?;
                }
            }
        }
        for node in graph.nodes.values() {
            if let Some(resource) = mode(graph, node.id) {
                self.scopes
                    .entry(scope_key(node.id, resource))
                    .or_insert_with(|| Scope {
                        object_id: node.id,
                        resource,
                        watermark: Counter(0),
                        branches: BTreeMap::new(),
                    });
            }
        }
        for (key, scope) in &mut self.scopes {
            let current = participants.get(key);
            if mode(graph, scope.object_id) == Some(scope.resource) {
                for node in graph
                    .nodes
                    .values()
                    .filter(|n| n.parent == Some(scope.object_id))
                {
                    scope
                        .branches
                        .entry(node.id)
                        .or_insert_with(|| Branch::new(weight(graph, node.id)));
                }
            }
            for (&id, branch) in &mut scope.branches {
                let rate = current.and_then(|p| p.get(&id));
                let competing = rate.is_some();
                if competing && !branch.competing && branch.service.0 < scope.watermark.0 {
                    branch.service = scope.watermark;
                    branch.remainder = 0;
                }
                if graph.nodes.contains_key(&id) {
                    let next_weight = weight(graph, id);
                    if next_weight != branch.weight {
                        branch.service.0 = branch.ceiling()?;
                        branch.remainder = 0;
                        branch.weight = next_weight;
                    }
                }
                branch.competing = competing;
                branch.rate = Counter(rate.copied().unwrap_or(0));
            }
        }
        Ok(())
    }

    fn ranked_branch(
        &self,
        graph: &Graph,
        scope: u64,
        child: u64,
        provisional: &BTreeMap<(u64, u64), u128>,
    ) -> Result<Branch, String> {
        let resource = mode(graph, scope).unwrap();
        let stored = self.scopes.get(&scope_key(scope, resource));
        let mut branch = stored
            .and_then(|s| s.branches.get(&child))
            .cloned()
            .unwrap_or_else(|| Branch::new(weight(graph, child)));
        if !branch.competing {
            branch.service.0 = branch.service.0.max(stored.map_or(0, |s| s.watermark.0));
        }
        let projected = branch
            .rate
            .0
            .checked_mul(crate::daemon::TICK.as_millis())
            .ok_or_else(overflow)?;
        branch.charge(
            projected
                .checked_add(provisional.get(&(scope, child)).copied().unwrap_or(0))
                .ok_or_else(overflow)?,
        )?;
        Ok(branch)
    }

    fn choose<'a>(
        &self,
        graph: &Graph,
        ledger: &Ledger,
        pending: &[&'a Job],
        node: u64,
        provisional: &BTreeMap<(u64, u64), u128>,
    ) -> Result<Option<&'a Job>, String> {
        let priority = |job: &Job| {
            (
                std::cmp::Reverse(ledger.score(graph, job)),
                job.waiting_since(),
                job.id,
            )
        };
        if graph.nodes[&node].kind == Kind::Queue {
            return Ok(pending
                .iter()
                .copied()
                .filter(|j| j.queue_id == Some(node))
                .min_by_key(|j| priority(j)));
        }
        let fair = mode(graph, node).is_some();
        let mut best: Option<(u64, &'a Job)> = None;
        for child in graph.nodes.values().filter(|n| n.parent == Some(node)) {
            let Some(job) = self.choose(graph, ledger, pending, child.id, provisional)? else {
                continue;
            };
            let replace = match best {
                None => true,
                Some((other_id, other)) => {
                    let order = if fair {
                        self.ranked_branch(graph, node, child.id, provisional)?
                            .compare(&self.ranked_branch(graph, node, other_id, provisional)?)
                    } else {
                        Ordering::Equal
                    };
                    order
                        .then_with(|| priority(job).cmp(&priority(other)))
                        .is_lt()
                }
            };
            if replace {
                best = Some((child.id, job));
            }
        }
        Ok(best.map(|(_, job)| job))
    }

    pub fn order<'a>(
        &self,
        graph: &Graph,
        ledger: &Ledger,
        mut pending: Vec<&'a Job>,
    ) -> Result<Vec<&'a Job>, String> {
        if !pending
            .iter()
            .any(|job| !scopes(graph, job.queue_id.unwrap()).is_empty())
        {
            pending.sort_by_key(|job| {
                (
                    std::cmp::Reverse(ledger.score(graph, job)),
                    job.waiting_since(),
                    job.id,
                )
            });
            return Ok(pending);
        }
        let mut provisional = BTreeMap::<(u64, u64), u128>::new();
        let mut ordered = Vec::new();
        while !pending.is_empty() {
            let job = self
                .choose(graph, ledger, &pending, ROOT, &provisional)?
                .ok_or_else(|| message("invalid fair-share accounting"))?;
            for (scope, child, resource) in scopes(graph, job.queue_id.unwrap()) {
                let charge = (resource.rate(job) as u128)
                    .checked_mul(crate::daemon::TICK.as_millis())
                    .ok_or_else(overflow)?;
                let value = provisional.entry((scope, child)).or_default();
                *value = value.checked_add(charge).ok_or_else(overflow)?;
            }
            pending.retain(|j| j.id != job.id);
            ordered.push(job);
        }
        Ok(ordered)
    }

    pub fn explain(&self, graph: &Graph, queue: u64) -> Vec<Explanation> {
        scopes(graph, queue)
            .into_iter()
            .map(|(scope, child, resource)| {
                let stored = self.scopes.get(&scope_key(scope, resource));
                let branch = stored.and_then(|s| s.branches.get(&child));
                Explanation {
                    scope_id: scope,
                    scope_path: graph.path(scope),
                    child_id: child,
                    child_path: graph.path(child),
                    resource,
                    weight: branch.map_or_else(|| weight(graph, child), |b| b.weight),
                    service: branch.map_or(Counter(0), |b| b.service),
                    remainder: branch.map_or(0, |b| b.remainder),
                    watermark: stored.map_or(Counter(0), |s| s.watermark),
                    active_rate: branch.map_or(Counter(0), |b| b.rate),
                    lookahead_ms: crate::daemon::TICK.as_millis() as u64,
                }
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explanation {
    pub scope_id: u64,
    pub scope_path: String,
    pub child_id: u64,
    pub child_path: String,
    pub resource: Resource,
    pub weight: u64,
    pub service: Counter,
    pub remainder: u64,
    pub watermark: Counter,
    pub active_rate: Counter,
    pub lookahead_ms: u64,
}

pub fn scope_changes(before: &Graph, after: &Graph) -> BTreeSet<u64> {
    before
        .nodes
        .keys()
        .chain(after.nodes.keys())
        .copied()
        .filter(|&id| mode(before, id) != mode(after, id))
        .collect()
}

#[cfg(test)]
mod tests;
