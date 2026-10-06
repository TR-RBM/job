use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

mod messages;
pub use messages::message;

pub const STARTS_PER_PASS: u32 = 1;
pub const BATCH_FROM: usize = 8;
const RESCAN_MS: u64 = 10_000;
const RUNNING_EVERY_MS: u64 = 1_000;
const STAGING: &str = "cancelling";
const YIELD_STEP: Duration = Duration::from_micros(200);
const YIELD_LIMIT: Duration = Duration::from_millis(20);

static SYNCS: AtomicU64 = AtomicU64::new(0);
static JOB_RECORDS: AtomicU64 = AtomicU64::new(0);
static LAST_CHANGE: Mutex<Option<Spent>> = Mutex::new(None);

thread_local! {
    static OWN: std::cell::Cell<(u64, u64)> = const { std::cell::Cell::new((0, 0)) };
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spent {
    pub action: String,
    pub sync_calls: u64,
    pub job_record_writes: u64,
}

pub fn request_begins() {
    OWN.with(|own| own.set((0, 0)));
}

pub fn request_spent(action: &str) {
    let (sync_calls, job_record_writes) = OWN.with(std::cell::Cell::get);
    *LAST_CHANGE.lock().unwrap_or_else(|p| p.into_inner()) = Some(Spent {
        action: action.to_owned(),
        sync_calls,
        job_record_writes,
    });
}

pub fn charge(syncs: u64) {
    OWN.with(|own| {
        let (sync_calls, records) = own.get();
        own.set((sync_calls + syncs, records));
    });
}
static BACKLOG: AtomicBool = AtomicBool::new(false);
static CANCELLATIONS_FAILING: AtomicU64 = AtomicU64::new(0);
static CANCELLATION_RETRIES: AtomicU64 = AtomicU64::new(0);
static STARTER_FAILURES: AtomicU64 = AtomicU64::new(0);
const SYNC_THREADS: usize = 64;
const RETRY_FIRST_MS: u64 = 250;
const RETRY_LONGEST_MS: u64 = 30_000;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    pub sync_calls: u64,
    pub job_record_writes: u64,
    pub starts_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_change: Option<Spent>,
}

pub fn activity() -> Activity {
    Activity {
        sync_calls: SYNCS.load(Ordering::Relaxed),
        job_record_writes: JOB_RECORDS.load(Ordering::Relaxed),
        starts_pending: BACKLOG.load(Ordering::Relaxed),
        last_change: LAST_CHANGE
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone(),
    }
}

pub fn job_record_written() {
    JOB_RECORDS.fetch_add(1, Ordering::Relaxed);
    OWN.with(|own| {
        let (syncs, records) = own.get();
        own.set((syncs, records + 1));
    });
}

pub fn file(file: &File) -> io::Result<()> {
    SYNCS.fetch_add(1, Ordering::Relaxed);
    charge(1);
    file.sync_all()
}

pub fn data(file: &File) -> io::Result<()> {
    SYNCS.fetch_add(1, Ordering::Relaxed);
    charge(1);
    file.sync_data()
}

pub fn data_shared(file: &File) -> io::Result<()> {
    SYNCS.fetch_add(1, Ordering::Relaxed);
    file.sync_data()
}

pub fn directory(path: &Path) -> io::Result<()> {
    file(&File::open(path)?)
}

pub fn together(files: &[&File], directories: &[&Path]) -> io::Result<()> {
    std::thread::scope(|scope| {
        let mut pending = Vec::new();
        for handle in files {
            pending.push(scope.spawn(move || file(handle)));
        }
        for path in directories {
            pending.push(scope.spawn(move || directory(path)));
        }
        let mut outcome = Ok(());
        for thread in pending {
            let result = thread
                .join()
                .unwrap_or_else(|_| Err(io::Error::other(message("a sync thread ended early"))));
            if outcome.is_ok() {
                outcome = result;
            }
        }
        charge((files.len() + directories.len()) as u64);
        outcome
    })
}

#[derive(Debug, Default)]
pub struct Launches {
    left: u32,
    brief: bool,
    deferred: bool,
    reasons: std::collections::HashMap<u64, Option<String>>,
}

impl Launches {
    pub fn begin(&mut self, allowed: u32, brief: bool) {
        self.left = allowed;
        self.brief = brief;
        self.deferred = false;
    }

    pub fn take(&mut self) -> bool {
        if self.left == 0 {
            self.deferred = true;
            return false;
        }
        self.left -= 1;
        true
    }

    pub fn brief(&self) -> bool {
        self.brief
    }

