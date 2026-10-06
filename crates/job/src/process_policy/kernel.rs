use super::{Affinity, Bound, Controls, Ids, Numa, Pair, message};
use crate::resource_policy::Limit;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;

const BITS: usize = super::MAX_ID as usize + 1;
const WORD_BITS: usize = libc::c_ulong::BITS as usize;
const WORDS: usize = BITS / WORD_BITS;
type Mask = Vec<libc::c_ulong>;

pub fn resource(name: &str) -> Option<libc::__rlimit_resource_t> {
    Some(match name {
        "as" => libc::RLIMIT_AS,
        "core" => libc::RLIMIT_CORE,
        "cpu" => libc::RLIMIT_CPU,
        "data" => libc::RLIMIT_DATA,
        "fsize" => libc::RLIMIT_FSIZE,
        "memlock" => libc::RLIMIT_MEMLOCK,
        "msgqueue" => libc::RLIMIT_MSGQUEUE,
        "nice" => libc::RLIMIT_NICE,
        "nofile" => libc::RLIMIT_NOFILE,
        "nproc" => libc::RLIMIT_NPROC,
        "rtprio" => libc::RLIMIT_RTPRIO,
        "rttime" => libc::RLIMIT_RTTIME,
        "sigpending" => libc::RLIMIT_SIGPENDING,
        "stack" => libc::RLIMIT_STACK,
        _ => return None,
    })
}
fn mask(ids: &Ids, words: usize) -> Mask {
    let mut out = vec![0; words];
    for &id in &ids.0 {
        out[id as usize / WORD_BITS] |= 1 << (id as usize % WORD_BITS);
    }
    out
}
fn ids(mask: &[libc::c_ulong]) -> Ids {
    Ids((0..mask.len() * WORD_BITS)
        .filter(|&n| mask[n / WORD_BITS] & (1 << (n % WORD_BITS)) != 0)
        .map(|n| n as u32)
        .collect())
}
fn check(result: libc::c_long) -> io::Result<()> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn affinity(out: &mut [libc::c_ulong]) -> io::Result<()> {
    check(
        unsafe { libc::sched_getaffinity(0, std::mem::size_of_val(out), out.as_mut_ptr().cast()) }
            as _,
    )
}
fn memory_nodes(out: &mut [libc::c_ulong]) -> io::Result<()> {
    check(unsafe {
        libc::syscall(
            libc::SYS_get_mempolicy,
            std::ptr::null_mut::<libc::c_int>(),
            out.as_mut_ptr(),
            (out.len() * WORD_BITS) as libc::c_ulong,
            std::ptr::null_mut::<libc::c_void>(),
            4 as libc::c_ulong,
        )
    })
}
fn context(error: impl std::fmt::Display) -> String {
    format!(
        "{}: {error}",
        message("requested process control is unavailable")
    )
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Applied {
    pub cpu_affinity: Option<Ids>,
    pub numa_policy: Option<Numa>,
    pub rlimits: BTreeMap<String, Pair>,
}

pub struct Prepared {
    cpu: Option<Mask>,
    cpu_readback: Mask,
    numa: Option<(libc::c_int, Mask, bool)>,
    numa_readback: Mask,
    limits: Vec<(libc::__rlimit_resource_t, libc::rlimit)>,
    pub applied: Option<Applied>,
}
impl Prepared {
    pub fn new(controls: &Controls) -> Result<Self, String> {
        controls.validate()?;
        let mut applied = Applied::default();
        let mut prepared = Self {
            cpu: None,
            cpu_readback: vec![0; WORDS],
            numa: None,
            numa_readback: vec![
                0;
                crate::cgroup::page_size() as usize
                    / std::mem::size_of::<libc::c_ulong>()
            ],
            limits: Vec::new(),
            applied: None,
        };
        if let Some(Affinity::Set(wanted)) = &controls.cpu_affinity {
            let cpu = mask(wanted, WORDS);
            affinity(&mut prepared.cpu_readback).map_err(context)?;
            if cpu
                .iter()
                .zip(&prepared.cpu_readback)
                .any(|(a, b)| a & !b != 0)
            {
                return Err(message("CPU affinity includes unavailable CPUs"));
            }
            applied.cpu_affinity = Some(wanted.clone());
            prepared.cpu = Some(cpu);
            prepared.cpu_readback.fill(0);
        }
        if let Some(numa) = &controls.numa_policy
            && numa.mode != "inherit"
        {
            memory_nodes(&mut prepared.numa_readback).map_err(context)?;
            let allowed = ids(&prepared.numa_readback);
            if numa
                .nodes
                .as_ref()
                .is_some_and(|wanted| wanted.0.iter().any(|id| !allowed.0.contains(id)))
            {
                return Err(message("NUMA policy includes unavailable memory nodes"));
            }
            let requested = numa
                .nodes
                .as_ref()
                .map(|ids| mask(ids, prepared.numa_readback.len()))
                .unwrap_or_else(|| vec![0; prepared.numa_readback.len()]);
            if let Some(wanted) = &numa.nodes {
                let memory = std::fs::read_to_string("/sys/devices/system/node/has_memory")
                    .map_err(context)?;
                let memory = Ids::try_from(memory.trim().to_owned())?;
                if wanted.0.iter().any(|id| !memory.0.contains(id)) {
                    return Err(message("NUMA policy includes unavailable memory nodes"));
                }
            }
            let mode = match numa.mode.as_str() {
                "default" => libc::MPOL_DEFAULT,
                "local" => libc::MPOL_LOCAL,
                "bind" => libc::MPOL_BIND,
                "interleave" => libc::MPOL_INTERLEAVE,
                "preferred" => libc::MPOL_PREFERRED,
                _ => return Err(message("invalid NUMA policy")),
            };
            let mode = mode
                | if numa.nodes.is_some() {
                    libc::MPOL_F_STATIC_NODES
                } else {
                    0
                };
            prepared.numa = Some((mode, requested, numa.nodes.is_some()));
            prepared.numa_readback.fill(0);
            applied.numa_policy = Some(numa.clone());
        }
        for (name, value) in controls.fields() {
            let Some(name) = name.strip_prefix("rlimit_") else {
                continue;
            };
            if value.is_null() {
                continue;
            }
            let pair: Pair = serde_json::from_value(value).map_err(context)?;
            if pair.soft == Bound::Inherit && pair.hard == Bound::Inherit {
                continue;
            }
            let resource = resource(name).ok_or_else(|| message("unsupported rlimit resource"))?;
            let mut inherited = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            check(unsafe { libc::getrlimit(resource, &mut inherited) } as _).map_err(context)?;
            let limit = pair.resolve(inherited)?;
            let bound = |n| {
                Bound::Limit(if n == libc::RLIM_INFINITY {
                    Limit::Unlimited
                } else {
                    Limit::Value(n)
                })
            };
            applied.rlimits.insert(
                name.to_owned(),
                Pair {
                    soft: bound(limit.rlim_cur),
                    hard: bound(limit.rlim_max),
                },
            );
            prepared.limits.push((resource, limit));
        }
        if applied != Applied::default() {
            prepared.applied = Some(applied);
        }
        Ok(prepared)
    }

    pub fn apply(&mut self) -> io::Result<()> {
        if let Some(cpu) = &self.cpu {
            check(unsafe {
                libc::sched_setaffinity(
                    0,
                    std::mem::size_of_val(cpu.as_slice()),
                    cpu.as_ptr().cast(),
                )
            } as _)?;
            affinity(&mut self.cpu_readback)?;
            if *cpu != self.cpu_readback {
                return Err(io::Error::from_raw_os_error(libc::ERANGE));
            }
        }
        if let Some((mode, mask, has_nodes)) = &self.numa {
            let pointer = if *has_nodes {
                mask.as_ptr()
            } else {
                std::ptr::null()
            };
            let maxnode = if *has_nodes {
                (mask.len() * WORD_BITS) as libc::c_ulong
            } else {
                0
            };
            check(unsafe { libc::syscall(libc::SYS_set_mempolicy, *mode, pointer, maxnode) })?;
            let mut observed = 0 as libc::c_int;
            check(unsafe {
                libc::syscall(
                    libc::SYS_get_mempolicy,
                    &mut observed,
                    self.numa_readback.as_mut_ptr(),
                    (self.numa_readback.len() * WORD_BITS) as libc::c_ulong,
                    std::ptr::null_mut::<libc::c_void>(),
                    0 as libc::c_ulong,
                )
            })?;
            if observed != *mode || self.numa_readback != *mask {
                return Err(io::Error::from_raw_os_error(libc::ERANGE));
            }
        }
        for (resource, wanted) in &self.limits {
            check(unsafe { libc::setrlimit(*resource, wanted) } as _)?;
            let mut observed = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            check(unsafe { libc::getrlimit(*resource, &mut observed) } as _)?;
            if observed.rlim_cur != wanted.rlim_cur || observed.rlim_max != wanted.rlim_max {
                return Err(io::Error::from_raw_os_error(libc::ERANGE));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub allowed_cpus: Option<Ids>,
    pub affinity_error: Option<String>,
    pub allowed_memory_nodes: Option<Ids>,
    pub numa_query_error: Option<String>,
    pub inherited_rlimits: BTreeMap<String, Pair>,
}
pub fn capabilities() -> Capabilities {
    let mut result = Capabilities::default();
    let mut buffer = vec![0; WORDS];
    match affinity(&mut buffer) {
        Ok(()) => result.allowed_cpus = Some(ids(&buffer)),
        Err(e) => result.affinity_error = Some(e.to_string()),
    }
    buffer = vec![0; crate::cgroup::page_size() as usize / std::mem::size_of::<libc::c_ulong>()];
    match memory_nodes(&mut buffer) {
        Ok(()) => result.allowed_memory_nodes = Some(ids(&buffer)),
        Err(e) => result.numa_query_error = Some(e.to_string()),
    }
    for key in Controls::default().fields().keys() {
        let Some(name) = key.strip_prefix("rlimit_") else {
            continue;
        };
        let mut inherited = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::getrlimit(resource(name).unwrap(), &mut inherited) } == 0 {
            let bound = |n| {
                Bound::Limit(if n == libc::RLIM_INFINITY {
                    Limit::Unlimited
                } else {
                    Limit::Value(n)
                })
            };
            result.inherited_rlimits.insert(
                name.to_owned(),
                Pair {
                    soft: bound(inherited.rlim_cur),
                    hard: bound(inherited.rlim_max),
                },
            );
        }
    }
    result
}
