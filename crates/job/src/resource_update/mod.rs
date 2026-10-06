use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cgroup::{Limits, Tree, control_matches};
use crate::model::{Job, State};
use crate::objects::{Graph, Kind};
use crate::store::Store;

mod messages;
pub use messages::{help, message};

type Values = BTreeMap<String, String>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    Job { id: u64 },
    Object { kind: Kind, path: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub id: u64,
    pub attempt: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub file: String,
    pub before: String,
    pub after: String,
    pub write: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub operation: String,
    pub target: Target,
    pub object_id: Option<u64>,
    pub members: Vec<Member>,
    pub boot_id: String,
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub patch: BTreeMap<String, Value>,
    pub allow_oom: bool,
    pub before: Values,
    pub after: Values,
    pub config_before: Option<BTreeMap<String, Value>>,
    pub config_after: Option<BTreeMap<String, Value>>,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Pending,
    Verified,
    Applied,
    Ended,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub schema_version: u32,
    pub actor_uid: u32,
    pub requested_ms: u64,
    pub plan: Plan,
    pub phase: Phase,
    pub completed_steps: usize,
    pub error: Option<String>,
}

impl Operation {
    pub fn new(plan: Plan) -> Self {
        Self {
            schema_version: 1,
            actor_uid: crate::service::access::actor_uid(),
            requested_ms: crate::shim::now_ms(),
            plan,
            phase: Phase::Pending,
            completed_steps: 0,
            error: None,
        }
    }

    pub fn pending(&self) -> bool {
        matches!(self.phase, Phase::Pending | Phase::Verified)
    }

    pub fn accepts(&self, path: &Path, file: &str, actual: &str) -> bool {
        self.pending()
            && self.plan.path == path
            && self.plan.steps.iter().any(|step| {
                step.file == file
                    && (control_matches(file, &step.before, actual)
                        || control_matches(file, &step.after, actual))
            })
    }
}

fn error(key: &str) -> io::Error {
    io::Error::other(message(key))
}

pub fn token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn valid_token(token: &str) -> bool {
    token.len() == 32
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn allowed_file(file: &str) -> bool {
    [
        "cpu.max",
        "cpu.weight",
        "memory.high",
        "memory.max",
        "memory.swap.max",
        "pids.max",
        "io.max",
        "io.weight",
        "io.bfq.weight",
    ]
    .contains(&file)
}

fn directory(store: &Store, create: bool) -> io::Result<bool> {
    let path = store.root.join("resource-updates");
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_dir() => Ok(true),
        Ok(_) => Err(error("invalid resource update journal")),
        Err(e) if e.kind() == io::ErrorKind::NotFound && create => {
            fs::DirBuilder::new().mode(0o700).create(path)?;
            File::open(&store.root)?.sync_all()?;
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn validate(operation: &Operation) -> io::Result<()> {
    let plan = &operation.plan;
    if operation.schema_version != 1
        || !valid_token(&plan.operation)
        || !plan.path.is_absolute()
        || plan.boot_id.is_empty()
        || plan.inode == 0
        || plan.members.iter().any(|m| m.id == 0 || m.attempt == 0)
        || !plan.members.windows(2).all(|w| w[0].id < w[1].id)
        || plan
            .before
            .keys()
            .chain(plan.after.keys())
            .any(|f| !allowed_file(f))
        || plan
            .steps
            .iter()
            .any(|s| !allowed_file(&s.file) || s.write.contains('\n'))
        || operation.completed_steps > plan.steps.len()
        || (matches!(operation.phase, Phase::Verified | Phase::Applied)
            && operation.completed_steps != plan.steps.len())
        || (plan.object_id.is_some() != plan.config_before.is_some())
        || (plan.object_id.is_some() != plan.config_after.is_some())
    {
        return Err(error("invalid resource update journal"));
    }
    Ok(())
}

pub fn save(store: &Store, operation: &Operation) -> io::Result<()> {
    validate(operation)?;
    directory(store, true)?;
    crate::store::write_json(
        &store
            .root
            .join("resource-updates")
            .join(format!("{}.json", operation.plan.operation)),
        operation,
    )
}

pub fn load(store: &Store) -> io::Result<BTreeMap<String, Operation>> {
    let mut all = BTreeMap::new();
    if !directory(store, false)? {
        return Ok(all);
    }
    for entry in fs::read_dir(store.root.join("resource-updates"))? {
        let entry = entry?;
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|s| s.to_string_lossy().starts_with("tmp"))
        {
            continue;
        }
        if !entry.file_type()?.is_file() || path.extension().is_none_or(|s| s != "json") {
            return Err(error("invalid resource update journal"));
        }
        let op: Operation = serde_json::from_slice(&fs::read(&path)?)?;
        validate(&op)?;
        if entry.file_name() != format!("{}.json", op.plan.operation).as_str() {
            return Err(error("invalid resource update journal"));
        }
        all.insert(op.plan.operation.clone(), op);
    }
    if all.values().filter(|op| op.pending()).count() > 1 {
        return Err(error("invalid resource update journal"));
    }
    Ok(all)
}

fn io_rows(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (id, values) = line.trim().split_once(char::is_whitespace)?;
            (id != "default").then(|| (id.to_owned(), values.trim().to_owned()))
        })
        .collect()
}

