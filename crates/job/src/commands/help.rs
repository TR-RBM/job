use std::process::ExitCode;

use super::{Arity, Command, Opt, Section, all, message, resolve, text};

const WIDTH: usize = 78;
const SECTIONS: [(Section, &str); 5] = [
    (Section::Execution, "Execution"),
    (Section::Interaction, "Interaction"),
    (Section::Inspection, "Inspection"),
    (Section::Organization, "Organization"),
    (Section::Administration, "Administration"),
];

fn wrap(label: &str, indent: usize, words: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = format!("{label:<indent$}");
    let mut empty = true;
    for word in words {
        if !empty && line.chars().count() + 1 + word.chars().count() > WIDTH {
            lines.push(line);
            line = " ".repeat(indent);
            empty = true;
        }
        if !empty {
            line.push(' ');
        }
        line.push_str(word);
        empty = false;
    }
    lines.push(line);
    lines
}

fn listed(section: Section) -> Vec<String> {
    let names: Vec<&str> = all()
        .filter(|command| {
            command.section == section
                && command.path.len() == 1
                && command.available
                && !command.hidden
        })
        .map(|command| command.path[0])
        .collect();
    let last = names.len().saturating_sub(1);
    let mut words: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            if index == last {
                (*name).to_owned()
            } else {
                format!("{name},")
            }
        })
        .collect();
    if section == Section::Interaction {
        words.extend(
            message("(for Jobs started with --pty)")
                .split(' ')
                .map(str::to_owned),
        );
    }
    words
}

pub fn short() -> String {
    let labels: Vec<(Section, String)> = SECTIONS
        .iter()
        .map(|(section, name)| (*section, format!("{}:", message(name))))
        .collect();
    let indent = labels
        .iter()
        .map(|(_, label)| label.chars().count())
        .max()
        .unwrap_or(0)
        + 1;
    let mut lines = vec![
        message("job - execute and schedule jobs"),
        String::new(),
        message("Usage: job [OPTIONS] COMMAND [ARGS...]"),
        String::new(),
    ];
    for (section, label) in &labels {
        let words = listed(*section);
        if !words.is_empty() {
            lines.extend(wrap(label, indent, &words));
        }
    }
    lines.push(String::new());
    lines.push(format!("{}:", message(super::table::GLOBAL.heading)));
    for option in super::table::GLOBAL.options {
        lines.push(format!("  {}  {}", option.long, message(option.help)));
    }
    lines.push(String::new());
    lines.push(message("See 'job COMMAND --help' or 'man job'."));
    lines.join("\n")
}

fn label(option: &Opt) -> String {
    let mut label = match option.short {
        Some(short) => format!("{short}, {}", option.long),
        None => format!("    {}", option.long),
    };
    if option.arity != Arity::Flag {
        label.push(' ');
        label.push_str(option.meta);
    }
    label
}

fn synopsis(command: &Command) -> Vec<String> {
    let mut line = format!("job {}", command.name());
    if command.groups().len() > 1 || command.kind.is_some() {
        line.push_str(" [OPTIONS]");
    }
    for operand in command.operands {
        let name = if operand.repeat {
            format!("{}...", operand.name)
        } else {
            operand.name.to_owned()
        };
        if operand.required {
            line.push_str(&format!(" {name}"));
        } else {
            line.push_str(&format!(" [{name}]"));
        }
    }
    if command.trailing_command {
        line.push_str(" -- COMMAND [ARG...]");
    }
    let mut lines = vec![line];
    if !command.children().is_empty() {
        lines.push(format!("job {} COMMAND [ARGS...]", command.name()));
    }
    lines
}

fn table(rows: &[(String, String)]) -> Vec<String> {
    let width = rows
        .iter()
        .map(|(left, _)| left.chars().count())
        .filter(|width| *width <= 30)
        .max()
        .unwrap_or(0);
    rows.iter()
        .flat_map(|(left, right)| {
            if left.chars().count() > width {
                vec![format!("  {left}"), format!("  {:width$}  {right}", "")]
            } else {
                vec![format!("  {left:<width$}  {right}")]
            }
        })
        .collect()
}

fn children(command: &Command, legacy: bool) -> Vec<(String, String)> {
    command
        .children()
        .into_iter()
        .filter(|child| {
            (child.section == Section::Legacy) == legacy && child.available && !child.hidden
        })
        .map(|child| {
            (
                child.path[child.path.len() - 1].to_owned(),
                message(child.summary),
            )
        })
        .collect()
}

pub fn usage(command: &Command) -> String {
    let mut lines = vec![
        format!("job {} - {}", command.name(), message(command.summary)),
        String::new(),
    ];
    for (index, line) in synopsis(command).iter().enumerate() {
        let lead = if index == 0 {
            message("Usage:")
        } else {
            " ".repeat(message("Usage:").chars().count())
        };
        lines.push(format!("{lead} {line}"));
    }
    for (heading, legacy) in [("Commands:", false), ("Earlier spellings:", true)] {
        let rows = children(command, legacy);
        if !rows.is_empty() {
            lines.push(String::new());
            lines.push(message(heading));
            lines.extend(table(&rows));
        }
    }
    let mut sections: Vec<(&str, Vec<(String, String)>)> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for group in command.groups() {
        let at = match sections
            .iter()
            .position(|(heading, _)| *heading == group.heading)
        {
            Some(at) => at,
            None => {
                sections.push((group.heading, Vec::new()));
                sections.len() - 1
            }
        };
        for option in group.options {
            if !seen.contains(&option.long) {
                seen.push(option.long);
                sections[at].1.push((label(option), message(option.help)));
            }
        }
    }
    for (heading, rows) in &sections {
        lines.push(String::new());
        lines.push(format!("{}:", message(heading)));
        lines.extend(table(rows));
    }
    if !command.operands.is_empty() {
        lines.push(String::new());
        lines.push(message("Operands:"));
        let rows: Vec<(String, String)> = command
            .operands
            .iter()
            .map(|operand| (operand.name.to_owned(), message(operand.help)))
            .collect();
        lines.extend(table(&rows));
    }
    for note in command.notes {
        lines.push(String::new());
        lines.push(message(note));
    }
    if command.kind.is_some() {
        lines.push(String::new());
        lines.push(message(
            "--json prints the earlier JSON unchanged; --format json wraps the same data in the versioned envelope.",
        ));
    }
    lines.push(String::new());
    lines.push(message("Exit status:"));
    let rows: Vec<(String, String)> = command
        .exits
        .iter()
        .map(|(status, meaning)| ((*status).to_owned(), message(meaning)))
        .collect();
    lines.extend(table(&rows));
    lines.push(String::new());
    lines.push(message("See 'man job'."));
    lines.join("\n")
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}

