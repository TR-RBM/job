use serde_json::{Value, json};

use crate::daemon::Shared;
use crate::daemon::client_service::{self, Held};
use crate::model::Backend;

use super::row::STATES;
use super::{index, measure};

pub fn body(held: &Held, ended: [u64; STATES.len()]) -> String {
    let counts: serde_json::Map<String, Value> = STATES
        .iter()
        .enumerate()
        .map(|(at, state)| {
            let count = held.states.get(at).copied().unwrap_or(0) + ended[at];
            ((*state).to_owned(), Value::from(count))
        })
        .collect();
    let meminfo = crate::host::read_meminfo();
    let capacity = json!({
        "pool": {
            "cores_milli": held.pool.cores_milli,
            "memory": held.pool.memory,
            "pids": held.pool.pids,
        },
        "reserved": {
            "cores_milli": held.reserved.cores_milli,
            "memory": held.reserved.memory,
            "pids": held.reserved.pids,
        },
        "running": held.states[2..].iter().sum::<u64>(),
        "waiting": held.states[..2].iter().sum::<u64>(),
        "memory_total": meminfo.total,
        "memory_available": meminfo.available,
        "output": {
            "recorded_bytes": measure::figure(crate::streams::budget::passed()),
            "budget_bytes": held.budget,
        },
        "state_free_bytes": measure::figure(
            crate::host::space_of(&held.store.root).map(|(_, space)| space.free),
        ),
    });
    format!(
        "\"counts\":{},\"capacity\":{capacity}",
        Value::Object(counts)
    )
}

pub fn held(shared: &Shared) -> (Held, [u64; STATES.len()], u64) {
    client_service::seen(shared, |seen| {
        let index = index::locked();
        (
            client_service::held(seen),
            index.counts(),
            super::changes::last(),
        )
    })
}

pub fn answer(shared: &Shared) -> String {
    let (held, ended, seq) = held(shared);
    format!(
        "{{\"seq\":{seq},\"now_ms\":{},{}}}",
        crate::shim::now_ms(),
        body(&held, ended)
    )
}

pub fn health(held: &Held) -> String {
    let health = crate::operations::health::fixed(&held.store, held.backend);
    json!({
        "started_ms": health.started_ms,
        "state_schema": health.state_schema,
        "backend": match held.backend {
            Backend::Cgroup => "cgroup",
            Backend::Watch => "watch",
        },
        "enforcement": health.enforcement,
        "cgroup_required": health.cgroup_required,
        "supervisors_adopted": health.supervisors_adopted,
        "last_recovery_ms": health.last_recovery_ms,
        "recovery_changed_records": health.recovery_changed_records,
        "audit_writable": health.audit_writable,
        "events_writable": health.events_writable,
        "starter_failures": health.starter_failures,
        "cancellations_failing": health.cancellations_failing,
        "cancellation_retries": health.cancellation_retries,
        "unreadable_records": crate::streams::budget::unreadable(),
    })
    .to_string()
}
