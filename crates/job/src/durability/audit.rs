use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::model::{Request, Response};
use crate::store::Store;

pub const SCHEMA: u32 = 1;
const PARAMS_LIMIT: usize = 4096;
const RESULT_LIMIT: usize = 512;
const CURRENT: &str = "current.jsonl";
const SESSION_LIMIT: usize = 128;
const TARGET_LIMIT: usize = 256;
const LINE_LIMIT: usize = 8192;
const SAMPLE_BYTES: usize = 64;
const EVENT_LIMIT: u32 = 30;
const EVENT_WINDOW_MS: u64 = 60_000;
const BEGIN: &str = "begin";
const END: &str = "end";
const UNKNOWN: &str = "outcome unknown";

fn unset(value: &bool) -> bool {
    !*value
}

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
            return Err(super::message(
                "audit keep_files and rotate_bytes must be at least 1",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub seq: u64,
    pub at_ms: u64,
    pub action: String,
    pub target: Option<String>,
    pub peer_uid: Option<u32>,
    pub peer_pid: Option<i32>,
    pub session: Option<String>,
    pub params: Value,
    pub result: String,
    pub operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "unset")]
    pub truncated: bool,
}

pub struct Pending {
    action: &'static str,
    target: Option<String>,
    session: Option<String>,
    params: Value,
    truncated: bool,
    mutating: bool,
}

pub struct Begun {
    seq: Option<u64>,
    pending: Pending,
}

struct Journal {
    directory: PathBuf,
    file: File,
    size: u64,
    next_seq: u64,
    first_seq: Option<u64>,
    settings: Settings,
    torn: bool,
    window_ms: u64,
    events: u32,
    suppressed: u64,
    lines: u64,
    deferred: bool,
}

static GROUP: crate::pacing::Group = crate::pacing::Group::new();
static JOURNAL: Mutex<Option<Journal>> = Mutex::new(None);
static WRITABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn writable() -> bool {
    JOURNAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some()
        && WRITABLE.load(std::sync::atomic::Ordering::Relaxed)
}

fn directory(store: &Store) -> PathBuf {
    store.root.join("audit")
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

fn rotated(directory: &Path) -> io::Result<Vec<u64>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)? {
        let name = entry?.file_name();
        if let Some(first) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".jsonl"))
            .and_then(|stem| stem.parse::<u64>().ok())
            .filter(|first| name.to_str() == Some(format!("{first}.jsonl").as_str()))
        {
            files.push(first);
        }
    }
    files.sort_unstable();
    Ok(files)
}

