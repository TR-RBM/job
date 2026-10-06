use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const HELLO: &str =
    "{\"hello\":{\"versions\":{\"min\":1,\"max\":1},\"client\":\"listings-test 1\"}}";
const ENDED: [&str; 4] = ["succeeded", "failed", "cancelled", "lost"];
const ACTIVE: [&str; 4] = ["starting", "running", "suspended", "stopping"];

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
        let base = std::env::temp_dir().join(format!("job-listings-{}-{name}", std::process::id()));
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
            .env("JOB_SESSION", "listings")
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
            json!({"words": words, "cwd": daemon.work(), "session": "listings"}),
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

    fn output(&mut self, args: Value) -> (Value, Vec<Value>) {
        let id = self.request("output", args);
        let answer = self.line().expect("no answer");
        assert_eq!(answer["re"], id, "{answer}");
        assert!(answer.get("error").is_none(), "{answer}");
        let mut lines = Vec::new();
        if answer["ok"].get("missing").is_some() {
            return (answer["ok"].clone(), lines);
        }
        loop {
            let line = self.line().expect("the output ended without an end line");
            assert_eq!(line["re"], id, "{line}");
            let last = line.get("end").is_some();
            lines.push(line);
            if last {
                return (answer["ok"].clone(), lines);
            }
        }
    }
}

fn decoded(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    assert_eq!(text.len() % 4, 0, "{text}");
    let mut bytes = Vec::new();
    for chunk in text.as_bytes().chunks(4) {
        let padding = chunk.iter().filter(|byte| **byte == b'=').count();
        let mut group = 0u32;
        for symbol in chunk {
            let value = if *symbol == b'=' {
                0
            } else {
                ALPHABET.iter().position(|known| known == symbol).unwrap() as u32
            };
            group = group << 6 | value;
        }
        bytes.extend_from_slice(&group.to_be_bytes()[1..4 - padding]);
    }
    bytes
}

fn data(lines: &[Value], stream: Option<&str>) -> Vec<u8> {
    lines
        .iter()
        .filter_map(|line| line.get("record"))
        .filter(|record| stream.is_none_or(|stream| record["stream"] == stream))
        .flat_map(|record| decoded(record["data"].as_str().unwrap()))
        .collect()
}

fn ids(rows: &Value) -> Vec<u64> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_u64().unwrap())
        .collect()
}

fn expected(order: &str, rows: &[Value]) -> Vec<u64> {
    let number = |row: &Value, field: &str| row[field].as_u64();
    let late = |value: u64| u64::MAX - value;
    let mut keyed: Vec<([u64; 5], u64)> = rows
        .iter()
        .map(|row| {
            let id = row["id"].as_u64().unwrap();
            let state = row["state"].as_str().unwrap();
            let key = match order {
                "id" => [id, 0, 0, 0, 0],
                "started" => match number(row, "started_ms") {
                    Some(started) => [0, late(started), late(id), 0, 0],
                    None => [1, late(id), 0, 0, 0],
                },
                "ended" => match number(row, "finished_ms") {
                    Some(finished) => [0, late(finished), late(id), 0, 0],
                    None => [1, late(id), 0, 0, 0],
                },
                _ if ACTIVE.contains(&state) => [0, id, 0, 0, 0],
                _ if state == "queued" => [
                    1,
                    u64::from(number(row, "predicted_start_ms").is_none()),
                    number(row, "predicted_start_ms").unwrap_or(0),
                    number(row, "waiting_since_ms").unwrap(),
                    id,
                ],
                _ if state == "held" => [2, id, 0, 0, 0],
                _ => [
                    3,
                    late(number(row, "finished_ms").unwrap_or(0)),
                    late(id),
                    0,
                    0,
                ],
            };
            (key, id)
        })
        .collect();
    keyed.sort_unstable();
    keyed.into_iter().map(|(_, id)| id).collect()
}

struct Mixed {
    daemon: Daemon,
    client: Client,
    all: BTreeSet<u64>,
    running: u64,
    queued: u64,
    held: u64,
    failed: u64,
}

fn mixed(name: &str) -> Mixed {
    let daemon = Daemon::start(name);
    let (mut client, _) = Client::greet(&daemon);
    client.command(&daemon, &["group", "create", "builds"]);
    client.command(
        &daemon,
        &["queue", "create", "builds/linux", "--max-running", "1"],
    );
    let mut all = BTreeSet::new();
    let mut ended = Vec::new();
    for number in 0..5 {
        let label = format!("round={number}");
        ended.push(client.submit(&daemon, &["--label", &label], &["true"]));
    }
    let failed = client.submit(&daemon, &["--label", "kind=bad"], &["false"]);
    ended.push(failed);
    for id in &ended {
        client.until(*id, &ENDED);
    }
    let running = client.submit(
        &daemon,
        &["--queue", "builds/linux", "--label", "kind=long"],
        &["sleep", "40"],
    );
    client.until(running, &["running"]);
    let queued = client.submit(&daemon, &["--queue", "builds/linux"], &["sleep", "41"]);
    let held =
        client.command(&daemon, &["create", "--", "echo", "needle in a haystack"])["data"]["id"]
            .as_u64()
            .unwrap();
    let second =
        client.command(&daemon, &["create", "--session", "other", "--", "true"])["data"]["id"]
            .as_u64()
            .unwrap();
    all.extend(ended);
    all.extend([running, queued, held, second]);
    assert_eq!(client.state(queued), "queued");
    Mixed {
        daemon,
        client,
        all,
        running,
        queued,
        held,
        failed,
    }
}

#[test]
fn listings_page_every_order_forward_and_back_across_boundaries() {
    let mut mixed = mixed("orders");
    let client = &mut mixed.client;
    for order in ["activity", "id", "started", "ended"] {
        let whole = client.ok("jobs", json!({"order": order, "limit": 1000}));
        assert_eq!(whole["order"], order);
        assert_eq!(whole["offset"], 0);
        assert_eq!(whole["before"], Value::Null);
        assert_eq!(whole["after"], Value::Null);
        assert_eq!(whole["matching"]["total"], mixed.all.len());
        assert_eq!(whole["matching"]["ended_not_searched"], 0);
        let rows = whole["rows"].as_array().unwrap();
        let listed = ids(&whole["rows"]);
        assert_eq!(listed, expected(order, rows), "{order}");
        assert_eq!(
            listed.iter().copied().collect::<BTreeSet<u64>>(),
            mixed.all,
            "{order}"
        );
        let mut forward = Vec::new();
        let mut page = client.ok("jobs", json!({"order": order, "limit": 3}));
        assert_eq!(page["before"], Value::Null, "{order}");
        let mut last = Value::Null;
        loop {
            assert_eq!(page["offset"], forward.len(), "{order}");
            forward.extend(ids(&page["rows"]));
            if page["after"].is_null() {
                break;
            }
            assert_eq!(page["rows"].as_array().unwrap().len(), 3, "{order}");
            last = page["after"].clone();
            page = client.ok(
                "jobs",
                json!({"order": order, "limit": 3, "cursor": last, "direction": "forward"}),
            );
            assert!(page["before"].is_string(), "{order}");
        }
        assert_eq!(forward, listed, "{order}");
        let mut backward: Vec<u64> = ids(&page["rows"]);
        let mut before = page["before"].clone();
        while !before.is_null() {
            let page = client.ok(
                "jobs",
                json!({"order": order, "limit": 3, "cursor": before, "direction": "backward"}),
            );
            let mut rows = ids(&page["rows"]);
            assert!(!rows.is_empty(), "{order}");
            assert_eq!(
                page["offset"].as_u64().unwrap() as usize + rows.len() + backward.len(),
                listed.len(),
                "{order}"
            );
            rows.extend(backward);
            backward = rows;
            before = page["before"].clone();
        }
        assert_eq!(backward, listed, "{order}");
        let again = client.ok(
            "jobs",
            json!({"order": order, "limit": 3, "cursor": last, "direction": "backward"}),
        );
        let tail = ids(&page["rows"]).len();
        assert_eq!(
            ids(&again["rows"]),
            listed[listed.len() - tail - 3..listed.len() - tail],
            "{order}"
        );
    }
    let activity = ids(&client.ok("jobs", json!({}))["rows"]);
    assert_eq!(activity[0], mixed.running);
    assert_eq!(activity[1], mixed.queued);
    assert_eq!(activity[2], mixed.held);
    let counts = client.ok("jobs", json!({"limit": 0}));
    assert_eq!(counts["rows"], json!([]));
    assert_eq!(counts["offset"], Value::Null);
    assert_eq!(counts["before"], Value::Null);
    assert_eq!(counts["after"], Value::Null);
    assert_eq!(
        counts["matching"]["counts"],
        json!({"held": 2, "queued": 1, "running": 1, "succeeded": 5, "failed": 1})
    );
    let totals = client.ok("totals", json!({}));
    assert_eq!(totals["counts"]["succeeded"], 5);
    assert!(counts["now_ms"].as_u64().is_some());
    assert_eq!(counts["seq"], totals["seq"]);
}

