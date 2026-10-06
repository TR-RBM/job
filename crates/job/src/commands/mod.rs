use std::process::ExitCode;

mod completion;
mod help;
mod inspect;
mod messages;
pub mod output;
mod sets;
mod table;

pub use help::synopsis;
pub use messages::{german, message, message_in};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    JobId,
    AnyJobId,
    JobIdSet,
    EndedJobIdSet,
    QueuePath,
    GroupPath,
    File,
    Dir,
    Words(&'static [&'static [&'static str]]),
    Network,
    Preset,
    Signal,
    Program,
    Free,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arity {
    Flag,
    One,
    Many,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opt {
    pub long: &'static str,
    pub short: Option<&'static str>,
    pub arity: Arity,
    pub value: Value,
    pub meta: &'static str,
    pub help: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    pub heading: &'static str,
    pub legacy: bool,
    pub options: &'static [Opt],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Operand {
    pub name: &'static str,
    pub value: Value,
    pub required: bool,
    pub repeat: bool,
    pub help: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Execution,
    Interaction,
    Inspection,
    Organization,
    Administration,
    Legacy,
    Internal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Dispatch,
    Parser,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    pub path: &'static [&'static str],
    pub section: Section,
    pub summary: &'static str,
    pub groups: &'static [Group],
    pub operands: &'static [Operand],
    pub trailing_command: bool,
    pub end_of_options: bool,
    pub check: Check,
    pub exits: &'static [(&'static str, &'static str)],
    pub kind: Option<&'static str>,
    pub available: bool,
    pub hidden: bool,
    pub notes: &'static [&'static str],
}

impl Command {
    pub fn name(&self) -> String {
        self.path.join(" ")
    }

    pub fn groups(&self) -> Vec<&'static Group> {
        let mut groups: Vec<&'static Group> = self.groups.iter().collect();
        if self.kind.is_some_and(output::tabular) {
            groups.push(&table::FORMAT_TSV);
        } else if self.kind.is_some() {
            groups.push(&table::FORMAT);
        }
        groups.push(&table::HELP);
        groups
    }

    pub fn options(&self, legacy: bool) -> Vec<&'static Opt> {
        let mut options: Vec<&'static Opt> = Vec::new();
        for group in self.groups() {
            if group.legacy && !legacy {
                continue;
            }
            for option in group.options {
                if !options.iter().any(|known| known.long == option.long) {
                    options.push(option);
                }
            }
        }
        options
    }

    fn option(&self, word: &str, legacy: bool) -> Option<&'static Opt> {
        self.options(legacy)
            .into_iter()
            .find(|option| option.long == word || option.short == Some(word))
    }

    pub fn children(&self) -> Vec<&'static Command> {
        all()
            .filter(|command| {
                command.path.len() == self.path.len() + 1 && command.path.starts_with(self.path)
            })
            .collect()
    }
}

pub fn all() -> impl Iterator<Item = &'static Command> {
    table::COMMANDS
        .iter()
        .chain(table::QUEUE.iter())
        .chain(table::GROUP.iter())
        .chain(table::PRESETS.iter())
}

pub fn resolve(words: &[String]) -> Option<&'static Command> {
    all()
        .filter(|command| {
            command.path.len() <= words.len()
                && command
                    .path
                    .iter()
                    .zip(words)
                    .all(|(name, word)| name == word)
        })
        .max_by_key(|command| command.path.len())
}

enum Token<'a> {
    Option {
        option: &'static Opt,
        value: Option<&'a str>,
        at: usize,
    },
    Missing(&'static Opt),
    Unknown(&'a str),
    Operand(&'a str),
    End,
}

fn scan<'a>(command: &Command, args: &'a [String], legacy: bool) -> Vec<Token<'a>> {
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let word = args[index].as_str();
        if word == "--" {
            tokens.push(Token::End);
            break;
        }
        if !word.starts_with('-') || word.len() == 1 {
            tokens.push(Token::Operand(word));
            index += 1;
            continue;
        }
        let Some(option) = command.option(word, legacy) else {
            tokens.push(Token::Unknown(word));
            index += 1;
            continue;
        };
        if option.arity == Arity::Flag {
            tokens.push(Token::Option {
                option,
                value: None,
                at: index,
            });
            index += 1;
            continue;
        }
        match args.get(index + 1) {
            Some(value) => tokens.push(Token::Option {
                option,
                value: Some(value.as_str()),
                at: index,
            }),
            None => tokens.push(Token::Missing(option)),
        }
        index += 2;
    }
    tokens
}

