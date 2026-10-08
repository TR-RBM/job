use std::collections::{BTreeMap, VecDeque};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Number, Value, json};

use crate::daemon::Shared;
use crate::daemon::client_service;
use crate::model::Backend;

mod args;
mod base64;
pub mod changes;
mod command;
pub mod confirmed;
pub mod index;
pub mod invocation;
mod jobs;
pub mod measure;
mod messages;
mod native;
mod output;
mod processes;
mod record;
mod row;
mod sample;
mod spell;
mod subscribe;
mod table;
mod totals;
pub mod tree;

pub use messages::message;

const VERSION_MIN: u64 = 1;
const VERSION_MAX: u64 = 1;
const LINE_IN: usize = 4_194_304;
const LINE_OUT: usize = 16_777_216;
const CLIENT_NAME: usize = 128;
const CONNECTIONS: usize = 32;
const IDLE: Duration = Duration::from_millis(60_000);
const WRITE: Duration = Duration::from_secs(30);
const TURN: Duration = Duration::from_millis(50);
const WAITING: usize = 64;
const WAITING_BYTES: usize = 2 * LINE_IN;
const SHORT: usize = 4096;
const REQUESTS: &[&str] = &[
    "subscribe",
    "interest",
    "jobs",
    "job",
    "tree",
    "totals",
    "processes",
    "output",
    "commands",
    "command",
    "native",
];

static OPEN: Mutex<BTreeMap<u32, usize>> = Mutex::new(BTreeMap::new());

struct Slot(u32);

impl Slot {
    fn take(uid: u32) -> Option<Self> {
        let mut open = OPEN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = open.entry(uid).or_default();
        if *count >= CONNECTIONS {
            return None;
        }
        *count += 1;
        Some(Self(uid))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut open = OPEN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = open.get_mut(&self.0) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                open.remove(&self.0);
            }
        }
    }
}

enum Read {
    Line(Vec<u8>),
    Long,
    End,
}

enum Step {
    Answer(String),
    Served,
    Close(String),
}

pub fn greeted(line: &[u8]) -> bool {
    let mut bytes = line.iter().filter(|byte| !byte.is_ascii_whitespace());
    if bytes.next() != Some(&b'{') || !bytes.take(7).eq(b"\"hello\"") {
        return false;
    }
    matches!(
        serde_json::from_slice::<Value>(line),
        Ok(Value::Object(members)) if members.len() == 1 && members.contains_key("hello")
    )
}

fn failure(text: &str) -> String {
    json!({"error": {"message": text}}).to_string()
}

pub(super) fn send(writer: &mut UnixStream, line: &str) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    writer.write_all(&bytes)
}

fn too_long() -> String {
    failure(
        &message("the line is longer than {limit} bytes; the connection is closed")
            .replace("{limit}", &LINE_IN.to_string()),
    )
}

fn range(hello: &Value) -> Option<(u64, u64)> {
    let versions = hello.get("versions")?;
    let min = versions.get("min")?.as_u64()?;
    let max = versions.get("max")?.as_u64()?;
    (min >= 1 && min <= max).then_some((min, max))
}

fn limits() -> Value {
    json!({
        "line_bytes_in": LINE_IN,
        "line_bytes_out": LINE_OUT,
        "page_rows": jobs::PAGE,
        "page_rows_default": jobs::PAGE_DEFAULT,
        "interest_jobs": subscribe::INTEREST,
        "connections": CONNECTIONS,
        "subscriptions": sample::SUBSCRIPTIONS,
        "retained_changes": changes::KEPT,
        "heartbeat_ms": subscribe::HEARTBEAT.as_millis() as u64,
        "live_interval_ms": sample::INTERVAL.as_millis() as u64,
        "output_piece_bytes": output::PIECE,
        "processes": processes::LIMIT,
        "idle_ms": IDLE.as_millis() as u64,
        "indexed_ended_jobs": index::bound(),
        "ids": jobs::IDS,
        "poll_ms": 5000,
    })
}