fn entries(bytes: &[u8]) -> (Vec<Entry>, usize) {
    let mut found = Vec::new();
    let mut unreadable = 0;
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<Entry>(line) {
            Ok(entry) => found.push(entry),
            Err(_) => unreadable += 1,
        }
    }
    (found, unreadable)
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
            return Err(io::Error::other(super::message("invalid audit journal")));
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
        let (found, _) = entries(&existing);
        let mut last = found.iter().map(|entry| entry.seq).max();
        if last.is_none()
            && let Some(first) = rotated(&directory)?.last().copied()
        {
            let (older, _) = entries(&fs::read(directory.join(format!("{first}.jsonl")))?);
            last = Some(older.iter().map(|entry| entry.seq).max().unwrap_or(first));
        }
        Ok(Self {
            directory,
            file,
            size,
            next_seq: last.map_or(1, |seq| seq + 1),
            first_seq: found.first().map(|entry| entry.seq),
            settings,
            torn: false,
            window_ms: 0,
            events: 0,
            suppressed: 0,
            lines: 0,
            deferred: false,
        })
    }

    fn put(&mut self, line: &[u8]) -> io::Result<()> {
        if self.torn {
            self.file.write_all(b"\n")?;
            self.size += 1;
            self.torn = false;
        }
        if super::failpoint::fails("audit-partial-write").is_err() {
            self.file.write_all(&line[..line.len() / 2])?;
            return Err(io::Error::other(super::message(
                "injected failure at failpoint",
            )));
        }
        self.file.write_all(line)?;
        self.lines += 1;
        if self.deferred {
            return Ok(());
        }
        crate::pacing::data(&self.file)?;
        GROUP.covered(self.lines);
        Ok(())
    }

    fn write(&mut self, entry: &Entry) -> io::Result<()> {
        let mut line = serde_json::to_vec(entry)?;
        if line.len() + 1 > LINE_LIMIT {
            let mut short = entry.clone();
            short.params = json!({"truncated": true, "bytes": line.len()});
            short.result = clip(&short.result, RESULT_LIMIT / 2).0;
            short.truncated = true;
            line = serde_json::to_vec(&short)?;
        }
        line.push(b'\n');
        if let Err(error) = self.put(&line) {
            self.torn = true;
            return Err(error);
        }
        self.size += line.len() as u64;
        self.first_seq.get_or_insert(entry.seq);
        self.next_seq = self.next_seq.max(entry.seq + 1);
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        let first = self.first_seq.unwrap_or(self.next_seq);
        let closed = format!("{first}.jsonl");
        crate::pacing::data(&self.file)?;
        GROUP.covered(self.lines);
        fs::rename(self.directory.join(CURRENT), self.directory.join(&closed))?;
        let mut files = rotated(&self.directory)?;
        let mut dropped = Vec::new();
        while files.len() + 1 > self.settings.keep_files as usize && !files.is_empty() {
            let oldest = files.remove(0);
            fs::remove_file(self.directory.join(format!("{oldest}.jsonl")))?;
            dropped.push(oldest);
        }
        self.file = append_only(&self.directory.join(CURRENT))?;
        self.size = 0;
        self.first_seq = None;
        sync(&self.directory)?;
        let remaining = files.first().copied().unwrap_or(self.next_seq);
        let entry = Entry {
            seq: self.next_seq,
            at_ms: crate::shim::now_ms(),
            action: "audit-rotation".to_owned(),
            target: Some(closed),
            peer_uid: None,
            peer_pid: None,
            session: None,
            params: json!({
                "dropped_files": dropped.iter().map(|first| format!("{first}.jsonl")).collect::<Vec<_>>(),
                "dropped_first_seq": dropped.first(),
                "dropped_last_seq": dropped.first().map(|_| remaining.saturating_sub(1)),
                "keep_files": self.settings.keep_files,
                "rotate_bytes": self.settings.rotate_bytes,
            }),
            result: "ok".to_owned(),
            operation: None,
            phase: None,
            truncated: false,
        };
        self.write(&entry)
    }

    fn later(&mut self, entry: Entry, seq: Option<u64>) -> io::Result<u64> {
        self.deferred = true;
        let written = self.append(entry, seq);
        self.deferred = false;
        written
    }

    fn append(&mut self, mut entry: Entry, seq: Option<u64>) -> io::Result<u64> {
        entry.seq = seq.unwrap_or(self.next_seq);
        let length = serde_json::to_vec(&entry)?.len() as u64 + 1;
        if self.size > 0 && self.size + length > self.settings.rotate_bytes {
            self.rotate()?;
            entry.seq = seq.unwrap_or(self.next_seq);
        }
        let written = self.write(&entry);
        WRITABLE.store(written.is_ok(), std::sync::atomic::Ordering::Relaxed);
        written?;
        Ok(entry.seq)
    }

    fn limited(&mut self, entry: Entry) -> io::Result<()> {
        let now = entry.at_ms;
        if now.saturating_sub(self.window_ms) >= EVENT_WINDOW_MS {
            let dropped = std::mem::take(&mut self.suppressed);
            self.window_ms = now;
            self.events = 0;
            if dropped > 0 {
                self.append(suppression(now, Some(dropped)), None)?;
            }
        }
        if self.events < EVENT_LIMIT {
            self.events += 1;
            return self.append(entry, None).map(|_| ());
        }
        self.suppressed += 1;
        if self.suppressed == 1 {
            self.append(suppression(now, None), None)?;
        }
        Ok(())
    }
}

