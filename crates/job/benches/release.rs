use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

#[allow(dead_code)]
#[path = "../tests/support/mod.rs"]
mod support;

const JOB: &str = env!("CARGO_BIN_EXE_job");
const CAP: Duration = Duration::from_secs(110);
const SUBMISSIONS: usize = 500;
const SUBMITTERS: usize = 4;
const SIZES: [usize; 2] = [1000, 10000];
const POPULATORS: usize = 8;
const THROUGH_CLI: usize = 100;
const OUTPUT_BYTES: u64 = 256 * 1024 * 1024;
const LINE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde";

enum Host {
    Watch { root: PathBuf, child: Child },
    Cgroup(Box<support::Service>),
}

struct Bench {
    host: Host,
}

fn spawn_watch(root: &Path) -> Child {
    Command::new(JOB)
        .arg("daemon")
        .env("JOB_CGROUP_ROOT", root.join("absent-cgroup"))
        .env("JOB_CONFIG", root.join("config.toml"))
        .env("JOB_STATE_DIR", root.join("state"))
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(root.join("daemon.err"))
                .unwrap(),
        ))
        .spawn()
        .unwrap()
}

impl Bench {
    fn start(name: &str, backend: &str) -> Self {
        let bench = if backend == "cgroup" {
            Self {
                host: Host::Cgroup(Box::new(support::Service::start(name))),
            }
        } else {
            let root =
                std::env::temp_dir().join(format!("job-bench-{}-{name}", std::process::id()));
            fs::create_dir(&root).unwrap();
            fs::write(
                root.join("config.toml"),
                "schema_version = 1\nprofile = 'ordinary'\n",
            )
            .unwrap();
            let child = spawn_watch(&root);
            Self {
                host: Host::Watch { root, child },
            }
        };
        fs::create_dir(bench.root().join("work")).unwrap();
        bench.answer();
        bench
    }

    fn root(&self) -> &Path {
        match &self.host {
            Host::Watch { root, .. } => root,
            Host::Cgroup(service) => &service.root,
        }
    }

    fn pid(&self) -> u32 {
        match &self.host {
            Host::Watch { child, .. } => child.id(),
            Host::Cgroup(service) => service.child.id(),
        }
    }

    fn ping(&self) -> bool {
        let Ok(mut stream) = UnixStream::connect(self.root().join("state/daemon.sock")) else {
            return false;
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(120)))
            .unwrap();
        let mut response = String::new();
        writeln!(stream, "\"Ping\"").is_ok()
            && stream.read_to_string(&mut response).is_ok()
            && response.contains("Pong")
    }

    fn answer(&self) -> Duration {
        let start = Instant::now();
        while !self.ping() {
            assert!(start.elapsed() < CAP, "the service did not answer");
            std::thread::sleep(Duration::from_millis(1));
        }
        start.elapsed()
    }

    fn stop(&mut self) {
        let child = match &mut self.host {
            Host::Watch { child, .. } => child,
            Host::Cgroup(service) => &mut service.child,
        };
        child.kill().unwrap();
        child.wait().unwrap();
    }

    fn begin(&mut self) -> Duration {
        let start = Instant::now();
        match &mut self.host {
            Host::Watch { root, child } => *child = spawn_watch(root),
            Host::Cgroup(service) => {
                service.child = support::Service::spawn(&service.root, &service.cgroup);
            }
        }
        self.answer();
        start.elapsed()
    }

    fn restart(&mut self) -> Duration {
        self.stop();
        self.begin()
    }

    fn capped(&self, args: &[&str]) -> (f64, Option<i32>, bool) {
        let start = Instant::now();
        let mut child = self
            .command(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return (milliseconds(start.elapsed()), status.code(), true);
            }
            if start.elapsed() > CAP {
                let _ = child.kill();
                let _ = child.wait();
                return (milliseconds(start.elapsed()), None, false);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(JOB);
        command
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.root())
            .env("LC_ALL", "C")
            .env("JOB_STATE_DIR", self.root().join("state"))
            .env("JOB_SESSION", "release-benchmark")
            .current_dir(self.root().join("work"))
            .stdin(Stdio::null());
        command
    }

    fn cli(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.cli(args);
        assert!(
            output.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn timed(&self, args: &[&str]) -> (f64, Output) {
        let start = Instant::now();
        let output = self.cli(args);
        (milliseconds(start.elapsed()), output)
    }

    fn status(&self, id: &str) -> Value {
        serde_json::from_str(&self.ok(&["status", id, "--json"])).unwrap()
    }

    fn rss_kib(&self) -> u64 {
        fs::read_to_string(format!("/proc/{}/status", self.pid()))
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))
            .unwrap()
            .trim()
            .trim_end_matches("kB")
            .trim()
            .parse()
            .unwrap()
    }

    fn waiting(&self, queue: &str) -> usize {
        let listed: Value =
            serde_json::from_str(&self.ok(&["list", "-q", queue, "--json"])).unwrap();
        listed["jobs"].as_array().unwrap().len()
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        if let Host::Watch { root, child } = &mut self.host {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_dir_all(root);
        }
    }
}

