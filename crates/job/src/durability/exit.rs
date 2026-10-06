use std::fs;
use std::path::Path;

use crate::model::{Job, ShimResult};
use crate::store::Store;

fn sync(path: &Path) {
    if let Ok(directory) = fs::File::open(path) {
        let _ = directory.sync_all();
    }
}

fn set_aside(store: &Store, id: u64, attempt: u64, name: &str) {
    let directory = store.job_dir(id);
    let archive = directory.join("attempts").join(attempt.to_string());
    let target = if fs::symlink_metadata(&archive).is_ok_and(|meta| meta.is_dir()) {
        archive.join(format!("late-{name}"))
    } else {
        directory.join(format!("stale-{attempt}-{name}"))
    };
    if fs::rename(directory.join(name), &target).is_ok() {
        if let Some(parent) = target.parent() {
            sync(parent);
        }
        sync(&directory);
    }
}

pub fn result(store: &Store, job: &Job) -> Option<ShimResult> {
    let result = store.load_result(job.id)?;
    match result.attempt {
        Some(attempt) if attempt != job.attempt => {
            set_aside(store, job.id, attempt, "result.json");
            None
        }
        _ => Some(result),
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Exit {
    pub attempt: u64,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub at_ms: u64,
}

pub fn record(
    store: &Store,
    id: u64,
    attempt: u64,
    status: &std::io::Result<std::process::ExitStatus>,
) {
    use std::os::unix::process::ExitStatusExt;
    let Ok(status) = status else {
        return;
    };
    let exit = Exit {
        attempt,
        exit_code: status.code(),
        signal: status.signal(),
        at_ms: crate::shim::now_ms(),
    };
    if let Err(error) = super::write_in_boot(&store.job_dir(id).join("exit.json"), &exit) {
        eprintln!(
            "{}: {error}",
            super::message("cannot record the exit status")
        );
    }
}

pub fn recorded(store: &Store, job: &Job) -> Option<ShimResult> {
    let exit: Exit = crate::store::read_json(&store.job_dir(job.id).join("exit.json"))?;
    if exit.attempt != job.attempt {
        set_aside(store, job.id, exit.attempt, "exit.json");
        return None;
    }
    Some(ShimResult {
        attempt: Some(exit.attempt),
        exit_code: exit.exit_code,
        signal: exit.signal,
        finished_ms: exit.at_ms,
        notes: vec![super::message(
            "its supervisor ended after the command exited and before cleanup completed; leftover processes, usage and the end of the output were not recorded",
        )],
        ..ShimResult::default()
    })
}

pub fn save_result(store: &Store, id: u64, result: &ShimResult) {
    super::failpoint("shim-before-result");
    for attempt in 1..=2 {
        match super::failpoint::fails("shim-result-save")
            .and_then(|()| store.save_result(id, result))
        {
            Ok(()) => return,
            Err(error) => eprintln!(
                "{} ({attempt}/2): {error}",
                super::message("cannot write the result")
            ),
        }
    }
}

pub fn save_terminal(store: &Store, job: &Job) {
    for attempt in 1..=2 {
        match super::failpoint::fails("finalize-save").and_then(|()| store.save_job(job)) {
            Ok(()) => return,
            Err(error) => eprintln!(
                "job {}: {} ({attempt}/2): {error}",
                job.id,
                super::message("cannot write the final record")
            ),
        }
    }
}
