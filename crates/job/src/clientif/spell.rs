use serde_json::{Map, Value, json};

use crate::model::{Job, Net, State, StopKind};
use crate::resource_policy::Origin;

pub fn state(job: &Job) -> &'static str {
    let settled = if job.state == State::Finished {
        job.outcome()
    } else {
        job.state.clone()
    };
    crate::operations::journal::state_name(&settled)
}

pub fn stop(kind: StopKind) -> &'static str {
    match kind {
        StopKind::Memory => "memory",
        StopKind::Processes => "processes",
        StopKind::Disk => "disk",
        StopKind::FilesystemFloor => "filesystem_floor",
        StopKind::HostMemory => "host_memory",
        StopKind::MemoryPressure => "memory_pressure",
        StopKind::WallTime => "wall_time",
        StopKind::Cancelled => "cancelled",
        StopKind::WorkingDirectoryGone => "working_directory_gone",
        StopKind::LimitChanged => "limit_changed",
        StopKind::DaemonLost => "daemon_lost",
    }
}

pub fn path(path: &str) -> &str {
    if path == "/" { "" } else { path }
}

pub fn placed(id: u64, at: &str) -> Value {
    json!({"from": "object", "id": id, "path": path(at)})
}

fn selector(source: &crate::presets::Source) -> Value {
    match source {
        crate::presets::Source::Job => json!({"from": "job"}),
        crate::presets::Source::Object { id, path } => placed(*id, path),
    }
}

pub fn origin(origin: &Origin) -> Value {
    match origin {
        Origin::Job => json!({"from": "job"}),
        Origin::Service => json!({"from": "service"}),
        Origin::Compatibility => json!({"from": "compatibility"}),
        Origin::Object { id, path } => placed(*id, path),
        Origin::Preset {
            definition,
            sha256,
            selected_by,
        } => json!({
            "from": "profile",
            "definition": definition,
            "sha256": sha256,
            "selected_by": selector(selected_by),
        }),
    }
}

fn reserved(source: &crate::resources::Source) -> Value {
    use crate::resources::Source;
    match source {
        Source::Unset => json!({"kind": "unset"}),
        Source::Declared => json!({"kind": "declared"}),
        Source::Default => json!({"kind": "default"}),
        Source::History { runs } => json!({"kind": "history", "runs": runs}),
        Source::RepositoryHistory { runs } => json!({"kind": "repository_history", "runs": runs}),
        Source::Rule { tool } => json!({"kind": "rule", "tool": tool}),
        Source::AfterStop { limit } => json!({"kind": "after_stop", "limit": limit}),
    }
}

fn net(net: &Net) -> String {
    match net {
        Net::Host => "host".to_owned(),
        Net::None => "none".to_owned(),
        Net::Proxy(url) => crate::netsecret::split(url).0,
        Net::WireGuard(file) => format!("wireguard:{}", file.display()),
        Net::OpenVpn(file) => format!("openvpn:{}", file.display()),
        Net::Namespace(name) => format!("ns:{name}"),
        Net::Profile(name) => format!("profile:{name}"),
    }
}

fn spec(record: &mut Map<String, Value>, member: &str, spec: Option<&crate::model::Spec>) {
    let Some(spec) = spec else {
        return;
    };
    if let Some(declared) = record
        .get_mut(member)
        .and_then(|spec| spec.get_mut("declared"))
        .and_then(Value::as_object_mut)
    {
        declared.insert(
            "net".to_owned(),
            spec.declared
                .net
                .as_ref()
                .map_or(Value::Null, |selected| Value::from(net(selected))),
        );
    }
}

pub fn record(job: &Job, revision: u64) -> Value {
    let bytes = crate::netsecret::rendered(job).unwrap_or_default();
    let mut value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let Some(record) = value.as_object_mut() else {
        return value;
    };
    record.insert("state".to_owned(), Value::from(state(job)));
    record.insert(
        "backend".to_owned(),
        Value::from(match job.backend {
            crate::model::Backend::Cgroup => "cgroup",
            crate::model::Backend::Watch => "watch",
        }),
    );
    record.insert("revision".to_owned(), Value::from(revision));
    record.insert(
        "priority_source".to_owned(),
        job.priority_source.as_ref().map_or(Value::Null, origin),
    );
    record.insert(
        "resource_sources".to_owned(),
        Value::Object(
            job.resource_sources
                .iter()
                .map(|(name, source)| (name.clone(), origin(source)))
                .collect(),
        ),
    );
    if let Some(reservation) = record.get_mut("reservation").and_then(Value::as_object_mut) {
        for (member, source) in [
            ("cores_source", &job.reservation.cores_source),
            ("memory_source", &job.reservation.memory_source),
            ("disk_source", &job.reservation.disk_source),
        ] {
            reservation.insert(member.to_owned(), reserved(source));
        }
    }
    if let (Some(stopped), Some(kind)) = (
        record.get_mut("stop").and_then(Value::as_object_mut),
        job.stop.as_ref().map(|stopped| stopped.kind),
    ) {
        stopped.insert("kind".to_owned(), Value::from(stop(kind)));
    }
    spec(record, "spec", Some(&job.spec));
    spec(record, "submitted_spec", job.submitted_spec.as_ref());
    spec(record, "requested_spec", job.requested_spec.as_ref());
    spec(record, "effective_spec", job.effective_spec.as_ref());
    value
}
