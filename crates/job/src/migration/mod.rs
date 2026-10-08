use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::model::Job;
use crate::objects::{DEFAULT_QUEUE, Graph, Kind};
use crate::store::{Store, write_json};
mod messages;
pub use messages::message;

pub const SCHEMA: u32 = 18;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schema {
    pub schema_version: u32,
    pub resource_vocabulary: String,
}

impl Default for Schema {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA,
            resource_vocabulary: "independent_resources_with_legacy_aliases".to_owned(),
        }
    }
}

#[derive(Serialize)]
pub struct Inventory {
    pub schema_version: Option<u32>,
    pub jobs: usize,
    pub active_jobs: Vec<u64>,
    pub active_helpers: Vec<i32>,
    pub legacy_jobs: usize,
    pub queues: usize,
    pub groups: usize,
    pub unbound_jobs: Vec<u64>,
    pub recovered_historical_queues: Vec<String>,
    pub legacy_service_required: bool,
    pub conversion_ready: bool,
    pub pending_cancellations: Vec<String>,
    pub pending_resource_updates: Vec<String>,
    pub planned_service_policy: Option<crate::config::Effective>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub leftover_temporary_files: Vec<String>,
}

struct Validated {
    inventory: Inventory,
    graph: Graph,
    jobs: Vec<Job>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    bytes: u64,
    sha256: Option<String>,
    mode: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    source: PathBuf,
    files: BTreeMap<String, Entry>,
}

pub fn ensure_schema(store: &Store) -> io::Result<()> {
    if let Some(schema) = schema(store)? {
        return if schema.schema_version == SCHEMA {
            Ok(())
        } else {
            Err(error("state schema requires an explicit offline migration"))
        };
    }
    if [
        "objects.json",
        "presets.json",
        "next-id",
        "history.jsonl",
        "links",
        "templates",
        "remote-work",
    ]
    .iter()
    .any(|name| store.root.join(name).exists())
        || !store.job_ids().is_empty()
        || !store.load_queues()?.is_empty()
    {
        return Err(error(
            "unversioned state requires an explicit offline migration",
        ));
    }
    write_json(&store.root.join("schema.json"), &Schema::default())
}

pub fn recorded(root: &Path) -> io::Result<Option<u32>> {
    let store = Store {
        root: root.to_path_buf(),
    };
    Ok(schema(&store)?.map(|schema| schema.schema_version))
}

fn schema(store: &Store) -> io::Result<Option<Schema>> {
    let bytes = match fs::read(store.root.join("schema.json")) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let value: Schema = serde_json::from_slice(&bytes)?;
    if ![
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, SCHEMA,
    ]
    .contains(&value.schema_version)
        || value.resource_vocabulary
            != if value.schema_version >= 7 {
                "independent_resources_with_legacy_aliases"
            } else {
                "legacy_declarations"
            }
    {
        return Err(error("unsupported state schema or resource vocabulary"));
    }
    Ok(Some(value))
}

