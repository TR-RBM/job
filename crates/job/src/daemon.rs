use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::cgroup::{Limits, Tree};
use crate::estimate::{self, Context, HISTORY_WINDOW};
use crate::host;
use crate::model::{
    Backend, Env, HistoryEntry, HostInfo, Job, Net, Parallel, Queue, QueueChange, QueueEntry,
    QueueView, Request, Response, Settings, ShimResult, Spec, State, Stop, StopKind,
};
use crate::procs;
use crate::resources::{MILLI, Vector, format_cores};
use crate::schedule::{self, Candidate, Lane, Occupant, Placement, QueueHold};
use crate::shim::now_ms;
use crate::store::Store;
use crate::units::{format_bytes, format_duration_ms};

pub const TICK: Duration = Duration::from_millis(250);
pub(crate) const GRACE_MS: u64 = 10_000;
const PRESSURE_LIMIT_PERCENT: f64 = 60.0;
const PRESSURE_DURATION_MS: u64 = 30_000;
const RESULT_POLL_MS: u64 = 250;
const REMOTE_PIDS: u64 = 64;
const OWN_IMAGE: &str = "/proc/self/exe";

#[derive(Default)]
struct Watched {
    memory: u64,
    peak_memory: u64,
    peak_pids: u64,
    written: u64,
}

pub struct Daemon {
    boot_id: String,
    supervisors: HashMap<u64, crate::process::Handle>,
    config: crate::config::Config,
    config_source: Option<PathBuf>,
    objects: crate::objects::Graph,
    ordering: crate::admission::Ledger,
    ordering_error: Option<String>,
    pressure_control: crate::pressure::control::Controller,
    store: Store,
    backend: Backend,
    tree: Option<Tree>,
    cores: u64,
    pool: Vector,
    devices: Vec<String>,
    jobs: BTreeMap<u64, Job>,
    cancellations: BTreeMap<String, crate::cancellation::Operation>,
    resource_updates: BTreeMap<String, crate::resource_update::Operation>,
    resource_worker: Option<std::thread::JoinHandle<io::Result<()>>>,
    queues: BTreeMap<String, Queue>,
    links: BTreeMap<String, crate::link::Link>,
    unisolated_links: std::collections::BTreeSet<String>,
    history: HashMap<String, Vec<HistoryEntry>>,
    watched: HashMap<u64, Watched>,
    limits: HashMap<u64, Limits>,
    terminating: HashMap<u64, u64>,
    predicted_starts: HashMap<u64, Option<u64>>,
    pressure_since: Option<u64>,
    host_stop_pending: Option<u64>,
    host_cores_written: Option<Option<u64>>,
    launches: crate::pacing::Launches,
    pass_memory: std::cell::Cell<Option<host::MemInfo>>,
    trims: crate::pacing::Trims,
    cancel_retries: crate::pacing::Retries,
}

#[path = "cli2/service.rs"]
mod cli2_service;
#[path = "clientif/service.rs"]
pub mod client_service;
#[path = "idset/service.rs"]
mod idset_service;

pub struct Shared {
    daemon: Mutex<Daemon>,
    changed: Condvar,
}

fn cgroup_of(tree: &Option<Tree>, job: &Job) -> Option<PathBuf> {
    tree.as_ref()
        .map(|tree| crate::aggregate::leaf(tree, &job.aggregate_domains, job.id))
}

impl Daemon {
    fn checkpoint_ordering(&mut self) -> io::Result<()> {
        let result =
            self.ordering
                .checkpoint(&self.store, &self.objects, &self.jobs, &self.boot_id);
        self.ordering_error = result.as_ref().err().map(ToString::to_string);
        result
    }

    fn ordered_plan(&self, now: u64) -> Vec<crate::admission::planner::Decision> {
        let gates = self
            .jobs
            .values()
            .filter(|j| j.state == State::Queued)
            .map(|j| (j.id, self.admissible_now(j)))
            .collect();
        crate::admission::planner::plan(
            &self.objects,
            &self.ordering,
            &self.jobs,
            self.pool,
            now,
            &gates,
        )
    }

    fn explain(&self, job: &Job) -> crate::admission::Explanation {
        if !job.state.editable()
            && let Some(mut snapshot) = job.admission_snapshot.clone()
        {
            snapshot.state = job.state.clone();
            snapshot.blocking_reason = None;
            snapshot.fifo_predecessor = None;
            snapshot.network = job.network.clone();
            return snapshot;
        }
        let queue = job.queue_id.unwrap_or(crate::objects::DEFAULT_QUEUE);
        let plan = self.ordered_plan(now_ms());
        let decision = plan.iter().find(|d| d.id == job.id);
        crate::admission::Explanation {
            schema_version: 1,
            id: job.id,
            attempt: job.attempt,
            state: job.state.clone(),
            ordering_enabled: crate::admission::ordered(&self.objects, job),
            priority: job.spec.declared.priority,
            priority_source: job.priority_source.clone(),
            bounds: crate::admission::described_bounds(&self.objects, queue),
            eligible_wait_ms: self.ordering.credit(job),
            aging: crate::admission::aging(&self.objects, queue),
            effective_priority: self.ordering.score(&self.objects, job).to_string(),
            fifo_predecessor: crate::admission::fifo_predecessor(&self.objects, &self.jobs, job),
            backfill: crate::admission::backfill(&self.objects, queue, job.policy.legacy()),
            blocking_reason: (job.state == State::Queued)
                .then(|| {
                    self.ordering_error
                        .clone()
                        .or_else(|| decision.and_then(|d| d.reason.clone()))
                        .or_else(|| job.waited_for.clone())
                })
                .flatten(),
            predicted_start_ms: decision.and_then(|d| d.start),
            caveat: crate::admission::message(
                "Priority and aging do not change kernel CPU or I/O weights. Predictions are advisory; indefinite running work or persistent holds can prevent a start.",
            ),
            fair_share: self.ordering.fair.explain(&self.objects, queue),
            pressure: self.pressure_control.views(&self.objects, job),
            network: crate::netpolicy::view(&self.config.network, job),
        }
    }

    fn reprioritize(&mut self, id: u64, attempt: u64, priority: i32) -> io::Result<Job> {
        self.checkpoint_ordering()?;
        let mut job = self
            .load(id)
            .ok_or_else(|| io::Error::other(crate::admission::message("no such Job")))?;
        if job.attempt != attempt {
            return Err(io::Error::other(crate::admission::message(
                "attempt changed before reprioritization",
            )));
        }
        if !job.state.editable() {
            return Err(io::Error::other(crate::admission::message(
                "only held or queued Jobs can be reprioritized",
            )));
        }
        crate::admission::bounds(&self.objects, job.queue_id.unwrap(), priority)
            .map_err(io::Error::other)?;
        job.priority_changes.push(crate::admission::PriorityChange {
            attempt,
            actor_uid: crate::service::access::actor_uid(),
            at_ms: now_ms(),
            before: job.spec.declared.priority,
            after: priority,
        });
        job.spec.declared.priority = Some(priority);
        if let Some(spec) = &mut job.requested_spec {
            spec.declared.priority = Some(priority);
        }
        job.priority_source = Some(crate::resource_policy::Origin::Job);
        self.store.save_job(&job)?;
        self.jobs.insert(id, job.clone());
        self.admit(now_ms());
        Ok(job)
    }

    fn update_pending(&self) -> bool {
        self.resource_updates.values().any(|op| op.pending())
    }

    fn accepts_update(&self, path: &Path, file: &str, actual: &str) -> bool {
        self.resource_updates
            .values()
            .any(|op| op.accepts(path, file, actual))
    }

    fn changed_limits(&self, path: &Path, limits: &Limits) -> Option<(String, String, String)> {
        if !self.update_pending() {
            return Tree::changed_limit(path, limits);
        }
        limits.files().into_iter().find_map(|(file, expected)| {
            let actual = crate::cgroup::changed_control(path, file, expected)?;
            (!self.accepts_update(path, file, &actual))
                .then(|| (file.to_owned(), expected.to_owned(), actual))
        })
    }

    fn changed_domains(&self, tree: &Tree, domains: &[crate::aggregate::Domain]) -> Option<String> {
        if !self.update_pending() {
            return crate::aggregate::changed(tree, domains);
        }
        crate::aggregate::changed_except(tree, domains, |path, file, actual| {
            self.accepts_update(path, file, actual)
        })
    }

