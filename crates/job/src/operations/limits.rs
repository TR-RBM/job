use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::units::format_bytes;

use super::message;

const MOUNT: &str = "/sys/fs/cgroup";
const DEPTH: usize = 64;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limit {
    pub value: Option<String>,
    pub set_by: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rlimit {
    pub soft: Option<u64>,
    pub hard: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Surrounding {
    pub cgroup: Option<String>,
    pub discoverable: bool,
    pub memory_max: Limit,
    pub memory_high: Limit,
    pub cpu_max: Limit,
    pub pids_max: Limit,
    pub cpuset_cpus_effective: Limit,
    pub rlimits: BTreeMap<String, Rlimit>,
}

pub const RLIMITS: [&str; 14] = [
    "as",
    "core",
    "cpu",
    "data",
    "fsize",
    "memlock",
    "msgqueue",
    "nice",
    "nofile",
    "nproc",
    "rtprio",
    "rttime",
    "sigpending",
    "stack",
];

fn chain(own: &Path) -> Vec<PathBuf> {
    let mount = Path::new(MOUNT);
    let mut found = Vec::new();
    let mut current = Some(own);
    while let Some(path) = current {
        if path == mount || !path.starts_with(mount) || found.len() >= DEPTH {
            break;
        }
        found.push(path.to_path_buf());
        current = path.parent();
    }
    found
}

fn shown(path: &Path) -> String {
    format!(
        "/{}",
        path.strip_prefix(MOUNT)
            .unwrap_or(path)
            .display()
            .to_string()
            .trim_start_matches('/')
    )
}

fn read(path: &Path, file: &str) -> Option<String> {
    fs::read_to_string(path.join(file))
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

fn amount(text: &str) -> Option<u64> {
    text.parse().ok()
}

fn bandwidth(text: &str) -> Option<u64> {
    let (quota, period) = text.split_once(' ')?;
    let (quota, period) = (quota.parse::<u64>().ok()?, period.parse::<u64>().ok()?);
    (period > 0).then(|| quota.saturating_mul(1_000_000) / period)
}

fn tightest(chain: &[PathBuf], file: &str, rank: fn(&str) -> Option<u64>) -> Limit {
    let mut best: Option<(u64, String, &Path)> = None;
    for path in chain {
        let Some(text) = read(path, file) else {
            continue;
        };
        if let Some(value) = rank(&text)
            && best.as_ref().is_none_or(|(least, _, _)| value <= *least)
        {
            best = Some((value, text, path));
        }
    }
    match best {
        Some((_, text, path)) => Limit {
            value: Some(text),
            set_by: Some(shown(path)),
        },
        None => Limit {
            value: chain
                .iter()
                .any(|path| path.join(file).exists())
                .then(|| "max".to_owned()),
            set_by: None,
        },
    }
}

fn cpuset(chain: &[PathBuf]) -> Limit {
    let file = "cpuset.cpus.effective";
    let Some(own) = chain.iter().find_map(|path| read(path, file)) else {
        return Limit::default();
    };
    if read(Path::new(MOUNT), file).as_deref() == Some(own.as_str()) {
        return Limit {
            value: Some(own),
            set_by: None,
        };
    }
    let set_by = chain
        .iter()
        .rev()
        .find(|path| read(path, file).as_deref() == Some(own.as_str()))
        .map(|path| shown(path));
    Limit {
        value: Some(own),
        set_by,
    }
}

fn rlimits() -> BTreeMap<String, Rlimit> {
    let finite = |value: libc::rlim_t| (value != libc::RLIM_INFINITY).then_some(value);
    RLIMITS
        .iter()
        .filter_map(|name| {
            let resource = crate::process_policy::kernel::resource(name)?;
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            (unsafe { libc::getrlimit(resource, &mut limit) } == 0).then(|| {
                (
                    (*name).to_owned(),
                    Rlimit {
                        soft: finite(limit.rlim_cur),
                        hard: finite(limit.rlim_max),
                    },
                )
            })
        })
        .collect()
}

pub fn read_own() -> Surrounding {
    let own = crate::cgroup::Tree::own();
    let chain = own.as_deref().map(chain).unwrap_or_default();
    Surrounding {
        cgroup: own.as_deref().map(shown),
        discoverable: chain
            .first()
            .is_some_and(|path| path.join("cgroup.procs").exists()),
        memory_max: tightest(&chain, "memory.max", amount),
        memory_high: tightest(&chain, "memory.high", amount),
        cpu_max: tightest(&chain, "cpu.max", bandwidth),
        pids_max: tightest(&chain, "pids.max", amount),
        cpuset_cpus_effective: cpuset(&chain),
        rlimits: rlimits(),
    }
}

fn described(name: &str, limit: &Limit, bytes: bool) -> String {
    let value = match &limit.value {
        None => message("not discoverable", &[]),
        Some(text) if text == "max" || text.starts_with("max ") => message("unlimited", &[]),
        Some(text) if bytes => text.parse().map_or_else(|_| text.clone(), format_bytes),
        Some(text) => text.clone(),
    };
    match &limit.set_by {
        Some(path) => message(
            "{name} {value} set by {path}",
            &[
                ("name", name.to_owned()),
                ("value", value),
                ("path", path.clone()),
            ],
        ),
        None => format!("{name} {value}"),
    }
}

pub fn render(limits: &Surrounding) -> Vec<String> {
    let number =
        |value: Option<u64>| value.map_or_else(|| message("unlimited", &[]), |n| n.to_string());
    vec![
        message(
            "surrounding cgroup {cgroup}: {limits}",
            &[
                (
                    "cgroup",
                    limits
                        .cgroup
                        .clone()
                        .unwrap_or_else(|| message("not discoverable", &[])),
                ),
                (
                    "limits",
                    [
                        described("memory.max", &limits.memory_max, true),
                        described("memory.high", &limits.memory_high, true),
                        described("cpu.max", &limits.cpu_max, false),
                        described("pids.max", &limits.pids_max, false),
                        described(
                            "cpuset.cpus.effective",
                            &limits.cpuset_cpus_effective,
                            false,
                        ),
                    ]
                    .join("; "),
                ),
            ],
        ),
        message(
            "surrounding rlimits of the service (soft/hard): {limits}",
            &[(
                "limits",
                limits
                    .rlimits
                    .iter()
                    .map(|(name, limit)| {
                        format!("{name} {}/{}", number(limit.soft), number(limit.hard))
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            )],
        ),
    ]
}