fn greeting(shared: &Shared, version: u64, peer: crate::durability::peer::Peer) -> String {
    let facts = client_service::facts(shared);
    let own = unsafe { libc::geteuid() };
    let service = facts.service.as_ref();
    let mut capabilities: Vec<&str> = REQUESTS.to_vec();
    if facts.backend == Backend::Cgroup {
        capabilities.push("cgroup");
    }
    if service.is_some_and(|service| service.socket_group.is_some()) {
        capabilities.push("socket_group");
    }
    json!({"hello": {
        "version": version,
        "versions": {"min": VERSION_MIN, "max": VERSION_MAX},
        "job_version": crate::daemon::version(),
        "native_protocol": crate::model::PROTOCOL,
        "capabilities": capabilities,
        "now_ms": crate::shim::now_ms(),
        "started_ms": crate::operations::health::start(),
        "hostname": crate::host::hostname(),
        "service": {
            "mode": service.map_or("user", |service| service.mode.as_str()),
            "uid": service.map_or(own, |service| service.uid),
            "socket_rule": service.map_or("", |service| service.socket_rule.as_str()),
            "socket_group": service.and_then(|service| service.socket_group.clone()),
        },
        "backend": match facts.backend {
            Backend::Cgroup => "cgroup",
            Backend::Watch => "watch",
        },
        "enforcement": crate::operations::health::enforcement(facts.backend),
        "peer": {
            "uid": peer.uid,
            "pid": peer.pid,
            "via": if peer.uid == own { "owner" } else { "group" },
            "may": {"read": true, "change": true, "events": false, "audit": false},
        },
        "limits": limits(),
    }})
    .to_string()
}

pub struct Lines {
    reader: BufReader<UnixStream>,
    partial: Vec<u8>,
    waiting: VecDeque<Read>,
    held: usize,
    ended: bool,
}

impl Lines {
    fn taken(&mut self, wait: Duration) -> Option<Read> {
        if self.ended {
            return Some(Read::End);
        }
        let wait = wait.max(Duration::from_millis(1));
        if self.reader.get_ref().set_read_timeout(Some(wait)).is_err() {
            self.ended = true;
            return Some(Read::End);
        }
        let buffer = match self.reader.fill_buf() {
            Ok(buffer) => buffer,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                ) =>
            {
                return None;
            }
            Err(_) => {
                self.ended = true;
                return Some(Read::End);
            }
        };
        if buffer.is_empty() {
            self.ended = true;
            return Some(Read::End);
        }
        match buffer.iter().position(|byte| *byte == b'\n') {
            Some(at) => {
                self.partial.extend_from_slice(&buffer[..at]);
                self.reader.consume(at + 1);
                let line = std::mem::take(&mut self.partial);
                if line.len() >= LINE_IN {
                    self.ended = true;
                    Some(Read::Long)
                } else {
                    Some(Read::Line(line))
                }
            }
            None => {
                let taken = buffer.len();
                self.partial.extend_from_slice(buffer);
                self.reader.consume(taken);
                if self.partial.len() >= LINE_IN {
                    self.ended = true;
                    return Some(Read::Long);
                }
                None
            }
        }
    }

    fn kept(&mut self) -> Option<Read> {
        let read = self.waiting.pop_front()?;
        if let Read::Line(line) = &read {
            self.held = self.held.saturating_sub(line.len());
        }
        Some(read)
    }

    fn glance(&mut self, wait: Duration) -> Option<Read> {
        self.kept().or_else(|| self.taken(wait))
    }

    fn next(&mut self) -> Read {
        if let Some(read) = self.kept() {
            return read;
        }
        let deadline = Instant::now() + IDLE;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Read::End;
            }
            if let Some(read) = self.taken(remaining) {
                return read;
            }
        }
    }

    pub fn poll(&mut self, wait: Duration) {
        let deadline = Instant::now() + wait;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if self.ended || self.waiting.len() >= WAITING || self.held >= WAITING_BYTES {
                std::thread::sleep(remaining);
                return;
            }
            match self.taken(remaining) {
                Some(read) => {
                    if let Read::Line(line) = &read {
                        self.held += line.len();
                    }
                    self.waiting.push_back(read);
                }
                None if Instant::now() >= deadline => return,
                None => {}
            }
        }
    }

    pub fn stop(&mut self, of: &Number) -> Option<Number> {
        let (at, request) = self.waiting.iter().enumerate().find_map(|(at, read)| {
            let Read::Line(line) = read else {
                return None;
            };
            if line.len() > SHORT {
                return None;
            }
            let request: Value = serde_json::from_slice(line).ok()?;
            let named = request.get("args")?.get("of")?.as_number()?;
            match (request.get("op")?.as_str()?, request.get("id")?) {
                ("end", Value::Number(id)) if named == of => Some((at, id.clone())),
                _ => None,
            }
        })?;
        if let Some(Read::Line(line)) = self.waiting.remove(at) {
            self.held = self.held.saturating_sub(line.len());
        }
        Some(request)
    }

    pub fn closed(&self) -> bool {
        self.ended
            && !self
                .waiting
                .iter()
                .any(|read| matches!(read, Read::Line(_)))
    }
}

