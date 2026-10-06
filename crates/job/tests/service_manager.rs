use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const JOB: &str = env!("CARGO_BIN_EXE_job");

const TICKER: &str = "trap 'echo got-usr1' USR1; echo $$ > ticker.pid; i=0; while [ $i -lt 900 ]; do echo tick $i; i=$((i+1)); sleep 0.1; done";

const ECHO: &str = "stty -echo; printf 'READY\\n'; while IFS= read -r line; do case \"$line\" in quit) exit 7;; *) printf 'received:%s\\n' \"$line\";; esac; done";

fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(tool).is_file()))
}

fn start_ticks(pid: u64) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    if fields.first() == Some(&"Z") {
        return None;
    }
    fields.get(19)?.parse().ok()
}

struct Supervised {
    base: PathBuf,
    service: PathBuf,
    runsv: Child,
    jobs: Vec<String>,
}

impl Supervised {
    fn start(name: &str) -> Option<Self> {
        for tool in ["runsv", "sv"] {
            if !on_path(tool) {
                println!("skipped: {tool} is not on PATH, no runit supervision to test under");
                return None;
            }
        }
        let base = std::env::temp_dir().join(format!("job-sm-{}-{name}", std::process::id()));
        let service = base.join("sv/jobd");
        std::fs::create_dir_all(&service).unwrap();
        std::fs::create_dir_all(base.join("work")).unwrap();
        std::fs::create_dir_all(base.join("bin")).unwrap();
        std::os::unix::fs::symlink(JOB, base.join("bin/jobd")).unwrap();
        std::fs::write(
            base.join("config.toml"),
            "schema_version = 1\nprofile = 'ordinary'\n",
        )
        .unwrap();
        let run = service.join("run");
        std::fs::write(
            &run,
            format!(
                "#!/bin/sh\nexec env JOB_CONFIG='{base}/config.toml' JOB_STATE_DIR='{base}/state' JOB_RUNTIME_DIR='{base}/run' JOB_CGROUP_ROOT='{base}/absent-cgroup' LC_ALL=C '{base}/bin/jobd' --foreground 2>>'{base}/daemon.err'\n",
                base = base.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runsv = Command::new("runsv")
            .arg(&service)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let supervised = Self {
            base,
            service,
            runsv,
            jobs: Vec::new(),
        };
        supervised.ready(None);
        Some(supervised)
    }

    fn errors(&self) -> String {
        std::fs::read_to_string(self.base.join("daemon.err")).unwrap_or_default()
    }

    fn daemon_pid(&self) -> Option<u64> {
        std::fs::read_to_string(self.service.join("supervise/pid"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    fn ping(&self) -> bool {
        self.call(serde_json::json!("Ping"))
            .is_some_and(|value| value.get("Pong").is_some())
    }

    fn ready(&self, previous: Option<u64>) -> u64 {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if let Some(pid) = self.daemon_pid()
                && previous != Some(pid)
                && self.ping()
            {
                return pid;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the supervised daemon did not answer: {}", self.errors());
    }

    fn sv(&self, operation: &str) -> Output {
        Command::new("sv")
            .args(["-w", "10", operation])
            .arg(&self.service)
            .output()
            .unwrap()
    }

    fn sv_ok(&self, operation: &str) -> String {
        let output = self.sv(operation);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success(), "sv {operation}: {text}");
        text
    }

    fn call(&self, request: serde_json::Value) -> Option<serde_json::Value> {
        let mut stream = UnixStream::connect(self.base.join("run/job.sock")).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let request = if request.is_string() {
            request
        } else {
            serde_json::json!({"Versioned": {"protocol": 20, "request": request}})
        };
        writeln!(stream, "{request}").ok()?;
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn job(&self, args: &[&str]) -> Output {
        Command::new(JOB)
            .args(args)
            .env_remove("JOB_CLI_COMPAT")
            .env("JOB_STATE_DIR", self.base.join("state"))
            .env("JOB_RUNTIME_DIR", self.base.join("run"))
            .env("JOB_SESSION", "service-manager")
            .env("LC_ALL", "C")
            .current_dir(self.base.join("work"))
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.job(args);
        assert!(
            output.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn submit(&mut self, args: &[&str]) -> String {
        let mut all = vec!["submit"];
        all.extend_from_slice(args);
        let id = self.ok(&all);
        assert!(id.parse::<u64>().is_ok(), "{id}");
        self.jobs.push(id.clone());
        id
    }

    fn status(&self, id: &str) -> serde_json::Value {
        let output = self.job(&["status", id, "--json"]);
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{error}: {}\nsv: {:?}\ndaemon: {}",
                String::from_utf8_lossy(&output.stderr),
                self.sv("status"),
                self.errors()
            )
        })
    }

    fn wait_state(&self, id: &str, state: &str) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = self.status(id);
            if status["state"] == state {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "job {id} is {} instead of {state}",
                status["state"]
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn count(&self, id: &str, text: &str) -> usize {
        self.ok(&["logs", id])
            .lines()
            .filter(|line| line.contains(text))
            .count()
    }

    fn wait_count(&self, id: &str, text: &str, more_than: usize) -> usize {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let count = self.count(id, text);
            if count > more_than {
                return count;
            }
            assert!(
                Instant::now() < deadline,
                "job {id} printed {text} {count} times, expected more than {more_than}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn attach(&self, id: &str) -> Terminal {
        let response = self
            .call(serde_json::json!({"Attach":{"id":id.parse::<u64>().unwrap()}}))
            .unwrap();
        let path = response["Attached"]["path"]
            .as_str()
            .unwrap_or_else(|| panic!("{response}"));
        let stream = UnixStream::connect(path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut terminal = Terminal {
            stream,
            seen: Vec::new(),
        };
        terminal.send(2, &[24u16.to_be_bytes(), 80u16.to_be_bytes()].concat());
        terminal
    }
}

impl Drop for Supervised {
    fn drop(&mut self) {
        if self.runsv.try_wait().ok().flatten().is_some() {
            let _ = std::fs::remove_dir_all(&self.base);
            return;
        }
        let _ = self.sv("up");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.ping() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        for id in &self.jobs {
            let _ = self.job(&["cancel", id]);
        }
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline
            && self.jobs.iter().any(|id| {
                let output = self.job(&["status", id, "--json"]);
                serde_json::from_slice::<serde_json::Value>(&output.stdout).is_ok_and(|status| {
                    ["Running", "Stopping", "Starting"]
                        .iter()
                        .any(|state| status["state"] == *state)
                })
            })
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.sv("exit");
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && self.runsv.try_wait().ok().flatten().is_none() {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.runsv.kill();
        let _ = self.runsv.wait();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

struct Terminal {
    stream: UnixStream,
    seen: Vec<u8>,
}

impl Terminal {
    fn send(&mut self, kind: u8, body: &[u8]) {
        self.stream
            .write_all(&[&[kind][..], &(body.len() as u32).to_be_bytes()[..], body].concat())
            .unwrap();
    }

    fn receive(&mut self) -> (u8, Vec<u8>) {
        let mut header = [0; 5];
        self.stream.read_exact(&mut header).unwrap();
        let mut body = vec![0; u32::from_be_bytes(header[1..].try_into().unwrap()) as usize];
        assert!(body.len() <= 1024 * 1024);
        self.stream.read_exact(&mut body).unwrap();
        if header[0] == 3 {
            self.seen.extend_from_slice(&body);
        }
        (header[0], body)
    }

    fn until(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !String::from_utf8_lossy(&self.seen).contains(text) {
            assert!(Instant::now() < deadline, "no {text} on the terminal");
            let (kind, _) = self.receive();
            assert_eq!(kind, 3, "the terminal ended before {text}");
        }
    }
}

struct Watched {
    id: String,
    supervisor: u64,
    ticks: u64,
}

impl Watched {
    fn of(service: &Supervised, id: &str) -> Self {
        let status = service.wait_state(id, "Running");
        let supervisor = status["shim_pid"].as_u64().unwrap();
        Self {
            id: id.to_owned(),
            supervisor,
            ticks: start_ticks(supervisor).unwrap(),
        }
    }

    fn check(&self, service: &Supervised, after: &str) {
        let status = service.status(&self.id);
        assert_eq!(status["state"], "Running", "job {} after {after}", self.id);
        assert_eq!(
            status["shim_pid"].as_u64(),
            Some(self.supervisor),
            "job {} after {after}",
            self.id
        );
        assert_eq!(
            start_ticks(self.supervisor),
            Some(self.ticks),
            "the supervisor of job {} is not the same process after {after}",
            self.id
        );
    }
}

fn workload(service: &Supervised) -> (u64, u64) {
    let path = service.base.join("work/ticker.pid");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(pid) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
        {
            return (pid, start_ticks(pid).unwrap());
        }
        assert!(Instant::now() < deadline, "the ticker did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn gone(pid: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if start_ticks(pid).is_none() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn daemon_gone(pid: u64) -> bool {
    gone(pid) || !Path::new(&format!("/proc/{pid}")).exists()
}

#[test]
fn runit_restart_term_kill_and_down_up_keep_running_jobs_adopted_and_controllable() {
    let Some(mut service) = Supervised::start("restart") else {
        return;
    };
    assert!(service.sv_ok("status").starts_with("run:"));
    service.ok(&["queue", "create", "parked"]);
    service.ok(&["queue", "pause", "parked"]);
    let ticker = service.submit(&["--", "sh", "-c", TICKER]);
    let sleeper = service.submit(&["--", "sleep", "30"]);
    let terminal = service.submit(&["--pty", "--", "sh", "-c", ECHO]);
    let parked = service.submit(&[
        "-q",
        "parked",
        "--",
        "sh",
        "-c",
        "echo parked-ran > parked.out",
    ]);
    let watched: Vec<Watched> = [&ticker, &sleeper, &terminal]
        .iter()
        .map(|id| Watched::of(&service, id))
        .collect();
    let (ticker_pid, ticker_ticks) = workload(&service);
    service.attach(&terminal).until("READY");
    let mut usr1 = 0;
    let mut daemons = vec![service.daemon_pid().unwrap()];
    for operation in ["restart", "term", "kill", "down-up"] {
        let previous = *daemons.last().unwrap();
        if operation == "down-up" {
            assert!(service.sv_ok("down").starts_with("ok: down:"));
            assert!(daemon_gone(previous));
            let refused = service.job(&["status", &ticker, "--json"]);
            assert_eq!(refused.status.code(), Some(125));
            assert!(
                String::from_utf8_lossy(&refused.stderr).contains("the daemon does not answer"),
                "{}",
                String::from_utf8_lossy(&refused.stderr)
            );
            assert_eq!(start_ticks(ticker_pid), Some(ticker_ticks));
            for job in &watched {
                assert_eq!(start_ticks(job.supervisor), Some(job.ticks));
            }
            assert!(service.sv_ok("up").starts_with("ok: run:"));
        } else {
            service.sv_ok(operation);
        }
        let current = service.ready(Some(previous));
        assert!(
            !daemons.contains(&current),
            "{operation} left the same daemon"
        );
        assert!(daemon_gone(previous));
        daemons.push(current);
        assert!(service.sv_ok("status").starts_with("run:"));
        for job in &watched {
            job.check(&service, operation);
        }
        assert_eq!(
            start_ticks(ticker_pid),
            Some(ticker_ticks),
            "the workload changed after {operation}"
        );
        let ticks = service.count(&ticker, "tick ");
        service.wait_count(&ticker, "tick ", ticks);
        service.ok(&["signal", "-s", "USR1", &ticker]);
        usr1 = service.wait_count(&ticker, "got-usr1", usr1);
        let mut attached = service.attach(&terminal);
        attached.send(1, format!("after-{operation}\n").as_bytes());
        attached.until(&format!("received:after-{operation}"));
        assert_eq!(
            service.status(&parked)["state"],
            "Queued",
            "after {operation}"
        );
        let fresh = service.submit(&["--", "sh", "-c", &format!("echo fresh-{operation}")]);
        assert_eq!(
            service
                .job(&["wait", &fresh, "--timeout", "10s"])
                .status
                .code(),
            Some(0)
        );
        assert_eq!(service.ok(&["logs", &fresh]), format!("fresh-{operation}"));
    }
    assert_eq!(daemons.len(), 5);
    assert_eq!(usr1, 4);
    service.ok(&["queue", "resume", "parked"]);
    assert_eq!(
        service
            .job(&["wait", &parked, "--timeout", "10s"])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        std::fs::read_to_string(service.base.join("work/parked.out")).unwrap(),
        "parked-ran\n"
    );
    let mut attached = service.attach(&terminal);
    attached.send(1, b"quit\n");
    loop {
        let (kind, body) = attached.receive();
        if kind == 4 {
            assert_eq!(i32::from_be_bytes(body.try_into().unwrap()), 7);
            break;
        }
    }
    assert_eq!(
        service
            .job(&["wait", &terminal, "--timeout", "10s"])
            .status
            .code(),
        Some(7)
    );
    service.ok(&["cancel", &ticker]);
    service.wait_state(&ticker, "Cancelled");
    assert!(gone(ticker_pid), "cancel left the ticker running");
    service.ok(&["cancel", &sleeper]);
    service.wait_state(&sleeper, "Cancelled");
    for job in &watched {
        assert!(gone(job.supervisor), "a supervisor outlived its job");
    }
    let last = *daemons.last().unwrap();
    service.sv_ok("exit");
    let deadline = Instant::now() + Duration::from_secs(10);
    while service.runsv.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "runsv did not exit");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(daemon_gone(last));
}

#[test]
fn systemd_units_pass_static_verification_where_systemd_is_installed() {
    if !on_path("systemd-analyze") {
        println!(
            "skipped: systemd-analyze is not on PATH; the systemd units stay unverified on this host"
        );
        return;
    }
    let base = std::env::temp_dir().join(format!("job-sm-{}-units", std::process::id()));
    std::fs::create_dir_all(base.join("bin")).unwrap();
    std::os::unix::fs::symlink(JOB, base.join("bin/jobd")).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/systemd");
    let mut failures = Vec::new();
    for (template, scope) in [
        ("jobd.service.in", "--system"),
        ("jobd-user.service.in", "--user"),
    ] {
        let unit = std::fs::read_to_string(root.join(template))
            .unwrap()
            .replace("@bindir@", base.join("bin").to_str().unwrap());
        let directory = base.join(scope.trim_start_matches('-'));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("jobd.service");
        std::fs::write(&path, unit).unwrap();
        let output = Command::new("systemd-analyze")
            .args(["verify", "--man=no", scope])
            .arg(&path)
            .output()
            .unwrap();
        if !output.status.success() {
            failures.push(format!(
                "{template}: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    assert!(failures.is_empty(), "{failures:?}");
}