fn suppression(now: u64, dropped: Option<u64>) -> Entry {
    Entry {
        seq: 0,
        at_ms: now,
        action: "audit-suppressed".to_owned(),
        target: None,
        peer_uid: None,
        peer_pid: None,
        session: None,
        params: json!({
            "limit": EVENT_LIMIT,
            "window_ms": EVENT_WINDOW_MS,
            "state": if dropped.is_some() { "ended" } else { "started" },
            "suppressed": dropped,
        }),
        result: "ok".to_owned(),
        operation: None,
        phase: None,
        truncated: false,
    }
}

fn clip(text: &str, limit: usize) -> (String, bool) {
    if text.len() <= limit {
        return (text.to_owned(), false);
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), true)
}

pub fn open(store: &Store, settings: &Settings) -> io::Result<()> {
    let journal = Journal::open(store, settings.clone())?;
    *JOURNAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(journal);
    Ok(())
}

pub fn configure(settings: &Settings) {
    if let Some(journal) = JOURNAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_mut()
    {
        journal.settings = settings.clone();
    }
}

fn command(argv: &[Value]) -> Value {
    json!({
        "program": argv.first().and_then(Value::as_str).map(|program| clip(program, TARGET_LIMIT).0),
        "count": argv.len(),
        "sha256": format!("{:x}", Sha256::digest(Value::Array(argv.to_vec()).to_string())),
    })
}

fn names(value: &Value) -> Value {
    let pairs = match value {
        Value::Null => return Value::Null,
        Value::Object(map) => map.get("vars").and_then(Value::as_array),
        Value::Array(items) => Some(items),
        _ => None,
    };
    match pairs {
        Some(pairs) => json!({
            "names": pairs
                .iter()
                .filter_map(|pair| pair.get(0).and_then(Value::as_str))
                .collect::<Vec<_>>()
        }),
        None => json!("redacted"),
    }
}

