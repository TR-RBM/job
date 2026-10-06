mod messages;
pub use messages::message;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const SOCKET: &str = "job.sock";
pub const STATE_SOCKET: &str = "daemon.sock";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    User,
    System,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::User => "user",
            Mode::System => "system",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketRule {
    RuntimeOverride,
    SystemRuntime,
    ExplicitState,
    SessionRuntime,
    NoSessionRuntime,
}

impl SocketRule {
    pub fn name(self) -> &'static str {
        match self {
            SocketRule::RuntimeOverride => "JOB_RUNTIME_DIR",
            SocketRule::SystemRuntime => "system runtime directory",
            SocketRule::ExplicitState => "JOB_STATE_DIR without JOB_RUNTIME_DIR",
            SocketRule::SessionRuntime => "XDG_RUNTIME_DIR",
            SocketRule::NoSessionRuntime => "state directory, XDG_RUNTIME_DIR is not set",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runtime {
    pub directory: PathBuf,
    pub socket: PathBuf,
    pub rule: SocketRule,
}

impl Runtime {
    pub fn separate(&self) -> bool {
        !matches!(
            self.rule,
            SocketRule::ExplicitState | SocketRule::NoSessionRuntime
        )
    }
}

pub type Lookup<'a> = &'a dyn Fn(&str) -> Option<OsString>;

pub fn process(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

fn set(env: Lookup, name: &str) -> Option<PathBuf> {
    env(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn absolute(env: Lookup, name: &str) -> Option<PathBuf> {
    set(env, name).filter(|path| path.is_absolute())
}

fn home(env: Lookup, relative: &str, missing: &str) -> Result<PathBuf, String> {
    absolute(env, "HOME")
        .map(|home| home.join(relative))
        .ok_or_else(|| message(missing, &[]))
}

pub fn mode(env: Lookup) -> Mode {
    if env("JOB_SYSTEM").is_some_and(|v| v == "1") {
        Mode::System
    } else {
        Mode::User
    }
}

pub fn config_file(mode: Mode, env: Lookup) -> Result<PathBuf, String> {
    if let Some(path) = set(env, "JOB_CONFIG") {
        return Ok(path);
    }
    let directory = match mode {
        Mode::System => PathBuf::from("/etc/job"),
        Mode::User => match absolute(env, "XDG_CONFIG_HOME") {
            Some(base) => base.join("job"),
            None => home(
                env,
                ".config/job",
                "cannot locate the configuration: set HOME, XDG_CONFIG_HOME or JOB_CONFIG",
            )?,
        },
    };
    Ok(directory.join("config.toml"))
}

pub fn policy_files(mode: Mode, env: Lookup) -> Vec<PathBuf> {
    const NAME: &str = "policy.json";
    let system = PathBuf::from("/etc/job").join(NAME);
    if let Some(path) = set(env, "JOB_CONFIG") {
        return vec![path.with_file_name(NAME)];
    }
    match (mode, config_file(mode, env)) {
        (Mode::User, Ok(path)) => vec![path.with_file_name(NAME), system],
        _ => vec![system],
    }
}

pub fn config_explicit(env: Lookup) -> bool {
    set(env, "JOB_CONFIG").is_some()
}

pub fn state_explicit(env: Lookup) -> bool {
    set(env, "JOB_STATE_DIR").is_some()
}

pub fn state(mode: Mode, env: Lookup) -> Result<PathBuf, String> {
    if let Some(path) = set(env, "JOB_STATE_DIR") {
        return Ok(path);
    }
    match mode {
        Mode::System => Ok(PathBuf::from("/var/lib/job")),
        Mode::User => match absolute(env, "XDG_STATE_HOME") {
            Some(base) => Ok(base.join("job")),
            None => home(
                env,
                ".local/state/job",
                "cannot locate the state directory: set HOME, XDG_STATE_HOME or JOB_STATE_DIR",
            ),
        },
    }
}

pub fn cache(mode: Mode, env: Lookup) -> Result<PathBuf, String> {
    if let Some(path) = set(env, "JOB_CACHE_DIR") {
        return Ok(path);
    }
    match mode {
        Mode::System => Ok(PathBuf::from("/var/cache/job")),
        Mode::User => match absolute(env, "XDG_CACHE_HOME") {
            Some(base) => Ok(base.join("job")),
            None => home(
                env,
                ".cache/job",
                "cannot locate the cache directory: set HOME, XDG_CACHE_HOME or JOB_CACHE_DIR",
            ),
        },
    }
}

fn beside_state(state: PathBuf, rule: SocketRule) -> Runtime {
    Runtime {
        socket: state.join(STATE_SOCKET),
        directory: state,
        rule,
    }
}

fn separate(directory: PathBuf, rule: SocketRule) -> Runtime {
    Runtime {
        socket: directory.join(SOCKET),
        directory,
        rule,
    }
}

pub fn runtime(mode: Mode, env: Lookup) -> Result<Runtime, String> {
    if let Some(path) = set(env, "JOB_RUNTIME_DIR") {
        return Ok(separate(path, SocketRule::RuntimeOverride));
    }
    if state_explicit(env) {
        return Ok(beside_state(state(mode, env)?, SocketRule::ExplicitState));
    }
    if mode == Mode::System {
        return Ok(separate(
            PathBuf::from("/run/job"),
            SocketRule::SystemRuntime,
        ));
    }
    match absolute(env, "XDG_RUNTIME_DIR") {
        Some(base) => Ok(separate(base.join("job"), SocketRule::SessionRuntime)),
        None => Ok(beside_state(
            state(mode, env)?,
            SocketRule::NoSessionRuntime,
        )),
    }
}

pub fn client_sockets(mode: Mode, env: Lookup, uid: u32) -> Result<Vec<PathBuf>, String> {
    let runtime = runtime(mode, env)?;
    let session = runtime.rule == SocketRule::NoSessionRuntime;
    let mut sockets = vec![runtime.socket];
    if let Ok(state) = state(mode, env) {
        let old = state.join(STATE_SOCKET);
        if !sockets.contains(&old) {
            sockets.push(old);
        }
    }
    if session {
        sockets.push(PathBuf::from(format!("/run/user/{uid}/job")).join(SOCKET));
    }
    Ok(sockets)
}

fn own_user() -> u32 {
    unsafe { libc::getuid() }
}

fn answers(socket: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(socket).is_ok()
}

pub fn choose(sockets: &[PathBuf]) -> Option<PathBuf> {
    let present: Vec<&PathBuf> = sockets.iter().filter(|path| path.exists()).collect();
    match present.as_slice() {
        [] => sockets.first().cloned(),
        [only] => Some((*only).clone()),
        several => several
            .iter()
            .find(|path| answers(path))
            .or(several.first())
            .map(|path| (*path).clone()),
    }
}

pub fn client_socket() -> Result<PathBuf, String> {
    let sockets = client_sockets(mode(&process), &process, own_user())?;
    choose(&sockets).ok_or_else(|| message("cannot locate the service socket", &[]))
}

pub fn socket_beside(state: &Path) -> PathBuf {
    let mut sockets = client_sockets(mode(&process), &process, own_user()).unwrap_or_default();
    let old = state.join(STATE_SOCKET);
    if !sockets.contains(&old) {
        sockets.push(old.clone());
    }
    choose(&sockets).unwrap_or(old)
}

pub fn enter_system_mode() {
    unsafe { std::env::set_var("JOB_SYSTEM", "1") };
}
