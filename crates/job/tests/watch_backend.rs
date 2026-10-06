use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Daemon {
    child: Child,
    base: PathBuf,
    state: PathBuf,
    work: PathBuf,
}

impl Daemon {
    fn start(name: &str) -> Daemon {
        Self::with_profile(name, "legacy")
    }

    fn with_profile(name: &str, profile: &str) -> Daemon {
        Self::launch(name, profile, None)
    }

    fn with_path(name: &str, path: &str) -> Daemon {
        Self::launch(name, "legacy", Some(path))
    }

    fn launch(name: &str, profile: &str, path: Option<&str>) -> Daemon {
        let base = std::env::temp_dir().join(format!("job-it-{}-{name}", std::process::id()));
        let state = base.join("state");
        let work = base.join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(
            base.join("config.toml"),
            format!("schema_version = 1\nprofile = '{profile}'\n"),
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_job"));
        command
            .arg("daemon")
            .env("JOB_CGROUP_ROOT", base.join("absent-cgroup"))
            .env("JOB_CONFIG", base.join("config.toml"))
            .env("JOB_STATE_DIR", &state)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(path) = path {
            command.env("PATH", path);
        }
        let child = command.spawn().unwrap();
        let mut daemon = Daemon {
            child,
            base,
            state,
            work,
        };
        daemon.ready();
        daemon
    }

    fn restart(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(self.state.join("daemon.sock"));
        self.child = Command::new(env!("CARGO_BIN_EXE_job"))
            .arg("daemon")
            .env("JOB_CGROUP_ROOT", self.base.join("absent-cgroup"))
            .env("JOB_CONFIG", self.base.join("config.toml"))
            .env("JOB_STATE_DIR", &self.state)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        self.ready();
    }

    fn ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "daemon exited during startup"
            );
            if let Ok(mut stream) = UnixStream::connect(self.state.join("daemon.sock")) {
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut response = String::new();
                if writeln!(stream, "\"Ping\"").is_ok()
                    && stream.read_to_string(&mut response).is_ok()
                    && serde_json::from_str::<serde_json::Value>(&response)
                        .is_ok_and(|value| value.get("Pong").is_some())
                {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("daemon did not become ready");
    }

    fn job_command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_job"));
        command
            .args(args)
            .env("JOB_CLI_COMPAT", "legacy")
            .env("JOB_STATE_DIR", &self.state)
            .env("JOB_SESSION", "integration")
            .current_dir(&self.work);
        command
    }

    fn job(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_job"))
            .args(args)
            .env("JOB_CLI_COMPAT", "legacy")
            .env("JOB_STATE_DIR", &self.state)
            .env("JOB_SESSION", "integration")
            .current_dir(&self.work)
            .output()
            .unwrap()
    }

    fn file(&self, name: &str) -> PathBuf {
        self.work.join(name)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn started_and_finished(daemon: &Daemon, id: &str) -> (u64, u64) {
    let status = daemon.job(&["status", id, "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    (
        job["started_ms"].as_u64().unwrap(),
        job["finished_ms"].as_u64().unwrap(),
    )
}

#[test]
fn a_finished_job_answers_with_its_exit_status_and_its_output() {
    let daemon = Daemon::start("answer");
    let output = daemon.job(&["run", "--", "echo hello; echo to-stderr >&2; exit 3"]);
    let answer = text(&output);
    assert_eq!(output.status.code(), Some(3));
    assert!(answer.starts_with("[job] job "), "{answer}");
    assert!(answer.contains("exit 3"), "{answer}");
    assert!(answer.contains("hello\nto-stderr"), "{answer}");
}

#[test]
fn two_jobs_whose_reservations_do_not_fit_together_run_one_after_the_other() {
    let daemon = Daemon::start("serial");
    let cores = std::thread::available_parallelism().unwrap().get();
    let most = (cores / 2 + 1).to_string();
    let first = text(&daemon.job(&["submit", "--cores", &most, "--", "sleep 1"]));
    let second = text(&daemon.job(&["submit", "--cores", &most, "--", "true"]));
    let waited = daemon.job(&["wait", second.trim()]);
    assert_eq!(waited.status.code(), Some(0), "{}", text(&waited));
    let (_, first_end) = started_and_finished(&daemon, first.trim());
    let (second_start, _) = started_and_finished(&daemon, second.trim());
    assert!(second_start >= first_end, "{second_start} < {first_end}");
    let status = daemon.job(&["status", second.trim(), "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    let reason = job["waited_for"].as_str().unwrap_or_default();
    assert!(
        reason.starts_with("cores held by jobs") && reason.contains(first.trim()),
        "{reason}"
    );
}

#[test]
fn a_job_past_its_memory_is_stopped_with_a_record_line() {
    let daemon = Daemon::start("memory");
    let output = daemon.job(&[
        "run",
        "--mem",
        "100M",
        "--",
        "python3 -c 'import time; b = bytearray(300 * 1024 * 1024); time.sleep(3)'",
    ]);
    let answer = text(&output);
    assert_ne!(output.status.code(), Some(0));
    assert!(answer.contains("stopped: memory reached"), "{answer}");
    assert!(answer.contains("reservation of 100 MiB"), "{answer}");
}

#[test]
fn a_job_past_its_processes_is_stopped_with_a_record_line() {
    let daemon = Daemon::start("pids");
    let output = daemon.job(&[
        "run",
        "--pids",
        "20",
        "--",
        "for i in $(seq 50); do sleep 3 & done; wait",
    ]);
    let answer = text(&output);
    assert!(answer.contains("past its limit of 20"), "{answer}");
    assert!(!answer.contains("were still running"), "{answer}");
}

#[test]
fn a_job_past_its_disk_reservation_is_stopped_with_a_record_line() {
    let daemon = Daemon::start("disk");
    let output = daemon.job(&[
        "run",
        "--disk",
        "10M",
        "--",
        "dd if=/dev/zero of=fill.bin bs=1M count=30 status=none; sync fill.bin; sleep 3",
    ]);
    let answer = text(&output);
    assert!(
        answer.contains("past its disk reservation of 10 MiB"),
        "{answer}"
    );
    let _ = std::fs::remove_file(daemon.file("fill.bin"));
}

#[test]
fn the_log_answers_queries() {
    let daemon = Daemon::start("log");
    let output = daemon.job(&["run", "--", "seq 1 100; echo 'error: the one that matters'"]);
    let answer = text(&output);
    assert!(answer.contains("[the log has 101 lines"), "{answer}");
    assert!(answer.contains("error: the one that matters"), "{answer}");
    let id = answer
        .split(':')
        .next()
        .unwrap()
        .trim_start_matches("[job] job ")
        .to_string();
    let errors = text(&daemon.job(&["log", &id, "errors"]));
    assert!(
        errors.trim().ends_with("error: the one that matters"),
        "{errors}"
    );
    let lines = text(&daemon.job(&["log", &id, "lines", "2..3"]));
    assert_eq!(lines.lines().count(), 2, "{lines}");
    assert!(
        Path::new(&daemon.state)
            .join("jobs")
            .join(&id)
            .join("output.log")
            .exists()
    );
}

#[test]
fn a_wait_survives_a_daemon_restart_and_the_running_job_is_adopted() {
    let mut daemon = Daemon::start("restart");
    let id = text(&daemon.job(&["submit", "--", "sleep 2; echo finished after the restart"]));
    let waiter = daemon
        .job_command(&["wait", id.trim()])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    daemon.restart();
    let output = waiter.wait_with_output().unwrap();
    let answer = text(&output);
    assert_eq!(output.status.code(), Some(0), "{answer}");
    assert!(
        answer.starts_with(&format!("[job] job {}: exit 0", id.trim())),
        "{answer}"
    );
    assert!(answer.contains("finished after the restart"), "{answer}");
}

#[test]
fn a_cargo_job_without_history_does_not_hold_back_a_small_one() {
    let daemon = Daemon::start("cargo-admission");
    let cargo = daemon
        .job_command(&[
            "submit",
            "--",
            "cargo build --manifest-path /nonexistent/Cargo.toml 2>/dev/null; sleep 2",
        ])
        .env("CARGO_BUILD_JOBS", "1")
        .output()
        .unwrap();
    let cargo = text(&cargo);
    let small = text(&daemon.job(&["submit", "--", "true"]));
    let waited = daemon.job(&["wait", small.trim()]);
    assert_eq!(waited.status.code(), Some(0), "{}", text(&waited));
    daemon.job(&["wait", cargo.trim()]);
    let (small_start, _) = started_and_finished(&daemon, small.trim());
    let (_, cargo_end) = started_and_finished(&daemon, cargo.trim());
    assert!(small_start < cargo_end, "{small_start} >= {cargo_end}");
    let status = daemon.job(&["status", cargo.trim(), "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(job["reservation"]["disk"], 0);
    assert_eq!(job["reservation"]["cores_source"]["Rule"]["tool"], "cargo");
}

#[test]
fn a_restarted_daemon_reestimates_a_reservation_that_older_rules_made_too_large() {
    let mut daemon = Daemon::start("reestimate");
    let first = text(&daemon.job(&["submit", "--cores", "1", "--", "sleep 4"]));
    std::thread::sleep(Duration::from_millis(500));
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap();
    let total_kib: u64 = meminfo
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap();
    let pool = total_kib * 1024 / 4 * 3;
    let _ = daemon.child.kill();
    let _ = daemon.child.wait();
    let path = daemon
        .state
        .join("jobs")
        .join(first.trim())
        .join("job.json");
    let mut job: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    job["reservation"]["vector"]["memory"] = serde_json::json!(pool);
    std::fs::write(&path, serde_json::to_string(&job).unwrap()).unwrap();
    daemon.restart();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !daemon.state.join("daemon.sock").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(300));
    let half = (pool / 2).to_string();
    let second = text(&daemon.job(&["submit", "--mem", &half, "--", "true"]));
    let waited = daemon.job(&["wait", second.trim()]);
    assert_eq!(waited.status.code(), Some(0), "{}", text(&waited));
    daemon.job(&["wait", first.trim()]);
    let (second_start, _) = started_and_finished(&daemon, second.trim());
    let (_, first_end) = started_and_finished(&daemon, first.trim());
    assert!(second_start < first_end, "{second_start} >= {first_end}");
}

#[test]
fn a_disk_declaration_no_filesystem_can_hold_is_refused_at_once() {
    let daemon = Daemon::start("disk-refusal");
    let output = daemon.job(&["submit", "--disk", "100T", "--", "true"]);
    let message = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(125), "{message}");
    assert!(message.contains("declares 100.00 TiB of disk"), "{message}");
    assert!(message.contains("free above its floor"), "{message}");
}

#[test]
fn a_queued_job_whose_directory_vanished_ends_with_that_reason() {
    let daemon = Daemon::start("vanished");
    let cores = std::thread::available_parallelism().unwrap().get();
    let most = (cores / 2 + 1).to_string();
    let first = text(&daemon.job(&["submit", "--cores", &most, "--", "sleep 2"]));
    let doomed = daemon.file("doomed");
    std::fs::create_dir_all(&doomed).unwrap();
    let second = daemon
        .job_command(&["submit", "--cores", &most, "--", "true"])
        .current_dir(&doomed)
        .output()
        .unwrap();
    let second = text(&second);
    std::fs::remove_dir(&doomed).unwrap();
    let answer = daemon.job(&["wait", second.trim(), "--timeout", "1s"]);
    let answer = text(&answer);
    assert!(
        answer.contains("did not start: its working directory"),
        "{answer}"
    );
    assert!(answer.contains("no longer exists"), "{answer}");
    daemon.job(&["wait", first.trim()]);
}

#[test]
fn a_job_carries_the_label_from_the_environment_when_no_option_is_given() {
    let daemon = Daemon::start("session-id");
    let id = daemon
        .job_command(&["submit", "--", "true"])
        .env("JOB_SESSION", "label-from-the-environment")
        .output()
        .unwrap();
    let id = text(&id);
    let status = daemon.job(&["status", id.trim(), "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(job["spec"]["session"], "label-from-the-environment");
    let id = daemon
        .job_command(&["submit", "--", "true"])
        .env_remove("JOB_SESSION")
        .output()
        .unwrap();
    let id = text(&id);
    let status = daemon.job(&["status", id.trim(), "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(job["spec"]["session"], "unnamed");
    daemon.job(&["wait", id.trim()]);
}

#[test]
fn a_confined_job_writes_in_its_own_tree_and_is_refused_elsewhere() {
    let daemon = Daemon::start("confine");
    let probe = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("confine-probe-{}", std::process::id()));
    let _ = std::fs::remove_file(&probe);
    let line = format!("touch inside.txt && touch {}", probe.display());
    let output = daemon.job(&["run", "--confine", "--", &line]);
    let answer = text(&output);
    assert!(daemon.file("inside.txt").exists(), "{answer}");
    assert!(!probe.exists(), "{answer}");
    assert!(answer.contains("confined"), "{answer}");
    assert!(
        answer.contains("its output reports a refused write"),
        "{answer}"
    );
    let allowed = daemon.job(&[
        "run",
        "--allow-write",
        env!("CARGO_TARGET_TMPDIR"),
        "--",
        &format!("touch {}", probe.display()),
    ]);
    assert_eq!(allowed.status.code(), Some(0), "{}", text(&allowed));
    assert!(probe.exists());
    let _ = std::fs::remove_file(&probe);
    let record = std::fs::read_to_string(daemon.state.join("confine.jsonl")).unwrap();
    let entries: Vec<serde_json::Value> = record
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(entries.len(), 2, "{record}");
    assert_eq!(entries[0]["refused"], true, "{record}");
    assert_eq!(entries[1]["refused"], false, "{record}");
}

#[test]
fn a_process_left_behind_is_named_when_it_is_killed() {
    let daemon = Daemon::start("leftover");
    let output = daemon.job(&["run", "--", "sleep 30 & echo started"]);
    let answer = text(&output);
    assert!(answer.contains("started"), "{answer}");
    assert!(answer.contains("were killed: sleep 30 (pid"), "{answer}");
}

#[test]
fn a_queue_of_one_place_runs_its_jobs_in_order_and_says_what_each_waits_for() {
    let daemon = Daemon::start("queue-serial");
    let added = daemon.job(&["queue", "add", "serial", "--parallel", "1"]);
    assert_eq!(added.status.code(), Some(0), "{}", text(&added));
    assert!(
        text(&added).contains("one job at a time"),
        "{}",
        text(&added)
    );
    let ids: Vec<String> = (0..3)
        .map(|_| {
            text(&daemon.job(&[
                "submit",
                "-q",
                "serial",
                "--",
                "for n in {1..500}; do test -e release && exit 0; sleep .02; done; exit 1",
            ]))
            .trim()
            .to_string()
        })
        .collect();
    let shown = text(&daemon.job(&["queue", "serial"]));
    assert!(shown.contains("1 running, 2 waiting"), "{shown}");
    let waiting = daemon.job(&["status", &ids[2], "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&waiting.stdout).unwrap();
    assert_eq!(
        job["waited_for"].as_str(),
        Some(format!("job {}, before it in queue serial", ids[1]).as_str())
    );
    std::fs::write(daemon.file("release"), "go").unwrap();
    let last = daemon.job(&["wait", &ids[2]]);
    assert!(
        text(&last).starts_with(&format!("[job] job {} in queue serial: exit 0", ids[2])),
        "{}",
        text(&last)
    );
    let times: Vec<(u64, u64)> = ids
        .iter()
        .map(|id| started_and_finished(&daemon, id))
        .collect();
    assert!(
        times[1].0 >= times[0].1 && times[2].0 >= times[1].1,
        "{times:?}"
    );
}

#[test]
fn a_queue_with_jobs_is_removed_only_by_draining_and_survives_a_restart() {
    let mut daemon = Daemon::start("queue-rm");
    daemon.job(&["queue", "add", "slow", "--parallel", "2"]);
    let id = text(&daemon.job(&["submit", "-q", "slow", "--", "sleep 1"]))
        .trim()
        .to_string();
    let refused = daemon.job(&["queue", "rm", "slow"]);
    assert_ne!(refused.status.code(), Some(0));
    let message = String::from_utf8_lossy(&refused.stderr);
    assert!(
        message.contains("job queue rm slow --when-empty"),
        "{message}"
    );
    daemon.restart();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !daemon.state.join("daemon.sock").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        text(&daemon.job(&["queue"])).contains("queue slow: 2 jobs at a time"),
        "{}",
        text(&daemon.job(&["queue"]))
    );
    let drained = daemon.job(&["queue", "rm", "slow", "--when-empty"]);
    assert!(
        text(&drained).contains("takes no new jobs"),
        "{}",
        text(&drained)
    );
    let late = daemon.job(&["submit", "-q", "slow", "--", "true"]);
    assert_ne!(late.status.code(), Some(0));
    daemon.job(&["wait", &id]);
    std::thread::sleep(Duration::from_millis(600));
    assert!(!text(&daemon.job(&["queue"])).contains("queue slow"));
}

#[test]
fn a_paused_queue_holds_its_jobs_until_it_is_resumed_and_clear_cancels_them() {
    let daemon = Daemon::start("queue-pause");
    daemon.job(&["queue", "add", "held"]);
    daemon.job(&["queue", "pause", "held"]);
    let first = text(&daemon.job(&["submit", "-q", "held", "--", "true"]))
        .trim()
        .to_string();
    let second = text(&daemon.job(&["submit", "-q", "held", "--", "true"]))
        .trim()
        .to_string();
    std::thread::sleep(Duration::from_millis(600));
    let status = daemon.job(&["status", &first, "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(job["state"].as_str(), Some("Queued"));
    assert_eq!(
        job["waited_for"].as_str(),
        Some("queue held, which is paused")
    );
    let cleared = daemon.job(&["queue", "clear", "held"]);
    assert!(
        text(&cleared).contains(&format!("cancelled jobs {first}, {second}")),
        "{}",
        text(&cleared)
    );
    let third = text(&daemon.job(&["submit", "-q", "held", "--", "true"]))
        .trim()
        .to_string();
    daemon.job(&["queue", "resume", "held"]);
    let answer = daemon.job(&["wait", &third]);
    assert_eq!(answer.status.code(), Some(0), "{}", text(&answer));
}

#[test]
fn a_job_for_a_queue_that_does_not_exist_is_refused_with_the_command_to_create_it() {
    let daemon = Daemon::start("queue-missing");
    let refused = daemon.job(&["run", "-q", "nowhere", "--", "true"]);
    assert_ne!(refused.status.code(), Some(0));
    let message = String::from_utf8_lossy(&refused.stderr);
    assert!(message.contains("job queue add nowhere"), "{message}");
}

#[test]
fn a_daemon_whose_executable_was_replaced_still_starts_jobs() {
    let base = std::env::temp_dir().join(format!("job-it-{}-replaced", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let installed = base.join("job");
    std::fs::copy(env!("CARGO_BIN_EXE_job"), &installed).unwrap();
    let state = base.join("state");
    let mut attempts = 0;
    let mut daemon = loop {
        match Command::new(&installed)
            .arg("daemon")
            .env("JOB_CGROUP_ROOT", base.join("absent-cgroup"))
            .env("JOB_STATE_DIR", &state)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 50 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            other => break other.unwrap(),
        }
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !state.join("daemon.sock").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let replacement = base.join("job.new");
    std::fs::copy(env!("CARGO_BIN_EXE_job"), &replacement).unwrap();
    std::fs::rename(&replacement, &installed).unwrap();
    let answer = Command::new(env!("CARGO_BIN_EXE_job"))
        .args(["run", "--", "true"])
        .env("JOB_STATE_DIR", &state)
        .env("JOB_SESSION", "integration")
        .current_dir(&base)
        .output()
        .unwrap();
    let _ = daemon.kill();
    let _ = daemon.wait();
    let _ = std::fs::remove_dir_all(&base);
    assert_eq!(answer.status.code(), Some(0), "{}", text(&answer));
}

#[test]
fn a_queue_holding_two_cores_runs_two_jobs_of_two_cores_one_after_another() {
    let daemon = Daemon::start("queue-cores");
    let added = daemon.job(&["queue", "add", "pair", "--parallel", "all", "--cores", "2"]);
    assert!(
        text(&added).contains("as many jobs at a time as the pool admits")
            && text(&added).contains("holding at most 2 cores together"),
        "{}",
        text(&added)
    );
    let first = text(&daemon.job(&["submit", "-q", "pair", "--cores", "2", "--", "sleep 1"]))
        .trim()
        .to_string();
    let second = text(&daemon.job(&["submit", "-q", "pair", "--cores", "2", "--", "true"]))
        .trim()
        .to_string();
    let waited = daemon.job(&["wait", &second]);
    assert_eq!(waited.status.code(), Some(0), "{}", text(&waited));
    let (_, first_end) = started_and_finished(&daemon, &first);
    let (second_start, _) = started_and_finished(&daemon, &second);
    assert!(second_start >= first_end, "{second_start} < {first_end}");
    let status = daemon.job(&["status", &second, "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(
        job["waited_for"].as_str(),
        Some(format!("the 2 cores of queue pair, held by job {first}").as_str())
    );
    let refused = daemon.job(&["run", "-q", "pair", "--cores", "3", "--", "true"]);
    assert_ne!(refused.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("queue pair holds 2 cores"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
}

#[test]
fn a_queue_sets_the_directory_and_the_network_and_a_job_may_override_them() {
    let daemon = Daemon::start("queue-dir-net");
    let place = daemon.file("place");
    std::fs::create_dir_all(&place).unwrap();
    daemon.job(&[
        "queue",
        "add",
        "offline",
        "--dir",
        place.to_str().unwrap(),
        "--net",
        "none",
    ]);
    let isolated = daemon.job(&[
        "run",
        "-q",
        "offline",
        "--",
        "pwd; tail -n +3 /proc/net/dev | cut -d: -f1 | tr -d ' '",
    ]);
    let answer = text(&isolated);
    assert!(answer.contains("no network"), "{answer}");
    assert!(
        answer.contains(&format!("{}\nlo", place.display())),
        "{answer}"
    );
    let open = daemon.job(&[
        "run", "-q", "offline", "--net", "default", "--dir", ".", "--", "pwd",
    ]);
    let answer = text(&open);
    assert!(!answer.contains("no network"), "{answer}");
    assert!(
        answer.contains(&daemon.work.display().to_string()),
        "{answer}"
    );
}

fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(tool).is_file()))
}

#[test]
fn a_job_in_a_queue_with_a_bandwidth_is_held_to_its_share() {
    let tools = [
        "iperf3",
        "slirp4netns",
        "nft",
        "tc",
        "ip",
        "unshare",
        "nsenter",
    ];
    if let Some(missing) = tools.iter().find(|t| !on_path(t)) {
        eprintln!("skipped: {missing} is not installed");
        return;
    }
    let daemon = Daemon::start("bandwidth");
    let added = daemon.job(&[
        "queue",
        "add",
        "slow",
        "--bandwidth",
        "4Mbit",
        "--job-bandwidth",
        "2Mbit",
    ]);
    assert!(
        text(&added).contains("4 Mbit/s together each way, 2 Mbit/s per job"),
        "{}",
        text(&added)
    );
    let port = (20000 + std::process::id() % 20000).to_string();
    let mut server = Command::new("iperf3")
        .args(["-s", "-1", "-B", "127.0.0.1", "-p", &port])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let measured = daemon.job(&[
        "run",
        "-q",
        "slow",
        "--",
        &format!("iperf3 -c 198.18.0.2 -p {port} -t 6 -O 1 -l 8K -R -J"),
    ]);
    let _ = server.kill();
    let _ = server.wait();
    daemon.job(&["queue", "rm", "slow"]);
    let answer = text(&measured);
    assert!(
        answer.contains("network capped at 2 Mbit/s each way within 4 Mbit/s"),
        "{answer}"
    );
    let log = daemon.job(&["log", "1", "full"]);
    let report: serde_json::Value = serde_json::from_slice(&log.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: the log was {:?}; the answer was {answer}",
            String::from_utf8_lossy(&log.stdout)
        )
    });
    let received = report["end"]["sum_received"]["bits_per_second"]
        .as_f64()
        .unwrap_or_else(|| panic!("missing throughput: {report}; answer: {answer}"));
    assert!(
        (1_500_000.0..=2_000_000.0).contains(&received),
        "{received} bit/s"
    );
}

fn free_port(offset: u32) -> String {
    (21000 + (std::process::id() * 7 + offset) % 20000).to_string()
}

#[test]
fn a_job_behind_a_proxy_reaches_what_the_proxy_reaches_and_nothing_else() {
    let tools = [
        "python3",
        "curl",
        "slirp4netns",
        "nft",
        "ip",
        "unshare",
        "nsenter",
    ];
    if let Some(missing) = tools.iter().find(|t| !on_path(t)) {
        eprintln!("skipped: {missing} is not installed");
        return;
    }
    let daemon = Daemon::start("proxy");
    std::fs::write(daemon.file("page.txt"), "served through the proxy\n").unwrap();
    let web_port = free_port(1);
    let proxy_port = free_port(2);
    let mut web = Command::new("python3")
        .args(["-m", "http.server", "--bind", "127.0.0.1", &web_port])
        .current_dir(&daemon.work)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let socks = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/socks5.py");
    let log = daemon.file("socks.log");
    let mut proxy = Command::new("python3")
        .args([socks, &proxy_port, log.to_str().unwrap(), "30"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let answer = daemon.job(&[
        "run",
        "--net",
        &format!("socks5://127.0.0.1:{proxy_port}"),
        "--",
        &format!(
            "curl -sS -m 5 http://127.0.0.1:{web_port}/page.txt; curl --noproxy '*' -sS -m 5 http://198.18.0.2:{web_port}/page.txt; echo around=$?"
        ),
    ]);
    let _ = web.kill();
    let _ = proxy.kill();
    let _ = web.wait();
    let _ = proxy.wait();
    let text = text(&answer);
    assert!(text.contains("network only through the proxy"), "{text}");
    assert!(text.contains("served through the proxy"), "{text}");
    assert!(text.contains("around=7"), "{text}");
    assert!(
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .contains(&format!("CONNECT 127.0.0.1:{web_port}")),
        "the proxy saw no request"
    );
}

#[test]
fn a_remote_request_of_another_protocol_is_refused_with_both_versions() {
    let daemon = Daemon::start("remote-protocol");
    let mut child = daemon
        .job_command(&["remote"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"protocol\":999,\"op\":\"Host\"}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let said = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(124), "{said}");
    assert!(
        said.contains("speaks job protocol 20, the caller 999"),
        "{said}"
    );
}

#[test]
fn a_remote_request_is_answered_by_the_hosts_daemon() {
    let daemon = Daemon::start("remote-host");
    let mut child = daemon
        .job_command(&["remote"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"protocol\":20,\"op\":\"Host\"}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let info: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stderr)));
    assert_eq!(info["protocol"], 20);
    assert!(info["cores"].as_u64().unwrap() >= 1);
}

#[test]
fn a_remote_queue_hands_its_network_and_directory_to_the_other_host() {
    let daemon = Daemon::start("remote-settings");
    daemon.job(&[
        "queue",
        "add",
        "far",
        "--on",
        "nobody@127.0.0.1",
        "--ssh-option",
        "ConnectTimeout=1",
        "--net",
        "wireguard:/nowhere/tunnel.conf",
        "--dir",
        "~/work",
        "--job-bandwidth",
        "2Mbit",
        "--monitor",
        "left",
    ]);
    let id = text(&daemon.job(&["submit", "-q", "far", "--", "true"]))
        .trim()
        .to_string();
    daemon.job(&["wait", &id]);
    let status = daemon.job(&["status", &id, "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    let declared = &job["spec"]["declared"];
    assert_eq!(declared["net"]["WireGuard"], "/nowhere/tunnel.conf");
    assert_eq!(declared["dir"], "~/work");
    assert_eq!(declared["bandwidth"], 2_000_000);
    assert_eq!(declared["on"]["target"], "nobody@127.0.0.1");
    assert_eq!(declared["monitor"], "left");
}

#[test]
fn mixed_groups_keep_queue_identity_through_move_rename_and_restart() {
    let mut daemon = Daemon::start("mixed-groups");
    for args in [
        vec!["group", "create", "engineering"],
        vec!["group", "create", "engineering/releases"],
        vec!["queue", "create", "engineering/builds"],
    ] {
        let output = daemon.job(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let inspect = |daemon: &Daemon, path: &str| -> serde_json::Value {
        serde_json::from_slice(&daemon.job(&["queue", "show", path, "--json"]).stdout).unwrap()
    };
    let original = inspect(&daemon, "engineering/builds");
    assert_eq!(
        original["objects"][0]["object"]["config"],
        serde_json::json!({})
    );
    assert!(
        daemon
            .job(&[
                "queue",
                "move",
                "--group",
                "engineering/releases",
                "engineering/builds"
            ])
            .status
            .success()
    );
    assert!(
        daemon
            .job(&["group", "rename", "engineering", "development"])
            .status
            .success()
    );
    assert!(
        !daemon
            .job(&[
                "group",
                "move",
                "--group",
                "development/releases",
                "development"
            ])
            .status
            .success()
    );
    let moved = inspect(&daemon, "development/releases/builds");
    assert_eq!(
        moved["objects"][0]["object"]["id"],
        original["objects"][0]["object"]["id"]
    );
    daemon.restart();
    let after = inspect(&daemon, "development/releases/builds");
    assert_eq!(after, moved);
}

#[test]
fn group_pause_and_close_preserve_independent_queue_state() {
    let daemon = Daemon::start("group-holds");
    daemon.job(&["group", "create", "work"]);
    daemon.job(&["queue", "create", "work/builds"]);
    daemon.job(&["group", "pause", "work"]);
    daemon.job(&["queue", "pause", "work/builds"]);
    let first = text(&daemon.job(&["submit", "--queue", "work/builds", "--", "true"]));
    daemon.job(&["group", "resume", "work"]);
    let held: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["status", first.trim(), "--json"]).stdout).unwrap();
    assert_eq!(held["state"], "Queued");
    daemon.job(&["group", "close", "work"]);
    let refused = daemon.job(&["submit", "--queue", "work/builds", "--", "true"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("closed by work"));
    daemon.job(&["queue", "resume", "work/builds"]);
    assert!(
        daemon
            .job(&["wait", first.trim(), "--timeout", "5s"])
            .status
            .success()
    );
    assert!(
        daemon
            .job(&["queue", "show", "work/builds"])
            .status
            .success()
    );
}

#[test]
fn ancestor_concurrency_applies_across_sibling_queues() {
    let daemon = Daemon::start("group-concurrency");
    daemon.job(&["group", "create", "--max-running", "1", "work"]);
    daemon.job(&["queue", "create", "work/a"]);
    daemon.job(&["queue", "create", "work/b"]);
    let first = text(&daemon.job(&["submit", "--queue", "work/a", "--", "sleep 0.4"]));
    let second = text(&daemon.job(&["submit", "--queue", "work/b", "--", "true"]));
    assert!(
        daemon
            .job(&["wait", second.trim(), "--timeout", "5s"])
            .status
            .success()
    );
    let (_, end) = started_and_finished(&daemon, first.trim());
    let (start, _) = started_and_finished(&daemon, second.trim());
    assert!(start >= end);
}

#[test]
fn a_new_queue_does_not_serialize_its_jobs() {
    let daemon = Daemon::with_profile("empty-queue", "ordinary");
    daemon.job(&["queue", "create", "work"]);
    let first = text(&daemon.job(&[
        "submit",
        "--queue",
        "work",
        "--time",
        "5s",
        "--",
        "while [ ! -e second-started ]; do sleep 0.05; done",
    ]));
    let second = text(&daemon.job(&["submit", "--queue", "work", "--", "touch second-started"]));
    assert!(
        daemon
            .job(&["wait", second.trim(), "--timeout", "5s"])
            .status
            .success()
    );
    assert!(
        daemon
            .job(&["wait", first.trim(), "--timeout", "5s"])
            .status
            .success()
    );
    let (_, end) = started_and_finished(&daemon, first.trim());
    let (start, _) = started_and_finished(&daemon, second.trim());
    assert!(start < end);
}

#[test]
fn ordinary_jobs_have_no_synthetic_reservations_or_history_estimates() {
    let mut daemon = Daemon::with_profile("ordinary-reservations", "ordinary");
    daemon.job(&["queue", "create", "plain"]);
    for _ in 0..2 {
        let id = text(&daemon.job(&["submit", "--queue", "plain", "--", "true"]));
        assert!(
            daemon
                .job(&["wait", id.trim(), "--timeout", "5s"])
                .status
                .success()
        );
        let job: serde_json::Value =
            serde_json::from_slice(&daemon.job(&["status", id.trim(), "--json"]).stdout).unwrap();
        assert_eq!(job["policy"], "ordinary");
        assert_eq!(
            job["reservation"]["vector"],
            serde_json::json!({"cores_milli":0,"memory":0,"pids":0})
        );
        assert_eq!(job["reservation"]["predicted_ms"], serde_json::Value::Null);
        assert_eq!(job["reservation"]["cores_source"], "Unset");
        daemon.restart();
    }
    let config: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["config", "show", "--json"]).stdout).unwrap();
    assert_eq!(config["emergency_host_termination"], false);
    assert_eq!(config["host_and_filesystem_floors"], false);
    assert_eq!(config["implicit_swap_limit"], serde_json::Value::Null);
}

#[test]
fn ordinary_dispatch_does_not_reserve_future_backfill_slots() {
    let daemon = Daemon::with_profile("ordinary-dispatch", "ordinary");
    let cores = std::thread::available_parallelism()
        .unwrap()
        .get()
        .to_string();
    let first = text(&daemon.job(&[
        "submit",
        "--cores",
        &cores,
        "--time",
        "5s",
        "--",
        "while [ ! -e free-started ]; do sleep 0.05; done",
    ]));
    let second = text(&daemon.job(&["submit", "--cores", &cores, "--", "true"]));
    let free = text(&daemon.job(&["submit", "--", "touch free-started"]));
    assert!(
        daemon
            .job(&["wait", free.trim(), "--timeout", "5s"])
            .status
            .success()
    );
    assert!(
        daemon
            .job(&["wait", second.trim(), "--timeout", "5s"])
            .status
            .success()
    );
    let (_, first_end) = started_and_finished(&daemon, first.trim());
    let (free_start, _) = started_and_finished(&daemon, free.trim());
    let (second_start, _) = started_and_finished(&daemon, second.trim());
    assert!(free_start < first_end);
    assert!(second_start >= first_end);
}

#[test]
fn service_profile_changes_require_a_drained_restart() {
    let daemon = Daemon::with_profile("profile-reload", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "sleep 0.5"]));
    assert!(!daemon.job(&["config", "reload"]).status.success());
    assert!(
        daemon
            .job(&["wait", id.trim(), "--timeout", "5s"])
            .status
            .success()
    );
    std::fs::write(
        daemon.base.join("config.toml"),
        "schema_version = 1\nprofile = 'legacy'\n",
    )
    .unwrap();
    let refused = daemon.job(&["config", "reload"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("requires a daemon restart"));
}

#[test]
fn ordinary_ancestor_admission_budgets_cannot_be_bypassed_by_child_defaults() {
    let daemon = Daemon::with_profile("ordinary-budgets", "ordinary");
    daemon.job(&["group", "create", "work", "--cores", "1"]);
    daemon.job(&["queue", "create", "work/a", "--cores", "2"]);
    let refused = daemon.job(&["submit", "--queue", "work/a", "--cores", "2", "--", "true"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("budget 1000 at work"));
}

fn check_unrelated_process_is_not_adopted(name: &str, previous_boot: bool) {
    let mut daemon = Daemon::with_profile(name, "ordinary");
    let id = text(&daemon.job(&["submit", "--", "true"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    daemon.child.kill().unwrap();
    daemon.child.wait().unwrap();
    let mut unrelated = Command::new("sleep").arg("10").spawn().unwrap();
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", unrelated.id())).unwrap();
    let ticks: u64 = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap();
    let directory = daemon.state.join("jobs").join(&id);
    let path = directory.join("job.json");
    let mut job: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    job["state"] = serde_json::json!("Running");
    job["result"] = serde_json::Value::Null;
    job["finished_ms"] = serde_json::Value::Null;
    job["shim_pid"] = serde_json::json!(unrelated.id());
    job["shim_start_ticks"] = serde_json::json!(if previous_boot {
        ticks
    } else {
        ticks.wrapping_add(1)
    });
    if previous_boot {
        job["supervisor_boot_id"] = serde_json::json!("previous-boot");
    }
    std::fs::write(path, serde_json::to_vec(&job).unwrap()).unwrap();
    std::fs::remove_file(directory.join("result.json")).unwrap();
    std::fs::remove_file(directory.join("exit.json")).unwrap();
    daemon.restart();
    let answer = daemon.job(&["wait", &id, "--timeout", "3s"]);
    let untouched = unrelated.try_wait().unwrap().is_none();
    let _ = unrelated.kill();
    let _ = unrelated.wait();
    assert!(untouched, "recovery signalled an unrelated process");
    assert!(!answer.status.success());
    assert!(text(&answer).contains("lost:"), "{}", text(&answer));
}

#[test]
fn recovery_does_not_adopt_a_matching_pid_from_another_boot() {
    check_unrelated_process_is_not_adopted("other-boot", true);
}

#[test]
fn recovery_does_not_adopt_a_matching_pid_with_different_start_ticks() {
    check_unrelated_process_is_not_adopted("other-process", false);
}

#[test]
fn an_explicit_signal_reaches_the_workload_after_daemon_recovery() {
    let mut daemon = Daemon::with_profile("signal-recovery", "ordinary");
    let id = text(&daemon.job(&[
        "submit",
        "--time",
        "5s",
        "--",
        "trap 'echo delivered; exit 0' USR1; echo ready > ready; while :; do sleep 0.05; done",
    ]))
    .trim()
    .to_owned();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !daemon.file("ready").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(daemon.file("ready").exists());
    daemon.restart();
    let sent = daemon.job(&["signal", "-s", "USR1", &id]);
    assert!(
        sent.status.success(),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    assert!(sent.stdout.is_empty());
    let waited = daemon.job(&["wait", &id, "--timeout", "3s"]);
    assert!(waited.status.success(), "{}", text(&waited));
    assert!(text(&waited).contains("delivered"));
    assert!(!daemon.job(&["signal", &id]).status.success());
}

#[test]
fn a_held_job_can_be_edited_recovered_and_released_without_losing_its_submission() {
    let mut daemon = Daemon::with_profile("held-edit", "ordinary");
    let created = daemon.job(&["create", "--json", "--", "bash", "-c", "echo original"]);
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let created: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let id = created["id"].as_u64().unwrap().to_string();
    assert_eq!(created["state"], "Held");
    let status: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["status", &id, "--json"]).stdout).unwrap();
    assert_eq!(status["state"], "Held");
    assert!(status["started_ms"].is_null());
    assert!(status["effective_spec"].is_null());
    let edited = daemon
        .job_command(&["edit", &id, "--", "bash", "-c", "printf '%s' \"$EDITION\""])
        .env("EDITION", "edited-environment")
        .env("JOB_SESSION", "collaborator")
        .output()
        .unwrap();
    assert!(
        edited.status.success(),
        "{}",
        String::from_utf8_lossy(&edited.stderr)
    );
    assert_eq!(text(&edited).trim(), id);
    daemon.restart();
    let status: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["status", &id, "--json"]).stdout).unwrap();
    assert_eq!(status["state"], "Held");
    assert_eq!(status["submitted_spec"]["argv"][2], "echo original");
    assert_eq!(status["requested_spec"]["session"], "collaborator");
    assert!(daemon.job(&["release", &id]).status.success());
    let output = daemon.job(&["wait", &id, "--timeout", "5s"]);
    assert!(output.status.success());
    assert!(text(&output).contains("edited-environment"));
    let status: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["status", &id, "--json"]).stdout).unwrap();
    assert_eq!(status["state"], "Succeeded");
    assert_eq!(
        status["effective_spec"]["argv"],
        status["requested_spec"]["argv"]
    );
    assert!(!daemon.job(&["edit", &id, "--", "true"]).status.success());
}

#[test]
fn cancelling_held_work_records_no_execution_time() {
    let daemon = Daemon::with_profile("held-cancel", "ordinary");
    let id = text(&daemon.job(&["create", "--", "touch must-not-exist"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["cancel", &id]).status.success());
    let status: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["status", &id, "--json"]).stdout).unwrap();
    assert_eq!(status["state"], "Cancelled");
    assert!(status["started_ms"].is_null());
    assert!(!daemon.file("must-not-exist").exists());
    assert!(!daemon.state.join("history.jsonl").exists());
}

#[test]
fn an_invalid_edit_preserves_the_previously_accepted_job() {
    let daemon = Daemon::with_profile("held-invalid-edit", "ordinary");
    let id = text(&daemon.job(&["create", "--", "echo kept"]))
        .trim()
        .to_owned();
    let before = daemon.job(&["status", &id, "--json"]);
    let edit = daemon.job(&["edit", &id, "--queue", "missing", "--", "echo replaced"]);
    assert!(!edit.status.success());
    assert_eq!(before.stdout, daemon.job(&["status", &id, "--json"]).stdout);
    assert!(daemon.job(&["release", &id]).status.success());
    assert!(text(&daemon.job(&["wait", &id])).contains("kept"));
}

#[test]
fn a_failed_command_has_an_outcome_separate_from_its_exit_status() {
    let daemon = Daemon::with_profile("failed-state", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "exit 23"]))
        .trim()
        .to_owned();
    assert_eq!(daemon.job(&["wait", &id]).status.code(), Some(23));
    let status: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["status", &id, "--json"]).stdout).unwrap();
    assert_eq!(status["state"], "Failed");
    assert_eq!(status["result"]["exit_code"], 23);
}

#[test]
fn stopping_survives_restart_with_its_original_deadline() {
    let mut daemon = Daemon::with_profile("stopping-restart", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "trap '' TERM; echo ready > ready; sleep 20"]))
        .trim()
        .to_owned();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !daemon.file("ready").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(daemon.file("ready").exists());
    assert!(daemon.job(&["cancel", &id]).status.success());
    let before: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["status", &id, "--json"]).stdout).unwrap();
    assert_eq!(before["state"], "Stopping");
    daemon.restart();
    let after: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["status", &id, "--json"]).stdout).unwrap();
    assert_eq!(after["state"], "Stopping");
    assert_eq!(
        after["termination_deadline_ms"],
        before["termination_deadline_ms"]
    );
    assert!(daemon.job(&["signal", "-s", "KILL", &id]).status.success());
    assert!(
        !daemon
            .job(&["wait", &id, "--timeout", "3s"])
            .status
            .success()
    );
}

#[test]
fn a_failed_storage_edit_keeps_both_command_and_environment() {
    let daemon = Daemon::with_profile("edit-storage-failure", "ordinary");
    let id = text(&daemon.job(&["create", "--", "echo kept"]))
        .trim()
        .to_owned();
    let directory = daemon.state.join("jobs").join(&id);
    let command = std::fs::read(directory.join("job.json")).unwrap();
    let environment = std::fs::read(directory.join("env.json")).unwrap();
    std::fs::create_dir(directory.join("unexpected-directory")).unwrap();
    let output = daemon
        .job_command(&["edit", &id, "--", "echo changed"])
        .env("EDITION", "changed")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(std::fs::read(directory.join("job.json")).unwrap(), command);
    assert_eq!(
        std::fs::read(directory.join("env.json")).unwrap(),
        environment
    );
    std::fs::remove_dir(directory.join("unexpected-directory")).unwrap();
    assert!(daemon.job(&["release", &id]).status.success());
    assert!(text(&daemon.job(&["wait", &id])).contains("kept"));
}

#[test]
fn retries_keep_identity_original_submission_and_prior_logs_through_restart_and_edit() {
    let mut daemon = Daemon::with_profile("attempt-history", "ordinary");
    let created = daemon
        .job_command(&["submit", "--", "bash", "-c", "echo $EDITION; exit 23"])
        .env("EDITION", "original-environment")
        .output()
        .unwrap();
    let id = text(&created).trim().to_owned();
    assert_eq!(daemon.job(&["wait", &id]).status.code(), Some(23));
    let original = daemon.job(&["status", &id, "--json"]);
    let original: serde_json::Value = serde_json::from_slice(&original.stdout).unwrap();
    let retried = daemon
        .job_command(&["retry", &id, "--hold", "--json"])
        .env("EDITION", "not-selected")
        .output()
        .unwrap();
    assert!(
        retried.status.success(),
        "{}",
        String::from_utf8_lossy(&retried.stderr)
    );
    let retry: serde_json::Value = serde_json::from_slice(&retried.stdout).unwrap();
    assert_eq!(retry["id"], original["id"]);
    assert_eq!(retry["attempt"], 2);
    assert_eq!(retry["submitted_ms"], original["submitted_ms"]);
    assert!(retry["result"].is_null());
    assert!(retry["effective_spec"].is_null());
    daemon.restart();
    assert!(daemon.job(&["release", &id]).status.success());
    let second = daemon.job(&["wait", &id]);
    assert_eq!(second.status.code(), Some(23));
    assert!(text(&second).contains("original-environment"));
    let archive = daemon
        .state
        .join("jobs")
        .join(&id)
        .join("attempts/1/job.json");
    let archived_bytes = std::fs::read(&archive).unwrap();
    assert!(daemon.job(&["retry", &id, "--hold"]).status.success());
    assert!(
        daemon
            .job(&["edit", &id, "--", "echo third-attempt"])
            .status
            .success()
    );
    assert!(daemon.job(&["release", &id]).status.success());
    assert!(daemon.job(&["wait", &id]).status.success());
    let attempts = daemon.job(&["attempts", &id, "--json"]);
    let attempts: serde_json::Value = serde_json::from_slice(&attempts.stdout).unwrap();
    assert_eq!(attempts["schema_version"], 1);
    assert_eq!(attempts["attempts"].as_array().unwrap().len(), 3);
    assert_eq!(attempts["attempts"][0]["state"], "Failed");
    assert_eq!(attempts["attempts"][2]["state"], "Succeeded");
    assert_eq!(
        attempts["attempts"][2]["submitted_spec"],
        original["submitted_spec"]
    );
    assert_eq!(std::fs::read(archive).unwrap(), archived_bytes);
    for attempt in ["1", "2"] {
        let log = daemon.job(&["log", &id, "--attempt", attempt, "full"]);
        assert!(
            log.status.success(),
            "{}",
            String::from_utf8_lossy(&log.stderr)
        );
        assert!(text(&log).contains("original-environment"));
        assert!(!text(&log).contains("third-attempt"));
    }
    assert!(text(&daemon.job(&["log", &id, "full"])).contains("third-attempt"));
}

#[test]
fn retry_follows_renamed_queue_identity_and_requires_a_new_destination_after_removal() {
    let daemon = Daemon::with_profile("retry-queue", "ordinary");
    assert!(daemon.job(&["queue", "create", "before"]).status.success());
    let id = text(&daemon.job(&["submit", "--queue", "before", "--", "true"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    assert!(
        daemon
            .job(&["queue", "rename", "before", "after"])
            .status
            .success()
    );
    let retried = daemon.job(&["retry", &id, "--json"]);
    assert!(
        retried.status.success(),
        "{}",
        String::from_utf8_lossy(&retried.stderr)
    );
    let retried: serde_json::Value = serde_json::from_slice(&retried.stdout).unwrap();
    assert!(
        retried["spec"]["queue"]
            .as_str()
            .unwrap()
            .ends_with("after")
    );
    assert!(daemon.job(&["wait", &id]).status.success());
    assert!(daemon.job(&["queue", "rm", "after"]).status.success());
    assert!(!daemon.job(&["retry", &id]).status.success());
    assert!(
        daemon
            .job(&["retry", &id, "--queue", "default"])
            .status
            .success()
    );
    assert!(daemon.job(&["wait", &id]).status.success());
}

#[test]
fn missing_saved_environment_requires_explicit_replacement_for_retry() {
    let daemon = Daemon::with_profile("retry-env", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "echo $EDITION"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    std::fs::remove_file(daemon.state.join("jobs").join(&id).join("env.json")).unwrap();
    assert!(!daemon.job(&["retry", &id]).status.success());
    let retried = daemon
        .job_command(&["retry", &id, "--current-env"])
        .env("EDITION", "replacement-environment")
        .output()
        .unwrap();
    assert!(
        retried.status.success(),
        "{}",
        String::from_utf8_lossy(&retried.stderr)
    );
    assert!(text(&daemon.job(&["wait", &id])).contains("replacement-environment"));
}

#[test]
fn retry_enters_behind_already_waiting_work_instead_of_using_its_old_id() {
    let daemon = Daemon::with_profile("retry-fifo", "ordinary");
    assert!(
        daemon
            .job(&["queue", "create", "serial", "--parallel", "1"])
            .status
            .success()
    );
    let id = text(&daemon.job(&["submit", "-q", "serial", "--", "echo old >> order"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    assert!(daemon.job(&["queue", "pause", "serial"]).status.success());
    let other = text(&daemon.job(&["submit", "-q", "serial", "--", "echo waiting >> order"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["retry", &id]).status.success());
    assert!(daemon.job(&["queue", "resume", "serial"]).status.success());
    assert!(daemon.job(&["wait", &id]).status.success());
    assert!(daemon.job(&["wait", &other]).status.success());
    assert_eq!(
        std::fs::read_to_string(daemon.file("order")).unwrap(),
        "old\nwaiting\nold\n"
    );
}

#[test]
fn failed_retry_publication_preserves_the_terminal_attempt() {
    let daemon = Daemon::with_profile("retry-storage-failure", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "echo preserved"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    let directory = daemon.state.join("jobs").join(&id);
    let before = std::fs::read(directory.join("job.json")).unwrap();
    std::fs::create_dir(directory.join("unexpected-directory")).unwrap();
    assert!(!daemon.job(&["retry", &id]).status.success());
    assert_eq!(std::fs::read(directory.join("job.json")).unwrap(), before);
    assert!(!directory.join("attempts").exists());
    assert!(text(&daemon.job(&["log", &id, "full"])).contains("preserved"));
}

#[test]
fn a_replayed_retry_request_cannot_start_an_extra_attempt() {
    let daemon = Daemon::with_profile("retry-replay", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "true"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    assert!(daemon.job(&["retry", &id]).status.success());
    assert!(daemon.job(&["wait", &id]).status.success());
    let mut stream = UnixStream::connect(daemon.state.join("daemon.sock")).unwrap();
    let request = serde_json::json!({"Versioned": {"protocol": 20, "request": {"Retry": {"id": id.parse::<u64>().unwrap(), "expected_attempt": 1, "held": false, "env": null, "queue": null, "allow_lost": false}}}});
    writeln!(stream, "{request}").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert!(response.get("Error").is_some());
    let attempts: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["attempts", &id, "--json"]).stdout).unwrap();
    assert_eq!(attempts["attempts"].as_array().unwrap().len(), 2);
}

#[test]
fn retry_waits_for_the_previous_supervisor_without_publishing_an_archive() {
    let daemon = Daemon::with_profile("retry-live-supervisor", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "true"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    let directory = daemon.state.join("jobs").join(&id);
    let path = directory.join("job.json");
    let mut job: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let pid = std::process::id();
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let ticks: u64 = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap();
    job["shim_pid"] = serde_json::json!(pid);
    job["shim_start_ticks"] = serde_json::json!(ticks);
    let original = serde_json::to_vec(&job).unwrap();
    std::fs::write(&path, &original).unwrap();
    let mut stream = UnixStream::connect(daemon.state.join("daemon.sock")).unwrap();
    let request = serde_json::json!({"Versioned": {"protocol": 20, "request": {"Retry": {"id": id.parse::<u64>().unwrap(), "expected_attempt": 1, "held": false, "env": null, "queue": null, "allow_lost": false}}}});
    writeln!(stream, "{request}").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response).unwrap(),
        "RetryPending"
    );
    assert_eq!(std::fs::read(path).unwrap(), original);
    assert!(!directory.join("attempts").exists());
}

#[test]
fn lost_attempts_require_explicit_acknowledgement_before_retry() {
    let daemon = Daemon::with_profile("retry-lost", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "true"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    let path = daemon.state.join("jobs").join(&id).join("job.json");
    let mut job: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    job["state"] = serde_json::json!("Lost");
    job["supervisor_boot_id"] = serde_json::json!("past-boot");
    std::fs::write(path, serde_json::to_vec(&job).unwrap()).unwrap();
    assert!(!daemon.job(&["retry", &id]).status.success());
    let retry = daemon.job(&["retry", &id, "--allow-lost"]);
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert!(daemon.job(&["wait", &id]).status.success());
}

#[test]
fn retry_is_new_admission_and_cannot_bypass_a_closed_queue() {
    let daemon = Daemon::with_profile("retry-closed", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "true"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    assert!(daemon.job(&["queue", "close", "default"]).status.success());
    assert!(!daemon.job(&["retry", &id, "--hold"]).status.success());
    let attempts: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["attempts", &id, "--json"]).stdout).unwrap();
    assert_eq!(attempts["attempts"].as_array().unwrap().len(), 1);
}

#[test]
fn watch_backend_refuses_suspension_without_changing_the_running_job() {
    let daemon = Daemon::with_profile("watch-suspend", "ordinary");
    let id = text(&daemon.job(&["submit", "--time", "5s", "--", "sleep 4"]))
        .trim()
        .to_owned();
    let info: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["host", "--json"]).stdout).unwrap();
    assert_eq!(info["freezer"], false);
    let output = daemon.job(&["suspend", &id, "--json"]);
    assert!(!output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["results"][0]["accepted"], false);
    let job: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["status", &id, "--json"]).stdout).unwrap();
    assert_eq!(job["state"], "Running");
    assert_eq!(job["suspension"]["requested"], false);
    daemon.job(&["cancel", &id]);
    daemon.job(&["wait", &id, "--timeout", "3s"]);
}

fn request(daemon: &Daemon, value: serde_json::Value) -> serde_json::Value {
    let value = if value.is_string() || value.get("Versioned").is_some() {
        value
    } else {
        serde_json::json!({"Versioned": {"protocol": 20, "request": value}})
    };
    let mut stream = UnixStream::connect(daemon.state.join("daemon.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    writeln!(stream, "{value}").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    serde_json::from_str(&response).unwrap()
}

fn cancellation_preview(daemon: &Daemon, path: &str) -> serde_json::Value {
    let output = daemon.job(&[
        "group",
        "cancel",
        "--recursive",
        path,
        "--dry-run",
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["selection"].clone()
}

fn cancellation_commit(
    daemon: &Daemon,
    path: &str,
    selection: &serde_json::Value,
) -> serde_json::Value {
    request(
        daemon,
        serde_json::json!({"CancelSelection": {"target": {"Object": {"kind": "group", "path": path}}, "recursive": true, "expected": selection}}),
    )
}

#[test]
fn recursive_cancellation_captures_mixed_work_without_starting_selected_waiters() {
    let daemon = Daemon::with_profile("cancel-tree", "ordinary");
    for args in [
        vec!["group", "create", "tree"],
        vec!["group", "create", "tree/nested"],
        vec!["queue", "create", "tree/direct", "--max-running", "1"],
        vec!["queue", "create", "tree/nested/leaf"],
    ] {
        assert!(daemon.job(&args).status.success());
    }
    let running = text(&daemon.job(&["submit", "-q", "tree/direct", "--", "sleep 15"]))
        .trim()
        .to_owned();
    let queued = text(&daemon.job(&["submit", "-q", "tree/direct", "--", "touch must-not-start"]))
        .trim()
        .to_owned();
    let held = text(&daemon.job(&[
        "create",
        "-q",
        "tree/nested/leaf",
        "--",
        "touch held-must-not-start",
    ]))
    .trim()
    .to_owned();
    let unrelated = text(&daemon.job(&["create", "--", "true"]))
        .trim()
        .to_owned();
    assert!(!daemon.job(&["group", "cancel", "tree"]).status.success());
    let preview = cancellation_preview(&daemon, "tree");
    assert_eq!(preview["members"].as_array().unwrap().len(), 3);
    assert!(!daemon.state.join("cancellations").exists());
    let later = text(&daemon.job(&["create", "-q", "tree/direct", "--", "true"]))
        .trim()
        .to_owned();
    let response = cancellation_commit(&daemon, "tree", &preview);
    assert_eq!(
        response["Cancellation"]["operation"]["complete"], true,
        "{response}"
    );
    assert_eq!(
        response["Cancellation"]["operation"]["results"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    for id in [&running, &queued, &held] {
        let result = daemon.job(&["wait", id, "--timeout", "5s", "--json"]);
        let job: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(job["state"], "Cancelled", "{job}");
    }
    for id in [&unrelated, &later] {
        let output = daemon.job(&["status", id, "--json"]);
        let job: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(job["state"], "Held");
        assert!(daemon.job(&["cancel", id]).status.success());
    }
    assert!(!daemon.file("must-not-start").exists());
    assert!(!daemon.file("held-must-not-start").exists());
    let output = daemon.job(&["group", "show", "tree", "--json"]);
    assert!(output.status.success());
    assert_eq!(cancellation_commit(&daemon, "tree", &preview), response);
}

#[test]
fn cancellation_rejects_a_stale_attempt_before_commit() {
    let daemon = Daemon::with_profile("cancel-stale", "ordinary");
    assert!(daemon.job(&["group", "create", "tree"]).status.success());
    assert!(daemon.job(&["queue", "create", "tree/q"]).status.success());
    let id = text(&daemon.job(&["create", "-q", "tree/q", "--", "true"]))
        .trim()
        .to_owned();
    let preview = cancellation_preview(&daemon, "tree");
    assert!(daemon.job(&["cancel", &id]).status.success());
    assert!(daemon.job(&["retry", &id, "--hold"]).status.success());
    let response = cancellation_commit(&daemon, "tree", &preview);
    assert!(response.get("Error").is_some(), "{response}");
    let output = daemon.job(&["status", &id, "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(job["state"], "Held");
    assert_eq!(job["attempt"], 2);
    assert!(daemon.job(&["cancel", &id]).status.success());
}

#[test]
fn cancellation_rejects_a_moved_scope_before_commit() {
    let daemon = Daemon::with_profile("cancel-moved", "ordinary");
    assert!(daemon.job(&["group", "create", "tree"]).status.success());
    assert!(daemon.job(&["queue", "create", "tree/q"]).status.success());
    let id = text(&daemon.job(&["create", "-q", "tree/q", "--", "true"]))
        .trim()
        .to_owned();
    let preview = cancellation_preview(&daemon, "tree");
    assert!(
        daemon
            .job(&["queue", "move", "tree/q", "--group", "/"])
            .status
            .success()
    );
    assert!(
        cancellation_commit(&daemon, "tree", &preview)
            .get("Error")
            .is_some()
    );
    assert!(daemon.job(&["cancel", &id]).status.success());
}

#[test]
fn natural_completion_after_preview_is_a_successful_cancellation_noop() {
    let daemon = Daemon::with_profile("cancel-completed", "ordinary");
    assert!(daemon.job(&["group", "create", "tree"]).status.success());
    assert!(daemon.job(&["queue", "create", "tree/q"]).status.success());
    let id = text(&daemon.job(&["create", "-q", "tree/q", "--", "true"]))
        .trim()
        .to_owned();
    let preview = cancellation_preview(&daemon, "tree");
    assert!(daemon.job(&["release", &id]).status.success());
    assert!(daemon.job(&["wait", &id]).status.success());
    let response = cancellation_commit(&daemon, "tree", &preview);
    assert_eq!(response["Cancellation"]["operation"]["complete"], true);
    assert_eq!(
        response["Cancellation"]["operation"]["results"][0]["state"],
        "Succeeded"
    );
}

#[test]
fn pending_cancellation_blocks_dispatch_and_replays_after_restart() {
    let mut daemon = Daemon::with_profile("cancel-restart", "ordinary");
    assert!(daemon.job(&["group", "create", "tree"]).status.success());
    assert!(daemon.job(&["queue", "create", "tree/q"]).status.success());
    assert!(daemon.job(&["queue", "pause", "tree/q"]).status.success());
    let first = text(&daemon.job(&["submit", "-q", "tree/q", "--", "touch forbidden-start"]))
        .trim()
        .to_owned();
    let second = text(&daemon.job(&["create", "-q", "tree/q", "--", "true"]))
        .trim()
        .to_owned();
    let fault = daemon
        .state
        .join("jobs")
        .join(&first)
        .join(format!("job.tmp{}", daemon.child.id()));
    std::fs::create_dir(&fault).unwrap();
    let output = daemon.job(&["group", "cancel", "--recursive", "tree", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(75),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let operation: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(operation["complete"], false);
    assert_eq!(operation["results"][0]["pending"], true);
    assert_eq!(operation["results"][1]["state"], "Cancelled");
    assert!(daemon.job(&["queue", "resume", "tree/q"]).status.success());
    let output = daemon.job(&["status", &first, "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(job["state"], "Queued");
    daemon.child.kill().unwrap();
    daemon.child.wait().unwrap();
    std::fs::remove_dir(&fault).unwrap();
    daemon.restart();
    let output = daemon.job(&["wait", &first, "--timeout", "3s", "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(job["state"], "Cancelled");
    assert!(job["started_ms"].is_null());
    assert!(!daemon.file("forbidden-start").exists());
    let receipt = daemon
        .state
        .join("cancellations")
        .join(format!("{}.json", operation["operation"].as_str().unwrap()));
    let recorded: serde_json::Value =
        serde_json::from_slice(&std::fs::read(receipt).unwrap()).unwrap();
    assert_eq!(recorded["complete"], true);
    assert!(daemon.state.join("jobs").join(second).exists());
}

#[test]
fn failed_cancellation_publication_leaves_all_selected_jobs_untouched() {
    let daemon = Daemon::with_profile("cancel-intent", "ordinary");
    assert!(daemon.job(&["group", "create", "tree"]).status.success());
    assert!(daemon.job(&["queue", "create", "tree/q"]).status.success());
    let id = text(&daemon.job(&["create", "-q", "tree/q", "--", "true"]))
        .trim()
        .to_owned();
    let preview = cancellation_preview(&daemon, "tree");
    std::fs::write(daemon.state.join("cancellations"), "blocked").unwrap();
    let response = cancellation_commit(&daemon, "tree", &preview);
    assert!(response.get("Error").is_some());
    let output = daemon.job(&["status", &id, "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(job["state"], "Held");
    assert!(job["stop"].is_null());
    std::fs::remove_file(daemon.state.join("cancellations")).unwrap();
    assert!(daemon.job(&["cancel", &id]).status.success());
}

#[test]
fn cancellation_never_signals_before_stopping_state_is_saved() {
    let daemon = Daemon::with_profile("cancel-save-running", "ordinary");
    let id = text(&daemon.job(&[
        "submit",
        "--time",
        "15s",
        "--",
        "trap 'touch terminated; exit 0' TERM; touch ready; sleep 10",
    ]))
    .trim()
    .to_owned();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !daemon.file("ready").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let fault = daemon
        .state
        .join("jobs")
        .join(&id)
        .join(format!("job.tmp{}", daemon.child.id()));
    std::fs::create_dir(&fault).unwrap();
    let output = daemon.job(&["cancel", &id, "--json"]);
    assert_eq!(output.status.code(), Some(75), "{}", text(&output));
    let operation: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(operation["results"][0]["pending"], true);
    std::thread::sleep(Duration::from_millis(300));
    let output = daemon.job(&["status", &id, "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(job["state"], "Running");
    assert!(job["stop"].is_null());
    assert!(!daemon.file("terminated").exists());
    std::fs::remove_dir(fault).unwrap();
    daemon.job(&["wait", &id, "--timeout", "3s"]);
    assert!(daemon.file("terminated").exists());
}

#[test]
fn replaying_cancellation_never_extends_the_termination_deadline() {
    let mut daemon = Daemon::with_profile("cancel-deadline", "ordinary");
    assert!(daemon.job(&["group", "create", "tree"]).status.success());
    assert!(daemon.job(&["queue", "create", "tree/q"]).status.success());
    let id = text(&daemon.job(&[
        "submit",
        "-q",
        "tree/q",
        "--",
        "trap '' TERM; touch ready; sleep 3",
    ]))
    .trim()
    .to_owned();
    let limit = Instant::now() + Duration::from_secs(3);
    while !daemon.file("ready").exists() {
        assert!(Instant::now() < limit);
        std::thread::sleep(Duration::from_millis(20));
    }
    let preview = cancellation_preview(&daemon, "tree");
    let response = cancellation_commit(&daemon, "tree", &preview);
    assert_eq!(response["Cancellation"]["operation"]["complete"], true);
    let output = daemon.job(&["status", &id, "--json"]);
    let first: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(first["state"], "Stopping");
    assert!(first["termination_deadline_ms"].as_u64().is_some());
    daemon.restart();
    assert_eq!(cancellation_commit(&daemon, "tree", &preview), response);
    let output = daemon.job(&["status", &id, "--json"]);
    let second: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        first["termination_deadline_ms"],
        second["termination_deadline_ms"]
    );
    daemon.job(&["wait", &id, "--timeout", "5s"]);
}

#[test]
fn recovering_old_cancellation_does_not_cancel_a_replacement_attempt() {
    let mut daemon = Daemon::with_profile("cancel-old-attempt", "ordinary");
    assert!(daemon.job(&["group", "create", "tree"]).status.success());
    assert!(daemon.job(&["queue", "create", "tree/q"]).status.success());
    let id = text(&daemon.job(&["create", "-q", "tree/q", "--", "true"]))
        .trim()
        .to_owned();
    let preview = cancellation_preview(&daemon, "tree");
    let response = cancellation_commit(&daemon, "tree", &preview);
    let mut operation = response["Cancellation"]["operation"].clone();
    assert_eq!(operation["complete"], true);
    assert!(daemon.job(&["retry", &id, "--hold"]).status.success());
    daemon.child.kill().unwrap();
    daemon.child.wait().unwrap();
    operation["complete"] = serde_json::json!(false);
    operation["results"] = serde_json::json!([]);
    let path = daemon
        .state
        .join("cancellations")
        .join(format!("{}.json", operation["operation"].as_str().unwrap()));
    std::fs::write(&path, serde_json::to_vec(&operation).unwrap()).unwrap();
    daemon.restart();
    let output = daemon.job(&["status", &id, "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(job["state"], "Held");
    assert_eq!(job["attempt"], 2);
    let recorded: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(recorded["complete"], true);
    assert!(
        recorded["results"][0]["error"]
            .as_str()
            .unwrap()
            .contains("no longer current")
    );
    assert!(daemon.job(&["cancel", &id]).status.success());
}

#[test]
fn partial_cancellation_does_not_repeat_signals_for_already_applied_members() {
    let daemon = Daemon::with_profile("cancel-partial-signal", "ordinary");
    assert!(daemon.job(&["group", "create", "tree"]).status.success());
    assert!(daemon.job(&["queue", "create", "tree/q"]).status.success());
    let running = text(&daemon.job(&[
        "submit",
        "-q",
        "tree/q",
        "--",
        "trap 'echo signal >> signals' TERM; touch ready; for i in {1..150}; do sleep .02; done",
    ]))
    .trim()
    .to_owned();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !daemon.file("ready").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let held = text(&daemon.job(&["create", "-q", "tree/q", "--", "true"]))
        .trim()
        .to_owned();
    let fault = daemon
        .state
        .join("jobs")
        .join(&held)
        .join(format!("job.tmp{}", daemon.child.id()));
    std::fs::create_dir(&fault).unwrap();
    let output = daemon.job(&["group", "cancel", "tree", "--recursive", "--json"]);
    assert_eq!(output.status.code(), Some(75));
    let result = daemon.job(&["wait", &running, "--timeout", "5s"]);
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(
        std::fs::read_to_string(daemon.file("signals"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    std::fs::remove_dir(fault).unwrap();
    let result = daemon.job(&["wait", &held, "--timeout", "3s", "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(job["state"], "Cancelled");
}

#[test]
fn independent_requests_do_not_install_limits_or_weights() {
    let daemon = Daemon::with_profile("request-only", "ordinary");
    let id = text(&daemon.job(&[
        "submit",
        "--cpu-request",
        "0.25",
        "--memory-request",
        "1M",
        "--",
        "true",
    ]))
    .trim()
    .to_owned();
    let output = daemon.job(&["wait", &id, "--json"]);
    assert!(output.status.success(), "{}", text(&output));
    let job: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(job["reservation"]["vector"]["cores_milli"], 250);
    assert_eq!(job["reservation"]["vector"]["memory"], 1 << 20);
    for field in [
        "cpu_limit_milli",
        "cpu_weight",
        "memory_high",
        "memory_max",
        "memory_swap_max",
    ] {
        assert!(
            job["effective_spec"]["declared"][field].is_null(),
            "{field}: {job}"
        );
    }
    assert_eq!(job["applied_resources"], serde_json::json!({}));
    assert_eq!(job["resource_sources"]["cpu_request_milli"], "Job");
}

#[test]
fn resource_defaults_are_per_field_and_existing_attempts_keep_their_resolution() {
    let daemon = Daemon::with_profile("resource-defaults", "ordinary");
    assert!(
        daemon
            .job(&[
                "group",
                "create",
                "tree",
                "--job-cpu-request",
                "0.25",
                "--job-memory-request",
                "2M"
            ])
            .status
            .success()
    );
    assert!(
        daemon
            .job(&["queue", "create", "tree/q", "--job-memory-request", "3M"])
            .status
            .success()
    );
    let id = text(&daemon.job(&["create", "-q", "tree/q", "--", "true"]))
        .trim()
        .to_owned();
    let output = daemon.job(&["status", &id, "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(job["spec"]["declared"]["cpu_request_milli"], 250);
    assert_eq!(job["spec"]["declared"]["memory_request"], 3 << 20);
    assert_eq!(
        job["resource_sources"]["cpu_request_milli"]["Object"]["path"],
        "tree"
    );
    assert_eq!(
        job["resource_sources"]["memory_request"]["Object"]["path"],
        "tree/q"
    );
    assert!(job["submitted_spec"]["declared"]["memory_request"].is_null());
    assert!(
        daemon
            .job(&["group", "set", "tree", "--job-cpu-request", "0.5"])
            .status
            .success()
    );
    assert!(
        daemon
            .job(&["queue", "unset", "tree/q", "job-memory-request"])
            .status
            .success()
    );
    let output = daemon.job(&["status", &id, "--json"]);
    let unchanged: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(unchanged["spec"], job["spec"]);
    assert!(daemon.job(&["release", &id]).status.success());
    assert!(daemon.job(&["wait", &id]).status.success());
    assert!(daemon.job(&["retry", &id, "--hold"]).status.success());
    let output = daemon.job(&["status", &id, "--json"]);
    let retry: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(retry["spec"]["declared"]["cpu_request_milli"], 500);
    assert_eq!(retry["spec"]["declared"]["memory_request"], 2 << 20);
    assert_eq!(retry["submitted_spec"], job["submitted_spec"]);
    assert!(daemon.job(&["cancel", &id]).status.success());
}

#[test]
fn explicit_kernel_controls_are_refused_without_a_delegated_backend() {
    let daemon = Daemon::with_profile("resource-unavailable", "ordinary");
    for (flag, value, file) in [
        ("--cpu-limit", "0.5", "cpu.max"),
        ("--cpu-weight", "100", "cpu.weight"),
        ("--memory-high", "16M", "memory.high"),
        ("--memory-max", "32M", "memory.max"),
        ("--memory-swap-max", "0", "memory.swap.max"),
    ] {
        let output = daemon.job(&["create", flag, value, "--", "true"]);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(file),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(
        !daemon
            .job(&[
                "create",
                "--mem",
                "32M",
                "--memory-max",
                "32M",
                "--",
                "true"
            ])
            .status
            .success()
    );
    assert!(
        std::fs::read_dir(daemon.state.join("jobs"))
            .unwrap()
            .next()
            .is_none()
    );
    let output = daemon.job(&["host", "--json"]);
    let host: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(host["resource_controls"]["cpu.max"], false);
}

#[test]
fn conflicting_legacy_options_and_impossible_requests_fail_before_creation() {
    let daemon = Daemon::with_profile("resource-validation", "ordinary");
    for args in [
        vec!["create", "--cores", "2", "--cpu-request", "1", "--", "true"],
        vec![
            "create",
            "--mem",
            "4M",
            "--memory-request",
            "2M",
            "--",
            "true",
        ],
        vec!["create", "--cpu-request", "1000000", "--", "true"],
        vec!["create", "--cpu-request", "NaN", "--", "true"],
        vec!["create", "--cpu-limit", "0.001", "--", "true"],
        vec!["group", "create", "invalid", "--job-cpu-weight", "10001"],
    ] {
        assert!(!daemon.job(&args).status.success(), "{args:?}");
    }
    assert!(
        std::fs::read_dir(daemon.state.join("jobs"))
            .unwrap()
            .next()
            .is_none()
    );
    let response = request(
        &daemon,
        serde_json::json!({"Object": {"kind": "group", "operation": {"Create": {"path": "invalid", "config": {"job_cpu_limit_milli": null}}}}}),
    );
    assert!(response.get("Error").is_some(), "{response}");
    assert!(!daemon.job(&["group", "show", "invalid"]).status.success());
}

#[test]
fn incompatible_local_resource_protocol_refuses_mutation() {
    let daemon = Daemon::with_profile("resource-protocol", "ordinary");
    let create = serde_json::json!({"Object": {"kind": "group", "operation": {"Create": {"path": "refused", "config": {}}}}});
    let response = request(
        &daemon,
        serde_json::json!({"Versioned": {"protocol": 6, "request": create}}),
    );
    assert_eq!(response["Unsupported"]["min"], 20);
    assert_eq!(response["Unsupported"]["max"], 20);
    assert!(response["Unsupported"]["version"].is_string());
    assert!(!daemon.job(&["group", "show", "refused"]).status.success());
}

#[test]
fn removal_purges_attempt_data_and_attributed_derivatives_but_preserves_work_files() {
    let mut daemon = Daemon::with_profile("remove-attempts", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "echo original; touch product"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    assert!(daemon.job(&["retry", &id, "--hold"]).status.success());
    assert!(
        daemon
            .job(&["edit", &id, "--", "echo second"])
            .status
            .success()
    );
    assert!(daemon.job(&["release", &id]).status.success());
    assert!(daemon.job(&["wait", &id]).status.success());
    let other = text(&daemon.job(&["submit", "--", "echo kept"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &other]).status.success());
    let output = daemon.job(&["remove", &id, "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["selection"]["jobs"][0]["attempt"], 2);
    assert!(!daemon.state.join("jobs").join(&id).exists());
    assert!(daemon.file("product").exists());
    for directory in std::fs::read_dir(daemon.state.join("templates")).unwrap() {
        assert!(!directory.unwrap().path().join(format!("{id}.txt")).exists());
    }
    let history = std::fs::read_to_string(daemon.state.join("history.jsonl")).unwrap();
    let history: Vec<serde_json::Value> = history
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        history
            .iter()
            .all(|entry| entry["job_id"].as_u64() != Some(id.parse().unwrap()))
    );
    assert!(
        history
            .iter()
            .any(|entry| entry["job_id"].as_u64() == Some(other.parse().unwrap()))
    );
    daemon.restart();
    assert!(!daemon.job(&["status", &id]).status.success());
    assert!(text(&daemon.job(&["log", &other, "full"])).contains("kept"));
    let next: u64 = text(&daemon.job(&["create", "--", "true"]))
        .trim()
        .parse()
        .unwrap();
    assert!(next > other.parse::<u64>().unwrap());
}

#[test]
fn removal_preview_is_read_only_and_live_states_are_never_deleted() {
    let daemon = Daemon::with_profile("remove-preview", "ordinary");
    let held = text(&daemon.job(&["create", "--", "touch forbidden"]))
        .trim()
        .to_owned();
    let directory = daemon.state.join("jobs").join(&held);
    let before = std::fs::read(directory.join("job.json")).unwrap();
    let preview = daemon.job(&["remove", &held, "--dry-run", "--json"]);
    assert!(preview.status.success());
    let preview: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(preview["ready"], false);
    assert!(!daemon.job(&["remove", &held]).status.success());
    assert_eq!(std::fs::read(directory.join("job.json")).unwrap(), before);
    assert!(!daemon.state.join("removals").exists());
    let running = text(&daemon.job(&["submit", "--time", "5s", "--", "sleep 4"]))
        .trim()
        .to_owned();
    assert!(!daemon.job(&["remove", &running]).status.success());
    let status: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["status", &running, "--json"]).stdout).unwrap();
    assert_eq!(status["state"], "Running");
    assert!(status["stop"].is_null());
    daemon.job(&["cancel", &running]);
    daemon.job(&["wait", &running, "--timeout", "3s"]);
    daemon.job(&["cancel", &held]);
    assert!(daemon.job(&["remove", &held]).status.success());
}

#[test]
fn recursive_removal_requires_completed_work_and_keeps_unselected_queues() {
    let daemon = Daemon::with_profile("remove-tree", "ordinary");
    for args in [
        vec!["group", "create", "tree"],
        vec!["group", "create", "tree/nested"],
        vec!["queue", "create", "tree/direct"],
        vec!["queue", "create", "tree/nested/leaf"],
        vec!["queue", "create", "kept"],
    ] {
        assert!(daemon.job(&args).status.success());
    }
    let id = text(&daemon.job(&["submit", "-q", "tree/direct", "--", "true"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    let held = text(&daemon.job(&["create", "-q", "tree/nested/leaf", "--", "true"]))
        .trim()
        .to_owned();
    assert!(!daemon.job(&["group", "remove", "tree"]).status.success());
    assert!(
        !daemon
            .job(&["group", "remove", "tree", "--recursive"])
            .status
            .success()
    );
    assert!(daemon.state.join("jobs").join(&id).exists());
    assert!(daemon.job(&["cancel", &held]).status.success());
    let preview = daemon.job(&[
        "group",
        "remove",
        "tree",
        "--recursive",
        "--dry-run",
        "--json",
    ]);
    let preview: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(preview["selection"]["jobs"].as_array().unwrap().len(), 2);
    assert_eq!(preview["selection"]["objects"].as_array().unwrap().len(), 4);
    let result = daemon.job(&["group", "remove", "tree", "--recursive"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!daemon.state.join("jobs").join(&held).exists());
    assert!(!daemon.job(&["group", "show", "tree"]).status.success());
    assert!(daemon.job(&["queue", "show", "kept"]).status.success());
    assert!(
        !daemon
            .job(&["queue", "remove", "default", "--recursive"])
            .status
            .success()
    );
    assert!(
        !daemon
            .job(&["group", "remove", "/", "--recursive"])
            .status
            .success()
    );
}

#[test]
fn stale_removal_selection_cannot_delete_a_new_attempt_and_completed_requests_are_idempotent() {
    let daemon = Daemon::with_profile("remove-selection", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "true"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    let target = serde_json::json!({"Job": {"id": id.parse::<u64>().unwrap()}});
    let preview = request(
        &daemon,
        serde_json::json!({"Remove": {"target": target, "recursive": false, "allow_lost": false, "expected": null}}),
    );
    let selection = preview["RemovalPreview"]["preview"]["selection"].clone();
    assert!(daemon.job(&["retry", &id]).status.success());
    assert!(daemon.job(&["wait", &id]).status.success());
    let old = request(
        &daemon,
        serde_json::json!({"Remove": {"target": target, "recursive": false, "allow_lost": false, "expected": selection}}),
    );
    assert!(old.get("Error").is_some());
    let output = daemon.job(&["remove", &id, "--json"]);
    assert!(output.status.success());
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let repeated = request(
        &daemon,
        serde_json::json!({"Remove": {"target": target, "recursive": false, "allow_lost": false, "expected": receipt["selection"]}}),
    );
    assert_eq!(repeated["Removed"]["receipt"], receipt);
}

#[test]
fn interrupted_removal_is_replayed_on_restart_before_accepting_new_work() {
    let mut daemon = Daemon::with_profile("remove-recovery", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "echo retired"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    let blocked = daemon
        .state
        .join(format!("history.tmp{}", daemon.child.id()));
    std::fs::create_dir(&blocked).unwrap();
    let output = daemon.job(&["remove", &id, "--json"]);
    assert_eq!(
        output.status.code(),
        Some(75),
        "{} {}",
        text(&output),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!daemon.state.join("jobs").join(&id).exists());
    assert!(daemon.state.join("removals/pending.json").exists());
    assert!(!daemon.job(&["create", "--", "true"]).status.success());
    let _ = daemon.child.kill();
    let _ = daemon.child.wait();
    std::fs::remove_dir(blocked).unwrap();
    let backup = daemon.base.join("blocked-backup");
    let maintenance = daemon.job(&[
        "state",
        "backup",
        "--source",
        daemon.state.to_str().unwrap(),
        "--destination",
        backup.to_str().unwrap(),
    ]);
    assert!(!maintenance.status.success());
    assert!(!backup.exists());
    daemon.restart();
    assert!(!daemon.state.join("removals/pending.json").exists());
    assert!(
        daemon
            .state
            .join(format!("removals/job-{id}.json"))
            .exists()
    );
    assert!(
        std::fs::read_to_string(daemon.state.join("history.jsonl"))
            .unwrap()
            .trim()
            .is_empty()
    );
    assert!(daemon.job(&["create", "--", "true"]).status.success());
}

#[test]
fn a_failed_removal_intent_write_leaves_the_record_and_environment_intact() {
    let daemon = Daemon::with_profile("remove-intent", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "echo kept"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    let directory = daemon.state.join("jobs").join(&id);
    let before = std::fs::read(directory.join("job.json")).unwrap();
    let env = std::fs::read(directory.join("env.json")).unwrap();
    std::fs::create_dir(daemon.state.join("removals")).unwrap();
    std::fs::create_dir(
        daemon
            .state
            .join(format!("removals/pending.tmp{}", daemon.child.id())),
    )
    .unwrap();
    assert!(!daemon.job(&["remove", &id]).status.success());
    assert_eq!(std::fs::read(directory.join("job.json")).unwrap(), before);
    assert_eq!(std::fs::read(directory.join("env.json")).unwrap(), env);
    assert!(!daemon.state.join("removals/pending.json").exists());
}

#[test]
fn removing_a_queue_preserves_its_retired_identity_for_attempts_now_in_another_queue() {
    let mut daemon = Daemon::with_profile("remove-reference", "ordinary");
    assert!(daemon.job(&["queue", "create", "old"]).status.success());
    let id = text(&daemon.job(&["submit", "-q", "old", "--", "echo old-output"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    assert!(
        daemon
            .job(&["retry", &id, "--queue", "default"])
            .status
            .success()
    );
    assert!(daemon.job(&["wait", &id]).status.success());
    let result = daemon.job(&["queue", "remove", "old"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    daemon.restart();
    assert!(text(&daemon.job(&["log", &id, "--attempt", "1", "full"])).contains("old-output"));
    assert!(daemon.job(&["retry", &id]).status.success());
    assert!(daemon.job(&["wait", &id]).status.success());
}

#[test]
fn removal_refuses_symlinked_data_and_never_touches_the_link_target() {
    let daemon = Daemon::with_profile("remove-symlink", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "true"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    std::fs::write(daemon.file("outside"), "retained").unwrap();
    std::os::unix::fs::symlink(
        daemon.file("outside"),
        daemon.state.join("jobs").join(&id).join("unexpected-link"),
    )
    .unwrap();
    assert!(!daemon.job(&["remove", &id]).status.success());
    assert!(daemon.state.join("jobs").join(&id).exists());
    assert_eq!(
        std::fs::read_to_string(daemon.file("outside")).unwrap(),
        "retained"
    );
}

#[test]
fn removal_reports_a_live_supervisor_without_discarding_the_completed_record() {
    let daemon = Daemon::with_profile("remove-supervisor", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "true"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    let path = daemon.state.join("jobs").join(&id).join("job.json");
    let mut job: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let pid = std::process::id();
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let ticks: u64 = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap();
    job["shim_pid"] = serde_json::json!(pid);
    job["shim_start_ticks"] = serde_json::json!(ticks);
    std::fs::write(&path, serde_json::to_vec(&job).unwrap()).unwrap();
    let preview = daemon.job(&["remove", &id, "--dry-run", "--json"]);
    let preview: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(preview["ready"], false);
    assert_eq!(preview["issues"][0]["pending"], true);
    assert!(path.exists());
    assert!(!daemon.state.join("removals/pending.json").exists());
}

#[test]
fn removal_requires_acknowledging_a_lost_attempt_even_after_a_successful_retry() {
    let daemon = Daemon::with_profile("remove-lost", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "true"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    let path = daemon.state.join("jobs").join(&id).join("job.json");
    let mut job: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    job["state"] = serde_json::json!("Lost");
    job["supervisor_boot_id"] = serde_json::json!("past-boot");
    std::fs::write(&path, serde_json::to_vec(&job).unwrap()).unwrap();
    assert!(daemon.job(&["retry", &id, "--allow-lost"]).status.success());
    assert!(daemon.job(&["wait", &id]).status.success());
    assert!(!daemon.job(&["remove", &id]).status.success());
    assert!(path.exists());
    assert!(
        daemon
            .job(&["remove", &id, "--allow-lost"])
            .status
            .success()
    );
}

#[test]
fn removal_stops_only_the_recorded_helpers_of_the_selected_queue() {
    let daemon = Daemon::with_profile("remove-helpers", "ordinary");
    assert!(daemon.job(&["queue", "create", "network"]).status.success());
    let mut helper = Command::new("sleep").arg("20").spawn().unwrap();
    let pid = helper.id();
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let ticks: u64 = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap();
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
    let boot = u128::from_str_radix(&boot.trim().replace('-', ""), 16)
        .unwrap()
        .to_be_bytes();
    let process = serde_json::json!({"pid": pid, "start_ticks": ticks, "boot_id": boot});
    let link =
        serde_json::json!({"name": "network", "holder": process, "relay": process, "rate": null});
    std::fs::create_dir_all(daemon.state.join("links")).unwrap();
    std::fs::write(
        daemon.state.join("links/network.json"),
        serde_json::to_vec(&link).unwrap(),
    )
    .unwrap();
    let output = daemon.job(&["queue", "remove", "network", "--json"]);
    let ended = helper.try_wait().unwrap().is_some();
    let _ = helper.kill();
    let _ = helper.wait();
    assert!(
        output.status.success(),
        "{} {}",
        text(&output),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(ended);
    assert!(!daemon.state.join("links/network.json").exists());
}

#[test]
fn removal_refuses_a_symlinked_retirement_directory_before_moving_records() {
    let daemon = Daemon::with_profile("remove-parent-symlink", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "true"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    let outside = daemon.file("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep"), "untouched").unwrap();
    std::os::unix::fs::symlink(&outside, daemon.state.join(".removed")).unwrap();
    assert!(!daemon.job(&["remove", &id]).status.success());
    assert!(daemon.state.join("jobs").join(&id).exists());
    assert!(!daemon.state.join("removals/pending.json").exists());
    assert_eq!(
        std::fs::read_to_string(outside.join("keep")).unwrap(),
        "untouched"
    );
}

#[test]
fn removal_refuses_a_populated_recorded_cgroup_even_for_a_terminal_job() {
    let daemon = Daemon::with_profile("remove-populated", "ordinary");
    let id = text(&daemon.job(&["submit", "--", "true"]))
        .trim()
        .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    let group = daemon.file("recorded-cgroup");
    std::fs::create_dir(&group).unwrap();
    std::fs::write(group.join("cgroup.events"), "populated 1\nfrozen 0\n").unwrap();
    let path = daemon.state.join("jobs").join(&id).join("job.json");
    let mut job: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    job["workload_cgroup"] = serde_json::json!(group);
    std::fs::write(&path, serde_json::to_vec(&job).unwrap()).unwrap();
    assert!(!daemon.job(&["remove", &id]).status.success());
    assert!(path.exists());
    std::fs::write(group.join("cgroup.events"), "populated 0\nfrozen 0\n").unwrap();
    assert!(daemon.job(&["remove", &id]).status.success());
}

#[test]
fn aggregate_controls_refuse_watch_backend_before_creating_objects() {
    let daemon = Daemon::with_profile("aggregate-unavailable", "ordinary");
    for option in [
        "--cpu-limit",
        "--cpu-weight",
        "--memory-high",
        "--memory-max",
        "--memory-swap-max",
    ] {
        let output = daemon.job(&["group", "create", "limited", option, "1"]);
        assert!(!output.status.success(), "{option}");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("requested resource control is unavailable")
        );
    }
    let output = daemon.job(&["group", "show", "limited", "--json"]);
    assert!(!output.status.success());
    assert!(daemon.job(&["group", "create", "empty"]).status.success());
    assert!(daemon.job(&["queue", "create", "empty/q"]).status.success());
    assert!(
        daemon
            .job(&["run", "--queue", "empty/q", "--", "true"])
            .status
            .success()
    );
}

#[test]
fn io_policies_are_validated_and_do_not_silently_fall_back_to_watching() {
    let daemon = Daemon::with_profile("io-watch-refusal", "ordinary");
    let host: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["host", "--json"]).stdout).unwrap();
    for device in host["io_devices"].as_array().unwrap() {
        assert_eq!(device["io_max"], false);
        assert_eq!(device["io_weight"], false);
        assert_eq!(device["io_bfq_weight"], false);
    }
    for (option, value) in [
        ("--io-max", "8:0,wbps=1M"),
        ("--io-weight", "8:0=100"),
        ("--io-bfq-weight", "8:0=100"),
        ("--io-max", "8:0,riops=0"),
        ("--io-max", "8:0,riops=4294967295"),
        ("--io-weight", "8:0=10001"),
        ("--io-bfq-weight", "8:0=1001"),
    ] {
        assert!(
            !daemon
                .job(&["create", option, value, "--", "true"])
                .status
                .success()
        );
    }
    assert!(
        daemon
            .job(&["queue", "create", "defaults", "--job-io-max", "8:0,wbps=1M"])
            .status
            .success()
    );
    assert!(
        !daemon
            .job(&["create", "-q", "defaults", "--", "true"])
            .status
            .success()
    );
    assert!(
        daemon
            .job(&["queue", "unset", "defaults", "job-io-max"])
            .status
            .success()
    );
    assert_eq!(
        text(&daemon.job(&["create", "-q", "defaults", "--", "true"])).trim(),
        "1"
    );
}

fn ordering_ok(daemon: &Daemon, args: &[&str]) -> String {
    let output = daemon.job(args);
    assert!(
        output.status.success()
            || (args.first() == Some(&"status") && output.status.code() == Some(75)),
        "{args:?}: {} {}",
        text(&output),
        String::from_utf8_lossy(&output.stderr)
    );
    text(&output).trim().to_owned()
}

fn ordering_explain(daemon: &Daemon, id: &str) -> serde_json::Value {
    serde_json::from_str(&ordering_ok(daemon, &["explain", id, "--json"])).unwrap()
}

#[test]
fn unix_cli_executes_literal_argv_and_selects_shells_explicitly() {
    use std::os::unix::fs::PermissionsExt;
    let daemon = Daemon::with_profile("unix-argv", "ordinary");
    let run = |args: &[&str]| {
        daemon
            .job_command(args)
            .env_remove("JOB_CLI_COMPAT")
            .output()
            .unwrap()
    };
    let executable = daemon.file("literal name; touch injected");
    std::fs::write(&executable, "#!/bin/sh\nprintf 'literal-output\\n'\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let submitted = run(&["submit", "--", executable.to_str().unwrap()]);
    assert!(
        submitted.status.success(),
        "{}",
        String::from_utf8_lossy(&submitted.stderr)
    );
    let id = text(&submitted).trim().to_owned();
    let waited = run(&["wait", &id]);
    assert!(waited.status.success());
    assert!(waited.stdout.is_empty() && waited.stderr.is_empty());
    assert!(!daemon.file("injected").exists());
    assert_eq!(text(&daemon.job(&["log", &id, "full"])), "literal-output\n");
    let status = preset_record(&daemon, &id);
    assert_eq!(
        status["spec"]["argv"],
        serde_json::json!([executable.to_str().unwrap()])
    );
    let explicit = run(&[
        "submit",
        "--shell",
        "/bin/sh",
        "--",
        "printf '%s:%s' \"$0\" \"$1\"",
        "one argument; no expansion",
    ]);
    assert!(explicit.status.success());
    let id = text(&explicit).trim().to_owned();
    assert!(run(&["wait", &id]).status.success());
    assert_eq!(
        text(&daemon.job(&["log", &id, "full"])),
        "job:one argument; no expansion\n"
    );
    let explicit_argv = run(&["submit", "--", "sh", "-c", "printf pipeline | cat"]);
    assert!(explicit_argv.status.success());
    assert!(run(&["wait", text(&explicit_argv).trim()]).status.success());
    let legacy = run(&["submit", "--legacy-shell", "--", "printf compatibility"]);
    assert!(legacy.status.success());
    assert!(run(&["wait", text(&legacy).trim()]).status.success());
    assert!(
        !run(&["submit", "--shell", "sh", "--legacy-shell", "--", "true"])
            .status
            .success()
    );
    assert!(
        !daemon
            .job_command(&["submit", "--", "true"])
            .env("JOB_CLI_COMPAT", "unknown")
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn unix_wait_is_quiet_and_distinguishes_outcomes_from_command_exit_codes() {
    let daemon = Daemon::with_profile("unix-wait", "ordinary");
    let run = |args: &[&str]| {
        daemon
            .job_command(args)
            .env_remove("JOB_CLI_COMPAT")
            .output()
            .unwrap()
    };
    let output = run(&["submit", "--", "sh", "-c", "printf private-output; exit 7"]);
    assert!(output.status.success());
    let id = text(&output).trim().to_owned();
    let wait = run(&["wait", "--timeout", "5s", &id]);
    assert_eq!(wait.status.code(), Some(7));
    assert!(wait.stdout.is_empty() && wait.stderr.is_empty());
    let summary = run(&["wait", "--summary", &id]);
    assert_eq!(summary.status.code(), Some(7));
    assert!(text(&summary).contains("private-output"));
    let structured = run(&["wait", "--json", &id]);
    let result: serde_json::Value = serde_json::from_slice(&structured.stdout).unwrap();
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["outcome"], "completed");
    assert_eq!(result["exit_status"], 7);
    assert_eq!(result["job"]["id"], id.parse::<u64>().unwrap());
    assert!(!run(&["wait", &id, "--summary", "--json"]).status.success());
    for (command, expected, outcome) in [
        ("exit 125", 125, "completed"),
        ("exit 75", 75, "completed"),
        ("kill -TERM $$", 143, "completed"),
    ] {
        let output = run(&["submit", "--shell", "sh", "--", command]);
        assert!(output.status.success());
        let waited = run(&["wait", "--json", text(&output).trim()]);
        assert_eq!(waited.status.code(), Some(expected));
        let result: serde_json::Value = serde_json::from_slice(&waited.stdout).unwrap();
        assert_eq!(result["outcome"], outcome);
    }
    let missing = run(&["wait", "--json", "999999"]);
    assert_eq!(missing.status.code(), Some(125));
    let result: serde_json::Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(result["outcome"], "service_error");
    let held = run(&["create", "--", "true"]);
    let id = text(&held).trim().to_owned();
    let timeout = run(&["wait", "--timeout", "1ms", &id]);
    assert_eq!(timeout.status.code(), Some(75));
    assert!(timeout.stdout.is_empty() && timeout.stderr.is_empty());
    let timeout = run(&["wait", "--timeout", "1ms", "--json", &id]);
    let result: serde_json::Value = serde_json::from_slice(&timeout.stdout).unwrap();
    assert_eq!(result["outcome"], "timeout");
    assert_eq!(preset_record(&daemon, &id)["state"], "Held");
    ordering_ok(&daemon, &["cancel", &id]);
    let cancelled = run(&["wait", "--json", &id]);
    let result: serde_json::Value = serde_json::from_slice(&cancelled.stdout).unwrap();
    assert_eq!(result["outcome"], "cancelled");
    assert_eq!(cancelled.status.code(), Some(1));
    let started = Instant::now();
    let unavailable = daemon
        .job_command(&["wait", "--timeout", "20ms", "--json", &id])
        .env_remove("JOB_CLI_COMPAT")
        .env("JOB_STATE_DIR", daemon.base.join("unused-daemon"))
        .output()
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(unavailable.status.code(), Some(75));
    let result: serde_json::Value = serde_json::from_slice(&unavailable.stdout).unwrap();
    assert_eq!(result["outcome"], "unavailable");
}

const PRESETS: &str = r#"
schema_version = 1
[[presets.classes]]
name = 'interactive'
revision = 1
priority = 100
[[presets.classes]]
name = 'batch'
revision = 1
priority = -10
[[presets.profiles]]
name = 'base'
revision = 1
scheduling_class = 'interactive@1'
[presets.profiles.values]
cpu_request_milli = 1000
memory_request = 1048576
[[presets.profiles]]
name = 'build'
revision = 1
extends = ['base@1']
[presets.profiles.values]
cpu_request_milli = 2000
"#;

fn load_presets(daemon: &Daemon, config: &str) {
    std::fs::write(daemon.base.join("config.toml"), config).unwrap();
    ordering_ok(daemon, &["config", "reload"]);
}

fn preset_record(daemon: &Daemon, id: &str) -> serde_json::Value {
    serde_json::from_str(&ordering_ok(daemon, &["status", id, "--json"])).unwrap()
}

#[test]
fn versioned_presets_resolve_hierarchy_overrides_and_explicit_absence() {
    let mut daemon = Daemon::with_profile("presets-resolution", "ordinary");
    load_presets(&daemon, PRESETS);
    ordering_ok(&daemon, &["queue", "create", "plain"]);
    let object: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["queue", "show", "plain", "--json"])).unwrap();
    assert_eq!(
        object["objects"][0]["object"]["config"],
        serde_json::json!({})
    );
    let plain = ordering_ok(&daemon, &["create", "-q", "plain", "--", "true"]);
    assert!(preset_record(&daemon, &plain)["preset_snapshot"].is_null());
    ordering_ok(
        &daemon,
        &[
            "group",
            "create",
            "work",
            "--job-execution-profile",
            "build@1",
        ],
    );
    ordering_ok(
        &daemon,
        &["queue", "create", "work/q", "--job-cpu-request", "3"],
    );
    let inherited = ordering_ok(&daemon, &["create", "-q", "work/q", "--", "true"]);
    let record = preset_record(&daemon, &inherited);
    assert_eq!(record["spec"]["declared"]["cpu_request_milli"], 3000);
    assert_eq!(record["spec"]["declared"]["memory_request"], 1048576);
    assert_eq!(record["spec"]["declared"]["priority"], 100);
    assert!(record["spec"]["declared"]["cpu_weight"].is_null());
    assert!(record["requested_spec"]["declared"]["execution_profile"].is_null());
    assert_eq!(
        record["resource_sources"]["memory_request"]["Preset"]["definition"],
        "profile:base@1"
    );
    assert_eq!(
        record["resource_sources"]["cpu_request_milli"]["Object"]["path"],
        "work/q"
    );
    assert_eq!(
        ordering_explain(&daemon, &inherited)["priority_source"]["Preset"]["definition"],
        "class:interactive@1"
    );
    let direct = ordering_ok(
        &daemon,
        &[
            "create",
            "-q",
            "work/q",
            "--execution-profile",
            "build@1",
            "--class",
            "batch@1",
            "--memory-request",
            "2M",
            "--",
            "true",
        ],
    );
    let record = preset_record(&daemon, &direct);
    assert_eq!(record["spec"]["declared"]["cpu_request_milli"], 2000);
    assert_eq!(record["spec"]["declared"]["memory_request"], 2097152);
    assert_eq!(record["spec"]["declared"]["priority"], -10);
    assert_eq!(record["resource_sources"]["memory_request"], "Job");
    assert_eq!(
        record["preset_snapshot"]["definitions"]
            .as_object()
            .unwrap()
            .len(),
        4
    );
    let disabled = ordering_ok(
        &daemon,
        &[
            "create",
            "-q",
            "work/q",
            "--execution-profile",
            "none",
            "--class",
            "none",
            "--",
            "true",
        ],
    );
    let record = preset_record(&daemon, &disabled);
    assert_eq!(record["spec"]["declared"]["cpu_request_milli"], 3000);
    assert!(record["spec"]["declared"]["memory_request"].is_null());
    assert!(record["spec"]["declared"]["priority"].is_null());
    ordering_ok(
        &daemon,
        &["queue", "set", "work/q", "--job-class", "batch@1"],
    );
    let class_override = ordering_ok(&daemon, &["create", "-q", "work/q", "--", "true"]);
    assert_eq!(
        preset_record(&daemon, &class_override)["spec"]["declared"]["priority"],
        -10
    );
    ordering_ok(&daemon, &["queue", "unset", "work/q", "job-class"]);
    ordering_ok(&daemon, &["group", "set", "work", "--priority-max", "50"]);
    assert!(
        !daemon
            .job(&["create", "-q", "work/q", "--", "true"])
            .status
            .success()
    );
    ordering_ok(&daemon, &["group", "unset", "work", "priority-max"]);
    let before = preset_record(&daemon, &direct)["preset_snapshot"].clone();
    daemon.restart();
    assert_eq!(preset_record(&daemon, &direct)["preset_snapshot"], before);
    for id in [plain, inherited, direct, disabled, class_override] {
        ordering_ok(&daemon, &["cancel", &id]);
    }
}

#[test]
fn versioned_presets_keep_revisions_and_attempt_snapshots_immutable() {
    let mut daemon = Daemon::with_profile("presets-revisions", "ordinary");
    load_presets(&daemon, PRESETS);
    let shown: serde_json::Value = serde_json::from_str(&ordering_ok(
        &daemon,
        &["profile", "show", "build@1", "--json"],
    ))
    .unwrap();
    assert_eq!(
        shown["definitions"][0]["sha256"].as_str().unwrap().len(),
        64
    );
    let class: serde_json::Value = serde_json::from_str(&ordering_ok(
        &daemon,
        &["class", "show", "interactive@1", "--json"],
    ))
    .unwrap();
    assert_eq!(class["definitions"][0]["definition"]["priority"], 100);
    ordering_ok(
        &daemon,
        &[
            "queue",
            "create",
            "work",
            "--job-execution-profile",
            "build@1",
        ],
    );
    let id = ordering_ok(&daemon, &["submit", "-q", "work", "--", "true"]);
    ordering_ok(&daemon, &["wait", &id, "--timeout", "5s"]);
    let first = preset_record(&daemon, &id)["preset_snapshot"].clone();
    std::fs::write(
        daemon.base.join("config.toml"),
        PRESETS.replace("priority = 100", "priority = 200"),
    )
    .unwrap();
    let refused = daemon.job(&["config", "reload"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("immutable"));
    let shown: serde_json::Value = serde_json::from_str(&ordering_ok(
        &daemon,
        &["class", "show", "interactive@1", "--json"],
    ))
    .unwrap();
    assert_eq!(shown["definitions"][0]["definition"]["priority"], 100);
    std::fs::write(daemon.base.join("config.toml"), "schema_version = 1\n").unwrap();
    assert!(!daemon.job(&["config", "reload"]).status.success());
    let next = format!(
        "{PRESETS}\n[[presets.profiles]]\nname = 'build'\nrevision = 2\nscheduling_class = 'batch@1'\n[presets.profiles.values]\ncpu_request_milli = 500\n"
    );
    load_presets(&daemon, &next);
    ordering_ok(
        &daemon,
        &["queue", "set", "work", "--job-execution-profile", "build@2"],
    );
    ordering_ok(&daemon, &["retry", &id, "--hold"]);
    let second = preset_record(&daemon, &id);
    assert_eq!(second["attempt"], 2);
    assert_eq!(second["spec"]["declared"]["cpu_request_milli"], 500);
    assert_eq!(second["spec"]["declared"]["priority"], -10);
    let attempts: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["attempts", &id, "--json"])).unwrap();
    assert_eq!(attempts["attempts"][0]["preset_snapshot"], first);
    ordering_ok(&daemon, &["cancel", &id]);
    ordering_ok(
        &daemon,
        &["queue", "unset", "work", "job-execution-profile"],
    );
    load_presets(&daemon, "schema_version = 1\n");
    assert!(ordering_ok(&daemon, &["profile", "list", "--json"]).contains("\"definitions\": []"));
    daemon.restart();
    let attempts: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["attempts", &id, "--json"])).unwrap();
    assert_eq!(attempts["attempts"][0]["preset_snapshot"], first);
    assert!(daemon.state.join("presets.json").is_file());
}

#[test]
fn versioned_presets_reject_invalid_and_excessive_composition_at_config_boundary() {
    let daemon = Daemon::with_profile("presets-invalid", "ordinary");
    let path = daemon.base.join("candidate.toml");
    let mut excessive =
        "schema_version = 1\n[[presets.profiles]]\nname = 'p0'\nrevision = 1\n".to_owned();
    for depth in 1..=11 {
        excessive.push_str(&format!(
            "[[presets.profiles]]\nname = 'p{depth}'\nrevision = 1\nextends = ['p{}@1', 'p{}@1']\n",
            depth - 1,
            depth - 1
        ));
    }
    for candidate in [
        PRESETS.replace("priority = 100", "priority = 100\ncpu_weight = 200"),
        PRESETS.replace("priority = 100", "priority = 1001"),
        PRESETS.replace("revision = 1", "revision = 0"),
        PRESETS.replace("extends = ['base@1']", "extends = ['build@1']"),
        PRESETS.replace("extends = ['base@1']", "extends = ['missing@1']"),
        PRESETS.replace("extends = ['base@1']", "extends = ['none']"),
        PRESETS.replace("cpu_request_milli = 2000", "memory_hihg = 2000"),
        PRESETS.replace("cpu_request_milli = 2000", "confine = 'false'"),
        PRESETS.replace("cpu_request_milli = 2000", "net = 'socks5://secret'"),
        format!("{PRESETS}\n[[presets.classes]]\nname = 'batch'\nrevision = 1\npriority = 0\n"),
        excessive,
    ] {
        std::fs::write(&path, &candidate).unwrap();
        let refused = daemon.job(&["config", "check", path.to_str().unwrap()]);
        assert!(!refused.status.success(), "{candidate}");
    }
    for reference in ["base", "base@01", "none@1", "base@0", "missing@1"] {
        assert!(
            !daemon
                .job(&["create", "--execution-profile", reference, "--", "true"])
                .status
                .success()
        );
    }
    assert!(!daemon.state.join("presets.json").exists());
}

#[test]
fn admission_priority_selects_waiting_jobs_without_changing_kernel_weights() {
    let daemon = Daemon::with_profile("priority-order", "ordinary");
    ordering_ok(&daemon, &["queue", "create", "work", "--max-running", "1"]);
    ordering_ok(&daemon, &["queue", "pause", "work"]);
    let low = ordering_ok(
        &daemon,
        &[
            "submit",
            "-q",
            "work",
            "--priority",
            "-100",
            "--",
            "echo low >> order",
        ],
    );
    let high = ordering_ok(
        &daemon,
        &[
            "submit",
            "-q",
            "work",
            "--priority",
            "100",
            "--",
            "echo high >> order",
        ],
    );
    let before: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["status", &high, "--json"])).unwrap();
    assert!(before["spec"]["declared"]["cpu_weight"].is_null());
    assert!(before["spec"]["declared"]["io_weight"].is_null());
    ordering_ok(&daemon, &["queue", "resume", "work"]);
    ordering_ok(&daemon, &["wait", &low]);
    ordering_ok(&daemon, &["wait", &high]);
    assert_eq!(
        std::fs::read_to_string(daemon.file("order")).unwrap(),
        "high\nlow\n"
    );
}

#[test]
fn admission_strict_fifo_overrides_urgency_through_a_group() {
    let daemon = Daemon::with_profile("priority-fifo", "ordinary");
    ordering_ok(
        &daemon,
        &[
            "group",
            "create",
            "work",
            "--strict-fifo",
            "true",
            "--max-running",
            "1",
        ],
    );
    ordering_ok(&daemon, &["queue", "create", "work/a"]);
    ordering_ok(
        &daemon,
        &["queue", "create", "work/b", "--strict-fifo", "false"],
    );
    ordering_ok(&daemon, &["group", "pause", "work"]);
    let low = ordering_ok(
        &daemon,
        &[
            "submit",
            "-q",
            "work/a",
            "--priority",
            "-100",
            "--",
            "echo low >> order",
        ],
    );
    let high = ordering_ok(
        &daemon,
        &[
            "submit",
            "-q",
            "work/b",
            "--priority",
            "100",
            "--",
            "echo high >> order",
        ],
    );
    let explain = ordering_explain(&daemon, &high);
    assert_eq!(explain["fifo_predecessor"][0].as_u64(), low.parse().ok());
    ordering_ok(&daemon, &["group", "resume", "work"]);
    ordering_ok(&daemon, &["wait", &low]);
    ordering_ok(&daemon, &["wait", &high]);
    assert_eq!(
        std::fs::read_to_string(daemon.file("order")).unwrap(),
        "low\nhigh\n"
    );
}

#[test]
fn admission_defaults_are_snapshots_and_ancestor_bounds_remain_binding() {
    let daemon = Daemon::with_profile("priority-bounds", "ordinary");
    ordering_ok(
        &daemon,
        &[
            "group",
            "create",
            "work",
            "--priority",
            "20",
            "--priority-max",
            "30",
        ],
    );
    ordering_ok(
        &daemon,
        &["queue", "create", "work/q", "--priority-max", "100"],
    );
    let id = ordering_ok(&daemon, &["create", "-q", "work/q", "--", "true"]);
    let explanation = ordering_explain(&daemon, &id);
    assert_eq!(explanation["priority"], 20);
    assert_eq!(explanation["priority_source"]["Object"]["path"], "work");
    ordering_ok(&daemon, &["group", "set", "work", "--priority", "25"]);
    assert_eq!(ordering_explain(&daemon, &id)["priority"], 20);
    assert!(
        !daemon
            .job(&["reprioritize", &id, "--priority", "31"])
            .status
            .success()
    );
    assert!(
        !daemon
            .job(&["submit", "-q", "work/q", "--priority", "31", "--", "true"])
            .status
            .success()
    );
    assert!(
        !daemon
            .job(&["queue", "set", "work/q", "--priority-min", "40"])
            .status
            .success()
    );
    ordering_ok(&daemon, &["reprioritize", &id, "--priority", "30"]);
    let record: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["status", &id, "--json"])).unwrap();
    assert_eq!(record["priority_changes"][0]["before"], 20);
    assert_eq!(record["priority_changes"][0]["after"], 30);
    assert_eq!(record["priority_changes"][0]["actor_uid"], unsafe {
        libc::geteuid()
    });
    assert!(record["submitted_spec"]["declared"]["priority"].is_null());
    ordering_ok(&daemon, &["cancel", &id]);
}

#[test]
fn admission_aging_survives_restart_and_excludes_paused_time() {
    let mut daemon = Daemon::with_profile("priority-aging", "ordinary");
    ordering_ok(
        &daemon,
        &[
            "queue",
            "create",
            "work",
            "--aging",
            "10ms",
            "--max-running",
            "1",
        ],
    );
    let occupant = ordering_ok(&daemon, &["submit", "-q", "work", "--", "sleep 20"]);
    let low = ordering_ok(
        &daemon,
        &[
            "submit",
            "-q",
            "work",
            "--priority",
            "-10",
            "--",
            "echo low >> order",
        ],
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if ordering_explain(&daemon, &low)["eligible_wait_ms"]
            .as_u64()
            .unwrap()
            >= 250
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    ordering_ok(&daemon, &["queue", "pause", "work"]);
    let credit = ordering_explain(&daemon, &low)["eligible_wait_ms"]
        .as_u64()
        .unwrap();
    daemon.restart();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(ordering_explain(&daemon, &low)["eligible_wait_ms"], credit);
    let high = ordering_ok(
        &daemon,
        &[
            "submit",
            "-q",
            "work",
            "--priority",
            "10",
            "--",
            "echo high >> order",
        ],
    );
    ordering_ok(&daemon, &["cancel", &occupant]);
    daemon.job(&["wait", &occupant]);
    ordering_ok(&daemon, &["queue", "resume", "work"]);
    ordering_ok(&daemon, &["wait", &low]);
    ordering_ok(&daemon, &["wait", &high]);
    assert_eq!(
        std::fs::read_to_string(daemon.file("order")).unwrap(),
        "low\nhigh\n"
    );
}

#[test]
fn admission_retry_starts_without_its_old_waiting_credit() {
    let daemon = Daemon::with_profile("priority-retry", "ordinary");
    ordering_ok(
        &daemon,
        &[
            "queue",
            "create",
            "work",
            "--aging",
            "1ms",
            "--max-running",
            "1",
        ],
    );
    let occupant = ordering_ok(&daemon, &["submit", "-q", "work", "--", "sleep 20"]);
    let id = ordering_ok(&daemon, &["submit", "-q", "work", "--", "true"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while ordering_explain(&daemon, &id)["eligible_wait_ms"]
        .as_u64()
        .unwrap()
        == 0
    {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    ordering_ok(&daemon, &["cancel", &id]);
    ordering_ok(&daemon, &["retry", &id, "--hold"]);
    assert_eq!(ordering_explain(&daemon, &id)["eligible_wait_ms"], 0);
    ordering_ok(&daemon, &["cancel", &id]);
    ordering_ok(&daemon, &["cancel", &occupant]);
    daemon.job(&["wait", &occupant]);
}

fn wait_path(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(Instant::now() < deadline, "{}", path.display());
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn fair_share_keeps_collection_debt_across_restart_and_selects_before_urgency() {
    let mut daemon = Daemon::with_profile("fair-share-restart", "ordinary");
    ordering_ok(
        &daemon,
        &[
            "group",
            "create",
            "team",
            "--fair-share",
            "cpu-request-time",
            "--max-running",
            "1",
        ],
    );
    ordering_ok(&daemon, &["queue", "create", "team/a"]);
    ordering_ok(&daemon, &["queue", "create", "team/b"]);
    let occupant = ordering_ok(
        &daemon,
        &[
            "submit",
            "-q",
            "team/a",
            "--cpu-request",
            "0.1",
            "--",
            "touch ready; sleep 20",
        ],
    );
    wait_path(&daemon.file("ready"));
    let high = ordering_ok(
        &daemon,
        &[
            "submit",
            "-q",
            "team/a",
            "--priority",
            "1000",
            "--cpu-request",
            "0.1",
            "--",
            "echo a >> order",
        ],
    );
    let low = ordering_ok(
        &daemon,
        &[
            "submit",
            "-q",
            "team/b",
            "--priority",
            "-1000",
            "--cpu-request",
            "0.1",
            "--",
            "echo b >> order",
        ],
    );
    let before = ordering_explain(&daemon, &high)["fair_share"][0]["service"]
        .as_str()
        .unwrap()
        .parse::<u128>()
        .unwrap();
    daemon.restart();
    let after = ordering_explain(&daemon, &high)["fair_share"][0]["service"]
        .as_str()
        .unwrap()
        .parse::<u128>()
        .unwrap();
    assert!(after >= before);
    ordering_ok(&daemon, &["cancel", &occupant]);
    daemon.job(&["wait", &occupant]);
    ordering_ok(&daemon, &["wait", &high]);
    ordering_ok(&daemon, &["wait", &low]);
    assert_eq!(
        std::fs::read_to_string(daemon.file("order")).unwrap(),
        "b\na\n"
    );
    assert_eq!(
        ordering_explain(&daemon, &low)["fair_share"][0]["resource"],
        "cpu-request-time"
    );
}

#[test]
fn fair_share_requires_declared_resources_and_basis_changes_require_draining() {
    let daemon = Daemon::with_profile("fair-share-resource", "ordinary");
    ordering_ok(
        &daemon,
        &[
            "group",
            "create",
            "team",
            "--fair-share",
            "memory-request-time",
        ],
    );
    ordering_ok(
        &daemon,
        &["queue", "create", "team/a", "--share-weight", "2"],
    );
    assert!(
        !daemon
            .job(&["submit", "-q", "team/a", "--", "true"])
            .status
            .success()
    );
    assert!(
        !daemon
            .job(&["queue", "create", "bad", "--fair-share", "cpu-request-time"])
            .status
            .success()
    );
    let id = ordering_ok(
        &daemon,
        &[
            "submit",
            "-q",
            "team/a",
            "--memory-request",
            "1M",
            "--",
            "touch ready; sleep 20",
        ],
    );
    wait_path(&daemon.file("ready"));
    assert!(
        !daemon
            .job(&["group", "set", "team", "--fair-share", "off"])
            .status
            .success()
    );
    ordering_ok(&daemon, &["queue", "set", "team/a", "--share-weight", "3"]);
    let explanation = ordering_explain(&daemon, &id);
    assert_eq!(explanation["fair_share"][0]["weight"], 2);
    ordering_ok(&daemon, &["cancel", &id]);
    daemon.job(&["wait", &id]);
    ordering_ok(&daemon, &["group", "set", "team", "--fair-share", "off"]);
    ordering_ok(&daemon, &["run", "-q", "team/a", "--", "true"]);
}

#[test]
fn fair_share_nested_groups_expose_each_account_and_fifo_still_wins() {
    let daemon = Daemon::with_profile("fair-share-nested", "ordinary");
    ordering_ok(
        &daemon,
        &[
            "group",
            "create",
            "team",
            "--fair-share",
            "cpu-request-time",
            "--strict-fifo",
            "true",
            "--max-running",
            "1",
        ],
    );
    ordering_ok(
        &daemon,
        &[
            "group",
            "create",
            "team/nested",
            "--fair-share",
            "memory-request-time",
            "--share-weight",
            "2",
        ],
    );
    ordering_ok(&daemon, &["queue", "create", "team/nested/a"]);
    ordering_ok(&daemon, &["queue", "create", "team/b"]);
    ordering_ok(&daemon, &["group", "pause", "team"]);
    let first = ordering_ok(
        &daemon,
        &[
            "submit",
            "-q",
            "team/nested/a",
            "--cpu-request",
            "0.1",
            "--memory-request",
            "1M",
            "--priority",
            "-1000",
            "--",
            "echo first >> order",
        ],
    );
    let second = ordering_ok(
        &daemon,
        &[
            "submit",
            "-q",
            "team/b",
            "--cpu-request",
            "0.1",
            "--priority",
            "1000",
            "--",
            "echo second >> order",
        ],
    );
    let explanation = ordering_explain(&daemon, &first);
    assert_eq!(explanation["fair_share"].as_array().unwrap().len(), 2);
    assert_eq!(
        explanation["fair_share"][0]["resource"],
        "memory-request-time"
    );
    assert_eq!(explanation["fair_share"][1]["weight"], 2);
    ordering_ok(&daemon, &["group", "resume", "team"]);
    ordering_ok(&daemon, &["wait", &first]);
    ordering_ok(&daemon, &["wait", &second]);
    assert_eq!(
        std::fs::read_to_string(daemon.file("order")).unwrap(),
        "first\nsecond\n"
    );
}

#[test]
fn fair_share_balances_weighted_service_under_a_bounded_continuous_backlog() {
    let daemon = Daemon::with_profile("fair-share-backlog", "ordinary");
    ordering_ok(
        &daemon,
        &[
            "group",
            "create",
            "fair",
            "--fair-share",
            "cpu-request-time",
            "--max-running",
            "1",
        ],
    );
    ordering_ok(
        &daemon,
        &["queue", "create", "fair/a", "--share-weight", "1"],
    );
    ordering_ok(
        &daemon,
        &["queue", "create", "fair/b", "--share-weight", "2"],
    );
    ordering_ok(&daemon, &["group", "pause", "fair"]);
    let mut ids = Vec::new();
    for (queue, request, command) in [
        ("fair/a", "0.2", "echo a >> order; sleep 0.1"),
        ("fair/b", "0.1", "echo b >> order; sleep 0.1"),
    ] {
        for _ in 0..48 {
            ids.push(ordering_ok(
                &daemon,
                &[
                    "submit",
                    "-q",
                    queue,
                    "--cpu-request",
                    request,
                    "--",
                    command,
                ],
            ));
        }
    }
    ordering_ok(&daemon, &["group", "resume", "fair"]);
    let deadline = Instant::now() + Duration::from_secs(90);
    let order = loop {
        let order = std::fs::read_to_string(daemon.file("order")).unwrap_or_default();
        if order.lines().count() >= 40 {
            break order;
        }
        assert!(Instant::now() < deadline, "{order}");
        std::thread::sleep(Duration::from_millis(20));
    };
    ordering_ok(&daemon, &["host"]);
    let ledger: serde_json::Value =
        serde_json::from_slice(&std::fs::read(daemon.state.join("scheduling.json")).unwrap())
            .unwrap();
    let scope = ledger["fair"]["scopes"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    let branches: Vec<_> = scope["branches"].as_object().unwrap().values().collect();
    assert_eq!(branches.len(), 2);
    let a = branches
        .iter()
        .find(|branch| branch["weight"] == 1)
        .unwrap()["service"]
        .as_str()
        .unwrap()
        .parse::<u128>()
        .unwrap();
    let b = branches
        .iter()
        .find(|branch| branch["weight"] == 2)
        .unwrap()["service"]
        .as_str()
        .unwrap()
        .parse::<u128>()
        .unwrap();
    let mut largest_charge = 0;
    for id in &ids {
        let record: serde_json::Value = serde_json::from_slice(
            &std::fs::read(daemon.state.join(format!("jobs/{id}/job.json"))).unwrap(),
        )
        .unwrap();
        if let (Some(start), Some(end)) = (
            record["started_ms"].as_u64(),
            record["finished_ms"].as_u64(),
        ) {
            largest_charge = largest_charge.max(end.saturating_sub(start) as u128 * 200);
        }
    }
    let bound = largest_charge + 200 * 250;
    let a_starts = order.lines().filter(|line| *line == "a").count();
    let b_starts = order.lines().filter(|line| *line == "b").count();
    ordering_ok(&daemon, &["group", "cancel", "fair", "--recursive"]);
    for id in &ids {
        daemon.job(&["wait", id]);
    }
    assert!(
        a_starts < 48 && b_starts < 48,
        "both queues must stay backlogged"
    );
    assert!(
        b_starts >= 2 * a_starts,
        "larger weight and smaller request should start more work: {order}"
    );
    assert!(
        a + b > 4 * bound,
        "not enough service to assess balance: {a}, {b}, {bound}"
    );
    assert!(
        a.abs_diff(b) <= bound,
        "weighted services {a}, {b}; largest-job/lookahead bound {bound}"
    );
}

#[test]
fn pressure_reports_host_availability_without_enabling_a_policy() {
    let daemon = Daemon::with_profile("pressure-host", "ordinary");
    let output = daemon.job(&["pressure", "--json"]);
    assert!(output.status.success());
    let snapshot: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(snapshot["schema_version"], 1);
    assert_eq!(snapshot["scope"], "host");
    assert!(snapshot["boot_id"].as_str().is_some());
    assert!(snapshot["sampled_boot_ms"].as_u64().is_some());
    let observations = snapshot["observations"].as_array().unwrap();
    assert_eq!(observations.len(), 3);
    assert_eq!(observations[0]["resource"], "cpu");
    assert_eq!(observations[0]["full"]["availability"], "undefined");
    for observation in observations {
        assert!(observation["path"].as_str().is_some());
        let reading = &observation["some"];
        if reading["availability"] == "available" {
            assert!(reading["avg10_bp"].as_u64().unwrap() <= 10000);
            assert!(reading["total_us"].as_u64().is_some());
        } else {
            assert_eq!(reading["availability"], "unavailable");
            assert!(reading["avg10_bp"].is_null());
        }
    }
    let graph = ordering_ok(&daemon, &["queue", "show", "default", "--json"]);
    let graph: serde_json::Value = serde_json::from_str(&graph).unwrap();
    assert_eq!(
        graph["objects"][0]["object"]["config"],
        serde_json::json!({})
    );
    ordering_ok(&daemon, &["run", "--", "true"]);
    assert!(!daemon.state.join("pressure.json").exists());
}

#[test]
fn pressure_never_substitutes_host_data_for_a_watch_or_held_job() {
    let daemon = Daemon::with_profile("pressure-watch", "ordinary");
    let held = ordering_ok(&daemon, &["create", "--", "true"]);
    let running = ordering_ok(&daemon, &["submit", "--", "touch ready; sleep 10"]);
    wait_path(&daemon.file("ready"));
    for id in [&held, &running] {
        let value: serde_json::Value =
            serde_json::from_str(&ordering_ok(&daemon, &["pressure", id, "--json"])).unwrap();
        assert_eq!(value["scope"], "job");
        assert_eq!(value["job_id"].as_u64(), id.parse().ok());
        assert_eq!(value["attempt"], 1);
        assert!(value["cgroup"].is_null());
        for observation in value["observations"].as_array().unwrap() {
            assert!(observation["path"].is_null());
            for metric in ["some", "full"] {
                assert_eq!(observation[metric]["availability"], "unavailable");
                assert!(observation[metric]["total_us"].is_null());
            }
        }
    }
    assert!(!daemon.job(&["pressure", "999999"]).status.success());
    ordering_ok(&daemon, &["cancel", &held]);
    ordering_ok(&daemon, &["cancel", &running]);
    daemon.job(&["wait", &running]);
}

#[test]
fn pressure_parse_reads_saved_metrics_and_preserves_partial_unavailability() {
    let daemon = Daemon::with_profile("pressure-saved", "ordinary");
    let path = daemon.file("psi.txt");
    std::fs::write(&path, "some avg10=1.23 avg60=4.56 avg300=7.89 total=9007199254740993\nfull avg10=NaN avg60=0 avg300=0 total=10\n").unwrap();
    let output = daemon.job(&[
        "pressure",
        "parse",
        path.to_str().unwrap(),
        "--resource",
        "memory",
    ]);
    assert!(output.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["observation"]["some"]["avg10_bp"], 123);
    assert_eq!(parsed["observation"]["some"]["avg300_bp"], 789);
    assert_eq!(
        parsed["observation"]["some"]["total_us"],
        9_007_199_254_740_993_u64
    );
    assert_eq!(parsed["observation"]["full"]["availability"], "unavailable");
    let output = daemon.job(&[
        "pressure",
        "parse",
        path.to_str().unwrap(),
        "--resource",
        "cpu",
        "--host",
    ]);
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["observation"]["full"]["availability"], "undefined");
    let link = daemon.file("psi-link");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(
        !daemon
            .job(&[
                "pressure",
                "parse",
                link.to_str().unwrap(),
                "--resource",
                "memory"
            ])
            .status
            .success()
    );
}

fn pressure_rule(required: bool) -> serde_json::Value {
    serde_json::json!([{"id":"protect","resource":"memory","metric":"full","window":"avg10","high_bp":100,"low_bp":10,"sustain_ms":1000,"minimum_hold_ms":2000,"recovery_ms":1000,"step_ms":1000,"required":required}])
}

fn wait_pressure(
    daemon: &Daemon,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let until = Instant::now() + Duration::from_secs(8);
    loop {
        let report: serde_json::Value =
            serde_json::from_str(&ordering_ok(daemon, &["pressure", "status"])).unwrap();
        if predicate(&report) {
            return report;
        }
        assert!(Instant::now() < until, "{report}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn required_pressure_holds_ancestors_preserves_aging_and_recovers_after_removal() {
    let mut daemon = Daemon::with_profile("pressure-required", "ordinary");
    let rules = daemon.file("rules.json");
    std::fs::write(&rules, serde_json::to_vec(&pressure_rule(true)).unwrap()).unwrap();
    ordering_ok(
        &daemon,
        &[
            "group",
            "create",
            "protected",
            "--pressure",
            rules.to_str().unwrap(),
            "--aging",
            "1ms",
        ],
    );
    ordering_ok(&daemon, &["queue", "create", "protected/a"]);
    let pending = ordering_ok(
        &daemon,
        &[
            "submit",
            "-q",
            "protected/a",
            "--priority",
            "1000",
            "--",
            "touch admitted",
        ],
    );
    wait_pressure(&daemon, |s| {
        s["rules"]
            .as_object()
            .unwrap()
            .values()
            .any(|r| r["phase"] == "holding")
    });
    assert!(!daemon.file("admitted").exists());
    let explain: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["explain", &pending, "--json"])).unwrap();
    assert_eq!(
        explain["pressure"][0]["signal"]["availability"],
        "unavailable"
    );
    assert_eq!(explain["pressure"][0]["scope_path"], "protected");
    assert_eq!(explain["pressure"][0]["temporary_max_running"], 0);
    ordering_ok(&daemon, &["run", "--", "true"]);
    daemon.restart();
    wait_pressure(&daemon, |s| {
        s["rules"]
            .as_object()
            .unwrap()
            .values()
            .any(|r| r["phase"] == "holding")
    });
    let after: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["explain", &pending, "--json"])).unwrap();
    assert!(
        after["eligible_wait_ms"].as_u64().unwrap()
            >= explain["eligible_wait_ms"].as_u64().unwrap()
    );
    ordering_ok(&daemon, &["group", "rename", "protected", "renamed"]);
    ordering_ok(&daemon, &["group", "unset", "renamed", "pressure"]);
    ordering_ok(&daemon, &["wait", &pending, "--timeout", "5s"]);
    assert!(daemon.file("admitted").exists());
    let events: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["pressure", "events"])).unwrap();
    assert!(
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["removed"] == true)
    );
}

#[test]
fn optional_missing_pressure_is_visible_and_persistence_failure_holds_starts() {
    let daemon = Daemon::with_profile("pressure-optional", "ordinary");
    let rules = daemon.file("rules.json");
    std::fs::write(&rules, serde_json::to_vec(&pressure_rule(false)).unwrap()).unwrap();
    ordering_ok(
        &daemon,
        &[
            "queue",
            "create",
            "optional",
            "--pressure",
            rules.to_str().unwrap(),
        ],
    );
    ordering_ok(&daemon, &["run", "-q", "optional", "--", "true"]);
    let report = wait_pressure(&daemon, |s| !s["rules"].as_object().unwrap().is_empty());
    let rule = report["rules"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(rule["phase"], "open");
    assert_eq!(rule["signal"]["availability"], "unavailable");
    let ledger = daemon.state.join("pressure.json");
    let backup = daemon.state.join("pressure.saved");
    std::fs::rename(&ledger, &backup).unwrap();
    std::fs::create_dir(&ledger).unwrap();
    wait_pressure(&daemon, |s| s["error"].is_string());
    let pending = ordering_ok(
        &daemon,
        &["submit", "-q", "optional", "--", "touch should-wait"],
    );
    let other = ordering_ok(&daemon, &["create", "--", "true"]);
    ordering_ok(&daemon, &["cancel", &other]);
    assert!(!daemon.file("should-wait").exists());
    std::fs::remove_dir(&ledger).unwrap();
    std::fs::rename(&backup, &ledger).unwrap();
    ordering_ok(&daemon, &["wait", &pending, "--timeout", "5s"]);
    assert!(daemon.file("should-wait").exists());
}

#[test]
fn service_pressure_configuration_is_explicit_and_rejects_host_cpu_full() {
    let daemon = Daemon::with_profile("pressure-config", "ordinary");
    let config = "schema_version = 1\nprofile = 'ordinary'\n[[pressure]]\nid = 'host-memory'\nresource = 'memory'\nmetric = 'full'\nwindow = 'avg60'\nhigh_bp = 10000\nlow_bp = 0\nsustain_ms = 1000\nminimum_hold_ms = 2000\nrecovery_ms = 1000\nstep_ms = 1000\nrequired = true\n";
    let path = daemon.base.join("config.toml");
    std::fs::write(&path, config).unwrap();
    ordering_ok(&daemon, &["config", "check", path.to_str().unwrap()]);
    ordering_ok(&daemon, &["config", "reload"]);
    let report = wait_pressure(&daemon, |s| s["rules"]["host:host-memory"].is_object());
    let rule = &report["rules"]["host:host-memory"];
    assert!(rule["scope_id"].is_null());
    assert_eq!(rule["aggregation"], "host");
    assert_eq!(rule["rule"]["window"], "avg60");
    let shown: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["config", "show", "--json"])).unwrap();
    assert_eq!(shown["config"]["pressure"][0]["id"], "host-memory");
    std::fs::write(
        &path,
        config.replace("resource = 'memory'", "resource = 'cpu'"),
    )
    .unwrap();
    assert!(
        !daemon
            .job(&["config", "check", path.to_str().unwrap()])
            .status
            .success()
    );
    assert!(!daemon.job(&["config", "reload"]).status.success());
    let unchanged = wait_pressure(&daemon, |s| {
        s["rules"]["host:host-memory"]["rule"]["resource"] == "memory"
    });
    assert!(unchanged["error"].is_null());
}

#[test]
fn pressure_replay_explains_spikes_recovery_missing_data_and_boot_boundaries_offline() {
    let daemon = Daemon::with_profile("pressure-replay", "ordinary");
    let path = daemon.file("trace.json");
    let original = include_bytes!("../../../docs/examples/pressure-trace.json");
    std::fs::write(&path, original).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_job"))
            .args(["pressure", "replay", path.to_str().unwrap(), "--json"])
            .env("JOB_STATE_DIR", daemon.base.join("unused-service"))
            .output()
            .unwrap()
    };
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    let samples = report["samples"].as_array().unwrap();
    let expected = [
        "open",
        "open",
        "open",
        "open",
        "open",
        "holding",
        "holding",
        "holding",
        "holding",
        "recovering",
        "recovering",
        "recovering",
        "recovering",
        "open",
        "holding",
        "holding",
        "holding",
        "recovering",
        "recovering",
        "recovering",
        "holding",
        "holding",
        "holding",
        "recovering",
        "recovering",
        "holding",
        "holding",
        "holding",
        "holding",
        "recovering",
        "recovering",
        "open",
    ];
    assert_eq!(
        samples
            .iter()
            .map(|s| s["view"]["phase"].as_str().unwrap())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(samples[9]["view"]["temporary_max_running"], 3);
    assert_eq!(samples[10]["admission_held"], true);
    assert_eq!(samples[12]["view"]["temporary_max_running"], 4);
    assert_eq!(samples[20]["view"]["hold_since_ms"], 0);
    assert_eq!(samples[24]["view"]["high_since_ms"], 7000);
    assert_eq!(samples[29]["view"]["temporary_max_running"], 1);
    assert_eq!(samples[30]["view"]["temporary_max_running"], 2);
    assert!(!daemon.base.join("unused-service").exists());
    let mut optional: serde_json::Value = serde_json::from_slice(original).unwrap();
    optional["rule"]["required"] = serde_json::json!(false);
    std::fs::write(&path, serde_json::to_vec(&optional).unwrap()).unwrap();
    let output = run();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["samples"][14]["view"]["phase"], "open");
    assert_eq!(report["samples"][15]["view"]["phase"], "open");
    optional["frames"][1]["at_ms"] = serde_json::json!(0);
    std::fs::write(&path, serde_json::to_vec(&optional).unwrap()).unwrap();
    assert!(!run().status.success());
    assert!(!daemon.base.join("unused-service").exists());
}

#[test]
fn stream_run_delivers_binary_stdout_and_stderr_before_exit() {
    let daemon = Daemon::with_profile("stream-live", "ordinary");
    let mut child = daemon.job_command(&["run", "--budget", "none", "--", "sh", "-c", "printf 'out\\000\\377'; printf 'err\\033[31m' >&2; i=0; while [ ! -e release ] && [ $i -lt 250 ]; do sleep .02; i=$((i+1)); done; printf end; printf done >&2; exit 7"])
        .env_remove("JOB_CLI_COMPAT").stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out_tx = tx.clone();
    let out = std::thread::spawn(move || {
        let mut first = [0; 5];
        stdout.read_exact(&mut first).unwrap();
        out_tx.send(first.to_vec()).unwrap();
        let mut tail = Vec::new();
        stdout.read_to_end(&mut tail).unwrap();
        tail
    });
    let err = std::thread::spawn(move || {
        let mut first = [0; 8];
        stderr.read_exact(&mut first).unwrap();
        tx.send(first.to_vec()).unwrap();
        let mut tail = Vec::new();
        stderr.read_to_end(&mut tail).unwrap();
        tail
    });
    let mut first = vec![
        rx.recv_timeout(Duration::from_secs(4)).unwrap(),
        rx.recv_timeout(Duration::from_secs(4)).unwrap(),
    ];
    first.sort();
    assert_eq!(first, [b"err\x1b[31m".to_vec(), b"out\0\xff".to_vec()]);
    assert!(child.try_wait().unwrap().is_none());
    std::fs::write(daemon.file("release"), "").unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(7));
    assert_eq!(out.join().unwrap(), b"end");
    assert_eq!(err.join().unwrap(), b"done");
    let stdout = daemon.job(&["logs", "1", "--stream", "stdout", "--raw"]);
    assert!(
        stdout.status.success(),
        "{}",
        String::from_utf8_lossy(&stdout.stderr)
    );
    assert_eq!(stdout.stdout, b"out\0\xffend");
    assert_eq!(
        daemon
            .job(&["logs", "1", "--stream", "stderr", "--raw"])
            .stdout,
        b"err\x1b[31mdone"
    );
    let safe = daemon.job(&["logs", "1"]);
    assert!(safe.status.success());
    assert!(!safe.stdout.contains(&27));
    assert!(text(&safe).contains("\\x1b[31m"));
    let records = daemon.job(&["logs", "1", "--json"]);
    let records: Vec<serde_json::Value> = text(&records)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(records.iter().any(|r| r["stream"] == "stdout"));
    assert!(records.iter().any(|r| r["stream"] == "stderr"));
    assert!(
        records
            .windows(2)
            .all(|r| r[0]["sequence"].as_u64().unwrap() < r[1]["sequence"].as_u64().unwrap())
    );
    assert!(
        daemon
            .job(&["logs", "1", "--since-ms", "18446744073709551615"])
            .stdout
            .is_empty()
    );
}

#[test]
fn stream_follow_survives_daemon_restart_and_pins_retry_attempt() {
    let mut daemon = Daemon::with_profile("stream-follow", "ordinary");
    let id = ordering_ok(&daemon, &["submit", "--", "sh", "-c", "printf ready; i=0; while [ ! -e release ] && [ $i -lt 250 ]; do sleep .02; i=$((i+1)); done; head -c 1048576 /dev/zero; printf last >&2"]).trim().to_owned();
    let mut follower = daemon
        .job_command(&["logs", &id, "--follow", "--raw", "--stream", "stdout"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = follower.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let first = std::thread::spawn(move || {
        let mut bytes = [0; 5];
        pipe.read_exact(&mut bytes).unwrap();
        tx.send((bytes, pipe)).unwrap();
    });
    let (bytes, mut pipe) = rx.recv_timeout(Duration::from_secs(4)).unwrap();
    first.join().unwrap();
    assert_eq!(&bytes, b"ready");
    let snapshot = daemon.job(&["logs", &id, "--raw", "--stream", "stdout"]);
    assert!(snapshot.status.success());
    assert_eq!(snapshot.stdout, b"ready");
    daemon.restart();
    std::fs::write(daemon.file("release"), "").unwrap();
    assert!(daemon.job(&["wait", &id]).status.success());
    assert!(daemon.job(&["retry", &id, "--hold"]).status.success());
    let mut tail = Vec::new();
    pipe.read_to_end(&mut tail).unwrap();
    assert_eq!(tail, vec![0; 1048576]);
    assert!(follower.wait().unwrap().success());
    assert_eq!(
        daemon
            .job(&["logs", &id, "--attempt", "1", "--stream", "stderr", "--raw"])
            .stdout,
        b"last"
    );
    let current = daemon.job(&["logs", &id, "--raw"]);
    assert!(current.status.success() && current.stdout.is_empty());
    assert!(daemon.job(&["cancel", &id]).status.success());
    assert!(daemon.job(&["logs", &id]).status.success());
}

#[test]
fn stream_retention_is_bounded_and_reports_gaps() {
    let daemon = Daemon::with_profile("stream-retention", "ordinary");
    let id = ordering_ok(
        &daemon,
        &[
            "submit",
            "--",
            "sh",
            "-c",
            "printf ready; i=0; while [ ! -e release ] && [ $i -lt 250 ]; do sleep .02; i=$((i+1)); done; head -c 75497472 /dev/zero; printf retained-error >&2",
        ],
    )
    .trim()
    .to_owned();
    let mut follower = daemon
        .job_command(&["logs", &id, "--follow", "--raw", "--stream", "stdout"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = follower.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let prefix = std::thread::spawn(move || {
        let mut first = [0; 5];
        pipe.read_exact(&mut first).unwrap();
        tx.send((first, pipe)).unwrap();
    });
    let (first, mut pipe) = rx.recv_timeout(Duration::from_secs(4)).unwrap();
    prefix.join().unwrap();
    assert_eq!(&first, b"ready");
    std::fs::write(daemon.file("release"), "").unwrap();
    assert!(daemon.job(&["wait", &id]).status.success());
    let followed = std::io::copy(&mut pipe, &mut std::io::sink()).unwrap();
    assert!(followed < 75497472);
    let finished = follower.wait_with_output().unwrap();
    assert!(
        finished.status.success(),
        "{}",
        String::from_utf8_lossy(&finished.stderr)
    );
    assert!(String::from_utf8_lossy(&finished.stderr).contains("removed by retention"));
    let dir = daemon.state.join("jobs").join(&id);
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("streams.json")).unwrap()).unwrap();
    assert_eq!(meta["complete"], true);
    assert_eq!(meta["segments"].as_array().unwrap().len(), 64);
    assert!(meta["retired_records"].as_u64().unwrap() > 0);
    assert!(
        meta["segments"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["bytes"].as_u64().unwrap() <= 1048576)
    );
    let output_file = std::fs::File::create(daemon.file("retained")).unwrap();
    let output = daemon
        .job_command(&["logs", &id, "--raw", "--stream", "stdout"])
        .stdout(output_file)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("removed by retention"));
    assert!(std::fs::metadata(daemon.file("retained")).unwrap().len() < 67108864);
    let stderr = daemon.job(&["logs", &id, "--raw", "--stream", "stderr"]);
    assert!(stderr.status.success());
    assert_eq!(stderr.stdout, b"retained-error");
}

#[test]
fn stream_remote_frames_preserve_identity_and_corruption_is_visible() {
    let daemon = Daemon::with_profile("stream-remote", "ordinary");
    let id = ordering_ok(
        &daemon,
        &[
            "submit",
            "--",
            "sh",
            "-c",
            "printf remote-out; printf remote-err >&2",
        ],
    )
    .trim()
    .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    let mut remote = daemon
        .job_command(&["remote"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(remote.stdin.take().unwrap(), "{}", serde_json::json!({"protocol":20,"op":{"FollowStreams":{"id":id.parse::<u64>().unwrap(),"attempt":1}}})).unwrap();
    let output = remote.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut bytes = output.stdout.as_slice();
    let mut streams = std::collections::BTreeMap::<u8, Vec<u8>>::new();
    while !bytes.is_empty() {
        assert_eq!(&bytes[..4], b"JOL1");
        let length = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        streams
            .entry(bytes[24])
            .or_default()
            .extend_from_slice(&bytes[32..32 + length]);
        bytes = &bytes[32 + length..];
    }
    assert_eq!(streams[&1], b"remote-out");
    assert_eq!(streams[&2], b"remote-err");
    assert_eq!(streams[&7], [0]);
    let dir = daemon.state.join("jobs").join(&id);
    let segment = dir.join("streams-00000000000000000000.bin");
    let original = std::fs::read(&segment).unwrap();
    std::fs::write(&segment, &original[..original.len() - 1]).unwrap();
    assert!(!daemon.job(&["logs", &id]).status.success());
    let mut meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("streams.json")).unwrap()).unwrap();
    meta["complete"] = false.into();
    std::fs::write(dir.join("streams.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
    let incomplete = daemon.job(&["logs", &id]);
    assert!(!incomplete.status.success());
    assert!(String::from_utf8_lossy(&incomplete.stderr).contains("incomplete"));
    std::fs::write(&segment, original).unwrap();
    meta["complete"] = true.into();
    std::fs::write(dir.join("streams.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
}

#[test]
fn stream_client_disconnect_and_timeout_leave_work_running() {
    let daemon = Daemon::with_profile("stream-disconnect", "ordinary");
    let mut run = daemon
        .job_command(&[
            "run",
            "--budget",
            "none",
            "--",
            "sh",
            "-c",
            "printf ready; sleep .3; printf more; sleep 1; printf finished > survived",
        ])
        .env_remove("JOB_CLI_COMPAT")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(run.stdout.take());
    assert!(!run.wait().unwrap().success());
    assert!(daemon.job(&["wait", "1"]).status.success());
    assert_eq!(std::fs::read(daemon.file("survived")).unwrap(), b"finished");
    let timeout = daemon
        .job_command(&[
            "run",
            "--budget",
            "10ms",
            "--",
            "sh",
            "-c",
            "sleep .1; printf timeout-finished > timeout-survived",
        ])
        .env_remove("JOB_CLI_COMPAT")
        .output()
        .unwrap();
    assert_eq!(timeout.status.code(), Some(75));
    assert!(timeout.stdout.is_empty());
    assert!(daemon.job(&["wait", "2"]).status.success());
    assert_eq!(
        std::fs::read(daemon.file("timeout-survived")).unwrap(),
        b"timeout-finished"
    );
    let filtered = daemon.job(&["logs", "1", "--grep", "readymore", "--raw"]);
    assert!(filtered.status.success());
    assert_eq!(filtered.stdout, b"readymore");
}

#[test]
fn stream_backup_preserves_records_and_legacy_split_is_rejected() {
    let mut daemon = Daemon::with_profile("stream-backup", "ordinary");
    let id = ordering_ok(
        &daemon,
        &["submit", "--", "sh", "-c", "printf out; printf err >&2"],
    )
    .trim()
    .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    assert!(daemon.job(&["retry", &id]).status.success());
    assert!(daemon.job(&["wait", &id]).status.success());
    daemon.child.kill().unwrap();
    daemon.child.wait().unwrap();
    let backup = daemon.base.join("backup");
    let restored = daemon.base.join("restored");
    let back = daemon.job(&[
        "state",
        "backup",
        "--source",
        daemon.state.to_str().unwrap(),
        "--destination",
        backup.to_str().unwrap(),
    ]);
    assert!(
        back.status.success(),
        "{}",
        String::from_utf8_lossy(&back.stderr)
    );
    let restore = daemon.job(&[
        "state",
        "restore",
        "--source",
        backup.to_str().unwrap(),
        "--destination",
        restored.to_str().unwrap(),
    ]);
    assert!(
        restore.status.success(),
        "{}",
        String::from_utf8_lossy(&restore.stderr)
    );
    let output = daemon
        .job_command(&["logs", &id, "--attempt", "1", "--raw", "--stream", "stderr"])
        .env("JOB_STATE_DIR", &restored)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"err");
    let path = restored.join("jobs").join(&id).join("job.json");
    let mut job: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    job.as_object_mut().unwrap().remove("output_mode");
    std::fs::write(&path, serde_json::to_vec(&job).unwrap()).unwrap();
    let legacy = daemon
        .job_command(&["logs", &id, "--raw"])
        .env("JOB_STATE_DIR", &restored)
        .output()
        .unwrap();
    assert!(legacy.status.success());
    assert!(legacy.stdout.windows(3).any(|s| s == b"out"));
    let split = daemon
        .job_command(&["logs", &id, "--stream", "stderr"])
        .env("JOB_STATE_DIR", &restored)
        .output()
        .unwrap();
    assert!(!split.status.success());
    assert!(String::from_utf8_lossy(&split.stderr).contains("unavailable"));
}

#[test]
fn stream_pty_recording_stays_combined() {
    let daemon = Daemon::with_profile("stream-pty", "ordinary");
    let id = ordering_ok(
        &daemon,
        &[
            "submit",
            "--pty",
            "--",
            "sh",
            "-c",
            "printf out; printf err >&2",
        ],
    )
    .trim()
    .to_owned();
    assert!(daemon.job(&["wait", &id]).status.success());
    let combined = daemon.job(&["logs", &id, "--raw", "--stream", "combined"]);
    assert!(
        combined.status.success(),
        "{}",
        String::from_utf8_lossy(&combined.stderr)
    );
    assert_eq!(combined.stdout, b"outerr");
    let split = daemon.job(&["logs", &id, "--stream", "stderr"]);
    assert!(!split.status.success());
    let records = daemon.job(&["logs", &id, "--json"]);
    assert!(text(&records).lines().all(|line| {
        serde_json::from_str::<serde_json::Value>(line).unwrap()["stream"] == "terminal"
    }));
}

#[test]
fn stream_remote_supervisor_decodes_frames_and_separates_transport_stderr() {
    use std::os::unix::fs::PermissionsExt;
    let remote = Daemon::with_profile("stream-remote-peer", "ordinary");
    let mut local = Daemon::with_profile("stream-remote-local", "ordinary");
    let bin = local.base.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let ssh = bin.join("ssh");
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\nprintf transport-warning >&2\n[ $(ulimit -n) -gt 32 ] || exit 91\ngrep -q 'Seccomp:.0' /proc/self/status || exit 93\n[ $(wc -l < /proc/net/dev) -gt 3 ] || exit 94\nexec env JOB_STATE_DIR='{}' '{}' remote\n",
            remote.state.display(),
            env!("CARGO_BIN_EXE_job")
        ),
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = format!("{}:/usr/bin:/bin", bin.display());
    local.child.kill().unwrap();
    local.child.wait().unwrap();
    local.child = Command::new(env!("CARGO_BIN_EXE_job"))
        .arg("daemon")
        .env("JOB_CGROUP_ROOT", local.base.join("absent-cgroup"))
        .env("JOB_CONFIG", local.base.join("config.toml"))
        .env("JOB_STATE_DIR", &local.state)
        .env("PATH", &path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    local.ready();
    let submitted = local
        .job_command(&[
            "submit",
            "--on",
            "test-peer",
            "--rlimit", "nofile=32",
            "--seccomp-deny", "getppid",
            "--net", "none",
            "--",
            "sh",
            "-c",
            "[ $(ulimit -n) -eq 32 ] || exit 92; grep -q 'Seccomp:.2' /proc/self/status || exit 95; [ $(wc -l < /proc/net/dev) -eq 3 ] || exit 96; printf payload-out; printf payload-err >&2; exit 9",
        ])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(
        submitted.status.success(),
        "{}",
        String::from_utf8_lossy(&submitted.stderr)
    );
    let id = text(&submitted).trim().to_owned();
    let waited = local
        .job_command(&["wait", &id])
        .env_remove("JOB_CLI_COMPAT")
        .output()
        .unwrap();
    assert_eq!(
        waited.status.code(),
        Some(9),
        "{}",
        String::from_utf8_lossy(&waited.stderr)
    );
    let stdout = local.job(&["logs", &id, "--raw", "--stream", "stdout"]);
    assert!(
        stdout.status.success(),
        "{}",
        String::from_utf8_lossy(&stdout.stderr)
    );
    assert_eq!(stdout.stdout, b"payload-out");
    assert_eq!(
        local
            .job(&["logs", &id, "--raw", "--stream", "stderr"])
            .stdout,
        b"payload-err"
    );
    assert_eq!(
        local
            .job(&["logs", &id, "--raw", "--stream", "diagnostic"])
            .stdout,
        b"transport-warning"
    );
    let record: serde_json::Value =
        serde_json::from_slice(&local.job(&["status", &id, "--json"]).stdout).unwrap();
    assert_eq!(
        record["result"]["security_controls"]["seccomp_deny"],
        "getppid"
    );
}

#[test]
#[ignore]
fn process_policy_probe() {
    let Ok(mode) = std::env::var("JOB_PROCESS_PROBE") else {
        return;
    };
    let read = |resource| {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(unsafe { libc::getrlimit(resource, &mut limit) }, 0);
        (limit.rlim_cur, limit.rlim_max)
    };
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let cpus = status
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))
        .unwrap()
        .trim();
    let mut numa = 0 as libc::c_int;
    let mut nodes = vec![
        0 as libc::c_ulong;
        unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize
            / std::mem::size_of::<libc::c_ulong>()
    ];
    let numa_result = unsafe {
        libc::syscall(
            libc::SYS_get_mempolicy,
            &mut numa,
            nodes.as_mut_ptr(),
            (nodes.len() * libc::c_ulong::BITS as usize) as libc::c_ulong,
            std::ptr::null_mut::<libc::c_void>(),
            0 as libc::c_ulong,
        )
    };
    let mut opened = Vec::new();
    let mut open_error = None;
    if mode == "fds" {
        for _ in 0..512 {
            match std::fs::File::open("/dev/null") {
                Ok(f) => opened.push(f),
                Err(e) => {
                    open_error = e.raw_os_error();
                    break;
                }
            }
        }
    }
    let mut file_error = None;
    let mut written = 0;
    if mode == "fsize" {
        unsafe {
            libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
        }
        let mut file = std::fs::File::create("bounded-fsize-probe").unwrap();
        for _ in 0..16 {
            match file.write(&[0; 1024]) {
                Ok(n) => written += n,
                Err(e) => {
                    file_error = e.raw_os_error();
                    break;
                }
            }
        }
    }
    let child = if mode == "child" {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "process_policy_probe",
                "--nocapture",
            ])
            .env("JOB_PROCESS_PROBE", "read")
            .output()
            .unwrap();
        assert!(output.status.success());
        Some(String::from_utf8(output.stdout).unwrap())
    } else {
        None
    };
    println!(
        "PROCESS={}",
        serde_json::json!({"file_error":file_error,"written":written,"cpus":cpus,"nofile":read(libc::RLIMIT_NOFILE),"core":read(libc::RLIMIT_CORE),"fsize":read(libc::RLIMIT_FSIZE),"numa":numa,"numa_result":numa_result,"nodes":nodes.iter().enumerate().flat_map(|(word,bits)|(0..libc::c_ulong::BITS).filter(move |bit| bits & (1<<bit)!=0).map(move |bit|word as u32*libc::c_ulong::BITS+bit)).collect::<Vec<_>>(),"open_error":open_error,"child":child})
    );
}

fn process_probe(daemon: &Daemon, options: &[&str], mode: &str) -> (String, serde_json::Value) {
    let executable = std::env::current_exe().unwrap();
    let mut args = vec!["submit"];
    args.extend_from_slice(options);
    args.extend([
        "--",
        executable.to_str().unwrap(),
        "--ignored",
        "--exact",
        "process_policy_probe",
        "--nocapture",
    ]);
    let submitted = daemon
        .job_command(&args)
        .env("JOB_PROCESS_PROBE", mode)
        .output()
        .unwrap();
    assert!(
        submitted.status.success(),
        "{}",
        String::from_utf8_lossy(&submitted.stderr)
    );
    let id = text(&submitted).trim().to_owned();
    let waited = daemon.job(&["wait", &id]);
    assert!(
        waited.status.success(),
        "{} {}",
        text(&waited),
        String::from_utf8_lossy(&waited.stderr)
    );
    let output = daemon.job(&["logs", &id, "--stream", "stdout", "--raw"]);
    assert!(output.status.success());
    let output = text(&output);
    let value = output
        .lines()
        .find_map(|line| line.split_once("PROCESS=").map(|(_, value)| value))
        .unwrap_or_else(|| panic!("{output}"));
    (id, serde_json::from_str(value).unwrap())
}

#[test]
fn process_policy_affinity_and_rlimits_apply_only_to_workload_and_descendants() {
    let daemon = Daemon::with_profile("process-controls", "ordinary");
    let host: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["host", "--json"])).unwrap();
    let allowed = host["process_controls"]["allowed_cpus"].as_str().unwrap();
    let cpu = allowed.split([',', '-']).next().unwrap();
    let (_, normal) = process_probe(&daemon, &[], "read");
    assert_eq!(normal["cpus"], allowed);
    let pair = &host["process_controls"]["inherited_rlimits"]["nofile"];
    assert_eq!(normal["nofile"][0], pair["soft"]);
    let (id, limited) = process_probe(
        &daemon,
        &[
            "--cpu-affinity",
            cpu,
            "--rlimit",
            "nofile=64:96",
            "--rlimit",
            "core=0",
        ],
        "child",
    );
    assert_eq!(limited["cpus"], cpu);
    assert_eq!(limited["nofile"], serde_json::json!([64, 96]));
    assert_eq!(limited["core"], serde_json::json!([0, 0]));
    let child = limited["child"]
        .as_str()
        .unwrap()
        .lines()
        .find_map(|line| line.split_once("PROCESS=").map(|(_, value)| value))
        .unwrap();
    let child: serde_json::Value = serde_json::from_str(child).unwrap();
    assert_eq!(child["cpus"], cpu);
    assert_eq!(child["nofile"], limited["nofile"]);
    let saved = preset_record(&daemon, &id);
    assert_eq!(saved["result"]["process_controls"]["cpu_affinity"], cpu);
    assert_eq!(
        saved["result"]["process_controls"]["rlimits"]["nofile"],
        serde_json::json!({"soft":64,"hard":96})
    );
    let (_, fds) = process_probe(&daemon, &["--rlimit", "nofile=64:96"], "fds");
    assert_eq!(fds["open_error"], libc::EMFILE);
    let (_, file) = process_probe(&daemon, &["--rlimit", "fsize=1K:2K"], "fsize");
    assert_eq!(file["written"], 1024);
    assert_eq!(file["file_error"], libc::EFBIG);
    let (_, after) = process_probe(&daemon, &[], "read");
    assert_eq!(after["cpus"], normal["cpus"]);
    assert_eq!(after["nofile"], normal["nofile"]);
}

#[test]
fn process_policy_defaults_profiles_unset_and_retry_preserve_provenance() {
    let mut daemon = Daemon::with_profile("process-defaults", "ordinary");
    std::fs::write(daemon.base.join("config.toml"),"schema_version=1\nprofile='ordinary'\n[[presets.profiles]]\nname='bounded'\nrevision=1\n[presets.profiles.values]\ncpu_affinity='inherit'\nrlimit_nofile={soft=64,hard=96}\nrlimit_core={soft=0,hard=0}\n").unwrap();
    ordering_ok(&daemon, &["config", "reload"]);
    ordering_ok(
        &daemon,
        &[
            "group",
            "create",
            "g",
            "--job-execution-profile",
            "bounded@1",
        ],
    );
    ordering_ok(
        &daemon,
        &["queue", "create", "g/q", "--job-rlimit", "nofile=128:192"],
    );
    let (id, result) = process_probe(&daemon, &["--queue", "g/q"], "read");
    assert_eq!(result["nofile"], serde_json::json!([128, 192]));
    assert_eq!(result["core"], serde_json::json!([0, 0]));
    let saved = preset_record(&daemon, &id);
    assert_eq!(
        saved["resource_sources"]["rlimit_nofile"]["Object"]["path"],
        "g/q"
    );
    assert!(saved["resource_sources"]["rlimit_core"]["Preset"].is_object());
    let (_, overridden) = process_probe(
        &daemon,
        &["--queue", "g/q", "--rlimit", "nofile=80:96"],
        "read",
    );
    assert_eq!(overridden["nofile"], serde_json::json!([80, 96]));
    ordering_ok(&daemon, &["queue", "unset", "g/q", "job-rlimit-nofile"]);
    ordering_ok(&daemon, &["retry", &id]);
    assert!(daemon.job(&["wait", &id]).status.success());
    let retried = preset_record(&daemon, &id);
    assert_eq!(
        retried["result"]["process_controls"]["rlimits"]["nofile"]["soft"],
        64
    );
    let archive: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            daemon
                .state
                .join("jobs")
                .join(&id)
                .join("attempts/1/job.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        archive["result"]["process_controls"]["rlimits"]["nofile"]["soft"],
        128
    );
    let (_, reset) = process_probe(
        &daemon,
        &["--queue", "g/q", "--rlimit", "nofile=inherit"],
        "read",
    );
    let host: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["host", "--json"])).unwrap();
    assert_eq!(
        reset["nofile"][0],
        host["process_controls"]["inherited_rlimits"]["nofile"]["soft"]
    );
    daemon.restart();
    let (_, restarted) = process_probe(&daemon, &["--queue", "g/q"], "read");
    assert_eq!(restarted["nofile"], serde_json::json!([64, 96]));
}

#[test]
fn process_policy_rejects_invalid_requests_and_kernel_refusals_before_payload() {
    let daemon = Daemon::with_profile("process-invalid", "ordinary");
    for options in [
        ["--cpu-affinity", "65535"],
        ["--cpu-affinity", "3-1"],
        ["--cpu-affinity", "65536"],
        ["--numa-policy", "bind:65535"],
        ["--numa-policy", "preferred:0-1"],
        ["--rlimit", "nofile=100:99"],
        ["--rlimit", "rss=1"],
        ["--rlimit", "locks=1"],
        ["--rlimit", "cpu=1.2"],
        ["--rlimit", "as=18446744073709551615"],
    ] {
        let out = daemon.job(&[
            "submit",
            options[0],
            options[1],
            "--",
            "sh",
            "-c",
            "touch invalid-ran",
        ]);
        assert!(!out.status.success(), "{options:?}");
    }
    let id = ordering_ok(
        &daemon,
        &[
            "submit",
            "--rlimit",
            "nofile=unlimited",
            "--",
            "sh",
            "-c",
            "touch invalid-ran",
        ],
    )
    .trim()
    .to_owned();
    let waited = daemon
        .job_command(&["wait", &id])
        .env_remove("JOB_CLI_COMPAT")
        .output()
        .unwrap();
    assert_eq!(waited.status.code(), Some(125));
    let failed: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["status", &id, "--json"]).stdout).unwrap();
    assert!(failed["result"]["start_error"].is_string());
    assert!(!daemon.file("invalid-ran").exists());
}

#[test]
fn process_policy_numa_modes_are_read_back_by_the_executed_program() {
    let daemon = Daemon::with_profile("process-numa", "ordinary");
    let host: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["host", "--json"])).unwrap();
    if std::env::var_os("JOB_REQUIRE_NUMA").is_some() {
        assert!(
            host["process_controls"]["allowed_memory_nodes"].is_string(),
            "{host}"
        );
    }
    let Some(allowed) = host["process_controls"]["allowed_memory_nodes"].as_str() else {
        let out = daemon.job(&["submit", "--numa-policy", "bind:0", "--", "true"]);
        assert!(!out.status.success());
        return;
    };
    let node = allowed.split([',', '-']).next().unwrap();
    for (policy, mode) in [
        ("default".to_owned(), libc::MPOL_DEFAULT),
        ("local".to_owned(), libc::MPOL_LOCAL),
        (
            format!("bind:{node}"),
            libc::MPOL_BIND | libc::MPOL_F_STATIC_NODES,
        ),
        (
            format!("interleave:{node}"),
            libc::MPOL_INTERLEAVE | libc::MPOL_F_STATIC_NODES,
        ),
        (
            format!("preferred:{node}"),
            libc::MPOL_PREFERRED | libc::MPOL_F_STATIC_NODES,
        ),
    ] {
        let (id, result) = process_probe(&daemon, &["--numa-policy", &policy], "read");
        assert_eq!(result["numa_result"], 0);
        assert_eq!(result["numa"], mode, "{policy}");
        eprintln!("verified NUMA policy {policy}, kernel mode {mode}");
        assert_eq!(
            preset_record(&daemon, &id)["result"]["process_controls"]["numa_policy"],
            policy
        );
    }
}

#[test]
#[ignore]
fn security_policy_probe() {
    let Ok(mode) = std::env::var("JOB_SECURITY_PROBE") else {
        return;
    };
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .unwrap()
            .trim()
            .to_owned()
    };
    let errno = |number: libc::c_long| {
        if unsafe { libc::syscall(number) } < 0 {
            std::io::Error::last_os_error().raw_os_error()
        } else {
            None
        }
    };
    let bounding = |capability: libc::c_ulong| unsafe {
        libc::prctl(libc::PR_CAPBSET_READ, capability, 0, 0, 0)
    };
    let child = if mode == "child" {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "security_policy_probe",
                "--nocapture",
            ])
            .env("JOB_SECURITY_PROBE", "read")
            .output()
            .unwrap();
        assert!(output.status.success());
        Some(String::from_utf8(output.stdout).unwrap())
    } else {
        None
    };
    println!(
        "SECURITY={}",
        serde_json::json!({"no_new_privs":field("NoNewPrivs:"),"seccomp":field("Seccomp:"),"effective":field("CapEff:"),"permitted":field("CapPrm:"),"inheritable":field("CapInh:"),"bounding":field("CapBnd:"),"ambient":field("CapAmb:"),"bounding_net_raw":bounding(13),"bounding_chown":bounding(0),"getppid":errno(libc::SYS_getppid),"getuid":errno(libc::SYS_getuid),"child":child})
    );
}

fn security_probe(daemon: &Daemon, options: &[&str], mode: &str) -> (String, serde_json::Value) {
    let executable = std::env::current_exe().unwrap();
    let mut args = vec!["submit"];
    args.extend_from_slice(options);
    args.extend([
        "--",
        executable.to_str().unwrap(),
        "--ignored",
        "--exact",
        "security_policy_probe",
        "--nocapture",
    ]);
    let submitted = daemon
        .job_command(&args)
        .env("JOB_SECURITY_PROBE", mode)
        .output()
        .unwrap();
    assert!(
        submitted.status.success(),
        "{}",
        String::from_utf8_lossy(&submitted.stderr)
    );
    let id = text(&submitted).trim().to_owned();
    let waited = daemon.job(&["wait", &id]);
    assert!(
        waited.status.success(),
        "{} {}",
        text(&waited),
        String::from_utf8_lossy(&waited.stderr)
    );
    let output = daemon.job(&["logs", &id, "--stream", "stdout", "--raw"]);
    assert!(output.status.success());
    let output = text(&output);
    let value = output
        .lines()
        .find_map(|line| line.split_once("SECURITY=").map(|(_, value)| value))
        .unwrap_or_else(|| panic!("{output}"));
    (id, serde_json::from_str(value).unwrap())
}

#[test]
fn security_policy_applies_only_to_requesting_workload_and_descendants() {
    let daemon = Daemon::with_profile("security-controls", "ordinary");
    let host: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["host", "--json"])).unwrap();
    assert!(
        host["security_controls"]["supported_syscalls"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("getppid"))
    );
    let (plain, normal) = security_probe(&daemon, &[], "read");
    assert_eq!(
        normal["no_new_privs"] == "1",
        host["security_controls"]["service_no_new_privs"] == true
    );
    assert!(normal["getppid"].is_null());
    assert!(preset_record(&daemon, &plain)["result"]["security_controls"].is_null());
    let (id, locked) = security_probe(&daemon, &["--no-new-privs", "yes"], "read");
    assert_eq!(locked["no_new_privs"], "1");
    assert_eq!(locked["seccomp"], normal["seccomp"]);
    assert!(locked["getppid"].is_null());
    assert_eq!(
        preset_record(&daemon, &id)["result"]["security_controls"],
        serde_json::json!({"no_new_privs":true,"cap_drop":null,"cap_bounding_reduced":false,"seccomp_deny":null,"seccomp_arch":null})
    );
    let (id, denied) = security_probe(&daemon, &["--seccomp-deny", "getppid,getppid"], "child");
    assert_eq!(denied["no_new_privs"], "1");
    assert_eq!(denied["seccomp"], "2");
    assert_eq!(denied["getppid"], libc::EPERM);
    assert!(denied["getuid"].is_null());
    let child = denied["child"]
        .as_str()
        .unwrap()
        .lines()
        .find_map(|line| line.split_once("SECURITY=").map(|(_, value)| value))
        .unwrap();
    let child: serde_json::Value = serde_json::from_str(child).unwrap();
    assert_eq!(child["seccomp"], "2");
    assert_eq!(child["getppid"], libc::EPERM);
    let saved = preset_record(&daemon, &id);
    assert_eq!(
        saved["result"]["security_controls"]["seccomp_deny"],
        "getppid"
    );
    assert_eq!(
        saved["result"]["security_controls"]["seccomp_arch"],
        host["security_controls"]["seccomp_arch"]
    );
    assert_eq!(saved["resource_sources"]["seccomp_deny"], "Job");
    let (_, after) = security_probe(&daemon, &[], "read");
    assert_eq!(after, normal);
}

#[test]
fn security_policy_defaults_profiles_unset_and_retry_preserve_provenance() {
    let mut daemon = Daemon::with_profile("security-defaults", "ordinary");
    std::fs::write(daemon.base.join("config.toml"),"schema_version=1\nprofile='ordinary'\n[[presets.profiles]]\nname='locked'\nrevision=1\n[presets.profiles.values]\nno_new_privs=true\nseccomp_deny='getuid'\n").unwrap();
    ordering_ok(&daemon, &["config", "reload"]);
    ordering_ok(
        &daemon,
        &[
            "group",
            "create",
            "g",
            "--job-execution-profile",
            "locked@1",
        ],
    );
    ordering_ok(
        &daemon,
        &["queue", "create", "g/q", "--job-seccomp-deny", "getppid"],
    );
    let (id, result) = security_probe(&daemon, &["--queue", "g/q"], "read");
    assert_eq!(result["getppid"], libc::EPERM);
    assert!(result["getuid"].is_null());
    let saved = preset_record(&daemon, &id);
    assert_eq!(
        saved["resource_sources"]["seccomp_deny"]["Object"]["path"],
        "g/q"
    );
    assert!(saved["resource_sources"]["no_new_privs"]["Preset"].is_object());
    let (_, reset) = security_probe(
        &daemon,
        &["--queue", "g/q", "--seccomp-deny", "inherit"],
        "read",
    );
    assert!(reset["getppid"].is_null());
    assert_eq!(reset["seccomp"], "0");
    assert_eq!(reset["no_new_privs"], "1");
    ordering_ok(&daemon, &["queue", "unset", "g/q", "job-seccomp-deny"]);
    ordering_ok(&daemon, &["retry", &id]);
    assert!(daemon.job(&["wait", &id]).status.success());
    assert_eq!(
        preset_record(&daemon, &id)["result"]["security_controls"]["seccomp_deny"],
        "getuid"
    );
    let archive: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            daemon
                .state
                .join("jobs")
                .join(&id)
                .join("attempts/1/job.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        archive["result"]["security_controls"]["seccomp_deny"],
        "getppid"
    );
    daemon.restart();
    let (_, restarted) = security_probe(&daemon, &["--queue", "g/q"], "read");
    assert_eq!(restarted["getuid"], libc::EPERM);
    assert!(restarted["getppid"].is_null());
}

#[test]
fn security_policy_rejects_invalid_and_contradictory_requests_before_payload() {
    let daemon = Daemon::with_profile("security-invalid", "ordinary");
    for options in [
        vec!["--no-new-privs", "no"],
        vec!["--cap-drop", "everything"],
        vec!["--cap-drop", ""],
        vec!["--seccomp-deny", "all"],
        vec!["--seccomp-deny", "getppid,"],
        vec!["--seccomp-deny", "write"],
        vec!["--seccomp-deny", "getppid", "--seccomp-deny", "getuid"],
        vec!["--no-new-privs", "inherit", "--seccomp-deny", "getppid"],
        vec!["--no-new-privs", "inherit", "--cap-drop", "all"],
    ] {
        let mut args = vec!["submit"];
        args.extend(&options);
        args.extend(["--", "sh", "-c", "touch invalid-ran"]);
        let out = daemon.job(&args);
        assert!(!out.status.success(), "{options:?}");
    }
    let refused = daemon.job(&["queue", "create", "bad", "--job-seccomp-deny", "write"]);
    assert!(!refused.status.success());
    assert!(!daemon.file("invalid-ran").exists());
}

#[test]
fn security_policy_capability_reduction_reports_what_the_launch_context_permits() {
    let daemon = Daemon::with_profile("security-capabilities", "ordinary");
    let host: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["host", "--json"])).unwrap();
    assert!(
        host["security_controls"]["supported_cap_names"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("net_raw"))
    );
    let (_, normal) = security_probe(&daemon, &[], "read");
    let (id, reduced) = security_probe(&daemon, &["--cap-drop", "net_raw"], "read");
    assert_eq!(reduced["no_new_privs"], "1");
    for set in ["effective", "permitted", "inheritable", "ambient"] {
        let bits = u64::from_str_radix(reduced[set].as_str().unwrap(), 16).unwrap();
        assert_eq!(bits & (1 << 13), 0, "{set}");
    }
    let saved = preset_record(&daemon, &id);
    assert_eq!(saved["result"]["security_controls"]["cap_drop"], "net_raw");
    let privileged =
        u64::from_str_radix(normal["effective"].as_str().unwrap(), 16).unwrap() & (1 << 8) != 0;
    assert_eq!(
        saved["result"]["security_controls"]["cap_bounding_reduced"],
        privileged
    );
    assert_eq!(
        reduced["bounding_net_raw"] == 0,
        privileged || normal["bounding_net_raw"] == 0
    );
    let (id, isolated) = security_probe(
        &daemon,
        &["--net", "none", "--cap-drop", "all", "--confine"],
        "child",
    );
    for set in [
        "effective",
        "permitted",
        "inheritable",
        "ambient",
        "bounding",
    ] {
        assert_eq!(isolated[set], "0000000000000000", "{set}");
    }
    let child = isolated["child"]
        .as_str()
        .unwrap()
        .lines()
        .find_map(|line| line.split_once("SECURITY=").map(|(_, value)| value))
        .unwrap();
    let child: serde_json::Value = serde_json::from_str(child).unwrap();
    assert_eq!(child["bounding"], "0000000000000000");
    assert_eq!(
        preset_record(&daemon, &id)["result"]["security_controls"],
        serde_json::json!({"no_new_privs":true,"cap_drop":"all","cap_bounding_reduced":true,"seccomp_deny":null,"seccomp_arch":null})
    );
    let (_, combined) = security_probe(
        &daemon,
        &[
            "--net",
            "none",
            "--cap-drop",
            "all",
            "--seccomp-deny",
            "getppid",
            "--rlimit",
            "core=0",
        ],
        "read",
    );
    assert_eq!(combined["seccomp"], "2");
    assert_eq!(combined["getppid"], libc::EPERM);
    assert_eq!(combined["bounding"], "0000000000000000");
}

const ISOLATION_LINKS: &str =
    "for kind in uts ipc user cgroup mnt; do readlink /proc/self/ns/$kind; done";

fn isolation_submit(daemon: &Daemon, options: &[&str], script: &str) -> Output {
    let mut args = vec!["submit"];
    args.extend_from_slice(options);
    args.extend(["--", "sh", "-c", script]);
    daemon.job(&args)
}

fn isolation_run(daemon: &Daemon, options: &[&str], script: &str) -> (String, Vec<String>) {
    let submitted = isolation_submit(daemon, options, script);
    assert!(
        submitted.status.success(),
        "{options:?}: {}",
        String::from_utf8_lossy(&submitted.stderr)
    );
    let id = text(&submitted).trim().to_owned();
    let waited = daemon.job(&["wait", &id]);
    assert!(
        waited.status.success(),
        "{options:?}: {} {}",
        text(&waited),
        String::from_utf8_lossy(&waited.stderr)
    );
    let output = daemon.job(&["logs", &id, "--stream", "stdout", "--raw"]);
    assert!(output.status.success());
    (id, text(&output).lines().map(str::to_owned).collect())
}

fn isolation_area(name: &str) -> PathBuf {
    let area = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("isolation-{}-{name}", std::process::id()));
    assert!(!area.starts_with("/tmp") && !area.starts_with("/var/tmp"));
    std::fs::create_dir_all(&area).unwrap();
    area
}

#[test]
fn isolation_policy_host_reports_namespace_kinds_and_probes() {
    let daemon = Daemon::with_profile("isolation-host", "ordinary");
    let host: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["host", "--json"])).unwrap();
    let isolation = &host["isolation_controls"];
    for (kind, file) in [
        ("user", "user"),
        ("mount", "mnt"),
        ("ipc", "ipc"),
        ("uts", "uts"),
        ("cgroup", "cgroup"),
        ("pid", "pid"),
        ("net", "net"),
    ] {
        assert_eq!(
            isolation["namespace_kinds"][kind],
            Path::new("/proc/self/ns").join(file).exists(),
            "{kind}"
        );
    }
    assert_eq!(
        isolation["requestable_namespaces"],
        serde_json::json!(["cgroup", "ipc", "mount", "pid", "user", "uts"])
    );
    assert_eq!(isolation["pid_namespace_requestable"], true);
    assert!(isolation["pid_namespace_error"].is_null());
    assert_eq!(isolation["unprivileged_user_namespaces"], true);
    assert_eq!(isolation["mount_setattr"], true);
    assert!(isolation["mount_setattr_error"].is_null());
    assert!(isolation["landlock_abi"].is_null() || isolation["landlock_abi"].is_i64());
    assert!(isolation["missing_link_tools"].is_array());
    assert_eq!(isolation["ipv6_in_linked_network"], false);
    assert_eq!(
        isolation["network_modes"]["host"],
        serde_json::json!({"available":true,"namespace":"host","link":"none"})
    );
    assert_eq!(
        isolation["network_modes"]["none"],
        serde_json::json!({"available":true,"namespace":"per_job","link":"none"})
    );
    assert_eq!(
        isolation["network_modes"]["proxy"]["link"],
        "per_queue_or_per_job"
    );
    assert_eq!(isolation["network_modes"]["openvpn"]["available"], false);
    let shown = ordering_ok(&daemon, &["host"]);
    for expected in [
        "security controls: no_new_privs yes",
        "process controls: CPU affinity yes",
        "isolation: namespaces cgroup, ipc, mount, pid, user, uts",
        "pid namespaces with an init and a fresh /proc yes",
        "network namespaces: ",
        "host shared with the host",
        "none per job",
    ] {
        assert!(shown.contains(expected), "{expected}: {shown}");
    }
}

#[test]
fn isolation_policy_enters_only_the_requested_namespaces() {
    let daemon = Daemon::with_profile("isolation-namespaces", "ordinary");
    let service: Vec<String> = ["uts", "ipc", "user", "cgroup", "mnt"]
        .iter()
        .map(|kind| {
            std::fs::read_link(format!("/proc/{}/ns/{kind}", daemon.child.id()))
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let (plain, normal) = isolation_run(&daemon, &[], ISOLATION_LINKS);
    assert_eq!(normal, service);
    assert!(preset_record(&daemon, &plain)["result"]["isolation_controls"].is_null());
    let (id, entered) = isolation_run(
        &daemon,
        &["--namespaces", "uts,user,ipc,uts"],
        &format!("{ISOLATION_LINKS}; id -u"),
    );
    for index in 0..3 {
        assert_ne!(entered[index], service[index], "{index}");
    }
    assert_eq!(entered[3..5], service[3..5]);
    assert_eq!(entered[5], unsafe { libc::getuid() }.to_string());
    let saved = preset_record(&daemon, &id);
    assert_eq!(
        saved["result"]["isolation_controls"],
        serde_json::json!({"namespaces":["ipc","user","uts"],"user_namespace_from_network":false,"root_read_only":false,"private_tmp":[],"writable":[]})
    );
    assert_eq!(saved["spec"]["declared"]["namespaces"], "ipc,user,uts");
    assert_eq!(saved["resource_sources"]["namespaces"], "Job");
    let (id, networked) = isolation_run(
        &daemon,
        &["--net", "none", "--namespaces", "user,cgroup"],
        ISOLATION_LINKS,
    );
    assert_eq!(networked[0..2], service[0..2]);
    assert_ne!(networked[2], service[2]);
    assert_ne!(networked[3], service[3]);
    assert_eq!(
        preset_record(&daemon, &id)["result"]["isolation_controls"]["user_namespace_from_network"],
        true
    );
    let (_, after) = isolation_run(&daemon, &[], ISOLATION_LINKS);
    assert_eq!(after, service);
}

#[test]
fn isolation_policy_mount_policies_limit_writes_to_named_paths_and_private_tmp() {
    let daemon = Daemon::with_profile("isolation-mounts", "ordinary");
    let area = isolation_area("mounts");
    let writable = area.join("writable");
    let other = area.join("other");
    let cwd = area.join("cwd");
    std::fs::create_dir(&writable).unwrap();
    std::fs::create_dir(&other).unwrap();
    std::fs::create_dir(&cwd).unwrap();
    let marker = format!("job-isolation-private-{}", std::process::id());
    let script = format!(
        "touch /etc/{marker} 2>/dev/null && echo etc-written
touch '{other}/no' 2>/dev/null && echo other-written
touch cwd-no 2>/dev/null && echo cwd-written
echo inside > '{writable}/yes' && echo writable-ok
echo private > /tmp/{marker} && echo tmp-ok
ls /tmp
readlink /proc/self/ns/mnt",
        other = other.display(),
        writable = writable.display()
    );
    let (id, lines) = isolation_run(
        &daemon,
        &[
            "--dir",
            cwd.to_str().unwrap(),
            "--namespaces",
            "user,mount",
            "--root",
            "read-only",
            "--private-tmp",
            "yes",
            "--writable",
            writable.to_str().unwrap(),
        ],
        &script,
    );
    assert_eq!(lines[..3], ["writable-ok", "tmp-ok", marker.as_str()]);
    assert_ne!(
        Path::new(&lines[3]),
        std::fs::read_link(format!("/proc/{}/ns/mnt", daemon.child.id())).unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(writable.join("yes")).unwrap(),
        "inside\n"
    );
    assert!(!other.join("no").exists());
    assert!(!cwd.join("cwd-no").exists());
    assert!(!Path::new("/tmp").join(&marker).exists());
    assert!(!Path::new("/etc").join(&marker).exists());
    assert_eq!(review_tmp_entries(&daemon, &id), Vec::<String>::new());
    let saved = preset_record(&daemon, &id);
    let applied = &saved["result"]["isolation_controls"];
    assert_eq!(applied["namespaces"], serde_json::json!(["mount", "user"]));
    assert_eq!(applied["root_read_only"], true);
    assert_eq!(applied["private_tmp"][0], "/tmp");
    assert_eq!(
        applied["writable"],
        serde_json::json!([writable.to_str().unwrap()])
    );
    for field in ["namespaces", "root", "private_tmp", "writable"] {
        assert_eq!(saved["resource_sources"][field], "Job", "{field}");
    }
    let (_, inside) = isolation_run(
        &daemon,
        &[
            "--dir",
            writable.to_str().unwrap(),
            "--namespaces",
            "mount,user",
            "--root",
            "read-only",
            "--writable",
            writable.to_str().unwrap(),
        ],
        &format!(
            "echo relative > relative && echo relative-ok; echo shared > /tmp/{marker}-shared 2>/dev/null && echo tmp-written; true"
        ),
    );
    assert_eq!(inside, ["relative-ok"]);
    assert!(writable.join("relative").exists());
    assert!(!Path::new("/tmp").join(format!("{marker}-shared")).exists());
    let (_, private_only) = isolation_run(
        &daemon,
        &[
            "--dir",
            cwd.to_str().unwrap(),
            "--namespaces",
            "user,mount",
            "--private-tmp",
            "yes",
        ],
        &format!(
            "ls /tmp | wc -l; echo again > '{}/again' && echo root-writable",
            other.display()
        ),
    );
    assert_eq!(private_only, ["0", "root-writable"]);
    let (_, after) = isolation_run(&daemon, &[], "touch plain-after && echo plain-ok");
    assert_eq!(after, ["plain-ok"]);
    std::fs::remove_dir_all(&area).unwrap();
}

#[test]
fn isolation_policy_refuses_unavailable_and_incomplete_requests_before_payload() {
    let daemon = Daemon::with_profile("isolation-invalid", "ordinary");
    let area = isolation_area("invalid");
    let missing = area.join("missing");
    let missing = missing.to_str().unwrap();
    let existing = area.to_str().unwrap();
    let full = ["--namespaces", "user,mount", "--root", "read-only"];
    let mut cases: Vec<(Vec<&str>, &str)> = vec![
        (
            vec!["--namespaces", "pid"],
            "pid needs mount in --namespaces",
        ),
        (
            vec!["--namespaces", "user,pid"],
            "because the job gets its own /proc; add mount",
        ),
        (vec!["--namespaces", "net"], "requested with --net"),
        (vec!["--namespaces", ""], "invalid namespace list"),
        (vec!["--namespaces", "user,time"], "invalid namespace list"),
        (
            vec!["--namespaces", "user", "--namespaces", "uts"],
            "duplicate isolation control",
        ),
        (vec!["--root", "read-only"], "add --namespaces user,mount"),
        (
            vec!["--namespaces", "user", "--root", "read-only"],
            "--root read-only needs both mount and user",
        ),
        (
            vec!["--namespaces", "mount", "--root", "read-only"],
            "add --namespaces user,mount",
        ),
        (
            vec!["--namespaces", "inherit", "--private-tmp", "yes"],
            "--private-tmp yes needs both mount and user",
        ),
        (
            vec!["--namespaces", "user,mount", "--writable", existing],
            "--writable needs --root read-only",
        ),
        (
            vec![
                "--namespaces",
                "user",
                "--root",
                "read-only",
                "--writable",
                existing,
            ],
            "needs both mount and user",
        ),
        (
            vec!["--root", "read-write"],
            "--root requires read-only or inherit",
        ),
        (
            vec!["--private-tmp", "no"],
            "--private-tmp requires yes or inherit",
        ),
        (
            [&full[..], &["--writable", "relative/path"]].concat(),
            "absolute paths",
        ),
        ([&full[..], &["--writable", "/"]].concat(), "absolute paths"),
        (
            [&full[..], &["--writable", "/etc/../etc"]].concat(),
            "absolute paths",
        ),
        (
            [&full[..], &["--writable", missing]].concat(),
            "does not exist",
        ),
        (
            [&full[..], &["--private-tmp", "yes", "--writable", "/tmp"]].concat(),
            "hides /tmp",
        ),
    ];
    if unsafe { libc::geteuid() } != 0 {
        cases.push((vec!["--namespaces", "uts"], "add user"));
    }
    for (options, expected) in cases {
        let refused = isolation_submit(&daemon, &options, "touch invalid-ran");
        assert!(!refused.status.success(), "{options:?}");
        let message = String::from_utf8_lossy(&refused.stderr);
        assert!(message.contains(expected), "{options:?}: {message}");
    }
    let refused = daemon.job(&["queue", "create", "bad", "--job-namespaces", "net"]);
    assert!(!refused.status.success());
    ordering_ok(
        &daemon,
        &["queue", "create", "strict", "--job-root", "read-only"],
    );
    let refused = isolation_submit(&daemon, &["--queue", "strict"], "touch invalid-ran");
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("add --namespaces user,mount"));
    assert!(!daemon.file("invalid-ran").exists());
    assert!(
        std::fs::read_dir(daemon.state.join("jobs"))
            .map(|entries| entries.count())
            .unwrap_or(0)
            == 0
    );
    let gone = area.join("gone");
    std::fs::create_dir(&gone).unwrap();
    ordering_ok(&daemon, &["queue", "create", "later"]);
    ordering_ok(&daemon, &["queue", "pause", "later"]);
    let submitted = isolation_submit(
        &daemon,
        &[
            "--queue",
            "later",
            "--namespaces",
            "user,mount",
            "--root",
            "read-only",
            "--writable",
            gone.to_str().unwrap(),
        ],
        "touch invalid-ran",
    );
    assert!(
        submitted.status.success(),
        "{}",
        String::from_utf8_lossy(&submitted.stderr)
    );
    let id = text(&submitted).trim().to_owned();
    std::fs::remove_dir(&gone).unwrap();
    ordering_ok(&daemon, &["queue", "resume", "later"]);
    assert!(!daemon.job(&["wait", &id]).status.success());
    let failed: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["status", &id, "--json"]).stdout).unwrap();
    assert!(
        failed["result"]["start_error"]
            .as_str()
            .unwrap()
            .contains("does not exist"),
        "{failed}"
    );
    assert!(failed["result"]["isolation_controls"].is_null());
    assert!(!daemon.file("invalid-ran").exists());
    std::fs::remove_dir_all(&area).unwrap();
}

#[test]
fn isolation_policy_defaults_profiles_unset_and_retry_preserve_provenance() {
    let mut daemon = Daemon::with_profile("isolation-defaults", "ordinary");
    std::fs::write(daemon.base.join("config.toml"),"schema_version=1\nprofile='ordinary'\n[[presets.profiles]]\nname='apart'\nrevision=1\n[presets.profiles.values]\nnamespaces='user,uts'\n").unwrap();
    ordering_ok(&daemon, &["config", "reload"]);
    ordering_ok(
        &daemon,
        &["group", "create", "g", "--job-execution-profile", "apart@1"],
    );
    ordering_ok(
        &daemon,
        &["queue", "create", "g/q", "--job-namespaces", "ipc,user"],
    );
    let (_, service) = isolation_run(&daemon, &["--namespaces", "inherit"], ISOLATION_LINKS);
    let (id, queued) = isolation_run(&daemon, &["--queue", "g/q"], ISOLATION_LINKS);
    assert_eq!(queued[0], service[0]);
    assert_ne!(queued[1], service[1]);
    assert_ne!(queued[2], service[2]);
    let saved = preset_record(&daemon, &id);
    assert_eq!(
        saved["resource_sources"]["namespaces"]["Object"]["path"],
        "g/q"
    );
    assert_eq!(
        saved["result"]["isolation_controls"]["namespaces"],
        serde_json::json!(["ipc", "user"])
    );
    let (reset_id, reset) = isolation_run(
        &daemon,
        &["--queue", "g/q", "--namespaces", "inherit"],
        ISOLATION_LINKS,
    );
    assert_eq!(reset, service);
    let reset_record = preset_record(&daemon, &reset_id);
    assert_eq!(reset_record["resource_sources"]["namespaces"], "Job");
    assert!(reset_record["result"]["isolation_controls"].is_null());
    ordering_ok(
        &daemon,
        &[
            "queue",
            "set",
            "g/q",
            "--job-root",
            "read-only",
            "--job-namespaces",
            "mount,user",
        ],
    );
    let (mixed_id, mixed) = isolation_run(
        &daemon,
        &[
            "--queue",
            "g/q",
            "--dir",
            env!("CARGO_TARGET_TMPDIR"),
            "--private-tmp",
            "yes",
        ],
        "touch denied 2>/dev/null; ls /tmp | wc -l",
    );
    assert_eq!(mixed, ["0"]);
    let mixed_record = preset_record(&daemon, &mixed_id);
    assert_eq!(
        mixed_record["resource_sources"]["root"]["Object"]["path"],
        "g/q"
    );
    assert_eq!(mixed_record["resource_sources"]["private_tmp"], "Job");
    assert_eq!(
        mixed_record["result"]["isolation_controls"]["root_read_only"],
        true
    );
    ordering_ok(
        &daemon,
        &["queue", "unset", "g/q", "job-namespaces", "job-root"],
    );
    ordering_ok(&daemon, &["retry", &id]);
    assert!(daemon.job(&["wait", &id]).status.success());
    let retried = preset_record(&daemon, &id);
    assert_eq!(
        retried["result"]["isolation_controls"]["namespaces"],
        serde_json::json!(["user", "uts"])
    );
    assert!(retried["resource_sources"]["namespaces"]["Preset"].is_object());
    let archive: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            daemon
                .state
                .join("jobs")
                .join(&id)
                .join("attempts/1/job.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        archive["result"]["isolation_controls"]["namespaces"],
        serde_json::json!(["ipc", "user"])
    );
    daemon.restart();
    let (_, restarted) = isolation_run(&daemon, &["--queue", "g/q"], ISOLATION_LINKS);
    assert_ne!(restarted[0], service[0]);
    assert_eq!(restarted[1], service[1]);
    assert_ne!(restarted[2], service[2]);
}

#[test]
fn isolation_policy_remote_transport_stays_in_the_service_namespaces() {
    use std::os::unix::fs::PermissionsExt;
    let remote = Daemon::with_profile("isolation-remote-peer", "ordinary");
    let mut local = Daemon::with_profile("isolation-remote-local", "ordinary");
    let bin = local.base.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let seen = local.base.join("transport-namespaces");
    let ssh = bin.join("ssh");
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\nfor kind in uts user; do readlink /proc/self/ns/$kind; done >> '{}'\nexec env JOB_STATE_DIR='{}' '{}' remote\n",
            seen.display(),
            remote.state.display(),
            env!("CARGO_BIN_EXE_job")
        ),
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = format!("{}:/usr/bin:/bin", bin.display());
    local.child.kill().unwrap();
    local.child.wait().unwrap();
    local.child = Command::new(env!("CARGO_BIN_EXE_job"))
        .arg("daemon")
        .env("JOB_CGROUP_ROOT", local.base.join("absent-cgroup"))
        .env("JOB_CONFIG", local.base.join("config.toml"))
        .env("JOB_STATE_DIR", &local.state)
        .env("PATH", &path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    local.ready();
    let service: Vec<String> = ["uts", "user"]
        .iter()
        .map(|kind| {
            std::fs::read_link(format!("/proc/{}/ns/{kind}", local.child.id()))
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let submitted = local
        .job_command(&[
            "submit",
            "--on",
            "test-peer",
            "--namespaces",
            "user,uts",
            "--",
            "sh",
            "-c",
            "for kind in uts user; do readlink /proc/self/ns/$kind; done",
        ])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(
        submitted.status.success(),
        "{}",
        String::from_utf8_lossy(&submitted.stderr)
    );
    let id = text(&submitted).trim().to_owned();
    let waited = local.job(&["wait", &id]);
    assert!(
        waited.status.success(),
        "{}",
        String::from_utf8_lossy(&waited.stderr)
    );
    let payload = text(&local.job(&["logs", &id, "--raw", "--stream", "stdout"]));
    let payload: Vec<&str> = payload.lines().collect();
    assert_eq!(payload.len(), 2);
    assert_ne!(payload[0], service[0]);
    assert_ne!(payload[1], service[1]);
    let transport = std::fs::read_to_string(&seen).unwrap();
    assert!(transport.lines().count() >= 4, "{transport}");
    for (index, line) in transport.lines().enumerate() {
        assert_eq!(line, service[index % 2], "{transport}");
    }
    let record: serde_json::Value =
        serde_json::from_slice(&local.job(&["status", &id, "--json"]).stdout).unwrap();
    assert_eq!(
        record["result"]["isolation_controls"]["namespaces"],
        serde_json::json!(["user", "uts"])
    );
}

#[test]
fn isolation_policy_coexists_with_security_and_process_controls() {
    let daemon = Daemon::with_profile("isolation-coexistence", "ordinary");
    let (id, combined) = security_probe(
        &daemon,
        &[
            "--namespaces",
            "user,mount,uts",
            "--root",
            "read-only",
            "--seccomp-deny",
            "getppid",
            "--cap-drop",
            "all",
            "--rlimit",
            "core=0",
        ],
        "read",
    );
    assert_eq!(combined["seccomp"], "2");
    assert_eq!(combined["no_new_privs"], "1");
    assert_eq!(combined["getppid"], libc::EPERM);
    assert!(combined["getuid"].is_null());
    for set in [
        "effective",
        "permitted",
        "inheritable",
        "ambient",
        "bounding",
    ] {
        assert_eq!(combined[set], "0000000000000000", "{set}");
    }
    let saved = preset_record(&daemon, &id);
    assert_eq!(
        saved["result"]["isolation_controls"],
        serde_json::json!({"namespaces":["mount","user","uts"],"user_namespace_from_network":false,"root_read_only":true,"private_tmp":[],"writable":[]})
    );
    assert_eq!(
        saved["result"]["security_controls"]["cap_bounding_reduced"],
        true
    );
    assert_eq!(
        saved["result"]["security_controls"]["seccomp_deny"],
        "getppid"
    );
    assert_eq!(
        saved["result"]["process_controls"]["rlimits"]["core"],
        serde_json::json!({"soft":0,"hard":0})
    );
    let (_, denied) = isolation_run(
        &daemon,
        &[
            "--namespaces",
            "user,mount,uts",
            "--root",
            "read-only",
            "--cap-drop",
            "all",
        ],
        "touch denied 2>/dev/null && echo written; echo done",
    );
    assert_eq!(denied, ["done"]);
    assert!(!daemon.file("denied").exists());
    let area = isolation_area("confined");
    let writable = area.join("writable");
    let other = area.join("other");
    std::fs::create_dir(&writable).unwrap();
    std::fs::create_dir(&other).unwrap();
    let (_, confined) = isolation_run(
        &daemon,
        &[
            "--dir",
            area.to_str().unwrap(),
            "--confine",
            "--allow-write",
            writable.to_str().unwrap(),
            "--allow-write",
            other.to_str().unwrap(),
            "--namespaces",
            "user,mount",
            "--root",
            "read-only",
            "--private-tmp",
            "yes",
            "--writable",
            writable.to_str().unwrap(),
        ],
        &format!(
            "echo a > /tmp/inside && echo tmp-ok; echo b > '{}/yes' && echo writable-ok; touch '{}/no' 2>/dev/null && echo other-written; true",
            writable.display(),
            other.display()
        ),
    );
    assert_eq!(confined, ["tmp-ok", "writable-ok"]);
    assert!(writable.join("yes").exists());
    assert!(!other.join("no").exists());
    std::fs::remove_dir_all(&area).unwrap();
}

const QUOTA_LINES: usize = 450_000;

fn quota_expected(tag: char) -> Vec<u8> {
    (0..QUOTA_LINES)
        .flat_map(|line| format!("{tag}{line:08}\n").into_bytes())
        .collect()
}

fn quota_gap(notices: &str, stream: &str) -> Option<u64> {
    notices
        .lines()
        .find(|line| line.contains(&format!("stream={stream},")))
        .and_then(|line| line.split("bytes=").nth(1))
        .and_then(|rest| rest.split(';').next())
        .and_then(|bytes| bytes.parse().ok())
}

fn quota_head_and_tail(expected: &[u8], kept: &[u8]) -> (usize, usize) {
    let head = expected
        .iter()
        .zip(kept)
        .take_while(|(a, b)| a == b)
        .count();
    let tail = kept.len() - head;
    assert_eq!(kept[head..], expected[expected.len() - tail..]);
    (head, tail)
}

fn quota_logs(daemon: &Daemon, id: &str, stream: &str, name: &str) -> (Vec<u8>, String) {
    let file = std::fs::File::create(daemon.file(name)).unwrap();
    let output = daemon
        .job_command(&["logs", id, "--raw", "--stream", stream])
        .stdout(file)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (
        std::fs::read(daemon.file(name)).unwrap(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn output_quota_keeps_head_and_tail_of_each_stream_and_names_the_gap() {
    let daemon = Daemon::with_profile("output-quota-gap", "ordinary");
    let program = format!(
        "printf ready; i=0; while [ ! -e release ] && [ $i -lt 250 ]; do sleep .02; i=$((i+1)); done; awk 'BEGIN{{for(i=0;i<{QUOTA_LINES};i++){{printf \"o%08d\\n\", i; printf \"e%08d\\n\", i > \"/dev/stderr\"}}}}'"
    );
    let id = ordering_ok(
        &daemon,
        &[
            "submit",
            "--output-head",
            "1M",
            "--output-tail",
            "1M",
            "--",
            "sh",
            "-c",
            &program,
        ],
    );
    let follower = daemon
        .job_command(&["logs", &id, "--follow", "--raw", "--stream", "stdout"])
        .stdout(std::fs::File::create(daemon.file("followed")).unwrap())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let running = ordering_ok(&daemon, &["status", &id]);
    std::fs::write(daemon.file("release"), "").unwrap();
    assert!(daemon.job(&["wait", &id]).status.success());
    let followed = follower.wait_with_output().unwrap();
    assert!(
        followed.status.success(),
        "{}",
        String::from_utf8_lossy(&followed.stderr)
    );
    let mut expected_out = b"ready".to_vec();
    expected_out.extend(quota_expected('o'));
    let expected_err = quota_expected('e');
    let live = std::fs::read(daemon.file("followed")).unwrap();
    assert!(live.starts_with(b"readyo00000000\n"));
    assert!(live.ends_with(format!("o{:08}\n", QUOTA_LINES - 1).as_bytes()));
    assert!(
        live.len() == expected_out.len()
            || String::from_utf8_lossy(&followed.stderr).contains("stream=stdout"),
        "{}",
        String::from_utf8_lossy(&followed.stderr)
    );
    let (out, out_notices) = quota_logs(&daemon, &id, "stdout", "kept-out");
    let (err, err_notices) = quota_logs(&daemon, &id, "stderr", "kept-err");
    let (out_head, out_tail) = quota_head_and_tail(&expected_out, &out);
    let (err_head, err_tail) = quota_head_and_tail(&expected_err, &err);
    let out_gap = (expected_out.len() - out.len()) as u64;
    let err_gap = (expected_err.len() - err.len()) as u64;
    assert!(out_gap > 0 && err_gap > 0);
    assert!((900_000..=1_048_576).contains(&out_head), "{out_head}");
    assert!((900_000..=3 * 1_048_576).contains(&out_tail), "{out_tail}");
    assert!(err_head > 0 && err_head <= 1_048_576, "{err_head}");
    assert!(err_tail > 0 && err_tail <= 3 * 1_048_576, "{err_tail}");
    for notices in [&out_notices, &err_notices] {
        assert_eq!(quota_gap(notices, "stdout"), Some(out_gap), "{notices}");
        assert_eq!(quota_gap(notices, "stderr"), Some(err_gap), "{notices}");
        assert!(notices.contains("removed by retention"));
        assert!(
            notices.contains("kept per stream: at least the first 1 MiB and the last 1 MiB"),
            "{notices}"
        );
    }
    let record = preset_record(&daemon, &id);
    assert_eq!(record["spec"]["declared"]["output_head_bytes"], 1_048_576);
    assert_eq!(record["spec"]["declared"]["output_tail_bytes"], 1_048_576);
    assert_eq!(record["resource_sources"]["output_head_bytes"], "Job");
    assert_eq!(record["resource_sources"]["output_tail_bytes"], "Job");
    let retention = &record["result"]["output_retention"];
    assert_eq!(retention["head_bytes"], 1_048_576);
    assert_eq!(
        retention["streams"],
        serde_json::json!([
            {"stream":"stdout","written_bytes":expected_out.len(),"dropped_bytes":out_gap},
            {"stream":"stderr","written_bytes":expected_err.len(),"dropped_bytes":err_gap},
        ])
    );
    assert!(
        running.contains("output quota per stream: first 1 MiB (this Job), last 1 MiB (this Job)")
    );
    let status = ordering_ok(&daemon, &["status", &id]);
    assert!(
        status.contains(&format!(
            "removed by retention: stdout {out_gap} bytes, stderr {err_gap} bytes"
        )),
        "{status}"
    );
    assert!(ordering_ok(&daemon, &["attempts", &id]).contains("output quota per stream"));
    let json = daemon.job(&["logs", &id, "--json"]);
    let gaps: Vec<serde_json::Value> = text(&json)
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|record| record["stream"] == "gap")
        .collect();
    assert_eq!(gaps.len(), 2);
    assert!(gaps.iter().any(|gap| gap["gap"]
        == serde_json::json!({"stream":"stdout","records":gap["gap"]["records"],"bytes":out_gap,"quota":{"head_bytes":1_048_576,"tail_bytes":1_048_576}})));
    let dir = daemon.state.join("jobs").join(&id);
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("streams.json")).unwrap()).unwrap();
    assert_eq!(
        meta["quota"],
        serde_json::json!({"head_bytes":1_048_576,"tail_bytes":1_048_576})
    );
    assert!(std::fs::metadata(dir.join("output.log")).unwrap().len() < 2_200_000);
    let stored: u64 = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("streams-"))
        .map(|entry| entry.metadata().unwrap().len())
        .sum();
    assert!(stored <= 2 * (4 * 1_048_576), "{stored}");
}

#[test]
fn output_quota_absent_settings_keep_the_previous_recording_shape() {
    let daemon = Daemon::with_profile("output-quota-default", "ordinary");
    let id = ordering_ok(
        &daemon,
        &[
            "submit",
            "--",
            "sh",
            "-c",
            "printf first; head -c 70000000 /dev/zero; printf last; printf err >&2",
        ],
    );
    assert!(daemon.job(&["wait", &id]).status.success());
    let dir = daemon.state.join("jobs").join(&id);
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("streams.json")).unwrap()).unwrap();
    let mut keys: Vec<&str> = meta
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "attempt",
            "complete",
            "error",
            "job_id",
            "mode",
            "next_sequence",
            "omitted_bytes",
            "retired_bytes",
            "retired_records",
            "schema_version",
            "segments"
        ]
    );
    assert_eq!(meta["segments"].as_array().unwrap().len(), 64);
    assert!(
        meta["segments"]
            .as_array()
            .unwrap()
            .iter()
            .all(|segment| segment.as_object().unwrap().len() == 4)
    );
    let (kept, notices) = quota_logs(&daemon, &id, "stdout", "kept-default");
    let total = 70_000_000 + 9;
    let gap = (total - kept.len()) as u64;
    assert!(kept.starts_with(b"first\0") && kept.ends_with(b"\0last"));
    assert!(
        notices.contains(&format!(
            "bytes={gap}; kept for all streams together: the first 32 MiB and the last 32 MiB"
        )),
        "{notices}"
    );
    let record = preset_record(&daemon, &id);
    assert!(record["spec"]["declared"]["output_head_bytes"].is_null());
    assert!(record["resource_sources"]["output_head_bytes"].is_null());
    let retention = &record["result"]["output_retention"];
    assert!(retention["head_bytes"].is_null());
    assert_eq!(
        retention["streams"],
        serde_json::json!([
            {"stream":"stdout","written_bytes":total,"dropped_bytes":gap},
            {"stream":"stderr","written_bytes":3,"dropped_bytes":0},
        ])
    );
    let status = ordering_ok(&daemon, &["status", &id]);
    assert!(
        status.contains(&format!("removed by retention: stdout {gap} bytes")),
        "{status}"
    );
    let small = ordering_ok(&daemon, &["submit", "--", "printf", "small"]);
    assert!(daemon.job(&["wait", &small]).status.success());
    assert!(!ordering_ok(&daemon, &["status", &small]).contains("output quota"));
    assert!(!ordering_ok(&daemon, &["attempts", &small]).contains("output"));
}

#[test]
fn output_quota_defaults_overrides_unset_and_retry_keep_origins() {
    let daemon = Daemon::with_profile("output-quota-origins", "ordinary");
    std::fs::write(
        daemon.base.join("config.toml"),
        "schema_version = 1\nprofile = 'ordinary'\n[output]\nhead_bytes = '2M'\nbudget_bytes = 4294967296\n[[presets.profiles]]\nname = 'quiet'\nrevision = 1\n[presets.profiles.values]\noutput_tail_bytes = 6291456\n",
    )
    .unwrap();
    ordering_ok(&daemon, &["config", "reload"]);
    let shown: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["config", "show", "--json"])).unwrap();
    assert!(shown.to_string().contains("\"head_bytes\":2097152"));
    ordering_ok(
        &daemon,
        &["group", "create", "g", "--job-output-head", "5M"],
    );
    ordering_ok(
        &daemon,
        &["queue", "create", "g/q", "--job-output-tail", "3M"],
    );
    let id = ordering_ok(&daemon, &["submit", "--queue", "g/q", "--", "printf", "x"]);
    assert!(daemon.job(&["wait", &id]).status.success());
    let record = preset_record(&daemon, &id);
    assert_eq!(record["spec"]["declared"]["output_head_bytes"], 5 << 20);
    assert_eq!(record["spec"]["declared"]["output_tail_bytes"], 3 << 20);
    assert_eq!(
        record["resource_sources"]["output_head_bytes"]["Object"]["path"],
        "g"
    );
    assert_eq!(
        record["resource_sources"]["output_tail_bytes"]["Object"]["path"],
        "g/q"
    );
    let status = ordering_ok(&daemon, &["status", &id]);
    assert!(
        status.contains("output quota per stream: first 5 MiB (g), last 3 MiB (g/q)"),
        "{status}"
    );
    let own = ordering_ok(
        &daemon,
        &[
            "submit",
            "--queue",
            "g/q",
            "--output-tail",
            "4M",
            "--",
            "true",
        ],
    );
    assert!(daemon.job(&["wait", &own]).status.success());
    let record = preset_record(&daemon, &own);
    assert_eq!(record["spec"]["declared"]["output_tail_bytes"], 4 << 20);
    assert_eq!(record["resource_sources"]["output_tail_bytes"], "Job");
    assert_eq!(
        record["resource_sources"]["output_head_bytes"]["Object"]["path"],
        "g"
    );
    ordering_ok(&daemon, &["group", "unset", "g", "job-output-head"]);
    ordering_ok(&daemon, &["queue", "unset", "g/q", "job-output-tail"]);
    ordering_ok(&daemon, &["retry", &id]);
    assert!(daemon.job(&["wait", &id]).status.success());
    let record = preset_record(&daemon, &id);
    assert_eq!(record["spec"]["declared"]["output_head_bytes"], 2 << 20);
    assert_eq!(record["resource_sources"]["output_head_bytes"], "Service");
    assert!(record["spec"]["declared"]["output_tail_bytes"].is_null());
    assert!(record["resource_sources"]["output_tail_bytes"].is_null());
    let status = ordering_ok(&daemon, &["status", &id]);
    assert!(
        status.contains(
            "output quota per stream: first 2 MiB (service configuration), last 32 MiB (built-in)"
        ),
        "{status}"
    );
    let archive: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            daemon
                .state
                .join("jobs")
                .join(&id)
                .join("attempts/1/job.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        archive["resource_sources"]["output_tail_bytes"]["Object"]["path"],
        "g/q"
    );
    assert_eq!(archive["spec"]["declared"]["output_head_bytes"], 5 << 20);
    let profiled = ordering_ok(
        &daemon,
        &["submit", "--execution-profile", "quiet@1", "--", "true"],
    );
    assert!(daemon.job(&["wait", &profiled]).status.success());
    let record = preset_record(&daemon, &profiled);
    assert_eq!(record["spec"]["declared"]["output_tail_bytes"], 6 << 20);
    assert!(record["resource_sources"]["output_tail_bytes"]["Preset"].is_object());
    assert_eq!(record["resource_sources"]["output_head_bytes"], "Service");
    for refused in [
        vec!["submit", "--output-head", "4K", "--", "true"],
        vec!["submit", "--output-tail", "2G", "--", "true"],
        vec!["submit", "--output-head", "unlimited", "--", "true"],
        vec![
            "submit",
            "--output-head",
            "1M",
            "--output-head",
            "2M",
            "--",
            "true",
        ],
        vec!["queue", "create", "bad", "--job-output-tail", "1K"],
    ] {
        assert!(!daemon.job(&refused).status.success(), "{refused:?}");
    }
    std::fs::write(
        daemon.base.join("bad.toml"),
        "schema_version = 1\n[output]\ntail_bytes = '1K'\n",
    )
    .unwrap();
    assert!(
        !daemon
            .job(&[
                "config",
                "check",
                daemon.base.join("bad.toml").to_str().unwrap()
            ])
            .status
            .success()
    );
    std::fs::write(
        daemon.base.join("unknown.toml"),
        "schema_version = 1\n[output]\nunlimited = true\n",
    )
    .unwrap();
    assert!(
        !daemon
            .job(&[
                "config",
                "check",
                daemon.base.join("unknown.toml").to_str().unwrap()
            ])
            .status
            .success()
    );
}

#[test]
fn output_quota_recording_without_quota_metadata_still_reads() {
    let daemon = Daemon::with_profile("output-quota-old", "ordinary");
    let id = ordering_ok(
        &daemon,
        &[
            "submit",
            "--",
            "sh",
            "-c",
            "printf old-out; printf old-err >&2",
        ],
    );
    assert!(daemon.job(&["wait", &id]).status.success());
    let dir = daemon.state.join("jobs").join(&id);
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("streams.json")).unwrap()).unwrap();
    let segment = &written["segments"][0];
    let old = format!(
        "{{\n  \"schema_version\": 1,\n  \"job_id\": {id},\n  \"attempt\": 1,\n  \"mode\": \"pipe\",\n  \"segments\": [\n    {{\n      \"number\": 0,\n      \"first_sequence\": 0,\n      \"bytes\": {},\n      \"records\": {}\n    }}\n  ],\n  \"complete\": true,\n  \"error\": null,\n  \"omitted_bytes\": 0,\n  \"retired_bytes\": 0,\n  \"retired_records\": 0,\n  \"next_sequence\": {}\n}}",
        segment["bytes"], segment["records"], written["next_sequence"]
    );
    std::fs::write(dir.join("streams.json"), old).unwrap();
    let path = dir.join("job.json");
    let mut job: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    job["result"]
        .as_object_mut()
        .unwrap()
        .remove("output_retention");
    job["spec"]["declared"]
        .as_object_mut()
        .unwrap()
        .remove("output_head_bytes");
    job["spec"]["declared"]
        .as_object_mut()
        .unwrap()
        .remove("output_tail_bytes");
    std::fs::write(&path, serde_json::to_vec(&job).unwrap()).unwrap();
    assert_eq!(quota_logs(&daemon, &id, "stdout", "old-out").0, b"old-out");
    assert_eq!(quota_logs(&daemon, &id, "stderr", "old-err").0, b"old-err");
    assert!(!ordering_ok(&daemon, &["status", &id]).contains("output quota"));
    let mut unknown: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("streams.json")).unwrap()).unwrap();
    unknown["quota"] = serde_json::json!({"head_bytes":1,"tail_bytes":1});
    std::fs::write(
        dir.join("streams.json"),
        serde_json::to_vec(&unknown).unwrap(),
    )
    .unwrap();
    assert!(!daemon.job(&["logs", &id]).status.success());
}

#[test]
fn output_quota_service_budget_trims_oldest_completed_recordings_visibly() {
    let daemon = Daemon::with_profile("output-quota-budget", "ordinary");
    std::fs::write(
        daemon.base.join("config.toml"),
        "schema_version = 1\nprofile = 'ordinary'\n[output]\nbudget_bytes = '700K'\n",
    )
    .unwrap();
    ordering_ok(&daemon, &["config", "reload"]);
    let fill = "head -c 200000 /dev/zero | tr '\\0' a; head -c 50000 /dev/zero | tr '\\0' b >&2";
    let first = ordering_ok(&daemon, &["submit", "--", "sh", "-c", fill]);
    assert!(daemon.job(&["wait", &first]).status.success());
    let dir = |id: &str| daemon.state.join("jobs").join(id);
    let segments = |id: &str| {
        std::fs::read_dir(dir(id))
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("streams-"))
            .count()
    };
    assert_eq!(segments(&first), 1);
    assert_eq!(
        quota_logs(&daemon, &first, "stdout", "kept").0.len(),
        200_000
    );
    let second = ordering_ok(&daemon, &["submit", "--", "sh", "-c", fill]);
    assert!(daemon.job(&["wait", &second]).status.success());
    assert_eq!(segments(&first), 0);
    assert!(!dir(&first).join("output.log").exists());
    assert_eq!(segments(&second), 1);
    let trimmed = daemon.job(&["logs", &first, "--raw"]);
    assert!(
        trimmed.status.success(),
        "{}",
        String::from_utf8_lossy(&trimmed.stderr)
    );
    assert!(trimmed.stdout.is_empty());
    let record = preset_record(&daemon, &first);
    let bytes = record["result"]["output_retention"]["trimmed_bytes"]
        .as_u64()
        .unwrap();
    assert!(bytes > 500_000 && bytes < 520_000, "{bytes}");
    assert!(
        String::from_utf8_lossy(&trimmed.stderr).contains(&format!(
            "removed to keep the service within its output budget (bytes={bytes})"
        )),
        "{}",
        String::from_utf8_lossy(&trimmed.stderr)
    );
    assert!(ordering_ok(&daemon, &["status", &first]).contains(&format!(
        "the recording was removed by the service output budget ({bytes} bytes)"
    )));
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir(&first).join("streams.json")).unwrap()).unwrap();
    assert_eq!(meta["trimmed_bytes"], bytes);
    let live = ordering_ok(
        &daemon,
        &[
            "submit",
            "--",
            "sh",
            "-c",
            "head -c 400000 /dev/zero | tr '\\0' c; i=0; while [ ! -e release ] && [ $i -lt 500 ]; do sleep .02; i=$((i+1)); done; printf end",
        ],
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline
        && std::fs::metadata(dir(&live).join("output.log")).map_or(0, |file| file.len()) < 400_000
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    let last = ordering_ok(&daemon, &["submit", "--", "printf", "tiny"]);
    assert!(daemon.job(&["wait", &last]).status.success());
    assert_eq!(segments(&second), 0);
    assert!(
        preset_record(&daemon, &second)["result"]["output_retention"]["trimmed_bytes"]
            .as_u64()
            .is_some()
    );
    assert_eq!(segments(&live), 1);
    std::fs::write(daemon.file("release"), "").unwrap();
    assert!(daemon.job(&["wait", &live]).status.success());
}

#[test]
fn output_quota_remote_job_records_with_the_quota_on_both_hosts() {
    use std::os::unix::fs::PermissionsExt;
    let remote = Daemon::with_profile("output-quota-peer", "ordinary");
    let mut local = Daemon::with_profile("output-quota-local", "ordinary");
    let bin = local.base.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let ssh = bin.join("ssh");
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\nexec env JOB_STATE_DIR='{}' '{}' remote\n",
            remote.state.display(),
            env!("CARGO_BIN_EXE_job")
        ),
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = format!("{}:/usr/bin:/bin", bin.display());
    local.child.kill().unwrap();
    local.child.wait().unwrap();
    local.child = Command::new(env!("CARGO_BIN_EXE_job"))
        .arg("daemon")
        .env("JOB_CGROUP_ROOT", local.base.join("absent-cgroup"))
        .env("JOB_CONFIG", local.base.join("config.toml"))
        .env("JOB_STATE_DIR", &local.state)
        .env("PATH", &path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    local.ready();
    let submitted = local
        .job_command(&[
            "submit",
            "--on",
            "test-peer",
            "--output-head",
            "1M",
            "--output-tail",
            "1M",
            "--",
            "sh",
            "-c",
            "printf first; head -c 6000000 /dev/zero | tr '\\0' m; printf last; printf err >&2",
        ])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(
        submitted.status.success(),
        "{}",
        String::from_utf8_lossy(&submitted.stderr)
    );
    let id = text(&submitted).trim().to_owned();
    let waited = local.job(&["wait", &id]);
    assert!(
        waited.status.success(),
        "{} {}",
        text(&waited),
        String::from_utf8_lossy(&waited.stderr)
    );
    for (daemon, id) in [(&remote, "1"), (&local, id.as_str())] {
        let (kept, notices) = quota_logs(daemon, id, "stdout", "kept-remote");
        assert!(kept.starts_with(b"firstmmm") && kept.ends_with(b"mmmlast"));
        assert!(
            kept.len() > 2_000_000 && kept.len() < 6_000_000,
            "{}",
            kept.len()
        );
        assert!(notices.contains("removed by retention"), "{notices}");
        assert_eq!(
            quota_logs(daemon, id, "stderr", "kept-remote-err").0,
            b"err"
        );
        let record = preset_record(daemon, id);
        assert_eq!(record["spec"]["declared"]["output_head_bytes"], 1_048_576);
        assert_eq!(record["resource_sources"]["output_tail_bytes"], "Job");
        let meta: serde_json::Value = serde_json::from_slice(
            &std::fs::read(daemon.state.join("jobs").join(id).join("streams.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(meta["quota"]["tail_bytes"], 1_048_576);
    }
    let peer = quota_logs(&remote, "1", "stdout", "peer").0;
    let here = quota_logs(&local, &id, "stdout", "here").0;
    assert!(here.len() <= peer.len());
}

const NETPROBE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/netprobe.py");
const SOCKS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/socks5.py");
const LINK_TOOLS: [&str; 9] = [
    "python3",
    "curl",
    "slirp4netns",
    "nft",
    "tc",
    "ip",
    "unshare",
    "nsenter",
    "sleep",
];

fn network_boundary_missing(tools: &[&str]) -> Option<String> {
    if let Some(missing) = tools.iter().find(|t| !on_path(t)) {
        return Some(format!("{missing} is not installed"));
    }
    let namespaces = Command::new("unshare")
        .args(["--user", "--map-root-user", "--net", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    (!namespaces).then(|| "this host does not let a user create namespaces".to_string())
}

struct Helper(Child);

impl Helper {
    fn start(program: &str, args: &[&str], dir: &Path) -> Helper {
        Helper(
            Command::new(program)
                .args(args)
                .current_dir(dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    fn stop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.stop();
    }
}

fn appears(path: &Path, seconds: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn listens(address: &str, port: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(format!("{address}:{port}")).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("nothing listens on {address}:{port}");
}

fn web_server(daemon: &Daemon, bind: &str, port: &str) -> Helper {
    std::fs::write(daemon.file("page.txt"), "served from outside\n").unwrap();
    let helper = Helper::start(
        "python3",
        &["-m", "http.server", "--bind", bind, port],
        &daemon.work,
    );
    listens(if bind == "::1" { "[::1]" } else { bind }, port);
    helper
}

fn socks_proxy(daemon: &Daemon, port: &str) -> (Helper, PathBuf) {
    let log = daemon.file(&format!("socks-{port}.log"));
    let helper = Helper::start(
        "python3",
        &[SOCKS, port, log.to_str().unwrap(), "60"],
        &daemon.work,
    );
    listens("127.0.0.1", port);
    (helper, log)
}

fn udp_listener(daemon: &Daemon, port: &str) -> (Helper, PathBuf) {
    let count = daemon.file(&format!("udp-{port}.count"));
    let helper = Helper::start(
        "python3",
        &[NETPROBE, "listen", port, count.to_str().unwrap(), "30"],
        &daemon.work,
    );
    assert!(appears(&count, 5), "the UDP listener did not start");
    (helper, count)
}

fn received(count: &Path) -> u64 {
    std::fs::read_to_string(count)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

fn record(daemon: &Daemon, id: &str) -> serde_json::Value {
    serde_json::from_slice(&daemon.job(&["status", id, "--json"]).stdout).unwrap()
}

#[test]
fn network_boundary_no_network_leaves_only_loopback_without_ipv4_ipv6_or_dns() {
    if let Some(reason) = network_boundary_missing(&["python3", "curl", "unshare"]) {
        eprintln!("skipped: {reason}");
        return;
    }
    let daemon = Daemon::start("boundary-none");
    let web = free_port(10);
    let web6 = free_port(11);
    let _web = web_server(&daemon, "127.0.0.1", &web);
    let _web6 = web_server(&daemon, "::1", &web6);
    let answer = text(&daemon.job(&[
        "run",
        "--net",
        "none",
        "--",
        &format!(
            "echo interfaces=$(tail -n +3 /proc/net/dev | cut -d: -f1 | tr -d ' ' | tr '\\n' ' '); \
             curl -sS -m 2 http://127.0.0.1:{web}/page.txt; echo v4-host=$?; \
             curl -sS -m 2 http://192.0.2.1/; echo v4-out=$?; \
             curl -g -sS -m 2 'http://[::1]:{web6}/page.txt'; echo v6-host=$?; \
             curl -g -sS -m 2 'http://[2001:db8::1]/'; echo v6-out=$?; \
             python3 {NETPROBE} resolver"
        ),
    ]));
    assert!(answer.contains("interfaces=lo\n"), "{answer}");
    assert!(!answer.contains("served from outside"), "{answer}");
    for expected in [
        "v4-host=7",
        "v4-out=7",
        "v6-host=7",
        "v6-out=7",
        "dns=closed",
    ] {
        assert!(answer.contains(expected), "{expected} is missing: {answer}");
    }
}

#[test]
fn network_boundary_a_proxy_job_has_no_ipv6_udp_icmp_or_dns_of_its_own() {
    if let Some(reason) = network_boundary_missing(&LINK_TOOLS) {
        eprintln!("skipped: {reason}");
        return;
    }
    let daemon = Daemon::start("boundary-proxy");
    let web = free_port(12);
    let port = free_port(13);
    let udp = free_port(14);
    let _web = web_server(&daemon, "127.0.0.1", &web);
    let (_proxy, log) = socks_proxy(&daemon, &port);
    let (_listener, count) = udp_listener(&daemon, &udp);
    let answer = text(&daemon.job(&[
        "run",
        "--net",
        &format!("socks5://127.0.0.1:{port}"),
        "--",
        &format!(
            "cat /etc/resolv.conf; \
             curl -sS -m 5 http://localhost:{web}/page.txt; echo through=$?; \
             curl --noproxy '*' -sS -m 2 http://198.18.0.2:{web}/page.txt; echo around=$?; \
             curl --noproxy '*' -g -sS -m 2 'http://[2001:db8::1]/'; echo v6=$?; \
             echo v6-routes=$(ip -6 route show default | wc -l); \
             python3 {NETPROBE} send 198.18.0.2 {udp} 5 100; \
             python3 {NETPROBE} ask 198.18.0.3; \
             ping -c 1 -W 2 198.18.0.2 >/dev/null 2>&1; echo ping=$?"
        ),
    ]));
    std::thread::sleep(Duration::from_millis(300));
    assert!(answer.contains("nameserver 127.0.0.1\n"), "{answer}");
    assert!(
        answer.contains("served from outside\nthrough=0"),
        "{answer}"
    );
    for expected in ["around=7", "v6=7", "v6-routes=0", "udp-sent=", "dns=closed"] {
        assert!(answer.contains(expected), "{expected} is missing: {answer}");
    }
    assert!(!answer.contains("ping=0"), "{answer}");
    assert_eq!(received(&count), 0, "UDP left the job: {answer}");
    let seen = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        seen.contains(&format!("CONNECT localhost:{web}")),
        "the proxy did not resolve the name: {seen}"
    );
}

#[test]
fn network_boundary_a_proxy_that_dies_leaves_its_job_without_network() {
    if let Some(reason) = network_boundary_missing(&LINK_TOOLS) {
        eprintln!("skipped: {reason}");
        return;
    }
    let daemon = Daemon::start("boundary-proxy-dies");
    let web = free_port(15);
    let port = free_port(16);
    let _web = web_server(&daemon, "127.0.0.1", &web);
    let (mut proxy, _log) = socks_proxy(&daemon, &port);
    let id = text(&daemon.job(&[
        "submit",
        "--net",
        &format!("socks5://127.0.0.1:{port}"),
        "--",
        &format!(
            "curl -sS -m 5 http://127.0.0.1:{web}/page.txt; echo before=$?; touch reached; \
             n=0; while [ ! -e gone ] && [ $n -lt 100 ]; do sleep 0.1; n=$((n+1)); done; \
             curl -sS -m 3 http://127.0.0.1:{web}/page.txt; echo after=$?; \
             curl --noproxy '*' -sS -m 2 http://198.18.0.2:{web}/page.txt; echo around=$?"
        ),
    ]))
    .trim()
    .to_string();
    assert!(
        appears(&daemon.file("reached"), 15),
        "the job did not start"
    );
    proxy.stop();
    std::fs::write(daemon.file("gone"), "").unwrap();
    daemon.job(&["wait", &id, "--timeout", "30s"]);
    let log = text(&daemon.job(&["log", &id, "full"]));
    assert!(log.contains("served from outside\nbefore=0"), "{log}");
    assert_eq!(log.matches("served from outside").count(), 1, "{log}");
    assert!(log.contains("after=") && !log.contains("after=0"), "{log}");
    assert!(log.contains("around=7"), "{log}");
}

#[test]
fn network_boundary_a_bandwidth_holds_udp_as_well_as_tcp() {
    if let Some(reason) = network_boundary_missing(&LINK_TOOLS)
        .or_else(|| (!on_path("iperf3")).then(|| "iperf3 is not installed".to_string()))
    {
        eprintln!("skipped: {reason}");
        return;
    }
    let daemon = Daemon::start("boundary-udp-rate");
    daemon.job(&["queue", "add", "slow", "--bandwidth", "2Mbit"]);
    let port = free_port(17);
    let udp = free_port(18);
    let _server = Helper::start(
        "iperf3",
        &["-s", "-1", "-B", "127.0.0.1", "-p", &port],
        &daemon.work,
    );
    std::thread::sleep(Duration::from_millis(300));
    let (_listener, count) = udp_listener(&daemon, &udp);
    let answer = text(&daemon.job(&[
        "run",
        "-q",
        "slow",
        "--",
        &format!(
            "echo v6-routes=$(ip -6 route show default | wc -l) >&2; \
             python3 {NETPROBE} send 198.18.0.2 {udp} 5 100 >&2; \
             iperf3 -c 198.18.0.2 -p {port} -u -b 8M -l 1200 -t 4 -J"
        ),
    ]));
    daemon.job(&["queue", "rm", "slow"]);
    assert!(
        answer.contains("network capped at 2 Mbit/s each way"),
        "{answer}"
    );
    assert_eq!(received(&count), 500, "UDP did not pass the shaped link");
    let log = daemon.job(&["log", "1", "full"]);
    let body = String::from_utf8_lossy(&log.stdout);
    assert!(body.contains("v6-routes=0\n"), "{body}");
    let report: serde_json::Value = serde_json::from_str(&body[body.find('{').unwrap_or(0)..])
        .unwrap_or_else(|e| panic!("{e}: {body}"));
    let rate = |part: &str| {
        report["end"][part]["bits_per_second"]
            .as_f64()
            .unwrap_or_else(|| panic!("{report}"))
    };
    let (sent, arrived) = (rate("sum_sent"), rate("sum_received"));
    assert!(sent > 6_000_000.0, "sent only {sent} bit/s");
    assert!(
        (1_000_000.0..=2_100_000.0).contains(&arrived),
        "{arrived} bit/s of UDP arrived of {sent} sent"
    );
}

#[test]
fn network_boundary_jobs_of_one_queue_share_a_holder_and_cannot_reach_each_other() {
    if let Some(reason) = network_boundary_missing(&LINK_TOOLS) {
        eprintln!("skipped: {reason}");
        return;
    }
    let daemon = Daemon::start("boundary-shared");
    daemon.job(&[
        "queue",
        "add",
        "pair",
        "--parallel",
        "all",
        "--bandwidth",
        "100Mbit",
    ]);
    let port = free_port(19);
    let first = text(&daemon.job(&[
        "submit",
        "-q",
        "pair",
        "--",
        &format!(
            "ip -4 -o addr show dev eth0 | awk '{{print $4}}' | cut -d/ -f1 > a.v4; \
             ip -6 -o addr show dev eth0 scope link | awk '{{print $4}}' | cut -d/ -f1 > a.v6; \
             echo neighbour > served.txt; \
             python3 -m http.server --bind :: {port} >/dev/null 2>&1 & \
             n=0; until curl -sS -m 2 http://127.0.0.1:{port}/served.txt > a.self 2>/dev/null || [ $n -ge 30 ]; do sleep 0.2; n=$((n+1)); done; \
             touch a.ready; \
             n=0; while [ ! -e b.done ] && [ $n -lt 150 ]; do sleep 0.1; n=$((n+1)); done; \
             kill $!"
        ),
    ]))
    .trim()
    .to_string();
    assert!(
        appears(&daemon.file("a.ready"), 20),
        "the first job did not start"
    );
    let second = daemon.job(&[
        "run",
        "-q",
        "pair",
        "--",
        &format!(
            "echo own=$(ip -4 -o addr show dev eth0 | awk '{{print $4}}' | cut -d/ -f1); \
             curl -sS -m 2 http://$(cat a.v4):{port}/served.txt; echo peer4=$?; \
             curl -g -sS -m 2 \"http://[$(cat a.v6)%25eth0]:{port}/served.txt\"; echo peer6=$?"
        ),
    ]);
    let answer = text(&second);
    let records = [record(&daemon, &first), record(&daemon, "2")];
    std::fs::write(daemon.file("b.done"), "").unwrap();
    daemon.job(&["wait", &first, "--timeout", "30s"]);
    daemon.job(&["queue", "rm", "pair"]);
    assert_eq!(
        std::fs::read_to_string(daemon.file("a.self")).unwrap_or_default(),
        "neighbour\n",
        "the first job's server did not answer the job itself"
    );
    assert_eq!(records[0]["link"]["name"], "pair");
    assert_eq!(records[1]["link"]["name"], "pair");
    assert_eq!(
        std::fs::read_to_string(daemon.file("a.v4")).unwrap().trim(),
        "198.19.0.2"
    );
    assert!(answer.contains("own=198.19.0.3"), "{answer}");
    assert!(!answer.contains("neighbour"), "{answer}");
    assert!(
        answer.contains("peer4=") && !answer.contains("peer4=0"),
        "{answer}"
    );
    assert!(
        answer.contains("peer6=") && !answer.contains("peer6=0"),
        "{answer}"
    );
}

#[test]
fn network_boundary_missing_tools_refuse_the_job_with_their_names() {
    let daemon = Daemon::with_path("boundary-tools", "/nonexistent-job-tools");
    for options in [
        vec!["--bandwidth", "1Mbit"],
        vec!["--net", "socks5://127.0.0.1:9"],
    ] {
        let mut args = vec!["submit"];
        args.extend(options);
        args.extend(["--", "true"]);
        let refused = daemon.job(&args);
        let said = String::from_utf8_lossy(&refused.stderr);
        assert_ne!(refused.status.code(), Some(0), "{said}");
        assert!(
            said.contains(
                "this host cannot give a job its own network: it lacks unshare, nsenter, sleep, ip, tc, nft, slirp4netns"
            ),
            "{said}"
        );
    }
    let plain = daemon.job(&["queue"]);
    assert!(!text(&plain).contains("true"), "{}", text(&plain));
}

fn files_holding(root: &Path, needle: &str, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            files_holding(&path, needle, found);
        } else if kind.is_file()
            && std::fs::read(&path).is_ok_and(|bytes| {
                bytes
                    .windows(needle.len())
                    .any(|window| window == needle.as_bytes())
            })
        {
            found.push(path);
        }
    }
}

fn command_lines_holding(needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if let Ok(bytes) = std::fs::read(entry.path().join("cmdline")) {
            let line = String::from_utf8_lossy(&bytes).replace('\0', " ");
            if line.contains(needle) {
                found.push(format!("{} {line}", name.to_string_lossy()));
            }
        }
    }
    found
}

#[test]
fn network_boundary_proxy_credentials_stay_out_of_answers_records_and_arguments() {
    if let Some(reason) = network_boundary_missing(&LINK_TOOLS) {
        eprintln!("skipped: {reason}");
        return;
    }
    let mark = ["SECRET", "MARK"].concat();
    let daemon = Daemon::start("boundary-credentials");
    let port = free_port(20);
    let (_proxy, _log) = socks_proxy(&daemon, &port);
    let url = format!("socks5://user:{mark}@127.0.0.1:{port}");
    let shown = format!("socks5://***@127.0.0.1:{port}");
    let check = "half=MARK; case \"$ALL_PROXY\" in socks5h://user:SECRET$half@198.18.0.2:*) echo own=yes;; *) echo own=no;; esac";
    let hold = |name: &str| {
        format!(
            "{check}; touch {name}.reached; n=0; while [ ! -e {name}.release ] && [ $n -lt 150 ]; do sleep 0.1; n=$((n+1)); done"
        )
    };
    let mut said = Vec::new();
    let submitted = daemon.job(&["submit", "--net", &url, "--", &hold("alone")]);
    let alone = text(&submitted).trim().to_string();
    said.push(submitted);
    let added = daemon.job(&["queue", "add", "vault", "--parallel", "all", "--net", &url]);
    assert!(text(&added).contains(&shown), "{}", text(&added));
    said.push(added);
    let submitted = daemon.job(&["submit", "-q", "vault", "--", &hold("queued")]);
    let queued = text(&submitted).trim().to_string();
    said.push(submitted);
    assert!(
        appears(&daemon.file("alone.reached"), 20),
        "job {alone} did not start"
    );
    assert!(
        appears(&daemon.file("queued.reached"), 20),
        "job {queued} did not start"
    );
    let running = command_lines_holding(&mark);
    for args in [
        vec!["status", alone.as_str(), "--json"],
        vec!["status", queued.as_str(), "--json"],
        vec!["status", alone.as_str()],
        vec!["queue"],
        vec!["queue", "vault"],
        vec!["queue", "show", "vault", "--json"],
        vec!["queue", "list", "--json"],
        vec!["host"],
        vec!["host", "--json"],
    ] {
        said.push(daemon.job(&args));
    }
    let refused = daemon.job(&[
        "submit",
        "-q",
        "vault",
        "--net",
        &format!("socks5://other:{mark}@127.0.0.1:1"),
        "--",
        "true",
    ]);
    assert_ne!(refused.status.code(), Some(0));
    said.push(refused);
    said.push(daemon.job(&[
        "run",
        "--net",
        &format!("socks5://user:{mark}@nowhere"),
        "--",
        "true",
    ]));
    let far = daemon.job(&[
        "submit",
        "--on",
        "nobody@127.0.0.1",
        "--ssh-option",
        "ConnectTimeout=1",
        "--net",
        &url,
        "--",
        "true",
    ]);
    let far_id = text(&far).trim().to_string();
    said.push(far);
    daemon.job(&["wait", &far_id, "--timeout", "30s"]);
    said.push(daemon.job(&["status", &far_id]));
    said.push(daemon.job(&["status", &far_id, "--json"]));
    std::fs::write(daemon.file("alone.release"), "").unwrap();
    std::fs::write(daemon.file("queued.release"), "").unwrap();
    for id in [&alone, &queued] {
        daemon.job(&["wait", id, "--timeout", "30s"]);
        said.push(daemon.job(&["status", id]));
        said.push(daemon.job(&["status", id, "--json"]));
        let log = text(&daemon.job(&["log", id, "full"]));
        assert!(
            log.contains("own=yes"),
            "job {id} did not get its own credentials: {log}"
        );
    }
    assert!(
        running.is_empty(),
        "process arguments held the password: {running:?}"
    );
    let mut shown_somewhere = false;
    for output in &said {
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!all.contains(&mark), "{all}");
        shown_somewhere |= all.contains(&shown);
    }
    assert!(shown_somewhere, "no answer named the proxy at all");
    let declared = &record(&daemon, &alone)["spec"]["declared"];
    assert_eq!(declared["net"]["Proxy"], shown.as_str());
    let private = PathBuf::from(declared["net_secret_file"].as_str().unwrap());
    assert!(private.starts_with(&daemon.state), "{}", private.display());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&private).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read_to_string(&private).unwrap(),
        format!("user:{mark}\n")
    );
    let shared = PathBuf::from(
        record(&daemon, &queued)["spec"]["declared"]["net_secret_file"]
            .as_str()
            .unwrap(),
    );
    assert!(
        shared.starts_with(daemon.state.join("net-secrets")),
        "{}",
        shared.display()
    );
    let retried = daemon.job(&["retry", &alone]);
    assert_eq!(
        retried.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&retried.stderr)
    );
    daemon.job(&["wait", &alone, "--timeout", "30s"]);
    let again = record(&daemon, &alone);
    assert_eq!(again["attempt"], 2);
    let log = text(&daemon.job(&["log", &alone, "full"]));
    assert!(
        log.contains("own=yes"),
        "the retry lost its credentials: {log}"
    );
    for output in [&retried, &daemon.job(&["status", &alone, "--json"])] {
        assert!(!text(output).contains(&mark), "{}", text(output));
    }
    let mut holding = Vec::new();
    files_holding(&daemon.state, &mark, &mut holding);
    assert!(
        holding.contains(&private) && holding.contains(&shared),
        "{holding:?}"
    );
    daemon.job(&["queue", "rm", "vault"]);
    for path in &holding {
        assert!(
            path.file_name().is_some_and(|name| name == "net-secret")
                || path.starts_with(daemon.state.join("net-secrets")),
            "{} holds the password",
            path.display()
        );
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o077,
            0,
            "{}",
            path.display()
        );
    }
}

#[test]
fn network_boundary_a_secret_file_is_checked_and_only_its_path_is_recorded() {
    if let Some(reason) = network_boundary_missing(&LINK_TOOLS) {
        eprintln!("skipped: {reason}");
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let mark = ["SECRET", "MARK"].concat();
    let daemon = Daemon::start("boundary-secret-file");
    let port = free_port(21);
    let (_proxy, _log) = socks_proxy(&daemon, &port);
    let url = format!("socks5://127.0.0.1:{port}");
    let file = daemon.file("proxy.secret");
    std::fs::write(&file, format!("user:{mark}\n")).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let open = daemon.job(&[
        "run",
        "--net",
        &url,
        "--net-secret-file",
        "proxy.secret",
        "--",
        "true",
    ]);
    let said = String::from_utf8_lossy(&open.stderr);
    assert_ne!(open.status.code(), Some(0), "{said}");
    assert!(
        said.contains("can be read by its group or by others; run: chmod 600"),
        "{said}"
    );
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let both = daemon.job(&[
        "run",
        "--net",
        &format!("socks5://user:{mark}@127.0.0.1:{port}"),
        "--net-secret-file",
        "proxy.secret",
        "--",
        "true",
    ]);
    let said = String::from_utf8_lossy(&both.stderr);
    assert!(
        said.contains("either inside --net or in --net-secret-file, not both"),
        "{said}"
    );
    assert!(!said.contains(&mark), "{said}");
    let without = daemon.job(&["run", "--net-secret-file", "proxy.secret", "--", "true"]);
    let said = String::from_utf8_lossy(&without.stderr);
    assert!(
        said.contains("--net-secret-file belongs to a proxy"),
        "{said}"
    );
    let ran = daemon.job(&[
        "run",
        "--net",
        &url,
        "--net-secret-file",
        "proxy.secret",
        "--",
        "half=MARK; case \"$ALL_PROXY\" in socks5h://user:SECRET$half@198.18.0.2:*) echo own=yes;; *) echo own=no;; esac",
    ]);
    let answer = text(&ran);
    assert!(answer.contains("own=yes"), "{answer}");
    assert!(!answer.contains(&mark), "{answer}");
    let id = answer
        .split("[job] job ")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .unwrap()
        .to_string();
    let job = record(&daemon, &id);
    assert_eq!(
        job["spec"]["declared"]["net_secret_file"],
        file.to_str().unwrap()
    );
    assert_eq!(job["spec"]["declared"]["net"]["Proxy"], url.as_str());
    let mut holding = Vec::new();
    files_holding(&daemon.state, &mark, &mut holding);
    assert!(holding.is_empty(), "{holding:?}");
    let added = daemon.job(&[
        "queue",
        "add",
        "vault",
        "--net",
        &url,
        "--net-secret-file",
        "proxy.secret",
    ]);
    assert_eq!(
        added.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let inherited = text(&daemon.job(&[
        "run",
        "-q",
        "vault",
        "--",
        "half=MARK; case \"$ALL_PROXY\" in socks5h://user:SECRET$half@198.18.0.2:*) echo own=yes;; *) echo own=no;; esac",
    ]));
    assert!(inherited.contains("own=yes"), "{inherited}");
    let changed = daemon.job(&["queue", "set", "vault", "--net", &url]);
    assert_eq!(
        changed.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&changed.stderr)
    );
    let dropped = text(&daemon.job(&["run", "-q", "vault", "--", "echo proxy=$ALL_PROXY"]));
    assert!(
        dropped.contains(&format!("proxy=socks5h://198.18.0.2:{port}")),
        "{dropped}"
    );
    daemon.job(&["queue", "rm", "vault"]);
    holding.clear();
    files_holding(&daemon.state, &mark, &mut holding);
    assert!(holding.is_empty(), "{holding:?}");
}

#[test]
fn network_boundary_the_hook_logs_a_denied_command_without_proxy_credentials() {
    let daemon = Daemon::start("boundary-hook");
    let mark = ["SECRET", "MARK"].concat();
    let event = serde_json::json!({
        "caller": "boundary",
        "cwd": daemon.work,
        "command": format!(
            "job run --net socks5://user:{mark}@127.0.0.1:1080 -- true; {} job",
            ["p", "kill"].concat()
        )
    });
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/policy.json"),
        daemon.base.join("policy.json"),
    )
    .unwrap();
    let mut child = daemon
        .job_command(&["hook"])
        .env("JOB_CONFIG", daemon.base.join("config.toml"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(event.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let answer = text(&output);
    assert!(answer.contains(r#""decision":"deny""#), "{answer}");
    assert!(answer.contains(r#""rule":"kill-by-pattern""#), "{answer}");
    let logged = std::fs::read_to_string(daemon.state.join("denials.jsonl")).unwrap();
    assert!(
        logged.contains("socks5://***@127.0.0.1:1080 -- true"),
        "{logged}"
    );
    assert!(!logged.contains(&mark), "{logged}");
}

fn wireguard_keys() -> Option<Vec<(String, String)>> {
    let made = Command::new("python3")
        .args([
            "-c",
            r#"import base64
from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey
from cryptography.hazmat.primitives import serialization as s
for _ in range(2):
    k = X25519PrivateKey.generate()
    private = k.private_bytes(s.Encoding.Raw, s.PrivateFormat.Raw, s.NoEncryption())
    public = k.public_key().public_bytes(s.Encoding.Raw, s.PublicFormat.Raw)
    print(base64.b64encode(private).decode(), base64.b64encode(public).decode())
"#,
        ])
        .output()
        .ok()?;
    let keys: Vec<(String, String)> = String::from_utf8_lossy(&made.stdout)
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(private, public)| (private.to_string(), public.to_string()))
        .collect();
    (made.status.success() && keys.len() == 2).then_some(keys)
}

fn in_namespace(holder: u32, script: &str) -> Output {
    Command::new("nsenter")
        .args([
            "-t",
            &holder.to_string(),
            "-U",
            "-n",
            "--preserve-credentials",
            "--",
            "sh",
            "-e",
            "-c",
            script,
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn network_boundary_a_wireguard_job_reaches_only_its_tunnel_and_nothing_once_the_peer_is_gone() {
    let mut tools = LINK_TOOLS.to_vec();
    tools.push("ss");
    if let Some(reason) = network_boundary_missing(&tools) {
        eprintln!("skipped: {reason}");
        return;
    }
    let Some(keys) = wireguard_keys() else {
        eprintln!("skipped: python3 cannot make X25519 keys (no cryptography module)");
        return;
    };
    let (server, client) = (&keys[0], &keys[1]);
    let daemon = Daemon::start("boundary-wireguard");
    let job = env!("CARGO_BIN_EXE_job");
    let web = free_port(22);
    let outer = free_port(23);
    let peer = Helper::start(
        "unshare",
        &["--user", "--map-root-user", "--net", "sleep", "60"],
        &daemon.work,
    );
    let holder = peer.0.id();
    let own = std::fs::read_link("/proc/self/ns/net").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !(std::fs::read_link(format!("/proc/{holder}/ns/net")).is_ok_and(|net| net != own)
        && std::fs::read_link(format!("/proc/{holder}/exe"))
            .is_ok_and(|exe| exe.ends_with("sleep")))
    {
        assert!(
            Instant::now() < deadline,
            "the peer's namespace did not appear"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let api = daemon.base.join("slirp.sock");
    let relay = Helper::start(
        "slirp4netns",
        &[
            "--configure",
            "--mtu=1500",
            "--api-socket",
            api.to_str().unwrap(),
            &holder.to_string(),
            "tap0",
        ],
        &daemon.work,
    );
    assert!(appears(&api, 5), "slirp4netns gave no control socket");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !in_namespace(holder, "ip -4 addr show dev tap0 | grep -q inet")
        .status
        .success()
    {
        assert!(
            Instant::now() < deadline,
            "the peer's namespace got no address"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    std::fs::write(
        daemon.file("peer.conf"),
        format!(
            "[Interface]\nPrivateKey = {}\nAddress = 10.9.0.1/24\n[Peer]\nPublicKey = {}\nAllowedIPs = 10.9.0.2/32\nEndpoint = 127.0.0.1:9\n",
            server.0, client.1
        ),
    )
    .unwrap();
    let made = in_namespace(
        holder,
        &format!(
            "ip link add wg0 type wireguard\n{job} link-wireguard wg0 {} -\nip addr add 10.9.0.1/24 dev wg0\nip link set wg0 up\nss -Hlun",
            daemon.file("peer.conf").display()
        ),
    );
    if !made.status.success() {
        eprintln!(
            "skipped: this host cannot make a WireGuard interface in a user's namespace: {}",
            String::from_utf8_lossy(&made.stderr).trim()
        );
        return;
    }
    let listening = text(&made);
    let inner: u16 = listening
        .lines()
        .filter_map(|line| line.split_whitespace().nth(3))
        .filter_map(|local| local.rsplit(':').next())
        .find_map(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("the peer listens nowhere: {listening}"));
    let mut control = UnixStream::connect(&api).unwrap();
    control
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let request = serde_json::json!({
        "execute": "add_hostfwd",
        "arguments": {
            "proto": "udp",
            "host_addr": "127.0.0.1",
            "host_port": outer.parse::<u16>().unwrap(),
            "guest_addr": "10.0.2.100",
            "guest_port": inner
        }
    });
    control.write_all(request.to_string().as_bytes()).unwrap();
    let mut reply = [0u8; 256];
    let got = control.read(&mut reply).unwrap();
    let reply = String::from_utf8_lossy(&reply[..got]).to_string();
    assert!(reply.contains("return"), "{reply}");
    std::fs::write(daemon.file("page.txt"), "served through the tunnel\n").unwrap();
    let served = Command::new("nsenter")
        .args([
            "-t",
            &holder.to_string(),
            "-U",
            "-n",
            "--preserve-credentials",
            "--",
        ])
        .args(["python3", "-m", "http.server", "--bind", "10.9.0.1", &web])
        .current_dir(&daemon.work)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut served = Helper(served);
    let direct = web_server(&daemon, "127.0.0.1", &web);
    std::fs::write(daemon.file("page.txt"), "served through the tunnel\n").unwrap();
    let config = daemon.file("it's a tunnel.conf");
    std::fs::write(
        &config,
        format!(
            "[Interface]\nPrivateKey = {}\nAddress = 10.9.0.2/24\nDNS = 10.9.0.1\n[Peer]\nPublicKey = {}\nAllowedIPs = 0.0.0.0/0\nEndpoint = 198.18.0.2:{outer}\n",
            client.0, server.1
        ),
    )
    .unwrap();
    let submitted = daemon.job(&[
        "submit",
        "--net",
        "wireguard:it's a tunnel.conf",
        "--",
        &format!(
            "cat /etc/resolv.conf; echo v6-routes=$(ip -6 route show default | wc -l); \
             n=0; until curl -sS -m 2 http://10.9.0.1:{web}/page.txt || [ $n -ge 5 ]; do sleep 0.3; n=$((n+1)); done; \
             curl -sS -m 2 -o /dev/null http://10.9.0.1:{web}/page.txt; echo before=$?; \
             curl -sS -m 2 http://198.18.0.2:{web}/page.txt; echo around=$?; \
             touch reached; \
             n=0; while [ ! -e gone ] && [ $n -lt 100 ]; do sleep 0.1; n=$((n+1)); done; \
             curl -sS -m 2 http://10.9.0.1:{web}/page.txt; echo after=$?; \
             curl -sS -m 2 http://198.18.0.2:{web}/page.txt; echo still-around=$?"
        ),
    ]);
    let id = text(&submitted).trim().to_string();
    assert!(
        appears(&daemon.file("reached"), 25),
        "the job did not start: {}",
        String::from_utf8_lossy(&submitted.stderr)
    );
    let helpers = command_lines_holding(&client.0);
    served.stop();
    drop(relay);
    drop(peer);
    std::fs::write(daemon.file("gone"), "").unwrap();
    daemon.job(&["wait", &id, "--timeout", "30s"]);
    drop(direct);
    let log = text(&daemon.job(&["log", &id, "full"]));
    assert!(log.contains("nameserver 10.9.0.1\n"), "{log}");
    assert!(log.contains("v6-routes=0\n"), "{log}");
    assert!(log.contains("served through the tunnel\n"), "{log}");
    assert!(log.contains("before=0"), "{log}");
    assert_eq!(log.matches("served through the tunnel").count(), 1, "{log}");
    assert!(log.contains("around=7"), "{log}");
    assert!(log.contains("after=28") || log.contains("after=7"), "{log}");
    assert!(log.contains("still-around=7"), "{log}");
    assert!(helpers.is_empty(), "{helpers:?}");
    let mut holding = Vec::new();
    files_holding(&daemon.state, &client.0, &mut holding);
    assert!(
        holding.is_empty(),
        "the private key was copied: {holding:?}"
    );
}

#[test]
fn network_boundary_a_stored_queue_password_moves_to_a_private_file_at_start() {
    use std::os::unix::fs::PermissionsExt;
    let mark = ["SECRET", "MARK"].concat();
    let mut daemon = Daemon::start("boundary-stored");
    daemon.job(&["queue", "add", "old", "--net", "socks5://127.0.0.1:9"]);
    let objects = daemon.state.join("objects.json");
    let stored = std::fs::read_to_string(&objects).unwrap();
    assert!(stored.contains("socks5://127.0.0.1:9"), "{stored}");
    std::fs::write(
        &objects,
        stored.replace(
            "socks5://127.0.0.1:9",
            &format!("socks5://user:{mark}@127.0.0.1:9"),
        ),
    )
    .unwrap();
    daemon.restart();
    let stored = std::fs::read_to_string(&objects).unwrap();
    assert!(stored.contains("socks5://***@127.0.0.1:9"), "{stored}");
    assert!(!stored.contains(&mark), "{stored}");
    let shown = daemon.job(&["queue", "show", "old", "--json"]);
    assert!(
        text(&shown).contains("socks5://***@127.0.0.1:9"),
        "{}",
        text(&shown)
    );
    assert!(!text(&shown).contains(&mark), "{}", text(&shown));
    let mut holding = Vec::new();
    files_holding(&daemon.state, &mark, &mut holding);
    assert_eq!(holding.len(), 1, "{holding:?}");
    assert!(holding[0].starts_with(daemon.state.join("net-secrets")));
    assert_eq!(
        std::fs::metadata(&holding[0]).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(stored.contains(holding[0].to_str().unwrap()), "{stored}");
}

#[test]
fn network_boundary_a_remote_job_carries_its_proxy_password_in_the_request_not_in_arguments() {
    use std::os::unix::fs::PermissionsExt;
    let mark = ["SECRET", "MARK"].concat();
    let fake = std::env::temp_dir().join(format!("job-it-{}-fake-ssh", std::process::id()));
    std::fs::create_dir_all(&fake).unwrap();
    std::fs::write(
        fake.join("ssh"),
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {dir}/arguments\ncat >> {dir}/request\nexit 255\n",
            dir = fake.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(fake.join("ssh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        fake.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let daemon = Daemon::with_path("boundary-remote", &path);
    let file = daemon.file("proxy.secret");
    std::fs::write(&file, format!("filed:{mark}\n")).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut said = Vec::new();
    for options in [
        vec![
            "--net".to_string(),
            format!("socks5://inline:{mark}@127.0.0.1:9"),
        ],
        vec![
            "--net".to_string(),
            "socks5://127.0.0.1:9".to_string(),
            "--net-secret-file".to_string(),
            "proxy.secret".to_string(),
        ],
    ] {
        let mut args = vec!["submit", "--on", "far@elsewhere"];
        args.extend(options.iter().map(String::as_str));
        args.extend(["--", "true"]);
        let submitted = daemon.job(&args);
        let id = text(&submitted).trim().to_string();
        daemon.job(&["wait", &id, "--timeout", "30s"]);
        said.push(submitted);
        said.push(daemon.job(&["status", &id]));
        said.push(daemon.job(&["status", &id, "--json"]));
    }
    let arguments = std::fs::read_to_string(fake.join("arguments")).unwrap_or_default();
    let request = std::fs::read_to_string(fake.join("request")).unwrap_or_default();
    let _ = std::fs::remove_file(fake.join("arguments"));
    let _ = std::fs::remove_file(fake.join("request"));
    let _ = std::fs::remove_file(fake.join("ssh"));
    let _ = std::fs::remove_dir(&fake);
    assert!(arguments.contains("far@elsewhere"), "{arguments}");
    assert!(!arguments.contains(&mark), "{arguments}");
    let sent: Vec<serde_json::Value> = request
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    assert_eq!(sent.len(), 2, "{request}");
    for (request, user) in sent.iter().zip(["inline", "filed"]) {
        let declared = &request["op"]["Submit"]["declared"];
        assert_eq!(
            declared["net"]["Proxy"],
            format!("socks5://{user}:{mark}@127.0.0.1:9").as_str()
        );
        assert!(declared.get("net_secret_file").is_none(), "{declared}");
    }
    for output in &said {
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!all.contains(&mark), "{all}");
    }
}

fn review_tmp_area(parent: &str, name: &str) -> PathBuf {
    let area = Path::new(parent).join(format!("job-review-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&area).unwrap();
    area
}

fn review_state(daemon: &Daemon, id: &str) -> String {
    record(daemon, id)["state"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn review_rewrite(path: &Path, change: impl Fn(&mut serde_json::Value)) {
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    change(&mut value);
    std::fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

#[test]
fn review_private_tmp_refuses_a_working_directory_under_tmp_and_never_shows_the_host_tmp() {
    let mut daemon = Daemon::with_profile("review-tmp-cwd", "ordinary");
    let outside = isolation_area("review-tmp-cwd");
    let private = ["--namespaces", "user,mount", "--private-tmp", "yes"];
    for parent in ["/tmp", "/var/tmp"] {
        let inside = review_tmp_area(parent, "cwd");
        let marker = format!("job-review-host-marker-{}", std::process::id());
        std::fs::write(inside.join(&marker), "host\n").unwrap();
        let refused = isolation_submit(
            &daemon,
            &[&["--dir", inside.to_str().unwrap()], &private[..]].concat(),
            "ls .",
        );
        let said = String::from_utf8_lossy(&refused.stderr).into_owned();
        assert!(!refused.status.success(), "{parent}: {}", text(&refused));
        assert!(
            said.contains("--private-tmp yes hides the working directory"),
            "{parent}: {said}"
        );
        let held = daemon.job(
            &[
                &["create", "--dir", outside.to_str().unwrap()],
                &private[..],
                &["--", "sh", "-c", "ls .; ls \"$PWD\""],
            ]
            .concat(),
        );
        assert!(
            held.status.success(),
            "{}",
            String::from_utf8_lossy(&held.stderr)
        );
        let id = text(&held).trim().to_owned();
        let _ = daemon.child.kill();
        let _ = daemon.child.wait();
        review_rewrite(
            &daemon.state.join("jobs").join(&id).join("job.json"),
            |job| {
                for spec in ["spec", "requested_spec", "submitted_spec"] {
                    job[spec]["cwd"] = serde_json::json!(inside);
                    job[spec]["declared"]["dir"] = serde_json::json!(inside);
                }
            },
        );
        daemon.restart();
        let released = daemon.job(&["release", &id]);
        assert!(
            released.status.success(),
            "{}",
            String::from_utf8_lossy(&released.stderr)
        );
        daemon.job(&["wait", &id, "--timeout", "30s"]);
        let log = text(&daemon.job(&["log", &id, "full"]));
        assert!(!log.contains(&marker), "{parent}: {log}");
        assert_ne!(review_state(&daemon, &id), "Succeeded", "{parent}: {log}");
        std::fs::remove_file(inside.join(&marker)).unwrap();
        std::fs::remove_dir(&inside).unwrap();
    }
    let (_, lines) = isolation_run(
        &daemon,
        &[&["--dir", outside.to_str().unwrap()], &private[..]].concat(),
        "echo here > relative && cat \"$PWD/relative\" && ls /tmp | wc -l",
    );
    assert_eq!(lines, ["here", "0"]);
    std::fs::remove_dir_all(&outside).unwrap();
}

#[test]
fn review_writable_symlink_is_resolved_once_bound_and_recorded_as_its_target() {
    let daemon = Daemon::with_profile("review-writable-link", "ordinary");
    let area = isolation_area("review-writable-link");
    let real = area.join("real");
    let link = area.join("link");
    std::fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let (id, lines) = isolation_run(
        &daemon,
        &[
            "--dir",
            area.to_str().unwrap(),
            "--namespaces",
            "user,mount",
            "--root",
            "read-only",
            "--writable",
            link.to_str().unwrap(),
        ],
        &format!(
            "echo through > '{}/through' && echo link-ok; echo direct > '{}/direct' && echo real-ok; touch outside 2>/dev/null && echo outside-written; true",
            link.display(),
            real.display()
        ),
    );
    assert_eq!(lines, ["link-ok", "real-ok"]);
    assert!(real.join("through").exists() && real.join("direct").exists());
    let resolved = std::fs::canonicalize(&real).unwrap();
    assert_eq!(
        preset_record(&daemon, &id)["result"]["isolation_controls"]["writable"],
        serde_json::json!([resolved.to_str().unwrap()])
    );
    std::fs::remove_dir_all(&area).unwrap();
}

#[test]
fn review_confined_job_writes_its_writable_path_and_nothing_else_outside_its_tree() {
    let daemon = Daemon::with_profile("review-writable-confine", "ordinary");
    let host: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["host", "--json"])).unwrap();
    if host["isolation_controls"]["landlock_abi"].is_null() {
        eprintln!("skipped: this kernel has no Landlock");
        return;
    }
    let area = isolation_area("review-writable-confine");
    let writable = area.join("writable");
    let other = area.join("other");
    for directory in [&writable, &other] {
        std::fs::create_dir(directory).unwrap();
    }
    assert!(!area.starts_with(&daemon.work));
    let (id, lines) = isolation_run(
        &daemon,
        &[
            "--confine",
            "--namespaces",
            "user,mount",
            "--root",
            "read-only",
            "--writable",
            writable.to_str().unwrap(),
        ],
        &format!(
            "echo inside > '{}/yes' && echo writable-ok; touch '{}/no' 2>/dev/null && echo other-written; true",
            writable.display(),
            other.display()
        ),
    );
    assert_eq!(lines, ["writable-ok"]);
    assert!(writable.join("yes").exists());
    assert!(!other.join("no").exists());
    assert_eq!(
        preset_record(&daemon, &id)["result"]["isolation_controls"]["writable"],
        serde_json::json!([std::fs::canonicalize(&writable).unwrap().to_str().unwrap()])
    );
    std::fs::remove_dir_all(&area).unwrap();
}

fn review_tmp_entries(daemon: &Daemon, id: &str) -> Vec<String> {
    std::fs::read_dir(daemon.state.join("jobs").join(id))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("tmp"))
        .collect()
}

#[test]
fn review_private_tmp_left_without_write_permission_is_removed_and_a_retry_starts() {
    let daemon = Daemon::with_profile("review-tmp-locked", "ordinary");
    let area = isolation_area("review-tmp-locked");
    let (id, lines) = isolation_run(
        &daemon,
        &[
            "--dir",
            area.to_str().unwrap(),
            "--namespaces",
            "user,mount",
            "--private-tmp",
            "yes",
        ],
        "mkdir -p /tmp/a/b && touch /tmp/a/b/file && chmod 500 /tmp/a/b /tmp/a && echo made",
    );
    assert_eq!(lines, ["made"]);
    assert_eq!(review_tmp_entries(&daemon, &id), Vec::<String>::new());
    assert!(
        preset_record(&daemon, &id)["result"]["notes"]
            .as_array()
            .is_none_or(|notes| notes.is_empty())
    );
    let retried = daemon.job(&["retry", &id]);
    assert!(
        retried.status.success(),
        "{}",
        String::from_utf8_lossy(&retried.stderr)
    );
    daemon.job(&["wait", &id, "--timeout", "30s"]);
    let again = record(&daemon, &id);
    assert_eq!(again["attempt"], 2);
    assert_eq!(again["state"], "Succeeded", "{again}");
    assert_eq!(review_tmp_entries(&daemon, &id), Vec::<String>::new());
    std::fs::remove_dir_all(&area).unwrap();
}

#[test]
fn review_private_tmp_that_cannot_be_removed_is_noted_and_does_not_block_a_retry() {
    let daemon = Daemon::with_profile("review-tmp-deep", "ordinary");
    let area = isolation_area("review-tmp-deep");
    let (id, lines) = isolation_run(
        &daemon,
        &[
            "--dir",
            area.to_str().unwrap(),
            "--namespaces",
            "user,mount",
            "--private-tmp",
            "yes",
        ],
        "d=/tmp; i=0; while [ $i -lt 200 ]; do d=$d/x; mkdir $d || exit 1; i=$((i+1)); done; echo made",
    );
    assert_eq!(lines, ["made"]);
    let saved = preset_record(&daemon, &id);
    let notes = saved["result"]["notes"].to_string();
    assert!(
        notes.contains("the private tmp could not be removed"),
        "{notes}"
    );
    assert_eq!(review_tmp_entries(&daemon, &id), Vec::<String>::new());
    let retried = daemon.job(&["retry", &id]);
    assert!(
        retried.status.success(),
        "{}",
        String::from_utf8_lossy(&retried.stderr)
    );
    daemon.job(&["wait", &id, "--timeout", "30s"]);
    let again = record(&daemon, &id);
    assert_eq!(again["attempt"], 2);
    assert_eq!(again["state"], "Succeeded", "{again}");
    std::fs::remove_dir_all(&area).unwrap();
}

#[test]
fn review_ports_of_a_surviving_network_are_isolated_again_when_the_service_starts() {
    if let Some(reason) = network_boundary_missing(&LINK_TOOLS) {
        eprintln!("skipped: {reason}");
        return;
    }
    let mut daemon = Daemon::start("review-isolated");
    daemon.job(&[
        "queue",
        "add",
        "pair",
        "--parallel",
        "all",
        "--bandwidth",
        "100Mbit",
    ]);
    let first = text(&daemon.job(&[
        "submit",
        "-q",
        "pair",
        "--",
        "touch a.ready; n=0; while [ ! -e a.done ] && [ $n -lt 300 ]; do sleep 0.1; n=$((n+1)); done",
    ]))
    .trim()
    .to_string();
    assert!(
        appears(&daemon.file("a.ready"), 20),
        "the first job did not start"
    );
    let link: serde_json::Value =
        serde_json::from_slice(&std::fs::read(daemon.state.join("links/pair.json")).unwrap())
            .unwrap();
    let holder = link["holder"]["pid"].as_u64().unwrap() as u32;
    let index = record(&daemon, &first)["link"]["job"]["index"]
        .as_u64()
        .unwrap();
    let shown = || {
        let output = in_namespace(holder, &format!("ip -d link show dev q{index}"));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        text(&output)
    };
    assert!(shown().contains("isolated on"), "{}", shown());
    let cleared = in_namespace(
        holder,
        &format!("ip link set q{index} type bridge_slave isolated off"),
    );
    assert!(
        cleared.status.success(),
        "{}",
        String::from_utf8_lossy(&cleared.stderr)
    );
    assert!(shown().contains("isolated off"), "{}", shown());
    daemon.restart();
    let after = shown();
    std::fs::write(daemon.file("a.done"), "").unwrap();
    daemon.job(&["wait", &first, "--timeout", "40s"]);
    daemon.job(&["queue", "rm", "pair"]);
    assert!(after.contains("isolated on"), "{after}");
}

#[test]
fn review_stored_job_records_and_links_lose_their_proxy_password_at_start_and_in_answers() {
    use std::os::unix::fs::PermissionsExt;
    let mark = ["SECRET", "MARK"].concat();
    let url = format!("socks5://user:{mark}@127.0.0.1:9");
    let mut daemon = Daemon::start("review-stored-records");
    let ended = text(&daemon.job(&["submit", "--", "true"]))
        .trim()
        .to_string();
    daemon.job(&["wait", &ended, "--timeout", "30s"]);
    let held = text(&daemon.job(&["create", "--", "true"]))
        .trim()
        .to_string();
    let _ = daemon.child.kill();
    let _ = daemon.child.wait();
    let state = daemon.state.clone();
    let job_file = |id: &str| state.join("jobs").join(id).join("job.json");
    let old_net = |job: &mut serde_json::Value| {
        for spec in ["spec", "requested_spec", "submitted_spec"] {
            job[spec]["declared"]["net"] = serde_json::json!({"Proxy": url});
        }
    };
    review_rewrite(&job_file(&held), old_net);
    review_rewrite(&job_file(&ended), |job| {
        old_net(job);
        job["link"] = serde_json::json!({
            "name": format!("job-{ended}"),
            "job": {"holder": {"pid": 1, "start_ticks": 1}, "index": 2},
            "cap": null,
            "link_rate": null,
            "egress": {"Proxy": url},
            "env": [["ALL_PROXY", format!("socks5h://user:{mark}@198.18.0.2:9")]],
            "resolver": "nameserver 127.0.0.1\n"
        });
    });
    std::fs::create_dir_all(daemon.state.join("links")).unwrap();
    let link_file = daemon.state.join("links/old.json");
    std::fs::write(
        &link_file,
        serde_json::to_vec_pretty(&serde_json::json!({
            "name": "old",
            "holder": {"pid": 1, "start_ticks": 1},
            "relay": {"pid": 1, "start_ticks": 1},
            "rate": null,
            "egress": {"Proxy": url}
        }))
        .unwrap(),
    )
    .unwrap();
    daemon.restart();
    let stored_link = std::fs::read_to_string(&link_file).unwrap();
    assert!(!stored_link.contains(&mark), "{stored_link}");
    assert!(
        stored_link.contains("socks5://***@127.0.0.1:9"),
        "{stored_link}"
    );
    let stored = std::fs::read_to_string(job_file(&held)).unwrap();
    assert!(!stored.contains(&mark), "{stored}");
    let private = daemon.state.join("jobs").join(&held).join("net-secret");
    assert_eq!(
        std::fs::read_to_string(&private).unwrap(),
        format!("user:{mark}\n")
    );
    assert_eq!(
        std::fs::metadata(&private).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let settled = record(&daemon, &held);
    for spec in ["spec", "requested_spec", "submitted_spec"] {
        assert_eq!(
            settled[spec]["declared"]["net"]["Proxy"], "socks5://***@127.0.0.1:9",
            "{spec}"
        );
        assert_eq!(
            settled[spec]["declared"]["net_secret_file"],
            private.to_str().unwrap(),
            "{spec}"
        );
    }
    let mut shown_somewhere = false;
    for args in [
        vec!["status", ended.as_str(), "--json"],
        vec!["status", ended.as_str()],
        vec!["status", held.as_str(), "--json"],
        vec!["attempts", ended.as_str(), "--json"],
        vec!["attempts", held.as_str(), "--json"],
        vec!["queue"],
    ] {
        let output = daemon.job(&args);
        let all = format!(
            "{}{}",
            text(&output),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!all.contains(&mark), "{args:?}: {all}");
        shown_somewhere |= all.contains("***@");
    }
    assert!(shown_somewhere, "no answer named the proxy at all");
    let raw = request(
        &daemon,
        serde_json::json!({"Status": {"id": ended.parse::<u64>().unwrap()}}),
    );
    assert!(!raw.to_string().contains(&mark), "{raw}");
    assert!(raw.to_string().contains("***@198.18.0.2:9"), "{raw}");
    let mut holding = Vec::new();
    files_holding(&daemon.state, &mark, &mut holding);
    holding.sort();
    assert_eq!(holding, [job_file(&ended), private]);
}

fn idset_daemon(name: &str, failpoint: Option<&str>) -> Daemon {
    let base = std::env::temp_dir().join(format!("job-it-{}-{name}", std::process::id()));
    let state = base.join("state");
    let work = base.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(
        base.join("config.toml"),
        "schema_version = 1\nprofile = 'ordinary'\n",
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_job"));
    command
        .arg("daemon")
        .env("JOB_CGROUP_ROOT", base.join("absent-cgroup"))
        .env("JOB_CONFIG", base.join("config.toml"))
        .env("JOB_STATE_DIR", &state)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(failpoint) = failpoint {
        command.env("JOB_FAILPOINT", failpoint);
    }
    let mut daemon = Daemon {
        child: command.spawn().unwrap(),
        base,
        state,
        work,
    };
    daemon.ready();
    daemon
}

fn idset_made(daemon: &Daemon, verb: &str, count: usize, argv: &[&str]) -> Vec<String> {
    (0..count)
        .map(|_| {
            let mut args = vec![verb, "--"];
            args.extend_from_slice(argv);
            let output = daemon.job(&args);
            assert!(output.status.success(), "{output:?}");
            text(&output).trim().to_owned()
        })
        .collect()
}

fn idset_answer(daemon: &Daemon, args: &[&str]) -> (i32, String, String) {
    let output = daemon
        .job_command(args)
        .env("LC_ALL", "C")
        .env_remove("LANG")
        .env_remove("LC_MESSAGES")
        .output()
        .unwrap();
    (
        output.status.code().unwrap(),
        text(&output),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn idset_state(daemon: &Daemon, id: &str) -> String {
    let output = daemon.job(&["list", "--id", id, "--format", "tsv"]);
    text(&output)
        .lines()
        .nth(1)
        .and_then(|line| line.split('\t').nth(2))
        .unwrap_or("absent")
        .to_owned()
}

fn idset_states(daemon: &Daemon, ids: &[String]) -> Vec<String> {
    ids.iter().map(|id| idset_state(daemon, id)).collect()
}

fn idset_until(daemon: &Daemon, ids: &[String], wanted: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let states = idset_states(daemon, ids);
        if states.iter().all(|state| state == wanted) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{ids:?} are {states:?}, not {wanted}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn idset_lines(daemon: &Daemon, file: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(daemon.state.join(file))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn idset_cancelled_events(daemon: &Daemon) -> Vec<u64> {
    let mut jobs: Vec<u64> = idset_lines(daemon, "events/current.jsonl")
        .into_iter()
        .filter(|line| line["kind"] == "job" && line["to"] == "cancelled")
        .map(|line| line["job"].as_u64().unwrap())
        .collect();
    jobs.sort_unstable();
    jobs
}

fn idset_operations(daemon: &Daemon) -> Vec<serde_json::Value> {
    let Ok(entries) = std::fs::read_dir(daemon.state.join("cancellations")) else {
        return Vec::new();
    };
    entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap())
        .collect()
}

#[test]
fn idset_cancel_of_a_range_cancels_those_jobs_and_nothing_else() {
    let daemon = idset_daemon("idset-range", None);
    let ids = idset_made(&daemon, "create", 11, &["true"]);
    assert_eq!(ids[0], "1");
    assert_eq!(ids[10], "11");
    let (status, out, err) = idset_answer(&daemon, &["cancel", "1-10"]);
    assert_eq!(status, 0, "{out}{err}");
    assert_eq!(
        out,
        (1..=10)
            .map(|id| format!("{id}: cancelled\n"))
            .collect::<String>()
    );
    assert_eq!(err, "");
    assert!(
        idset_states(&daemon, &ids[..10])
            .iter()
            .all(|state| state == "cancelled")
    );
    assert_eq!(idset_state(&daemon, "11"), "held");
    let operations = idset_operations(&daemon);
    assert_eq!(operations.len(), 1, "{operations:?}");
    assert_eq!(operations[0]["complete"], true);
    assert_eq!(operations[0]["selection"]["set"], "1-10");
    assert_eq!(
        operations[0]["selection"]["members"]
            .as_array()
            .unwrap()
            .len(),
        10
    );
    assert_eq!(
        idset_cancelled_events(&daemon),
        (1..=10).collect::<Vec<u64>>()
    );
}

#[test]
fn idset_a_range_over_a_gap_skips_the_missing_ids_silently() {
    let daemon = idset_daemon("idset-gap", None);
    let ids = idset_made(&daemon, "create", 5, &["true"]);
    assert_eq!(idset_answer(&daemon, &["cancel", "2"]).0, 0);
    assert_eq!(idset_answer(&daemon, &["remove", "2"]).0, 0);
    assert_eq!(idset_state(&daemon, "2"), "absent");
    let (status, out, err) = idset_answer(&daemon, &["cancel", "1-5", "7-9"]);
    assert_eq!(status, 0, "{out}{err}");
    assert_eq!(
        out,
        "1: cancelled\n3: cancelled\n4: cancelled\n5: cancelled\n"
    );
    assert_eq!(err, "");
    assert_eq!(
        idset_states(&daemon, &ids),
        ["cancelled", "absent", "cancelled", "cancelled", "cancelled"]
    );
    let (status, out, err) = idset_answer(&daemon, &["cancel", "20-30"]);
    assert_eq!(status, 1);
    assert_eq!(out, "");
    assert_eq!(err, "job: the set `20-30` selects no existing Job\n");
}

#[test]
fn idset_an_ended_job_and_a_missing_id_are_reported_and_do_not_stop_the_others() {
    let daemon = idset_daemon("idset-mix", None);
    idset_made(&daemon, "create", 4, &["true"]);
    assert_eq!(idset_answer(&daemon, &["cancel", "2"]).0, 0);
    let (status, out, err) = idset_answer(&daemon, &["cancel", "1-3,99", "4"]);
    assert_eq!(status, 1, "{out}{err}");
    assert_eq!(
        out,
        "1: cancelled\n2: already ended (cancelled)\n3: cancelled\n4: cancelled\n99: no such Job\n"
    );
    assert_eq!(
        idset_states(&daemon, &["1", "2", "3", "4"].map(str::to_owned)),
        ["cancelled"; 4]
    );
    let (status, out, _) = idset_answer(&daemon, &["cancel", "1-4,99", "--json"]);
    assert_eq!(status, 1);
    let report: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["kind"], "job_set");
    assert_eq!(report["action"], "cancel");
    assert_eq!(report["set"], "1-4,99");
    assert_eq!(report["selected"], 4);
    let results = report["results"].as_array().unwrap();
    assert_eq!(results.len(), 5);
    assert_eq!(results[0]["id"], 1);
    assert_eq!(results[0]["outcome"], "refused");
    assert_eq!(results[0]["result"], "already_ended");
    assert_eq!(results[0]["state"], "cancelled");
    assert_eq!(results[4]["id"], 99);
    assert_eq!(results[4]["outcome"], "missing");
    assert_eq!(results[4]["result"], "no_such_job");
    let (status, out, _) = idset_answer(&daemon, &["cancel", "1-4,99", "--format", "json"]);
    assert_eq!(status, 1);
    let envelope: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(envelope["schema_version"], 1);
    assert_eq!(envelope["kind"], "cancel");
    assert_eq!(envelope["data"]["kind"], "job_set");
    assert_eq!(envelope["data"]["results"].as_array().unwrap().len(), 5);
}

#[test]
fn idset_running_jobs_in_a_range_are_cancelled_by_the_graceful_path() {
    let daemon = idset_daemon("idset-running", None);
    let script = "trap 'echo bye >> ended; exit 0' TERM; echo up >> started; sleep 30 & wait";
    let running = idset_made(&daemon, "submit", 2, &["sh", "-c", script]);
    let held = idset_made(&daemon, "create", 1, &["true"]);
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::fs::read_to_string(daemon.file("started"))
        .unwrap_or_default()
        .lines()
        .count()
        < 2
    {
        assert!(Instant::now() < deadline, "the Jobs did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (status, out, err) = idset_answer(&daemon, &["cancel", "1-3"]);
    assert_eq!(status, 0, "{out}{err}");
    assert_eq!(out, "1: stopping\n2: stopping\n3: cancelled\n");
    idset_until(&daemon, &running, "cancelled");
    assert_eq!(idset_state(&daemon, &held[0]), "cancelled");
    assert_eq!(
        std::fs::read_to_string(daemon.file("ended")).unwrap(),
        "bye\nbye\n"
    );
    let record: serde_json::Value =
        serde_json::from_slice(&daemon.job(&["status", "1", "--json"]).stdout).unwrap();
    assert_eq!(record["stop"]["kind"], "Cancelled");
    assert_eq!(idset_cancelled_events(&daemon), [1, 2, 3]);
}

#[test]
fn idset_dry_run_shows_the_selection_and_changes_nothing() {
    let daemon = idset_daemon("idset-dry", None);
    let ids = idset_made(&daemon, "create", 3, &["true"]);
    let (status, out, err) = idset_answer(&daemon, &["cancel", "1-3,8", "--dry-run"]);
    assert_eq!(status, 1, "{out}{err}");
    assert_eq!(
        out,
        "1: cancelled (dry run, nothing changed)\n2: cancelled (dry run, nothing changed)\n3: cancelled (dry run, nothing changed)\n8: no such Job (dry run, nothing changed)\n"
    );
    let (status, out, _) = idset_answer(&daemon, &["release", "1-2", "--dry-run", "--json"]);
    assert_eq!(status, 0);
    let report: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(report["dry_run"], true);
    assert_eq!(report["results"][1]["result"], "released");
    let (status, out, _) = idset_answer(&daemon, &["remove", "1-3", "--dry-run"]);
    assert_eq!(status, 1);
    assert!(
        out.starts_with(
            "1: refused: only completed Jobs can be removed; cancel separately (dry run, nothing changed)\n"
        ),
        "{out}"
    );
    assert_eq!(idset_states(&daemon, &ids), ["held"; 3]);
    assert!(idset_operations(&daemon).is_empty());
    assert!(
        idset_lines(&daemon, "audit/current.jsonl")
            .iter()
            .all(|line| line["action"] == "create"),
        "a dry run was written to the audit journal"
    );
    let many = idset_made(&daemon, "create", 22, &["true"]);
    let (status, out, _) = idset_answer(&daemon, &["cancel", "4-25,40", "--dry-run"]);
    assert_eq!(status, 1);
    assert_eq!(
        out,
        "dry run, nothing changed; cancelled: 4-25; no such Job: 40\n"
    );
    let (status, out, _) = idset_answer(&daemon, &["cancel", "1,4-25,40"]);
    assert_eq!(status, 1);
    assert_eq!(out, "cancelled: 1, 4-25; no such Job: 40\n");
    assert_eq!(idset_states(&daemon, &many), ["cancelled"; 22]);
    assert_eq!(idset_states(&daemon, &ids[1..]), ["held"; 2]);
}

#[test]
fn idset_usage_errors_change_nothing() {
    let daemon = idset_daemon("idset-usage", None);
    let ids = idset_made(&daemon, "create", 3, &["true"]);
    for (set, reason) in [
        ("3-1", "`3-1` is a reversed range; write the lower ID first"),
        ("1,,2", "`1,,2` has an empty item"),
        ("1-3,", "`1-3,` has an empty item"),
        ("0-2", "`0-2` is not a Job ID"),
        ("1-x", "`1-x` is not a Job ID"),
        ("1-", "`1-` is an open range; write both ends, as in 5-10"),
        ("1-2-3", "`1-2-3` is not a Job ID"),
        ("1-10001", "the set names more than 10000 IDs"),
    ] {
        for command in ["cancel", "release", "remove", "wait"] {
            let (status, out, err) = idset_answer(&daemon, &[command, set]);
            assert_eq!(status, 125, "{command} {set}: {out}{err}");
            assert_eq!(out, "", "{command} {set}");
            assert!(err.contains(reason), "{command} {set}: {err}");
        }
    }
    let (status, _, err) = idset_answer(&daemon, &["cancel", "1", "2", "x"]);
    assert_eq!(status, 125);
    assert!(err.contains("`x` is not a Job ID"), "{err}");
    assert_eq!(idset_states(&daemon, &ids), ["held"; 3]);
    assert!(idset_operations(&daemon).is_empty());
}

#[test]
fn idset_a_single_id_answers_exactly_as_before() {
    let daemon = idset_daemon("idset-single", None);
    let ids = idset_made(&daemon, "create", 4, &["true"]);
    let (status, out, err) = idset_answer(&daemon, &["cancel", &ids[0]]);
    assert_eq!(
        (status, out.as_str(), err.as_str()),
        (0, "cancelled before it started\n", "")
    );
    let (status, out, err) = idset_answer(&daemon, &["cancel", &ids[1], "--json"]);
    assert_eq!((status, err.as_str()), (0, ""));
    let record: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(record["id"], 2);
    assert_eq!(record["state"], "Cancelled");
    assert!(record.get("kind").is_none());
    let (status, out, err) = idset_answer(&daemon, &["cancel", &ids[0]]);
    assert_eq!(
        (status, out.as_str(), err.as_str()),
        (125, "", "job: Job has already finished\n")
    );
    let (status, out, err) = idset_answer(&daemon, &["cancel", "99"]);
    assert_eq!(
        (status, out.as_str(), err.as_str()),
        (125, "", "job: invalid Job record\n")
    );
    let (status, out, err) = idset_answer(&daemon, &["release", &ids[2]]);
    assert_eq!((status, out.as_str(), err.as_str()), (0, "", ""));
    let (status, out, err) = idset_answer(&daemon, &["release", &ids[2]]);
    assert_eq!(
        (status, out.as_str(), err.as_str()),
        (125, "", "job: only held Jobs can be released\n")
    );
    let (status, out, err) = idset_answer(&daemon, &["remove", &ids[0]]);
    assert_eq!((status, out.as_str(), err.as_str()), (0, "", ""));
    let (status, out, err) = idset_answer(&daemon, &["signal", &ids[3]]);
    assert_eq!(
        (status, out.as_str(), err.as_str()),
        (125, "", "job: job 4 is not running\n")
    );
    let receipt: serde_json::Value = serde_json::from_slice(
        &std::fs::read(daemon.state.join("removals").join("job-1.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["selection"]["jobs"].as_array().unwrap().len(), 1);
}

#[test]
fn idset_a_crash_in_the_middle_of_a_set_cancel_finishes_after_restart() {
    let mut daemon = idset_daemon("idset-crash", Some("cancel-batch-mid-rename@9"));
    let ids = idset_made(&daemon, "create", 13, &["true"]);
    let (status, out, _) = idset_answer(&daemon, &["cancel", "1-12"]);
    assert_ne!(status, 0, "{out}");
    let deadline = Instant::now() + Duration::from_secs(30);
    while daemon.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "the daemon did not stop");
        std::thread::sleep(Duration::from_millis(10));
    }
    let recorded: Vec<String> = ids[..12]
        .iter()
        .map(|id| {
            let job: serde_json::Value = serde_json::from_slice(
                &std::fs::read(daemon.state.join("jobs").join(id).join("job.json")).unwrap(),
            )
            .unwrap();
            job["state"].as_str().unwrap().to_owned()
        })
        .collect();
    assert_eq!(
        recorded
            .iter()
            .filter(|state| *state == "Cancelled")
            .count(),
        8,
        "{recorded:?}"
    );
    assert_eq!(
        recorded.iter().filter(|state| *state == "Held").count(),
        4,
        "{recorded:?}"
    );
    daemon.restart();
    idset_until(&daemon, &ids[..12], "cancelled");
    assert_eq!(idset_state(&daemon, &ids[12]), "held");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let operations = idset_operations(&daemon);
        assert_eq!(operations.len(), 1, "{operations:?}");
        if operations[0]["complete"] == true {
            assert_eq!(operations[0]["selection"]["set"], "1-12");
            assert_eq!(operations[0]["results"].as_array().unwrap().len(), 12);
            break;
        }
        assert!(Instant::now() < deadline, "the operation stayed open");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        idset_cancelled_events(&daemon),
        (1..=12).collect::<Vec<u64>>()
    );
    let (status, out, _) = idset_answer(&daemon, &["cancel", "1-13"]);
    assert_eq!(status, 1);
    assert!(out.ends_with("13: cancelled\n"), "{out}");
}

#[test]
fn idset_remove_of_a_range_removes_the_ended_jobs_in_one_transaction() {
    let daemon = idset_daemon("idset-remove", None);
    let ended = idset_made(&daemon, "submit", 10, &["true"]);
    idset_until(&daemon, &ended, "succeeded");
    let running = idset_made(&daemon, "submit", 1, &["sleep", "30"]);
    idset_until(&daemon, &running, "running");
    let (status, out, err) = idset_answer(&daemon, &["remove", "1-11"]);
    assert_eq!(status, 1, "{out}{err}");
    let mut expected: String = (1..=10).map(|id| format!("{id}: removed\n")).collect();
    expected.push_str("11: refused: only completed Jobs can be removed; cancel separately\n");
    assert_eq!(out, expected);
    for id in &ended {
        assert!(!daemon.state.join("jobs").join(id).exists(), "job {id}");
    }
    assert!(daemon.state.join("jobs").join("11").exists());
    assert_eq!(idset_state(&daemon, "11"), "running");
    let receipts: Vec<_> = std::fs::read_dir(daemon.state.join("removals"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(receipts.len(), 1, "{receipts:?}");
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&receipts[0]).unwrap()).unwrap();
    assert_eq!(receipt["operation"], "job-1");
    assert_eq!(receipt["selection"]["jobs"].as_array().unwrap().len(), 10);
    let (status, out, err) = idset_answer(&daemon, &["remove", "1-10"]);
    assert_eq!(status, 1);
    assert_eq!(out, "");
    assert_eq!(err, "job: the set `1-10` selects no existing Job\n");
    assert_eq!(idset_answer(&daemon, &["cancel", "11"]).0, 0);
    let next = idset_made(&daemon, "create", 1, &["true"]);
    assert_eq!(next[0], "12");
}

#[test]
fn idset_release_of_a_range_releases_the_held_jobs() {
    let daemon = idset_daemon("idset-release", None);
    let held = idset_made(&daemon, "create", 4, &["true"]);
    let (status, out, err) = idset_answer(&daemon, &["release", "1-3"]);
    assert_eq!(status, 0, "{out}{err}");
    assert_eq!(out, "1: released\n2: released\n3: released\n");
    idset_until(&daemon, &held[..3], "succeeded");
    assert_eq!(idset_state(&daemon, "4"), "held");
    let (status, out, _) = idset_answer(&daemon, &["release", "3-4"]);
    assert_eq!(status, 1);
    assert_eq!(
        out,
        "3: refused: only held Jobs can be released\n4: released\n"
    );
    idset_until(&daemon, &held[3..], "succeeded");
}

#[test]
fn idset_wait_for_a_set_waits_for_all_and_returns_the_first_failure() {
    let daemon = idset_daemon("idset-wait", None);
    idset_made(&daemon, "submit", 1, &["sh", "-c", "sleep 0.3"]);
    idset_made(&daemon, "submit", 1, &["sh", "-c", "sleep 0.6; exit 4"]);
    idset_made(&daemon, "submit", 1, &["sh", "-c", "exit 2"]);
    let (status, out, err) = idset_answer(&daemon, &["wait", "1-3", "--timeout", "30s"]);
    assert_eq!(status, 4, "{out}{err}");
    assert_eq!(
        out,
        "1: succeeded\n2: failed, exit status 4\n3: failed, exit status 2\n"
    );
    let (status, out, _) = idset_answer(&daemon, &["wait", "1", "3", "--json"]);
    assert_eq!(status, 2);
    let report: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(report["action"], "wait");
    assert_eq!(report["results"][0]["exit_status"], 0);
    assert_eq!(report["results"][1]["exit_status"], 2);
    let slow = idset_made(&daemon, "submit", 1, &["sleep", "30"]);
    let begun = Instant::now();
    let (status, out, _) = idset_answer(&daemon, &["wait", "1,4", "--timeout", "500ms"]);
    assert_eq!(status, 75, "{out}");
    assert_eq!(out, "1: succeeded\n4: still running\n");
    assert!(begun.elapsed() < Duration::from_secs(10));
    assert_eq!(idset_state(&daemon, &slow[0]), "running");
    let (status, out, _) = idset_answer(&daemon, &["wait", "1,9"]);
    assert_eq!(status, 1);
    assert_eq!(out, "1: succeeded\n9: no such Job\n");
    assert_eq!(idset_answer(&daemon, &["cancel", "4"]).0, 0);
}

#[test]
fn idset_list_filters_by_a_set_of_ids() {
    let daemon = idset_daemon("idset-list", None);
    idset_made(&daemon, "create", 5, &["true"]);
    assert_eq!(idset_answer(&daemon, &["cancel", "3"]).0, 0);
    let listed = |args: &[&str]| -> Vec<String> {
        let (status, out, err) = idset_answer(&daemon, args);
        assert_eq!(status, 0, "{out}{err}");
        out.lines()
            .skip(1)
            .map(|line| {
                let mut fields = line.split('\t');
                format!("{} {}", fields.next().unwrap(), fields.nth(1).unwrap())
            })
            .collect()
    };
    assert_eq!(
        listed(&["list", "--id", "2-4", "--format", "tsv"]),
        ["2 held", "3 cancelled", "4 held"]
    );
    assert_eq!(
        listed(&[
            "list", "--id", "2-4,9", "--state", "held", "--format", "tsv"
        ]),
        ["2 held", "4 held"]
    );
    let (status, out, _) = idset_answer(&daemon, &["list", "--id", "1,5", "--json"]);
    assert_eq!(status, 0);
    let listing: serde_json::Value = serde_json::from_str(&out).unwrap();
    let ids: Vec<u64> = listing["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["job"]["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, [1, 5]);
    let (status, out, err) = idset_answer(&daemon, &["list", "--id", "4-2"]);
    assert_eq!(status, 125, "{out}");
    assert!(err.contains("`4-2` is a reversed range"), "{err}");
}

#[test]
fn idset_the_audit_journal_records_one_request_with_the_set_and_the_counts() {
    let daemon = idset_daemon("idset-audit", None);
    idset_made(&daemon, "create", 4, &["true"]);
    assert_eq!(idset_answer(&daemon, &["cancel", "2"]).0, 0);
    let (status, _, _) = idset_answer(&daemon, &["cancel", "1-3,9", "--session", "mine"]);
    assert_eq!(status, 1);
    let ended: Vec<serde_json::Value> = idset_lines(&daemon, "audit/current.jsonl")
        .into_iter()
        .filter(|line| line["target"] == "1-3,9" && line["result"] != "begin")
        .collect();
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert_eq!(ended[0]["action"], "cancel");
    assert_eq!(
        ended[0]["result"],
        "error: cancelled 2, already_ended 1, no_such_job 1"
    );
    assert_eq!(
        ended[0]["operation"],
        idset_operations(&daemon)
            .iter()
            .find(|operation| operation["selection"]["set"] == "1-3,9")
            .unwrap()["operation"]
    );
    assert_eq!(
        ended[0]["params"]["call"]["Set"]["call"]["operands"],
        serde_json::json!(["1-3,9"])
    );
    let begun = idset_lines(&daemon, "audit/current.jsonl")
        .into_iter()
        .filter(|line| line["target"] == "1-3,9" && line["result"] == "begin")
        .count();
    assert_eq!(begun, 1);
    assert_eq!(idset_cancelled_events(&daemon), [1, 2, 3]);
    let (status, out, _) = idset_answer(&daemon, &["release", "4", "5-9"]);
    assert_eq!(status, 0);
    assert_eq!(out, "4: released\n");
    let released: Vec<serde_json::Value> = idset_lines(&daemon, "audit/current.jsonl")
        .into_iter()
        .filter(|line| line["action"] == "release" && line["result"] != "begin")
        .collect();
    assert_eq!(released.len(), 1);
    assert_eq!(released[0]["target"], "4 5-9");
    assert_eq!(released[0]["result"], "ok: released 1");
}

#[test]
fn idset_reprioritize_move_retry_and_signal_take_sets() {
    let daemon = idset_daemon("idset-others", None);
    let ids = idset_made(&daemon, "create", 3, &["true"]);
    let record = |id: &str| -> serde_json::Value {
        serde_json::from_slice(&daemon.job(&["status", id, "--json"]).stdout).unwrap()
    };
    let (status, out, err) = idset_answer(&daemon, &["reprioritize", "1-3", "--priority", "7"]);
    assert_eq!(status, 0, "{out}{err}");
    assert_eq!(
        out,
        "1: reprioritized\n2: reprioritized\n3: reprioritized\n"
    );
    for id in &ids {
        assert_eq!(record(id)["spec"]["declared"]["priority"], 7);
    }
    assert!(daemon.job(&["queue", "create", "other"]).status.success());
    let (status, out, _) = idset_answer(&daemon, &["move", "1-2,9", "--queue", "other"]);
    assert_eq!(status, 1);
    assert_eq!(out, "1: moved\n2: moved\n9: no such Job\n");
    assert_eq!(record("1")["spec"]["queue"], "other");
    assert_eq!(record("2")["spec"]["queue"], "other");
    assert_eq!(record("3")["spec"]["queue"], "default");
    let (status, out, _) = idset_answer(&daemon, &["signal", "1-3"]);
    assert_eq!(status, 1);
    assert_eq!(
        out,
        "1: refused: job 1 is not running\n2: refused: job 2 is not running\n3: refused: job 3 is not running\n"
    );
    let (status, out, _) = idset_answer(&daemon, &["retry", "1-3", "--dry-run"]);
    assert_eq!(status, 1);
    assert!(
        out.starts_with("1: refused: retry requires the expected completed attempt"),
        "{out}"
    );
    assert_eq!(idset_answer(&daemon, &["cancel", "1-2"]).0, 0);
    let (status, out, err) = idset_answer(&daemon, &["retry", "1-3", "--hold"]);
    assert_eq!(status, 1, "{out}{err}");
    assert_eq!(
        out,
        "1: retried\n2: retried\n3: refused: retry requires the expected completed attempt\n"
    );
    assert_eq!(record("1")["attempt"], 2);
    assert_eq!(idset_states(&daemon, &ids), ["held"; 3]);
    let actions: Vec<String> = idset_lines(&daemon, "audit/current.jsonl")
        .into_iter()
        .filter(|line| {
            line["result"] != "begin" && line["target"].as_str().is_some_and(|t| t.contains('-'))
        })
        .map(|line| {
            format!(
                "{} {}",
                line["action"].as_str().unwrap(),
                line["result"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(
        actions,
        [
            "reprioritize ok: reprioritized 3",
            "move error: moved 2, no_such_job 1",
            "signal error: refused 3",
            "cancel ok: cancelled 2",
            "retry error: retried 2, refused 1",
        ]
    );
}

const PIDNS: [&str; 2] = ["--namespaces", "user,mount,pid"];

fn pidns_options<'a>(extra: &[&'a str]) -> Vec<&'a str> {
    PIDNS.iter().copied().chain(extra.iter().copied()).collect()
}

fn pidns_start(daemon: &Daemon, options: &[&str], script: &str) -> String {
    let submitted = isolation_submit(daemon, options, script);
    assert!(
        submitted.status.success(),
        "{options:?}: {}",
        String::from_utf8_lossy(&submitted.stderr)
    );
    text(&submitted).trim().to_owned()
}

fn pidns_finish(
    daemon: &Daemon,
    options: &[&str],
    script: &str,
) -> (serde_json::Value, Vec<String>) {
    let id = pidns_start(daemon, options, script);
    pidns_ended(daemon, &id)
}

fn pidns_ended(daemon: &Daemon, id: &str) -> (serde_json::Value, Vec<String>) {
    daemon.job(&["wait", id, "--timeout", "40s"]);
    let record = pidns_record(daemon, id);
    assert!(
        ["Succeeded", "Failed", "Cancelled"].contains(&record["state"].as_str().unwrap()),
        "{record}"
    );
    let output = daemon.job(&["logs", id, "--stream", "stdout", "--raw"]);
    (record, text(&output).lines().map(str::to_owned).collect())
}

fn pidns_record(daemon: &Daemon, id: &str) -> serde_json::Value {
    serde_json::from_slice(&daemon.job(&["status", id, "--json"]).stdout).unwrap()
}

fn pidns_on_host(marker: &str) -> bool {
    std::fs::read_dir("/proc").unwrap().flatten().any(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            && std::fs::read(entry.path().join("cmdline")).is_ok_and(|bytes| {
                String::from_utf8_lossy(&bytes)
                    .split('\0')
                    .any(|part| part == marker)
            })
    })
}

fn pidns_no_init_stays(daemon: &Daemon) {
    let state = daemon.state.to_string_lossy().into_owned();
    pidns_until("an init stayed behind", 3, || {
        !std::fs::read_dir("/proc").unwrap().flatten().any(|entry| {
            std::fs::read_to_string(entry.path().join("comm"))
                .is_ok_and(|name| name.trim() == "job-init")
                && std::fs::read(entry.path().join("cmdline"))
                    .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(&state))
        })
    });
}

fn pidns_until(what: &str, seconds: u64, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn pidns_command_runs_under_an_init_in_its_own_namespace_with_a_fresh_proc() {
    let daemon = Daemon::with_profile("pidns-basic", "ordinary");
    let service = std::fs::read_link(format!("/proc/{}/ns/pid", daemon.child.id()))
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let script = "readlink /proc/self/ns/pid; echo $$; echo /proc/[0-9]*; cat /proc/1/comm; test -r /proc/self/status && echo self; cut -d' ' -f4 /proc/$$/stat; cat /proc/1/environ >/dev/null 2>&1 || echo hidden";
    let (plain, outside) = pidns_finish(&daemon, &[], script);
    assert_eq!(outside[0], service);
    assert_ne!(outside[1], "2");
    assert!(plain["result"]["isolation_controls"].is_null());
    let (record, inside) = pidns_finish(&daemon, &PIDNS, script);
    assert_eq!(record["state"], "Succeeded", "{record}");
    assert_ne!(inside[0], service);
    assert!(inside[0].starts_with("pid:["));
    assert_eq!(
        inside[1..],
        ["2", "/proc/1 /proc/2", "job-init", "self", "1", "hidden"]
    );
    assert_eq!(
        record["result"]["isolation_controls"],
        serde_json::json!({"namespaces":["mount","pid","user"],"user_namespace_from_network":false,"root_read_only":false,"private_tmp":[],"writable":[],"pid_namespace":{"proc":"/proc","init_pid":1,"command_pid":2}})
    );
    assert_eq!(record["spec"]["declared"]["namespaces"], "mount,pid,user");
    assert_eq!(record["result"]["leftover_processes"], 0);
    let (_, after) = pidns_finish(&daemon, &[], "readlink /proc/self/ns/pid");
    assert_eq!(after[0], service);
}

#[test]
fn pidns_exit_status_fatal_signal_and_start_error_match_a_job_without_the_namespace() {
    let daemon = Daemon::with_profile("pidns-status", "ordinary");
    for script in ["exit 7", "kill -SEGV $$", "kill -TERM $$", "exit 0"] {
        let (plain, _) = pidns_finish(&daemon, &[], script);
        let (inside, _) = pidns_finish(&daemon, &PIDNS, script);
        assert_eq!(inside["state"], plain["state"], "{script}");
        assert_eq!(
            inside["result"]["exit_code"], plain["result"]["exit_code"],
            "{script}"
        );
        assert_eq!(
            inside["result"]["signal"], plain["result"]["signal"],
            "{script}"
        );
        assert!(inside["result"]["start_error"].is_null(), "{inside}");
    }
    let (failed, _) = pidns_finish(&daemon, &PIDNS, "exit 7");
    assert_eq!(failed["result"]["exit_code"], 7);
    let (killed, _) = pidns_finish(&daemon, &PIDNS, "kill -SEGV $$");
    assert_eq!(killed["result"]["signal"], libc::SIGSEGV);
    assert!(killed["result"]["exit_code"].is_null());
    let missing = |options: &[&str]| {
        let mut args = vec!["submit"];
        args.extend_from_slice(options);
        args.extend(["--", "/nonexistent/pidns-program", "argument"]);
        let id = text(&daemon.job(&args)).trim().to_owned();
        pidns_ended(&daemon, &id).0
    };
    let plain = missing(&[]);
    let inside = missing(&PIDNS);
    assert!(
        plain["result"]["start_error"]
            .as_str()
            .unwrap()
            .contains("/nonexistent/pidns-program"),
        "{plain}"
    );
    assert_eq!(
        inside["result"]["start_error"],
        plain["result"]["start_error"]
    );
    pidns_no_init_stays(&daemon);
}

#[test]
fn pidns_background_children_end_with_the_job_and_the_result_names_them() {
    let daemon = Daemon::with_profile("pidns-leftovers", "ordinary");
    let (plain, _) = pidns_finish(&daemon, &[], "sleep 29.7311 & echo started");
    let (inside, lines) = pidns_finish(&daemon, &PIDNS, "sleep 29.7312 & echo started");
    assert_eq!(lines, ["started"]);
    for (record, marker) in [(&plain, "29.7311"), (&inside, "29.7312")] {
        assert_eq!(record["state"], "Succeeded", "{record}");
        assert_eq!(record["result"]["leftover_processes"], 1, "{record}");
        assert!(
            record["result"]["leftover_names"][0]
                .as_str()
                .unwrap()
                .starts_with(&format!("sleep {marker} (pid ")),
            "{record}"
        );
        assert_eq!(
            record["result"]["kept_helpers"],
            serde_json::json!([]),
            "{record}"
        );
        pidns_until("the background child stayed", 3, || !pidns_on_host(marker));
    }
    let (quiet, _) = pidns_finish(&daemon, &PIDNS, "true");
    assert_eq!(quiet["result"]["leftover_processes"], 0);
    pidns_no_init_stays(&daemon);
}

#[test]
fn pidns_signal_reaches_the_command_once_and_the_init_forwards_from_inside() {
    let daemon = Daemon::with_profile("pidns-signal", "ordinary");
    let id = pidns_start(
        &daemon,
        &pidns_options(&["--time", "30s"]),
        "n=0; trap 'n=$((n+1))' USR1; : > ready; i=0; while [ $i -lt 300 ] && [ ! -e stop ]; do sleep 0.05 & wait $!; i=$((i+1)); done; echo count=$n",
    );
    pidns_until("the command did not start", 10, || {
        daemon.file("ready").exists()
    });
    ordering_ok(&daemon, &["signal", "-s", "USR1", &id]);
    std::thread::sleep(Duration::from_millis(500));
    std::fs::write(daemon.file("stop"), "").unwrap();
    let (record, lines) = pidns_ended(&daemon, &id);
    assert_eq!(record["state"], "Succeeded", "{record}");
    assert_eq!(lines, ["count=1"]);
    let (record, lines) = pidns_finish(
        &daemon,
        &PIDNS,
        "n=0; trap 'n=$((n+1))' USR1; kill -USR1 1; sleep 0.3; kill -USR1 1; sleep 0.3; kill -TERM 1; kill -KILL 1; sleep 0.3; echo count=$n",
    );
    assert_eq!(record["state"], "Failed", "{record}");
    assert_eq!(record["result"]["signal"], libc::SIGTERM);
    assert_eq!(lines, Vec::<String>::new());
    let (record, lines) = pidns_finish(
        &daemon,
        &PIDNS,
        "n=0; trap 'n=$((n+1))' USR1; kill -USR1 1; sleep 0.3; kill -USR1 1; sleep 0.3; kill -KILL 1; sleep 0.3; echo count=$n",
    );
    assert_eq!(record["state"], "Succeeded", "{record}");
    assert_eq!(lines, ["count=2"]);
}

#[test]
fn pidns_cancel_ends_a_command_that_ignores_term_and_its_whole_tree() {
    let daemon = Daemon::with_profile("pidns-cancel", "ordinary");
    let id = pidns_start(
        &daemon,
        &pidns_options(&["--time", "40s"]),
        "trap '' TERM; sleep 28.4321 & : > ready; wait",
    );
    pidns_until("the command did not start", 10, || {
        daemon.file("ready").exists()
    });
    assert!(pidns_on_host("28.4321"));
    let begun = Instant::now();
    ordering_ok(&daemon, &["cancel", &id]);
    let (record, _) = pidns_ended(&daemon, &id);
    let took = begun.elapsed();
    assert_eq!(record["state"], "Cancelled", "{record}");
    assert_eq!(record["result"]["signal"], libc::SIGKILL);
    assert!(took >= Duration::from_secs(9), "{took:?}");
    assert!(took < Duration::from_secs(16), "{took:?}");
    pidns_until("the tree stayed", 3, || !pidns_on_host("28.4321"));
    let graceful = pidns_start(
        &daemon,
        &pidns_options(&["--time", "40s"]),
        "trap 'echo graceful; exit 0' TERM; : > ready2; sleep 27.4321 & wait",
    );
    pidns_until("the command did not start", 10, || {
        daemon.file("ready2").exists()
    });
    let begun = Instant::now();
    ordering_ok(&daemon, &["cancel", &graceful]);
    let (record, lines) = pidns_ended(&daemon, &graceful);
    assert!(begun.elapsed() < Duration::from_secs(8));
    assert_eq!(record["state"], "Cancelled", "{record}");
    assert_eq!(lines, ["graceful"]);
    pidns_until("the tree stayed", 3, || !pidns_on_host("27.4321"));
    pidns_no_init_stays(&daemon);
}

#[test]
fn pidns_init_reaps_two_hundred_orphans() {
    let daemon = Daemon::with_profile("pidns-orphans", "ordinary");
    let (record, lines) = pidns_finish(
        &daemon,
        &PIDNS,
        "i=0; while [ $i -lt 200 ]; do (true &); i=$((i+1)); done; sleep 0.5; z=0; for s in /proc/[0-9]*/status; do while read -r key value rest; do [ \"$key\" = State: ] && [ \"$value\" = Z ] && z=$((z+1)); done < $s; done 2>/dev/null; echo zombies=$z; echo /proc/[0-9]*",
    );
    assert_eq!(record["state"], "Succeeded", "{record}");
    assert_eq!(lines, ["zombies=0", "/proc/1 /proc/2"]);
    assert_eq!(record["result"]["leftover_processes"], 0);
}

#[test]
fn pidns_combines_with_read_only_root_private_tmp_and_writable_paths() {
    let daemon = Daemon::with_profile("pidns-mounts", "ordinary");
    let area = isolation_area("pidns-mounts");
    let cwd = area.join("cwd");
    let writable = area.join("writable");
    std::fs::create_dir(&cwd).unwrap();
    std::fs::create_dir(&writable).unwrap();
    let script = format!(
        "echo $$; test -r /proc/self/status && echo self; touch denied 2>/dev/null || echo read-only; touch '{}/ok' && echo writable; ls /tmp | wc -l; touch /tmp/t && echo tmp; (echo 0 > /proc/sys/kernel/ns_last_pid) 2>/dev/null || echo proc-read-only; echo /proc/[0-9]*",
        writable.display()
    );
    let (record, lines) = pidns_finish(
        &daemon,
        &pidns_options(&[
            "--dir",
            cwd.to_str().unwrap(),
            "--root",
            "read-only",
            "--private-tmp",
            "yes",
            "--writable",
            writable.to_str().unwrap(),
        ]),
        &script,
    );
    assert_eq!(record["state"], "Succeeded", "{record}");
    assert_eq!(
        lines,
        [
            "2",
            "self",
            "read-only",
            "writable",
            "0",
            "tmp",
            "proc-read-only",
            "/proc/1 /proc/2"
        ]
    );
    assert!(writable.join("ok").exists());
    assert!(!cwd.join("denied").exists());
    let applied = &record["result"]["isolation_controls"];
    assert_eq!(applied["root_read_only"], true);
    assert_eq!(applied["pid_namespace"]["command_pid"], 2);
    assert!(
        record["result"]["notes"]
            .as_array()
            .is_none_or(Vec::is_empty)
    );
    let (record, lines) = pidns_finish(
        &daemon,
        &pidns_options(&["--dir", cwd.to_str().unwrap(), "--private-tmp", "yes"]),
        "echo $$; ls /tmp | wc -l; touch kept && echo writable",
    );
    assert_eq!(record["state"], "Succeeded", "{record}");
    assert_eq!(lines, ["2", "0", "writable"]);
    std::fs::remove_dir_all(&area).unwrap();
}

#[test]
fn pidns_combines_with_security_process_and_network_controls() {
    let daemon = Daemon::with_profile("pidns-controls", "ordinary");
    let status = "grep -E '^(Seccomp|NoNewPrivs|CapEff):' /proc/self/status | tr -d '\\t' | tr '\\n' ' '; echo; grep -E '^(Seccomp|NoNewPrivs):' /proc/1/status | tr -d '\\t' | tr '\\n' ' '; echo";
    let (record, lines) = pidns_finish(
        &daemon,
        &pidns_options(&[
            "--seccomp-deny",
            "ptrace,mount",
            "--cap-drop",
            "all",
            "--no-new-privs",
            "yes",
            "--rlimit",
            "nofile=64",
        ]),
        &format!("echo $$; ulimit -n; {status}; (true &); sleep 0.2; echo /proc/[0-9]*; exit 7"),
    );
    assert_eq!(record["state"], "Failed", "{record}");
    assert_eq!(record["result"]["exit_code"], 7);
    assert_eq!(
        lines,
        [
            "2",
            "64",
            "CapEff:0000000000000000 NoNewPrivs:1 Seccomp:2 ",
            "NoNewPrivs:0 Seccomp:0 ",
            "/proc/1 /proc/2"
        ]
    );
    assert!(record["result"]["security_controls"].is_object());
    assert!(record["result"]["process_controls"].is_object());
    let service_net = std::fs::read_link(format!("/proc/{}/ns/net", daemon.child.id())).unwrap();
    let (record, lines) = pidns_finish(
        &daemon,
        &pidns_options(&["--net", "none"]),
        "echo $$; readlink /proc/self/ns/net; echo /proc/[0-9]*; cat /proc/net/dev | grep -c :",
    );
    assert_eq!(record["state"], "Succeeded", "{record}");
    assert_eq!(lines[0], "2");
    assert_ne!(lines[1], service_net.to_string_lossy());
    assert_eq!(lines[2..], ["/proc/1 /proc/2", "1"]);
    assert_eq!(
        record["result"]["isolation_controls"]["user_namespace_from_network"],
        true
    );
    let host: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["host", "--json"])).unwrap();
    if !host["isolation_controls"]["landlock_abi"].is_null() {
        let probe = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("pidns-confined-{}", std::process::id()));
        let (record, lines) = pidns_finish(
            &daemon,
            &pidns_options(&["--confine"]),
            &format!(
                "echo $$; touch inside && echo inside; touch '{}' 2>/dev/null || echo confined; (true &); sleep 0.2; echo /proc/[0-9]*",
                probe.display()
            ),
        );
        assert_eq!(record["state"], "Succeeded", "{record}");
        assert_eq!(lines, ["2", "inside", "confined", "/proc/1 /proc/2"]);
        assert!(!probe.exists());
    }
}

#[test]
fn pidns_refuses_requests_without_mount_or_user_and_says_what_to_add() {
    let daemon = Daemon::with_profile("pidns-refusals", "ordinary");
    assert_ne!(unsafe { libc::geteuid() }, 0);
    for (list, expected) in [
        (
            "pid",
            "pid needs mount in --namespaces, because the job gets its own /proc; add mount",
        ),
        ("user,pid", "add mount"),
        ("ipc,pid,uts", "add mount"),
        (
            "mount,pid",
            "this service is not privileged, so --namespaces needs user in its list; add user",
        ),
    ] {
        let refused = isolation_submit(&daemon, &["--namespaces", list], "touch pidns-ran");
        assert!(!refused.status.success(), "{list}");
        let message = String::from_utf8_lossy(&refused.stderr);
        assert!(message.contains(expected), "{list}: {message}");
    }
    ordering_ok(
        &daemon,
        &["queue", "create", "lone", "--job-namespaces", "pid"],
    );
    let refused = isolation_submit(&daemon, &["--queue", "lone"], "touch pidns-ran");
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("add mount"));
    std::thread::sleep(Duration::from_millis(200));
    assert!(!daemon.file("pidns-ran").exists());
    let listed: serde_json::Value =
        serde_json::from_str(&ordering_ok(&daemon, &["list", "--all", "--json"])).unwrap();
    assert_eq!(listed["jobs"].as_array().map_or(0, Vec::len), 0, "{listed}");
}

#[test]
fn pidns_queue_default_profile_inherit_and_retry_keep_the_namespace() {
    let daemon = Daemon::with_profile("pidns-defaults", "ordinary");
    std::fs::write(daemon.base.join("config.toml"),"schema_version=1\nprofile='ordinary'\n[[presets.profiles]]\nname='alone'\nrevision=1\n[presets.profiles.values]\nnamespaces='mount,pid,user'\n").unwrap();
    ordering_ok(&daemon, &["config", "reload"]);
    ordering_ok(
        &daemon,
        &["queue", "create", "q", "--job-namespaces", "user,mount,pid"],
    );
    ordering_ok(
        &daemon,
        &["queue", "create", "p", "--job-execution-profile", "alone@1"],
    );
    let (queued, lines) = pidns_finish(&daemon, &["--queue", "q"], "echo $$");
    assert_eq!(lines, ["2"]);
    assert_eq!(
        queued["resource_sources"]["namespaces"]["Object"]["path"],
        "q"
    );
    assert_eq!(
        queued["result"]["isolation_controls"]["pid_namespace"]["command_pid"],
        2
    );
    let (reset, lines) = pidns_finish(
        &daemon,
        &["--queue", "q", "--namespaces", "inherit"],
        "echo $$",
    );
    assert_ne!(lines, ["2"]);
    assert!(reset["result"]["isolation_controls"].is_null());
    let (profiled, lines) = pidns_finish(&daemon, &["--queue", "p"], "echo $$");
    assert_eq!(lines, ["2"]);
    assert!(profiled["resource_sources"]["namespaces"]["Preset"].is_object());
    let id = queued["id"].to_string();
    ordering_ok(&daemon, &["retry", &id]);
    let (retried, _) = pidns_ended(&daemon, &id);
    assert_eq!(retried["attempt"], 2);
    assert_eq!(retried["state"], "Succeeded");
    assert_eq!(
        retried["result"]["isolation_controls"]["namespaces"],
        serde_json::json!(["mount", "pid", "user"])
    );
    let output = text(&daemon.job(&["logs", &id, "--stream", "stdout", "--raw"]));
    assert_eq!(output.lines().last(), Some("2"));
}

#[test]
fn pidns_job_survives_a_daemon_restart_and_can_still_be_signalled_and_cancelled() {
    let mut daemon = Daemon::with_profile("pidns-restart", "ordinary");
    let id = pidns_start(
        &daemon,
        &pidns_options(&["--time", "40s"]),
        "n=0; trap 'n=$((n+1)); echo got $n' USR1; : > ready; (trap '' USR1; exec sleep 26.4321) & while :; do wait $!; [ $n -ge 1 ] && break; done; echo after; wait",
    );
    pidns_until("the command did not start", 10, || {
        daemon.file("ready").exists()
    });
    daemon.restart();
    assert_eq!(pidns_record(&daemon, &id)["state"], "Running");
    assert!(pidns_on_host("26.4321"));
    ordering_ok(&daemon, &["signal", "-s", "USR1", &id]);
    pidns_until("the signal did not arrive", 5, || {
        text(&daemon.job(&["logs", &id, "--stream", "stdout", "--raw"])).contains("got 1")
    });
    assert_eq!(pidns_record(&daemon, &id)["state"], "Running");
    let short = pidns_start(&daemon, &pidns_options(&["--time", "40s"]), "sleep 25.4321");
    pidns_until("the second command did not start", 10, || {
        pidns_on_host("25.4321")
    });
    daemon.restart();
    ordering_ok(&daemon, &["cancel", &short]);
    let (record, _) = pidns_ended(&daemon, &short);
    assert_eq!(record["state"], "Cancelled", "{record}");
    assert_eq!(record["result"]["signal"], libc::SIGTERM);
    pidns_until("the tree stayed", 3, || !pidns_on_host("25.4321"));
    ordering_ok(&daemon, &["cancel", &id]);
    let (record, lines) = pidns_ended(&daemon, &id);
    assert_eq!(record["state"], "Cancelled", "{record}");
    assert_eq!(lines, ["got 1", "after"]);
    pidns_until("the tree stayed", 3, || !pidns_on_host("26.4321"));
    pidns_no_init_stays(&daemon);
}

#[test]
fn pidns_nested_client_reaches_the_service_from_inside_the_namespace() {
    let daemon = Daemon::with_profile("pidns-nested", "ordinary");
    let (record, lines) = pidns_finish(
        &daemon,
        &PIDNS,
        &format!(
            "echo $$; '{job}' status \"$JOB_ID\" --json | grep -c '\"state\"'; inner=$('{job}' submit -- 'echo nested-ran > nested'); '{job}' wait \"$inner\" && echo waited",
            job = env!("CARGO_BIN_EXE_job")
        ),
    );
    assert_eq!(record["state"], "Succeeded", "{record}");
    assert_eq!(lines[0], "2");
    assert_eq!(
        lines.last().map(String::as_str),
        Some("waited"),
        "{lines:?}"
    );
    assert_eq!(
        std::fs::read_to_string(daemon.file("nested"))
            .unwrap()
            .trim(),
        "nested-ran"
    );
}

#[test]
fn pidns_remote_job_applies_on_the_destination_and_never_to_the_transport() {
    use std::os::unix::fs::PermissionsExt;
    let remote = Daemon::with_profile("pidns-remote-peer", "ordinary");
    let mut local = Daemon::with_profile("pidns-remote-local", "ordinary");
    let bin = local.base.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let seen = local.base.join("transport-namespaces");
    let ssh = bin.join("ssh");
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\n{{ readlink /proc/self/ns/pid; echo $$; }} >> '{}'\nexec env JOB_STATE_DIR='{}' '{}' remote\n",
            seen.display(),
            remote.state.display(),
            env!("CARGO_BIN_EXE_job")
        ),
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = format!("{}:/usr/bin:/bin", bin.display());
    local.child.kill().unwrap();
    local.child.wait().unwrap();
    local.child = Command::new(env!("CARGO_BIN_EXE_job"))
        .arg("daemon")
        .env("JOB_CGROUP_ROOT", local.base.join("absent-cgroup"))
        .env("JOB_CONFIG", local.base.join("config.toml"))
        .env("JOB_STATE_DIR", &local.state)
        .env("PATH", &path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    local.ready();
    let service = std::fs::read_link(format!("/proc/{}/ns/pid", local.child.id()))
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let submitted = local
        .job_command(&[
            "submit",
            "--on",
            "test-peer",
            "--namespaces",
            "user,mount,pid",
            "--",
            "sh",
            "-c",
            "readlink /proc/self/ns/pid; echo $$; echo /proc/[0-9]*",
        ])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(
        submitted.status.success(),
        "{}",
        String::from_utf8_lossy(&submitted.stderr)
    );
    let id = text(&submitted).trim().to_owned();
    assert!(local.job(&["wait", &id]).status.success());
    let payload = text(&local.job(&["logs", &id, "--raw", "--stream", "stdout"]));
    let payload: Vec<&str> = payload.lines().collect();
    assert_ne!(payload[0], service);
    assert_eq!(payload[1..], ["2", "/proc/1 /proc/2"]);
    let transport = std::fs::read_to_string(&seen).unwrap();
    let transport: Vec<&str> = transport.lines().collect();
    assert!(transport.len() >= 4, "{transport:?}");
    for pair in transport.chunks(2) {
        assert_eq!(pair[0], service, "{transport:?}");
        assert!(pair[1].parse::<u32>().unwrap() > 2, "{transport:?}");
    }
    let record: serde_json::Value =
        serde_json::from_slice(&local.job(&["status", &id, "--json"]).stdout).unwrap();
    assert_eq!(
        record["result"]["isolation_controls"]["pid_namespace"]["command_pid"],
        2
    );
}
