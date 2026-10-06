use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::daemon::Shared;
use crate::daemon::client_service::{self, Seen};
use crate::model::State;
use crate::objects::ROOT;

use super::{measure, spell};

#[derive(Default, Clone)]
struct Below {
    held: u64,
    queued: u64,
    starting: u64,
    running: u64,
    suspended: u64,
    stopping: u64,
    oldest: Option<u64>,
    cores_milli: u64,
    memory: u64,
}

impl Below {
    fn active(&self) -> u64 {
        self.starting + self.running + self.suspended + self.stopping
    }
}

fn counted(seen: &Seen<'_>) -> HashMap<u64, Below> {
    let mut below: HashMap<u64, Below> = HashMap::new();
    let mut chains: HashMap<u64, Vec<u64>> = HashMap::new();
    for job in seen.jobs.values() {
        let Some(queue) = job.queue_id else {
            continue;
        };
        let chain = chains
            .entry(queue)
            .or_insert_with(|| seen.objects.ancestors(queue));
        for object in chain.iter() {
            let entry = below.entry(*object).or_default();
            match job.state {
                State::Held => entry.held += 1,
                State::Queued => {
                    entry.queued += 1;
                    let since = job.waiting_since();
                    entry.oldest = Some(entry.oldest.map_or(since, |oldest| oldest.min(since)));
                }
                State::Starting => entry.starting += 1,
                State::Running => entry.running += 1,
                State::Suspended => entry.suspended += 1,
                State::Stopping => entry.stopping += 1,
                _ => {}
            }
            if job.state.active() {
                entry.cores_milli = entry
                    .cores_milli
                    .saturating_add(job.reservation.vector.cores_milli);
                entry.memory = entry.memory.saturating_add(job.reservation.vector.memory);
            }
        }
    }
    below
}

pub fn ordered(objects: &crate::objects::Graph) -> Vec<u64> {
    let mut children: BTreeMap<u64, Vec<(&str, u64)>> = BTreeMap::new();
    for node in objects.nodes.values() {
        if let Some(parent) = node.parent {
            children
                .entry(parent)
                .or_default()
                .push((node.name.as_str(), node.id));
        }
    }
    let mut order = Vec::with_capacity(objects.nodes.len());
    let mut waiting = std::collections::VecDeque::from([ROOT]);
    while let Some(id) = waiting.pop_front() {
        if !objects.nodes.contains_key(&id) || order.contains(&id) {
            continue;
        }
        order.push(id);
        if let Some(found) = children.get_mut(&id) {
            found.sort_unstable();
            waiting.extend(found.iter().map(|(_, child)| *child));
        }
    }
    order
}

pub fn node(objects: &crate::objects::Graph, id: u64) -> Value {
    let view = objects.view(id);
    let bytes = crate::netsecret::rendered(&view.object).unwrap_or_default();
    let object: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let effective: Map<String, Value> = view
        .effective
        .iter()
        .map(|(name, effective)| {
            (
                name.clone(),
                json!({
                    "value": effective.value,
                    "source": spell::placed(effective.source_id, &effective.source_path),
                }),
            )
        })
        .collect();
    let shown = |paths: &[String]| -> Vec<String> {
        paths
            .iter()
            .map(|path| spell::path(path).to_owned())
            .collect()
    };
    json!({
        "object": object,
        "path": spell::path(&view.path),
        "effective": effective,
        "aggregate_domains": view
            .aggregate_domains
            .iter()
            .map(|domain| json!({
                "object_id": domain.object_id,
                "object_path": spell::path(&domain.object_path),
                "limits": domain.limits,
            }))
            .collect::<Vec<_>>(),
        "paused_by": shown(&view.paused_by),
        "closed_by": shown(&view.closed_by),
    })
}