fn validate(store: &Store) -> io::Result<Validated> {
    let presets = crate::presets::Registry::load(store)?;
    crate::admission::Ledger::load(store)?;
    crate::pressure::control::Ledger::load(store)?;
    crate::durability::audit::validate(store)?;
    crate::operations::journal::validate(store)?;
    crate::durability::idempotency::validate(store)?;
    let pending_resource_updates: Vec<_> = crate::resource_update::load(store)?
        .into_values()
        .filter(|op| op.pending())
        .map(|op| op.plan.operation)
        .collect();
    let pending_cancellations: Vec<_> = crate::cancellation::load(store)?
        .into_values()
        .filter(|op| !op.complete)
        .map(|op| op.operation)
        .collect();
    if crate::removal::pending(store) {
        return Err(io::Error::other(crate::removal::message(
            "a removal transaction is pending; restart the service to reconcile it",
        )));
    }
    let version = schema(store)?;
    let mut graph = Graph::load(store)?;
    let mut jobs = Vec::new();
    let mut unbound = Vec::new();
    let mut recovered_historical_queues = Vec::new();
    let mut entries = fs::read_dir(store.root.join("jobs"))?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if !entry.file_type()?.is_dir() {
            return Err(error("unexpected entry in the Jobs directory"));
        }
        let id: u64 = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| error("invalid Job directory name"))?;
        if id == 0 || entry.file_name() != id.to_string().as_str() {
            return Err(error("invalid Job directory name"));
        }
        let mut job: Job = serde_json::from_slice(&fs::read(entry.path().join("job.json"))?)?;
        if job.id != id {
            return Err(error("Job directory and record identities differ"));
        }
        for attempt in crate::attempts::list(store, id)? {
            let recording = if attempt.attempt == job.attempt {
                store.job_dir(id)
            } else {
                store
                    .job_dir(id)
                    .join("attempts")
                    .join(attempt.attempt.to_string())
            };
            crate::streams::validate(&attempt, &recording)?;
            crate::process_policy::validate_job(&attempt).map_err(io::Error::other)?;
            crate::security::validate_job(&attempt).map_err(io::Error::other)?;
            crate::isolation::validate_job(&attempt).map_err(io::Error::other)?;
            if let Some(snapshot) = &attempt.preset_snapshot {
                presets.validate_snapshot(snapshot)?;
            }
            attempt
                .spec
                .declared
                .resources
                .validate()
                .map_err(io::Error::other)?;
            if attempt.attempt != job.attempt
                && attempt.queue_id.is_some_and(|id| {
                    graph
                        .nodes
                        .get(&id)
                        .map(|node| node.kind)
                        .or_else(|| graph.retired.get(&id).map(|view| view.object.kind))
                        != Some(Kind::Queue)
                })
            {
                return Err(error("Job references a missing Queue identity"));
            }
        }
        if job.queue_id.is_none() {
            unbound.push(id);
            let path = job.spec.queue.as_deref().unwrap_or("default");
            let queue_id = if path == "default" {
                DEFAULT_QUEUE
            } else if let Ok(id) = graph.resolve(path) {
                id
            } else if let Some((&id, _)) = graph.retired.iter().find(|(_, view)| view.path == path)
            {
                id
            } else {
                let id = graph
                    .create(path, Kind::Queue, BTreeMap::new())
                    .map_err(io::Error::other)?;
                let node = graph.nodes.get_mut(&id).unwrap();
                node.closed = true;
                node.created_ms = job.submitted_ms;
                let view = graph.view(id);
                graph.nodes.remove(&id);
                graph.retired.insert(id, view);
                recovered_historical_queues.push(path.to_owned());
                id
            };
            job.queue_id = Some(queue_id);
        }
        let id = job.queue_id.unwrap();
        let kind = graph
            .nodes
            .get(&id)
            .map(|n| n.kind)
            .or_else(|| graph.retired.get(&id).map(|v| v.object.kind));
        if kind != Some(Kind::Queue) {
            return Err(error("Job references a missing Queue identity"));
        }
        if version.is_some() && unbound.last() == Some(&job.id) {
            return Err(error("versioned Job is missing its Queue identity"));
        }
        if graph.nodes.contains_key(&id) {
            job.spec.queue = Some(graph.path(id));
        }
        if job.state.editable() {
            let _: crate::model::Env = serde_json::from_slice(&fs::read(store.env_file(job.id))?)?;
        }
        if store.result_file(job.id).exists() {
            let _: crate::model::ShimResult =
                serde_json::from_slice(&fs::read(store.result_file(job.id))?)?;
        }
        jobs.push(job);
    }
    jobs.sort_by_key(|job| job.id);
    let next = match fs::read_to_string(store.root.join("next-id")) {
        Ok(text) => text.trim().parse::<u64>().map_err(io::Error::other)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound && jobs.is_empty() => 1,
        Err(e) => return Err(e),
    };
    if next == 0
        || next == u64::MAX
        || jobs.last().is_some_and(|job| job.id >= next)
        || crate::removal::highest_removed_id(store)? >= next
    {
        return Err(error(
            "next Job identity does not exceed all persisted identities",
        ));
    }
    let mut active_helpers = std::collections::BTreeSet::new();
    let links = store.root.join("links");
    if links.exists() {
        for entry in fs::read_dir(links)? {
            let entry = entry?;
            if entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                let link: crate::link::Link = serde_json::from_slice(&fs::read(entry.path())?)?;
                if entry.file_name() != format!("{}.json", link.name).as_str() {
                    return Err(error("network link file and record names differ"));
                }
                for process in [link.holder, link.relay] {
                    if process.boot_id.is_some()
                        && process.boot_id != crate::process::boot_token().ok()
                    {
                        continue;
                    }
                    match crate::process::Handle::open(process.pid, Some(process.start_ticks)) {
                        Ok(handle) => {
                            if handle.alive()? {
                                active_helpers.insert(process.pid);
                            }
                        }
                        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
        }
    }
    let active_jobs: Vec<_> = jobs
        .iter()
        .filter(|j| !j.state.terminal())
        .map(|j| j.id)
        .collect();
    let legacy_jobs = jobs.iter().filter(|j| j.policy.legacy()).count();
    let legacy_service_required = legacy_jobs != 0 || !store.load_queues()?.is_empty();
    graph.validate().map_err(io::Error::other)?;
    match fs::read_to_string(store.history_file()) {
        Ok(history) => {
            for line in history.lines().filter(|line| !line.trim().is_empty()) {
                let _: crate::model::HistoryEntry = serde_json::from_str(line)?;
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    Ok(Validated {
        inventory: Inventory {
            schema_version: version.map(|v| v.schema_version),
            jobs: jobs.len(),
            legacy_jobs,
            queues: graph
                .nodes
                .values()
                .filter(|n| n.kind == Kind::Queue)
                .count(),
            groups: graph
                .nodes
                .values()
                .filter(|n| n.kind == Kind::Group)
                .count(),
            unbound_jobs: unbound,
            recovered_historical_queues,
            legacy_service_required,
            conversion_ready: active_jobs.is_empty()
                && active_helpers.is_empty()
                && pending_cancellations.is_empty()
                && pending_resource_updates.is_empty(),
            pending_cancellations,
            pending_resource_updates,
            active_jobs,
            active_helpers: active_helpers.into_iter().collect(),
            planned_service_policy: None,
            leftover_temporary_files: crate::durability::idempotency::leftovers(store)?,
        },
        graph,
        jobs,
    })
}

pub fn validate_runtime(store: &Store) -> io::Result<()> {
    validate(store).map(|_| ())
}

fn error(message: &str) -> io::Error {
    io::Error::other(self::message(message))
}

fn source(path: &Path) -> io::Result<(Store, File)> {
    let root = fs::canonicalize(path)?;
    if fs::metadata(&root)?.uid() != unsafe { libc::geteuid() } {
        return Err(error("state must be owned by the current service user"));
    }
    let store = Store { root };
    let lock = crate::daemon::lock_exclusively(&store.lock())?;
    Ok((store, lock))
}

fn destination(path: &Path) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| error("destination requires a directory name"))?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let path = fs::canonicalize(parent)?.join(name);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(path),
        Err(e) => Err(e),
        Ok(_) => Err(error("destination already exists")),
    }
}

fn disjoint(paths: &[&Path]) -> io::Result<()> {
    for (index, path) in paths.iter().enumerate() {
        for other in paths.iter().skip(index + 1) {
            if path.starts_with(other) || other.starts_with(path) {
                return Err(error(
                    "source, backup and destination must be separate directories",
                ));
            }
        }
    }
    Ok(())
}

struct Staging(PathBuf);

impl Staging {
    fn new(destination: &Path) -> io::Result<Self> {
        let path = destination.with_file_name(format!(
            ".job-stage-{}-{}",
            std::process::id(),
            crate::shim::now_ms()
        ));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }

    fn publish(self, destination: &Path) -> io::Result<()> {
        let old = std::ffi::CString::new(self.0.as_os_str().as_encoded_bytes())?;
        let new = std::ffi::CString::new(destination.as_os_str().as_encoded_bytes())?;
        crate::rename::renameat2(&old, &new, libc::RENAME_NOREPLACE)?;
        File::open(destination.parent().unwrap())?.sync_all()
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn relative(path: &Path) -> io::Result<String> {
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(error("invalid relative backup path"));
    }
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| error("state paths must be UTF-8"))
}

fn fingerprint(path: &Path) -> io::Result<Entry> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(error("backup entry is not a regular file"));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut bytes = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        bytes += read as u64;
    }
    Ok(Entry {
        bytes,
        sha256: Some(format!("{:x}", digest.finalize())),
        mode: metadata.mode() & 0o777,
    })
}

