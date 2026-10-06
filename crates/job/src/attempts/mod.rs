use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
use std::path::Path;

use crate::model::Job;
use crate::store::{Store, read_json};

fn invalid() -> io::Error {
    io::Error::other(crate::lifecycle::message("invalid attempt archive"))
}

pub fn list(store: &Store, id: u64) -> io::Result<Vec<Job>> {
    let current = store.load_job(id).ok_or_else(invalid)?;
    let root = store.job_dir(id).join("attempts");
    match fs::symlink_metadata(&root) {
        Ok(metadata) if !metadata.is_dir() => return Err(invalid()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut archives = Vec::new();
    match fs::read_dir(&root) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let number: u64 = entry
                    .file_name()
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(invalid)?;
                if !entry.file_type()?.is_dir() || entry.file_name() != number.to_string().as_str()
                {
                    return Err(invalid());
                }
                let mut job: Job = read_json(&entry.path().join("job.json")).ok_or_else(invalid)?;
                for file in fs::read_dir(entry.path())? {
                    if !file?.file_type()?.is_file() {
                        return Err(invalid());
                    }
                }
                for name in ["env.json", "submitted-env.json"] {
                    if entry.path().join(name).exists() {
                        let _: crate::model::Env =
                            serde_json::from_slice(&fs::read(entry.path().join(name))?)?;
                    }
                }
                if entry.path().join("result.json").exists() {
                    let _: crate::model::ShimResult =
                        serde_json::from_slice(&fs::read(entry.path().join("result.json"))?)?;
                }
                if job.id != id || job.attempt != number || !job.state.terminal() {
                    return Err(invalid());
                }
                job.log = entry.path().join("output.log");
                job.update_timing(crate::shim::now_ms());
                archives.push(job);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    archives.sort_by_key(|job| job.attempt);
    if current.attempt == 0
        || archives.len() as u64 != current.attempt - 1
        || archives
            .iter()
            .enumerate()
            .any(|(index, job)| job.attempt != index as u64 + 1)
    {
        return Err(invalid());
    }
    archives.push(current);
    Ok(archives)
}

pub fn stage(store: &Store, job: &Job, destination: &Path, retry: bool) -> io::Result<()> {
    let attempts = list(store, job.id)?;
    let previous = attempts.last().ok_or_else(invalid)?;
    if retry && (!previous.state.terminal() || previous.attempt.checked_add(1) != Some(job.attempt))
    {
        return Err(invalid());
    }
    let archive = destination
        .join("attempts")
        .join(previous.attempt.to_string());
    if retry {
        fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&archive)?;
    }
    for entry in fs::read_dir(store.job_dir(job.id))? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if entry.file_name() == "attempts" && kind.is_dir() {
            link_archives(&entry.path(), &destination.join("attempts"))?;
        } else if kind.is_file() {
            let target = if retry { &archive } else { destination };
            fs::copy(entry.path(), target.join(entry.file_name()))?;
            fs::File::open(target.join(entry.file_name()))?.sync_all()?;
            if retry && entry.file_name() == "submitted-env.json" {
                fs::copy(entry.path(), destination.join(entry.file_name()))?;
                fs::File::open(destination.join(entry.file_name()))?.sync_all()?;
            }
        } else if !(retry && kind.is_socket() && entry.file_name() == "terminal.sock") {
            return Err(invalid());
        }
    }
    if retry {
        fs::File::open(&archive)?.sync_all()?;
        fs::File::open(destination.join("attempts"))?.sync_all()?;
    }
    Ok(())
}

fn link_archives(source: &Path, destination: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            return Err(invalid());
        }
        let target = destination.join(entry.file_name());
        fs::DirBuilder::new().mode(0o700).create(&target)?;
        for file in fs::read_dir(entry.path())? {
            let file = file?;
            if !file.file_type()?.is_file() {
                return Err(invalid());
            }
            fs::hard_link(file.path(), target.join(file.file_name()))?;
        }
        fs::File::open(target)?.sync_all()?;
    }
    fs::File::open(destination)?.sync_all()
}
