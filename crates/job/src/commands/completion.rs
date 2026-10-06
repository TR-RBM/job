use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use super::{Arity, Command, Opt, Value, all, help, resolve};
use crate::model::{Request, Response};

pub const KINDS: &[&str] = &[
    "job",
    "anyjob",
    "ended",
    "queue",
    "group",
    "object",
    "options",
    "untranslated",
    "net",
    "netns",
    "netprofile",
];
type Place = (&'static str, usize, String);

const LIMIT: usize = 200;
const STEP: Duration = Duration::from_millis(300);
const SIGNALS: &str = "HUP INT QUIT KILL TERM USR1 USR2 STOP CONT TSTP WINCH";

fn exchange(request: &Request) -> Option<Response> {
    crate::store::Store::validate_default_selection().ok()?;
    let mut stream = UnixStream::connect(crate::socket()).ok()?;
    stream.set_read_timeout(Some(STEP)).ok()?;
    stream.set_write_timeout(Some(STEP)).ok()?;
    let mut bytes = serde_json::to_vec(request).ok()?;
    bytes.push(b'\n');
    stream.write_all(&bytes).ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

fn ask(request: Request) -> Option<Response> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(exchange(&request));
    });
    receiver.recv_timeout(STEP * 2).ok().flatten()
}

fn groups() -> Vec<String> {
    let request = Request::Versioned {
        protocol: crate::model::PROTOCOL,
        min: None,
        request: Box::new(Request::Object {
            kind: crate::objects::Kind::Group,
            operation: crate::objects::Operation::List,
        }),
    };
    match ask(request) {
        Some(Response::Objects { objects, .. }) => {
            objects.into_iter().map(|object| object.path).collect()
        }
        _ => Vec::new(),
    }
}

fn finished() -> Vec<String> {
    let request = Request::Extended {
        call: crate::cli2::Call::List {
            query: crate::cli2::listing::Query {
                states: crate::cli2::listing::STATES
                    .iter()
                    .filter(|name| crate::cli2::listing::ended(name))
                    .map(|name| (*name).to_owned())
                    .collect(),
                queue: None,
                labels: Vec::new(),
                limit: LIMIT as u64,
                brief: true,
                ids: None,
            },
        },
    };
    match ask(request) {
        Some(Response::Extended { answer }) => match *answer {
            crate::cli2::Answer::Jobs { listing } => listing
                .jobs
                .iter()
                .map(|entry| entry.job.id.to_string())
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn candidates(kind: &str) -> Vec<String> {
    let view = || match ask(Request::Queue) {
        Some(Response::Queue { view }) => Some(view),
        _ => None,
    };
    let queues = || {
        view().map_or_else(Vec::new, |view| {
            view.queues.into_iter().map(|queue| queue.name).collect()
        })
    };
    match kind {
        "job" => view().map_or_else(Vec::new, |view| {
            view.jobs
                .iter()
                .map(|entry| entry.job.id.to_string())
                .collect()
        }),
        "anyjob" | "ended" => {
            let mut ids = if kind == "anyjob" {
                candidates("job")
            } else {
                Vec::new()
            };
            ids.extend(finished());
            ids
        }
        "queue" => queues(),
        "net" | "netns" | "netprofile" => {
            let report = match ask(Request::Host) {
                Some(Response::Host { info }) => info.network.unwrap_or_default(),
                _ => Default::default(),
            };
            let prefix = |word: &str| if kind == "net" { word } else { "" }.to_owned();
            let mut names = Vec::new();
            if kind == "net" {
                names.extend(["default".to_owned(), "none".to_owned()]);
            }
            if kind != "netprofile" {
                names.extend(
                    report
                        .namespaces
                        .iter()
                        .map(|found| prefix("ns:") + &found.name),
                );
            }
            if kind != "netns" {
                names.extend(
                    report
                        .profiles
                        .iter()
                        .map(|found| prefix("profile:") + &found.name),
                );
            }
            names
        }
        "group" => groups(),
        "object" => {
            let mut paths = queues();
            paths.extend(groups());
            paths
        }
        _ => Vec::new(),
    }
}

pub fn query(args: &[String]) -> ExitCode {
    let kind = args.first().map_or("", String::as_str);
    let prefix = args.get(1).map_or("", String::as_str);
    let lines: Vec<String> = match kind {
        "options" => resolve(&args[1..])
            .filter(|command| command.path.len() == args.len() - 1)
            .map_or_else(Vec::new, |command| {
                command
                    .options(false)
                    .into_iter()
                    .map(|option| {
                        format!(
                            "{}\t{}",
                            option.long,
                            if option.arity == Arity::Flag {
                                "flag"
                            } else {
                                "value"
                            }
                        )
                    })
                    .collect()
            }),
        "untranslated" => help::untranslated()
            .into_iter()
            .map(str::to_owned)
            .collect(),
        _ => {
            let mut found: Vec<String> = candidates(kind)
                .into_iter()
                .filter(|candidate| candidate.starts_with(prefix))
                .collect();
            found.sort();
            found.dedup();
            found.truncate(LIMIT);
            found
        }
    };
    let mut out = std::io::stdout().lock();
    for line in lines {
        if writeln!(out, "{line}").is_err() {
            break;
        }
    }
    ExitCode::SUCCESS
}

fn completed() -> Vec<&'static Command> {
    all()
        .filter(|command| !command.hidden && command.available)
        .collect()
}

fn words(value: Value) -> Option<String> {
    match value {
        Value::Words(lists) => Some(
            lists
                .iter()
                .flat_map(|list| list.iter().copied())
                .collect::<Vec<_>>()
                .join(" "),
        ),
        Value::Signal => Some(SIGNALS.to_owned()),
        _ => None,
    }
}

fn dynamic(value: Value) -> Option<&'static str> {
    match value {
        Value::JobId | Value::JobIdSet => Some("job"),
        Value::AnyJobId => Some("anyjob"),
        Value::EndedJobIdSet => Some("ended"),
        Value::QueuePath => Some("queue"),
        Value::GroupPath => Some("group"),
        Value::Network => Some("net"),
        _ => None,
    }
}

fn names(option: &Opt) -> Vec<&'static str> {
    std::iter::once(option.long).chain(option.short).collect()
}

