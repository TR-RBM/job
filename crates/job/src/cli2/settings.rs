use std::collections::BTreeSet;
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::message;
use crate::objects::Graph;

const CEILINGS: &[&str] = &[
    "cores_milli",
    "memory",
    "max_running",
    "cpu_limit_milli",
    "memory_high",
    "memory_max",
    "memory_swap_max",
    "pids_max",
];
const OWN: &[&str] = &["fair_share", "share_weight", "pressure", "labels"];
const NAMES: &[(&str, &str)] = &[
    ("cores_milli", "cores"),
    ("memory", "mem"),
    ("host_cores_milli", "host-cores"),
    ("aging_ms", "aging"),
    ("job_scheduling_class", "job-class"),
    ("job_output_head_bytes", "job-output-head"),
    ("job_output_tail_bytes", "job-output-tail"),
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub key: String,
    pub option: String,
    pub configured: Option<Value>,
    pub effective: Option<Value>,
    pub source: Option<String>,
    pub takes_effect: bool,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Explanation {
    pub schema_version: u32,
    pub id: u64,
    pub path: String,
    pub kind: crate::objects::Kind,
    pub settings: Vec<Entry>,
}

pub fn option(key: &str) -> String {
    if let Some((_, name)) = NAMES.iter().find(|(known, _)| *known == key) {
        return (*name).to_owned();
    }
    let (prefix, rest) = match key.strip_prefix("job_") {
        Some(rest) => ("job-", rest),
        None => ("", key),
    };
    format!(
        "{prefix}{}",
        rest.strip_suffix("_milli")
            .unwrap_or(rest)
            .replace('_', "-")
    )
}

pub fn normalize(word: &str) -> String {
    let word = word.trim_start_matches('-');
    if let Some((key, _)) = NAMES.iter().find(|(_, name)| *name == word) {
        return (*key).to_owned();
    }
    if word == "parallel" {
        return "max_running".to_owned();
    }
    if let Some(key) = word
        .strip_prefix("job-")
        .and_then(crate::resource_policy::option_key)
    {
        return format!("job_{key}");
    }
    crate::admission::option_key(word)
        .or_else(|| crate::resource_policy::option_key(word))
        .map_or_else(|| word.replace('-', "_"), str::to_owned)
}

fn ceiling(graph: &Graph, id: u64, key: &str) -> Option<(u64, u64)> {
    graph
        .ancestors(id)
        .into_iter()
        .filter_map(|ancestor| {
            graph.nodes[&ancestor]
                .config
                .get(key)
                .and_then(Value::as_u64)
                .map(|value| (ancestor, value))
        })
        .min_by_key(|(_, value)| *value)
}

fn filled(key: &str, pairs: &[(&str, String)]) -> String {
    pairs.iter().fold(message(key), |text, (name, value)| {
        text.replace(&format!("{{{name}}}"), value)
    })
}

fn entry(graph: &Graph, id: u64, key: &str, supports: &dyn Fn(&str) -> bool) -> Entry {
    let view = graph.view(id);
    let configured = view.object.config.get(key).cloned();
    let mut reasons = Vec::new();
    let (effective, source) = if CEILINGS.contains(&key) {
        match ceiling(graph, id, key) {
            Some((holder, value)) => {
                let holder_path = graph.path(holder);
                if holder != id {
                    match &configured {
                        Some(own) if own.as_u64().is_some_and(|own| own > value) => {
                            reasons.push(filled(
                                "ancestor {path} sets a lower {option}: {value}; the lower one applies",
                                &[
                                    ("path", holder_path.clone()),
                                    ("option", option(key)),
                                    ("value", value.to_string()),
                                ],
                            ));
                        }
                        Some(own) if own == "unlimited" => reasons.push(filled(
                            "unlimited here, but ancestor {path} still bounds this subtree: {option} {value}",
                            &[
                                ("path", holder_path.clone()),
                                ("option", option(key)),
                                ("value", value.to_string()),
                            ],
                        )),
                        _ => {}
                    }
                }
                (Some(Value::from(value)), Some(holder_path))
            }
            None => match view.effective.get(key) {
                Some(found) => (Some(found.value.clone()), Some(found.source_path.clone())),
                None => (
                    configured.clone(),
                    configured.as_ref().map(|_| view.path.clone()),
                ),
            },
        }
    } else if let Some(found) = view.effective.get(key) {
        (Some(found.value.clone()), Some(found.source_path.clone()))
    } else {
        (
            configured.clone(),
            configured
                .as_ref()
                .filter(|_| OWN.contains(&key))
                .map(|_| view.path.clone()),
        )
    };
    let job_field = key.strip_prefix("job_");
    if let Some(field) = job_field
        && CEILINGS.contains(&field)
        && let Some((holder, bound)) = ceiling(graph, id, field)
        && let Some(value) = &effective
        && (value == "unlimited" || value.as_u64().is_some_and(|value| value > bound))
    {
        reasons.push(filled(
            "each Job may have {value}, but all Jobs below {path} together are bounded by {option} {bound}",
            &[
                ("value", value.to_string().trim_matches('"').to_owned()),
                ("path", graph.path(holder)),
                ("option", option(field)),
                ("bound", bound.to_string()),
            ],
        ));
    }
    for (request, budget) in [
        ("job_cpu_request_milli", "cores_milli"),
        ("job_memory_request", "memory"),
    ] {
        if key == request
            && let Some((holder, bound)) = ceiling(graph, id, budget)
            && effective
                .as_ref()
                .and_then(Value::as_u64)
                .is_some_and(|value| value > bound)
        {
            reasons.push(filled(
                "a Job with this request is never admitted: the admission budget {option} at {path} is {bound}",
                &[
                    ("option", option(budget)),
                    ("path", graph.path(holder)),
                    ("bound", bound.to_string()),
                ],
            ));
        }
    }
    if effective.is_some()
        && let Some(file) = crate::resource_policy::kernel_file(job_field.unwrap_or(key))
        && !supports(file)
    {
        reasons.push(filled(
            "this host cannot apply {file}: the service has no delegated cgroup controller for it, so Jobs that need it are refused",
            &[("file", file.to_owned())],
        ));
    }
    Entry {
        key: key.to_owned(),
        option: option(key),
        configured,
        effective,
        source,
        takes_effect: reasons.is_empty(),
        reasons,
    }
}

pub fn explain(
    graph: &Graph,
    id: u64,
    key: Option<&str>,
    supports: &dyn Fn(&str) -> bool,
) -> Explanation {
    let view = graph.view(id);
    let keys: BTreeSet<String> = match key {
        Some(key) => BTreeSet::from([normalize(key)]),
        None => view
            .object
            .config
            .keys()
            .chain(view.effective.keys())
            .cloned()
            .chain(
                CEILINGS
                    .iter()
                    .filter(|key| ceiling(graph, id, key).is_some())
                    .map(|key| (*key).to_owned()),
            )
            .collect(),
    };
    Explanation {
        schema_version: 1,
        id,
        path: view.path.clone(),
        kind: view.object.kind,
        settings: keys
            .iter()
            .map(|key| entry(graph, id, key, supports))
            .collect(),
    }
}

fn shown(value: &Option<Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(value) => value.to_string(),
        None => message("not set"),
    }
}