#[test]
fn listings_filter_by_state_subtree_label_actor_session_and_text() {
    let mut mixed = mixed("filters");
    let client = &mut mixed.client;
    let uid = unsafe { libc::geteuid() };
    let listed = |client: &mut Client, filter: Value| -> (Vec<u64>, Value) {
        let answer = client.ok("jobs", json!({"order": "id", "filter": filter}));
        (ids(&answer["rows"]), answer["matching"].clone())
    };
    let (found, matching) = listed(client, json!({"states": ["running", "queued", "failed"]}));
    assert_eq!(found, vec![mixed.failed, mixed.running, mixed.queued]);
    assert_eq!(
        matching,
        json!({"total": 3, "counts": {"queued": 1, "running": 1, "failed": 1}, "ended_not_searched": 0})
    );
    for subtree in ["builds", "builds/linux", "/builds/"] {
        let (found, _) = listed(client, json!({"subtree": subtree}));
        assert_eq!(found, vec![mixed.running, mixed.queued], "{subtree}");
    }
    assert_eq!(
        listed(client, json!({"subtree": "build"})).0,
        Vec::<u64>::new()
    );
    assert_eq!(
        listed(client, json!({"subtree": ""})).0.len(),
        mixed.all.len()
    );
    assert_eq!(
        listed(client, json!({"subtree": "default"})).0.len(),
        mixed.all.len() - 2
    );
    assert_eq!(
        listed(client, json!({"labels": {"kind": "long"}})).0,
        vec![mixed.running]
    );
    assert_eq!(
        listed(client, json!({"labels": {"kind": "long", "round": "1"}})).0,
        Vec::<u64>::new()
    );
    assert_eq!(listed(client, json!({"labels": {"round": "3"}})).0.len(), 1);
    assert_eq!(
        listed(client, json!({"actor_uid": uid})).0.len(),
        mixed.all.len()
    );
    assert_eq!(
        listed(client, json!({"actor_uid": uid + 1})).0,
        Vec::<u64>::new()
    );
    assert_eq!(listed(client, json!({"session": "other"})).0.len(), 1);
    assert_eq!(
        listed(client, json!({"session": "listings"})).0.len(),
        mixed.all.len() - 1
    );
    assert_eq!(
        listed(client, json!({"text": "needle in a hay"})).0,
        vec![mixed.held]
    );
    assert_eq!(
        listed(client, json!({"text": "builds/lin"})).0,
        vec![mixed.running, mixed.queued]
    );
    assert_eq!(
        listed(client, json!({"text": "kind=bad"})).0,
        vec![mixed.failed]
    );
    assert_eq!(listed(client, json!({"text": "other"})).0.len(), 1);
    assert_eq!(
        listed(client, json!({"text": "Needle"})).0,
        Vec::<u64>::new()
    );
    assert_eq!(
        listed(
            client,
            json!({"states": ["held"], "text": "needle", "session": "listings"})
        )
        .0,
        vec![mixed.held]
    );
    let first = client.ok(
        "jobs",
        json!({"order": "id", "limit": 2, "filter": {"states": ["succeeded"]}}),
    );
    let cursor = first["after"].clone();
    assert!(cursor.is_string());
    let next = client.ok(
        "jobs",
        json!({"order": "id", "limit": 2, "cursor": cursor, "filter": {"states": ["succeeded"]}}),
    );
    assert_eq!(next["offset"], 2);
    assert_eq!(next["rows"].as_array().unwrap().len(), 2);
    for args in [
        json!({"order": "id", "limit": 2, "cursor": cursor, "filter": {"states": ["failed"]}}),
        json!({"order": "id", "limit": 2, "cursor": cursor}),
        json!({"order": "ended", "cursor": cursor, "filter": {"states": ["succeeded"]}}),
    ] {
        let said = client.refused("jobs", args);
        assert!(said.contains("another order or filter"), "{said}");
    }
    for args in [
        json!({"cursor": "bm90IGEgY3Vyc29y"}),
        json!({"cursor": "%%%"}),
        json!({"cursor": 5}),
        json!({"direction": "backward"}),
        json!({"direction": "forward"}),
        json!({"cursor": cursor, "direction": "sideways", "order": "id", "filter": {"states": ["succeeded"]}}),
        json!({"limit": 1001}),
        json!({"limit": -1}),
        json!({"order": "priority"}),
        json!({"filter": {"states": ["Running"]}}),
        json!({"filter": {"states": "running"}}),
        json!({"filter": {"owner": 5}}),
        json!({"filter": {"text": "x".repeat(257)}}),
        json!({"filter": {"labels": {"a": 1}}}),
        json!({"filter": []}),
        json!({"cursor": cursor, "at": 1}),
        json!({"ids_only": true, "limit": 5}),
        json!({"ids_only": true, "at": 1}),
        json!({"ids_only": true, "cursor": cursor}),
        json!({"ids_only": "yes"}),
        json!({"at": "1"}),
    ] {
        client.refused("jobs", args);
    }
    assert_eq!(
        client.ok(
            "jobs",
            json!({"limit": 1000, "filter": {"text": "x".repeat(256)}})
        )["rows"],
        json!([])
    );
}

#[test]
fn listings_cursor_expires_with_the_run_of_the_service() {
    let mut mixed = mixed("expiry");
    let page = mixed
        .client
        .ok("jobs", json!({"order": "ended", "limit": 2}));
    let cursor = page["after"].clone();
    let at = ids(&page["rows"])[1];
    assert!(cursor.is_string());
    let (_, greeting) = Client::greet(&mixed.daemon);
    let before = greeting["hello"]["started_ms"].clone();
    mixed.daemon.restart();
    let (mut client, greeting) = Client::greet(&mixed.daemon);
    assert_ne!(greeting["hello"]["started_ms"], before);
    let expired = client.ok(
        "jobs",
        json!({"order": "ended", "limit": 2, "cursor": cursor}),
    );
    assert_eq!(expired["cursor_expired"], true);
    assert_eq!(expired["rows"], json!([]));
    assert_eq!(expired["before"], Value::Null);
    assert_eq!(expired["after"], Value::Null);
    assert_eq!(expired["offset"], Value::Null);
    assert_eq!(expired["matching"]["total"], mixed.all.len());
    let said = client.refused("jobs", json!({"order": "id", "cursor": cursor}));
    assert!(said.contains("another order or filter"), "{said}");
    let placed = client.ok("jobs", json!({"order": "ended", "limit": 2, "at": at}));
    assert_eq!(placed["found"], true);
    assert!(ids(&placed["rows"]).contains(&at));
    assert!(placed.get("cursor_expired").is_none());
    let fresh = client.ok("jobs", json!({"order": "ended", "limit": 2}));
    assert_eq!(ids(&fresh["rows"]), ids(&page["rows"]));
    assert_ne!(fresh["after"], cursor);
}

