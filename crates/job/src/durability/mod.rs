use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::model::{Request, Response};
use crate::store::Store;

pub mod audit;
pub mod boot;
pub mod cli;
pub mod exit;
pub mod failpoint;
pub mod idempotency;
pub mod launch;
mod messages;
pub mod negotiation;
pub mod peer;
pub use failpoint::failpoint;
pub use messages::message;

pub fn sweep_transactions(store: &Store) -> io::Result<usize> {
    let directory = store.root.join(".transactions");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
        removed += 1;
    }
    if removed > 0 {
        fs::File::open(&directory)?.sync_all()?;
    }
    Ok(removed)
}

pub fn swept(result: io::Result<usize>) {
    match result {
        Ok(0) => {}
        Ok(count) => eprintln!(
            "job daemon: {}: {count}",
            message("removed leftover temporary files of the idempotency index")
        ),
        Err(error) => eprintln!(
            "job daemon: {}: {error}",
            message("cannot remove leftover temporary files of the idempotency index")
        ),
    }
}

pub fn exchange(line: &[u8], handler: impl FnOnce(Request) -> Response) -> Response {
    let request = match negotiation::receive(line) {
        Ok(request) => request,
        Err(negotiation::Refusal::Unreadable(response)) => {
            if !line.iter().all(u8::is_ascii_whitespace) {
                audit::unreadable(line);
            }
            return *response;
        }
        Err(negotiation::Refusal::Unknown(name, response)) => {
            audit::unknown(&name);
            return *response;
        }
        Err(negotiation::Refusal::Unsupported(request, response)) => {
            audit::record(request.as_deref().and_then(audit::describe), &response);
            return *response;
        }
    };
    crate::pacing::request_begins();
    let begun = match audit::begin(audit::describe(&request)) {
        Ok(begun) => begun,
        Err(error) => {
            return Response::Error {
                message: format!(
                    "{}: {error}",
                    message(
                        "the service cannot write its audit journal, so it changes nothing; free space or repair the audit directory in the state directory and try again"
                    )
                ),
            };
        }
    };
    let response = handler(request);
    if let Some(action) = audit::action(&begun) {
        crate::pacing::request_spent(action);
    }
    if begun.is_some() {
        failpoint("audit-before-end");
    }
    audit::end(begun, &response);
    response
}

fn unset(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_digest_version: Option<u32>,
    #[serde(default, skip_serializing_if = "unset")]
    pub replayed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_uid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_pid: Option<i32>,
    #[serde(default, skip_serializing_if = "unset")]
    pub launch_gated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admitted_ms: Option<u64>,
}

pub fn write_in_boot<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let temporary = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(value)?)?;
    }
    fs::rename(&temporary, path)
}
