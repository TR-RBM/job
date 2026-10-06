use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::Job;

pub mod control;
mod messages;
pub mod notify;
pub mod replay;
pub use messages::{help, message};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Resource {
    Cpu,
    Memory,
    Io,
}

impl Resource {
    fn name(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Memory => "memory",
            Self::Io => "io",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum Reading {
    Available {
        avg10_bp: u16,
        avg60_bp: u16,
        avg300_bp: u16,
        total_us: u64,
    },
    Unavailable {
        reason: String,
    },
    Undefined,
}

fn unavailable(reason: impl ToString) -> Reading {
    Reading::Unavailable {
        reason: reason.to_string(),
    }
}

fn percentage(text: &str) -> Option<u16> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 2
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || (text.contains('.') && fraction.is_empty())
    {
        return None;
    }
    let fractional = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u16>().ok()? * if fraction.len() == 1 { 10 } else { 1 }
    };
    whole
        .parse::<u16>()
        .ok()?
        .checked_mul(100)?
        .checked_add(fractional)
        .filter(|v| *v <= 10000)
}

fn parse_metric(text: &str, metric: &str) -> Reading {
    let lines: Vec<_> = text
        .lines()
        .filter(|line| line.split_whitespace().next() == Some(metric))
        .collect();
    if lines.len() != 1 {
        return unavailable(message("PSI metric is missing or duplicated"));
    }
    let mut fields = BTreeMap::new();
    for field in lines[0].split_whitespace().skip(1) {
        let Some((key, value)) = field.split_once('=') else {
            return unavailable(message("invalid PSI metric"));
        };
        if fields.insert(key, value).is_some() {
            return unavailable(message("invalid PSI metric"));
        }
    }
    let parsed = (|| {
        let total = *fields.get("total")?;
        if total.is_empty() || !total.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(Reading::Available {
            avg10_bp: percentage(fields.get("avg10")?)?,
            avg60_bp: percentage(fields.get("avg60")?)?,
            avg300_bp: percentage(fields.get("avg300")?)?,
            total_us: total.parse().ok()?,
        })
    })();
    parsed.unwrap_or_else(|| unavailable(message("invalid PSI metric")))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub resource: Resource,
    pub path: Option<PathBuf>,
    pub some: Reading,
    pub full: Reading,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CgroupIdentity {
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub scope: String,
    pub job_id: Option<u64>,
    pub attempt: Option<u64>,
    pub cgroup: Option<CgroupIdentity>,
    pub sampled_at_ms: u64,
    pub sampled_boot_ms: Option<u64>,
    pub boot_id: Option<String>,
    pub observations: Vec<Observation>,
}

fn read_file(file: File) -> io::Result<String> {
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(io::Error::other(message(
            "PSI input exceeds the size limit",
        )));
    }
    String::from_utf8(bytes).map_err(io::Error::other)
}