    fn reconcile_resource_updates(&mut self, start_worker: bool) -> io::Result<()> {
        if self.resource_worker.is_none() && !self.update_pending() {
            return Ok(());
        }
        if self
            .resource_worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            return Ok(());
        }
        if let Some(worker) = self.resource_worker.take() {
            let outcome = worker.join().map_err(|_| {
                io::Error::other(crate::resource_update::message(
                    "resource update is pending",
                ))
            })?;
            self.resource_updates = crate::resource_update::load(&self.store)?;
            outcome?;
        }
        let mut published = Vec::new();
        for operation in self
            .resource_updates
            .values_mut()
            .filter(|op| op.phase == crate::resource_update::Phase::Verified)
        {
            let mut next = operation.clone();
            if let Err(error) = crate::resource_update::publish(&self.store, &mut next) {
                operation.error = Some(error.to_string());
                let _ = crate::resource_update::save(&self.store, operation);
                return Err(error);
            }
            published.push(next);
        }
        if !published.is_empty() {
            self.objects = crate::objects::Graph::load(&self.store)?;
            self.queues = self.objects.queues();
            for job in self.jobs.values_mut() {
                crate::resource_update::refresh(job, &self.store);
                if job.state.active() {
                    self.limits.insert(
                        job.id,
                        Limits::for_job(job).with_recorded(&job.applied_resources)?,
                    );
                }
            }
            for operation in published {
                self.resource_updates
                    .insert(operation.plan.operation.clone(), operation);
            }
        }
        if start_worker
            && let Some(operation) = self
                .resource_updates
                .values()
                .find(|op| op.pending())
                .cloned()
        {
            let store = self.store.clone();
            self.resource_worker = Some(std::thread::spawn(move || {
                let mut operation = operation;
                if let Err(error) = crate::resource_update::execute(&store, &mut operation) {
                    let mut saved = crate::resource_update::load(&store)?
                        .remove(&operation.plan.operation)
                        .ok_or_else(|| {
                            io::Error::other(crate::resource_update::message(
                                "invalid resource update journal",
                            ))
                        })?;
                    saved.error = Some(error.to_string());
                    crate::resource_update::save(&store, &saved)?;
                    return Err(error);
                }
                Ok(())
            }));
        }
        Ok(())
    }

    fn cgroup(&self, id: u64) -> Option<PathBuf> {
        self.jobs
            .get(&id)
            .and_then(|job| cgroup_of(&self.tree, job))
    }

    fn clean_domains(&self) {
        if let Some(tree) = &self.tree {
            let protected = self
                .jobs
                .values()
                .filter(|job| job.state.active())
                .flat_map(|job| crate::aggregate::paths(tree, &job.aggregate_domains))
                .collect();
            crate::aggregate::cleanup(tree, &self.objects, &protected);
        }
    }

    pub fn new(store: Store, tree: Option<Tree>) -> io::Result<Daemon> {
        crate::process::Handle::open(std::process::id() as i32, None)?.signal(0)?;
        let boot_id = crate::process::boot_id()?;
        crate::durability::boot::note(&store, &boot_id);
        let (config, config_source) = crate::config::Config::load()?;
        for id in store.job_ids() {
            if let Some(job) = store.load_job(id)
                && job.state.active()
                && job.supervisor_boot_id.is_none()
            {
                return Err(io::Error::other(crate::process::missing_boot_identity()));
            }
            if let Some(job) = store.load_job(id)
                && !job.state.terminal()
                && job.policy != config.profile
            {
                return Err(io::Error::other(crate::config::message(
                    "active Jobs use a different service policy; restart with their original profile and drain before changing it",
                    &[],
                )));
            }
        }
        if config_source.is_none()
            && (!store.load_queues()?.is_empty()
                || store
                    .job_ids()
                    .iter()
                    .filter_map(|&id| store.load_job(id))
                    .any(|j| j.policy.legacy()))
        {
            return Err(io::Error::other(crate::config::message(
                "legacy state requires an explicit service profile; create a legacy configuration and set JOB_CONFIG before starting this daemon",
                &[],
            )));
        }
        crate::migration::ensure_schema(&store)?;
        crate::removal::recover(&store)?;
        let mut resource_updates = crate::resource_update::load(&store)?;
        for operation in resource_updates.values_mut() {
            crate::resource_update::publish(&store, operation)?;
        }
        crate::migration::validate_runtime(&store)?;
        let ordering = crate::admission::Ledger::load(&store)?;
        let pressure_control = crate::pressure::control::Controller::load(&store)?;
        let cancellations = crate::cancellation::load(&store)?;
        let mut objects = crate::objects::Graph::load(&store)?;
        crate::netsecret::settle_stored(&mut objects, &store.root).map_err(io::Error::other)?;
        config
            .presets
            .validate_graph(&objects)
            .map_err(io::Error::other)?;
        config
            .network
            .validate_graph(&objects)
            .map_err(io::Error::other)?;
        crate::presets::Registry::enroll(&store, &config.presets)?;
        objects.save(&store)?;
        let cores = host::cores();
        let backend = if tree.is_some() {
            Backend::Cgroup
        } else {
            Backend::Watch
        };
        let pool = match (&tree, config.profile.legacy()) {
            (Some(tree), false) => tree.ordinary_pool(cores * MILLI),
            (None, false) => Vector {
                cores_milli: cores * MILLI,
                memory: host::read_meminfo().total,
                pids: host::pid_max(),
            },
            (Some(tree), true) => tree.pool(cores * MILLI),
            (None, true) => Vector {
                cores_milli: cores * MILLI,
                memory: host::read_meminfo().total / 4 * 3,
                pids: host::pid_max() / 2,
            },
        };
        let mut history: HashMap<String, Vec<HistoryEntry>> = HashMap::new();
        for entry in store.load_history() {
            let runs = history.entry(entry.key.clone()).or_default();
            runs.push(entry);
            if runs.len() > HISTORY_WINDOW {
                runs.remove(0);
            }
        }
        let (links, unisolated_links) = crate::link::reload(&store);
        let queues = objects.queues();
        Ok(Daemon {
            boot_id,
            supervisors: HashMap::new(),
            config,
            config_source,
            objects,
            ordering,
            ordering_error: None,
            pressure_control,
            store,
            backend,
            tree,
            cores,
            pool,
            devices: Vec::new(),
            jobs: BTreeMap::new(),
            cancellations,
            resource_updates,
            resource_worker: None,
            links,
            unisolated_links,
            queues,
            history,
            watched: HashMap::new(),
            limits: HashMap::new(),
            terminating: HashMap::new(),
            predicted_starts: HashMap::new(),
            pressure_since: None,
            host_stop_pending: None,
            host_cores_written: None,
            launches: Default::default(),
            pass_memory: Default::default(),
            trims: Default::default(),
            cancel_retries: Default::default(),
        })
    }

    pub fn reestimate(&mut self) {
        let ids: Vec<u64> = self.jobs.keys().copied().collect();
        for id in ids {
            let job = &self.jobs[&id];
            let env = self.store.load_env(id).unwrap_or(Env { vars: Vec::new() });
            let mut reservation = job.reservation.clone();
            match job.state {
                State::Queued => {
                    let fresh = self.reserve(&job.spec, &env);
                    if fresh.vector.first_excess(self.pool).is_none() {
                        reservation = fresh;
                    }
                }
                State::Running => {
                    if !job.policy.legacy() {
                        continue;
                    }
                    let mut spec = job.spec.clone();
                    spec.declared.cores_milli = Some(reservation.vector.cores_milli);
                    let fresh = self.reserve(&spec, &env);
                    if fresh.vector.memory < reservation.vector.memory {
                        reservation.vector.memory = fresh.vector.memory;
                        reservation.memory_source = fresh.memory_source;
                    }
                    if fresh.disk < reservation.disk {
                        reservation.disk = fresh.disk;
                        reservation.disk_source = fresh.disk_source;
                    }
                }
                _ => {}
            }
            if let Some(job) = self.jobs.get_mut(&id)
                && job.reservation != reservation
            {
                job.reservation = reservation;
                let _ = self.store.save_job(job);
            }
        }
    }

    pub fn recover(&mut self) -> io::Result<()> {
        for id in self.store.job_ids() {
            let Some(mut job) = self.store.load_job(id) else {
                continue;
            };
            let queue_id = job.queue_id.or_else(|| {
                self.objects
                    .resolve(job.spec.queue.as_deref().unwrap_or("default"))
                    .ok()
            });
            if let Some(queue_id) = queue_id {
                job.queue_id = Some(queue_id);
                job.spec.queue = Some(self.objects.path(queue_id));
            }
            crate::clientif::index::observe(&self.store, &job);
            if !job.state.terminal()
                && crate::netsecret::settle_record(&mut job, &self.store.job_dir(id))
                    .map_err(io::Error::other)?
            {
                self.store.save_job(&job)?;
            }
            match job.state {
                State::Held | State::Queued => {
                    self.jobs.insert(id, job);
                }
                State::Starting | State::Running | State::Suspended | State::Stopping => {
                    if job.state == State::Starting
                        && crate::durability::launch::never_started(&self.store, &job)
                    {
                        self.jobs.insert(id, job);
                        self.unstarted(id, &crate::durability::launch::unconfirmed(), now_ms());
                        continue;
                    }
                    if job.supervisor_boot_id.as_deref() == Some(self.boot_id.as_str())
                        && crate::durability::exit::result(&self.store, &job).is_none()
                        && (!job.aggregate_domains.is_empty()
                            || job.queue_id.is_some_and(|q| {
                                !crate::aggregate::chain(&self.objects, q).is_empty()
                            }))
                        && (job.queue_id.is_none_or(|q| {
                            crate::aggregate::chain(&self.objects, q) != job.aggregate_domains
                        }) || job.workload_cgroup != cgroup_of(&self.tree, &job)
                            || self.tree.as_ref().is_none_or(|tree| {
                                self.changed_domains(tree, &job.aggregate_domains).is_some()
                            }))
                    {
                        return Err(io::Error::other(crate::resource_policy::message(
                            "recovery requires the original aggregate resource domains",
                        )));
                    }
                    if job.spec.declared.on.is_none()
                        && job.supervisor_boot_id.as_deref() == Some(self.boot_id.as_str())
                        && crate::durability::exit::result(&self.store, &job).is_none()
                        && (!job.applied_resources.is_empty()
                            || job.resource_sources.iter().any(|(field, origin)| {
                                *origin != crate::resource_policy::Origin::Compatibility
                                    && crate::resource_policy::kernel_file(field).is_some()
                            }))
                        && (job.workload_cgroup.is_none()
                            || job.workload_cgroup != cgroup_of(&self.tree, &job)
                            || self.tree.as_ref().is_none_or(|tree| {
                                job.applied_resources
                                    .keys()
                                    .any(|file| !tree.supports(file))
                                    || job.resource_sources.iter().any(|(field, origin)| {
                                        *origin != crate::resource_policy::Origin::Compatibility
                                            && crate::resource_policy::kernel_file(field)
                                                .is_some_and(|file| !tree.supports(file))
                                    })
                            }))
                    {
                        return Err(io::Error::other(crate::resource_policy::message(
                            "recovery requires the original resource-control backend",
                        )));
                    }
                    if job.supervisor_boot_id.as_deref() == Some(self.boot_id.as_str())
                        && crate::durability::exit::result(&self.store, &job).is_none()
                        && (job.suspension.requested
                            || job.suspension.pending
                            || job.state == State::Suspended)
                        && (job.workload_cgroup.is_none()
                            || job.workload_cgroup != cgroup_of(&self.tree, &job)
                            || crate::freezer::observed(job.workload_cgroup.as_ref().unwrap())
                                .is_err())
                    {
                        return Err(io::Error::other(crate::freezer::message(
                            "recovery requires the original cgroup backend for suspended work",
                        )));
                    }
                    if job.state == State::Stopping {
                        self.terminating.insert(
                            id,
                            job.termination_deadline_ms
                                .unwrap_or_else(|| now_ms().saturating_add(GRACE_MS)),
                        );
                    }
                    if job.supervisor_boot_id.as_deref() == Some(self.boot_id.as_str())
                        && let Some((pid, ticks)) = shim_of(&job)
                    {
                        match crate::process::Handle::open(pid, Some(ticks)) {
                            Ok(handle) => {
                                self.supervisors.insert(id, handle);
                            }
                            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
                            Err(error) => return Err(error),
                        }
                    }
                    if let Some(tree) = &self.tree {
                        let path = crate::aggregate::leaf(tree, &job.aggregate_domains, id);
                        let limits = Limits::for_job(&job).with_recorded(&job.applied_resources)?;
                        if job.supervisor_boot_id.as_deref() == Some(self.boot_id.as_str())
                            && crate::durability::exit::result(&self.store, &job).is_none()
                            && (Tree::is_populated(&path)
                                || self
                                    .supervisors
                                    .get(&id)
                                    .is_some_and(|handle| handle.alive().unwrap_or(true)))
                            && let Some((file, _, actual)) = self.changed_limits(&path, &limits)
                        {
                            return Err(io::Error::other(format!(
                                "{}: {file}: {actual}",
                                crate::resource_policy::message(
                                    "recovery requires the original resource-control backend"
                                )
                            )));
                        }
                        self.limits.insert(id, limits);
                    }
                    self.jobs.insert(id, job);
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn disk_context(&self, cwd: &Path) -> (u64, Option<PathBuf>) {
        match host::space_of(cwd) {
            Some((mount, space)) => (
                space.free.saturating_sub(if self.config.profile.legacy() {
                    host::filesystem_floor(space.size)
                } else {
                    0
                }),
                Some(mount.mount_point),
            ),
            None => (0, None),
        }
    }

    fn reserve(&self, spec: &Spec, env: &Env) -> crate::resources::Reservation {
        if !self.config.profile.legacy() {
            use crate::resources::{Reservation, Source};
            let source = |value: Option<u64>| {
                if value.is_some() {
                    Source::Declared
                } else {
                    Source::Unset
                }
            };
            return Reservation {
                vector: if spec.declared.on.is_some() {
                    Vector::default()
                } else {
                    Vector {
                        cores_milli: spec
                            .declared
                            .resources
                            .cpu_request_milli
                            .or(spec.declared.cores_milli)
                            .unwrap_or(0),
                        memory: spec
                            .declared
                            .resources
                            .memory_request
                            .or(spec.declared.memory)
                            .unwrap_or(0),
                        pids: spec.declared.pids.unwrap_or(0),
                    }
                },
                disk: spec.declared.disk.unwrap_or(0),
                devices: spec.declared.devices.clone(),
                cores_source: source(
                    spec.declared
                        .resources
                        .cpu_request_milli
                        .or(spec.declared.cores_milli),
                ),
                memory_source: source(
                    spec.declared
                        .resources
                        .memory_request
                        .or(spec.declared.memory),
                ),
                disk_source: source(spec.declared.disk),
                predicted_ms: None,
                wall_limit_ms: spec.declared.wall_ms,
            };
        }
        let key = estimate::history_key(spec);
        let cargo_build_jobs = env
            .vars
            .iter()
            .find(|(k, _)| k == "CARGO_BUILD_JOBS")
            .and_then(|(_, v)| v.parse().ok());
        let suffix = format!(" @ {}", estimate::repository_of(&spec.cwd).display());
        let own_cargo = estimate::cargo_rule(&spec.argv);
        let mut repository_cargo: Vec<HistoryEntry> = self
            .history
            .iter()
            .filter(|(k, _)| {
                **k != key
                    && k.ends_with(&suffix)
                    && match (&own_cargo, estimate::cargo_rule_of_key(k)) {
                        (Some(own), Some(other)) => own.same_kind(&other),
                        _ => false,
                    }
            })
            .flat_map(|(_, runs)| runs.iter().cloned())
            .collect();
        repository_cargo.sort_by_key(|e| e.finished_ms);
        let context = Context {
            repository_cargo,
            cores: self.cores,
            cargo_build_jobs,
            pool: self.pool,
        };
        let history = self.history.get(&key).map(Vec::as_slice).unwrap_or(&[]);
        let mut reservation = estimate::estimate(spec, history, &context);
        if spec.declared.on.is_some()
            || spec
                .queue
                .as_ref()
                .and_then(|q| self.queues.get(q))
                .is_some_and(|q| q.settings.on.is_some())
        {
            reservation.vector = Vector {
                cores_milli: 0,
                memory: 0,
                pids: REMOTE_PIDS,
            };
            return reservation;
        }
        let settings = spec
            .queue
            .as_ref()
            .and_then(|q| self.queues.get(q))
            .map(|q| q.settings.clone())
            .unwrap_or_default();
        if spec.declared.cores_milli.is_none()
            && let Some(cap) = settings.cores_milli
        {
            reservation.vector.cores_milli = reservation.vector.cores_milli.min(cap);
        }
        if spec.declared.memory.is_none()
            && let Some(cap) = settings.memory
        {
            reservation.vector.memory = reservation.vector.memory.min(cap);
        }
        if let Some(request) = spec.declared.resources.cpu_request_milli {
            reservation.vector.cores_milli = request;
            reservation.cores_source = crate::resources::Source::Declared;
        }
        if let Some(request) = spec.declared.resources.memory_request {
            reservation.vector.memory = request;
            reservation.memory_source = crate::resources::Source::Declared;
        }
        reservation
    }

    fn resolve_settings(&self, spec: &mut Spec) -> Result<(), String> {
        let settings = spec
            .queue
            .as_ref()
            .and_then(|q| self.queues.get(q))
            .map(|q| q.settings.clone())
            .unwrap_or_default();
        spec.declared.on = spec.declared.on.clone().or(settings.on.clone());
        crate::netsecret::resolve(&mut spec.declared, &settings)?;
        if let Some(size) = spec.declared.terminal {
            if !size.valid() {
                return Err(crate::terminal::message("invalid terminal size", &[]));
            }
            if spec.declared.on.is_some() {
                return Err(crate::terminal::message(
                    "--pty is local to a job service; connect with SSH and run job there",
                    &[],
                ));
            }
        }
        if spec.declared.on.is_some() {
            spec.declared.dir = spec.declared.dir.clone().or(settings.dir.clone());
            spec.declared.net = spec.declared.net.clone().or(settings.net.clone());
            spec.declared.monitor = spec.declared.monitor.clone().or(settings.monitor.clone());
            spec.declared.bandwidth = spec
                .declared
                .bandwidth
                .or(settings.job_bandwidth)
                .or(settings.bandwidth);
            return Ok(());
        }
        if let Some(dir) = spec.declared.dir.clone().or(settings.dir.clone()) {
            spec.cwd = match (dir.strip_prefix("~"), std::env::var_os("HOME")) {
                (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
                _ => dir,
            };
        }
        let queue_link = settings.net.as_ref().is_some_and(Net::needs_link);
        if queue_link && spec.declared.net.is_some() && spec.declared.net != settings.net {
            return Err(format!(
                "queue {}'s jobs have {}; this job asks for {}; run it outside the queue",
                spec.queue.clone().unwrap_or_default(),
                settings.net.as_ref().map(Net::describe).unwrap_or_default(),
                spec.declared
                    .net
                    .as_ref()
                    .map(Net::describe)
                    .unwrap_or_default()
            ));
        }
        spec.declared.net = Some(
            spec.declared
                .net
                .clone()
                .or(settings.net.clone())
                .unwrap_or(Net::Host),
        );
        spec.declared.monitor = spec.declared.monitor.clone().or(settings.monitor.clone());
        crate::netpolicy::admit(
            &self.config.network,
            &spec.declared,
            &settings,
            spec.queue.as_deref().unwrap_or_default(),
        )?;
        if let Some(Net::WireGuard(path)) = &spec.declared.net {
            crate::wg::read(path)?;
        }
        if let Some(Net::OpenVpn(_)) = &spec.declared.net {
            return Err("openvpn: not yet; use wireguard:FILE or a proxy".to_string());
        }
        if spec.declared.on.is_none()
            && spec.declared.net == Some(Net::None)
            && !host::user_namespaces()
        {
            return Err(
                "this host does not let a user create namespaces, so it cannot run a job with --net none"
                    .to_string(),
            );
        }
        let shaped = spec.declared.bandwidth.is_some()
            || settings.bandwidth.is_some()
            || settings.job_bandwidth.is_some();
        let routed = spec.declared.net.as_ref().is_some_and(Net::needs_link);
        if shaped || routed {
            if spec.declared.net == Some(Net::None) {
                return Err(
                    "a job with no network has no bandwidth to cap; drop --net none or the bandwidth"
                        .to_string(),
                );
            }
            let missing = crate::link::missing_tools();
            if !missing.is_empty() {
                return Err(format!(
                    "this host cannot give a job its own network: it lacks {}",
                    missing.join(", ")
                ));
            }
            if !host::user_namespaces() {
                return Err(
                    "this host does not let a user create namespaces, so it cannot give a job its own network"
                        .to_string(),
                );
            }
        }
        let name = spec.queue.clone().unwrap_or_default();
        let over =
            |declared: Option<u64>, cap: Option<u64>| declared.zip(cap).filter(|(d, c)| d > c);
        if let Some((declared, cap)) = over(spec.declared.cores_milli, settings.cores_milli) {
            return Err(format!(
                "this job declares {}, and queue {name} holds {} for all its jobs",
                crate::resources::format_cores(declared),
                crate::resources::format_cores(cap)
            ));
        }
        if let Some((declared, cap)) = over(spec.declared.memory, settings.memory) {
            return Err(format!(
                "this job declares {} memory, and queue {name} holds {} for all its jobs",
                format_bytes(declared),
                format_bytes(cap)
            ));
        }
        Ok(())
    }

    fn submit(&mut self, spec: Spec, env: Env, held: bool, key: Option<String>) -> Response {
        let key = match crate::durability::idempotency::key(key, &spec, held) {
            Ok(key) => key,
            Err(message) => return Response::Error { message },
        };
        if let Some(key) = &key {
            match crate::durability::idempotency::lookup(&self.store, key) {
                Ok(crate::durability::idempotency::Lookup::Fresh) => {}
                Ok(crate::durability::idempotency::Lookup::Existing(id)) => {
                    return match self.load(id) {
                        Some(mut job) => {
                            job.durability.replayed = true;
                            Response::Submitted { job }
                        }
                        None => Response::Error {
                            message: format!("job {id} was not recorded"),
                        },
                    };
                }
                Err(error) => {
                    return Response::Error {
                        message: error.to_string(),
                    };
                }
            }
        }
        self.put_job(spec, env, None, held, false, key)
    }

    fn retry(
        &mut self,
        id: u64,
        expected_attempt: u64,
        held: bool,
        env: Option<Env>,
        queue: Option<String>,
        allow_lost: bool,
    ) -> Response {
        let result = (|| -> Result<Response, String> {
            let previous = self
                .load(id)
                .ok_or_else(|| crate::lifecycle::message("no such Job"))?;
            if previous.attempt != expected_attempt || !previous.state.terminal() {
                return Err(crate::lifecycle::message(
                    "retry requires the expected completed attempt",
                ));
            }
            if previous.state == State::Lost && !allow_lost {
                return Err(crate::lifecycle::message(
                    "lost work may still exist; retry requires --allow-lost",
                ));
            }
            if previous
                .supervisor_boot_id
                .as_ref()
                .is_none_or(|boot| boot == &self.boot_id)
                && let Some(pid) = previous.shim_pid
            {
                match crate::process::Handle::open(pid, previous.shim_start_ticks) {
                    Ok(handle) if handle.alive().map_err(|e| e.to_string())? => {
                        return Ok(Response::RetryPending);
                    }
                    Ok(_) => {}
                    Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
            if previous
                .workload_cgroup
                .as_ref()
                .is_some_and(|path| Tree::is_populated(path))
            {
                return Err(crate::lifecycle::message(
                    "previous workload cgroup is still populated",
                ));
            }
            let env = env.or_else(|| self.store.load_env(id)).ok_or_else(|| {
                crate::lifecycle::message(
                    "saved environment is unavailable; use --current-env to replace it",
                )
            })?;
            let mut spec = previous
                .requested_spec
                .clone()
                .unwrap_or_else(|| previous.spec.clone());
            spec.queue = match queue {
                Some(path) => Some(path),
                None => match previous
                    .queue_id
                    .filter(|id| self.objects.nodes.contains_key(id))
                {
                    Some(id) => Some(self.objects.path(id)),
                    None => {
                        return Err(crate::lifecycle::message(
                            "original Queue was removed; choose --queue PATH",
                        ));
                    }
                },
            };
            Ok(self.put_job(spec, env, Some(id), held, true, None))
        })();
        result.unwrap_or_else(|message| Response::Error { message })
    }

    fn put_job(
        &mut self,
        mut spec: Spec,
        env: Env,
        existing_id: Option<u64>,
        held: bool,
        retry: bool,
        idempotency: Option<crate::durability::idempotency::Key>,
    ) -> Response {
        let previous = existing_id.and_then(|id| self.load(id));
        if existing_id.is_some()
            && !previous.as_ref().is_some_and(|job| {
                if retry {
                    job.state.terminal()
                } else {
                    job.state.editable()
                }
            })
        {
            return Response::Error {
                message: crate::lifecycle::message("only held or queued Jobs can be edited"),
            };
        }
        let net_secret = match crate::netsecret::take_for_job(
            &mut spec.declared,
            existing_id.map(|id| self.store.job_dir(id)).as_deref(),
        ) {
            Ok(secret) => secret,
            Err(message) => return Response::Error { message },
        };
        let mut requested = spec.clone();
        let path = spec.queue.as_deref().unwrap_or("default");
        let queue_id = match self.objects.resolve(path) {
            Ok(id) if self.objects.nodes[&id].kind == crate::objects::Kind::Queue => id,
            _ => {
                return Response::Error {
                    message: format!(
                        "there is no queue {path}; create it with: job queue create {path} (legacy: job queue add {path})"
                    ),
                };
            }
        };
        let view = self.objects.view(queue_id);
        let preset_snapshot = match crate::presets::resolve(
            &self.config.presets,
            &self.objects,
            queue_id,
            &mut spec.declared,
        ) {
            Ok(snapshot) => snapshot,
            Err(message) => return Response::Error { message },
        };
        let mut priority_source =
            match crate::admission::resolve(&mut spec.declared.priority, &view.effective) {
                Ok(source) => source,
                Err(message) => return Response::Error { message },
            };
        if let Some(origin) = preset_snapshot
            .as_ref()
            .and_then(|s| s.applied.get("priority"))
        {
            priority_source = Some(origin.clone());
        }
        if let Err(message) =
            crate::admission::bounds(&self.objects, queue_id, spec.declared.priority.unwrap_or(0))
        {
            return Response::Error { message };
        }
        if !view.closed_by.is_empty()
            && (retry
                || previous
                    .as_ref()
                    .is_none_or(|job| job.queue_id != Some(queue_id)))
        {
            return Response::Error {
                message: format!(
                    "queue {path} takes no new jobs: closed by {}",
                    view.closed_by.join(", ")
                ),
            };
        }
        spec.queue = Some(self.objects.path(queue_id));
        let mut resource_sources =
            match crate::resource_policy::resolve(&mut spec.declared, &view.effective) {
                Ok(sources) => sources,
                Err(message) => return Response::Error { message },
            };
        match crate::process_policy::resolve(&mut spec.declared, &view.effective) {
            Ok(sources) => resource_sources.extend(sources),
            Err(message) => return Response::Error { message },
        }
        match crate::security::resolve(&mut spec.declared, &view.effective) {
            Ok(sources) => resource_sources.extend(sources),
            Err(message) => return Response::Error { message },
        }
        match crate::isolation::resolve(&mut spec.declared, &view.effective) {
            Ok(sources) => resource_sources.extend(sources),
            Err(message) => return Response::Error { message },
        }
        match crate::streams::quota::resolve(
            &mut spec.declared,
            &view.effective,
            &self.config.output,
        ) {
            Ok(sources) => resource_sources.extend(sources),
            Err(message) => return Response::Error { message },
        }
        if let Some(snapshot) = &preset_snapshot {
            for (field, origin) in &snapshot.applied {
                if resource_sources.contains_key(field) {
                    resource_sources.insert(field.clone(), origin.clone());
                }
            }
        }
        if let Some(name) = &spec.queue
            && !self.queues.contains_key(name)
        {
            return Response::Error {
                message: format!(
                    "there is no queue {name}; {}; create it with: job queue add {name}",
                    self.queue_names()
                ),
            };
        }
        if let Err(message) = self.resolve_settings(&mut spec) {
            return Response::Error { message };
        }
        if spec.declared.on.is_some()
            && self.objects.ancestors(queue_id).iter().any(|id| {
                crate::pressure::control::local(&self.objects, *id)
                    .iter()
                    .any(|r| r.required)
            })
        {
            return Response::Error {
                message: crate::pressure::message("remote workload PSI is unavailable locally"),
            };
        }
        if let Err(message) = crate::aggregate::available(
            self.tree.as_ref(),
            &crate::aggregate::chain(&self.objects, queue_id),
            spec.declared.on.is_some(),
        ) {
            return Response::Error { message };
        }
        if spec.declared.on.is_none() {
            if let Err(message) = crate::isolation::kernel::admissible(&spec.declared, &spec.cwd) {
                return Response::Error { message };
            }
            if let Err(message) = crate::security::kernel::Prepared::new(
                &spec.declared.security,
                spec.declared.isolation.enters_user_namespace(),
            ) {
                return Response::Error { message };
            }
            if let Err(message) =
                crate::process_policy::kernel::Prepared::new(&spec.declared.process)
            {
                return Response::Error { message };
            }
            for (file, value) in Limits::for_controls(&spec.declared.resources).files() {
                if let Err(message) = crate::io_policy::available(file, value) {
                    return Response::Error { message };
                }
            }
            for (field, source) in &resource_sources {
                if *source == crate::resource_policy::Origin::Compatibility {
                    continue;
                }
                if let Some(file) = crate::resource_policy::kernel_file(field)
                    && self.tree.as_ref().is_none_or(|tree| !tree.supports(file))
                {
                    return Response::Error {
                        message: format!(
                            "{}: {file}",
                            crate::resource_policy::message(
                                "requested resource control is unavailable"
                            )
                        ),
                    };
                }
            }
            if !held
                && let Err(message) = crate::operations::mode::refuse(
                    self.backend,
                    self.config.profile,
                    &spec.declared,
                    &resource_sources,
                )
            {
                return Response::Error { message };
            }
        }
        if spec.argv.is_empty() {
            return Response::Error {
                message: "the command is empty".to_string(),
            };
        }
        if !spec.cwd.is_absolute() || !spec.cwd.is_dir() {
            return Response::Error {
                message: format!(
                    "the working directory {} does not exist",
                    spec.cwd.display()
                ),
            };
        }
        if let Some(device) = spec
            .declared
            .devices
            .iter()
            .find(|d| !self.devices.contains(d))
        {
            let known = if self.devices.is_empty() {
                "none".to_string()
            } else {
                self.devices.join(", ")
            };
            return Response::Error {
                message: format!("this host has no device named {device}; its devices: {known}"),
            };
        }
        if let Some(name) = &spec.queue {
            match self.queues.get(name) {
                None => {
                    return Response::Error {
                        message: format!(
                            "there is no queue {name}; {}; create it with: job queue add {name}",
                            self.queue_names()
                        ),
                    };
                }
                Some(queue) if queue.draining => {
                    return Response::Error {
                        message: format!(
                            "queue {name} is being removed and takes no new jobs; it ends when its last job ends"
                        ),
                    };
                }
                Some(_) => {}
            }
        }
        let key = estimate::history_key(&spec);
        let reservation = self.reserve(&spec, &env);
        for ancestor in self.objects.ancestors(queue_id) {
            for (key, need) in [
                ("cores_milli", reservation.vector.cores_milli),
                ("memory", reservation.vector.memory),
            ] {
                if let Some(cap) = self.objects.nodes[&ancestor]
                    .config
                    .get(key)
                    .and_then(|v| v.as_u64())
                    && need > cap
                {
                    return Response::Error {
                        message: format!(
                            "request {need} exceeds {key} admission budget {cap} at {}",
                            self.objects.path(ancestor)
                        ),
                    };
                }
            }
        }
        if let Some(declared) = spec.declared.disk.filter(|_| spec.declared.on.is_none()) {
            let (room, mount) = self.disk_context(&spec.cwd);
            if declared > room {
                return Response::Error {
                    message: format!(
                        "this job declares {} of disk; {} has {} free above its floor",
                        format_bytes(declared),
                        mount.map_or_else(
                            || "its filesystem".to_string(),
                            |m| m.display().to_string()
                        ),
                        format_bytes(room)
                    ),
                };
            }
        }
        if let Some(excess) = reservation.vector.first_excess(self.pool) {
            return Response::Error {
                message: format!(
                    "this job needs {excess}; declare less, or run it on a larger host"
                ),
            };
        }
        let id = match existing_id.map_or_else(|| self.store.next_id(), Ok) {
            Ok(id) => id,
            Err(e) => {
                return Response::Error {
                    message: format!("cannot allocate a job id: {e}"),
                };
            }
        };
        if net_secret.is_some() {
            let path = crate::netsecret::private_path(&self.store.job_dir(id));
            spec.declared.net_secret_file = Some(path.clone());
            requested.declared.net_secret_file = Some(path);
        }
        let job = Job {
            durability: crate::durability::idempotency::record(
                idempotency.as_ref(),
                previous.as_ref(),
            ),
            output_mode: Some(if spec.declared.terminal.is_some() {
                crate::streams::Mode::Pty
            } else {
                crate::streams::Mode::Pipe
            }),
            preset_snapshot,
            priority_source,
            admission_snapshot: None,
            priority_changes: previous
                .as_ref()
                .filter(|_| !retry)
                .map_or_else(Vec::new, |job| job.priority_changes.clone()),
            resource_sources,
            applied_resources: Default::default(),
            workload_cgroup: None,
            aggregate_domains: Vec::new(),
            suspension: Default::default(),
            timing: Default::default(),
            attempt: match previous.as_ref() {
                Some(job) if retry => match job.attempt.checked_add(1) {
                    Some(attempt) => attempt,
                    None => {
                        return Response::Error {
                            message: crate::lifecycle::message("attempt counter exhausted"),
                        };
                    }
                },
                Some(job) => job.attempt,
                None => 1,
            },
            attempt_submitted_ms: if retry {
                Some(now_ms())
            } else {
                previous
                    .as_ref()
                    .and_then(|job| job.attempt_submitted_ms)
                    .or_else(|| Some(now_ms()))
            },
            submitted_spec: match &previous {
                Some(job) => job.submitted_spec.clone(),
                None => Some(requested.clone()),
            },
            requested_spec: Some(requested),
            effective_spec: None,
            released_ms: if held {
                None
            } else if retry {
                Some(now_ms())
            } else {
                previous
                    .as_ref()
                    .and_then(|job| job.released_ms)
                    .or_else(|| Some(now_ms()))
            },
            policy: self.config.profile,
            queue_id: Some(queue_id),
            id,
            log: self.store.log_file(id),
            spec,
            key,
            reservation,
            backend: self.backend,
            state: if held { State::Held } else { State::Queued },
            submitted_ms: previous
                .as_ref()
                .map_or_else(now_ms, |job| job.submitted_ms),
            started_ms: None,
            finished_ms: None,
            shim_pid: None,
            shim_start_ticks: None,
            supervisor_boot_id: None,
            termination_deadline_ms: None,
            waited_for: None,
            stop: None,
            result: None,
            usage: Default::default(),
            link: None,
            network: None,
        };
        if let Err(message) = crate::admission::fair::validate_job(&self.objects, &job) {
            return Response::Error { message };
        }
        let idempotency = idempotency.filter(|_| previous.is_none());
        if let Some(key) = &idempotency
            && let Err(e) = crate::durability::idempotency::reserve(&self.store, key, id)
        {
            return Response::Error {
                message: format!("cannot record the job: {e}"),
            };
        }
        crate::durability::failpoint("submit-after-index");
        if let Err(e) = self.store.publish_submission(
            &job,
            &env,
            previous.is_some(),
            retry,
            net_secret.as_deref(),
        ) {
            if let Some(key) = &idempotency {
                crate::durability::idempotency::release(&self.store, key);
            }
            return Response::Error {
                message: format!("cannot record the job: {e}"),
            };
        }
        crate::durability::failpoint("submit-after-publish");
        self.jobs.insert(id, job);
        self.admit_briefly(now_ms());
        match self.load(id) {
            Some(job) => Response::Submitted { job },
            None => Response::Error {
                message: format!("job {id} was not recorded"),
            },
        }
    }

    fn load(&self, id: u64) -> Option<Job> {
        let mut job = self
            .jobs
            .get(&id)
            .cloned()
            .or_else(|| self.store.load_job(id))?;
        self.learn_start(&mut job);
        job.update_timing(now_ms());
        Some(job)
    }

    fn learn_start(&self, job: &mut Job) {
        if job.started_ms.is_none() && job.state.active() && job.state != State::Starting {
            job.started_ms = crate::durability::launch::confirmed_at(&self.store, job);
        }
    }

    fn publish_start(&self, job: &mut Job) {
        if job.started_ms.is_none() {
            self.learn_start(job);
            if job.started_ms.is_some() {
                crate::clientif::index::learned(&self.store, job);
            }
        }
    }

    fn request_freezer(&mut self, id: u64, frozen: bool) -> Result<(), String> {
        let mut job = self
            .load(id)
            .ok_or_else(|| crate::freezer::message("no such control target"))?;
        if !matches!(job.state, State::Running | State::Suspended) {
            return Err(crate::freezer::message(
                "only running or suspended Jobs can be controlled",
            ));
        }
        let path = self
            .cgroup(id)
            .filter(|path| Some(path) == job.workload_cgroup.as_ref())
            .filter(|_| job.spec.declared.on.is_none())
            .ok_or_else(|| {
                crate::freezer::message("suspension requires a local cgroup v2 workload")
            })?;
        crate::freezer::observed(&path).map_err(|e| e.to_string())?;
        crate::freezer::requested(&path).map_err(|e| e.to_string())?;
        if job.suspension.requested != frozen || job.suspension.generation == 0 {
            job.suspension.generation = job
                .suspension
                .generation
                .checked_add(1)
                .ok_or_else(|| crate::freezer::message("invalid cgroup freezer value"))?;
        }
        job.suspension.requested = frozen;
        job.suspension.pending = true;
        job.suspension.error = None;
        job.suspension.actor_uid = Some(crate::durability::peer::actor_uid());
        self.store.save_job(&job).map_err(|e| e.to_string())?;
        self.jobs.insert(id, job);
        self.reconcile_freezer(id, now_ms());
        Ok(())
    }

    fn reconcile_freezer(&mut self, id: u64, now: u64) {
        let Some(mut job) = self.jobs.get(&id).cloned() else {
            return;
        };
        if !job.state.active() || job.spec.declared.on.is_some() {
            return;
        }
        let Some(path) = job
            .workload_cgroup
            .clone()
            .filter(|path| Some(path) == self.cgroup(id).as_ref())
        else {
            return;
        };
        if job.started_ms.is_none() {
            self.publish_start(&mut job);
            if job.started_ms.is_some() {
                self.jobs.insert(id, job.clone());
            }
        }
        let before = (job.state.clone(), job.suspension.clone());
        let update = (|| -> io::Result<()> {
            let local = crate::freezer::requested(&path)?;
            if (job.suspension.pending || job.suspension.requested)
                && local != job.suspension.requested
            {
                crate::freezer::write(&path, job.suspension.requested)?;
                job.suspension.pending = true;
            }
            let frozen = crate::freezer::observed(&path)?;
            job.suspension.observe(frozen, now);
            if job.suspension.pending && frozen == job.suspension.requested {
                job.suspension.pending = false;
            }
            if matches!(job.state, State::Running | State::Suspended) {
                job.state = if frozen {
                    State::Suspended
                } else {
                    State::Running
                };
            }
            Ok(())
        })();
        job.suspension.error = update.err().map(|error| {
            format!(
                "{}: {error}",
                crate::freezer::message("request saved; kernel control failed")
            )
        });
        if before != (job.state.clone(), job.suspension.clone()) {
            job.update_timing(now);
            if let Err(error) = self.store.save_job(&job) {
                job.suspension.error = Some(format!(
                    "{}: {error}",
                    crate::freezer::message("kernel state changed; recording failed")
                ));
            }
            self.jobs.insert(id, job);
        }
    }

    fn running_ids(&self) -> Vec<u64> {
        self.jobs
            .values()
            .filter(|j| j.state.active())
            .map(|j| j.id)
            .collect()
    }

    fn current_memory(&self, id: u64) -> u64 {
        match (&self.tree, self.backend) {
            (Some(_), Backend::Cgroup) => self
                .cgroup(id)
                .map_or(0, |path| Tree::counters(&path).memory_current),
            _ => self.watched.get(&id).map_or(0, |w| w.memory),
        }
    }

    fn current_written(&self, id: u64) -> u64 {
        match (&self.tree, self.backend) {
            (Some(_), Backend::Cgroup) => self
                .cgroup(id)
                .map_or(0, |path| Tree::counters(&path).written()),
            _ => self.watched.get(&id).map_or(0, |w| w.written),
        }
    }

    fn admissible_now(&self, job: &Job) -> Result<(), (String, bool)> {
        self.pressure_control
            .gate(&self.objects, &self.jobs, job)
            .map_err(|reason| (reason, false))?;
        crate::admission::fair::validate_job(&self.objects, job)
            .map_err(|reason| (reason, false))?;
        if let Some(error) = &self.ordering_error {
            return Err((error.clone(), false));
        }
        if let Some(queue) = job.queue_id {
            crate::admission::bounds(
                &self.objects,
                queue,
                job.spec.declared.priority.unwrap_or(0),
            )
            .map_err(|error| (error, false))?;
        }
        if self.update_pending() {
            return Err((
                crate::resource_update::message("resource update is pending"),
                false,
            ));
        }
        if self.cancellations.values().any(|op| {
            !op.complete
                && op
                    .selection
                    .members
                    .iter()
                    .any(|member| member.id == job.id && member.attempt == job.attempt)
        }) {
            return Err((
                crate::cancellation::message("cancellation is pending"),
                false,
            ));
        }
        if job.spec.declared.on.is_none() {
            for (file, value) in Limits::for_job(job).files() {
                crate::io_policy::available(file, value).map_err(|reason| (reason, false))?;
            }
        }
        if let Some(id) = job.queue_id {
            crate::aggregate::available(
                self.tree.as_ref(),
                &crate::aggregate::chain(&self.objects, id),
                job.spec.declared.on.is_some(),
            )
            .map_err(|message| (message, false))?;
            for ancestor in self.objects.ancestors(id) {
                let node = &self.objects.nodes[&ancestor];
                if node.paused {
                    return Err((
                        format!("admission paused by {}", self.objects.path(ancestor)),
                        false,
                    ));
                }
                if let Some(cap) = node.config.get("max_running").and_then(|v| v.as_u64()) {
                    let running = self
                        .jobs
                        .values()
                        .filter(|j| {
                            j.state.active()
                                && j.queue_id.is_some_and(|q| self.objects.within(q, ancestor))
                        })
                        .count() as u64;
                    if running >= cap {
                        return Err((
                            format!("max-running {cap} at {}", self.objects.path(ancestor)),
                            true,
                        ));
                    }
                }
                for (key, needed) in [
                    ("cores_milli", job.reservation.vector.cores_milli),
                    ("memory", job.reservation.vector.memory),
                ] {
                    if let Some(cap) = node.config.get(key).and_then(|v| v.as_u64()) {
                        let used = self
                            .jobs
                            .values()
                            .filter(|j| {
                                j.state.active()
                                    && j.queue_id.is_some_and(|q| self.objects.within(q, ancestor))
                            })
                            .fold(needed, |sum, j| {
                                sum.saturating_add(if key == "memory" {
                                    j.reservation.vector.memory
                                } else {
                                    j.reservation.vector.cores_milli
                                })
                            });
                        if used > cap {
                            return Err((
                                format!(
                                    "{key} admission budget {cap} at {}",
                                    self.objects.path(ancestor)
                                ),
                                needed <= cap,
                            ));
                        }
                    }
                }
            }
        }
        let meminfo = self.pass_memory.get().unwrap_or_else(host::read_meminfo);
        let growth: u64 = self
            .running_ids()
            .iter()
            .map(|&id| {
                let reserved = self.jobs[&id].reservation.vector.memory;
                reserved.saturating_sub(self.current_memory(id))
            })
            .sum();
        let memory_room = (if job.policy.legacy() {
            meminfo.room_above_floor()
        } else {
            meminfo.available
        })
        .saturating_sub(growth);
        if job.reservation.vector.memory > memory_room {
            let released: u64 = self
                .running_ids()
                .iter()
                .map(|&id| {
                    self.jobs[&id]
                        .reservation
                        .vector
                        .memory
                        .max(self.current_memory(id))
                })
                .sum();
            return Err((
                format!(
                    "host memory: {} available for jobs, it needs {}",
                    format_bytes(memory_room),
                    format_bytes(job.reservation.vector.memory)
                ),
                memory_room + released >= job.reservation.vector.memory,
            ));
        }
        if job.spec.declared.disk.is_none() || job.spec.declared.on.is_some() {
            return Ok(());
        }
        let (disk_room, mount) = self.disk_context(&job.spec.cwd);
        let pending: u64 = self
            .running_ids()
            .iter()
            .filter(|&&id| self.jobs[&id].spec.declared.disk.is_some())
            .filter(|&&id| self.disk_context(&self.jobs[&id].spec.cwd).1 == mount)
            .map(|&id| {
                self.jobs[&id]
                    .reservation
                    .disk
                    .saturating_sub(self.current_written(id))
            })
            .sum();
        let disk_room = disk_room.saturating_sub(pending);
        if job.reservation.disk > disk_room {
            return Err((
                format!(
                    "disk on {}: {} free for jobs, it declared {}",
                    mount.map_or_else(|| "its filesystem".to_string(), |m| m.display().to_string()),
                    format_bytes(disk_room),
                    format_bytes(job.reservation.disk)
                ),
                disk_room + pending >= job.reservation.disk,
            ));
        }
        Ok(())
    }

    fn wait_reason(&self, job: &Job, admissible: &Result<(), (String, bool)>) -> String {
        if let Err((reason, _)) = admissible {
            return reason.clone();
        }
        let running: Vec<&Job> = self.jobs.values().filter(|j| j.state.active()).collect();
        let used = running.iter().fold(job.reservation.vector, |sum, j| {
            sum.add(j.reservation.vector)
        });
        let holders = |held: &dyn Fn(&Job) -> bool| -> String {
            running
                .iter()
                .filter(|j| held(j))
                .map(|j| j.id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        if let Some(device) = job
            .reservation
            .devices
            .iter()
            .find(|d| running.iter().any(|j| j.reservation.devices.contains(d)))
        {
            return format!(
                "{device} held by job {}",
                holders(&|j| j.reservation.devices.contains(device))
            );
        }
        if used.cores_milli > self.pool.cores_milli {
            return format!(
                "cores held by jobs {}",
                holders(&|j| j.reservation.vector.cores_milli > 0)
            );
        }
        if used.memory > self.pool.memory {
            return format!(
                "memory held by jobs {}",
                holders(&|j| j.reservation.vector.memory > 0)
            );
        }
        if used.pids > self.pool.pids {
            return format!(
                "processes held by jobs {}",
                holders(&|j| j.reservation.vector.pids > 0)
            );
        }
        let earlier: Vec<String> = self
            .jobs
            .values()
            .filter(|j| j.state == State::Queued && j.id < job.id)
            .map(|j| j.id.to_string())
            .collect();
        format!("jobs queued before it: {}", earlier.join(", "))
    }

    fn admit(&mut self, now: u64) {
        self.admit_within(now, 0, false);
    }

    fn admit_briefly(&mut self, now: u64) {
        self.admit_within(now, crate::pacing::STARTS_PER_PASS, true);
    }

    fn admit_later(&mut self, now: u64) {
        self.admit_within(now, 0, true);
    }

    fn admit_within(&mut self, now: u64, starts: u32, brief: bool) {
        self.launches.begin(starts, brief);
        self.pass_memory.set(Some(host::read_meminfo()));
        self.admit_pass(now);
        self.pass_memory.set(None);
        self.launches.settle();
    }

    fn wait_turn(&mut self, id: u64) {
        if let Some(job) = self.jobs.get_mut(&id) {
            self.launches.wait_turn(id, &mut job.waited_for);
        }
    }

    fn admit_pass(&mut self, now: u64) {
        self.pressure_control.checkpoint(
            &self.store,
            &self.objects,
            &self.jobs,
            &self.config.pressure,
            self.tree.as_ref().map(|t| t.jobs()).as_deref(),
            &self.boot_id,
        );
        if self.checkpoint_ordering().is_err() {
            return;
        }
        if self.update_pending() {
            for job in self
                .jobs
                .values_mut()
                .filter(|job| job.state == State::Queued)
            {
                let reason = crate::resource_update::message("resource update is pending");
                if job.waited_for.as_ref() != Some(&reason) {
                    job.waited_for = Some(reason);
                }
                self.predicted_starts.insert(job.id, None);
            }
            return;
        }
        let stranded: Vec<(u64, PathBuf)> = self
            .jobs
            .values()
            .filter(|j| j.state == State::Queued && !j.spec.cwd.is_dir())
            .map(|j| (j.id, j.spec.cwd.clone()))
            .collect();
        for (id, cwd) in stranded {
            if let Some(job) = self.jobs.get_mut(&id) {
                job.stop = Some(Stop {
                    kind: StopKind::WorkingDirectoryGone,
                    line: format!(
                        "did not start: its working directory {} no longer exists",
                        cwd.display()
                    ),
                });
            }
            self.finalize(
                id,
                ShimResult {
                    finished_ms: now,
                    ..ShimResult::default()
                },
                now,
            );
        }
        if crate::admission::enabled(&self.objects, &self.jobs) {
            self.admit_ordered(now);
            return;
        }
        if !self.config.profile.legacy() {
            self.admit_ordinary(now);
            return;
        }
        loop {
            let mut reasons: HashMap<u64, String> = HashMap::new();
            let running: Vec<Occupant> = self
                .jobs
                .values()
                .filter(|j| j.state.active())
                .map(|j| {
                    let start = j.durability.admitted_ms.or(j.started_ms).unwrap_or(now);
                    Occupant {
                        need: j.reservation.vector,
                        devices: j.reservation.devices.clone(),
                        start,
                        end: j
                            .reservation
                            .predicted_ms
                            .map(|p| schedule::extended_end(start, p, now)),
                        queue: j.spec.queue.clone(),
                    }
                })
                .collect();
            let mut waiting: Vec<_> = self
                .jobs
                .values()
                .filter(|j| j.state == State::Queued)
                .collect();
            waiting.sort_by_key(|j| (j.waiting_since(), j.id));
            let queued: Vec<Candidate> = waiting
                .into_iter()
                .map(|j| (j, self.admissible_now(j)))
                .collect::<Vec<_>>()
                .into_iter()
                .map(|(j, admissible)| {
                    let candidate = Candidate {
                        id: j.id,
                        need: j.reservation.vector,
                        devices: j.reservation.devices.clone(),
                        duration: j.reservation.predicted_ms,
                        admissible_now: admissible.is_ok(),
                        holds_place: admissible
                            .as_ref()
                            .err()
                            .is_none_or(|(_, relievable)| *relievable),
                        queue: j.spec.queue.clone(),
                    };
                    reasons.insert(j.id, self.wait_reason(j, &admissible));
                    candidate
                })
                .collect();
            let plan = schedule::plan(now, self.pool, &self.lanes(), &running, &queued);
            self.predicted_starts = plan
                .iter()
                .map(|planned| {
                    let start = match planned.placement {
                        Placement::Now => Some(now),
                        Placement::At(t) => Some(t),
                        Placement::AfterUnknownEnd => None,
                    };
                    (planned.id, start)
                })
                .collect();
            for planned in &plan {
                if planned.placement == Placement::Now {
                    continue;
                }
                let reason = match planned.hold {
                    Some(hold) => self.queue_reason(planned.id, hold),
                    None => reasons.remove(&planned.id),
                };
                if let (Some(job), Some(reason)) = (self.jobs.get_mut(&planned.id), reason)
                    && job.waited_for.as_ref() != Some(&reason)
                {
                    job.waited_for = Some(reason);
                }
            }
            let Some(id) = plan
                .into_iter()
                .find(|p| p.placement == Placement::Now)
                .map(|p| p.id)
            else {
                break;
            };
            if !self.launches.take() {
                self.wait_turn(id);
                break;
            }
            self.start(id, now);
        }
    }

    fn admit_ordinary(&mut self, now: u64) {
        self.predicted_starts.clear();
        let mut queued = self
            .jobs
            .values()
            .filter(|j| j.state == State::Queued)
            .map(|j| j.id)
            .collect::<Vec<_>>();
        queued.sort_by_key(|id| (self.jobs[id].waiting_since(), *id));
        for id in queued {
            let job = &self.jobs[&id];
            let admissible = self.admissible_now(job);
            let used = self
                .jobs
                .values()
                .filter(|j| j.state.active())
                .fold(job.reservation.vector, |sum, j| {
                    sum.add(j.reservation.vector)
                });
            let device_busy = self.jobs.values().filter(|j| j.state.active()).any(|j| {
                j.reservation
                    .devices
                    .iter()
                    .any(|device| job.reservation.devices.contains(device))
            });
            let reason = match admissible {
                Err((reason, _)) => Some(reason),
                Ok(()) if !used.fits_within(self.pool) || device_busy => {
                    Some(self.wait_reason(job, &Ok(())))
                }
                Ok(()) => None,
            };
            if let Some(reason) = reason {
                let job = self.jobs.get_mut(&id).unwrap();
                if job.waited_for.as_ref() != Some(&reason) {
                    job.waited_for = Some(reason);
                }
            } else if self.launches.take() {
                self.start(id, now);
            } else {
                self.wait_turn(id);
                if self.launches.brief() {
                    break;
                }
            }
        }
    }

    fn admit_ordered(&mut self, now: u64) {
        loop {
            let plan = self.ordered_plan(now);
            self.predicted_starts = plan.iter().map(|d| (d.id, d.start)).collect();
            for decision in &plan {
                if let Some(reason) = &decision.reason
                    && let Some(job) = self.jobs.get_mut(&decision.id)
                    && job.waited_for.as_ref() != Some(reason)
                {
                    job.waited_for = Some(reason.clone());
                }
            }
            let Some(id) = plan.iter().find(|d| d.start == Some(now)).map(|d| d.id) else {
                break;
            };
            if !self.launches.take() {
                self.wait_turn(id);
                break;
            }
            self.start(id, now);
        }
        let _ = self.checkpoint_ordering();
    }

    fn start(&mut self, id: u64, now: u64) {
        let Some(mut job) = self.jobs.get(&id).cloned() else {
            return;
        };
        self.launches.turn_came(id, &mut job.waited_for);
        self.clean_domains();
        if crate::admission::ordered(&self.objects, &job)
            || !self.pressure_control.views(&self.objects, &job).is_empty()
        {
            job.admission_snapshot = Some(self.explain(&job));
        }
        job.aggregate_domains = crate::aggregate::chain(&self.objects, job.queue_id.unwrap());
        job.state = State::Starting;
        job.workload_cgroup = cgroup_of(&self.tree, &job);
        job.started_ms = None;
        job.durability.admitted_ms = Some(now);
        job.supervisor_boot_id = Some(self.boot_id.clone());
        job.effective_spec = Some(job.spec.clone());
        job.durability.launch_gated = true;
        if let Err(error) = self.store.save_job_before_sync(&job) {
            self.finalize(
                id,
                ShimResult {
                    start_error: Some(error.to_string()),
                    finished_ms: now,
                    ..Default::default()
                },
                now,
            );
            return;
        }
        self.jobs.insert(id, job.clone());
        crate::durability::failpoint("launch-after-starting");
        if let Err(error) = self.checkpoint_ordering() {
            self.finalize(
                id,
                ShimResult {
                    start_error: Some(error.to_string()),
                    finished_ms: now,
                    ..Default::default()
                },
                now,
            );
            return;
        }
        match self.connect(&mut job) {
            Ok(link) => {
                if link != job.link || job.network.is_some() {
                    job.link = link;
                    let _ = self.store.save_job(&job);
                    self.jobs.insert(id, job.clone());
                }
            }
            Err(e) => {
                let result = ShimResult {
                    start_error: Some(format!("cannot give it its own network: {e}")),
                    finished_ms: now,
                    ..ShimResult::default()
                };
                job.state = State::Running;
                self.jobs.insert(id, job);
                self.finalize(id, result, now);
                return;
            }
        }
        let cgroup = match &self.tree {
            Some(tree) => {
                let limits = Limits::for_job(&job);
                match crate::aggregate::prepare(tree, &job.aggregate_domains)
                    .and_then(|()| Tree::create_at(job.workload_cgroup.as_ref().unwrap(), &limits))
                {
                    Ok(path) => {
                        job.applied_resources = limits
                            .files()
                            .iter()
                            .map(|(file, value)| (file.to_string(), value.to_string()))
                            .collect();
                        self.limits.insert(id, limits);
                        Some(path)
                    }
                    Err(e) => {
                        let result = ShimResult {
                            start_error: Some(format!("cannot create its cgroup: {e}")),
                            finished_ms: now,
                            ..ShimResult::default()
                        };
                        job.state = State::Running;
                        self.jobs.insert(id, job);
                        self.finalize(id, result, now);
                        return;
                    }
                }
            }
            None => None,
        };
        let spawned = spawn_shim(
            &self.store,
            id,
            cgroup.as_deref(),
            self.tree.as_ref().map(Tree::shims),
        );
        crate::durability::failpoint("launch-after-spawn");
        job.state = State::Running;
        match spawned {
            Ok((handle, gate)) => {
                job.shim_pid = Some(handle.pid);
                job.shim_start_ticks = Some(handle.start_ticks);
                job.supervisor_boot_id = Some(self.boot_id.clone());
                let confirmed = crate::durability::failpoint::fails("launch-identity-save")
                    .and_then(|()| self.store.save_job(&job))
                    .and_then(|()| {
                        crate::durability::failpoint("launch-after-identity");
                        gate.open()
                    });
                match confirmed {
                    Ok(()) => {
                        self.supervisors.insert(id, handle);
                        self.jobs.insert(id, job);
                    }
                    Err(error) => {
                        let _ = handle.signal(libc::SIGKILL);
                        self.unstarted(id, &error.to_string(), now);
                    }
                }
            }
            Err(e) => {
                self.jobs.insert(id, job);
                let result = ShimResult {
                    start_error: Some(format!("cannot start its supervisor: {e}")),
                    finished_ms: now,
                    ..ShimResult::default()
                };
                self.finalize(id, result, now);
            }
        }
    }

    fn unstarted(&mut self, id: u64, reason: &str, now: u64) {
        let Some(mut job) = self.jobs.get(&id).cloned() else {
            return;
        };
        if !crate::durability::launch::retry(&job) {
            self.finalize(id, crate::durability::launch::failed(reason, now), now);
            return;
        }
        eprintln!(
            "job {id}: {}: {reason}",
            crate::durability::message("the launch could not be recorded; the Job stays queued")
        );
        if let Some(link) = job.link.clone() {
            self.disconnect(&link, id);
        }
        if let Some(path) = &job.workload_cgroup {
            let _ = Tree::remove(path);
        }
        self.limits.remove(&id);
        self.supervisors.remove(&id);
        self.watched.remove(&id);
        crate::durability::launch::reset(&mut job);
        let _ = self.store.save_job(&job);
        self.jobs.insert(id, job);
    }

    fn finalize(&mut self, id: u64, mut result: ShimResult, now: u64) {
        let _ = self.checkpoint_ordering();
        let Some(mut job) = self.jobs.remove(&id) else {
            return;
        };
        let _ = self.checkpoint_ordering();
        let watched = self.watched.remove(&id).unwrap_or_default();
        self.supervisors.remove(&id);
        if let Some(link) = job.link.clone() {
            self.disconnect(&link, id);
        }
        self.limits.remove(&id);
        self.terminating.remove(&id);
        let mut usage = result.usage.clone();
        usage.peak_memory = usage.peak_memory.max(watched.peak_memory);
        usage.peak_pids = usage.peak_pids.max(watched.peak_pids);
        usage.written = usage.written.max(watched.written);
        let reservation = &job.reservation;
        let below_only = result.exit_code == Some(0)
            && result.own_oom == Some(0)
            && result.own_oom_kill == Some(0);
        if job.stop.is_none() && result.oom_kill > 0 && below_only {
            result.notes.push(
                crate::resource_policy::message(
                    "{count} processes in cgroups that this Job made below its own were killed for memory; the command itself exited 0",
                )
                .replace("{count}", &result.oom_kill.to_string()),
            );
        }
        if job.stop.is_none() && (result.oom_group_kill > 0 || result.oom_kill > 0) && !below_only {
            job.stop = Some(Stop {
                kind: StopKind::Memory,
                line: crate::resource_policy::message(
                    "stopped: kernel OOM kill; inspect Job, aggregate and enclosing memory controls",
                ),
            });
        }
        if job.stop.is_none() && result.pids_max_events > 0 && result.exit_code != Some(0) {
            job.stop = Some(Stop {
                kind: StopKind::Processes,
                line: format!(
                    "failed at its limit of {} processes; forks were refused",
                    reservation.vector.pids
                ),
            });
        }
        if job.started_ms.is_none() {
            job.started_ms = crate::durability::launch::confirmed_at(&self.store, &job);
        }
        job.finished_ms = Some(result.finished_ms.max(job.started_ms.unwrap_or(0)));
        job.suspension
            .observe(false, job.finished_ms.unwrap_or(now));
        job.suspension.pending = false;
        job.update_timing(now);
        job.usage = usage.clone();
        job.result = Some(result);
        job.state = job.outcome();
        crate::durability::exit::save_terminal(&self.store, &job);
        let wall_ms = job
            .finished_ms
            .unwrap_or(now)
            .saturating_sub(job.started_ms.unwrap_or(now));
        let started = job.started_ms.is_some()
            && job.result.as_ref().is_some_and(|r| r.start_error.is_none());
        if started {
            let entry = HistoryEntry {
                job_id: Some(id),
                attempt: Some(job.attempt),
                key: job.key.clone(),
                finished_ms: now,
                wall_ms,
                usage,
                stop: job.stop.as_ref().map(|s| s.kind),
                memory_limit: job.reservation.vector.memory,
                succeeded: job.succeeded(),
            };
            let _ = self.store.append_history(&entry);
            let runs = self.history.entry(entry.key.clone()).or_default();
            runs.push(entry);
            if runs.len() > HISTORY_WINDOW {
                runs.remove(0);
            }
        }
        if job.spec.declared.confine && job.spec.declared.on.is_none() {
            self.record_confinement(&job);
        }
        if job.succeeded() {
            let lines = crate::logfile::read_lines(&job.log).unwrap_or_default();
            crate::templates::remember(&self.store, &job.key, id, &lines);
        }
        if self.host_stop_pending == Some(id) {
            self.host_stop_pending = None;
        }
        let budget = self.config.output.budget();
        let ended = crate::streams::budget::recorded_in(&self.store.job_dir(id));
        if self.trims.running_stale(now) {
            let running: u64 = self
                .running_ids()
                .iter()
                .map(|id| crate::streams::budget::recorded_in(&self.store.job_dir(*id)))
                .sum();
            self.trims.running_measured(running, now);
        }
        if self.trims.due(ended, now) {
            self.store.trim_logs(budget);
            self.trims
                .scanned(budget, crate::streams::budget::measured(), now);
        }
    }

    fn record_confinement(&self, job: &Job) {
        let refusals: Vec<String> = crate::logfile::read_lines(&job.log)
            .unwrap_or_default()
            .into_iter()
            .filter(|l| crate::confine::REFUSAL_WORDS.iter().any(|w| l.contains(w)))
            .take(5)
            .collect();
        let entry = serde_json::json!({
            "id": job.id,
            "key": job.key,
            "exit": job.result.as_ref().and_then(|r| r.exit_code),
            "refused": !refusals.is_empty(),
            "refusal_lines": refusals,
        });
        let path = self.store.root.join("confine.jsonl");
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(format!("{entry}\n").as_bytes());
        }
    }

    fn stop(&mut self, id: u64, kind: StopKind, line: String, graceful: bool, now: u64) {
        let _ = self.stop_checked(id, kind, line, graceful, now);
    }

    fn stop_checked(
        &mut self,
        id: u64,
        kind: StopKind,
        line: String,
        graceful: bool,
        now: u64,
    ) -> io::Result<()> {
        let Some(mut job) = self.jobs.get(&id).cloned() else {
            return Ok(());
        };
        if job.stop.is_some() {
            return Ok(());
        }
        self.publish_start(&mut job);
        job.stop = Some(Stop { kind, line });
        job.suspension.requested = false;
        job.suspension.pending = job.workload_cgroup.is_some();
        job.state = State::Stopping;
        job.termination_deadline_ms = Some(if graceful {
            now.saturating_add(GRACE_MS)
        } else {
            now
        });
        self.store.save_job(&job)?;
        self.terminating
            .insert(id, job.termination_deadline_ms.unwrap());
        self.jobs.insert(id, job);
        let cgroup = self.cgroup(id);
        if graceful {
            if let Some(path) = &cgroup {
                crate::freezer::write(path, false)?;
            }
            signal_job(cgroup.as_deref(), self.supervisors.get(&id), libc::SIGTERM)?;
        } else {
            if let Some(path) = &cgroup {
                let _ = Tree::freeze(path);
                let _ = Tree::kill(path);
            }
            let _ = signal_job(cgroup.as_deref(), self.supervisors.get(&id), libc::SIGKILL);
        }
        Ok(())
    }

    fn collect_finished(&mut self, now: u64) {
        for id in self.running_ids() {
            let Some(mut current) = self.jobs.get(&id).cloned() else {
                continue;
            };
            if current.started_ms.is_none() {
                self.publish_start(&mut current);
                if current.started_ms.is_some() {
                    self.jobs.insert(id, current.clone());
                }
            }
            if let Some(result) = crate::durability::exit::result(&self.store, &current) {
                self.finalize(id, result, now);
                continue;
            }
            let alive = match self.supervisors.get(&id) {
                Some(handle) => crate::durability::launch::probe(handle.alive()),
                None => Some(false),
            };
            if alive == Some(false) {
                if let Some(result) = crate::durability::exit::result(&self.store, &current) {
                    self.finalize(id, result, now);
                    continue;
                }
                if let Some(result) = crate::durability::exit::recorded(&self.store, &current) {
                    if let Some(path) = self.cgroup(id) {
                        let _ = Tree::kill(&path);
                        let _ = Tree::remove(&path);
                    }
                    self.finalize(id, result, now);
                    continue;
                }
                if crate::durability::launch::never_started(&self.store, &current) {
                    self.unstarted(id, &crate::durability::launch::unconfirmed(), now);
                    continue;
                }
                if let Some(job) = self.jobs.get_mut(&id) {
                    let line = crate::durability::boot::lost(&self.store, job, &self.boot_id);
                    job.stop.get_or_insert(Stop {
                        kind: StopKind::DaemonLost,
                        line,
                    });
                }
                self.finalize(
                    id,
                    ShimResult {
                        finished_ms: now,
                        ..ShimResult::default()
                    },
                    now,
                );
            }
        }
    }

    fn sample(&mut self, id: u64) {
        let path = self.cgroup(id);
        let entry = self.watched.entry(id).or_default();
        match (&self.tree, self.backend) {
            (Some(_), Backend::Cgroup) => {
                let c = Tree::counters(path.as_ref().unwrap());
                entry.memory = c.memory_current;
                entry.peak_memory = entry.peak_memory.max(c.memory_peak.max(c.memory_current));
                entry.peak_pids = entry.peak_pids.max(c.pids_peak.max(c.pids_current));
                entry.written = entry.written.max(c.written());
            }
            _ => {
                let Some(handle) = self
                    .supervisors
                    .get(&id)
                    .filter(|handle| handle.alive().unwrap_or(false))
                else {
                    return;
                };
                let tree = procs::descendants(handle.pid, &procs::all());
                if !handle.alive().unwrap_or(false) {
                    return;
                }
                let memory: u64 = tree.iter().map(|s| procs::page_bytes(s.rss_pages)).sum();
                let threads: u64 = tree.iter().map(|s| s.threads).sum();
                let written: u64 = tree.iter().map(|s| write_bytes(s.pid)).sum();
                entry.memory = memory;
                entry.peak_memory = entry.peak_memory.max(memory);
                entry.peak_pids = entry.peak_pids.max(threads);
                entry.written = entry.written.max(written);
            }
        }
    }

    fn watch(&mut self, now: u64) {
        for id in self.running_ids() {
            self.sample(id);
            let job = &self.jobs[&id];
            let (reservation, started_ms) = (job.reservation.clone(), job.started_ms);
            let already_stopped = job.stop.is_some();
            let here = job.spec.declared.on.is_none();
            let declared_disk = here && job.spec.declared.disk.is_some();
            let declared_memory = here && job.spec.declared.memory.is_some();
            let (memory, pids, written) = self
                .watched
                .get(&id)
                .map_or((0, 0, 0), |w| (w.memory, w.peak_pids, w.written));
            if let Some(&deadline) = self.terminating.get(&id) {
                if now >= deadline {
                    let cgroup = self.cgroup(id);
                    if let Some(path) = &cgroup {
                        let _ = Tree::kill(path);
                    }
                    let _ = signal_job(cgroup.as_deref(), self.supervisors.get(&id), libc::SIGKILL);
                }
                continue;
            }
            if already_stopped {
                continue;
            }
            if let (Some(tree), Some(limits)) = (&self.tree, self.limits.get(&id)) {
                if let Some(line) = self.changed_domains(tree, &job.aggregate_domains) {
                    self.stop(id, StopKind::LimitChanged, line, false, now);
                    continue;
                }
                if let Some((file, expected, actual)) = self.changed_limits(
                    &crate::aggregate::leaf(tree, &job.aggregate_domains, id),
                    limits,
                ) {
                    let line = format!(
                        "stopped: its {file} was changed from {expected} to {actual} by something other than the service"
                    );
                    self.stop(id, StopKind::LimitChanged, line, false, now);
                    continue;
                }
                let counters =
                    Tree::counters(&crate::aggregate::leaf(tree, &job.aggregate_domains, id));
                if counters.pids_max_events > 0 {
                    let line = format!(
                        "stopped: reached {} processes, its limit",
                        reservation.vector.pids
                    );
                    self.stop(id, StopKind::Processes, line, false, now);
                    continue;
                }
            } else {
                if declared_memory && memory > reservation.vector.memory {
                    let line = format!(
                        "stopped: memory reached {}, past its reservation of {}",
                        format_bytes(memory),
                        format_bytes(reservation.vector.memory)
                    );
                    self.stop(id, StopKind::Memory, line, false, now);
                    continue;
                }
                if (job.policy.legacy() || (here && job.spec.declared.pids.is_some()))
                    && pids > reservation.vector.pids
                {
                    let line = format!(
                        "stopped: reached {pids} processes and threads, past its limit of {}",
                        reservation.vector.pids
                    );
                    self.stop(id, StopKind::Processes, line, false, now);
                    continue;
                }
            }
            if declared_disk && written > reservation.disk {
                let line = format!(
                    "stopped: wrote {}, past its disk reservation of {}",
                    format_bytes(written),
                    format_bytes(reservation.disk)
                );
                self.stop(id, StopKind::Disk, line, false, now);
                continue;
            }
            if let (Some(limit), Some(started)) = (reservation.wall_limit_ms, started_ms)
                && now.saturating_sub(started) > limit
            {
                let line = format!(
                    "stopped: ran past its time limit of {}",
                    format_duration_ms(limit)
                );
                self.stop(id, StopKind::WallTime, line, true, now);
            }
        }
        if self.config.profile.legacy() {
            self.watch_host(now);
        }
    }

    fn largest_memory_job(&self) -> Option<(u64, u64)> {
        self.running_ids()
            .into_iter()
            .filter(|id| self.jobs[id].stop.is_none())
            .map(|id| (id, self.watched.get(&id).map_or(0, |w| w.memory)))
            .max_by_key(|&(_, memory)| memory)
    }

    fn watch_host(&mut self, now: u64) {
        if self.host_stop_pending.is_none() {
            let meminfo = host::read_meminfo();
            let floor = host::host_memory_floor(meminfo.total);
            if meminfo.below_floor()
                && let Some((id, memory)) = self.largest_memory_job()
            {
                let line = format!(
                    "stopped: the host's available memory fell to {}, below the floor of {}; this job held {}, the most of any job",
                    format_bytes(meminfo.available),
                    format_bytes(floor),
                    format_bytes(memory)
                );
                self.stop(id, StopKind::HostMemory, line, false, now);
                self.host_stop_pending = Some(id);
            }
        }
        let pressure = host::read_memory_pressure();
        if pressure.full_avg10 >= PRESSURE_LIMIT_PERCENT {
            let since = *self.pressure_since.get_or_insert(now);
            if now.saturating_sub(since) >= PRESSURE_DURATION_MS
                && self.host_stop_pending.is_none()
                && let Some((id, memory)) = self.largest_memory_job()
            {
                let line = format!(
                    "stopped: memory pressure stayed at {:.0} % (full, avg10) for {}; this job held {}, the most of any job",
                    pressure.full_avg10,
                    format_duration_ms(now - since),
                    format_bytes(memory)
                );
                self.stop(id, StopKind::MemoryPressure, line, false, now);
                self.host_stop_pending = Some(id);
                self.pressure_since = None;
            }
        } else {
            self.pressure_since = None;
        }
        let mut by_mount: HashMap<PathBuf, Vec<u64>> = HashMap::new();
        for id in self.running_ids() {
            if let Some((mount, _)) = host::space_of(&self.jobs[&id].spec.cwd) {
                by_mount.entry(mount.mount_point).or_default().push(id);
            }
        }
        for (mount_point, ids) in by_mount {
            let Some((_, space)) = host::space_of(&mount_point) else {
                continue;
            };
            let floor = host::filesystem_floor(space.size);
            if space.free >= floor {
                continue;
            }
            let writer = ids
                .into_iter()
                .filter(|id| self.jobs[id].stop.is_none())
                .map(|id| (id, self.watched.get(&id).map_or(0, |w| w.written)))
                .max_by_key(|&(_, written)| written);
            if let Some((id, written)) = writer {
                let line = format!(
                    "stopped: {} fell to {} free, below the floor of {}; this job had written {}, the most of any job there",
                    mount_point.display(),
                    format_bytes(space.free),
                    format_bytes(floor),
                    format_bytes(written)
                );
                self.stop(id, StopKind::FilesystemFloor, line, false, now);
            }
        }
    }

    fn cancel_member(
        &mut self,
        member: &crate::cancellation::Member,
        now: u64,
        batch: Option<&mut Vec<Job>>,
    ) -> io::Result<Option<State>> {
        let Some(mut job) = self.load(member.id) else {
            return Err(io::Error::other(crate::cancellation::message(
                "invalid Job record",
            )));
        };
        if job.attempt != member.attempt {
            return Err(io::Error::other(crate::cancellation::message(
                "selected attempt is no longer current",
            )));
        }
        if job.state.terminal() {
            return Ok(Some(job.state));
        }
        if job.state.editable() {
            job.stop = Some(Stop {
                kind: StopKind::Cancelled,
                line: crate::cancellation::message("cancelled before it started"),
            });
            job.state = State::Cancelled;
            job.finished_ms = Some(now);
            job.result = Some(ShimResult {
                finished_ms: now,
                ..ShimResult::default()
            });
            job.update_timing(now);
            self.jobs.remove(&job.id);
            self.predicted_starts.remove(&job.id);
            match batch {
                Some(batch) => batch.push(job),
                None => {
                    let _ = fs::remove_file(crate::pacing::staged_record(&self.store, job.id));
                    if let Err(error) = self.store.save_job(&job) {
                        self.restore_unsaved(&[job]);
                        return Err(error);
                    }
                }
            }
            return Ok(Some(State::Cancelled));
        }
        if job.stop.is_none() {
            self.stop_checked(
                job.id,
                StopKind::Cancelled,
                crate::cancellation::message("stopped: cancellation requested"),
                true,
                now,
            )?;
        } else if job
            .stop
            .as_ref()
            .is_some_and(|stop| stop.kind == StopKind::Cancelled)
        {
            let cgroup = cgroup_of(&self.tree, &job);
            if let Some(path) = &cgroup {
                crate::freezer::write(path, false)?;
            }
            signal_job(
                cgroup.as_deref(),
                self.supervisors.get(&job.id),
                libc::SIGTERM,
            )?;
        }
        Ok(Some(State::Stopping))
    }

    fn restore_unsaved(&mut self, jobs: &[Job]) {
        for job in jobs {
            if let Some(recorded) = self.store.load_job(job.id)
                && !recorded.state.terminal()
            {
                self.jobs.insert(recorded.id, recorded);
            }
        }
    }

    fn reconcile_cancellations(&mut self, now: u64, asked: Option<&str>) {
        let pending: Vec<_> = self
            .cancellations
            .values()
            .filter(|op| !op.complete)
            .filter(|op| {
                asked == Some(op.operation.as_str()) || self.cancel_retries.due(&op.operation, now)
            })
            .cloned()
            .collect();
        for mut operation in pending {
            let recovering = self.cancel_retries.recovering(&operation.operation);
            if let Err(error) = crate::cancellation::save(&self.store, &operation) {
                operation.persistence_error = Some(error.to_string());
                self.cancel_retries
                    .failure(&operation.operation, now, &error.to_string());
                self.cancellations
                    .insert(operation.operation.clone(), operation);
                continue;
            }
            if recovering {
                for member in &operation.selection.members {
                    let _ = fs::remove_file(crate::pacing::staged_record(&self.store, member.id));
                }
            }
            let waiting = operation
                .selection
                .members
                .iter()
                .filter(|member| {
                    self.jobs
                        .get(&member.id)
                        .is_some_and(|job| job.attempt == member.attempt && job.state.editable())
                })
                .count();
            let mut batch = (waiting >= crate::pacing::BATCH_FROM).then(Vec::new);
            let applied: HashMap<(u64, u64), crate::cancellation::Outcome> = operation
                .results
                .iter()
                .filter(|result| !result.pending)
                .map(|result| ((result.id, result.attempt), result.clone()))
                .collect();
            operation.results = operation
                .selection
                .members
                .iter()
                .map(|member| {
                    if let Some(applied) = applied.get(&(member.id, member.attempt)) {
                        return applied.clone();
                    }
                    let result = self.cancel_member(member, now, batch.as_mut());
                    let superseded = self
                        .load(member.id)
                        .is_none_or(|job| job.attempt != member.attempt);
                    crate::cancellation::Outcome {
                        id: member.id,
                        attempt: member.attempt,
                        state: result.as_ref().ok().cloned().flatten(),
                        pending: result.is_err() && !superseded,
                        error: result.err().map(|error| error.to_string()),
                    }
                })
                .collect();
            let mut failure = None;
            if let Some(batch) = batch
                && let Err(error) = crate::pacing::settle_records(&self.store, &batch)
            {
                let text = format!(
                    "{}: {error}",
                    crate::pacing::message("cannot make the cancelled records durable")
                );
                self.restore_unsaved(&batch);
                let staged: std::collections::HashSet<u64> =
                    batch.iter().map(|job| job.id).collect();
                for result in &mut operation.results {
                    if staged.contains(&result.id) {
                        result.state = None;
                        result.pending = true;
                        result.error = Some(text.clone());
                    }
                }
                failure = Some(text);
            }
            if recovering && failure.is_none() {
                let ended: Vec<Job> = operation
                    .selection
                    .members
                    .iter()
                    .filter(|member| !self.jobs.contains_key(&member.id))
                    .filter_map(|member| self.store.load_job(member.id))
                    .filter(|job| job.state.terminal())
                    .collect();
                let ids: Vec<u64> = ended.iter().map(|job| job.id).collect();
                if let Err(error) = crate::pacing::sync_job_directories(&self.store, &ids)
                    .and_then(|()| crate::operations::journal::ensure_terminal(&ended).map(|_| ()))
                {
                    failure = Some(error.to_string());
                }
            }
            crate::operations::journal::flush();
            if failure.is_none() {
                failure = operation
                    .results
                    .iter()
                    .find(|result| result.pending)
                    .map(|result| result.error.clone().unwrap_or_default());
            }
            operation.complete = failure.is_none();
            operation.persistence_error = None;
            if let Err(error) = crate::cancellation::save(&self.store, &operation) {
                operation.complete = false;
                operation.persistence_error = Some(error.to_string());
                failure = Some(error.to_string());
            }
            match &failure {
                Some(error) => {
                    if operation.persistence_error.is_none()
                        && operation.results.iter().all(|result| !result.pending)
                    {
                        operation.persistence_error = Some(error.clone());
                    }
                    self.cancel_retries
                        .failure(&operation.operation, now, error)
                }
                None => self.cancel_retries.done(&operation.operation),
            }
            self.cancellations
                .insert(operation.operation.clone(), operation);
        }
    }

    fn commit_cancellation(
        &mut self,
        selection: crate::cancellation::Selection,
        now: u64,
    ) -> io::Result<crate::cancellation::Operation> {
        let mut operation = crate::cancellation::Operation::new(selection, now);
        let key = operation.operation.clone();
        if !self.cancellations.contains_key(&key) {
            let saved = crate::cancellation::save(&self.store, &operation);
            if let Err(error) = saved {
                if self
                    .store
                    .root
                    .join("cancellations")
                    .join(format!("{key}.json"))
                    .exists()
                {
                    operation.persistence_error = Some(error.to_string());
                    self.cancellations.insert(key, operation.clone());
                    return Ok(operation);
                }
                return Err(error);
            }
            self.cancel_retries.created(&key);
            self.cancellations.insert(key.clone(), operation);
        }
        self.reconcile_cancellations(now, Some(&key));
        Ok(self.cancellations[&key].clone())
    }

    fn cancel(&mut self, id: u64, _session: &str, now: u64) -> Response {
        let result = (|| -> io::Result<Response> {
            let selection = crate::cancellation::select(
                &self.store,
                &self.objects,
                &crate::cancellation::Target::Job { id },
                false,
            )?;
            let operation = self.commit_cancellation(selection, now)?;
            if !operation.complete
                || operation
                    .results
                    .iter()
                    .any(|result| result.error.is_some())
            {
                return Ok(Response::Cancellation { operation });
            }
            self.admit(now);
            self.load(id)
                .map(|job| Response::Cancelled { job })
                .ok_or_else(|| io::Error::other(crate::cancellation::message("invalid Job record")))
        })();
        result.unwrap_or_else(|error| Response::Error {
            message: error.to_string(),
        })
    }

    fn queue_view(&self) -> QueueView {
        QueueView {
            jobs: self
                .jobs
                .values()
                .map(|job| QueueEntry {
                    job: {
                        let mut snapshot = job.clone();
                        snapshot.update_timing(now_ms());
                        snapshot
                    },
                    predicted_start_ms: self.predicted_starts.get(&job.id).copied().flatten(),
                })
                .collect(),
            queues: self.queues.values().cloned().collect(),
            backend: self.backend,
            pool: self.pool,
        }
    }

    fn link_plan(&self, job: &Job) -> Option<(String, Option<u64>, Option<u64>, Net)> {
        let settings = job
            .spec
            .queue
            .as_ref()
            .and_then(|q| self.queues.get(q))
            .map(|q| q.settings.clone())
            .unwrap_or_default();
        if job.spec.declared.on.is_some() {
            return None;
        }
        let own = job.spec.declared.bandwidth.or(settings.job_bandwidth);
        let net = job.spec.declared.net.clone().unwrap_or(Net::Host);
        let queue_link =
            settings.bandwidth.is_some() || settings.net.as_ref().is_some_and(Net::needs_link);
        match &job.spec.queue {
            Some(queue) if queue_link => {
                let rate = settings.bandwidth;
                let cap = match (own, rate) {
                    (Some(own), Some(rate)) => Some(own.min(rate)),
                    (own, rate) => own.or(rate),
                };
                Some((queue.clone(), rate, cap, net))
            }
            _ if own.is_some() || net.needs_link() => {
                Some((format!("job-{}", job.id), own, own, net))
            }
            _ => None,
        }
    }

    fn linked_jobs(&self, name: &str, except: u64) -> Vec<(u32, Option<u64>)> {
        self.jobs
            .values()
            .filter(|j| j.id != except && j.state.active())
            .filter_map(|j| j.link.as_ref())
            .filter(|l| l.name == name)
            .map(|l| (l.job.index, l.cap))
            .collect()
    }

    fn rates(link: &crate::link::Link, jobs: &[(u32, Option<u64>)]) -> Vec<(u32, u64)> {
        jobs.iter()
            .map(|&(index, cap)| (index, cap.or(link.rate).unwrap_or(u64::MAX)))
            .collect()
    }

    fn connect(&mut self, job: &mut Job) -> Result<Option<crate::model::LinkRef>, String> {
        job.network = crate::netpolicy::apply(&self.config.network, job)?;
        let route = job.network.as_ref().map(crate::netpolicy::Applied::route);
        let plan = match &route {
            Some(route) => route
                .as_ref()
                .map(|r| (r.name.clone(), r.rate, r.cap, r.egress.clone())),
            None => self.link_plan(job),
        };
        let Some((name, rate, cap, egress)) = plan else {
            return Ok(None);
        };
        let route = route.flatten();
        let filter = route
            .as_ref()
            .map(|route| route.filter.clone())
            .unwrap_or_default();
        let others = self.linked_jobs(&name, job.id);
        let existing = self
            .links
            .get(&name)
            .filter(|l| crate::link::alive(l))
            .filter(|l| l.egress.as_ref() == Some(&egress) || !others.is_empty())
            .cloned();
        let link = match existing {
            Some(mut link) => {
                crate::link::admit(&link, &mut self.unisolated_links)?;
                if link.rate != rate && link.rate.is_some() && rate.is_some() {
                    link.rate = rate;
                    crate::link::set_total(&link).map_err(|e| e.to_string())?;
                    let _ = self.store.save_link(&link);
                }
                link
            }
            None => {
                self.unisolated_links.remove(&name);
                if let Some(stale) = self.links.remove(&name) {
                    crate::link::destroy(&stale);
                }
                let link =
                    crate::link::create(&name, rate, &egress, &self.store.link_log(&name), &filter)
                        .map_err(|e| e.to_string())?;
                let _ = self.store.save_link(&link);
                link
            }
        };
        let index = (crate::link::FIRST_INDEX..=crate::link::LAST_INDEX)
            .find(|i| others.iter().all(|(used, _)| used != i))
            .ok_or_else(|| format!("link {name} has no free address for another job"))?;
        let attached = crate::link::attach(&link, index).map_err(|e| e.to_string())?;
        let mut all = others;
        all.push((index, cap));
        crate::link::set_rates(&link, &Self::rates(&link, &all)).map_err(|e| e.to_string())?;
        let (env, resolver) = crate::link::job_view(&link).map_err(|e| e.to_string())?;
        let resolver = route.and_then(|route| route.resolver).unwrap_or(resolver);
        let egress = link.egress.clone();
        let link_rate = link.rate;
        self.links.insert(name.clone(), link);
        Ok(Some(crate::model::LinkRef {
            name,
            job: attached,
            cap,
            link_rate,
            egress,
            env,
            resolver,
        }))
    }

    fn disconnect(&mut self, reference: &crate::model::LinkRef, id: u64) {
        let Some(link) = self.links.get(&reference.name).cloned() else {
            return;
        };
        crate::link::detach(&link, &reference.job);
        let others = self.linked_jobs(&reference.name, id);
        if reference.name.starts_with("job-")
            || (others.is_empty() && !self.queue_has_link(&reference.name))
        {
            crate::link::destroy(&link);
            self.links.remove(&reference.name);
            self.store.remove_link(&reference.name);
        } else {
            let _ = crate::link::set_rates(&link, &Self::rates(&link, &others));
        }
    }

    fn drop_link(&mut self, name: &str) {
        if let Some(link) = self.links.remove(name) {
            crate::link::destroy(&link);
            self.store.remove_link(name);
        }
    }

    fn sweep_net_secrets(&self) {
        let used: Vec<PathBuf> = self
            .queues
            .values()
            .filter_map(|q| q.settings.net_secret_file.clone())
            .chain(
                self.objects
                    .nodes
                    .values()
                    .filter_map(|n| n.config.get("net_secret_file"))
                    .filter_map(|v| v.as_str().map(PathBuf::from)),
            )
            .chain(
                self.jobs
                    .values()
                    .filter_map(|j| j.spec.declared.net_secret_file.clone()),
            )
            .collect();
        crate::netsecret::sweep(&self.store.root, &used);
    }

    fn queue_has_link(&self, name: &str) -> bool {
        self.queues.get(name).is_some_and(|q| {
            q.settings.bandwidth.is_some() || q.settings.net.as_ref().is_some_and(Net::needs_link)
        })
    }

    fn lanes(&self) -> HashMap<String, Lane> {
        self.queues
            .values()
            .map(|q| {
                (
                    q.name.clone(),
                    Lane {
                        parallel: q.parallel,
                        paused: q.paused,
                        cap: Vector {
                            cores_milli: q.settings.cores_milli.unwrap_or(u64::MAX),
                            memory: q.settings.memory.unwrap_or(u64::MAX),
                            pids: u64::MAX,
                        },
                    },
                )
            })
            .collect()
    }

    fn queue_names(&self) -> String {
        if self.queues.is_empty() {
            "there are no queues".to_string()
        } else {
            format!(
                "the queues are {}",
                self.queues.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        }
    }

    fn jobs_in(&self, name: &str, state: State) -> Vec<u64> {
        self.jobs
            .values()
            .filter(|j| {
                (if state == State::Running {
                    j.state.active()
                } else if state == State::Queued {
                    j.state.editable()
                } else {
                    j.state == state
                }) && j.spec.queue.as_deref() == Some(name)
            })
            .map(|j| j.id)
            .collect()
    }

    fn queue_reason(&self, id: u64, hold: QueueHold) -> Option<String> {
        let name = self.jobs.get(&id)?.spec.queue.clone()?;
        let queue = self.queues.get(&name)?;
        Some(match hold {
            QueueHold::Paused => format!("queue {name}, which is paused"),
            QueueHold::Behind(before) => format!("job {before}, before it in queue {name}"),
            QueueHold::Places => {
                let running = self.jobs_in(&name, State::Running);
                let places = match queue.parallel {
                    Some(1) => "its 1 place".to_string(),
                    Some(n) => format!("its {n} places"),
                    None => "its places".to_string(),
                };
                format!(
                    "a place in queue {name}; {places} held by {}",
                    job_list(&running)
                )
            }
            QueueHold::Share => format!(
                "the {} of queue {name}, held by {}",
                describe_caps(&queue.settings),
                job_list(&self.jobs_in(&name, State::Running))
            ),
        })
    }

    fn queue_add(&mut self, name: String, mut change: QueueChange) -> Response {
        if let Err(message) = check_queue_name(&name) {
            return Response::Error { message };
        }
        if let Err(message) = crate::netsecret::settle(&mut change.settings, &self.store.root) {
            return Response::Error { message };
        }
        if self.queues.contains_key(&name) {
            return Response::Error {
                message: format!(
                    "queue {name} exists already; change it with: job queue set {name} --parallel N"
                ),
            };
        }
        let queue = Queue {
            name: name.clone(),
            parallel: change.parallel.and_then(Parallel::limit),
            paused: change.paused.unwrap_or(false),
            draining: false,
            created_ms: now_ms(),
            settings: Settings::default(),
        };
        let queue = match apply_settings(queue, &change.settings) {
            Ok(queue) => queue,
            Err(message) => return Response::Error { message },
        };
        if let Err(message) = self.config.network.settle(&queue.settings) {
            return Response::Error { message };
        }
        self.save_queue(queue, "added", change.parallel == Some(Parallel::All))
    }

    fn save_queue(&mut self, queue: Queue, verb: &str, unlimited: bool) -> Response {
        let mut objects = self.objects.clone();
        let id = match objects.resolve(&queue.name) {
            Ok(id) => id,
            Err(_) => {
                match objects.create(&queue.name, crate::objects::Kind::Queue, Default::default()) {
                    Ok(id) => id,
                    Err(message) => return Response::Error { message },
                }
            }
        };
        objects.update_queue(id, &queue);
        crate::netsecret::shadow(&mut objects.nodes.get_mut(&id).unwrap().config);
        if unlimited {
            objects.nodes.get_mut(&id).unwrap().config.insert(
                "max_running".to_owned(),
                serde_json::Value::from("unlimited"),
            );
        }
        if let Err(e) = objects.save(&self.store) {
            return Response::Error {
                message: format!("cannot record queue {}: {e}", queue.name),
            };
        }
        let line = format!("queue {} {verb}: {}", queue.name, describe_queue(&queue));
        self.objects = objects;
        self.queues = self.objects.queues();
        self.sweep_net_secrets();
        self.admit(now_ms());
        Response::Done { line }
    }

    fn existing_queue(&self, name: &str) -> Result<Queue, String> {
        self.queues
            .get(name)
            .cloned()
            .ok_or_else(|| format!("there is no queue {name}; {}", self.queue_names()))
    }

    fn queue_set(&mut self, name: String, mut change: QueueChange) -> Response {
        let current = match self.existing_queue(&name) {
            Ok(queue) => queue,
            Err(message) => return Response::Error { message },
        };
        if let Err(message) = crate::netsecret::settle(&mut change.settings, &self.store.root) {
            return Response::Error { message };
        }
        let change_names_net = change.settings.net.is_some();
        let change_names_secret = change.settings.net_secret_file.is_some();
        match apply_settings(current, &change.settings) {
            Ok(queue) => {
                if let Err(message) = self.config.network.settle(&queue.settings) {
                    return Response::Error { message };
                }
            }
            Err(message) => return Response::Error { message },
        }
        let id = match self.objects.resolve(&name) {
            Ok(id) => id,
            Err(message) => return Response::Error { message },
        };
        let mut graph = self.objects.clone();
        let node = graph.nodes.get_mut(&id).unwrap();
        if let Some(parallel) = change.parallel {
            node.config.insert(
                "max_running".to_owned(),
                parallel.limit().map_or(
                    serde_json::Value::from("unlimited"),
                    serde_json::Value::from,
                ),
            );
        }
        if let Some(paused) = change.paused {
            node.paused = paused;
        }
        for (key, value) in serde_json::to_value(change.settings)
            .unwrap()
            .as_object()
            .unwrap()
        {
            if !value.is_null() {
                node.config.insert(key.clone(), value.clone());
            }
        }
        if change_names_net {
            if !change_names_secret {
                node.config.remove("net_secret_file");
            }
            crate::netsecret::shadow(&mut node.config);
        }
        if let Err(message) = self.config.network.validate_graph(&graph) {
            return Response::Error { message };
        }
        if let Err(e) = graph.save(&self.store) {
            return Response::Error {
                message: format!("cannot record queue {name}: {e}"),
            };
        }
        self.objects = graph;
        self.queues = self.objects.queues();
        self.sweep_net_secrets();
        self.admit(now_ms());
        Response::Done {
            line: format!(
                "queue {name} changed: {}",
                describe_queue(&self.queues[&name])
            ),
        }
    }

    fn queue_remove(&mut self, name: String, drain: bool) -> Response {
        let mut queue = match self.existing_queue(&name) {
            Ok(queue) => queue,
            Err(message) => return Response::Error { message },
        };
        let waiting = self.jobs_in(&name, State::Queued);
        let running = self.jobs_in(&name, State::Running);
        if waiting.is_empty() && running.is_empty() {
            return match self.remove_queue_object(&name) {
                Ok(()) => {
                    self.queues.remove(&name);
                    self.drop_link(&name);
                    Response::Done {
                        line: format!("queue {name} removed"),
                    }
                }
                Err(e) => Response::Error {
                    message: format!("cannot remove queue {name}: {e}"),
                },
            };
        }
        let holding = format!(
            "{} waiting and {} running",
            job_list(&waiting),
            job_list(&running)
        );
        if !drain {
            return Response::Error {
                message: format!(
                    "queue {name} holds jobs ({holding}); remove it when they end with: job queue rm {name} --when-empty, or empty it first with: job queue clear {name}"
                ),
            };
        }
        queue.draining = true;
        let mut graph = self.objects.clone();
        let id = match graph.resolve(&name) {
            Ok(id) => id,
            Err(message) => return Response::Error { message },
        };
        graph.nodes.get_mut(&id).unwrap().draining = true;
        if let Err(e) = graph.save(&self.store) {
            return Response::Error {
                message: format!("cannot record queue {name}: {e}"),
            };
        }
        self.objects = graph;
        self.queues = self.objects.queues();
        Response::Done {
            line: format!(
                "queue {name} takes no new jobs and is removed when its jobs end ({holding})"
            ),
        }
    }

    fn queue_clear(&mut self, name: String) -> Response {
        if let Err(message) = self.existing_queue(&name) {
            return Response::Error { message };
        }
        let now = now_ms();
        let waiting = self.jobs_in(&name, State::Queued);
        if !waiting.is_empty() {
            let selection = crate::cancellation::Selection {
                scope_id: self.objects.resolve(&name).ok(),
                members: waiting
                    .iter()
                    .filter_map(|id| self.jobs.get(id))
                    .map(crate::cancellation::Member::from)
                    .collect(),
                set: None,
            };
            match self.commit_cancellation(selection, now) {
                Ok(operation)
                    if operation.complete
                        && operation.results.iter().all(|r| r.error.is_none()) => {}
                Ok(operation) => return Response::Cancellation { operation },
                Err(error) => {
                    return Response::Error {
                        message: error.to_string(),
                    };
                }
            }
        }
        let running = self.jobs_in(&name, State::Running);
        let mut line = if waiting.is_empty() {
            format!("queue {name} had no waiting jobs")
        } else {
            format!("queue {name} cleared: cancelled {}", job_list(&waiting))
        };
        if !running.is_empty() {
            line.push_str(&format!(
                "; still running: {}, stopped only by job cancel",
                job_list(&running)
            ));
        }
        self.admit(now);
        Response::Done { line }
    }

    fn remove_queue_object(&mut self, name: &str) -> io::Result<()> {
        let mut graph = self.objects.clone();
        graph
            .change(
                crate::objects::Kind::Queue,
                &crate::objects::Operation::Remove {
                    path: name.to_owned(),
                },
            )
            .map_err(io::Error::other)?;
        graph.save(&self.store)?;
        self.objects = graph;
        Ok(())
    }

    fn remove_drained_queues(&mut self) {
        let drained: Vec<String> = self
            .queues
            .values()
            .filter(|q| q.draining)
            .filter(|q| {
                !self
                    .jobs
                    .values()
                    .any(|j| j.spec.queue.as_ref() == Some(&q.name))
            })
            .map(|q| q.name.clone())
            .collect();
        for name in drained {
            if self.remove_queue_object(&name).is_ok() {
                self.queues.remove(&name);
                self.drop_link(&name);
            }
        }
    }

    fn host_info(&self) -> HostInfo {
        let meminfo = host::read_meminfo();
        let count = |state: State| {
            self.jobs
                .values()
                .filter(|j| {
                    if state == State::Running {
                        j.state.active()
                    } else {
                        j.state.editable()
                    }
                })
                .count()
        };
        let io_devices = crate::io_policy::capabilities(self.tree.as_ref());
        HostInfo {
            isolation_controls: Some(crate::isolation::kernel::capabilities()),
            security_controls: crate::security::kernel::capabilities(),
            process_controls: crate::process_policy::kernel::capabilities(),
            output_protocol: Some(crate::streams::PROTOCOL),
            io_devices: io_devices.clone(),
            resource_controls: crate::resource_policy::Controls::default()
                .fields()
                .keys()
                .filter_map(|name| {
                    crate::resource_policy::kernel_file(name).map(|file| {
                        (
                            file.to_owned(),
                            self.tree.as_ref().is_some_and(|tree| tree.supports(file))
                                && match file {
                                    "io.max" => io_devices.iter().any(|device| device.io_max),
                                    "io.weight" => io_devices.iter().any(|device| device.io_weight),
                                    "io.bfq.weight" => {
                                        io_devices.iter().any(|device| device.io_bfq_weight)
                                    }
                                    _ => true,
                                },
                        )
                    })
                })
                .collect(),
            freezer: self.tree.as_ref().is_some_and(|tree| {
                crate::freezer::observed(&tree.jobs()).is_ok()
                    && tree.jobs().join("cgroup.freeze").exists()
            }),
            protocol: crate::model::PROTOCOL,
            terminal_protocol: Some(crate::terminal::PROTOCOL),
            pidfd: true,
            service: crate::service::published(),
            unreadable_records: crate::streams::budget::unreadable(),
            activity: Some(crate::pacing::activity()),
            health: Some(crate::operations::health::report(
                &self.store,
                &self.jobs,
                self.backend,
            )),
            surrounding_limits: Some(crate::operations::limits::read_own()),
            filesystem_quota: Some(crate::cli2::capability::filesystem_quota()),
            network: crate::netpolicy::report(&self.config.network),
            version: version(),
            hostname: host::hostname(),
            kernel: host::kernel_release(),
            cores: self.cores,
            memory_total: meminfo.total,
            memory_available: meminfo.available,
            backend: self.backend,
            pool: self.pool,
            devices: self.devices.clone(),
            queues: self.queues.values().cloned().collect(),
            running: count(State::Running),
            waiting: count(State::Queued),
        }
    }

    fn reload_removed_state(&mut self) -> io::Result<()> {
        self.objects = crate::objects::Graph::load(&self.store)?;
        self.queues = self.objects.queues();
        self.links = self
            .store
            .load_links()
            .into_iter()
            .map(|link| (link.name.clone(), link))
            .collect();
        self.history.clear();
        for entry in self.store.load_history() {
            let runs = self.history.entry(entry.key.clone()).or_default();
            runs.push(entry);
            if runs.len() > HISTORY_WINDOW {
                runs.remove(0);
            }
        }
        Ok(())
    }

    fn reconcile_removal(&mut self) -> io::Result<()> {
        if crate::removal::recover(&self.store)? {
            self.reload_removed_state()?;
        }
        Ok(())
    }

    pub fn tick(&mut self) {
        let updates_ready = self.reconcile_resource_updates(true).is_ok() && !self.update_pending();
        let removal_ready = self.reconcile_removal().is_ok();
        let now = now_ms();
        self.reconcile_cancellations(now, None);
        self.collect_finished(now);
        if updates_ready {
            self.clean_domains();
        }
        for id in self.running_ids() {
            self.reconcile_freezer(id, now);
        }
        self.watch(now);
        if removal_ready {
            let jobs = &self.jobs;
            self.launches.keep(|id| jobs.contains_key(&id));
            self.admit_within(now, crate::pacing::STARTS_PER_PASS, false);
        }
        if removal_ready && updates_ready {
            self.remove_drained_queues();
        }
        self.hold_host_cores();
    }

    pub fn host_cores_held(&self) -> Option<(u64, String)> {
        self.queues
            .values()
            .filter(|q| !self.jobs_in(&q.name, State::Running).is_empty())
            .filter_map(|q| Some((q.settings.host_cores_milli?, q.name.clone())))
            .min()
    }

    fn hold_host_cores(&mut self) {
        let Some(tree) = &self.tree else {
            return;
        };
        let held = self.host_cores_held().map(|(milli, _)| milli);
        if self.host_cores_written == Some(held)
            || (held.is_none()
                && self.host_cores_written.is_none()
                && !self.config.profile.legacy())
        {
            return;
        }
        if std::fs::write(tree.jobs().join("cpu.max"), cpu_max(held)).is_ok() {
            self.host_cores_written = Some(held);
        }
    }
}

pub fn cpu_max(held_milli: Option<u64>) -> String {
    const PERIOD_US: u64 = 100_000;
    match held_milli {
        Some(milli) => format!("{} {PERIOD_US}", (milli * PERIOD_US / MILLI).max(1000)),
        None => format!("max {PERIOD_US}"),
    }
}

pub fn version() -> String {
    match option_env!("JOB_COMMIT") {
        Some(commit) => format!("{} ({commit})", env!("CARGO_PKG_VERSION")),
        None => env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn job_list(ids: &[u64]) -> String {
    match ids {
        [] => "no job".to_string(),
        [one] => format!("job {one}"),
        _ => format!(
            "jobs {}",
            ids.iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

const QUEUE_VERBS: [&str; 8] = [
    "add", "set", "rm", "clear", "pause", "resume", "help", "default",
];
const QUEUE_NAME_LIMIT: usize = 32;

pub fn check_queue_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= QUEUE_NAME_LIMIT
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        && name.as_bytes()[0].is_ascii_lowercase();
    if !valid {
        return Err(format!(
            "`{name}` is not a queue name; a name starts with a lowercase letter and has at most {QUEUE_NAME_LIMIT} lowercase letters, digits, - and _"
        ));
    }
    if QUEUE_VERBS.contains(&name) {
        return Err(format!(
            "`{name}` is a word of the queue command and cannot name a queue"
        ));
    }
    Ok(())
}

fn apply_settings(mut queue: Queue, change: &Settings) -> Result<Queue, String> {
    if let Some(dir) = &change.dir {
        let here = change.on.is_none() && queue.settings.on.is_none();
        if here && (!dir.is_absolute() || !dir.is_dir()) {
            return Err(format!("the directory {} does not exist", dir.display()));
        }
        queue.settings.dir = Some(dir.clone());
    }
    if change.cores_milli.is_some() {
        queue.settings.cores_milli = change.cores_milli;
    }
    if change.memory.is_some() {
        queue.settings.memory = change.memory;
    }
    if change.net.is_some() {
        queue.settings.net = change.net.clone();
        queue.settings.net_secret_file = change.net_secret_file.clone();
    } else if change.net_secret_file.is_some() {
        queue.settings.net_secret_file = change.net_secret_file.clone();
    }
    if change.on.is_some() {
        queue.settings.on = change.on.clone();
    }
    if change.bandwidth.is_some() {
        queue.settings.bandwidth = change.bandwidth;
    }
    if change.job_bandwidth.is_some() {
        queue.settings.job_bandwidth = change.job_bandwidth;
    }
    if change.monitor.is_some() {
        queue.settings.monitor = change.monitor.clone();
    }
    if change.host_cores_milli.is_some() {
        queue.settings.host_cores_milli = change.host_cores_milli;
    }
    Ok(queue)
}

fn describe_caps(settings: &Settings) -> String {
    let mut caps = Vec::new();
    if let Some(cores) = settings.cores_milli {
        caps.push(crate::resources::format_cores(cores));
    }
    if let Some(memory) = settings.memory {
        caps.push(format!("{} memory", format_bytes(memory)));
    }
    caps.join(" and ")
}

pub fn describe_queue(queue: &Queue) -> String {
    let mut parts = vec![match queue.parallel {
        Some(1) => "one job at a time".to_string(),
        Some(n) => format!("{n} jobs at a time"),
        None => "as many jobs at a time as the pool admits".to_string(),
    }];
    parts.push("in the order they came".to_string());
    let caps = describe_caps(&queue.settings);
    if !caps.is_empty() {
        parts.push(format!("holding at most {caps} together"));
    }
    if let Some(dir) = &queue.settings.dir {
        parts.push(format!("in {}", dir.display()));
    }
    if let Some(net) = &queue.settings.net {
        parts.push(net.describe());
    }
    if let Some(remote) = &queue.settings.on {
        parts.push(format!("on {}", remote.target));
    }
    if let Some(rate) = queue.settings.bandwidth {
        parts.push(format!(
            "{} together each way",
            crate::units::format_rate(rate)
        ));
    }
    if let Some(rate) = queue.settings.job_bandwidth {
        parts.push(format!("{} per job", crate::units::format_rate(rate)));
    }
    if let Some(monitor) = &queue.settings.monitor {
        parts.push(format!("on monitor {monitor}"));
    }
    if let Some(milli) = queue.settings.host_cores_milli {
        parts.push(format!(
            "holding this host's jobs to {} together while one of its jobs runs",
            format_cores(milli)
        ));
    }
    if queue.paused {
        parts.push("paused".to_string());
    }
    if queue.draining {
        parts.push("removed when its jobs end".to_string());
    }
    parts.join(", ")
}

fn write_bytes(pid: i32) -> u64 {
    fs::read_to_string(format!("/proc/{pid}/io"))
        .ok()
        .map(|t| crate::cgroup::keyed_value(&t.replace(':', ""), "write_bytes"))
        .unwrap_or(0)
}

fn shim_of(job: &Job) -> Option<(i32, u64)> {
    job.shim_pid.zip(job.shim_start_ticks)
}

fn signal_job(
    cgroup: Option<&Path>,
    supervisor: Option<&crate::process::Handle>,
    signal: i32,
) -> io::Result<usize> {
    let targets = match cgroup {
        Some(path) => Tree::processes(path)
            .into_iter()
            .filter_map(procs::stat_of)
            .collect(),
        None => {
            let Some(supervisor) = supervisor.filter(|handle| handle.alive().unwrap_or(false))
            else {
                return Err(io::Error::from_raw_os_error(libc::ESRCH));
            };
            let targets = procs::descendants(supervisor.pid, &procs::all());
            if !supervisor.alive().unwrap_or(false) {
                return Err(io::Error::from_raw_os_error(libc::ESRCH));
            }
            targets
        }
    };
    let mut delivered = 0;
    for stat in targets {
        let result =
            crate::process::Handle::open(stat.pid, Some(stat.start_ticks)).and_then(|handle| {
                if let Some(path) = cgroup
                    && !Tree::processes(path).contains(&stat.pid)
                {
                    return Err(io::Error::from_raw_os_error(libc::ESRCH));
                }
                handle.signal(signal)
            });
        match result {
            Ok(()) => delivered += 1,
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(delivered)
}

fn spawn_shim(
    store: &Store,
    id: u64,
    cgroup: Option<&Path>,
    shims: Option<PathBuf>,
) -> io::Result<(crate::process::Handle, crate::durability::launch::Gate)> {
    let mut command = Command::new(OWN_IMAGE);
    command
        .arg("shim")
        .arg("--state")
        .arg(&store.root)
        .arg("--id")
        .arg(id.to_string())
        .args(crate::service::access::supervisor_arguments());
    if let Some(path) = cgroup {
        command.arg("--cgroup").arg(path);
    }
    if let Some(path) = crate::operations::access::supervisor_argument(store, id)? {
        command.arg("--terminal-socket").arg(path);
    }
    let errors = fs::File::create(store.job_dir(id).join("shim.err"))?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(errors);
    let shims_procs = shims.and_then(|p| {
        std::ffi::CString::new(p.join("cgroup.procs").as_os_str().as_encoded_bytes()).ok()
    });
    unsafe {
        command.pre_exec(move || {
            libc::setsid();
            if let Some(file) = &shims_procs {
                let fd = libc::open(file.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
                if fd >= 0 {
                    libc::write(fd, b"0".as_ptr().cast(), 1);
                    libc::close(fd);
                }
            }
            Ok(())
        });
    }
    let (gate, held) = crate::durability::launch::gate(&mut command)?;
    let mut child = command.spawn()?;
    drop(held);
    let pid = child.id() as i32;
    let handle = match crate::process::Handle::open(pid, None) {
        Ok(handle) => handle,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    std::thread::spawn(move || child.wait());
    Ok((handle, gate))
}

fn handle(shared: &Arc<Shared>, stream: UnixStream) -> io::Result<()> {
    let admitted = crate::service::access::shared().admits(&stream);
    let peer_ok = admitted.is_some();
    if let Some(uid) = admitted {
        crate::service::access::enter(uid);
    }
    crate::durability::peer::enter(crate::durability::peer::of(&stream).filter(|_| peer_ok));
    let mut writer = stream.try_clone()?;
    let mut line = Vec::new();
    let mut reader = BufReader::new(stream);
    reader.read_until(b'\n', &mut line)?;
    if crate::clientif::greeted(&line) {
        return crate::clientif::serve(shared, &line, reader, writer, peer_ok);
    }
    let response = if !peer_ok {
        let message = crate::service::access::shared().refusal();
        crate::durability::audit::refused(crate::durability::peer::of(&writer), &message);
        Response::Error { message }
    } else {
        if let Ok(request) = crate::durability::negotiation::receive(&line)
            && crate::operations::output::wanted(&request)
        {
            let store = shared
                .daemon
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .store
                .clone();
            return crate::operations::output::serve(&store, request, writer);
        }
        crate::durability::exchange(&line, |request| respond(shared, request))
    };
    let mut bytes = crate::netsecret::rendered(&response)?;
    bytes.push(b'\n');
    let sent = writer.write_all(&bytes);
    drop(writer);
    crate::durability::audit::flush();
    crate::operations::journal::flush();
    sent
}

fn respond(shared: &Shared, request: Request) -> Response {
    let arrival = crate::pacing::TURNS.arrive();
    let mut daemon = shared.daemon.lock().unwrap_or_else(|p| p.into_inner());
    drop(arrival);
    let _ = daemon.checkpoint_ordering();
    let _ = daemon.reconcile_resource_updates(!matches!(
        request,
        Request::ResourceUpdateStatus { abandon: true, .. }
    ));
    if daemon.update_pending()
        && !matches!(
            request,
            Request::Status { .. }
                | Request::Wait { .. }
                | Request::Done { .. }
                | Request::Queue
                | Request::Host
                | Request::Ping
                | Request::Cancel { .. }
                | Request::CancelSelection { .. }
                | Request::Signal { .. }
                | Request::Freeze { .. }
                | Request::Attach { .. }
                | Request::Attempts { .. }
                | Request::ResourceUpdate { .. }
                | Request::ResourceUpdateStatus { .. }
                | Request::Explain { .. }
                | Request::Pressure { .. }
                | Request::PressureControl
        )
        && !crate::cli2::read_only(&request)
        && !crate::idset::unhindered(&request)
    {
        return Response::Error {
            message: crate::resource_update::message("resource update is pending"),
        };
    }
    if let Err(error) = daemon.reconcile_removal()
        && !matches!(
            request,
            Request::Status { .. }
                | Request::Wait { .. }
                | Request::Done { .. }
                | Request::Queue
                | Request::Host
                | Request::Ping
                | Request::Cancel { .. }
                | Request::CancelSelection { .. }
                | Request::Signal { .. }
                | Request::Freeze { .. }
                | Request::Explain { .. }
                | Request::Pressure { .. }
                | Request::PressureControl
        )
        && !crate::cli2::read_only(&request)
        && !crate::idset::unhindered(&request)
    {
        let message = format!(
            "{}: {error}",
            crate::removal::message("removal committed; cleanup is pending")
        );
        if let Request::Remove {
            expected: Some(expected),
            ..
        } = &request
            && let Ok(receipt) = crate::removal::pending_receipt(&daemon.store)
            && &receipt.selection == expected
        {
            return Response::RemovalInProgress { receipt, message };
        }
        return Response::Error { message };
    }
    match request {
        Request::Versioned { .. } => unreachable!(),
        Request::Extended {
            call: crate::cli2::Call::Set { call },
        } => idset_service::respond(shared, daemon, call),
        Request::Extended { call } => {
            let response = daemon.extended(call);
            shared.changed.notify_all();
            response
        }
        Request::PressureControl => Response::PressureControl {
            ledger: daemon.pressure_control.ledger.clone(),
            error: daemon.pressure_control.error.clone(),
            sampling_interval_ms: crate::pressure::control::SAMPLE_MS,
            notifications: daemon.pressure_control.monitors.report(),
        },
        Request::Pressure { id } => {
            let job = id.and_then(|id| daemon.load(id));
            if id.is_some() && job.is_none() {
                Response::Error {
                    message: crate::pressure::message("no such Job"),
                }
            } else {
                Response::Pressure {
                    snapshot: crate::pressure::snapshot(job.as_ref()),
                }
            }
        }
        Request::Explain { id } => match daemon.load(id) {
            Some(job) => Response::Explanation {
                explanation: daemon.explain(&job),
            },
            None => Response::Error {
                message: crate::admission::message("no such Job"),
            },
        },
        Request::Reprioritize {
            id,
            attempt,
            priority,
        } => {
            let response = match daemon.reprioritize(id, attempt, priority) {
                Ok(job) => Response::Submitted { job },
                Err(error) => Response::Error {
                    message: error.to_string(),
                },
            };
            shared.changed.notify_all();
            response
        }
        Request::ResourceUpdateStatus { operation, abandon } => {
            if abandon && let Some(mut next) = daemon.resource_updates.get(&operation).cloned() {
                if daemon.resource_worker.is_some() {
                    return Response::Error {
                        message: crate::resource_update::message("resource update is pending"),
                    };
                }
                if let Err(error) = crate::resource_update::abandon(&daemon.store, &mut next) {
                    return Response::Error {
                        message: error.to_string(),
                    };
                }
                daemon.resource_updates.insert(operation.clone(), next);
            }
            match daemon.resource_updates.get(&operation) {
                Some(operation) => Response::ResourceUpdate {
                    operation: Box::new(operation.clone()),
                },
                None => Response::Error {
                    message: crate::resource_update::message("no such resource update"),
                },
            }
        }
        Request::ResourceUpdate {
            target,
            patch,
            allow_oom,
            expected,
        } => {
            let result = (|| -> io::Result<Response> {
                if let Some(expected) = &expected
                    && let Some(op) = daemon.resource_updates.get(&expected.operation)
                {
                    if op.plan != **expected
                        || target != op.plan.target
                        || patch != op.plan.patch
                        || allow_oom != op.plan.allow_oom
                    {
                        return Err(io::Error::other(crate::resource_update::message(
                            "resource update target changed",
                        )));
                    }
                    return Ok(Response::ResourceUpdate {
                        operation: Box::new(op.clone()),
                    });
                }
                if daemon.update_pending() {
                    return Err(io::Error::other(crate::resource_update::message(
                        "resource update is pending",
                    )));
                }
                let token = match &expected {
                    Some(plan) => plan.operation.clone(),
                    None => crate::resource_update::token()?,
                };
                let plan = crate::resource_update::preview(
                    &daemon.store,
                    &daemon.objects,
                    daemon.tree.as_ref(),
                    target,
                    patch,
                    allow_oom,
                    token,
                )?;
                let Some(expected) = expected else {
                    return Ok(Response::ResourceUpdatePreview {
                        plan: Box::new(plan),
                    });
                };
                if plan != *expected {
                    return Err(io::Error::other(crate::resource_update::message(
                        "resource update target changed",
                    )));
                }
                let operation = crate::resource_update::Operation::new(plan);
                daemon
                    .resource_updates
                    .insert(operation.plan.operation.clone(), operation.clone());
                crate::resource_update::save(&daemon.store, &operation)?;
                let _ = daemon.reconcile_resource_updates(true);
                Ok(Response::ResourceUpdate {
                    operation: Box::new(operation),
                })
            })();
            shared.changed.notify_all();
            result.unwrap_or_else(|e| Response::Error {
                message: e.to_string(),
            })
        }
        Request::CancelSelection {
            target,
            recursive,
            expected,
        } => {
            let result = (|| -> io::Result<Response> {
                let Some(selection) = expected else {
                    let selection = crate::cancellation::select(
                        &daemon.store,
                        &daemon.objects,
                        &target,
                        recursive,
                    )?;
                    return Ok(Response::CancellationPreview {
                        schema_version: 1,
                        selection,
                    });
                };
                if !daemon.cancellations.contains_key(&selection.operation_id()) {
                    crate::cancellation::validate_selection(
                        &daemon.store,
                        &daemon.objects,
                        &target,
                        recursive,
                        &selection,
                    )?;
                }
                let operation = daemon.commit_cancellation(selection, now_ms())?;
                daemon.admit(now_ms());
                Ok(Response::Cancellation { operation })
            })();
            shared.changed.notify_all();
            result.unwrap_or_else(|error| Response::Error {
                message: error.to_string(),
            })
        }
        Request::Remove {
            target,
            recursive,
            allow_lost,
            expected,
        } => {
            let result = (|| -> io::Result<Response> {
                if let Some(expected) = &expected
                    && let Some(receipt) = crate::removal::receipts(&daemon.store)?
                        .into_iter()
                        .find(|receipt| &receipt.selection == expected)
                {
                    return Ok(Response::Removed { receipt });
                }
                let prepared = crate::removal::prepare(
                    &daemon.store,
                    &daemon.objects,
                    &target,
                    recursive,
                    allow_lost,
                )?;
                if let Some(expected) = expected {
                    if expected != prepared.preview.selection {
                        return Err(io::Error::other(crate::removal::message(
                            "removal selection changed",
                        )));
                    }
                    if prepared.preview.ready {
                        let receipt = crate::removal::commit(&daemon.store, prepared)?;
                        daemon.reload_removed_state()?;
                        return Ok(Response::Removed { receipt });
                    }
                }
                Ok(Response::RemovalPreview {
                    preview: prepared.preview,
                })
            })();
            shared.changed.notify_all();
            result.unwrap_or_else(|error| {
                if let Ok(receipt) = crate::removal::pending_receipt(&daemon.store) {
                    return Response::RemovalInProgress {
                        receipt,
                        message: format!(
                            "{}: {error}",
                            crate::removal::message("removal committed; cleanup is pending")
                        ),
                    };
                }
                Response::Error {
                    message: error.to_string(),
                }
            })
        }
        Request::Freeze {
            target,
            recursive,
            frozen,
            timeout_ms,
        } => {
            let ids = match target {
                crate::freezer::Target::Job { id } => vec![id],
                crate::freezer::Target::Object { kind, path } => {
                    if !recursive {
                        return Response::Error {
                            message: crate::freezer::message(
                                "collection suspension requires --recursive",
                            ),
                        };
                    }
                    let Ok(parent) = daemon.objects.resolve(&path) else {
                        return Response::Error {
                            message: crate::freezer::message("no such control target"),
                        };
                    };
                    if daemon.objects.nodes[&parent].kind != kind {
                        return Response::Error {
                            message: crate::freezer::message("no such control target"),
                        };
                    }
                    daemon
                        .jobs
                        .values()
                        .filter(|job| {
                            job.state.active()
                                && job
                                    .queue_id
                                    .is_some_and(|id| daemon.objects.within(id, parent))
                        })
                        .map(|job| job.id)
                        .collect()
                }
            };
            Response::Controlled {
                schema_version: 1,
                results: controlled(shared, daemon, ids, frozen, timeout_ms),
            }
        }
        Request::Retry {
            id,
            expected_attempt,
            held,
            env,
            queue,
            allow_lost,
        } => {
            let response = daemon.retry(id, expected_attempt, held, env, queue, allow_lost);
            shared.changed.notify_all();
            response
        }
        Request::Attempts { id } => match crate::attempts::list(&daemon.store, id) {
            Ok(attempts) => Response::Attempts {
                schema_version: 1,
                attempts,
            },
            Err(error) => Response::Error {
                message: error.to_string(),
            },
        },
        Request::Create {
            spec,
            env,
            idempotency_key,
        } => daemon.submit(*spec, env, true, idempotency_key),
        Request::Edit { id, spec, env } => {
            let held = daemon
                .jobs
                .get(&id)
                .is_some_and(|job| job.state == State::Held);
            match crate::durability::idempotency::edited(daemon.jobs.get(&id), &spec, held) {
                Ok(key) => daemon.put_job(*spec, env, Some(id), held, false, key),
                Err(message) => Response::Error { message },
            }
        }
        Request::Release { id } => match daemon.release_job(id) {
            Ok(job) => {
                daemon.admit_briefly(now_ms());
                shared.changed.notify_all();
                Response::Submitted {
                    job: daemon.load(id).unwrap_or(job),
                }
            }
            Err(message) => Response::Error { message },
        },
        Request::Signal { id, signal } => {
            if !(0..=libc::SIGRTMAX()).contains(&signal) {
                return Response::Error {
                    message: crate::process::message("invalid signal", &[]),
                };
            }
            match daemon.signal_processes(id, signal) {
                Ok(delivered) => Response::Signalled { delivered },
                Err(message) => Response::Error { message },
            }
        }
        Request::Config { reload } => {
            if reload {
                if !daemon.jobs.is_empty() {
                    return Response::Error {
                        message: crate::config::message(
                            "configuration reload requires draining active and waiting Jobs",
                            &[],
                        ),
                    };
                }
                match crate::config::Config::load() {
                    Ok((config, _))
                        if crate::service::restart_needed(&daemon.config, &config).is_some() =>
                    {
                        return Response::Error {
                            message: crate::service::restart_needed(&daemon.config, &config)
                                .unwrap_or_default(),
                        };
                    }
                    Ok((config, source)) if config.profile == daemon.config.profile => {
                        if let Err(message) = config.presets.validate_graph(&daemon.objects) {
                            return Response::Error { message };
                        }
                        if let Err(message) = config.network.validate_graph(&daemon.objects) {
                            return Response::Error { message };
                        }
                        if let Err(error) =
                            crate::presets::Registry::enroll(&daemon.store, &config.presets)
                        {
                            return Response::Error {
                                message: error.to_string(),
                            };
                        }
                        crate::durability::audit::configure(&config.audit);
                        crate::operations::journal::configure(&config.events);
                        daemon.config = config;
                        daemon.config_source = source;
                        daemon.trims.forget();
                        daemon.pressure_control.invalidate();
                    }
                    Ok(_) => {
                        return Response::Error {
                            message: crate::config::message(
                                "changing service profile requires a daemon restart after draining",
                                &[],
                            ),
                        };
                    }
                    Err(e) => {
                        return Response::Error {
                            message: e.to_string(),
                        };
                    }
                }
            }
            Response::Configuration {
                effective: daemon.config.effective(daemon.config_source.clone()),
            }
        }
        Request::Object { kind, operation } => {
            let response = daemon.object_operation(kind, operation);
            shared.changed.notify_all();
            response
        }
        Request::Attach { id } => match daemon.load(id) {
            Some(job) if job.spec.declared.terminal.is_none() => Response::Error {
                message: crate::terminal::message(
                    "job {id} has no terminal; start it with --pty",
                    &[("id", id.to_string())],
                ),
            },
            Some(job)
                if job.state.active()
                    && crate::operations::access::listening_terminal(&daemon.store, id)
                        .is_some() =>
            {
                Response::Attached {
                    path: crate::operations::access::listening_terminal(&daemon.store, id)
                        .unwrap_or_default(),
                    terminal_protocol: crate::terminal::PROTOCOL,
                }
            }
            Some(job) => finished_or_running(job),
            None => Response::Error {
                message: format!("there is no job {id}"),
            },
        },
        Request::Ping => Response::Pong {
            backend: daemon.backend,
            pool: daemon.pool,
        },
        Request::Submit {
            spec,
            env,
            idempotency_key,
        } => {
            let response = daemon.submit(*spec, env, false, idempotency_key);
            shared.changed.notify_all();
            response
        }
        Request::Status { id } => match daemon.load(id) {
            Some(job) => finished_or_running(job),
            None => Response::Error {
                message: format!("there is no job {id}"),
            },
        },
        Request::Done { id } => {
            if let Some(job) = daemon.jobs.get(&id).filter(|j| j.state.active())
                && let Some(result) = crate::durability::exit::result(&daemon.store, job)
            {
                daemon.finalize(id, result, now_ms());
                daemon.admit_later(now_ms());
            }
            shared.changed.notify_all();
            Response::Pong {
                backend: daemon.backend,
                pool: daemon.pool,
            }
        }
        Request::Queue => Response::Queue {
            view: daemon.queue_view(),
        },
        Request::QueueAdd { name, change } => {
            let response = daemon.queue_add(name, change);
            shared.changed.notify_all();
            response
        }
        Request::QueueSet { name, change } => {
            let response = daemon.queue_set(name, change);
            shared.changed.notify_all();
            response
        }
        Request::QueueRemove { name, drain } => daemon.queue_remove(name, drain),
        Request::Host => Response::Host {
            info: Box::new(daemon.host_info()),
        },
        Request::Output { .. } | Request::LogQuery { .. } => Response::Error {
            message: crate::streams::invalid().to_string(),
        },
        Request::QueueClear { name } => {
            let response = daemon.queue_clear(name);
            shared.changed.notify_all();
            response
        }
        Request::Cancel { id, session } => {
            let response = daemon.cancel(id, &session, now_ms());
            shared.changed.notify_all();
            response
        }
        Request::Wait { id, timeout_ms } => {
            let deadline = now_ms() + timeout_ms;
            loop {
                if let Some(job) = daemon.jobs.get(&id).filter(|j| j.state.active())
                    && let Some(result) = crate::durability::exit::result(&daemon.store, job)
                {
                    daemon.finalize(id, result, now_ms());
                    daemon.admit_later(now_ms());
                    shared.changed.notify_all();
                }
                let Some(job) = daemon.load(id) else {
                    return Response::Error {
                        message: format!("there is no job {id}"),
                    };
                };
                let now = now_ms();
                if job.state.terminal() || now >= deadline {
                    return finished_or_running(job);
                }
                let wait = Duration::from_millis((deadline - now).min(RESULT_POLL_MS));
                daemon = shared
                    .changed
                    .wait_timeout(daemon, wait)
                    .map(|(g, _)| g)
                    .unwrap_or_else(|p| p.into_inner().0);
            }
        }
    }
}

fn controlled(
    shared: &Shared,
    mut daemon: std::sync::MutexGuard<'_, Daemon>,
    ids: Vec<u64>,
    frozen: bool,
    timeout_ms: u64,
) -> Vec<crate::freezer::Outcome> {
    let mut results = Vec::new();
    for id in ids {
        let error = daemon.request_freezer(id, frozen).err();
        let job = daemon.load(id);
        results.push(crate::freezer::Outcome {
            id,
            attempt: job.as_ref().map_or(0, |j| j.attempt),
            generation: job.as_ref().map_or(0, |j| j.suspension.generation),
            state: job.map_or(State::Lost, |j| j.state),
            accepted: error.is_none(),
            confirmed: false,
            error,
        });
    }
    shared.changed.notify_all();
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms.min(60_000));
    loop {
        for result in results
            .iter_mut()
            .filter(|result| result.accepted && !result.confirmed && result.error.is_none())
        {
            daemon.reconcile_freezer(result.id, now_ms());
            match daemon.load(result.id) {
                Some(job)
                    if job.attempt == result.attempt
                        && job.suspension.generation == result.generation
                        && job.suspension.requested == frozen =>
                {
                    result.error = job.suspension.error.clone();
                    result.confirmed = !job.suspension.pending
                        && matches!(job.state, State::Running | State::Suspended)
                        && (job.state == State::Suspended) == frozen;
                    if !matches!(job.state, State::Running | State::Suspended) {
                        result.error = Some(crate::freezer::message(
                            "execution ended before confirmation",
                        ));
                    }
                    result.state = job.state;
                }
                _ => {
                    result.error = Some(crate::freezer::message(
                        "previous control request was superseded",
                    ))
                }
            }
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero()
            || results
                .iter()
                .all(|result| result.confirmed || result.error.is_some())
        {
            return results;
        }
        daemon = shared
            .changed
            .wait_timeout(daemon, remaining.min(TICK))
            .map(|(g, _)| g)
            .unwrap_or_else(|p| p.into_inner().0);
    }
}

fn finished_or_running(job: Job) -> Response {
    if job.state.terminal() {
        Response::Finished { job }
    } else {
        Response::StillRunning { job }
    }
}

pub(crate) fn lock_exclusively(path: &Path) -> io::Result<fs::File> {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::other(format!(
            "another daemon holds {}",
            path.display()
        )));
    }
    Ok(file)
}

pub fn serve(store: Store) -> io::Result<()> {
    let _lock = lock_exclusively(&store.lock())?;
    crate::durability::failpoint::check().map_err(io::Error::other)?;
    crate::durability::sweep_transactions(&store)?;
    crate::pacing::sweep_staged(&store);
    crate::durability::swept(crate::durability::idempotency::sweep(&store));
    let found = crate::service::find_cgroup()?;
    let tree = found.tree.clone();
    if let Some(tree) = &tree {
        tree.leave_root()?;
        tree.prepare()?;
    }
    let endpoint = crate::service::Endpoint::plan(&store)?;
    let mut daemon = Daemon::new(store, tree)?;
    crate::durability::audit::open(&daemon.store, &daemon.config.audit)?;
    crate::operations::opened(
        &daemon.store,
        &daemon.config,
        &daemon.objects,
        &daemon.pressure_control.ledger,
    )?;
    daemon.recover()?;
    daemon.reconcile_cancellations(now_ms(), None);
    crate::operations::health::recovered(
        daemon.supervisors.len(),
        crate::operations::journal::transitions(),
    );
    daemon.reestimate();
    daemon.admit_within(now_ms(), 0, false);
    let listener = endpoint.listen(&daemon.config.socket)?;
    eprintln!(
        "job daemon: {}",
        crate::service::announce(&endpoint, &found, &daemon.config.socket)
    );
    eprintln!(
        "job daemon: {}",
        crate::operations::mode::startup(daemon.backend, daemon.config.cgroup.required)
    );
    eprintln!(
        "job daemon: {}, pool {} cores, {} memory, {} processes",
        daemon.backend.describe(),
        daemon.pool.cores_milli / MILLI,
        format_bytes(daemon.pool.memory),
        daemon.pool.pids
    );
    let shared = Arc::new(Shared {
        daemon: Mutex::new(daemon),
        changed: Condvar::new(),
    });
    let ticker = Arc::clone(&shared);
    std::thread::spawn(move || {
        let mut next_tick = std::time::Instant::now() + TICK;
        loop {
            let sources = ticker
                .daemon
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .pressure_control
                .monitors
                .sources();
            let notices = crate::pressure::notify::wait(
                sources,
                next_tick.saturating_duration_since(std::time::Instant::now()),
            );
            let mut daemon = ticker.daemon.lock().unwrap_or_else(|p| p.into_inner());
            let early_sample = daemon.pressure_control.notifications(notices);
            let tick = early_sample || std::time::Instant::now() >= next_tick;
            if tick {
                daemon.tick();
                next_tick = std::time::Instant::now() + TICK;
            }
            drop(daemon);
            if tick {
                ticker.changed.notify_all();
                crate::operations::journal::flush();
            }
        }
    });
    let starter = Arc::clone(&shared);
    std::thread::spawn(move || {
        loop {
            let pass = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if !crate::pacing::TURNS.pass_requested(TICK) {
                    return;
                }
                loop {
                    crate::pacing::TURNS.give_way();
                    let mut daemon = starter.daemon.lock().unwrap_or_else(|p| p.into_inner());
                    if daemon.reconcile_removal().is_err() {
                        drop(daemon);
                        std::thread::sleep(TICK);
                        break;
                    }
                    crate::durability::failpoint::panics("starter-panic");
                    daemon.admit_within(now_ms(), crate::pacing::STARTS_PER_PASS, true);
                    drop(daemon);
                    starter.changed.notify_all();
                    if !crate::pacing::TURNS.pass_requested(Duration::ZERO) {
                        break;
                    }
                }
            }));
            if let Err(panic) = pass {
                let what = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
                    .unwrap_or_default();
                crate::pacing::starter_failed(&what);
                std::thread::sleep(TICK);
                crate::pacing::TURNS.request_pass();
            }
        }
    });
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            let _ = handle(&shared, stream);
        });
    }
    Ok(())
}

impl Daemon {
    fn object_operation(
        &mut self,
        kind: crate::objects::Kind,
        mut operation: crate::objects::Operation,
    ) -> Response {
        use crate::objects::Operation;
        if let Operation::Create { config, .. } | Operation::Set { config, .. } = &mut operation
            && let Err(message) = crate::netsecret::settle_config(config, &self.store.root)
        {
            return Response::Error { message };
        }
        let mut graph = self.objects.clone();
        let subject = match &operation {
            Operation::Rename { path, .. }
            | Operation::Move { path, .. }
            | Operation::Remove { path } => graph.resolve(path).ok(),
            _ => None,
        };
        if let Some(id) = subject {
            let running = self
                .jobs
                .values()
                .any(|j| j.state.active() && j.queue_id.is_some_and(|q| graph.within(q, id)));
            if running {
                return Response::Error {
                    message: "cannot move, rename or remove a container with running Jobs"
                        .to_owned(),
                };
            }
            if matches!(&operation, Operation::Remove { .. })
                && self
                    .store
                    .job_ids()
                    .iter()
                    .filter_map(|&id| self.store.load_job(id))
                    .any(|j| j.queue_id.is_some_and(|q| graph.within(q, id)))
            {
                return Response::Error {
                    message: "object contains Job records; remove those records explicitly first"
                        .to_owned(),
                };
            }
        }
        let selected = match graph.change(kind, &operation) {
            Ok(id) => id,
            Err(message) => return Response::Error { message },
        };
        if let Err(message) = self.config.presets.validate_graph(&graph) {
            return Response::Error { message };
        }
        if let Err(message) = self.config.network.validate_graph(&graph) {
            return Response::Error { message };
        }
        if !matches!(&operation, Operation::List | Operation::Show { .. }) {
            let changes = crate::admission::fair::scope_changes(&self.objects, &graph);
            if self.jobs.values().any(|job| {
                job.state.active()
                    && job.queue_id.is_some_and(|queue| {
                        changes
                            .iter()
                            .any(|scope| self.objects.within(queue, *scope))
                    })
            }) {
                return Response::Error {
                    message: crate::admission::message(
                        "fair-share basis changes require an inactive subtree",
                    ),
                };
            }
            for node in graph.nodes.values() {
                let domains = crate::aggregate::chain(&graph, node.id);
                if let Err(message) =
                    crate::aggregate::available(self.tree.as_ref(), &domains, false)
                {
                    return Response::Error { message };
                }
            }
            for node in self.objects.nodes.values() {
                let before = crate::aggregate::chain(&self.objects, node.id);
                let after = if graph.nodes.contains_key(&node.id) {
                    crate::aggregate::chain(&graph, node.id)
                } else {
                    Vec::new()
                };
                if before == after {
                    continue;
                }
                let running = self
                    .jobs
                    .values()
                    .any(|job| job.state.active() && job.queue_id == Some(node.id));
                let populated = before
                    .last()
                    .is_some_and(|domain| domain.object_id == node.id)
                    && self.tree.as_ref().is_some_and(|tree| {
                        crate::aggregate::paths(tree, &before)
                            .last()
                            .is_some_and(|path| Tree::is_populated(path))
                    });
                let retained = node.kind == crate::objects::Kind::Queue
                    && self
                        .store
                        .job_ids()
                        .into_iter()
                        .filter_map(|id| self.store.load_job(id))
                        .any(|job| {
                            job.queue_id == Some(node.id)
                                && job
                                    .workload_cgroup
                                    .as_ref()
                                    .is_some_and(|path| Tree::is_populated(path))
                        });
                if running || populated || retained {
                    return Response::Error {
                        message: crate::resource_policy::message(
                            "aggregate policy changes require an inactive subtree",
                        ),
                    };
                }
            }
            if let Err(e) = graph.save(&self.store) {
                return Response::Error {
                    message: format!("cannot record object change: {e}"),
                };
            }
            self.objects = graph;
            self.queues = self.objects.queues();
            self.sweep_net_secrets();
            for job in self.jobs.values_mut() {
                if let Some(queue_id) = job.queue_id {
                    job.spec.queue = Some(self.objects.path(queue_id));
                }
            }
            self.admit(now_ms());
        }
        let objects = if matches!(&operation, Operation::List) {
            self.objects
                .nodes
                .values()
                .filter(|n| n.kind == kind)
                .map(|n| self.objects.view(n.id))
                .collect()
        } else {
            selected
                .map(|id| {
                    let mut view = self.objects.view(id);
                    if matches!(&operation, Operation::Show { .. }) {
                        view.depth = Some(crate::operations::depth::measure(
                            &self.objects,
                            &self.jobs,
                            id,
                            now_ms(),
                        ));
                    }
                    view
                })
                .into_iter()
                .collect()
        };
        Response::Objects {
            schema_version: 1,
            objects,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queue_name_is_lowercase_short_and_not_a_word_of_the_command() {
        assert!(check_queue_name("gpu-left_2").is_ok());
        assert!(check_queue_name(&"a".repeat(QUEUE_NAME_LIMIT)).is_ok());
        assert!(check_queue_name(&"a".repeat(QUEUE_NAME_LIMIT + 1)).is_err());
        assert!(check_queue_name("Downloads").is_err());
        assert!(check_queue_name("2fast").is_err());
        assert!(check_queue_name("rm").is_err());
    }

    #[test]
    fn a_restart_never_raises_what_a_running_job_reserves() {
        let root = std::env::temp_dir().join(format!("job-reestimate-{}", std::process::id()));
        let store = Store::open(root.clone()).unwrap();
        let mut daemon = Daemon::new(store, None).unwrap();
        let spec = Spec {
            argv: vec!["cargo".to_string(), "test".to_string()],
            cwd: root.clone(),
            session: "test".to_string(),
            declared: crate::model::Declared::default(),
            queue: None,
        };
        let mut reservation = daemon.reserve(&spec, &Env { vars: Vec::new() });
        reservation.vector.cores_milli = 4 * MILLI;
        reservation.vector.memory = daemon.pool.memory / 8;
        let job = Job {
            durability: Default::default(),
            preset_snapshot: None,
            output_mode: None,
            workload_cgroup: None,
            admission_snapshot: None,
            priority_source: None,
            priority_changes: Vec::new(),
            aggregate_domains: Vec::new(),
            applied_resources: Default::default(),
            resource_sources: Default::default(),
            suspension: Default::default(),
            timing: Default::default(),
            attempt: 1,
            attempt_submitted_ms: None,
            submitted_spec: None,
            requested_spec: None,
            effective_spec: None,
            released_ms: None,
            policy: crate::config::Profile::Ordinary,
            queue_id: None,
            id: 1,
            log: root.join("log"),
            key: estimate::history_key(&spec),
            spec,
            reservation: reservation.clone(),
            backend: Backend::Watch,
            state: State::Running,
            submitted_ms: 0,
            started_ms: Some(0),
            finished_ms: None,
            shim_pid: None,
            shim_start_ticks: None,
            supervisor_boot_id: None,
            termination_deadline_ms: None,
            waited_for: None,
            stop: None,
            result: None,
            usage: Default::default(),
            link: None,
            network: None,
        };
        daemon.jobs.insert(1, job);
        daemon.reestimate();
        let kept = daemon.jobs[&1].reservation.vector.memory;
        let _ = fs::remove_dir_all(&root);
        assert!(kept <= reservation.vector.memory, "{kept}");
    }

    #[test]
    fn a_held_host_writes_its_cores_as_a_quota_and_a_free_host_writes_max() {
        assert_eq!(cpu_max(Some(4 * MILLI)), "400000 100000");
        assert_eq!(cpu_max(Some(MILLI / 2)), "50000 100000");
        assert_eq!(cpu_max(Some(0)), "1000 100000");
        assert_eq!(cpu_max(None), "max 100000");
    }

    #[test]
    fn the_host_is_held_only_while_a_holding_queue_runs_a_job_and_the_smallest_hold_wins() {
        let root = std::env::temp_dir().join(format!("job-host-cores-{}", std::process::id()));
        let store = Store::open(root.clone()).unwrap();
        let mut daemon = Daemon::new(store, None).unwrap();
        for (name, milli) in [
            ("gpu", Some(4 * MILLI)),
            ("wide", Some(8 * MILLI)),
            ("free", None),
        ] {
            let settings = Settings {
                host_cores_milli: milli,
                ..Settings::default()
            };
            daemon.queues.insert(
                name.to_string(),
                Queue {
                    name: name.to_string(),
                    parallel: Some(1),
                    paused: false,
                    draining: false,
                    created_ms: 0,
                    settings,
                },
            );
        }
        let running_in = |daemon: &mut Daemon, id: u64, queue: &str| {
            let spec = Spec {
                argv: vec!["true".to_string()],
                cwd: root.clone(),
                session: "test".to_string(),
                declared: crate::model::Declared::default(),
                queue: Some(queue.to_string()),
            };
            let reservation = daemon.reserve(&spec, &Env { vars: Vec::new() });
            daemon.jobs.insert(
                id,
                Job {
                    durability: Default::default(),
                    preset_snapshot: None,
                    output_mode: None,
                    workload_cgroup: None,
                    admission_snapshot: None,
                    priority_source: None,
                    priority_changes: Vec::new(),
                    aggregate_domains: Vec::new(),
                    applied_resources: Default::default(),
                    resource_sources: Default::default(),
                    suspension: Default::default(),
                    timing: Default::default(),
                    attempt: 1,
                    attempt_submitted_ms: None,
                    submitted_spec: None,
                    requested_spec: None,
                    effective_spec: None,
                    released_ms: None,
                    policy: crate::config::Profile::Ordinary,
                    queue_id: None,
                    id,
                    log: root.join("log"),
                    key: estimate::history_key(&spec),
                    spec,
                    reservation,
                    backend: Backend::Watch,
                    state: State::Running,
                    submitted_ms: 0,
                    started_ms: Some(0),
                    finished_ms: None,
                    shim_pid: None,
                    shim_start_ticks: None,
                    supervisor_boot_id: None,
                    termination_deadline_ms: None,
                    waited_for: None,
                    stop: None,
                    result: None,
                    usage: Default::default(),
                    link: None,
                    network: None,
                },
            );
        };
        assert_eq!(daemon.host_cores_held(), None);
        running_in(&mut daemon, 1, "free");
        assert_eq!(daemon.host_cores_held(), None);
        running_in(&mut daemon, 2, "wide");
        assert_eq!(
            daemon.host_cores_held(),
            Some((8 * MILLI, "wide".to_string()))
        );
        running_in(&mut daemon, 3, "gpu");
        assert_eq!(
            daemon.host_cores_held(),
            Some((4 * MILLI, "gpu".to_string()))
        );
        daemon.jobs.get_mut(&3).unwrap().state = State::Finished;
        let _ = fs::remove_dir_all(&root);
        assert_eq!(
            daemon.host_cores_held(),
            Some((8 * MILLI, "wide".to_string()))
        );
    }
}
