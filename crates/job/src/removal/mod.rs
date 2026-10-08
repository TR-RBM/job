use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

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
pub struct JobSelection {
    pub id: u64,
    pub attempt: u64,
    pub state: State,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectSelection {
    pub id: u64,
    pub kind: Kind,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub jobs: Vec<JobSelection>,
    pub objects: Vec<ObjectSelection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub job_id: Option<u64>,
    pub pending: bool,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preview {
    pub schema_version: u32,
    pub selection: Selection,
    pub helpers: Vec<String>,
    pub unattributed_history_samples_retained: usize,
    pub ready: bool,
    pub issues: Vec<Issue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub schema_version: u32,
    pub operation: String,
    pub actor_uid: u32,
    pub requested_ms: u64,
    pub selection: Selection,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Template {
    hash: String,
    id: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Transaction {
    receipt: Receipt,
    graph: Option<Graph>,
    templates: Vec<Template>,
    links: Vec<crate::link::Link>,
}

pub struct Prepared {
    pub preview: Preview,
    transaction: Transaction,
}

fn error(text: &str) -> io::Error {
    io::Error::other(message(text))
}

fn issue(job_id: Option<u64>, pending: bool, text: &str) -> Issue {
    Issue {
        job_id,
        pending,
        message: message(text),
    }
}

fn process_alive(job: &Job, boot: &str) -> io::Result<bool> {
    if job
        .supervisor_boot_id
        .as_deref()
        .is_some_and(|value| value != boot)
    {
        return Ok(false);
    }
    let Some(pid) = job.shim_pid else {
        return Ok(false);
    };
    match crate::process::Handle::open(pid, job.shim_start_ticks) {
        Ok(handle) => handle.alive(),
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(false),
        Err(error) => Err(error),
    }
}

fn cgroup_busy(job: &Job) -> io::Result<bool> {
    let Some(path) = &job.workload_cgroup else {
        return Ok(false);
    };
    let text = match fs::read_to_string(path.join("cgroup.events")) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        if fields.next() == Some("populated") {
            return match fields.next() {
                Some("0") => Ok(false),
                Some("1") => Ok(true),
                _ => Err(error("invalid workload cgroup state")),
            };
        }
    }
    Err(error("invalid workload cgroup state"))
}

fn examine(
    store: &Store,
    job: &Job,
    allow_lost: bool,
    boot: &str,
) -> io::Result<(Vec<Issue>, Vec<Template>)> {
    let mut issues = Vec::new();
    let mut templates = Vec::new();
    if !job.state.terminal() {
        issues.push(issue(
            Some(job.id),
            false,
            "only completed Jobs can be removed; cancel separately",
        ));
        return Ok((issues, templates));
    }
    check_tree(&store.job_dir(job.id))?;
    for attempt in crate::attempts::list(store, job.id)? {
        if attempt.state == State::Lost && !allow_lost {
            issues.push(issue(
                Some(job.id),
                false,
                "lost work may remain; removal requires --allow-lost",
            ));
        }
        if process_alive(&attempt, boot)? {
            issues.push(issue(
                Some(job.id),
                true,
                "previous supervisor has not exited",
            ));
        }
        if cgroup_busy(&attempt)? {
            issues.push(issue(
                Some(job.id),
                false,
                "workload cgroup still contains processes",
            ));
        }
        templates.push(Template {
            hash: crate::templates::key_hash(&attempt.key),
            id: job.id,
        });
    }
    Ok((issues, templates))
}

pub fn prepare_set(
    store: &Store,
    jobs: Vec<Job>,
    allow_lost: bool,
) -> io::Result<(Option<Prepared>, Vec<Issue>)> {
    let boot = crate::process::boot_id()?;
    let mut issues = Vec::new();
    let mut selected = Vec::new();
    let mut templates = Vec::new();
    for job in jobs {
        match examine(store, &job, allow_lost, &boot) {
            Ok((found, known)) if found.is_empty() => {
                selected.push(JobSelection {
                    id: job.id,
                    attempt: job.attempt,
                    state: job.state.clone(),
                });
                templates.extend(known);
            }
            Ok((found, _)) => issues.extend(found),
            Err(error) => issues.push(Issue {
                job_id: Some(job.id),
                pending: false,
                message: error.to_string(),
            }),
        }
    }
    selected.sort_by_key(|job| job.id);
    let Some(first) = selected.first() else {
        return Ok((None, issues));
    };
    let selection = Selection {
        jobs: selected.clone(),
        objects: Vec::new(),
    };
    let receipt = Receipt {
        schema_version: 1,
        operation: format!("job-{}", first.id),
        actor_uid: crate::service::access::actor_uid(),
        requested_ms: crate::shim::now_ms(),
        selection: selection.clone(),
    };
    Ok((
        Some(Prepared {
            preview: Preview {
                schema_version: 1,
                selection,
                helpers: Vec::new(),
                unattributed_history_samples_retained: store
                    .load_history()
                    .iter()
                    .filter(|entry| entry.job_id.is_none())
                    .count(),
                ready: true,
                issues: Vec::new(),
            },
            transaction: Transaction {
                receipt,
                graph: None,
                templates,
                links: Vec::new(),
            },
        }),
        issues,
    ))
}

pub fn prepare(
    store: &Store,
    graph: &Graph,
    target: &Target,
    recursive: bool,
    allow_lost: bool,
) -> io::Result<Prepared> {
    let mut selected_objects = Vec::new();
    let mut issues = Vec::new();
    let (operation, jobs, next_graph) = match target {
        Target::Job { id } => (
            format!("job-{id}"),
            vec![
                store
                    .load_job(*id)
                    .ok_or_else(|| error("no such removal target"))?,
            ],
            None,
        ),
        Target::Object { kind, path } => {
            let id = graph.resolve(path).map_err(io::Error::other)?;
            if graph.nodes[&id].kind != *kind {
                return Err(error("no such removal target"));
            }
            if [crate::objects::ROOT, crate::objects::DEFAULT_QUEUE].contains(&id) {
                return Err(error("root Group and default Queue cannot be removed"));
            }
            let members: Vec<_> = graph
                .nodes
                .values()
                .filter(|node| graph.within(node.id, id))
                .map(|node| node.id)
                .collect();
            let jobs = store
                .job_ids()
                .into_iter()
                .map(|id| {
                    store
                        .load_job(id)
                        .ok_or_else(|| error("invalid Job record"))
                })
                .collect::<io::Result<Vec<_>>>()?
                .into_iter()
                .filter(|job| job.queue_id.is_some_and(|queue| graph.within(queue, id)))
                .collect::<Vec<_>>();
            if !recursive && (members.len() > 1 || !jobs.is_empty()) {
                issues.push(issue(
                    None,
                    false,
                    "nonempty containers require --recursive; removal never cancels work",
                ));
            }
            let mut next = graph.clone();
            let mut order = members;
            order.sort_by_key(|id| std::cmp::Reverse(graph.ancestors(*id).len()));
            for member in order {
                selected_objects.push(ObjectSelection {
                    id: member,
                    kind: graph.nodes[&member].kind,
                    path: graph.path(member),
                });
                next.change(
                    graph.nodes[&member].kind,
                    &crate::objects::Operation::Remove {
                        path: graph.path(member),
                    },
                )
                .map_err(io::Error::other)?;
            }
            selected_objects.sort_by_key(|object| object.id);
            (format!("object-{id}"), jobs, Some(next))
        }
    };
    let boot = crate::process::boot_id()?;
    let mut selected_jobs = Vec::new();
    let mut templates = Vec::new();
    for job in jobs {
        selected_jobs.push(JobSelection {
            id: job.id,
            attempt: job.attempt,
            state: job.state.clone(),
        });
        let (found, known) = examine(store, &job, allow_lost, &boot)?;
        issues.extend(found);
        templates.extend(known);
    }
    selected_jobs.sort_by_key(|job| job.id);
    let links: Vec<_> = store
        .load_links()
        .into_iter()
        .filter(|link| {
            selected_objects
                .iter()
                .any(|object| object.kind == Kind::Queue && object.path == link.name)
        })
        .collect();
    if links
        .iter()
        .any(|link| link.holder.boot_id.is_none() || link.relay.boot_id.is_none())
    {
        issues.push(issue(None, false, "network helper has no boot identity"));
    }
    let selection = Selection {
        jobs: selected_jobs,
        objects: selected_objects,
    };
    let receipt = Receipt {
        schema_version: 1,
        operation,
        actor_uid: crate::service::access::actor_uid(),
        requested_ms: crate::shim::now_ms(),
        selection: selection.clone(),
    };
    Ok(Prepared {
        preview: Preview {
            schema_version: 1,
            selection,
            helpers: links.iter().map(|link| link.name.clone()).collect(),
            unattributed_history_samples_retained: store
                .load_history()
                .iter()
                .filter(|entry| entry.job_id.is_none())
                .count(),
            ready: issues.is_empty(),
            issues,
        },
        transaction: Transaction {
            receipt,
            graph: next_graph,
            templates,
            links,
        },
    })
}

fn check_tree(path: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(error("unexpected entry in removal data"));
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            check_tree(&entry.path())?;
        } else if !kind.is_file() && !(kind.is_socket() && entry.file_name() == "terminal.sock") {
            return Err(error("unexpected entry in removal data"));
        }
    }
    Ok(())
}

fn journal(store: &Store) -> std::path::PathBuf {
    store.root.join("removals/pending.json")
}

fn private_directory(path: &Path) -> io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(error)
            if error.kind() == io::ErrorKind::AlreadyExists
                && fs::symlink_metadata(path)?.is_dir() =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn validate_parents(store: &Store, path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| error("unexpected entry in removal data"))?;
    let relative = parent.strip_prefix(&store.root).map_err(io::Error::other)?;
    let mut current = store.root.clone();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(error("unexpected entry in removal data"));
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(error("unexpected entry in removal data")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub fn pending(store: &Store) -> bool {
    fs::symlink_metadata(journal(store)).is_ok()
}

pub fn pending_receipt(store: &Store) -> io::Result<Receipt> {
    let transaction: Transaction = serde_json::from_slice(&fs::read(journal(store))?)?;
    validate_transaction(&transaction)?;
    Ok(transaction.receipt)
}

pub fn receipts(store: &Store) -> io::Result<Vec<Receipt>> {
    validate_parents(store, &journal(store))?;
    let entries = match fs::read_dir(store.root.join("removals")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut receipts = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_name() == "pending.json"
            || entry
                .path()
                .extension()
                .is_none_or(|extension| extension != "json")
        {
            continue;
        }
        if !entry.file_type()?.is_file() {
            return Err(error("invalid removal journal"));
        }
        let receipt: Receipt = serde_json::from_slice(&fs::read(entry.path())?)?;
        if entry.file_name() != format!("{}.json", receipt.operation).as_str() {
            return Err(error("invalid removal journal"));
        }
        validate_transaction(&Transaction {
            receipt: receipt.clone(),
            graph: None,
            templates: Vec::new(),
            links: Vec::new(),
        })?;
        receipts.push(receipt);
    }
    Ok(receipts)
}

pub fn highest_removed_id(store: &Store) -> io::Result<u64> {
    Ok(receipts(store)?
        .into_iter()
        .flat_map(|receipt| receipt.selection.jobs)
        .map(|job| job.id)
        .max()
        .unwrap_or(0))
}

pub fn commit(store: &Store, prepared: Prepared) -> io::Result<Receipt> {
    if !prepared.preview.ready || pending(store) {
        return Err(error("removal is not ready"));
    }
    let directory = store.root.join("removals");
    private_directory(&directory)?;
    let trash = store
        .root
        .join(".removed")
        .join(&prepared.transaction.receipt.operation);
    validate_parents(store, &trash)?;
    match fs::symlink_metadata(&trash) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
        Ok(_) => return Err(error("unexpected entry in removal data")),
    }
    sync(&store.root)?;
    write_json(&journal(store), &prepared.transaction)?;
    recover(store)?;
    Ok(prepared.transaction.receipt)
}

fn remove_file(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => fs::remove_file(path),
        Ok(_) => Err(error("unexpected entry in removal data")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn sync(path: &Path) -> io::Result<()> {
    crate::pacing::directory(path)
}

fn validate_transaction(transaction: &Transaction) -> io::Result<()> {
    let receipt = &transaction.receipt;
    let number = receipt
        .operation
        .strip_prefix("job-")
        .or_else(|| receipt.operation.strip_prefix("object-"));
    if receipt.schema_version != 1
        || number
            .and_then(|s| s.parse::<u64>().ok())
            .is_none_or(|id| id == 0)
        || receipt
            .selection
            .jobs
            .iter()
            .any(|job| job.id == 0 || job.attempt == 0 || !job.state.terminal())
        || transaction.templates.iter().any(|template| {
            template.hash.len() != 16
                || !template.hash.bytes().all(|c| c.is_ascii_hexdigit())
                || !receipt
                    .selection
                    .jobs
                    .iter()
                    .any(|job| job.id == template.id)
        })
        || transaction.links.iter().any(|link| {
            !Path::new(&link.name)
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
        })
    {
        return Err(error("invalid removal journal"));
    }
    if let Some(graph) = &transaction.graph {
        graph.validate().map_err(io::Error::other)?;
    }
    Ok(())
}

fn retire_jobs(store: &Store, transaction: &Transaction, trash: &Path) -> io::Result<()> {
    let mut moved = false;
    let result = move_jobs(store, transaction, trash, &mut moved);
    if moved {
        sync(&store.root.join("jobs"))?;
        sync(trash)?;
    }
    result
}

fn move_jobs(
    store: &Store,
    transaction: &Transaction,
    trash: &Path,
    moved: &mut bool,
) -> io::Result<()> {
    for selected in &transaction.receipt.selection.jobs {
        let source = store.job_dir(selected.id);
        validate_parents(store, &source)?;
        let retired = trash.join(selected.id.to_string());
        match fs::symlink_metadata(&source) {
            Ok(_) => {
                let job = store
                    .load_job(selected.id)
                    .ok_or_else(|| error("invalid Job record"))?;
                if job.attempt != selected.attempt || !job.state.terminal() {
                    return Err(error("removal selection changed"));
                }
                check_tree(&source)?;
                let source = std::ffi::CString::new(source.as_os_str().as_encoded_bytes())?;
                let destination = std::ffi::CString::new(retired.as_os_str().as_encoded_bytes())?;
                crate::rename::renameat2(&source, &destination, libc::RENAME_NOREPLACE)?;
                *moved = true;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn prune_history(store: &Store, ids: &BTreeSet<u64>) -> io::Result<()> {
    match fs::symlink_metadata(store.history_file()) {
        Ok(metadata) if !metadata.is_file() => {
            return Err(error("unexpected entry in removal data"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    let bytes = match fs::read(store.history_file()) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut retained = Vec::new();
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if line.iter().all(|byte| byte.is_ascii_whitespace()) {
            retained.extend_from_slice(line);
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(line)?;
        if !value
            .get("job_id")
            .and_then(|id| id.as_u64())
            .is_some_and(|id| ids.contains(&id))
        {
            retained.extend_from_slice(line);
        }
    }
    if retained != bytes {
        crate::store::write_atomic(&store.history_file(), &retained)?;
    }
    Ok(())
}

fn remove_links(store: &Store, links: &[crate::link::Link]) -> io::Result<()> {
    let boot = crate::process::boot_token()?;
    for link in links {
        validate_parents(store, &store.link_file(&link.name))?;
        for process in [link.relay, link.holder] {
            if process.boot_id.is_none() {
                return Err(error("network helper has no boot identity"));
            }
            if process.boot_id != Some(boot) {
                continue;
            }
            match crate::process::Handle::open(process.pid, Some(process.start_ticks)) {
                Ok(handle) => {
                    if handle.alive()? {
                        handle.signal(libc::SIGKILL)?;
                    }
                    if handle.alive()? {
                        return Err(error("network helper termination is pending"));
                    }
                }
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
                Err(error) => return Err(error),
            }
        }
        remove_file(&store.link_file(&link.name))?;
        remove_file(&store.link_log(&link.name))?;
        if let Some(parent) = store
            .link_file(&link.name)
            .parent()
            .filter(|path| path.exists())
        {
            sync(parent)?;
        }
    }
    Ok(())
}

pub fn recover(store: &Store) -> io::Result<bool> {
    let path = journal(store);
    validate_parents(store, &path)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() {
        return Err(error("invalid removal journal"));
    }
    let transaction: Transaction = serde_json::from_slice(&fs::read(&path)?)?;
    validate_transaction(&transaction)?;
    let trash = store
        .root
        .join(".removed")
        .join(&transaction.receipt.operation);
    private_directory(&store.root.join(".removed"))?;
    private_directory(&trash)?;
    sync(&store.root.join(".removed"))?;
    sync(&store.root)?;
    retire_jobs(store, &transaction, &trash)?;
    for job in &transaction.receipt.selection.jobs {
        crate::clientif::index::removed(job.id);
    }
    crate::durability::idempotency::forget(
        store,
        &transaction
            .receipt
            .selection
            .jobs
            .iter()
            .map(|job| job.id)
            .collect(),
    )?;
    remove_links(store, &transaction.links)?;
    if let Some(graph) = &transaction.graph {
        graph.save(store)?;
    }
    prune_history(
        store,
        &transaction
            .receipt
            .selection
            .jobs
            .iter()
            .map(|job| job.id)
            .collect(),
    )?;
    for template in &transaction.templates {
        let directory = store.root.join("templates").join(&template.hash);
        validate_parents(store, &directory.join("check"))?;
        remove_file(&directory.join(format!("{}.txt", template.id)))?;
        if directory.exists() {
            sync(&directory)?;
        }
    }
    check_tree(&trash)?;
    let selected: BTreeSet<String> = transaction
        .receipt
        .selection
        .jobs
        .iter()
        .map(|job| job.id.to_string())
        .collect();
    for entry in fs::read_dir(&trash)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir()
            || !entry
                .file_name()
                .to_str()
                .is_some_and(|name| selected.contains(name))
        {
            return Err(error("unexpected entry in removal data"));
        }
    }
    fs::remove_dir_all(&trash)?;
    sync(&store.root.join(".removed"))?;
    let receipt_path = store
        .root
        .join("removals")
        .join(format!("{}.json", transaction.receipt.operation));
    if receipt_path.exists() {
        let old: Receipt = serde_json::from_slice(&fs::read(&receipt_path)?)?;
        if old != transaction.receipt {
            return Err(error("removal receipt differs from journal"));
        }
    } else {
        write_json(&receipt_path, &transaction.receipt)?;
    }
    fs::remove_file(path)?;
    sync(&store.root.join("removals"))?;
    Ok(true)
}
