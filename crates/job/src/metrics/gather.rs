use serde_json::Value;

use crate::clientif::measure::Counters;
use crate::clientif::{index, tree};
use crate::daemon::Shared;
use crate::daemon::client_service;
use crate::model::Backend;
use crate::operations::health::STATES;

use super::observed;
use super::text::{Exposition, Kind, cores, flag, seconds};

const ACTIVE: [&str; 6] = [
    "held",
    "queued",
    "starting",
    "running",
    "suspended",
    "stopping",
];

fn single(
    text: &mut Exposition,
    name: &str,
    kind: Kind,
    help: &str,
    value: impl std::fmt::Display,
) {
    text.family(name, kind, help);
    text.sample(name, &[], value);
}

fn health(text: &mut Exposition, held: &client_service::Held) {
    let health = crate::operations::health::fixed(&held.store, held.backend);
    let backend = match held.backend {
        Backend::Cgroup => "cgroup",
        Backend::Watch => "watch",
    };
    single(
        text,
        "job_up",
        Kind::Gauge,
        "1 when the job service answered this request.",
        1,
    );
    text.family(
        "job_service_info",
        Kind::Gauge,
        "Always 1; the labels name the version of the service, its protocol and state schema, its backend and whether limits are enforced.",
    );
    text.sample(
        "job_service_info",
        &[
            ("version", &crate::daemon::version()),
            ("protocol", &health.protocol.to_string()),
            ("state_schema", &health.state_schema.to_string()),
            ("backend", backend),
            ("enforcement", &health.enforcement),
        ],
        1,
    );
    single(
        text,
        "job_service_start_time_seconds",
        Kind::Gauge,
        "Unix time at which the service started.",
        seconds(health.started_ms),
    );
    text.family(
        "job_journal_writable",
        Kind::Gauge,
        "1 when the service can write the journal, 0 when its last write failed.",
    );
    text.sample(
        "job_journal_writable",
        &[("journal", "audit")],
        flag(health.audit_writable),
    );
    text.sample(
        "job_journal_writable",
        &[("journal", "events")],
        flag(health.events_writable),
    );
    single(
        text,
        "job_starter_failures_total",
        Kind::Counter,
        "Passes of the service's starter that failed since the service started.",
        health.starter_failures,
    );
    single(
        text,
        "job_cancellations_failing",
        Kind::Gauge,
        "Cancellations the service is still retrying.",
        health.cancellations_failing,
    );
    single(
        text,
        "job_cancellation_retries_total",
        Kind::Counter,
        "Retries of cancellations since the service started.",
        health.cancellation_retries,
    );
    single(
        text,
        "job_unreadable_records",
        Kind::Gauge,
        "Job records in the state directory that the service cannot read.",
        crate::streams::budget::unreadable().len(),
    );
    if let Some(free) = health.state_free_bytes {
        single(
            text,
            "job_state_free_bytes",
            Kind::Gauge,
            "Free space in the file system of the state directory.",
            free,
        );
    }
}

fn counts(text: &mut Exposition, held: &client_service::Held, ended: &[u64]) {
    text.family(
        "job_jobs",
        Kind::Gauge,
        "Jobs the service holds a record of, by state.",
    );
    for (at, state) in STATES.iter().enumerate() {
        let count = held.states.get(at).copied().unwrap_or(0) + ended.get(at).copied().unwrap_or(0);
        text.sample("job_jobs", &[("state", state)], count);
    }
}

