use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::commands::{self, output};
use crate::daemon::Shared;
use crate::daemon::client_service::{self, Saved};
use crate::model::Env;
use crate::terminal::Size;

use super::invocation::{self, Invocation};
use super::message;

const USAGE_STATUS: u8 = 125;
const SINGLE: &[&str] = &[
    "submit",
    "create",
    "edit",
    "release",
    "retry",
    "cancel",
    "signal",
    "suspend",
    "continue",
    "reprioritize",
    "move",
    "remove",
    "update",
    "resource-update",
];
const OBJECT: &[&str] = &[
    "create", "set", "unset", "rename", "move", "pause", "resume", "close", "open", "update",
    "cancel", "suspend", "continue", "remove",
];
const QUEUE_ONLY: &[&str] = &["add", "rm", "clear"];
const PLACED: &[&str] = &["submit", "create", "edit"];

fn carried(words: &[String]) -> bool {
    let first = words[0].as_str();
    let second = words.get(1).map(String::as_str);
    match first {
        "queue" => second.is_some_and(|verb| OBJECT.contains(&verb) || QUEUE_ONLY.contains(&verb)),
        "group" => second.is_some_and(|verb| OBJECT.contains(&verb)),
        "config" => second == Some("reload"),
        _ => SINGLE.contains(&first),
    }
}

fn instead(words: &[String]) -> Option<&'static str> {
    let second = words.get(1).map(String::as_str);
    match (words[0].as_str(), second) {
        ("list", _) | ("queue", Some("list")) => Some("jobs"),
        ("status" | "show", _) => Some("job"),
        ("queue" | "group", Some("show")) | ("group", Some("list")) => Some("tree"),
        ("host", _) => Some("totals"),
        ("logs", _) => Some("output"),
        ("help" | "completion", _) => Some("commands"),
        ("attach" | "explain" | "attempts" | "pressure" | "config", _) => Some("native"),
        ("net", Some("list" | "show")) => Some("native"),
        _ => None,
    }
}

fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}

struct Outcome<'a> {
    outcome: &'a str,
    status: u8,
    kind: Option<&'a str>,
    data: Option<String>,
    text: Vec<String>,
    diagnostics: Vec<String>,
    instead: Option<Option<&'a str>>,
}

impl Outcome<'_> {
    fn written(self) -> String {
        let mut line = format!(
            "{{\"outcome\":{},\"exit_status\":{},\"kind\":{},\"data\":{},\"text\":{},\"diagnostics\":{}",
            json!(self.outcome),
            self.status,
            json!(self.kind),
            self.data.as_deref().unwrap_or("null"),
            json!(self.text),
            json!(self.diagnostics),
        );
        if let Some(instead) = self.instead {
            line.push_str(&format!(",\"use\":{}", json!(instead)));
        }
        line.push('}');
        line
    }
}

fn refused(outcome: &str, sentence: Option<String>) -> String {
    Outcome {
        outcome,
        status: USAGE_STATUS,
        kind: None,
        data: None,
        text: Vec::new(),
        diagnostics: sentence
            .map(|sentence| lines(&format!("job: {sentence}")))
            .unwrap_or_default(),
        instead: None,
    }
    .written()
}

struct Asked {
    words: Vec<String>,
    cwd: Option<PathBuf>,
    env: Option<Env>,
    terminal: Size,
    session: String,
    dry_run: bool,
}

fn asked(args: &Map<String, Value>) -> Result<Asked, String> {
    let words: Vec<String> = args
        .get("words")
        .and_then(|words| serde_json::from_value(words.clone()).ok())
        .filter(|words: &Vec<String>| !words.is_empty())
        .ok_or_else(|| message("command needs words: an array with at least one string"))?;
    let cwd = match args.get("cwd") {
        None => None,
        Some(Value::String(path)) if path.starts_with('/') => Some(PathBuf::from(path)),
        Some(_) => return Err(message("cwd is not an absolute path")),
    };
    let env = match args.get("env") {
        None => None,
        Some(env @ Value::Object(_)) => {
            Some(serde_json::from_value::<Env>(env.clone()).map_err(|_| {
                message("env is not an object {\"vars\":[[\"NAME\",\"value\"],...]}")
            })?)
        }
        Some(_) => {
            return Err(message(
                "env is not an object {\"vars\":[[\"NAME\",\"value\"],...]}",
            ));
        }
    };
    let terminal = match args.get("terminal") {
        None => Size::default(),
        Some(size) => serde_json::from_value::<Size>(size.clone())
            .ok()
            .filter(|size| size.valid())
            .ok_or_else(|| {
                message(
                    "terminal is not an object {\"rows\":R,\"cols\":C} with 1 to 200 rows and 1 to 500 columns",
                )
            })?,
    };
    let session = match args.get("session") {
        None => "unnamed".to_owned(),
        Some(Value::String(session)) => session.clone(),
        Some(_) => return Err(message("session is not a string")),
    };
    let dry_run = match args.get("dry_run") {
        None => false,
        Some(Value::Bool(dry_run)) => *dry_run,
        Some(_) => return Err(message("dry_run is not true or false")),
    };
    Ok(Asked {
        words,
        cwd,
        env,
        terminal,
        session,
        dry_run,
    })
}