#[test]
fn listings_place_a_page_around_an_id_and_answer_ids_only() {
    let mut mixed = mixed("placement");
    let client = &mut mixed.client;
    let whole = ids(&client.ok("jobs", json!({"order": "id", "limit": 1000}))["rows"]);
    let target = whole[6];
    let placed = client.ok("jobs", json!({"order": "id", "limit": 4, "at": target}));
    assert_eq!(placed["found"], true);
    assert_eq!(ids(&placed["rows"]), whole[4..8]);
    assert_eq!(placed["offset"], 4);
    assert!(placed["before"].is_string());
    assert!(placed["after"].is_string());
    let first = client.ok("jobs", json!({"order": "id", "limit": 4, "at": whole[1]}));
    assert_eq!(ids(&first["rows"]), whole[0..4]);
    assert_eq!(first["before"], Value::Null);
    let single = client.ok("jobs", json!({"order": "id", "limit": 1, "at": target}));
    assert_eq!(ids(&single["rows"]), vec![target]);
    let earlier = client.ok(
        "jobs",
        json!({"order": "id", "limit": 4, "cursor": placed["before"], "direction": "backward"}),
    );
    assert_eq!(ids(&earlier["rows"]), whole[0..4]);
    for args in [
        json!({"order": "id", "limit": 4, "at": 99_999}),
        json!({"order": "id", "limit": 4, "at": mixed.held, "filter": {"states": ["running"]}}),
    ] {
        let absent = client.ok("jobs", args);
        assert_eq!(absent["found"], false);
        assert_eq!(absent["rows"], json!([]));
        assert_eq!(absent["offset"], Value::Null);
    }
    let named = client.ok("jobs", json!({"ids_only": true, "order": "id"}));
    assert_eq!(ids_of(&named["ids"]), whole);
    assert_eq!(named["ids_more"], 0);
    assert_eq!(named["matching"]["total"], whole.len());
    assert!(named.get("rows").is_none());
    assert!(named.get("after").is_none());
    let some = client.ok(
        "jobs",
        json!({"ids_only": true, "filter": {"states": ["running", "held"]}}),
    );
    assert_eq!(ids_of(&some["ids"])[0], mixed.running);
    assert_eq!(some["ids"].as_array().unwrap().len(), 3);
    assert_eq!(some["order"], "activity");
}

fn ids_of(value: &Value) -> Vec<u64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_u64().unwrap())
        .collect()
}

fn cloned(daemon: &mut Daemon, template: u64, copies: u64) -> Duration {
    daemon.stop();
    let record =
        std::fs::read_to_string(daemon.state.join(format!("jobs/{template}/job.json"))).unwrap();
    let mut job: Value = serde_json::from_str(&record).unwrap();
    let finished = job["finished_ms"].as_u64().unwrap();
    for number in 1..=copies {
        let id = template + number;
        job["id"] = json!(id);
        job["finished_ms"] = json!(finished + number);
        job["result"]["finished_ms"] = json!(finished + number);
        let directory = daemon.state.join(format!("jobs/{id}"));
        std::fs::create_dir(&directory).unwrap();
        for entry in std::fs::read_dir(daemon.state.join(format!("jobs/{template}"))).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                std::fs::copy(entry.path(), directory.join(entry.file_name())).unwrap();
            }
        }
        let mut streams: Value =
            serde_json::from_slice(&std::fs::read(directory.join("streams.json")).unwrap())
                .unwrap();
        streams["job_id"] = json!(id);
        std::fs::write(directory.join("streams.json"), streams.to_string()).unwrap();
        std::fs::write(directory.join("job.json"), job.to_string()).unwrap();
    }
    std::fs::write(
        daemon.state.join("next-id"),
        (template + copies + 1).to_string(),
    )
    .unwrap();
    let started = Instant::now();
    daemon.child = Daemon::spawn(&daemon.base, daemon.indexed);
    daemon.ready();
    started.elapsed()
}

#[test]
fn listings_say_how_many_ids_and_ended_jobs_they_left_out() {
    let mut daemon = Daemon::start("left-out");
    let (mut client, greeting) = Client::greet(&daemon);
    assert_eq!(greeting["hello"]["limits"]["ids"], 10_000);
    let template = client.submit(&daemon, &[], &["true"]);
    client.until(template, &ENDED);
    drop(client);
    cloned(&mut daemon, template, 10_049);
    let (mut client, _) = Client::greet(&daemon);
    let named = client.ok("jobs", json!({"ids_only": true, "order": "id"}));
    assert_eq!(named["matching"]["total"], 10_050);
    assert_eq!(named["matching"]["ended_not_searched"], 0);
    assert_eq!(named["ids"].as_array().unwrap().len(), 10_000);
    assert_eq!(named["ids_more"], 50);
    assert_eq!(named["ids"][0], template);
    assert_eq!(named["ids"][9_999], template + 9_999);
    let newest = client.ok("jobs", json!({"order": "ended", "limit": 2}));
    assert_eq!(
        ids(&newest["rows"]),
        vec![template + 10_049, template + 10_048]
    );
    drop(client);
    drop(daemon);

    let mut daemon = Daemon::bounded("bounded", Some(5));
    let (mut client, greeting) = Client::greet(&daemon);
    assert_eq!(greeting["hello"]["limits"]["indexed_ended_jobs"], 5);
    let template = client.submit(&daemon, &[], &["true"]);
    client.until(template, &ENDED);
    drop(client);
    cloned(&mut daemon, template, 8);
    let (mut client, _) = Client::greet(&daemon);
    let listed = client.ok("jobs", json!({"order": "id"}));
    assert_eq!(
        listed["matching"],
        json!({"total": 5, "counts": {"succeeded": 5}, "ended_not_searched": 4})
    );
    assert_eq!(
        ids(&listed["rows"]),
        (template + 4..=template + 8).collect::<Vec<u64>>()
    );
    assert_eq!(client.ok("totals", json!({}))["counts"]["succeeded"], 9);
    let outside = client.ok("jobs", json!({"at": template}));
    assert_eq!(outside["found"], false);
    let by_id = client.ok("job", json!({"id": template}));
    assert_eq!(by_id["row"]["id"], template);
    assert_eq!(by_id["row"]["state"], "succeeded");
    let held = client.command(&daemon, &["create", "--", "true"])["data"]["id"]
        .as_u64()
        .unwrap();
    let live = client.ok("jobs", json!({"order": "id"}));
    assert_eq!(live["matching"]["total"], 6);
    assert_eq!(live["matching"]["ended_not_searched"], 4);
    let ended = client.submit(&daemon, &[], &["true"]);
    client.until(ended, &ENDED);
    let moved = client.ok("jobs", json!({"order": "ended"}));
    assert_eq!(
        moved["matching"]["counts"],
        json!({"held": 1, "succeeded": 5})
    );
    assert_eq!(moved["matching"]["ended_not_searched"], 5);
    assert_eq!(ids(&moved["rows"])[0], ended);
    client.command(&daemon, &["retry", &template.to_string(), "--hold"]);
    let retried = client.ok("jobs", json!({"order": "id"}));
    assert_eq!(retried["matching"]["ended_not_searched"], 4);
    assert_eq!(
        retried["matching"]["counts"],
        json!({"held": 2, "succeeded": 5})
    );
    client.command(&daemon, &["remove", &(template + 1).to_string()]);
    let removed = client.ok("jobs", json!({"order": "id"}));
    assert_eq!(removed["matching"]["ended_not_searched"], 3);
    assert!(ids(&removed["rows"]).contains(&held));
}