fn valued(command: &Command) -> Vec<&'static str> {
    command
        .options(true)
        .into_iter()
        .filter(|option| option.arity != Arity::Flag)
        .flat_map(names)
        .collect()
}

fn subcommands(command: &Command) -> Vec<&'static str> {
    command
        .children()
        .into_iter()
        .filter(|child| !child.hidden && child.available)
        .map(|child| child.path[child.path.len() - 1])
        .collect()
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn bash_source(value: Value) -> Option<String> {
    if let Some(words) = words(value) {
        return Some(format!("compgen -W {} -- \"$cur\"", quote(&words)));
    }
    if let Some(kind) = dynamic(value) {
        return Some(format!(
            "\"${{COMP_WORDS[0]}}\" __complete {kind} \"$cur\" 2>/dev/null"
        ));
    }
    match value {
        Value::File => Some("compgen -f -- \"$cur\"".to_owned()),
        Value::Dir => Some("compgen -d -- \"$cur\"".to_owned()),
        Value::Program => Some("compgen -c -- \"$cur\"".to_owned()),
        _ => None,
    }
}

fn arms(lines: &mut Vec<String>, indent: &str, bodies: &[(String, Vec<String>)]) {
    let mut merged: Vec<(String, Vec<String>)> = Vec::new();
    for (body, patterns) in bodies {
        match merged.iter_mut().find(|(known, _)| known == body) {
            Some((_, known)) => known.extend(patterns.iter().cloned()),
            None => merged.push((body.clone(), patterns.clone())),
        }
    }
    for (body, patterns) in merged {
        lines.push(format!("{indent}{}) {body} ;;", patterns.join("|")));
    }
}

fn fill(sources: &[String]) -> String {
    format!("mapfile -t COMPREPLY < <({})", sources.join("; "))
}

