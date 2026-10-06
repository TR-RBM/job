use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::logfile::CappedLog;
use crate::model::Job;

pub mod budget;
pub mod capture;
pub mod cli;
mod messages;
pub mod quota;
pub mod reader;
mod split;
pub use messages::{message, text};

pub const PROTOCOL: u32 = 1;
pub const CHUNK: usize = 65_536;
pub const HEADER: usize = 32;
const SEGMENT_BYTES: u64 = 1_048_576;
const SEGMENTS: usize = 64;
const HEAD_SEGMENTS: usize = 32;
const METADATA_BYTES: usize = 4 << 20;
const SOURCES: usize = 7;
pub const READ: usize = 1 << 20;
pub const PUBLISH_MS: u64 = 100;
const DOOMED: usize = 64;
const SYNC_EACH: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Pipe,
    Pty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Stdout,
    Stderr,
    Terminal,
    Diagnostic,
    Gap,
    Combined,
    End,
}

impl Source {
    fn code(self) -> u8 {
        match self {
            Self::Stdout => 1,
            Self::Stderr => 2,
            Self::Terminal => 3,
            Self::Diagnostic => 4,
            Self::Gap => 5,
            Self::Combined => 6,
            Self::End => 7,
        }
    }
    fn parse(code: u8) -> io::Result<Self> {
        match code {
            1 => Ok(Self::Stdout),
            2 => Ok(Self::Stderr),
            3 => Ok(Self::Terminal),
            4 => Ok(Self::Diagnostic),
            5 => Ok(Self::Gap),
            6 => Ok(Self::Combined),
            7 => Ok(Self::End),
            _ => Err(invalid()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Record {
    pub sequence: u64,
    pub at_ms: u64,
    pub source: Source,
    pub bytes: Vec<u8>,
    pub gap_stream: Option<Source>,
}

impl Record {
    pub fn encode(&self) -> io::Result<Vec<u8>> {
        if self.bytes.is_empty() || self.bytes.len() > CHUNK {
            return Err(invalid());
        }
        let mut bytes = vec![0; HEADER];
        bytes[..4].copy_from_slice(b"JOL1");
        bytes[4..8].copy_from_slice(&(self.bytes.len() as u32).to_le_bytes());
        bytes[8..16].copy_from_slice(&self.sequence.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.at_ms.to_le_bytes());
        bytes[24] = self.source.code();
        bytes.extend_from_slice(&self.bytes);
        Ok(bytes)
    }

    fn frame(
        out: &mut Vec<u8>,
        sequence: u64,
        at_ms: u64,
        source: Source,
        bytes: &[u8],
    ) -> io::Result<()> {
        if bytes.is_empty() || bytes.len() > CHUNK {
            return Err(invalid());
        }
        let start = out.len();
        out.resize(start + HEADER, 0);
        out[start..start + 4].copy_from_slice(b"JOL1");
        out[start + 4..start + 8].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        out[start + 8..start + 16].copy_from_slice(&sequence.to_le_bytes());
        out[start + 16..start + 24].copy_from_slice(&at_ms.to_le_bytes());
        out[start + 24] = source.code();
        out.extend_from_slice(bytes);
        Ok(())
    }

    pub fn decode(header: &[u8; HEADER], bytes: Vec<u8>) -> io::Result<Self> {
        if header[..4] != *b"JOL1"
            || header[25..].iter().any(|v| *v != 0)
            || Self::length(header)? != bytes.len()
        {
            return Err(invalid());
        }
        let source = Source::parse(header[24])?;
        if (source == Source::End && bytes != [0]) || (source == Source::Gap && bytes.len() != 16) {
            return Err(invalid());
        }
        Ok(Self {
            sequence: u64::from_le_bytes(header[8..16].try_into().unwrap()),
            at_ms: u64::from_le_bytes(header[16..24].try_into().unwrap()),
            source,
            bytes,
            gap_stream: None,
        })
    }

    pub fn length(header: &[u8; HEADER]) -> io::Result<usize> {
        let length = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        if header[..4] != *b"JOL1" || length == 0 || length > CHUNK {
            return Err(invalid());
        }
        Ok(length)
    }

    pub fn gap(sequence: u64, at_ms: u64, records: u64, bytes: u64) -> Self {
        Self {
            sequence,
            at_ms,
            source: Source::Gap,
            bytes: [records.to_le_bytes(), bytes.to_le_bytes()].concat(),
            gap_stream: None,
        }
    }

    pub fn gap_counts(&self) -> (u64, u64) {
        (
            u64::from_le_bytes(self.bytes[..8].try_into().unwrap()),
            u64::from_le_bytes(self.bytes[8..].try_into().unwrap()),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    Head,
    Tail,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chain {
    pub stream: Source,
    pub part: Part,
    pub offset: u64,
    pub record: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    pub number: u64,
    pub first_sequence: u64,
    pub bytes: u64,
    pub records: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<Chain>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub schema_version: u32,
    pub job_id: u64,
    pub attempt: u64,
    pub mode: Mode,
    pub segments: Vec<Segment>,
    pub complete: bool,
    pub error: Option<String>,
    pub omitted_bytes: u64,
    pub retired_bytes: u64,
    pub retired_records: u64,
    pub next_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<quota::Quota>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub totals: Vec<quota::Total>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trimmed_bytes: Option<u64>,
}

impl Metadata {
    pub fn growing(&self, index: usize) -> bool {
        !self.complete
            && match &self.segments[index].chain {
                None => index + 1 == self.segments.len(),
                Some(chain) => !self.segments[index + 1..].iter().any(|later| {
                    later
                        .chain
                        .as_ref()
                        .is_some_and(|c| c.stream == chain.stream && c.part == chain.part)
                }),
            }
    }

    pub fn validate(&self) -> io::Result<()> {
        if self.schema_version != PROTOCOL || self.job_id == 0 || self.attempt == 0 {
            return Err(invalid());
        }
        if self.trimmed_bytes.is_some() {
            return if self.segments.is_empty() {
                Ok(())
            } else {
                Err(invalid())
            };
        }
        if let Some(quota) = self.quota {
            return split::validate(self, quota);
        }
        if !self.totals.is_empty()
            || self.segments.iter().any(|s| s.chain.is_some())
            || self.segments.is_empty()
            || self.segments.len() > SEGMENTS
            || self.segments.iter().any(|s| s.bytes > SEGMENT_BYTES)
            || self
                .segments
                .windows(2)
                .any(|p| p[0].number >= p[1].number || p[0].first_sequence >= p[1].first_sequence)
        {
            return Err(invalid());
        }
        let first = &self.segments[0];
        let last = self.segments.last().unwrap();
        let records = self
            .segments
            .iter()
            .try_fold(self.retired_records, |sum, segment| {
                sum.checked_add(segment.records)
            })
            .ok_or_else(invalid)?;
        if first.number != 0
            || first.first_sequence != 0
            || records != self.next_sequence
            || last.first_sequence.checked_add(last.records) != Some(self.next_sequence)
            || self.segments.iter().any(|s| {
                s.records
                    .checked_mul((HEADER + 1) as u64)
                    .is_none_or(|min| min > s.bytes)
            })
            || self.segments.windows(2).any(|p| {
                let end = p[0].first_sequence.checked_add(p[0].records);
                end.is_none_or(|end| {
                    end > p[1].first_sequence
                        || (p[0].number.checked_add(1) == Some(p[1].number)
                            && end != p[1].first_sequence)
                })
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
}

pub fn invalid() -> io::Error {
    io::Error::other(message("invalid output recording"))
}
pub fn segment_name(number: u64) -> String {
    format!("streams-{number:020}.bin")
}

pub fn read_metadata(path: &Path) -> io::Result<Metadata> {
    let mut bytes = Vec::new();
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid());
    }
    file.take(METADATA_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > METADATA_BYTES {
        return Err(invalid());
    }
    let meta: Metadata = serde_json::from_slice(&bytes)?;
    meta.validate()?;
    Ok(meta)
}

#[derive(Default)]
struct Tally {
    written: [u64; SOURCES],
    dropped: [u64; SOURCES],
    segments: Vec<[u64; SOURCES]>,
}

struct Writer {
    root: PathBuf,
    meta: Metadata,
    file: Option<File>,
    last_ms: u64,
    tally: Tally,
    split: Option<split::Chains>,
    pending: Vec<u8>,
    interval: std::time::Duration,
    published: std::time::Instant,
    dirty: bool,
    doomed: Vec<u64>,
}

impl Writer {
    fn create(root: &Path, job: &Job, mode: Mode, quota: Option<quota::Quota>) -> io::Result<Self> {
        let file = match quota {
            None => Some(Self::create_segment(root, 0)?),
            Some(_) => None,
        };
        let writer = Self {
            root: root.to_owned(),
            file,
            last_ms: 0,
            tally: Tally {
                segments: vec![[0; SOURCES]],
                ..Tally::default()
            },
            split: quota.map(split::Chains::new),
            pending: Vec::new(),
            interval: std::time::Duration::ZERO,
            published: std::time::Instant::now(),
            dirty: false,
            doomed: Vec::new(),
            meta: Metadata {
                schema_version: PROTOCOL,
                job_id: job.id,
                attempt: job.attempt,
                mode,
                segments: match quota {
                    None => vec![Segment {
                        number: 0,
                        first_sequence: 0,
                        bytes: 0,
                        records: 0,
                        chain: None,
                    }],
                    Some(_) => Vec::new(),
                },
                complete: false,
                error: None,
                omitted_bytes: 0,
                retired_bytes: 0,
                retired_records: 0,
                next_sequence: 0,
                quota,
                totals: Vec::new(),
                trimmed_bytes: None,
            },
        };
        writer.save(true)?;
        Ok(writer)
    }

    fn create_segment(root: &Path, number: u64) -> io::Result<File> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join(segment_name(number)))
    }

    fn totals(&self) -> Vec<quota::Total> {
        (0..SOURCES)
            .filter(|index| {
                self.tally.written[*index] > 0 && *index + 1 != Source::Gap.code() as usize
            })
            .filter_map(|index| {
                Some(quota::Total {
                    stream: Source::parse(index as u8 + 1).ok()?,
                    written_bytes: self.tally.written[index],
                    dropped_bytes: self.tally.dropped[index],
                })
            })
            .collect()
    }

    fn retention(&self) -> quota::Retention {
        quota::Retention {
            head_bytes: self.meta.quota.map(|quota| quota.head_bytes),
            tail_bytes: self.meta.quota.map(|quota| quota.tail_bytes),
            streams: self.totals(),
            omitted_bytes: self.meta.omitted_bytes,
            trimmed_bytes: None,
        }
    }

    fn save(&self, durable: bool) -> io::Result<()> {
        let path = self.root.join("streams.json");
        let bytes = if self.meta.quota.is_some() {
            let mut meta = self.meta.clone();
            meta.totals = self.totals();
            serde_json::to_vec(&meta)?
        } else {
            serde_json::to_vec_pretty(&self.meta)?
        };
        if durable {
            return crate::store::write_atomic(&path, &bytes);
        }
        let temporary = path.with_extension(format!("tmp{}", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_data()?;
        std::fs::rename(&temporary, &path)
    }

    fn publish(&mut self) -> io::Result<()> {
        self.save(false)?;
        self.dirty = false;
        self.published = std::time::Instant::now();
        for number in std::mem::take(&mut self.doomed) {
            std::fs::remove_file(self.root.join(segment_name(number)))?;
        }
        Ok(())
    }

    fn changed(&mut self) -> io::Result<()> {
        self.dirty = true;
        if self.doomed.len() >= DOOMED || self.published.elapsed() >= self.interval {
            self.publish()?;
        }
        Ok(())
    }

    fn tick(&mut self) -> io::Result<()> {
        if self.dirty && self.published.elapsed() >= self.interval {
            self.publish()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let written = self
            .file
            .as_mut()
            .ok_or_else(invalid)?
            .write_all(&self.pending);
        self.pending.clear();
        written
    }

    fn stamp(&mut self, timestamp: Option<u64>) -> u64 {
        timestamp.unwrap_or_else(|| {
            self.last_ms = crate::shim::now_ms().max(self.last_ms);
            self.last_ms
        })
    }

    fn write(&mut self, source: Source, bytes: &[u8], timestamp: Option<u64>) -> io::Result<()> {
        if self.split.is_some() {
            return self.write_split(source, bytes, timestamp);
        }
        let index = source.code() as usize - 1;
        for bytes in bytes.chunks(CHUNK) {
            let at_ms = self.stamp(timestamp);
            let length = (HEADER + bytes.len()) as u64;
            if self.meta.segments.last().unwrap().bytes + length > SEGMENT_BYTES {
                self.flush()?;
                self.rotate()?;
            }
            Record::frame(
                &mut self.pending,
                self.meta.next_sequence,
                at_ms,
                source,
                bytes,
            )?;
            let last = self.meta.segments.last_mut().unwrap();
            last.bytes += length;
            last.records += 1;
            self.tally.written[index] += bytes.len() as u64;
            self.tally.segments.last_mut().unwrap()[index] += bytes.len() as u64;
            self.meta.next_sequence = self.meta.next_sequence.checked_add(1).ok_or_else(invalid)?;
        }
        self.flush()
    }

    fn rotate(&mut self) -> io::Result<()> {
        let number = self
            .meta
            .segments
            .last()
            .unwrap()
            .number
            .checked_add(1)
            .ok_or_else(invalid)?;
        let file = Self::create_segment(&self.root, number)?;
        self.meta.segments.push(Segment {
            number,
            first_sequence: self.meta.next_sequence,
            bytes: 0,
            records: 0,
            chain: None,
        });
        self.tally.segments.push([0; SOURCES]);
        let retired = if self.meta.segments.len() > SEGMENTS {
            let payload = self.tally.segments.remove(HEAD_SEGMENTS);
            for (dropped, bytes) in self.tally.dropped.iter_mut().zip(payload) {
                *dropped += bytes;
            }
            Some(self.meta.segments.remove(HEAD_SEGMENTS))
        } else {
            None
        };
        if let Some(segment) = &retired {
            self.meta.retired_bytes = self.meta.retired_bytes.saturating_add(segment.bytes);
            self.meta.retired_records = self.meta.retired_records.saturating_add(segment.records);
        }
        self.file = Some(file);
        if let Some(segment) = retired {
            self.doomed.push(segment.number);
        }
        self.changed()
    }

    fn settle(&self) -> io::Result<()> {
        if self.meta.segments.len() > SYNC_EACH {
            let directory = File::open(&self.root)?;
            if unsafe { libc::syncfs(std::os::fd::AsRawFd::as_raw_fd(&directory)) } < 0 {
                return Err(io::Error::last_os_error());
            }
            return Ok(());
        }
        for segment in &self.meta.segments {
            File::open(self.root.join(segment_name(segment.number)))?.sync_data()?;
        }
        Ok(())
    }

    fn finish(mut self, error: Option<String>) -> io::Result<()> {
        let stored = self.flush().and_then(|_| self.settle());
        self.meta.error = error.or_else(|| stored.as_ref().err().map(|e| e.to_string()));
        self.meta.complete = true;
        self.save(true)?;
        for number in std::mem::take(&mut self.doomed) {
            std::fs::remove_file(self.root.join(segment_name(number)))?;
        }
        stored
    }
}

pub struct Recorder {
    legacy: CappedLog,
    writer: Writer,
    error: Option<String>,
}

impl Recorder {
    pub fn create(job: &Job, path: &Path) -> io::Result<Self> {
        let mode = job.output_mode.ok_or_else(invalid)?;
        let quota = quota::Quota::of(&job.spec.declared.output);
        if quota.is_some_and(|quota| !quota.valid()) {
            return Err(invalid());
        }
        Ok(Self {
            legacy: match quota {
                Some(quota) => CappedLog::with_limits(path, quota.head_bytes, quota.tail_bytes)?,
                None => CappedLog::create(path)?,
            },
            writer: Writer::create(path.parent().ok_or_else(invalid)?, job, mode, quota)?,
            error: None,
        })
    }

    pub fn paced(&mut self) {
        self.writer.interval = std::time::Duration::from_millis(PUBLISH_MS);
    }

    pub fn tick(&mut self) {
        if self.error.is_none()
            && let Err(e) = self.writer.tick()
        {
            self.error(e.to_string());
        }
    }

    pub fn error(&mut self, error: String) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }

    pub fn write(&mut self, source: Source, bytes: &[u8]) {
        if let Err(e) = self.legacy.write_all(bytes) {
            self.error(e.to_string());
        }
        if self.error.is_none()
            && let Err(e) = self.writer.write(source, bytes, None)
        {
            self.error(e.to_string());
        }
    }

    pub fn remote(&mut self, record: &Record) {
        if record.source == Source::Gap {
            self.writer.meta.omitted_bytes = self
                .writer
                .meta
                .omitted_bytes
                .saturating_add(record.gap_counts().1);
        } else if let Err(e) = self.legacy.write_all(&record.bytes) {
            self.error(e.to_string());
        }
        if self.error.is_none()
            && let Err(e) = self
                .writer
                .write(record.source, &record.bytes, Some(record.at_ms))
        {
            self.error(e.to_string());
        }
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.write(Source::Terminal, bytes);
        Ok(())
    }

    pub fn omit(&mut self, bytes: u64) {
        self.writer.meta.omitted_bytes = self.writer.meta.omitted_bytes.saturating_add(bytes);
        if let Err(e) = self
            .writer
            .write(Source::Gap, &Record::gap(0, 0, 0, bytes).bytes, None)
        {
            self.error(e.to_string());
        }
    }

    pub fn finish(self) -> Outcome {
        let Self {
            legacy,
            writer,
            mut error,
        } = self;
        let bytes = match legacy.finish() {
            Ok(bytes) => bytes,
            Err(e) => {
                error.get_or_insert_with(|| e.to_string());
                0
            }
        };
        let retention = Some(writer.retention());
        if let Err(e) = writer.finish(error.clone()) {
            error.get_or_insert_with(|| e.to_string());
        }
        Outcome {
            bytes,
            error,
            retention,
        }
    }
}

pub struct Outcome {
    pub bytes: u64,
    pub error: Option<String>,
    pub retention: Option<quota::Retention>,
}
impl Outcome {
    pub fn incomplete() -> Self {
        Self {
            bytes: 0,
            error: Some(message("output recording is incomplete")),
            retention: None,
        }
    }
}

pub fn validate(job: &Job, root: &Path) -> io::Result<()> {
    let Some(mode) = job.output_mode else {
        return Ok(());
    };
    let meta = match read_metadata(&root.join("streams.json")) {
        Ok(meta) => meta,
        Err(e)
            if e.kind() == io::ErrorKind::NotFound
                && (!job.state.terminal()
                    || job.started_ms.is_none()
                    || job.result.as_ref().is_some_and(|r| r.start_error.is_some())) =>
        {
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    if meta.job_id != job.id
        || meta.attempt != job.attempt
        || meta.mode != mode
        || meta.quota != quota::Quota::of(&job.spec.declared.output)
    {
        return Err(invalid());
    }
    for (index, segment) in meta.segments.iter().enumerate() {
        let file = match std::fs::symlink_metadata(root.join(segment_name(segment.number))) {
            Err(e) if e.kind() == io::ErrorKind::NotFound && !meta.complete => continue,
            result => result?,
        };
        if !file.is_file()
            || file.len() > SEGMENT_BYTES
            || (!meta.growing(index)
                && if meta.complete {
                    file.len() != segment.bytes
                } else {
                    file.len() > segment.bytes
                })
        {
            return Err(invalid());
        }
    }
    Ok(())
}
