use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use super::{CHUNK, HEADER, Mode, Record, Source, invalid, message, read_metadata, segment_name};
use crate::model::Job;
use crate::store::Store;

pub struct Batch {
    pub records: Vec<Record>,
    pub complete: bool,
    pub job: Job,
    pub error: Option<String>,
    pub quota: Option<super::quota::Quota>,
    pub trimmed: Option<u64>,
}

pub struct Reader {
    root: PathBuf,
    id: u64,
    pub attempt: u64,
    pub mode: Option<Mode>,
    segment: Option<u64>,
    offset: u64,
    pub(super) next_sequence: u64,
    pub(super) stop_sequence: Option<u64>,
    legacy_limit: Option<u64>,
    pub(super) split: super::split::State,
}

pub(super) fn regular(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid());
    }
    Ok(file)
}

fn directory(store: &Store, id: u64, attempt: u64) -> io::Result<(File, Job)> {
    for _ in 0..3 {
        let current = store.load_job(id).ok_or_else(invalid)?;
        let path = if current.attempt == attempt {
            store.job_dir(id)
        } else if current.attempt > attempt {
            store.job_dir(id).join("attempts").join(attempt.to_string())
        } else {
            return Err(invalid());
        };
        let dir = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
        {
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            result => result?,
        };
        let path = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
        let mut bytes = Vec::new();
        regular(&path.join("job.json"))?
            .take(16_777_217)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 16_777_216 {
            return Err(invalid());
        }
        let job: Job = serde_json::from_slice(&bytes)?;
        if job.id == id && job.attempt == attempt {
            return Ok((dir, job));
        }
    }
    Err(invalid())
}

impl Reader {
    pub fn open(store: &Store, id: u64, attempt: Option<u64>) -> io::Result<Self> {
        let attempt = attempt
            .or_else(|| store.load_job(id).map(|j| j.attempt))
            .ok_or_else(invalid)?;
        let (_, job) = directory(store, id, attempt)?;
        Ok(Self {
            root: store.root.clone(),
            id,
            attempt,
            mode: job.output_mode,
            segment: None,
            offset: 0,
            next_sequence: 0,
            stop_sequence: None,
            legacy_limit: None,
            split: Default::default(),
        })
    }

