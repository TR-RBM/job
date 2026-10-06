use std::process::ExitCode;

use crate::store::Store;

use super::audit;

pub fn help() -> String {
    [
        "Reliability: --idempotency-key KEY on run, submit and create returns the Job already recorded for KEY instead of adding another; KEY is 1 to 128 characters of A-Z a-z 0-9 . _ : -",
        "job audit [--target ID|PATH] [--action NAME] [--since MS] [--json]  read the audit journal of mutating requests",
    ]
    .map(super::message)
    .join("\n")
}

fn usage() -> String {
    super::message("usage: job audit [--target ID|PATH] [--action NAME] [--since MS] [--json]")
}

pub fn audit_command(args: &[String]) -> Result<ExitCode, String> {
    if args == ["--help"] || args == ["-h"] {
        println!("{}", help());
        return Ok(ExitCode::SUCCESS);
    }
    let mut filter = audit::Filter {
        target: None,
        action: None,
        since_ms: None,
    };
    let mut json = false;
    let mut words = args.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--json" => json = true,
            "--target" => filter.target = Some(words.next().ok_or_else(usage)?.clone()),
            "--action" => filter.action = Some(words.next().ok_or_else(usage)?.clone()),
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
    let (entries, unreadable) = audit::read(&store, &filter).map_err(|error| error.to_string())?;
    if crate::commands::output::enveloped() {
        println!(
            "{}",
            serde_json::json!({"schema_version": audit::SCHEMA, "entries": entries, "unreadable": unreadable})
        );
        return Ok(ExitCode::SUCCESS);
    }
    if !json {
        println!("seq\tat_ms\taction\ttarget\tpeer_uid\tpeer_pid\tsession\tresult\toperation");
    }
    let text = |value: Option<String>| value.unwrap_or_else(|| "-".to_owned());
    for entry in entries {
        if json {
            let mut value = serde_json::to_value(&entry).map_err(|error| error.to_string())?;
            value["schema_version"] = serde_json::json!(audit::SCHEMA);
            println!("{value}");
        } else {
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                entry.seq,
                entry.at_ms,
                entry.action,
                text(entry.target),
                text(entry.peer_uid.map(|uid| uid.to_string())),
                text(entry.peer_pid.map(|pid| pid.to_string())),
                text(entry.session),
                entry.result,
                text(entry.operation)
            );
        }
    }
    if unreadable > 0 {
        eprintln!(
            "job: {}: {unreadable}",
            super::message(
                "audit lines that could not be read, such as a line cut short by a crash"
            )
        );
    }
    Ok(ExitCode::SUCCESS)
}

pub fn replay_note(job: &crate::model::Job) {
    if job.durability.replayed {
        eprintln!(
            "job: {}: {}",
            super::message("this idempotency key was already recorded; no new Job was added"),
            job.id
        );
    }
}
