use std::process::ExitCode;

use crate::model::{Job, Response, State};

mod messages;
pub use messages::{help, message};

pub fn legacy() -> Result<bool, String> {
    if crate::clientif::invocation::active() {
        return Ok(false);
    }
    match std::env::var("JOB_CLI_COMPAT") {
        Err(std::env::VarError::NotPresent) => Ok(false),
        Ok(value) if value == "unix" => Ok(false),
        Ok(value) if value == "legacy" => Ok(true),
        _ => Err(message("JOB_CLI_COMPAT must be unix or legacy")),
    }
}

pub fn argv(
    words: &[String],
    shell: Option<&str>,
    legacy_shell: bool,
) -> Result<Vec<String>, String> {
    if words.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(shell) = shell {
        if shell.is_empty() {
            return Err(message("--shell requires an executable name or path"));
        }
        let mut argv = vec![
            shell.to_owned(),
            "-c".to_owned(),
            words[0].clone(),
            "job".to_owned(),
        ];
        argv.extend_from_slice(&words[1..]);
        return Ok(argv);
    }
    if legacy_shell && words.len() == 1 {
        return Ok(vec!["bash".to_owned(), "-c".to_owned(), words[0].clone()]);
    }
    Ok(words.to_vec())
}

pub fn exit_status(job: &Job) -> u8 {
    if job.state == State::Lost || job.result.as_ref().is_some_and(|r| r.start_error.is_some()) {
        return 125;
    }
    if job.stop.is_some() || job.state == State::Cancelled {
        return 1;
    }
    match &job.result {
        Some(result) => result
            .exit_code
            .or_else(|| result.signal.and_then(|signal| 128i32.checked_add(signal)))
            .unwrap_or(1)
            .clamp(0, 255) as u8,
        None => 1,
    }
}

pub fn wait_result(response: Response, json: bool, summary: bool, compatibility: bool) -> ExitCode {
    if compatibility {
        return crate::finish(response, json);
    }
    if !json && summary {
        let code = match &response {
            Response::Finished { job } => exit_status(job),
            Response::StillRunning { .. } | Response::Submitted { .. } => 75,
            _ => 125,
        };
        crate::finish(response, false);
        return ExitCode::from(code);
    }
    let (outcome, code, job, error) = match response {
        Response::Finished { job } => {
            let outcome = match job.state {
                State::Cancelled => "cancelled",
                State::Lost => "lost",
                _ if job.result.as_ref().is_some_and(|r| r.start_error.is_some()) => "start_error",
                _ if job.stop.is_some() => "stopped",
                _ => "completed",
            };
            (outcome, exit_status(&job), Some(job), None)
        }
        Response::StillRunning { job } | Response::Submitted { job } => {
            ("timeout", 75, Some(job), None)
        }
        Response::Error { message } => ("service_error", 125, None, Some(message)),
        _ => (
            "service_error",
            125,
            None,
            Some(message("unexpected service response")),
        ),
    };
    if json {
        println!(
            "{}",
            serde_json::json!({"schema_version":1,"outcome":outcome,"exit_status":code,"job":job,"error":error})
        );
    } else if let Some(error) = error {
        eprintln!("job: {error}");
    }
    ExitCode::from(code)
}

pub fn unavailable(id: u64, json: bool) -> ExitCode {
    let error = message(
        "service unavailable while waiting; execution is unknown, wait for the same Job again",
    );
    if json {
        println!(
            "{}",
            serde_json::json!({"schema_version":1,"outcome":"unavailable","exit_status":75,"job":null,"job_id":id,"error":error})
        );
    } else {
        eprintln!("job {id}: {error}");
    }
    ExitCode::from(75)
}

pub struct WaitOptions {
    pub id: u64,
    pub timeout_ms: u64,
    pub json: bool,
    pub summary: bool,
    pub compatibility: bool,
}

impl WaitOptions {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let compatibility = legacy()?;
        let mut id = None;
        let mut timeout_ms = crate::UNLIMITED_BUDGET_MS;
        let mut json = false;
        let mut summary = compatibility;
        let mut explicit_summary = false;
        let mut words = args.iter();
        while let Some(word) = words.next() {
            match word.as_str() {
                "--json" => json = true,
                "--summary" => {
                    summary = true;
                    explicit_summary = true;
                }
                "--timeout" => {
                    timeout_ms = crate::units::parse_duration_ms(words.next().ok_or_else(help)?)?;
                }
                "--" => {
                    let value = words.next().ok_or_else(help)?;
                    if id.is_some() || words.next().is_some() {
                        return Err(help());
                    }
                    id = Some(value.parse::<u64>().map_err(|_| help())?);
                }
                value if !value.starts_with('-') && id.is_none() => {
                    id = Some(value.parse::<u64>().map_err(|_| help())?);
                }
                _ => return Err(help()),
            }
        }
        if explicit_summary && json {
            return Err(message("--summary and --json are mutually exclusive"));
        }
        Ok(Self {
            id: id.filter(|id| *id != 0).ok_or_else(help)?,
            timeout_ms,
            json,
            summary,
            compatibility,
        })
    }
}