#[test]
fn listings_one_job_in_full_for_running_ended_absent_and_removed_jobs() {
    let daemon = Daemon::start("record");
    let (mut client, _) = Client::greet(&daemon);
    let running = client.submit(
        &daemon,
        &["--label", "kind=long", "--cores", "2", "--priority", "5"],
        &["sleep", "40"],
    );
    client.until(running, &["running"]);
    let deadline = Instant::now() + Duration::from_secs(20);
    let answer = loop {
        let answer = client.ok("job", json!({"id": running}));
        if answer["confirmed"].is_object() {
            break answer;
        }
        assert!(Instant::now() < deadline, "{answer}");
        std::thread::sleep(Duration::from_millis(50));
    };
    let (row, job) = (&answer["row"], &answer["job"]);
    assert_eq!(row["id"], running);
    assert_eq!(row["state"], "running");
    assert_eq!(job["state"], "running");
    assert_eq!(job["backend"], "watch");
    assert_eq!(job["id"], running);
    assert_eq!(job["attempt"], 1);
    assert_eq!(job["spec"]["argv"], json!(["sleep", "40"]));
    assert_eq!(job["spec"]["declared"]["labels"], json!({"kind": "long"}));
    assert_eq!(job["spec"]["declared"]["net"], "host");
    assert_eq!(job["priority_source"], json!({"from": "job"}));
    assert_eq!(row["priority_source"], json!({"from": "job"}));
    assert_eq!(row["priority"], 5);
    assert_eq!(
        job["reservation"]["cores_source"],
        json!({"kind": "declared"})
    );
    assert!(job["reservation"]["memory_source"]["kind"].is_string());
    assert_eq!(job["reservation"]["vector"]["cores_milli"], 2000);
    assert_eq!(row["reserved"]["cores_milli"], 2000);
    for source in job["resource_sources"].as_object().unwrap().values() {
        assert!(source["from"].is_string(), "{source}");
    }
    assert_eq!(job["policy"], "ordinary");
    assert!(job["log"].as_str().unwrap().ends_with("output.log"));
    assert_eq!(job["revision"], row["revision"]);
    assert!(row["revision"].as_u64().unwrap() >= 2);
    assert_eq!(row["labels"], json!({"kind": "long"}));
    assert_eq!(row["argv"], json!(["sleep", "40"]));
    assert_eq!(row["argv_truncated"], false);
    assert_eq!(row["session"], "listings");
    assert_eq!(row["queue"], "default");
    assert_eq!(row["actor_uid"], unsafe { libc::geteuid() });
    assert_eq!(row["actor_pid"], std::process::id());
    assert!(row["started_ms"].as_u64().is_some());
    assert_eq!(row["finished_ms"], Value::Null);
    assert_eq!(row["usage"], Value::Null);
    assert_eq!(row["memory_max"], "not_applicable");
    assert_eq!(row["cgroup"], Value::Null);
    assert_eq!(row["stop_kind"], Value::Null);
    assert_eq!(row["suspended"], json!({"total_ms": 0, "since_ms": null}));
    assert_eq!(
        row["confirmed"],
        json!({
            "namespaces": [], "pid_namespace": false, "no_new_privs": false,
            "cap_drop": null, "seccomp_denied": 0, "cpu_affinity": null,
        })
    );
    let confirmed = &answer["confirmed"];
    assert!(confirmed["at_ms"].as_u64().unwrap() >= row["submitted_ms"].as_u64().unwrap());
    assert_eq!(confirmed["isolation_controls"], Value::Null);
    assert_eq!(confirmed["security_controls"], Value::Null);
    assert!(confirmed["sources"].is_object());
    let live = &answer["live"];
    assert_eq!(live["attempt"], 1);
    assert!(live["at_ms"].as_u64().is_some());
    assert!(live["memory"].as_u64().is_some(), "{live}");
    assert!(live["pids"].as_u64().unwrap() >= 1, "{live}");
    for unmeasured in [
        "cpu_at_ms",
        "cpu_ms",
        "throttled_ms",
        "oom_kill",
        "oom_group_kill",
        "pids_max_events",
    ] {
        assert_eq!(live[unmeasured], "not_measured", "{unmeasured}");
    }
    let listed = client.ok("jobs", json!({"filter": {"states": ["running"]}}));
    let mut same = listed["rows"][0].clone();
    assert_eq!(same["confirmed"], row["confirmed"]);
    same["waited_for"] = row["waited_for"].clone();
    assert_eq!(&same, row);

    let long: String = "x".repeat(700);
    let held =
        client.command(&daemon, &["create", "--", "echo", &long, &long, "tail"])["data"]["id"]
            .as_u64()
            .unwrap();
    let waiting = client.ok("job", json!({"id": held}));
    assert_eq!(waiting["row"]["state"], "held");
    assert_eq!(waiting["confirmed"], "not_applicable");
    assert_eq!(waiting["row"]["confirmed"], "not_applicable");
    assert_eq!(waiting["live"], Value::Null);
    assert_eq!(waiting["row"]["argv_truncated"], true);
    let kept: usize = waiting["row"]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|word| word.as_str().unwrap().len())
        .sum();
    assert_eq!(kept, 1024);
    assert_eq!(waiting["job"]["spec"]["argv"][3], "tail");
    let before = waiting["row"]["revision"].as_u64().unwrap();
    client.command(&daemon, &["release", &held.to_string()]);
    client.until(held, &ENDED);
    let after = client.ok("job", json!({"id": held}));
    assert!(after["row"]["revision"].as_u64().unwrap() > before);

    let failing = client.submit(&daemon, &[], &["bash", "-c", "exit 3"]);
    client.until(failing, &ENDED);
    let ended = client.ok("job", json!({"id": failing}));
    assert_eq!(ended["row"]["state"], "failed");
    assert_eq!(ended["job"]["state"], "failed");
    assert_eq!(ended["row"]["exit_code"], 3);
    assert_eq!(ended["job"]["result"]["exit_code"], 3);
    assert_eq!(ended["live"], Value::Null);
    assert_eq!(ended["predicted_start_ms"], Value::Null);
    assert!(ended["row"]["finished_ms"].as_u64().is_some());
    assert_eq!(ended["row"]["usage"]["cpu_ms"], "not_measured");
    assert_eq!(ended["row"]["usage"]["throttled_ms"], "not_measured");
    assert!(ended["row"]["usage"]["peak_memory"].as_u64().is_some());
    assert!(ended["confirmed"]["at_ms"].as_u64().is_some());
    assert!(ended["job"]["timing"]["elapsed_ms"].as_u64().is_some());

    client.command(&daemon, &["cancel", &running.to_string()]);
    client.until(running, &ENDED);
    let cancelled = client.ok("job", json!({"id": running}));
    assert_eq!(cancelled["row"]["state"], "cancelled");
    assert_eq!(cancelled["row"]["stop_kind"], "cancelled");
    assert_eq!(cancelled["job"]["stop"]["kind"], "cancelled");

    client.command(&daemon, &["retry", &failing.to_string(), "--hold"]);
    let second = client.ok("job", json!({"id": failing}));
    assert_eq!(second["row"]["attempt"], 2);
    assert_eq!(second["row"]["state"], "held");
    assert!(
        second["row"]["revision"].as_u64().unwrap() > ended["row"]["revision"].as_u64().unwrap()
    );
    let first = client.ok("job", json!({"id": failing, "attempt": 1}));
    assert_eq!(first["row"]["attempt"], 1);
    assert_eq!(first["row"]["state"], "failed");
    assert_eq!(first["job"]["result"]["exit_code"], 3);
    assert!(first["confirmed"]["at_ms"].as_u64().is_some());
    assert_eq!(first["live"], Value::Null);
    assert_eq!(
        client.ok("job", json!({"id": failing, "attempt": 2}))["row"]["attempt"],
        2
    );
    assert_eq!(
        client.ok("job", json!({"id": failing, "attempt": 7})),
        json!({"missing": "attempt"})
    );
    assert_eq!(
        client.ok("job", json!({"id": 99_999})),
        json!({"missing": "job"})
    );
    client.command(&daemon, &["remove", &held.to_string()]);
    assert_eq!(
        client.ok("job", json!({"id": held})),
        json!({"missing": "job"})
    );
    assert_eq!(
        client.ok("processes", json!({"id": held})),
        json!({"missing": "job"})
    );
    assert!(!ids(&client.ok("jobs", json!({"order": "id"}))["rows"]).contains(&held));
    std::fs::write(
        daemon.state.join(format!("jobs/{running}/job.json")),
        "{ not a record",
    )
    .unwrap();
    for op in ["job", "processes", "output"] {
        let id = client.request(op, json!({"id": running}));
        let answer = client.line().unwrap();
        assert_eq!(
            answer,
            json!({"re": id, "ok": {"missing": "record"}}),
            "{op}"
        );
    }
    for args in [
        json!({}),
        json!({"id": "1"}),
        json!({"id": 1, "attempt": "x"}),
    ] {
        client.refused("job", args);
    }
}