fn scrub(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map.iter_mut() {
                if key == "env" {
                    *inner = names(inner);
                } else if let ("argv", Value::Array(argv)) = (key.as_str(), &*inner) {
                    *inner = command(argv);
                } else {
                    scrub(inner);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(scrub),
        Value::String(text) if text.contains("://") => {
            *text = crate::netsecret::redact(text);
        }
        _ => {}
    }
}

fn bounded(mut params: Value) -> Value {
    scrub(&mut params);
    let text = params.to_string();
    if text.len() <= PARAMS_LIMIT {
        return params;
    }
    let mut end = PARAMS_LIMIT - 256;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    json!({"truncated": true, "bytes": text.len(), "prefix": &text[..end]})
}

fn inner(request: &Request) -> Value {
    match serde_json::to_value(request) {
        Ok(Value::Object(map)) => map
            .into_iter()
            .next()
            .map_or(Value::Null, |(_, value)| value),
        _ => Value::Null,
    }
}

fn object_action(operation: &crate::objects::Operation) -> Option<(&'static str, &str)> {
    use crate::objects::Operation;
    Some(match operation {
        Operation::List | Operation::Show { .. } => return None,
        Operation::Create { path, .. } => ("object-create", path),
        Operation::Set { path, .. } => ("object-set", path),
        Operation::Unset { path, .. } => ("object-unset", path),
        Operation::Rename { path, .. } => ("object-rename", path),
        Operation::Move { path, .. } => ("object-move", path),
        Operation::Pause { path, paused: true } => ("object-pause", path),
        Operation::Pause { path, .. } => ("object-resume", path),
        Operation::Close { path, closed: true } => ("object-close", path),
        Operation::Close { path, .. } => ("object-open", path),
        Operation::Remove { path } => ("object-remove", path),
    })
}

pub fn describe(request: &Request) -> Option<Pending> {
    let job = |id: &u64| Some(id.to_string());
    let (action, target, session) = match request {
        Request::Versioned { request, .. } => return describe(request),
        Request::Attach { id } => ("attach", job(id), None),
        Request::Submit { spec, .. } => ("submit", None, Some(spec.session.clone())),
        Request::Create { spec, .. } => ("create", None, Some(spec.session.clone())),
        Request::Edit { id, spec, .. } => ("edit", job(id), Some(spec.session.clone())),
        Request::Release { id } => ("release", job(id), None),
        Request::Retry { id, .. } => ("retry", job(id), None),
        Request::Cancel { id, session } => ("cancel", job(id), Some(session.clone())),
        Request::CancelSelection {
            target,
            expected: Some(_),
            ..
        } => (
            "cancel",
            Some(match target {
                crate::cancellation::Target::Job { id } => id.to_string(),
                crate::cancellation::Target::Object { path, .. } => path.clone(),
            }),
            None,
        ),
        Request::Remove {
            target,
            expected: Some(_),
            ..
        } => (
            "remove",
            Some(match target {
                crate::removal::Target::Job { id } => id.to_string(),
                crate::removal::Target::Object { path, .. } => path.clone(),
            }),
            None,
        ),
        Request::Freeze { target, frozen, .. } => (
            if *frozen { "suspend" } else { "continue" },
            Some(match target {
                crate::freezer::Target::Job { id } => id.to_string(),
                crate::freezer::Target::Object { path, .. } => path.clone(),
            }),
            None,
        ),
        Request::Signal { id, .. } => ("signal", job(id), None),
        Request::Reprioritize { id, .. } => ("reprioritize", job(id), None),
        Request::ResourceUpdate {
            target,
            expected: Some(_),
            ..
        } => (
            "update",
            Some(match target {
                crate::resource_update::Target::Job { id } => id.to_string(),
                crate::resource_update::Target::Object { path, .. } => path.clone(),
            }),
            None,
        ),
        Request::ResourceUpdateStatus {
            operation,
            abandon: true,
        } => ("update-abandon", Some(operation.clone()), None),
        Request::Config { reload: true } => ("config-reload", None, None),
        Request::Object { operation, .. } => {
            let (action, path) = object_action(operation)?;
            (action, Some(path.to_owned()), None)
        }
        Request::QueueAdd { name, .. } => ("queue-add", Some(name.clone()), None),
        Request::QueueSet { name, .. } => ("queue-set", Some(name.clone()), None),
        Request::QueueRemove { name, .. } => ("queue-remove", Some(name.clone()), None),
        Request::QueueClear { name } => ("queue-clear", Some(name.clone()), None),
        Request::Extended { call } => {
            let (action, target) = call.audited()?;
            (action, Some(target), None)
        }
        _ => return None,
    };
    let target = target.map(|target| clip(&crate::netsecret::redact(&target), TARGET_LIMIT));
    let session = session.map(|session| clip(&crate::netsecret::redact(&session), SESSION_LIMIT));
    Some(Pending {
        action,
        truncated: target.as_ref().is_some_and(|(_, cut)| *cut)
            || session.as_ref().is_some_and(|(_, cut)| *cut),
        target: target.map(|(target, _)| target),
        session: session.map(|(session, _)| session),
        params: bounded(inner(request)),
        mutating: !matches!(request, Request::Attach { .. }),
    })
}

pub fn mutating(request: &Request) -> bool {
    describe(request).is_some_and(|pending| pending.mutating)
}

fn outcome(response: &Response) -> (String, Option<String>, Option<String>) {
    let ok = || "ok".to_owned();
    match response {
        Response::Error { message } => {
            let mut text = format!("error: {}", crate::netsecret::redact(message));
            if text.len() > RESULT_LIMIT {
                let mut end = RESULT_LIMIT;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
            }
            (text, None, None)
        }
        Response::Submitted { job } => (
            if job.durability.replayed {
                "replayed".to_owned()
            } else {
                ok()
            },
            None,
            Some(job.id.to_string()),
        ),
        Response::RetryPending => ("pending".to_owned(), None, None),
        Response::Cancellation { operation } => (
            if operation.complete {
                ok()
            } else {
                "pending".to_owned()
            },
            Some(operation.operation.clone()),
            None,
        ),
        Response::Removed { receipt } => (ok(), Some(receipt.operation.clone()), None),
        Response::RemovalInProgress { receipt, .. } => {
            ("pending".to_owned(), Some(receipt.operation.clone()), None)
        }
        Response::RemovalPreview { .. } => ("refused: not ready".to_owned(), None, None),
        Response::ResourceUpdate { operation } => {
            (ok(), Some(operation.plan.operation.clone()), None)
        }
        Response::Controlled { results, .. } => (
            if results
                .iter()
                .all(|result| result.accepted && result.error.is_none())
            {
                ok()
            } else {
                "error: not every member was controlled".to_owned()
            },
            None,
            None,
        ),
        Response::Signalled { delivered } => (format!("ok: delivered {delivered}"), None, None),
        Response::Unsupported { .. } => ("error: unsupported protocol".to_owned(), None, None),
        Response::Extended { answer } => match &**answer {
            crate::cli2::Answer::Set { report } => {
                let (result, operation) = report.audited();
                (result, operation, None)
            }
            _ => (ok(), None, None),
        },
        _ => (ok(), None, None),
    }
}

fn journal() -> std::sync::MutexGuard<'static, Option<Journal>> {
    JOURNAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn complain(error: &io::Error) {
    eprintln!(
        "{}: {error}",
        super::message("cannot write the audit record")
    );
}

fn entry(pending: &Pending) -> Entry {
    let peer = super::peer::current();
    Entry {
        seq: 0,
        at_ms: crate::shim::now_ms(),
        action: pending.action.to_owned(),
        target: pending.target.clone(),
        peer_uid: peer.map(|peer| peer.uid),
        peer_pid: peer.map(|peer| peer.pid),
        session: pending.session.clone(),
        params: pending.params.clone(),
        result: BEGIN.to_owned(),
        operation: None,
        phase: None,
        truncated: pending.truncated,
    }
}

fn finished(pending: &Pending, response: &Response) -> Entry {
    let (result, operation, target) = outcome(response);
    let mut entry = entry(pending);
    if let Some(target) = target {
        entry.target = Some(target);
    }
    entry.result = result;
    entry.operation = operation;
    entry
}

pub fn begin(pending: Option<Pending>) -> io::Result<Option<Begun>> {
    let Some(pending) = pending else {
        return Ok(None);
    };
    if !pending.mutating {
        return Ok(Some(Begun { seq: None, pending }));
    }
    let (seq, position) = {
        let mut journal = journal();
        let Some(journal) = journal.as_mut() else {
            return Ok(Some(Begun { seq: None, pending }));
        };
        let mut intent = entry(&pending);
        intent.phase = Some(BEGIN.to_owned());
        let seq = journal.later(intent, None).inspect_err(complain)?;
        (seq, journal.lines)
    };
    settle(position).inspect_err(complain)?;
    crate::pacing::charge(1);
    Ok(Some(Begun {
        seq: Some(seq),
        pending,
    }))
}

pub fn action(begun: &Option<Begun>) -> Option<&'static str> {
    begun
        .as_ref()
        .filter(|begun| begun.seq.is_some())
        .map(|begun| begun.pending.action)
}

