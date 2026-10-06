use std::ffi::CString;
use std::io::Write;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::cgroup::Tree;
use crate::model::{ShimResult, Usage};
use crate::procs;
use crate::store::Store;
use crate::streams::Recorder;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn children_usage() -> Usage {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, &mut usage) };
    let ms = |t: libc::timeval| t.tv_sec as u64 * 1000 + t.tv_usec as u64 / 1000;
    Usage {
        cpu_ms: ms(usage.ru_utime) + ms(usage.ru_stime),
        peak_memory: usage.ru_maxrss as u64 * 1024,
        ..Usage::default()
    }
}

fn enter_cgroup(procs_file: &CString) -> std::io::Result<()> {
    let fd = unsafe { libc::open(procs_file.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let written = unsafe { libc::write(fd, b"0".as_ptr().cast(), 1) };
    let error = std::io::Error::last_os_error();
    unsafe { libc::close(fd) };
    if written == 1 { Ok(()) } else { Err(error) }
}

fn reap_children(kept: &[i32]) {
    let own = std::process::id() as i32;
    let deadline = Instant::now() + EMPTY_WAIT;
    loop {
        let mut status = 0;
        let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if pid > 0 {
            continue;
        }
        if pid < 0 {
            break;
        }
        let others = procs::all()
            .iter()
            .any(|s| s.parent == own && !kept.contains(&s.pid));
        if !others || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub const HELPERS: [&str; 5] = ["sccache", "gpg-agent", "keyboxd", "ssh-agent", "dirmngr"];
const EMPTY_WAIT: Duration = Duration::from_secs(5);

const ARGUMENTS_TRIES: u32 = 10;
const ARGUMENTS_PAUSE: Duration = Duration::from_millis(2);

fn arguments(pid: i32) -> String {
    for attempt in 0..ARGUMENTS_TRIES {
        let Ok(bytes) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            return String::new();
        };
        let command = bytes
            .split(|b| *b == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        if !command.is_empty() || attempt + 1 == ARGUMENTS_TRIES {
            return command;
        }
        std::thread::sleep(ARGUMENTS_PAUSE);
    }
    String::new()
}

fn describe(pid: i32) -> String {
    let command = arguments(pid);
    let command = if command.is_empty() {
        name_of(pid)
    } else {
        command
    };
    let short: String = command.chars().take(80).collect();
    format!("{short} (pid {pid})")
}

fn name_of(pid: i32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|t| t.trim().to_string())
        .unwrap_or_default()
}

pub fn is_helper(name: &str) -> bool {
    HELPERS.contains(&name)
}

struct Leftovers {
    killed: Vec<String>,
    kept: Vec<String>,
    kept_pids: Vec<i32>,
}

fn handle_leftovers(cgroup: Option<&Path>, init: Option<i32>) -> Leftovers {
    let own = std::process::id() as i32;
    let processes = match cgroup {
        Some(path) => Tree::processes(path)
            .into_iter()
            .filter_map(procs::stat_of)
            .collect(),
        None => procs::descendants(own, &procs::all()),
    };
    let helpers = cgroup.and_then(|p| p.parent()?.parent().map(|exec| exec.join("helpers")));
    let mut leftovers = Leftovers {
        killed: Vec::new(),
        kept: Vec::new(),
        kept_pids: Vec::new(),
    };
    for process in processes {
        let pid = process.pid;
        if init == Some(pid) {
            continue;
        }
        let Ok(handle) = crate::process::Handle::open(pid, Some(process.start_ticks)) else {
            continue;
        };
        let name = name_of(pid);
        let moved = init.is_none()
            && is_helper(&name)
            && helpers.as_ref().is_some_and(|dir| {
                (dir.exists() || std::fs::create_dir(dir).is_ok())
                    && std::fs::write(dir.join("cgroup.procs"), pid.to_string()).is_ok()
            });
        if moved {
            leftovers.kept.push(describe(pid));
            leftovers.kept_pids.push(pid);
        } else {
            leftovers.killed.push(describe(pid));
            if cgroup.is_none() {
                let _ = handle.signal(libc::SIGKILL);
            }
        }
    }
    if let Some(path) = cgroup
        && !leftovers.killed.is_empty()
    {
        let _ = Tree::kill(path);
    }
    leftovers
}

fn wait_until_empty(cgroup: &Path) {
    let deadline = Instant::now() + EMPTY_WAIT;
    while Tree::is_populated(cgroup) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn submit_remote(
    target: &crate::model::Remote,
    job: &crate::model::Job,
    session: &str,
) -> Result<crate::remote::Submitted, String> {
    let declared = &job.spec.declared;
    let op = crate::remote::Op::Submit {
        argv: job.spec.argv.clone(),
        dir: declared.dir.clone(),
        declared: Box::new(crate::netsecret::for_remote(
            crate::resource_policy::for_remote(declared, &job.resource_sources),
        )?),
        session: session.to_string(),
        send: !declared.send.is_empty(),
    };
    let output = if declared.send.is_empty() {
        crate::remote::call(target, op, None)?
    } else {
        let mut tar = Command::new("tar")
            .arg("-c")
            .arg("-C")
            .arg(&job.spec.cwd)
            .arg("--")
            .args(&declared.send)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run tar: {e}"))?;
        let mut packed = tar.stdout.take().ok_or("tar gave no output")?;
        let sent = crate::remote::call(target, op, Some(&mut packed));
        let packed_ok = tar.wait().is_ok_and(|s| s.success());
        if !packed_ok {
            return Err(format!(
                "tar could not pack {:?} in {}",
                declared.send,
                job.spec.cwd.display()
            ));
        }
        sent?
    };
    serde_json::from_slice(&output)
        .map_err(|e| format!("{} answered what this job cannot read: {e}", target.target))
}

fn conclude_remote(
    target: &crate::model::Remote,
    job: &crate::model::Job,
    submitted: &crate::remote::Submitted,
    session: &str,
    stopped_here: bool,
    result: &mut ShimResult,
) {
    let status = || -> Option<crate::model::Job> {
        crate::remote::call(
            target,
            crate::remote::Op::AttemptStatus {
                id: submitted.id,
                attempt: submitted.attempt,
            },
            None,
        )
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    };
    let mut remote_job = status();
    let unfinished = remote_job.as_ref().is_some_and(|r| !r.state.terminal());
    if unfinished && stopped_here {
        let _ = crate::remote::call(
            target,
            crate::remote::Op::Cancel {
                id: submitted.id,
                session: session.to_string(),
            },
            None,
        );
        std::thread::sleep(Duration::from_secs(1));
        remote_job = status();
    } else if unfinished {
        result.remote_lines.push(format!(
            "the connection to {} ended while its job {} still ran there; it runs on",
            target.target, submitted.id
        ));
    }
    result.remote = Some(match &remote_job {
        Some(remote) => format!(
            "ran on {} as its job {} in {}, {}",
            target.target,
            submitted.id,
            submitted.dir.display(),
            remote.backend.describe()
        ),
        None => format!("ran on {} as its job {}", target.target, submitted.id),
    });
    if let Some(remote) = &remote_job {
        if result.signal.is_none() && remote.state.terminal() {
            if let Some(remote_result) = &remote.result {
                result.exit_code = remote_result.exit_code;
                result.signal = remote_result.signal;
                result.start_error = remote_result.start_error.clone();
                result.process_controls = remote_result.process_controls.clone();
                result.security_controls = remote_result.security_controls.clone();
                result.isolation_controls = remote_result.isolation_controls.clone();
                if result.output_error.is_none() {
                    result.output_error = remote_result.output_error.clone();
                }
            }
            if remote.stop.is_some() || remote.state == crate::model::State::Cancelled {
                result.exit_code = Some(1);
            }
            if remote.state == crate::model::State::Lost {
                result.exit_code = Some(125);
            }
        }
        if let Some(stop) = &remote.stop {
            result.remote_lines.push(format!("there: {}", stop.line));
        }
    }
    let wanted = &job.spec.declared.fetch;
    if wanted.is_empty() {
        return;
    }
    let fetched = crate::remote::ssh(target)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut ssh| {
            if let Some(mut stdin) = ssh.stdin.take() {
                stdin.write_all(&crate::remote::request_line(crate::remote::Op::Fetch {
                    dir: submitted.dir.clone(),
                    paths: wanted.clone(),
                }))?;
            }
            let output = ssh
                .stdout
                .take()
                .ok_or_else(|| std::io::Error::other("no output"))?;
            let unpacked = Command::new("tar")
                .arg("-x")
                .arg("-C")
                .arg(&job.spec.cwd)
                .stdin(Stdio::from(output))
                .status()?;
            let transferred = ssh.wait()?;
            Ok(unpacked.success() && transferred.success())
        });
    result.remote_lines.push(match fetched {
        Ok(true) => format!(
            "fetched {} into {}",
            wanted
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            job.spec.cwd.display()
        ),
        Ok(false) => format!("could not fetch {:?} from {}", wanted, target.target),
        Err(e) => format!("could not fetch {:?} from {}: {e}", wanted, target.target),
    });
}

fn monitor_for(
    display_name: Option<&str>,
    wanted: &str,
) -> Result<(String, crate::display::Monitor), String> {
    let display = crate::display::open(display_name).map_err(|e| e.to_string())?;
    let monitors = display.monitors().map_err(|e| e.to_string())?;
    let found = crate::display::choose(&monitors, wanted)?.clone();
    Ok((display.name, found))
}

const PLACE_POLL: Duration = Duration::from_millis(100);
const PLACE_SETTLE: Duration = Duration::from_millis(300);

fn place_windows(
    display_name: &str,
    target: &crate::display::Monitor,
    root: i32,
    running: &std::sync::atomic::AtomicBool,
) -> Vec<String> {
    let mut lines = Vec::new();
    let Ok(display) = crate::display::open(Some(display_name)) else {
        return vec![format!(
            "could not open {display_name} to place the job's windows"
        )];
    };
    let mut handled = std::collections::HashSet::new();
    while running.load(std::sync::atomic::Ordering::Relaxed) {
        let mut pids: Vec<i32> = procs::descendants(root, &procs::all())
            .iter()
            .map(|s| s.pid)
            .collect();
        pids.push(root);
        let windows = display.windows_of(&pids).unwrap_or_default();
        let monitors = display.monitors().unwrap_or_default();
        for window in windows {
            if !handled.insert(window) {
                continue;
            }
            let Ok((x, y, _, _)) = display.position_of(window) else {
                continue;
            };
            let current = crate::display::monitor_at(&monitors, x, y);
            if current.is_some_and(|m| m.name == target.name) {
                continue;
            }
            let (dx, dy) = current.map_or((0, 0), |m| (x - m.x, y - m.y));
            let _ = display.move_window(window, target.x + dx, target.y + dy);
            std::thread::sleep(PLACE_SETTLE);
            let from =
                current.map_or_else(|| "outside every monitor".to_string(), |m| m.name.clone());
            let now_on = display
                .position_of(window)
                .ok()
                .and_then(|(x, y, _, _)| crate::display::monitor_at(&monitors, x, y))
                .map(|m| m.name.clone());
            lines.push(match now_on {
                Some(name) if name == target.name => {
                    format!("window 0x{window:x} opened on {from} and was moved to {}", target.name)
                }
                other => format!(
                    "window 0x{window:x} opened on {from} and stayed on {}: the window manager did not move it to {}",
                    other.unwrap_or_else(|| "no monitor".to_string()),
                    target.name
                ),
            });
        }
        std::thread::sleep(PLACE_POLL);
    }
    lines
}

pub fn run(store: &Store, id: u64, cgroup: Option<PathBuf>) -> i32 {
    unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
    let Some(job) = store.load_job(id) else {
        return 2;
    };
    let env = store.load_env(id);
    let recording = std::cell::RefCell::new(None::<Recorder>);
    let finish = |mut result: ShimResult| {
        if let Some(log) = recording.take() {
            let _ = log.finish();
        }
        result.attempt = Some(job.attempt);
        crate::durability::exit::save_result(store, id, &result);
        let _ = crate::client::call(&store.socket(), &crate::model::Request::Done { id });
        if let Some(path) = &cgroup {
            let _ = Tree::remove(path);
        }
    };
    let start_error = |error: String| ShimResult {
        start_error: Some(error),
        finished_ms: now_ms(),
        ..ShimResult::default()
    };

    match Recorder::create(&job, &store.log_file(id)) {
        Ok(log) => {
            recording.replace(Some(log));
        }
        Err(e) => {
            finish(start_error(format!("cannot create the log: {e}")));
            return 1;
        }
    };
    let (reader, writer) = match std::io::pipe() {
        Ok(pair) => pair,
        Err(e) => {
            finish(start_error(format!("cannot create a pipe: {e}")));
            return 1;
        }
    };
    let terminal = match job.spec.declared.terminal {
        Some(size) => {
            match crate::terminal::Pty::open(
                &crate::operations::access::supervisor_terminal(store, id),
                size,
            ) {
                Ok(terminal) => Some(terminal),
                Err(e) => {
                    finish(start_error(crate::terminal::message(
                        "cannot create terminal: {error}",
                        &[("error", e.to_string())],
                    )));
                    return 1;
                }
            }
        }
        None => None,
    };
    let remote = job.spec.declared.on.clone();
    let remote_session = format!("remote:{}:{id}", crate::host::hostname());
    let submitted = match &remote {
        Some(target) => match submit_remote(target, &job, &remote_session) {
            Ok(submitted) => Some(submitted),
            Err(e) => {
                finish(start_error(e));
                return 1;
            }
        },
        None => None,
    };
    let Some((program, arguments)) = job.spec.argv.split_first() else {
        finish(start_error("the command is empty".to_string()));
        return 1;
    };
    let mut command = match (&remote, &submitted) {
        (Some(target), Some(_)) => crate::remote::ssh(target),
        _ => {
            let mut local = Command::new(program);
            local.args(arguments);
            local
        }
    };
    let input = if crate::cli2::stdin::wanted(&job) {
        match crate::cli2::stdin::listen(&store.job_dir(id)) {
            Ok(listener) => Some(listener),
            Err(e) => {
                finish(start_error(
                    crate::cli2::message("cannot open the input of the Job: {error}")
                        .replace("{error}", &e.to_string()),
                ));
                return 1;
            }
        }
    } else {
        None
    };
    command
        .current_dir(&job.spec.cwd)
        .stdin(if submitted.is_some() || input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let (error_reader, error_writer) = match std::io::pipe() {
        Ok(pair) => pair,
        Err(e) => {
            finish(start_error(format!("cannot create a pipe: {e}")));
            return 1;
        }
    };
    command.stdout(writer).stderr(error_writer);
    if let Some(env) = &env {
        command
            .env_clear()
            .envs(env.vars.iter().map(|(k, v)| (k, v)));
    }
    if let Some(terminal) = &terminal
        && let Err(e) = terminal.configure(&mut command)
    {
        finish(start_error(crate::terminal::message(
            "cannot attach terminal: {error}",
            &[("error", e.to_string())],
        )));
        return 1;
    }
    let monitor = match job
        .spec
        .declared
        .monitor
        .as_ref()
        .filter(|_| job.spec.declared.on.is_none())
    {
        Some(wanted) => {
            let display_name = env.as_ref().and_then(|e| {
                e.vars
                    .iter()
                    .find(|(k, _)| k == "DISPLAY")
                    .map(|(_, v)| v.clone())
            });
            match monitor_for(display_name.as_deref(), wanted) {
                Ok((display, found)) => {
                    command
                        .env("DISPLAY", &display)
                        .env("JOB_MONITOR", found.geometry())
                        .env("JOB_MONITOR_NAME", &found.name);
                    Some((display, found))
                }
                Err(e) => {
                    finish(start_error(e));
                    return 1;
                }
            }
        }
        None => None,
    };
    let ruleset = if submitted.is_none() && job.spec.declared.confine {
        let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
        let cwd = &job.spec.cwd;
        let mut places = crate::confine::default_places(
            cwd,
            &home,
            &crate::confine::tree_of(cwd),
            &crate::estimate::repository_of(cwd),
        );
        places.extend(job.spec.declared.allow_write.iter().map(|p| cwd.join(p)));
        places.extend(crate::isolation::kernel::confined_places(
            &job.spec.declared.isolation,
            &store.job_dir(id),
            job.attempt,
        ));
        match crate::confine::Ruleset::build(&places) {
            Ok(ruleset) => Some(ruleset),
            Err(e) => {
                finish(start_error(format!("cannot confine the job: {e}")));
                return 1;
            }
        }
    } else {
        None
    };
    let ruleset_fd = ruleset.as_ref().map(crate::confine::Ruleset::fd);
    let network = job.network.as_ref().filter(|_| submitted.is_none());
    let no_network = (submitted.is_none()
        && (matches!(job.spec.declared.net, Some(crate::model::Net::None))
            || network.is_some_and(crate::netpolicy::Applied::isolated)))
    .then(crate::isolate::NoNetwork::prepare);
    let joined = match network.and_then(|network| network.namespace.as_ref()) {
        Some(place) => match crate::netpolicy::enter(place) {
            Ok(joined) => Some(joined),
            Err(e) => {
                finish(start_error(e));
                return 1;
            }
        },
        None => None,
    };
    let linked = match &job.link {
        Some(link) => {
            let resolver = store.job_dir(id).join("resolv.conf");
            let declared = match network {
                Some(network) => network.secret(&job.spec.declared),
                None => job.spec.declared.clone(),
            };
            match crate::netsecret::job_environment(&declared, &link.env) {
                Ok(variables) => {
                    command.envs(variables);
                }
                Err(e) => {
                    finish(start_error(e));
                    return 1;
                }
            }
            let prepared = std::fs::write(&resolver, &link.resolver).and_then(|_| {
                crate::isolate::LinkedNetwork::prepare(link.job.holder.pid, &resolver)
            });
            match prepared {
                Ok(linked) => Some(linked),
                Err(e) => {
                    finish(start_error(format!("cannot join its shaped network: {e}")));
                    return 1;
                }
            }
        }
        None => None,
    };
    let controls = if submitted.is_some() {
        crate::process_policy::Controls::default()
    } else {
        job.spec.declared.process.clone()
    };
    let mut process_controls = match crate::process_policy::kernel::Prepared::new(&controls) {
        Ok(prepared) => prepared,
        Err(error) => {
            finish(start_error(error));
            return 1;
        }
    };
    let applied_process = process_controls.applied.clone();
    let security = if submitted.is_some() {
        crate::security::Controls::default()
    } else {
        job.spec.declared.security.clone()
    };
    let isolation = if submitted.is_some() {
        crate::isolation::Controls::default()
    } else {
        job.spec.declared.isolation.clone()
    };
    let in_user_namespace = no_network.is_some()
        || linked.is_some()
        || joined
            .as_ref()
            .is_some_and(crate::netpolicy::Joined::enters_user_namespace);
    let isolation_controls = match crate::isolation::kernel::Prepared::new(
        &isolation,
        &security,
        in_user_namespace,
        &store.job_dir(id),
        job.attempt,
        &job.spec.cwd,
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            finish(start_error(error));
            return 1;
        }
    };
    let mut isolation_handle = isolation_controls.handle();
    let own_init = isolation.has("pid");
    let security_controls = match crate::security::kernel::Prepared::new(
        &security,
        in_user_namespace || isolation.enters_user_namespace(),
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            finish(start_error(error));
            return 1;
        }
    };
    let applied_security = security_controls.applied.clone();
    let procs_file = cgroup
        .as_ref()
        .and_then(|p| CString::new(p.join("cgroup.procs").as_os_str().as_encoded_bytes()).ok());
    let has_terminal = terminal.is_some();
    unsafe {
        let lead = move || {
            if has_terminal {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            } else {
                libc::setpgid(0, 0);
            }
            Ok(())
        };
        command.pre_exec(move || {
            if !own_init {
                lead()?;
            }
            if let Some(file) = &procs_file {
                enter_cgroup(file)?;
            }
            if let Some(isolation) = &no_network {
                isolation.enter()?;
            }
            if let Some(linked) = &linked {
                linked.enter()?;
            }
            if let Some(joined) = &joined {
                joined.enter()?;
            }
            isolation_controls.apply()?;
            if own_init {
                lead()?;
            }
            if let Some(fd) = ruleset_fd {
                crate::confine::restrict_self(fd)?;
            }
            process_controls.apply()?;
            security_controls.apply()?;
            Ok(())
        });
    }
    let spawned = command.spawn();
    drop(command);
    drop(ruleset);
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            isolation_handle.abandon();
            isolation_handle.cleanup();
            finish(start_error(
                isolation_handle
                    .explain(&e)
                    .unwrap_or_else(|| format!("{program}: {e}")),
            ));
            return 1;
        }
    };
    isolation_handle.adopt(&child);

    let placing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let placer = monitor.clone().map(|(display, target)| {
        let running = std::sync::Arc::clone(&placing);
        let root = isolation_handle.init().unwrap_or(child.id() as i32);
        std::thread::spawn(move || place_windows(&display, &target, root, &running))
    });
    let (terminal_exit, terminal_ended) = std::sync::mpsc::channel();
    let framed_remote = submitted.is_some();
    let Some(log) = recording.take() else {
        return 1;
    };
    let copier = std::thread::spawn(move || {
        if let Some(terminal) = terminal {
            return terminal.serve(log, terminal_ended);
        }
        crate::streams::capture::pipes(reader, error_reader, log, terminal_ended, framed_remote)
    });

    if let Some(listener) = input
        && let Some(stdin) = child.stdin.take()
    {
        crate::cli2::stdin::feed(listener, &store.job_dir(id), stdin);
    }
    if let (Some(submitted), Some(mut stdin)) = (&submitted, child.stdin.take()) {
        let _ = stdin.write_all(&crate::remote::request_line(
            crate::remote::Op::FollowStreams {
                id: submitted.id,
                attempt: submitted.attempt,
            },
        ));
    }
    let status = isolation_handle.wait(&mut child);
    if remote.is_none() {
        crate::durability::exit::record(store, id, job.attempt, &status);
    }
    crate::durability::failpoint("shim-after-exit-record");
    placing.store(false, std::sync::atomic::Ordering::Relaxed);
    let windows = placer.and_then(|p| p.join().ok()).unwrap_or_default();
    let leftovers = handle_leftovers(cgroup.as_deref(), isolation_handle.init());
    isolation_handle.release();
    if let Some(path) = &cgroup {
        wait_until_empty(path);
    }
    reap_children(&leftovers.kept_pids);
    let isolation_note = isolation_handle.cleanup();
    let terminal_code = status
        .as_ref()
        .ok()
        .map(|s| s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0)))
        .unwrap_or(125);
    let _ = terminal_exit.send(terminal_code);
    let deadline = Instant::now() + EMPTY_WAIT;
    while !copier.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let output_held_open = !copier.is_finished();
    let output = if output_held_open {
        crate::streams::Outcome::incomplete()
    } else {
        copier
            .join()
            .unwrap_or_else(|_| crate::streams::Outcome::incomplete())
    };

    let mut result = ShimResult {
        notes: isolation_note.into_iter().collect(),
        isolation_controls: isolation_handle.applied.clone(),
        security_controls: applied_security,
        process_controls: applied_process,
        monitor: monitor.map(|(_, m)| format!("{} {}", m.name, m.geometry())),
        windows,
        finished_ms: now_ms(),
        leftover_processes: leftovers.killed.len() as u64,
        leftover_names: leftovers.killed,
        kept_helpers: leftovers.kept,
        output_held_open,
        output_bytes: output.bytes,
        output_error: output.error,
        output_retention: output.retention,
        ..ShimResult::default()
    };
    if let Ok(status) = status {
        result.exit_code = status.code();
        result.signal = status.signal();
    }
    if let (Some(target), Some(submitted)) = (&remote, &submitted) {
        let stopped_here = store.load_job(id).is_some_and(|j| j.stop.is_some());
        conclude_remote(
            target,
            &job,
            submitted,
            &remote_session,
            stopped_here,
            &mut result,
        );
    }
    result.usage = match &cgroup {
        Some(path) => {
            let counters = Tree::counters(path);
            result.oom_group_kill = counters.oom_group_kill;
            result.oom_kill = counters.oom_kill;
            result.pids_max_events = counters.pids_max_events;
            Usage {
                peak_memory: counters.memory_peak,
                peak_pids: counters.pids_peak,
                cpu_ms: counters.cpu_usage_us / 1000,
                throttled_ms: counters.cpu_throttled_us / 1000,
                written: counters.written(),
            }
        }
        None => children_usage(),
    };
    finish(result);
    0
}