    pub fn wait_turn(&mut self, id: u64, reason: &mut Option<String>) {
        let turn = message("its turn to start");
        if reason.as_ref() != Some(&turn) {
            self.reasons.insert(id, reason.replace(turn));
        }
    }

    pub fn turn_came(&mut self, id: u64, reason: &mut Option<String>) {
        if let Some(earlier) = self.reasons.remove(&id)
            && reason.as_ref() == Some(&message("its turn to start"))
        {
            *reason = earlier;
        }
    }

    pub fn keep(&mut self, live: impl Fn(u64) -> bool) {
        self.reasons.retain(|id, _| live(*id));
    }

    pub fn settle(&mut self) -> bool {
        BACKLOG.store(self.deferred, Ordering::Relaxed);
        if self.deferred {
            TURNS.request_pass();
        }
        self.deferred
    }
}

pub static TURNS: Turns = Turns::new();

#[derive(Debug)]
pub struct Turns {
    waiting: AtomicUsize,
    wanted: Mutex<bool>,
    wake: Condvar,
}

pub struct Waiting<'a>(&'a Turns);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.waiting.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Turns {
    pub const fn new() -> Self {
        Self {
            waiting: AtomicUsize::new(0),
            wanted: Mutex::new(false),
            wake: Condvar::new(),
        }
    }

    pub fn arrive(&self) -> Waiting<'_> {
        self.waiting.fetch_add(1, Ordering::AcqRel);
        Waiting(self)
    }

    pub fn give_way(&self) {
        let mut waited = Duration::ZERO;
        while self.waiting.load(Ordering::Acquire) > 0 && waited < YIELD_LIMIT {
            std::thread::sleep(YIELD_STEP);
            waited += YIELD_STEP;
        }
    }

    pub fn request_pass(&self) {
        *self.wanted.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.wake.notify_all();
    }

    pub fn pass_requested(&self, timeout: Duration) -> bool {
        let mut wanted = self.wanted.lock().unwrap_or_else(|p| p.into_inner());
        if !*wanted && !timeout.is_zero() {
            wanted = self
                .wake
                .wait_timeout(wanted, timeout)
                .map(|(guard, _)| guard)
                .unwrap_or_else(|p| p.into_inner().0);
        }
        std::mem::take(&mut *wanted)
    }
}

#[derive(Debug)]
pub struct Group {
    state: Mutex<(u64, bool)>,
    done: Condvar,
}

impl Group {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new((0, false)),
            done: Condvar::new(),
        }
    }

    pub fn commit(
        &self,
        position: u64,
        written: impl Fn() -> u64,
        sync: impl Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if state.0 >= position {
                return Ok(());
            }
            if state.1 {
                state = self.done.wait(state).unwrap_or_else(|p| p.into_inner());
                continue;
            }
            state.1 = true;
            drop(state);
            let target = written();
            let result = sync();
            state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.1 = false;
            if result.is_ok() {
                state.0 = state.0.max(target);
            }
            self.done.notify_all();
            result?;
        }
    }

    pub fn covered(&self, position: u64) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.0 = state.0.max(position);
    }
}

pub fn staged_record(store: &crate::store::Store, id: u64) -> std::path::PathBuf {
    store.job_file(id).with_extension(STAGING)
}

fn each(paths: &[std::path::PathBuf]) -> io::Result<()> {
    let share = paths.len().div_ceil(SYNC_THREADS).max(1);
    charge(paths.len() as u64);
    std::thread::scope(|scope| {
        let workers: Vec<_> = paths
            .chunks(share)
            .map(|part| {
                scope.spawn(move || -> io::Result<()> {
                    for path in part {
                        directory(path)?;
                    }
                    Ok(())
                })
            })
            .collect();
        let mut outcome = Ok(());
        for worker in workers {
            let result = worker
                .join()
                .unwrap_or_else(|_| Err(io::Error::other(message("a sync thread ended early"))));
            if outcome.is_ok() {
                outcome = result;
            }
        }
        outcome
    })
}

pub fn sync_job_directories(store: &crate::store::Store, ids: &[u64]) -> io::Result<()> {
    let paths: Vec<_> = ids.iter().map(|id| store.job_dir(*id)).collect();
    each(&paths)
}