pub fn end(begun: Option<Begun>, response: &Response) {
    let Some(begun) = begun else {
        return;
    };
    let mut entry = finished(&begun.pending, response);
    entry.phase = begun.seq.map(|_| END.to_owned());
    if let Some(journal) = journal().as_mut()
        && let Err(error) = journal.later(entry, begun.seq)
    {
        complain(&error);
    }
}

fn settle(position: u64) -> io::Result<()> {
    let result = GROUP.commit(
        position,
        || journal().as_ref().map_or(0, |journal| journal.lines),
        || {
            let file = match journal().as_ref() {
                Some(journal) => journal.file.try_clone()?,
                None => return Ok(()),
            };
            crate::pacing::data_shared(&file)
        },
    );
    if result.is_err() {
        WRITABLE.store(false, std::sync::atomic::Ordering::Relaxed);
    }
    result
}

pub fn flush() {
    let position = journal().as_ref().map_or(0, |journal| journal.lines);
    if let Err(error) = settle(position) {
        complain(&error);
    }
}

pub fn record(pending: Option<Pending>, response: &Response) {
    let Some(pending) = pending else {
        return;
    };
    let entry = finished(&pending, response);
    if let Some(journal) = journal().as_mut()
        && let Err(error) = journal.append(entry, None)
    {
        complain(&error);
    }
}

