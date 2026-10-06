use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

#[allow(dead_code)]
mod support;

const JOB: &str = env!("CARGO_BIN_EXE_job");

struct Bench {
    base: PathBuf,
    children: Vec<Child>,
}

impl Bench {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("job-li-{}-{name}", std::process::id()));
        fs::create_dir_all(base.join("work")).unwrap();
        fs::write(
            base.join("config.toml"),
            "schema_version = 1\nprofile = 'ordinary'\n",
        )
        .unwrap();
        Self {
            base,
            children: Vec::new(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.base.join(name)
    }

    fn clean(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("LC_ALL", "C")
            .env("JOB_SESSION", "linux-integration")
            .env("JOB_CGROUP_ROOT", self.path("absent-cgroup"))
            .current_dir(self.path("work"));
        command
    }

    fn explicit(&self, program: &Path) -> Command {
        let mut command = self.clean(program);
        command
            .env("JOB_CONFIG", self.path("config.toml"))
            .env("JOB_STATE_DIR", self.path("state"));
        command
    }

    fn start(&mut self, mut command: Command, socket: &Path) -> bool {
        let errors = fs::File::create(self.path("daemon.err")).unwrap();
        let mut child = command
            .stdout(Stdio::null())
            .stderr(Stdio::from(errors))
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let started = loop {
            if child.try_wait().unwrap().is_some() {
                break false;
            }
            if socket.exists() && std::os::unix::net::UnixStream::connect(socket).is_ok() {
                break true;
            }
            assert!(Instant::now() < deadline, "{}", self.errors());
            std::thread::sleep(Duration::from_millis(10));
        };
        self.children.push(child);
        started
    }

    fn errors(&self) -> String {
        fs::read_to_string(self.path("daemon.err")).unwrap_or_default()
    }

    fn said(&self, text: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.errors().contains(text) {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn both(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn report(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", both(output)))
}

fn check<'a>(report: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == name)
        .unwrap_or_else(|| panic!("no check {name} in {report}"))
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().mode() & 0o7777
}

#[test]
fn user_paths_follow_the_xdg_fallbacks_and_overrides() {
    let bench = Bench::new("xdg");
    let home = bench.path("home");
    let mut command = bench.clean(Path::new(JOB));
    let output = command
        .args(["doctor", "--json"])
        .env("HOME", &home)
        .output()
        .unwrap();
    let found = report(&output);
    assert_eq!(found["schema_version"], 1);
    let paths = check(&found, "paths");
    assert_eq!(paths["status"], "ok");
    let detail = paths["detail"].as_str().unwrap();
    for expected in [
        format!("configuration {}/.config/job/config.toml", home.display()),
        format!("state {}/.local/state/job", home.display()),
        format!("runtime {}/.local/state/job", home.display()),
        format!("cache {}/.cache/job", home.display()),
    ] {
        assert!(detail.contains(&expected), "{detail}");
    }
    let socket = check(&found, "socket")["detail"].as_str().unwrap();
    assert!(socket.contains("XDG_RUNTIME_DIR is not set"), "{socket}");

    let mut command = bench.clean(Path::new(JOB));
    let output = command
        .args(["doctor", "--json"])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", bench.path("c"))
        .env("XDG_STATE_HOME", bench.path("s"))
        .env("XDG_RUNTIME_DIR", bench.path("r"))
        .env("XDG_CACHE_HOME", "relative/cache")
        .output()
        .unwrap();
    let found = report(&output);
    let detail = check(&found, "paths")["detail"].as_str().unwrap();
    for expected in [
        format!(
            "configuration {}/job/config.toml",
            bench.path("c").display()
        ),
        format!("state {}/job", bench.path("s").display()),
        format!("runtime {}/job", bench.path("r").display()),
        format!("cache {}/.cache/job", home.display()),
    ] {
        assert!(detail.contains(&expected), "{detail}");
    }
    let socket = check(&found, "socket")["detail"].as_str().unwrap();
    assert!(
        socket.contains(&format!("{}/job/job.sock", bench.path("r").display())),
        "{socket}"
    );

    let mut command = bench.clean(Path::new(JOB));
    let output = command
        .args(["doctor", "--json"])
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", bench.path("r"))
        .env("JOB_STATE_DIR", bench.path("elsewhere"))
        .env("JOB_CACHE_DIR", bench.path("cache"))
        .output()
        .unwrap();
    let found = report(&output);
    let detail = check(&found, "paths")["detail"].as_str().unwrap();
    assert!(
        detail.contains(&format!("runtime {}", bench.path("elsewhere").display())),
        "{detail}"
    );
    assert!(
        detail.contains(&format!("cache {}", bench.path("cache").display())),
        "{detail}"
    );
    let socket = check(&found, "socket")["detail"].as_str().unwrap();
    assert!(
        socket.contains(&format!(
            "{}/daemon.sock",
            bench.path("elsewhere").display()
        )),
        "{socket}"
    );
    assert!(
        socket.contains("JOB_STATE_DIR without JOB_RUNTIME_DIR"),
        "{socket}"
    );
}

#[test]
fn system_paths_and_missing_home_are_reported() {
    let bench = Bench::new("system");
    let mut command = bench.clean(Path::new(JOB));
    let output = command
        .args(["doctor", "--json"])
        .env("JOB_SYSTEM", "1")
        .output()
        .unwrap();
    let found = report(&output);
    let detail = check(&found, "paths")["detail"].as_str().unwrap();
    assert!(
        detail.contains(
            "system service: configuration /etc/job/config.toml, state /var/lib/job, runtime /run/job, cache /var/cache/job"
        ),
        "{detail}"
    );
    let socket = check(&found, "socket")["detail"].as_str().unwrap();
    assert!(socket.contains("/run/job/job.sock"), "{socket}");

    let mut command = bench.clean(Path::new(JOB));
    let output = command.args(["doctor", "--json"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", both(&output));
    let found = report(&output);
    let paths = check(&found, "paths");
    assert_eq!(paths["status"], "fail");
    let detail = paths["detail"].as_str().unwrap();
    assert!(
        detail.contains("XDG_STATE_HOME or JOB_STATE_DIR"),
        "{detail}"
    );
    assert!(detail.contains("XDG_CONFIG_HOME or JOB_CONFIG"), "{detail}");
    assert!(!detail.contains("/tmp"), "{detail}");

    let mut command = bench.clean(Path::new(JOB));
    let output = command.args(["status", "1"]).output().unwrap();
    assert_eq!(output.status.code(), Some(125), "{}", both(&output));
    assert!(
        both(&output).contains("set HOME, XDG_STATE_HOME or JOB_STATE_DIR"),
        "{}",
        both(&output)
    );
}

#[test]
fn doctor_works_without_a_service_and_speaks_german() {
    let bench = Bench::new("down");
    let mut command = bench.explicit(Path::new(JOB));
    let output = command.arg("doctor").output().unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", both(&output));
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let line = |name: &str| {
        text.lines()
            .find(|line| line.split_whitespace().nth(1) == Some(&format!("{name}:")))
            .unwrap_or_else(|| panic!("no line for {name} in {text}"))
            .to_owned()
    };
    assert!(line("daemon").starts_with("fail"), "{text}");
    assert!(line("daemon").contains("Do: start the service"), "{text}");
    assert!(line("config").starts_with("ok"), "{text}");
    assert!(line("pidfd").starts_with("ok"), "{text}");
    assert!(
        line("state_directory").contains("does not exist yet"),
        "{text}"
    );
    assert!(line("daemon_lock").starts_with("warn"), "{text}");
    assert!(!bench.path("state").exists());
    for name in [
        "paths",
        "state_schema",
        "legacy_store",
        "socket",
        "access",
        "cgroup_v2",
        "cgroup_own",
        "cgroup_root",
        "cgroup_controllers",
        "io_devices",
        "freezer",
        "pressure",
        "user_namespaces",
        "landlock",
        "seccomp",
        "no_new_privs",
        "capabilities",
        "network_tools",
        "service_manager",
    ] {
        line(name);
    }

    let mut command = bench.explicit(Path::new(JOB));
    let output = command
        .arg("doctor")
        .env("LC_ALL", "de_DE.UTF-8")
        .output()
        .unwrap();
    let german = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(german.contains("Zu tun: starte den Dienst"), "{german}");
    assert!(german.contains("fail daemon: keine Antwort"), "{german}");

    fs::write(
        bench.path("config.toml"),
        "schema_version = 1\nsockets = 1\n",
    )
    .unwrap();
    let mut command = bench.explicit(Path::new(JOB));
    let output = command.args(["doctor", "--json"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let found = report(&output);
    assert_eq!(check(&found, "config")["status"], "fail");
    assert!(check(&found, "config")["fix"].as_str().is_some());
    assert!(check(&found, "paths")["fix"].is_null());

    let mut command = bench.explicit(Path::new(JOB));
    let output = command.args(["doctor", "--verbose"]).output().unwrap();
    assert_eq!(output.status.code(), Some(125));
}

#[test]
fn a_state_schema_this_program_does_not_serve_fails_the_doctor() {
    let bench = Bench::new("schema");
    fs::create_dir_all(bench.path("state")).unwrap();
    fs::write(
        bench.path("state/schema.json"),
        "{\"schema_version\":16,\"resource_vocabulary\":\"independent_resources_with_legacy_aliases\"}",
    )
    .unwrap();
    let mut command = bench.explicit(Path::new(JOB));
    let output = command.args(["doctor", "--json"]).output().unwrap();
    let found = report(&output);
    let schema = check(&found, "state_schema");
    assert_eq!(schema["status"], "fail", "{found}");
    assert!(
        schema["fix"]
            .as_str()
            .unwrap()
            .contains("job state migrate"),
        "{found}"
    );
    assert_eq!(
        check(&found, "state_directory")["status"],
        "warn",
        "a directory created with the default mode is readable by others: {found}"
    );
}

#[test]
fn the_runtime_socket_is_separate_and_a_stale_state_socket_is_removed() {
    let mut bench = Bench::new("runtime");
    fs::create_dir_all(bench.path("state")).unwrap();
    drop(UnixListener::bind(bench.path("state/daemon.sock")).unwrap());
    assert!(bench.path("state/daemon.sock").exists());
    let mut daemon = bench.explicit(Path::new(JOB));
    daemon
        .arg("daemon")
        .env("JOB_RUNTIME_DIR", bench.path("run"));
    assert!(
        bench.start(daemon, &bench.path("run/job.sock")),
        "{}",
        bench.errors()
    );
    assert!(!bench.path("state/daemon.sock").exists());
    assert!(bench.path("state/daemon.lock").exists());
    assert_eq!(mode(&bench.path("run")), 0o700);
    assert_eq!(mode(&bench.path("run/job.sock")), 0o600);
    assert!(
        bench.said("chosen by JOB_RUNTIME_DIR"),
        "{}",
        bench.errors()
    );

    let mut client = bench.explicit(Path::new(JOB));
    let output = client
        .args(["run", "--", "true"])
        .env("JOB_RUNTIME_DIR", bench.path("run"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", both(&output));

    let mut client = bench.explicit(Path::new(JOB));
    let output = client.args(["run", "--", "true"]).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(125),
        "a client that names only the state directory must not find the socket elsewhere: {}",
        both(&output)
    );

    let mut client = bench.explicit(Path::new(JOB));
    let output = client
        .args(["doctor", "--json"])
        .env("JOB_RUNTIME_DIR", bench.path("run"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", both(&output));
    let found = report(&output);
    assert_eq!(check(&found, "daemon")["status"], "ok", "{found}");
    assert_eq!(check(&found, "daemon_lock")["status"], "ok", "{found}");
    assert_eq!(check(&found, "socket")["status"], "ok", "{found}");
    assert!(
        check(&found, "cgroup_root")["detail"]
            .as_str()
            .unwrap()
            .contains("JOB_CGROUP_ROOT names"),
        "{found}"
    );
}

#[test]
fn a_client_falls_back_to_the_socket_in_the_state_directory() {
    let mut bench = Bench::new("fallback");
    let mut daemon = bench.explicit(Path::new(JOB));
    daemon.arg("daemon");
    assert!(
        bench.start(daemon, &bench.path("state/daemon.sock")),
        "{}",
        bench.errors()
    );
    fs::create_dir_all(bench.path("empty-run")).unwrap();
    let mut client = bench.explicit(Path::new(JOB));
    let output = client
        .args(["run", "--", "true"])
        .env("JOB_RUNTIME_DIR", bench.path("empty-run"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", both(&output));
}

#[test]
fn a_default_user_service_binds_under_xdg_runtime_dir() {
    let mut bench = Bench::new("session");
    let home = bench.path("home");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(bench.path("xdg-run")).unwrap();
    let session = |bench: &Bench| {
        let mut command = bench.clean(Path::new(JOB));
        command
            .env("HOME", bench.path("home"))
            .env("XDG_RUNTIME_DIR", bench.path("xdg-run"));
        command
    };
    let mut daemon = session(&bench);
    daemon.arg("daemon");
    let socket = bench.path("xdg-run/job/job.sock");
    assert!(bench.start(daemon, &socket), "{}", bench.errors());
    assert!(home.join(".local/state/job/daemon.lock").exists());
    assert!(!home.join(".local/state/job/daemon.sock").exists());
    assert_eq!(mode(&home.join(".local/state/job")), 0o700);
    let output = session(&bench)
        .args(["run", "--", "true"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", both(&output));
}

#[test]
fn jobd_is_the_daemon_and_everyday_help_omits_internal_commands() {
    let mut bench = Bench::new("jobd");
    fs::create_dir_all(bench.path("bin")).unwrap();
    placed(&bench.path("bin/job"));
    std::os::unix::fs::symlink("job", bench.path("bin/jobd")).unwrap();
    let jobd = bench.path("bin/jobd");

    let output = bench.explicit(&jobd).arg("--bogus").output().unwrap();
    assert_eq!(output.status.code(), Some(125), "{}", both(&output));
    assert!(
        both(&output).contains("usage: jobd [--system] [--foreground]"),
        "{}",
        both(&output)
    );
    assert!(!bench.path("state").exists());
    let output = bench.explicit(&jobd).arg("--help").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(both(&output).contains("jobd(8)"), "{}", both(&output));

    let mut daemon = bench.explicit(&jobd);
    daemon.arg("--foreground");
    assert!(
        bench.start(daemon, &bench.path("state/daemon.sock")),
        "{}",
        bench.errors()
    );
    let output = bench
        .explicit(&bench.path("bin/job"))
        .args(["run", "--", "sh", "-c", "echo from-jobd"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", both(&output));
    assert!(both(&output).contains("from-jobd"), "{}", both(&output));

    let help = bench.explicit(Path::new(JOB)).arg("help").output().unwrap();
    let help = both(&help);
    assert!(help.contains("doctor"), "{help}");
    for internal in ["daemon", "hook", "classify", "shim"] {
        assert!(
            !help
                .split(|c: char| !c.is_alphanumeric())
                .any(|word| word == internal),
            "{internal} in {help}"
        );
    }
}

fn groups() -> (u32, Vec<u32>) {
    let primary = unsafe { libc::getegid() };
    let mut all = vec![0 as libc::gid_t; 256];
    let count = unsafe { libc::getgroups(all.len() as i32, all.as_mut_ptr()) };
    all.truncate(count.max(0) as usize);
    (primary, all)
}

fn group_config(bench: &Bench, group: &str) {
    fs::write(
        bench.path("config.toml"),
        format!("schema_version = 1\nprofile = 'ordinary'\n[socket]\ngroup = \"{group}\"\n"),
    )
    .unwrap();
}

#[test]
fn a_group_shared_socket_gets_group_modes_and_needs_its_own_directory() {
    let mut bench = Bench::new("group");
    let (primary, all) = groups();
    let group = match all.iter().find(|group| **group != primary) {
        Some(group) => *group,
        None => {
            println!(
                "this user has only its primary group {primary}; the change of group ownership cannot be told apart, modes are still checked"
            );
            primary
        }
    };
    group_config(&bench, &group.to_string());

    let mut daemon = bench.explicit(Path::new(JOB));
    daemon.arg("daemon");
    assert!(!bench.start(daemon, &bench.path("state/daemon.sock")));
    assert!(
        bench
            .errors()
            .contains("needs a runtime directory apart from the private state directory"),
        "{}",
        bench.errors()
    );

    let mut daemon = bench.explicit(Path::new(JOB));
    daemon
        .arg("daemon")
        .env("JOB_RUNTIME_DIR", bench.path("run"));
    assert!(
        bench.start(daemon, &bench.path("run/job.sock")),
        "{}",
        bench.errors()
    );
    let directory = fs::metadata(bench.path("run")).unwrap();
    let socket = fs::metadata(bench.path("run/job.sock")).unwrap();
    assert_eq!(directory.mode() & 0o7777, 0o750);
    assert_eq!(socket.mode() & 0o7777, 0o660);
    assert_eq!(directory.gid(), group);
    assert_eq!(socket.gid(), group);
    assert_eq!(mode(&bench.path("state")), 0o700);

    let mut client = bench.explicit(Path::new(JOB));
    let output = client
        .args(["doctor", "--json"])
        .env("JOB_RUNTIME_DIR", bench.path("run"))
        .output()
        .unwrap();
    let found = report(&output);
    assert_eq!(check(&found, "socket")["status"], "ok", "{found}");
    let access = check(&found, "access")["detail"].as_str().unwrap();
    assert!(
        access.contains(
            "they get output from the service and terminals through the runtime directory"
        ),
        "{access}"
    );
    let mut client = bench.explicit(Path::new(JOB));
    let output = client
        .args(["host", "--json"])
        .env("JOB_RUNTIME_DIR", bench.path("run"))
        .output()
        .unwrap();
    let host = report(&output);
    assert_eq!(host["service"]["socket_group"], group.to_string());
    assert_eq!(host["service"]["uid"], unsafe { libc::getuid() });
}

#[test]
fn an_unknown_socket_group_refuses_startup() {
    let mut bench = Bench::new("nogroup");
    group_config(&bench, "job-no-such-group-51f3");
    let mut daemon = bench.explicit(Path::new(JOB));
    daemon
        .arg("daemon")
        .env("JOB_RUNTIME_DIR", bench.path("run"));
    assert!(!bench.start(daemon, &bench.path("run/job.sock")));
    assert!(
        bench
            .errors()
            .contains("the socket group job-no-such-group-51f3 does not exist"),
        "{}",
        bench.errors()
    );
    assert!(!bench.path("run/job.sock").exists());
}

fn subordinate(file: &str) -> Option<u32> {
    let name = String::from_utf8(Command::new("id").arg("-un").output().ok()?.stdout).ok()?;
    fs::read_to_string(file).ok()?.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.next() == Some(name.trim()))
            .then(|| fields.next()?.parse().ok())
            .flatten()
    })
}

fn foreign(bench: &Bench, uid: u32, gid: u32, inner_gid: u32, arguments: &[&str]) -> Command {
    let mut command = bench.clean(Path::new("unshare"));
    command
        .arg(format!("--map-users=1:{uid}:1"))
        .arg("--map-user=0")
        .arg(format!("--map-groups=1:{gid}:1"))
        .arg("--map-group=0")
        .args(["setpriv", "--reuid", "1", "--regid"])
        .arg(inner_gid.to_string())
        .arg("--clear-groups")
        .arg(bench.path("bin/job"))
        .args(arguments)
        .env("JOB_STATE_DIR", bench.path("state"))
        .env("JOB_RUNTIME_DIR", bench.path("run"))
        .current_dir("/");
    command
}

#[test]
fn another_user_is_admitted_only_through_the_socket_group_and_recorded_as_actor() {
    let mut bench = Bench::new("peer");
    let (Some(uid), Some(gid)) = (subordinate("/etc/subuid"), subordinate("/etc/subgid")) else {
        println!(
            "skipped: this user has no subordinate user and group ids, so no second user can be made without root"
        );
        return;
    };
    fs::create_dir_all(bench.path("bin")).unwrap();
    placed(&bench.path("bin/job"));
    fs::set_permissions(bench.path("bin/job"), fs::Permissions::from_mode(0o755)).unwrap();
    let probe = foreign(&bench, uid, gid, 1, &["doctor", "--bogus"]).output();
    if !probe
        .as_ref()
        .is_ok_and(|output| output.status.code() == Some(125))
    {
        println!(
            "skipped: a process with a subordinate user id cannot be started here: {:?}",
            probe.map(|output| both(&output))
        );
        return;
    }
    let primary = unsafe { libc::getegid() };
    group_config(&bench, &primary.to_string());
    let mut daemon = bench.explicit(Path::new(JOB));
    daemon
        .arg("daemon")
        .env("JOB_RUNTIME_DIR", bench.path("run"));
    assert!(
        bench.start(daemon, &bench.path("run/job.sock")),
        "{}",
        bench.errors()
    );
    let own = |arguments: &[&str]| {
        let output = bench
            .explicit(Path::new(JOB))
            .args(arguments)
            .env("JOB_RUNTIME_DIR", bench.path("run"))
            .output()
            .unwrap();
        assert!(
            output.status.success()
                || (arguments[0] == "status" && output.status.code() == Some(75)),
            "{arguments:?}: {}",
            both(&output)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    own(&["queue", "create", "q"]);
    let id = own(&["create", "-q", "q", "--", "true"]);

    let stranger = foreign(
        &bench,
        uid,
        gid,
        1,
        &["reprioritize", &id, "--priority", "7"],
    )
    .output()
    .unwrap();
    assert!(!stranger.status.success(), "{}", both(&stranger));
    assert!(
        both(&stranger).contains("Permission denied"),
        "the directory keeps a stranger out before the service is asked: {}",
        both(&stranger)
    );

    fs::set_permissions(bench.path("run"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(
        bench.path("run/job.sock"),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    let stranger = foreign(
        &bench,
        uid,
        gid,
        1,
        &["reprioritize", &id, "--priority", "7"],
    )
    .output()
    .unwrap();
    assert!(!stranger.status.success(), "{}", both(&stranger));
    assert!(
        both(&stranger)
            .contains("refused: the caller is neither the service user nor a member of group"),
        "{}",
        both(&stranger)
    );
    let record: serde_json::Value = serde_json::from_str(&own(&["status", &id, "--json"])).unwrap();
    assert_eq!(record["priority_changes"].as_array().unwrap().len(), 0);

    let member = foreign(
        &bench,
        uid,
        gid,
        0,
        &["reprioritize", &id, "--priority", "7"],
    )
    .output()
    .unwrap();
    assert!(member.status.success(), "{}", both(&member));
    let record: serde_json::Value = serde_json::from_str(&own(&["status", &id, "--json"])).unwrap();
    assert_eq!(record["priority_changes"][0]["after"], 7);
    assert_eq!(record["priority_changes"][0]["actor_uid"], uid);

    let logs = foreign(&bench, uid, gid, 0, &["log", &id])
        .output()
        .unwrap();
    assert!(
        !logs.status.success() && both(&logs).contains("cannot be read"),
        "the service answers a member that a Job which never ran has no log: {}",
        both(&logs)
    );
    assert_eq!(mode(&bench.path("state")), 0o700);
    own(&["reprioritize", &id, "--priority", "8"]);
    let record: serde_json::Value = serde_json::from_str(&own(&["status", &id, "--json"])).unwrap();
    assert_eq!(record["priority_changes"][1]["actor_uid"], unsafe {
        libc::getuid()
    });
    own(&["cancel", &id]);
}

#[test]
fn a_private_socket_refuses_another_user_even_when_the_files_are_opened_up() {
    let mut bench = Bench::new("private");
    let (Some(uid), Some(gid)) = (subordinate("/etc/subuid"), subordinate("/etc/subgid")) else {
        println!(
            "skipped: this user has no subordinate user and group ids, so no second user can be made without root"
        );
        return;
    };
    fs::create_dir_all(bench.path("bin")).unwrap();
    placed(&bench.path("bin/job"));
    let probe = foreign(&bench, uid, gid, 1, &["doctor", "--bogus"]).output();
    if !probe
        .as_ref()
        .is_ok_and(|output| output.status.code() == Some(125))
    {
        println!(
            "skipped: a process with a subordinate user id cannot be started here: {:?}",
            probe.map(|output| both(&output))
        );
        return;
    }
    let mut daemon = bench.explicit(Path::new(JOB));
    daemon
        .arg("daemon")
        .env("JOB_RUNTIME_DIR", bench.path("run"));
    assert!(
        bench.start(daemon, &bench.path("run/job.sock")),
        "{}",
        bench.errors()
    );
    fs::set_permissions(bench.path("run"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(
        bench.path("run/job.sock"),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    for inner_gid in [0, 1] {
        let stranger = foreign(&bench, uid, gid, inner_gid, &["queue"])
            .output()
            .unwrap();
        assert!(!stranger.status.success(), "{}", both(&stranger));
        assert!(
            both(&stranger).contains("refused: the caller runs as a different user"),
            "{}",
            both(&stranger)
        );
    }
}

fn enter(command: &mut Command, cgroup: &Path) {
    let procs =
        std::ffi::CString::new(cgroup.join("cgroup.procs").as_os_str().as_encoded_bytes()).unwrap();
    unsafe {
        command.pre_exec(move || {
            let fd = libc::open(procs.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let result = libc::write(fd, b"0".as_ptr().cast(), 1);
            let error = std::io::Error::last_os_error();
            libc::close(fd);
            if result != 1 {
                return Err(error);
            }
            Ok(())
        });
    }
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn the_cgroup_root_is_discovered_from_delegation_and_named_at_startup() {
    let mut service = support::Service::start("discovery");
    let explicit = fs::read_to_string(service.root.join("daemon.err")).unwrap();
    assert!(
        explicit.contains(&format!(
            "cgroup root {}, chosen by JOB_CGROUP_ROOT",
            service.cgroup.display()
        )),
        "{explicit}"
    );
    service.child.kill().unwrap();
    service.child.wait().unwrap();

    let mut command = Command::new(JOB);
    command
        .arg("daemon")
        .env_remove("JOB_CGROUP_ROOT")
        .env("JOB_CONFIG", service.root.join("config.toml"))
        .env("JOB_STATE_DIR", service.root.join("state"))
        .env("LC_ALL", "C")
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            fs::File::create(service.root.join("daemon.err")).unwrap(),
        ));
    enter(&mut command, &service.cgroup.join("daemon"));
    service.child = command.spawn().unwrap();
    service.ready();
    let discovered = fs::read_to_string(service.root.join("daemon.err")).unwrap();
    assert!(
        discovered.contains(&format!(
            "cgroup root {}, chosen by delegated own cgroup",
            service.cgroup.display()
        )),
        "{discovered}"
    );
    let found: serde_json::Value =
        serde_json::from_str(&service.ok(&["doctor", "--json"])).unwrap();
    let root = check(&found, "cgroup_root");
    assert_eq!(root["status"], "ok", "{found}");
    assert!(
        root["detail"]
            .as_str()
            .unwrap()
            .contains("chosen by delegated own cgroup"),
        "{found}"
    );
    assert_eq!(
        check(&found, "cgroup_controllers")["status"],
        "ok",
        "{found}"
    );
    assert_eq!(check(&found, "freezer")["status"], "ok", "{found}");
    let id = service.ok(&["submit", "--", "true"]);
    let output = service.cli(&["wait", &id]);
    assert_eq!(output.status.code(), Some(0), "{}", both(&output));
    let host: serde_json::Value = serde_json::from_str(&service.ok(&["host", "--json"])).unwrap();
    assert_eq!(host["backend"], "Cgroup");
    assert_eq!(host["service"]["cgroup_rule"], "delegated own cgroup");
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn a_cgroup_root_holding_other_processes_is_refused() {
    let service = support::Service::start("foreign");
    let shared = service
        .cgroup
        .parent()
        .unwrap()
        .join(format!("job-li-foreign-{}", std::process::id()));
    fs::create_dir(&shared).unwrap();
    let mut sleeper = Command::new("sleep");
    sleeper.arg("20");
    enter(&mut sleeper, &shared);
    let mut sleeper = sleeper.spawn().unwrap();
    let base = service.root.join("second");
    fs::create_dir_all(&base).unwrap();
    let run = |explicit: bool| {
        let mut command = Command::new(JOB);
        command
            .arg("daemon")
            .env("JOB_CONFIG", service.root.join("config.toml"))
            .env("JOB_STATE_DIR", base.join("state"))
            .env("LC_ALL", "C")
            .stdout(Stdio::null())
            .stderr(Stdio::from(fs::File::create(base.join("err")).unwrap()));
        if explicit {
            command.env("JOB_CGROUP_ROOT", &shared);
        } else {
            command.env_remove("JOB_CGROUP_ROOT");
        }
        enter(&mut command, &shared);
        command.spawn().unwrap()
    };
    let mut refused = run(true);
    let status = refused.wait().unwrap();
    let explicit = fs::read_to_string(base.join("err")).unwrap();

    let mut watching = run(false);
    let deadline = Instant::now() + Duration::from_secs(10);
    let discovered = loop {
        let said = fs::read_to_string(base.join("err")).unwrap();
        if (said.contains("processes are watched instead")
            && said.contains("holds 1 other processes"))
            || watching.try_wait().unwrap().is_some()
            || Instant::now() >= deadline
        {
            break fs::read_to_string(base.join("err")).unwrap();
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let _ = watching.kill();
    let _ = watching.wait();
    let _ = sleeper.kill();
    let _ = sleeper.wait();
    let deadline = Instant::now() + Duration::from_secs(5);
    while fs::remove_dir(&shared).is_err() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(status.code(), Some(125), "{explicit}");
    assert!(
        explicit.contains("holds 1 other processes")
            && explicit.contains("must hold only the service"),
        "{explicit}"
    );
    assert!(
        discovered.contains("no cgroup root, processes are watched instead")
            && discovered.contains("holds 1 other processes"),
        "{discovered}"
    );
}

fn placed(to: &std::path::Path) {
    fs::copy(JOB, to).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let tried = Command::new(to)
            .arg("help")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match tried {
            Err(error)
                if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => break,
        }
    }
}

fn finished(mut child: Child, limit: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn review_a_second_daemon_on_the_same_runtime_directory_is_refused_and_the_first_stays_reachable() {
    let mut bench = Bench::new("review-two");
    let mut first = bench.explicit(Path::new(JOB));
    first
        .arg("daemon")
        .env("JOB_RUNTIME_DIR", bench.path("run"));
    assert!(
        bench.start(first, &bench.path("run/job.sock")),
        "{}",
        bench.errors()
    );
    assert_eq!(
        fs::read_to_string(bench.path("run/job.lock"))
            .unwrap()
            .trim(),
        bench.path("state").to_str().unwrap()
    );
    assert_eq!(mode(&bench.path("run/job.lock")), 0o600);
    let mut second = bench.clean(Path::new(JOB));
    let errors = fs::File::create(bench.path("second.err")).unwrap();
    let child = second
        .arg("daemon")
        .env("JOB_CONFIG", bench.path("config.toml"))
        .env("JOB_STATE_DIR", bench.path("other-state"))
        .env("JOB_RUNTIME_DIR", bench.path("run"))
        .stdout(Stdio::null())
        .stderr(Stdio::from(errors))
        .spawn()
        .unwrap();
    let status = finished(child, Duration::from_secs(10));
    let said = fs::read_to_string(bench.path("second.err")).unwrap();
    assert!(
        status.is_some_and(|status| !status.success()),
        "the second daemon kept running: {said}"
    );
    assert!(
        said.contains("another job service already serves the runtime directory"),
        "{said}"
    );
    assert!(
        said.contains(&format!(
            "its state directory is {}",
            bench.path("state").display()
        )),
        "{said}"
    );
    let mut client = bench.explicit(Path::new(JOB));
    let output = client
        .args(["run", "--", "true"])
        .env("JOB_RUNTIME_DIR", bench.path("run"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", both(&output));
    let mut child = bench.children.pop().unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    let mut third = bench.clean(Path::new(JOB));
    third
        .arg("daemon")
        .env("JOB_CONFIG", bench.path("config.toml"))
        .env("JOB_STATE_DIR", bench.path("other-state"))
        .env("JOB_RUNTIME_DIR", bench.path("run"));
    assert!(
        bench.start(third, &bench.path("run/job.sock")),
        "{}",
        bench.errors()
    );
    assert_eq!(
        fs::read_to_string(bench.path("run/job.lock"))
            .unwrap()
            .trim(),
        bench.path("other-state").to_str().unwrap()
    );
}

#[test]
fn review_a_terminal_socket_gets_the_policy_the_daemon_started_with() {
    let mut bench = Bench::new("review-terminal");
    let (primary, all) = groups();
    let group = all
        .iter()
        .copied()
        .find(|group| *group != primary)
        .unwrap_or(primary);
    let mut daemon = bench.explicit(Path::new(JOB));
    daemon
        .arg("daemon")
        .env("JOB_RUNTIME_DIR", bench.path("run"));
    assert!(
        bench.start(daemon, &bench.path("run/job.sock")),
        "{}",
        bench.errors()
    );
    group_config(&bench, &group.to_string());
    let mut client = bench.explicit(Path::new(JOB));
    let output = client
        .args(["submit", "--pty", "--", "sleep", "20"])
        .env("JOB_RUNTIME_DIR", bench.path("run"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", both(&output));
    let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let socket = bench.path("state/jobs").join(&id).join("terminal.sock");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(Instant::now() < deadline, "no terminal socket for job {id}");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(200));
    let found = fs::symlink_metadata(&socket).unwrap();
    assert_eq!(found.mode() & 0o7777, 0o600);
    assert_eq!(
        found.gid(),
        fs::metadata(bench.path("state")).unwrap().gid()
    );
    assert_eq!(mode(&bench.path("run/job.sock")), 0o600);
    let mut client = bench.explicit(Path::new(JOB));
    let output = client
        .args(["cancel", &id])
        .env("JOB_RUNTIME_DIR", bench.path("run"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", both(&output));
}

#[test]
fn review_doctor_never_makes_a_starting_daemon_fail() {
    let mut bench = Bench::new("review-doctor");
    fs::create_dir_all(bench.path("state")).unwrap();
    let stop = std::sync::atomic::AtomicBool::new(false);
    let doctors: Vec<Command> = (0..2)
        .map(|_| {
            let mut command = bench.explicit(Path::new(JOB));
            command.args(["doctor", "--json"]);
            command
        })
        .collect();
    let ran = std::thread::scope(|scope| {
        let workers: Vec<_> = doctors
            .into_iter()
            .map(|mut command| {
                let stop = &stop;
                scope.spawn(move || {
                    let mut ran = 0;
                    for _ in 0..200 {
                        if stop.load(std::sync::atomic::Ordering::Relaxed) {
                            break;
                        }
                        let output = command.output().unwrap();
                        assert!(output.status.code().is_some(), "{}", both(&output));
                        ran += 1;
                    }
                    ran
                })
            })
            .collect();
        let mut failures = Vec::new();
        for round in 0..20 {
            let mut daemon = bench.explicit(Path::new(JOB));
            daemon.arg("daemon");
            if !bench.start(daemon, &bench.path("state/daemon.sock")) {
                failures.push(format!("start {round}: {}", bench.errors()));
            }
            let mut child = bench.children.pop().unwrap();
            let _ = child.kill();
            let _ = child.wait();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let ran: usize = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .sum();
        assert!(failures.is_empty(), "{failures:?}");
        ran
    });
    assert!(ran > 0);
    let mut daemon = bench.explicit(Path::new(JOB));
    daemon.arg("daemon");
    assert!(bench.start(daemon, &bench.path("state/daemon.sock")));
    let mut client = bench.explicit(Path::new(JOB));
    let found = report(&client.args(["doctor", "--json"]).output().unwrap());
    let lock = check(&found, "daemon_lock");
    assert_eq!(lock["status"], "ok", "{found}");
    assert!(lock["detail"].as_str().unwrap().contains("a service holds"));
    let mut child = bench.children.pop().unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    let mut client = bench.explicit(Path::new(JOB));
    let found = report(&client.args(["doctor", "--json"]).output().unwrap());
    let lock = check(&found, "daemon_lock");
    assert_eq!(lock["status"], "warn", "{found}");
    assert!(lock["detail"].as_str().unwrap().contains("nothing holds"));
}

#[test]
fn review_the_system_option_selects_the_system_instance() {
    let mut bench = Bench::new("review-system");
    let mut command = bench.clean(Path::new(JOB));
    let found = report(
        &command
            .args(["--system", "doctor", "--json"])
            .output()
            .unwrap(),
    );
    let detail = check(&found, "paths")["detail"].as_str().unwrap();
    assert!(
        detail.contains(
            "system service: configuration /etc/job/config.toml, state /var/lib/job, runtime /run/job, cache /var/cache/job"
        ),
        "{detail}"
    );
    let mut command = bench.clean(Path::new(JOB));
    let found = report(
        &command
            .args(["doctor", "--system", "--json"])
            .output()
            .unwrap(),
    );
    let socket = check(&found, "socket")["detail"].as_str().unwrap();
    assert!(socket.contains("/run/job/job.sock"), "{socket}");
    let mut command = bench.clean(Path::new(JOB));
    let output = command.args(["status", "1"]).output().unwrap();
    assert!(
        both(&output).contains("set HOME, XDG_STATE_HOME or JOB_STATE_DIR"),
        "{}",
        both(&output)
    );
    let mut command = bench.clean(Path::new(JOB));
    let output = command.args(["--system", "status", "1"]).output().unwrap();
    assert_eq!(output.status.code(), Some(125), "{}", both(&output));
    assert!(
        !both(&output).contains("XDG_STATE_HOME"),
        "{}",
        both(&output)
    );

    fs::create_dir_all(bench.path("bin")).unwrap();
    placed(&bench.path("bin/job"));
    std::os::unix::fs::symlink("job", bench.path("bin/jobd")).unwrap();
    let private = |bench: &Bench, program: &str| {
        let mut command = bench.clean(&bench.path("bin").join(program));
        command
            .env("JOB_CONFIG", bench.path("config.toml"))
            .env("JOB_STATE_DIR", bench.path("state"))
            .env("JOB_RUNTIME_DIR", bench.path("run"));
        command
    };
    let mut daemon = private(&bench, "jobd");
    daemon.arg("--system");
    assert!(
        bench.start(daemon, &bench.path("run/job.sock")),
        "{}",
        bench.errors()
    );
    assert!(bench.said("system service"), "{}", bench.errors());
    let output = private(&bench, "job")
        .args(["--system", "run", "--", "true"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", both(&output));
    let host = report(
        &private(&bench, "job")
            .args(["--system", "host", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(host["service"]["mode"], "system");
    for arguments in [
        ["--system", "doctor", "--json"],
        ["doctor", "--system", "--json"],
    ] {
        let found = report(&private(&bench, "job").args(arguments).output().unwrap());
        assert_eq!(check(&found, "daemon")["status"], "ok", "{found}");
        let detail = check(&found, "paths")["detail"].as_str().unwrap();
        assert!(detail.contains("system service"), "{detail}");
        assert!(
            detail.contains(bench.path("state").to_str().unwrap()),
            "{detail}"
        );
    }
    let help = private(&bench, "job").arg("--help").output().unwrap();
    assert!(both(&help).contains("--system"), "{}", both(&help));
    let german = private(&bench, "job")
        .arg("--help")
        .env("LC_ALL", "de_DE.UTF-8")
        .output()
        .unwrap();
    assert!(
        both(&german).contains("Globale Optionen"),
        "{}",
        both(&german)
    );
    let refused = private(&bench, "job")
        .args(["status", "--system", "1"])
        .output()
        .unwrap();
    assert_ne!(refused.status.code(), Some(0), "{}", both(&refused));
}

#[test]
fn review_a_refused_peer_is_recorded_in_the_audit_journal_a_bounded_number_of_times() {
    let mut bench = Bench::new("review-refused");
    let (Some(uid), Some(gid)) = (subordinate("/etc/subuid"), subordinate("/etc/subgid")) else {
        println!(
            "skipped: this user has no subordinate user and group ids, so no second user can be made without root"
        );
        return;
    };
    fs::create_dir_all(bench.path("bin")).unwrap();
    placed(&bench.path("bin/job"));
    let probe = foreign(&bench, uid, gid, 1, &["doctor", "--bogus"]).output();
    if !probe
        .as_ref()
        .is_ok_and(|output| output.status.code() == Some(125))
    {
        println!(
            "skipped: a process with a subordinate user id cannot be started here: {:?}",
            probe.map(|output| both(&output))
        );
        return;
    }
    let mut daemon = bench.explicit(Path::new(JOB));
    daemon
        .arg("daemon")
        .env("JOB_RUNTIME_DIR", bench.path("run"));
    assert!(
        bench.start(daemon, &bench.path("run/job.sock")),
        "{}",
        bench.errors()
    );
    fs::set_permissions(bench.path("run"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(
        bench.path("run/job.sock"),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    for _ in 0..34 {
        let stranger = foreign(&bench, uid, gid, 1, &["queue"]).output().unwrap();
        assert!(!stranger.status.success(), "{}", both(&stranger));
    }
    let mut client = bench.explicit(Path::new(JOB));
    let output = client
        .args(["audit", "--json", "--action", "connection-refused"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", both(&output));
    let entries: Vec<serde_json::Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(entries.len(), 30, "{}", both(&output));
    assert_eq!(entries[0]["peer_uid"], uid);
    assert!(entries[0]["peer_pid"].as_i64().unwrap() > 1);
    assert_eq!(entries[0]["result"], "refused");
    assert!(
        entries[0]["params"]["reason"]
            .as_str()
            .unwrap()
            .contains("different user"),
        "{}",
        entries[0]
    );
    let mut client = bench.explicit(Path::new(JOB));
    let output = client
        .args(["audit", "--json", "--action", "audit-suppressed"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 1);
}

const POLICY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/policy.json");

fn fed(mut command: Command, input: &str) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), input.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

fn hook_event(command: &str) -> String {
    serde_json::json!({"command": command, "cwd": "/tmp", "caller": "policy-test"}).to_string()
}

fn answer_of(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", both(output)))
}

fn policy_command(bench: &Bench, arguments: &[&str]) -> Command {
    let mut command = bench.explicit(Path::new(JOB));
    command.args(arguments);
    command
}

#[test]
fn without_a_policy_file_the_command_policy_forbids_nothing_and_says_so() {
    let bench = Bench::new("policy-none");
    let expected = bench.path("policy.json");
    let shown = policy_command(&bench, &["policy", "--show"])
        .output()
        .unwrap();
    assert_eq!(shown.status.code(), Some(0), "{}", both(&shown));
    assert_eq!(
        String::from_utf8_lossy(&shown.stdout),
        format!(
            "no policy file at {}; the command policy forbids nothing\n",
            expected.display()
        )
    );
    let verdicts = fed(
        policy_command(&bench, &["policy"]),
        "rm -rf /\nsudo true\ngit push origin main\nfind / -name x\n",
    );
    assert_eq!(
        String::from_utf8_lossy(&verdicts.stdout),
        "allow\nallow\nallow\nallow\n",
        "{}",
        both(&verdicts)
    );
    let answer = fed(
        policy_command(&bench, &["hook"]),
        &hook_event("rm -rf scratch"),
    );
    assert_eq!(answer.status.code(), Some(0), "{}", both(&answer));
    assert_eq!(
        String::from_utf8_lossy(&answer.stdout),
        "{\"decision\":\"allow\"}\n"
    );
    assert!(!bench.path("state").join("denials.jsonl").exists());
    let doctor = policy_command(&bench, &["doctor", "--json"])
        .output()
        .unwrap();
    let found = report(&doctor);
    let policy = check(&found, "policy");
    assert_eq!(policy["status"], "ok");
    assert!(
        policy["detail"]
            .as_str()
            .unwrap()
            .contains("the command policy forbids nothing"),
        "{policy}"
    );
}

#[test]
fn a_policy_file_beside_the_configuration_is_applied_and_reported() {
    let bench = Bench::new("policy-file");
    fs::copy(POLICY, bench.path("policy.json")).unwrap();
    let shown = policy_command(&bench, &["policy", "--show"])
        .output()
        .unwrap();
    assert_eq!(shown.status.code(), Some(0), "{}", both(&shown));
    assert_eq!(
        String::from_utf8_lossy(&shown.stdout),
        format!(
            "{} is valid; rules in force: rm-recursive-force, rm-protected, empty-variable, find-root, kill-by-pattern, other-worktree, no-verify, git-push, pipe-to-shell, sudo\n",
            bench.path("policy.json").display()
        )
    );
    let verdicts = fed(
        policy_command(&bench, &["policy"]),
        "rm -rf scratch\nls\nfind / -name x\n",
    );
    assert_eq!(
        String::from_utf8_lossy(&verdicts.stdout),
        "deny\trm-recursive-force\nallow\ndeny\tfind-root\n",
        "{}",
        both(&verdicts)
    );
    let answer = fed(
        policy_command(&bench, &["hook"]),
        &hook_event("rm -rf scratch"),
    );
    let found = answer_of(&answer);
    assert_eq!(found["decision"], "deny");
    assert_eq!(found["rule"], "rm-recursive-force");
    assert!(
        found["reason"]
            .as_str()
            .unwrap()
            .contains("denied by rule rm-recursive-force"),
        "{found}"
    );
    let doctor = policy_command(&bench, &["doctor", "--json"])
        .output()
        .unwrap();
    let found = report(&doctor);
    let policy = check(&found, "policy");
    assert_eq!(policy["status"], "ok");
    assert!(
        policy["detail"]
            .as_str()
            .unwrap()
            .contains("rules in force: rm-recursive-force"),
        "{policy}"
    );

    fs::write(
        bench.path("policy.json"),
        r#"{"rules": {"find-root": {"forbids": "find from the root", "instead": "find in one directory"}}}"#,
    )
    .unwrap();
    let verdicts = fed(
        policy_command(&bench, &["policy"]),
        "rm -rf scratch\nfind / -name x\n",
    );
    assert_eq!(
        String::from_utf8_lossy(&verdicts.stdout),
        "allow\ndeny\tfind-root\n",
        "{}",
        both(&verdicts)
    );
}

#[test]
fn a_user_policy_file_is_found_under_xdg_config_home() {
    let bench = Bench::new("policy-xdg");
    let directory = bench.path("c").join("job");
    fs::create_dir_all(&directory).unwrap();
    fs::copy(POLICY, directory.join("policy.json")).unwrap();
    let mut command = bench.clean(Path::new(JOB));
    command
        .args(["policy", "--show"])
        .env("HOME", bench.path("home"))
        .env("XDG_CONFIG_HOME", bench.path("c"));
    let shown = command.output().unwrap();
    assert_eq!(shown.status.code(), Some(0), "{}", both(&shown));
    assert!(
        String::from_utf8_lossy(&shown.stdout).starts_with(&format!(
            "{} is valid; rules in force:",
            directory.join("policy.json").display()
        )),
        "{}",
        both(&shown)
    );
}

#[test]
fn an_invalid_policy_file_is_refused_with_the_reason() {
    let bench = Bench::new("policy-invalid");
    let file = bench.path("policy.json");
    for (text, reason) in [
        ("{", "not a valid policy file: "),
        (r#"{"colour": 1}"#, "unknown field `colour`"),
        (
            r#"{"rules": {"no-such-rule": {"forbids": "a", "instead": "b"}}}"#,
            "unknown rule no-such-rule; the rules are rm-recursive-force, ",
        ),
        (
            r#"{"rules": {"sudo": {"forbids": "", "instead": "b"}}}"#,
            "rule sudo needs a text for forbids and for instead",
        ),
        (
            r#"{"work_root": "work"}"#,
            "work_root must be an absolute path",
        ),
        (
            r#"{"rules": {"other-worktree": {"forbids": "a", "instead": "b"}}}"#,
            "rule other-worktree needs work_root or repos_root",
        ),
        (
            r#"{"rules": {"rm-protected": {"forbids": "a", "instead": "b"}}}"#,
            "rule rm-protected needs protected_paths",
        ),
    ] {
        fs::write(&file, text).unwrap();
        let shown = policy_command(&bench, &["policy", "--show"])
            .output()
            .unwrap();
        assert_ne!(shown.status.code(), Some(0), "{text}");
        let message = both(&shown);
        assert!(message.contains(&file.display().to_string()), "{message}");
        assert!(message.contains(reason), "{text}: {message}");
        let verdicts = fed(policy_command(&bench, &["policy"]), "ls\n");
        assert_ne!(verdicts.status.code(), Some(0), "{text}");
        assert!(both(&verdicts).contains(reason), "{}", both(&verdicts));
    }
    let answer = fed(policy_command(&bench, &["hook"]), &hook_event("ls"));
    let text = both(&answer);
    assert!(text.contains("\"deny\""), "{text}");
    assert!(
        text.contains("every command is refused until the file is corrected or removed"),
        "{text}"
    );
    assert!(
        text.contains("rule rm-protected needs protected_paths"),
        "{text}"
    );
    let doctor = policy_command(&bench, &["doctor", "--json"])
        .output()
        .unwrap();
    assert_eq!(doctor.status.code(), Some(1), "{}", both(&doctor));
    let found = report(&doctor);
    let policy = check(&found, "policy");
    assert_eq!(policy["status"], "fail");
    assert!(
        policy["fix"]
            .as_str()
            .unwrap()
            .contains("job policy --show validates it"),
        "{policy}"
    );
}

#[test]
fn the_hook_answers_one_json_question_with_allow_deny_or_rewrite() {
    let mut bench = Bench::new("hook-contract");
    let allowed = fed(
        policy_command(&bench, &["hook"]),
        r#"{"command": "ls -la"}"#,
    );
    assert_eq!(
        String::from_utf8_lossy(&allowed.stdout),
        "{\"decision\":\"allow\"}\n",
        "{}",
        both(&allowed)
    );
    let unanswered = answer_of(&fed(
        policy_command(&bench, &["hook"]),
        r#"{"command": "cargo test"}"#,
    ));
    assert_eq!(unanswered["decision"], "allow");
    assert!(
        unanswered["reason"]
            .as_str()
            .unwrap()
            .contains("the service does not answer"),
        "{unanswered}"
    );
    let refused = answer_of(&fed(
        policy_command(&bench, &["hook"]),
        r#"{"command": "cd build && cargo test"}"#,
    ));
    assert_eq!(refused["decision"], "deny");
    assert!(
        refused["reason"]
            .as_str()
            .unwrap()
            .contains("Run `cd build` as its own call first, then `cargo test`"),
        "{refused}"
    );
    for input in ["", "not json", "{}", r#"{"cwd": "/tmp"}"#, r#"["ls"]"#] {
        let output = fed(policy_command(&bench, &["hook"]), input);
        assert_eq!(output.status.code(), Some(125), "{input}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "", "{input}");
        assert!(
            both(&output).contains("standard input is not one JSON object with a command"),
            "{}",
            both(&output)
        );
    }

    let mut daemon = bench.explicit(Path::new(JOB));
    daemon.arg("daemon");
    let socket = bench.path("state").join("daemon.sock");
    assert!(bench.start(daemon, &socket), "{}", bench.errors());
    let routed = answer_of(&fed(
        policy_command(&bench, &["hook"]),
        r#"{"command": "cargo test", "caller": "build 7"}"#,
    ));
    assert_eq!(routed["decision"], "rewrite");
    assert_eq!(routed["detach"], true);
    let command = routed["command"].as_str().unwrap();
    assert!(
        command.starts_with(
            "job run --session 'build 7' --budget none --shell bash --summary -- 'cargo test';"
        ),
        "{command}"
    );
    assert!(
        routed["reason"]
            .as_str()
            .unwrap()
            .contains("because it runs cargo"),
        "{routed}"
    );
    let waiting = answer_of(&fed(
        policy_command(&bench, &["hook"]),
        r#"{"command": "job wait 3", "detached": true}"#,
    ));
    assert_eq!(waiting, serde_json::json!({"decision": "allow"}));
    let held = answer_of(&fed(
        policy_command(&bench, &["hook"]),
        r#"{"command": "job wait 3"}"#,
    ));
    assert_eq!(held["decision"], "rewrite");
    assert!(held["command"].as_str().unwrap().starts_with("job wait 3;"));
}