pub fn settle_records(store: &crate::store::Store, jobs: &[crate::model::Job]) -> io::Result<()> {
    let mut staged = Vec::with_capacity(jobs.len());
    let jobs: Vec<crate::model::Job> = jobs.iter().map(crate::store::revised).collect();
    let jobs = jobs.as_slice();
    let result = (|| -> io::Result<()> {
        for job in jobs {
            let staging = staged_record(store, job.id);
            crate::store::prepare(&staging, &serde_json::to_vec_pretty(job)?)?;
            staged.push(staging);
        }
        crate::durability::failpoint::fails("cancel-batch-sync")?;
        each(&staged)?;
        crate::durability::failpoint("cancel-batch-after-stage");
        for (staging, job) in staged.iter().zip(jobs) {
            crate::durability::failpoint::fails("cancel-batch-rename")?;
            crate::durability::failpoint("cancel-batch-mid-rename");
            std::fs::rename(staging, store.job_file(job.id))?;
            job_record_written();
        }
        sync_job_directories(store, &jobs.iter().map(|job| job.id).collect::<Vec<_>>())?;
        crate::durability::failpoint("cancel-batch-after-publish");
        Ok(())
    })();
    if let Err(error) = result {
        for staging in &staged {
            let _ = std::fs::remove_file(staging);
        }
        return Err(error);
    }
    for job in jobs {
        crate::store::observed(store, job);
    }
    Ok(())
}

pub fn sweep_staged(store: &crate::store::Store) -> usize {
    store
        .job_ids()
        .into_iter()
        .filter(|id| std::fs::remove_file(staged_record(store, *id)).is_ok())
        .count()
}

#[derive(Debug, Default)]
pub struct Retries {
    fresh: std::collections::HashSet<String>,
    failed: std::collections::HashMap<String, (u32, u64)>,
}

impl Retries {
    pub fn created(&mut self, operation: &str) {
        self.fresh.insert(operation.to_owned());
    }

    pub fn due(&self, operation: &str, now: u64) -> bool {
        self.failed
            .get(operation)
            .is_none_or(|(_, next)| now >= *next)
    }

    pub fn recovering(&self, operation: &str) -> bool {
        !self.fresh.contains(operation) || self.failed.contains_key(operation)
    }

    pub fn failure(&mut self, operation: &str, now: u64, error: &str) {
        let entry = self.failed.entry(operation.to_owned()).or_insert((0, now));
        entry.0 = entry.0.saturating_add(1);
        let wait = RETRY_FIRST_MS
            .saturating_mul(1u64 << entry.0.min(16))
            .min(RETRY_LONGEST_MS);
        entry.1 = now.saturating_add(wait);
        if entry.0 == 1 {
            eprintln!(
                "job daemon: {} {operation}: {error}",
                message("cancellation could not be completed and is tried again; operation")
            );
        }
        CANCELLATIONS_FAILING.store(self.failed.len() as u64, Ordering::Relaxed);
        CANCELLATION_RETRIES.fetch_add(1, Ordering::Relaxed);
    }

    pub fn done(&mut self, operation: &str) {
        self.fresh.remove(operation);
        self.failed.remove(operation);
        CANCELLATIONS_FAILING.store(self.failed.len() as u64, Ordering::Relaxed);
    }
}

pub fn cancellations_failing() -> (u64, u64) {
    (
        CANCELLATIONS_FAILING.load(Ordering::Relaxed),
        CANCELLATION_RETRIES.load(Ordering::Relaxed),
    )
}

pub fn starter_failed(what: &str) {
    if STARTER_FAILURES.fetch_add(1, Ordering::Relaxed) == 0 {
        eprintln!(
            "job daemon: {}: {what}",
            message(
                "the starter failed in one pass and goes on; further failures are only counted"
            )
        );
    }
}

pub fn starter_failures() -> u64 {
    STARTER_FAILURES.load(Ordering::Relaxed)
}

#[derive(Debug, Default)]
pub struct Trims {
    room: Option<u64>,
    grown: u64,
    scanned_ms: u64,
    running: u64,
    running_ms: Option<u64>,
}

impl Trims {
    pub fn running_stale(&self, now: u64) -> bool {
        self.running_ms
            .is_none_or(|at| now.saturating_sub(at) >= RUNNING_EVERY_MS)
    }

    pub fn running_measured(&mut self, bytes: u64, now: u64) {
        self.running = bytes;
        self.running_ms = Some(now);
    }

    pub fn due(&mut self, ended: u64, now: u64) -> bool {
        self.grown = self.grown.saturating_add(ended);
        self.room
            .is_none_or(|room| self.grown.saturating_add(self.running) >= room)
            || now.saturating_sub(self.scanned_ms) >= RESCAN_MS
    }

    pub fn scanned(&mut self, budget: u64, total: u64, now: u64) {
        self.room = Some(budget.saturating_sub(total));
        self.grown = 0;
        self.scanned_ms = now;
    }

    pub fn forget(&mut self) {
        self.room = None;
    }
}
