use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

struct Daemon {
    child: Child,
    base: PathBuf,
    state: PathBuf,
}

fn config(metrics: Option<&str>) -> String {
    let mut text = "schema_version = 1\nprofile = 'ordinary'\n".to_owned();
    if let Some(listen) = metrics {
        text.push_str(&format!("\n[metrics]\nlisten = \"{listen}\"\n"));
    }
    text
}

fn spawn(base: &Path, state: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_job"))
        .arg("daemon")
        .env("JOB_CGROUP_ROOT", base.join("absent-cgroup"))
        .env("JOB_CONFIG", base.join("config.toml"))
        .env("JOB_STATE_DIR", state)
        .env("LC_ALL", "C")
        .env_remove("LANG")
        .env_remove("LC_MESSAGES")
        .env_remove("JOB_CLI_COMPAT")
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            std::fs::File::create(base.join("daemon.err")).unwrap(),
        ))
        .spawn()
        .unwrap()
}

fn prepared(name: &str, metrics: Option<&str>) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("job-metrics-{}-{name}", std::process::id()));
    let state = base.join("state");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(base.join("config.toml"), config(metrics)).unwrap();
    (base, state)
}

impl Daemon {
    fn start(name: &str, metrics: Option<&str>) -> Daemon {
        let (base, state) = prepared(name, metrics);
        let child = spawn(&base, &state);
        let mut daemon = Daemon { child, base, state };
        daemon.ready();
        daemon
    }

    fn ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "daemon exited during startup: {}",
                self.stderr()
            );
            if self.job(&["host", "--json"]).status.success() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("daemon did not become ready");
    }

    fn stderr(&self) -> String {
        std::fs::read_to_string(self.base.join("daemon.err")).unwrap_or_default()
    }

    fn job(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_job"))
            .args(args)
            .env("LC_ALL", "C")
            .env_remove("LANG")
            .env_remove("LC_MESSAGES")
            .env_remove("JOB_CLI_COMPAT")
            .env("JOB_STATE_DIR", &self.state)
            .env("JOB_CONFIG", self.base.join("config.toml"))
            .env("JOB_SESSION", "metrics")
            .current_dir(&self.base)
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
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn metrics(&self) -> Exposition {
        parse(&self.ok(&["metrics"]))
    }

    fn address(&self) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let text = self.stderr();
            if let Some(rest) = text.split("metrics at http://").nth(1) {
                return rest.split("/metrics").next().unwrap().to_owned();
            }
            assert!(Instant::now() < deadline, "no metrics address in {text}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

#[derive(Default)]
struct Exposition {
    types: BTreeMap<String, String>,
    samples: BTreeMap<String, f64>,
}

impl Exposition {
    fn value(&self, series: &str) -> Option<f64> {
        self.samples.get(series).copied()
    }

    fn get(&self, series: &str) -> f64 {
        self.value(series)
            .unwrap_or_else(|| panic!("no series {series} in {:?}", self.samples.keys()))
    }
}

fn valid_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_' || first == ':')
        && characters.all(|rest| rest.is_ascii_alphanumeric() || rest == '_' || rest == ':')
}

fn labels(text: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let (name, after) = rest.split_once("=\"").expect("label without =\"");
        assert!(valid_name(name), "label name {name}");
        let mut value = String::new();
        let mut characters = after.char_indices();
        let end = loop {
            let (at, character) = characters.next().expect("unterminated label value");
            match character {
                '\\' => match characters.next().expect("dangling escape").1 {
                    '\\' => value.push('\\'),
                    '"' => value.push('"'),
                    'n' => value.push('\n'),
                    other => panic!("unknown escape \\{other}"),
                },
                '"' => break at,
                other => value.push(other),
            }
        };
        found.push((name.to_owned(), value));
        rest = &after[end + 1..];
        rest = rest.strip_prefix(',').unwrap_or(rest);
    }
    found
}

fn family_of<'a>(name: &'a str, types: &BTreeMap<String, String>) -> &'a str {
    for suffix in ["_bucket", "_sum", "_count"] {
        if let Some(base) = name.strip_suffix(suffix)
            && types.get(base).map(String::as_str) == Some("histogram")
        {
            return base;
        }
    }
    name
}