fn wants_help(command: &Command, args: &[String]) -> bool {
    if command.check == Check::None {
        return args
            .first()
            .is_some_and(|word| word == "--help" || word == "-h");
    }
    scan(command, args, true).iter().any(
        |token| matches!(token, Token::Option { option, .. } if option.long == table::HELP_OPTION),
    )
}

pub fn check(prefix: &[&str], args: &[String]) -> Result<(), String> {
    let words: Vec<String> = prefix
        .iter()
        .map(|word| (*word).to_owned())
        .chain(args.iter().cloned())
        .collect();
    let Some(command) = resolve(&words) else {
        return Ok(());
    };
    if command.check == Check::None {
        return Ok(());
    }
    validate(command, &words[command.path.len()..])
}

pub fn check_object(kind: crate::objects::Kind, args: &[String]) -> Result<(), String> {
    check(
        &[match kind {
            crate::objects::Kind::Queue => "queue",
            crate::objects::Kind::Group => "group",
        }],
        args,
    )
}

pub fn listed(path: &[&str], flags: &[&str]) -> Result<(), String> {
    let words: Vec<String> = path.iter().map(|word| (*word).to_owned()).collect();
    let Some(command) = resolve(&words) else {
        return Ok(());
    };
    let flags: Vec<String> = flags.iter().map(|word| (*word).to_owned()).collect();
    let unknown = scan(command, &flags, true)
        .into_iter()
        .find_map(|token| match token {
            Token::Unknown(word) => Some(word.to_owned()),
            _ => None,
        });
    match unknown {
        Some(word) => {
            output::usage();
            let options: Vec<&str> = command
                .options(true)
                .into_iter()
                .map(|option| option.long)
                .collect();
            Err(text(
                "unknown option `{word}` for job {command}; it takes {options}; see job {command} --help",
                command,
                &word,
            )
            .replace("{options}", &options.join(", ")))
        }
        None => Ok(()),
    }
}

fn validate(command: &Command, args: &[String]) -> Result<(), String> {
    let fail = |key: &str, word: &str| {
        output::usage();
        Err(text(key, command, word))
    };
    for token in scan(command, args, false) {
        match token {
            Token::Unknown(word) => {
                return fail(
                    "unknown option `{word}` for job {command}; see job {command} --help",
                    word,
                );
            }
            Token::Missing(option) => {
                return fail(
                    "option `{word}` of job {command} needs a value; see job {command} --help",
                    option.long,
                );
            }
            Token::End if !command.trailing_command && !command.end_of_options => {
                return fail(
                    "job {command} takes no `{word}`; see job {command} --help",
                    "--",
                );
            }
            Token::Option {
                option,
                value: Some(value),
                ..
            } if option.long == table::FORMAT_OPTION
                && !output::formats(command.kind).contains(&value) =>
            {
                return fail(
                    if command.kind.is_some_and(output::tabular) {
                        "`{word}` is not an output format of job {command}; write text, json or tsv"
                    } else {
                        "`{word}` is not an output format of job {command}; write text or json"
                    },
                    value,
                );
            }
            _ => {}
        }
    }
    Ok(())
}

fn misused(command: &Command, args: &[String]) -> bool {
    let tokens = scan(command, args, true);
    if tokens.iter().any(|token| {
        matches!(token, Token::Unknown(_) | Token::Missing(_))
            || (matches!(token, Token::End) && command.end_of_options)
    }) {
        return tokens
            .iter()
            .any(|token| matches!(token, Token::Unknown(_) | Token::Missing(_)));
    }
    let count = tokens
        .iter()
        .filter(|token| matches!(token, Token::Operand(_)))
        .count();
    let least = command
        .operands
        .iter()
        .filter(|operand| operand.required)
        .count();
    let open =
        command.operands.iter().any(|operand| operand.repeat) || !command.children().is_empty();
    count < least || (!open && count > command.operands.len())
}

fn operands<'a>(command: &Command, args: &'a [String]) -> Vec<&'a str> {
    scan(command, args, false)
        .into_iter()
        .filter_map(|token| match token {
            Token::Operand(word) => Some(word),
            _ => None,
        })
        .collect()
}

fn value<'a>(command: &Command, args: &'a [String], long: &str) -> Option<&'a str> {
    scan(command, args, false)
        .into_iter()
        .find_map(|token| match token {
            Token::Option { option, value, .. } if option.long == long => value,
            _ => None,
        })
}

fn values<'a>(command: &Command, args: &'a [String], long: &str) -> Vec<&'a str> {
    scan(command, args, false)
        .into_iter()
        .filter_map(|token| match token {
            Token::Option { option, value, .. } if option.long == long => value,
            _ => None,
        })
        .collect()
}