fn open_relative(directory: &File, name: &str, writable: bool) -> io::Result<File> {
    let name = std::ffi::CString::new(name).map_err(io::Error::other)?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            (if writable {
                libc::O_RDWR
            } else {
                libc::O_RDONLY
            }) | libc::O_CLOEXEC
                | libc::O_NOFOLLOW
                | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn accounting_enabled(directory: &File) -> io::Result<()> {
    match open_relative(directory, "cgroup.pressure", false).and_then(read_file) {
        Ok(text) if text.trim() == "1" => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(io::Error::other(message(
            "cgroup pressure accounting is disabled or invalid",
        ))),
        Err(error) => Err(error),
    }
}

pub fn snapshot(job: Option<&Job>) -> Snapshot {
    let host = job.is_none();
    let mut result = Snapshot {
        schema_version: 1,
        scope: if host { "host" } else { "job" }.to_owned(),
        job_id: job.map(|j| j.id),
        attempt: job.map(|j| j.attempt),
        cgroup: None,
        sampled_at_ms: crate::shim::now_ms(),
        sampled_boot_ms: crate::admission::clock_ms().ok(),
        boot_id: crate::process::boot_id().ok(),
        observations: Vec::new(),
    };
    let directory = if let Some(job) = job {
        if job.spec.declared.on.is_some() {
            Err(message("remote workload PSI is unavailable locally"))
        } else if !job.state.active() {
            Err(message("PSI requires an active local workload"))
        } else if job.backend != crate::model::Backend::Cgroup {
            Err(message("workload cgroup PSI is unavailable"))
        } else if let Some(path) = &job.workload_cgroup {
            File::options()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(path)
                .map_err(|e| e.to_string())
                .and_then(|file| {
                    let meta = file.metadata().map_err(|e| e.to_string())?;
                    result.cgroup = Some(CgroupIdentity {
                        device: meta.dev(),
                        inode: meta.ino(),
                    });
                    Ok((path.clone(), file))
                })
        } else {
            Err(message("workload cgroup PSI is unavailable"))
        }
    } else {
        File::options()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open("/proc/pressure")
            .map(|file| (PathBuf::from("/proc/pressure"), file))
            .map_err(|e| e.to_string())
    };
    for resource in [Resource::Cpu, Resource::Memory, Resource::Io] {
        let name = if host {
            resource.name().to_owned()
        } else {
            format!("{}.pressure", resource.name())
        };
        let path = directory.as_ref().ok().map(|(path, _)| path.join(&name));
        let contents = directory
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|(_, file)| {
                (if host {
                    Ok(())
                } else {
                    accounting_enabled(file)
                })
                .and_then(|()| open_relative(file, &name, false))
                .and_then(read_file)
                .map_err(|e| e.to_string())
            });
        let some = contents
            .as_ref()
            .map_or_else(unavailable, |text| parse_metric(text, "some"));
        let full = if host && resource == Resource::Cpu {
            Reading::Undefined
        } else {
            contents
                .as_ref()
                .map_or_else(unavailable, |text| parse_metric(text, "full"))
        };
        result.observations.push(Observation {
            resource,
            path,
            some,
            full,
        });
    }
    result
}

impl Snapshot {
    pub fn describe(&self) -> String {
        let mut lines = vec![format!(
            "{}: {}; {}: {}",
            message("PSI scope"),
            self.job_id.map_or_else(
                || message("host"),
                |id| format!("Job {id}, {} {}", message("attempt"), self.attempt.unwrap())
            ),
            message("sample time (Unix milliseconds)"),
            self.sampled_at_ms.to_string()
        )];
        for observation in &self.observations {
            for (metric, reading) in [("some", &observation.some), ("full", &observation.full)] {
                let format = |bp: &u16| format!("{}.{:02}%", bp / 100, bp % 100);
                let value = match reading {
                    Reading::Available {
                        avg10_bp,
                        avg60_bp,
                        avg300_bp,
                        total_us,
                    } => format!(
                        "avg10={} avg60={} avg300={} total={total_us} us",
                        format(avg10_bp),
                        format(avg60_bp),
                        format(avg300_bp)
                    ),
                    Reading::Unavailable { reason } => {
                        format!("{}: {reason}", message("unavailable"))
                    }
                    Reading::Undefined => message("undefined for host CPU"),
                };
                lines.push(format!("{} {metric}: {value}", observation.resource.name()));
            }
        }
        lines.push(message("Pressure describes affected work, not the cause of contention. Observation changes no admission policy."));
        lines.join("\n")
    }
}

pub fn parse_file(path: &Path, resource: Resource, host: bool) -> io::Result<Observation> {
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let text = read_file(file)?;
    Ok(Observation {
        resource,
        path: Some(path.to_owned()),
        some: parse_metric(&text, "some"),
        full: if host && resource == Resource::Cpu {
            Reading::Undefined
        } else {
            parse_metric(&text, "full")
        },
    })
}