fn milliseconds(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1e6).round() / 1e3
}

fn realtime_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn summary(samples: &[f64]) -> Value {
    assert!(!samples.is_empty());
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = |fraction: f64| {
        let index = (fraction * sorted.len() as f64).ceil() as usize;
        sorted[index.clamp(1, sorted.len()) - 1]
    };
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    json!({
        "count": sorted.len(),
        "min": sorted[0],
        "median": rank(0.5),
        "p95": rank(0.95),
        "max": sorted[sorted.len() - 1],
        "mean": (mean * 1e3).round() / 1e3,
    })
}

fn tree_bytes(root: &Path) -> (u64, u64, u64) {
    let mut total = (0, 0, 0);
    let Ok(entries) = fs::read_dir(root) else {
        return total;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            let inner = tree_bytes(&entry.path());
            total = (total.0 + inner.0, total.1 + inner.1, total.2 + inner.2);
        } else if metadata.is_file() {
            total = (
                total.0 + metadata.len(),
                total.1 + metadata.blocks() * 512,
                total.2 + 1,
            );
        }
    }
    total
}

fn submit_many(bench: &Bench, queue: &str, submitters: usize) -> (Vec<String>, Value) {
    let each = SUBMISSIONS / submitters;
    let start = Instant::now();
    let results: Vec<(Vec<String>, Vec<f64>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..submitters)
            .map(|_| {
                scope.spawn(|| {
                    let mut ids = Vec::new();
                    let mut times = Vec::new();
                    while ids.len() < each && start.elapsed() < CAP {
                        let (time, output) = bench.timed(&["submit", "-q", queue, "--", "true"]);
                        assert!(
                            output.status.success(),
                            "{}",
                            String::from_utf8_lossy(&output.stderr)
                        );
                        ids.push(String::from_utf8(output.stdout).unwrap().trim().to_owned());
                        times.push(time);
                    }
                    (ids, times)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    let wall = start.elapsed();
    let ids: Vec<String> = results.iter().flat_map(|(ids, _)| ids.clone()).collect();
    let times: Vec<f64> = results
        .iter()
        .flat_map(|(_, times)| times.clone())
        .collect();
    let record = json!({
        "record": "measurement",
        "name": if submitters == 1 { "submit_latency_sequential" } else { "submit_latency_concurrent" },
        "what": "one job submit -q QUEUE -- true client process, from spawn until it printed the ID and exited, into a paused Queue",
        "submitters": submitters,
        "submissions": ids.len(),
        "reached_target_within_cap": ids.len() == each * submitters,
        "client_ms": summary(&times),
        "wall_ms": milliseconds(wall),
        "submissions_per_second": (ids.len() as f64 / wall.as_secs_f64()).round(),
    });
    (ids, record)
}

fn marker(bench: &Bench, id: &str, name: &str) -> Option<u64> {
    let path = bench.root().join("state/jobs").join(id).join(name);
    serde_json::from_slice::<Value>(&fs::read(path).ok()?).ok()?["at_ms"].as_u64()
}

fn dispatch(bench: &Bench, queue: &str, ids: &[String], limit: Option<usize>) -> Value {
    let done = std::sync::atomic::AtomicBool::new(false);
    let (resumed, resume_ms, complete, probes) = std::thread::scope(|scope| {
        let probe = scope.spawn(|| {
            let mut times = Vec::new();
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                times.push(bench.timed(&["queue", "show", queue]).0);
                std::thread::sleep(Duration::from_millis(50));
            }
            times
        });
        let resumed = realtime_ms();
        let (resume_ms, output) = bench.timed(&["queue", "resume", queue]);
        assert!(output.status.success());
        let start = Instant::now();
        let mut complete = true;
        while bench.waiting(queue) > 0 {
            if start.elapsed() > CAP {
                complete = false;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        (resumed, resume_ms, complete, probe.join().unwrap())
    });
    let mut recorded = Vec::new();
    let mut starts = Vec::new();
    let mut runs = Vec::new();
    let mut last = resumed;
    let mut succeeded = 0;
    for id in ids {
        let status = bench.status(id);
        if status["state"] == "Succeeded" {
            succeeded += 1;
        }
        if let Some(started) = status["started_ms"].as_u64() {
            recorded.push(started.saturating_sub(resumed) as f64);
        }
        if let (Some(began), Some(ended)) = (
            marker(bench, id, "started.json"),
            marker(bench, id, "exit.json"),
        ) {
            starts.push(began.saturating_sub(resumed) as f64);
            runs.push(ended.saturating_sub(began) as f64);
            last = last.max(ended);
        }
    }
    let mut ordered = starts.clone();
    ordered.sort_by(f64::total_cmp);
    let gaps: Vec<f64> = ordered.windows(2).map(|pair| pair[1] - pair[0]).collect();
    json!({
        "record": "measurement",
        "name": if limit.is_some() { "dispatch_max_running_4" } else { "dispatch_unlimited" },
        "what": "job queue resume of a Queue holding the submitted true Jobs. Command start and exit are the at_ms of started.json and exit.json, which the supervisor writes just before it runs the command and just after it has waited for it; they are compared with the driver's clock at the resume. A second client ran job queue show every 50 ms meanwhile",
        "jobs": ids.len(),
        "max_running": limit,
        "completed_within_cap": complete,
        "succeeded": succeeded,
        "resume_call_ms": resume_ms,
        "resume_to_command_start_ms": summary(&starts),
        "gap_between_consecutive_command_starts_ms": if gaps.is_empty() { Value::Null } else { summary(&gaps) },
        "command_start_to_exit_ms": summary(&runs),
        "recorded_started_ms_after_resume": summary(&recorded),
        "resume_to_last_exit_ms": last - resumed,
        "completions_per_second": ((succeeded as f64) * 1e4 / ((last - resumed).max(1) as f64)).round() / 10.0,
        "other_client_queue_show_ms_meanwhile": summary(&probes),
    })
}

fn idle_round_trip(bench: &Bench) -> Value {
    let mut clients = Vec::new();
    let mut starts = Vec::new();
    let mut totals = Vec::new();
    let mut commands = Vec::new();
    for _ in 0..100 {
        let begin = Instant::now();
        let id = bench.ok(&["submit", "--", "true"]);
        let waited = bench.cli(&["wait", &id, "--timeout", "60s"]);
        clients.push(milliseconds(begin.elapsed()));
        assert!(waited.status.success());
        let status = bench.status(&id);
        let submitted = status["submitted_ms"].as_u64().unwrap();
        starts.push((status["started_ms"].as_u64().unwrap() - submitted) as f64);
        if let Some(began) = marker(bench, &id, "started.json") {
            commands.push(began.saturating_sub(submitted) as f64);
        }
        totals.push((status["finished_ms"].as_u64().unwrap() - submitted) as f64);
    }
    json!({
        "record": "measurement",
        "name": "idle_submit_and_wait",
        "what": "job submit -- true followed by job wait ID on an idle service, one after another; the record times are the service's own",
        "client_submit_plus_wait_ms": summary(&clients),
        "record_submit_to_start_ms": summary(&starts),
        "record_submit_to_command_start_ms": summary(&commands),
        "record_submit_to_finish_ms": summary(&totals),
    })
}

fn create_held(bench: &Bench, count: usize) -> (Vec<u64>, f64) {
    let start = Instant::now();
    let ids: Vec<u64> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..POPULATORS)
            .map(|worker| {
                scope.spawn(move || {
                    let share = count / POPULATORS + usize::from(worker < count % POPULATORS);
                    let mut ids = Vec::new();
                    while ids.len() < share && start.elapsed() < CAP {
                        let output = bench.cli(&["create", "-q", "held", "--", "true"]);
                        assert!(
                            output.status.success(),
                            "{}",
                            String::from_utf8_lossy(&output.stderr)
                        );
                        ids.push(
                            String::from_utf8(output.stdout)
                                .unwrap()
                                .trim()
                                .parse()
                                .unwrap(),
                        );
                    }
                    ids
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect()
    });
    (ids, milliseconds(start.elapsed()))
}

fn clone_held(bench: &Bench, template: u64, count: usize) -> u64 {
    let state = bench.root().join("state");
    let source = state.join("jobs").join(template.to_string());
    let next: u64 = fs::read_to_string(state.join("next-id"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let record: Value =
        serde_json::from_slice(&fs::read(source.join("job.json")).unwrap()).unwrap();
    assert_eq!(record["state"], "Held");
    for id in next..next + count as u64 {
        let directory = state.join("jobs").join(id.to_string());
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::metadata(&source).unwrap().permissions()).unwrap();
        let mut record = record.clone();
        record["id"] = json!(id);
        record["log"] = json!(directory.join("output.log"));
        fs::write(
            directory.join("job.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        for name in ["env.json", "submitted-env.json"] {
            fs::copy(source.join(name), directory.join(name)).unwrap();
        }
        for name in ["job.json", "env.json", "submitted-env.json"] {
            fs::set_permissions(
                directory.join(name),
                fs::metadata(source.join(name)).unwrap().permissions(),
            )
            .unwrap();
        }
    }
    fs::write(state.join("next-id"), (next + count as u64).to_string()).unwrap();
    next + count as u64 - 1
}

fn states(bench: &Bench) -> std::collections::BTreeMap<String, usize> {
    let mut found = std::collections::BTreeMap::new();
    if let Ok(entries) = fs::read_dir(bench.root().join("state/jobs")) {
        for entry in entries.flatten() {
            if let Ok(bytes) = fs::read(entry.path().join("job.json"))
                && let Ok(record) = serde_json::from_slice::<Value>(&bytes)
                && let Some(state) = record["state"].as_str()
            {
                *found.entry(state.to_owned()).or_insert(0) += 1;
            }
        }
    }
    found
}

fn repeated(bench: &Bench, args: &[&str]) -> Value {
    let mut times = Vec::new();
    let mut bytes = 0;
    let mut code = None;
    for _ in 0..5 {
        let (time, output) = bench.timed(args);
        times.push(time);
        bytes = output.stdout.len();
        code = output.status.code();
    }
    json!({"ms": summary(&times), "stdout_bytes": bytes, "exit": code})
}

fn large(backend: &str, size: usize, emit: &mut dyn FnMut(Value)) {
    let mut bench = Bench::start(&format!("held-{size}"), backend);
    let idle = bench.rss_kib();
    bench.ok(&["queue", "create", "held"]);
    let (created, create_wall) = create_held(&bench, THROUGH_CLI);
    let template = *created.iter().max().unwrap();
    bench.stop();
    let last = clone_held(&bench, template, size - created.len()).to_string();
    let validated = bench.cli(&[
        "state",
        "validate",
        "--source",
        bench.root().join("state").to_str().unwrap(),
    ]);
    assert!(
        validated.status.success(),
        "{}",
        String::from_utf8_lossy(&validated.stderr)
    );
    let first_start = milliseconds(bench.begin());
    let state = tree_bytes(&bench.root().join("state"));
    let rss = bench.rss_kib();
    let (create_ms, extra) = bench.timed(&["create", "-q", "held", "--", "true"]);
    let extra = String::from_utf8(extra.stdout).unwrap().trim().to_owned();
    let (cancel_ms, _) = bench.timed(&["cancel", &extra]);
    emit(json!({
        "record": "measurement",
        "name": format!("held_jobs_{size}"),
        "what": "held Jobs in one Queue; the first created with job create -q held -- true by eight parallel clients, the rest cloned offline from one of those records while the service was stopped and accepted by job state validate; then each command five times",
        "held_jobs": size,
        "created_through_cli": created.len(),
        "cli_create_wall_ms": create_wall,
        "cli_creations_per_second": (created.len() as f64 * 1e4 / create_wall).round() / 10.0,
        "cloned_offline": size - created.len(),
        "rss_kib_idle_before": idle,
        "rss_kib": rss,
        "start_until_answer_ms": first_start,
        "state_bytes": state.0,
        "state_allocated_bytes": state.1,
        "state_files": state.2,
        "job_list": repeated(&bench, &["list"]),
        "job_list_json": repeated(&bench, &["list", "--json"]),
        "job_queue_show": repeated(&bench, &["queue", "show", "held"]),
        "job_queue": repeated(&bench, &["queue"]),
        "job_explain": repeated(&bench, &["explain", &last]),
        "job_status": repeated(&bench, &["status", &last, "--json"]),
        "job_create_one_more_ms": create_ms,
        "job_cancel_that_one_ms": cancel_ms,
        "rss_kib_after_commands": bench.rss_kib(),
    }));
    let mut restarts = Vec::new();
    for _ in 0..3 {
        restarts.push(milliseconds(bench.restart()));
    }
    emit(json!({
        "record": "measurement",
        "name": format!("restart_with_{size}_records"),
        "what": "the service process killed, then started again; from spawn until the socket answers Ping, three times",
        "held_jobs": size,
        "job_records": size + 1,
        "start_until_answer_ms": summary(&restarts),
        "rss_kib_after_restart": bench.rss_kib(),
    }));
    let (cancel_ms, cancel_exit, cancel_done) =
        bench.capped(&["queue", "cancel", "--recursive", "held"]);
    let mut removal = json!({
        "record": "measurement",
        "name": format!("removal_of_{size}"),
        "what": "job queue cancel --recursive held, then job queue remove --recursive held, one client call each with the cap; a call that did not return within the cap was abandoned and the store read afterwards",
        "held_jobs": size,
        "queue_cancel_ms": cancel_ms,
        "queue_cancel_exit": cancel_exit,
        "queue_cancel_returned_within_cap": cancel_done,
    });
    if cancel_done && cancel_exit == Some(0) {
        removal["rss_kib_after_cancel"] = json!(bench.rss_kib());
        let (remove_ms, remove_exit, remove_done) =
            bench.capped(&["queue", "remove", "--recursive", "held"]);
        removal["queue_remove_ms"] = json!(remove_ms);
        removal["queue_remove_exit"] = json!(remove_exit);
        removal["queue_remove_returned_within_cap"] = json!(remove_done);
        if remove_done {
            removal["rss_kib_after_remove"] = json!(bench.rss_kib());
            removal["job_list_after_remove"] = repeated(&bench, &["list"]);
            let restart = milliseconds(bench.restart());
            removal["restart_after_remove_ms"] = json!(restart);
            removal["rss_kib_after_remove_and_restart"] = json!(bench.rss_kib());
        }
    }
    bench.stop();
    let state = tree_bytes(&bench.root().join("state"));
    removal["record_states_at_end"] = json!(states(&bench));
    removal["state_bytes_at_end"] = json!(state.0);
    removal["state_files_at_end"] = json!(state.2);
    emit(removal);
}

fn output(bench: &Bench, emit: &mut dyn FnMut(Value)) {
    let script = format!("yes {LINE} | head -c {OUTPUT_BYTES}");
    let start = Instant::now();
    let direct = Command::new("sh")
        .args(["-c", &format!("{script} | cat > /dev/null")])
        .status()
        .unwrap();
    assert!(direct.success());
    emit(json!({
        "record": "measurement",
        "name": "output_baseline_pipe",
        "what": "the same writer through a pipe into cat > /dev/null, without job",
        "bytes": OUTPUT_BYTES,
        "wall_ms": milliseconds(start.elapsed()),
    }));
    for bounded in [false, true] {
        let mut args = vec!["submit"];
        if bounded {
            args.extend_from_slice(&["--output-head", "1M", "--output-tail", "1M"]);
        }
        args.extend_from_slice(&["--", "sh", "-c", &script]);
        let start = Instant::now();
        let id = bench.ok(&args);
        let waited = bench.cli(&["wait", &id, "--timeout", "110s"]);
        let wall = start.elapsed();
        let status = bench.status(&id);
        let directory = bench.root().join("state/jobs").join(&id);
        let disk = tree_bytes(&directory);
        let mut read = Vec::new();
        let mut returned = 0;
        for _ in 0..3 {
            let (time, logs) = bench.timed(&["logs", &id, "--raw"]);
            read.push(time);
            returned = logs.stdout.len();
        }
        let (tail_ms, tail) = bench.timed(&["log", &id, "tail", "50"]);
        emit(json!({
            "record": "measurement",
            "name": if bounded { "output_head_1m_tail_1m" } else { "output_default_retention" },
            "what": "one Job writing 64-byte lines to stdout; job submit then job wait; then job logs ID --raw three times",
            "written_bytes": OUTPUT_BYTES,
            "wait_exit": waited.status.code(),
            "state": status["state"],
            "submit_to_wait_return_ms": milliseconds(wall),
            "record_start_to_finish_ms": status["finished_ms"].as_u64().zip(status["started_ms"].as_u64()).map(|(end, begin)| end - begin),
            "megabytes_per_second": status["finished_ms"].as_u64().zip(status["started_ms"].as_u64()).map(|(end, begin)| ((OUTPUT_BYTES as f64 / 1048576.0) * 1e4 / (end - begin).max(1) as f64).round() / 10.0),
            "job_directory_bytes": disk.0,
            "job_directory_allocated_bytes": disk.1,
            "job_directory_files": disk.2,
            "logs_raw_ms": summary(&read),
            "logs_raw_bytes": returned,
            "log_tail_50_ms": tail_ms,
            "log_tail_50_bytes": tail.stdout.len(),
        }));
    }
}

fn text(command: &mut Command) -> String {
    command
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_default()
}

fn environment(backend: &str) -> Value {
    let read = |path: &str| {
        fs::read_to_string(path)
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let meminfo = read("/proc/meminfo");
    let field = |name: &str| {
        meminfo
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map(|value| value.trim().to_owned())
    };
    let cgroup = read("/proc/self/cgroup")
        .lines()
        .find_map(|line| line.strip_prefix("0::").map(str::to_owned))
        .unwrap_or_default();
    let own = format!("/sys/fs/cgroup{cgroup}");
    let temp = std::env::temp_dir();
    json!({
        "record": "environment",
        "schema_version": 1,
        "backend": backend,
        "profile": "ordinary",
        "kernel": text(Command::new("uname").arg("-srm")),
        "cpu_model": read("/proc/cpuinfo").lines().find(|line| line.starts_with("model name")).and_then(|line| line.split_once(':')).map(|pair| pair.1.trim().to_owned()),
        "cpus_online": std::thread::available_parallelism().map(usize::from).unwrap_or(0),
        "cpus_allowed": read("/proc/self/status").lines().find_map(|line| line.strip_prefix("Cpus_allowed_list:")).map(|value| value.trim().to_owned()),
        "memory_total": field("MemTotal:"),
        "memory_available": field("MemAvailable:"),
        "own_cgroup": cgroup,
        "own_cgroup_memory_max": read(&format!("{own}/memory.max")),
        "own_cgroup_cpu_max": read(&format!("{own}/cpu.max")),
        "temporary_directory": temp,
        "filesystem": text(Command::new("findmnt").args(["-no", "FSTYPE,OPTIONS", "-T"]).arg(&temp)),
        "load_average": read("/proc/loadavg"),
        "unix_ms": realtime_ms(),
        "program": JOB,
        "program_bytes": fs::metadata(JOB).map(|metadata| metadata.len()).unwrap_or(0),
        "source_commit": text(Command::new("git").args(["rev-parse", "HEAD"])),
        "source_changed_files": text(Command::new("git").args(["status", "--porcelain"])).lines().count(),
        "client_environment": "PATH, HOME, LC_ALL, JOB_STATE_DIR, JOB_SESSION only",
        "submissions": SUBMISSIONS,
        "concurrent_submitters": SUBMITTERS,
        "held_sizes": SIZES,
        "populating_clients": POPULATORS,
        "held_created_through_cli": THROUGH_CLI,
        "output_bytes": OUTPUT_BYTES,
        "cap_ms_per_measurement": CAP.as_millis() as u64,
        "quantiles": "nearest rank",
        "unit": "milliseconds unless named otherwise",
        "clock": "client times are monotonic in the driver; record times are the service's realtime milliseconds",
    })
}

fn main() {
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|argument| argument != "--bench")
        .collect();
    let usage = "--output FILE [--backend watch|cgroup] [--only submit|dispatch|idle|large|output]";
    let mut output_path = None;
    let mut backend = "watch".to_owned();
    let mut only = None;
    let mut rest = args.iter();
    while let Some(argument) = rest.next() {
        let value = rest.next().unwrap_or_else(|| panic!("{usage}"));
        match argument.as_str() {
            "--output" => output_path = Some(value.clone()),
            "--backend" => backend = value.clone(),
            "--only" => only = Some(value.clone()),
            _ => panic!("{usage}"),
        }
    }
    assert!(matches!(backend.as_str(), "watch" | "cgroup"), "{usage}");
    assert!(
        only.as_deref()
            .is_none_or(|name| ["submit", "dispatch", "idle", "large", "output"].contains(&name)),
        "{usage}"
    );
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output_path.unwrap_or_else(|| panic!("{usage}")))
        .unwrap();
    let mut emit = |value: Value| {
        println!("{value}");
        writeln!(file, "{value}").unwrap();
        file.sync_data().unwrap();
    };
    emit(environment(&backend));
    let wanted = |name: &str| only.as_deref().is_none_or(|only| only == name);
    if wanted("submit") || wanted("dispatch") {
        let bench = Bench::start("flow", &backend);
        bench.ok(&["queue", "create", "open"]);
        bench.ok(&["queue", "pause", "open"]);
        bench.ok(&["queue", "create", "four"]);
        bench.ok(&["queue", "set", "four", "--max-running", "4"]);
        bench.ok(&["queue", "pause", "four"]);
        let (open, sequential) = submit_many(&bench, "open", 1);
        let (four, concurrent) = submit_many(&bench, "four", SUBMITTERS);
        if wanted("submit") {
            emit(sequential);
            emit(concurrent);
        }
        if wanted("dispatch") {
            emit(dispatch(&bench, "open", &open, None));
            emit(dispatch(&bench, "four", &four, Some(4)));
        }
    }
    if wanted("idle") {
        let bench = Bench::start("idle", &backend);
        emit(idle_round_trip(&bench));
    }
    if wanted("large") {
        for size in SIZES {
            large(&backend, size, &mut emit);
        }
    }
    if wanted("output") {
        let bench = Bench::start("output", &backend);
        output(&bench, &mut emit);
    }
    emit(
        json!({"record": "end", "load_average": fs::read_to_string("/proc/loadavg").unwrap_or_default().trim(), "unix_ms": realtime_ms()}),
    );
}
