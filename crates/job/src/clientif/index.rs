use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::model::Job;
use crate::store::Store;

use super::row::{Row, STATES};
use super::{changes, confirmed};

pub const ENDED: usize = 50_000;
const RECHECK: Duration = Duration::from_secs(1);

struct Live {
    revision: u64,
    attempt: u64,
    confirmed: Option<Value>,
    checked: Option<Instant>,
}

pub struct Index {
    live: HashMap<u64, Live>,
    ended: BTreeMap<u64, Arc<Row>>,
    recent: BTreeSet<(u64, u64)>,
    outside: HashMap<u64, usize>,
    counts: [u64; STATES.len()],
    limit: usize,
}

static INDEX: Mutex<Option<Index>> = Mutex::new(None);

pub struct Guard(MutexGuard<'static, Option<Index>>);

impl std::ops::Deref for Guard {
    type Target = Index;
    fn deref(&self) -> &Index {
        self.0.as_ref().expect("the index is created when locked")
    }
}

impl std::ops::DerefMut for Guard {
    fn deref_mut(&mut self) -> &mut Index {
        self.0.as_mut().expect("the index is created when locked")
    }
}

fn limit() -> usize {
    std::env::var("JOB_CLIENT_INDEX_ENDED")
        .ok()
        .and_then(|text| text.parse::<usize>().ok())
        .filter(|limit| *limit >= 1)
        .unwrap_or(ENDED)
}

pub fn bound() -> usize {
    locked().limit
}

pub fn locked() -> Guard {
    let mut guard = INDEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        *guard = Some(Index {
            live: HashMap::new(),
            ended: BTreeMap::new(),
            recent: BTreeSet::new(),
            outside: HashMap::new(),
            counts: [0; STATES.len()],
            limit: limit(),
        });
    }
    Guard(guard)
}

fn placed(row: &Row) -> usize {
    STATES
        .iter()
        .position(|state| *state == row.state)
        .unwrap_or(STATES.len())
}

fn unconfirmed(job: &Job) -> bool {
    job.started_ms.is_none() && job.state.active() && job.state != crate::model::State::Starting
}

impl Index {
    pub fn started(&self, store: &Store, job: &Job) -> (Option<u64>, u64) {
        let revision = self.revision(job);
        if !unconfirmed(job) {
            return (job.started_ms, revision);
        }
        match crate::durability::launch::confirmed_at(store, job) {
            Some(started) => (Some(started), revision + 1),
            None => (None, revision),
        }
    }

    fn known(&self, id: u64) -> u64 {
        self.live
            .get(&id)
            .map(|live| live.revision)
            .or_else(|| self.ended.get(&id).map(|row| row.revision))
            .unwrap_or(0)
    }

    fn leave(&mut self, id: u64) {
        if let Some(row) = self.ended.remove(&id) {
            self.recent.remove(&(row.finished_ms.unwrap_or(0), id));
            self.counted(placed(&row), false);
        }
        if let Some(state) = self.outside.remove(&id) {
            self.counted(state, false);
        }
    }

    fn counted(&mut self, state: usize, more: bool) {
        if let Some(count) = self.counts.get_mut(state) {
            *count = if more {
                *count + 1
            } else {
                count.saturating_sub(1)
            };
        }
    }

    pub fn counts(&self) -> [u64; STATES.len()] {
        self.counts
    }

    fn enter(&mut self, row: Arc<Row>) {
        self.leave(row.id);
        self.live.remove(&row.id);
        self.recent.insert((row.finished_ms.unwrap_or(0), row.id));
        self.counted(placed(&row), true);
        self.ended.insert(row.id, row);
        while self.ended.len() > self.limit {
            let Some((_, oldest)) = self.recent.pop_first() else {
                break;
            };
            if let Some(row) = self.ended.remove(&oldest) {
                self.outside.insert(oldest, placed(&row));
            }
        }
    }

    pub fn revision(&self, job: &Job) -> u64 {
        self.known(job.id).max(job.durability.revision.unwrap_or(0))
    }

    pub fn confirmed(&mut self, store: &Store, job: &Job) -> Value {
        if !confirmed::started(job) && job.state != crate::model::State::Starting {
            return confirmed::absent(job);
        }
        let revision = self.revision(job);
        let live = self.live.entry(job.id).or_insert(Live {
            revision,
            attempt: job.attempt,
            confirmed: None,
            checked: None,
        });
        if live.attempt != job.attempt {
            live.attempt = job.attempt;
            live.confirmed = None;
            live.checked = None;
        }
        if let Some(known) = &live.confirmed {
            return known.clone();
        }
        if live.checked.is_some_and(|at| at.elapsed() < RECHECK) {
            return confirmed::absent(job);
        }
        live.checked = Some(Instant::now());
        match confirmed::read(&store.job_dir(job.id), job) {
            Some(read) => {
                let short = confirmed::short(&read);
                live.confirmed = Some(short.clone());
                short
            }
            None => confirmed::absent(job),
        }
    }

    pub fn ended(&self) -> impl Iterator<Item = &Arc<Row>> {
        self.ended.values()
    }

    pub fn outside(&self) -> usize {
        self.outside.len()
    }
}

pub fn revised(job: &Job) -> u64 {
    locked().revision(job) + 1
}

pub fn observe(store: &Store, job: &Job) {
    let mut index = locked();
    let revision = index.revision(job);
    if job.state.terminal() {
        let read = confirmed::read(&store.job_dir(job.id), job);
        let short = read
            .as_ref()
            .map_or_else(|| confirmed::absent(job), confirmed::short);
        let row = Arc::new(Row::of(job, revision, None, short));
        index.enter(Arc::clone(&row));
        changes::job(&row);
        return;
    }
    index.leave(job.id);
    match index.live.get_mut(&job.id) {
        Some(live) if live.attempt == job.attempt => live.revision = revision,
        _ => {
            index.live.insert(
                job.id,
                Live {
                    revision,
                    attempt: job.attempt,
                    confirmed: None,
                    checked: None,
                },
            );
        }
    }
    if !changes::wanted() {
        return;
    }
    if !unconfirmed(job) {
        let short = index.confirmed(store, job);
        changes::job(&Row::of(job, revision, None, short));
    }
}

pub fn learned(store: &Store, job: &Job) {
    let mut index = locked();
    let revision = index.revision(job) + 1;
    match index.live.get_mut(&job.id) {
        Some(live) => live.revision = revision,
        None => {
            index.live.insert(
                job.id,
                Live {
                    revision,
                    attempt: job.attempt,
                    confirmed: None,
                    checked: None,
                },
            );
        }
    }
    if changes::wanted() {
        let short = index.confirmed(store, job);
        changes::job(&Row::of(job, revision, None, short));
    }
}

pub fn removed(id: u64) {
    let mut index = locked();
    index.live.remove(&id);
    index.leave(id);
    changes::job_removed(id);
}
