use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::model::{Env, HistoryEntry, Job, Queue, ShimResult};

#[derive(Clone, Debug)]
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn default_root() -> PathBuf {
        use crate::paths;
        paths::state(paths::mode(&paths::process), &paths::process)
            .unwrap_or_else(|_| PathBuf::from("/dev/null/job"))
    }

    pub fn validate_default_selection() -> io::Result<()> {
        use crate::paths;
        let mode = paths::mode(&paths::process);
        let root = paths::state(mode, &paths::process).map_err(io::Error::other)?;
        if paths::state_explicit(&paths::process) || mode == paths::Mode::System {
            return Ok(());
        }
        if Self::legacy_store(&root).is_some() {
            return Err(io::Error::other(crate::migration::message(
                "legacy exec state exists; migrate explicitly or select JOB_STATE_DIR",
            )));
        }
        Ok(())
    }

    pub fn legacy_store(root: &Path) -> Option<PathBuf> {
        let legacy = root.with_file_name("exec");
        ["jobs", "objects.json", "next-id", "daemon.sock"]
            .iter()
            .any(|name| legacy.join(name).exists())
            .then_some(legacy)
    }

    pub fn open(root: PathBuf) -> io::Result<Store> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)?;
        fs::create_dir_all(root.join("jobs"))?;
        Ok(Store { root })
    }

    pub fn socket(&self) -> PathBuf {
        crate::paths::socket_beside(&self.root)
    }

    pub fn lock(&self) -> PathBuf {
        self.root.join("daemon.lock")
    }

    pub fn job_dir(&self, id: u64) -> PathBuf {
        self.root.join("jobs").join(id.to_string())
    }

    pub fn job_file(&self, id: u64) -> PathBuf {
        self.job_dir(id).join("job.json")
    }

    pub fn env_file(&self, id: u64) -> PathBuf {
        self.job_dir(id).join("env.json")
    }

    pub fn result_file(&self, id: u64) -> PathBuf {
        self.job_dir(id).join("result.json")
    }

    pub fn log_file(&self, id: u64) -> PathBuf {
        self.job_dir(id).join("output.log")
    }

    pub fn load_queues(&self) -> io::Result<Vec<Queue>> {
        let entries = match fs::read_dir(self.root.join("queues")) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut queues = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.path().extension().is_some_and(|x| x == "json") {
                queues.push(
                    serde_json::from_slice::<Queue>(&fs::read(entry.path())?)
                        .map_err(io::Error::other)?,
                );
            }
        }
        queues.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(queues)
    }

    pub fn link_file(&self, name: &str) -> PathBuf {
        self.root.join("links").join(format!("{name}.json"))
    }

    pub fn link_log(&self, name: &str) -> PathBuf {
        self.root.join("links").join(format!("{name}.log"))
    }

    pub fn save_link(&self, link: &crate::link::Link) -> io::Result<()> {
        fs::create_dir_all(self.root.join("links"))?;
        write_json(&self.link_file(&link.name), link)
    }

    pub fn remove_link(&self, name: &str) {
        let _ = fs::remove_file(self.link_file(name));
        let _ = fs::remove_file(self.link_log(name));
    }

    pub fn load_links(&self) -> Vec<crate::link::Link> {
        let Ok(entries) = fs::read_dir(self.root.join("links")) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| read_json(&e.path()))
            .collect()
    }

    pub fn history_file(&self) -> PathBuf {
        self.root.join("history.jsonl")
    }

    pub fn next_id(&self) -> io::Result<u64> {
        let path = self.root.join("next-id");
        let next = fs::read_to_string(&path)
            .ok()
            .and_then(|t| t.trim().parse().ok())
            .unwrap_or(1u64);
        let highest_on_disk = self.job_ids().into_iter().max().map_or(0, |id| id + 1);
        let after_removed = crate::removal::highest_removed_id(self)?
            .checked_add(1)
            .ok_or_else(|| io::Error::other(crate::removal::message("invalid removal journal")))?;
        let id = next.max(highest_on_disk).max(after_removed);
        write_unsynced(&path, (id + 1).to_string().as_bytes())?;
        Ok(id)
    }

    pub fn job_ids(&self) -> Vec<u64> {
        let Ok(entries) = fs::read_dir(self.root.join("jobs")) else {
            return Vec::new();
        };
        let mut ids: Vec<u64> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse().ok())
            .collect();
        ids.sort_unstable();
        ids
    }

    pub fn save_job(&self, job: &Job) -> io::Result<()> {
        let job = &revised(job);
        fs::create_dir_all(self.job_dir(job.id))?;
        write_json(&self.job_file(job.id), job)?;
        crate::pacing::job_record_written();
        observed(self, job);
        Ok(())
    }

    pub fn save_job_before_sync(&self, job: &Job) -> io::Result<()> {
        let job = &revised(job);
        fs::create_dir_all(self.job_dir(job.id))?;
        let path = self.job_file(job.id);
        let (temporary, file) = temporary(&path, &serde_json::to_vec_pretty(job)?)?;
        crate::pacing::file(&file)?;
        drop(file);
        fs::rename(&temporary, &path)?;
        crate::pacing::job_record_written();
        observed(self, job);
        Ok(())
    }

    pub fn load_job(&self, id: u64) -> Option<Job> {
        let mut job: Job = read_json(&self.job_file(id))?;
        job.log = self.log_file(id);
        if job.state == crate::model::State::Finished {
            job.state = job.outcome();
        }
        job.update_timing(crate::shim::now_ms());
        Some(job)
    }

    pub fn publish_submission(
        &self,
        job: &Job,
        env: &Env,
        replace: bool,
        retry: bool,
        net_secret: Option<&str>,
    ) -> io::Result<()> {
        let job = &revised(job);
        let transactions = self.root.join(".transactions");
        fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&transactions)?;
        let path = transactions.join(format!(
            "{}-{}-{}",
            job.id,
            std::process::id(),
            crate::shim::now_ms()
        ));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        let stage = SubmissionStage(path);
        if replace {
            crate::attempts::stage(self, job, &stage.0, retry)?;
        }
        crate::netsecret::stage(&stage.0, net_secret)?;
        let record = staged(&stage.0.join("job.json"), &serde_json::to_vec_pretty(job)?)?;
        let environment = serde_json::to_vec_pretty(env)?;
        let current = staged(&stage.0.join("env.json"), &environment)?;
        let submitted = if replace {
            None
        } else {
            Some(staged(&stage.0.join("submitted-env.json"), &environment)?)
        };
        let mut files = vec![&record, &current];
        files.extend(submitted.as_ref());
        crate::pacing::together(&files, &[&stage.0])?;
        crate::pacing::job_record_written();
        let source = std::ffi::CString::new(stage.0.as_os_str().as_encoded_bytes())?;
        let destination =
            std::ffi::CString::new(self.job_dir(job.id).as_os_str().as_encoded_bytes())?;
        let flag = if replace {
            libc::RENAME_EXCHANGE
        } else {
            libc::RENAME_NOREPLACE
        };
        if unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                flag,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        crate::pacing::directory(&self.root.join("jobs"))?;
        observed(self, job);
        Ok(())
    }

    pub fn load_env(&self, id: u64) -> Option<Env> {
        read_json(&self.env_file(id))
    }

    pub fn save_result(&self, id: u64, result: &ShimResult) -> io::Result<()> {
        write_json(&self.result_file(id), result)
    }

    pub fn load_result(&self, id: u64) -> Option<ShimResult> {
        read_json(&self.result_file(id))
    }

    pub fn append_history(&self, entry: &HistoryEntry) -> io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.history_file())?;
        let mut line = serde_json::to_vec(entry)?;
        line.push(b'\n');
        file.write_all(&line)
    }

    pub fn load_history(&self) -> Vec<HistoryEntry> {
        let Ok(file) = fs::File::open(self.history_file()) else {
            return Vec::new();
        };
        io::BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| serde_json::from_str(&line).ok())
            .collect()
    }

    pub fn trim_logs(&self, budget: u64) -> Vec<u64> {
        crate::streams::budget::trim(self, budget)
    }
}

