use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Reading, Resource, message};
use crate::model::{Job, State as JobState};
use crate::objects::Graph;
use crate::store::{Store, write_json};

pub const SAMPLE_MS: u64 = 1000;
const EVENT_LIMIT: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Metric {
    Some,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Window {
    Avg10,
    Avg60,
    Avg300,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub resource: Resource,
    pub metric: Metric,
    pub window: Window,
    pub high_bp: u16,
    pub low_bp: u16,
    pub sustain_ms: u64,
    pub minimum_hold_ms: u64,
    pub recovery_ms: u64,
    pub step_ms: u64,
    pub required: bool,
}

pub fn validate(rules: &[Rule], host: bool) -> Result<(), String> {
    let mut names = BTreeSet::new();
    if rules.len() > 32 {
        return Err(message("invalid pressure rules"));
    }
    for rule in rules {
        if rule.id.is_empty()
            || rule.id.len() > 64
            || !rule
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || !names.insert(&rule.id)
            || rule.high_bp > 10000
            || rule.low_bp >= rule.high_bp
            || [
                rule.sustain_ms,
                rule.minimum_hold_ms,
                rule.recovery_ms,
                rule.step_ms,
            ]
            .iter()
            .any(|n| *n < SAMPLE_MS || *n > u64::MAX / 2)
            || (host && rule.resource == Resource::Cpu && rule.metric == Metric::Full)
        {
            return Err(message("invalid pressure rules"));
        }
    }
    Ok(())
}

pub fn local(graph: &Graph, id: u64) -> Vec<Rule> {
    graph.nodes[&id]
        .config
        .get("pressure")
        .map(|v| serde_json::from_value(v.clone()).expect("validated pressure rules"))
        .unwrap_or_default()
}

pub fn validate_local(value: &serde_json::Value) -> Result<(), String> {
    let rules: Vec<Rule> = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    validate(&rules, false)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "availability", rename_all = "snake_case", deny_unknown_fields)]
pub enum Signal {
    Available {
        value_bp: u16,
        job_id: Option<u64>,
        attempt: Option<u64>,
        missing: u64,
    },
    Empty,
    Unavailable {
        reason: String,
    },
}

impl Signal {
    fn high(&self, rule: &Rule) -> bool {
        matches!(self, Self::Available { value_bp, .. } if *value_bp >= rule.high_bp)
    }

    fn low(&self, rule: &Rule) -> bool {
        matches!(self, Self::Empty)
            || matches!(self, Self::Available { value_bp, missing: 0, .. } if *value_bp <= rule.low_bp)
    }