fn io_text(file: &str, rows: &BTreeMap<String, String>) -> String {
    let mut lines: Vec<_> = rows
        .iter()
        .map(|(id, values)| format!("{id} {values}"))
        .collect();
    if file != "io.max" {
        lines.insert(0, "default 100".to_owned());
    }
    lines.join("\n")
}

fn transitions(file: &str, before: &str, after: &str) -> Vec<Step> {
    if control_matches(file, after, before) {
        return Vec::new();
    }
    if !crate::io_policy::is_file(file) {
        return vec![Step {
            file: file.to_owned(),
            before: before.to_owned(),
            after: after.to_owned(),
            write: after.to_owned(),
        }];
    }
    let mut current = io_rows(before);
    let desired = io_rows(after);
    let ids: std::collections::BTreeSet<_> =
        current.keys().chain(desired.keys()).cloned().collect();
    let mut steps = Vec::new();
    for id in ids {
        let old = io_text(file, &current);
        let write = if let Some(value) = desired.get(&id) {
            current.insert(id.clone(), value.clone());
            format!("{id} {value}")
        } else {
            current.remove(&id);
            if file == "io.max" {
                format!("{id} rbps=max wbps=max riops=max wiops=max")
            } else {
                format!("{id} default")
            }
        };
        let new = io_text(file, &current);
        if !control_matches(file, &new, &old) {
            steps.push(Step {
                file: file.to_owned(),
                before: old,
                after: new,
                write,
            });
        }
    }
    steps
}

