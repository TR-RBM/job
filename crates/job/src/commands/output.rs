use std::fs::File;
use std::io::{self, Read, Seek, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::process::ExitCode;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use super::message;

pub const FORMATS: &[&str] = &["text", "json"];
pub const TABULAR: &[&str] = &["text", "json", "tsv"];
pub const SCHEMA_VERSION: u32 = 1;

pub fn tabular(kind: &str) -> bool {
    matches!(kind, "list" | "queue_list" | "group_list")
}

pub fn formats(kind: Option<&str>) -> &'static [&'static str] {
    if kind.is_some_and(tabular) {
        TABULAR
    } else {
        FORMATS
    }
}

static ARGUMENTS: OnceLock<(Vec<String>, Option<&'static str>)> = OnceLock::new();
static USAGE: AtomicBool = AtomicBool::new(false);
static CONTACTED: AtomicBool = AtomicBool::new(false);

pub fn contacted() {
    CONTACTED.store(true, Ordering::Relaxed);
}

pub fn enveloped() -> bool {
    prepared().1.is_some()
}

fn prepared() -> &'static (Vec<String>, Option<&'static str>) {
    ARGUMENTS.get_or_init(super::rewritten)
}

pub fn arguments() -> &'static [String] {
    &prepared().0
}

pub fn usage() {
    USAGE.store(true, Ordering::Relaxed);
}

fn service() -> bool {
    static SERVICE: OnceLock<bool> = OnceLock::new();
    *SERVICE.get_or_init(|| {
        let mut words = std::env::args_os();
        crate::service::invoked_as_daemon(words.next().as_deref())
            || words
                .next()
                .is_some_and(|word| word == "daemon" || word == "shim")
    })
}

fn deliver(error: bool, line: bool, arguments: std::fmt::Arguments<'_>) -> io::Result<()> {
    let end: &[u8] = if line { b"\n" } else { b"" };
    if error {
        let mut stream = io::stderr().lock();
        stream.write_fmt(arguments)?;
        stream.write_all(end)
    } else {
        let mut stream = io::stdout().lock();
        stream.write_fmt(arguments)?;
        stream.write_all(end)
    }
}

pub fn write(error: bool, line: bool, arguments: std::fmt::Arguments<'_>) {
    let Err(failure) = deliver(error, line, arguments) else {
        return;
    };
    if service() {
        return;
    }
    if failure.kind() != io::ErrorKind::BrokenPipe && !error {
        let _ = writeln!(
            io::stderr(),
            "job: {}: {failure}",
            message("cannot write the output")
        );
    }
    std::process::exit(i32::from(crate::EXIT_SERVICE_ERROR));
}

fn raw(kind: &str, data: &str) {
    println!(
        "{{\"schema_version\":{SCHEMA_VERSION},\"kind\":{},\"data\":{data}}}",
        serde_json::Value::from(kind)
    );
}

pub fn emit(kind: &str, value: &serde_json::Value) {
    raw(kind, &value.to_string());
}

fn compact(text: &str) -> Option<String> {
    serde_json::from_str::<serde::de::IgnoredAny>(text).ok()?;
    let mut out = String::with_capacity(text.len());
    let mut quoted = false;
    let mut escaped = false;
    for character in text.chars() {
        if quoted {
            out.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = false;
            }
        } else if !character.is_ascii_whitespace() {
            quoted = character == '"';
            out.push(character);
        }
    }
    Some(out)
}

struct Capture {
    saved: RawFd,
    file: File,
}

impl Capture {
    fn start() -> io::Result<Self> {
        io::stdout().flush()?;
        let descriptor = unsafe { libc::memfd_create(c"job-output".as_ptr(), libc::MFD_CLOEXEC) };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_fd(descriptor) };
        let saved = unsafe { libc::fcntl(libc::STDOUT_FILENO, libc::F_DUPFD_CLOEXEC, 3) };
        if saved < 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::dup2(file.as_raw_fd(), libc::STDOUT_FILENO) } < 0 {
            let error = io::Error::last_os_error();
            unsafe { libc::close(saved) };
            return Err(error);
        }
        Ok(Self { saved, file })
    }

    fn finish(mut self) -> io::Result<String> {
        let flushed = io::stdout().flush();
        let restored = unsafe { libc::dup2(self.saved, libc::STDOUT_FILENO) };
        let error = io::Error::last_os_error();
        unsafe { libc::close(self.saved) };
        if restored < 0 {
            return Err(error);
        }
        flushed?;
        self.file.rewind()?;
        let mut bytes = Vec::new();
        self.file.read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

fn number(code: ExitCode) -> u8 {
    (0..=u8::MAX)
        .find(|status| ExitCode::from(*status) == code)
        .unwrap_or(crate::EXIT_SERVICE_ERROR)
}

fn misparsed(kind: &str) -> bool {
    let rest = arguments().get(1..).unwrap_or_default();
    match kind {
        "run" | "create" | "submit" => {
            crate::parse_options(rest).map_or(true, |o| o.command.is_empty())
        }
        "edit" => {
            crate::parse_id(rest.first()).is_err()
                || crate::parse_options(rest.get(1..).unwrap_or_default())
                    .map_or(true, |o| o.command.is_empty())
        }
        "wait" => {
            !super::takes_set(arguments()) && crate::cli_contract::WaitOptions::parse(rest).is_err()
        }
        _ => false,
    }
}

fn failure(kind: &str, status: u8, error: Option<&str>, refused: bool) {
    let local = refused
        && (!CONTACTED.load(Ordering::Relaxed)
            || error
                .is_some_and(|text| text.starts_with("usage: ") || text.starts_with("Aufruf: ")));
    let outcome = if USAGE.load(Ordering::Relaxed) || local || misparsed(kind) {
        "usage_error"
    } else {
        "service_error"
    };
    if kind == "run" || kind == "wait" {
        raw(
            kind,
            &format!(
                "{{\"schema_version\":{SCHEMA_VERSION},\"outcome\":\"{outcome}\",\"exit_status\":{status},\"job\":null,\"error\":{}}}",
                serde_json::json!(error)
            ),
        );
    } else {
        raw(
            "error",
            &format!(
                "{{\"command\":{},\"outcome\":\"{outcome}\",\"exit_status\":{status},\"error\":{}}}",
                serde_json::Value::from(kind),
                serde_json::json!(error)
            ),
        );
    }
}

pub fn run(action: impl FnOnce() -> Result<ExitCode, String>) -> Result<ExitCode, String> {
    let Some(kind) = prepared().1 else {
        return action();
    };
    let capture = Capture::start().map_err(|error| {
        format!(
            "{}: {error}",
            message("cannot collect the output for --format json")
        )
    })?;
    let result = action();
    let text = capture.finish().map_err(|error| {
        format!(
            "{}: {error}",
            message("cannot collect the output for --format json")
        )
    })?;
    match (&result, compact(&text)) {
        (_, Some(document)) => raw(kind, &document),
        (Err(error), None) => failure(kind, crate::EXIT_SERVICE_ERROR, Some(error), true),
        (Ok(code), None) => match (number(*code), text.trim()) {
            (0, "") => raw(kind, "null"),
            (0, words) => emit(kind, &serde_json::Value::from(words)),
            (status, "") => failure(kind, status, None, false),
            (status, words) => failure(kind, status, Some(words), false),
        },
    }
    result
}