fn bash() -> String {
    let commands = completed();
    let top: Vec<&str> = commands
        .iter()
        .filter(|command| command.path.len() == 1)
        .map(|command| command.path[0])
        .chain(
            super::table::GLOBAL
                .options
                .iter()
                .map(|option| option.long),
        )
        .collect();
    let pairs: Vec<String> = commands
        .iter()
        .filter(|command| command.path.len() == 2)
        .map(|command| quote(&command.name()))
        .collect();
    let mut lines = vec![
        "# Generated by `job completion bash` from the shared command definition.".to_owned(),
        "# Change crates/job/src/commands/table.rs, then write this file again.".to_owned(),
        "_job_complete() {".to_owned(),
        "    local cur prev key first count index word opts valued".to_owned(),
        "    cur=${COMP_WORDS[COMP_CWORD]}".to_owned(),
        "    prev=${COMP_WORDS[COMP_CWORD-1]}".to_owned(),
        "    COMPREPLY=()".to_owned(),
        "    for word in \"${COMP_WORDS[@]:1:COMP_CWORD-1}\"; do".to_owned(),
        "        [[ $word == -- ]] && return".to_owned(),
        "    done".to_owned(),
        "    if (( COMP_CWORD == 1 )); then".to_owned(),
        format!(
            "        mapfile -t COMPREPLY < <(compgen -W {} -- \"$cur\")",
            quote(&top.join(" "))
        ),
        "        return".to_owned(),
        "    fi".to_owned(),
        "    key=${COMP_WORDS[1]}".to_owned(),
        "    first=2".to_owned(),
        "    if (( COMP_CWORD > 2 )); then".to_owned(),
        "        case \"$key ${COMP_WORDS[2]}\" in".to_owned(),
        format!(
            "            {}) key=\"$key ${{COMP_WORDS[2]}}\"; first=3 ;;",
            pairs.join("|")
        ),
        "        esac".to_owned(),
        "    fi".to_owned(),
        "    opts=".to_owned(),
        "    valued=".to_owned(),
        "    case \"$key\" in".to_owned(),
    ];
    let lists: Vec<(String, Vec<String>)> = commands
        .iter()
        .map(|command| {
            let options: Vec<&str> = command
                .options(true)
                .into_iter()
                .map(|option| option.long)
                .collect();
            (
                format!(
                    "opts={}; valued={}",
                    quote(&options.join(" ")),
                    quote(&valued(command).join(" "))
                ),
                vec![quote(&command.name())],
            )
        })
        .collect();
    arms(&mut lines, "        ", &lists);
    lines.push("    esac".to_owned());
    let mut sources: BTreeMap<&str, Vec<Option<String>>> = BTreeMap::new();
    for command in &commands {
        for option in command.options(true) {
            if option.arity != Arity::Flag {
                for name in names(option) {
                    let known = sources.entry(name).or_default();
                    let source = bash_source(option.value);
                    if !known.contains(&source) {
                        known.push(source);
                    }
                }
            }
        }
    }
    let mut values: Vec<(String, Vec<String>)> = Vec::new();
    for command in &commands {
        for option in command.options(true) {
            let Some(source) = bash_source(option.value) else {
                continue;
            };
            if option.arity == Arity::Flag {
                continue;
            }
            for name in names(option) {
                let pattern = if sources[name].len() == 1 {
                    format!("*:{name}")
                } else {
                    quote(&format!("{}:{name}", command.name()))
                };
                match values.iter_mut().find(|(known, _)| *known == source) {
                    Some((_, patterns)) => {
                        if !patterns.contains(&pattern) {
                            patterns.push(pattern);
                        }
                    }
                    None => values.push((source.clone(), vec![pattern])),
                }
            }
        }
    }
    lines.extend([
        "    case \" $valued \" in".to_owned(),
        "        *\" $prev \"*)".to_owned(),
        "            case \"$key:$prev\" in".to_owned(),
    ]);
    let values: Vec<(String, Vec<String>)> = values
        .into_iter()
        .map(|(source, patterns)| (fill(&[source]), patterns))
        .collect();
    arms(&mut lines, "                ", &values);
    lines.extend([
        "            esac".to_owned(),
        "            return ;;".to_owned(),
        "    esac".to_owned(),
        "    if [[ $cur == -* ]]; then".to_owned(),
        "        mapfile -t COMPREPLY < <(compgen -W \"$opts\" -- \"$cur\")".to_owned(),
        "        return".to_owned(),
        "    fi".to_owned(),
        "    count=0".to_owned(),
        "    index=$first".to_owned(),
        "    while (( index < COMP_CWORD )); do".to_owned(),
        "        word=${COMP_WORDS[index]}".to_owned(),
        "        if [[ $word == -* ]]; then".to_owned(),
        "            case \" $valued \" in *\" $word \"*) index=$((index + 1)) ;; esac".to_owned(),
        "        else".to_owned(),
        "            count=$((count + 1))".to_owned(),
        "        fi".to_owned(),
        "        index=$((index + 1))".to_owned(),
        "    done".to_owned(),
        "    case \"$key:$count\" in".to_owned(),
    ]);
    let mut exact: Vec<(String, Vec<String>)> = Vec::new();
    let mut open: Vec<(String, Vec<String>)> = Vec::new();
    for command in &commands {
        let children = subcommands(command);
        for (index, operand) in command.operands.iter().enumerate() {
            let mut sources = Vec::new();
            if index == 0 && !children.is_empty() {
                sources.push(format!(
                    "compgen -W {} -- \"$cur\"",
                    quote(&children.join(" "))
                ));
            }
            sources.extend(bash_source(operand.value));
            if sources.is_empty() {
                continue;
            }
            exact.push((
                fill(&sources),
                vec![quote(&format!("{}:{index}", command.name()))],
            ));
            if operand.repeat {
                open.push((
                    fill(&sources),
                    vec![format!("{}*", quote(&format!("{}:", command.name())))],
                ));
            }
        }
        if command.operands.is_empty() && !children.is_empty() {
            exact.push((
                fill(&[format!(
                    "compgen -W {} -- \"$cur\"",
                    quote(&children.join(" "))
                )]),
                vec![quote(&format!("{}:0", command.name()))],
            ));
        }
    }
    arms(&mut lines, "        ", &exact);
    arms(&mut lines, "        ", &open);
    lines.extend([
        "    esac".to_owned(),
        "}".to_owned(),
        "complete -o default -o bashdefault -F _job_complete job".to_owned(),
    ]);
    lines.join("\n")
}

