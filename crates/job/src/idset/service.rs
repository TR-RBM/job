use std::collections::{BTreeMap, BTreeSet};
use std::sync::MutexGuard;
use std::time::Duration;

use super::{Daemon, RESULT_POLL_MS, Shared, signal_job};
use crate::cli2::Answer;
use crate::cli2::listing::state_name;
use crate::idset::{Action, Call, Entry, KIND, Outcome, Report, Set};
use crate::model::{Job, Response, State};
use crate::shim::now_ms;

fn entry(id: u64, outcome: Outcome, result: &str) -> Entry {
    Entry {
        id,
        outcome,
        result: result.to_owned(),
        state: None,
        attempt: None,
        exit_status: None,
        reason: None,
    }
}

fn about(job: &Job, outcome: Outcome, result: &str) -> Entry {
    Entry {
        state: Some(state_name(job).to_owned()),
        attempt: Some(job.attempt),
        ..entry(job.id, outcome, result)
    }
}

fn refused(job: &Job, reason: String) -> Entry {
    Entry {
        reason: Some(reason),
        ..about(job, Outcome::Refused, "refused")
    }
}

fn failed(id: u64, reason: String) -> Entry {
    Entry {
        reason: Some(reason),
        ..entry(id, Outcome::Failed, "failed")
    }
}

impl Daemon {
    pub(super) fn release_job(&mut self, id: u64) -> Result<Job, String> {
        let Some(mut job) = self
            .jobs
            .get(&id)
            .filter(|job| job.state == State::Held)
            .cloned()
        else {
            return Err(crate::lifecycle::message("only held Jobs can be released"));
        };
        if job
            .queue_id
            .is_some_and(|id| !self.objects.view(id).closed_by.is_empty())
        {
            return Err(crate::lifecycle::message("queue is closed to release"));
        }
        crate::operations::mode::refuse(
            self.backend,
            self.config.profile,
            &job.spec.declared,
            &job.resource_sources,
        )?;
        job.state = State::Queued;
        job.released_ms = Some(now_ms());
        self.store
            .save_job(&job)
            .map_err(|error| error.to_string())?;
        self.jobs.insert(id, job.clone());
        Ok(job)
    }

    pub(super) fn signal_processes(&mut self, id: u64, signal: i32) -> Result<usize, String> {
        if !self.jobs.get(&id).is_some_and(|job| job.state.active()) {
            return Err(crate::process::message(
                "job {id} is not running",
                &[("id", id.to_string())],
            ));
        }
        signal_job(
            self.cgroup(id).as_deref(),
            self.supervisors.get(&id),
            signal,
        )
        .map_err(|error| {
            crate::process::message(
                "signal delivery failed: {error}; some processes may already have received it",
                &[("error", error.to_string())],
            )
        })
    }

    fn selected(&self, set: &Set) -> (Vec<Job>, Vec<Entry>) {
        let stored: BTreeSet<u64> = self.store.job_ids().into_iter().collect();
        let mut jobs = Vec::new();
        let mut entries = Vec::new();
        for id in &set.ids {
            if self.jobs.contains_key(id) || stored.contains(id) {
                match self.load(*id) {
                    Some(job) => jobs.push(job),
                    None => entries.push(failed(
                        *id,
                        crate::cancellation::message("invalid Job record"),
                    )),
                }
            } else if set.explicit.binary_search(id).is_ok() {
                entries.push(entry(*id, Outcome::Missing, "no_such_job"));
            }
        }
        (jobs, entries)
    }