pub fn preview(
    store: &Store,
    graph: &Graph,
    tree: Option<&Tree>,
    target: Target,
    patch: BTreeMap<String, Value>,
    allow_oom: bool,
    operation: String,
) -> io::Result<Plan> {
    if patch.is_empty()
        || patch
            .iter()
            .any(|(k, v)| !crate::aggregate::key(k) || v.is_null())
    {
        return Err(error(
            "live updates accept only explicit kernel resource controls",
        ));
    }
    let controls = crate::aggregate::controls(&patch).map_err(io::Error::other)?;
    let tree = tree.ok_or_else(|| error("live updates require an existing resource domain"))?;
    let mut members = Vec::new();
    let (object_id, path, before, config_before, config_after) = match &target {
        Target::Job { id } => {
            let job = store
                .load_job(*id)
                .ok_or_else(|| error("live updates require a local running or suspended Job"))?;
            if !matches!(job.state, State::Running | State::Suspended)
                || job.spec.declared.on.is_some()
                || job.supervisor_boot_id.as_deref() != Some(crate::process::boot_id()?.as_str())
            {
                return Err(error(
                    "live updates require a local running or suspended Job",
                ));
            }
            let path = crate::aggregate::leaf(tree, &job.aggregate_domains, *id);
            if job.workload_cgroup.as_ref() != Some(&path) || !Tree::is_populated(&path) {
                return Err(error("resource update target changed"));
            }
            members.push(Member {
                id: *id,
                attempt: job.attempt,
            });
            let limits = Limits::for_job(&job).with_recorded(&job.applied_resources)?;
            (
                None,
                path,
                limits
                    .files()
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect::<Values>(),
                None,
                None,
            )
        }
        Target::Object { kind, path } => {
            let id = graph.resolve(path).map_err(io::Error::other)?;
            if graph.nodes[&id].kind != *kind {
                return Err(error("resource update target changed"));
            }
            let domains = crate::aggregate::chain(graph, id);
            let domain = domains
                .last()
                .filter(|d| d.object_id == id)
                .ok_or_else(|| error("live updates require an existing resource domain"))?;
            let path = crate::aggregate::paths(tree, &domains).pop().unwrap();
            let before_config = graph.nodes[&id].config.clone();
            let mut after_config = before_config.clone();
            after_config.extend(patch.clone());
            crate::aggregate::controls(&after_config).map_err(io::Error::other)?;
            for job_id in store.job_ids() {
                let Some(job) = store.load_job(job_id) else {
                    continue;
                };
                if job.aggregate_domains.iter().any(|d| d.object_id == id)
                    && (job.state.active()
                        || Tree::is_populated(&crate::aggregate::leaf(
                            tree,
                            &job.aggregate_domains,
                            job_id,
                        )))
                {
                    members.push(Member {
                        id: job_id,
                        attempt: job.attempt,
                    });
                }
            }
            (
                Some(id),
                path,
                domain.limits.clone(),
                Some(before_config),
                Some(after_config),
            )
        }
    };
    let dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)?;
    let meta = dir.metadata()?;
    for (file, expected) in &before {
        if crate::cgroup::changed_control(&path, file, expected).is_some() {
            return Err(error("resource control differs from the recorded update"));
        }
    }
    let mut after = before.clone();
    let mut steps = Vec::new();
    for (file, desired) in Limits::for_controls(&controls).files() {
        if !tree.supports(file) {
            return Err(io::Error::other(crate::resource_policy::message(
                "requested resource control is unavailable",
            )));
        }
        crate::io_policy::available(file, desired).map_err(io::Error::other)?;
        let current = fs::read_to_string(path.join(file))?.trim().to_owned();
        if !before.contains_key(file)
            && crate::io_policy::is_file(file)
            && !control_matches(file, "", &current)
        {
            return Err(error("resource control differs from the recorded update"));
        }
        if file == "memory.max"
            && desired != "max"
            && !allow_oom
            && (current == "max"
                || desired.parse::<u64>().unwrap()
                    < current.parse::<u64>().map_err(io::Error::other)?)
        {
            return Err(error(
                "lowering memory.max requires --allow-oom; processes may be killed",
            ));
        }
        steps.extend(transitions(file, &current, desired));
        after.insert(file.to_owned(), desired.to_owned());
    }
    Ok(Plan {
        operation,
        target,
        object_id,
        members,
        boot_id: crate::process::boot_id()?,
        path,
        device: meta.dev(),
        inode: meta.ino(),
        patch,
        allow_oom,
        before,
        after,
        config_before,
        config_after,
        steps,
    })
}

