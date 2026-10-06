use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::model::{Backend, Job};

use super::spell;

const ARGV: usize = 1024;

pub const STATES: [&str; 10] = crate::operations::health::STATES;

#[derive(Clone)]
pub struct Row {
    pub id: u64,
    pub revision: u64,
    pub attempt: u64,
    pub state: &'static str,
    pub queue: Option<String>,
    pub queue_id: Option<u64>,
    pub actor_uid: Option<u32>,
    pub actor_pid: Option<i32>,
    pub session: String,
    pub labels: BTreeMap<String, String>,
    pub argv: Vec<String>,
    pub argv_truncated: bool,
    pub priority: Option<i32>,
    pub priority_source: Value,
    pub idempotency_key: Option<String>,
    pub submitted_ms: u64,
    pub waiting_since_ms: u64,
    pub admitted_ms: Option<u64>,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
    pub suspended_total_ms: u64,
    pub suspended_since_ms: Option<u64>,
    pub wall_limit_ms: Option<u64>,
    pub termination_deadline_ms: Option<u64>,
    pub predicted_start_ms: Option<u64>,
    pub waited_for: Option<String>,
    pub reserved: [u64; 3],
    pub memory_max: Value,
    pub stop_kind: Option<&'static str>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub start_error: Option<String>,
    pub usage: Value,
    pub network: Option<String>,
    pub terminal: bool,
    pub on: Option<String>,
    pub cgroup: Option<String>,
    pub confirmed: Value,
}

fn cut(argv: &[String]) -> (Vec<String>, bool) {
    let mut kept = Vec::with_capacity(argv.len());
    let mut room = ARGV;
    for word in argv {
        if word.len() <= room {
            room -= word.len();
            kept.push(word.clone());
            continue;
        }
        let mut end = room;
        while !word.is_char_boundary(end) {
            end -= 1;
        }
        if end > 0 {
            kept.push(word[..end].to_owned());
        }
        return (kept, true);
    }
    (kept, false)
}

fn ceiling(job: &Job) -> Value {
    match job.applied_resources.get("memory.max").map(String::as_str) {
        None => Value::from("not_applicable"),
        Some("max") => Value::from("unlimited"),
        Some(text) => text
            .trim()
            .parse::<u64>()
            .map_or_else(|_| Value::from("unknown"), Value::from),
    }
}

fn usage(job: &Job) -> Value {
    if !job.state.terminal() {
        return Value::Null;
    }
    let measured = job.backend == Backend::Cgroup && job.workload_cgroup.is_some();
    let timed = |value: u64| {
        if measured {
            Value::from(value)
        } else {
            Value::from("not_measured")
        }
    };
    json!({
        "peak_memory": job.usage.peak_memory,
        "peak_pids": job.usage.peak_pids,
        "written": job.usage.written,
        "cpu_ms": timed(job.usage.cpu_ms),
        "throttled_ms": timed(job.usage.throttled_ms),
    })
}

impl Row {
    pub fn of(job: &Job, revision: u64, predicted: Option<u64>, confirmed: Value) -> Self {
        let (argv, argv_truncated) = cut(&job.spec.argv);
        let result = job.result.as_ref();
        Self {
            id: job.id,
            revision,
            attempt: job.attempt,
            state: spell::state(job),
            queue: job.spec.queue.clone(),
            queue_id: job.queue_id,
            actor_uid: job.durability.actor_uid,
            actor_pid: job.durability.actor_pid,
            session: job.spec.session.clone(),
            labels: job.spec.declared.labels.clone(),
            argv,
            argv_truncated,
            priority: job.spec.declared.priority,
            priority_source: job
                .priority_source
                .as_ref()
                .map_or(Value::Null, spell::origin),
            idempotency_key: job.durability.idempotency_key.clone(),
            submitted_ms: job.submitted_ms,
            waiting_since_ms: job.waiting_since(),
            admitted_ms: job.durability.admitted_ms,
            started_ms: job.started_ms,
            finished_ms: job.finished_ms,
            suspended_total_ms: job.suspension.total_ms,
            suspended_since_ms: job.suspension.since_ms,
            wall_limit_ms: job.reservation.wall_limit_ms,
            termination_deadline_ms: job
                .termination_deadline_ms
                .filter(|_| !job.state.terminal()),
            predicted_start_ms: predicted,
            waited_for: job.waited_for.clone(),
            reserved: [
                job.reservation.vector.cores_milli,
                job.reservation.vector.memory,
                job.reservation.vector.pids,
            ],
            memory_max: ceiling(job),
            stop_kind: job.stop.as_ref().map(|stop| spell::stop(stop.kind)),
            exit_code: result.and_then(|result| result.exit_code),
            signal: result.and_then(|result| result.signal),
            start_error: result.and_then(|result| result.start_error.clone()),
            usage: usage(job),
            network: job
                .network
                .as_ref()
                .map(|network| network.selected.clone())
                .or_else(|| job.spec.declared.net.as_ref().map(spell::net)),
            terminal: job.spec.declared.terminal.is_some(),
            on: job
                .spec
                .declared
                .on
                .as_ref()
                .map(|remote| remote.target.clone()),
            cgroup: job
                .workload_cgroup
                .as_ref()
                .map(|path| path.display().to_string()),
            confirmed,
        }
    }

