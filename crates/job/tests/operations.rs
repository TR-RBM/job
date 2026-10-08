use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;

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

    fn configured(name: &str, config: &str, failpoint: &str) -> Daemon {
        let base = Self::prepare(name, config);
        let child = Self::spawn(&base, failpoint);
        let mut daemon = Daemon {
            child,
            state: base.join("state"),
            work: base.join("work"),
            base,
        };
        daemon.ready();
        daemon
    }

    fn prepare(name: &str, config: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("job-ops-{}-{name}", std::process::id()));
        std::fs::create_dir_all(base.join("work")).unwrap();
        std::fs::write(
            base.join("config.toml"),
            format!("schema_version = 1\nprofile = 'ordinary'\n{config}"),
        )
        .unwrap();
        base
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
            .env_remove("JOB_RUNTIME_DIR")
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

    fn restart(&mut self) {
        self.stop();
        self.child = Self::spawn(&self.base, "");
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
                self.log()
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

    fn log(&self) -> String {
        std::fs::read_to_string(self.base.join("daemon.err")).unwrap_or_default()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_job"));
        command
            .args(args)
            .env("JOB_STATE_DIR", &self.state)
            .env("JOB_CONFIG", self.base.join("config.toml"))
            .env("JOB_SESSION", "ops")
            .env("LC_ALL", "C")
            .env_remove("JOB_FAILPOINT")
            .env_remove("JOB_CLI_COMPAT")
            .env_remove("JOB_RUNTIME_DIR")
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

    fn json(&self, args: &[&str]) -> Value {
        let text = self.ok(args);
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{args:?}: {error}: {text}"))
    }

    fn status(&self, id: &str) -> Value {
        let output = self.job(&["status", id, "--json"]);
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)))
    }

    fn until(&self, id: &str, wanted: &[&str]) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
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

    fn events(&self, args: &[&str]) -> Vec<Value> {
        let mut all = vec!["events", "--format", "json"];
        all.extend_from_slice(args);
        self.ok(&all)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn events_until(&self, args: &[&str], done: impl Fn(&[Value]) -> bool) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let events = self.events(args);
            if done(&events) {
                return events;
            }
            assert!(Instant::now() < deadline, "events stayed {events:?}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn moves(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| event["to"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn ops_events_record_every_job_transition_with_its_exit() {
    let daemon = Daemon::start("job-events");
    let before = daemon.events(&[]).len();
    assert_eq!(before, 0);
    let id = daemon.ok(&["submit", "--", "sh", "-c", "exit 3"]);
    daemon.until(&id, &["Failed"]);
    let events = daemon.events(&["--job", &id]);
    assert_eq!(
        moves(&events),
        ["queued", "starting", "running", "failed"],
        "{events:?}"
    );
    let seqs: Vec<u64> = events
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, [1, 2, 3, 4]);
    for event in &events {
        assert_eq!(event["schema_version"], 1);
        assert_eq!(event["kind"], "job");
        assert_eq!(event["job"].as_u64().unwrap().to_string(), id);
        assert_eq!(event["attempt"], 1);
        assert_eq!(event["queue_path"], "default");
        assert_eq!(event["queue_id"], 2);
        assert!(event["at_ms"].as_u64().unwrap() > 0);
    }
    assert!(events[0]["from"].is_null());
    assert_eq!(events[0]["reason"], "submitted");
    assert_eq!(events[1]["from"], "queued");
    assert!(events[1]["wait_ms"].is_u64());
    assert_eq!(events[2]["from"], "starting");
    assert_eq!(events[3]["from"], "running");
    assert_eq!(events[3]["exit_code"], 3);
    assert!(events[3]["signal"].is_null());
    assert_eq!(events[3]["reason"], "exit 3");
    assert!(events[2]["exit_code"].is_null());

    let text = daemon.ok(&["events", "--job", &id]);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("seq\tat_ms\tkind\tjob"), "{text}");
    assert_eq!(lines.len(), 5);
    assert!(lines[4].contains("\trunning\tfailed\t3\t"), "{text}");

    assert!(daemon.events(&["--job", "999"]).is_empty());
    let later = events[3]["at_ms"].as_u64().unwrap() + 1;
    assert!(daemon.events(&["--since", &later.to_string()]).is_empty());
    assert_eq!(daemon.events(&["--queue", "default"]).len(), 4);
    assert!(daemon.events(&["--queue", "elsewhere"]).is_empty());

    let refused = daemon.job(&["events", "--nonsense"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--nonsense"));
}

#[test]
fn ops_events_record_hold_release_cancel_signal_and_requeue() {
    let daemon = Daemon::start("lifecycle-events");
    daemon.ok(&["create", "--", "sh", "-c", "exit 0"]);
    let listed = daemon.events(&[]);
    assert_eq!(moves(&listed), ["held"]);
    let id = listed[0]["job"].as_u64().unwrap().to_string();
    assert_eq!(listed[0]["reason"], "created held");
    daemon.ok(&["release", &id]);
    daemon.until(&id, &["Succeeded"]);
    let events = daemon.events(&["--job", &id]);
    assert_eq!(
        moves(&events),
        ["held", "queued", "starting", "running", "succeeded"]
    );
    assert_eq!(events[1]["reason"], "released");
    assert_eq!(events[4]["exit_code"], 0);

    daemon.ok(&["retry", &id]);
    let events = daemon.events_until(&["--job", &id], |events| {
        events
            .last()
            .is_some_and(|event| event["to"] == "succeeded")
            && events.len() > 5
    });
    assert_eq!(events[5]["to"], "requeued", "{events:?}");
    assert_eq!(events[5]["from"], "succeeded");
    assert_eq!(events[5]["attempt"], 2);
    assert_eq!(events[5]["reason"], "retry");
    assert_eq!(
        moves(&events[6..]),
        ["starting", "running", "succeeded"],
        "{events:?}"
    );

    let long = daemon.ok(&["submit", "--", "sleep", "30"]);
    daemon.until(&long, &["Running"]);
    daemon.ok(&["cancel", &long]);
    daemon.until(&long, &["Cancelled"]);
    let events = daemon.events(&["--job", &long]);
    assert_eq!(
        moves(&events),
        ["queued", "starting", "running", "stopping", "cancelled"],
        "{events:?}"
    );
    assert!(
        events[3]["reason"]
            .as_str()
            .unwrap()
            .starts_with("Cancelled"),
        "{events:?}"
    );
    assert!(events[4]["exit_code"].is_null() || events[4]["signal"].is_u64());

    daemon.ok(&["queue", "pause", "default"]);
    let waiting = daemon.ok(&["submit", "--", "true"]);
    daemon.ok(&["cancel", &waiting]);
    daemon.until(&waiting, &["Cancelled"]);
    assert_eq!(
        moves(&daemon.events(&["--job", &waiting])),
        ["queued", "cancelled"]
    );
}

#[test]
fn ops_events_record_a_launch_that_was_requeued_by_recovery() {
    let mut daemon = Daemon::configured("requeue-events", "", "launch-after-starting");
    let submitted = daemon.job(&["submit", "--", "true"]);
    assert!(!submitted.status.success());
    daemon.died();
    daemon.restart();
    daemon.until("1", &["Succeeded"]);
    let events = daemon.events(&["--job", "1"]);
    assert_eq!(
        moves(&events),
        [
            "queued",
            "starting",
            "requeued",
            "starting",
            "running",
            "succeeded"
        ],
        "{events:?}"
    );
    assert_eq!(events[2]["from"], "starting");
    assert_eq!(events[2]["reason"], "launch not confirmed");
    let host = daemon.json(&["host", "--json"]);
    assert_eq!(host["health"]["recovery_changed_records"], 1, "{host}");
    assert_eq!(host["health"]["supervisors_adopted"], 0);
}

#[test]
fn ops_events_record_queue_and_group_admission_changes() {
    let daemon = Daemon::start("admission-events");
    daemon.ok(&["group", "create", "team"]);
    daemon.ok(&["queue", "create", "build", "--group", "team"]);
    daemon.ok(&["queue", "pause", "team/build"]);
    daemon.ok(&["queue", "resume", "team/build"]);
    daemon.ok(&["queue", "close", "team/build"]);
    daemon.ok(&["queue", "open", "team/build"]);
    daemon.ok(&["group", "pause", "team"]);
    daemon.ok(&["group", "resume", "team"]);
    daemon.ok(&["group", "close", "team"]);
    let events = daemon.events(&[]);
    let seen: Vec<(String, String, String, String)> = events
        .iter()
        .map(|event| {
            (
                event["kind"].as_str().unwrap().to_owned(),
                event["object_path"].as_str().unwrap().to_owned(),
                event["from"].as_str().unwrap().to_owned(),
                event["to"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let expected = [
        ("queue", "team/build", "resumed", "paused"),
        ("queue", "team/build", "paused", "resumed"),
        ("queue", "team/build", "open", "closed"),
        ("queue", "team/build", "closed", "open"),
        ("group", "team", "resumed", "paused"),
        ("group", "team", "paused", "resumed"),
        ("group", "team", "open", "closed"),
    ]
    .map(|(kind, path, from, to)| {
        (
            kind.to_owned(),
            path.to_owned(),
            from.to_owned(),
            to.to_owned(),
        )
    });
    assert_eq!(seen, expected, "{events:?}");
    assert!(events.iter().all(|event| event["job"].is_null()));
    assert!(events.iter().all(|event| event["object_id"].is_u64()));
    assert_eq!(daemon.events(&["--queue", "team/build"]).len(), 4);
    assert_eq!(daemon.events(&["--queue", "team"]).len(), 7);
    daemon.ok(&["queue", "pause", "team/build"]);
    daemon.ok(&["queue", "pause", "team/build"]);
    assert_eq!(daemon.events(&["--queue", "team/build"]).len(), 5);
}

#[test]
fn ops_events_record_pressure_holds_of_a_group() {
    let daemon = Daemon::start("pressure-events");
    let rules = daemon.work.join("rules.json");
    std::fs::write(
        &rules,
        r#"[{"id":"protect","resource":"memory","metric":"full","window":"avg10","high_bp":100,"low_bp":10,"sustain_ms":1000,"minimum_hold_ms":2000,"recovery_ms":1000,"step_ms":1000,"required":true}]"#,
    )
    .unwrap();
    daemon.ok(&[
        "group",
        "create",
        "protected",
        "--pressure",
        rules.to_str().unwrap(),
    ]);
    daemon.ok(&["queue", "create", "protected/a"]);
    let pending = daemon.ok(&["submit", "-q", "protected/a", "--", "true"]);
    let events = daemon.events_until(&["--queue", "protected"], |events| {
        events.iter().any(|event| event["to"] == "pressure_hold")
    });
    let hold = events
        .iter()
        .find(|event| event["to"] == "pressure_hold")
        .unwrap();
    assert_eq!(hold["kind"], "group", "{events:?}");
    assert_eq!(hold["from"], "pressure_open");
    assert_eq!(hold["object_path"], "protected");
    assert_eq!(hold["reason"], "pressure rule protect");
    assert!(hold["job"].is_null());
    daemon.ok(&["group", "unset", "protected", "pressure"]);
    let events = daemon.events_until(&["--queue", "protected"], |events| {
        events.iter().any(|event| event["to"] == "pressure_open")
    });
    let open = events
        .iter()
        .find(|event| event["to"] == "pressure_open")
        .unwrap();
    assert_eq!(open["from"], "pressure_hold");
    assert_eq!(open["reason"], "pressure rule removed");
    daemon.until(&pending, &["Succeeded"]);
}

struct Follower {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Follower {
    fn start(daemon: &Daemon, args: &[&str]) -> Self {
        let mut all = vec!["events", "--follow", "--format", "json"];
        all.extend_from_slice(args);
        let mut child = daemon
            .command(&all)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self { child, lines }
    }

    fn next(&self) -> Value {
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(20))
            .expect("the follower printed nothing in time");
        serde_json::from_str(&line).unwrap()
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn ops_events_follow_prints_new_records_across_rotation_without_a_gap() {
    let daemon = Daemon::configured(
        "follow-rotation",
        "[events]\nrotate_bytes = 1500\nkeep_files = 4\n",
        "",
    );
    let first = daemon.ok(&["submit", "--", "true"]);
    daemon.until(&first, &["Succeeded"]);
    let follower = Follower::start(&daemon, &[]);
    let mut seen = Vec::new();
    while seen
        .last()
        .is_none_or(|event: &Value| event["to"] != "succeeded")
    {
        seen.push(follower.next());
    }
    assert_eq!(moves(&seen), ["queued", "starting", "running", "succeeded"]);
    let mut last = String::new();
    for _ in 0..5 {
        last = daemon.ok(&["submit", "--", "true"]);
        daemon.until(&last, &["Succeeded"]);
    }
    loop {
        let event = follower.next();
        let done = event["to"] == "succeeded" && event["job"].as_u64().unwrap().to_string() == last;
        seen.push(event);
        if done {
            break;
        }
    }
    let seqs: Vec<u64> = seen
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect();
    let expected: Vec<u64> = (1..=seqs.len() as u64).collect();
    assert_eq!(seqs, expected, "{seen:?}");
    let rotations = seen
        .iter()
        .filter(|event| event["kind"] == "journal" && event["to"] == "rotated")
        .count();
    assert!(rotations >= 3, "{seen:?}");
    let files: Vec<String> = std::fs::read_dir(daemon.state.join("events"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(files.contains(&"current.jsonl".to_owned()));
    assert!(files.len() >= 3 && files.len() <= 4, "{files:?}");
    for file in &files {
        let size = std::fs::metadata(daemon.state.join("events").join(file))
            .unwrap()
            .len();
        assert!(size <= 1500, "{file} has {size} bytes");
    }
    let kept = daemon.events(&[]);
    assert!(kept.len() < seen.len());
    assert_eq!(kept.last().unwrap()["seq"], seen.last().unwrap()["seq"]);

    let only = Follower::start(&daemon, &["--job", &last]);
    assert_eq!(only.next()["job"].as_u64().unwrap().to_string(), last);
}

#[test]
fn ops_events_survive_a_torn_last_line_and_keep_their_sequence() {
    let mut daemon = Daemon::start("torn-events");
    let id = daemon.ok(&["submit", "--", "true"]);
    daemon.until(&id, &["Succeeded"]);
    daemon.stop();
    let current = daemon.state.join("events/current.jsonl");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&current)
        .unwrap()
        .write_all(b"{\"seq\":5,\"at_ms\":1,\"kind\":\"job\",\"to\":\"que")
        .unwrap();
    let offline = daemon.job(&["events", "--format", "json"]);
    assert!(offline.status.success());
    assert_eq!(String::from_utf8_lossy(&offline.stdout).lines().count(), 4);
    assert!(
        String::from_utf8_lossy(&offline.stderr).contains("could not be read"),
        "{}",
        String::from_utf8_lossy(&offline.stderr)
    );
    daemon.restart();
    let next = daemon.ok(&["submit", "--", "true"]);
    daemon.until(&next, &["Succeeded"]);
    let events = daemon.events(&[]);
    let seqs: Vec<u64> = events
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(
        moves(&events[4..]),
        ["queued", "starting", "running", "succeeded"]
    );
    let mode = std::os::unix::fs::MetadataExt::mode(&std::fs::metadata(&current).unwrap());
    assert_eq!(mode & 0o777, 0o600);

    daemon.stop();
    std::fs::write(daemon.state.join("events/notes.txt"), "x").unwrap();
    let invalid = daemon.job(&[
        "state",
        "validate",
        "--source",
        daemon.state.to_str().unwrap(),
    ]);
    assert!(!invalid.status.success());
    assert!(
        String::from_utf8_lossy(&invalid.stderr).contains("invalid event journal"),
        "{}",
        String::from_utf8_lossy(&invalid.stderr)
    );
}

#[test]
fn ops_queue_and_group_show_report_depth_and_wait_statistics() {
    let daemon = Daemon::start("depth");
    daemon.ok(&["group", "create", "team"]);
    daemon.ok(&["queue", "create", "build", "--group", "team"]);
    daemon.ok(&["queue", "pause", "team/build"]);
    let first = daemon.ok(&["submit", "--queue", "team/build", "--", "true"]);
    let second = daemon.ok(&["submit", "--queue", "team/build", "--", "true"]);
    daemon.ok(&["create", "--queue", "team/build", "--", "true"]);
    let other = daemon.ok(&["submit", "--", "sleep", "30"]);
    daemon.until(&other, &["Running"]);
    std::thread::sleep(Duration::from_millis(300));

    let shown = daemon.json(&["queue", "show", "team/build", "--json"]);
    let depth = &shown["objects"][0]["depth"];
    assert_eq!(depth["held"], 1, "{shown}");
    assert_eq!(depth["queued"], 2);
    assert_eq!(depth["running"], 0);
    assert_eq!(depth["suspended"], 0);
    assert!(depth["oldest_queued_age_ms"].as_u64().unwrap() >= 300);
    assert_eq!(depth["started_last_hour"]["count"], 0);
    assert!(depth["started_last_hour"]["median_ms"].is_null());
    assert_eq!(depth["started_last_hour"]["window_ms"], 3_600_000);
    assert_eq!(depth["started_last_hour"]["sample_limit"], 4096);

    let root = daemon.json(&["group", "show", "/", "--json"]);
    let depth = &root["objects"][0]["depth"];
    assert_eq!(depth["queued"], 2, "{root}");
    assert_eq!(depth["running"], 1);
    assert_eq!(depth["started_last_hour"]["count"], 1);

    let listed = daemon.json(&["queue", "list", "--json"]);
    assert!(listed["objects"][0].get("depth").is_none());

    daemon.ok(&["queue", "resume", "team/build"]);
    daemon.until(&first, &["Succeeded"]);
    daemon.until(&second, &["Succeeded"]);
    let shown = daemon.json(&["queue", "show", "team/build", "--json"]);
    let depth = &shown["objects"][0]["depth"];
    assert_eq!(depth["queued"], 0, "{shown}");
    assert!(depth["oldest_queued_age_ms"].is_null());
    let waits = &depth["started_last_hour"];
    assert_eq!(waits["count"], 2);
    let median = waits["median_ms"].as_u64().unwrap();
    let longest = waits["max_ms"].as_u64().unwrap();
    assert!(median >= 300 && median <= longest, "{waits}");

    let text = daemon.ok(&["group", "show", "team"]);
    assert!(
        text.contains("jobs: 1 held, 0 queued, 0 starting, 0 running, 0 suspended, 0 stopping"),
        "{text}"
    );
    assert!(
        text.contains("oldest queued Job waits since: none"),
        "{text}"
    );
    assert!(
        text.contains("started in the last hour: 2 Jobs, median wait"),
        "{text}"
    );
    let german = daemon
        .command(&["group", "show", "team"])
        .env("LC_ALL", "de_DE.UTF-8")
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&german.stdout).contains("in der letzten Stunde gestartet: 2 Jobs"),
        "{}",
        String::from_utf8_lossy(&german.stdout)
    );
    daemon.ok(&["cancel", &other]);
}

#[test]
fn ops_wait_statistics_are_rebuilt_from_records_after_a_restart() {
    let mut daemon = Daemon::start("depth-restart");
    let id = daemon.ok(&["submit", "--", "true"]);
    daemon.until(&id, &["Succeeded"]);
    daemon.restart();
    let shown = daemon.json(&["queue", "show", "default", "--json"]);
    assert_eq!(
        shown["objects"][0]["depth"]["started_last_hour"]["count"], 1,
        "{shown}"
    );
}

#[test]
fn ops_host_reports_health_and_doctor_reads_it() {
    let mut daemon = Daemon::start("health");
    let done = daemon.ok(&["submit", "--", "true"]);
    daemon.until(&done, &["Succeeded"]);
    let failed = daemon.ok(&["submit", "--", "false"]);
    daemon.until(&failed, &["Failed"]);
    let running = daemon.ok(&["submit", "--", "sleep", "30"]);
    daemon.until(&running, &["Running"]);
    daemon.ok(&["create", "--", "true"]);

    let host = daemon.json(&["host", "--json"]);
    let health = &host["health"];
    assert_eq!(health["protocol"], host["protocol"], "{host}");
    assert!(health["state_schema"].as_u64().unwrap() >= 18);
    assert_eq!(health["backend"], "Watch");
    assert_eq!(health["enforcement"], "monitoring_only");
    assert_eq!(health["jobs"]["succeeded"], 1);
    assert_eq!(health["jobs"]["failed"], 1);
    assert_eq!(health["jobs"]["running"], 1);
    assert_eq!(health["jobs"]["held"], 1);
    assert_eq!(health["jobs"]["queued"], 0);
    assert_eq!(health["jobs"]["lost"], 0);
    assert_eq!(health["supervisors_adopted"], 0);
    assert_eq!(health["recovery_changed_records"], 0);
    assert!(health["last_recovery_ms"].is_u64());
    assert!(health["state_free_bytes"].as_u64().unwrap() > 0);
    assert_eq!(health["audit_writable"], true);
    assert_eq!(health["events_writable"], true);
    let first_start = health["started_ms"].as_u64().unwrap();

    let text = daemon.ok(&["host"]);
    assert!(text.contains("health: up "), "{text}");
    assert!(
        text.contains("health: jobs 1 held, 0 queued, 0 starting, 1 running"),
        "{text}"
    );
    assert!(
        text.contains("audit journal writable, event journal writable"),
        "{text}"
    );

    let doctor = daemon.json(&["doctor", "--json"]);
    let check = doctor["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "health")
        .unwrap_or_else(|| panic!("{doctor}"));
    assert_eq!(check["status"], "ok", "{doctor}");
    assert!(
        check["detail"]
            .as_str()
            .unwrap()
            .contains("1 active and 1 waiting Jobs"),
        "{check}"
    );

    std::thread::sleep(Duration::from_millis(20));
    daemon.restart();
    let host = daemon.json(&["host", "--json"]);
    let health = &host["health"];
    assert_eq!(health["supervisors_adopted"], 1, "{host}");
    assert_eq!(health["jobs"]["running"], 1);
    assert_eq!(health["jobs"]["succeeded"], 1);
    assert!(health["started_ms"].as_u64().unwrap() > first_start);
    assert!(health["uptime_ms"].as_u64().unwrap() < 20_000);
    daemon.ok(&["cancel", &running]);

    daemon.stop();
    let doctor = daemon.job(&["doctor", "--json"]);
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let check = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "health")
        .unwrap();
    assert_eq!(check["status"], "warn");
}

#[test]
fn ops_required_cgroup_refuses_to_start_without_one_and_says_what_is_missing() {
    let base = Daemon::prepare("required", "[cgroup]\nrequired = true\n");
    let mut child = Daemon::spawn(&base, "");
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("the service kept running without a cgroup");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(!status.success());
    let log = std::fs::read_to_string(base.join("daemon.err")).unwrap();
    assert!(
        log.contains("[cgroup] required = true, and no usable delegated cgroup was found"),
        "{log}"
    );
    assert!(log.contains("JOB_CGROUP_ROOT names"), "{log}");
    assert!(log.contains("set required = false"), "{log}");
    assert!(!base.join("state/daemon.sock").exists());

    let doctor = Command::new(env!("CARGO_BIN_EXE_job"))
        .args(["doctor", "--json"])
        .env("JOB_STATE_DIR", base.join("state"))
        .env("JOB_CONFIG", base.join("config.toml"))
        .env("JOB_CGROUP_ROOT", base.join("absent-cgroup"))
        .env("LC_ALL", "C")
        .env_remove("JOB_RUNTIME_DIR")
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let check = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "cgroup_root")
        .unwrap();
    assert_eq!(check["status"], "fail", "{report}");
    let checked = Command::new(env!("CARGO_BIN_EXE_job"))
        .args([
            "config",
            "check",
            base.join("config.toml").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(checked.status.success());
    std::fs::remove_dir_all(&base).unwrap();
}

#[test]
fn ops_monitoring_mode_is_announced_and_reported() {
    let daemon = Daemon::configured("monitoring", "[cgroup]\nrequired = false\n", "");
    let log = daemon.log();
    assert!(
        log.contains(
            "job daemon: monitoring mode: no delegated cgroup, limits are watched, not enforced"
        ),
        "{log}"
    );
    assert!(log.contains("limits watched, not enforced, pool"), "{log}");
    let host = daemon.json(&["host", "--json"]);
    assert_eq!(host["backend"], "Watch");
    assert_eq!(host["health"]["enforcement"], "monitoring_only");
    assert_eq!(host["health"]["cgroup_required"], false);
    let text = daemon.ok(&["host"]);
    assert!(
        text.contains(
            "health: monitoring only: no delegated cgroup, limits are watched, not enforced"
        ),
        "{text}"
    );
    let doctor = daemon.json(&["doctor", "--json"]);
    let check = doctor["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "enforcement")
        .unwrap_or_else(|| panic!("{doctor}"));
    assert_eq!(check["status"], "warn");
    assert!(
        check["detail"]
            .as_str()
            .unwrap()
            .contains("monitoring mode"),
        "{check}"
    );
    assert!(
        check["fix"]
            .as_str()
            .unwrap()
            .contains("[cgroup] required = true"),
        "{check}"
    );
    let plain = daemon.job(&["run", "--summary", "--", "true"]);
    assert!(plain.status.success());
    assert!(
        String::from_utf8_lossy(&plain.stdout).contains("limits watched, not enforced"),
        "{}",
        String::from_utf8_lossy(&plain.stdout)
    );
}

#[test]
fn ops_monitoring_mode_refuses_explicit_limits_with_a_capability_error() {
    let daemon = Daemon::start("monitoring-refusal");
    for (option, value, named) in [
        ("--mem", "64M", "--mem (memory.max)"),
        ("--pids", "20", "--pids (pids.max)"),
    ] {
        for verb in ["run", "submit"] {
            let refused = daemon.job(&[verb, option, value, "--", "true"]);
            let error = String::from_utf8_lossy(&refused.stderr).into_owned();
            assert!(!refused.status.success(), "{verb} {option}");
            assert!(error.contains("capability error"), "{error}");
            assert!(error.contains(named), "{error}");
            assert!(error.contains("job doctor"), "{error}");
        }
    }
    for (option, value, file) in [
        ("--memory-max", "64M", "memory.max"),
        ("--memory-high", "64M", "memory.high"),
        ("--cpu-limit", "1", "cpu.max"),
    ] {
        let refused = daemon.job(&["submit", option, value, "--", "true"]);
        let error = String::from_utf8_lossy(&refused.stderr).into_owned();
        assert!(!refused.status.success(), "{option}");
        assert!(
            error.contains("requested resource control is unavailable") && error.contains(file),
            "{error}"
        );
    }
    assert!(
        std::fs::read_dir(daemon.state.join("jobs"))
            .unwrap()
            .next()
            .is_none()
    );
    daemon.ok(&["create", "--mem", "64M", "--", "true"]);
    let released = daemon.job(&["release", "1"]);
    assert!(!released.status.success());
    assert!(
        String::from_utf8_lossy(&released.stderr).contains("capability error"),
        "{}",
        String::from_utf8_lossy(&released.stderr)
    );
    assert_eq!(daemon.status("1")["state"], "Held");
    let requests = daemon.ok(&[
        "submit",
        "--cores",
        "1",
        "--cpu-request",
        "1",
        "--memory-request",
        "8M",
        "--time",
        "20s",
        "--",
        "true",
    ]);
    daemon.until(&requests, &["Succeeded"]);
}

#[test]
fn ops_legacy_profile_keeps_watching_limits_in_monitoring_mode() {
    let base = std::env::temp_dir().join(format!("job-ops-{}-legacy-watch", std::process::id()));
    std::fs::create_dir_all(base.join("work")).unwrap();
    std::fs::write(
        base.join("config.toml"),
        "schema_version = 1\nprofile = 'legacy'\n",
    )
    .unwrap();
    let child = Daemon::spawn(&base, "");
    let mut daemon = Daemon {
        child,
        state: base.join("state"),
        work: base.join("work"),
        base,
    };
    daemon.ready();
    let output = daemon.job(&[
        "run",
        "--summary",
        "--mem",
        "64M",
        "--pids",
        "64",
        "--",
        "true",
    ]);
    let answer = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{answer} {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(answer.contains("limits watched, not enforced"), "{answer}");
}

fn tightest_around_this_test(file: &str) -> (Option<u64>, Option<String>) {
    let own = std::fs::read_to_string("/proc/self/cgroup").unwrap();
    let relative = own
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .unwrap()
        .trim()
        .trim_start_matches('/')
        .to_owned();
    let mount = std::path::Path::new("/sys/fs/cgroup");
    let mut path = mount.join(&relative);
    let mut best: (Option<u64>, Option<String>) = (None, None);
    while path != mount && path.starts_with(mount) {
        if let Some(value) = std::fs::read_to_string(path.join(file))
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            && best.0.is_none_or(|least| value <= least)
        {
            best = (
                Some(value),
                Some(format!("/{}", path.strip_prefix(mount).unwrap().display())),
            );
        }
        path = path.parent().unwrap().to_path_buf();
    }
    best
}

#[test]
fn ops_host_and_doctor_report_the_limits_around_the_service() {
    use std::os::unix::process::CommandExt;
    let base = Daemon::prepare("surrounding", "");
    let mut command = Command::new(env!("CARGO_BIN_EXE_job"));
    command
        .arg("daemon")
        .env("JOB_CGROUP_ROOT", base.join("absent-cgroup"))
        .env("JOB_CONFIG", base.join("config.toml"))
        .env("JOB_STATE_DIR", base.join("state"))
        .env("LC_ALL", "C")
        .env_remove("JOB_FAILPOINT")
        .env_remove("JOB_RUNTIME_DIR")
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            std::fs::File::create(base.join("daemon.err")).unwrap(),
        ));
    unsafe {
        command.pre_exec(|| {
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            limit.rlim_cur = 777;
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let core = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::setrlimit(libc::RLIMIT_CORE, &core) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn().unwrap();
    let mut daemon = Daemon {
        child,
        state: base.join("state"),
        work: base.join("work"),
        base,
    };
    daemon.ready();
    let host = daemon.json(&["host", "--json"]);
    let limits = &host["surrounding_limits"];
    assert_eq!(limits["rlimits"]["nofile"]["soft"], 777, "{limits}");
    assert!(
        limits["rlimits"]["nofile"]["hard"]
            .as_u64()
            .is_none_or(|hard| hard >= 777)
    );
    assert_eq!(limits["rlimits"]["core"]["soft"], 0);
    assert_eq!(limits["rlimits"]["core"]["hard"], 0);
    assert_eq!(limits["rlimits"].as_object().unwrap().len(), 14);
    assert_eq!(limits["discoverable"], true);
    let own = std::fs::read_to_string("/proc/self/cgroup").unwrap();
    let own = own
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .unwrap()
        .trim();
    assert_eq!(limits["cgroup"], own);
    for (file, field) in [
        ("memory.max", "memory_max"),
        ("memory.high", "memory_high"),
        ("pids.max", "pids_max"),
    ] {
        let (value, set_by) = tightest_around_this_test(file);
        match value {
            Some(value) => {
                assert_eq!(
                    limits[field]["value"],
                    value.to_string(),
                    "{file}: {limits}"
                );
                assert_eq!(limits[field]["set_by"], set_by.unwrap(), "{file}: {limits}");
            }
            None => {
                assert!(limits[field]["set_by"].is_null(), "{file}: {limits}");
                assert!(
                    limits[field]["value"].is_null() || limits[field]["value"] == "max",
                    "{file}: {limits}"
                );
            }
        }
    }
    assert!(limits.get("cpu_max").is_some());
    assert!(limits.get("cpuset_cpus_effective").is_some());

    let text = daemon.ok(&["host"]);
    assert!(text.contains("surrounding cgroup /"), "{text}");
    assert!(text.contains("memory.max "), "{text}");
    assert!(
        text.contains("surrounding rlimits of the service (soft/hard): "),
        "{text}"
    );
    assert!(text.contains("nofile 777/"), "{text}");
    assert!(text.contains("core 0/0"), "{text}");

    let doctor = daemon.json(&["doctor", "--json"]);
    let check = doctor["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "surrounding_limits")
        .unwrap_or_else(|| panic!("{doctor}"));
    assert_eq!(check["status"], "ok", "{check}");
    let detail = check["detail"].as_str().unwrap();
    assert!(detail.contains("nofile 777/"), "{detail}");
    assert!(detail.contains("pids.max "), "{detail}");
    assert!(!detail.contains("not of a running service"), "{detail}");

    daemon.stop();
    let offline: Value = serde_json::from_slice(&daemon.job(&["doctor", "--json"]).stdout).unwrap();
    let check = offline["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "surrounding_limits")
        .unwrap();
    assert!(
        check["detail"]
            .as_str()
            .unwrap()
            .contains("limits of this command, not of a running service"),
        "{check}"
    );
}

struct Shared {
    base: PathBuf,
    child: Child,
    uid: u32,
    gid: u32,
}

fn subordinate(file: &str) -> Option<u32> {
    let name = String::from_utf8(Command::new("id").arg("-un").output().ok()?.stdout).ok()?;
    std::fs::read_to_string(file)
        .ok()?
        .lines()
        .find_map(|line| {
            let mut fields = line.split(':');
            (fields.next() == Some(name.trim()))
                .then(|| fields.next()?.parse().ok())
                .flatten()
        })
}

impl Shared {
    fn clean(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("LC_ALL", "C")
            .env("JOB_SESSION", "ops-shared")
            .env("JOB_CGROUP_ROOT", self.base.join("absent-cgroup"))
            .env("JOB_STATE_DIR", self.base.join("state"))
            .env("JOB_RUNTIME_DIR", self.base.join("run"));
        command
    }

    fn foreign(&self, inner_gid: u32, arguments: &[&str]) -> Command {
        let mut command = self.clean("unshare");
        command
            .arg(format!("--map-users=1:{}:1", self.uid))
            .arg("--map-user=0")
            .arg(format!("--map-groups=1:{}:1", self.gid))
            .arg("--map-group=0")
            .args(["setpriv", "--reuid", "1", "--regid"])
            .arg(inner_gid.to_string())
            .arg("--clear-groups")
            .arg(self.base.join("bin/job"))
            .args(arguments)
            .current_dir("/");
        command
    }

    fn member(&self, arguments: &[&str]) -> Output {
        self.foreign(0, arguments).output().unwrap()
    }

    fn own(&self, arguments: &[&str]) -> String {
        let output = self
            .clean(env!("CARGO_BIN_EXE_job"))
            .env("JOB_CONFIG", self.base.join("config.toml"))
            .args(arguments)
            .current_dir(self.base.join("work"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{arguments:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn start(name: &str) -> Option<Self> {
        use std::os::unix::fs::PermissionsExt;
        let (Some(uid), Some(gid)) = (subordinate("/etc/subuid"), subordinate("/etc/subgid"))
        else {
            println!(
                "skipped: this user has no subordinate user and group ids, so no second user can be made without root"
            );
            return None;
        };
        let base = std::env::temp_dir().join(format!("job-ops-{}-{name}", std::process::id()));
        std::fs::create_dir_all(base.join("work")).unwrap();
        std::fs::create_dir_all(base.join("bin")).unwrap();
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_job"), base.join("bin/job")).unwrap();
        std::fs::set_permissions(base.join("bin/job"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let primary = unsafe { libc::getegid() };
        std::fs::write(
            base.join("config.toml"),
            format!("schema_version = 1\nprofile = 'ordinary'\n[socket]\ngroup = \"{primary}\"\n"),
        )
        .unwrap();
        let mut daemon = Command::new(env!("CARGO_BIN_EXE_job"));
        daemon
            .arg("daemon")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("LC_ALL", "C")
            .env("JOB_CGROUP_ROOT", base.join("absent-cgroup"))
            .env("JOB_CONFIG", base.join("config.toml"))
            .env("JOB_STATE_DIR", base.join("state"))
            .env("JOB_RUNTIME_DIR", base.join("run"))
            .current_dir(base.join("work"))
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                std::fs::File::create(base.join("daemon.err")).unwrap(),
            ));
        let child = daemon.spawn().unwrap();
        let mut shared = Self {
            base,
            child,
            uid,
            gid,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while UnixStream::connect(shared.base.join("run/job.sock")).is_err() {
            assert!(
                shared.child.try_wait().unwrap().is_none() && Instant::now() < deadline,
                "{}",
                std::fs::read_to_string(shared.base.join("daemon.err")).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let probe = shared.foreign(1, &["doctor", "--bogus"]).output();
        if !probe
            .as_ref()
            .is_ok_and(|output| output.status.code() == Some(125))
        {
            println!(
                "skipped: a process with a subordinate user id cannot be started here: {:?}",
                probe.map(|output| String::from_utf8_lossy(&output.stderr).into_owned())
            );
            return None;
        }
        Some(shared)
    }

    fn finished(&self, id: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let output = self
                .clean(env!("CARGO_BIN_EXE_job"))
                .env("JOB_CONFIG", self.base.join("config.toml"))
                .args(["status", id, "--json"])
                .current_dir(self.base.join("work"))
                .output()
                .unwrap();
            let status: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
            if ["Succeeded", "Failed", "Cancelled"]
                .iter()
                .any(|state| status["state"] == *state)
            {
                return;
            }
            assert!(Instant::now() < deadline, "job {id} did not end: {status}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn texts(output: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn ops_a_group_member_gets_output_from_the_service_and_the_state_stays_private() {
    use std::os::unix::fs::MetadataExt;
    let Some(shared) = Shared::start("member-output") else {
        return;
    };
    let id = shared.own(&[
        "submit",
        "--",
        "sh",
        "-c",
        "echo out-line; echo err-line >&2; exit 3",
    ]);
    shared.finished(&id);
    assert_eq!(
        std::fs::metadata(shared.base.join("state")).unwrap().mode() & 0o7777,
        0o700
    );
    let denied = shared.member(&["audit"]);
    assert!(!denied.status.success(), "{:?}", texts(&denied));

    let logs = shared.member(&["logs", &id]);
    let (out, err) = texts(&logs);
    assert!(logs.status.success(), "{out}{err}");
    assert!(
        out.contains("out-line") && out.contains("err-line"),
        "{out}{err}"
    );

    let only = shared.member(&["logs", &id, "--stream", "stderr"]);
    let (out, err) = texts(&only);
    assert!(only.status.success(), "{out}{err}");
    assert!(
        out.contains("err-line") && !out.contains("out-line"),
        "{out}"
    );

    let json = shared.member(&["logs", &id, "--json"]);
    let (out, err) = texts(&json);
    assert!(json.status.success(), "{out}{err}");
    let records: Vec<Value> = out
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(records.iter().any(|record| record["stream"] == "stdout"));
    assert!(records.iter().any(|record| record["stream"] == "stderr"));
    assert!(records.iter().all(|record| record["attempt"] == 1));
    let direct = shared.own(&["logs", &id, "--json"]);
    assert_eq!(out.trim(), direct);

    let log = shared.member(&["log", &id]);
    let (out, err) = texts(&log);
    assert!(log.status.success(), "{out}{err}");
    assert!(out.contains("out-line"), "{out}");
    let tail = shared.member(&["log", &id, "grep", "err-"]);
    let (out, _) = texts(&tail);
    assert!(
        out.contains("err-line") && !out.contains("out-line"),
        "{out}"
    );
    assert_eq!(out.trim(), shared.own(&["log", &id, "grep", "err-"]));

    let missing = shared.member(&["logs", "999"]);
    assert!(!missing.status.success());
    let other = shared.member(&["logs", &id, "--attempt", "7"]);
    assert!(!other.status.success());

    let slow = shared.own(&[
        "submit",
        "--",
        "sh",
        "-c",
        "echo first; sleep 1; echo second",
    ]);
    let followed = shared.member(&["logs", "--follow", &slow]);
    let (out, err) = texts(&followed);
    assert!(followed.status.success(), "{out}{err}");
    assert!(
        out.contains("first") && out.contains("second"),
        "{out}{err}"
    );

    let ran = shared.member(&["run", "--", "sh", "-c", "echo ran-here; exit 4"]);
    let (out, err) = texts(&ran);
    assert_eq!(ran.status.code(), Some(4), "{out}{err}");
    assert!(out.contains("ran-here"), "{out}{err}");

    let stranger = shared.foreign(1, &["logs", &id]).output().unwrap();
    let (out, err) = texts(&stranger);
    assert!(!stranger.status.success(), "{out}{err}");
    assert!(!out.contains("out-line"), "{out}");

    let doctor = shared.member(&["doctor", "--json"]);
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let find = |name: &str| {
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(find("access")["status"], "ok", "{report}");
    assert!(
        find("access")["detail"]
            .as_str()
            .unwrap()
            .contains("get output from the service"),
        "{report}"
    );
    assert_eq!(find("state_directory")["status"], "ok", "{report}");
}

#[test]
fn ops_a_group_member_attaches_to_a_terminal_in_the_runtime_directory() {
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::process::CommandExt;
    let Some(shared) = Shared::start("member-terminal") else {
        return;
    };
    let id = shared.own(&[
        "submit",
        "--pty",
        "--time",
        "30s",
        "--",
        "sh",
        "-c",
        "echo READY; read line; echo got:$line; sleep 0.3",
    ]);
    let socket = shared.base.join(format!("run/terminals/{id}.sock"));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(
            Instant::now() < deadline,
            "no terminal socket at {}",
            socket.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let primary = unsafe { libc::getegid() };
    let directory = std::fs::metadata(shared.base.join("run/terminals")).unwrap();
    assert_eq!(directory.mode() & 0o7777, 0o750);
    assert_eq!(directory.gid(), primary);
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::fs::metadata(&socket).unwrap().mode() & 0o7777 != 0o660 && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    let listed = std::fs::metadata(&socket).unwrap();
    assert_eq!(listed.mode() & 0o7777, 0o660);
    assert_eq!(listed.gid(), primary);
    assert!(
        !shared
            .base
            .join(format!("state/jobs/{id}/terminal.sock"))
            .exists()
    );

    let (mut master_fd, mut slave_fd) = (-1, -1);
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    let mut master = unsafe { File::from_raw_fd(master_fd) };
    let slave = unsafe { File::from_raw_fd(slave_fd) };
    for fd in [master_fd, slave_fd] {
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    let size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(unsafe { libc::ioctl(slave_fd, libc::TIOCSWINSZ, &size) }, 0);
    let mut command = shared.foreign(0, &["attach", &id]);
    command
        .stdin(slave.try_clone().unwrap())
        .stdout(slave.try_clone().unwrap())
        .stderr(slave.try_clone().unwrap());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    drop(command);
    drop(slave);
    let mut seen = Vec::new();
    let until = |text: &str, master: &mut File, seen: &mut Vec<u8>| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !String::from_utf8_lossy(seen).contains(text) {
            assert!(
                Instant::now() < deadline,
                "no {text} in {}",
                String::from_utf8_lossy(seen)
            );
            let mut poll = libc::pollfd {
                fd: master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut poll, 1, 100) } > 0 {
                let mut bytes = [0; 8192];
                match master.read(&mut bytes) {
                    Ok(count) if count > 0 => seen.extend_from_slice(&bytes[..count]),
                    _ => break,
                }
            }
        }
    };
    until("READY", &mut master, &mut seen);
    master.write_all(b"hello\r").unwrap();
    until("got:hello", &mut master, &mut seen);
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("the member's attach did not end with the Job");
        }
        let mut poll = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, 50) } > 0 {
            let mut bytes = [0; 8192];
            let _ = master.read(&mut bytes);
        }
    };
    assert!(status.success(), "{status:?}");
    shared.finished(&id);
    let deadline = Instant::now() + Duration::from_secs(5);
    while socket.exists() {
        assert!(Instant::now() < deadline, "the terminal socket stayed");
        std::thread::sleep(Duration::from_millis(20));
    }
}
