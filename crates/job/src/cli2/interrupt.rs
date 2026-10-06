use std::io::Write;
use std::process::ExitCode;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

use super::message;
use crate::model::{Request, Response};

pub const DETACHED: u8 = 75;

static COUNT: AtomicU32 = AtomicU32::new(0);
static LAST: AtomicI32 = AtomicI32::new(0);
static HANGUP: AtomicBool = AtomicBool::new(false);
static HANDLED: AtomicU32 = AtomicU32::new(0);
static ACTIVE: OnceLock<(Policy, String)> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Forward,
    Detach,
    Cancel,
}

extern "C" fn caught(signal: i32) {
    if signal == libc::SIGHUP {
        HANGUP.store(true, Ordering::Relaxed);
    } else {
        LAST.store(signal, Ordering::Relaxed);
        COUNT.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn parse(text: &str) -> Result<Policy, String> {
    match text {
        "forward" => Ok(Policy::Forward),
        "detach" => Ok(Policy::Detach),
        "cancel" => Ok(Policy::Cancel),
        _ => Err(message(
            "--on-interrupt: `{word}` is not a choice; write forward, detach or cancel",
        )
        .replace("{word}", text)),
    }
}

pub fn install(policy: Policy, session: &str) -> Result<(), String> {
    if ACTIVE.set((policy, session.to_owned())).is_err() {
        return Ok(());
    }
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = caught as *const () as usize;
        if unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}

fn name(signal: i32) -> &'static str {
    if signal == libc::SIGTERM {
        "SIGTERM"
    } else {
        "SIGINT"
    }
}

fn note(key: &str, id: u64, detail: &str) {
    let _ = writeln!(
        std::io::stderr(),
        "[job] {}",
        message(key)
            .replace("{id}", &id.to_string())
            .replace("{detail}", detail)
    );
}

fn detached(id: u64, json: bool, key: &str) -> ExitCode {
    let text = message(key).replace("{id}", &id.to_string());
    if json {
        let _ = writeln!(
            std::io::stdout(),
            "{}",
            serde_json::json!({"schema_version":1,"outcome":"detached","exit_status":DETACHED,"job":null,"job_id":id,"error":text})
        );
    }
    let _ = writeln!(std::io::stderr(), "[job] {text}");
    ExitCode::from(DETACHED)
}

pub fn poll(id: u64, json: bool) -> Option<ExitCode> {
    let (policy, session) = ACTIVE.get()?;
    if HANGUP.load(Ordering::Relaxed) {
        return Some(detached(
            id,
            json,
            "the terminal hung up; Job {id} goes on; wait for it with job wait {id}, cancel it with job cancel {id}",
        ));
    }
    let count = COUNT.load(Ordering::Relaxed);
    let handled = HANDLED.load(Ordering::Relaxed);
    if count == handled {
        return None;
    }
    HANDLED.store(count, Ordering::Relaxed);
    let leave = "interrupted; Job {id} goes on; wait for it with job wait {id}, cancel it with job cancel {id}";
    if handled > 0 || count > 1 || *policy == Policy::Detach {
        return Some(detached(id, json, leave));
    }
    let signal = LAST.load(Ordering::Relaxed);
    match policy {
        Policy::Forward => match crate::call(Request::Signal { id, signal }) {
            Ok(Response::Signalled { .. }) => note(
                "{detail} forwarded to Job {id}; interrupt again to leave it running and return",
                id,
                name(signal),
            ),
            Ok(Response::Error { message }) | Err(message) => note(
                "Job {id} did not receive the signal: {detail}; interrupt again to leave it and return",
                id,
                &message,
            ),
            Ok(_) => {}
        },
        Policy::Cancel => match crate::call(Request::Cancel {
            id,
            session: session.clone(),
        }) {
            Ok(Response::Error { message }) | Err(message) => note(
                "Job {id} was not cancelled: {detail}; interrupt again to leave it and return",
                id,
                &message,
            ),
            Ok(_) => note(
                "cancelling Job {id}; interrupt again to return without waiting",
                id,
                "",
            ),
        },
        Policy::Detach => {}
    }
    None
}

pub fn closed(id: u64, error: &std::io::Error) -> Option<ExitCode> {
    ACTIVE.get()?;
    (error.kind() == std::io::ErrorKind::BrokenPipe).then(|| {
        detached(
            id,
            false,
            "the output was closed; Job {id} goes on; wait for it with job wait {id}, cancel it with job cancel {id}",
        )
    })
}
