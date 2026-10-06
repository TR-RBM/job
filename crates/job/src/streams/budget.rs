use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::store::Store;

fn recorded(name: &str) -> bool {
    matches!(name, "output.log" | "output.tail0" | "output.tail1")
        || (name.starts_with("streams-") && name.ends_with(".bin"))
}

fn files(directory: &Path) -> Vec<(PathBuf, u64)> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_name().to_str().is_some_and(recorded))
        .filter_map(|entry| {
            let metadata = fs::symlink_metadata(entry.path()).ok()?;
            metadata.is_file().then(|| (entry.path(), metadata.len()))
        })
        .collect()
}

fn mark(directory: &Path, bytes: u64) {
    let streams = directory.join("streams.json");
    if let Ok(mut meta) = super::read_metadata(&streams) {
        meta.trimmed_bytes = Some(meta.trimmed_bytes.unwrap_or(0).saturating_add(bytes));
        meta.segments.clear();
        let _ = crate::store::write_json(&streams, &meta);
    }
    let record = directory.join("job.json");
    let Some(mut job) = crate::store::read_json::<serde_json::Value>(&record) else {
        return;
    };
    let Some(result) = job.get_mut("result").filter(|result| result.is_object()) else {
        return;
    };
    if !result
        .get("output_retention")
        .is_some_and(|retention| retention.is_object())
    {
        result["output_retention"] = serde_json::json!({});
    }
    let before = result["output_retention"]["trimmed_bytes"]
        .as_u64()
        .unwrap_or(0);
    result["output_retention"]["trimmed_bytes"] = before.saturating_add(bytes).into();
    let _ = crate::store::write_json(&record, &job);
}

static UNREADABLE: Mutex<BTreeSet<u64>> = Mutex::new(BTreeSet::new());
static MEASURED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

static PASSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn passed() -> Option<u64> {
    PASSED
        .load(std::sync::atomic::Ordering::Relaxed)
        .then(measured)
}

pub fn measured() -> u64 {
    MEASURED.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn recorded_in(directory: &Path) -> u64 {
    whole(directory)
}

pub fn unreadable() -> Vec<u64> {
    UNREADABLE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .copied()
        .collect()
}

pub fn describe(ids: &[u64]) -> Option<String> {
    (!ids.is_empty()).then(|| {
        super::text(
            "output budget: the records of Jobs {ids} cannot be read; their output is counted and never trimmed",
            &[(
                "ids",
                ids.iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            )],
        )
    })
}

fn note(found: BTreeSet<u64>) {
    let mut known = UNREADABLE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for id in found.difference(&known) {
        eprintln!(
            "job daemon: {}",
            super::text(
                "the record of Job {id} cannot be read; its output is counted in the output budget and is never trimmed",
                &[("id", id.to_string())],
            )
        );
    }
    *known = found;
}

fn whole(directory: &Path) -> u64 {
    let own: u64 = files(directory).iter().map(|(_, bytes)| bytes).sum();
    let archived: u64 = fs::read_dir(directory.join("attempts"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| {
                    files(&entry.path())
                        .iter()
                        .map(|(_, bytes)| bytes)
                        .sum::<u64>()
                })
                .sum()
        })
        .unwrap_or(0);
    own.saturating_add(archived)
}

pub fn trim(store: &Store, budget: u64) -> Vec<u64> {
    let mut total: u64 = 0;
    let mut candidates = Vec::new();
    let mut unread = BTreeSet::new();
    for id in store.job_ids() {
        if let Ok(attempts) = crate::attempts::list(store, id) {
            for job in attempts {
                let Some(directory) = job.log.parent().map(Path::to_owned) else {
                    continue;
                };
                let files = files(&directory);
                let bytes: u64 = files.iter().map(|(_, bytes)| bytes).sum();
                total = total.saturating_add(bytes);
                if job.state.terminal() && bytes > 0 {
                    candidates.push((id, job.attempt, directory, files, bytes));
                }
            }
        } else if fs::symlink_metadata(store.job_file(id))
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            let directory = store.job_dir(id);
            let files: Vec<_> = files(&directory)
                .into_iter()
                .filter(|(path, _)| path.ends_with("output.log"))
                .collect();
            let bytes: u64 = files.iter().map(|(_, bytes)| bytes).sum();
            total = total.saturating_add(bytes);
            if bytes > 0 {
                candidates.push((id, 1, directory, files, bytes));
            }
        } else {
            total = total.saturating_add(whole(&store.job_dir(id)));
            unread.insert(id);
        }
    }
    note(unread);
    candidates.sort_unstable_by_key(|(id, attempt, ..)| (*id, *attempt));
    let mut removed = Vec::new();
    for (id, _, directory, files, bytes) in candidates {
        if total <= budget {
            break;
        }
        mark(&directory, bytes);
        for (path, _) in &files {
            let _ = fs::remove_file(path);
        }
        total = total.saturating_sub(bytes);
        removed.push(id);
    }
    MEASURED.store(total, std::sync::atomic::Ordering::Relaxed);
    PASSED.store(true, std::sync::atomic::Ordering::Relaxed);
    removed
}
