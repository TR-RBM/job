use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::model::{Backend, Job};
use crate::store::Store;
use crate::units::{format_bytes, format_duration_ms};

use super::{journal, message};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub started_ms: u64,
    pub uptime_ms: u64,
    pub state_schema: u32,
    pub protocol: u32,
    pub backend: Backend,
    #[serde(default)]
    pub enforcement: String,
    #[serde(default)]
    pub cgroup_required: bool,
    pub jobs: BTreeMap<String, u64>,
    pub supervisors_adopted: u64,
    pub last_recovery_ms: Option<u64>,
    pub recovery_changed_records: u64,
    pub state_free_bytes: Option<u64>,
    pub audit_writable: bool,
    pub events_writable: bool,
    #[serde(default)]
    pub starter_failures: u64,
    #[serde(default)]
    pub cancellations_failing: u64,
    #[serde(default)]
    pub cancellation_retries: u64,
}

#[derive(Clone, Copy, Default)]
struct Recovery {
    at_ms: Option<u64>,
    adopted: u64,
    changed: u64,
}

static STARTED: OnceLock<u64> = OnceLock::new();
static RECOVERY: Mutex<Recovery> = Mutex::new(Recovery {
    at_ms: None,
    adopted: 0,
    changed: 0,
});
static REQUIRED: OnceLock<bool> = OnceLock::new();

pub fn started() {
    let _ = STARTED.set(crate::shim::now_ms());
}

pub fn cgroup_required(required: bool) {
    let _ = REQUIRED.set(required);
}

pub fn recovered(adopted: usize, changed: u64) {
    *RECOVERY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Recovery {
        at_ms: Some(crate::shim::now_ms()),
        adopted: adopted as u64,
        changed,
    };
}

pub fn start() -> u64 {
    STARTED.get().copied().unwrap_or_else(crate::shim::now_ms)
}

pub fn enforcement(backend: Backend) -> &'static str {
    match backend {
        Backend::Cgroup => "enforced",
        Backend::Watch => "monitoring_only",
    }
}

pub const STATES: [&str; 10] = [
    "held",
    "queued",
    "starting",
    "running",
    "suspended",
    "stopping",
    "succeeded",
    "failed",
    "cancelled",
    "lost",
];

pub fn report(store: &Store, live: &BTreeMap<u64, Job>, backend: Backend) -> Health {
    let mut health = fixed(store, backend);
    let jobs = &mut health.jobs;
    for id in store.job_ids() {
        let state = live
            .get(&id)
            .map(|job| job.state.clone())
            .or_else(|| journal::known_state(id))
            .or_else(|| store.load_job(id).map(|job| job.state));
        if let Some(state) = state {
            *jobs
                .entry(journal::state_name(&state).to_owned())
                .or_default() += 1;
        }
    }
    health
}

pub fn fixed(store: &Store, backend: Backend) -> Health {
    let now = crate::shim::now_ms();
    let started = STARTED.get().copied().unwrap_or(now);
    let jobs: BTreeMap<String, u64> = STATES.iter().map(|name| ((*name).to_owned(), 0)).collect();
    let recovery = *RECOVERY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Health {
        started_ms: started,
        uptime_ms: now.saturating_sub(started),
        state_schema: crate::migration::recorded(&store.root)
            .ok()
            .flatten()
            .unwrap_or(crate::migration::SCHEMA),
        protocol: crate::model::PROTOCOL,
        backend,
        enforcement: enforcement(backend).to_owned(),
        cgroup_required: REQUIRED.get().copied().unwrap_or(false),
        jobs,
        supervisors_adopted: recovery.adopted,
        last_recovery_ms: recovery.at_ms,
        recovery_changed_records: recovery.changed,
        state_free_bytes: crate::host::space_of(&store.root).map(|(_, space)| space.free),
        audit_writable: crate::durability::audit::writable(),
        events_writable: journal::writable(),
        starter_failures: crate::pacing::starter_failures(),
        cancellations_failing: crate::pacing::cancellations_failing().0,
        cancellation_retries: crate::pacing::cancellations_failing().1,
    }
}

