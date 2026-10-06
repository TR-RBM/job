use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::Job;
use crate::store::Store;

const FILE: &str = "confirmed.json";

#[derive(Clone, Serialize, Deserialize)]
pub struct Confirmed {
    pub attempt: u64,
    pub at_ms: u64,
    #[serde(default)]
    pub isolation_controls: Option<crate::isolation::kernel::Applied>,
    #[serde(default)]
    pub security_controls: Option<crate::security::kernel::Applied>,
    #[serde(default)]
    pub process_controls: Option<crate::process_policy::kernel::Applied>,
}

pub fn record(
    store: &Store,
    id: u64,
    attempt: u64,
    isolation: Option<&crate::isolation::kernel::Applied>,
    security: Option<&crate::security::kernel::Applied>,
    process: Option<&crate::process_policy::kernel::Applied>,
) {
    let confirmed = Confirmed {
        attempt,
        at_ms: crate::shim::now_ms(),
        isolation_controls: isolation.cloned(),
        security_controls: security.cloned(),
        process_controls: process.cloned(),
    };
    let _ = crate::durability::write_in_boot(&store.job_dir(id).join(FILE), &confirmed);
}

pub fn directory(store: &Store, job: &Job, current: bool) -> PathBuf {
    if current {
        store.job_dir(job.id)
    } else {
        store
            .job_dir(job.id)
            .join("attempts")
            .join(job.attempt.to_string())
    }
}

pub fn read(directory: &Path, job: &Job) -> Option<Confirmed> {
    crate::store::read_json::<Confirmed>(&directory.join(FILE))
        .filter(|confirmed| confirmed.attempt == job.attempt)
}

pub fn started(job: &Job) -> bool {
    let failed = job
        .result
        .as_ref()
        .is_some_and(|result| result.start_error.is_some());
    match job.state {
        crate::model::State::Held | crate::model::State::Queued | crate::model::State::Starting => {
            false
        }
        _ if job.state.terminal() => job.started_ms.is_some() && !failed,
        _ => true,
    }
}

pub fn absent(job: &Job) -> Value {
    Value::from(if started(job) {
        "unknown"
    } else {
        "not_applicable"
    })
}

pub fn short(confirmed: &Confirmed) -> Value {
    let isolation = confirmed.isolation_controls.as_ref();
    let security = confirmed.security_controls.as_ref();
    json!({
        "namespaces": isolation.map(|applied| applied.namespaces.clone()).unwrap_or_default(),
        "pid_namespace": isolation.is_some_and(|applied| applied.pid_namespace.is_some()),
        "no_new_privs": security.is_some_and(|applied| applied.no_new_privs),
        "cap_drop": security.and_then(|applied| applied.cap_drop.clone()),
        "seccomp_denied": security
            .and_then(|applied| applied.seccomp_deny.as_ref())
            .map_or(0, |deny| {
                String::from(deny.clone())
                    .split(',')
                    .filter(|name| !name.trim().is_empty())
                    .count()
            }),
        "cpu_affinity": confirmed
            .process_controls
            .as_ref()
            .and_then(|applied| applied.cpu_affinity.clone()),
    })
}

pub fn full(confirmed: &Confirmed, job: &Job) -> Value {
    let named = |settings: &[&str]| -> Vec<(String, Value)> {
        job.resource_sources
            .iter()
            .filter(|(name, _)| settings.iter().any(|setting| name.starts_with(setting)))
            .map(|(name, source)| (name.clone(), super::spell::origin(source)))
            .collect()
    };
    let mut sources = serde_json::Map::new();
    if confirmed.isolation_controls.is_some() {
        sources.extend(named(&["namespaces", "root", "private_tmp", "writable"]));
    }
    if confirmed.security_controls.is_some() {
        sources.extend(named(&["no_new_privs", "cap_drop", "seccomp_deny"]));
    }
    if confirmed.process_controls.is_some() {
        sources.extend(named(&["cpu_affinity", "numa_policy", "rlimit_"]));
    }
    json!({
        "at_ms": confirmed.at_ms,
        "isolation_controls": confirmed.isolation_controls,
        "security_controls": confirmed.security_controls,
        "process_controls": confirmed.process_controls,
        "sources": sources,
    })
}
