use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const HELLO: &str =
    "{\"hello\":{\"versions\":{\"min\":1,\"max\":1},\"client\":\"subscription-test 1\"}}";
const ENDED: [&str; 4] = ["succeeded", "failed", "cancelled", "lost"];
const SEQUENCED: [&str; 5] = ["job", "job_removed", "object", "object_removed", "health"];

struct Daemon {
    child: Child,
    base: PathBuf,
    state: PathBuf,
    work: PathBuf,
    indexed: Option<usize>,
}

impl Daemon {
    fn start(name: &str) -> Daemon {
        Self::bounded(name, None)
    }

    fn bounded(name: &str, indexed: Option<usize>) -> Daemon {
        let base =
            std::env::temp_dir().join(format!("job-subscription-{}-{name}", std::process::id()));
        let state = base.join("state");
        let work = base.join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(
            base.join("config.toml"),
            "schema_version = 1\nprofile = 'ordinary'\n",
        )
        .unwrap();
        let child = Self::spawn(&base, indexed);
        let mut daemon = Daemon {
            child,
            base,
            state,
            work,
            indexed,
        };
        daemon.ready();
        daemon
    }

    fn spawn(base: &std::path::Path, indexed: Option<usize>) -> Child {
        let mut command = Command::new(env!("CARGO_BIN_EXE_job"));
        command
            .arg("daemon")
            .env("JOB_CGROUP_ROOT", base.join("absent-cgroup"))
            .env("JOB_CONFIG", base.join("config.toml"))
            .env("JOB_STATE_DIR", base.join("state"))
            .env("LC_ALL", "C")
            .env_remove("LANG")
            .env_remove("LC_MESSAGES")
            .env_remove("JOB_CLI_COMPAT")
            .env_remove("JOB_CLIENT_INDEX_ENDED")
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                std::fs::File::create(base.join("daemon.err")).unwrap(),
            ));
        if let Some(indexed) = indexed {
            command.env("JOB_CLIENT_INDEX_ENDED", indexed.to_string());
        }
        command.spawn().unwrap()
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn restart(&mut self) {
        self.stop();
        self.child = Self::spawn(&self.base, self.indexed);
        self.ready();
    }

    fn stream(&self) -> Option<UnixStream> {
        let stream = UnixStream::connect(self.state.join("daemon.sock")).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        Some(stream)
    }

    fn ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(600);
        while Instant::now() < deadline {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "daemon exited during startup: {}",
                std::fs::read_to_string(self.base.join("daemon.err")).unwrap_or_default()
            );
            if let Some(mut stream) = self.stream() {
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

    fn job(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_job"))
            .args(args)
            .env("LC_ALL", "C")
            .env_remove("LANG")
            .env_remove("LC_MESSAGES")
            .env_remove("JOB_CLI_COMPAT")
            .env("JOB_STATE_DIR", &self.state)
            .env("JOB_SESSION", "subscription")
            .current_dir(&self.work)
            .output()
            .unwrap()
    }

    fn work(&self) -> String {
        self.work.display().to_string()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some((mut client, _)) = self.stream().map(|_| Client::greet(self)) {
            let ids = client.ok("jobs", json!({"ids_only": true, "filter": {"states": ["held", "queued", "starting", "running", "suspended", "stopping"]}}));
            for id in ids["ids"].as_array().into_iter().flatten() {
                let _ = self.job(&["cancel", &id.to_string()]);
            }
        }
        self.stop();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next: u64,
}

impl Client {
    fn greet(daemon: &Daemon) -> (Client, Value) {
        let writer = daemon.stream().unwrap();
        let mut client = Client {
            reader: BufReader::new(writer.try_clone().unwrap()),
            writer,
            next: 1,
        };
        client.send(HELLO);
        let greeting = client.line().expect("no greeting");
        (client, greeting)
    }

    fn send(&mut self, line: &str) {
        self.writer.write_all(line.as_bytes()).unwrap();
        self.writer.write_all(b"\n").unwrap();
    }

    fn line(&mut self) -> Option<Value> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => None,
            Ok(_) => {
                Some(serde_json::from_str(&line).unwrap_or_else(|error| panic!("{error}: {line}")))
            }
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => None,
            Err(error) => panic!("{error}"),
        }
    }

    fn request(&mut self, op: &str, args: Value) -> u64 {
        let id = self.next;
        self.next += 1;
        self.send(&json!({"id": id, "op": op, "args": args}).to_string());
        id
    }

    fn ask(&mut self, op: &str, args: Value) -> Value {
        let id = self.request(op, args);
        let answer = self.line().expect("no answer");
        assert_eq!(answer["re"], id, "{answer}");
        answer
    }

    fn ok(&mut self, op: &str, args: Value) -> Value {
        let answer = self.ask(op, args);
        assert!(answer.get("error").is_none(), "{answer}");
        answer["ok"].clone()
    }

    fn refused(&mut self, op: &str, args: Value) -> String {
        let answer = self.ask(op, args.clone());
        let text = answer["error"]["message"].as_str().unwrap_or_default();
        assert!(!text.is_empty(), "{op} {args}: {answer}");
        text.to_owned()
    }

    fn command(&mut self, daemon: &Daemon, words: &[&str]) -> Value {
        let answer = self.ok(
            "command",
            json!({"words": words, "cwd": daemon.work(), "session": "subscription"}),
        );
        assert_eq!(answer["exit_status"], 0, "{words:?}: {answer}");
        answer
    }

    fn submit(&mut self, daemon: &Daemon, options: &[&str], program: &[&str]) -> u64 {
        let mut words = vec!["submit"];
        words.extend_from_slice(options);
        words.push("--");
        words.extend_from_slice(program);
        self.command(daemon, &words)["data"]["id"].as_u64().unwrap()
    }

    fn state(&mut self, id: u64) -> String {
        self.ok("job", json!({"id": id}))["row"]["state"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    fn until(&mut self, id: u64, wanted: &[&str]) -> String {
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            let state = self.state(id);
            if wanted.contains(&state.as_str()) {
                return state;
            }
            assert!(Instant::now() < deadline, "Job {id} stayed {state}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

fn sequenced(line: &Value) -> bool {
    line["push"]
        .as_str()
        .is_some_and(|kind| SEQUENCED.contains(&kind))
}

impl Client {
    fn snapshot(&mut self, args: Value) -> Value {
        let answer = self.ok("subscribe", args);
        assert!(answer["snapshot"].is_object(), "{answer}");
        answer["snapshot"].clone()
    }

    fn pushed(&mut self) -> Value {
        let line = self.line().expect("the subscription ended");
        assert!(line.get("push").is_some(), "{line}");
        line
    }

    fn changes_until(&mut self, done: impl Fn(&Value) -> bool) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut seen = Vec::new();
        loop {
            assert!(Instant::now() < deadline, "{seen:?}");
            let line = self.pushed();
            if !sequenced(&line) {
                continue;
            }
            let last = done(&line);
            seen.push(line);
            if last {
                return seen;
            }
        }
    }

    fn changes_to(&mut self, seq: u64) -> Vec<Value> {
        self.changes_until(|line| line["seq"].as_u64().unwrap() >= seq)
    }
}

fn unbroken(after: u64, lines: &[Value]) {
    for (number, line) in lines.iter().enumerate() {
        assert_eq!(line["seq"], after + 1 + number as u64, "{line}");
        assert!(line["at_ms"].as_u64().is_some(), "{line}");
    }
}

fn last_seq(client: &mut Client) -> u64 {
    client.ok("totals", json!({}))["seq"].as_u64().unwrap()
}

#[test]
fn subscription_snapshot_then_job_changes_without_a_hole_for_two_subscribers() {
    let daemon = Daemon::start("jobs");
    let (mut actor, greeting) = Client::greet(&daemon);
    let capabilities = greeting["hello"]["capabilities"].as_array().unwrap();
    assert_eq!(capabilities[0], "subscribe");
    assert_eq!(capabilities[1], "interest");
    assert_eq!(actor.ok("totals", json!({}))["seq"], 0);
    let before = actor.submit(&daemon, &[], &["true"]);
    actor.until(before, &ENDED);
    let (mut first, _) = Client::greet(&daemon);
    let (mut second, _) = Client::greet(&daemon);
    let snapshot = first.snapshot(json!({"rows": {"order": "id", "limit": 50}}));
    let other = second.snapshot(json!({}));
    let seq = snapshot["seq"].as_u64().unwrap();
    assert_eq!(seq, 0);
    assert_eq!(other["seq"], seq);
    assert_eq!(snapshot["started_ms"], greeting["hello"]["started_ms"]);
    assert!(snapshot["now_ms"].as_u64().is_some());
    assert_eq!(snapshot["interest"], 0);
    assert_eq!(snapshot["totals"]["counts"]["succeeded"], 1);
    assert!(
        snapshot["totals"]["capacity"]["pool"]["cores_milli"]
            .as_u64()
            .is_some()
    );
    assert!(snapshot["totals"].get("seq").is_none());
    let health = &snapshot["health"];
    assert_eq!(health["started_ms"], snapshot["started_ms"]);
    assert_eq!(health["backend"], "watch");
    assert_eq!(health["enforcement"], "monitoring_only");
    assert_eq!(health["unreadable_records"], json!([]));
    for field in [
        "state_schema",
        "cgroup_required",
        "supervisors_adopted",
        "last_recovery_ms",
        "recovery_changed_records",
        "audit_writable",
        "events_writable",
        "starter_failures",
        "cancellations_failing",
        "cancellation_retries",
    ] {
        assert!(!health[field].is_null(), "{field}: {health}");
    }
    assert_eq!(health.as_object().unwrap().len(), 14);
    let nodes = snapshot["tree"]["nodes"].as_array().unwrap();
    assert_eq!(nodes[0]["path"], "");
    assert!(nodes[0]["depth"].is_object() && nodes[0]["use"].is_object());
    let page = &snapshot["page"];
    assert_eq!(page["order"], "id");
    assert_eq!(page["matching"]["total"], 1);
    assert_eq!(page["rows"][0]["id"], before);
    assert!(page.get("seq").is_none() && page.get("now_ms").is_none());
    assert_eq!(other["page"]["order"], "activity");

    let quick = actor.submit(&daemon, &["--label", "kind=quick"], &["true"]);
    let slow = actor.submit(&daemon, &[], &["sleep", "40"]);
    let held = actor.command(&daemon, &["create", "--", "true"])["data"]["id"]
        .as_u64()
        .unwrap();
    actor.until(quick, &ENDED);
    actor.until(slow, &["running"]);
    actor.command(&daemon, &["cancel", &slow.to_string()]);
    actor.until(slow, &ENDED);
    actor.command(&daemon, &["remove", &quick.to_string()]);
    let lines = first.changes_until(|line| line["push"] == "job_removed");
    unbroken(seq, &lines);
    let end = lines.last().unwrap()["seq"].as_u64().unwrap();
    assert_eq!(lines.last().unwrap()["id"], quick);
    assert_eq!(second.changes_to(end), lines);
    assert_eq!(last_seq(&mut actor), end);
    let mut states: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    let mut revisions: BTreeMap<u64, u64> = BTreeMap::new();
    for line in lines.iter().filter(|line| line["push"] == "job") {
        let row = &line["row"];
        let id = row["id"].as_u64().unwrap();
        let state = row["state"].as_str().unwrap().to_owned();
        let revision = row["revision"].as_u64().unwrap();
        assert!(
            revisions
                .insert(id, revision)
                .is_none_or(|known| known < revision),
            "{line}"
        );
        if ["running", "suspended", "stopping"].contains(&state.as_str()) {
            assert!(row["started_ms"].as_u64().is_some(), "{line}");
        }
        assert_eq!(row.as_object().unwrap().len(), 37, "{row}");
        let seen = states.entry(id).or_default();
        if seen.last() != Some(&state) {
            seen.push(state);
        }
    }
    let order = [
        "queued",
        "starting",
        "running",
        "stopping",
        "succeeded",
        "cancelled",
    ];
    for id in [quick, slow] {
        let places: Vec<usize> = states[&id]
            .iter()
            .map(|state| order.iter().position(|known| known == state).unwrap())
            .collect();
        assert!(
            places.windows(2).all(|pair| pair[0] < pair[1]),
            "{:?}",
            states[&id]
        );
        assert_eq!(&states[&id][..2], ["queued", "starting"]);
    }
    assert_eq!(states[&quick].last().unwrap(), "succeeded");
    assert!(
        states[&slow].contains(&"running".to_owned()),
        "{:?}",
        states[&slow]
    );
    assert_eq!(states[&slow].last().unwrap(), "cancelled");
    assert_eq!(states[&held], ["held"]);
    assert!(!states.contains_key(&before));
    for id in [slow, held] {
        let answer = actor.ok("job", json!({"id": id}));
        assert_eq!(answer["seq"], end);
        let newest = lines
            .iter()
            .rev()
            .find(|line| line["push"] == "job" && line["row"]["id"] == id)
            .unwrap();
        let mut row = answer["row"].clone();
        row["predicted_start_ms"] = Value::Null;
        row["waited_for"] = newest["row"]["waited_for"].clone();
        assert_eq!(row, newest["row"], "{id}");
    }
    let listed = actor.ok("jobs", json!({"order": "id"}));
    assert_eq!(listed["seq"], end);
    assert_eq!(actor.ok("tree", json!({}))["seq"], end);
    actor.command(&daemon, &["release", &held.to_string()]);
    actor.until(held, &ENDED);
    let more = first.changes_until(|line| {
        line["row"]["id"] == held && ENDED.contains(&line["row"]["state"].as_str().unwrap_or(""))
    });
    unbroken(end, &more);
    let fresh = first.ok("jobs", json!({"order": "id"}));
    assert!(fresh["seq"].as_u64().unwrap() >= more.last().unwrap()["seq"].as_u64().unwrap());
}

#[test]
fn subscription_sends_object_changes_parents_first() {
    let daemon = Daemon::start("objects");
    let (mut actor, _) = Client::greet(&daemon);
    let (mut watcher, _) = Client::greet(&daemon);
    let snapshot = watcher.snapshot(json!({"rows": {"limit": 0}}));
    let mut seq = snapshot["seq"].as_u64().unwrap();
    let mut step = |actor: &mut Client, watcher: &mut Client, words: &[&str], count: usize| {
        actor.command(&daemon, words);
        let lines = watcher.changes_to(seq + count as u64);
        unbroken(seq, &lines);
        assert_eq!(lines.len(), count, "{words:?}: {lines:?}");
        seq += count as u64;
        assert_eq!(last_seq(actor), seq, "{words:?}");
        lines
    };
    let created = step(&mut actor, &mut watcher, &["group", "create", "builds"], 1);
    assert_eq!(created[0]["push"], "object");
    let node = &created[0]["node"];
    let group = node["object"]["id"].as_u64().unwrap();
    assert_eq!(node["path"], "builds");
    assert_eq!(node["object"]["kind"], "group");
    assert!(node.get("depth").is_none() && node.get("use").is_none());
    for member in ["effective", "aggregate_domains", "paused_by", "closed_by"] {
        assert!(!node[member].is_null(), "{member}");
    }
    let queue = step(
        &mut actor,
        &mut watcher,
        &["queue", "create", "builds/linux"],
        1,
    )[0]["node"]["object"]["id"]
        .as_u64()
        .unwrap();
    let set = step(
        &mut actor,
        &mut watcher,
        &["group", "set", "builds", "--max-running", "3"],
        2,
    );
    assert_eq!(set[0]["node"]["object"]["id"], group);
    assert_eq!(set[0]["node"]["object"]["config"]["max_running"], 3);
    assert_eq!(set[1]["node"]["object"]["id"], queue);
    assert_eq!(
        set[1]["node"]["effective"]["max_running"],
        json!({"value": 3, "source": {"from": "object", "id": group, "path": "builds"}})
    );
    let paused = step(
        &mut actor,
        &mut watcher,
        &["queue", "pause", "builds/linux"],
        1,
    );
    assert_eq!(paused[0]["node"]["object"]["paused"], true);
    assert_eq!(paused[0]["node"]["paused_by"], json!(["builds/linux"]));
    let renamed = step(
        &mut actor,
        &mut watcher,
        &["group", "rename", "builds", "made"],
        2,
    );
    assert_eq!(renamed[0]["node"]["object"]["id"], group);
    assert_eq!(renamed[0]["node"]["path"], "made");
    assert_eq!(renamed[0]["node"]["object"]["name"], "made");
    assert_eq!(renamed[1]["node"]["object"]["id"], queue);
    assert_eq!(renamed[1]["node"]["path"], "made/linux");
    assert_eq!(renamed[1]["node"]["paused_by"], json!(["made/linux"]));
    step(&mut actor, &mut watcher, &["group", "create", "other"], 1);
    let moved = step(
        &mut actor,
        &mut watcher,
        &["queue", "move", "made/linux", "--group", "other"],
        1,
    );
    assert_eq!(moved[0]["node"]["object"]["id"], queue);
    assert_eq!(moved[0]["node"]["path"], "other/linux");
    assert_eq!(moved[0]["node"]["effective"], json!({}));
    let removed = step(&mut actor, &mut watcher, &["group", "remove", "made"], 1);
    assert_eq!(removed[0]["push"], "object_removed");
    assert_eq!(removed[0]["id"], group);
    let tree = actor.ok("tree", json!({}));
    assert_eq!(tree["seq"], seq);
    let paths: Vec<&str> = tree["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["", "default", "other", "other/linux"]);
}

#[test]
fn subscription_resumes_after_a_number_and_says_resync_otherwise() {
    let mut daemon = Daemon::start("resume");
    let (mut actor, _) = Client::greet(&daemon);
    let running = actor.submit(&daemon, &[], &["sleep", "40"]);
    actor.until(running, &["running"]);
    std::thread::sleep(Duration::from_secs(2));
    let (mut watcher, _) = Client::greet(&daemon);
    let snapshot = watcher.snapshot(json!({}));
    let started = snapshot["started_ms"].clone();
    let seq = snapshot["seq"].as_u64().unwrap();
    actor.command(&daemon, &["group", "create", "one"]);
    actor.command(&daemon, &["group", "create", "two"]);
    let applied = watcher.changes_to(seq + 2);
    unbroken(seq, &applied);
    drop(watcher);
    for name in ["three", "four", "five"] {
        actor.command(&daemon, &["group", "create", name]);
    }
    let (mut watcher, greeting) = Client::greet(&daemon);
    assert_eq!(greeting["hello"]["started_ms"], started);
    let answer = watcher.ok(
        "subscribe",
        json!({"after": {"started_ms": started, "seq": seq + 2}, "interest": [running]}),
    );
    assert_eq!(answer["resumed"]["seq"], seq + 5);
    assert_eq!(answer["resumed"]["started_ms"], started);
    assert!(answer["resumed"]["now_ms"].as_u64().is_some());
    assert!(answer.get("snapshot").is_none());
    let mut missed = Vec::new();
    let mut sampled: BTreeMap<String, Value> = BTreeMap::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while missed.len() < 3 || sampled.len() < 3 {
        assert!(Instant::now() < deadline, "{missed:?} {sampled:?}");
        let line = watcher.pushed();
        if sequenced(&line) {
            assert!(sampled.is_empty(), "{line}");
            missed.push(line);
        } else if line["push"] != "heartbeat" {
            sampled
                .entry(line["push"].as_str().unwrap().to_owned())
                .or_insert(line);
        }
    }
    unbroken(seq + 2, &missed);
    let paths: Vec<&str> = missed
        .iter()
        .map(|line| line["node"]["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["three", "four", "five"]);
    assert_eq!(sampled["totals"]["seq"], seq + 5);
    assert_eq!(sampled["totals"]["counts"]["running"], 1);
    assert_eq!(sampled["use"]["objects"].as_array().unwrap().len(), 7);
    assert_eq!(sampled["live"]["jobs"][0]["id"], running);
    for after in [
        json!({"started_ms": started.as_u64().unwrap() - 1, "seq": seq}),
        json!({"started_ms": started.as_u64().unwrap() + 1, "seq": 0}),
    ] {
        let (mut stale, _) = Client::greet(&daemon);
        assert_eq!(
            stale.ok("subscribe", json!({"after": after})),
            json!({"resync": {"reason": "restarted"}})
        );
        assert!(stale.ok("subscribe", json!({}))["snapshot"].is_object());
    }
    let (mut ahead, _) = Client::greet(&daemon);
    assert_eq!(
        ahead.ok(
            "subscribe",
            json!({"after": {"started_ms": started, "seq": seq + 500}})
        ),
        json!({"resync": {"reason": "expired"}})
    );
    assert_eq!(ahead.ok("totals", json!({}))["seq"], seq + 5);
    for args in [
        json!({"after": 5}),
        json!({"after": {"seq": 1}}),
        json!({"rows": []}),
        json!({"rows": {"order": "priority"}}),
        json!({"rows": {"limit": 1001}}),
        json!({"interest": "1"}),
        json!({"interest": [1, "2"]}),
        json!({"interest": (0..501).collect::<Vec<u64>>()}),
    ] {
        ahead.refused("subscribe", args);
    }
    drop(actor);
    drop(watcher);
    drop(ahead);
    daemon.restart();
    let (mut later, greeting) = Client::greet(&daemon);
    assert_ne!(greeting["hello"]["started_ms"], started);
    assert_eq!(
        later.ok(
            "subscribe",
            json!({"after": {"started_ms": started, "seq": seq + 5}})
        ),
        json!({"resync": {"reason": "restarted"}})
    );
    let again = later.snapshot(json!({}));
    assert_eq!(again["seq"], 0);
    assert_eq!(again["started_ms"], greeting["hello"]["started_ms"]);
    assert_eq!(again["tree"]["nodes"].as_array().unwrap().len(), 7);
}

#[test]
fn subscription_of_a_reader_that_falls_behind_is_ended_with_resync() {
    let daemon = Daemon::start("slow");
    let (mut actor, greeting) = Client::greet(&daemon);
    assert_eq!(greeting["hello"]["limits"]["retained_changes"], 8192);
    actor.command(&daemon, &["group", "create", "wide"]);
    for number in 0..60 {
        let path = format!("wide/q{number}");
        actor.command(&daemon, &["queue", "create", &path]);
    }
    let (mut sleeper, _) = Client::greet(&daemon);
    let snapshot = sleeper.snapshot(json!({"rows": {"limit": 0}}));
    let seq = snapshot["seq"].as_u64().unwrap();
    let started = snapshot["started_ms"].clone();
    for round in 0..200 {
        let limit = (2 + round % 2).to_string();
        actor.command(&daemon, &["group", "set", "wide", "--max-running", &limit]);
    }
    let end = last_seq(&mut actor);
    assert_eq!(end, seq + 200 * 61);
    let mut expected = seq;
    let reason = loop {
        let line = sleeper.pushed();
        if line["push"] == "resync" {
            break line["reason"].clone();
        }
        if sequenced(&line) {
            expected += 1;
            assert_eq!(line["seq"], expected, "{line}");
        }
    };
    assert_eq!(reason, "slow");
    assert!(expected > seq && expected < end - 8192, "{expected}");
    sleeper.refused("interest", json!({"jobs": []}));
    assert_eq!(
        sleeper.ok(
            "subscribe",
            json!({"after": {"started_ms": started, "seq": expected}})
        ),
        json!({"resync": {"reason": "expired"}})
    );
    let fresh = sleeper.snapshot(json!({"rows": {"limit": 0}}));
    assert_eq!(fresh["seq"], end);
    let kept = sleeper.ok("totals", json!({}));
    assert_eq!(kept["seq"], end);
    let (mut recent, _) = Client::greet(&daemon);
    let resumed = recent.ok(
        "subscribe",
        json!({"after": {"started_ms": started, "seq": end - 8192}}),
    );
    assert_eq!(resumed["resumed"]["seq"], end);
    let lines = recent.changes_to(end);
    unbroken(end - 8192, &lines);
    assert_eq!(lines.len(), 8192);
    let (mut older, _) = Client::greet(&daemon);
    assert_eq!(
        older.ok(
            "subscribe",
            json!({"after": {"started_ms": started, "seq": end - 8193}})
        ),
        json!({"resync": {"reason": "expired"}})
    );
}

#[test]
fn subscription_sends_live_lines_only_for_the_interest_set() {
    let daemon = Daemon::start("interest");
    let (mut actor, greeting) = Client::greet(&daemon);
    assert_eq!(greeting["hello"]["limits"]["interest_jobs"], 500);
    assert_eq!(greeting["hello"]["limits"]["live_interval_ms"], 1000);
    let first = actor.submit(&daemon, &[], &["sleep", "40"]);
    let second = actor.submit(&daemon, &[], &["sleep", "41"]);
    actor.until(first, &["running"]);
    actor.until(second, &["running"]);
    actor.refused("interest", json!({"jobs": [first]}));
    let (mut watcher, _) = Client::greet(&daemon);
    let snapshot = watcher.snapshot(json!({"interest": [first, 99_999]}));
    assert_eq!(snapshot["interest"], 2);
    let live = |watcher: &mut Client, seconds: u64| -> (Vec<Value>, BTreeMap<String, usize>) {
        let until = Instant::now() + Duration::from_secs(seconds);
        let mut lines = Vec::new();
        let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
        while Instant::now() < until {
            let line = watcher.pushed();
            *kinds
                .entry(line["push"].as_str().unwrap().to_owned())
                .or_default() += 1;
            if line["push"] == "live" {
                lines.push(line);
            }
        }
        (lines, kinds)
    };
    let (lines, kinds) = live(&mut watcher, 5);
    assert!(!lines.is_empty(), "{kinds:?}");
    for kind in ["totals", "use", "live"] {
        assert!(kinds.get(kind).copied().unwrap_or(0) <= 7, "{kinds:?}");
    }
    let entry = &lines[0]["jobs"][0];
    assert!(lines[0]["at_ms"].as_u64().is_some());
    assert_eq!(entry["id"], first);
    assert_eq!(entry["attempt"], 1);
    assert!(entry["memory"].as_u64().is_some(), "{entry}");
    assert!(entry["pids"].as_u64().unwrap() >= 1);
    for unmeasured in [
        "cpu_at_ms",
        "cpu_ms",
        "throttled_ms",
        "oom_kill",
        "oom_group_kill",
        "pids_max_events",
    ] {
        assert_eq!(entry[unmeasured], "not_measured", "{entry}");
    }
    assert_eq!(entry.as_object().unwrap().len(), 13, "{entry}");
    let named: BTreeSet<u64> = lines
        .iter()
        .flat_map(|line| line["jobs"].as_array().unwrap().iter())
        .map(|entry| entry["id"].as_u64().unwrap())
        .collect();
    assert_eq!(named, BTreeSet::from([first]));
    let too_many: Vec<u64> = (1..=501).collect();
    watcher.send(&json!({"id": 70, "op": "interest", "args": {"jobs": too_many}}).to_string());
    watcher
        .send(&json!({"id": 71, "op": "interest", "args": {"jobs": [second, second]}}).to_string());
    let mut answers = Vec::new();
    let mut after = Vec::new();
    let until = Instant::now() + Duration::from_secs(6);
    while Instant::now() < until {
        let line = watcher.line().unwrap();
        if line.get("re").is_some() {
            answers.push(line);
        } else if line["push"] == "live" && answers.len() == 2 {
            after.push(line);
        }
    }
    assert_eq!(answers[0]["re"], 70);
    assert!(answers[0]["error"]["message"].as_str().is_some());
    assert_eq!(answers[1], json!({"re": 71, "ok": {"jobs": 1}}));
    let named: BTreeSet<u64> = after
        .iter()
        .flat_map(|line| line["jobs"].as_array().unwrap().iter())
        .map(|entry| entry["id"].as_u64().unwrap())
        .collect();
    assert_eq!(named, BTreeSet::from([second]));
    watcher.send(&json!({"id": 72, "op": "interest", "args": {"jobs": []}}).to_string());
    let until = Instant::now() + Duration::from_secs(4);
    let mut emptied = false;
    while Instant::now() < until {
        let line = watcher.line().unwrap();
        if line.get("re").is_some() {
            assert_eq!(line, json!({"re": 72, "ok": {"jobs": 0}}));
            emptied = true;
        } else {
            assert!(!(emptied && line["push"] == "live"), "{line}");
        }
    }
    assert!(emptied);
    for args in [json!({}), json!({"jobs": "1"}), json!({"jobs": [-1]})] {
        watcher.send(&json!({"id": 73, "op": "interest", "args": args}).to_string());
        loop {
            let line = watcher.line().unwrap();
            if line.get("re").is_some() {
                assert!(line["error"]["message"].as_str().is_some(), "{line}");
                break;
            }
        }
    }
}

#[test]
fn subscription_is_never_silent_longer_than_the_heartbeat() {
    let daemon = Daemon::start("heartbeat");
    let (mut watcher, greeting) = Client::greet(&daemon);
    assert_eq!(greeting["hello"]["limits"]["heartbeat_ms"], 5000);
    let snapshot = watcher.snapshot(json!({"rows": {"limit": 0}}));
    let until = Instant::now() + Duration::from_secs(13);
    let mut last = Instant::now();
    let mut longest = Duration::ZERO;
    let mut clocks = Vec::new();
    while Instant::now() < until {
        let line = watcher.pushed();
        longest = longest.max(last.elapsed());
        last = Instant::now();
        let kind = line["push"].as_str().unwrap();
        assert!(["totals", "use", "heartbeat"].contains(&kind), "{line}");
        if kind == "heartbeat" {
            println!("heartbeat {line}");
            assert_eq!(line.as_object().unwrap().len(), 3, "{line}");
        }
        if kind != "use" {
            assert_eq!(line["seq"], snapshot["seq"], "{line}");
            clocks.push(line["now_ms"].as_u64().unwrap());
        }
    }
    println!(
        "longest silence {longest:?}, lines with a clock {}",
        clocks.len()
    );
    assert!(longest < Duration::from_secs(8), "{longest:?}");
    assert!(clocks.len() >= 2 && clocks.len() <= 16, "{clocks:?}");
    assert!(clocks.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[test]
fn subscription_limits_one_per_connection_and_four_per_user() {
    let daemon = Daemon::start("limits");
    let (_, greeting) = Client::greet(&daemon);
    assert_eq!(greeting["hello"]["limits"]["subscriptions"], 4);
    let mut held: Vec<Client> = (0..4)
        .map(|_| {
            let (mut client, _) = Client::greet(&daemon);
            client.snapshot(json!({"rows": {"limit": 0}}));
            client
        })
        .collect();
    let (mut fifth, _) = Client::greet(&daemon);
    let said = fifth.refused("subscribe", json!({}));
    assert!(said.contains('4'), "{said}");
    assert!(fifth.ok("totals", json!({}))["counts"].is_object());
    assert!(fifth.ok("jobs", json!({}))["rows"].is_array());
    let subscribed = &mut held[0];
    for (op, args) in [("subscribe", json!({})), ("output", json!({"id": 1}))] {
        let id = subscribed.request(op, args);
        loop {
            let line = subscribed.line().unwrap();
            if line.get("re").is_some() {
                assert_eq!(line["re"], id);
                assert!(line["error"]["message"].as_str().is_some(), "{line}");
                break;
            }
        }
    }
    held.pop();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let answer = fifth.ask("subscribe", json!({"rows": {"limit": 0}}));
        if answer["ok"]["snapshot"].is_object() {
            break;
        }
        assert!(Instant::now() < deadline, "{answer}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn subscription_and_listings_on_another_connection_agree_by_sequence_number() {
    let daemon = Daemon::start("agree");
    let (mut actor, _) = Client::greet(&daemon);
    let (mut watcher, _) = Client::greet(&daemon);
    let snapshot = watcher.snapshot(json!({"rows": {"order": "id", "limit": 100}}));
    let mut rows: BTreeMap<u64, Value> = BTreeMap::new();
    let mut applied = snapshot["seq"].as_u64().unwrap();
    let mut submitted = Vec::new();
    for round in 0..6 {
        submitted.push(actor.submit(&daemon, &[], &["true"]));
        let page = actor.ok("jobs", json!({"order": "id", "limit": 100}));
        let valid = page["seq"].as_u64().unwrap();
        assert!(valid >= applied, "{round}");
        if valid > applied {
            for line in watcher.changes_to(valid) {
                applied = line["seq"].as_u64().unwrap();
                if line["push"] == "job" {
                    rows.insert(line["row"]["id"].as_u64().unwrap(), line["row"].clone());
                }
            }
        }
        assert_eq!(applied, valid);
        for row in page["rows"].as_array().unwrap() {
            let id = row["id"].as_u64().unwrap();
            let Some(pushed) = rows.get(&id) else {
                panic!("row {id} of the page was never pushed: {row}");
            };
            if pushed["revision"] == row["revision"] {
                let mut same = row.clone();
                same["predicted_start_ms"] = Value::Null;
                same["waited_for"] = pushed["waited_for"].clone();
                assert_eq!(&same, pushed, "{round}");
            } else {
                assert!(
                    pushed["revision"].as_u64() < row["revision"].as_u64(),
                    "{row} {pushed}"
                );
            }
        }
    }
    for id in submitted {
        actor.until(id, &ENDED);
    }
    let end = last_seq(&mut actor);
    for line in watcher.changes_to(end) {
        if line["push"] == "job" {
            rows.insert(line["row"]["id"].as_u64().unwrap(), line["row"].clone());
        }
    }
    let page = actor.ok("jobs", json!({"order": "id", "limit": 100}));
    assert_eq!(page["seq"], end);
    let listed: Vec<Value> = page["rows"].as_array().unwrap().clone();
    assert_eq!(listed, rows.values().cloned().collect::<Vec<Value>>());
}