pub fn syntax() -> String {
    let mut lines = vec![
        format!("# {}", message("job syntax reference")),
        String::new(),
        message(
            "Generated by `job help --syntax` from the shared command definition. Change the definition, not this file.",
        ),
    ];
    lines.push(String::new());
    lines.push(format!("## {}", message(super::table::GLOBAL.heading)));
    lines.push(String::new());
    lines.push(message(
        "Written before the command, as in `job --system status 7`.",
    ));
    lines.push(String::new());
    lines.push(format!(
        "| {} | {} |",
        message("Option"),
        message("Meaning")
    ));
    lines.push("|---|---|".to_owned());
    for option in super::table::GLOBAL.options {
        lines.push(format!(
            "| `{}` | {} |",
            cell(option.long),
            cell(&message(option.help))
        ));
    }
    let sections = SECTIONS.iter().copied().chain([
        (Section::Legacy, "Earlier spellings"),
        (Section::Internal, "Service and tools"),
    ]);
    for (section, name) in sections {
        let commands: Vec<&Command> = all()
            .filter(|command| command.section == section && !command.hidden)
            .collect();
        if commands.is_empty() {
            continue;
        }
        lines.push(String::new());
        lines.push(format!("## {}", message(name)));
        for command in commands {
            lines.push(String::new());
            lines.push(format!("### job {}", command.name()));
            lines.push(String::new());
            lines.push(format!("{}.", message(command.summary)));
            if !command.available {
                lines.push(String::new());
                lines.push(message(
                    "This command is defined by another part of job; its options are not listed here.",
                ));
            }
            lines.push(String::new());
            for line in synopsis(command) {
                lines.push(format!("    {line}"));
            }
            let options = command.options(true);
            lines.push(String::new());
            lines.push(format!(
                "| {} | {} |",
                message("Option"),
                message("Meaning")
            ));
            lines.push("|---|---|".to_owned());
            for option in options {
                lines.push(format!(
                    "| `{}` | {} |",
                    cell(label(option).trim_start()),
                    cell(&message(option.help))
                ));
            }
            if !command.operands.is_empty() {
                lines.push(String::new());
                lines.push(format!(
                    "| {} | {} |",
                    message("Operand"),
                    message("Meaning")
                ));
                lines.push("|---|---|".to_owned());
                for operand in command.operands {
                    lines.push(format!(
                        "| `{}` | {} |",
                        cell(operand.name),
                        cell(&message(operand.help))
                    ));
                }
            }
            lines.push(String::new());
            lines.push(format!(
                "| {} | {} |",
                message("Exit status"),
                message("Meaning")
            ));
            lines.push("|---|---|".to_owned());
            for (status, meaning) in command.exits {
                lines.push(format!("| {status} | {} |", cell(&message(meaning))));
            }
            if let Some(kind) = command.kind {
                lines.push(String::new());
                lines.push(format!(
                    "{} `{kind}`.",
                    message("With --format json the envelope kind is")
                ));
            }
        }
    }
    lines.join("\n")
}

pub fn command(args: &[String]) -> Result<Option<ExitCode>, String> {
    let own = resolve(&["help".to_owned()]);
    match args.first().map(String::as_str) {
        None => println!("{}", short()),
        Some("--all") => return Ok(None),
        Some("--syntax") => println!("{}", syntax()),
        Some("--help" | "-h") => match own {
            Some(own) => println!("{}", usage(own)),
            None => println!("{}", short()),
        },
        Some(word) => match resolve(args) {
            Some(command) => println!("{}", usage(command)),
            None => {
                super::output::usage();
                return Err(
                    message("unknown command `{word}`; see job --help").replace("{word}", word)
                );
            }
        },
    }
    Ok(Some(ExitCode::SUCCESS))
}

pub fn untranslated() -> Vec<&'static str> {
    let mut keys: Vec<&'static str> = Vec::new();
    for command in all() {
        keys.push(command.summary);
        keys.extend(command.notes);
        keys.extend(command.exits.iter().map(|(_, meaning)| *meaning));
        keys.extend(command.operands.iter().map(|operand| operand.help));
        for group in command.groups() {
            keys.push(group.heading);
            keys.extend(group.options.iter().map(|option| option.help));
        }
    }
    keys.sort_unstable();
    keys.dedup();
    keys.retain(|key| !super::messages::translated(key));
    keys
}

pub fn unreadable(command: &Command, word: &str) -> String {
    text(
        "job {command} cannot read `{word}`; see job {command} --help",
        command,
        word,
    )
}