fn parse(text: &str) -> Exposition {
    let mut exposition = Exposition::default();
    let mut helped = BTreeSet::new();
    let mut current: Option<String> = None;
    let mut finished = BTreeSet::new();
    assert!(text.ends_with('\n'), "the exposition ends with a newline");
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# HELP ") {
            let (name, help) = rest.split_once(' ').unwrap();
            assert!(valid_name(name), "{line}");
            assert!(!help.is_empty(), "{line}");
            assert!(helped.insert(name.to_owned()), "two HELP lines for {name}");
            continue;
        }
        if let Some(rest) = line.strip_prefix("# TYPE ") {
            let (name, kind) = rest.split_once(' ').unwrap();
            assert!(valid_name(name), "{line}");
            assert!(["gauge", "counter", "histogram"].contains(&kind), "{line}");
            assert!(
                exposition
                    .types
                    .insert(name.to_owned(), kind.to_owned())
                    .is_none(),
                "two TYPE lines for {name}"
            );
            if let Some(previous) = current.replace(name.to_owned()) {
                finished.insert(previous);
            }
            continue;
        }
        assert!(!line.starts_with('#'), "{line}");
        let (series, value) = line.rsplit_once(' ').expect(line);
        let (name, label_text) = match series.split_once('{') {
            Some((name, rest)) => (name, rest.strip_suffix('}').expect(line)),
            None => (series, ""),
        };
        assert!(valid_name(name), "{line}");
        let family = family_of(name, &exposition.types);
        assert_eq!(
            current.as_deref(),
            Some(family),
            "sample outside its family: {line}"
        );
        assert!(!finished.contains(family), "{line}");
        let found = labels(label_text);
        assert!(
            found
                .iter()
                .all(|(label, _)| label != "job" && label != "id"),
            "{line}"
        );
        let parsed: f64 = match value {
            "+Inf" => f64::INFINITY,
            "-Inf" => f64::NEG_INFINITY,
            other => other.parse().unwrap_or_else(|_| panic!("value of {line}")),
        };
        if exposition.types[family] == "counter" {
            assert!(name.ends_with("_total"), "{line}");
            assert!(parsed >= 0.0, "{line}");
        }
        assert!(
            exposition
                .samples
                .insert(series.to_owned(), parsed)
                .is_none(),
            "two samples of {series}"
        );
    }
    assert_eq!(
        helped,
        exposition.types.keys().cloned().collect::<BTreeSet<_>>(),
        "every family has HELP and TYPE"
    );
    for (family, kind) in &exposition.types {
        if kind != "histogram" {
            continue;
        }
        let mut cumulative: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
        for (series, value) in &exposition.samples {
            let Some(rest) = series.strip_prefix(&format!("{family}_bucket{{")) else {
                continue;
            };
            let found = labels(rest.strip_suffix('}').unwrap());
            let le = found
                .iter()
                .find(|(name, _)| name == "le")
                .unwrap()
                .1
                .clone();
            let others: Vec<String> = found
                .iter()
                .filter(|(name, _)| name != "le")
                .map(|(name, value)| format!("{name}=\"{value}\""))
                .collect();
            let bound = if le == "+Inf" {
                f64::INFINITY
            } else {
                le.parse().unwrap()
            };
            cumulative
                .entry(others.join(","))
                .or_default()
                .push((bound, *value));
        }
        for (labels, mut buckets) in cumulative {
            buckets.sort_by(|a, b| a.0.total_cmp(&b.0));
            assert!(
                buckets.windows(2).all(|pair| pair[0].1 <= pair[1].1),
                "{family}{{{labels}}} is not cumulative"
            );
            let last = buckets.last().unwrap();
            assert!(
                last.0.is_infinite(),
                "{family}{{{labels}}} has no +Inf bucket"
            );
            assert_eq!(
                Some(last.1),
                exposition
                    .samples
                    .get(&format!("{family}_count{{{labels}}}"))
                    .copied(),
                "{family}{{{labels}}}"
            );
            assert!(
                exposition
                    .samples
                    .contains_key(&format!("{family}_sum{{{labels}}}"))
            );
        }
    }
    exposition
}

fn states(daemon: &Daemon) -> BTreeMap<String, f64> {
    let listing: Value =
        serde_json::from_str(&daemon.ok(&["list", "--all", "--format", "json"])).unwrap();
    let mut counted = BTreeMap::new();
    for row in listing["data"]["jobs"].as_array().unwrap() {
        let state = row["job"]["state"].as_str().unwrap().to_lowercase();
        *counted.entry(state).or_insert(0.0) += 1.0;
    }
    counted
}

