use std::process::ExitCode;

use serde::{Deserialize, Serialize};

use super::message;
use crate::model::{Job, QueueEntry, State};

pub const STATES: &[&str] = &[
    "held",
    "queued",
    "starting",
    "running",
    "suspended",
    "stopping",
    "succeeded",
    "failed",
    "cancelled",
    "lost",
];
pub const DEFAULT_LIMIT: u64 = 200;
pub const MAX_LIMIT: u64 = 10_000;
pub const SCAN_LIMIT: usize = 50_000;
pub const JOB_COLUMNS: &[&str] = &[
    "id",
    "attempt",
    "state",
    "queue",
    "session",
    "priority",
    "submitted_ms",
    "started_ms",
    "finished_ms",
    "exit_status",
    "labels",
    "command",
];
pub const OBJECT_COLUMNS: &[&str] = &["id", "path", "kind", "paused", "closed", "labels"];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    #[serde(default)]
    pub states: Vec<String>,
    #[serde(default)]
    pub queue: Option<String>,
    #[serde(default)]
    pub labels: Vec<(String, String)>,
    #[serde(default)]
    pub limit: u64,
    #[serde(default)]
    pub brief: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    pub schema_version: u32,
    pub jobs: Vec<QueueEntry>,
    pub more: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
    Tsv,
}

pub fn brief(job: &mut Job) {
    job.submitted_spec = None;
    job.requested_spec = None;
    job.effective_spec = None;
    job.admission_snapshot = None;
    job.preset_snapshot = None;
    job.priority_changes.clear();
    job.applied_resources.clear();
    job.resource_sources.clear();
    job.aggregate_domains.clear();
    job.key.clear();
}

pub fn rows() -> Option<crate::model::QueueView> {
    match super::ask(super::Call::Rows) {
        Ok(super::Answer::Rows { view }) => Some(view),
        _ => None,
    }
}

pub fn state_name(job: &Job) -> &'static str {
    let state = if job.state == State::Finished {
        job.outcome()
    } else {
        job.state.clone()
    };
    match state {
        State::Held => "held",
        State::Queued => "queued",
        State::Starting => "starting",
        State::Running => "running",
        State::Suspended => "suspended",
        State::Stopping => "stopping",
        State::Succeeded => "succeeded",
        State::Cancelled => "cancelled",
        State::Lost => "lost",
        State::Failed | State::Finished => "failed",
    }
}

pub fn ended(name: &str) -> bool {
    matches!(name, "succeeded" | "failed" | "cancelled" | "lost")
}

pub fn states(all: bool, text: Option<&str>) -> Result<Vec<String>, String> {
    let mut chosen: Vec<String> = Vec::new();
    if let Some(text) = text {
        for word in text.split(',') {
            if !STATES.contains(&word) {
                return Err(message("`{word}` is not a Job state; write one or more of held, queued, starting, running, suspended, stopping, succeeded, failed, cancelled, lost, separated by commas").replace("{word}", word));
            }
            if !chosen.iter().any(|known| known == word) {
                chosen.push(word.to_owned());
            }
        }
    }
    if all && text.is_some() {
        return Err(message("--all and --state are mutually exclusive"));
    }
    if all {
        chosen = STATES.iter().map(|name| (*name).to_owned()).collect();
    } else if text.is_none() {
        chosen = STATES
            .iter()
            .filter(|name| !ended(name))
            .map(|name| (*name).to_owned())
            .collect();
    }
    Ok(chosen)
}

pub fn limit(text: Option<&str>) -> Result<u64, String> {
    match text {
        None => Ok(DEFAULT_LIMIT),
        Some(text) => text
            .parse::<u64>()
            .ok()
            .filter(|number| (1..=MAX_LIMIT).contains(number))
            .ok_or_else(|| {
                message("--limit: `{word}` is not a number from 1 through 10000")
                    .replace("{word}", text)
            }),
    }
}