    pub fn snapshot(&mut self) -> io::Result<()> {
        let (dir, job) = directory(
            &Store {
                root: self.root.clone(),
            },
            self.id,
            self.attempt,
        )?;
        let root = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
        if self.mode.is_none() {
            self.legacy_limit = Some(match regular(&root.join("output.log")) {
                Ok(f) => f.metadata()?.len(),
                Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
                Err(e) => return Err(e),
            });
            return Ok(());
        }
        let meta = match read_metadata(&root.join("streams.json")) {
            Ok(meta) => meta,
            Err(e)
                if e.kind() == io::ErrorKind::NotFound
                    && (!job.state.terminal()
                        || job.started_ms.is_none()
                        || job.result.as_ref().is_some_and(|r| r.start_error.is_some())) =>
            {
                self.stop_sequence = Some(0);
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        if meta.quota.is_some() || meta.trimmed_bytes.is_some() {
            self.stop_sequence = Some(super::split::snapshot(&root, &meta)?);
            return Ok(());
        }
        let segment = meta.segments.last().ok_or_else(invalid)?;
        let file = regular(&root.join(segment_name(segment.number)))?;
        let size = file.metadata()?.len();
        if size > super::SEGMENT_BYTES {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        file.take(size).read_to_end(&mut bytes)?;
        let mut offset = 0;
        let mut next = segment.first_sequence;
        while bytes.len() - offset >= HEADER {
            let header = bytes[offset..offset + HEADER].try_into().unwrap();
            let length = Record::length(header)?;
            if bytes.len() - offset - HEADER < length {
                break;
            }
            let record = Record::decode(
                header,
                bytes[offset + HEADER..offset + HEADER + length].to_vec(),
            )?;
            if record.sequence != next {
                return Err(invalid());
            }
            next = next.checked_add(1).ok_or_else(invalid)?;
            offset += HEADER + length;
        }
        if meta.complete && (offset != bytes.len() || next != meta.next_sequence) {
            return Err(invalid());
        }
        self.stop_sequence = Some(next);
        Ok(())
    }

    pub fn read(&mut self) -> io::Result<Batch> {
        let (dir, job) = directory(
            &Store {
                root: self.root.clone(),
            },
            self.id,
            self.attempt,
        )?;
        if job.output_mode != self.mode {
            return Err(invalid());
        }
        let root = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
        let mut batch = Batch {
            records: Vec::new(),
            complete: false,
            error: job.result.as_ref().and_then(|r| r.output_error.clone()),
            job,
            quota: None,
            trimmed: None,
        };
        if self.mode.is_none() {
            return self.legacy(&root, batch);
        }
        let meta = match read_metadata(&root.join("streams.json")) {
            Err(e) if e.kind() == io::ErrorKind::NotFound && !batch.job.state.terminal() => {
                batch.complete = self.stop_sequence == Some(0);
                return Ok(batch);
            }
            Err(e)
                if e.kind() == io::ErrorKind::NotFound
                    && (batch.job.started_ms.is_none()
                        || batch
                            .job
                            .result
                            .as_ref()
                            .is_some_and(|r| r.start_error.is_some())) =>
            {
                batch.complete = true;
                return Ok(batch);
            }
            result => result?,
        };
        if meta.job_id != self.id || meta.attempt != self.attempt || Some(meta.mode) != self.mode {
            return Err(invalid());
        }
        if meta.quota.is_some() || meta.trimmed_bytes.is_some() {
            return super::split::read(self, &root, meta, batch);
        }
        let mut consumed = 0;
        while consumed < 4 * CHUNK {
            if self
                .stop_sequence
                .is_some_and(|end| self.next_sequence >= end)
            {
                batch.complete = true;
                if meta.complete {
                    batch.error = meta.error.clone().or(batch.error);
                } else if batch.job.state.terminal() {
                    batch.error = Some(message("output recording is incomplete"));
                }
                return Ok(batch);
            }
            let selected = match self.segment {
                None => 0,
                Some(number) => match meta.segments.iter().position(|s| s.number >= number) {
                    Some(index) => index,
                    None => return Err(invalid()),
                },
            };
            let segment = &meta.segments[selected];
            if self.segment != Some(segment.number) {
                self.segment = Some(segment.number);
                self.offset = 0;
            }
            let mut file = match regular(&root.join(segment_name(segment.number))) {
                Err(e) if e.kind() == io::ErrorKind::NotFound && !meta.complete => {
                    return Ok(batch);
                }
                result => result?,
            };
            let size = file.metadata()?.len();
            let torn = !meta.complete
                && batch.job.state.terminal()
                && selected + 1 < meta.segments.len()
                && size < segment.bytes;
            if size > super::SEGMENT_BYTES
                || self.offset > size
                || (!torn
                    && (meta.complete || selected + 1 < meta.segments.len())
                    && size != segment.bytes)
            {
                return Err(invalid());
            }
            file.seek(SeekFrom::Start(self.offset))?;
            if size == self.offset {
                if torn {
                    batch.complete = true;
                    batch.error = Some(message("output recording is incomplete"));
                    return Ok(batch);
                }
                if selected + 1 < meta.segments.len() {
                    self.segment = Some(meta.segments[selected + 1].number);
                    self.offset = 0;
                    continue;
                }
                batch.complete = meta.complete;
                batch.error = meta.error.clone().or(batch.error);
                if meta.complete && self.next_sequence != meta.next_sequence {
                    return Err(invalid());
                }
                if !meta.complete && batch.job.state.terminal() {
                    batch.complete = true;
                    batch.error = Some(message("output recording is incomplete"));
                }
                return Ok(batch);
            }
            let last = torn || selected + 1 == meta.segments.len();
            if size - self.offset < HEADER as u64 {
                if last && !meta.complete {
                    batch.complete = batch.job.state.terminal();
                    batch.error = batch
                        .complete
                        .then(|| message("output recording is incomplete"));
                    return Ok(batch);
                }
                return Err(invalid());
            }
            let mut header = [0; HEADER];
            file.read_exact(&mut header)?;
            let length = Record::length(&header)?;
            if size - self.offset - (HEADER as u64) < length as u64 {
                if last && !meta.complete {
                    batch.complete = batch.job.state.terminal();
                    batch.error = batch
                        .complete
                        .then(|| message("output recording is incomplete"));
                    return Ok(batch);
                }
                return Err(invalid());
            }
            let mut bytes = vec![0; length];
            file.read_exact(&mut bytes)?;
            let record = Record::decode(&header, bytes)?;
            if self.stop_sequence.is_some_and(|end| record.sequence >= end) {
                batch.complete = true;
                return Ok(batch);
            }
            if self.offset == 0 && record.sequence != segment.first_sequence {
                return Err(invalid());
            }
            if record.sequence < self.next_sequence {
                return Err(invalid());
            }
            if record.sequence > self.next_sequence {
                if self.offset != 0
                    || record.sequence != segment.first_sequence
                    || record.sequence - self.next_sequence > meta.retired_records
                {
                    return Err(invalid());
                }
                let missing = record.sequence - self.next_sequence;
                batch.records.push(Record::gap(
                    self.next_sequence,
                    record.at_ms,
                    missing,
                    if missing == meta.retired_records {
                        meta.retired_bytes
                            .saturating_sub(missing.saturating_mul(HEADER as u64))
                    } else {
                        0
                    },
                ));
            }
            if (meta.mode == Mode::Pty && matches!(record.source, Source::Stdout | Source::Stderr))
                || (meta.mode == Mode::Pipe && record.source == Source::Terminal)
                || matches!(record.source, Source::Combined | Source::End)
            {
                return Err(invalid());
            }
            self.next_sequence = record.sequence.checked_add(1).ok_or_else(invalid)?;
            self.offset += (HEADER + length) as u64;
            consumed += length;
            batch.records.push(record);
        }
        Ok(batch)
    }

    fn trimmed(&self, batch: &Batch) -> Option<u64> {
        batch
            .job
            .result
            .as_ref()
            .and_then(|result| result.output_retention.as_ref())
            .and_then(|retention| retention.trimmed_bytes)
    }

    fn legacy(&mut self, root: &Path, mut batch: Batch) -> io::Result<Batch> {
        if let Some(bytes) = self.trimmed(&batch)
            && !root.join("output.log").exists()
        {
            batch.trimmed = (!self.split.reported()).then_some(bytes);
            self.split.report();
            batch.complete = true;
            return Ok(batch);
        }
        if self.legacy_limit.is_some_and(|limit| self.offset >= limit) {
            batch.complete = true;
            return Ok(batch);
        }
        let mut file = match regular(&root.join("output.log")) {
            Ok(file) => file,
            Err(e)
                if e.kind() == io::ErrorKind::NotFound
                    && (!batch.job.state.terminal()
                        || (batch.job.started_ms.is_none()
                            || batch
                                .job
                                .result
                                .as_ref()
                                .is_some_and(|r| r.start_error.is_some()))) =>
            {
                batch.complete = batch.job.state.terminal();
                return Ok(batch);
            }

            Err(e) => return Err(e),
        };
        file.seek(SeekFrom::Start(self.offset))?;
        let length = self.legacy_limit.map_or(CHUNK, |limit| {
            (limit - self.offset).min(CHUNK as u64) as usize
        });
        let mut bytes = vec![0; length];
        let count = file.read(&mut bytes)?;
        bytes.truncate(count);
        self.offset += count as u64;
        if count > 0 {
            batch.records.push(Record {
                sequence: self.next_sequence,
                at_ms: 0,
                source: Source::Combined,
                bytes,
                gap_stream: None,
            });
            self.next_sequence += 1;
        } else {
            batch.complete = batch.job.state.terminal();
        }
        Ok(batch)
    }
}