fn request(address: &str, bytes: &[u8]) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    stream.write_all(bytes).unwrap();
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).unwrap();
    String::from_utf8(answer).unwrap()
}

fn body(answer: &str) -> &str {
    answer.split_once("\r\n\r\n").unwrap().1
}

fn listening(pid: u32) -> bool {
    let sockets: BTreeSet<String> = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .filter_map(|entry| std::fs::read_link(entry.ok()?.path()).ok())
        .filter_map(|target| {
            target
                .to_str()?
                .strip_prefix("socket:[")?
                .strip_suffix(']')
                .map(str::to_owned)
        })
        .collect();
    ["/proc/net/tcp", "/proc/net/tcp6"].iter().any(|table| {
        std::fs::read_to_string(table)
            .unwrap_or_default()
            .lines()
            .skip(1)
            .any(|line| {
                let fields: Vec<&str> = line.split_whitespace().collect();
                fields.get(3) == Some(&"0A")
                    && fields.get(9).is_some_and(|inode| sockets.contains(*inode))
            })
    })
}

#[test]
fn metrics_command_agrees_with_list_and_host() {
    let daemon = Daemon::start("agree", None);
    daemon.ok(&["queue", "create", "build", "--parallel", "1"]);
    let quick = daemon.ok(&["submit", "--", "true"]).trim().to_owned();
    daemon.ok(&["wait", &quick, "--timeout", "20s"]);
    let held = daemon.ok(&["create", "--", "true"]).trim().to_owned();
    let long = daemon
        .ok(&["submit", "--queue", "build", "--", "sleep", "20"])
        .trim()
        .to_owned();
    let waiting = daemon
        .ok(&["submit", "--queue", "build", "--", "true"])
        .trim()
        .to_owned();
    let deadline = Instant::now() + Duration::from_secs(10);
    while states(&daemon).get("running") != Some(&1.0) {
        assert!(Instant::now() < deadline, "the long Job did not start");
        std::thread::sleep(Duration::from_millis(50));
    }

    let seen = daemon.metrics();
    let counted = states(&daemon);
    for state in [
        "held",
        "queued",
        "starting",
        "running",
        "suspended",
        "stopping",
        "succeeded",
        "failed",
        "cancelled",
        "lost",
    ] {
        assert_eq!(
            seen.get(&format!("job_jobs{{state=\"{state}\"}}")),
            counted.get(state).copied().unwrap_or(0.0),
            "{state}"
        );
    }
    let host: Value = serde_json::from_str(&daemon.ok(&["host", "--json"])).unwrap();
    assert_eq!(
        seen.get("job_pool_cores") * 1000.0,
        host["pool"]["cores_milli"].as_f64().unwrap()
    );
    assert_eq!(
        seen.get("job_pool_memory_bytes"),
        host["pool"]["memory"].as_f64().unwrap()
    );
    assert_eq!(
        seen.get("job_pool_processes"),
        host["pool"]["pids"].as_f64().unwrap()
    );
    assert_eq!(
        seen.get("job_host_memory_bytes"),
        host["memory_total"].as_f64().unwrap()
    );
    assert_eq!(seen.get("job_up"), 1.0);
    assert_eq!(seen.types["job_service_info"], "gauge");
    assert!(seen.samples.keys().any(|series| {
        series.starts_with("job_service_info{")
            && series.contains(&format!(
                "version=\"{}\"",
                host["version"].as_str().unwrap()
            ))
            && series.contains("backend=\"watch\"")
    }));
    assert_eq!(seen.get("job_journal_writable{journal=\"events\"}"), 1.0);
    assert!(seen.get("job_service_start_time_seconds") > 1.0e9);
    assert!(seen.get("job_output_budget_bytes") > 0.0);
    assert_eq!(
        seen.value("job_use_memory_bytes"),
        None,
        "no cgroup figures without a cgroup"
    );

    let build = "kind=\"queue\",path=\"build\"";
    assert_eq!(
        seen.get(&format!("job_object_jobs{{{build},state=\"running\"}}")),
        1.0
    );
    assert_eq!(
        seen.get(&format!("job_object_jobs{{{build},state=\"queued\"}}")),
        1.0
    );
    assert_eq!(
        seen.get(&format!("job_object_running_limit{{{build}}}")),
        1.0
    );
    assert!(seen.get(&format!("job_object_oldest_queued_age_seconds{{{build}}}")) >= 0.0);
    let default = "kind=\"queue\",path=\"default\"";
    assert_eq!(
        seen.get(&format!("job_object_jobs{{{default},state=\"held\"}}")),
        1.0
    );
    assert_eq!(
        seen.value(&format!("job_object_running_limit{{{default}}}")),
        None
    );
    assert_eq!(
        seen.get("job_object_jobs{kind=\"group\",path=\"/\",state=\"queued\"}"),
        1.0
    );

    assert_eq!(
        seen.get("job_wait_duration_seconds_count{queue=\"default\"}"),
        1.0
    );
    assert_eq!(
        seen.get("job_wait_duration_seconds_count{queue=\"build\"}"),
        1.0
    );
    assert_eq!(
        seen.get("job_run_duration_seconds_count{queue=\"default\"}"),
        1.0
    );
    assert_eq!(
        seen.value("job_run_duration_seconds_count{queue=\"build\"}"),
        None
    );
    assert_eq!(
        seen.get("job_ended_total{queue=\"default\",state=\"succeeded\"}"),
        1.0
    );

    daemon.ok(&["cancel", &waiting]);
    daemon.ok(&["cancel", &long]);
    let _ = daemon.job(&["wait", &long, "--timeout", "20s"]);
    let _ = daemon.job(&["wait", &waiting, "--timeout", "20s"]);
    let after = daemon.metrics();
    assert_eq!(
        after.get("job_ended_total{queue=\"build\",state=\"cancelled\"}"),
        2.0
    );
    assert_eq!(
        after.get("job_run_duration_seconds_count{queue=\"build\"}"),
        1.0
    );
    assert_eq!(
        after.get("job_wait_duration_seconds_count{queue=\"build\"}"),
        1.0
    );
    assert_eq!(after.get("job_jobs{state=\"cancelled\"}"), 2.0);
    assert_eq!(after.get("job_jobs{state=\"held\"}"), 1.0);
    daemon.ok(&["cancel", &held]);
}

