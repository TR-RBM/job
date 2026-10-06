use std::collections::BTreeMap;

use crate::model::{Job, State};
use crate::objects::Graph;
use crate::resources::Vector;

use super::{Ledger, backfill, fifo_predecessor, message};

#[derive(Clone, Debug)]
struct Slot {
    start: u64,
    end: Option<u64>,
    need: Vector,
    devices: Vec<String>,
    ancestors: Vec<u64>,
}

#[derive(Clone, Debug)]
pub struct Decision {
    pub id: u64,
    pub start: Option<u64>,
    pub reason: Option<String>,
}

fn duration(job: &Job) -> Option<u64> {
    match job.reservation.wall_limit_ms {
        Some(limit) => limit
            .checked_add(crate::daemon::GRACE_MS)
            .and_then(|duration| duration.checked_add(crate::daemon::TICK.as_millis() as u64)),
        None => job.reservation.predicted_ms,
    }
}

fn add_time(at: u64, duration: Option<u64>) -> Option<u64> {
    duration.and_then(|duration| at.checked_add(duration))
}

fn room(capacity: Vector, active: &[&Slot], need: Vector) -> bool {
    let used = active.iter().fold(
        [
            need.cores_milli as u128,
            need.memory as u128,
            need.pids as u128,
        ],
        |mut sum, slot| {
            sum[0] = sum[0].saturating_add(slot.need.cores_milli as u128);
            sum[1] = sum[1].saturating_add(slot.need.memory as u128);
            sum[2] = sum[2].saturating_add(slot.need.pids as u128);
            sum
        },
    );
    used[0] <= capacity.cores_milli as u128
        && used[1] <= capacity.memory as u128
        && used[2] <= capacity.pids as u128
}

fn competes(graph: &Graph, earlier: &Slot, later: &Slot) -> bool {
    (earlier.need.cores_milli > 0 && later.need.cores_milli > 0)
        || (earlier.need.memory > 0 && later.need.memory > 0)
        || (earlier.need.pids > 0 && later.need.pids > 0)
        || earlier
            .devices
            .iter()
            .any(|device| later.devices.contains(device))
        || earlier.ancestors.iter().any(|id| {
            later.ancestors.contains(id)
                && graph.nodes[id]
                    .config
                    .get("max_running")
                    .is_some_and(|value| value.as_u64().is_some())
        })
}

fn fits(graph: &Graph, capacity: Vector, slots: &[Slot], candidate: &Slot) -> bool {
    let overlaps = |slot: &&Slot| {
        candidate.end.is_none_or(|end| slot.start < end)
            && slot.end.is_none_or(|end| end > candidate.start)
    };
    let relevant: Vec<_> = slots.iter().filter(overlaps).collect();
    if relevant.iter().any(|slot| {
        slot.devices
            .iter()
            .any(|device| candidate.devices.contains(device))
    }) {
        return false;
    }
    let mut points = vec![candidate.start];
    points.extend(
        relevant
            .iter()
            .map(|slot| slot.start)
            .filter(|&time| time > candidate.start),
    );
    for time in points {
        let active: Vec<_> = relevant
            .iter()
            .copied()
            .filter(|slot| slot.start <= time && slot.end.is_none_or(|end| end > time))
            .collect();
        if !room(capacity, &active, candidate.need) {
            return false;
        }
        for id in &candidate.ancestors {
            let config = &graph.nodes[id].config;
            let scoped: Vec<_> = active
                .iter()
                .copied()
                .filter(|slot| slot.ancestors.contains(id))
                .collect();
            if config
                .get("max_running")
                .and_then(|v| v.as_u64())
                .is_some_and(|cap| scoped.len() as u64 >= cap)
            {
                return false;
            }
            let cap = Vector {
                cores_milli: config
                    .get("cores_milli")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(u64::MAX),
                memory: config
                    .get("memory")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(u64::MAX),
                pids: u64::MAX,
            };
            if !room(cap, &scoped, candidate.need) {
                return false;
            }
        }
    }
    true
}