fn measured(seen: &Seen<'_>, id: u64, below: &HashMap<u64, Below>, now: u64) -> (Value, Value) {
    let nothing = Below::default();
    let own = below.get(&id).unwrap_or(&nothing);
    let waits = crate::operations::depth::measure(seen.objects, &BTreeMap::new(), id, now);
    let depth = json!({
        "held": own.held,
        "queued": own.queued,
        "starting": own.starting,
        "running": own.running,
        "suspended": own.suspended,
        "stopping": own.stopping,
        "oldest_queued_age_ms": own.oldest.map(|since| now.saturating_sub(since)),
        "oldest_queued_since_ms": own.oldest,
        "started_last_hour": waits.started_last_hour,
    });
    let mut limits = Vec::new();
    let mut budgets = Vec::new();
    for ancestor in seen.objects.ancestors(id) {
        let config = &seen.objects.nodes[&ancestor].config;
        let counted = below.get(&ancestor).unwrap_or(&nothing);
        let set_by = json!({
            "id": ancestor,
            "path": spell::path(&seen.objects.path(ancestor)),
        });
        if let Some(limit) = config.get("max_running").and_then(Value::as_u64) {
            limits.push(json!({"limit": limit, "count": counted.active(), "set_by": set_by}));
        }
        for (resource, reserved) in [
            ("cores_milli", counted.cores_milli),
            ("memory", counted.memory),
        ] {
            if let Some(limit) = config.get(resource).and_then(Value::as_u64) {
                budgets.push(json!({
                    "resource": resource,
                    "limit": limit,
                    "reserved": reserved,
                    "set_by": set_by,
                }));
            }
        }
    }
    let chain = crate::aggregate::chain(seen.objects, id);
    let domains: Vec<Value> = match seen.tree {
        Some(tree) => chain
            .iter()
            .zip(crate::aggregate::paths(tree, &chain))
            .map(|(domain, path)| {
                json!({
                    "object_id": domain.object_id,
                    "object_path": spell::path(&domain.object_path),
                    "cgroup": path.display().to_string(),
                    "limits": domain.limits,
                })
            })
            .collect(),
        None => Vec::new(),
    };
    let usage = json!({
        "running": {"count": own.active(), "limits": limits},
        "budgets": budgets,
        "domains": domains,
    });
    (depth, usage)
}

fn read(path: &Path, limits: &Value) -> (Value, Value) {
    let held: Map<String, Value> = limits
        .as_object()
        .map(|limits| {
            limits
                .keys()
                .map(|name| (name.clone(), measure::held(path, name)))
                .collect()
        })
        .unwrap_or_default();
    let at = crate::shim::now_ms();
    let mut counters = measure::Counters::read(path).written();
    counters.insert("at_ms".to_owned(), Value::from(at));
    (Value::Object(held), Value::Object(counters))
}

pub fn drawn(seen: &Seen<'_>, now: u64) -> Vec<Value> {
    let below = counted(seen);
    ordered(seen.objects)
        .into_iter()
        .map(|id| {
            let mut node = node(seen.objects, id);
            let (depth, usage) = measured(seen, id, &below, now);
            node["depth"] = depth;
            node["use"] = usage;
            node
        })
        .collect()
}

pub fn filled(nodes: &mut [Value]) {
    let mut known: HashMap<PathBuf, (Value, Value)> = HashMap::new();
    for node in nodes {
        let Some(domains) = node["use"]["domains"].as_array_mut() else {
            continue;
        };
        for domain in domains {
            let Some(path) = domain["cgroup"].as_str().map(PathBuf::from) else {
                continue;
            };
            let (limits, counters) = known
                .entry(path.clone())
                .or_insert_with(|| read(&path, &domain["limits"]))
                .clone();
            domain["limits"] = limits;
            domain["counters"] = counters;
        }
    }
}

pub fn mark(node: &Value) -> String {
    let mut depth = node["depth"].clone();
    if let Some(depth) = depth.as_object_mut() {
        depth.remove("oldest_queued_age_ms");
    }
    let mut usage = node["use"].clone();
    for domain in usage["domains"].as_array_mut().into_iter().flatten() {
        if let Some(counters) = domain["counters"].as_object_mut() {
            counters.remove("at_ms");
        }
    }
    format!("{depth}{usage}")
}

pub fn nodes(shared: &Shared) -> (Vec<Value>, u64, u64) {
    let now = crate::shim::now_ms();
    let (mut nodes, seq) =
        client_service::seen(shared, |seen| (drawn(seen, now), super::changes::last()));
    filled(&mut nodes);
    (nodes, now, seq)
}

pub fn answer(shared: &Shared) -> Result<String, String> {
    let (nodes, now, seq) = nodes(shared);
    Ok(json!({
        "seq": seq,
        "now_ms": now,
        "nodes": nodes,
    })
    .to_string())
}
