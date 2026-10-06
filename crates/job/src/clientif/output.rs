use std::collections::VecDeque;
use std::io;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use serde_json::{Number, Value, json};

use crate::daemon::Shared;
use crate::streams::reader::{Batch, Kept, Reader};
use crate::streams::{CHUNK, Mode, Record, Source};

use super::args::{self, Args};
use super::record::{Chosen, chosen};
use super::{Lines, base64, message, send};

const OP: &str = "output";
const STREAMS: [&str; 5] = ["stdout", "stderr", "terminal", "combined", "diagnostic"];
const POLL: Duration = Duration::from_millis(25);
const GLANCE: Duration = Duration::from_millis(1);
const TOLD: Duration = Duration::from_secs(1);
const FAILED: u8 = 125;

pub const PIECE: u64 = 4_194_304;
const PIECE_DEFAULT: u64 = 1_048_576;

#[derive(Clone, Copy)]
enum From {
    Start,
    End,
    Sequence(u64),
}

struct Asked {
    id: u64,
    attempt: Option<u64>,
    from: From,
    backward: bool,
    streams: Option<Vec<Source>>,
    follow: bool,
    max: u64,
}

struct Covered {
    first: Option<u64>,
    next: u64,
}

impl Covered {
    fn sent(&mut self, from: u64, to: u64) {
        self.first = Some(self.first.map_or(from, |first| first.min(from)));
        self.next = self.next.max(to);
    }
}

fn name(source: Source) -> &'static str {
    match source {
        Source::Stdout => "stdout",
        Source::Stderr => "stderr",
        Source::Terminal => "terminal",
        Source::Diagnostic => "diagnostic",
        Source::Combined => "combined",
        Source::Gap => "gap",
        Source::End => "end",
    }
}

fn source(name: &str) -> Option<Source> {
    match name {
        "stdout" => Some(Source::Stdout),
        "stderr" => Some(Source::Stderr),
        "terminal" => Some(Source::Terminal),
        "diagnostic" => Some(Source::Diagnostic),
        "combined" => Some(Source::Combined),
        _ => None,
    }
}

fn asked(args: &Args) -> Result<Asked, String> {
    let position = || {
        message(
            "`from` in the arguments of output must be \"start\", \"end\" or {\"sequence\":N} with a whole number N",
        )
    };
    let from = match args.get("from") {
        None => From::Start,
        Some(Value::String(word)) if word == "start" => From::Start,
        Some(Value::String(word)) if word == "end" => From::End,
        Some(Value::Object(place)) if place.len() == 1 => From::Sequence(
            place
                .get("sequence")
                .and_then(Value::as_u64)
                .ok_or_else(position)?,
        ),
        Some(_) => return Err(position()),
    };
    let streams = match args.get("streams") {
        None => None,
        Some(Value::Array(names)) => Some(
            names
                .iter()
                .map(|name| name.as_str().and_then(source))
                .collect::<Option<Vec<Source>>>()
                .ok_or_else(|| {
                    message("`streams` in the arguments of output must be an array of {streams}")
                        .replace("{streams}", &STREAMS.join(", "))
                })?,
        ),
        Some(_) => {
            return Err(message(
                "`streams` in the arguments of output must be an array of {streams}",
            )
            .replace("{streams}", &STREAMS.join(", ")));
        }
    };
    let backward = args::word(args, OP, "direction", &["forward", "backward"])? == Some("backward");
    let follow = args::flag(args, OP, "follow")?;
    if follow && backward {
        return Err(message(
            "output follows forward only; leave out `follow` or ask with direction forward",
        ));
    }
    let max = match args::number(args, OP, "max_bytes")? {
        None => PIECE_DEFAULT,
        Some(max) if (1..=PIECE).contains(&max) => max,
        Some(_) => {
            return Err(message(
                "`max_bytes` in the arguments of output must be from 1 to {limit}",
            )
            .replace("{limit}", &PIECE.to_string()));
        }
    };
    Ok(Asked {
        id: args::needed(args, OP, "id")?,
        attempt: args::number(args, OP, "attempt")?,
        from,
        backward,
        streams,
        follow,
        max,
    })
}