fn field(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn labels_text(labels: &std::collections::BTreeMap<String, String>) -> String {
    if labels.is_empty() {
        return "-".to_owned();
    }
    labels
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn exit_text(job: &Job) -> String {
    if job.state.terminal() {
        crate::cli_contract::exit_status(job).to_string()
    } else {
        String::new()
    }
}

fn optional(value: Option<u64>) -> String {
    value.map_or_else(String::new, |number| number.to_string())
}

fn table(rows: &[Vec<String>]) -> Vec<String> {
    let columns = rows.first().map_or(0, Vec::len);
    let widths: Vec<usize> = (0..columns)
        .map(|index| {
            rows.iter()
                .map(|row| row[index].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(index, cell)| {
                    if index + 1 == columns {
                        cell.clone()
                    } else {
                        format!("{cell:<width$}", width = widths[index])
                    }
                })
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn job_row(job: &Job) -> Vec<String> {
    vec![
        job.id.to_string(),
        job.attempt.to_string(),
        state_name(job).to_owned(),
        job.spec.queue.clone().unwrap_or_default(),
        field(&job.spec.session),
        job.spec
            .declared
            .priority
            .map_or_else(String::new, |priority| priority.to_string()),
        job.submitted_ms.to_string(),
        optional(job.started_ms),
        optional(job.finished_ms),
        exit_text(job),
        serde_json::to_string(&job.spec.declared.labels).unwrap_or_default(),
        serde_json::to_string(&job.spec.argv).unwrap_or_default(),
    ]
}

pub fn jobs(query: Query, format: Format) -> Result<ExitCode, String> {
    let wanted = match &query.ids {
        Some(text) => Some(crate::idset::parse(&[text.as_str()])?),
        None => None,
    };
    let answer = super::ask(super::Call::List { query })?;
    let super::Answer::Jobs { mut listing } = answer else {
        return Err(format!("unexpected answer {answer:?}"));
    };
    if let Some(set) = wanted {
        listing
            .jobs
            .retain(|entry| set.ids.binary_search(&entry.job.id).is_ok());
    }
    match format {
        Format::Json => println!(
            "{}",
            serde_json::to_string_pretty(&listing).map_err(|error| error.to_string())?
        ),
        Format::Tsv => {
            println!("{}", JOB_COLUMNS.join("\t"));
            for entry in &listing.jobs {
                println!("{}", job_row(&entry.job).join("\t"));
            }
        }
        Format::Text => {
            if listing.jobs.is_empty() {
                println!("{}", message("no Jobs match"));
            } else {
                let mut rows = vec![
                    [
                        "ID", "STATE", "QUEUE", "SESSION", "EXIT", "LABELS", "COMMAND",
                    ]
                    .iter()
                    .map(|name| message(name))
                    .collect::<Vec<_>>(),
                ];
                for entry in &listing.jobs {
                    let job = &entry.job;
                    let exit = exit_text(job);
                    rows.push(vec![
                        job.id.to_string(),
                        state_name(job).to_owned(),
                        job.spec.queue.clone().unwrap_or_else(|| "-".to_owned()),
                        field(&job.spec.session).chars().take(14).collect(),
                        if exit.is_empty() {
                            "-".to_owned()
                        } else {
                            exit
                        },
                        field(&labels_text(&job.spec.declared.labels)),
                        field(&crate::estimate::command_text(&job.spec.argv)),
                    ]);
                }
                for line in table(&rows) {
                    println!("{line}");
                }
            }
            if listing.more {
                eprintln!(
                    "job: {}",
                    message(
                        "more Jobs match than were listed; raise --limit or narrow --state, --queue or --label"
                    )
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn node_labels(object: &crate::objects::View) -> std::collections::BTreeMap<String, String> {
    object
        .object
        .config
        .get("labels")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

pub fn objects(
    kind: crate::objects::Kind,
    labels: &[(String, String)],
    format: Format,
) -> Result<ExitCode, String> {
    let (schema_version, mut objects) = match crate::call(crate::model::Request::Object {
        kind,
        operation: crate::objects::Operation::List,
    })? {
        crate::model::Response::Objects {
            schema_version,
            objects,
        } => (schema_version, objects),
        crate::model::Response::Error { message } => return Err(message),
        other => return Err(format!("unexpected answer {other:?}")),
    };
    objects.retain(|object| {
        let own = node_labels(object);
        labels
            .iter()
            .all(|(key, value)| own.get(key).is_some_and(|known| known == value))
    });
    match format {
        Format::Json => println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"schema_version": schema_version, "objects": objects})
            )
            .map_err(|error| error.to_string())?
        ),
        Format::Tsv => {
            println!("{}", OBJECT_COLUMNS.join("\t"));
            for object in &objects {
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}",
                    object.object.id,
                    object.path,
                    match object.object.kind {
                        crate::objects::Kind::Queue => "queue",
                        crate::objects::Kind::Group => "group",
                    },
                    !object.paused_by.is_empty(),
                    !object.closed_by.is_empty(),
                    serde_json::to_string(&node_labels(object)).unwrap_or_default()
                );
            }
        }
        Format::Text => {
            for object in &objects {
                println!(
                    "{}\t{}\t{:?}\t{}",
                    object.object.id,
                    object.path,
                    object.object.kind,
                    serde_json::to_string(&object.object.config)
                        .map_err(|error| error.to_string())?
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
