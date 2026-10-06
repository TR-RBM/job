use serde_json::{Map, Value, json};

use crate::commands::{self, Arity, Command, Opt, Section};

use super::message;

fn kind(value: commands::Value) -> Value {
    let name = match value {
        commands::Value::JobId => "job_id",
        commands::Value::AnyJobId => "any_job_id",
        commands::Value::JobIdSet => "job_id_set",
        commands::Value::EndedJobIdSet => "ended_job_id_set",
        commands::Value::QueuePath => "queue_path",
        commands::Value::GroupPath => "group_path",
        commands::Value::File => "file",
        commands::Value::Dir => "dir",
        commands::Value::Network => "network",
        commands::Value::Preset => "preset",
        commands::Value::Signal => "signal",
        commands::Value::Program => "program",
        commands::Value::Free => "free",
        commands::Value::Words(lists) => {
            let words: Vec<&str> = lists.iter().flat_map(|list| list.iter().copied()).collect();
            return json!({"kind": "words", "words": words});
        }
    };
    json!({"kind": name})
}

fn option(option: &Opt, german: bool) -> Value {
    json!({
        "long": option.long,
        "short": option.short,
        "arity": match option.arity {
            Arity::Flag => "flag",
            Arity::One => "one",
            Arity::Many => "many",
        },
        "value": kind(option.value),
        "meta": option.meta,
        "help": commands::message_in(german, option.help),
    })
}

fn groups(command: &Command, german: bool) -> Vec<Value> {
    let mut merged: Vec<(&str, bool, Vec<&Opt>)> = Vec::new();
    for group in command.groups() {
        let at = match merged
            .iter()
            .position(|(heading, legacy, _)| *heading == group.heading && *legacy == group.legacy)
        {
            Some(at) => at,
            None => {
                merged.push((group.heading, group.legacy, Vec::new()));
                merged.len() - 1
            }
        };
        for option in group.options {
            if !merged
                .iter()
                .any(|(_, _, known)| known.iter().any(|known| known.long == option.long))
            {
                merged[at].2.push(option);
            }
        }
    }
    merged
        .into_iter()
        .filter(|(_, _, options)| !options.is_empty())
        .map(|(heading, legacy, options)| {
            json!({
                "heading": commands::message_in(german, heading),
                "legacy": legacy,
                "options": options
                    .into_iter()
                    .map(|known| option(known, german))
                    .collect::<Vec<_>>(),
            })
        })
        .collect()
}

fn section(section: Section) -> Option<&'static str> {
    match section {
        Section::Execution => Some("execution"),
        Section::Interaction => Some("interaction"),
        Section::Inspection => Some("inspection"),
        Section::Organization => Some("organization"),
        Section::Administration => Some("administration"),
        Section::Legacy => Some("legacy"),
        Section::Internal => None,
    }
}

fn described(command: &Command, german: bool) -> Option<Value> {
    if command.hidden {
        return None;
    }
    let section = section(command.section)?;
    Some(json!({
        "path": command.path,
        "section": section,
        "summary": commands::message_in(german, command.summary),
        "usage": commands::synopsis(command).first().cloned().unwrap_or_default(),
        "groups": groups(command, german),
        "operands": command
            .operands
            .iter()
            .map(|operand| {
                json!({
                    "name": operand.name,
                    "value": kind(operand.value),
                    "required": operand.required,
                    "repeat": operand.repeat,
                    "help": commands::message_in(german, operand.help),
                })
            })
            .collect::<Vec<_>>(),
        "trailing_command": command.trailing_command,
        "end_of_options": command.end_of_options,
        "exits": command
            .exits
            .iter()
            .map(|(status, meaning)| json!([status, commands::message_in(german, meaning)]))
            .collect::<Vec<_>>(),
        "kind": command.kind,
        "available": command.available,
        "notes": command
            .notes
            .iter()
            .map(|note| commands::message_in(german, note))
            .collect::<Vec<_>>(),
    }))
}

pub fn answer(args: &Map<String, Value>) -> Result<Value, String> {
    let german = match args.get("language") {
        None => commands::german(),
        Some(Value::String(language)) if language == "en" => false,
        Some(Value::String(language)) if language == "de" => true,
        Some(_) => return Err(message("language is `en` or `de`")),
    };
    Ok(json!({
        "language": if german { "de" } else { "en" },
        "global": commands::global_options()
            .options
            .iter()
            .map(|known| option(known, german))
            .collect::<Vec<_>>(),
        "commands": commands::all()
            .filter_map(|command| described(command, german))
            .collect::<Vec<_>>(),
    }))
}
