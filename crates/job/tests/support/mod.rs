use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub(crate) struct Service {
    pub(crate) root: PathBuf,
    pub(crate) cgroup: PathBuf,
    pub(crate) child: Child,
    _delegation: Delegation,
}

static DELEGATION: Mutex<()> = Mutex::new(());

struct Delegation {
    root: PathBuf,
    driver: PathBuf,
    controllers: Vec<String>,
    _guard: MutexGuard<'static, ()>,
}

impl Delegation {
    fn new() -> Self {
        let guard = DELEGATION.lock().unwrap_or_else(|error| error.into_inner());
        let own = fs::read_to_string("/proc/self/cgroup").unwrap();
        let relative = own
            .lines()
            .find_map(|line| line.strip_prefix("0::"))
            .unwrap();
        let root = Path::new("/sys/fs/cgroup").join(relative.trim_start_matches('/'));
        assert_ne!(
            root,
            Path::new("/sys/fs/cgroup"),
            "run through job in a delegated cgroup"
        );
        assert!(
            fs::read_to_string(root.join("cgroup.subtree_control"))
                .unwrap()
                .trim()
                .is_empty()
        );
        let driver = root.join(format!("job-test-driver-{}", std::process::id()));
        fs::create_dir(&driver).unwrap();
        let mut delegation = Self {
            root,
            driver,
            controllers: Vec::new(),
            _guard: guard,
        };
        let available = fs::read_to_string(delegation.root.join("cgroup.controllers")).unwrap();
        for controller in ["cpu", "memory", "pids"] {
            assert!(
                available.split_whitespace().any(|name| name == controller),
                "missing delegated controller {controller}"
            );
        }
        let processes = fs::read_to_string(delegation.root.join("cgroup.procs")).unwrap();
        for pid in processes.split_whitespace() {
            if let Err(error) = fs::write(delegation.driver.join("cgroup.procs"), pid) {
                assert_eq!(error.raw_os_error(), Some(libc::ESRCH), "{error}");
            }
        }
        delegation.controllers = available
            .split_whitespace()
            .filter(|name| ["cpu", "memory", "pids", "io"].contains(name))
            .map(str::to_owned)
            .collect();
        fs::write(
            delegation.root.join("cgroup.subtree_control"),
            delegation
                .controllers
                .iter()
                .map(|name| format!("+{name}"))
                .collect::<Vec<_>>()
                .join(" "),
        )
        .unwrap();
        delegation
    }
}

impl Drop for Delegation {
    fn drop(&mut self) {
        let disabled = fs::write(
            self.root.join("cgroup.subtree_control"),
            self.controllers
                .iter()
                .map(|name| format!("-{name}"))
                .collect::<Vec<_>>()
                .join(" "),
        );
        if disabled.is_ok() {
            for pid in fs::read_to_string(self.driver.join("cgroup.procs"))
                .unwrap_or_default()
                .split_whitespace()
            {
                let _ = fs::write(self.root.join("cgroup.procs"), pid);
            }
            let _ = fs::remove_dir(&self.driver);
        }
    }
}

impl Service {
    pub(crate) fn start(name: &str) -> Self {
        let delegation = Delegation::new();
        let root =
            std::env::temp_dir().join(format!("job-freezer-live-{}-{name}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("config.toml"),
            "schema_version = 1\nprofile = 'ordinary'\n",
        )
        .unwrap();
        let cgroup = delegation
            .root
            .join(format!("job-freezer-test-{}-{name}", std::process::id()));
        fs::create_dir(&cgroup)
            .expect("run this test inside a writable delegated cgroup through job");
        let child = Self::spawn(&root, &cgroup);
        let mut service = Self {
            root,
            cgroup,
            child,
            _delegation: delegation,
        };
        service.ready();
        service
    }

    pub(crate) fn spawn(root: &Path, cgroup: &Path) -> Child {
        let daemon_group = if cgroup.join("daemon").is_dir() {
            cgroup.join("daemon")
        } else {
            cgroup.to_path_buf()
        };
        let procs = std::ffi::CString::new(
            daemon_group
                .join("cgroup.procs")
                .as_os_str()
                .as_encoded_bytes(),
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_job"));
        command
            .arg("daemon")
            .env("JOB_CONFIG", root.join("config.toml"))
            .env("JOB_STATE_DIR", root.join("state"))
            .env("JOB_CGROUP_ROOT", cgroup)
            .env("LC_ALL", "C")
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                fs::File::create(root.join("daemon.err")).unwrap(),
            ));
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
        command.spawn().unwrap()
    }

    pub(crate) fn ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "{}",
                fs::read_to_string(self.root.join("daemon.err")).unwrap()
            );
            if let Ok(mut stream) = UnixStream::connect(self.root.join("state/daemon.sock")) {
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                if writeln!(stream, "\"Ping\"").is_ok() {
                    let mut response = String::new();
                    if stream.read_to_string(&mut response).is_ok() && response.contains("Pong") {
                        break;
                    }
                }
            }
            assert!(Instant::now() < deadline, "daemon startup timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub(crate) fn restart(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        self.child = Self::spawn(&self.root, &self.cgroup);
        self.ready();
    }

    pub(crate) fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_job"))
            .args(args)
            .env("JOB_CLI_COMPAT", "legacy")
            .env("JOB_STATE_DIR", self.root.join("state"))
            .env("JOB_SESSION", "freezer-test")
            .env("LC_ALL", "C")
            .current_dir(&self.root)
            .output()
            .unwrap()
    }

    pub(crate) fn ok(&self, args: &[&str]) -> String {
        let output = self.cli(args);
        assert!(
            output.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    pub(crate) fn status(&self, id: &str) -> serde_json::Value {
        let output = self.cli(&["status", id, "--json"]);
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)))
    }

    pub(crate) fn wait_file(&self, name: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.root.join(name).exists() {
            assert!(Instant::now() < deadline, "missing {name}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub(crate) fn events(&self, id: &str) -> String {
        fs::read_to_string(self.cgroup.join("jobs").join(id).join("cgroup.events")).unwrap()
    }
}

fn remove_cgroups(path: &Path) {
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                remove_cgroups(&entry.path());
            }
        }
    }
    let _ = fs::remove_dir(path);
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = fs::write(self.cgroup.join("cgroup.kill"), "1");
        let _ = self.child.wait();
        let deadline = Instant::now() + Duration::from_secs(5);
        while fs::read_to_string(self.cgroup.join("cgroup.events"))
            .is_ok_and(|value| value.contains("populated 1"))
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        remove_cgroups(&self.cgroup);
        let _ = fs::remove_dir_all(&self.root);
    }
}
