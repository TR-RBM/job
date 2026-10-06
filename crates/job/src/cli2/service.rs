use super::Daemon;
use crate::cli2::listing::{Listing, Query, SCAN_LIMIT, STATES, ended, state_name};
use crate::cli2::{Answer, Call, message};
use crate::model::{Job, QueueEntry, Response};

fn refused(text: String) -> Response {
    Response::Error { message: text }
}

impl Daemon {
    pub(super) fn extended(&mut self, call: Call) -> Response {
        match call {
            Call::List { query } => self.listed(&query),
            Call::Rows => {
                let mut view = self.queue_view();
                for entry in &mut view.jobs {
                    crate::cli2::listing::brief(&mut entry.job);
                }
                Response::Extended {
                    answer: Box::new(Answer::Rows { view }),
                }
            }
            Call::Move { id, queue } => self.moved(id, &queue),
            Call::Set { .. } => refused(crate::idset::message("a set of Job IDs is needed")),
            Call::Settings { path, key } => match self.objects.resolve(&path) {
                Ok(id) => Response::Extended {
                    answer: Box::new(Answer::Settings {
                        explanation: crate::cli2::settings::explain(
                            &self.objects,
                            id,
                            key.as_deref(),
                            &|file| self.tree.as_ref().is_some_and(|tree| tree.supports(file)),
                        ),
                    }),
                },
                Err(_) => refused(
                    message("there is no Queue or Group `{word}`")
                        .replace("{word}", path.trim_start_matches('/')),
                ),
            },
        }
    }

    fn listed(&self, query: &Query) -> Response {
        if let Some(unknown) = query
            .states
            .iter()
            .find(|name| !STATES.contains(&name.as_str()))
        {
            return refused(format!("unknown Job state {unknown}"));
        }
        let queue = match &query.queue {
            Some(path) => match self.objects.resolve(path) {
                Ok(id) if self.objects.nodes[&id].kind == crate::objects::Kind::Queue => {
                    Some(self.objects.path(id))
                }
                _ => {
                    return refused(
                        message("there is no Queue `{word}`")
                            .replace("{word}", path.trim_start_matches('/')),
                    );
                }
            },
            None => None,
        };
        let limit = usize::try_from(query.limit.clamp(1, crate::cli2::listing::MAX_LIMIT))
            .unwrap_or(usize::MAX);
        let wanted = |job: &Job| {
            query.states.iter().any(|name| name == state_name(job))
                && queue
                    .as_ref()
                    .is_none_or(|path| job.spec.queue.as_ref() == Some(path))
                && query.labels.iter().all(|(key, value)| {
                    job.spec
                        .declared
                        .labels
                        .get(key)
                        .is_some_and(|known| known == value)
                })
        };
        let now = crate::shim::now_ms();
        let mut jobs: Vec<QueueEntry> = Vec::new();
        let mut more = false;
        let stored = if query.states.iter().any(|name| ended(name)) {
            self.store.job_ids()
        } else {
            Vec::new()
        };
        let mut ids: Vec<u64> = self.jobs.keys().copied().chain(stored).collect();
        ids.sort_unstable();
        ids.dedup();
        if let Some(text) = &query.ids {
            match crate::idset::parse(&[text.as_str()]) {
                Ok(set) => ids.retain(|id| set.ids.binary_search(id).is_ok()),
                Err(message) => return refused(message),
            }
        }
        let mut read = 0usize;
        for id in ids.into_iter().rev() {
            let job = match self.jobs.get(&id) {
                Some(job) => {
                    let mut snapshot = job.clone();
                    snapshot.update_timing(now);
                    snapshot
                }
                None => {
                    if read == SCAN_LIMIT {
                        more = true;
                        break;
                    }
                    read += 1;
                    match self.store.load_job(id) {
                        Some(job) => job,
                        None => continue,
                    }
                }
            };
            let mut job = job;
            if job.state == crate::model::State::Finished {
                job.state = job.outcome();
            }
            if !wanted(&job) {
                continue;
            }
            if query.brief {
                crate::cli2::listing::brief(&mut job);
            }
            if jobs.len() == limit {
                more = true;
                break;
            }
            jobs.push(QueueEntry {
                predicted_start_ms: self.predicted_starts.get(&job.id).copied().flatten(),
                job,
            });
        }
        jobs.reverse();
        Response::Extended {
            answer: Box::new(Answer::Jobs {
                listing: Listing {
                    schema_version: 1,
                    jobs,
                    more,
                },
            }),
        }
    }

    pub(super) fn moved(&mut self, id: u64, queue: &str) -> Response {
        let Some(job) = self.load(id) else {
            return refused(message("there is no Job {id}").replace("{id}", &id.to_string()));
        };
        if !job.state.editable() {
            return refused(
                message("Job {id} is {state}; only held and queued Jobs can be moved")
                    .replace("{id}", &id.to_string())
                    .replace("{state}", state_name(&job)),
            );
        }
        let destination = match self.objects.resolve(queue) {
            Ok(target) if self.objects.nodes[&target].kind == crate::objects::Kind::Queue => target,
            _ => {
                return refused(
                    message("there is no Queue `{word}`")
                        .replace("{word}", queue.trim_start_matches('/')),
                );
            }
        };
        if job.queue_id == Some(destination) {
            return Response::Submitted { job };
        }
        let Some(env) = self.store.load_env(id) else {
            return refused(message(
                "the saved environment of the Job is unavailable; it was not moved",
            ));
        };
        let mut spec = job
            .requested_spec
            .clone()
            .unwrap_or_else(|| job.spec.clone());
        spec.queue = Some(self.objects.path(destination));
        self.put_job(
            spec,
            env,
            Some(id),
            job.state == crate::model::State::Held,
            false,
            None,
        )
    }
}