fn fish_keys(commands: &[&Command]) -> String {
    commands
        .iter()
        .map(|command| {
            if command.path.len() == 1 {
                command.path[0].to_owned()
            } else {
                format!("\"{}\"", command.name())
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn fish_source(value: Value) -> String {
    if let Some(words) = words(value) {
        return format!(" -x -a {}", quote(&words));
    }
    if let Some(kind) = dynamic(value) {
        return format!(" -x -a '(__job_candidates {kind})'");
    }
    match value {
        Value::File => " -r -F".to_owned(),
        Value::Dir => " -x -a '(__fish_complete_directories)'".to_owned(),
        Value::Program => " -x -a '(__fish_complete_command)'".to_owned(),
        _ => " -x".to_owned(),
    }
}

fn fish() -> String {
    let commands = completed();
    let pairs: Vec<String> = commands
        .iter()
        .filter(|command| command.path.len() == 2)
        .map(|command| quote(&command.name()))
        .collect();
    let mut every: Vec<&str> = Vec::new();
    for command in &commands {
        for name in valued(command) {
            if !every.contains(&name) {
                every.push(name);
            }
        }
    }
    let mut lines: Vec<String> = [
        "# Generated by `job completion fish` from the shared command definition.",
        "# Change crates/job/src/commands/table.rs, then write this file again.",
    ]
    .map(str::to_owned)
    .to_vec();
    lines.push(format!("set -g __job_pairs {}", pairs.join(" ")));
    lines.push(format!("set -g __job_valued {}", every.join(" ")));
    lines.extend(
        [
            "function __job_key",
            "    set -l words (commandline -opc)",
            "    set -e words[1]",
            "    if test (count $words) -ge 2; and contains -- \"$words[1] $words[2]\" $__job_pairs",
            "        echo \"$words[1] $words[2]\"",
            "    else if test (count $words) -ge 1",
            "        echo $words[1]",
            "    end",
            "end",
            "function __job_in",
            "    contains -- -- (commandline -opc); and return 1",
            "    set -l key (__job_key)",
            "    test -n \"$key\"; and contains -- $key $argv",
            "end",
            "function __job_count",
            "    set -l words (commandline -opc)",
            "    set -e words[1]",
            "    set -l index (math (count (string split ' ' -- (__job_key))) + 1)",
            "    set -l count 0",
            "    while test $index -le (count $words)",
            "        if string match -q -- '-*' $words[$index]",
            "            if contains -- $words[$index] $__job_valued",
            "                set index (math $index + 1)",
            "            end",
            "        else",
            "            set count (math $count + 1)",
            "        end",
            "        set index (math $index + 1)",
            "    end",
            "    echo $count",
            "end",
            "function __job_operand",
            "    __job_in $argv[3..]; and test (__job_count) $argv[1] $argv[2]",
            "end",
            "function __job_candidates",
            "    command job __complete $argv[1] (commandline -ct) 2>/dev/null",
            "end",
            "complete -c job -f",
            "complete -c job -n 'contains -- -- (commandline -opc)' -F",
        ]
        .map(str::to_owned),
    );
    for command in commands.iter().filter(|command| command.path.len() == 1) {
        lines.push(format!(
            "complete -c job -n 'test (count (commandline -opc)) -eq 1' -a {} -d {}",
            command.path[0],
            quote(command.summary)
        ));
    }
    for option in super::table::GLOBAL.options {
        lines.push(format!(
            "complete -c job -n 'test (count (commandline -opc)) -eq 1' -l {} -d {}",
            option.long.trim_start_matches('-'),
            quote(option.help)
        ));
    }
    for command in &commands {
        for child in command.children() {
            if !child.hidden && child.available {
                lines.push(format!(
                    "complete -c job -n '__job_operand -eq 0 {}' -a {} -d {}",
                    fish_keys(&[command]),
                    child.path[child.path.len() - 1],
                    quote(child.summary)
                ));
            }
        }
    }
    let mut options: Vec<(&Opt, Vec<&Command>)> = Vec::new();
    for command in &commands {
        for option in command.options(true) {
            match options.iter_mut().find(|(known, _)| *known == option) {
                Some((_, users)) => users.push(command),
                None => options.push((option, vec![command])),
            }
        }
    }
    for (option, users) in options {
        let mut line = format!(
            "complete -c job -n '__job_in {}' -l {}",
            fish_keys(&users),
            option.long.trim_start_matches('-')
        );
        if let Some(short) = option.short {
            line.push_str(&format!(" -s {}", short.trim_start_matches('-')));
        }
        if option.arity != Arity::Flag {
            line.push_str(&fish_source(option.value));
        }
        line.push_str(&format!(" -d {}", quote(option.help)));
        lines.push(line);
    }
    let mut operands: Vec<(Place, Vec<&Command>)> = Vec::new();
    for command in &commands {
        for (index, operand) in command.operands.iter().enumerate() {
            let source = fish_source(operand.value);
            if source == " -x" {
                continue;
            }
            let place = (
                if operand.repeat { "-ge" } else { "-eq" },
                index,
                source.replace(" -x", "").replace(" -r", ""),
            );
            match operands.iter_mut().find(|(known, _)| *known == place) {
                Some((_, users)) => users.push(command),
                None => operands.push((place, vec![command])),
            }
        }
    }
    for ((comparison, index, source), users) in operands {
        lines.push(format!(
            "complete -c job -n '__job_operand {comparison} {index} {}'{source}",
            fish_keys(&users)
        ));
    }
    lines.join("\n")
}

pub fn script(command: &Command, args: &[String]) -> Result<ExitCode, String> {
    match args {
        [shell] if shell == "bash" => println!("{}", bash()),
        [shell] if shell == "fish" => println!("{}", fish()),
        _ => {
            super::output::usage();
            return Err(help::unreadable(command, &args.join(" ")));
        }
    }
    Ok(ExitCode::SUCCESS)
}