struct Head {
    first: Option<u64>,
    next: u64,
    written: Value,
}

fn head(reader: &Reader, kept: &Kept, existing: Option<u64>) -> Head {
    let unmeasured = |streams: &[&str]| -> Vec<Value> {
        streams
            .iter()
            .map(|stream| {
                json!({
                    "stream": stream,
                    "written_bytes": "not_measured",
                    "kept_bytes": "not_measured",
                })
            })
            .collect()
    };
    let retention = kept
        .job
        .result
        .as_ref()
        .and_then(|result| result.output_retention.as_ref());
    let counted = kept
        .meta
        .as_ref()
        .map(|meta| meta.totals.as_slice())
        .filter(|totals| !totals.is_empty())
        .or_else(|| {
            retention
                .map(|retention| retention.streams.as_slice())
                .filter(|totals| !totals.is_empty())
        });
    let totals: Vec<Value> = match (counted, reader.mode) {
        (Some(totals), _) => totals
            .iter()
            .map(|total| {
                json!({
                    "stream": name(total.stream),
                    "written_bytes": total.written_bytes,
                    "kept_bytes": total.written_bytes.saturating_sub(total.dropped_bytes),
                })
            })
            .collect(),
        (None, Some(Mode::Pipe)) => unmeasured(&["stdout", "stderr"]),
        (None, Some(Mode::Pty)) => unmeasured(&["terminal"]),
        (None, None) => unmeasured(&["combined"]),
    };
    let ended = kept.job.state.terminal();
    let (first, next, complete, quota, trimmed) = match (&kept.meta, kept.legacy) {
        (Some(meta), _) => (
            meta.segments
                .iter()
                .filter(|segment| segment.records > 0)
                .map(|segment| segment.first_sequence)
                .min(),
            meta.next_sequence.max(existing.unwrap_or(0)),
            meta.complete,
            meta.quota,
            meta.trimmed_bytes,
        ),
        (None, Some(bytes)) => (
            (bytes > 0).then_some(0),
            bytes.div_ceil(CHUNK as u64),
            ended,
            None,
            retention.and_then(|retention| retention.trimmed_bytes),
        ),
        (None, None) => (None, existing.unwrap_or(0), ended, None, None),
    };
    Head {
        first,
        next,
        written: json!({
            "attempt": reader.attempt,
            "mode": reader.mode,
            "first_sequence": first,
            "next_sequence": next,
            "complete": complete,
            "quota": quota,
            "trimmed_bytes": trimmed,
            "totals": totals,
        }),
    }
}

struct Stream<'a> {
    id: &'a Number,
    out: &'a mut UnixStream,
    legacy: bool,
}

