use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::control::Metric;
use super::{Resource, message};

pub const WINDOW_US: u64 = 2_000_000;
pub const CAPACITY: usize = 256;
const RETRY_MS: u64 = 30_000;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Key {
    pub path: PathBuf,
    pub cgroup_identity: Option<(u64, u64)>,
    pub job_attempt: Option<(u64, u64)>,
    pub resource: Resource,
    pub metric: Metric,
    pub high_bp: u16,
}

impl Key {
    fn open(&self) -> io::Result<File> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other(message("invalid pressure monitor source")))?;
        let directory = File::options()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(parent)?;
        let metadata = directory.metadata()?;
        if self
            .cgroup_identity
            .is_some_and(|identity| identity != (metadata.dev(), metadata.ino()))
        {
            return Err(io::Error::other(message(
                "pressure monitor identity changed",
            )));
        }
        let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
        if unsafe { libc::fstatfs(directory.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let expected = if self.cgroup_identity.is_some() {
            libc::CGROUP2_SUPER_MAGIC
        } else {
            libc::PROC_SUPER_MAGIC
        };
        if unsafe { stat.assume_init() }.f_type != expected {
            return Err(io::Error::other(message(
                "invalid pressure monitor filesystem",
            )));
        }
        if self.cgroup_identity.is_some() {
            super::accounting_enabled(&directory)?;
        }
        let name = self
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| io::Error::other(message("invalid pressure monitor source")))?;
        let mut file = super::open_relative(&directory, name, true)?;
        let metric = match self.metric {
            Metric::Some => "some",
            Metric::Full => "full",
        };
        let command = format!(
            "{metric} {} {WINDOW_US}\0",
            self.high_bp as u64 * WINDOW_US / 10000
        );
        if file.write(command.as_bytes())? != command.len() {
            return Err(io::Error::other(message(
                "incomplete pressure monitor registration",
            )));
        }
        Ok(file)
    }
}

struct Entry {
    file: Option<Arc<File>>,
    rules: BTreeSet<String>,
    error: Option<String>,
    retry_boot_ms: Option<u64>,
    notifications: u64,
}

#[derive(Clone)]
pub struct Source {
    key: Key,
    file: Arc<File>,
}

pub struct Notice {
    source: Source,
    failed: bool,
}

pub fn wait(sources: Vec<Source>, timeout: Duration) -> Vec<Notice> {
    let mut polls: Vec<_> = sources
        .iter()
        .map(|s| libc::pollfd {
            fd: s.file.as_raw_fd(),
            events: libc::POLLPRI,
            revents: 0,
        })
        .collect();
    let ready = unsafe {
        libc::poll(
            polls.as_mut_ptr(),
            polls.len() as libc::nfds_t,
            timeout.as_nanos().div_ceil(1_000_000).min(i32::MAX as u128) as i32,
        )
    };
    if ready < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
        return Vec::new();
    }
    sources
        .into_iter()
        .zip(polls)
        .filter_map(|(source, poll)| {
            let failed =
                ready < 0 || poll.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0;
            (failed || poll.revents & libc::POLLPRI != 0).then_some(Notice { source, failed })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Monitor {
    pub source: Key,
    pub rules: Vec<String>,
    pub registered: bool,
    pub error: Option<String>,
    pub retry_boot_ms: Option<u64>,
    pub notifications: u64,
    pub threshold_us: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub window_us: u64,
    pub capacity: usize,
    pub omitted_sources: usize,
    pub notifications: u64,
    pub monitors: Vec<Monitor>,
}

#[derive(Default)]
pub struct Registry {
    entries: BTreeMap<Key, Entry>,
    omitted: usize,
    notifications: u64,
}

impl Registry {
    pub fn sync(&mut self, candidates: BTreeMap<Key, BTreeSet<String>>, now: u64) {
        self.omitted = candidates.len().saturating_sub(CAPACITY);
        let selected: BTreeMap<_, _> = candidates.into_iter().take(CAPACITY).collect();
        self.entries.retain(|key, _| selected.contains_key(key));
        for (key, rules) in selected {
            let entry = self.entries.entry(key.clone()).or_insert(Entry {
                file: None,
                rules: BTreeSet::new(),
                error: None,
                retry_boot_ms: None,
                notifications: 0,
            });
            entry.rules = rules;
            if entry.file.is_some() || entry.retry_boot_ms.is_some_and(|retry| now < retry) {
                continue;
            }
            match key.open() {
                Ok(file) => {
                    entry.file = Some(Arc::new(file));
                    entry.error = None;
                    entry.retry_boot_ms = None;
                }
                Err(error) => {
                    entry.error = Some(error.to_string());
                    entry.retry_boot_ms = Some(now.saturating_add(RETRY_MS));
                }
            }
        }
    }

    pub fn sources(&self) -> Vec<Source> {
        self.entries
            .iter()
            .filter_map(|(key, entry)| {
                Some(Source {
                    key: key.clone(),
                    file: entry.file.clone()?,
                })
            })
            .collect()
    }

    pub fn accept(&mut self, notices: Vec<Notice>, now: u64) -> bool {
        let mut changed = false;
        for notice in notices {
            let Some(entry) = self.entries.get_mut(&notice.source.key).filter(|e| {
                e.file
                    .as_ref()
                    .is_some_and(|file| Arc::ptr_eq(file, &notice.source.file))
            }) else {
                continue;
            };
            changed = true;
            if notice.failed {
                entry.file = None;
                entry.error = Some(message("pressure monitor source closed or polling failed"));
                entry.retry_boot_ms = Some(now.saturating_add(RETRY_MS));
            } else {
                entry.notifications = entry.notifications.saturating_add(1);
                self.notifications = self.notifications.saturating_add(1);
            }
        }
        changed
    }

    pub fn report(&self) -> Report {
        Report {
            window_us: WINDOW_US,
            capacity: CAPACITY,
            omitted_sources: self.omitted,
            notifications: self.notifications,
            monitors: self
                .entries
                .iter()
                .map(|(key, entry)| Monitor {
                    source: key.clone(),
                    rules: entry.rules.iter().cloned().collect(),
                    registered: entry.file.is_some(),
                    error: entry.error.clone(),
                    retry_boot_ms: entry.retry_boot_ms,
                    notifications: entry.notifications,
                    threshold_us: key.high_bp as u64 * WINDOW_US / 10000,
                })
                .collect(),
        }
    }
}