#[test]
fn metrics_endpoint_serves_only_get_metrics() {
    let daemon = Daemon::start("endpoint", Some("127.0.0.1:0"));
    let address = daemon.address();
    assert!(listening(daemon.pid()));

    let answer = request(
        &address,
        b"GET /metrics HTTP/1.1\r\nHost: test\r\nAccept: text/plain\r\n\r\n",
    );
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(
        answer.contains("\r\nContent-Type: text/plain; version=0.0.4; charset=utf-8\r\n"),
        "{answer}"
    );
    let text = body(&answer);
    assert!(answer.contains(&format!("\r\nContent-Length: {}\r\n", text.len())));
    let served = parse(text);
    assert_eq!(served.get("job_up"), 1.0);
    let local = daemon.metrics();
    assert_eq!(
        served.types, local.types,
        "the endpoint and job metrics carry the same families"
    );

    let query = request(&address, b"GET /metrics?x=1 HTTP/1.0\r\n\r\n");
    assert!(query.starts_with("HTTP/1.1 200 OK\r\n"), "{query}");
    for refused in [
        &b"GET / HTTP/1.1\r\n\r\n"[..],
        b"GET /metrics/x HTTP/1.1\r\n\r\n",
        b"POST /metrics HTTP/1.1\r\nContent-Length: 0\r\n\r\n",
        b"HEAD /metrics HTTP/1.1\r\n\r\n",
    ] {
        let answer = request(&address, refused);
        assert!(answer.starts_with("HTTP/1.1 404 Not Found\r\n"), "{answer}");
    }
    let malformed = request(&address, b"hello\r\n\r\n");
    assert!(
        malformed.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "{malformed}"
    );
    let oversized = request(&address, &[b'a'; 8192]);
    assert!(
        oversized.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "{oversized}"
    );

    let started = Instant::now();
    let silent = request(&address, b"GET /metrics");
    assert_eq!(silent, "", "an unfinished request gets no answer");
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_secs(4) && waited < Duration::from_secs(15),
        "{waited:?}"
    );

    let idle: Vec<TcpStream> = (0..8)
        .map(|_| TcpStream::connect(&address).unwrap())
        .collect();
    let mut ninth = TcpStream::connect(&address).unwrap();
    ninth
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut nothing = Vec::new();
    assert_eq!(
        ninth.read_to_end(&mut nothing).unwrap(),
        0,
        "a ninth connection is closed at once"
    );
    drop(idle);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let answer = request(&address, b"GET /metrics HTTP/1.1\r\n\r\n");
        if answer.starts_with("HTTP/1.1 200 OK\r\n") {
            break;
        }
        assert!(Instant::now() < deadline, "the slots were not given back");
        std::thread::sleep(Duration::from_millis(50));
    }

    let shown: Value = serde_json::from_str(&daemon.ok(&["config", "show", "--json"])).unwrap();
    assert_eq!(
        shown["config"]["metrics"]["listen"], "127.0.0.1:0",
        "{shown}"
    );
}