pub fn run(path: &str, key: Option<&str>, json: bool) -> Result<ExitCode, String> {
    let answer = super::ask(super::Call::Settings {
        path: path.to_owned(),
        key: key.map(str::to_owned),
    })?;
    let super::Answer::Settings { explanation } = answer else {
        return Err(format!("unexpected answer {answer:?}"));
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&explanation).map_err(|error| error.to_string())?
        );
        return Ok(ExitCode::SUCCESS);
    }
    println!(
        "{} {} (#{})",
        match explanation.kind {
            crate::objects::Kind::Queue => "Queue",
            crate::objects::Kind::Group => "Group",
        },
        explanation.path,
        explanation.id
    );
    if explanation.settings.is_empty() {
        println!("{}", message("no setting is configured here or inherited"));
    }
    for entry in &explanation.settings {
        println!(
            "{}",
            filled(
                "{option}: configured {configured}; effective {effective}{source}",
                &[
                    ("option", entry.option.clone()),
                    ("configured", shown(&entry.configured)),
                    ("effective", shown(&entry.effective)),
                    (
                        "source",
                        entry.source.as_ref().map_or_else(String::new, |source| {
                            filled("; supplied by {path}", &[("path", source.clone())])
                        }),
                    ),
                ],
            )
        );
        for reason in &entry.reasons {
            println!("  {reason}");
        }
    }
    Ok(ExitCode::SUCCESS)
}