fn event(action: &str, peer: Option<super::peer::Peer>, target: Option<&str>, params: Value) {
    let target = target.map(|target| clip(target, TARGET_LIMIT));
    let entry = Entry {
        seq: 0,
        at_ms: crate::shim::now_ms(),
        action: action.to_owned(),
        truncated: target.as_ref().is_some_and(|(_, cut)| *cut),
        target: target.map(|(target, _)| target),
        peer_uid: peer.map(|peer| peer.uid),
        peer_pid: peer.map(|peer| peer.pid),
        session: None,
        params,
        result: "refused".to_owned(),
        operation: None,
        phase: None,
    };
    if let Some(journal) = journal().as_mut()
        && let Err(error) = journal.limited(entry)
    {
        complain(&error);
    }
}

pub fn refused(peer: Option<super::peer::Peer>, reason: &str) {
    event(
        "connection-refused",
        peer,
        None,
        json!({"reason": clip(reason, RESULT_LIMIT).0}),
    );
}

pub fn unreadable(line: &[u8]) {
    let sample: String = line
        .iter()
        .take(SAMPLE_BYTES)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    event(
        "unreadable-request",
        super::peer::current(),
        None,
        json!({"bytes": line.len(), "first_bytes_hex": sample}),
    );
}

pub fn unknown(name: &str) {
    event(
        "unknown-request",
        super::peer::current(),
        Some(name),
        Value::Null,
    );
}

pub fn validate(store: &Store) -> io::Result<()> {
    let directory = directory(store);
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(io::Error::other(super::message("invalid audit journal"))),
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
                .any(|first| name.to_str() == Some(format!("{first}.jsonl").as_str()));
        if !listed || !entry.file_type()?.is_file() {
            return Err(io::Error::other(super::message("invalid audit journal")));
        }
    }
    Ok(())
}

pub struct Filter {
    pub target: Option<String>,
    pub action: Option<String>,
    pub since_ms: Option<u64>,
}

pub fn read(store: &Store, filter: &Filter) -> io::Result<(Vec<Entry>, usize)> {
    let directory = directory(store);
    let mut files: Vec<PathBuf> = match rotated(&directory) {
        Ok(files) => files
            .into_iter()
            .map(|first| directory.join(format!("{first}.jsonl")))
            .collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
        Err(error) => return Err(error),
    };
    files.push(directory.join(CURRENT));
    let mut found = Vec::new();
    let mut unreadable = 0;
    for path in files {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let (entries, skipped) = entries(&bytes);
        unreadable += skipped;
        found.extend(entries);
    }
    Ok((paired(found, filter), unreadable))
}

fn paired(found: Vec<Entry>, filter: &Filter) -> Vec<Entry> {
    let ended: std::collections::BTreeSet<u64> = found
        .iter()
        .filter(|entry| entry.phase.as_deref() == Some(END))
        .map(|entry| entry.seq)
        .collect();
    let mut shown: Vec<Entry> = found
        .into_iter()
        .filter_map(|mut entry| match entry.phase.as_deref() {
            Some(BEGIN) if ended.contains(&entry.seq) => None,
            Some(BEGIN) => {
                entry.result = UNKNOWN.to_owned();
                Some(entry)
            }
            _ => {
                entry.phase = None;
                Some(entry)
            }
        })
        .filter(|entry| {
            filter
                .target
                .as_ref()
                .is_none_or(|target| entry.target.as_ref() == Some(target))
                && filter
                    .action
                    .as_ref()
                    .is_none_or(|action| &entry.action == action)
                && filter.since_ms.is_none_or(|since| entry.at_ms >= since)
        })
        .collect();
    shown.sort_by_key(|entry| entry.seq);
    shown
}