    fn wire(&self) -> Wire<'_> {
        Wire {
            id: self.id,
            revision: self.revision,
            attempt: self.attempt,
            state: self.state,
            queue: self.queue.as_deref(),
            queue_id: self.queue_id,
            actor_uid: self.actor_uid,
            actor_pid: self.actor_pid,
            session: &self.session,
            labels: &self.labels,
            argv: &self.argv,
            argv_truncated: self.argv_truncated,
            priority: self.priority,
            priority_source: &self.priority_source,
            idempotency_key: self.idempotency_key.as_deref(),
            submitted_ms: self.submitted_ms,
            waiting_since_ms: self.waiting_since_ms,
            admitted_ms: self.admitted_ms,
            started_ms: self.started_ms,
            finished_ms: self.finished_ms,
            suspended: Suspended {
                total_ms: self.suspended_total_ms,
                since_ms: self.suspended_since_ms,
            },
            wall_limit_ms: self.wall_limit_ms,
            termination_deadline_ms: self.termination_deadline_ms,
            predicted_start_ms: self.predicted_start_ms,
            waited_for: self.waited_for.as_deref(),
            reserved: Reserved {
                cores_milli: self.reserved[0],
                memory: self.reserved[1],
                pids: self.reserved[2],
            },
            memory_max: &self.memory_max,
            stop_kind: self.stop_kind,
            exit_code: self.exit_code,
            signal: self.signal,
            start_error: self.start_error.as_deref(),
            usage: &self.usage,
            network: self.network.as_deref(),
            terminal: self.terminal,
            on: self.on.as_deref(),
            cgroup: self.cgroup.as_deref(),
            confirmed: &self.confirmed,
        }
    }

    pub fn written(&self) -> Value {
        serde_json::to_value(self.wire()).unwrap_or(Value::Null)
    }

    pub fn write(&self, line: &mut String) {
        line.push_str(&serde_json::to_string(&self.wire()).unwrap_or_else(|_| "null".to_owned()));
    }

    pub fn holds(&self, text: &str, joined: &mut String) -> bool {
        if self.argv.iter().any(|word| word.contains(text))
            || self
                .queue
                .as_ref()
                .is_some_and(|queue| queue.contains(text))
            || self.session.contains(text)
        {
            return true;
        }
        if text.contains(' ') && self.argv.len() > 1 {
            joined.clear();
            for (at, word) in self.argv.iter().enumerate() {
                if at > 0 {
                    joined.push(' ');
                }
                joined.push_str(word);
            }
            if joined.contains(text) {
                return true;
            }
        }
        self.labels.iter().any(|(name, value)| {
            joined.clear();
            joined.push_str(name);
            joined.push('=');
            joined.push_str(value);
            joined.contains(text)
        })
    }
}

#[derive(serde::Serialize)]
struct Suspended {
    total_ms: u64,
    since_ms: Option<u64>,
}

#[derive(serde::Serialize)]
struct Reserved {
    cores_milli: u64,
    memory: u64,
    pids: u64,
}

#[derive(serde::Serialize)]
struct Wire<'a> {
    id: u64,
    revision: u64,
    attempt: u64,
    state: &'a str,
    queue: Option<&'a str>,
    queue_id: Option<u64>,
    actor_uid: Option<u32>,
    actor_pid: Option<i32>,
    session: &'a str,
    labels: &'a BTreeMap<String, String>,
    argv: &'a [String],
    argv_truncated: bool,
    priority: Option<i32>,
    priority_source: &'a Value,
    idempotency_key: Option<&'a str>,
    submitted_ms: u64,
    waiting_since_ms: u64,
    admitted_ms: Option<u64>,
    started_ms: Option<u64>,
    finished_ms: Option<u64>,
    suspended: Suspended,
    wall_limit_ms: Option<u64>,
    termination_deadline_ms: Option<u64>,
    predicted_start_ms: Option<u64>,
    waited_for: Option<&'a str>,
    reserved: Reserved,
    memory_max: &'a Value,
    stop_kind: Option<&'a str>,
    exit_code: Option<i32>,
    signal: Option<i32>,
    start_error: Option<&'a str>,
    usage: &'a Value,
    network: Option<&'a str>,
    terminal: bool,
    on: Option<&'a str>,
    cgroup: Option<&'a str>,
    confirmed: &'a Value,
}