#[test]
fn listings_tree_names_depth_and_use_of_groups_and_queues() {
    let daemon = Daemon::start("tree");
    let (mut client, greeting) = Client::greet(&daemon);
    assert!(
        !greeting["hello"]["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("cgroup"))
    );
    client.command(
        &daemon,
        &[
            "group",
            "create",
            "builds",
            "--max-running",
            "2",
            "--mem",
            "1G",
        ],
    );
    client.command(
        &daemon,
        &[
            "queue",
            "create",
            "builds/linux",
            "--max-running",
            "1",
            "--cores",
            "2",
        ],
    );
    client.command(&daemon, &["queue", "create", "builds/mac"]);
    client.command(&daemon, &["queue", "pause", "builds/mac"]);
    let running = client.submit(
        &daemon,
        &["--queue", "builds/linux", "--cores", "1"],
        &["sleep", "40"],
    );
    client.until(running, &["running"]);
    let queued = client.submit(&daemon, &["--queue", "builds/linux"], &["sleep", "41"]);
    let waiting = client.submit(&daemon, &["--queue", "builds/mac"], &["true"]);
    let reserved = client.ok("job", json!({"id": running}))["row"]["reserved"]["memory"]
        .as_u64()
        .unwrap();
    let tree = client.ok("tree", json!({}));
    assert!(tree["now_ms"].as_u64().is_some());
    assert_eq!(tree["seq"], 0);
    let nodes = tree["nodes"].as_array().unwrap();
    let paths: Vec<&str> = nodes
        .iter()
        .map(|node| node["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        ["", "builds", "default", "builds/linux", "builds/mac"]
    );
    let named = |path: &str| nodes.iter().find(|node| node["path"] == path).unwrap();
    let root = named("");
    assert_eq!(root["object"]["parent"], Value::Null);
    assert_eq!(root["object"]["kind"], "group");
    assert_eq!(root["depth"]["running"], 1);
    assert_eq!(root["depth"]["queued"], 2);
    assert_eq!(root["use"]["running"], json!({"count": 1, "limits": []}));
    assert_eq!(root["use"]["budgets"], json!([]));
    assert_eq!(root["use"]["domains"], json!([]));
    let builds = named("builds");
    let builds_id = builds["object"]["id"].as_u64().unwrap();
    assert_eq!(builds["object"]["kind"], "group");
    assert_eq!(builds["object"]["parent"], root["object"]["id"]);
    assert_eq!(builds["object"]["config"]["max_running"], 2);
    assert_eq!(
        builds["effective"]["max_running"],
        json!({"value": 2, "source": {"from": "object", "id": builds_id, "path": "builds"}})
    );
    assert_eq!(
        builds["use"]["running"],
        json!({"count": 1, "limits": [
            {"limit": 2, "count": 1, "set_by": {"id": builds_id, "path": "builds"}},
        ]})
    );
    assert_eq!(
        builds["use"]["budgets"],
        json!([{
            "resource": "memory", "limit": 1u64 << 30, "reserved": reserved,
            "set_by": {"id": builds_id, "path": "builds"},
        }])
    );
    let linux = named("builds/linux");
    let linux_id = linux["object"]["id"].as_u64().unwrap();
    assert_eq!(linux["object"]["kind"], "queue");
    assert_eq!(linux["object"]["parent"], builds_id);
    assert_eq!(linux["object"]["name"], "linux");
    assert_eq!(linux["depth"]["running"], 1);
    assert_eq!(linux["depth"]["queued"], 1);
    assert_eq!(linux["depth"]["held"], 0);
    let since = linux["depth"]["oldest_queued_since_ms"].as_u64().unwrap();
    let age = linux["depth"]["oldest_queued_age_ms"].as_u64().unwrap();
    assert_eq!(since + age, tree["now_ms"].as_u64().unwrap());
    assert_eq!(
        since,
        client.ok("job", json!({"id": queued}))["row"]["waiting_since_ms"]
    );
    assert_eq!(linux["depth"]["started_last_hour"]["count"], 1);
    assert_eq!(
        linux["effective"]["max_running"]["source"],
        json!({"from": "object", "id": linux_id, "path": "builds/linux"})
    );
    assert_eq!(
        linux["use"]["running"]["limits"],
        json!([
            {"limit": 1, "count": 1, "set_by": {"id": linux_id, "path": "builds/linux"}},
            {"limit": 2, "count": 1, "set_by": {"id": builds_id, "path": "builds"}},
        ])
    );
    assert_eq!(
        linux["use"]["budgets"],
        json!([
            {"resource": "cores_milli", "limit": 2000, "reserved": 1000,
             "set_by": {"id": linux_id, "path": "builds/linux"}},
            {"resource": "memory", "limit": 1u64 << 30, "reserved": reserved,
             "set_by": {"id": builds_id, "path": "builds"}},
        ])
    );
    let mac = named("builds/mac");
    assert_eq!(mac["object"]["paused"], true);
    assert_eq!(mac["paused_by"], json!(["builds/mac"]));
    assert_eq!(mac["closed_by"], json!([]));
    assert_eq!(mac["depth"]["queued"], 1);
    assert_eq!(
        mac["depth"]["oldest_queued_since_ms"],
        client.ok("job", json!({"id": waiting}))["row"]["waiting_since_ms"]
    );
    assert_eq!(mac["use"]["running"]["count"], 0);
    assert_eq!(mac["depth"]["started_last_hour"]["count"], 0);
    let idle = named("default");
    assert_eq!(idle["depth"]["oldest_queued_age_ms"], Value::Null);
    assert_eq!(idle["depth"]["oldest_queued_since_ms"], Value::Null);
    let row = client.ok("job", json!({"id": running}))["row"].clone();
    assert_eq!(row["queue_id"], linux_id);
    assert_eq!(row["queue"], "builds/linux");
}

#[test]
fn listings_processes_of_a_job_with_children_and_of_one_that_ended() {
    let daemon = Daemon::start("processes");
    let (mut client, greeting) = Client::greet(&daemon);
    assert_eq!(greeting["hello"]["limits"]["processes"], 4096);
    let id = client.submit(&daemon, &[], &["bash", "-c", "sleep 40 & sleep 41 & wait"]);
    client.until(id, &["running"]);
    let deadline = Instant::now() + Duration::from_secs(20);
    let answer = loop {
        let answer = client.ok("processes", json!({"id": id}));
        if answer["processes"].as_array().unwrap().len() >= 3 {
            break answer;
        }
        assert!(Instant::now() < deadline, "{answer}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(answer["active"], true);
    assert_eq!(answer["attempt"], 1);
    assert_eq!(answer["source"], "descendants");
    assert_eq!(answer["truncated"], false);
    assert!(answer["at_ms"].as_u64().unwrap() <= answer["now_ms"].as_u64().unwrap());
    let processes = answer["processes"].as_array().unwrap();
    let pids: Vec<u64> = processes
        .iter()
        .map(|p| p["pid"].as_u64().unwrap())
        .collect();
    let mut sorted = pids.clone();
    sorted.sort_unstable();
    assert_eq!(pids, sorted);
    let shell = processes.iter().find(|p| p["name"] == "bash").unwrap();
    let sleeps: Vec<&Value> = processes.iter().filter(|p| p["name"] == "sleep").collect();
    assert_eq!(sleeps.len(), 2);
    for sleep in sleeps {
        assert_eq!(sleep["parent"], shell["pid"]);
        assert_eq!(sleep["threads"], 1);
        assert!(sleep["rss"].as_u64().is_some());
        assert!(sleep["start_ticks"].as_u64().unwrap() > 0);
        let stat = std::fs::read_to_string(format!("/proc/{}/stat", sleep["pid"])).unwrap();
        assert!(stat.contains("(sleep)"));
    }
    client.command(&daemon, &["cancel", &id.to_string()]);
    client.until(id, &ENDED);
    let ended = client.ok("processes", json!({"id": id}));
    assert_eq!(ended["active"], false);
    assert_eq!(ended["processes"], json!([]));
    assert_eq!(ended["truncated"], false);
    assert_eq!(
        client.ok("processes", json!({"id": 4242})),
        json!({"missing": "job"})
    );
    client.refused("processes", json!({}));
}

fn ending(lines: &[Value]) -> &Value {
    &lines.last().unwrap()["end"]
}

#[test]
fn listings_output_from_the_start_a_position_and_the_end_with_binary_bytes() {
    let daemon = Daemon::start("output");
    let (mut client, greeting) = Client::greet(&daemon);
    assert_eq!(greeting["hello"]["limits"]["output_piece_bytes"], 4_194_304);
    let all: Vec<u8> = (0..=255u8).collect();
    let octal: String = all.iter().map(|byte| format!("\\{byte:03o}")).collect();
    let script = format!(
        "printf 'one\\n'; sleep 0.3; printf 'two\\n' >&2; sleep 0.3; printf '{octal}'; sleep 0.3; printf 'three\\n'"
    );
    let id = client.submit(&daemon, &[], &["bash", "-c", &script]);
    client.until(id, &ENDED);
    let (head, lines) = client.output(json!({"id": id}));
    assert_eq!(head["attempt"], 1);
    assert_eq!(head["mode"], "pipe");
    assert_eq!(head["first_sequence"], 0);
    assert_eq!(head["complete"], true);
    assert_eq!(head["quota"], Value::Null);
    assert_eq!(head["trimmed_bytes"], Value::Null);
    let count = head["next_sequence"].as_u64().unwrap();
    assert!(count >= 4, "{head}");
    assert_eq!(
        head["totals"],
        json!([
            {"stream": "stdout", "written_bytes": 266, "kept_bytes": 266},
            {"stream": "stderr", "written_bytes": 4, "kept_bytes": 4},
        ])
    );
    let mut expected: Vec<u8> = b"one\n".to_vec();
    expected.extend_from_slice(&all);
    expected.extend_from_slice(b"three\n");
    assert_eq!(data(&lines, Some("stdout")), expected);
    assert_eq!(data(&lines, Some("stderr")), b"two\n");
    let records: Vec<&Value> = lines.iter().filter_map(|line| line.get("record")).collect();
    assert_eq!(records.len() as u64, count);
    for (number, record) in records.iter().enumerate() {
        assert_eq!(record["sequence"], number);
        assert!(record["at_ms"].as_u64().unwrap() > 0);
    }
    let status = lines[lines.len() - 2]["status"].clone();
    assert_eq!(
        status,
        json!({"complete": true, "terminal": true, "exit_status": 0, "error": null, "quota": null, "trimmed": null})
    );
    assert_eq!(
        ending(&lines),
        &json!({"reason": "complete", "first_sequence": 0, "next_sequence": count, "more": false})
    );
    let (_, later) = client.output(json!({"id": id, "from": {"sequence": 2}}));
    assert_eq!(
        later
            .iter()
            .filter(|line| line.get("record").is_some())
            .count() as u64,
        count - 2
    );
    assert_eq!(later[0]["record"]["sequence"], 2);
    assert_eq!(ending(&later)["first_sequence"], 2);
    assert_eq!(ending(&later)["next_sequence"], count);
    let (_, errors) = client.output(json!({"id": id, "streams": ["stderr"]}));
    assert_eq!(data(&errors, None), b"two\n");
    assert_eq!(errors[0]["record"]["sequence"], 1);
    assert_eq!(ending(&errors)["first_sequence"], 1);
    assert_eq!(ending(&errors)["next_sequence"], count);
    let (_, nothing) = client.output(json!({"id": id, "streams": ["terminal"]}));
    assert_eq!(nothing.len(), 2);
    assert_eq!(
        ending(&nothing),
        &json!({"reason": "complete", "first_sequence": null, "next_sequence": count, "more": false})
    );
    let (_, past) = client.output(json!({"id": id, "from": "end"}));
    assert_eq!(past.len(), 1);
    assert_eq!(
        ending(&past),
        &json!({"reason": "complete", "first_sequence": null, "next_sequence": count, "more": false})
    );
    let mut pieces = Vec::new();
    let mut from = json!("start");
    let mut rounds = 0;
    loop {
        let (_, piece) = client.output(json!({"id": id, "from": from, "max_bytes": 5}));
        pieces.extend(data(&piece, None));
        let end = ending(&piece).clone();
        rounds += 1;
        if end["more"] == false {
            assert_eq!(end["reason"], "complete");
            break;
        }
        assert_eq!(end["reason"], "limit");
        from = json!({"sequence": end["next_sequence"]});
        assert!(rounds < 50);
    }
    assert!(rounds >= 3);
    let mut whole = b"one\ntwo\n".to_vec();
    whole.extend_from_slice(&all);
    whole.extend_from_slice(b"three\n");
    assert_eq!(pieces, whole);
    let (_, tail) =
        client.output(json!({"id": id, "from": "end", "direction": "backward", "max_bytes": 7}));
    assert_eq!(data(&tail, None), b"three\n");
    assert_eq!(
        ending(&tail),
        &json!({"reason": "limit", "first_sequence": count - 1, "next_sequence": count, "more": true})
    );
    assert!(tail[tail.len() - 2].get("status").is_some());
    let mut gathered: Vec<u8> = Vec::new();
    let mut from = json!("end");
    loop {
        let (_, piece) = client
            .output(json!({"id": id, "from": from, "direction": "backward", "max_bytes": 300}));
        let mut bytes = data(&piece, None);
        bytes.extend(gathered);
        gathered = bytes;
        let end = ending(&piece).clone();
        if end["more"] == false {
            break;
        }
        from = json!({"sequence": end["first_sequence"]});
    }
    assert_eq!(gathered, whole);
    let (_, none) = client.output(json!({"id": id, "from": "start", "direction": "backward"}));
    assert_eq!(
        ending(&none),
        &json!({"reason": "complete", "first_sequence": null, "next_sequence": 0, "more": false})
    );
    let (_, below) =
        client.output(json!({"id": id, "from": {"sequence": 2}, "direction": "backward"}));
    assert_eq!(data(&below, None), b"one\ntwo\n");
    assert_eq!(ending(&below)["first_sequence"], 0);
    assert_eq!(ending(&below)["next_sequence"], 2);
    assert_eq!(ending(&below)["more"], false);
    let (missing, _) = client.output(json!({"id": 777}));
    assert_eq!(missing, json!({"missing": "job"}));
    let (missing, _) = client.output(json!({"id": id, "attempt": 4}));
    assert_eq!(missing, json!({"missing": "attempt"}));
    for args in [
        json!({}),
        json!({"id": id, "from": "middle"}),
        json!({"id": id, "from": {"sequence": "2"}}),
        json!({"id": id, "direction": "backward", "follow": true}),
        json!({"id": id, "streams": ["stdout", "gap"]}),
        json!({"id": id, "max_bytes": 0}),
        json!({"id": id, "max_bytes": 4_194_305}),
        json!({"id": id, "follow": 1}),
    ] {
        client.refused("output", args);
    }
    assert_eq!(client.ok("end", json!({"of": 3}))["stopped"], false);
    client.refused("end", json!({}));
    assert!(client.ok("totals", json!({}))["counts"].is_object());
}

#[test]
fn listings_output_follows_until_the_job_ends_and_stops_on_end() {
    let daemon = Daemon::start("follow");
    let (mut client, _) = Client::greet(&daemon);
    let short = client.submit(
        &daemon,
        &[],
        &["bash", "-c", "echo first; sleep 3; echo second"],
    );
    let (head, lines) = client.output(json!({"id": short, "follow": true}));
    assert_eq!(head["attempt"], 1);
    assert_eq!(data(&lines, Some("stdout")), b"first\nsecond\n");
    assert!(
        lines
            .iter()
            .filter(|line| line.get("status").is_some())
            .count()
            >= 3
    );
    let last = &lines[lines.len() - 2]["status"];
    assert_eq!(last["complete"], true);
    assert_eq!(last["terminal"], true);
    assert_eq!(last["exit_status"], 0);
    assert_eq!(ending(&lines)["reason"], "complete");
    assert_eq!(ending(&lines)["more"], false);
    assert_eq!(client.state(short), "succeeded");

    let long = client.submit(
        &daemon,
        &[],
        &["bash", "-c", "echo started; sleep 40; echo never"],
    );
    let output = client.request("output", json!({"id": long, "follow": true}));
    let waiting = client.request("totals", json!({}));
    let answer = client.line().unwrap();
    assert_eq!(answer["re"], output);
    assert!(answer["ok"]["attempt"].is_u64(), "{answer}");
    assert_eq!(answer["ok"]["complete"], false);
    assert_eq!(
        answer["ok"]["totals"],
        json!([
            {"stream": "stdout", "written_bytes": "not_measured", "kept_bytes": "not_measured"},
            {"stream": "stderr", "written_bytes": "not_measured", "kept_bytes": "not_measured"},
        ])
    );
    let quiet = Instant::now() + Duration::from_secs(3);
    let mut seen = Vec::new();
    while Instant::now() < quiet {
        let line = client.line().unwrap();
        assert_eq!(line["re"], output, "{line}");
        assert!(line.get("ok").is_none(), "{line}");
        assert!(line.get("end").is_none(), "{line}");
        seen.push(line);
    }
    assert_eq!(data(&seen, None), b"started\n");
    assert!(seen.iter().any(|line| line["status"]["terminal"] == false));
    let behind = client.request("job", json!({"id": long}));
    let stop = client.request("end", json!({"of": output}));
    let mut rest = Vec::new();
    loop {
        let line = client.line().unwrap();
        let ended = line.get("end").is_some();
        rest.push(line);
        if ended {
            break;
        }
    }
    let end = rest.last().unwrap();
    assert_eq!(end["re"], output);
    assert_eq!(end["end"]["reason"], "stopped");
    assert_eq!(end["end"]["first_sequence"], 0);
    assert_eq!(end["end"]["next_sequence"], 1);
    assert_eq!(end["end"]["more"], true);
    assert_eq!(
        client.line().unwrap(),
        json!({"re": stop, "ok": {"stopped": true}})
    );
    let first = client.line().unwrap();
    assert_eq!(first["re"], waiting);
    assert_eq!(first["ok"]["counts"]["running"], 1);
    let second = client.line().unwrap();
    assert_eq!(second["re"], behind);
    assert_eq!(second["ok"]["row"]["state"], "running");
    assert_eq!(client.ok("end", json!({"of": output}))["stopped"], false);
    let (_, again) = client.output(json!({"id": long, "from": {"sequence": 1}}));
    assert_eq!(
        ending(&again),
        &json!({"reason": "complete", "first_sequence": null, "next_sequence": 1, "more": false})
    );
    let (mut other, _) = Client::greet(&daemon);
    let followed = other.request("output", json!({"id": long, "from": "end", "follow": true}));
    assert_eq!(other.line().unwrap()["re"], followed);
    client.command(&daemon, &["cancel", &long.to_string()]);
    let mut closing = Vec::new();
    loop {
        let line = other.line().unwrap();
        assert_eq!(line["re"], followed);
        let ended = line.get("end").is_some();
        closing.push(line);
        if ended {
            break;
        }
    }
    assert_eq!(data(&closing, Some("stdout")), b"");
    assert_eq!(ending(&closing)["reason"], "complete");
    assert_eq!(closing[closing.len() - 2]["status"]["terminal"], true);
    assert_ne!(closing[closing.len() - 2]["status"]["exit_status"], 0);
    assert!(other.ok("totals", json!({}))["counts"].is_object());
}

#[test]
fn listings_output_names_a_gap_where_the_window_dropped_records() {
    let daemon = Daemon::start("gap");
    let (mut client, _) = Client::greet(&daemon);
    let id = client.submit(
        &daemon,
        &["--output-head", "1M", "--output-tail", "1M"],
        &[
            "bash",
            "-c",
            "head -c 6291456 /dev/zero | tr '\\0' 'a'; echo; echo done",
        ],
    );
    client.until(id, &ENDED);
    let mut gaps = Vec::new();
    let mut bytes = 0usize;
    let mut sequences = Vec::new();
    let mut from = json!("start");
    let mut head = Value::Null;
    for _ in 0..40 {
        let (answer, piece) =
            client.output(json!({"id": id, "from": from, "max_bytes": 4_194_304}));
        head = answer;
        for line in &piece {
            if let Some(gap) = line.get("gap") {
                gaps.push(gap.clone());
            }
            if let Some(record) = line.get("record") {
                sequences.push(record["sequence"].as_u64().unwrap());
            }
        }
        bytes += data(&piece, Some("stdout")).len();
        let end = ending(&piece).clone();
        if end["more"] == false {
            break;
        }
        from = json!({"sequence": end["next_sequence"]});
    }
    assert_eq!(
        head["quota"],
        json!({"head_bytes": 1 << 20, "tail_bytes": 1 << 20})
    );
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    let gap = &gaps[0];
    assert_eq!(gap["stream"], "stdout");
    let lost = gap["bytes"].as_u64().unwrap() as usize;
    let records = gap["records"].as_u64().unwrap();
    assert!(records > 0);
    assert_eq!(bytes + lost, 6_291_456 + 6);
    assert!(lost > 0);
    assert!((2 << 20..5 << 20).contains(&bytes), "{bytes}");
    let total = head["totals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|total| total["stream"] == "stdout")
        .unwrap();
    assert_eq!(total["written_bytes"], 6_291_456 + 6);
    assert_eq!(total["kept_bytes"], bytes);
    let start = gap["sequence"].as_u64().unwrap();
    assert!(
        sequences
            .iter()
            .all(|sequence| *sequence < start || *sequence >= start + records)
    );
    assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
    let (_, inside) =
        client.output(json!({"id": id, "from": {"sequence": start + 1}, "max_bytes": 65_536}));
    assert_eq!(inside[0]["gap"], *gap);
    assert_eq!(inside[1]["record"]["sequence"], start + records);
    assert_eq!(ending(&inside)["first_sequence"], start);
    let (_, back) = client.output(json!({
        "id": id, "from": {"sequence": start + records + 1}, "direction": "backward", "max_bytes": 1,
    }));
    assert_eq!(back[0]["gap"], *gap, "{back:?}");
    assert_eq!(back[1]["record"]["sequence"], start + records);
    assert_eq!(
        ending(&back),
        &json!({"reason": "limit", "first_sequence": start, "next_sequence": start + records + 1, "more": true})
    );
}

fn timed(client: &mut Client, args: Value) -> (Duration, Duration, usize) {
    let mut times = Vec::new();
    let mut bytes = 0;
    for _ in 0..21 {
        let line = json!({"id": 1, "op": "jobs", "args": args}).to_string();
        let started = Instant::now();
        client.send(&line);
        let mut answer = String::new();
        client.reader.read_line(&mut answer).unwrap();
        times.push(started.elapsed());
        bytes = answer.len();
        assert!(answer.contains("\"matching\""), "{answer}");
    }
    times.sort_unstable();
    (times[10], times[20], bytes)
}

#[test]
#[ignore]
fn listings_measure_one_page_among_many_ended_jobs() {
    let mut daemon = Daemon::start("measure");
    let (mut client, _) = Client::greet(&daemon);
    let started = Instant::now();
    for number in 0..2000 {
        let label = format!("n={number}");
        client.submit(&daemon, &["--label", &label], &["true"]);
    }
    let deadline = Instant::now() + Duration::from_secs(900);
    loop {
        let counts = client.ok("totals", json!({}))["counts"].clone();
        if counts["succeeded"] == 2000 {
            break;
        }
        assert!(Instant::now() < deadline, "{counts}");
        std::thread::sleep(Duration::from_millis(200));
    }
    println!(
        "2000 Jobs of `true` submitted and ended in {:?}",
        started.elapsed()
    );
    for (name, args) in [
        ("activity, 200 rows", json!({"limit": 200})),
        ("ended, 200 rows", json!({"order": "ended", "limit": 200})),
        (
            "id, 200 rows from the middle",
            json!({"order": "id", "limit": 200, "at": 1000}),
        ),
        (
            "activity, text filter, 200 rows",
            json!({"limit": 200, "filter": {"text": "n=1"}}),
        ),
        ("counts only", json!({"limit": 0})),
        ("ids only", json!({"ids_only": true})),
    ] {
        let (median, slowest, bytes) = timed(&mut client, args);
        println!("2000 ended, {name}: median {median:?}, slowest {slowest:?}, {bytes} bytes");
    }
    let page = client.ok("jobs", json!({"limit": 200}));
    assert_eq!(page["rows"].as_array().unwrap().len(), 200);
    assert_eq!(page["matching"]["total"], 2000);
    drop(client);
    let restart = cloned(&mut daemon, 2000, 48_000);
    println!("start of the service with 50000 records, 48000 of them copies: {restart:?}");
    let (mut client, _) = Client::greet(&daemon);
    for (name, args) in [
        ("activity, 200 rows", json!({"limit": 200})),
        (
            "id, 200 rows from the middle",
            json!({"order": "id", "limit": 200, "at": 25_000}),
        ),
        (
            "activity, text filter, 200 rows",
            json!({"limit": 200, "filter": {"text": "n=1"}}),
        ),
        ("counts only", json!({"limit": 0})),
    ] {
        let (median, slowest, bytes) = timed(&mut client, args);
        println!("50000 ended, {name}: median {median:?}, slowest {slowest:?}, {bytes} bytes");
    }
    let page = client.ok("jobs", json!({"limit": 200}));
    assert_eq!(page["matching"]["total"], 50_000);
    assert_eq!(page["matching"]["ended_not_searched"], 0);
}

#[test]
fn listings_output_of_a_terminal_job_is_the_stream_terminal() {
    let daemon = Daemon::start("terminal");
    let (mut client, _) = Client::greet(&daemon);
    let id = client.submit(
        &daemon,
        &["--pty"],
        &["bash", "-c", "echo terminal-line-4711"],
    );
    client.until(id, &ENDED);
    let answer = client.ok("job", json!({"id": id}));
    assert_eq!(answer["row"]["terminal"], true);
    assert_eq!(answer["job"]["output_mode"], "pty");
    let (head, lines) = client.output(json!({"id": id, "streams": ["terminal"]}));
    assert_eq!(head["mode"], "pty");
    let written = String::from_utf8_lossy(&data(&lines, Some("terminal"))).into_owned();
    assert!(written.contains("terminal-line-4711"), "{written:?}");
    assert!(
        lines
            .iter()
            .filter_map(|line| line.get("record"))
            .all(|record| record["stream"] == "terminal")
    );
    assert_eq!(ending(&lines)["reason"], "complete");
    let (_, piped) = client.output(json!({"id": id, "streams": ["stdout"]}));
    assert!(
        piped.iter().all(|line| line.get("record").is_none()),
        "{piped:?}"
    );
    assert_eq!(ending(&piped)["first_sequence"], Value::Null);
    assert_eq!(ending(&piped)["reason"], "complete");
    let (_, whole) = client.output(json!({"id": id}));
    assert_eq!(data(&whole, None), data(&lines, Some("terminal")));
}

#[test]
fn listings_row_names_the_network_drops_an_old_deadline_and_counts_silent_streams() {
    let mut daemon = Daemon::start("honest");
    let (mut client, _) = Client::greet(&daemon);
    let silent = client.submit(&daemon, &[], &["true"]);
    let half = client.submit(&daemon, &[], &["bash", "-c", "printf abc"]);
    let stopped = client.submit(&daemon, &[], &["sleep", "60"]);
    let held = client.command(&daemon, &["create", "--net", "none", "--", "true"])["data"]["id"]
        .as_u64()
        .unwrap();
    client.until(silent, &ENDED);
    client.until(half, &ENDED);
    client.until(stopped, &["running"]);
    let rows = client.ok("jobs", json!({"order": "id"}))["rows"].clone();
    for row in rows.as_array().unwrap() {
        let network = if row["id"] == held { "none" } else { "host" };
        assert_eq!(row["network"], network, "{row}");
        assert_eq!(row["termination_deadline_ms"], Value::Null, "{row}");
    }
    assert_eq!(rows.as_array().unwrap().len(), 4);
    client.command(&daemon, &["cancel", &stopped.to_string()]);
    client.until(stopped, &["cancelled"]);
    let (head, _) = client.output(json!({"id": silent}));
    assert_eq!(head["first_sequence"], Value::Null);
    assert_eq!(head["next_sequence"], 0);
    assert_eq!(head["complete"], true);
    assert_eq!(
        head["totals"],
        json!([
            {"stream": "stdout", "written_bytes": 0, "kept_bytes": 0},
            {"stream": "stderr", "written_bytes": 0, "kept_bytes": 0},
        ])
    );
    let (head, _) = client.output(json!({"id": half}));
    assert_eq!(
        head["totals"],
        json!([
            {"stream": "stdout", "written_bytes": 3, "kept_bytes": 3},
            {"stream": "stderr", "written_bytes": 0, "kept_bytes": 0},
        ])
    );
    drop(client);
    daemon.stop();
    let path = daemon.state.join(format!("jobs/{stopped}/job.json"));
    let mut record: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record["termination_deadline_ms"] = json!(1_790_000_000_000u64);
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let path = daemon.state.join(format!("jobs/{silent}/job.json"));
    let mut record: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record["result"]["output_retention"] = Value::Null;
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let path = daemon.state.join(format!("jobs/{half}/job.json"));
    let mut record: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record["result"]["output_retention"] = Value::Null;
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    daemon.restart();
    let (mut client, _) = Client::greet(&daemon);
    let answer = client.ok("job", json!({"id": stopped}));
    assert_eq!(answer["row"]["state"], "cancelled");
    assert_eq!(answer["row"]["stop_kind"], "cancelled");
    assert_eq!(
        answer["job"]["termination_deadline_ms"], 1_790_000_000_000u64,
        "{answer}"
    );
    assert_eq!(answer["row"]["termination_deadline_ms"], Value::Null);
    let listed = client.ok("jobs", json!({"order": "id", "at": stopped, "limit": 1}));
    assert_eq!(listed["rows"][0]["id"], stopped);
    assert_eq!(listed["rows"][0]["termination_deadline_ms"], Value::Null);
    assert_eq!(listed["rows"][0]["network"], "host");
    let (head, _) = client.output(json!({"id": silent}));
    assert_eq!(
        head["totals"],
        json!([
            {"stream": "stdout", "written_bytes": 0, "kept_bytes": 0},
            {"stream": "stderr", "written_bytes": 0, "kept_bytes": 0},
        ])
    );
    let (head, lines) = client.output(json!({"id": half}));
    assert_eq!(data(&lines, Some("stdout")), b"abc");
    assert_eq!(
        head["totals"],
        json!([
            {"stream": "stdout", "written_bytes": "not_measured", "kept_bytes": "not_measured"},
            {"stream": "stderr", "written_bytes": "not_measured", "kept_bytes": "not_measured"},
        ])
    );
}
