use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{Job, State};
use crate::objects::Graph;
use crate::units::format_duration_ms;

use super::{journal, message};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waits {
    pub window_ms: u64,
    pub sample_limit: u64,
    pub count: u64,
    pub median_ms: Option<u64>,
    pub max_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Depth {
    pub held: u64,
    pub queued: u64,
    pub starting: u64,
    pub running: u64,
    pub suspended: u64,
    pub stopping: u64,
    pub oldest_queued_age_ms: Option<u64>,
    pub started_last_hour: Waits,
}

pub fn median(sorted: &[u64]) -> Option<u64> {
    let middle = sorted.len() / 2;
    match sorted.len() {
        0 => None,
        length if length % 2 == 1 => Some(sorted[middle]),
        _ => Some(
            sorted[middle - 1] / 2
                + sorted[middle] / 2
                + (sorted[middle - 1] % 2 + sorted[middle] % 2) / 2,
        ),
    }
}

pub fn measure(graph: &Graph, jobs: &BTreeMap<u64, Job>, id: u64, now: u64) -> Depth {
    let inside = |queue: Option<u64>| queue.is_some_and(|queue| graph.within(queue, id));
    let mut depth = Depth::default();
    for job in jobs.values().filter(|job| inside(job.queue_id)) {
        match job.state {
            State::Held => depth.held += 1,
            State::Queued => {
                depth.queued += 1;
                let age = now.saturating_sub(job.waiting_since());
                depth.oldest_queued_age_ms = Some(
                    depth
                        .oldest_queued_age_ms
                        .map_or(age, |oldest| oldest.max(age)),
                );
            }
            State::Starting => depth.starting += 1,
            State::Running => depth.running += 1,
            State::Suspended => depth.suspended += 1,
            State::Stopping => depth.stopping += 1,
            _ => {}
        }
    }
    let mut waits: Vec<u64> = journal::starts_since(now.saturating_sub(journal::WAIT_WINDOW_MS))
        .into_iter()
        .filter(|start| inside(start.queue_id))
        .map(|start| start.wait_ms)
        .collect();
    waits.sort_unstable();
    depth.started_last_hour = Waits {
        window_ms: journal::WAIT_WINDOW_MS,
        sample_limit: journal::WAIT_SAMPLES as u64,
        count: waits.len() as u64,
        median_ms: median(&waits),
        max_ms: waits.last().copied(),
    };
    depth
}

pub fn render(depth: &Depth) -> Vec<String> {
    let duration =
        |value: Option<u64>| value.map_or_else(|| message("none", &[]), format_duration_ms);
    vec![
        message(
            "jobs: {held} held, {queued} queued, {starting} starting, {running} running, {suspended} suspended, {stopping} stopping",
            &[
                ("held", depth.held.to_string()),
                ("queued", depth.queued.to_string()),
                ("starting", depth.starting.to_string()),
                ("running", depth.running.to_string()),
                ("suspended", depth.suspended.to_string()),
                ("stopping", depth.stopping.to_string()),
            ],
        ),
        message(
            "oldest queued Job waits since: {age}",
            &[("age", duration(depth.oldest_queued_age_ms))],
        ),
        message(
            "started in the last hour: {count} Jobs, median wait {median}, longest wait {max}",
            &[
                ("count", depth.started_last_hour.count.to_string()),
                ("median", duration(depth.started_last_hour.median_ms)),
                ("max", duration(depth.started_last_hour.max_ms)),
            ],
        ),
    ]
}