pub fn revised(job: &Job) -> Job {
    let mut saved = job.clone();
    saved.durability.revision = Some(crate::clientif::index::revised(job));
    saved
}

pub fn observed(store: &Store, job: &Job) {
    crate::operations::journal::observe_job(job);
    crate::clientif::index::observe(store, job);
}

struct SubmissionStage(PathBuf);

impl Drop for SubmissionStage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn staged(path: &Path, bytes: &[u8]) -> io::Result<fs::File> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.write_all(bytes)?;
    Ok(file)
}

fn temporary(path: &Path, bytes: &[u8]) -> io::Result<(PathBuf, fs::File)> {
    let temporary = path.with_extension(format!("tmp{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)?;
    file.write_all(bytes)?;
    Ok((temporary, file))
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let (temporary, file) = temporary(path, bytes)?;
    crate::pacing::file(&file)?;
    drop(file);
    fs::rename(&temporary, path)?;
    if let Some(parent) = path.parent() {
        crate::pacing::directory(parent)?;
    }
    Ok(())
}

pub fn write_unsynced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let (temporary, file) = temporary(path, bytes)?;
    drop(file);
    fs::rename(&temporary, path)
}

pub fn prepare(staging: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(staging)?;
    file.write_all(bytes)
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_atomic(path, &serde_json::to_vec_pretty(value)?)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> Store {
        let root =
            std::env::temp_dir().join(format!("job-store-test-{}-{name}", std::process::id()));
        Store::open(root).unwrap()
    }

    #[test]
    fn ids_are_never_reused() {
        let store = store("ids");
        let first = store.next_id().unwrap();
        let second = store.next_id().unwrap();
        assert_eq!(second, first + 1);
        fs::create_dir_all(store.job_dir(second + 10)).unwrap();
        assert_eq!(store.next_id().unwrap(), second + 11);
    }

    #[test]
    fn the_oldest_logs_are_removed_first_until_the_budget_holds() {
        let store = store("trim");
        for id in 1..=3u64 {
            fs::create_dir_all(store.job_dir(id)).unwrap();
            fs::write(store.log_file(id), vec![b'x'; 100]).unwrap();
        }
        assert_eq!(store.trim_logs(150), vec![1, 2]);
        assert!(store.log_file(3).exists());
    }
}