fn unreadable(detail: &str) -> Step {
    Step::Close(failure(
        &message(
            "the line is not a request: {detail}; write one JSON object with id and op on one line",
        )
        .replace("{detail}", detail),
    ))
}

fn stopped(args: &Map<String, Value>) -> Result<String, String> {
    match args.get("of") {
        Some(Value::Number(_)) => Ok(json!({"stopped": false}).to_string()),
        _ => Err(message(
            "end needs `of`: the id of the output request to stop",
        )),
    }
}

struct Talk {
    lines: Lines,
    writer: UnixStream,
    uid: u32,
    subscription: Option<subscribe::Subscription>,
}

fn answered(shared: &Arc<Shared>, line: &[u8], talk: &mut Talk) -> io::Result<Step> {
    if std::str::from_utf8(line).is_err() {
        return Ok(unreadable(&message("it is not valid UTF-8")));
    }
    let Ok(Value::Object(request)) = serde_json::from_slice::<Value>(line) else {
        return Ok(unreadable(&message("it is not a JSON object")));
    };
    let Some(Value::String(op)) = request.get("op") else {
        return Ok(unreadable(&message("op is missing or not a string")));
    };
    let Some(Value::Number(id)) = request.get("id") else {
        return Ok(unreadable(&message("id is missing or not a number")));
    };
    let refused = |text: String| {
        Step::Answer(format!(
            "{{\"re\":{id},\"error\":{}}}",
            json!({"message": text})
        ))
    };
    let empty = Map::new();
    let args = match request.get("args") {
        None => &empty,
        Some(Value::Object(args)) => args,
        Some(_) => return Ok(refused(message("args is not an object"))),
    };
    let result = match op.as_str() {
        "output" if talk.subscription.is_some() => Err(message(
            "output is not accepted on a connection that has a subscription; use a second connection",
        )),
        "output" => {
            output::serve(shared, id, args, &mut talk.lines, &mut talk.writer)?;
            return Ok(Step::Served);
        }
        "subscribe" if talk.subscription.is_some() => Err(message(
            "this connection has a subscription already; a connection carries one",
        )),
        "subscribe" => subscribe::subscribe(shared, talk.uid, args).map(|(answer, started)| {
            talk.subscription = started;
            answer
        }),
        "interest" => match &mut talk.subscription {
            Some(subscription) => subscription.interest(args),
            None => Err(message(
                "interest needs a subscription on the same connection; send subscribe first",
            )),
        },
        "end" => stopped(args),
        "jobs" => jobs::answer(shared, args),
        "job" => record::answer(shared, args),
        "tree" => tree::answer(shared),
        "processes" => processes::answer(shared, args),
        "totals" => Ok(totals::answer(shared)),
        "commands" => table::answer(args).map(|answer| answer.to_string()),
        "command" => command::answer(shared, args),
        "native" => native::answer(shared, args),
        unknown => Err(message(
            "this service does not know the request `{op}`; the capabilities in the greeting list what it answers",
        )
        .replace("{op}", &unknown.chars().take(64).collect::<String>())),
    };
    Ok(match result {
        Ok(answer) if answer.len() + id.to_string().len() + 16 > LINE_OUT => refused(
            message("the answer would be longer than {limit} bytes; ask for less")
                .replace("{limit}", &LINE_OUT.to_string()),
        ),
        Ok(answer) => Step::Answer(format!("{{\"re\":{id},\"ok\":{answer}}}")),
        Err(text) => refused(text),
    })
}

