use std::collections::BTreeMap;
use std::io;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Job, State};
use crate::objects::{Effective, Graph};
use crate::resource_policy::Origin;
use crate::store::Store;

pub mod fair;
mod messages;
pub use messages::{help, message};
pub mod planner;
#[cfg(test)]
mod tests;

pub const MIN_PRIORITY: i32 = -1000;
pub const MAX_PRIORITY: i32 = 1000;

pub fn key(key: &str) -> bool {
    [
        "priority",
        "priority_min",
        "priority_max",
        "aging_ms",
        "strict_fifo",
        "backfill",
        "fair_share",
        "share_weight",
    ]
    .contains(&key)
}

pub fn priority(text: &str) -> Result<i32, String> {
    text.parse::<i32>()
        .ok()
        .filter(|v| (MIN_PRIORITY..=MAX_PRIORITY).contains(v))
        .ok_or_else(|| message("priority must be an integer from -1000 through 1000"))
}

pub fn option_key(option: &str) -> Option<&'static str> {
    match option {
        "priority" => Some("priority"),
        "priority-min" => Some("priority_min"),
        "priority-max" => Some("priority_max"),
        "aging" => Some("aging_ms"),
        "strict-fifo" => Some("strict_fifo"),
        "backfill" => Some("backfill"),
        "fair-share" => Some("fair_share"),
        "share-weight" => Some("share_weight"),
        _ => None,
    }
}

