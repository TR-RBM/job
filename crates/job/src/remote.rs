use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::model::{Declared, Job, PROTOCOL, Remote, Request, Response};
use crate::store::Store;

const REMOTE_COMMAND: &str = "sh -c 'PATH=\"$HOME/.local/bin:$PATH\" exec job remote'";
const FOLLOW_POLL: Duration = Duration::from_millis(200);
const EXIT_PROTOCOL: u8 = 124;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Host,
    Submit {
        argv: Vec<String>,
        dir: Option<PathBuf>,
        declared: Box<Declared>,
        session: String,
        send: bool,
    },
    FollowStreams {
        id: u64,
        attempt: u64,
    },
    Follow {
        id: u64,
        offset: u64,
    },
    Status {
        id: u64,
    },
    AttemptStatus {
        id: u64,
        attempt: u64,
    },
    Cancel {
        id: u64,
        session: String,
    },
    Fetch {
        dir: PathBuf,
        paths: Vec<PathBuf>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub protocol: u32,
    pub op: Op,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submitted {
    pub attempt: u64,
    pub id: u64,
    pub dir: PathBuf,
}

pub fn ssh(remote: &Remote) -> Command {
    let mut command = Command::new("ssh");
    command.args(["-T", "-o", "BatchMode=yes", "-o", "ServerAliveInterval=15"]);
    if let Some(key) = &remote.key {
        command
            .arg("-o")
            .arg("IdentitiesOnly=yes")
            .arg("-i")
            .arg(key);
    }
    for option in &remote.options {
        command.arg("-o").arg(option);
    }
    command.arg(&remote.target).arg(REMOTE_COMMAND);
    command
}

pub fn request_line(op: Op) -> Vec<u8> {
    let mut line = serde_json::to_vec(&Envelope {
        protocol: PROTOCOL,
        op,
    })
    .unwrap_or_default();
    line.push(b'\n');
    line
}

pub fn call(remote: &Remote, op: Op, extra: Option<&mut dyn Read>) -> Result<Vec<u8>, String> {
    crate::commands::output::contacted();
    let mut child = ssh(remote)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run ssh: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let written = stdin
            .write_all(&request_line(op))
            .and_then(|_| match extra {
                Some(source) => io::copy(source, &mut stdin).map(|_| ()),
                None => Ok(()),
            });
        drop(stdin);
        if let Err(e) = written {
            let _ = child.kill();
            return Err(format!("cannot send the request to {}: {e}", remote.target));
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("ssh to {}: {e}", remote.target))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!(
            "{}: {}",
            remote.target,
            said.trim_start_matches("job: ")
        ))
    }
}

fn read_line_raw() -> io::Result<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let read = unsafe { libc::read(0, byte.as_mut_ptr().cast(), 1) };
        if read < 0 {
            return Err(io::Error::last_os_error());
        }
        if read == 0 || byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
    }
    Ok(String::from_utf8_lossy(&line).into_owned())
}

fn expand(path: &Path) -> PathBuf {
    match (path.strip_prefix("~"), std::env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path.to_path_buf(),
    }
}

fn daemon(request: Request) -> Result<Response, String> {
    let socket = crate::paths::client_socket()
        .unwrap_or_else(|_| Store::default_root().join(crate::paths::STATE_SOCKET));
    crate::client::call(&socket, &request).map_err(|e| {
        format!(
            "the job daemon on {} does not answer ({e}); start it, for example with: systemctl --user start jobd, or: jobd",
            crate::host::hostname()
        )
    })
}

