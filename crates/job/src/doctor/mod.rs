mod messages;
pub use messages::message;

use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::Serialize;

use crate::cgroup::Tree;
use crate::model::{HostInfo, Request, Response};
use crate::paths;
use crate::service::discovery;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    fn name(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "fail",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
    pub fix: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub checks: Vec<Check>,
}

fn ok(name: &'static str, detail: String) -> Check {
    Check {
        name,
        status: Status::Ok,
        detail,
        fix: None,
    }
}

fn warn(name: &'static str, detail: String, fix: String) -> Check {
    Check {
        name,
        status: Status::Warn,
        detail,
        fix: Some(fix),
    }
}

fn fail(name: &'static str, detail: String, fix: String) -> Check {
    Check {
        name,
        status: Status::Fail,
        detail,
        fix: Some(fix),
    }
}

fn plain(key: &str) -> String {
    message(key, &[])
}

fn shown(path: &Path) -> String {
    path.display().to_string()
}

const SOCKET_PATH_LIMIT: usize = 108;

struct Context {
    mode: paths::Mode,
    host: Option<HostInfo>,
    host_error: Option<String>,
    socket: Option<PathBuf>,
    config: Result<(crate::config::Config, Option<PathBuf>), String>,
    local: Option<discovery::Found>,
}

impl Context {
    fn gather() -> Self {
        let mode = paths::mode(&paths::process);
        let socket = paths::client_socket().ok();
        let (host, host_error) = match &socket {
            Some(socket) => match crate::client::call(socket, &Request::Host) {
                Ok(Response::Host { info }) => (Some(*info), None),
                Ok(Response::Error { message }) => (None, Some(message)),
                Ok(_) => (None, Some(plain("unexpected answer"))),
                Err(error) => (None, Some(error.to_string())),
            },
            None => (None, None),
        };
        let config = crate::config::Config::load().map_err(|e| e.to_string());
        let local = config
            .as_ref()
            .ok()
            .and_then(|(config, _)| discovery::discover(config.cgroup.root.as_deref()).ok());
        Self {
            mode,
            host,
            host_error,
            socket,
            config,
            local,
        }
    }

    fn service(&self) -> Option<&crate::service::Info> {
        self.host.as_ref().and_then(|host| host.service.as_ref())
    }

    fn root(&self) -> Option<PathBuf> {
        self.service()
            .and_then(|info| info.cgroup_root.clone())
            .or_else(|| {
                self.local
                    .as_ref()
                    .and_then(|found| found.tree.as_ref().map(|tree| tree.root.clone()))
            })
    }

    fn group(&self) -> Option<String> {
        self.service()
            .and_then(|info| info.socket_group.clone())
            .or_else(|| {
                self.config
                    .as_ref()
                    .ok()
                    .and_then(|(config, _)| config.socket.group.clone())
            })
    }
}

fn locations(context: &Context) -> Check {
    let env: paths::Lookup = &paths::process;
    let mode = context.mode;
    let cache = paths::cache(mode, env)
        .map(|path| shown(&path))
        .unwrap_or_else(|_| plain("not located, and not used yet"));
    let all = (
        paths::config_file(mode, env),
        paths::state(mode, env),
        paths::runtime(mode, env),
    );
    match all {
        (Ok(config), Ok(state), Ok(runtime)) => ok(
            "paths",
            message(
                "{mode} service: configuration {config}, state {state}, runtime {runtime}, cache {cache}",
                &[
                    ("mode", plain(mode.name())),
                    ("config", shown(&config)),
                    ("state", shown(&state)),
                    ("runtime", shown(&runtime.directory)),
                    ("cache", cache),
                ],
            ),
        ),
        (config, state, runtime) => {
            let mut errors: Vec<String> = [config.err(), state.err(), runtime.map(|_| ()).err()]
                .into_iter()
                .flatten()
                .collect();
            errors.dedup();
            fail(
                "paths",
                errors.join("; "),
                plain(
                    "set the named variable in the environment of the service and of its clients",
                ),
            )
        }
    }
}