fn open_control(dir: &File, name: &str, write: bool) -> io::Result<File> {
    if !allowed_file(name) {
        return Err(error("invalid resource update journal"));
    }
    let name = CString::new(name).map_err(io::Error::other)?;
    let flags = if write {
        libc::O_WRONLY | libc::O_NONBLOCK
    } else {
        libc::O_RDONLY
    };
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn read_control(dir: &File, name: &str) -> io::Result<String> {
    let mut text = String::new();
    open_control(dir, name, false)?.read_to_string(&mut text)?;
    Ok(text.trim().to_owned())
}

pub fn execute(store: &Store, operation: &mut Operation) -> io::Result<()> {
    if operation.phase != Phase::Pending {
        return Ok(());
    }
    validate(operation)?;
    save(store, operation)?;
    let plan = &operation.plan;
    let dir = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&plan.path)
    {
        Ok(dir) => Some(dir),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let same = if let Some(dir) = &dir {
        let meta = dir.metadata()?;
        meta.dev() == plan.device
            && meta.ino() == plan.inode
            && plan.boot_id == crate::process::boot_id()?
    } else {
        false
    };
    if !same {
        operation.phase = Phase::Ended;
        operation.error = Some(message("resource update target ended"));
        return save(store, operation);
    }
    let dir = dir.unwrap();
    while operation.completed_steps < operation.plan.steps.len() {
        let step = &operation.plan.steps[operation.completed_steps];
        let actual = read_control(&dir, &step.file)?;
        if !control_matches(&step.file, &step.after, &actual) {
            if !control_matches(&step.file, &step.before, &actual) {
                return Err(error("resource control differs from the recorded update"));
            }
            crate::io_policy::available(&step.file, &step.after).map_err(io::Error::other)?;
            open_control(&dir, &step.file, true)?.write_all(step.write.as_bytes())?;
            if !control_matches(&step.file, &step.after, &read_control(&dir, &step.file)?) {
                return Err(error("resource control differs from the recorded update"));
            }
        }
        operation.completed_steps += 1;
        operation.error = None;
        save(store, operation)?;
    }
    for (file, expected) in &operation.plan.after {
        crate::io_policy::available(file, expected).map_err(io::Error::other)?;
        if !control_matches(file, expected, &read_control(&dir, file)?) {
            return Err(error("resource control differs from the recorded update"));
        }
    }
    operation.phase = Phase::Verified;
    save(store, operation)
}

pub fn publish(store: &Store, operation: &mut Operation) -> io::Result<()> {
    if operation.phase != Phase::Verified {
        return Ok(());
    }
    let plan = &operation.plan;
    if let Some(id) = plan.object_id {
        let mut graph = Graph::load(store)?;
        let node = graph
            .nodes
            .get_mut(&id)
            .ok_or_else(|| error("resource update target changed"))?;
        if Some(&node.config) != plan.config_before.as_ref()
            && Some(&node.config) != plan.config_after.as_ref()
        {
            return Err(error("resource update target changed"));
        }
        node.config = plan.config_after.clone().unwrap();
        graph.save(store)?;
    }
    for member in &plan.members {
        let mut job = store
            .load_job(member.id)
            .ok_or_else(|| error("resource update target changed"))?;
        if job.attempt != member.attempt {
            return Err(error("resource update target changed"));
        }
        if let Some(id) = plan.object_id {
            let domain = job
                .aggregate_domains
                .iter_mut()
                .find(|d| d.object_id == id)
                .ok_or_else(|| error("resource update target changed"))?;
            if domain.limits != plan.before && domain.limits != plan.after {
                return Err(error("resource update target changed"));
            }
            domain.limits = plan.after.clone();
        } else {
            job.applied_resources = plan.after.clone();
        }
        store.save_job(&job)?;
    }
    operation.phase = Phase::Applied;
    operation.error = None;
    save(store, operation)
}

pub fn abandon(store: &Store, operation: &mut Operation) -> io::Result<()> {
    if !operation.pending() {
        return Ok(());
    }
    if operation.phase != Phase::Pending {
        return Err(error("resource update is pending"));
    }
    if operation.plan.boot_id == crate::process::boot_id()? {
        match fs::metadata(&operation.plan.path) {
            Ok(meta)
                if meta.dev() == operation.plan.device && meta.ino() == operation.plan.inode =>
            {
                let events = fs::read_to_string(operation.plan.path.join("cgroup.events"))?;
                if !events.lines().any(|line| line == "populated 0") {
                    return Err(error("abandon requires an empty resource domain"));
                }
            }
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    operation.phase = Phase::Ended;
    operation.error = Some(message(
        "resource update abandoned; completed writes were not rolled back",
    ));
    save(store, operation)
}

pub fn refresh(job: &mut Job, store: &Store) {
    if let Some(saved) = store.load_job(job.id)
        && saved.attempt == job.attempt
    {
        job.applied_resources = saved.applied_resources;
        job.aggregate_domains = saved.aggregate_domains;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        store: Store,
        operation: Operation,
    }

    impl Fixture {
        fn new() -> Self {
            let token = token().unwrap();
            let root = std::env::temp_dir().join(format!("job-resource-update-{token}"));
            let store = Store::open(root).unwrap();
            let path = store.root.join("cgroup");
            fs::create_dir(&path).unwrap();
            fs::write(path.join("cpu.weight"), "100").unwrap();
            let meta = fs::metadata(&path).unwrap();
            let plan = Plan {
                operation: token,
                target: Target::Job { id: 1 },
                object_id: None,
                members: vec![Member { id: 1, attempt: 1 }],
                boot_id: crate::process::boot_id().unwrap(),
                path,
                device: meta.dev(),
                inode: meta.ino(),
                patch: BTreeMap::from([("cpu_weight".to_owned(), Value::from(200))]),
                allow_oom: false,
                before: BTreeMap::from([("cpu.weight".to_owned(), "100".to_owned())]),
                after: BTreeMap::from([("cpu.weight".to_owned(), "200".to_owned())]),
                config_before: None,
                config_after: None,
                steps: transitions("cpu.weight", "100", "200"),
            };
            Self {
                store,
                operation: Operation::new(plan),
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.store.root);
        }
    }

    #[test]
    fn acknowledgment_loss_reconciles_the_already_written_value() {
        let mut f = Fixture::new();
        save(&f.store, &f.operation).unwrap();
        fs::write(f.operation.plan.path.join("cpu.weight"), "200").unwrap();
        let mut recovered = load(&f.store)
            .unwrap()
            .remove(&f.operation.plan.operation)
            .unwrap();
        execute(&f.store, &mut recovered).unwrap();
        assert_eq!(recovered.phase, Phase::Verified);
        assert_eq!(recovered.completed_steps, 1);
        f.operation = recovered;
    }

    #[test]
    fn an_unexpected_kernel_value_is_not_overwritten() {
        let mut f = Fixture::new();
        fs::write(f.operation.plan.path.join("cpu.weight"), "300").unwrap();
        assert!(execute(&f.store, &mut f.operation).is_err());
        assert_eq!(
            fs::read_to_string(f.operation.plan.path.join("cpu.weight")).unwrap(),
            "300"
        );
        assert_eq!(
            load(&f.store).unwrap()[&f.operation.plan.operation].completed_steps,
            0
        );
    }

    #[test]
    fn a_replaced_directory_never_receives_a_write() {
        let mut f = Fixture::new();
        fs::rename(&f.operation.plan.path, f.store.root.join("old-cgroup")).unwrap();
        fs::create_dir(&f.operation.plan.path).unwrap();
        fs::write(f.operation.plan.path.join("cpu.weight"), "100").unwrap();
        execute(&f.store, &mut f.operation).unwrap();
        assert_eq!(f.operation.phase, Phase::Ended);
        assert_eq!(
            fs::read_to_string(f.operation.plan.path.join("cpu.weight")).unwrap(),
            "100"
        );
    }

    #[test]
    fn an_old_boot_never_receives_a_write() {
        let mut f = Fixture::new();
        f.operation.plan.boot_id = "previous-boot".to_owned();
        execute(&f.store, &mut f.operation).unwrap();
        assert_eq!(f.operation.phase, Phase::Ended);
        assert_eq!(
            fs::read_to_string(f.operation.plan.path.join("cpu.weight")).unwrap(),
            "100"
        );
    }

    #[test]
    fn completed_files_are_not_replayed_on_resume() {
        let mut f = Fixture::new();
        fs::write(f.operation.plan.path.join("memory.high"), "max").unwrap();
        f.operation
            .plan
            .steps
            .extend(transitions("memory.high", "max", "123"));
        f.operation
            .plan
            .after
            .insert("memory.high".to_owned(), "123".to_owned());
        f.operation.completed_steps = 1;
        fs::write(f.operation.plan.path.join("cpu.weight"), "200").unwrap();
        execute(&f.store, &mut f.operation).unwrap();
        assert_eq!(f.operation.phase, Phase::Verified);
        assert_eq!(f.operation.completed_steps, 2);
    }

    #[test]
    fn io_map_replacement_resets_an_omitted_device() {
        let steps = transitions(
            "io.max",
            "8:0 rbps=max wbps=1024 riops=max wiops=max",
            "8:16 rbps=max wbps=2048 riops=max wiops=max",
        );
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].write, "8:0 rbps=max wbps=max riops=max wiops=max");
        assert_eq!(
            steps[1].write,
            "8:16 rbps=max wbps=2048 riops=max wiops=max"
        );
        assert_eq!(steps[0].after, steps[1].before);
    }

    #[test]
    fn weight_map_replacement_resets_to_the_kernel_default() {
        let steps = transitions("io.weight", "default 100\n8:0 200", "8:16 300");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].write, "8:0 default");
        assert_eq!(steps[0].after, "default 100");
        assert_eq!(steps[1].after, "default 100\n8:16 300");
    }

    #[test]
    fn equivalent_unlimited_io_rows_need_no_write() {
        assert!(transitions("io.max", "", "8:0 rbps=max wbps=max riops=max wiops=max").is_empty());
    }

    #[test]
    fn an_invalid_control_file_is_rejected_before_intent_is_saved() {
        let mut f = Fixture::new();
        f.operation.plan.steps[0].file = "../cgroup.procs".to_owned();
        assert!(save(&f.store, &f.operation).is_err());
        assert!(load(&f.store).unwrap().is_empty());
    }

    #[test]
    fn recovery_rejects_multiple_pending_operations() {
        let f = Fixture::new();
        save(&f.store, &f.operation).unwrap();
        let mut other = f.operation.clone();
        other.plan.operation = token().unwrap();
        save(&f.store, &other).unwrap();
        assert!(load(&f.store).is_err());
    }
}