fn fresh_directory() -> io::Result<PathBuf> {
    let base = Store::default_root().join("remote-work");
    std::fs::create_dir_all(&base)?;
    let dir = base.join(format!("{}-{}", crate::shim::now_ms(), std::process::id()));
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

fn exit_of(job: &Job) -> u8 {
    match (&job.stop, job.result.as_ref().and_then(|r| r.exit_code)) {
        (None, Some(code)) => code.clamp(0, 255) as u8,
        _ => 1,
    }
}

pub fn serve() -> Result<ExitCode, String> {
    Store::validate_default_selection().map_err(|e| e.to_string())?;
    let line = read_line_raw().map_err(|e| format!("cannot read the request: {e}"))?;
    let protocol = serde_json::from_str::<serde_json::Value>(&line)
        .ok()
        .and_then(|v| v.get("protocol").and_then(serde_json::Value::as_u64));
    if protocol != Some(u64::from(PROTOCOL)) {
        eprintln!(
            "job: {} speaks job protocol {PROTOCOL}, the caller {}; install the same release of job on both",
            crate::host::hostname(),
            protocol.map_or_else(|| "none".to_string(), |p| p.to_string())
        );
        return Ok(ExitCode::from(EXIT_PROTOCOL));
    }
    let envelope: Envelope =
        serde_json::from_str(&line).map_err(|e| format!("unreadable request: {e}"))?;
    let mut out = io::stdout().lock();
    match envelope.op {
        Op::Host => match daemon(Request::Host)? {
            Response::Host { info } => {
                let _ = writeln!(out, "{}", serde_json::to_string(&info).unwrap_or_default());
            }
            other => return Err(format!("unexpected answer {other:?}")),
        },
        Op::Submit {
            argv,
            dir,
            declared,
            session,
            send,
        } => {
            let dir = match dir {
                Some(dir) => {
                    let dir = expand(&dir);
                    std::fs::create_dir_all(&dir)
                        .map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
                    dir
                }
                None if send => {
                    fresh_directory().map_err(|e| format!("cannot make a directory: {e}"))?
                }
                None => expand(Path::new("~")),
            };
            if send {
                let status = Command::new("tar")
                    .arg("-x")
                    .arg("-C")
                    .arg(&dir)
                    .stdin(Stdio::inherit())
                    .status()
                    .map_err(|e| format!("cannot run tar: {e}"))?;
                if !status.success() {
                    return Err(format!(
                        "tar could not unpack the files into {}",
                        dir.display()
                    ));
                }
            }
            let spec = crate::model::Spec {
                argv,
                cwd: dir.clone(),
                session,
                declared: Declared {
                    on: None,
                    send: Vec::new(),
                    fetch: Vec::new(),
                    ..*declared
                },
                queue: None,
            };
            match daemon(Request::Submit {
                spec: Box::new(spec),
                env: crate::client::env(),
                idempotency_key: None,
            })? {
                Response::Submitted { job } => {
                    let submitted = Submitted {
                        id: job.id,
                        attempt: job.attempt,
                        dir,
                    };
                    let _ = writeln!(
                        out,
                        "{}",
                        serde_json::to_string(&submitted).unwrap_or_default()
                    );
                }
                Response::Error { message } => return Err(message),
                other => return Err(format!("unexpected answer {other:?}")),
            }
        }
        Op::FollowStreams { id, attempt } => {
            let store = Store {
                root: Store::default_root(),
            };
            let mut reader = crate::streams::reader::Reader::open(&store, id, Some(attempt))
                .map_err(|e| e.to_string())?;
            if reader.mode != Some(crate::streams::Mode::Pipe) {
                return Err(crate::streams::invalid().to_string());
            }
            loop {
                let batch = reader.read().map_err(|e| e.to_string())?;
                for record in &batch.records {
                    out.write_all(&record.encode().map_err(|e| e.to_string())?)
                        .and_then(|_| out.flush())
                        .map_err(|e| e.to_string())?;
                }
                if let Some(error) = batch.error {
                    return Err(error);
                }
                if batch.complete && batch.job.state.terminal() {
                    let end = crate::streams::Record {
                        sequence: 0,
                        at_ms: 0,
                        source: crate::streams::Source::End,
                        bytes: vec![0],
                        gap_stream: None,
                    };
                    out.write_all(&end.encode().map_err(|e| e.to_string())?)
                        .and_then(|_| out.flush())
                        .map_err(|e| e.to_string())?;
                    return Ok(ExitCode::SUCCESS);
                }
                if batch.records.is_empty() {
                    std::thread::sleep(FOLLOW_POLL);
                }
            }
        }
        Op::Follow { id, mut offset } => loop {
            let job = match daemon(Request::Status { id })? {
                Response::Finished { job } | Response::StillRunning { job } => job,
                Response::Error { message } => return Err(message),
                other => return Err(format!("unexpected answer {other:?}")),
            };
            if let Ok(mut file) = std::fs::File::open(&job.log) {
                use std::io::Seek;
                if file.seek(io::SeekFrom::Start(offset)).is_ok() {
                    let mut buffer = Vec::new();
                    if file.read_to_end(&mut buffer).is_ok() && !buffer.is_empty() {
                        offset += buffer.len() as u64;
                        if out.write_all(&buffer).and_then(|_| out.flush()).is_err() {
                            return Ok(ExitCode::from(1));
                        }
                        continue;
                    }
                }
            }
            if job.state.terminal() {
                return Ok(ExitCode::from(exit_of(&job)));
            }
            std::thread::sleep(FOLLOW_POLL);
        },
        Op::AttemptStatus { id, attempt } => {
            let store = Store {
                root: Store::default_root(),
            };
            let job = crate::attempts::list(&store, id)
                .map_err(|e| e.to_string())?
                .into_iter()
                .find(|job| job.attempt == attempt)
                .ok_or_else(|| crate::lifecycle::message("no such attempt"))?;
            writeln!(
                out,
                "{}",
                serde_json::to_string(&job).map_err(|e| e.to_string())?
            )
            .map_err(|e| e.to_string())?;
        }
        Op::Status { id } => match daemon(Request::Status { id })? {
            Response::Finished { job } | Response::StillRunning { job } => {
                let _ = writeln!(out, "{}", serde_json::to_string(&job).unwrap_or_default());
            }
            Response::Error { message } => return Err(message),
            other => return Err(format!("unexpected answer {other:?}")),
        },
        Op::Cancel { id, session } => match daemon(Request::Cancel { id, session })? {
            Response::Cancelled { .. } => {}
            Response::Cancellation { operation } => {
                return Ok(crate::cancellation_result(&operation, false));
            }
            Response::Error { message } => return Err(message),
            other => return Err(format!("unexpected answer {other:?}")),
        },
        Op::Fetch { dir, paths } => {
            let status = Command::new("tar")
                .arg("-c")
                .arg("-C")
                .arg(&dir)
                .arg("--")
                .args(&paths)
                .stdout(Stdio::inherit())
                .status()
                .map_err(|e| format!("cannot run tar: {e}"))?;
            if !status.success() {
                return Err(format!(
                    "tar could not pack {:?} in {}",
                    paths,
                    dir.display()
                ));
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