    fn cancel_set(
        &mut self,
        set: &Set,
        jobs: Vec<Job>,
        dry_run: bool,
        operation: &mut Option<String>,
    ) -> Vec<Entry> {
        let mut entries = Vec::new();
        let mut members = Vec::new();
        for job in &jobs {
            if job.state.terminal() {
                entries.push(about(job, Outcome::Refused, "already_ended"));
            } else if dry_run {
                entries.push(about(
                    job,
                    Outcome::Done,
                    if job.state.editable() {
                        "cancelled"
                    } else {
                        "stopping"
                    },
                ));
            } else {
                members.push(crate::cancellation::Member::from(job));
            }
        }
        if members.is_empty() {
            return entries;
        }
        let now = now_ms();
        let selection = crate::cancellation::Selection {
            scope_id: None,
            members,
            set: Some(set.written.clone()),
        };
        match self.commit_cancellation(selection.clone(), now) {
            Ok(committed) => {
                *operation = Some(committed.operation.clone());
                let reported: BTreeMap<u64, &crate::cancellation::Outcome> = committed
                    .results
                    .iter()
                    .map(|result| (result.id, result))
                    .collect();
                for member in &selection.members {
                    let state = |state: &State| {
                        jobs.iter().find(|job| job.id == member.id).map(|job| {
                            state_name(&Job {
                                state: state.clone(),
                                ..job.clone()
                            })
                            .to_owned()
                        })
                    };
                    let mut answer = Entry {
                        attempt: Some(member.attempt),
                        ..entry(member.id, Outcome::Pending, "pending")
                    };
                    match reported.get(&member.id) {
                        Some(result) if result.pending => {
                            answer.reason = result
                                .error
                                .clone()
                                .or_else(|| committed.persistence_error.clone());
                        }
                        Some(result) if result.error.is_some() => {
                            answer.outcome = Outcome::Failed;
                            answer.result = "failed".to_owned();
                            answer.reason = result.error.clone();
                        }
                        Some(result) => match &result.state {
                            Some(State::Cancelled) => {
                                answer.result = "cancelled".to_owned();
                                answer.state = state(&State::Cancelled);
                            }
                            Some(State::Stopping) => {
                                answer.result = "stopping".to_owned();
                                answer.state = state(&State::Stopping);
                            }
                            Some(other) => {
                                answer.result = "already_ended".to_owned();
                                answer.state = state(other);
                            }
                            None => {}
                        },
                        None => answer.reason = committed.persistence_error.clone(),
                    }
                    if answer.result == "already_ended" {
                        answer.outcome = Outcome::Refused;
                    } else if answer.result != "pending" && answer.result != "failed" {
                        answer.outcome = if committed.complete {
                            Outcome::Done
                        } else {
                            Outcome::Pending
                        };
                    }
                    if answer.outcome == Outcome::Pending && answer.reason.is_none() {
                        answer.reason = committed.persistence_error.clone().or_else(|| {
                            Some(crate::cancellation::message("cancellation is pending"))
                        });
                    }
                    entries.push(answer);
                }
            }
            Err(error) => {
                entries.extend(
                    selection
                        .members
                        .iter()
                        .map(|member| failed(member.id, error.to_string())),
                );
            }
        }
        self.admit(now);
        entries
    }

    fn remove_set(
        &mut self,
        jobs: Vec<Job>,
        allow_lost: bool,
        dry_run: bool,
        operation: &mut Option<String>,
    ) -> Vec<Entry> {
        let known: BTreeMap<u64, Job> = jobs.iter().map(|job| (job.id, job.clone())).collect();
        let (prepared, issues) = match crate::removal::prepare_set(&self.store, jobs, allow_lost) {
            Ok(found) => found,
            Err(error) => {
                return known
                    .keys()
                    .map(|id| failed(*id, error.to_string()))
                    .collect();
            }
        };
        let mut entries = Vec::new();
        let mut blocked: BTreeMap<u64, (bool, Vec<String>)> = BTreeMap::new();
        for issue in issues {
            let Some(id) = issue.job_id else { continue };
            let slot = blocked.entry(id).or_insert((true, Vec::new()));
            slot.0 &= issue.pending;
            if !slot.1.contains(&issue.message) {
                slot.1.push(issue.message);
            }
        }
        for (id, (pending, reasons)) in blocked {
            let job = &known[&id];
            entries.push(if pending {
                Entry {
                    reason: Some(reasons.join("; ")),
                    ..about(job, Outcome::Pending, "pending")
                }
            } else {
                refused(job, reasons.join("; "))
            });
        }
        let Some(prepared) = prepared else {
            return entries;
        };
        let ready: Vec<u64> = prepared
            .preview
            .selection
            .jobs
            .iter()
            .map(|job| job.id)
            .collect();
        if dry_run {
            entries.extend(
                ready
                    .iter()
                    .map(|id| about(&known[id], Outcome::Done, "removed")),
            );
            return entries;
        }
        match crate::removal::commit(&self.store, prepared)
            .and_then(|receipt| self.reload_removed_state().map(|()| receipt))
        {
            Ok(receipt) => {
                *operation = Some(receipt.operation);
                entries.extend(ready.iter().map(|id| Entry {
                    attempt: Some(known[id].attempt),
                    ..entry(*id, Outcome::Done, "removed")
                }));
            }
            Err(error) if crate::removal::pending(&self.store) => {
                let reason = format!(
                    "{}: {error}",
                    crate::removal::message("removal committed; cleanup is pending")
                );
                entries.extend(ready.iter().map(|id| Entry {
                    reason: Some(reason.clone()),
                    ..about(&known[id], Outcome::Pending, "pending")
                }));
            }
            Err(error) => {
                entries.extend(ready.iter().map(|id| failed(*id, error.to_string())));
            }
        }
        entries
    }

