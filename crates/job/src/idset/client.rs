use std::process::ExitCode;
use std::time::{Duration, Instant};

use super::{Action, COMPACT_FROM, Call, Entry, Outcome, Report, message, ranges};
use crate::model::{Request, Response};

fn label(entry: &Entry) -> String {
    let mut label = message(&match entry.result.as_str() {
        "no_such_job" => "no such Job".to_owned(),
        word => word.replace('_', " "),
    });
    if let Some(status) = entry.exit_status.filter(|status| *status != 0) {
        label.push_str(&format!(", {} {status}", message("exit status")));
    }
    if entry.result == "already_ended"
        && let Some(state) = &entry.state
    {
        label.push_str(&format!(" ({})", message(state)));
    }
    label
}

fn described(entry: &Entry) -> String {
    match &entry.reason {
        Some(reason) => format!("{}: {reason}", label(entry)),
        None => label(entry),
    }
}

pub fn lines(report: &Report) -> Vec<String> {
    let mut groups: Vec<(String, Vec<u64>)> = Vec::new();
    for entry in &report.results {
        let text = described(entry);
        match groups.iter_mut().find(|(known, _)| *known == text) {
            Some((_, ids)) => ids.push(entry.id),
            None => groups.push((text, vec![entry.id])),
        }
    }
    let note = message("dry run, nothing changed");
    if groups.iter().any(|(_, ids)| ids.len() > COMPACT_FROM) {
        let mut parts: Vec<String> = Vec::new();
        if report.dry_run {
            parts.push(note);
        }
        parts.extend(
            groups
                .iter()
                .map(|(text, ids)| format!("{text}: {}", ranges(ids))),
        );
        return vec![parts.join("; ")];
    }
    report
        .results
        .iter()
        .map(|entry| {
            if report.dry_run {
                format!("{}: {} ({note})", entry.id, described(entry))
            } else {
                format!("{}: {}", entry.id, described(entry))
            }
        })
        .collect()
}

fn ask(call: &Call) -> Result<Report, String> {
    match crate::call(Request::Extended {
        call: crate::cli2::Call::Set { call: call.clone() },
    })? {
        Response::Extended { answer } => match *answer {
            crate::cli2::Answer::Set { report } => Ok(report),
            other => Err(format!("unexpected answer {other:?}")),
        },
        Response::Error { message } => Err(crate::cli2::unknown_request(message)),
        other => Err(format!("unexpected answer {other:?}")),
    }
}

fn waited(call: &Call, timeout_ms: u64) -> Result<Report, String> {
    let started = Instant::now();
    loop {
        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let remaining = timeout_ms.saturating_sub(elapsed);
        let report = ask(&Call {
            action: Action::Wait {
                timeout_ms: remaining.min(1000),
            },
            ..call.clone()
        })?;
        if remaining == 0
            || !report
                .results
                .iter()
                .any(|entry| entry.outcome == Outcome::Pending)
        {
            return Ok(report);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub fn run(call: Call, json: bool) -> Result<ExitCode, String> {
    let report = match call.action {
        Action::Wait { timeout_ms } => waited(&call, timeout_ms)?,
        _ => ask(&call)?,
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
    } else {
        for line in lines(&report) {
            println!("{line}");
        }
    }
    if report.selected == 0 {
        eprintln!(
            "job: {}",
            message("the set `{word}` selects no existing Job").replace("{word}", &report.set)
        );
    }
    Ok(ExitCode::from(report.exit_status()))
}
