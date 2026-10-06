use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use crate::store::Store;

use super::journal::{self, Filter, Reader, Record};
use super::message;

const POLL: Duration = Duration::from_millis(200);

fn usage() -> String {
    message(
        "usage: job events [--follow] [--job ID] [--queue PATH] [--since MS] [--format json|text]",
        &[],
    )
}

fn text(record: &Record) -> String {
    let dash = |value: Option<String>| value.unwrap_or_else(|| "-".to_owned());
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        record.seq,
        record.at_ms,
        record.kind,
        dash(record.job.map(|id| id.to_string())),
        dash(record.attempt.map(|attempt| attempt.to_string())),
        dash(record.queue_path.clone().or(record.object_path.clone())),
        dash(record.from.clone()),
        record.to,
        dash(record.exit_code.map(|code| code.to_string())),
        dash(record.signal.map(|signal| signal.to_string())),
        dash(record.reason.clone()),
    )
}

fn print(records: Vec<Record>, filter: &Filter, json: bool) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    for record in records.into_iter().filter(|record| filter.admits(record)) {
        if json {
            let mut value = serde_json::to_value(&record)?;
            value["schema_version"] = serde_json::json!(journal::SCHEMA);
            writeln!(out, "{value}")?;
        } else {
            writeln!(out, "{}", text(&record))?;
        }
    }
    out.flush()
}

pub fn command(args: &[String]) -> Result<ExitCode, String> {
    let mut filter = Filter::default();
    let mut json = false;
    let mut follow = false;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--json" => json = true,
            "--follow" | "-f" => follow = true,
            "--format" => match words.next().map(String::as_str) {
                Some("json") => json = true,
                Some("text") => json = false,
                _ => return Err(usage()),
            },
            "--job" => {
                filter.job = Some(
                    words
                        .next()
                        .and_then(|text| text.parse().ok())
                        .ok_or_else(usage)?,
                )
            }
            "--queue" => filter.queue = Some(words.next().ok_or_else(usage)?.clone()),
            "--since" => {
                filter.since_ms = Some(
                    words
                        .next()
                        .and_then(|text| text.parse().ok())
                        .ok_or_else(usage)?,
                )
            }
            _ => return Err(usage()),
        }
    }
    Store::validate_default_selection().map_err(|error| error.to_string())?;
    let store = Store {
        root: Store::default_root(),
    };
    let mut reader = Reader::new(&store);
    let first = if follow { reader.more() } else { reader.read() }
        .map_err(|error| format!("{}: {error}", message("cannot read the event journal", &[])))?;
    if !json {
        println!("seq\tat_ms\tkind\tjob\tattempt\tpath\tfrom\tto\texit_code\tsignal\treason");
    }
    if print(first, &filter, json).is_err() {
        return Ok(ExitCode::SUCCESS);
    }
    loop {
        if !follow {
            break;
        }
        std::thread::sleep(POLL);
        let records = reader.more().map_err(|error| {
            format!("{}: {error}", message("cannot read the event journal", &[]))
        })?;
        if print(records, &filter, json).is_err() {
            break;
        }
    }
    if reader.unreadable > 0 {
        eprintln!(
            "job: {}: {}",
            message(
                "event lines that could not be read, such as a line cut short by a crash",
                &[]
            ),
            reader.unreadable
        );
    }
    Ok(ExitCode::SUCCESS)
}
