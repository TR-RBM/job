use std::collections::BTreeMap;

use crate::config::Profile;
use crate::model::{Backend, Declared};
use crate::resource_policy::Origin;
use crate::service::discovery::Found;

use super::message;

pub fn unenforceable(declared: &Declared, sources: &BTreeMap<String, Origin>) -> Vec<&'static str> {
    let mut found = Vec::new();
    if declared.memory.is_some() && sources.get("memory_max") == Some(&Origin::Compatibility) {
        found.push("--mem (memory.max)");
    }
    if declared.pids.is_some() {
        found.push("--pids (pids.max)");
    }
    found
}

pub fn refuse(
    backend: Backend,
    profile: Profile,
    declared: &Declared,
    sources: &BTreeMap<String, Origin>,
) -> Result<(), String> {
    if backend != Backend::Watch || profile.legacy() || declared.on.is_some() {
        return Ok(());
    }
    let limits = unenforceable(declared, sources);
    if limits.is_empty() {
        return Ok(());
    }
    Err(message(
        "capability error: this service only monitors, it has no delegated cgroup, so {limits} cannot be enforced; delegate a cgroup to the service (job doctor says how) or leave the option out",
        &[("limits", limits.join(", "))],
    ))
}

pub fn startup(backend: Backend, required: bool) -> String {
    match backend {
        Backend::Cgroup => message(
            "enforcing mode: limits are enforced through the delegated cgroup ([cgroup] required = {required})",
            &[("required", required.to_string())],
        ),
        Backend::Watch => message(
            "monitoring mode: no delegated cgroup, limits are watched, not enforced; [cgroup] required = true makes the service refuse to start instead",
            &[],
        ),
    }
}

pub fn missing(found: &Found) -> String {
    let mut text = message(
        "[cgroup] required = true, and no usable delegated cgroup was found",
        &[],
    );
    if found.skipped.is_empty() {
        text.push_str("; ");
        text.push_str(&message(
            "the cgroup of the service is not writable by it, or it is the root of the hierarchy",
            &[],
        ));
    }
    for reason in &found.skipped {
        text.push_str("; ");
        text.push_str(reason);
    }
    text.push_str("; ");
    text.push_str(&message(
        "delegate a cgroup to the service (Delegate=yes in its systemd unit, or a directory the service user owns that holds only the service), name one with [cgroup] root, or set required = false to run in monitoring mode",
        &[],
    ));
    text
}