    fn predicted(&self, action: &Action, job: &Job) -> Entry {
        let (possible, word, reason) = match action {
            Action::Release => (
                job.state == State::Held,
                "released",
                crate::lifecycle::message("only held Jobs can be released"),
            ),
            Action::Retry { allow_lost, .. } if job.state == State::Lost && !allow_lost => (
                false,
                "retried",
                crate::lifecycle::message("lost work may still exist; retry requires --allow-lost"),
            ),
            Action::Retry { .. } => (
                job.state.terminal(),
                "retried",
                crate::lifecycle::message("retry requires the expected completed attempt"),
            ),
            Action::Freeze { frozen, .. } => (
                matches!(job.state, State::Running | State::Suspended),
                if *frozen { "suspended" } else { "continued" },
                crate::freezer::message("only running or suspended Jobs can be controlled"),
            ),
            Action::Signal { .. } => (
                job.state.active(),
                "signalled",
                crate::process::message("job {id} is not running", &[("id", job.id.to_string())]),
            ),
            Action::Reprioritize { .. } => (
                job.state.editable(),
                "reprioritized",
                crate::admission::message("only held or queued Jobs can be reprioritized"),
            ),
            Action::Move { .. } => (
                job.state.editable(),
                "moved",
                crate::cli2::message("Job {id} is {state}; only held and queued Jobs can be moved")
                    .replace("{id}", &job.id.to_string())
                    .replace("{state}", state_name(job)),
            ),
            Action::Cancel { .. } | Action::Remove { .. } | Action::Wait { .. } => {
                (false, "refused", String::new())
            }
        };
        if possible {
            about(job, Outcome::Done, word)
        } else {
            refused(job, reason)
        }
    }

    fn applied_each(&mut self, action: &Action, jobs: Vec<Job>) -> Vec<Entry> {
        let mut entries = Vec::new();
        let mut released = Vec::new();
        for job in &jobs {
            let answer = match action {
                Action::Release => match self.release_job(job.id) {
                    Ok(_) => {
                        released.push(job.id);
                        continue;
                    }
                    Err(reason) => refused(job, reason),
                },
                Action::Retry {
                    held,
                    env,
                    queue,
                    allow_lost,
                } => match self.retry(
                    job.id,
                    job.attempt,
                    *held,
                    env.clone(),
                    queue.clone(),
                    *allow_lost,
                ) {
                    Response::Submitted { job } => about(&job, Outcome::Done, "retried"),
                    Response::RetryPending => Entry {
                        reason: Some(crate::lifecycle::message(
                            "previous supervisor has not exited; retry was not created",
                        )),
                        ..about(job, Outcome::Pending, "pending")
                    },
                    Response::Error { message } => refused(job, message),
                    other => failed(job.id, format!("{other:?}")),
                },
                Action::Signal { signal } => match self.signal_processes(job.id, *signal) {
                    Ok(_) => about(job, Outcome::Done, "signalled"),
                    Err(reason) => refused(job, reason),
                },
                Action::Reprioritize { priority } => {
                    match self.reprioritize(job.id, job.attempt, *priority) {
                        Ok(job) => about(&job, Outcome::Done, "reprioritized"),
                        Err(error) => refused(job, error.to_string()),
                    }
                }
                Action::Move { queue } => match self.moved(job.id, queue) {
                    Response::Submitted { job } => about(&job, Outcome::Done, "moved"),
                    Response::Error { message } => refused(job, message),
                    other => failed(job.id, format!("{other:?}")),
                },
                Action::Cancel { .. }
                | Action::Remove { .. }
                | Action::Freeze { .. }
                | Action::Wait { .. } => self.predicted(action, job),
            };
            entries.push(answer);
        }
        if !released.is_empty() {
            self.admit_briefly(now_ms());
            for id in released {
                match self.load(id) {
                    Some(job) => entries.push(about(&job, Outcome::Done, "released")),
                    None => entries.push(entry(id, Outcome::Done, "released")),
                }
            }
        }
        entries
    }
}