pub fn given(command: &Command, args: &[String], long: &str) -> bool {
    scan(command, args, false)
        .iter()
        .any(|token| matches!(token, Token::Option { option, .. } if option.long == long))
}

fn text(key: &str, command: &Command, word: &str) -> String {
    message(key)
        .replace("{word}", word)
        .replace("{command}", &command.name())
}

pub fn global_options() -> &'static Group {
    &table::GLOBAL
}

pub fn placed(command: &Command, args: &[String], added: &[&str]) -> Vec<String> {
    let start = command.path.len().min(args.len());
    let rest = &args[start..];
    let mut dropped: Vec<usize> = Vec::new();
    for token in scan(command, rest, true) {
        match token {
            Token::Option {
                option,
                value: Some(value),
                at,
            } if option.long == table::FORMAT_OPTION && output::FORMATS.contains(&value) => {
                dropped.extend([at, at + 1]);
            }
            Token::Option { option, at, .. } if option.long == "--json" => dropped.push(at),
            _ => {}
        }
    }
    let end = rest
        .iter()
        .position(|word| word == "--")
        .unwrap_or(rest.len());
    let mut words: Vec<String> = args[..start].to_vec();
    words.extend(
        rest[..end]
            .iter()
            .enumerate()
            .filter(|(at, _)| !dropped.contains(at))
            .map(|(_, word)| word.clone()),
    );
    for option in added {
        if *option == "--json" || !given(command, rest, option) {
            words.push((*option).to_owned());
        }
    }
    words.extend(rest[end..].iter().cloned());
    words
}

pub fn takes_set(args: &[String]) -> bool {
    resolve(args).is_some_and(|command| sets::routed(command, &args[command.path.len()..]))
}

pub fn arguments() -> Vec<String> {
    output::arguments().to_vec()
}

fn global(args: &mut Vec<String>) {
    while args.first().is_some_and(|word| {
        table::GLOBAL
            .options
            .iter()
            .any(|option| option.long == word)
    }) {
        args.remove(0);
        crate::paths::enter_system_mode();
    }
}

fn rewritten() -> (Vec<String>, Option<&'static str>) {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    global(&mut args);
    let Some(command) = resolve(&args) else {
        return (args, None);
    };
    let Some(kind) = command.kind else {
        return (args, None);
    };
    let mut selected = None;
    let start = command.path.len();
    let found: Vec<(usize, String)> = scan(command, &args[start..], true)
        .into_iter()
        .filter_map(|token| match token {
            Token::Option {
                option,
                value: Some(value),
                at,
            } if option.long == table::FORMAT_OPTION && output::FORMATS.contains(&value) => {
                Some((start + at, value.to_owned()))
            }
            _ => None,
        })
        .collect();
    let mut plain = given(command, &args[start..], "--json");
    for (at, value) in found.into_iter().rev() {
        args.drain(at..at + 2);
        if value == "json" {
            selected = Some(kind);
            if !plain {
                args.insert(at, "--json".to_owned());
                plain = true;
            }
        }
    }
    (args, selected)
}

pub fn dispatch(args: &[String]) -> Result<Option<ExitCode>, String> {
    let Some(first) = args.first() else {
        eprintln!("{}", help::short());
        return Ok(Some(ExitCode::from(crate::EXIT_SERVICE_ERROR)));
    };
    if first == "--help" || first == "-h" {
        println!("{}", help::short());
        return Ok(Some(ExitCode::SUCCESS));
    }
    let Some(command) = resolve(args) else {
        return Ok(None);
    };
    let rest = &args[command.path.len()..];
    if command.path == ["help"] {
        return help::command(rest);
    }
    if wants_help(command, rest) {
        println!("{}", help::usage(command));
        return Ok(Some(ExitCode::SUCCESS));
    }
    if command.check == Check::Dispatch {
        validate(command, rest)?;
    }
    if command.check != Check::None && misused(command, rest) {
        output::usage();
    }
    if sets::routed(command, rest) {
        return sets::run(command, rest).map(Some);
    }
    match command.path {
        ["completion"] => completion::script(command, rest).map(Some),
        ["__complete"] => Ok(Some(completion::query(rest))),
        ["list"] => inspect::list(command, rest).map(Some),
        ["show"] => inspect::show(command, rest).map(Some),
        ["move"] => inspect::moved(command, rest).map(Some),
        ["explain"] => inspect::explained(command, rest),
        ["queue", "list"] | ["group", "list"] => inspect::objects(command, rest),
        _ => Ok(None),
    }
}
