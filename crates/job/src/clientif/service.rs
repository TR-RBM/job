use std::io;

use super::{Shared, respond};
use crate::model::{Backend, Env, Request, Response};

pub struct Facts {
    pub backend: Backend,
    pub service: Option<crate::service::Info>,
}

pub enum Saved {
    Missing,
    Unreadable,
    Kept(Env),
}

pub fn passed(shared: &Shared, line: &[u8]) -> Response {
    let response = crate::durability::exchange(line, |request| respond(shared, request));
    crate::durability::audit::flush();
    crate::operations::journal::flush();
    response
}

pub fn carried(shared: &Shared, request: &Request) -> io::Result<Response> {
    let line = serde_json::to_vec(&crate::durability::negotiation::envelope(request))?;
    let bytes = crate::netsecret::rendered(&passed(shared, &line))?;
    serde_json::from_slice(&bytes)
        .map(crate::durability::negotiation::understood)
        .map_err(io::Error::other)
}

pub fn queried(shared: &Shared, id: u64, attempt: Option<u64>, query: &[String]) -> Response {
    let store = shared
        .daemon
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .store
        .clone();
    crate::operations::output::queried(&store, id, attempt, query)
}

pub fn facts(shared: &Shared) -> Facts {
    let daemon = shared
        .daemon
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Facts {
        backend: daemon.backend,
        service: crate::service::published(),
    }
}

pub fn saved(shared: &Shared, id: u64) -> Saved {
    let daemon = shared
        .daemon
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if daemon.load(id).is_none() {
        return Saved::Missing;
    }
    match daemon.store.load_env(id) {
        Some(env) => Saved::Kept(env),
        None => Saved::Unreadable,
    }
}

pub struct Held {
    pub states: [u64; 6],
    pub pool: crate::resources::Vector,
    pub reserved: crate::resources::Vector,
    pub budget: u64,
    pub store: crate::store::Store,
    pub backend: Backend,
}

pub fn held(seen: &Seen<'_>) -> Held {
    use crate::model::State;
    let mut states = [0u64; 6];
    let mut reserved = crate::resources::Vector {
        cores_milli: 0,
        memory: 0,
        pids: 0,
    };
    for job in seen.jobs.values() {
        let at = match job.state {
            State::Held => 0,
            State::Queued => 1,
            State::Starting => 2,
            State::Running => 3,
            State::Suspended => 4,
            State::Stopping => 5,
            _ => continue,
        };
        states[at] += 1;
        if job.state.active() {
            let vector = &job.reservation.vector;
            reserved.cores_milli = reserved.cores_milli.saturating_add(vector.cores_milli);
            reserved.memory = reserved.memory.saturating_add(vector.memory);
            reserved.pids = reserved.pids.saturating_add(vector.pids);
        }
    }
    Held {
        states,
        pool: seen.pool,
        reserved,
        budget: seen.budget,
        store: seen.store.clone(),
        backend: seen.backend,
    }
}

pub struct Seen<'a> {
    pub jobs: &'a std::collections::BTreeMap<u64, crate::model::Job>,
    pub predicted: &'a std::collections::HashMap<u64, Option<u64>>,
    pub objects: &'a crate::objects::Graph,
    pub tree: Option<&'a crate::cgroup::Tree>,
    pub store: &'a crate::store::Store,
    pub pool: crate::resources::Vector,
    pub budget: u64,
    pub backend: Backend,
}

pub fn seen<T>(shared: &Shared, take: impl FnOnce(&Seen<'_>) -> T) -> T {
    let daemon = shared
        .daemon
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    take(&Seen {
        jobs: &daemon.jobs,
        predicted: &daemon.predicted_starts,
        objects: &daemon.objects,
        tree: daemon.tree.as_ref(),
        store: &daemon.store,
        pool: daemon.pool,
        budget: daemon.config.output.budget(),
        backend: daemon.backend,
    })
}

pub struct Sample {
    pub peak_memory: u64,
    pub peak_pids: u64,
    pub written: u64,
}

pub struct Loaded {
    pub job: crate::model::Job,
    pub learned: bool,
    pub predicted: Option<u64>,
    pub subject: Subject,
    pub store: crate::store::Store,
    pub backend: Backend,
}

pub enum Found {
    Missing,
    Unreadable,
    Job(Box<Loaded>),
}

pub fn found(shared: &Shared, id: u64) -> Found {
    let daemon = shared
        .daemon
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(job) = daemon.load(id) else {
        return if daemon.store.job_dir(id).is_dir() {
            Found::Unreadable
        } else {
            Found::Missing
        };
    };
    Found::Job(Box::new(Loaded {
        learned: job.started_ms.is_some()
            && daemon
                .jobs
                .get(&id)
                .is_some_and(|kept| kept.started_ms.is_none()),
        predicted: daemon.predicted_starts.get(&id).copied().flatten(),
        subject: subject(&daemon, &job),
        store: daemon.store.clone(),
        backend: daemon.backend,
        job,
    }))
}

pub struct Subject {
    pub id: u64,
    pub attempt: u64,
    pub cgroup: Option<std::path::PathBuf>,
    pub supervisor: Option<i32>,
    pub sample: Option<Sample>,
}

fn subject(daemon: &super::Daemon, job: &crate::model::Job) -> Subject {
    Subject {
        id: job.id,
        attempt: job.attempt,
        cgroup: job
            .workload_cgroup
            .clone()
            .filter(|_| daemon.backend == Backend::Cgroup),
        supervisor: daemon
            .supervisors
            .get(&job.id)
            .map(|handle| handle.pid)
            .or(job.shim_pid),
        sample: daemon.watched.get(&job.id).map(|watched| Sample {
            peak_memory: watched.peak_memory,
            peak_pids: watched.peak_pids,
            written: watched.written,
        }),
    }
}

pub fn subjects(shared: &Shared, ids: &std::collections::BTreeSet<u64>) -> Vec<Subject> {
    let daemon = shared
        .daemon
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ids.iter()
        .filter_map(|id| daemon.jobs.get(id))
        .filter(|job| job.state.active())
        .map(|job| subject(&daemon, job))
        .collect()
}