fn welcome(shared: &Shared, line: &[u8]) -> Result<(String, Slot, u32), String> {
    if line.len() > LINE_IN {
        return Err(too_long());
    }
    let unread = || {
        failure(&message(
            "the greeting is not readable: write {\"hello\":{\"versions\":{\"min\":N,\"max\":N}}} with whole numbers from 1 and min not above max",
        ))
    };
    let value: Value = serde_json::from_slice(line).map_err(|_| unread())?;
    let hello = value.get("hello").ok_or_else(unread)?;
    let (min, max) = range(hello).ok_or_else(unread)?;
    let client = match hello.get("client") {
        None => None,
        Some(Value::String(name)) if name.len() <= CLIENT_NAME => Some(name.clone()),
        Some(Value::String(_)) => {
            return Err(failure(&message(
                "the name of the client in the greeting is longer than 128 bytes",
            )));
        }
        Some(_) => {
            return Err(failure(&message(
                "the name of the client in the greeting is not a string",
            )));
        }
    };
    let Some(peer) = crate::durability::peer::current() else {
        return Err(failure(&crate::service::access::shared().refusal()));
    };
    let slot = Slot::take(peer.uid).ok_or_else(|| {
        failure(
            &message(
                "this user already has {limit} connections of the client interface open; close one and connect again",
            )
            .replace("{limit}", &CONNECTIONS.to_string()),
        )
    })?;
    if max < VERSION_MIN || min > VERSION_MAX {
        return Err(json!({"unsupported": {
            "versions": {"min": VERSION_MIN, "max": VERSION_MAX},
            "job_version": crate::daemon::version(),
        }})
        .to_string());
    }
    let version = max.min(VERSION_MAX);
    if let Some(client) = client {
        let shown: String = client
            .chars()
            .map(|character| {
                if character.is_control() {
                    '?'
                } else {
                    character
                }
            })
            .collect();
        eprintln!(
            "job daemon: {}",
            message(
                "client interface: {client} connected as user {uid}, process {pid}, version {version}"
            )
            .replace("{client}", &shown)
            .replace("{uid}", &peer.uid.to_string())
            .replace("{pid}", &peer.pid.to_string())
            .replace("{version}", &version.to_string())
        );
    }
    Ok((greeting(shared, version, peer), slot, peer.uid))
}

pub fn serve(
    shared: &Arc<Shared>,
    line: &[u8],
    reader: BufReader<UnixStream>,
    mut writer: UnixStream,
    admitted: bool,
) -> io::Result<()> {
    writer.set_write_timeout(Some(WRITE))?;
    if !admitted {
        let text = crate::service::access::shared().refusal();
        crate::durability::audit::refused(crate::durability::peer::of(&writer), &text);
        let sent = send(&mut writer, &failure(&text));
        crate::durability::audit::flush();
        return sent;
    }
    let (_slot, uid) = match welcome(shared, line) {
        Ok((greeting, slot, uid)) => {
            send(&mut writer, &greeting)?;
            (slot, uid)
        }
        Err(refusal) => return send(&mut writer, &refusal),
    };
    let mut talk = Talk {
        lines: Lines {
            reader,
            partial: Vec::new(),
            waiting: VecDeque::new(),
            held: 0,
            ended: false,
        },
        writer,
        uid,
        subscription: None,
    };
    loop {
        let read = match &mut talk.subscription {
            Some(subscription) => {
                if !subscription.pump(&mut talk.writer)? {
                    talk.subscription = None;
                    continue;
                }
                match talk.lines.glance(TURN) {
                    Some(read) => read,
                    None => continue,
                }
            }
            None => talk.lines.next(),
        };
        match read {
            Read::End => return Ok(()),
            Read::Long => return send(&mut talk.writer, &too_long()),
            Read::Line(line) => match answered(shared, &line, &mut talk)? {
                Step::Answer(answer) => send(&mut talk.writer, &answer)?,
                Step::Served => {}
                Step::Close(refusal) => return send(&mut talk.writer, &refusal),
            },
        }
    }
}
