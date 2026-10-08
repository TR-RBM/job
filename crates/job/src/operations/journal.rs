use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::model::{Job, State};
use crate::objects::{Graph, Kind};
use crate::pressure::control::{Ledger, Phase};
use crate::store::Store;

use super::message;

pub const SCHEMA: u32 = 1;
pub const WAIT_WINDOW_MS: u64 = 3_600_000;
pub const WAIT_SAMPLES: usize = 4096;
const CURRENT: &str = "current.jsonl";
const REASON_LIMIT: usize = 512;
const PATH_LIMIT: usize = 1024;
const ROTATION_RETRIES: usize = 4;
const SYNCED: [&str; 4] = ["succeeded", "failed", "cancelled", "lost"];

fn default_keep() -> u32 {
    8
}

fn default_rotate() -> u64 {
    8 << 20
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default = "default_keep")]
    pub keep_files: u32,
    #[serde(default = "default_rotate")]
    pub rotate_bytes: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            keep_files: default_keep(),
            rotate_bytes: default_rotate(),
        }
    }
}

impl Settings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.keep_files == 0 || self.rotate_bytes == 0 {
            return Err(message(
                "events keep_files and rotate_bytes must be at least 1",
                &[],
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub seq: u64,
    pub at_ms: u64,
    pub kind: String,
    #[serde(default)]
    pub job: Option<u64>,
    #[serde(default)]
    pub attempt: Option<u64>,
    #[serde(default)]
    pub queue_id: Option<u64>,
    #[serde(default)]
    pub queue_path: Option<String>,
    #[serde(default)]
    pub object_id: Option<u64>,
    #[serde(default)]
    pub object_path: Option<String>,
    #[serde(default)]
    pub from: Option<String>,
    pub to: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub signal: Option<i32>,
    #[serde(default)]
    pub wait_ms: Option<u64>,
}

impl Record {
    fn blank(kind: &str, to: &str) -> Self {
        Self {
            seq: 0,
            at_ms: crate::shim::now_ms(),
            kind: kind.to_owned(),
            job: None,
            attempt: None,
            queue_id: None,
            queue_path: None,
            object_id: None,
            object_path: None,
            from: None,
            to: to.to_owned(),
            reason: None,
            exit_code: None,
            signal: None,
            wait_ms: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Gate {
    kind: Kind,
    path: String,
    paused: bool,
    closed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Start {
    pub at_ms: u64,
    pub queue_id: Option<u64>,
    pub wait_ms: u64,
}

struct Journal {
    directory: PathBuf,
    file: File,
    size: u64,
    next_seq: u64,
    first_seq: Option<u64>,
    settings: Settings,
    jobs: HashMap<u64, (u64, State)>,
    gates: BTreeMap<u64, Gate>,
    holds: BTreeMap<String, (Phase, Option<u64>, String)>,
    starts: VecDeque<Start>,
    transitions: u64,
    unsynced: bool,
}

static JOURNAL: Mutex<Option<Journal>> = Mutex::new(None);
static WRITABLE: AtomicBool = AtomicBool::new(false);

fn directory(store: &Store) -> PathBuf {
    store.root.join("events")
}

fn sync(path: &Path) -> io::Result<()> {
    crate::pacing::directory(path)
}

fn append_only(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

fn file_name(first: u64) -> String {
    format!("{first}.jsonl")
}

fn rotated(directory: &Path) -> io::Result<Vec<u64>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)? {
        let name = entry?.file_name();
        if let Some(first) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".jsonl"))
            .and_then(|stem| stem.parse::<u64>().ok())
            .filter(|first| name.to_str() == Some(file_name(*first).as_str()))
        {
            files.push(first);
        }
    }
    files.sort_unstable();
    Ok(files)
}

pub fn parse(bytes: &[u8]) -> (Vec<Record>, usize) {
    let mut found = Vec::new();
    let mut unreadable = 0;
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<Record>(line) {
            Ok(record) => found.push(record),
            Err(_) => unreadable += 1,
        }
    }
    (found, unreadable)
}

fn bounded(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

pub fn state_name(state: &State) -> &'static str {
    match state {
        State::Held => "held",
        State::Queued => "queued",
        State::Starting => "starting",
        State::Running => "running",
        State::Suspended => "suspended",
        State::Stopping => "stopping",
        State::Succeeded => "succeeded",
        State::Failed => "failed",
        State::Cancelled => "cancelled",
        State::Lost => "lost",
        State::Finished => "finished",
    }
}

fn settled(job: &Job) -> State {
    if job.state == State::Finished {
        job.outcome()
    } else {
        job.state.clone()
    }
}

fn stop_reason(job: &Job) -> Option<String> {
    job.stop
        .as_ref()
        .map(|stop| format!("{:?}: {}", stop.kind, stop.line))
}

fn reason(job: &Job, from: Option<&State>, to: &State, retried: bool) -> String {
    match to {
        State::Held if retried => "retry held".to_owned(),
        State::Held => "created held".to_owned(),
        State::Queued if retried => "retry".to_owned(),
        State::Queued => match from {
            None => "submitted".to_owned(),
            Some(State::Held) => "released".to_owned(),
            Some(_) => "launch not confirmed".to_owned(),
        },
        State::Starting => "admitted".to_owned(),
        State::Running => match from {
            Some(State::Suspended) => "continued".to_owned(),
            _ => "supervisor started".to_owned(),
        },
        State::Suspended => "suspended".to_owned(),
        State::Stopping => stop_reason(job).unwrap_or_else(|| "stopping".to_owned()),
        _ => stop_reason(job).unwrap_or_else(|| job.exit_description()),
    }
}

impl Journal {
    fn open(store: &Store, settings: Settings) -> io::Result<Self> {
        let directory = directory(store);
        match fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => sync(&store.root)?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        if !fs::symlink_metadata(&directory)?.is_dir() {
            return Err(io::Error::other(message("invalid event journal", &[])));
        }
        let path = directory.join(CURRENT);
        let existing = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        let mut file = append_only(&path)?;
        let mut size = existing.len() as u64;
        if existing.last().is_some_and(|byte| *byte != b'\n') {
            file.write_all(b"\n")?;
            crate::pacing::data(&file)?;
            size += 1;
        }
        sync(&directory)?;
        let (found, _) = parse(&existing);
        let mut last = found.last().map(|record| record.seq);
        if last.is_none()
            && let Some(first) = rotated(&directory)?.last().copied()
        {
            let (older, _) = parse(&fs::read(directory.join(file_name(first)))?);
            last = Some(older.last().map_or(first, |record| record.seq));
        }
        Ok(Self {
            directory,
            file,
            size,
            next_seq: last.map_or(1, |seq| seq + 1),
            first_seq: found.first().map(|record| record.seq),
            settings,
            jobs: HashMap::new(),
            gates: BTreeMap::new(),
            holds: BTreeMap::new(),
            starts: VecDeque::new(),
            transitions: 0,
            unsynced: false,
        })
    }

    fn write(&mut self, record: &Record) -> io::Result<()> {
        let mut line = serde_json::to_vec(record)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        if record.kind != "job" {
            crate::pacing::data(&self.file)?;
            self.unsynced = false;
        } else if SYNCED.contains(&record.to.as_str()) {
            self.unsynced = true;
        }
        self.size += line.len() as u64;
        self.first_seq.get_or_insert(record.seq);
        self.next_seq = record.seq + 1;
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        let first = self.first_seq.unwrap_or(self.next_seq);
        let closed = file_name(first);
        crate::pacing::data(&self.file)?;
        self.unsynced = false;
        fs::rename(self.directory.join(CURRENT), self.directory.join(&closed))?;
        let mut files = rotated(&self.directory)?;
        let mut dropped = Vec::new();
        while files.len() + 1 > self.settings.keep_files as usize && !files.is_empty() {
            let oldest = files.remove(0);
            fs::remove_file(self.directory.join(file_name(oldest)))?;
            dropped.push(oldest);
        }
        self.file = append_only(&self.directory.join(CURRENT))?;
        self.size = 0;
        self.first_seq = None;
        sync(&self.directory)?;
        let mut record = Record::blank("journal", "rotated");
        record.seq = self.next_seq;
        record.from = Some(closed);
        record.reason = Some(format!(
            "dropped {} older files; keep_files {}, rotate_bytes {}",
            dropped.len(),
            self.settings.keep_files,
            self.settings.rotate_bytes
        ));
        self.write(&record)
    }

    fn append(&mut self, mut record: Record) {
        record.reason = record.reason.map(|text| bounded(&text, REASON_LIMIT));
        record.queue_path = record.queue_path.map(|text| bounded(&text, PATH_LIMIT));
        record.object_path = record.object_path.map(|text| bounded(&text, PATH_LIMIT));
        let result = (|| -> io::Result<()> {
            record.seq = self.next_seq;
            let length = serde_json::to_vec(&record)?.len() as u64 + 1;
            if self.size > 0 && self.size + length > self.settings.rotate_bytes {
                self.rotate()?;
                record.seq = self.next_seq;
            }
            self.write(&record)
        })();
        WRITABLE.store(result.is_ok(), Ordering::Relaxed);
        if let Err(error) = result {
            eprintln!("{}: {error}", message("cannot write the event record", &[]));
        }
    }

    fn remember_start(&mut self, start: Start) {
        while self
            .starts
            .front()
            .is_some_and(|oldest| oldest.at_ms.saturating_add(WAIT_WINDOW_MS) < start.at_ms)
            || self.starts.len() >= WAIT_SAMPLES
        {
            self.starts.pop_front();
        }
        self.starts.push_back(start);
    }

    fn observe_job(&mut self, job: &Job) {
        let to = settled(job);
        let previous = self.jobs.get(&job.id).cloned();
        if previous
            .as_ref()
            .is_some_and(|(attempt, state)| *attempt == job.attempt && *state == to)
        {
            return;
        }
        let from = previous.as_ref().map(|(_, state)| state.clone());
        let retried = previous
            .as_ref()
            .is_some_and(|(attempt, _)| *attempt != job.attempt);
        let requeued =
            to == State::Queued && from.as_ref().is_some_and(|state| *state != State::Held);
        let mut record = Record::blank(
            "job",
            if requeued {
                "requeued"
            } else {
                state_name(&to)
            },
        );
        record.job = Some(job.id);
        record.attempt = Some(job.attempt);
        record.queue_id = job.queue_id;
        record.queue_path = job.spec.queue.clone();
        record.from = from.as_ref().map(|state| state_name(state).to_owned());
        record.reason = Some(reason(job, from.as_ref(), &to, retried));
        if to.terminal() {
            record.exit_code = job.result.as_ref().and_then(|result| result.exit_code);
            record.signal = job.result.as_ref().and_then(|result| result.signal);
        }
        if to == State::Starting {
            let admitted = job.durability.admitted_ms.unwrap_or(record.at_ms);
            let wait = admitted.saturating_sub(job.waiting_since());
            record.wait_ms = Some(wait);
            self.remember_start(Start {
                at_ms: admitted,
                queue_id: job.queue_id,
                wait_ms: wait,
            });
        }
        self.jobs.insert(job.id, (job.attempt, to));
        self.transitions += 1;
        self.append(record);
    }

    fn observe_objects(&mut self, graph: &Graph, quiet: bool) {
        let mut next = BTreeMap::new();
        let mut records = Vec::new();
        for node in graph.nodes.values() {
            let gate = Gate {
                kind: node.kind,
                path: graph.path(node.id),
                paused: node.paused,
                closed: node.closed || node.draining,
            };
            let before = self.gates.get(&node.id);
            let kind = match node.kind {
                Kind::Queue => "queue",
                Kind::Group => "group",
            };
            let was_paused = before.is_some_and(|gate| gate.paused);
            let was_closed = before.is_some_and(|gate| gate.closed);
            for (old, new, on, off, why) in [
                (was_paused, gate.paused, "paused", "resumed", None),
                (
                    was_closed,
                    gate.closed,
                    "closed",
                    "open",
                    node.draining.then_some("draining"),
                ),
            ] {
                if old != new {
                    let mut record = Record::blank(kind, if new { on } else { off });
                    record.object_id = Some(node.id);
                    record.object_path = Some(gate.path.clone());
                    record.from = Some(if old { on } else { off }.to_owned());
                    record.reason = why.filter(|_| new).map(str::to_owned);
                    records.push(record);
                }
            }
            next.insert(node.id, gate);
        }
        self.gates = next;
        if !quiet {
            for record in records {
                self.append(record);
            }
        }
    }

    fn observe_pressure(&mut self, ledger: &Ledger, graph: &Graph, quiet: bool) {
        let name = |phase: Phase| match phase {
            Phase::Open => "pressure_open",
            Phase::Holding => "pressure_hold",
            Phase::Recovering => "pressure_recovering",
        };
        let kind_of = |scope: Option<u64>| match scope.and_then(|id| graph.nodes.get(&id)) {
            Some(node) if node.kind == Kind::Queue => "queue",
            Some(_) => "group",
            None if scope.is_none() => "host",
            None => "object",
        };
        let mut next = BTreeMap::new();
        let mut records = Vec::new();
        for (key, view) in &ledger.views {
            let before = self
                .holds
                .get(key)
                .map_or(Phase::Open, |(phase, _, _)| *phase);
            if before != view.phase {
                let mut record = Record::blank(kind_of(view.scope_id), name(view.phase));
                record.object_id = view.scope_id;
                record.object_path = Some(view.scope_path.clone());
                record.from = Some(name(before).to_owned());
                record.reason = Some(format!("pressure rule {}", view.rule.id));
                records.push(record);
            }
            next.insert(
                key.clone(),
                (view.phase, view.scope_id, view.scope_path.clone()),
            );
        }
        for (key, (phase, scope, path)) in &self.holds {
            if !next.contains_key(key) && *phase != Phase::Open {
                let mut record = Record::blank(kind_of(*scope), name(Phase::Open));
                record.object_id = *scope;
                record.object_path = Some(path.clone());
                record.from = Some(name(*phase).to_owned());
                record.reason = Some("pressure rule removed".to_owned());
                records.push(record);
            }
        }
        self.holds = next;
        if !quiet {
            for record in records {
                self.append(record);
            }
        }
    }
}

fn locked() -> std::sync::MutexGuard<'static, Option<Journal>> {
    JOURNAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn open(store: &Store, settings: &Settings, graph: &Graph, ledger: &Ledger) -> io::Result<()> {
    let mut journal = Journal::open(store, settings.clone())?;
    let now = crate::shim::now_ms();
    for id in store.job_ids() {
        let Some(job) = store.load_job(id) else {
            continue;
        };
        if let Some(started) = job
            .durability
            .admitted_ms
            .or(job.started_ms)
            .filter(|started| started.saturating_add(WAIT_WINDOW_MS) >= now)
        {
            journal.remember_start(Start {
                at_ms: started,
                queue_id: job.queue_id,
                wait_ms: started.saturating_sub(job.waiting_since()),
            });
        }
        journal.jobs.insert(id, (job.attempt, settled(&job)));
    }
    journal
        .starts
        .make_contiguous()
        .sort_by_key(|start| start.at_ms);
    journal.observe_objects(graph, true);
    journal.observe_pressure(ledger, graph, true);
    WRITABLE.store(true, Ordering::Relaxed);
    *locked() = Some(journal);
    Ok(())
}

pub fn configure(settings: &Settings) {
    if let Some(journal) = locked().as_mut() {
        journal.settings = settings.clone();
    }
}

pub fn observe_job(job: &Job) {
    if let Some(journal) = locked().as_mut() {
        journal.observe_job(job);
    }
}

pub fn observe_objects(graph: &Graph) {
    if let Some(journal) = locked().as_mut() {
        journal.observe_objects(graph, false);
    }
}

pub fn observe_pressure(ledger: &Ledger, graph: &Graph) {
    if let Some(journal) = locked().as_mut() {
        journal.observe_pressure(ledger, graph, false);
    }
}

pub fn ensure_terminal(jobs: &[Job]) -> io::Result<usize> {
    let mut journal = locked();
    let Some(journal) = journal.as_mut() else {
        return Ok(0);
    };
    let wanted: std::collections::HashMap<u64, u64> = jobs
        .iter()
        .filter(|job| settled(job).terminal())
        .map(|job| (job.id, job.attempt))
        .collect();
    if wanted.is_empty() {
        return Ok(0);
    }
    let mut journaled = std::collections::HashSet::new();
    let mut names: Vec<String> = rotated(&journal.directory)?
        .into_iter()
        .map(file_name)
        .collect();
    names.push(CURRENT.to_owned());
    for name in names {
        let bytes = match fs::read(journal.directory.join(name)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for record in parse(&bytes).0 {
            if record.kind == "job"
                && SYNCED.contains(&record.to.as_str())
                && let (Some(id), Some(attempt)) = (record.job, record.attempt)
                && wanted.get(&id) == Some(&attempt)
            {
                journaled.insert(id);
            }
        }
    }
    let mut written = 0;
    for job in jobs {
        if wanted.contains_key(&job.id) && !journaled.contains(&job.id) {
            journal.jobs.remove(&job.id);
            journal.observe_job(job);
            written += 1;
        }
    }
    Ok(written)
}

pub fn flush() {
    let file = {
        let mut journal = locked();
        match journal.as_mut() {
            Some(journal) if journal.unsynced => {
                journal.unsynced = false;
                journal.file.try_clone()
            }
            _ => return,
        }
    };
    let result = file.and_then(|file| crate::pacing::data(&file));
    if let Err(error) = result {
        WRITABLE.store(false, Ordering::Relaxed);
        eprintln!("{}: {error}", message("cannot write the event record", &[]));
    }
}

pub fn transitions() -> u64 {
    locked().as_ref().map_or(0, |journal| journal.transitions)
}

pub fn writable() -> bool {
    locked().is_some() && WRITABLE.load(Ordering::Relaxed)
}

pub fn known_state(id: u64) -> Option<State> {
    locked()
        .as_ref()
        .and_then(|journal| journal.jobs.get(&id).map(|(_, state)| state.clone()))
}

pub fn starts_since(since_ms: u64) -> Vec<Start> {
    locked().as_ref().map_or_else(Vec::new, |journal| {
        journal
            .starts
            .iter()
            .filter(|start| start.at_ms >= since_ms)
            .copied()
            .collect()
    })
}

pub fn validate(store: &Store) -> io::Result<()> {
    let directory = directory(store);
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(io::Error::other(message("invalid event journal", &[]))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    let known = rotated(&directory)?;
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let listed = name == CURRENT
            || known
                .iter()
                .any(|first| name.to_str() == Some(file_name(*first).as_str()));
        if !listed || !entry.file_type()?.is_file() {
            return Err(io::Error::other(message("invalid event journal", &[])));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub job: Option<u64>,
    pub queue: Option<String>,
    pub since_ms: Option<u64>,
}

impl Filter {
    pub fn admits(&self, record: &Record) -> bool {
        let within = |path: &Option<String>| {
            let Some(wanted) = &self.queue else {
                return true;
            };
            let wanted = wanted.trim_matches('/');
            path.as_ref().is_some_and(|path| {
                let path = path.trim_matches('/');
                wanted.is_empty()
                    || path == wanted
                    || path
                        .strip_prefix(wanted)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
        };
        self.job.is_none_or(|id| record.job == Some(id))
            && self.since_ms.is_none_or(|since| record.at_ms >= since)
            && (self.queue.is_none() || within(&record.queue_path) || within(&record.object_path))
    }
}

pub struct Reader {
    directory: PathBuf,
    last_seq: u64,
    position: Option<(u64, u64, Option<u64>)>,
    pub unreadable: usize,
    pub missed: u64,
}

fn identity(file: &File) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok((metadata.ino(), metadata.len()))
}

fn leading_seq(bytes: &[u8]) -> Option<u64> {
    let end = bytes.iter().position(|byte| *byte == b'\n')?;
    let (records, _) = parse(&bytes[..=end]);
    records.first().map(|record| record.seq)
}

fn leading_seq_of(file: &mut File) -> io::Result<Option<u64>> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};
    file.seek(SeekFrom::Start(0))?;
    let mut line = Vec::new();
    BufReader::new(&mut *file).read_until(b'\n', &mut line)?;
    Ok(leading_seq(&line))
}

fn whole_lines(bytes: &[u8]) -> &[u8] {
    match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(end) => &bytes[..=end],
        None => &[],
    }
}

impl Reader {
    pub fn new(store: &Store) -> Self {
        Self {
            directory: directory(store),
            last_seq: 0,
            position: None,
            unreadable: 0,
            missed: 0,
        }
    }

    fn take(&mut self, bytes: &[u8], found: &mut Vec<Record>) {
        let (records, unreadable) = parse(bytes);
        self.unreadable += unreadable;
        for record in records {
            if record.seq > self.last_seq {
                if self.last_seq > 0 && record.seq > self.last_seq + 1 {
                    self.missed += record.seq - self.last_seq - 1;
                }
                self.last_seq = record.seq;
                found.push(record);
            }
        }
    }

    fn pass(&mut self, complete: bool, files: &[u64]) -> io::Result<Vec<Record>> {
        let mut found = Vec::new();
        let start = files
            .iter()
            .rposition(|first| *first <= self.last_seq + 1)
            .unwrap_or(0);
        for first in &files[start..] {
            match fs::read(self.directory.join(file_name(*first))) {
                Ok(bytes) => self.take(&bytes, &mut found),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        match File::open(self.directory.join(CURRENT)) {
            Ok(mut file) => {
                use std::io::Read;
                let (inode, _) = identity(&file)?;
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)?;
                let usable = if complete {
                    &bytes[..]
                } else {
                    whole_lines(&bytes)
                };
                self.position = Some((inode, usable.len() as u64, leading_seq(usable)));
                self.take(usable, &mut found);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => self.position = None,
            Err(error) => return Err(error),
        }
        found.sort_by_key(|record| record.seq);
        Ok(found)
    }

    fn all(&mut self, complete: bool) -> io::Result<Vec<Record>> {
        let list = |directory: &Path| match rotated(directory) {
            Ok(files) => Ok(Some(files)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        };
        let (seq, unreadable, missed) = (self.last_seq, self.unreadable, self.missed);
        let mut found = Vec::new();
        for _ in 0..ROTATION_RETRIES {
            let Some(files) = list(&self.directory)? else {
                return Ok(Vec::new());
            };
            self.last_seq = seq;
            self.unreadable = unreadable;
            self.missed = missed;
            found = self.pass(complete, &files)?;
            if list(&self.directory)?.as_ref() == Some(&files) {
                break;
            }
        }
        Ok(found)
    }

    pub fn read(&mut self) -> io::Result<Vec<Record>> {
        self.all(true)
    }

    pub fn more(&mut self) -> io::Result<Vec<Record>> {
        use std::io::{Read, Seek, SeekFrom};
        let Some((inode, offset, leading)) = self.position else {
            return self.all(false);
        };
        let mut file = match File::open(self.directory.join(CURRENT)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let (now_inode, length) = identity(&file)?;
        if now_inode != inode || length < offset || leading_seq_of(&mut file)? != leading {
            return self.all(false);
        }
        if length == offset {
            return Ok(Vec::new());
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let usable = whole_lines(&bytes).to_vec();
        self.position = Some((inode, offset + usable.len() as u64, leading));
        let mut found = Vec::new();
        self.take(&usable, &mut found);
        Ok(found)
    }
}