fn cgroup_mounted() -> Check {
    if Path::new("/sys/fs/cgroup/cgroup.controllers").is_file() {
        ok(
            "cgroup_v2",
            plain("the unified hierarchy is mounted at /sys/fs/cgroup"),
        )
    } else {
        warn(
            "cgroup_v2",
            plain("no unified cgroup hierarchy at /sys/fs/cgroup"),
            plain(
                "mount cgroup2 at /sys/fs/cgroup; without it limits are watched and not enforced",
            ),
        )
    }
}

fn cgroup_own() -> Check {
    match Tree::own() {
        Some(own) => ok(
            "cgroup_own",
            message(
                "this command runs in {path}; the service may run in another cgroup",
                &[("path", shown(&own))],
            ),
        ),
        None => warn(
            "cgroup_own",
            plain("/proc/self/cgroup names no unified cgroup"),
            plain(
                "mount cgroup2 at /sys/fs/cgroup; without it limits are watched and not enforced",
            ),
        ),
    }
}

fn cgroup_root(context: &Context) -> Check {
    let delegate = plain(
        "delegate a cgroup to the service: Delegate=yes in its systemd unit, or a directory owned by the service user that holds only the service; see jobd(8)",
    );
    if let Some(info) = context.service() {
        return match (&info.cgroup_root, &info.cgroup_rule) {
            (Some(root), Some(rule)) => ok(
                "cgroup_root",
                message(
                    "the service manages {root}, chosen by {rule}",
                    &[("root", shown(root)), ("rule", plain(rule))],
                ),
            ),
            _ => warn(
                "cgroup_root",
                format!(
                    "{}{}",
                    plain("the service has no cgroup root and watches processes instead"),
                    info.cgroup_skipped
                        .iter()
                        .map(|reason| format!("; {reason}"))
                        .collect::<String>()
                ),
                delegate,
            ),
        };
    }
    match &context.local {
        Some(found) => match (&found.tree, found.rule) {
            (Some(tree), Some(rule)) => ok(
                "cgroup_root",
                message(
                    "a service started from here would manage {root}, chosen by {rule}",
                    &[("root", shown(&tree.root)), ("rule", plain(rule.name()))],
                ),
            ),
            _ if context
                .config
                .as_ref()
                .is_ok_and(|(config, _)| config.cgroup.required) =>
            {
                fail(
                    "cgroup_root",
                    crate::operations::mode::missing(found),
                    delegate,
                )
            }
            _ => warn(
                "cgroup_root",
                format!(
                    "{}{}",
                    plain(
                        "a service started from here would find no cgroup root and watch processes instead"
                    ),
                    found
                        .skipped
                        .iter()
                        .map(|reason| format!("; {reason}"))
                        .collect::<String>()
                ),
                delegate,
            ),
        },
        None => warn(
            "cgroup_root",
            plain("the cgroup root cannot be determined before the configuration loads"),
            plain("correct the configuration first"),
        ),
    }
}

fn controllers(context: &Context) -> Check {
    let Some(root) = context.root() else {
        return warn(
            "cgroup_controllers",
            plain("no cgroup root, so no controller is available to Jobs"),
            plain(
                "delegate a cgroup to the service: Delegate=yes in its systemd unit, or a directory owned by the service user that holds only the service; see jobd(8)",
            ),
        );
    };
    let available = fs::read_to_string(root.join("cgroup.controllers")).unwrap_or_default();
    let missing: Vec<&str> = crate::cgroup::WANTED_CONTROLLERS
        .iter()
        .copied()
        .filter(|name| !available.split_whitespace().any(|have| have == *name))
        .collect();
    if missing.is_empty() {
        ok(
            "cgroup_controllers",
            message(
                "{root} offers {controllers}",
                &[
                    ("root", shown(&root)),
                    ("controllers", crate::cgroup::WANTED_CONTROLLERS.join(", ")),
                ],
            ),
        )
    } else {
        warn(
            "cgroup_controllers",
            message(
                "{root} lacks {controllers}",
                &[("root", shown(&root)), ("controllers", missing.join(", "))],
            ),
            plain(
                "enable the missing controllers in cgroup.subtree_control of the parent cgroup; limits that need them are refused until then",
            ),
        )
    }
}

