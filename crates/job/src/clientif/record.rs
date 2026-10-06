use serde_json::{Value, json};

use crate::daemon::Shared;
use crate::daemon::client_service::{self, Found, Loaded, Subject};
use crate::model::Job;

use super::args::{self, Args};
use super::row::Row;
use super::{confirmed, index, measure, spell};

const OP: &str = "job";

pub fn missing(what: &str) -> String {
    json!({"missing": what}).to_string()
}

pub enum Chosen {
    Absent(&'static str),
    Attempt {
        job: Box<Job>,
        current: bool,
        loaded: Box<Loaded>,
    },
}

pub fn chosen(shared: &Shared, id: u64, attempt: Option<u64>) -> Chosen {
    let loaded = match client_service::found(shared, id) {
        Found::Missing => return Chosen::Absent("job"),
        Found::Unreadable => return Chosen::Absent("record"),
        Found::Job(loaded) => loaded,
    };
    match attempt {
        Some(number) if number != loaded.job.attempt => {
            let archived = crate::attempts::list(&loaded.store, id)
                .ok()
                .and_then(|attempts| attempts.into_iter().find(|job| job.attempt == number));
            match archived {
                Some(job) => Chosen::Attempt {
                    job: Box::new(job),
                    current: false,
                    loaded,
                },
                None => Chosen::Absent("attempt"),
            }
        }
        _ => Chosen::Attempt {
            job: Box::new(loaded.job.clone()),
            current: true,
            loaded,
        },
    }
}

pub fn figures(
    subject: &Subject,
    now: u64,
    table: &mut Option<Vec<crate::procs::Stat>>,
) -> serde_json::Map<String, Value> {
    let sample = subject.sample.as_ref();
    match &subject.cgroup {
        Some(path) => {
            let mut counters = measure::Counters::read(path);
            if let Some(sample) = sample {
                counters.peak_memory = counters
                    .peak_memory
                    .map(|peak| peak.max(sample.peak_memory));
                counters.peak_pids = counters.peak_pids.map(|peak| peak.max(sample.peak_pids));
            }
            let mut figures = counters.written();
            figures.insert("cpu_at_ms".to_owned(), Value::from(now));
            figures
        }
        None => {
            let processes = subject.supervisor.map(|pid| {
                crate::procs::descendants(pid, table.get_or_insert_with(crate::procs::all))
            });
            let memory = processes.as_ref().map(|found| {
                found
                    .iter()
                    .map(|stat| crate::procs::page_bytes(stat.rss_pages))
                    .sum::<u64>()
            });
            let pids = processes
                .as_ref()
                .map(|found| found.iter().map(|stat| stat.threads).sum::<u64>());
            let peak = |now: Option<u64>, kept: Option<u64>| match (now, kept) {
                (Some(now), Some(kept)) => Some(now.max(kept)),
                (now, kept) => now.or(kept),
            };
            let mut figures = serde_json::Map::new();
            for (name, value) in [
                ("memory", memory),
                (
                    "peak_memory",
                    peak(memory, sample.map(|sample| sample.peak_memory)),
                ),
                ("pids", pids),
                (
                    "peak_pids",
                    peak(pids, sample.map(|sample| sample.peak_pids)),
                ),
                ("written", sample.map(|sample| sample.written)),
            ] {
                figures.insert(name.to_owned(), measure::figure(value));
            }
            for name in [
                "cpu_at_ms",
                "cpu_ms",
                "throttled_ms",
                "oom_kill",
                "oom_group_kill",
                "pids_max_events",
            ] {
                figures.insert(name.to_owned(), Value::from("not_measured"));
            }
            figures
        }
    }
}

fn live(loaded: &Loaded) -> Value {
    let now = crate::shim::now_ms();
    let mut figures = figures(&loaded.subject, now, &mut None);
    figures.insert("at_ms".to_owned(), Value::from(now));
    figures.insert("attempt".to_owned(), Value::from(loaded.job.attempt));
    Value::Object(figures)
}

pub fn answer(shared: &Shared, args: &Args) -> Result<String, String> {
    let id = args::needed(args, OP, "id")?;
    let attempt = args::number(args, OP, "attempt")?;
    let (job, current, loaded) = match chosen(shared, id, attempt) {
        Chosen::Absent(what) => return Ok(missing(what)),
        Chosen::Attempt {
            job,
            current,
            loaded,
        } => (job, current, loaded),
    };
    let read = confirmed::read(&confirmed::directory(&loaded.store, &job, current), &job);
    let seq = super::changes::last();
    let revision = if current {
        index::locked().revision(&job) + u64::from(loaded.learned)
    } else {
        job.durability.revision.unwrap_or(0)
    };
    let predicted = loaded.predicted.filter(|_| current);
    let row = Row::of(
        &job,
        revision,
        predicted,
        read.as_ref()
            .map_or_else(|| confirmed::absent(&job), confirmed::short),
    );
    Ok(json!({
        "seq": seq,
        "now_ms": crate::shim::now_ms(),
        "row": row.written(),
        "job": spell::record(&job, revision),
        "predicted_start_ms": predicted,
        "confirmed": read
            .as_ref()
            .map_or_else(|| confirmed::absent(&job), |read| confirmed::full(read, &job)),
        "live": if current && job.state.active() {
            live(&loaded)
        } else {
            Value::Null
        },
    })
    .to_string())
}
