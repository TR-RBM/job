use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
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
        Self::with_failpoint(name, "")
    }

    fn with_failpoint(name: &str, failpoint: &str) -> Daemon {
        let base = std::env::temp_dir().join(format!("job-pace-{}-{name}", std::process::id()));
        std::fs::create_dir_all(base.join("work")).unwrap();
        std::fs::write(
            base.join("config.toml"),
            "schema_version = 1\nprofile = 'ordinary'\n",
        )
        .unwrap();
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

    fn died(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "the daemon did not stop");
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::remove_file(self.state.join("daemon.sock"));
    }

    fn restart(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(self.state.join("daemon.sock"));
        self.child = Self::spawn(&self.base, "");
        self.ready();
    }

    fn ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(20);
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

    fn job(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_job"))
            .args(args)
            .env("JOB_STATE_DIR", &self.state)
            .env("JOB_CONFIG", self.base.join("config.toml"))
            .env("JOB_SESSION", "pace")
            .env("LC_ALL", "C")
            .env_remove("JOB_FAILPOINT")
            .env_remove("JOB_CLI_COMPAT")
            .env_remove("JOB_RUNTIME_DIR")
            .current_dir(&self.work)
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

    fn status(&self, id: &str) -> Value {
        let output = self.job(&["status", id, "--json"]);
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)))
    }

    fn activity(&self) -> Value {
        let host: Value = serde_json::from_str(&self.ok(&["host", "--json"])).unwrap();
        host["activity"].clone()
    }

    fn last_change(&self, action: &str) -> (u64, u64) {
        let spent = self.activity()["last_change"].clone();
        assert_eq!(spent["action"], action, "{spent}");
        (
            spent["sync_calls"].as_u64().unwrap(),
            spent["job_record_writes"].as_u64().unwrap(),
        )
    }

    fn health(&self) -> Value {
        let host: Value = serde_json::from_str(&self.ok(&["host", "--json"])).unwrap();
        host["health"].clone()
    }

    fn cancelled_events(&self) -> Vec<u64> {
        let mut jobs: Vec<u64> = self
            .lines("events/current.jsonl")
            .into_iter()
            .filter(|line| line["kind"] == "job" && line["to"] == "cancelled")
            .map(|line| line["job"].as_u64().unwrap())
            .collect();
        jobs.sort_unstable();
        jobs
    }

    fn staged(&self) -> Vec<std::path::PathBuf> {
        std::fs::read_dir(self.state.join("jobs"))
            .unwrap()
            .map(|entry| entry.unwrap().path().join("job.cancelling"))
            .filter(|path| path.exists())
            .collect()
    }

    fn lines(&self, file: &str) -> Vec<Value> {
        std::fs::read_to_string(self.state.join(file))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    fn states(&self, ids: &[String]) -> Vec<String> {
        ids.iter()
            .map(|id| self.status(id)["state"].as_str().unwrap().to_lowercase())
            .collect()
    }

    fn until_all(&self, ids: &[String], wanted: &[&str]) {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let states = self.states(ids);
            if states.iter().all(|state| wanted.contains(&state.as_str())) {
                return;
            }
            assert!(Instant::now() < deadline, "jobs stayed {states:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn started(&self, id: &str) -> Option<u64> {
        let path = self.state.join("jobs").join(id).join("started.json");
        let value: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        value["at_ms"].as_u64()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn submit_many(daemon: &Daemon, queue: &str, count: usize, command: &[&str]) -> Vec<String> {
    (0..count)
        .map(|_| {
            let mut args = vec!["submit", "-q", queue, "--"];
            args.extend_from_slice(command);
            daemon.ok(&args)
        })
        .collect()
}

#[test]
fn pace_a_resume_returns_before_the_starts_it_enables_and_reads_are_answered_meanwhile() {
    const JOBS: usize = 40;
    let daemon = Daemon::start("resume");
    daemon.ok(&["queue", "create", "burst"]);
    daemon.ok(&["queue", "pause", "burst"]);
    let ids = submit_many(&daemon, "burst", JOBS, &["true"]);
    daemon.ok(&["queue", "resume", "burst"]);
    let mut seen_waiting = 0;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let listed: Value = serde_json::from_str(&daemon.ok(&["list", "--json"])).unwrap();
        let waiting = listed["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["job"]["state"] == "Queued")
            .count();
        if waiting == 0 {
            break;
        }
        if waiting < JOBS {
            seen_waiting += 1;
        }
        assert!(Instant::now() < deadline, "{waiting} Jobs stayed queued");
    }
    assert!(
        seen_waiting >= 1,
        "no read was answered while the burst was starting"
    );
    daemon.until_all(&ids, &["succeeded"]);
    let resumed = daemon
        .lines("audit/current.jsonl")
        .into_iter()
        .rfind(|line| {
            line["phase"] == "end"
                && (line["action"] == "object-resume" || line["action"] == "queue-set")
        })
        .expect("the resume has a result line");
    assert_eq!(resumed["result"], "ok");
    let answered = resumed["at_ms"].as_u64().unwrap();
    let starts: Vec<u64> = daemon
        .lines("events/current.jsonl")
        .into_iter()
        .filter(|line| line["kind"] == "job" && line["to"] == "starting")
        .map(|line| line["at_ms"].as_u64().unwrap())
        .collect();
    assert_eq!(starts.len(), JOBS);
    let before_answer = starts.iter().filter(|at| **at <= answered).count();
    assert!(
        before_answer <= 3,
        "{before_answer} of {JOBS} Jobs were admitted before the resume was answered"
    );
}

#[test]
fn pace_a_job_started_late_in_a_burst_reports_its_own_start_and_run_time() {
    const JOBS: usize = 12;
    let daemon = Daemon::start("start-time");
    daemon.ok(&["queue", "create", "burst"]);
    daemon.ok(&["queue", "pause", "burst"]);
    let ids = submit_many(&daemon, "burst", JOBS, &["sleep", "0.3"]);
    daemon.ok(&["queue", "resume", "burst"]);
    daemon.until_all(&ids, &["succeeded"]);
    let mut starts = Vec::new();
    for id in &ids {
        let status = daemon.status(id);
        let started = status["started_ms"].as_u64().unwrap();
        let admitted = status["admitted_ms"].as_u64().unwrap();
        let finished = status["finished_ms"].as_u64().unwrap();
        assert_eq!(
            Some(started),
            daemon.started(id),
            "job {id}: started_ms is the time its supervisor started the command"
        );
        assert!(admitted <= started, "job {id}: admitted after it started");
        assert_eq!(
            status["timing"]["elapsed_ms"].as_u64().unwrap(),
            finished - started,
            "job {id}"
        );
        starts.push(started);
    }
    let first = *starts.iter().min().unwrap();
    let last = *starts.iter().max().unwrap();
    assert!(last > first, "every Job carries the same start time");
    let late = daemon.status(ids.last().unwrap());
    let exit: Value = serde_json::from_slice(
        &std::fs::read(
            daemon
                .state
                .join("jobs")
                .join(ids.last().unwrap())
                .join("exit.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let ran = exit["at_ms"].as_u64().unwrap() - late["started_ms"].as_u64().unwrap();
    let elapsed = late["timing"]["elapsed_ms"].as_u64().unwrap();
    assert!(ran >= 300, "the late Job ran {ran} ms");
    assert!(
        elapsed >= ran,
        "the late Job ran {ran} ms and reports {elapsed} ms"
    );
    assert_eq!(late["timing"]["active_ms"], late["timing"]["elapsed_ms"]);
}

#[test]
fn pace_a_submission_is_acknowledged_after_six_syncs_and_one_record_write() {
    let daemon = Daemon::start("submit");
    daemon.ok(&["queue", "create", "parked"]);
    daemon.ok(&["queue", "pause", "parked"]);
    daemon.ok(&["submit", "-q", "parked", "--", "true"]);
    let id = daemon.ok(&["submit", "-q", "parked", "--", "true"]);
    assert_eq!(daemon.last_change("submit"), (6, 1));
    let status = daemon.status(&id);
    assert_eq!(status["state"], "Queued");
    assert_eq!(
        status["waited_for"].as_str(),
        Some("admission paused by parked")
    );
    let directory = daemon.state.join("jobs").join(&id);
    for name in ["job.json", "env.json", "submitted-env.json"] {
        assert!(directory.join(name).is_file(), "{name}");
    }
    assert_eq!(
        std::fs::read_dir(daemon.state.join(".transactions"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn pace_concurrent_submitters_are_all_recorded_with_distinct_ids() {
    let daemon = Daemon::start("group");
    daemon.ok(&["queue", "create", "parked"]);
    daemon.ok(&["queue", "pause", "parked"]);
    let mut ids: Vec<u64> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    (0..6)
                        .map(|_| {
                            daemon
                                .ok(&["submit", "-q", "parked", "--", "true"])
                                .parse::<u64>()
                                .unwrap()
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect()
    });
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 24);
    let audit = daemon.lines("audit/current.jsonl");
    for phase in ["begin", "end"] {
        assert_eq!(
            audit
                .iter()
                .filter(|line| line["action"] == "submit" && line["phase"] == phase)
                .count(),
            24,
            "{phase}"
        );
    }
    let mut begun: Vec<u64> = audit
        .iter()
        .filter(|line| line["phase"] == "begin")
        .map(|line| line["seq"].as_u64().unwrap())
        .collect();
    let total = begun.len();
    begun.sort_unstable();
    begun.dedup();
    assert_eq!(begun.len(), total, "two intents share a sequence number");
}

#[test]
fn pace_pause_and_resume_rewrite_no_waiting_job_record() {
    const WAITING: usize = 20;
    let daemon = Daemon::start("pause");
    daemon.ok(&["queue", "create", "narrow", "--max-running", "1"]);
    let running = daemon.ok(&["submit", "-q", "narrow", "--", "sleep", "60"]);
    let ids = submit_many(&daemon, "narrow", WAITING, &["true"]);
    daemon.until_all(std::slice::from_ref(&running), &["running"]);
    let inode = |id: &String| {
        std::fs::metadata(daemon.state.join("jobs").join(id).join("job.json"))
            .unwrap()
            .ino()
    };
    let before: Vec<u64> = ids.iter().map(inode).collect();
    assert_eq!(
        daemon.status(&ids[0])["waited_for"].as_str(),
        Some("max-running 1 at narrow")
    );
    daemon.ok(&["queue", "pause", "narrow"]);
    assert_eq!(daemon.last_change("queue-set").1, 0);
    assert_eq!(
        daemon.status(&ids[WAITING - 1])["waited_for"].as_str(),
        Some("admission paused by narrow")
    );
    daemon.ok(&["queue", "resume", "narrow"]);
    assert_eq!(
        daemon.status(&ids[WAITING - 1])["waited_for"].as_str(),
        Some("max-running 1 at narrow")
    );
    assert_eq!(daemon.last_change("queue-set").1, 0);
    assert_eq!(before, ids.iter().map(inode).collect::<Vec<_>>());
    daemon.ok(&["cancel", &running]);
    daemon.until_all(std::slice::from_ref(&running), &["cancelled"]);
    daemon.until_all(&ids, &["succeeded"]);
}

#[test]
fn pace_a_wait_reason_is_current_again_after_a_restart() {
    let mut daemon = Daemon::start("reason-restart");
    daemon.ok(&["queue", "create", "parked"]);
    let id = daemon.ok(&["create", "-q", "parked", "--", "true"]);
    daemon.ok(&["queue", "pause", "parked"]);
    daemon.ok(&["release", &id]);
    assert_eq!(
        daemon.status(&id)["waited_for"].as_str(),
        Some("admission paused by parked")
    );
    daemon.restart();
    let status = daemon.status(&id);
    assert_eq!(status["state"], "Queued");
    assert_eq!(
        status["waited_for"].as_str(),
        Some("admission paused by parked")
    );
}

fn held(daemon: &Daemon, count: usize) -> Vec<String> {
    daemon.ok(&["queue", "create", "many"]);
    (0..count)
        .map(|_| daemon.ok(&["create", "-q", "many", "--", "true"]))
        .collect()
}

#[test]
fn pace_recursive_cancel_and_remove_write_each_record_once_and_sync_no_more_than_twice_per_job() {
    const JOBS: usize = 40;
    let daemon = Daemon::start("cancel");
    let ids = held(&daemon, JOBS);
    daemon.ok(&["queue", "cancel", "many", "--recursive"]);
    let (syncs, writes) = daemon.last_change("cancel");
    assert_eq!(writes, JOBS as u64);
    assert!(
        syncs <= 2 * JOBS as u64 + 12,
        "cancelling {JOBS} held Jobs cost {syncs} sync calls"
    );
    assert!(daemon.states(&ids).iter().all(|state| state == "cancelled"));
    assert_eq!(
        daemon.cancelled_events(),
        ids.iter()
            .map(|id| id.parse::<u64>().unwrap())
            .collect::<Vec<_>>()
    );
    assert!(daemon.staged().is_empty());
    daemon.ok(&["queue", "remove", "many", "--recursive"]);
    let (syncs, writes) = daemon.last_change("remove");
    assert_eq!(writes, 0);
    assert!(
        syncs <= 30,
        "removing {JOBS} cancelled Jobs cost {syncs} sync calls"
    );
    assert!(
        daemon
            .state
            .join("jobs")
            .read_dir()
            .unwrap()
            .next()
            .is_none()
    );
}

fn interrupted_cancel(name: &str, failpoint: &str, published: bool) {
    const JOBS: usize = 12;
    let mut daemon = Daemon::with_failpoint(name, failpoint);
    let ids = held(&daemon, JOBS);
    let output = daemon.job(&["queue", "cancel", "many", "--recursive"]);
    assert!(!output.status.success());
    daemon.died();
    let jobs = daemon.state.join("jobs");
    let recorded = |id: &String| -> Value {
        serde_json::from_slice(&std::fs::read(jobs.join(id).join("job.json")).unwrap()).unwrap()
    };
    for id in &ids {
        let state = recorded(id)["state"].clone();
        assert_eq!(
            state,
            if published { "Cancelled" } else { "Held" },
            "job {id}"
        );
    }
    daemon.child = Daemon::spawn(&daemon.base, "");
    daemon.ready();
    daemon.until_all(&ids, &["cancelled"]);
    for id in &ids {
        assert_eq!(recorded(id)["state"], "Cancelled");
        assert!(
            !daemon
                .state
                .join("jobs")
                .join(id)
                .join("job.cancelling")
                .exists()
        );
    }
    let operations: Vec<Value> = std::fs::read_dir(daemon.state.join("cancellations"))
        .unwrap()
        .map(|entry| {
            serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .collect();
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0]["complete"], true);
    assert_eq!(operations[0]["results"].as_array().unwrap().len(), JOBS);
    assert_eq!(
        daemon.cancelled_events(),
        ids.iter()
            .map(|id| id.parse::<u64>().unwrap())
            .collect::<Vec<_>>(),
        "every member has exactly one cancelled event"
    );
    assert!(daemon.staged().is_empty());
}

#[test]
fn pace_a_crash_after_staging_a_cancel_batch_leaves_every_record_whole_and_is_replayed() {
    interrupted_cancel("cancel-stage", "cancel-batch-after-stage", false);
}

#[test]
fn pace_a_crash_after_publishing_a_cancel_batch_is_completed_on_restart() {
    interrupted_cancel("cancel-publish", "cancel-batch-after-publish", true);
}

#[test]
fn review_a_cancel_batch_that_cannot_be_synced_is_retried_with_backoff_and_reported() {
    const JOBS: usize = 12;
    let daemon = Daemon::with_failpoint("cancel-backoff", "cancel-batch-sync");
    let ids = held(&daemon, JOBS);
    let output = daemon.job(&["queue", "cancel", "many", "--recursive", "--json"]);
    let answer = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(
        answer.contains("cannot make the cancelled records durable"),
        "{answer}"
    );
    assert!(daemon.states(&ids).iter().all(|state| state == "held"));
    assert!(daemon.staged().is_empty());
    std::thread::sleep(Duration::from_secs(3));
    let health = daemon.health();
    assert_eq!(health["cancellations_failing"], 1, "{health}");
    let retries = health["cancellation_retries"].as_u64().unwrap();
    assert!(
        (2..=6).contains(&retries),
        "{retries} attempts in three seconds"
    );
    assert!(
        daemon
            .ok(&["host"])
            .contains("cancellations failing 1, retried")
    );
    assert_eq!(
        daemon
            .log()
            .matches("cancellation could not be completed")
            .count(),
        1
    );
    assert!(daemon.states(&ids).iter().all(|state| state == "held"));
    assert!(daemon.staged().is_empty());
}

#[test]
fn review_a_cancel_batch_that_fails_partway_completes_and_writes_each_event_once() {
    const JOBS: usize = 12;
    let daemon = Daemon::with_failpoint("cancel-partway", "cancel-batch-rename@5");
    let ids = held(&daemon, JOBS);
    let _ = daemon.job(&["queue", "cancel", "many", "--recursive"]);
    daemon.until_all(&ids, &["cancelled"]);
    let deadline = Instant::now() + Duration::from_secs(20);
    while daemon.health()["cancellations_failing"] != 0 {
        assert!(Instant::now() < deadline, "the operation stayed failing");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        daemon.cancelled_events(),
        ids.iter()
            .map(|id| id.parse::<u64>().unwrap())
            .collect::<Vec<_>>()
    );
    assert!(daemon.staged().is_empty());
}

#[test]
fn review_a_crash_in_the_middle_of_the_renames_leaves_no_staged_file_and_loses_no_event() {
    const JOBS: usize = 12;
    let mut daemon = Daemon::with_failpoint("cancel-mid-rename", "cancel-batch-mid-rename@9");
    let ids = held(&daemon, JOBS);
    daemon.ok(&["queue", "create", "other"]);
    let bystander = daemon.ok(&["create", "-q", "other", "--", "true"]);
    let output = daemon.job(&["queue", "cancel", "many", "--recursive"]);
    assert!(!output.status.success());
    daemon.died();
    assert_eq!(daemon.staged().len(), 4);
    let stray = daemon
        .state
        .join("jobs")
        .join(&bystander)
        .join("job.cancelling");
    std::fs::write(&stray, b"{}").unwrap();
    daemon.child = Daemon::spawn(&daemon.base, "");
    daemon.ready();
    daemon.until_all(&ids, &["cancelled"]);
    assert!(daemon.staged().is_empty());
    assert_eq!(daemon.status(&bystander)["state"], "Held");
    assert_eq!(
        daemon.cancelled_events(),
        ids.iter()
            .map(|id| id.parse::<u64>().unwrap())
            .collect::<Vec<_>>()
    );
}

#[test]
fn review_the_starter_goes_on_after_a_panic_and_the_service_says_so() {
    const JOBS: usize = 10;
    let daemon = Daemon::with_failpoint("starter-panic", "starter-panic@1");
    daemon.ok(&["queue", "create", "burst"]);
    daemon.ok(&["queue", "pause", "burst"]);
    let ids = submit_many(&daemon, "burst", JOBS, &["true"]);
    daemon.ok(&["queue", "resume", "burst"]);
    daemon.until_all(&ids, &["succeeded"]);
    assert_eq!(daemon.health()["starter_failures"], 1);
    assert_eq!(daemon.activity()["starts_pending"], false);
    assert_eq!(daemon.log().matches("the starter failed").count(), 1);
    assert!(daemon.ok(&["host"]).contains("the starter failed 1 times"));
    let more = submit_many(&daemon, "burst", 3, &["true"]);
    daemon.until_all(&more, &["succeeded"]);
}

#[test]
fn review_started_ms_has_one_value_while_the_job_runs_and_after_it_ended() {
    let daemon = Daemon::start("one-start");
    let id = daemon.ok(&["submit", "--", "sleep", "1.5"]);
    let deadline = Instant::now() + Duration::from_secs(20);
    let running = loop {
        let status = daemon.status(&id);
        if status["state"] == "Running" && status["started_ms"].is_u64() {
            break status;
        }
        assert!(status["started_ms"].is_null(), "{}", status["started_ms"]);
        assert!(status["admitted_ms"].is_u64() || status["state"] == "Queued");
        assert!(Instant::now() < deadline, "the start was never learned");
        std::thread::sleep(Duration::from_millis(20));
    };
    let started = running["started_ms"].as_u64().unwrap();
    assert_eq!(Some(started), daemon.started(&id));
    let row = |text: String| -> Vec<String> {
        text.lines()
            .nth(1)
            .unwrap()
            .split('\t')
            .map(str::to_owned)
            .collect()
    };
    let listed = loop {
        let listed = row(daemon.ok(&["list", "--format", "tsv"]));
        if !listed[7].is_empty() {
            break listed;
        }
        assert!(
            Instant::now() < deadline,
            "the listing never showed the start"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(listed[7], started.to_string());
    daemon.until_all(std::slice::from_ref(&id), &["succeeded"]);
    let ended = daemon.status(&id);
    assert_eq!(ended["started_ms"].as_u64(), Some(started));
    assert_eq!(ended["admitted_ms"], running["admitted_ms"]);
    assert!(ended["admitted_ms"].as_u64().unwrap() <= started);
    assert_eq!(
        ended["timing"]["elapsed_ms"].as_u64().unwrap(),
        ended["finished_ms"].as_u64().unwrap() - started
    );
    let listed = row(daemon.ok(&["list", "--state", "succeeded", "--format", "tsv"]));
    assert_eq!(listed[7], started.to_string());
    let listed: Value =
        serde_json::from_str(&daemon.ok(&["list", "--state", "succeeded", "--json"])).unwrap();
    assert_eq!(
        listed["jobs"][0]["job"]["admitted_ms"],
        ended["admitted_ms"]
    );
    assert_eq!(
        listed["jobs"][0]["job"]["started_ms"].as_u64(),
        Some(started)
    );
    let recorded: Value = serde_json::from_slice(
        &std::fs::read(daemon.state.join("jobs").join(&id).join("job.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(recorded["started_ms"].as_u64(), Some(started));
}

#[test]
fn review_wait_statistics_are_the_same_before_and_after_a_restart() {
    let mut daemon = Daemon::start("wait-restart");
    daemon.ok(&["queue", "create", "burst"]);
    daemon.ok(&["queue", "pause", "burst"]);
    let ids = submit_many(&daemon, "burst", 4, &["true"]);
    std::thread::sleep(Duration::from_millis(300));
    daemon.ok(&["queue", "resume", "burst"]);
    daemon.until_all(&ids, &["succeeded"]);
    let depth = |daemon: &Daemon| -> Value {
        let shown: Value =
            serde_json::from_str(&daemon.ok(&["queue", "show", "burst", "--json"])).unwrap();
        shown["objects"][0]["depth"]["started_last_hour"].clone()
    };
    let before = depth(&daemon);
    assert_eq!(before["count"], 4);
    let mut journaled: Vec<u64> = daemon
        .lines("events/current.jsonl")
        .into_iter()
        .filter(|line| line["kind"] == "job" && line["to"] == "starting")
        .map(|line| line["wait_ms"].as_u64().unwrap())
        .collect();
    journaled.sort_unstable();
    assert_eq!(before["max_ms"].as_u64(), journaled.last().copied());
    for id in &ids {
        let status = daemon.status(id);
        let wait = status["admitted_ms"].as_u64().unwrap()
            - status["released_ms"]
                .as_u64()
                .unwrap_or_else(|| status["submitted_ms"].as_u64().unwrap());
        assert!(journaled.contains(&wait), "{wait} is not in {journaled:?}");
    }
    daemon.restart();
    assert_eq!(depth(&daemon), before);
}

#[test]
fn review_a_pressure_hold_activates_in_a_replay_with_jobs_starting_every_half_second() {
    let base = std::env::temp_dir().join(format!("job-pace-{}-replay", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let rule = serde_json::json!({"id":"cpu","resource":"cpu","metric":"some","window":"avg10","high_bp":100,"low_bp":10,"sustain_ms":1000,"minimum_hold_ms":2000,"recovery_ms":1000,"step_ms":1000,"required":false});
    let frames = |spacing: u64| -> Vec<Value> {
        (0..8u64)
            .map(|index| {
                serde_json::json!({
                    "boot_id": "a",
                    "at_ms": index * spacing,
                    "signal": {"availability": "available", "value_bp": 1500, "job_id": 1, "attempt": 1, "missing": 0},
                    "active": 1 + index * spacing / 500,
                    "demand": 2 + index * spacing / 500
                })
            })
            .collect()
    };
    let phases = |spacing: u64| -> Vec<String> {
        let path = base.join(format!("trace-{spacing}.json"));
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"schema_version": 1, "host": false, "rule": rule, "frames": frames(spacing)})).unwrap(),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_job"))
            .args(["pressure", "replay", path.to_str().unwrap(), "--json"])
            .env("JOB_STATE_DIR", base.join("unused"))
            .env("LC_ALL", "C")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        report["samples"]
            .as_array()
            .unwrap()
            .iter()
            .map(|sample| sample["view"]["phase"].as_str().unwrap().to_owned())
            .collect()
    };
    let at_cadence = phases(1028);
    assert_eq!(at_cadence[0], "open");
    assert!(
        at_cadence[1..].iter().all(|phase| phase == "holding"),
        "{at_cadence:?}"
    );
    let delayed = phases(2028);
    assert!(delayed.iter().all(|phase| phase == "open"), "{delayed:?}");
    let _ = std::fs::remove_dir_all(&base);
}
