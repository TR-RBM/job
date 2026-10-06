use std::process::ExitCode;

use super::{Command, given, operands, text, validate, value, values};
use crate::cli2::listing::Format;
use crate::model::{Request, Response};

pub fn show(command: &Command, args: &[String]) -> Result<ExitCode, String> {
    let [id] = operands(command, args)[..] else {
        return Err(text(
            "job {command} takes exactly one ID; see job {command} --help",
            command,
            "",
        ));
    };
    let id = crate::parse_id(Some(&id.to_owned()))?;
    Ok(crate::finish(
        crate::call(Request::Status { id })?,
        given(command, args, "--json"),
    ))
}

pub fn list(command: &Command, args: &[String]) -> Result<ExitCode, String> {
    if let Some(word) = operands(command, args).first() {
        return Err(text(
            "job {command} takes no operand `{word}`; see job {command} --help",
            command,
            word,
        ));
    }
    let queue = value(command, args, "--queue").map(|path| path.trim_start_matches('/'));
    let format = format(command, args)?;
    let labels = values(command, args, "--label");
    let ids = values(command, args, "--id");
    let ids = if ids.is_empty() {
        None
    } else {
        Some(
            crate::idset::parse(&[ids.join(",").as_str()])
                .inspect_err(|_| super::output::usage())?
                .written,
        )
    };
    if given(command, args, "--all")
        || format == Format::Tsv
        || !labels.is_empty()
        || ids.is_some()
        || ["--state", "--limit"]
            .iter()
            .any(|option| given(command, args, option))
    {
        let query = (|| {
            Ok::<_, String>(crate::cli2::listing::Query {
                states: crate::cli2::listing::states(
                    given(command, args, "--all")
                        || (ids.is_some() && !given(command, args, "--state")),
                    value(command, args, "--state"),
                )?,
                queue: queue.map(str::to_owned),
                labels: labels
                    .iter()
                    .map(|label| crate::cli2::labels::pair(label))
                    .collect::<Result<_, _>>()?,
                limit: crate::cli2::listing::limit(value(command, args, "--limit"))?,
                brief: format != Format::Json,
                ids: ids.clone(),
            })
        })()
        .inspect_err(|_| super::output::usage())?;
        return crate::cli2::listing::jobs(query, format);
    }
    let brief = if given(command, args, "--json") {
        None
    } else {
        crate::cli2::listing::rows()
    };
    let mut view = match brief {
        Some(view) => view,
        None => match crate::call(Request::Queue)? {
            Response::Queue { view } => view,
            other => return Ok(crate::finish(other, false)),
        },
    };
    if let Some(queue) = queue {
        if !view.queues.iter().any(|known| known.name == queue) {
            return Err(text("there is no Queue `{word}`", command, queue));
        }
        view.jobs
            .retain(|entry| entry.job.spec.queue.as_deref() == Some(queue));
    }
    if given(command, args, "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": super::output::SCHEMA_VERSION,
                "jobs": view.jobs,
            }))
            .map_err(|error| error.to_string())?
        );
    } else {
        view.queues.clear();
        let rendered = crate::render_queue(&view, None);
        for line in rendered.lines().skip(1) {
            println!("{line}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn format(command: &Command, args: &[String]) -> Result<Format, String> {
    let tsv = value(command, args, "--format") == Some("tsv");
    let json = given(command, args, "--json");
    if tsv && json {
        super::output::usage();
        return Err(crate::cli2::message(
            "--format tsv and --json are mutually exclusive",
        ));
    }
    Ok(if tsv {
        Format::Tsv
    } else if json {
        Format::Json
    } else {
        Format::Text
    })
}

pub fn objects(command: &Command, args: &[String]) -> Result<Option<ExitCode>, String> {
    validate(command, args)?;
    let format = format(command, args)?;
    let labels = values(command, args, "--label");
    if format != Format::Tsv && labels.is_empty() {
        return Ok(None);
    }
    if let Some(word) = operands(command, args).first() {
        super::output::usage();
        return Err(text(
            "job {command} takes no operand `{word}`; see job {command} --help",
            command,
            word,
        ));
    }
    let labels = labels
        .iter()
        .map(|label| crate::cli2::labels::pair(label))
        .collect::<Result<Vec<_>, _>>()
        .inspect_err(|_| super::output::usage())?;
    crate::cli2::listing::objects(
        if command.path[0] == "queue" {
            crate::objects::Kind::Queue
        } else {
            crate::objects::Kind::Group
        },
        &labels,
        format,
    )
    .map(Some)
}

pub fn moved(command: &Command, args: &[String]) -> Result<ExitCode, String> {
    let ([id], Some(queue)) = (
        &operands(command, args)[..],
        value(command, args, "--queue"),
    ) else {
        super::output::usage();
        return Err(text(
            "job {command} takes one ID and --queue PATH; see job {command} --help",
            command,
            "",
        ));
    };
    let id = crate::parse_id(Some(&(*id).to_owned())).inspect_err(|_| super::output::usage())?;
    crate::cli2::moving::run(id, queue, given(command, args, "--json"))
}

pub fn explained(command: &Command, args: &[String]) -> Result<Option<ExitCode>, String> {
    let words = operands(command, args);
    match words[..] {
        [target] | [target, _] if target.parse::<u64>().is_err() => crate::cli2::settings::run(
            target,
            words.get(1).copied(),
            given(command, args, "--json"),
        )
        .map(Some),
        _ => Ok(None),
    }
}
