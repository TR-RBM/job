use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
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
        Self::configured(name, "", "")
    }

    fn with_failpoint(name: &str, failpoint: &str) -> Daemon {
        Self::configured(name, "", failpoint)
    }

    fn configured(name: &str, config: &str, failpoint: &str) -> Daemon {
        let base = std::env::temp_dir().join(format!("job-durable-{}-{name}", std::process::id()));
        let state = base.join("state");
        let work = base.join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(
            base.join("config.toml"),
            format!("schema_version = 1\nprofile = 'ordinary'\n{config}"),
        )
        .unwrap();
        let child = Self::spawn(&base, failpoint);
        let mut daemon = Daemon {
            child,
            base,
            state,
            work,
        };
        daemon.ready();
        daemon
    }

    fn spawn(base: &std::path::Path, failpoint: &str) -> Child {
        let mut command = Command::new(env!("CARGO_BIN_EXE_job"));
        command
            .arg("daemon")
            .env("JOB_CGROUP_ROOT", base.join("absent-cgroup"))
            .env("JOB_CONFIG", base.join("config.toml"))
            .env("JOB_STATE_DIR", base.join("state"))
            .env("LC_ALL", "C")
            .env_remove("JOB_FAILPOINT")
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(base.join("daemon.err"))
                    .unwrap(),
            ));
        if !failpoint.is_empty() {
            command.env("JOB_FAILPOINT", failpoint);
        }
        command.spawn().unwrap()
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(self.state.join("daemon.sock"));
    }

    fn restart(&mut self, failpoint: &str) {
        self.stop();
        self.child = Self::spawn(&self.base, failpoint);
        self.ready();
    }

    fn died(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "the daemon did not stop");
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::remove_file(self.state.join("daemon.sock"));
    }

    fn ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "daemon exited during startup: {}",
                std::fs::read_to_string(self.base.join("daemon.err")).unwrap_or_default()
            );
            if let Ok(mut stream) = UnixStream::connect(self.state.join("daemon.sock")) {
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut response = String::new();
                if writeln!(stream, "\"Ping\"").is_ok()
                    && stream.read_to_string(&mut response).is_ok()
                    && response.contains("Pong")
                {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("daemon did not become ready");
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_job"));
        command
            .args(args)
            .env("JOB_STATE_DIR", &self.state)
            .env("JOB_CONFIG", self.base.join("config.toml"))
            .env("JOB_SESSION", "durable")
            .env("LC_ALL", "C")
            .env_remove("JOB_FAILPOINT")
            .env_remove("JOB_CLI_COMPAT")
            .current_dir(&self.work);
        command
    }

    fn job(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
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

    fn status(&self, id: &str) -> serde_json::Value {
        let output = self.job(&["status", id, "--json"]);
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)))
    }

    fn record(&self, id: &str) -> serde_json::Value {
        serde_json::from_slice(
            &std::fs::read(self.state.join("jobs").join(id).join("job.json")).unwrap(),
        )
        .unwrap()
    }

    fn until(&self, id: &str, wanted: &[&str]) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let status = self.status(id);
            if wanted.iter().any(|state| status["state"] == *state) {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "job {id} stayed {}",
                status["state"]
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn rpc(&self, request: serde_json::Value) -> serde_json::Value {
        let mut stream = UnixStream::connect(self.state.join("daemon.sock")).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        writeln!(stream, "{request}").unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

#[test]
fn durable_startup_removes_stale_submission_stages() {
    let mut daemon = Daemon::start("sweep");
    daemon.stop();
    let stage = daemon.state.join(".transactions").join("7-1-1");
    std::fs::create_dir_all(&stage).unwrap();
    std::fs::write(stage.join("job.json"), "{}").unwrap();
    daemon.restart("");
    assert!(!stage.exists());
    assert!(daemon.state.join(".transactions").is_dir());
    let id = daemon.ok(&["submit", "--", "true"]);
    assert!(daemon.job(&["wait", &id]).status.success());
}

#[test]
fn durable_failpoints_refuse_an_unknown_name() {
    let base = std::env::temp_dir().join(format!("job-durable-{}-unknown", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(
        base.join("config.toml"),
        "schema_version = 1\nprofile = 'ordinary'\n",
    )
    .unwrap();
    let mut child = Daemon::spawn(&base, "no-such-point");
    let status = child.wait().unwrap();
    assert!(!status.success());
    assert!(
        std::fs::read_to_string(base.join("daemon.err"))
            .unwrap()
            .contains("no-such-point")
    );
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn durable_stale_result_does_not_finalize_a_retry() {
    let daemon = Daemon::start("stale-result");
    let id = daemon.ok(&[
        "submit",
        "--",
        "sh",
        "-c",
        "if [ -e second ]; then sleep 30; else exit 3; fi",
    ]);
    assert_eq!(daemon.until(&id, &["Failed"])["attempt"], 1);
    std::fs::write(daemon.work.join("second"), "").unwrap();
    daemon.ok(&["retry", &id]);
    assert_eq!(daemon.until(&id, &["Running"])["attempt"], 2);
    let directory = daemon.state.join("jobs").join(&id);
    let archived = directory.join("attempts/1/result.json");
    let stale: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&archived).unwrap()).unwrap();
    assert_eq!(stale["attempt"], 1);
    std::fs::write(
        directory.join("result.json"),
        std::fs::read(&archived).unwrap(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while directory.join("result.json").exists() {
        assert!(
            Instant::now() < deadline,
            "the stale result stayed in place"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(directory.join("attempts/1/late-result.json").exists());
    let status = daemon.status(&id);
    assert_eq!(status["state"], "Running");
    assert_eq!(status["attempt"], 2);
    assert!(status["result"].is_null());
    daemon.ok(&["cancel", &id]);
    daemon.until(&id, &["Cancelled"]);
    let attempts = daemon.ok(&["attempts", &id, "--json"]);
    assert!(attempts.contains("\"attempts\""));
}

const ONCE: &str = "echo run >> runs";

fn runs(daemon: &Daemon) -> usize {
    std::fs::read_to_string(daemon.work.join("runs"))
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

fn launch_crash(name: &str, failpoint: &str, state: &str, recorded_pid: bool) {
    launch_crash_with(name, failpoint, state, recorded_pid, &[]);
}

fn launch_crash_with(
    name: &str,
    failpoint: &str,
    state: &str,
    recorded_pid: bool,
    options: &[&str],
) {
    let mut daemon = Daemon::with_failpoint(name, failpoint);
    let mut args = vec!["submit"];
    args.extend_from_slice(options);
    args.extend(["--", "sh", "-c", ONCE]);
    let submitted = daemon.job(&args);
    assert!(!submitted.status.success());
    daemon.died();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(runs(&daemon), 0);
    let record = daemon.record("1");
    assert_eq!(record["state"], state);
    assert_eq!(record["shim_pid"].is_null(), !recorded_pid);
    assert_eq!(record["launch_gated"], true);
    let directory = daemon.state.join("jobs/1");
    assert!(!directory.join("result.json").exists());
    assert!(!directory.join("started.json").exists());
    daemon.restart("");
    let status = daemon.until("1", &["Succeeded", "Failed", "Lost"]);
    assert_eq!(status["state"], "Succeeded");
    assert_eq!(status["attempt"], 1);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(runs(&daemon), 1);
    assert!(directory.join("started.json").exists());
}

#[test]
fn durable_crash_after_the_starting_record_requeues_without_executing() {
    launch_crash(
        "launch-starting",
        "launch-after-starting",
        "Starting",
        false,
    );
}

#[test]
fn durable_crash_after_spawn_never_executes_without_a_recorded_supervisor() {
    launch_crash("launch-spawn", "launch-after-spawn", "Starting", false);
}

#[test]
fn durable_crash_after_the_identity_record_requeues_the_unconfirmed_launch() {
    launch_crash("launch-identity", "launch-after-identity", "Running", true);
}

#[test]
fn durable_failed_identity_save_keeps_the_job_queued_and_runs_it_once() {
    let daemon = Daemon::with_failpoint("identity-save", "launch-identity-save@1");
    let id = daemon.ok(&["submit", "--", "sh", "-c", ONCE]);
    let status = daemon.until(&id, &["Succeeded", "Failed", "Lost"]);
    assert_eq!(status["state"], "Succeeded");
    assert_eq!(status["attempt"], 1);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(runs(&daemon), 1);
    assert!(
        std::fs::read_to_string(daemon.base.join("daemon.err"))
            .unwrap()
            .contains("the launch could not be recorded; the Job stays queued")
    );
}

#[test]
fn durable_identity_save_that_keeps_failing_ends_as_a_start_error() {
    let daemon = Daemon::with_failpoint("identity-save-always", "launch-identity-save");
    let id = daemon.ok(&["submit", "--", "sh", "-c", ONCE]);
    let status = daemon.until(&id, &["Succeeded", "Failed", "Lost"]);
    assert_eq!(status["state"], "Failed");
    assert!(
        status["result"]["start_error"]
            .as_str()
            .unwrap()
            .contains("the command never started")
    );
    assert_eq!(runs(&daemon), 0);
}

#[test]
fn durable_exit_status_survives_a_supervisor_killed_during_cleanup() {
    let daemon = Daemon::with_failpoint("exit-record", "shim-after-exit-record");
    let failed = daemon.ok(&["submit", "--", "sh", "-c", "exit 7"]);
    let status = daemon.until(&failed, &["Succeeded", "Failed", "Lost"]);
    assert_eq!(status["state"], "Failed");
    assert_eq!(status["result"]["exit_code"], 7);
    assert!(
        status["result"]["notes"][0]
            .as_str()
            .unwrap()
            .contains("before cleanup completed")
    );
    assert!(
        !daemon
            .state
            .join("jobs")
            .join(&failed)
            .join("result.json")
            .exists()
    );
    let passed = daemon.ok(&["submit", "--", "true"]);
    let status = daemon.until(&passed, &["Succeeded", "Failed", "Lost"]);
    assert_eq!(status["state"], "Succeeded");
    assert_eq!(status["result"]["exit_code"], 0);
    let killed = daemon.ok(&["submit", "--", "sh", "-c", "kill -9 $$"]);
    let status = daemon.until(&killed, &["Succeeded", "Failed", "Lost"]);
    assert_eq!(status["state"], "Failed");
    assert_eq!(status["result"]["signal"], 9);
}

#[test]
fn durable_failed_result_write_is_retried_and_logged() {
    let daemon = Daemon::with_failpoint("result-retry", "shim-result-save@1");
    let id = daemon.ok(&["submit", "--", "sh", "-c", "exit 5"]);
    let status = daemon.until(&id, &["Succeeded", "Failed", "Lost"]);
    assert_eq!(status["state"], "Failed");
    assert_eq!(status["result"]["exit_code"], 5);
    assert!(status["result"]["notes"].is_null());
    let log =
        std::fs::read_to_string(daemon.state.join("jobs").join(&id).join("shim.err")).unwrap();
    assert!(log.contains("cannot write the result (1/2)"), "{log}");
    assert!(!log.contains("(2/2)"), "{log}");
}

#[test]
fn durable_result_write_that_keeps_failing_still_records_the_exit_status() {
    let daemon = Daemon::with_failpoint("result-fails", "shim-result-save");
    let id = daemon.ok(&["submit", "--", "sh", "-c", "exit 5"]);
    let status = daemon.until(&id, &["Succeeded", "Failed", "Lost"]);
    assert_eq!(status["state"], "Failed");
    assert_eq!(status["result"]["exit_code"], 5);
    assert!(!status["result"]["notes"].is_null());
    let log =
        std::fs::read_to_string(daemon.state.join("jobs").join(&id).join("shim.err")).unwrap();
    assert!(log.contains("cannot write the result (2/2)"), "{log}");
}

#[test]
fn durable_final_record_write_is_retried() {
    let daemon = Daemon::with_failpoint("final-save", "finalize-save@1");
    let id = daemon.ok(&["submit", "--", "true"]);
    assert_eq!(
        daemon.until(&id, &["Succeeded", "Failed", "Lost"])["state"],
        "Succeeded"
    );
    assert_eq!(daemon.record(&id)["state"], "Succeeded");
    assert!(
        std::fs::read_to_string(daemon.base.join("daemon.err"))
            .unwrap()
            .contains("cannot write the final record (1/2)")
    );
}

fn audit(daemon: &Daemon, args: &[&str]) -> Vec<serde_json::Value> {
    let mut all = vec!["audit", "--json"];
    all.extend_from_slice(args);
    daemon
        .ok(&all)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn uid() -> u64 {
    u64::from(unsafe { libc::getuid() })
}

#[test]
fn durable_audit_records_submit_signal_suspend_cancel_and_retry_with_the_peer() {
    let daemon = Daemon::start("audit-jobs");
    let submitted = daemon
        .command(&["submit", "--", "sleep", "30"])
        .env("DURABLE_SECRET_TOKEN", "hunter2-value")
        .output()
        .unwrap();
    assert!(submitted.status.success());
    let id = String::from_utf8(submitted.stdout)
        .unwrap()
        .trim()
        .to_owned();
    daemon.until(&id, &["Running"]);
    daemon.ok(&["signal", "-s", "CONT", &id]);
    assert!(!daemon.job(&["suspend", &id]).status.success());
    daemon.ok(&["cancel", &id]);
    daemon.until(&id, &["Cancelled"]);
    daemon.ok(&["retry", &id, "--hold"]);
    let entries = audit(&daemon, &["--target", &id]);
    let actions: Vec<_> = entries
        .iter()
        .map(|entry| entry["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["submit", "signal", "suspend", "cancel", "retry"]);
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(entry["schema_version"], 1);
        assert_eq!(entry["seq"], index as u64 + 1);
        assert_eq!(entry["peer_uid"], uid());
        assert!(entry["peer_pid"].as_i64().unwrap() > 1);
        assert_eq!(entry["target"], id.as_str());
        assert!(entry["at_ms"].as_u64().unwrap() > 0);
    }
    assert_eq!(entries[0]["session"], "durable");
    assert_eq!(entries[0]["result"], "ok");
    assert!(
        entries[0]["params"]["env"]["names"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "DURABLE_SECRET_TOKEN")
    );
    assert_eq!(entries[0]["params"]["spec"]["argv"]["program"], "sleep");
    assert_eq!(entries[0]["params"]["spec"]["argv"]["count"], 2);
    assert_eq!(entries[1]["params"]["signal"], libc::SIGCONT);
    assert!(entries[1]["result"].as_str().unwrap().starts_with("ok"));
    assert!(entries[2]["result"].as_str().unwrap().starts_with("error"));
    assert_eq!(entries[4]["result"], "ok");
    let raw = std::fs::read_to_string(daemon.state.join("audit/current.jsonl")).unwrap();
    assert!(!raw.contains("hunter2-value"));
    assert_eq!(audit(&daemon, &["--action", "signal"]).len(), 1);
    assert!(audit(&daemon, &["--since", "99999999999999"]).is_empty());
    assert_eq!(audit(&daemon, &["--target", "no-such"]).len(), 0);
    let record = daemon.record(&id);
    assert_eq!(record["actor_uid"], uid());
    assert_eq!(record["attempt"], 2);
    let text = daemon.ok(&["audit", "--action", "retry"]);
    assert!(text.starts_with("seq\tat_ms\taction\ttarget"));
    assert_eq!(text.lines().count(), 2);
    assert!(!daemon.job(&["audit", "--bogus"]).status.success());
}

#[test]
fn durable_audit_records_object_changes_reload_reprioritize_update_and_removal() {
    let daemon = Daemon::start("audit-objects");
    daemon.ok(&["config", "reload"]);
    daemon.ok(&["group", "create", "team"]);
    daemon.ok(&["queue", "create", "batch"]);
    daemon.ok(&["group", "set", "--max-running", "2", "team"]);
    daemon.ok(&["queue", "move", "--group", "team", "batch"]);
    daemon.ok(&["group", "create", "spare"]);
    daemon.ok(&["group", "rm", "spare"]);
    let held = daemon.ok(&["create", "-q", "team/batch", "--", "true"]);
    daemon.ok(&["reprioritize", &held, "--priority", "3"]);
    let proxied = daemon.job(&[
        "create",
        "--net",
        "socks5://someone:secretpw@127.0.0.1:9",
        "--",
        "true",
    ]);
    let plan = serde_json::json!({
        "operation": "0123456789abcdef", "target": {"Job": {"id": 1}}, "object_id": null,
        "members": [], "boot_id": "none", "path": "/nonexistent", "device": 0, "inode": 0,
        "patch": {}, "allow_oom": false, "before": {}, "after": {},
        "config_before": null, "config_after": null, "steps": []
    });
    let refused = daemon.rpc(serde_json::json!({"Versioned": {"protocol": 20, "request": {
        "ResourceUpdate": {"target": {"Job": {"id": 1}}, "patch": {}, "allow_oom": false, "expected": plan}
    }}}));
    assert!(refused.get("Error").is_some(), "{refused}");
    daemon.ok(&["release", &held]);
    daemon.until(&held, &["Succeeded"]);
    daemon.ok(&["remove", &held]);
    let entries = audit(&daemon, &[]);
    let seen: Vec<(String, String)> = entries
        .iter()
        .map(|entry| {
            (
                entry["action"].as_str().unwrap().to_owned(),
                entry["target"].as_str().unwrap_or("-").to_owned(),
            )
        })
        .collect();
    for wanted in [
        ("config-reload", "-"),
        ("object-create", "team"),
        ("object-create", "batch"),
        ("object-set", "team"),
        ("object-move", "batch"),
        ("object-remove", "spare"),
        ("create", held.as_str()),
        ("reprioritize", held.as_str()),
        ("update", "1"),
        ("release", held.as_str()),
        ("remove", held.as_str()),
    ] {
        assert!(
            seen.contains(&(wanted.0.to_owned(), wanted.1.to_owned())),
            "{wanted:?} missing in {seen:?}"
        );
    }
    let find = |action: &str| {
        entries
            .iter()
            .find(|entry| entry["action"] == action)
            .unwrap()
    };
    assert!(
        find("update")["result"]
            .as_str()
            .unwrap()
            .starts_with("error")
    );
    assert_eq!(find("remove")["operation"], format!("job-{held}"));
    assert_eq!(
        find("object-set")["params"]["operation"]["Set"]["config"]["max_running"],
        2
    );
    assert!(entries.iter().all(|entry| entry["peer_uid"] == uid()));
    let raw = std::fs::read_to_string(daemon.state.join("audit/current.jsonl")).unwrap();
    assert!(!raw.contains("secretpw"), "{raw}");
    if proxied.status.success() {
        assert!(raw.contains("socks5://***@127.0.0.1:9"));
    }
    let receipt: serde_json::Value = serde_json::from_slice(
        &std::fs::read(daemon.state.join(format!("removals/job-{held}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["actor_uid"], uid());
}

#[test]
fn durable_audit_rotates_keeps_the_configured_files_and_says_what_was_dropped() {
    let daemon = Daemon::configured(
        "audit-rotation",
        "[audit]\nkeep_files = 2\nrotate_bytes = 1500\n",
        "",
    );
    for _ in 0..8 {
        daemon.ok(&["create", "--", "true"]);
    }
    let directory = daemon.state.join("audit");
    let mut names: Vec<String> = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.contains(&"current.jsonl".to_owned()));
    let current = std::fs::read_to_string(directory.join("current.jsonl")).unwrap();
    let first: serde_json::Value = serde_json::from_str(current.lines().next().unwrap()).unwrap();
    assert_eq!(first["action"], "audit-rotation");
    assert_eq!(first["params"]["keep_files"], 2);
    let rotated = names.iter().find(|name| *name != "current.jsonl").unwrap();
    assert_eq!(first["target"], rotated.as_str());
    let entries = audit(&daemon, &[]);
    let rotations: Vec<_> = entries
        .iter()
        .filter(|entry| entry["action"] == "audit-rotation")
        .collect();
    let dropped = rotations
        .iter()
        .rev()
        .find(|entry| {
            !entry["params"]["dropped_files"]
                .as_array()
                .unwrap()
                .is_empty()
        })
        .expect("no rotation dropped a file");
    let from = dropped["params"]["dropped_first_seq"].as_u64().unwrap();
    let to = dropped["params"]["dropped_last_seq"].as_u64().unwrap();
    assert!(from <= to);
    assert_eq!(
        format!("{}.jsonl", rotated.trim_end_matches(".jsonl")),
        *rotated
    );
    let sequence: Vec<u64> = entries
        .iter()
        .map(|entry| entry["seq"].as_u64().unwrap())
        .collect();
    assert!(sequence.windows(2).all(|pair| pair[1] == pair[0] + 1));
    assert!(sequence[0] > to);
    assert_eq!(
        rotated.trim_end_matches(".jsonl").parse::<u64>().unwrap(),
        sequence[0]
    );
    assert_eq!(
        entries
            .iter()
            .rfind(|entry| entry["action"] == "create")
            .unwrap()["target"],
        "8"
    );
}

#[test]
fn durable_audit_reader_and_writer_tolerate_a_torn_last_line() {
    let mut daemon = Daemon::start("audit-torn");
    daemon.ok(&["create", "--", "true"]);
    daemon.ok(&["create", "--", "true"]);
    daemon.stop();
    let path = daemon.state.join("audit/current.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(b"{\"seq\":3,\"at_ms\":17,\"action\":\"cre")
        .unwrap();
    drop(file);
    let read = daemon.job(&["audit", "--json"]);
    assert!(read.status.success());
    assert_eq!(String::from_utf8_lossy(&read.stdout).lines().count(), 2);
    assert!(String::from_utf8_lossy(&read.stderr).contains("could not be read"));
    daemon.restart("");
    daemon.ok(&["create", "--", "true"]);
    let entries = audit(&daemon, &[]);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry["seq"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(entries[2]["target"], "3");
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.lines().count(), 7);
    assert!(text.lines().nth(4).unwrap().ends_with("\"cre"));
}

fn job_count(daemon: &Daemon) -> usize {
    std::fs::read_dir(daemon.state.join("jobs"))
        .unwrap()
        .count()
}

fn keyed_request(daemon: &Daemon, key: &str, argv: &[&str]) -> serde_json::Value {
    serde_json::json!({"Versioned": {"protocol": 20, "request": {"Submit": {
        "spec": {"argv": argv, "cwd": daemon.work, "session": "raw", "declared": {"devices": []}},
        "env": {"vars": [["PATH", std::env::var("PATH").unwrap()]]},
        "idempotency_key": key
    }}}})
}

#[test]
fn durable_resubmission_after_a_dropped_reply_returns_the_recorded_job() {
    let daemon = Daemon::start("idem-dropped");
    let request = keyed_request(&daemon, "deploy:2026-10-05", &["true"]);
    let mut stream = UnixStream::connect(daemon.state.join("daemon.sock")).unwrap();
    writeln!(stream, "{request}").unwrap();
    drop(stream);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !daemon.state.join("jobs/1/job.json").exists() {
        assert!(Instant::now() < deadline, "the submission was not recorded");
        std::thread::sleep(Duration::from_millis(10));
    }
    let replay = daemon.rpc(request);
    assert_eq!(replay["Submitted"]["job"]["id"], 1);
    assert_eq!(replay["Submitted"]["job"]["replayed"], true);
    assert_eq!(
        replay["Submitted"]["job"]["idempotency_key"],
        "deploy:2026-10-05"
    );
    assert_eq!(job_count(&daemon), 1);
    let record = daemon.record("1");
    assert!(record.get("replayed").is_none());
    assert_eq!(record["spec_digest"].as_str().unwrap().len(), 64);
    let entries = audit(&daemon, &["--action", "submit"]);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["result"], "ok");
    assert_eq!(entries[1]["result"], "replayed");
    assert_eq!(entries[1]["target"], "1");
}

fn crash_then_resubmit(name: &str, failpoint: &str, published: bool) {
    let mut daemon = Daemon::with_failpoint(name, failpoint);
    let arguments = [
        "submit",
        "--idempotency-key",
        "nightly.1",
        "--",
        "sh",
        "-c",
        ONCE,
    ];
    assert!(!daemon.job(&arguments).status.success());
    daemon.died();
    assert_eq!(job_count(&daemon), usize::from(published));
    daemon.restart("");
    let id = daemon.ok(&arguments);
    assert_eq!(job_count(&daemon), 1);
    assert_eq!(daemon.ok(&arguments), id);
    assert_eq!(
        daemon.until(&id, &["Succeeded", "Failed", "Lost"])["state"],
        "Succeeded"
    );
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(runs(&daemon), 1);
    assert_eq!(job_count(&daemon), 1);
    assert_eq!(
        std::fs::read_dir(daemon.state.join("idempotency"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn durable_crash_after_publication_then_resubmission_leaves_one_job() {
    crash_then_resubmit("idem-published", "submit-after-publish", true);
}

#[test]
fn durable_crash_between_index_and_publication_reuses_the_orphaned_key() {
    crash_then_resubmit("idem-orphan", "submit-after-index", false);
}

#[test]
fn durable_parallel_clients_with_one_key_get_one_job() {
    let daemon = Daemon::start("idem-parallel");
    let ids: Vec<String> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    daemon.ok(&[
                        "submit",
                        "--idempotency-key",
                        "shared_key",
                        "--",
                        "sh",
                        "-c",
                        ONCE,
                    ])
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect()
    });
    assert!(ids.iter().all(|id| *id == ids[0]), "{ids:?}");
    assert_eq!(job_count(&daemon), 1);
    daemon.until(&ids[0], &["Succeeded"]);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(runs(&daemon), 1);
}

#[test]
fn durable_key_with_a_different_specification_is_refused_and_is_free_after_removal() {
    let daemon = Daemon::start("idem-different");
    let id = daemon.ok(&["submit", "--idempotency-key", "k", "--", "true"]);
    let other = daemon.job(&["submit", "--idempotency-key", "k", "--", "false"]);
    assert!(!other.status.success());
    assert!(
        String::from_utf8_lossy(&other.stderr)
            .contains("already belongs to a Job with a different specification")
    );
    let held = daemon.job(&["create", "--idempotency-key", "k", "--", "true"]);
    assert!(!held.status.success());
    assert_eq!(job_count(&daemon), 1);
    let replay = daemon.job(&["submit", "--idempotency-key", "k", "--", "true"]);
    assert_eq!(String::from_utf8_lossy(&replay.stdout).trim(), id);
    assert!(String::from_utf8_lossy(&replay.stderr).contains("no new Job was added"));
    daemon.until(&id, &["Succeeded"]);
    daemon.ok(&["remove", &id]);
    assert_eq!(
        std::fs::read_dir(daemon.state.join("idempotency"))
            .unwrap()
            .count(),
        0
    );
    let next = daemon.ok(&["submit", "--idempotency-key", "k", "--", "false"]);
    assert_ne!(next, id);
    for bad in ["", "has space", "a/b", &"x".repeat(129)] {
        assert!(
            !daemon
                .job(&["submit", "--idempotency-key", bad, "--", "true"])
                .status
                .success()
        );
    }
    let refused = daemon.rpc(keyed_request(&daemon, "not valid", &["true"]));
    assert!(refused.get("Error").is_some(), "{refused}");
    assert!(
        !daemon
            .job(&["edit", &next, "--idempotency-key", "k2", "--", "true"])
            .status
            .success()
    );
}

#[test]
fn durable_run_with_a_replayed_key_waits_for_the_recorded_job() {
    let daemon = Daemon::start("idem-run");
    let arguments = [
        "run",
        "--idempotency-key",
        "run-1",
        "--summary",
        "--",
        "sh",
        "-c",
        "echo run >> runs; exit 4",
    ];
    let first = daemon.job(&arguments);
    let second = daemon.job(&arguments);
    assert_eq!(
        first.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        second.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(runs(&daemon), 1);
    assert_eq!(job_count(&daemon), 1);
    let held = daemon.ok(&[
        "create",
        "--idempotency-key",
        "held-1",
        "--json",
        "--",
        "true",
    ]);
    let again = daemon.ok(&[
        "create",
        "--idempotency-key",
        "held-1",
        "--json",
        "--",
        "true",
    ]);
    let held: serde_json::Value = serde_json::from_str(&held).unwrap();
    let again: serde_json::Value = serde_json::from_str(&again).unwrap();
    assert_eq!(held["id"], again["id"]);
    assert!(held.get("replayed").is_none());
    assert_eq!(again["replayed"], true);
    assert_eq!(again["state"], "Held");
}

fn bare_submit(daemon: &Daemon) -> serde_json::Value {
    serde_json::json!({"Submit": {
        "spec": {"argv": ["true"], "cwd": daemon.work, "session": "raw", "declared": {"devices": []}},
        "env": {"vars": []}
    }})
}

fn versioned(protocol: u32, min: Option<u32>, request: serde_json::Value) -> serde_json::Value {
    match min {
        Some(min) => {
            serde_json::json!({"Versioned": {"protocol": protocol, "min": min, "request": request}})
        }
        None => serde_json::json!({"Versioned": {"protocol": protocol, "request": request}}),
    }
}

#[test]
fn durable_bare_reads_and_supervisor_messages_are_still_answered() {
    let daemon = Daemon::start("bare-read");
    assert!(daemon.rpc(serde_json::json!("Ping")).get("Pong").is_some());
    assert!(daemon.rpc(serde_json::json!("Host")).get("Host").is_some());
    assert!(
        daemon
            .rpc(serde_json::json!("Queue"))
            .get("Queue")
            .is_some()
    );
    assert!(
        daemon
            .rpc(serde_json::json!({"Done": {"id": 1}}))
            .get("Pong")
            .is_some()
    );
    let status = daemon.rpc(serde_json::json!({"Status": {"id": 1}}));
    assert_eq!(status["Error"]["message"], "there is no job 1");
    let preview = daemon.rpc(serde_json::json!({"Remove": {
        "target": {"Job": {"id": 1}}, "recursive": false, "allow_lost": false, "expected": null
    }}));
    assert!(preview.get("Error").is_some(), "{preview}");
}

#[test]
fn durable_bare_mutating_requests_are_refused_with_the_supported_range() {
    let daemon = Daemon::start("bare-mutating");
    for request in [
        bare_submit(&daemon),
        serde_json::json!({"Cancel": {"id": 1, "session": "raw"}}),
        serde_json::json!({"Signal": {"id": 1, "signal": 15}}),
        serde_json::json!({"Config": {"reload": true}}),
        serde_json::json!({"QueueAdd": {"name": "q", "change": {"parallel": null, "paused": null}}}),
    ] {
        let response = daemon.rpc(request.clone());
        assert_eq!(response["Unsupported"]["min"], 20, "{request}: {response}");
        assert_eq!(response["Unsupported"]["max"], 20);
        assert!(
            !response["Unsupported"]["version"]
                .as_str()
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(job_count(&daemon), 0);
    let entries = audit(&daemon, &["--action", "submit"]);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["result"], "error: unsupported protocol");
}

#[test]
fn durable_versioned_requests_outside_the_range_are_refused() {
    let daemon = Daemon::start("out-of-range");
    for request in [
        versioned(19, None, bare_submit(&daemon)),
        versioned(21, None, bare_submit(&daemon)),
        versioned(21, Some(21), bare_submit(&daemon)),
        versioned(19, Some(18), serde_json::json!("Ping")),
        versioned(20, Some(21), bare_submit(&daemon)),
        versioned(20, None, versioned(20, None, bare_submit(&daemon))),
    ] {
        let response = daemon.rpc(request.clone());
        assert_eq!(response["Unsupported"]["max"], 20, "{request}: {response}");
    }
    assert_eq!(job_count(&daemon), 0);
}

#[test]
fn durable_versioned_requests_inside_the_range_are_served() {
    let daemon = Daemon::start("in-range");
    for (index, request) in [
        versioned(20, None, bare_submit(&daemon)),
        versioned(20, Some(20), bare_submit(&daemon)),
        versioned(21, Some(20), bare_submit(&daemon)),
        versioned(20, Some(1), bare_submit(&daemon)),
    ]
    .into_iter()
    .enumerate()
    {
        let response = daemon.rpc(request);
        assert_eq!(
            response["Submitted"]["job"]["id"],
            index as u64 + 1,
            "{response}"
        );
    }
    assert!(
        daemon
            .rpc(versioned(20, None, serde_json::json!("Ping")))
            .get("Pong")
            .is_some()
    );
}

#[test]
fn durable_client_names_both_ranges_and_exits_125_when_refused() {
    let base = std::env::temp_dir().join(format!("job-durable-{}-refused", std::process::id()));
    std::fs::create_dir_all(base.join("state")).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(base.join("state/daemon.sock")).unwrap();
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        writeln!(
            stream,
            "{{\"Unsupported\":{{\"min\":30,\"max\":31,\"version\":\"9.9-test\"}}}}"
        )
        .unwrap();
        serde_json::from_str::<serde_json::Value>(&line).unwrap()
    });
    let output = Command::new(env!("CARGO_BIN_EXE_job"))
        .args(["cancel", "7"])
        .env("JOB_STATE_DIR", base.join("state"))
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    let wire = peer.join().unwrap();
    assert_eq!(wire["Versioned"]["protocol"], 20);
    assert_eq!(wire["Versioned"]["min"], 20);
    assert_eq!(wire["Versioned"]["request"]["Cancel"]["id"], 7);
    assert_eq!(output.status.code(), Some(125));
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("protocol 20 to 20"), "{message}");
    assert!(
        message.contains("the daemon speaks 30 to 31 (version 9.9-test)"),
        "{message}"
    );
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn durable_exit_status_survives_a_supervisor_killed_before_the_result() {
    let daemon = Daemon::with_failpoint("before-result", "shim-before-result");
    let id = daemon.ok(&["submit", "--", "sh", "-c", "echo printed; exit 9"]);
    let status = daemon.until(&id, &["Succeeded", "Failed", "Lost"]);
    assert_eq!(status["state"], "Failed");
    assert_eq!(status["result"]["exit_code"], 9);
    assert_eq!(status["result"]["attempt"], 1);
    let answer = daemon.job(&["status", &id]);
    let text = String::from_utf8_lossy(&answer.stdout);
    assert!(text.contains("before cleanup completed"), "{text}");
    let exit: serde_json::Value = serde_json::from_slice(
        &std::fs::read(daemon.state.join("jobs").join(&id).join("exit.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(exit["attempt"], 1);
    assert_eq!(exit["exit_code"], 9);
    assert!(exit["signal"].is_null());
    assert!(exit["at_ms"].as_u64().unwrap() > 0);
}

fn review_files_holding(root: &std::path::Path, needle: &str, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            review_files_holding(&path, needle, found);
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

fn review_private(daemon: &Daemon, path: &std::path::Path) -> bool {
    path.file_name().is_some_and(|name| name == "net-secret")
        || path.starts_with(daemon.state.join("net-secrets"))
}

fn review_linked_network_unavailable() -> Option<String> {
    let tools = [
        "unshare",
        "nsenter",
        "sleep",
        "ip",
        "tc",
        "nft",
        "slirp4netns",
    ];
    let on_path = |tool: &str| {
        std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(tool).is_file()))
    };
    if let Some(missing) = tools.iter().find(|tool| !on_path(tool)) {
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

#[test]
fn review_audit_journal_never_keeps_a_proxy_password_with_url_delimiters() {
    let mark = ["SECRET", "MARK"].concat();
    let daemon = Daemon::start("review-audit-userinfo");
    let mut requests = 0;
    for (index, delimiter) in ["/", "?", "#", " "].iter().enumerate() {
        let url = format!("socks5://user:pa{delimiter}{mark}@127.0.0.1:9");
        let queue = format!("q{index}");
        daemon.job(&["create", "--net", &url, "--", "true"]);
        daemon.job(&["queue", "create", &queue, "--net", &url]);
        daemon.ok(&["queue", "create", &format!("plain{index}")]);
        daemon.job(&["queue", "set", &format!("plain{index}"), "--net", &url]);
        daemon.job(&["group", "create", &format!("g{index}"), "--net", &url]);
        requests += 1;
    }
    let entries = audit(&daemon, &[]);
    let shown = entries
        .iter()
        .filter(|entry| entry.to_string().contains("socks5://***@127.0.0.1:9"))
        .count();
    assert!(shown >= requests * 4, "{shown} of {}", entries.len());
    let listed = daemon.ok(&["audit", "--json"]);
    assert!(!listed.contains(&mark), "{listed}");
    let mut holding = Vec::new();
    review_files_holding(&daemon.state, &mark, &mut holding);
    let leaked: Vec<_> = holding
        .iter()
        .filter(|path| !review_private(&daemon, path))
        .collect();
    assert!(leaked.is_empty(), "{leaked:?}");
}

#[test]
fn review_crash_after_publication_keeps_the_proxy_password_of_the_published_job() {
    if let Some(reason) = review_linked_network_unavailable() {
        eprintln!("skipped: {reason}");
        return;
    }
    let mark = ["SECRET", "MARK"].concat();
    let url = format!("socks5://user:{mark}@127.0.0.1:9");
    let mut daemon = Daemon::with_failpoint("review-secret-staged", "submit-after-publish");
    assert!(
        !daemon
            .job(&["create", "--net", &url, "--", "true"])
            .status
            .success()
    );
    daemon.died();
    assert_eq!(job_count(&daemon), 1);
    let record = daemon.record("1");
    let private = daemon.state.join("jobs/1/net-secret");
    assert_eq!(
        record["spec"]["declared"]["net_secret_file"],
        private.to_str().unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(&private).unwrap_or_default(),
        format!("user:{mark}\n")
    );
    daemon.restart("");
    assert_eq!(daemon.status("1")["state"], "Held");
    assert!(
        std::fs::read_dir(daemon.state.join(".transactions"))
            .unwrap()
            .next()
            .is_none()
    );
}

fn review_net_request(daemon: &Daemon, declared: serde_json::Value) -> serde_json::Value {
    daemon.rpc(
        serde_json::json!({"Versioned": {"protocol": 20, "request": {"Create": {
            "spec": {"argv": ["true"], "cwd": daemon.work, "session": "raw", "declared": declared},
            "env": {"vars": [["PATH", std::env::var("PATH").unwrap()]]}
        }}}}),
    )
}

#[test]
fn review_daemon_refuses_an_undeclared_proxy_scheme_and_relative_network_files() {
    let daemon = Daemon::start("review-net-validated");
    let secret = daemon.work.join("secret");
    std::fs::write(&secret, "user:password\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600)).unwrap();
    for (declared, expected) in [
        (
            serde_json::json!({"devices": [], "net": {"Proxy": "ftp://user:password@127.0.0.1:9"}}),
            "is not a network",
        ),
        (
            serde_json::json!({"devices": [], "net": {"Proxy": "socks5://127.0.0.1"}}),
            "names no port",
        ),
        (
            serde_json::json!({"devices": [], "net": {"Proxy": "none"}}),
            "is not a network",
        ),
        (
            serde_json::json!({"devices": [], "net": {"WireGuard": "relative.conf"}}),
            "must be an absolute path",
        ),
        (
            serde_json::json!({"devices": [], "net": {"OpenVpn": "relative.ovpn"}}),
            "must be an absolute path",
        ),
        (
            serde_json::json!({"devices": [], "net": {"Proxy": "socks5://127.0.0.1:9"}, "net_secret_file": "secret"}),
            "must be an absolute path",
        ),
    ] {
        let answer = review_net_request(&daemon, declared.clone());
        let said = answer["Error"]["message"].as_str().unwrap_or_default();
        assert!(said.contains(expected), "{declared}: {said}");
        assert!(!said.contains("password"), "{said}");
    }
    assert_eq!(job_count(&daemon), 0);
    for (settings, expected) in [
        (
            serde_json::json!({"net": {"Proxy": "ftp://user:password@127.0.0.1:9"}}),
            "is not a network",
        ),
        (
            serde_json::json!({"net": {"WireGuard": "relative.conf"}}),
            "must be an absolute path",
        ),
        (
            serde_json::json!({"net": {"Proxy": "socks5://127.0.0.1:9"}, "net_secret_file": "secret"}),
            "must be an absolute path",
        ),
    ] {
        let answer = daemon.rpc(
            serde_json::json!({"Versioned": {"protocol": 20, "request": {"QueueAdd": {
                "name": "raw", "change": {"settings": settings}
            }}}}),
        );
        let said = answer["Error"]["message"].as_str().unwrap_or_default();
        assert!(said.contains(expected), "{settings}: {said}");
        let answer = daemon.rpc(
            serde_json::json!({"Versioned": {"protocol": 20, "request": {"Object": {
                "kind": "queue",
                "operation": {"Create": {"path": "rawobject", "config": settings}}
            }}}}),
        );
        let said = answer["Error"]["message"].as_str().unwrap_or_default();
        assert!(said.contains(expected), "{settings}: {said}");
    }
    let listed = daemon.ok(&["queue", "list", "--json"]);
    assert!(!listed.contains("raw"), "{listed}");
}

#[test]
fn review_idempotency_digest_leaves_the_proxy_password_out() {
    if let Some(reason) = review_linked_network_unavailable() {
        eprintln!("skipped: {reason}");
        return;
    }
    let daemon = Daemon::start("review-idem-password");
    let submit = |password: &str, port: &str| {
        daemon.job(&[
            "create",
            "--idempotency-key",
            "proxy.1",
            "--net",
            &format!("socks5://user:{password}@127.0.0.1:{port}"),
            "--",
            "true",
        ])
    };
    let first = submit("first", "9");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let again = submit("second", "9");
    assert!(
        again.status.success(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    assert_eq!(first.stdout, again.stdout);
    assert_eq!(job_count(&daemon), 1);
    let other = submit("first", "10");
    assert!(!other.status.success());
    assert!(
        String::from_utf8_lossy(&other.stderr).contains("different specification"),
        "{}",
        String::from_utf8_lossy(&other.stderr)
    );
    let digest = daemon.record("1")["spec_digest"]
        .as_str()
        .unwrap()
        .to_owned();
    let plain = daemon.ok(&[
        "create",
        "--idempotency-key",
        "proxy.2",
        "--net",
        "socks5://127.0.0.1:9",
        "--",
        "true",
    ]);
    assert_eq!(
        daemon.record(&plain)["spec_digest"].as_str().unwrap().len(),
        64
    );
    assert_ne!(daemon.record(&plain)["spec_digest"], digest.as_str());
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn raw_lines(daemon: &Daemon) -> Vec<String> {
    std::fs::read_to_string(daemon.state.join("audit/current.jsonl"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn review_stray_temporary_file_in_the_idempotency_index_does_not_wedge_the_service() {
    let mut daemon = Daemon::start("review-stray");
    let first = daemon.ok(&["submit", "--idempotency-key", "one", "--", "true"]);
    daemon.until(&first, &["Succeeded"]);
    let stray = daemon
        .state
        .join("idempotency")
        .join(format!("{}.tmp4242", "a".repeat(64)));
    std::fs::write(&stray, "{\"key\":\"tw").unwrap();
    let second = daemon.ok(&["submit", "--idempotency-key", "two", "--", "true"]);
    daemon.until(&second, &["Succeeded"]);
    daemon.ok(&["remove", &first]);
    let third = daemon.ok(&["submit", "--idempotency-key", "three", "--", "true"]);
    assert_eq!(
        daemon.ok(&["submit", "--idempotency-key", "three", "--", "true"]),
        third
    );
    daemon.ok(&["create", "--", "true"]);
    assert!(stray.exists());
    daemon.stop();
    let state = daemon.state.to_str().unwrap().to_owned();
    let inventory: serde_json::Value =
        serde_json::from_str(&daemon.ok(&["state", "validate", "--source", &state])).unwrap();
    assert_eq!(
        inventory["leftover_temporary_files"][0],
        format!("idempotency/{}.tmp4242", "a".repeat(64))
    );
    daemon.restart("");
    assert!(!stray.exists());
    let errors = std::fs::read_to_string(daemon.base.join("daemon.err")).unwrap();
    assert!(
        errors.contains("removed leftover temporary files of the idempotency index: 1"),
        "{errors}"
    );
    assert_eq!(
        daemon.ok(&["submit", "--idempotency-key", "three", "--", "true"]),
        third
    );
    daemon.stop();
    let inventory: serde_json::Value =
        serde_json::from_str(&daemon.ok(&["state", "validate", "--source", &state])).unwrap();
    assert!(inventory.get("leftover_temporary_files").is_none());
    daemon.restart("");
}

#[test]
fn review_negotiation_answers_before_the_inner_request_is_read() {
    let daemon = Daemon::start("review-negotiation");
    let invented = serde_json::json!({"Frobnicate": {"depth": [1, 2], "mode": "x"}});
    for request in [
        versioned(99, None, invented.clone()),
        versioned(99, Some(99), invented.clone()),
        versioned(99, Some(21), serde_json::json!("Frobnicate")),
        versioned(19, None, invented.clone()),
    ] {
        let response = daemon.rpc(request.clone());
        assert_eq!(response["Unsupported"]["min"], 20, "{request}: {response}");
        assert_eq!(response["Unsupported"]["max"], 20, "{request}: {response}");
    }
    for request in [
        versioned(20, None, invented.clone()),
        versioned(20, Some(20), serde_json::json!("Frobnicate")),
        versioned(99, Some(20), invented.clone()),
    ] {
        let response = daemon.rpc(request.clone());
        let message = response["Error"]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("this service does not know request Frobnicate"),
            "{request}: {response}"
        );
        assert!(message.contains("protocol 20 to 20 (version "), "{message}");
    }
    let shape = daemon.rpc(versioned(
        20,
        None,
        serde_json::json!({"Cancel": {"id": "seven", "session": 4}}),
    ));
    let message = shape["Error"]["message"].as_str().unwrap();
    assert!(
        message.contains("this service cannot read request Cancel"),
        "{message}"
    );
    assert!(message.contains("protocol 20 to 20"), "{message}");
    let envelope = daemon.rpc(serde_json::json!({"Versioned": {"request": "Ping"}}));
    assert!(
        envelope["Error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("unreadable request"),
        "{envelope}"
    );
    assert!(daemon.rpc(serde_json::json!("Ping")).get("Pong").is_some());
    assert!(daemon.rpc(serde_json::json!("Host")).get("Host").is_some());
    assert!(
        daemon
            .rpc(serde_json::json!({"Done": {"id": 1}}))
            .get("Pong")
            .is_some()
    );
    let wait = daemon.rpc(serde_json::json!({"Wait": {"id": 1, "timeout_ms": 0}}));
    assert_eq!(wait["Error"]["message"], "there is no job 1");
    assert!(
        daemon
            .rpc(versioned(20, None, serde_json::json!("Ping")))
            .get("Pong")
            .is_some()
    );
    assert_eq!(job_count(&daemon), 0);
    let unknown = audit(&daemon, &["--action", "unknown-request"]);
    assert_eq!(unknown.len(), 4);
    assert_eq!(unknown[3]["target"], "Cancel");
    assert_eq!(unknown[0]["target"], "Frobnicate");
    assert_eq!(unknown[0]["peer_uid"], uid());
}

fn pty_request(
    daemon: &Daemon,
    key: &str,
    rows: u16,
    cols: u16,
    session: &str,
) -> serde_json::Value {
    serde_json::json!({"Versioned": {"protocol": 20, "request": {"Create": {
        "spec": {"argv": ["true"], "cwd": daemon.work, "session": session,
            "declared": {"devices": [], "terminal": {"rows": rows, "cols": cols}}},
        "env": {"vars": [["PATH", std::env::var("PATH").unwrap()], ["CHANGING", session]]},
        "idempotency_key": key
    }}}})
}

#[test]
fn review_replay_from_a_resized_terminal_returns_the_recorded_job() {
    let daemon = Daemon::start("review-digest-pty");
    let first = daemon.rpc(pty_request(&daemon, "pty-1", 24, 80, "one"));
    assert_eq!(first["Submitted"]["job"]["id"], 1, "{first}");
    assert_eq!(first["Submitted"]["job"]["spec_digest_version"], 2);
    let resized = daemon.rpc(pty_request(&daemon, "pty-1", 50, 132, "two"));
    assert_eq!(resized["Submitted"]["job"]["id"], 1, "{resized}");
    assert_eq!(resized["Submitted"]["job"]["replayed"], true);
    assert_eq!(
        resized["Submitted"]["job"]["spec"]["declared"]["terminal"]["rows"],
        24
    );
    let mut plain = pty_request(&daemon, "pty-1", 24, 80, "one");
    plain["Versioned"]["request"]["Create"]["spec"]["declared"]
        .as_object_mut()
        .unwrap()
        .remove("terminal");
    let refused = daemon.rpc(plain);
    assert!(
        refused["Error"]["message"]
            .as_str()
            .unwrap()
            .contains("different specification"),
        "{refused}"
    );
    let mut longer = pty_request(&daemon, "pty-1", 24, 80, "one");
    longer["Versioned"]["request"]["Create"]["spec"]["declared"]["wall_ms"] = 5000.into();
    assert!(daemon.rpc(longer).get("Error").is_some());
    assert_eq!(job_count(&daemon), 1);
    let record = daemon.record("1");
    assert_eq!(record["spec_digest_version"], 2);
    let index = std::fs::read_dir(daemon.state.join("idempotency"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let entry: serde_json::Value = serde_json::from_slice(&std::fs::read(index).unwrap()).unwrap();
    assert_eq!(entry["digest_version"], 2);
    assert_eq!(entry["digest"], record["spec_digest"]);
}

#[test]
fn review_edit_moves_the_idempotency_key_to_the_edited_specification() {
    let daemon = Daemon::start("review-digest-edit");
    let original = [
        "create",
        "--idempotency-key",
        "edit-1",
        "--",
        "sh",
        "-c",
        "exit 0",
    ];
    let edited = [
        "create",
        "--idempotency-key",
        "edit-1",
        "--time",
        "30s",
        "--",
        "sh",
        "-c",
        "exit 3",
    ];
    let id = daemon.ok(&original);
    let before = daemon.record(&id)["spec_digest"].clone();
    daemon.ok(&["edit", &id, "--time", "30s", "--", "sh", "-c", "exit 3"]);
    let record = daemon.record(&id);
    assert_eq!(record["idempotency_key"], "edit-1");
    assert_ne!(record["spec_digest"], before);
    assert_eq!(record["spec_digest_version"], 2);
    assert_eq!(daemon.ok(&edited), id);
    let refused = daemon.job(&original);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("different specification"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(job_count(&daemon), 1);
    assert_eq!(
        std::fs::read_dir(daemon.state.join("idempotency"))
            .unwrap()
            .count(),
        1
    );
    let plain = daemon.ok(&["create", "--", "true"]);
    daemon.ok(&["edit", &plain, "--", "false"]);
    assert!(daemon.record(&plain).get("idempotency_key").is_none());
}

#[test]
fn review_audit_intent_survives_a_crash_between_the_change_and_its_result() {
    let mut daemon = Daemon::with_failpoint("review-audit-intent", "audit-before-end");
    assert!(!daemon.job(&["create", "--", "true"]).status.success());
    daemon.died();
    assert_eq!(job_count(&daemon), 1);
    let lines = raw_lines(&daemon);
    assert_eq!(lines.len(), 1, "{lines:?}");
    let intent: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(intent["phase"], "begin");
    daemon.restart("");
    let id = daemon.ok(&["create", "--", "true"]);
    assert_eq!(id, "2");
    let entries = audit(&daemon, &["--action", "create"]);
    assert_eq!(entries.len(), 2, "{entries:?}");
    assert_eq!(entries[0]["seq"], 1);
    assert_eq!(entries[0]["result"], "outcome unknown");
    assert_eq!(entries[0]["phase"], "begin");
    assert_eq!(entries[0]["peer_uid"], uid());
    assert_eq!(entries[1]["seq"], 2);
    assert_eq!(entries[1]["result"], "ok");
    assert_eq!(entries[1]["target"], "2");
    assert!(entries[1].get("phase").is_none());
    let phases: Vec<String> = raw_lines(&daemon)
        .iter()
        .map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            format!("{}:{}", value["seq"], value["phase"].as_str().unwrap())
        })
        .collect();
    assert_eq!(phases, ["1:begin", "2:begin", "2:end"]);
    let text = daemon.ok(&["audit"]);
    assert!(text.contains("outcome unknown"), "{text}");
}

#[test]
fn review_audit_write_failure_refuses_the_change_and_the_next_record_is_readable() {
    let daemon = Daemon::with_failpoint("review-audit-torn", "audit-partial-write@1");
    let refused = daemon.job(&["create", "--", "true"]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("the service cannot write its audit journal, so it changes nothing"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(job_count(&daemon), 0);
    assert!(
        daemon
            .rpc(serde_json::json!({"Done": {"id": 1}}))
            .get("Pong")
            .is_some()
    );
    daemon.ok(&["queue"]);
    assert_eq!(daemon.ok(&["create", "--", "true"]), "1");
    let read = daemon.job(&["audit", "--json"]);
    assert!(read.status.success());
    let shown: Vec<serde_json::Value> = String::from_utf8_lossy(&read.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0]["action"], "create");
    assert_eq!(shown[0]["result"], "ok");
    assert_eq!(shown[0]["target"], "1");
    assert!(
        String::from_utf8_lossy(&read.stderr).contains("could not be read"),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    assert!(
        String::from_utf8_lossy(&read.stderr)
            .trim()
            .ends_with(": 1")
    );
    let lines = raw_lines(&daemon);
    let unreadable = lines
        .iter()
        .filter(|line| serde_json::from_str::<serde_json::Value>(line).is_err())
        .count();
    assert_eq!(unreadable, 1, "{lines:?}");
    assert_eq!(lines.len(), 3, "{lines:?}");
}

#[test]
fn review_audit_bounds_every_client_string_and_the_whole_line() {
    let daemon = Daemon::start("review-audit-bounds");
    let long = "s".repeat(6000);
    let mut submit = bare_submit(&daemon);
    submit["Submit"]["spec"]["session"] = long.clone().into();
    let response = daemon.rpc(versioned(20, None, submit));
    assert!(response.get("Submitted").is_some(), "{response}");
    daemon.rpc(versioned(
        20,
        None,
        serde_json::json!({"Cancel": {"id": 1, "session": long}}),
    ));
    daemon.rpc(versioned(
        20,
        None,
        serde_json::json!({"QueueClear": {"name": "q".repeat(6000)}}),
    ));
    daemon.rpc(versioned(
        20,
        None,
        serde_json::json!({"ResourceUpdateStatus": {"operation": "o".repeat(9000), "abandon": true}}),
    ));
    let lines = raw_lines(&daemon);
    assert_eq!(lines.len(), 8, "{}", lines.len());
    for line in &lines {
        assert!(line.len() < 8192, "{}", line.len());
        let entry: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(entry["truncated"], true, "{entry}");
        assert!(
            entry["session"]
                .as_str()
                .is_none_or(|text| text.len() == 128)
        );
        assert!(
            entry["target"]
                .as_str()
                .is_none_or(|text| text.len() <= 256)
        );
    }
    let entries = audit(&daemon, &[]);
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0]["session"], "s".repeat(128));
    assert_eq!(entries[2]["target"], "q".repeat(256));
    assert_eq!(entries[3]["target"], "o".repeat(256));
}

#[test]
fn review_audit_records_attach_and_a_bounded_number_of_unreadable_requests() {
    let daemon = Daemon::start("review-audit-events");
    let attach = daemon.rpc(versioned(
        20,
        None,
        serde_json::json!({"Attach": {"id": 7}}),
    ));
    assert!(attach.get("Error").is_some(), "{attach}");
    let bare = daemon.rpc(serde_json::json!({"Attach": {"id": 8}}));
    assert!(bare.get("Error").is_some(), "{bare}");
    let attached = audit(&daemon, &["--action", "attach"]);
    assert_eq!(attached.len(), 2);
    assert_eq!(attached[0]["target"], "7");
    assert_eq!(attached[0]["peer_uid"], uid());
    assert!(attached[0]["peer_pid"].as_i64().unwrap() > 1);
    assert!(attached[0]["result"].as_str().unwrap().starts_with("error"));
    assert!(attached[0].get("phase").is_none());
    let garbage = b"\xff\xfenot json at all, and rather longer than sixty-four bytes so the sample is cut off\n";
    for _ in 0..45 {
        let mut stream = UnixStream::connect(daemon.state.join("daemon.sock")).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream.write_all(garbage).unwrap();
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer).unwrap();
        assert!(answer.contains("unreadable request"), "{answer}");
    }
    for _ in 0..5 {
        drop(UnixStream::connect(daemon.state.join("daemon.sock")).unwrap());
    }
    daemon.ok(&["create", "--", "true"]);
    let unreadable = audit(&daemon, &["--action", "unreadable-request"]);
    assert_eq!(unreadable.len(), 30);
    assert_eq!(unreadable[0]["params"]["bytes"], garbage.len());
    assert_eq!(
        unreadable[0]["params"]["first_bytes_hex"],
        hex(&garbage[..64])
    );
    assert_eq!(unreadable[0]["peer_uid"], uid());
    assert_eq!(unreadable[0]["result"], "refused");
    let suppressed = audit(&daemon, &["--action", "audit-suppressed"]);
    assert_eq!(suppressed.len(), 1);
    assert_eq!(suppressed[0]["params"]["state"], "started");
    assert_eq!(suppressed[0]["params"]["limit"], 30);
    assert_eq!(suppressed[0]["params"]["window_ms"], 60000);
    assert_eq!(audit(&daemon, &["--action", "create"]).len(), 1);
}

#[test]
fn review_audit_records_the_program_and_a_digest_instead_of_the_arguments() {
    let daemon = Daemon::start("review-audit-argv");
    let submitted = daemon
        .command(&[
            "create",
            "--",
            "curl",
            "--header",
            "Authorization: Bearer argv-secret-41",
            "https://example.invalid/",
        ])
        .env(
            "JOB_SESSION",
            "ci see ftp://alice:session-secret-42@files.invalid/x now",
        )
        .output()
        .unwrap();
    assert!(submitted.status.success());
    let mut request = bare_submit(&daemon);
    request["Submit"]["spec"]["argv"] = serde_json::json!([
        "sh",
        "-c",
        "curl https://bob:inline-secret-43@example.invalid/"
    ]);
    request["Submit"]["spec"]["cwd"] = serde_json::json!(
        daemon
            .work
            .join("at http://carol:cwd-secret-44@host.invalid/dir")
    );
    daemon.rpc(versioned(20, None, request));
    let entries = audit(&daemon, &[]);
    let argv = &entries[0]["params"]["spec"]["argv"];
    assert_eq!(argv["program"], "curl");
    assert_eq!(argv["count"], 4);
    assert_eq!(argv["sha256"].as_str().unwrap().len(), 64);
    assert!(entries[0]["params"]["spec"]["declared"].is_object());
    assert_eq!(
        entries[0]["session"],
        "ci see ftp://***@files.invalid/x now"
    );
    assert_eq!(entries[1]["params"]["spec"]["argv"]["program"], "sh");
    assert_ne!(
        entries[1]["params"]["spec"]["argv"]["sha256"],
        argv["sha256"]
    );
    let raw = raw_lines(&daemon).join("\n");
    for secret in [
        "argv-secret-41",
        "session-secret-42",
        "inline-secret-43",
        "cwd-secret-44",
        "Authorization",
    ] {
        assert!(!raw.contains(secret), "{secret} in {raw}");
    }
    assert!(raw.contains("http://***@host.invalid/dir"), "{raw}");
}

#[test]
fn review_unknown_supervisor_liveness_does_not_requeue_a_launch_in_progress() {
    let daemon =
        Daemon::with_failpoint("review-liveness", "liveness-unknown@1,launch-confirm-delay");
    let id = daemon.ok(&["submit", "--", "sh", "-c", ONCE]);
    assert_eq!(
        daemon.until(&id, &["Succeeded", "Failed", "Lost"])["state"],
        "Succeeded"
    );
    std::thread::sleep(Duration::from_millis(2200));
    assert_eq!(runs(&daemon), 1);
    let errors = std::fs::read_to_string(daemon.base.join("daemon.err")).unwrap();
    assert!(!errors.contains("the Job stays queued"), "{errors}");
    assert_eq!(daemon.record(&id)["attempt"], 1);
}

#[test]
fn review_output_budget_never_trims_a_job_whose_record_cannot_be_read() {
    let daemon = Daemon::configured("review-budget", "[output]\nbudget_bytes = 4096\n", "");
    let unreadable = daemon.state.join("jobs/900");
    std::fs::create_dir_all(unreadable.join("attempts/1")).unwrap();
    std::fs::write(unreadable.join("job.json"), "{\"id\": 900, \"sta").unwrap();
    std::fs::write(unreadable.join("output.log"), vec![b'x'; 9000]).unwrap();
    std::fs::write(unreadable.join("attempts/1/output.log"), vec![b'y'; 9000]).unwrap();
    let fill = "head -c 3000 /dev/zero | tr '\\0' a";
    let first = daemon.ok(&["submit", "--", "sh", "-c", fill]);
    daemon.until(&first, &["Succeeded"]);
    let second = daemon.ok(&["submit", "--", "sh", "-c", fill]);
    daemon.until(&second, &["Succeeded"]);
    assert_eq!(
        std::fs::metadata(unreadable.join("output.log"))
            .unwrap()
            .len(),
        9000
    );
    assert_eq!(
        std::fs::metadata(unreadable.join("attempts/1/output.log"))
            .unwrap()
            .len(),
        9000
    );
    assert!(
        !daemon
            .state
            .join("jobs")
            .join(&first)
            .join("output.log")
            .exists()
    );
    let host = daemon.ok(&["host"]);
    assert!(
        host.contains("output budget: the records of Jobs 900 cannot be read; their output is counted and never trimmed"),
        "{host}"
    );
    let json: serde_json::Value = serde_json::from_str(&daemon.ok(&["host", "--json"])).unwrap();
    assert_eq!(json["unreadable_records"], serde_json::json!([900]));
    let errors = std::fs::read_to_string(daemon.base.join("daemon.err")).unwrap();
    assert_eq!(
        errors
            .matches("the record of Job 900 cannot be read")
            .count(),
        1,
        "{errors}"
    );
}

fn ticks_of(pid: u64) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    if fields.first() == Some(&"Z") {
        return None;
    }
    fields.get(19)?.parse().ok()
}

fn end_as_a_reboot_would(pid: u64, ticks: u64) {
    if ticks_of(pid) != Some(ticks) {
        return;
    }
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(10);
    while ticks_of(pid) == Some(ticks) {
        assert!(Instant::now() < deadline, "process {pid} did not end");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn interrupted(name: &str) -> String {
    format!(
        "echo {name} >> runs; echo began-{name}; echo $$ > {name}.pid; [ -e fast ] || exec sleep 40"
    )
}

#[test]
fn durable_reboot_records_running_and_suspended_attempts_as_lost_and_starts_unlaunched_work_once() {
    let mut daemon = Daemon::start("reboot");
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
    let mut processes = Vec::new();
    for name in ["running", "suspended"] {
        let id = daemon.ok(&["submit", "--", "sh", "-c", &interrupted(name)]);
        let status = daemon.until(&id, &["Running"]);
        assert_eq!(status["supervisor_boot_id"], boot.trim());
        let deadline = Instant::now() + Duration::from_secs(10);
        let workload = loop {
            if let Some(pid) = std::fs::read_to_string(daemon.work.join(format!("{name}.pid")))
                .ok()
                .and_then(|text| text.trim().parse::<u64>().ok())
            {
                break pid;
            }
            assert!(Instant::now() < deadline, "{name} did not start");
            std::thread::sleep(Duration::from_millis(10));
        };
        processes.push((
            status["shim_pid"].as_u64().unwrap(),
            status["shim_start_ticks"].as_u64().unwrap(),
        ));
        processes.push((workload, ticks_of(workload).unwrap()));
    }
    daemon.restart("launch-after-starting");
    assert_eq!(daemon.status("1")["state"], "Running");
    assert_eq!(daemon.status("2")["state"], "Running");
    let submitted = daemon.job(&["submit", "--", "sh", "-c", "echo starting >> runs"]);
    assert!(!submitted.status.success());
    daemon.died();
    assert_eq!(daemon.record("3")["state"], "Starting");
    assert!(daemon.record("3")["shim_pid"].is_null());
    for (pid, ticks) in processes {
        end_as_a_reboot_would(pid, ticks);
    }
    for id in ["1", "2"] {
        let directory = daemon.state.join("jobs").join(id);
        assert!(directory.join("started.json").exists());
        assert!(!directory.join("result.json").exists());
        assert!(!directory.join("exit.json").exists());
        let mut job = daemon.record(id);
        assert_eq!(job["state"], "Running");
        job["supervisor_boot_id"] = serde_json::json!("the-boot-before");
        if id == "2" {
            job["state"] = serde_json::json!("Suspended");
            job["suspension"]["requested"] = serde_json::json!(true);
            job["suspension"]["generation"] = serde_json::json!(1);
            job["suspension"]["since_ms"] = job["started_ms"].clone();
        }
        std::fs::write(
            directory.join("job.json"),
            serde_json::to_vec(&job).unwrap(),
        )
        .unwrap();
    }
    assert_eq!(runs(&daemon), 2);
    let boots = daemon.state.join("boots.json");
    let mut known: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&boots).unwrap()).unwrap();
    assert!(known[boot.trim()].as_u64().is_some(), "{known}");
    known["the-boot-before"] = serde_json::json!(1_000_000_000u64);
    std::fs::write(&boots, serde_json::to_vec(&known).unwrap()).unwrap();
    daemon.child = Daemon::spawn(&daemon.base, "");
    daemon.ready();
    for id in ["1", "2"] {
        let wording = format!(
            "lost: the host restarted while the Job was {}; that boot began at 2001-09-09 01:46:40 UTC, this one at 2",
            if id == "1" { "running" } else { "suspended" }
        );
        let status = daemon.until(id, &["Lost", "Succeeded", "Failed", "Cancelled", "Queued"]);
        assert_eq!(status["state"], "Lost", "job {id}");
        assert_eq!(status["attempt"], 1);
        assert_eq!(status["stop"]["kind"], "DaemonLost");
        let line = status["stop"]["line"].as_str().unwrap();
        assert!(line.starts_with(&wording), "{line}");
        assert!(line.ends_with(" UTC"), "{line}");
        assert_eq!(status["supervisor_boot_id"], "the-boot-before");
        assert!(status["finished_ms"].as_u64().unwrap() >= status["started_ms"].as_u64().unwrap());
        assert!(
            status["result"]["exit_code"].is_null(),
            "{}",
            status["result"]
        );
        let shown = daemon.job(&["show", id]);
        assert_eq!(shown.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&shown.stdout).contains(&wording),
            "{}",
            String::from_utf8_lossy(&shown.stdout)
        );
        assert_eq!(
            daemon.job(&["wait", id, "--timeout", "2s"]).status.code(),
            Some(125)
        );
    }
    let started = daemon.until("3", &["Succeeded", "Failed", "Lost"]);
    assert_eq!(started["state"], "Succeeded");
    assert_eq!(started["attempt"], 1);
    assert_eq!(started["supervisor_boot_id"], boot.trim());
    std::thread::sleep(Duration::from_millis(1500));
    let ran = std::fs::read_to_string(daemon.work.join("runs")).unwrap();
    assert_eq!(ran, "running\nsuspended\nstarting\n");
    for id in ["1", "2"] {
        assert_eq!(daemon.status(id)["state"], "Lost");
        let attempts: serde_json::Value =
            serde_json::from_str(&daemon.ok(&["attempts", id, "--json"])).unwrap();
        assert_eq!(attempts["attempts"].as_array().unwrap().len(), 1);
    }
    let refused = daemon.job(&["retry", "1"]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("lost work may still exist; retry requires --allow-lost"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(daemon.status("1")["attempt"], 1);
    std::fs::write(daemon.work.join("fast"), b"").unwrap();
    daemon.ok(&["retry", "1", "--allow-lost"]);
    let retried = daemon.until("1", &["Succeeded", "Failed", "Lost"]);
    assert_eq!(retried["state"], "Succeeded");
    assert_eq!(retried["attempt"], 2);
    assert_eq!(retried["supervisor_boot_id"], boot.trim());
    let attempts: serde_json::Value =
        serde_json::from_str(&daemon.ok(&["attempts", "1", "--json"])).unwrap();
    let attempts = attempts["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["attempt"], 1);
    assert_eq!(attempts[0]["state"], "Lost");
    assert_eq!(attempts[0]["stop"]["kind"], "DaemonLost");
    assert_eq!(attempts[0]["supervisor_boot_id"], "the-boot-before");
    assert_eq!(attempts[1]["state"], "Succeeded");
    let cut = daemon.job(&["logs", "1", "--attempt", "1", "--raw"]);
    assert_eq!(cut.status.code(), Some(125));
    assert_eq!(String::from_utf8_lossy(&cut.stdout), "began-running\n");
    assert!(
        String::from_utf8_lossy(&cut.stderr).contains("output recording is incomplete"),
        "{}",
        String::from_utf8_lossy(&cut.stderr)
    );
    assert_eq!(daemon.ok(&["logs", "1", "--raw"]), "began-running");
    assert!(daemon.state.join("jobs/1/attempts/1/job.json").exists());
    let ran = std::fs::read_to_string(daemon.work.join("runs")).unwrap();
    assert_eq!(ran, "running\nsuspended\nstarting\nrunning\n");
    assert_eq!(daemon.status("2")["state"], "Lost");
    assert_eq!(daemon.status("2")["attempt"], 1);
}

const PIDNS: [&str; 2] = ["--namespaces", "user,mount,pid"];

fn pidns_on_host(marker: &str) -> bool {
    std::fs::read_dir("/proc").unwrap().flatten().any(|entry| {
        std::fs::read(entry.path().join("cmdline")).is_ok_and(|bytes| {
            String::from_utf8_lossy(&bytes)
                .split('\0')
                .any(|part| part == marker)
        })
    })
}

#[test]
fn pidns_crash_after_spawn_never_executes_without_a_recorded_supervisor() {
    launch_crash_with(
        "pidns-launch-spawn",
        "launch-after-spawn",
        "Starting",
        false,
        &PIDNS,
    );
}

#[test]
fn pidns_crash_after_the_identity_record_requeues_the_unconfirmed_launch() {
    launch_crash_with(
        "pidns-launch-identity",
        "launch-after-identity",
        "Running",
        true,
        &PIDNS,
    );
}

#[test]
fn pidns_failed_identity_save_runs_the_command_once_inside_the_namespace() {
    let daemon = Daemon::with_failpoint("pidns-identity-save", "launch-identity-save@1");
    let id = daemon.ok(&[
        "submit",
        "--namespaces",
        "user,mount,pid",
        "--",
        "sh",
        "-c",
        "echo run $$ >> runs",
    ]);
    let status = daemon.until(&id, &["Succeeded", "Failed", "Lost"]);
    assert_eq!(status["state"], "Succeeded");
    assert_eq!(
        std::fs::read_to_string(daemon.work.join("runs")).unwrap(),
        "run 2\n"
    );
    assert!(
        daemon
            .state
            .join("jobs")
            .join(&id)
            .join("started.json")
            .exists()
    );
}

#[test]
fn pidns_exit_status_survives_a_supervisor_killed_during_cleanup_and_the_tree_ends() {
    let daemon = Daemon::with_failpoint("pidns-exit-record", "shim-after-exit-record");
    let failed = daemon.ok(&[
        "submit",
        "--namespaces",
        "user,mount,pid",
        "--",
        "sh",
        "-c",
        "sleep 24.4321 & exit 7",
    ]);
    let status = daemon.until(&failed, &["Succeeded", "Failed", "Lost"]);
    assert_eq!(status["state"], "Failed");
    assert_eq!(status["result"]["exit_code"], 7);
    assert!(
        status["result"]["notes"][0]
            .as_str()
            .unwrap()
            .contains("before cleanup completed")
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while pidns_on_host("24.4321") {
        assert!(Instant::now() < deadline, "the background child stayed");
        std::thread::sleep(Duration::from_millis(20));
    }
    let killed = daemon.ok(&[
        "submit",
        "--namespaces",
        "user,mount,pid",
        "--",
        "sh",
        "-c",
        "kill -9 $$",
    ]);
    let status = daemon.until(&killed, &["Succeeded", "Failed", "Lost"]);
    assert_eq!(status["state"], "Failed");
    assert_eq!(status["result"]["signal"], 9);
}