fn io_devices(context: &Context) -> Check {
    let devices = match &context.host {
        Some(host) => host.io_devices.clone(),
        None => crate::io_policy::capabilities(
            context.local.as_ref().and_then(|found| found.tree.as_ref()),
        ),
    };
    let limited = devices.iter().filter(|device| device.io_max).count();
    let weighted = devices
        .iter()
        .filter(|device| device.io_weight || device.io_bfq_weight)
        .count();
    ok(
        "io_devices",
        message(
            "{count} block devices; {limited} accept io.max, {weighted} accept an I/O weight",
            &[
                ("count", devices.len().to_string()),
                ("limited", limited.to_string()),
                ("weighted", weighted.to_string()),
            ],
        ),
    )
}

fn freezer(context: &Context) -> Check {
    let available = match &context.host {
        Some(host) => host.freezer,
        None => context
            .root()
            .is_some_and(|path| path.join("cgroup.freeze").is_file()),
    };
    if available {
        ok("freezer", plain("cgroup.freeze is available"))
    } else {
        warn(
            "freezer",
            plain("cgroup.freeze is not available to the service"),
            plain("job suspend and job continue need a delegated cgroup root"),
        )
    }
}

fn pressure() -> Check {
    use crate::pressure::{Reading, Resource, parse_file};
    let mut problems = Vec::new();
    for (name, resource) in [
        ("cpu", Resource::Cpu),
        ("memory", Resource::Memory),
        ("io", Resource::Io),
    ] {
        let path = PathBuf::from("/proc/pressure").join(name);
        match parse_file(&path, resource, true) {
            Ok(observation) => {
                if let Reading::Unavailable { reason } = observation.some {
                    problems.push(format!("{}: {reason}", shown(&path)));
                }
            }
            Err(error) => problems.push(format!("{}: {error}", shown(&path))),
        }
    }
    if problems.is_empty() {
        ok(
            "pressure",
            plain("/proc/pressure/cpu, memory and io are readable"),
        )
    } else {
        warn(
            "pressure",
            problems.join("; "),
            plain(
                "boot a kernel with CONFIG_PSI and without psi=0; pressure rules and job pressure stay unavailable until then",
            ),
        )
    }
}

fn user_namespaces() -> Check {
    if crate::host::user_namespaces() {
        ok(
            "user_namespaces",
            plain("unprivileged user namespaces are allowed"),
        )
    } else {
        warn(
            "user_namespaces",
            plain("unprivileged user namespaces are not allowed"),
            plain(
                "--net none, proxy and tunnel networks need them; an administrator may allow them through the sysctl settings named in jobd(8)",
            ),
        )
    }
}

fn landlock() -> Check {
    let abi = crate::confine::abi_version();
    if abi > 0 {
        ok(
            "landlock",
            message("Landlock ABI {abi}", &[("abi", abi.to_string())]),
        )
    } else {
        warn(
            "landlock",
            plain("Landlock is not available"),
            plain("confined Jobs are refused; enable the Landlock security module in the kernel"),
        )
    }
}

fn security() -> Vec<Check> {
    let found = crate::security::kernel::capabilities();
    let unavailable =
        plain("Jobs that request this control are refused; other Jobs are not affected");
    let mut checks = Vec::new();
    checks.push(match (&found.seccomp_query_error, found.seccomp_arch) {
        (None, Some(_)) => ok("seccomp", plain("seccomp filters can be installed")),
        (Some(error), _) => warn("seccomp", error.clone(), unavailable.clone()),
        (None, None) => warn(
            "seccomp",
            plain("this architecture has no seccomp table in job"),
            unavailable.clone(),
        ),
    });
    checks.push(
        match (&found.no_new_privs_error, found.service_no_new_privs) {
            (None, Some(set)) => ok(
                "no_new_privs",
                message(
                    "no_new_privs can be queried; this command has it {state}",
                    &[("state", plain(if set { "set" } else { "unset" }))],
                ),
            ),
            (error, _) => warn(
                "no_new_privs",
                error.clone().unwrap_or_else(|| plain("unexpected answer")),
                unavailable.clone(),
            ),
        },
    );
    checks.push(match (&found.cap_query_error, found.cap_last_cap) {
        (None, Some(last)) => ok(
            "capabilities",
            message(
                "the kernel knows capabilities 0 to {last}",
                &[("last", last.to_string())],
            ),
        ),
        (error, _) => warn(
            "capabilities",
            error.clone().unwrap_or_else(|| plain("unexpected answer")),
            unavailable,
        ),
    });
    checks
}