pub fn answer(shared: &Arc<Shared>, args: &Map<String, Value>) -> Result<String, String> {
    let asked = asked(args)?;
    let first = asked.words[0].clone();
    if !carried(&asked.words) {
        return Ok(match commands::resolve(&asked.words[..1]) {
            Some(_) => Outcome {
                outcome: "not_a_command_request",
                status: USAGE_STATUS,
                kind: None,
                data: None,
                text: Vec::new(),
                diagnostics: Vec::new(),
                instead: Some(instead(&asked.words)),
            }
            .written(),
            None => refused(
                "usage_error",
                Some(
                    commands::message("unknown command `{word}`; see job --help")
                        .replace("{word}", &first),
                ),
            ),
        });
    }
    let Some(command) = commands::resolve(&asked.words) else {
        return Ok(refused(
            "usage_error",
            Some(
                commands::message("unknown command `{word}`; see job --help")
                    .replace("{word}", &first),
            ),
        ));
    };
    if PLACED.contains(&first.as_str()) && asked.cwd.is_none() {
        return Err(
            message("`{word}` needs cwd, an absolute path on the service's host")
                .replace("{word}", &first),
        );
    }
    let rest = &asked.words[command.path.len()..];
    if commands::given(command, rest, "--stdin") {
        return Ok(refused(
            "usage_error",
            Some(message(
                "--stdin cannot be carried: the standard input of the client is not on this connection",
            )),
        ));
    }
    if first == "retry" && commands::given(command, rest, "--current-env") {
        return Ok(refused(
            "usage_error",
            Some(message(
                "--current-env is not accepted here: the service has no environment of the client; give the environment in env",
            )),
        ));
    }
    let listed = |long: &str| {
        command
            .options(true)
            .iter()
            .any(|option| option.long == long)
    };
    if asked.dry_run && !listed("--dry-run") {
        return Ok(refused(
            "usage_error",
            Some(
                message("job {command} has no --dry-run, so dry_run is not accepted for it")
                    .replace("{command}", &command.name()),
            ),
        ));
    }
    let env = match (first.as_str(), asked.env.clone()) {
        (_, Some(env)) => env,
        ("edit", None) => match rest.first().and_then(|word| word.parse::<u64>().ok()) {
            Some(id) => match client_service::saved(shared, id) {
                Saved::Kept(env) => env,
                Saved::Missing => Env { vars: Vec::new() },
                Saved::Unreadable => {
                    return Ok(refused(
                        "done",
                        Some(
                            message(
                                "the saved environment of Job {id} cannot be read, so it was not edited; give the environment in env",
                            )
                            .replace("{id}", &id.to_string()),
                        ),
                    ));
                }
            },
            None => Env { vars: Vec::new() },
        },
        (_, None) => Env { vars: Vec::new() },
    };
    let mut added: Vec<&str> = Vec::new();
    if command.kind.is_some() {
        added.push("--json");
    }
    if asked.dry_run {
        added.push("--dry-run");
    }
    if first == "retry" && asked.env.is_some() {
        added.push("--current-env");
    }
    let words = commands::placed(command, &asked.words, &added);
    let (ran, invocation) = invocation::run(
        Invocation {
            service: Arc::clone(shared),
            cwd: asked.cwd,
            env,
            session: asked.session,
            terminal: asked.terminal,
            enveloped: command.kind.is_some(),
            usage: false,
            contacted: false,
            out: String::new(),
            err: String::new(),
        },
        || {
            let result = crate::carry_out(&words);
            let (flagged, contacted) = invocation::flags().unwrap_or_default();
            let misused = result.as_ref().err().is_some_and(|error| {
                output::misused(&output::Attempt {
                    kind: command.kind,
                    arguments: &words,
                    flagged,
                    contacted,
                    error: Some(error),
                    refused: true,
                })
            });
            (result, misused)
        },
    );
    let (Some((result, misused)), Some(invocation)) = (ran, invocation) else {
        return Err(message(
            "the service failed while it carried out the command; its log says more",
        ));
    };
    let mut diagnostics = lines(&invocation.err);
    let status = match &result {
        Ok(code) => output::number(*code),
        Err(error) => {
            diagnostics.extend(lines(&format!("job: {error}")));
            USAGE_STATUS
        }
    };
    let printed = invocation.out.trim();
    let document = command.kind.and_then(|_| output::compact(&invocation.out));
    let (kind, data, text) = match (command.kind, document, status) {
        (Some(kind), Some(document), _) => (Some(kind), Some(document), Vec::new()),
        (Some(kind), None, 0) if result.is_ok() && printed.is_empty() => {
            (Some(kind), None, Vec::new())
        }
        _ => (None, None, lines(&invocation.out)),
    };
    Ok(Outcome {
        outcome: if misused { "usage_error" } else { "done" },
        status,
        kind,
        data,
        text,
        diagnostics,
        instead: None,
    }
    .written())
}