impl Stream<'_> {
    fn line(&mut self, kind: &str, body: Value) -> io::Result<()> {
        send(
            self.out,
            &format!("{{\"re\":{},\"{kind}\":{body}}}", self.id),
        )
    }

    fn record(&mut self, record: &Record) -> io::Result<()> {
        if record.source == Source::Gap {
            let (records, bytes) = record.gap_counts();
            return self.line(
                "gap",
                json!({
                    "sequence": record.sequence,
                    "at_ms": record.at_ms,
                    "records": records,
                    "bytes": bytes,
                    "stream": record.gap_stream.map(name),
                }),
            );
        }
        let line = format!(
            "{{\"re\":{},\"record\":{{\"sequence\":{},\"at_ms\":{},\"stream\":\"{}\",\"data\":\"{}\"}}}}",
            self.id,
            record.sequence,
            if self.legacy {
                "null".to_owned()
            } else {
                record.at_ms.to_string()
            },
            name(record.source),
            base64::encoded(&record.bytes),
        );
        send(self.out, &line)
    }

    fn status(&mut self, batch: &Batch) -> io::Result<()> {
        self.line(
            "status",
            json!({
                "complete": batch.complete,
                "terminal": batch.job.state.terminal(),
                "exit_status": crate::cli_contract::exit_status(&batch.job),
                "error": batch.error,
                "quota": batch.quota,
                "trimmed": batch.trimmed,
            }),
        )
    }

    fn failed(&mut self, error: &io::Error, covered: &Covered) -> io::Result<()> {
        self.line(
            "status",
            json!({
                "complete": true,
                "terminal": true,
                "exit_status": FAILED,
                "error": error.to_string(),
                "quota": null,
                "trimmed": null,
            }),
        )?;
        self.end("error", covered, false)
    }

    fn end(&mut self, reason: &str, covered: &Covered, more: bool) -> io::Result<()> {
        self.line(
            "end",
            json!({
                "reason": reason,
                "first_sequence": covered.first,
                "next_sequence": covered.next,
                "more": more,
            }),
        )
    }

    fn stopped(&mut self, lines: &mut Lines, covered: &Covered) -> io::Result<bool> {
        if let Some(request) = lines.stop(self.id) {
            self.end("stopped", covered, true)?;
            send(
                self.out,
                &format!("{{\"re\":{request},\"ok\":{{\"stopped\":true}}}}"),
            )?;
            return Ok(true);
        }
        Ok(lines.closed())
    }
}

fn span(record: &Record) -> (u64, u64) {
    let length = if record.source == Source::Gap {
        record.gap_counts().0.max(1)
    } else {
        1
    };
    (record.sequence, record.sequence.saturating_add(length))
}

fn forward(
    asked: &Asked,
    start: u64,
    existing: u64,
    reader: &mut Reader,
    lines: &mut Lines,
    stream: &mut Stream<'_>,
) -> io::Result<()> {
    let mut covered = Covered {
        first: None,
        next: start,
    };
    let mut bytes = 0u64;
    let mut told = Instant::now();
    let mut idle = false;
    loop {
        lines.poll(if idle { POLL } else { GLANCE });
        if stream.stopped(lines, &covered)? {
            return Ok(());
        }
        let batch = match reader.read() {
            Ok(batch) => batch,
            Err(error) => return stream.failed(&error, &covered),
        };
        let mut wrote = false;
        for record in &batch.records {
            let (from, to) = span(record);
            if to <= start {
                continue;
            }
            if record.source != Source::Gap {
                if asked
                    .streams
                    .as_ref()
                    .is_some_and(|streams| !streams.contains(&record.source))
                {
                    covered.next = covered.next.max(to);
                    continue;
                }
                let length = record.bytes.len() as u64;
                let counted = !asked.follow || from < existing;
                if counted && bytes > 0 && bytes + length > asked.max {
                    if wrote {
                        stream.status(&batch)?;
                    }
                    return stream.end("limit", &covered, true);
                }
                if counted {
                    bytes += length;
                }
            }
            stream.record(record)?;
            covered.sent(from, to);
            wrote = true;
        }
        let ended = batch.job.state.terminal();
        let done = batch.error.is_some() || (batch.complete && (!asked.follow || ended));
        if done || wrote || batch.trimmed.is_some() || (asked.follow && told.elapsed() >= TOLD) {
            told = Instant::now();
            stream.status(&batch)?;
        }
        if done {
            let reason = if batch.error.is_some() {
                "error"
            } else {
                "complete"
            };
            return stream.end(reason, &covered, false);
        }
        idle = batch.records.is_empty();
    }
}