fn waited(
    shared: &Shared,
    mut daemon: MutexGuard<'_, Daemon>,
    ids: Vec<u64>,
    timeout_ms: u64,
) -> Vec<Entry> {
    let deadline = now_ms().saturating_add(timeout_ms);
    let mut ended: BTreeMap<u64, Entry> = BTreeMap::new();
    let mut open = ids;
    loop {
        let mut still = Vec::new();
        for id in open {
            if let Some(job) = daemon.jobs.get(&id).filter(|job| job.state.active())
                && let Some(result) = crate::durability::exit::result(&daemon.store, job)
            {
                daemon.finalize(id, result, now_ms());
                daemon.admit_later(now_ms());
                shared.changed.notify_all();
            }
            match daemon.load(id) {
                Some(job) if job.state.terminal() => {
                    ended.insert(
                        id,
                        Entry {
                            exit_status: Some(crate::cli_contract::exit_status(&job)),
                            ..about(&job, Outcome::Done, state_name(&job))
                        },
                    );
                }
                Some(_) => still.push(id),
                None => {
                    ended.insert(id, entry(id, Outcome::Missing, "no_such_job"));
                }
            }
        }
        let now = now_ms();
        if still.is_empty() || now >= deadline {
            for id in still {
                if let Some(job) = daemon.load(id) {
                    ended.insert(id, about(&job, Outcome::Pending, "still_running"));
                }
            }
            return ended.into_values().collect();
        }
        open = still;
        daemon = shared
            .changed
            .wait_timeout(
                daemon,
                Duration::from_millis((deadline - now).min(RESULT_POLL_MS)),
            )
            .map(|(guard, _)| guard)
            .unwrap_or_else(|poisoned| poisoned.into_inner().0);
    }
}

pub(super) fn respond(shared: &Shared, mut daemon: MutexGuard<'_, Daemon>, call: Call) -> Response {
    let operands: Vec<&str> = call.operands.iter().map(String::as_str).collect();
    let set = match crate::idset::parse(&operands) {
        Ok(set) => set,
        Err(message) => return Response::Error { message },
    };
    if let Action::Signal { signal } = &call.action
        && !(0..=libc::SIGRTMAX()).contains(signal)
    {
        return Response::Error {
            message: crate::process::message("invalid signal", &[]),
        };
    }
    let (jobs, mut results) = daemon.selected(&set);
    let selected = jobs.len();
    let mut operation = None;
    let action = call.action.name().to_owned();
    match call.action {
        Action::Wait { timeout_ms } => {
            let ids = jobs.iter().map(|job| job.id).collect();
            results.extend(waited(shared, daemon, ids, timeout_ms));
        }
        Action::Freeze { frozen, timeout_ms } if !call.dry_run => {
            let known: BTreeMap<u64, Job> = jobs.into_iter().map(|job| (job.id, job)).collect();
            let word = if frozen { "suspended" } else { "continued" };
            let controlled = super::controlled(
                shared,
                daemon,
                known.keys().copied().collect(),
                frozen,
                timeout_ms,
            );
            results.extend(controlled.into_iter().map(|result| {
                let (outcome, word) = match (&result.error, result.accepted, result.confirmed) {
                    (Some(_), false, _) => (Outcome::Refused, "refused"),
                    (Some(_), true, _) => (Outcome::Failed, "failed"),
                    (None, _, true) => (Outcome::Done, word),
                    (None, _, false) => (Outcome::Pending, "pending"),
                };
                let mut job = known[&result.id].clone();
                job.state = result.state.clone();
                Entry {
                    reason: result.error.clone().or_else(|| {
                        (outcome == Outcome::Pending)
                            .then(|| crate::freezer::message("pending kernel confirmation"))
                    }),
                    ..about(&job, outcome, word)
                }
            }));
        }
        Action::Cancel { .. } => {
            results.extend(daemon.cancel_set(&set, jobs, call.dry_run, &mut operation));
            shared.changed.notify_all();
        }
        Action::Remove { allow_lost } => {
            results.extend(daemon.remove_set(jobs, allow_lost, call.dry_run, &mut operation));
            shared.changed.notify_all();
        }
        action if call.dry_run => {
            results.extend(jobs.iter().map(|job| daemon.predicted(&action, job)));
        }
        action => {
            results.extend(daemon.applied_each(&action, jobs));
            shared.changed.notify_all();
        }
    }
    results.sort_by_key(|entry| entry.id);
    Response::Extended {
        answer: Box::new(Answer::Set {
            report: Report {
                schema_version: 1,
                kind: KIND.to_owned(),
                action,
                set: set.written,
                dry_run: call.dry_run,
                selected,
                results,
                operation,
            },
        }),
    }
}