fn inventory_tree(
    root: &Path,
    directory: &Path,
    entries: &mut BTreeMap<String, Entry>,
) -> io::Result<()> {
    for entry in fs::read_dir(root.join(directory))? {
        let entry = entry?;
        let path = directory.join(entry.file_name());
        let kind = entry.file_type()?;
        if (directory.as_os_str().is_empty() && path == Path::new("daemon.lock"))
            || kind.is_socket()
        {
            continue;
        }
        let key = relative(&path)?;
        if kind.is_dir() {
            entries.insert(
                key,
                Entry {
                    bytes: 0,
                    sha256: None,
                    mode: entry.metadata()?.mode() & 0o777,
                },
            );
            inventory_tree(root, &path, entries)?;
        } else if kind.is_file() {
            entries.insert(key, fingerprint(&entry.path())?);
        } else {
            return Err(error(
                "state contains a symlink or unsupported special file",
            ));
        }
    }
    Ok(())
}

fn inventory_files(root: &Path) -> io::Result<BTreeMap<String, Entry>> {
    let mut files = BTreeMap::new();
    inventory_tree(root, Path::new(""), &mut files)?;
    Ok(files)
}

fn copy_files(
    source: &Path,
    destination: &Path,
    files: &BTreeMap<String, Entry>,
) -> io::Result<()> {
    for (name, entry) in files {
        relative(Path::new(name))?;
        let target = destination.join(name);
        if entry.sha256.is_none() {
            fs::DirBuilder::new().mode(0o700).create(&target)?;
        } else {
            let mut input = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(source.join(name))?;
            let mut output = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(entry.mode)
                .open(&target)?;
            io::copy(&mut input, &mut output)?;
            output.set_permissions(fs::Permissions::from_mode(entry.mode))?;
            output.sync_all()?;
            if fingerprint(&target)? != *entry {
                return Err(error("backup changed while being copied"));
            }
        }
    }
    for (name, entry) in files.iter().rev() {
        if entry.sha256.is_none() {
            fs::set_permissions(
                destination.join(name),
                fs::Permissions::from_mode(entry.mode),
            )?;
            File::open(destination.join(name))?.sync_all()?;
        }
    }
    File::open(destination)?.sync_all()
}