fn capacity(text: &mut Exposition, held: &client_service::Held) {
    let pool = &held.pool;
    let reserved = &held.reserved;
    single(
        text,
        "job_pool_cores",
        Kind::Gauge,
        "Cores the service admits Jobs against.",
        cores(pool.cores_milli),
    );
    single(
        text,
        "job_pool_memory_bytes",
        Kind::Gauge,
        "Memory the service admits Jobs against.",
        pool.memory,
    );
    single(
        text,
        "job_pool_processes",
        Kind::Gauge,
        "Processes the service admits Jobs against.",
        pool.pids,
    );
    single(
        text,
        "job_reserved_cores",
        Kind::Gauge,
        "Cores reserved by Jobs that are starting, running, suspended or stopping.",
        cores(reserved.cores_milli),
    );
    single(
        text,
        "job_reserved_memory_bytes",
        Kind::Gauge,
        "Memory reserved by Jobs that are starting, running, suspended or stopping.",
        reserved.memory,
    );
    single(
        text,
        "job_reserved_processes",
        Kind::Gauge,
        "Processes reserved by Jobs that are starting, running, suspended or stopping.",
        reserved.pids,
    );
    let meminfo = crate::host::read_meminfo();
    single(
        text,
        "job_host_memory_bytes",
        Kind::Gauge,
        "Memory of the host, MemTotal of /proc/meminfo.",
        meminfo.total,
    );
    single(
        text,
        "job_host_memory_available_bytes",
        Kind::Gauge,
        "Memory of the host available for new work, MemAvailable of /proc/meminfo.",
        meminfo.available,
    );
    if let Some(recorded) = crate::streams::budget::passed() {
        single(
            text,
            "job_output_recorded_bytes",
            Kind::Gauge,
            "Bytes of recorded output that count against the output budget.",
            recorded,
        );
    }
    single(
        text,
        "job_output_budget_bytes",
        Kind::Gauge,
        "The output budget: recorded output of all Jobs together is kept below it.",
        held.budget,
    );
}

fn usage(text: &mut Exposition, jobs: Option<&std::path::Path>) {
    let Some(path) = jobs else {
        return;
    };
    let counters = Counters::read(path);
    let figures: [(&str, Kind, &str, Option<String>); 5] = [
        (
            "job_use_memory_bytes",
            Kind::Gauge,
            "Memory used by all Jobs together, memory.current of their cgroup.",
            counters.memory.map(|value| value.to_string()),
        ),
        (
            "job_use_processes",
            Kind::Gauge,
            "Processes of all Jobs together, pids.current of their cgroup.",
            counters.pids.map(|value| value.to_string()),
        ),
        (
            "job_use_cpu_seconds_total",
            Kind::Counter,
            "CPU time used by Jobs, usage_usec of cpu.stat of their cgroup.",
            counters.cpu_ms.map(|value| seconds(value).to_string()),
        ),
        (
            "job_use_cpu_throttled_seconds_total",
            Kind::Counter,
            "Time Jobs were throttled by a CPU limit, throttled_usec of cpu.stat of their cgroup.",
            counters
                .throttled_ms
                .map(|value| seconds(value).to_string()),
        ),
        (
            "job_use_oom_kills_total",
            Kind::Counter,
            "Processes of Jobs killed for memory, oom_kill of memory.events of their cgroup.",
            counters.oom_kill.map(|value| value.to_string()),
        ),
    ];
    for (name, kind, help, value) in figures {
        if let Some(value) = value {
            single(text, name, kind, help, value);
        }
    }
}

