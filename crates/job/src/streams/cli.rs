use std::io::{self, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use super::{CHUNK, Mode, Record, Source, message, quota::Quota};
use crate::model::{Job, Request, Response};
use crate::operations::output::Feed;

pub fn help() -> String {
    message(
        "usage: job logs ID [-f|--follow] [--stream all|stdout|stderr|combined|diagnostic] [--attempt N] [--raw|--json] [--since-ms N] [--until-ms N] [--grep TEXT]",
    )
}

fn safe(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for &byte in bytes {
        match byte {
            b'\n' | b'\t' | 32..=126 => out.push(byte),
            _ => out.extend_from_slice(format!("\\x{byte:02x}").as_bytes()),
        }
    }
    out
}

fn gap(record: &Record, quota: Option<Quota>) -> String {
    let (records, bytes) = record.gap_counts();
    let stream = record
        .gap_stream
        .map(|stream| format!("stream={}, ", super::quota::stream_name(stream)))
        .unwrap_or_default();
    format!(
        "job: {} ({stream}records={records}, bytes={bytes}; {})\n",
        message("output records were removed by retention"),
        super::quota::kept(quota)
    )
}

fn trimmed(bytes: u64) -> String {
    format!(
        "job: {} (bytes={bytes})\n",
        message(
            "the output of this attempt was removed to keep the service within its output budget"
        )
    )
}

fn unstarted(id: u64) {
    if let Ok(Response::Finished { job }) = crate::call(Request::Wait { id, timeout_ms: 0 })
        && job.result.as_ref().is_some_and(|r| r.start_error.is_some())
    {
        eprintln!("job {id}: {}", job.exit_description());
    }
}

pub fn live(job: &Job, timeout_ms: u64) -> Result<ExitCode, String> {
    let mut reader = Feed::open(job.id, Some(job.attempt), true)?;
    let started = Instant::now();
    let mut checked = Instant::now();
    let mut unavailable = None;
    loop {
        if let Some(code) = crate::cli2::interrupt::poll(job.id, false) {
            return Ok(code);
        }
        let batch = reader.read()?;
        if let Some(bytes) = batch.trimmed {
            eprint!("{}", trimmed(bytes));
        }
        for record in &batch.records {
            let result = match record.source {
                Source::Stdout => io::stdout()
                    .write_all(&record.bytes)
                    .and_then(|_| io::stdout().flush()),
                Source::Stderr => io::stderr()
                    .write_all(&record.bytes)
                    .and_then(|_| io::stderr().flush()),
                Source::Gap => io::stderr().write_all(gap(record, batch.quota).as_bytes()),
                Source::Diagnostic => io::stderr().write_all(&safe(&record.bytes)),
                _ => return Err(super::invalid().to_string()),
            };
            if let Err(e) = result {
                if let Some(code) = crate::cli2::interrupt::closed(job.id, &e) {
                    return Ok(code);
                }
                return Err(format!(
                    "job {}: {}: {e}",
                    job.id,
                    message("output client disconnected; the Job continues")
                ));
            }
        }
        if let Some(error) = batch.error {
            return Err(error);
        }
        if batch.complete && batch.terminal {
            unstarted(job.id);
            return Ok(ExitCode::from(batch.exit_status));
        }
        if started.elapsed() >= Duration::from_millis(timeout_ms) {
            eprintln!(
                "job {}: {}",
                job.id,
                message("output wait timed out; the Job continues")
            );
            return Ok(ExitCode::from(75));
        }
        if !reader.served() && checked.elapsed() >= Duration::from_secs(1) {
            checked = Instant::now();
            if crate::call(Request::Wait {
                id: job.id,
                timeout_ms: 0,
            })
            .is_err()
            {
                let since = unavailable.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_secs(60) {
                    return Ok(crate::cli_contract::unavailable(job.id, false));
                }
            } else {
                unavailable = None;
            }
        }
        if batch.records.is_empty() {
            std::thread::sleep(
                Duration::from_millis(25)
                    .min(Duration::from_millis(timeout_ms).saturating_sub(started.elapsed())),
            );
        }
    }
}

pub fn logs(args: &[String]) -> Result<ExitCode, String> {
    if args == ["--help"] || args == ["-h"] {
        println!("{}", help());
        return Ok(ExitCode::SUCCESS);
    }
    let mut id = None;
    let mut attempt = None;
    let mut follow = false;
    let mut raw = false;
    let mut json = false;
    let mut stream = "all";
    let mut since = None;
    let mut until = None;
    let mut pattern = None;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "-f" | "--follow" => follow = true,
            "--raw" => raw = true,
            "--json" => json = true,
            "--attempt" => {
                attempt = Some(
                    words
                        .next()
                        .ok_or_else(help)?
                        .parse::<u64>()
                        .map_err(|_| help())?,
                )
            }
            "--stream" => stream = words.next().ok_or_else(help)?,
            "--since-ms" => {
                since = Some(
                    words
                        .next()
                        .ok_or_else(help)?
                        .parse::<u64>()
                        .map_err(|_| help())?,
                )
            }
            "--until-ms" => {
                until = Some(
                    words
                        .next()
                        .ok_or_else(help)?
                        .parse::<u64>()
                        .map_err(|_| help())?,
                )
            }
            "--grep" => pattern = Some(words.next().ok_or_else(help)?.as_bytes().to_vec()),
            value if id.is_none() => id = Some(value.parse::<u64>().map_err(|_| help())?),
            _ => return Err(help()),
        }
    }
    if !matches!(
        stream,
        "all" | "stdout" | "stderr" | "combined" | "diagnostic"
    ) || (raw && json)
        || since.zip(until).is_some_and(|(a, b)| a > b)
        || (json && pattern.is_some())
    {
        return Err(help());
    }
    let id = id.filter(|n| *n != 0).ok_or_else(help)?;
    let mut reader = Feed::open(id, attempt, follow)?;
    if matches!(stream, "stdout" | "stderr") && reader.mode() != Some(Mode::Pipe) {
        return Err(message(
            "separate streams are unavailable for this recording",
        ));
    }
    if reader.mode().is_none() && (since.is_some() || until.is_some()) {
        return Err(message("timestamps are unavailable for this recording"));
    }
    let mut lines: [Vec<u8>; 7] = std::array::from_fn(|_| Vec::new());
    loop {
        let batch = reader.read()?;
        if let Some(bytes) = batch.trimmed {
            eprint!("{}", trimmed(bytes));
        }
        for record in &batch.records {
            if record.source == Source::Gap && !json {
                eprint!("{}", gap(record, batch.quota));
                continue;
            }
            if record.source != Source::Gap
                && !(match stream {
                    "all" => true,
                    "stdout" => record.source == Source::Stdout,
                    "stderr" => record.source == Source::Stderr,
                    "diagnostic" => record.source == Source::Diagnostic,
                    _ => matches!(
                        record.source,
                        Source::Stdout | Source::Stderr | Source::Terminal | Source::Combined
                    ),
                })
            {
                continue;
            }
            if record.source != Source::Gap
                && (since.is_some_and(|ms| record.at_ms < ms)
                    || until.is_some_and(|ms| record.at_ms > ms))
            {
                continue;
            }
            if json {
                let mut value = serde_json::json!({"schema_version":1,"job_id":id,"attempt":reader.attempt(),"sequence":record.sequence,"at_ms":if reader.mode().is_some() {Some(record.at_ms)} else {None},"stream":record.source,"bytes":record.bytes});
                if record.source == Source::Gap {
                    let (records, bytes) = record.gap_counts();
                    value["gap"] = serde_json::json!({"stream":record.gap_stream,"records":records,"bytes":bytes,"quota":batch.quota});
                }
                writeln!(io::stdout(), "{value}").map_err(|e| e.to_string())?;
            } else if let Some(pattern) = &pattern {
                let line = &mut lines[record.source.code() as usize - 1];
                for &byte in &record.bytes {
                    line.push(byte);
                    if byte == b'\n' || line.len() == CHUNK {
                        emit_match(line, pattern, raw)?;
                        line.clear();
                    }
                }
            } else {
                emit(&record.bytes, raw)?;
            }
        }
        if let Some(error) = batch.error {
            return Err(error);
        }
        if batch.complete && (!follow || batch.terminal) {
            if let Some(pattern) = &pattern {
                for line in &lines {
                    emit_match(line, pattern, raw)?;
                }
            }
            return Ok(ExitCode::SUCCESS);
        }
        if batch.records.is_empty() {
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

fn emit(bytes: &[u8], raw: bool) -> Result<(), String> {
    let bytes = if raw { bytes.to_vec() } else { safe(bytes) };
    io::stdout()
        .write_all(&bytes)
        .and_then(|_| io::stdout().flush())
        .map_err(|e| e.to_string())
}
fn emit_match(line: &[u8], pattern: &[u8], raw: bool) -> Result<(), String> {
    if pattern.is_empty() || line.windows(pattern.len()).any(|part| part == pattern) {
        emit(line, raw)?;
    }
    Ok(())
}