fn backup(store: &Store, target: &Path) -> io::Result<()> {
    let stage = Staging::new(target)?;
    fs::DirBuilder::new()
        .mode(0o700)
        .create(stage.0.join("data"))?;
    let files = inventory_files(&store.root)?;
    copy_files(&store.root, &stage.0.join("data"), &files)?;
    if inventory_files(&store.root)? != files {
        return Err(error("source changed during backup"));
    }
    write_json(
        &stage.0.join("manifest.json"),
        &Manifest {
            schema_version: SCHEMA,
            source: store.root.clone(),
            files,
        },
    )?;
    stage.publish(target)
}

fn drained(validated: &Validated) -> io::Result<()> {
    if !validated.inventory.pending_resource_updates.is_empty() {
        return Err(io::Error::other(crate::resource_update::message(
            "resource update is pending",
        )));
    }
    if !validated.inventory.pending_cancellations.is_empty() {
        return Err(io::Error::other(crate::cancellation::message(
            "cancellation is pending",
        )));
    }
    if !validated.inventory.conversion_ready {
        return Err(error(
            "finish or cancel Jobs and stop network helpers under the previous service before migration",
        ));
    }
    Ok(())
}

fn copyable(validated: &Validated, interrupted: bool) -> io::Result<()> {
    if !validated.inventory.pending_resource_updates.is_empty() {
        return Err(io::Error::other(crate::resource_update::message(
            "resource update is pending",
        )));
    }
    if !validated.inventory.pending_cancellations.is_empty() {
        return Err(io::Error::other(crate::cancellation::message(
            "cancellation is pending",
        )));
    }
    if !validated.inventory.active_helpers.is_empty() {
        return Err(error(
            "network helpers of this state are still running; stop them before a backup",
        ));
    }
    let executing: Vec<&Job> = validated
        .jobs
        .iter()
        .filter(|job| job.state.active())
        .collect();
    let listed = |jobs: &[&Job]| {
        jobs.iter()
            .map(|job| job.id.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !interrupted && !executing.is_empty() {
        return Err(io::Error::other(
            message(
                "Jobs recorded as starting, running, suspended or stopping: {ids}; let the service finish or cancel them, or pass --interrupted to copy these records as they are",
            )
            .replace("{ids}", &listed(&executing)),
        ));
    }
    let boot = crate::process::boot_id().ok();
    let mut alive = Vec::new();
    for job in executing {
        if job.supervisor_boot_id != boot {
            continue;
        }
        let Some((pid, ticks)) = job.shim_pid.zip(job.shim_start_ticks) else {
            continue;
        };
        match crate::process::Handle::open(pid, Some(ticks)) {
            Ok(handle) => {
                if handle.alive()? {
                    alive.push(job);
                }
            }
            Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {}
            Err(e) => return Err(e),
        }
    }
    if !alive.is_empty() {
        return Err(io::Error::other(
            message(
                "the supervisors of these Jobs are still running: {ids}; --interrupted copies only records whose processes have ended",
            )
            .replace("{ids}", &listed(&alive)),
        ));
    }
    Ok(())
}

fn migrate(
    store: &Store,
    target: &Path,
    backup_path: &Path,
    validated: Validated,
    profile: crate::config::Profile,
) -> io::Result<()> {
    drained(&validated)?;
    backup(store, backup_path)?;
    let stage = Staging::new(target)?;
    let manifest: Manifest = serde_json::from_slice(&fs::read(backup_path.join("manifest.json"))?)?;
    copy_files(&backup_path.join("data"), &stage.0, &manifest.files)?;
    let converted = Store {
        root: stage.0.clone(),
    };
    validated.graph.save(&converted)?;
    for mut job in validated.jobs {
        job.log = target
            .join("jobs")
            .join(job.id.to_string())
            .join("output.log");
        let mut original: serde_json::Value =
            serde_json::from_slice(&fs::read(converted.job_file(job.id))?)?;
        original["queue_id"] = serde_json::to_value(job.queue_id)?;
        original["policy"] = serde_json::to_value(job.policy)?;
        original["log"] = serde_json::to_value(job.log)?;
        original["spec"]["queue"] = serde_json::to_value(job.spec.queue)?;
        write_json(&converted.job_file(job.id), &original)?;
    }
    write_json(&converted.root.join("schema.json"), &Schema::default())?;
    write_json(
        &converted.root.join("migration-report.json"),
        &serde_json::json!({
            "schema_version": 1,
            "source": store.root,
            "destination": target,
            "backup": backup_path,
            "actor_uid": unsafe { libc::geteuid() },
            "created_ms": crate::shim::now_ms(),
            "inventory": validated.inventory,
        }),
    )?;
    let config = crate::config::Config {
        profile,
        ..Default::default()
    };
    crate::store::write_atomic(
        &converted.root.join("migration-config.toml"),
        toml::to_string_pretty(&config)
            .map_err(io::Error::other)?
            .as_bytes(),
    )?;
    validate(&converted)?;
    stage.publish(target)
}

fn restore(backup_path: &Path, target: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(backup_path.join("data"))?.is_dir() {
        return Err(error("backup data must be a directory, not a symlink"));
    }
    let manifest: Manifest = serde_json::from_slice(&fs::read(backup_path.join("manifest.json"))?)?;
    if ![
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, SCHEMA,
    ]
    .contains(&manifest.schema_version)
    {
        return Err(error("unsupported backup manifest version"));
    }
    if inventory_files(&backup_path.join("data"))? != manifest.files {
        return Err(error("backup integrity check failed"));
    }
    let stage = Staging::new(target)?;
    copy_files(&backup_path.join("data"), &stage.0, &manifest.files)?;
    validate(&Store {
        root: stage.0.clone(),
    })?;
    stage.publish(target)
}

pub fn command(args: &[String]) -> io::Result<()> {
    let Some(operation) = args.first().map(String::as_str) else {
        return Err(error(
            "usage: job state validate|backup|migrate|restore --source PATH [--destination PATH] [--backup PATH] [--dry-run] [--interrupted]",
        ));
    };
    let mut source_path = None;
    let mut target = None;
    let mut backup_path = None;
    let mut dry_run = false;
    let mut interrupted = false;
    let mut profile = None;
    let mut rest = args[1..].iter();
    while let Some(option) = rest.next() {
        match option.as_str() {
            "--source" => source_path = rest.next().map(PathBuf::from),
            "--destination" => target = rest.next().map(PathBuf::from),
            "--backup" => backup_path = rest.next().map(PathBuf::from),
            "--dry-run" => dry_run = true,
            "--interrupted" if operation == "backup" => interrupted = true,
            "--profile" => {
                profile = Some(match rest.next().map(String::as_str) {
                    Some("ordinary") => crate::config::Profile::Ordinary,
                    Some("legacy") => crate::config::Profile::Legacy,
                    _ => return Err(error("--profile must be ordinary or legacy")),
                })
            }
            _ => return Err(error("unknown state command option")),
        }
    }
    let source_path = source_path.ok_or_else(|| error("--source is required"))?;
    if operation == "restore" {
        if dry_run || backup_path.is_some() || profile.is_some() {
            return Err(error("restore accepts --source and --destination only"));
        }
        let target = destination(&target.ok_or_else(|| error("--destination is required"))?)?;
        let source_path = fs::canonicalize(source_path)?;
        disjoint(&[&source_path, &target])?;
        restore(&source_path, &target)?;
        return Ok(());
    }
    let (store, _lock) = source(&source_path)?;
    inventory_files(&store.root)?;
    let mut validated = validate(&store)?;
    match operation {
        "validate"
            if target.is_none() && backup_path.is_none() && !dry_run && profile.is_none() =>
        {
            println!("{}", serde_json::to_string_pretty(&validated.inventory)?);
        }
        "backup" if backup_path.is_none() && !dry_run && profile.is_none() => {
            copyable(&validated, interrupted)?;
            let target = destination(&target.ok_or_else(|| error("--destination is required"))?)?;
            disjoint(&[&store.root, &target])?;
            backup(&store, &target)?;
        }
        "migrate" => {
            let profile = profile.ok_or_else(|| {
                error("migration requires an explicit --profile ordinary or --profile legacy")
            })?;
            validated.inventory.planned_service_policy = Some(
                crate::config::Config {
                    profile,
                    ..Default::default()
                }
                .effective(None),
            );
            let target = destination(&target.ok_or_else(|| error("--destination is required"))?)?;
            let backup_path =
                destination(&backup_path.ok_or_else(|| error("--backup is required"))?)?;
            disjoint(&[&store.root, &target, &backup_path])?;
            if dry_run {
                println!("{}", serde_json::to_string_pretty(&validated.inventory)?);
            } else {
                migrate(&store, &target, &backup_path, validated, profile)?;
            }
        }
        _ => return Err(error("invalid state command or options")),
    }
    Ok(())
}