fn pidfd() -> Check {
    match crate::process::Handle::open(std::process::id() as i32, None)
        .and_then(|handle| handle.signal(0))
    {
        Ok(()) => ok("pidfd", plain("process file descriptors work")),
        Err(error) => fail(
            "pidfd",
            error.to_string(),
            plain(
                "the service does not start without pidfd_open and pidfd_send_signal; use Linux 5.3 or later",
            ),
        ),
    }
}

fn network_tools() -> Check {
    let missing = crate::link::missing_tools();
    if missing.is_empty() {
        ok(
            "network_tools",
            plain("every tool for bandwidth limits and job networks is on PATH"),
        )
    } else {
        warn(
            "network_tools",
            message("not on PATH: {tools}", &[("tools", missing.join(", "))]),
            plain(
                "install them to use --bandwidth, proxy and tunnel networks; other Jobs are not affected",
            ),
        )
    }
}

fn network_names(context: &Context) -> Vec<Check> {
    let report = context
        .host
        .as_ref()
        .and_then(|host| host.network.clone())
        .or_else(|| {
            context
                .config
                .as_ref()
                .ok()
                .and_then(|(config, _)| crate::netpolicy::report(&config.network))
        });
    crate::netpolicy::findings(report.as_ref())
        .into_iter()
        .map(|(name, fine, detail, fix)| {
            if fine {
                ok(name, detail)
            } else {
                warn(name, detail, fix)
            }
        })
        .collect()
}

fn configuration(context: &Context) -> Check {
    match &context.config {
        Ok((_, Some(path))) => ok(
            "config",
            message("{path} is valid", &[("path", shown(path))]),
        ),
        Ok((_, None)) => ok(
            "config",
            match crate::config::Config::path() {
                Ok(path) => message(
                    "no file at {path}; the ordinary defaults apply",
                    &[("path", shown(&path))],
                ),
                Err(error) => error.to_string(),
            },
        ),
        Err(error) => fail(
            "config",
            error.clone(),
            plain("correct the file; job config check FILE validates it without a service"),
        ),
    }
}

fn command_policy() -> Check {
    match crate::policy_file::load() {
        Ok(loaded) => ok("policy", crate::policy_file::describe(&loaded)),
        Err(error) => fail("policy", error, crate::policy_file::fix()),
    }
}

fn state_root(context: &Context) -> Option<PathBuf> {
    paths::state(context.mode, &paths::process).ok()
}

fn accessible(path: &Path, mode: i32) -> bool {
    std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .is_ok_and(|text| unsafe { libc::access(text.as_ptr(), mode) } == 0)
}

