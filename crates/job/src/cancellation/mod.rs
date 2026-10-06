use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::model::{Job, State};
use crate::objects::{Graph, Kind};
use crate::store::{Store, write_json};

mod messages;
pub use messages::message;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    Job { id: u64 },
    Object { kind: Kind, path: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub id: u64,
    pub attempt: u64,
    pub queue_id: Option<u64>,
    pub state: State,
}

impl From<&Job> for Member {
    fn from(job: &Job) -> Self {
        Self {
            id: job.id,
            attempt: job.attempt,
            queue_id: job.queue_id,
            state: job.state.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub scope_id: Option<u64>,
    pub members: Vec<Member>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<String>,
}

impl Selection {
    pub fn operation_id(&self) -> String {
        let identities: Vec<_> = self.members.iter().map(|m| (m.id, m.attempt)).collect();
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(self.scope_id, identities)).unwrap())
        )
    }

    fn valid(&self) -> bool {
        self.members
            .iter()
            .all(|m| m.id != 0 && m.attempt != 0 && !m.state.terminal())
            && self.members.windows(2).all(|w| w[0].id < w[1].id)
            && (self.scope_id.is_some_and(|id| id != 0)
                || self.members.len() == 1
                || (self.set.is_some() && !self.members.is_empty()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub id: u64,
    pub attempt: u64,
    pub state: Option<State>,
    pub pending: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    pub schema_version: u32,
    pub operation: String,
    pub actor_uid: u32,
    pub requested_ms: u64,
    pub selection: Selection,
    pub complete: bool,
    pub results: Vec<Outcome>,
    pub persistence_error: Option<String>,
}

impl Operation {
    pub fn new(selection: Selection, now: u64) -> Self {
        Self {
            schema_version: 1,
            operation: selection.operation_id(),
            actor_uid: crate::service::access::actor_uid(),
            requested_ms: now,
            selection,
            complete: false,
            results: Vec::new(),
            persistence_error: None,
        }
    }
}

fn invalid() -> io::Error {
    io::Error::other(message("invalid cancellation journal"))
}

fn directory(store: &Store, create: bool) -> io::Result<bool> {
    let path = store.root.join("cancellations");
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_dir() => Ok(true),
        Ok(_) => Err(invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
            fs::DirBuilder::new().mode(0o700).create(&path)?;
            fs::File::open(&store.root)?.sync_all()?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn save(store: &Store, operation: &Operation) -> io::Result<()> {
    directory(store, true)?;
    if operation.operation != operation.selection.operation_id() || !operation.selection.valid() {
        return Err(invalid());
    }
    write_json(
        &store
            .root
            .join("cancellations")
            .join(format!("{}.json", operation.operation)),
        operation,
    )
}

pub fn load(store: &Store) -> io::Result<BTreeMap<String, Operation>> {
    let mut operations = BTreeMap::new();
    if !directory(store, false)? {
        return Ok(operations);
    }
    for entry in fs::read_dir(store.root.join("cancellations"))? {
        let entry = entry?;
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|ext| ext.to_str().is_some_and(|ext| ext.starts_with("tmp")))
        {
            continue;
        }
        if !entry.file_type()?.is_file() || path.extension().is_none_or(|ext| ext != "json") {
            return Err(invalid());
        }
        let operation: Operation = serde_json::from_slice(&fs::read(&path)?)?;
        if operation.schema_version != 1
            || !operation.selection.valid()
            || operation.operation != operation.selection.operation_id()
            || entry.file_name() != format!("{}.json", operation.operation).as_str()
            || (operation.complete
                && (operation.results.len() != operation.selection.members.len()
                    || operation.results.iter().any(|result| result.pending)
                    || operation.persistence_error.is_some()))
            || !operation
                .results
                .iter()
                .zip(&operation.selection.members)
                .all(|(r, m)| r.id == m.id && r.attempt == m.attempt)
        {
            return Err(invalid());
        }
        operations.insert(operation.operation.clone(), operation);
    }
    Ok(operations)
}

pub fn select(
    store: &Store,
    graph: &Graph,
    target: &Target,
    recursive: bool,
) -> io::Result<Selection> {
    let scope_id = match target {
        Target::Job { .. } => None,
        Target::Object { kind, path } => {
            if !recursive {
                return Err(io::Error::other(message(
                    "collection cancellation requires --recursive",
                )));
            }
            let id = graph.resolve(path).map_err(io::Error::other)?;
            if graph.nodes[&id].kind != *kind {
                return Err(io::Error::other(message("no such cancellation target")));
            }
            Some(id)
        }
    };
    let ids = match target {
        Target::Job { id } => vec![*id],
        Target::Object { .. } => store.job_ids(),
    };
    let mut members = Vec::new();
    for id in ids {
        let job = store
            .load_job(id)
            .ok_or_else(|| io::Error::other(message("invalid Job record")))?;
        if !job.state.terminal()
            && scope_id
                .is_none_or(|scope| job.queue_id.is_some_and(|queue| graph.within(queue, scope)))
        {
            members.push(Member::from(&job));
        }
    }
    members.sort_by_key(|member| member.id);
    if scope_id.is_none() && members.is_empty() {
        return Err(io::Error::other(message("Job has already finished")));
    }
    Ok(Selection {
        scope_id,
        members,
        set: None,
    })
}

pub fn validate_selection(
    store: &Store,
    graph: &Graph,
    target: &Target,
    recursive: bool,
    expected: &Selection,
) -> io::Result<()> {
    if !expected.valid() {
        return Err(invalid());
    }
    match target {
        Target::Job { id }
            if expected.scope_id.is_none()
                && expected.members.len() == 1
                && expected.members[0].id == *id => {}
        Target::Object { kind, path } if recursive => {
            let id = graph.resolve(path).map_err(io::Error::other)?;
            if Some(id) != expected.scope_id || graph.nodes[&id].kind != *kind {
                return Err(io::Error::other(message("cancellation selection changed")));
            }
        }
        _ => return Err(io::Error::other(message("cancellation selection changed"))),
    }
    for member in &expected.members {
        let job = store
            .load_job(member.id)
            .ok_or_else(|| io::Error::other(message("cancellation selection changed")))?;
        if job.attempt != member.attempt
            || job.queue_id != member.queue_id
            || expected
                .scope_id
                .is_some_and(|scope| !job.queue_id.is_some_and(|queue| graph.within(queue, scope)))
        {
            return Err(io::Error::other(message("cancellation selection changed")));
        }
    }
    Ok(())
}