struct Object {
    path: String,
    kind: String,
    states: Vec<(&'static str, u64)>,
    oldest_ms: Option<u64>,
    running_limit: Option<u64>,
    budgets: Vec<(String, u64, u64)>,
}

fn object(node: &Value) -> Object {
    let id = node["object"]["id"].as_u64();
    let path = node["path"].as_str().unwrap_or_default();
    let own = |entry: &Value| entry["set_by"]["id"].as_u64() == id;
    Object {
        path: if path.is_empty() { "/" } else { path }.to_owned(),
        kind: node["object"]["kind"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        states: ACTIVE
            .iter()
            .map(|state| (*state, node["depth"][*state].as_u64().unwrap_or(0)))
            .collect(),
        oldest_ms: node["depth"]["oldest_queued_age_ms"].as_u64(),
        running_limit: node["use"]["running"]["limits"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|limit| own(limit))
            .and_then(|limit| limit["limit"].as_u64()),
        budgets: node["use"]["budgets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|budget| own(budget))
            .filter_map(|budget| {
                Some((
                    budget["resource"].as_str()?.to_owned(),
                    budget["limit"].as_u64()?,
                    budget["reserved"].as_u64()?,
                ))
            })
            .collect(),
    }
}

fn objects(text: &mut Exposition, nodes: &[Value]) {
    let objects: Vec<Object> = nodes.iter().map(object).collect();
    text.family(
        "job_object_jobs",
        Kind::Gauge,
        "Jobs that have not ended in a Queue or in the subtree of a Group, by state.",
    );
    for object in &objects {
        for (state, count) in &object.states {
            text.sample(
                "job_object_jobs",
                &[
                    ("kind", &object.kind),
                    ("path", &object.path),
                    ("state", state),
                ],
                count,
            );
        }
    }
    text.family(
        "job_object_oldest_queued_age_seconds",
        Kind::Gauge,
        "How long the oldest queued Job of a Queue or Group subtree has waited; absent when none is queued.",
    );
    for object in &objects {
        if let Some(age) = object.oldest_ms {
            text.sample(
                "job_object_oldest_queued_age_seconds",
                &[("kind", &object.kind), ("path", &object.path)],
                seconds(age),
            );
        }
    }
    text.family(
        "job_object_running_limit",
        Kind::Gauge,
        "The limit on Jobs running at once that a Queue or Group sets itself; absent when it sets none.",
    );
    for object in &objects {
        if let Some(limit) = object.running_limit {
            text.sample(
                "job_object_running_limit",
                &[("kind", &object.kind), ("path", &object.path)],
                limit,
            );
        }
    }
    for (resource, name, reserved_name, help, reserved_help) in [
        (
            "cores_milli",
            "job_object_cores_limit",
            "job_object_reserved_cores",
            "The cores a Queue or Group sets as what its Jobs hold together at most; absent when it sets none.",
            "Cores reserved by the Jobs below a Queue or Group that sets a core limit.",
        ),
        (
            "memory",
            "job_object_memory_limit_bytes",
            "job_object_reserved_memory_bytes",
            "The memory a Queue or Group sets as what its Jobs hold together at most; absent when it sets none.",
            "Memory reserved by the Jobs below a Queue or Group that sets a memory limit.",
        ),
    ] {
        let set: Vec<(&Object, u64, u64)> = objects
            .iter()
            .flat_map(|object| {
                object
                    .budgets
                    .iter()
                    .filter(|(name, _, _)| name == resource)
                    .map(move |(_, limit, reserved)| (object, *limit, *reserved))
            })
            .collect();
        let shown = |value: u64| {
            if resource == "cores_milli" {
                cores(value).to_string()
            } else {
                value.to_string()
            }
        };
        text.family(name, Kind::Gauge, help);
        for (object, limit, _) in &set {
            text.sample(
                name,
                &[("kind", &object.kind), ("path", &object.path)],
                shown(*limit),
            );
        }
        text.family(reserved_name, Kind::Gauge, reserved_help);
        for (object, _, reserved) in &set {
            text.sample(
                reserved_name,
                &[("kind", &object.kind), ("path", &object.path)],
                shown(*reserved),
            );
        }
    }
}

pub fn render(shared: &Shared) -> String {
    let now = crate::shim::now_ms();
    let (nodes, held, ended, jobs) = client_service::seen(shared, |seen| {
        let index = index::locked();
        (
            tree::drawn(seen, now),
            client_service::held(seen),
            index.counts(),
            seen.tree.map(crate::cgroup::Tree::jobs),
        )
    });
    let mut text = Exposition::default();
    health(&mut text, &held);
    counts(&mut text, &held, &ended);
    capacity(&mut text, &held);
    usage(&mut text, jobs.as_deref());
    objects(&mut text, &nodes);
    observed::written(&mut text);
    text.text()
}

pub fn down() -> String {
    let mut text = Exposition::default();
    single(
        &mut text,
        "job_up",
        Kind::Gauge,
        "1 when the job service answered this request.",
        0,
    );
    text.text()
}