fn backward(
    asked: &Asked,
    upper: u64,
    named: u64,
    reader: &mut Reader,
    lines: &mut Lines,
    stream: &mut Stream<'_>,
) -> io::Result<()> {
    let mut covered = Covered {
        first: None,
        next: named,
    };
    let mut window: VecDeque<Record> = VecDeque::new();
    let mut bytes = 0u64;
    let mut pieces = 0usize;
    let mut dropped = false;
    let last = loop {
        lines.poll(GLANCE);
        if stream.stopped(lines, &covered)? {
            return Ok(());
        }
        let batch = match reader.read() {
            Ok(batch) => batch,
            Err(error) => return stream.failed(&error, &covered),
        };
        let mut reached = false;
        for record in &batch.records {
            if record.sequence >= upper {
                reached = true;
                break;
            }
            if record.source != Source::Gap {
                if asked
                    .streams
                    .as_ref()
                    .is_some_and(|streams| !streams.contains(&record.source))
                {
                    continue;
                }
                bytes += record.bytes.len() as u64;
                pieces += 1;
            }
            window.push_back(record.clone());
            while bytes > asked.max && pieces > 1 {
                if let Some(old) = window.pop_front()
                    && old.source != Source::Gap
                {
                    bytes -= old.bytes.len() as u64;
                    pieces -= 1;
                }
                dropped = true;
            }
        }
        if reached || batch.complete || batch.error.is_some() || batch.records.is_empty() {
            break batch;
        }
    };
    for record in &window {
        let (from, to) = span(record);
        stream.record(record)?;
        covered.first = Some(covered.first.map_or(from, |first| first.min(from)));
        covered.next = to;
    }
    stream.status(&last)?;
    let reason = match (&last.error, dropped) {
        (Some(_), _) => "error",
        (None, true) => "limit",
        (None, false) => "complete",
    };
    stream.end(reason, &covered, dropped)
}

pub fn serve(
    shared: &Shared,
    id: &Number,
    args: &Args,
    lines: &mut Lines,
    out: &mut UnixStream,
) -> io::Result<()> {
    let refused = |out: &mut UnixStream, text: String| {
        send(
            out,
            &format!("{{\"re\":{id},\"error\":{}}}", json!({"message": text})),
        )
    };
    let asked = match asked(args) {
        Ok(asked) => asked,
        Err(text) => return refused(out, text),
    };
    let (job, loaded) = match chosen(shared, asked.id, asked.attempt) {
        Chosen::Absent(what) => {
            return send(
                out,
                &format!("{{\"re\":{id},\"ok\":{}}}", super::record::missing(what)),
            );
        }
        Chosen::Attempt { job, loaded, .. } => (job, loaded),
    };
    let opened = Reader::open(&loaded.store, asked.id, Some(job.attempt)).and_then(|mut reader| {
        let existing = if asked.follow {
            let mut glance = Reader::open(&loaded.store, asked.id, Some(job.attempt))?;
            glance.snapshot()?;
            glance.end()
        } else {
            reader.snapshot()?;
            reader.end()
        };
        let kept = reader.kept()?;
        Ok((reader, kept, existing))
    });
    let (mut reader, kept, existing) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            return refused(
                out,
                message("the recorded output of Job {id} cannot be read: {error}")
                    .replace("{id}", &asked.id.to_string())
                    .replace("{error}", &error.to_string()),
            );
        }
    };
    let head = head(&reader, &kept, existing);
    send(out, &format!("{{\"re\":{id},\"ok\":{}}}", head.written))?;
    let mut stream = Stream {
        id,
        out,
        legacy: reader.mode.is_none(),
    };
    if asked.backward {
        let upper = match asked.from {
            From::Start => head.first.unwrap_or(0),
            From::End => u64::MAX,
            From::Sequence(sequence) => sequence,
        };
        let named = if upper == u64::MAX { head.next } else { upper };
        return backward(&asked, upper, named, &mut reader, lines, &mut stream);
    }
    let start = match asked.from {
        From::Start => head.first.unwrap_or(0),
        From::End => head.next,
        From::Sequence(sequence) => sequence,
    };
    if matches!(asked.from, From::End) && !asked.follow {
        let covered = Covered {
            first: None,
            next: start,
        };
        return stream.end("complete", &covered, false);
    }
    forward(&asked, start, head.next, &mut reader, lines, &mut stream)
}