#[test]
fn metrics_listener_is_absent_unless_configured() {
    let daemon = Daemon::start("absent", None);
    daemon.ok(&["metrics"]);
    assert!(
        !listening(daemon.pid()),
        "the service listens on TCP without [metrics]"
    );
    assert!(
        !daemon.stderr().contains("metrics at"),
        "{}",
        daemon.stderr()
    );
}

#[test]
fn metrics_configuration_is_checked() {
    let (base, state) = prepared("checked", None);
    let daemon = Daemon {
        child: Command::new("true").spawn().unwrap(),
        base: base.clone(),
        state,
    };
    for (listen, accepted) in [
        ("127.0.0.1:9877", true),
        ("[::1]:9877", true),
        ("0.0.0.0:9877", true),
        ("localhost:9877", false),
        ("127.0.0.1", false),
        ("127.0.0.1:99999", false),
    ] {
        let file = base.join("candidate.toml");
        std::fs::write(&file, config(Some(listen))).unwrap();
        let output = daemon.job(&["config", "check", file.to_str().unwrap()]);
        assert_eq!(output.status.success(), accepted, "{listen}: {output:?}");
        if !accepted {
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("metrics listen must be"),
                "{output:?}"
            );
        }
    }
    let file = base.join("unknown.toml");
    std::fs::write(&file, "schema_version = 1\n[metrics]\nport = 9877\n").unwrap();
    assert!(
        !daemon
            .job(&["config", "check", file.to_str().unwrap()])
            .status
            .success()
    );
}

#[test]
fn metrics_address_in_use_stops_the_start() {
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = taken.local_addr().unwrap().to_string();
    let (base, state) = prepared("taken", Some(&address));
    let mut child = spawn(&base, &state);
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the service started on an address in use");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(!status.success());
    let stderr = std::fs::read_to_string(base.join("daemon.err")).unwrap();
    assert!(
        stderr.contains(&format!("cannot listen for metrics on {address}")),
        "{stderr}"
    );
    assert!(!state.join("daemon.sock").exists());
    drop(taken);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn metrics_reload_refuses_a_new_listener() {
    let daemon = Daemon::start("reload", None);
    std::fs::write(daemon.base.join("config.toml"), config(Some("127.0.0.1:0"))).unwrap();
    let output = daemon.job(&["config", "reload"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("changing the metrics listener requires a daemon restart"),
        "{output:?}"
    );
    assert!(!listening(daemon.pid()));
}

#[test]
fn metrics_without_a_service_say_it_is_down() {
    let (base, state) = prepared("down", None);
    std::fs::create_dir_all(&state).unwrap();
    let daemon = Daemon {
        child: Command::new("true").spawn().unwrap(),
        base,
        state,
    };
    let output = daemon.job(&["metrics"]);
    assert_eq!(output.status.code(), Some(125));
    let text = String::from_utf8(output.stdout).unwrap();
    let down = parse(&text);
    assert_eq!(down.get("job_up"), 0.0);
    assert_eq!(down.samples.len(), 1);
    assert!(!output.stderr.is_empty());

    let extra = daemon.job(&["metrics", "now"]);
    assert_eq!(extra.status.code(), Some(125));
    assert!(
        String::from_utf8_lossy(&extra.stderr).contains("job metrics takes no `now`"),
        "{extra:?}"
    );
    assert!(extra.stdout.is_empty());
    let help = daemon.job(&["metrics", "--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("Prometheus text format"));
}