fn aging_duration(text: &str) -> Result<u64, String> {
    let invalid =
        || message("aging requires a positive whole-millisecond duration within u64 range");
    let split = text
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let scale: u128 = match unit {
        "ms" => 1,
        "" | "s" => 1000,
        "m" | "min" => 60_000,
        "h" => 3_600_000,
        _ => return Err(invalid()),
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty()
        || fraction.len() > 18
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || (number.contains('.') && fraction.is_empty())
    {
        return Err(invalid());
    }
    let denominator = 10u128.pow(fraction.len() as u32);
    let whole: u128 = whole.parse().map_err(|_| invalid())?;
    let fraction: u128 = if fraction.is_empty() {
        0
    } else {
        fraction.parse().map_err(|_| invalid())?
    };
    let fractional_ms = fraction.checked_mul(scale).ok_or_else(invalid)?;
    if !fractional_ms.is_multiple_of(denominator) {
        return Err(invalid());
    }
    let milliseconds = whole
        .checked_mul(scale)
        .and_then(|v| v.checked_add(fractional_ms / denominator))
        .and_then(|v| u64::try_from(v).ok())
        .filter(|v| *v > 0)
        .ok_or_else(invalid)?;
    Ok(milliseconds)
}

pub fn parse(option: &str, text: &str) -> Option<Result<(String, Value), String>> {
    let key = option_key(option)?;
    Some((|| {
        let value = match key {
            "share_weight" => Value::from(
                text.parse::<u64>()
                    .ok()
                    .filter(|v| (1..=10000).contains(v))
                    .ok_or_else(|| {
                        message("share-weight must be an integer from 1 through 10000")
                    })?,
            ),
            "fair_share" => {
                if !["off", "cpu-request-time", "memory-request-time"].contains(&text) {
                    return Err(message(
                        "fair-share requires off, cpu-request-time or memory-request-time",
                    ));
                }
                Value::from(text)
            }
            "priority" | "priority_min" | "priority_max" => Value::from(priority(text)?),
            "aging_ms" => Value::from(aging_duration(text)?),
            "strict_fifo" => Value::from(
                text.parse::<bool>()
                    .map_err(|_| message("strict-fifo requires true or false"))?,
            ),
            "backfill" if ["off", "conservative"].contains(&text) => Value::from(text),
            _ => return Err(message("backfill requires off or conservative")),
        };
        Ok((key.to_owned(), value))
    })())
}

pub fn validate_config(config: &BTreeMap<String, Value>) -> Result<(), String> {
    for (key, value) in config.iter().filter(|(key, _)| self::key(key)) {
        match key.as_str() {
            "share_weight" if value.as_u64().is_some_and(|n| (1..=10000).contains(&n)) => {}
            "fair_share"
                if value == "off"
                    || value == "cpu-request-time"
                    || value == "memory-request-time" => {}
            "priority" | "priority_min" | "priority_max"
                if value
                    .as_i64()
                    .is_some_and(|n| (MIN_PRIORITY as i64..=MAX_PRIORITY as i64).contains(&n)) => {}
            "aging_ms" if value.as_u64().is_some_and(|n| n > 0) => {}
            "strict_fifo" if value.is_boolean() => {}
            "backfill" if value == "off" || value == "conservative" => {}
            _ => return Err(format!("{}: {key}", message("invalid scheduling ledger"))),
        }
    }
    Ok(())
}

pub fn resolve(
    priority: &mut Option<i32>,
    effective: &BTreeMap<String, Effective>,
) -> Result<Option<Origin>, String> {
    if let Some(value) = priority {
        self::priority(&value.to_string())?;
        return Ok(Some(Origin::Job));
    }
    if let Some(value) = effective.get("priority") {
        *priority = Some(value.value.as_i64().unwrap() as i32);
        return Ok(Some(Origin::Object {
            id: value.source_id,
            path: value.source_path.clone(),
        }));
    }
    Ok(None)
}

pub fn bounds(graph: &Graph, queue: u64, priority: i32) -> Result<(), String> {
    self::priority(&priority.to_string())?;
    for id in graph.ancestors(queue) {
        let node = &graph.nodes[&id];
        if node
            .config
            .get("priority_min")
            .and_then(Value::as_i64)
            .is_some_and(|min| (priority as i64) < min)
            || node
                .config
                .get("priority_max")
                .and_then(Value::as_i64)
                .is_some_and(|max| (priority as i64) > max)
        {
            return Err(format!(
                "{}: {}",
                message("priority is outside ancestor bounds"),
                graph.path(id)
            ));
        }
    }
    Ok(())
}

pub fn validate_bounds(graph: &Graph) -> Result<(), String> {
    for node in graph.nodes.values() {
        if node.kind != crate::objects::Kind::Group && node.config.contains_key("fair_share") {
            return Err(message("fair-share scopes require a Group"));
        }
        let (mut min, mut max) = (MIN_PRIORITY as i64, MAX_PRIORITY as i64);
        for id in graph.ancestors(node.id) {
            let config = &graph.nodes[&id].config;
            if let Some(n) = config.get("priority_min").and_then(Value::as_i64) {
                min = min.max(n);
            }
            if let Some(n) = config.get("priority_max").and_then(Value::as_i64) {
                max = max.min(n);
            }
        }
        if min > max {
            return Err(format!(
                "{}: {}",
                message("ancestor priority bounds do not overlap"),
                graph.path(node.id)
            ));
        }
    }
    Ok(())
}

pub fn enabled(graph: &Graph, jobs: &BTreeMap<u64, Job>) -> bool {
    jobs.values()
        .any(|job| job.state == State::Queued && ordered(graph, job))
}

pub fn ordered(graph: &Graph, job: &Job) -> bool {
    job.spec.declared.priority.is_some()
        || job.queue_id.is_some_and(|queue| {
            graph.ancestors(queue).iter().any(|id| {
                let config = &graph.nodes[id].config;
                config.contains_key("aging_ms")
                    || fair::mode(graph, *id).is_some()
                    || config.contains_key("backfill")
                    || config.get("strict_fifo") == Some(&Value::Bool(true))
            })
        })
}

pub fn aging(graph: &Graph, queue: u64) -> Option<Effective> {
    graph.ancestors(queue).into_iter().find_map(|id| {
        graph.nodes[&id]
            .config
            .get("aging_ms")
            .map(|value| Effective {
                value: value.clone(),
                source_id: id,
                source_path: graph.path(id),
            })
    })
}

pub fn backfill(graph: &Graph, queue: u64, legacy: bool) -> bool {
    graph
        .ancestors(queue)
        .into_iter()
        .find_map(|id| graph.nodes[&id].config.get("backfill"))
        .map_or(legacy, |value| value == "conservative")
}

pub fn fifo_predecessor(
    graph: &Graph,
    jobs: &BTreeMap<u64, Job>,
    job: &Job,
) -> Option<(u64, String)> {
    for id in graph.ancestors(job.queue_id?) {
        if graph.nodes[&id].config.get("strict_fifo") != Some(&Value::Bool(true)) {
            continue;
        }
        if let Some(earlier) = jobs
            .values()
            .filter(|other| {
                other.state == State::Queued
                    && other.queue_id.is_some_and(|q| graph.within(q, id))
                    && (other.waiting_since(), other.id) < (job.waiting_since(), job.id)
            })
            .min_by_key(|other| (other.waiting_since(), other.id))
        {
            return Some((earlier.id, graph.path(id)));
        }
    }
    None
}

fn identity(job: &Job) -> String {
    format!("{}:{}", job.id, job.attempt)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credit {
    pub milliseconds: u64,
    pub eligible: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ledger {
    pub schema_version: u32,
    pub boot_id: String,
    pub checkpoint_ms: u64,
    pub credits: BTreeMap<String, Credit>,
    #[serde(default)]
    pub fair: fair::Accounting,
}

pub fn clock_ms() -> io::Result<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut time) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(time.tv_sec as u64 * 1000 + time.tv_nsec as u64 / 1_000_000)
}

impl Ledger {
    pub fn load(store: &Store) -> io::Result<Self> {
        let path = store.root.join("scheduling.json");
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Self {
                    schema_version: 2,
                    boot_id: crate::process::boot_id()?,
                    checkpoint_ms: clock_ms()?,
                    credits: BTreeMap::new(),
                    fair: fair::Accounting::default(),
                });
            }
            Ok(meta) if meta.is_file() => {}
            _ => return Err(io::Error::other(message("invalid scheduling ledger"))),
        }
        let mut ledger: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        if ![1, 2].contains(&ledger.schema_version)
            || ledger.boot_id.is_empty()
            || (ledger.boot_id == crate::process::boot_id()? && ledger.checkpoint_ms > clock_ms()?)
            || ledger.credits.keys().any(|key| {
                key.split_once(':').is_none_or(|(id, attempt)| {
                    id.parse::<u64>()
                        .ok()
                        .filter(|n| *n > 0 && n.to_string() == id)
                        .is_none()
                        || attempt
                            .parse::<u64>()
                            .ok()
                            .filter(|n| *n > 0 && n.to_string() == attempt)
                            .is_none()
                })
            })
        {
            return Err(io::Error::other(message("invalid scheduling ledger")));
        }
        ledger.fair.validate().map_err(io::Error::other)?;
        ledger.schema_version = 2;
        Ok(ledger)
    }

    pub fn credit(&self, job: &Job) -> u64 {
        self.credits
            .get(&identity(job))
            .map_or(0, |c| c.milliseconds)
    }

    pub fn score(&self, graph: &Graph, job: &Job) -> i128 {
        let bonus = job
            .queue_id
            .and_then(|q| aging(graph, q))
            .map_or(0, |p| self.credit(job) / p.value.as_u64().unwrap());
        job.spec.declared.priority.unwrap_or(0) as i128 + bonus as i128
    }

    pub fn advance(
        &mut self,
        graph: &Graph,
        jobs: &BTreeMap<u64, Job>,
        boot: &str,
        now: u64,
    ) -> io::Result<()> {
        let delta = if self.boot_id == boot {
            now.checked_sub(self.checkpoint_ms)
                .ok_or_else(|| io::Error::other(message("invalid scheduling ledger")))?
        } else {
            0
        };
        self.fair
            .advance(graph, jobs, delta)
            .map_err(io::Error::other)?;
        let mut current = std::collections::BTreeSet::new();
        for job in jobs.values() {
            let key = identity(job);
            current.insert(key.clone());
            let eligible = job.state == State::Queued
                && job.queue_id.is_some_and(|q| {
                    aging(graph, q).is_some() && graph.view(q).paused_by.is_empty()
                });
            if !eligible && !self.credits.contains_key(&key) {
                continue;
            }
            let credit = self.credits.entry(key).or_default();
            if eligible && credit.eligible {
                credit.milliseconds = credit.milliseconds.checked_add(delta).ok_or_else(|| {
                    io::Error::other(message("waiting credit overflow; admission stopped"))
                })?;
            }
            credit.eligible = eligible;
        }
        for (key, credit) in &mut self.credits {
            if !current.contains(key) {
                credit.eligible = false;
            }
        }
        self.boot_id = boot.to_owned();
        self.checkpoint_ms = now;
        Ok(())
    }

    pub fn checkpoint(
        &mut self,
        store: &Store,
        graph: &Graph,
        jobs: &BTreeMap<u64, Job>,
        boot: &str,
    ) -> io::Result<()> {
        if !enabled(graph, jobs)
            && !self.credits.values().any(|c| c.eligible)
            && !self.fair.active()
        {
            return Ok(());
        }
        let mut next = self.clone();
        next.advance(graph, jobs, boot, clock_ms()?)?;
        crate::store::write_json(&store.root.join("scheduling.json"), &next)?;
        *self = next;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriorityChange {
    pub attempt: u64,
    pub actor_uid: u32,
    pub at_ms: u64,
    pub before: Option<i32>,
    pub after: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explanation {
    pub schema_version: u32,
    pub id: u64,
    pub attempt: u64,
    pub state: State,
    pub ordering_enabled: bool,
    pub priority: Option<i32>,
    pub priority_source: Option<Origin>,
    pub bounds: Vec<Bound>,
    pub eligible_wait_ms: u64,
    pub aging: Option<Effective>,
    pub effective_priority: String,
    pub fifo_predecessor: Option<(u64, String)>,
    pub backfill: bool,
    pub blocking_reason: Option<String>,
    pub predicted_start_ms: Option<u64>,
    pub caveat: String,
    #[serde(default)]
    pub fair_share: Vec<fair::Explanation>,
    #[serde(default)]
    pub pressure: Vec<crate::pressure::control::View>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<crate::netpolicy::Applied>,
}

impl Explanation {
    pub fn describe(&self) -> String {
        let source = match &self.priority_source {
            Some(Origin::Object { path, .. }) => path.clone(),
            Some(Origin::Preset { definition, .. }) => definition.clone(),
            Some(Origin::Job) => format!("Job {}", self.id),
            _ => message("unset"),
        };
        let policy = if self.state.editable() {
            "current policy"
        } else {
            "launch policy"
        };
        let mut lines = vec![
            format!(
                "{}: Job {}, {:?} ({})",
                message("Admission ordering"),
                self.id,
                self.state,
                message(policy)
            ),
            format!(
                "{}: {}; {}: {source}",
                message("base priority"),
                self.priority
                    .map_or_else(|| message("unset"), |p| p.to_string()),
                message("source")
            ),
            format!(
                "{}: {} ms; {}: {}",
                message("eligible waiting time"),
                self.eligible_wait_ms,
                message("effective priority"),
                self.effective_priority
            ),
        ];
        lines.push(format!(
            "{}: {}",
            message("ordering enabled"),
            self.ordering_enabled
        ));
        for fair in &self.fair_share {
            lines.push(format!(
                "{}: {} -> {}; {}; {}: {}; {}: {} + {}/{}; {}: {}; {}: {} x {} ms",
                message("fair share"),
                fair.scope_path,
                fair.child_path,
                fair.resource.name(),
                message("share weight"),
                fair.weight,
                message("normalized service"),
                fair.service.0,
                fair.remainder,
                fair.weight,
                message("service watermark"),
                fair.watermark.0,
                message("active lookahead"),
                fair.active_rate.0,
                fair.lookahead_ms
            ));
        }
        for pressure in &self.pressure {
            lines.push(format!(
                "{}: {} / {}; {:?}; {}={:?}; {}={}",
                crate::pressure::message("pressure policy"),
                pressure.scope_path,
                pressure.rule.id,
                pressure.phase,
                crate::pressure::message("temporary max-running"),
                pressure.temporary_max_running,
                crate::pressure::message("reconsider boot milliseconds"),
                pressure.reconsider_boot_ms
            ));
        }
        if let Some(network) = &self.network {
            lines.push(format!(
                "{}: {}",
                crate::netpolicy::message("network", &[]),
                network.describe()
            ));
        }
        if let Some(aging) = &self.aging {
            lines.push(format!(
                "{}: {} ms; {}: {}",
                message("aging interval"),
                aging.value,
                message("source"),
                aging.source_path
            ));
        }
        for bound in &self.bounds {
            lines.push(format!(
                "{}: {} [{}..{}]",
                message("ancestor priority bounds"),
                bound.path,
                bound.minimum.unwrap_or(MIN_PRIORITY),
                bound.maximum.unwrap_or(MAX_PRIORITY)
            ));
        }
        lines.push(format!(
            "{}: {}",
            message("backfill"),
            if self.backfill { "conservative" } else { "off" }
        ));
        lines.push(format!(
            "{}: {}",
            message("predicted start (Unix milliseconds)"),
            self.predicted_start_ms
                .map_or_else(|| message("unknown"), |at| at.to_string())
        ));
        if let Some((id, path)) = &self.fifo_predecessor {
            lines.push(format!(
                "{} {id}: {path}",
                message("strict FIFO waits for Job")
            ));
        }
        if let Some(reason) = &self.blocking_reason {
            lines.push(reason.clone());
        }
        lines.push(self.caveat.clone());
        lines.join("\n")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bound {
    pub object_id: u64,
    pub path: String,
    pub minimum: Option<i32>,
    pub maximum: Option<i32>,
}

pub fn described_bounds(graph: &Graph, queue: u64) -> Vec<Bound> {
    graph
        .ancestors(queue)
        .into_iter()
        .filter_map(|id| {
            let node = &graph.nodes[&id];
            let minimum = node
                .config
                .get("priority_min")
                .and_then(Value::as_i64)
                .map(|n| n as i32);
            let maximum = node
                .config
                .get("priority_max")
                .and_then(Value::as_i64)
                .map(|n| n as i32);
            (minimum.is_some() || maximum.is_some()).then(|| Bound {
                object_id: id,
                path: graph.path(id),
                minimum,
                maximum,
            })
        })
        .collect()
}
