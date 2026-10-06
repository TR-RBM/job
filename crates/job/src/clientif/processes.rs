use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::daemon::Shared;
use crate::daemon::client_service::{self, Found};
use crate::model::Backend;

use super::args::{self, Args};
use super::record::missing;

const OP: &str = "processes";
pub const LIMIT: usize = 4096;
const GROUPS: usize = 1024;

fn members(path: &Path) -> Vec<i32> {
    let mut found = Vec::new();
    let mut groups = vec![path.to_owned()];
    let mut seen = 0;
    while let Some(group) = groups.pop() {
        seen += 1;
        found.extend(crate::cgroup::Tree::processes(&group));
        if seen >= GROUPS {
            break;
        }
        if let Ok(entries) = fs::read_dir(&group) {
            groups.extend(
                entries
                    .flatten()
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .map(|entry| entry.path()),
            );
        }
    }
    found
}

fn described(pid: i32) -> Option<Value> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let stat = crate::procs::parse_stat(&text)?;
    let name = text.get(text.find('(')? + 1..text.rfind(')')?)?;
    Some(json!({
        "pid": stat.pid,
        "parent": stat.parent,
        "name": name,
        "threads": stat.threads,
        "rss": crate::procs::page_bytes(stat.rss_pages),
        "start_ticks": stat.start_ticks,
    }))
}

pub fn answer(shared: &Shared, args: &Args) -> Result<String, String> {
    let id = args::needed(args, OP, "id")?;
    let loaded = match client_service::found(shared, id) {
        Found::Missing => return Ok(missing("job")),
        Found::Unreadable => return Ok(missing("record")),
        Found::Job(loaded) => loaded,
    };
    let job = &loaded.job;
    let group = job
        .workload_cgroup
        .as_ref()
        .filter(|_| loaded.backend == Backend::Cgroup);
    let at = crate::shim::now_ms();
    let mut pids: Vec<i32> = if !job.state.active() {
        Vec::new()
    } else if let Some(path) = group {
        members(path)
    } else {
        loaded
            .subject
            .supervisor
            .map(|pid| crate::procs::descendants(pid, &crate::procs::all()))
            .unwrap_or_default()
            .iter()
            .map(|stat| stat.pid)
            .collect()
    };
    pids.sort_unstable();
    pids.dedup();
    let mut processes: Vec<Value> = Vec::new();
    let mut truncated = false;
    for pid in pids {
        let Some(process) = described(pid) else {
            continue;
        };
        if processes.len() == LIMIT {
            truncated = true;
            break;
        }
        processes.push(process);
    }
    Ok(json!({
        "at_ms": at,
        "now_ms": crate::shim::now_ms(),
        "attempt": job.attempt,
        "active": job.state.active(),
        "source": if group.is_some() { "cgroup" } else { "descendants" },
        "truncated": truncated,
        "processes": processes,
    })
    .to_string())
}
