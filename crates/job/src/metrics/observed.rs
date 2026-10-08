use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard};

use crate::model::{Job, State};

use super::text::{Exposition, Kind, seconds};

pub const BOUNDS_MS: [u64; 14] = [
    100, 500, 1_000, 5_000, 10_000, 30_000, 60_000, 300_000, 600_000, 1_800_000, 3_600_000,
    10_800_000, 43_200_000, 86_400_000,
];
pub const QUEUES: usize = 1024;
pub const OTHER: &str = "(other)";

#[derive(Clone, Default)]
struct Histogram {
    buckets: [u64; BOUNDS_MS.len()],
    count: u64,
    sum_ms: u64,
}

impl Histogram {
    fn observe(&mut self, ms: u64) {
        for (bucket, bound) in self.buckets.iter_mut().zip(BOUNDS_MS) {
            if ms <= bound {
                *bucket += 1;
            }
        }
        self.count += 1;
        self.sum_ms = self.sum_ms.saturating_add(ms);
    }
}

#[derive(Default)]
struct Observed {
    queues: BTreeSet<String>,
    waits: BTreeMap<String, Histogram>,
    runs: BTreeMap<String, Histogram>,
    ended: BTreeMap<(String, &'static str), u64>,
}

impl Observed {
    fn label(&mut self, job: &Job) -> String {
        let queue = job.spec.queue.clone().unwrap_or_default();
        if self.queues.contains(&queue) {
            return queue;
        }
        if self.queues.len() >= QUEUES {
            return OTHER.to_owned();
        }
        self.queues.insert(queue.clone());
        queue
    }
}

static OBSERVED: Mutex<Option<Observed>> = Mutex::new(None);

fn locked() -> MutexGuard<'static, Option<Observed>> {
    let mut guard = OBSERVED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        *guard = Some(Observed::default());
    }
    guard
}

pub fn transition(job: &Job, to: &State, wait_ms: Option<u64>) {
    let mut guard = locked();
    let Some(observed) = guard.as_mut() else {
        return;
    };
    if let Some(wait) = wait_ms {
        let queue = observed.label(job);
        observed.waits.entry(queue).or_default().observe(wait);
    }
    if to.terminal() {
        let queue = observed.label(job);
        if let (Some(started), Some(finished)) = (job.started_ms, job.finished_ms) {
            observed
                .runs
                .entry(queue.clone())
                .or_default()
                .observe(finished.saturating_sub(started));
        }
        *observed
            .ended
            .entry((queue, crate::operations::journal::state_name(to)))
            .or_default() += 1;
    }
}

fn histogram(text: &mut Exposition, name: &str, help: &str, kept: &BTreeMap<String, Histogram>) {
    text.family(name, Kind::Histogram, help);
    let bucket = format!("{name}_bucket");
    for (queue, histogram) in kept {
        for (count, bound) in histogram.buckets.iter().zip(BOUNDS_MS) {
            let le = seconds(bound).to_string();
            text.sample(&bucket, &[("queue", queue), ("le", &le)], count);
        }
        text.sample(
            &bucket,
            &[("queue", queue), ("le", "+Inf")],
            histogram.count,
        );
        text.sample(
            &format!("{name}_sum"),
            &[("queue", queue)],
            seconds(histogram.sum_ms),
        );
        text.sample(
            &format!("{name}_count"),
            &[("queue", queue)],
            histogram.count,
        );
    }
}

pub fn written(text: &mut Exposition) {
    let guard = locked();
    let Some(observed) = guard.as_ref() else {
        return;
    };
    histogram(
        text,
        "job_wait_duration_seconds",
        "Time a Job waited from becoming eligible to being admitted, observed when an attempt starts, since the service started.",
        &observed.waits,
    );
    histogram(
        text,
        "job_run_duration_seconds",
        "Wall time from the start of a Job's command to the end of its attempt, observed when the attempt ends, since the service started.",
        &observed.runs,
    );
    text.family(
        "job_ended_total",
        Kind::Counter,
        "Attempts that reached a final state since the service started, by Queue and final state.",
    );
    for ((queue, state), count) in &observed.ended {
        text.sample(
            "job_ended_total",
            &[("queue", queue), ("state", state)],
            count,
        );
    }
}
