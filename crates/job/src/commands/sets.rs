use std::process::ExitCode;

use super::{Command, Token, given, output, scan, value};
use crate::idset::{Action, Call};

const TAKING: &[&str] = &[
    "cancel",
    "remove",
    "release",
    "retry",
    "suspend",
    "continue",
    "signal",
    "reprioritize",
    "move",
    "wait",
];

fn operands<'a>(command: &Command, args: &'a [String]) -> Vec<&'a str> {
    let mut words: Vec<&str> = scan(command, args, false)
        .into_iter()
        .filter_map(|token| match token {
            Token::Operand(word) => Some(word),
            _ => None,
        })
        .collect();
    if command.end_of_options
        && let Some(at) = args.iter().position(|word| word == "--")
    {
        words.extend(args[at + 1..].iter().map(String::as_str));
    }
    words
}

pub fn routed(command: &Command, args: &[String]) -> bool {
    let [name] = command.path else {
        return false;
    };
    if !TAKING.contains(name) {
        return false;
    }
    let words = operands(command, args);
    if words.is_empty() {
        return false;
    }
    crate::idset::written(&words)
        || (*name != "remove" && given(command, args, "--dry-run"))
        || (matches!(*name, "signal" | "reprioritize") && given(command, args, "--json"))
}

fn usage(key: &str, command: &Command) -> String {
    output::usage();
    crate::idset::message(key).replace("{command}", &command.name())
}

fn action(command: &Command, args: &[String]) -> Result<Action, String> {
    let timeout = |fallback: u64| match value(command, args, "--timeout") {
        Some(text) => crate::units::parse_duration_ms(text).inspect_err(|_| output::usage()),
        None => Ok(fallback),
    };
    Ok(match command.path[0] {
        "cancel" => Action::Cancel {
            session: value(command, args, "--session")
                .map(str::to_owned)
                .unwrap_or_else(crate::clientif::invocation::session),
        },
        "remove" => Action::Remove {
            allow_lost: given(command, args, "--allow-lost"),
        },
        "release" => Action::Release,
        "retry" => Action::Retry {
            held: given(command, args, "--hold"),
            env: given(command, args, "--current-env").then(crate::client::env),
            queue: value(command, args, "--queue").map(str::to_owned),
            allow_lost: given(command, args, "--allow-lost"),
        },
        "suspend" | "continue" => Action::Freeze {
            frozen: command.path[0] == "suspend",
            timeout_ms: timeout(5_000)?,
        },
        "signal" => Action::Signal {
            signal: match value(command, args, "--signal") {
                Some(text) => {
                    crate::process::parse_signal(text).inspect_err(|_| output::usage())?
                }
                None => libc::SIGTERM,
            },
        },
        "reprioritize" => Action::Reprioritize {
            priority: crate::admission::priority(value(command, args, "--priority").ok_or_else(
                || {
                    usage(
                        "job {command} needs --priority N; see job {command} --help",
                        command,
                    )
                },
            )?)
            .inspect_err(|_| output::usage())?,
        },
        "move" => Action::Move {
            queue: value(command, args, "--queue")
                .ok_or_else(|| {
                    usage(
                        "job {command} needs --queue PATH; see job {command} --help",
                        command,
                    )
                })?
                .to_owned(),
        },
        _ => {
            if given(command, args, "--summary") {
                return Err(usage(
                    "--summary shows the output of one Job; leave it out when waiting for a set",
                    command,
                ));
            }
            Action::Wait {
                timeout_ms: timeout(crate::UNLIMITED_BUDGET_MS)?,
            }
        }
    })
}

pub fn run(command: &Command, args: &[String]) -> Result<ExitCode, String> {
    let words = operands(command, args);
    crate::idset::parse(&words).inspect_err(|_| output::usage())?;
    let call = Call {
        action: action(command, args)?,
        operands: words.iter().map(|word| (*word).to_owned()).collect(),
        dry_run: given(command, args, "--dry-run"),
    };
    crate::idset::client::run(call, given(command, args, "--json"))
}
