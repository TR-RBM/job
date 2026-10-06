use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

fn file(path: &Path, name: &str) -> Option<String> {
    fs::read_to_string(path.join(name)).ok()
}

fn number(path: &Path, name: &str) -> Option<u64> {
    file(path, name)?.trim().parse().ok()
}

fn keyed(text: Option<&String>, key: &str) -> Option<u64> {
    text?.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        (words.next() == Some(key))
            .then(|| words.next()?.parse().ok())
            .flatten()
    })
}

pub fn figure(value: Option<u64>) -> Value {
    value.map_or_else(|| Value::from("unknown"), Value::from)
}

pub fn held(path: &Path, name: &str) -> Value {
    file(path, name).map_or_else(
        || Value::from("unknown"),
        |text| Value::from(text.trim().to_owned()),
    )
}

pub struct Counters {
    pub memory: Option<u64>,
    pub peak_memory: Option<u64>,
    pub pids: Option<u64>,
    pub peak_pids: Option<u64>,
    pub written: Option<u64>,
    pub cpu_ms: Option<u64>,
    pub throttled_ms: Option<u64>,
    pub oom_kill: Option<u64>,
    pub oom_group_kill: Option<u64>,
    pub pids_max_events: Option<u64>,
}

impl Counters {
    pub fn read(path: &Path) -> Self {
        let memory_events = file(path, "memory.events");
        let pids_events = file(path, "pids.events");
        let cpu = file(path, "cpu.stat");
        let memory_stat = file(path, "memory.stat");
        let memory = number(path, "memory.current");
        let pids = number(path, "pids.current");
        let written = file(path, "io.stat")
            .map(|text| crate::cgroup::io_written_bytes(&text))
            .zip(keyed(memory_stat.as_ref(), "file_dirty"))
            .map(|(back, dirty)| back.saturating_add(dirty));
        Self {
            memory,
            peak_memory: number(path, "memory.peak").map(|peak| peak.max(memory.unwrap_or(0))),
            pids,
            peak_pids: number(path, "pids.peak").map(|peak| peak.max(pids.unwrap_or(0))),
            written,
            cpu_ms: keyed(cpu.as_ref(), "usage_usec").map(|micro| micro / 1000),
            throttled_ms: keyed(cpu.as_ref(), "throttled_usec").map(|micro| micro / 1000),
            oom_kill: keyed(memory_events.as_ref(), "oom_kill"),
            oom_group_kill: keyed(memory_events.as_ref(), "oom_group_kill"),
            pids_max_events: keyed(pids_events.as_ref(), "max"),
        }
    }

    pub fn written(&self) -> Map<String, Value> {
        [
            ("memory", self.memory),
            ("peak_memory", self.peak_memory),
            ("pids", self.pids),
            ("peak_pids", self.peak_pids),
            ("written", self.written),
            ("cpu_ms", self.cpu_ms),
            ("throttled_ms", self.throttled_ms),
            ("oom_kill", self.oom_kill),
            ("oom_group_kill", self.oom_group_kill),
            ("pids_max_events", self.pids_max_events),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), figure(value)))
        .collect()
    }
}