    fn missing(&self) -> bool {
        matches!(self, Self::Unavailable { .. })
            || matches!(self, Self::Available { missing, .. } if *missing > 0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Open,
    Holding,
    Recovering,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    pub scope_id: Option<u64>,
    pub scope_path: String,
    pub aggregation: String,
    pub rule: Rule,
    pub phase: Phase,
    pub temporary_max_running: Option<u64>,
    pub signal: Signal,
    pub sampled_boot_ms: u64,
    pub reconsider_boot_ms: u64,
    pub high_since_ms: Option<u64>,
    pub low_since_ms: Option<u64>,
    pub hold_since_ms: Option<u64>,
    pub step_since_ms: Option<u64>,
}

impl View {
    pub(super) fn new(scope: Option<u64>, rule: Rule, now: u64) -> Self {
        Self {
            scope_id: scope,
            scope_path: String::new(),
            aggregation: if scope.is_some() {
                "maximum_descendant_average"
            } else {
                "host"
            }
            .to_owned(),
            rule,
            phase: Phase::Open,
            temporary_max_running: None,
            signal: Signal::Unavailable {
                reason: message("not sampled"),
            },
            sampled_boot_ms: now,
            reconsider_boot_ms: now,
            high_since_ms: None,
            low_since_ms: None,
            hold_since_ms: None,
            step_since_ms: None,
        }
    }

    pub(super) fn restart(&mut self, now: u64, reboot: bool) {
        self.high_since_ms = None;
        self.low_since_ms = None;
        if reboot && self.phase != Phase::Open {
            self.phase = Phase::Holding;
            self.temporary_max_running = Some(0);
            self.hold_since_ms = Some(now);
            self.step_since_ms = None;
        }
    }

    pub(super) fn advance(&mut self, signal: Signal, now: u64, active: u64, demand: u64) {
        if now.saturating_sub(self.sampled_boot_ms) > SAMPLE_MS * 2 {
            self.high_since_ms = None;
            self.low_since_ms = None;
        }
        self.signal = signal;
        self.sampled_boot_ms = now;
        self.reconsider_boot_ms = now.saturating_add(SAMPLE_MS);
        if self.signal.high(&self.rule) {
            self.high_since_ms.get_or_insert(now);
        } else {
            self.high_since_ms = None;
        }
        if self.signal.low(&self.rule) {
            self.low_since_ms.get_or_insert(now);
        } else {
            self.low_since_ms = None;
        }
        let high = self
            .high_since_ms
            .is_some_and(|since| now.saturating_sub(since) >= self.rule.sustain_ms);
        if high || (self.rule.required && self.signal.missing()) {
            if self.phase != Phase::Holding {
                self.hold_since_ms = Some(now);
            }
            self.phase = Phase::Holding;
            self.temporary_max_running = Some(0);
            self.step_since_ms = None;
            return;
        }
        let recovered = self
            .low_since_ms
            .is_some_and(|since| now.saturating_sub(since) >= self.rule.recovery_ms);
        match self.phase {
            Phase::Holding
                if recovered
                    && self.hold_since_ms.is_some_and(|since| {
                        now.saturating_sub(since) >= self.rule.minimum_hold_ms
                    }) =>
            {
                self.phase = Phase::Recovering;
                self.temporary_max_running = Some(active.saturating_add(1));
                self.step_since_ms = Some(now);
            }
            Phase::Recovering
                if recovered
                    && self
                        .step_since_ms
                        .is_some_and(|since| now.saturating_sub(since) >= self.rule.step_ms) =>
            {
                let target = self.temporary_max_running.unwrap_or(0);
                if target >= demand {
                    self.phase = Phase::Open;
                    self.temporary_max_running = None;
                    self.hold_since_ms = None;
                    self.step_since_ms = None;
                } else {
                    self.temporary_max_running = Some(target.saturating_add(1));
                    self.step_since_ms = Some(now);
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub sequence: u64,
    pub at_ms: u64,
    pub boot_id: String,
    pub previous_phase: Option<Phase>,
    pub previous_target: Option<u64>,
    pub removed: bool,
    pub view: View,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ledger {
    pub schema_version: u32,
    pub boot_id: String,
    pub sequence: u64,
    pub views: BTreeMap<String, View>,
    pub events: VecDeque<Event>,
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            schema_version: 1,
            boot_id: String::new(),
            sequence: 0,
            views: BTreeMap::new(),
            events: VecDeque::new(),
        }
    }
}

impl Ledger {
    pub fn load(store: &Store) -> io::Result<Self> {
        match std::fs::read(store.root.join("pressure.json")) {
            Ok(bytes) => {
                let ledger: Self = serde_json::from_slice(&bytes)?;
                if ledger.schema_version != 1 || ledger.events.len() > EVENT_LIMIT {
                    return Err(io::Error::other(message("invalid pressure ledger")));
                }
                for (key, view) in &ledger.views {
                    validate(std::slice::from_ref(&view.rule), view.scope_id.is_none())
                        .map_err(io::Error::other)?;
                    if *key != scope_key(view.scope_id, &view.rule.id)
                        || (view.phase == Phase::Open) != view.temporary_max_running.is_none()
                        || (view.phase == Phase::Holding && view.temporary_max_running != Some(0))
                        || (view.phase != Phase::Open && view.hold_since_ms.is_none())
                        || (view.phase == Phase::Recovering
                            && (view.step_since_ms.is_none()
                                || view.temporary_max_running == Some(0)))
                        || [
                            view.high_since_ms,
                            view.low_since_ms,
                            view.hold_since_ms,
                            view.step_since_ms,
                        ]
                        .into_iter()
                        .flatten()
                        .any(|t| t > view.sampled_boot_ms)
                    {
                        return Err(io::Error::other(message("invalid pressure ledger")));
                    }
                }
                let mut prior = 0;
                for event in &ledger.events {
                    if event.sequence <= prior || event.sequence > ledger.sequence {
                        return Err(io::Error::other(message("invalid pressure ledger")));
                    }
                    prior = event.sequence;
                }
                Ok(ledger)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    fn event(
        &mut self,
        previous: Option<&View>,
        view: View,
        removed: bool,
        wall: u64,
    ) -> io::Result<()> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other(message("pressure event counter exhausted")))?;
        self.events.push_back(Event {
            sequence: self.sequence,
            at_ms: wall,
            boot_id: self.boot_id.clone(),
            previous_phase: previous.map(|v| v.phase),
            previous_target: previous.and_then(|v| v.temporary_max_running),
            removed,
            view,
        });
        while self.events.len() > EVENT_LIMIT {
            self.events.pop_front();
        }
        Ok(())
    }
}

fn scope_key(scope: Option<u64>, rule: &str) -> String {
    format!(
        "{}:{rule}",
        scope.map_or_else(|| "host".to_owned(), |v| v.to_string())
    )
}

fn affected(graph: &Graph, job: &Job, scope: Option<u64>) -> bool {
    scope.is_none_or(|scope| job.queue_id.is_some_and(|q| graph.within(q, scope)))
}

fn value(reading: &Reading, window: Window) -> Option<u16> {
    match reading {
        Reading::Available {
            avg10_bp,
            avg60_bp,
            avg300_bp,
            ..
        } => Some(match window {
            Window::Avg10 => *avg10_bp,
            Window::Avg60 => *avg60_bp,
            Window::Avg300 => *avg300_bp,
        }),
        _ => None,
    }
}

fn observed(snapshot: &super::Snapshot, rule: &Rule) -> Option<u16> {
    let observation = snapshot
        .observations
        .iter()
        .find(|o| o.resource == rule.resource)?;
    value(
        match rule.metric {
            Metric::Some => &observation.some,
            Metric::Full => &observation.full,
        },
        rule.window,
    )
}

#[derive(Default)]
pub struct Controller {
    pub ledger: Ledger,
    pub error: Option<String>,
    last_sample: Option<u64>,
    pub monitors: super::notify::Registry,
    notification_pending: bool,
}

impl Controller {
    pub fn notifications(&mut self, notices: Vec<super::notify::Notice>) -> bool {
        if notices.is_empty() {
            return false;
        }
        let now = crate::admission::clock_ms().unwrap_or(self.last_sample.unwrap_or(0));
        self.notification_pending |= self.monitors.accept(notices, now);
        self.notification_pending
            && self
                .last_sample
                .is_none_or(|last| now.saturating_sub(last) >= 250)
    }
    pub fn load(store: &Store) -> io::Result<Self> {
        Ok(Self {
            ledger: Ledger::load(store)?,
            ..Self::default()
        })
    }

    pub fn invalidate(&mut self) {
        self.last_sample = None;
    }

    pub fn views(&self, graph: &Graph, job: &Job) -> Vec<View> {
        self.ledger
            .views
            .values()
            .filter(|v| affected(graph, job, v.scope_id))
            .cloned()
            .collect()
    }

    pub fn gate(&self, graph: &Graph, jobs: &BTreeMap<u64, Job>, job: &Job) -> Result<(), String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        for view in self
            .ledger
            .views
            .values()
            .filter(|v| affected(graph, job, v.scope_id))
        {
            if view.scope_id.is_some() && view.rule.required && job.spec.declared.on.is_some() {
                return Err(format!(
                    "{}: {} / {}",
                    message("remote workload PSI is unavailable locally"),
                    view.scope_path,
                    view.rule.id
                ));
            }
            if let Some(target) = view.temporary_max_running {
                let active = jobs
                    .values()
                    .filter(|j| j.state.active() && affected(graph, j, view.scope_id))
                    .count() as u64;
                if active >= target {
                    return Err(format!(
                        "{}: {} / {}; {:?}; {} {:?} {:?}; {:?}; high_bp={} low_bp={}; {}={target}; {}={}",
                        message("pressure admission hold"),
                        view.scope_path,
                        view.rule.id,
                        view.phase,
                        view.rule.resource.name(),
                        view.rule.metric,
                        view.rule.window,
                        view.signal,
                        view.rule.high_bp,
                        view.rule.low_bp,
                        message("temporary max-running"),
                        message("reconsider boot milliseconds"),
                        view.reconsider_boot_ms
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn checkpoint(
        &mut self,
        store: &Store,
        graph: &Graph,
        jobs: &BTreeMap<u64, Job>,
        host_rules: &[Rule],
        cgroup: Option<&Path>,
        boot: &str,
    ) {
        let result = self.sample(store, graph, jobs, host_rules, cgroup, boot);
        self.error = result
            .err()
            .map(|e| format!("{}: {e}", message("pressure accounting unavailable")));
    }

    fn sample(
        &mut self,
        store: &Store,
        graph: &Graph,
        jobs: &BTreeMap<u64, Job>,
        host_rules: &[Rule],
        cgroup: Option<&Path>,
        boot: &str,
    ) -> io::Result<()> {
        let mut configured: Vec<_> = host_rules.iter().cloned().map(|r| (None, r)).collect();
        for id in graph.nodes.keys() {
            configured.extend(local(graph, *id).into_iter().map(|r| (Some(*id), r)));
        }
        if configured.is_empty() && self.ledger.views.is_empty() {
            return Ok(());
        }
        let now = crate::admission::clock_ms()?;
        let unchanged = configured.len() == self.ledger.views.len()
            && configured.iter().all(|(scope, rule)| {
                self.ledger
                    .views
                    .get(&scope_key(*scope, &rule.id))
                    .is_some_and(|v| v.rule == *rule)
            });
        if unchanged
            && self.error.is_none()
            && self.last_sample.is_some_and(|last| {
                now.saturating_sub(last)
                    < if self.notification_pending {
                        250
                    } else {
                        SAMPLE_MS
                    }
            })
        {
            return Ok(());
        }
        let local_needed = configured.iter().any(|(scope, _)| scope.is_some());
        let host = (!host_rules.is_empty()).then(|| super::snapshot(None));
        let snapshots: BTreeMap<_, _> = if local_needed {
            jobs.values()
                .filter(|j| j.state.active())
                .map(|j| (j.id, super::snapshot(Some(j))))
                .collect()
        } else {
            BTreeMap::new()
        };
        let mut monitors: BTreeMap<super::notify::Key, BTreeSet<String>> = BTreeMap::new();
        for (scope, rule) in &configured {
            let sources: Vec<_> = if let Some(scope) = scope {
                snapshots
                    .values()
                    .filter(|s| {
                        s.job_id
                            .and_then(|id| jobs.get(&id))
                            .is_some_and(|job| affected(graph, job, Some(*scope)))
                    })
                    .collect()
            } else {
                host.iter().collect()
            };
            for snapshot in sources {
                if observed(snapshot, rule).is_none() {
                    continue;
                }
                if let Some(path) = snapshot
                    .observations
                    .iter()
                    .find(|o| o.resource == rule.resource)
                    .and_then(|o| o.path.clone())
                {
                    monitors
                        .entry(super::notify::Key {
                            path,
                            cgroup_identity: snapshot
                                .cgroup
                                .as_ref()
                                .map(|id| (id.device, id.inode)),
                            job_attempt: snapshot.job_id.zip(snapshot.attempt),
                            resource: rule.resource,
                            metric: rule.metric,
                            high_bp: rule.high_bp,
                        })
                        .or_default()
                        .insert(scope_key(*scope, &rule.id));
                }
            }
        }
        self.monitors.sync(monitors, now);
        let mut next = self.ledger.clone();
        let changed_boot = next.boot_id != boot;
        next.boot_id = boot.to_owned();
        let wall = crate::shim::now_ms();
        let keep: BTreeSet<_> = configured
            .iter()
            .map(|(s, r)| scope_key(*s, &r.id))
            .collect();
        for key in self.ledger.views.keys().filter(|k| !keep.contains(*k)) {
            let old = next.views.remove(key).unwrap();
            next.event(Some(&old), old.clone(), true, wall)?;
        }
        for (scope, rule) in configured {
            let key = scope_key(scope, &rule.id);
            let previous = next.views.get(&key).cloned();
            let mut view = previous
                .as_ref()
                .filter(|v| v.rule == rule)
                .cloned()
                .unwrap_or_else(|| View::new(scope, rule.clone(), now));
            view.scope_path = scope.map_or_else(|| "host".to_owned(), |id| graph.path(id));
            if changed_boot || self.last_sample.is_none() {
                view.restart(now, changed_boot);
            }
            let members: Vec<_> = jobs
                .values()
                .filter(|j| affected(graph, j, scope))
                .collect();
            let active: Vec<_> = members
                .iter()
                .copied()
                .filter(|j| j.state.active())
                .collect();
            let demand = members
                .iter()
                .filter(|j| j.state.active() || j.state == JobState::Queued)
                .count() as u64;
            let signal = if scope.is_none() {
                observed(host.as_ref().unwrap(), &rule).map_or_else(
                    || Signal::Unavailable {
                        reason: message("host pressure measurement unavailable"),
                    },
                    |value_bp| Signal::Available {
                        value_bp,
                        job_id: None,
                        attempt: None,
                        missing: 0,
                    },
                )
            } else if active.is_empty() {
                let probe = cgroup.and_then(|path| {
                    super::parse_file(
                        &path.join(format!("{}.pressure", rule.resource.name())),
                        rule.resource,
                        false,
                    )
                    .ok()
                });
                if probe
                    .as_ref()
                    .and_then(|o| {
                        value(
                            match rule.metric {
                                Metric::Some => &o.some,
                                Metric::Full => &o.full,
                            },
                            rule.window,
                        )
                    })
                    .is_some()
                {
                    Signal::Empty
                } else {
                    Signal::Unavailable {
                        reason: message("workload cgroup PSI is unavailable"),
                    }
                }
            } else {
                let mut maximum = None;
                let mut missing = 0;
                for job in &active {
                    match snapshots.get(&job.id).and_then(|s| observed(s, &rule)) {
                        Some(n) if maximum.is_none_or(|(prior, _, _)| n > prior) => {
                            maximum = Some((n, job.id, job.attempt))
                        }
                        Some(_) => {}
                        None => missing += 1,
                    }
                }
                maximum.map_or_else(
                    || Signal::Unavailable {
                        reason: message("workload cgroup PSI is unavailable"),
                    },
                    |(value_bp, id, attempt)| Signal::Available {
                        value_bp,
                        job_id: Some(id),
                        attempt: Some(attempt),
                        missing,
                    },
                )
            };
            view.advance(signal, now, active.len() as u64, demand);
            if previous.as_ref().is_none_or(|old| {
                old.phase != view.phase
                    || old.temporary_max_running != view.temporary_max_running
                    || old.rule != view.rule
                    || old.signal.missing() != view.signal.missing()
            }) {
                next.event(previous.as_ref(), view.clone(), false, wall)?;
            }
            next.views.insert(key, view);
        }
        write_json(&store.root.join("pressure.json"), &next)?;
        self.ledger = next;
        crate::operations::journal::observe_pressure(&self.ledger, graph);
        self.last_sample = Some(now);
        self.notification_pending = false;
        Ok(())
    }
}