fn state_directory(context: &Context) -> Check {
    let Some(root) = state_root(context) else {
        return fail(
            "state_directory",
            plain("the state directory cannot be located"),
            plain("set the named variable in the environment of the service and of its clients"),
        );
    };
    let own = unsafe { libc::getuid() };
    match fs::metadata(&root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ok(
            "state_directory",
            message(
                "{path} does not exist yet; the service creates it with mode 0700",
                &[("path", shown(&root))],
            ),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => ok(
            "state_directory",
            message(
                "{path} is private to the service user; output reaches this user through the service and terminals through the runtime directory",
                &[("path", shown(&root))],
            ),
        ),
        Err(error) => fail(
            "state_directory",
            format!("{}: {error}", shown(&root)),
            plain("make the state directory a directory the service user owns"),
        ),
        Ok(meta) if !meta.is_dir() => fail(
            "state_directory",
            message("{path} is not a directory", &[("path", shown(&root))]),
            plain("make the state directory a directory the service user owns"),
        ),
        Ok(meta) if meta.uid() != own => ok(
            "state_directory",
            message(
                "{path} belongs to user {owner}; output reaches this user through the service and terminals through the runtime directory",
                &[("path", shown(&root)), ("owner", meta.uid().to_string())],
            ),
        ),
        Ok(_) if !accessible(&root, libc::W_OK | libc::X_OK) => fail(
            "state_directory",
            message("{path} is not writable", &[("path", shown(&root))]),
            plain("make the state directory a directory the service user owns"),
        ),
        Ok(meta) if meta.mode() & 0o077 != 0 => warn(
            "state_directory",
            message(
                "{path} has mode {mode}; records and logs are readable by others",
                &[
                    ("path", shown(&root)),
                    ("mode", format!("{:04o}", meta.mode() & 0o7777)),
                ],
            ),
            message("chmod 700 {path}", &[("path", shown(&root))]),
        ),
        Ok(_) => ok(
            "state_directory",
            message("{path} is present and writable", &[("path", shown(&root))]),
        ),
    }
}

fn state_schema(context: &Context) -> Check {
    let Some(root) = state_root(context) else {
        return fail(
            "state_schema",
            plain("the state directory cannot be located"),
            plain("set the named variable in the environment of the service and of its clients"),
        );
    };
    let current = crate::migration::SCHEMA;
    let migrate = plain(
        "stop the service and migrate offline with job state migrate; see the migration guide",
    );
    match crate::migration::recorded(&root) {
        Ok(Some(version)) if version == current => ok(
            "state_schema",
            message(
                "state schema {version} matches this program",
                &[("version", version.to_string())],
            ),
        ),
        Ok(Some(version)) => fail(
            "state_schema",
            message(
                "state schema {version}, this program needs {current}",
                &[
                    ("version", version.to_string()),
                    ("current", current.to_string()),
                ],
            ),
            migrate,
        ),
        Ok(None) => ok(
            "state_schema",
            message(
                "no schema recorded yet; a new state directory starts at {current}",
                &[("current", current.to_string())],
            ),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => ok(
            "state_schema",
            plain("not readable by this user; the service checks it at start"),
        ),
        Err(error) => fail("state_schema", error.to_string(), migrate),
    }
}

fn legacy_store(context: &Context) -> Check {
    let explicit = paths::state_explicit(&paths::process) || context.mode == paths::Mode::System;
    match state_root(context).and_then(|root| crate::store::Store::legacy_store(&root)) {
        Some(legacy) if !explicit => fail(
            "legacy_store",
            message(
                "an exec store exists at {path}; the service refuses to choose between it and the new one",
                &[("path", shown(&legacy))],
            ),
            plain(
                "stop the service and migrate offline with job state migrate; see the migration guide",
            ),
        ),
        Some(legacy) => ok(
            "legacy_store",
            message(
                "an exec store exists at {path}; the state directory is selected explicitly",
                &[("path", shown(&legacy))],
            ),
        ),
        None => ok(
            "legacy_store",
            plain("no exec store beside the state directory"),
        ),
    }
}

fn lock_held(path: &Path) -> std::io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    let device = metadata.dev();
    let (major, minor) = (libc::major(device), libc::minor(device));
    let locks = fs::read_to_string("/proc/locks")?;
    Ok(locks.lines().any(|line| {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(at) = words.iter().position(|word| *word == "FLOCK") else {
            return false;
        };
        let Some(place) = words.get(at + 4) else {
            return false;
        };
        let parts: Vec<&str> = place.split(':').collect();
        let [held_major, held_minor, inode] = parts.as_slice() else {
            return false;
        };
        inode.parse::<u64>().ok() == Some(metadata.ino())
            && (major == 0
                || (u32::from_str_radix(held_major, 16).ok() == Some(major)
                    && u32::from_str_radix(held_minor, 16).ok() == Some(minor)))
    }))
}

fn start_hint() -> String {
    plain(
        "start the service: systemctl start jobd, systemctl --user start jobd, sv up job, or jobd in a terminal",
    )
}

fn daemon_lock(context: &Context) -> Check {
    let Some(root) = state_root(context) else {
        return fail(
            "daemon_lock",
            plain("the state directory cannot be located"),
            plain("set the named variable in the environment of the service and of its clients"),
        );
    };
    let path = root.join("daemon.lock");
    let reachable = context.host.is_some();
    match lock_held(&path) {
        Ok(true) if reachable => ok(
            "daemon_lock",
            message("a service holds {path}", &[("path", shown(&path))]),
        ),
        Ok(true) => fail(
            "daemon_lock",
            message(
                "a process holds {path} but no service answers",
                &[("path", shown(&path))],
            ),
            plain(
                "a service is starting, migrating or hung; look at its log, then restart it through its service manager",
            ),
        ),
        Ok(false) => warn(
            "daemon_lock",
            message("nothing holds {path}", &[("path", shown(&path))]),
            start_hint(),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => warn(
            "daemon_lock",
            message(
                "{path} does not exist; no service has run on this state",
                &[("path", shown(&path))],
            ),
            start_hint(),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => ok(
            "daemon_lock",
            plain("not readable by this user; the lock lives in the private state directory"),
        ),
        Err(error) => warn(
            "daemon_lock",
            format!("{}: {error}", shown(&path)),
            start_hint(),
        ),
    }
}

fn group_name(id: u32) -> String {
    let mut buffer = vec![0u8; 4096];
    let mut group: libc::group = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::group = std::ptr::null_mut();
    let result = unsafe {
        libc::getgrgid_r(
            id,
            &mut group,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut found,
        )
    };
    if result == 0 && !found.is_null() {
        unsafe { std::ffi::CStr::from_ptr(group.gr_name) }
            .to_string_lossy()
            .into_owned()
    } else {
        id.to_string()
    }
}

fn user_name(id: u32) -> Option<String> {
    let mut buffer = vec![0u8; 4096];
    let mut user: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let result = unsafe {
        libc::getpwuid_r(
            id,
            &mut user,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut found,
        )
    };
    (result == 0 && !found.is_null()).then(|| {
        unsafe { std::ffi::CStr::from_ptr(user.pw_name) }
            .to_string_lossy()
            .into_owned()
    })
}

fn socket(context: &Context) -> Check {
    let Some(path) = &context.socket else {
        return fail(
            "socket",
            plain("the socket path cannot be located"),
            plain("set the named variable in the environment of the service and of its clients"),
        );
    };
    let rule = paths::runtime(context.mode, &paths::process)
        .map(|runtime| plain(runtime.rule.name()))
        .unwrap_or_default();
    if path.as_os_str().len() >= SOCKET_PATH_LIMIT {
        return fail(
            "socket",
            message(
                "{path} is longer than a socket address allows ({limit} bytes)",
                &[
                    ("path", shown(path)),
                    ("limit", (SOCKET_PATH_LIMIT - 1).to_string()),
                ],
            ),
            plain("choose a shorter JOB_RUNTIME_DIR or JOB_STATE_DIR"),
        );
    }
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) => {
            return warn(
                "socket",
                message(
                    "no socket at {path} ({error}); path chosen by {rule}",
                    &[
                        ("path", shown(path)),
                        ("error", error.to_string()),
                        ("rule", rule),
                    ],
                ),
                start_hint(),
            );
        }
    };
    if !meta.file_type().is_socket() {
        return fail(
            "socket",
            message("{path} is not a socket", &[("path", shown(path))]),
            plain("remove that file and restart the service"),
        );
    }
    let mode = meta.mode() & 0o777;
    let group = context.group();
    let expected = if group.is_some() { 0o660 } else { 0o600 };
    let detail = message(
        "{path}, owner {owner}, group {group}, mode {mode}; path chosen by {rule}",
        &[
            ("path", shown(path)),
            (
                "owner",
                user_name(meta.uid()).unwrap_or_else(|| meta.uid().to_string()),
            ),
            ("group", group_name(meta.gid())),
            ("mode", format!("{mode:04o}")),
            ("rule", rule),
        ],
    );
    if mode == expected {
        ok("socket", detail)
    } else {
        warn(
            "socket",
            detail,
            message(
                "the configuration asks for mode {mode}; restart the service so that it sets the socket up again",
                &[("mode", format!("{expected:04o}"))],
            ),
        )
    }
}

fn daemon(context: &Context) -> Check {
    let Some(host) = &context.host else {
        return fail(
            "daemon",
            match (&context.socket, &context.host_error) {
                (Some(socket), Some(error)) => message(
                    "no answer at {path}: {error}",
                    &[("path", shown(socket)), ("error", error.clone())],
                ),
                _ => plain("the socket path cannot be located"),
            },
            start_hint(),
        );
    };
    let version = crate::daemon::version();
    let protocol = crate::model::PROTOCOL;
    let detail = message(
        "service {version}, protocol {protocol}; this command {own_version}, protocol {own_protocol}",
        &[
            ("version", host.version.clone()),
            ("protocol", host.protocol.to_string()),
            ("own_version", version.clone()),
            ("own_protocol", protocol.to_string()),
        ],
    );
    let restart = plain(
        "restart the service after installing, so that the service and the command are the same build",
    );
    if host.protocol != protocol {
        fail("daemon", detail, restart)
    } else if host.version != version {
        warn("daemon", detail, restart)
    } else {
        ok("daemon", detail)
    }
}

fn health(context: &Context) -> Check {
    use crate::operations::{health, message as text};
    let Some(host) = &context.host else {
        return warn(
            "health",
            text("no health report, because the service does not answer", &[]),
            start_hint(),
        );
    };
    let Some(report) = &host.health else {
        return warn(
            "health",
            text(
                "the service answered without a health report; it is an older build",
                &[],
            ),
            text(
                "restart the service after installing, so that it reports its health",
                &[],
            ),
        );
    };
    let problems = health::problems(report);
    if problems.is_empty() {
        ok("health", health::summary(report))
    } else {
        fail(
            "health",
            problems.join("; "),
            text(
                "free space in the state directory and check that the service user can write audit/ and events/ in it; then run job doctor again",
                &[],
            ),
        )
    }
}

fn enforcement(context: &Context) -> Check {
    use crate::operations::message as text;
    match context.host.as_ref().map(|host| host.backend) {
        Some(crate::model::Backend::Cgroup) => ok(
            "enforcement",
            text(
                "enforcing mode: the service enforces limits through its delegated cgroup",
                &[],
            ),
        ),
        Some(crate::model::Backend::Watch) => warn(
            "enforcement",
            text(
                "monitoring mode: the service has no delegated cgroup; limits are watched, not enforced, and with the ordinary profile a Job that asks for --mem, --pids or a cgroup control is refused",
                &[],
            ),
            text(
                "delegate a cgroup to the service, see jobd(8); set [cgroup] required = true so that the service refuses to start without one",
                &[],
            ),
        ),
        None => warn(
            "enforcement",
            text("the mode is known only from a running service", &[]),
            start_hint(),
        ),
    }
}

fn surrounding_limits(context: &Context) -> Check {
    use crate::operations::{limits, message as text};
    let reported = context
        .host
        .as_ref()
        .and_then(|host| host.surrounding_limits.clone());
    let own = reported.is_none();
    let found = reported.unwrap_or_else(limits::read_own);
    let detail = limits::render(&found).join("; ");
    if !found.discoverable {
        return warn(
            "surrounding_limits",
            text(
                "the cgroup of the service cannot be read, so limits set around it are not discoverable; {limits}",
                &[("limits", detail)],
            ),
            text(
                "mount cgroup v2 at /sys/fs/cgroup readable for the service user",
                &[],
            ),
        );
    }
    if own {
        ok(
            "surrounding_limits",
            text(
                "limits of this command, not of a running service: {limits}",
                &[("limits", detail)],
            ),
        )
    } else {
        ok("surrounding_limits", detail)
    }
}

fn access(context: &Context) -> Check {
    let own = unsafe { libc::getuid() };
    let service_uid = context.service().map(|info| info.uid);
    match (context.group(), service_uid) {
        (None, _) => ok(
            "access",
            plain("the socket is private: only the service user connects"),
        ),
        (Some(group), Some(uid)) if uid != own => ok(
            "access",
            message(
                "members of group {group} control every Job, Queue, Group and terminal; you are not the service user, so run, logs and log get output from the service and attach uses the terminal socket in the runtime directory; job audit and job events read the private state directory and stay with the service user",
                &[("group", group)],
            ),
        ),
        (Some(group), _) => ok(
            "access",
            message(
                "members of group {group} control every Job, Queue, Group and terminal; they get output from the service and terminals through the runtime directory; job audit and job events read the private state directory and stay with the service user",
                &[("group", group)],
            ),
        ),
    }
}

fn systemd() -> bool {
    Path::new("/run/systemd/system").is_dir()
}

fn service_manager() -> Check {
    let mut found = Vec::new();
    if systemd() {
        found.push(plain("systemd is the init system"));
    }
    if std::env::var_os("INVOCATION_ID").is_some() {
        found.push(plain("this command runs inside a systemd unit"));
    }
    if let Some(directory) = std::env::var_os("SVDIR") {
        found.push(message(
            "runit service directory {path}",
            &[("path", directory.to_string_lossy().into_owned())],
        ));
    } else if Path::new("/run/runit").is_dir() {
        found.push(plain("runit runs on this host"));
    }
    if systemd() || std::env::var_os("INVOCATION_ID").is_some() {
        found.push(plain(
            "this release was never run under systemd; its unit files are a starting point",
        ));
        return warn(
            "service_manager",
            found.join("; "),
            plain(
                "verify once: start the unit, run job doctor, run a Job, systemctl restart jobd, and confirm that the Job kept running; see the systemd part of the administration guide",
            ),
        );
    }
    if found.is_empty() {
        found.push(plain(
            "neither systemd nor runit found; run jobd in the foreground under your supervisor",
        ));
    }
    ok("service_manager", found.join("; "))
}

fn lingering(context: &Context) -> Option<Check> {
    if !systemd() || context.mode != paths::Mode::User {
        return None;
    }
    let name = user_name(unsafe { libc::getuid() })?;
    Some(
        if Path::new("/var/lib/systemd/linger").join(&name).exists() {
            ok(
                "lingering",
                message("lingering is enabled for {user}", &[("user", name)]),
            )
        } else {
            warn(
                "lingering",
                message(
                    "lingering is not enabled for {user}; a user service and its Jobs stop at the last logout",
                    &[("user", name.clone())],
                ),
                message("loginctl enable-linger {user}", &[("user", name)]),
            )
        },
    )
}

pub fn examine() -> Report {
    let context = Context::gather();
    let mut checks = vec![
        locations(&context),
        configuration(&context),
        command_policy(),
        state_directory(&context),
        state_schema(&context),
        legacy_store(&context),
        daemon_lock(&context),
        socket(&context),
        daemon(&context),
        health(&context),
        access(&context),
        cgroup_mounted(),
        cgroup_own(),
        cgroup_root(&context),
        enforcement(&context),
        controllers(&context),
        surrounding_limits(&context),
        io_devices(&context),
        freezer(&context),
        pressure(),
        user_namespaces(),
        landlock(),
    ];
    checks.extend(security());
    checks.push(pidfd());
    checks.push(network_tools());
    checks.extend(network_names(&context));
    checks.push(service_manager());
    checks.extend(lingering(&context));
    Report {
        schema_version: 1,
        checks,
    }
}

pub fn command(arguments: &[String]) -> Result<ExitCode, String> {
    let mut json = false;
    for argument in arguments {
        match argument.as_str() {
            "--json" => json = true,
            "--system" => paths::enter_system_mode(),
            _ => return Err(plain("usage: job doctor [--json] [--system]")),
        }
    }
    let report = examine();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
    } else {
        for check in &report.checks {
            let fix = check
                .fix
                .as_ref()
                .map(|fix| format!(" {} {fix}.", plain("Do:")))
                .unwrap_or_default();
            println!(
                "{:<4} {}: {}.{fix}",
                check.status.name(),
                check.name,
                check.detail
            );
        }
    }
    Ok(
        if report
            .checks
            .iter()
            .any(|check| check.status == Status::Fail)
        {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        },
    )
}