pub fn plan(
    graph: &Graph,
    ledger: &Ledger,
    jobs: &BTreeMap<u64, Job>,
    capacity: Vector,
    now: u64,
    gates: &BTreeMap<u64, Result<(), (String, bool)>>,
) -> Vec<Decision> {
    let mut slots: Vec<_> = jobs
        .values()
        .filter(|job| job.state.active())
        .map(|job| {
            let start = job.durability.admitted_ms.or(job.started_ms).unwrap_or(now);
            let end = duration(job).and_then(|duration| {
                let at = start.checked_add(duration)?;
                if at > now {
                    Some(at)
                } else if job.reservation.wall_limit_ms.is_some() {
                    None
                } else {
                    Some(crate::schedule::extended_end(start, duration, now))
                }
            });
            Slot {
                start,
                end,
                need: job.reservation.vector,
                devices: job.reservation.devices.clone(),
                ancestors: graph.ancestors(job.queue_id.unwrap()),
            }
        })
        .collect();
    let queued: Vec<_> = jobs
        .values()
        .filter(|job| job.state == State::Queued)
        .collect();
    let mut decisions = Vec::new();
    let mut barriers = Vec::<(u64, Slot)>::new();
    let mut eligible = Vec::new();
    for job in queued {
        if let Some((id, path)) = fifo_predecessor(graph, jobs, job) {
            decisions.push(Decision {
                id: job.id,
                start: None,
                reason: Some(format!(
                    "{} {id}: {path}",
                    message("strict FIFO waits for Job")
                )),
            });
            continue;
        }
        if let Some(Err((reason, false))) = gates.get(&job.id) {
            decisions.push(Decision {
                id: job.id,
                start: None,
                reason: Some(reason.clone()),
            });
            continue;
        }
        eligible.push(job);
    }
    let ordered = match ledger.fair.order(graph, ledger, eligible.clone()) {
        Ok(ordered) => ordered,
        Err(reason) => {
            decisions.extend(eligible.into_iter().map(|job| Decision {
                id: job.id,
                start: None,
                reason: Some(reason.clone()),
            }));
            return decisions;
        }
    };
    for job in ordered {
        let candidate = |start| Slot {
            start,
            end: add_time(start, duration(job)),
            need: job.reservation.vector,
            devices: job.reservation.devices.clone(),
            ancestors: graph.ancestors(job.queue_id.unwrap()),
        };
        let protect = super::ordered(graph, job) || job.policy.legacy();
        let no_backfill = !backfill(graph, job.queue_id.unwrap(), job.policy.legacy());
        let immediate = candidate(now);
        if let Some(id) = barriers
            .iter()
            .find(|(_, earlier)| competes(graph, earlier, &immediate))
            .map(|(id, _)| *id)
        {
            if protect {
                barriers.push((job.id, immediate));
            }
            decisions.push(Decision {
                id: job.id,
                start: None,
                reason: Some(format!("{} {id}", message("protected opportunity for Job"))),
            });
            continue;
        }
        let mut times: Vec<_> = slots
            .iter()
            .filter_map(|slot| slot.end)
            .filter(|&at| at > now)
            .collect();
        times.push(now);
        times.sort_unstable();
        times.dedup();
        let at = times.into_iter().find(|&at| {
            fits(graph, capacity, &slots, &candidate(at))
                && (at != now || gates.get(&job.id).is_none_or(Result::is_ok))
        });
        if protect && at != Some(now) && no_backfill {
            barriers.push((job.id, immediate));
        }
        let reason = if at == Some(now) {
            None
        } else {
            Some(
                gates
                    .get(&job.id)
                    .and_then(|gate| gate.as_ref().err())
                    .map_or_else(
                        || message("waiting for resources or an unknown running-job end"),
                        |(reason, _)| reason.clone(),
                    ),
            )
        };
        let mut protected = candidate(at.unwrap_or(now));
        if at.is_none() {
            protected.end = None;
        }
        if protect || at == Some(now) {
            slots.push(protected);
        }
        decisions.push(Decision {
            id: job.id,
            start: at,
            reason,
        });
    }
    decisions
}