pub fn problems(health: &Health) -> Vec<String> {
    let mut found = Vec::new();
    if !health.audit_writable {
        found.push(message("the audit journal cannot be written", &[]));
    }
    if !health.events_writable {
        found.push(message("the event journal cannot be written", &[]));
    }
    if health.state_free_bytes.is_some_and(|free| free < LOW_SPACE) {
        found.push(message(
            "the state directory has less than {floor} left",
            &[("floor", format_bytes(LOW_SPACE))],
        ));
    }
    if health.cancellations_failing > 0 {
        found.push(message(
            "{count} cancellations could not be completed and are tried again",
            &[("count", health.cancellations_failing.to_string())],
        ));
    }
    found
}

pub fn summary(health: &Health) -> String {
    message(
        "up {uptime}, {active} active and {waiting} waiting Jobs, {adopted} supervisors adopted at the last start, {free} left in the state directory, both journals writable",
        &[
            ("uptime", format_duration_ms(health.uptime_ms)),
            (
                "active",
                ["starting", "running", "suspended", "stopping"]
                    .iter()
                    .map(|name| health.jobs.get(*name).copied().unwrap_or(0))
                    .sum::<u64>()
                    .to_string(),
            ),
            (
                "waiting",
                ["held", "queued"]
                    .iter()
                    .map(|name| health.jobs.get(*name).copied().unwrap_or(0))
                    .sum::<u64>()
                    .to_string(),
            ),
            ("adopted", health.supervisors_adopted.to_string()),
            (
                "free",
                health
                    .state_free_bytes
                    .map_or_else(|| message("unknown", &[]), format_bytes),
            ),
        ],
    )
}

pub const LOW_SPACE: u64 = 64 << 20;

pub fn mode_line(health: &Health) -> String {
    match health.backend {
        Backend::Cgroup => message("limits are enforced through the delegated cgroup", &[]),
        Backend::Watch => message(
            "monitoring only: no delegated cgroup, limits are watched, not enforced",
            &[],
        ),
    }
}

pub fn render(health: &Health) -> Vec<String> {
    let mut lines = described(health);
    if health.starter_failures > 0
        || health.cancellations_failing > 0
        || health.cancellation_retries > 0
    {
        lines.push(message(
            "health: the starter failed {count} times since the service started; cancellations failing {failing}, retried {retries} times",
            &[
                ("count", health.starter_failures.to_string()),
                ("failing", health.cancellations_failing.to_string()),
                ("retries", health.cancellation_retries.to_string()),
            ],
        ));
    }
    lines
}

fn described(health: &Health) -> Vec<String> {
    let yes = |value: bool| message(if value { "writable" } else { "not writable" }, &[]);
    vec![
        message(
            "health: up {uptime}, state schema {schema}, protocol {protocol}",
            &[
                ("uptime", format_duration_ms(health.uptime_ms)),
                ("schema", health.state_schema.to_string()),
                ("protocol", health.protocol.to_string()),
            ],
        ),
        format!("health: {}", mode_line(health)),
        message(
            "health: jobs {counts}",
            &[(
                "counts",
                STATES
                    .iter()
                    .map(|name| format!("{} {name}", health.jobs.get(*name).copied().unwrap_or(0)))
                    .collect::<Vec<_>>()
                    .join(", "),
            )],
        ),
        message(
            "health: last start adopted {adopted} supervisors and changed {changed} records",
            &[
                ("adopted", health.supervisors_adopted.to_string()),
                ("changed", health.recovery_changed_records.to_string()),
            ],
        ),
        message(
            "health: state directory has {free} left; audit journal {audit}, event journal {events}",
            &[
                (
                    "free",
                    health
                        .state_free_bytes
                        .map_or_else(|| message("unknown", &[]), format_bytes),
                ),
                ("audit", yes(health.audit_writable)),
                ("events", yes(health.events_writable)),
            ],
        ),
    ]
}
