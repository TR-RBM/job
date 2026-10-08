pub mod access;
pub mod discovery;
mod messages;
pub use messages::message;

use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::store::Store;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Socket {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

impl Socket {
    pub fn unset(&self) -> bool {
        self.group.is_none()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cgroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
}

impl Cgroup {
    pub fn unset(&self) -> bool {
        self.root.is_none() && !self.required
    }
}

pub fn restart_needed(old: &crate::config::Config, new: &crate::config::Config) -> Option<String> {
    if old.metrics != new.metrics {
        return Some(crate::metrics::message(
            "changing the metrics listener requires a daemon restart",
        ));
    }
    (old.socket != new.socket || old.cgroup != new.cgroup).then(|| {
        message(
            "changing the socket group or the cgroup root requires a daemon restart",
            &[],
        )
    })
}

pub fn invoked_as_daemon(argv0: Option<&std::ffi::OsStr>) -> bool {
    argv0
        .map(Path::new)
        .and_then(Path::file_name)
        .is_some_and(|name| name == "jobd")
}

pub fn run(arguments: &[String]) -> Result<ExitCode, String> {
    for argument in arguments {
        match argument.as_str() {
            "--system" => paths::enter_system_mode(),
            "--foreground" => {}
            "--help" | "-h" => {
                println!("{}", message("usage: jobd [--system] [--foreground]", &[]));
                println!(
                    "{}",
                    message(
                        "Runs the job service in the foreground and logs to standard error. See jobd(8).",
                        &[]
                    )
                );
                return Ok(ExitCode::SUCCESS);
            }
            other => {
                return Err(format!(
                    "{}\n{}",
                    message(
                        "jobd: unknown option {option}",
                        &[("option", other.to_owned())]
                    ),
                    message("usage: jobd [--system] [--foreground]", &[])
                ));
            }
        }
    }
    Store::validate_default_selection().map_err(|e| e.to_string())?;
    let store = Store::open(Store::default_root())
        .map_err(|e| format!("cannot open the state directory: {e}"))?;
    crate::daemon::serve(store).map_err(|e| format!("daemon: {e}"))?;
    Ok(ExitCode::SUCCESS)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub uid: u32,
    #[serde(default)]
    pub state: PathBuf,
    #[serde(default)]
    pub socket: PathBuf,
    #[serde(default)]
    pub socket_rule: String,
    #[serde(default)]
    pub socket_group: Option<String>,
    #[serde(default)]
    pub cgroup_root: Option<PathBuf>,
    #[serde(default)]
    pub cgroup_rule: Option<String>,
    #[serde(default)]
    pub cgroup_skipped: Vec<String>,
}

static PUBLISHED: std::sync::OnceLock<Info> = std::sync::OnceLock::new();

pub fn published() -> Option<Info> {
    PUBLISHED.get().cloned()
}

pub fn announce(endpoint: &Endpoint, found: &discovery::Found, settings: &Socket) -> String {
    let _ = PUBLISHED.set(Info {
        mode: endpoint.mode.name().to_owned(),
        uid: unsafe { libc::getuid() },
        state: endpoint.state.clone(),
        socket: endpoint.runtime.socket.clone(),
        socket_rule: endpoint.runtime.rule.name().to_owned(),
        socket_group: settings.group.clone(),
        cgroup_root: found.tree.as_ref().map(|tree| tree.root.clone()),
        cgroup_rule: found.rule.map(|rule| rule.name().to_owned()),
        cgroup_skipped: found.skipped.clone(),
    });
    format!("{}; {}", endpoint.describe(settings), found.describe())
}

pub fn find_cgroup() -> io::Result<discovery::Found> {
    let (config, _) = crate::config::Config::load()?;
    let found = discovery::discover(config.cgroup.root.as_deref())?;
    if config.cgroup.required && found.tree.is_none() {
        return Err(io::Error::other(crate::operations::mode::missing(&found)));
    }
    Ok(found)
}

pub struct Endpoint {
    pub mode: paths::Mode,
    pub runtime: paths::Runtime,
    state: PathBuf,
    _claimed: Option<fs::File>,
}

const RUNTIME_LOCK: &str = "job.lock";

fn claim(directory: &Path, state: &Path) -> io::Result<fs::File> {
    use std::io::{Read, Seek, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)?;
    let path = directory.join(RUNTIME_LOCK);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let mut other = String::new();
        let _ = file.read_to_string(&mut other);
        return Err(io::Error::other(message(
            "another job service already serves the runtime directory {path}; its state directory is {state}; stop it first, or give this one its own JOB_RUNTIME_DIR",
            &[
                ("path", directory.display().to_string()),
                ("state", other.trim().to_owned()),
            ],
        )));
    }
    file.set_len(0)?;
    file.rewind()?;
    writeln!(file, "{}", state.display())?;
    Ok(file)
}

fn remove_socket(path: &Path) {
    if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_socket()) {
        let _ = fs::remove_file(path);
    }
}

impl Endpoint {
    pub fn plan(store: &Store) -> io::Result<Self> {
        let mode = paths::mode(&paths::process);
        let runtime = paths::runtime(mode, &paths::process).map_err(io::Error::other)?;
        let runtime = if runtime.separate() {
            runtime
        } else {
            paths::Runtime {
                directory: store.root.clone(),
                socket: store.root.join(paths::STATE_SOCKET),
                rule: runtime.rule,
            }
        };
        let claimed = if runtime.separate() {
            Some(claim(&runtime.directory, &store.root)?)
        } else {
            None
        };
        remove_socket(&store.root.join(paths::STATE_SOCKET));
        remove_socket(&runtime.socket);
        Ok(Self {
            mode,
            runtime,
            state: store.root.clone(),
            _claimed: claimed,
        })
    }

    pub fn listen(&self, settings: &Socket) -> io::Result<UnixListener> {
        let policy = access::Policy::resolve(settings)?;
        let shared = policy.group.is_some();
        if self.runtime.separate() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&self.runtime.directory)?;
            let owner = fs::metadata(&self.runtime.directory)?.uid();
            if owner != policy.uid {
                return Err(io::Error::other(message(
                    "the runtime directory {path} belongs to user {owner}, not to this service",
                    &[
                        ("path", self.runtime.directory.display().to_string()),
                        ("owner", owner.to_string()),
                    ],
                )));
            }
            policy.share(&self.runtime.directory, if shared { 0o750 } else { 0o700 })?;
        } else if shared {
            return Err(io::Error::other(message(
                "a group-shared socket needs a runtime directory apart from the private state directory; set JOB_RUNTIME_DIR or XDG_RUNTIME_DIR, or run with --system",
                &[],
            )));
        }
        let listener = UnixListener::bind(&self.runtime.socket)?;
        policy.share_socket(&self.runtime.socket)?;
        access::install(policy);
        Ok(listener)
    }

    pub fn describe(&self, settings: &Socket) -> String {
        let mut text = message(
            "{mode} service, state {state}, socket {socket}, chosen by {rule}",
            &[
                ("mode", message(self.mode.name(), &[])),
                ("state", self.state.display().to_string()),
                ("socket", self.runtime.socket.display().to_string()),
                ("rule", message(self.runtime.rule.name(), &[])),
            ],
        );
        if let Some(group) = &settings.group {
            text.push_str(", ");
            text.push_str(&message(
                "shared with group {group}",
                &[("group", group.clone())],
            ));
        }
        text
    }
}
