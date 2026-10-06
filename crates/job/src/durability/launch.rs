use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::cgroup::Tree;
use crate::model::{Job, ShimResult, State};
use crate::store::{Store, read_json};

const GO: u8 = b'g';
const ATTEMPTS: u32 = 3;

static FAILURES: Mutex<Option<HashMap<(u64, u64), u32>>> = Mutex::new(None);

#[derive(Serialize, Deserialize)]
struct Started {
    attempt: u64,
    at_ms: u64,
}

pub struct Gate(io::PipeWriter);

impl Gate {
    pub fn open(mut self) -> io::Result<()> {
        self.0.write_all(&[GO])
    }
}

pub fn gate(command: &mut Command) -> io::Result<(Gate, io::PipeReader)> {
    let (reader, writer) = io::pipe()?;
    let fd = reader.as_raw_fd();
    command.arg("--gate").arg(fd.to_string());
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok((Gate(writer), reader))
}

fn marker(store: &Store, id: u64) -> std::path::PathBuf {
    store.job_dir(id).join("started.json")
}

pub fn admitted(store: &Store, id: u64, gate: Option<&str>) -> bool {
    let Some(fd) = gate.and_then(|text| text.parse::<i32>().ok()) else {
        return true;
    };
    let mut pipe = unsafe { fs::File::from_raw_fd(fd) };
    let mut byte = [0u8; 1];
    if pipe.read_exact(&mut byte).is_err() || byte[0] != GO {
        return false;
    }
    drop(pipe);
    super::failpoint::pause("launch-confirm-delay");
    let Some(job) = store.load_job(id) else {
        return false;
    };
    match super::write_in_boot(
        &marker(store, id),
        &Started {
            attempt: job.attempt,
            at_ms: crate::shim::now_ms(),
        },
    ) {
        Ok(()) => true,
        Err(error) => {
            eprintln!(
                "{}: {error}",
                super::message("cannot record the confirmed launch; the command was not started")
            );
            false
        }
    }
}

pub fn probe(alive: io::Result<bool>) -> Option<bool> {
    if super::failpoint::fails("liveness-unknown").is_err() {
        return None;
    }
    match alive {
        Ok(alive) => Some(alive),
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Some(false),
        Err(_) => None,
    }
}

fn confirmed(store: &Store, job: &Job) -> bool {
    read_json::<Started>(&marker(store, job.id))
        .is_some_and(|started| started.attempt == job.attempt)
}

pub fn confirmed_at(store: &Store, job: &Job) -> Option<u64> {
    read_json::<Started>(&marker(store, job.id))
        .filter(|started| started.attempt == job.attempt)
        .map(|started| started.at_ms)
}

fn populated(job: &Job) -> bool {
    job.workload_cgroup
        .as_ref()
        .is_some_and(|path| Tree::is_populated(path))
}

pub fn never_started(store: &Store, job: &Job) -> bool {
    job.durability.launch_gated
        && job.stop.is_none()
        && !populated(job)
        && match job.state {
            State::Starting => job.shim_pid.is_none(),
            State::Running => {
                job.supervisor_boot_id == crate::process::boot_id().ok() && !confirmed(store, job)
            }
            _ => false,
        }
}

pub fn reset(job: &mut Job) {
    job.state = State::Queued;
    job.started_ms = None;
    job.supervisor_boot_id = None;
    job.shim_pid = None;
    job.shim_start_ticks = None;
    job.workload_cgroup = None;
    job.aggregate_domains.clear();
    job.applied_resources.clear();
    job.effective_spec = None;
    job.admission_snapshot = None;
    job.link = None;
    job.durability.launch_gated = false;
    job.durability.admitted_ms = None;
}

pub fn retry(job: &Job) -> bool {
    let mut failures = FAILURES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let counts = failures.get_or_insert_with(HashMap::new);
    let count = counts.entry((job.id, job.attempt)).or_insert(0);
    *count += 1;
    if *count < ATTEMPTS {
        true
    } else {
        counts.remove(&(job.id, job.attempt));
        false
    }
}

pub fn failed(error: &str, now: u64) -> ShimResult {
    ShimResult {
        start_error: Some(format!(
            "{}: {error}",
            super::message("the launch could not be confirmed; the command never started")
        )),
        finished_ms: now,
        ..ShimResult::default()
    }
}

pub fn unconfirmed() -> String {
    super::message("its supervisor ended before it confirmed the launch")
}
