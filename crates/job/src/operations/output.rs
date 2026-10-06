use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::model::{Request, Response};
use crate::store::Store;
use crate::streams::quota::Quota;
use crate::streams::reader::Reader;
use crate::streams::{HEADER, Mode, Record, Source};

use super::message;

const META: &[u8; 4] = b"JOM1";
const RECORD: &[u8; 4] = b"JOL1";
const META_LIMIT: usize = 1 << 20;
const POLL: Duration = Duration::from_millis(25);
const HEARTBEAT: Duration = Duration::from_secs(1);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Serialize, Deserialize)]
enum Meta {
    Gap {
        sequence: u64,
        at_ms: u64,
        records: u64,
        bytes: u64,
        stream: Option<Source>,
    },
    Batch {
        complete: bool,
        terminal: bool,
        exit_status: u8,
        error: Option<String>,
        quota: Option<Quota>,
        trimmed: Option<u64>,
    },
}

pub struct Step {
    pub records: Vec<Record>,
    pub complete: bool,
    pub terminal: bool,
    pub exit_status: u8,
    pub error: Option<String>,
    pub quota: Option<Quota>,
    pub trimmed: Option<u64>,
}

pub struct Served {
    stream: BufReader<UnixStream>,
    attempt: u64,
    mode: Option<Mode>,
}

pub enum Feed {
    Direct(Box<Reader>),
    Served(Served),
}

fn state_readable(store: &Store) -> bool {
    std::ffi::CString::new(store.root.join("jobs").as_os_str().as_encoded_bytes())
        .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::R_OK | libc::X_OK) } == 0)
}

fn store() -> Result<Store, String> {
    Store::validate_default_selection().map_err(|error| error.to_string())?;
    Ok(Store {
        root: Store::default_root(),
    })
}

fn connect(request: &Request) -> Result<BufReader<UnixStream>, String> {
    let socket = crate::socket();
    let unreachable = |error: io::Error| {
        message(
            "the output of this Job is served by the service, which does not answer at {socket}: {error}",
            &[
                ("socket", socket.display().to_string()),
                ("error", error.to_string()),
            ],
        )
    };
    let mut stream = UnixStream::connect(&socket).map_err(unreachable)?;
    let mut bytes = serde_json::to_vec(&crate::durability::negotiation::envelope(request))
        .map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    stream.write_all(&bytes).map_err(unreachable)?;
    Ok(BufReader::new(stream))
}

fn answer(stream: &mut BufReader<UnixStream>) -> Result<Response, String> {
    let mut line = String::new();
    stream
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    serde_json::from_str::<Response>(&line)
        .map(crate::durability::negotiation::understood)
        .map_err(|_| {
            message(
                "the service does not serve output; restart it from the same release as this command",
                &[],
            )
        })
}

impl Served {
    fn open(id: u64, attempt: Option<u64>, follow: bool) -> Result<Self, String> {
        let mut stream = connect(&Request::Output {
            id,
            attempt,
            follow,
        })?;
        match answer(&mut stream)? {
            Response::Output { attempt, mode, .. } => Ok(Self {
                stream,
                attempt,
                mode,
            }),
            Response::Error { message } => Err(message),
            _ => Err(message(
                "the service does not serve output; restart it from the same release as this command",
                &[],
            )),
        }
    }

    fn read(&mut self) -> io::Result<Step> {
        let mut records = Vec::new();
        loop {
            let mut magic = [0u8; 4];
            self.stream.read_exact(&mut magic)?;
            if &magic == RECORD {
                let mut header = [0u8; HEADER];
                header[..4].copy_from_slice(&magic);
                self.stream.read_exact(&mut header[4..])?;
                let mut bytes = vec![0; Record::length(&header)?];
                self.stream.read_exact(&mut bytes)?;
                records.push(Record::decode(&header, bytes)?);
                continue;
            }
            if &magic != META {
                return Err(crate::streams::invalid());
            }
            let mut length = [0u8; 4];
            self.stream.read_exact(&mut length)?;
            let length = u32::from_le_bytes(length) as usize;
            if length > META_LIMIT {
                return Err(crate::streams::invalid());
            }
            let mut bytes = vec![0; length];
            self.stream.read_exact(&mut bytes)?;
            match serde_json::from_slice::<Meta>(&bytes)? {
                Meta::Gap {
                    sequence,
                    at_ms,
                    records: lost,
                    bytes,
                    stream,
                } => {
                    let mut record = Record::gap(sequence, at_ms, lost, bytes);
                    record.gap_stream = stream;
                    records.push(record);
                }
                Meta::Batch {
                    complete,
                    terminal,
                    exit_status,
                    error,
                    quota,
                    trimmed,
                } => {
                    return Ok(Step {
                        records,
                        complete,
                        terminal,
                        exit_status,
                        error,
                        quota,
                        trimmed,
                    });
                }
            }
        }
    }
}

impl Feed {
    pub fn open(id: u64, attempt: Option<u64>, follow: bool) -> Result<Self, String> {
        let store = store()?;
        if !state_readable(&store) {
            return Served::open(id, attempt, follow).map(Self::Served);
        }
        let mut reader = Reader::open(&store, id, attempt).map_err(|error| error.to_string())?;
        if !follow {
            reader.snapshot().map_err(|error| error.to_string())?;
        }
        Ok(Self::Direct(Box::new(reader)))
    }

    pub fn served(&self) -> bool {
        matches!(self, Self::Served(_))
    }

    pub fn mode(&self) -> Option<Mode> {
        match self {
            Self::Direct(reader) => reader.mode,
            Self::Served(served) => served.mode,
        }
    }

    pub fn attempt(&self) -> u64 {
        match self {
            Self::Direct(reader) => reader.attempt,
            Self::Served(served) => served.attempt,
        }
    }

    pub fn read(&mut self) -> Result<Step, String> {
        match self {
            Self::Direct(reader) => {
                let batch = reader.read().map_err(|error| error.to_string())?;
                Ok(Step {
                    records: batch.records,
                    complete: batch.complete,
                    terminal: batch.job.state.terminal(),
                    exit_status: crate::cli_contract::exit_status(&batch.job),
                    error: batch.error,
                    quota: batch.quota,
                    trimmed: batch.trimmed,
                })
            }
            Self::Served(served) => served.read().map_err(|error| {
                message(
                    "the service stopped serving the output: {error}",
                    &[("error", error.to_string())],
                )
            }),
        }
    }
}

fn line(out: &mut UnixStream, response: &Response) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(response)?;
    bytes.push(b'\n');
    out.write_all(&bytes)
}

fn meta(out: &mut UnixStream, meta: &Meta) -> io::Result<()> {
    let body = serde_json::to_vec(meta)?;
    let mut bytes = META.to_vec();
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&body);
    out.write_all(&bytes)
}

fn stream(
    store: &Store,
    id: u64,
    attempt: Option<u64>,
    follow: bool,
    out: &mut UnixStream,
) -> io::Result<()> {
    let opened = Reader::open(store, id, attempt).and_then(|mut reader| {
        if !follow {
            reader.snapshot()?;
        }
        Ok(reader)
    });
    let mut reader = match opened {
        Ok(reader) => reader,
        Err(error) => {
            return line(
                out,
                &Response::Error {
                    message: error.to_string(),
                },
            );
        }
    };
    line(
        out,
        &Response::Output {
            attempt: reader.attempt,
            mode: reader.mode,
            output_protocol: crate::streams::PROTOCOL,
        },
    )?;
    let mut told = Instant::now();
    loop {
        let batch = match reader.read() {
            Ok(batch) => batch,
            Err(error) => {
                return meta(
                    out,
                    &Meta::Batch {
                        complete: true,
                        terminal: true,
                        exit_status: crate::EXIT_SERVICE_ERROR,
                        error: Some(error.to_string()),
                        quota: None,
                        trimmed: None,
                    },
                );
            }
        };
        for record in &batch.records {
            if record.source == Source::Gap {
                let (records, bytes) = record.gap_counts();
                meta(
                    out,
                    &Meta::Gap {
                        sequence: record.sequence,
                        at_ms: record.at_ms,
                        records,
                        bytes,
                        stream: record.gap_stream,
                    },
                )?;
            } else {
                out.write_all(&record.encode()?)?;
            }
        }
        let terminal = batch.job.state.terminal();
        let done = batch.error.is_some() || (batch.complete && (!follow || terminal));
        if done
            || !batch.records.is_empty()
            || batch.trimmed.is_some()
            || told.elapsed() >= HEARTBEAT
        {
            told = Instant::now();
            meta(
                out,
                &Meta::Batch {
                    complete: batch.complete,
                    terminal,
                    exit_status: crate::cli_contract::exit_status(&batch.job),
                    error: batch.error,
                    quota: batch.quota,
                    trimmed: batch.trimmed,
                },
            )?;
            out.flush()?;
        }
        if done {
            return Ok(());
        }
        if batch.records.is_empty() {
            std::thread::sleep(POLL);
        }
    }
}

fn selected(store: &Store, id: u64, attempt: Option<u64>) -> Result<crate::model::Job, String> {
    match attempt {
        Some(number) => crate::attempts::list(store, id)
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|job| job.attempt == number)
            .ok_or_else(|| crate::lifecycle::message("no such attempt")),
        None => store
            .load_job(id)
            .ok_or_else(|| format!("there is no job {id}")),
    }
}

pub fn wanted(request: &Request) -> bool {
    match request {
        Request::Versioned { request, .. } => wanted(request),
        Request::Output { .. } | Request::LogQuery { .. } => true,
        _ => false,
    }
}

pub fn serve(store: &Store, request: Request, mut out: UnixStream) -> io::Result<()> {
    out.set_write_timeout(Some(WRITE_TIMEOUT))?;
    match request {
        Request::Output {
            id,
            attempt,
            follow,
        } => stream(store, id, attempt, follow, &mut out),
        Request::LogQuery { id, attempt, query } => {
            line(&mut out, &queried(store, id, attempt, &query))
        }
        _ => Ok(()),
    }
}

pub fn queried(store: &Store, id: u64, attempt: Option<u64>, query: &[String]) -> Response {
    match selected(store, id, attempt).and_then(|job| crate::query::run(&job, query)) {
        Ok(text) => Response::Done { line: text },
        Err(message) => Response::Error { message },
    }
}

pub fn log_command(rest: &[String]) -> Result<ExitCode, String> {
    let id = crate::parse_id(rest.first())?;
    let store = store()?;
    let (attempt, query) = if rest.get(1).is_some_and(|word| word == "--attempt") {
        (Some(crate::parse_id(rest.get(2))?), &rest[3..])
    } else {
        (None, &rest[1..])
    };
    let text = if state_readable(&store) {
        crate::query::run(&selected(&store, id, attempt)?, query)?
    } else {
        let mut stream = connect(&Request::LogQuery {
            id,
            attempt,
            query: query.to_vec(),
        })?;
        match answer(&mut stream)? {
            Response::Done { line } => line,
            Response::Error { message } => return Err(message),
            _ => {
                return Err(message(
                    "the service does not serve output; restart it from the same release as this command",
                    &[],
                ));
            }
        }
    };
    println!("{text}");
    Ok(ExitCode::SUCCESS)
}
