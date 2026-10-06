use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const HELLO: &str =
    "{\"hello\":{\"versions\":{\"min\":1,\"max\":1},\"client\":\"client-interface-test 1\"}}";
const LINE_IN: usize = 4_194_304;

struct Daemon {
    child: Child,
    base: PathBuf,
    state: PathBuf,
    work: PathBuf,
}

impl Daemon {
    fn start(name: &str) -> Daemon {
        let base = std::env::temp_dir().join(format!("job-clientif-{}-{name}", std::process::id()));
        let state = base.join("state");
        let work = base.join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(
            base.join("config.toml"),
            "schema_version = 1\nprofile = 'ordinary'\n",
        )
        .unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_job"))
            .arg("daemon")
            .env("JOB_CGROUP_ROOT", base.join("absent-cgroup"))
            .env("JOB_CONFIG", base.join("config.toml"))
            .env("JOB_STATE_DIR", &state)
            .env("LC_ALL", "C")
            .env_remove("LANG")
            .env_remove("LC_MESSAGES")
            .env_remove("JOB_CLI_COMPAT")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut daemon = Daemon {
            child,
            base,
            state,
            work,
        };
        daemon.ready();
        daemon
    }

    fn stream(&self) -> Option<UnixStream> {
        let stream = UnixStream::connect(self.state.join("daemon.sock")).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        Some(stream)
    }

    fn ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "daemon exited during startup"
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
            .env("JOB_SESSION", "client-interface")
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
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn held(&self) -> u64 {
        self.ok(&["create", "--", "true"]).trim().parse().unwrap()
    }

    fn work(&self) -> String {
        self.work.display().to_string()
    }

    fn native(&self, line: &str) -> String {
        let mut stream = self.stream().unwrap();
        writeln!(stream, "{line}").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next: u64,
}

impl Client {
    fn raw(daemon: &Daemon) -> Client {
        let writer = daemon.stream().unwrap();
        Client {
            reader: BufReader::new(writer.try_clone().unwrap()),
            writer,
            next: 1,
        }
    }

    fn greet(daemon: &Daemon) -> (Client, Value) {
        let mut client = Client::raw(daemon);
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

    fn closed(&mut self) -> bool {
        self.line().is_none()
    }

    fn ask(&mut self, op: &str, args: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(&json!({"id": id, "op": op, "args": args}).to_string());
        let answer = self.line().expect("no answer");
        assert_eq!(answer["re"], id, "{answer}");
        answer
    }

    fn ok(&mut self, op: &str, args: Value) -> Value {
        let answer = self.ask(op, args);
        assert!(answer.get("error").is_none(), "{answer}");
        answer["ok"].clone()
    }

    fn command(&mut self, daemon: &Daemon, words: &[&str], more: Value) -> Value {
        let mut args = json!({"words": words, "cwd": daemon.work()});
        for (name, value) in more.as_object().into_iter().flatten() {
            args[name] = value.clone();
        }
        self.ok("command", args)
    }

    fn state(&mut self, id: u64) -> String {
        let answer = self.ok("native", json!({"request": {"Status": {"id": id}}}));
        let job = answer["answer"]
            .as_object()
            .and_then(|answer| answer.values().next())
            .map(|body| body["job"].clone())
            .unwrap_or_default();
        job["state"].as_str().unwrap_or_default().to_owned()
    }
}

fn sentence(answer: &Value) -> String {
    let text = answer["error"]["message"].as_str().unwrap_or_default();
    assert!(!text.is_empty(), "{answer}");
    text.to_owned()
}

#[test]
fn client_interface_greeting_names_version_capabilities_peer_and_limits() {
    let daemon = Daemon::start("greeting");
    let (mut client, greeting) = Client::greet(&daemon);
    let hello = &greeting["hello"];
    assert_eq!(hello["version"], 1);
    assert_eq!(hello["versions"], json!({"min": 1, "max": 1}));
    assert_eq!(
        hello["capabilities"],
        json!([
            "subscribe",
            "interest",
            "jobs",
            "job",
            "tree",
            "totals",
            "processes",
            "output",
            "commands",
            "command",
            "native"
        ])
    );
    assert!(hello["job_version"].as_str().is_some_and(|v| !v.is_empty()));
    assert!(hello["native_protocol"].as_u64().is_some());
    assert!(hello["hostname"].as_str().is_some());
    assert!(hello["started_ms"].as_u64().unwrap() <= hello["now_ms"].as_u64().unwrap());
    assert_eq!(hello["backend"], "watch");
    assert_eq!(hello["enforcement"], "monitoring_only");
    assert_eq!(hello["service"]["mode"], "user");
    assert_eq!(hello["service"]["uid"], unsafe { libc::geteuid() });
    assert_eq!(hello["service"]["socket_group"], Value::Null);
    assert!(hello["service"]["socket_rule"].as_str().is_some());
    assert_eq!(
        hello["peer"],
        json!({
            "uid": unsafe { libc::geteuid() },
            "pid": std::process::id(),
            "via": "owner",
            "may": {"read": true, "change": true, "events": false, "audit": false},
        })
    );
    assert_eq!(
        hello["limits"],
        json!({
            "line_bytes_in": 4194304, "line_bytes_out": 16777216, "page_rows": 1000,
            "page_rows_default": 200, "interest_jobs": 500, "connections": 32,
            "subscriptions": 4, "retained_changes": 8192, "heartbeat_ms": 5000,
            "live_interval_ms": 1000, "output_piece_bytes": 4194304, "processes": 4096,
            "idle_ms": 60000, "indexed_ended_jobs": 50000, "ids": 10000, "poll_ms": 5000,
        })
    );
    let host = client.ok("native", json!({"request": "Host"}));
    assert_eq!(host["protocol"], hello["native_protocol"]);
    assert_eq!(
        host["answer"]["Host"]["info"]["version"],
        hello["job_version"]
    );
    assert_eq!(
        host["answer"]["Host"]["info"]["health"]["started_ms"],
        hello["started_ms"]
    );
}

#[test]
fn client_interface_chooses_the_highest_common_version_or_says_unsupported() {
    let daemon = Daemon::start("versions");
    let mut wide = Client::raw(&daemon);
    wide.send("{\"hello\":{\"versions\":{\"min\":1,\"max\":7}}}");
    assert_eq!(wide.line().unwrap()["hello"]["version"], 1);
    assert_eq!(wide.ok("totals", json!({}))["counts"]["running"], 0);
    let mut later = Client::raw(&daemon);
    later.send("{\"hello\":{\"versions\":{\"min\":2,\"max\":3}}}");
    let refusal = later.line().unwrap();
    assert_eq!(
        refusal["unsupported"]["versions"],
        json!({"min": 1, "max": 1})
    );
    assert!(refusal["unsupported"]["job_version"].as_str().is_some());
    assert!(later.closed());
    for malformed in [
        "{\"hello\":{}}",
        "{\"hello\":5}",
        "{\"hello\":{\"versions\":{\"min\":2,\"max\":1}}}",
        "{\"hello\":{\"versions\":{\"min\":0,\"max\":1}}}",
        "{\"hello\":{\"versions\":{\"min\":\"1\",\"max\":1}}}",
        "{\"hello\":{\"versions\":{\"min\":1,\"max\":1},\"client\":7}}",
    ] {
        let mut client = Client::raw(&daemon);
        client.send(malformed);
        let answer = client.line().unwrap();
        sentence(&answer);
        assert!(answer.get("re").is_none(), "{malformed}: {answer}");
        assert!(client.closed(), "{malformed}");
    }
    let mut named = Client::raw(&daemon);
    named.send(
        &json!({"hello": {"versions": {"min": 1, "max": 1}, "client": "n".repeat(129)}})
            .to_string(),
    );
    sentence(&named.line().unwrap());
    assert!(named.closed());
}

#[test]
fn client_interface_leaves_the_native_first_line_as_it_was() {
    let daemon = Daemon::start("native-first");
    assert!(daemon.native("\"Ping\"").starts_with("{\"Pong\":"));
    let host = daemon.native("{\"Versioned\":{\"protocol\":20,\"min\":20,\"request\":\"Host\"}}");
    let host: Value = serde_json::from_str(&host).unwrap();
    assert!(host["Host"]["info"]["version"].as_str().is_some());
    for unreadable in [
        "{\"hello\":{},\"more\":1}",
        "{\"hello\":",
        "[\"hello\"]",
        "{\"Hello\":{}}",
    ] {
        let answer = daemon.native(unreadable);
        assert!(
            answer.starts_with("{\"Error\":{\"message\":\"unreadable request: "),
            "{unreadable}: {answer}"
        );
        assert_eq!(answer.lines().count(), 1);
    }
    let (mut client, _) = Client::greet(&daemon);
    assert!(client.ok("totals", json!({}))["counts"].is_object());
}

#[test]
fn client_interface_totals_count_every_state_and_the_reserved_capacity() {
    let daemon = Daemon::start("totals");
    let (mut client, greeting) = Client::greet(&daemon);
    let empty = client.ok("totals", json!({}));
    assert_eq!(
        empty["counts"],
        json!({
            "held": 0, "queued": 0, "starting": 0, "running": 0, "suspended": 0,
            "stopping": 0, "succeeded": 0, "failed": 0, "cancelled": 0, "lost": 0,
        })
    );
    assert_eq!(empty["seq"], 0);
    assert!(empty["now_ms"].as_u64().unwrap() >= greeting["hello"]["now_ms"].as_u64().unwrap());
    let capacity = &empty["capacity"];
    for part in ["cores_milli", "memory", "pids"] {
        assert!(capacity["pool"][part].as_u64().unwrap() > 0, "{capacity}");
        assert_eq!(capacity["reserved"][part], 0, "{capacity}");
    }
    assert_eq!(capacity["running"], 0);
    assert_eq!(capacity["waiting"], 0);
    assert!(
        capacity["memory_total"].as_u64().unwrap()
            >= capacity["memory_available"].as_u64().unwrap()
    );
    assert!(capacity["output"]["budget_bytes"].as_u64().unwrap() > 0);
    assert!(
        capacity["output"]["recorded_bytes"] == "unknown"
            || capacity["output"]["recorded_bytes"].is_u64(),
        "{capacity}"
    );
    assert!(
        capacity["state_free_bytes"] == "unknown" || capacity["state_free_bytes"].is_u64(),
        "{capacity}"
    );
    daemon.held();
    let submitted = client.command(
        &daemon,
        &["submit", "--cores", "1", "--", "sleep", "20"],
        json!({}),
    );
    let id = submitted["data"]["id"]
        .as_u64()
        .unwrap_or_else(|| panic!("{submitted}"));
    let deadline = Instant::now() + Duration::from_secs(10);
    let running = loop {
        let totals = client.ok("totals", json!({}));
        if totals["counts"]["running"] == 1 {
            break totals;
        }
        assert!(Instant::now() < deadline, "{totals}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(running["counts"]["held"], 1);
    assert_eq!(running["capacity"]["running"], 1);
    assert_eq!(running["capacity"]["waiting"], 1);
    assert_eq!(running["capacity"]["reserved"]["cores_milli"], 1000);
    assert!(running["capacity"]["reserved"]["memory"].is_u64());
    assert!(running["capacity"]["reserved"]["pids"].is_u64());
    let cancelled = client.command(&daemon, &["cancel", &id.to_string()], json!({}));
    assert_eq!(cancelled["exit_status"], 0, "{cancelled}");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let totals = client.ok("totals", json!({}));
        if totals["counts"]["cancelled"] == 1 && totals["capacity"]["running"] == 0 {
            assert_eq!(totals["capacity"]["reserved"]["cores_milli"], 0);
            break;
        }
        assert!(Instant::now() < deadline, "{totals}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn client_interface_commands_are_the_command_table_in_both_languages() {
    let daemon = Daemon::start("commands");
    let (mut client, _) = Client::greet(&daemon);
    let english = client.ok("commands", json!({"language": "en"}));
    assert_eq!(english["language"], "en");
    assert_eq!(english["global"][0]["long"], "--system");
    assert_eq!(english["global"][0]["arity"], "flag");
    let commands = english["commands"].as_array().unwrap();
    let find = |path: Value| {
        commands
            .iter()
            .find(|command| command["path"] == path)
            .unwrap_or_else(|| panic!("no {path}"))
    };
    let pause = find(json!(["queue", "pause"]));
    assert_eq!(pause["section"], "organization");
    assert_eq!(pause["kind"], "queue_pause");
    assert_eq!(pause["available"], true);
    assert_eq!(pause["trailing_command"], false);
    assert!(pause["operands"][0]["name"].is_string());
    assert_eq!(pause["operands"][0]["repeat"], false);
    assert_eq!(pause["operands"][0]["value"], json!({"kind": "queue_path"}));
    assert_eq!(pause["operands"][0]["required"], true);
    let options: Vec<&Value> = pause["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|group| group["options"].as_array().unwrap())
        .collect();
    let format = options
        .iter()
        .find(|option| option["long"] == "--format")
        .unwrap();
    assert_eq!(format["arity"], "one");
    assert_eq!(format["meta"], "text|json");
    assert_eq!(
        format["value"],
        json!({"kind": "words", "words": ["text", "json"]})
    );
    assert!(
        daemon
            .ok(&["queue", "pause", "--help"])
            .contains(pause["usage"].as_str().unwrap())
    );
    assert!(
        daemon
            .ok(&["queue", "pause", "--help"])
            .contains(pause["summary"].as_str().unwrap())
    );
    let submit = find(json!(["submit"]));
    assert_eq!(submit["section"], "execution");
    assert_eq!(submit["trailing_command"], true);
    assert!(
        submit["exits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|exit| exit[0] == "0")
    );
    for internal in ["daemon", "shim", "hook", "remote", "__complete"] {
        assert!(
            !commands
                .iter()
                .any(|command| command["path"][0] == internal),
            "{internal}"
        );
    }
    let german = client.ok("commands", json!({"language": "de"}));
    assert_eq!(german["language"], "de");
    let translated = german["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["path"] == json!(["queue", "pause"]))
        .unwrap();
    assert_ne!(translated["summary"], pause["summary"]);
    assert_eq!(translated["usage"], pause["usage"]);
    assert_eq!(client.ok("commands", json!({}))["language"], "en");
    let refused = client.ask("commands", json!({"language": "fr"}));
    sentence(&refused);
    assert_eq!(client.ok("commands", json!({"language": "en"})), english);
}

#[test]
fn client_interface_command_submits_and_cancels_as_the_job_command_does() {
    let daemon = Daemon::start("command");
    let (mut client, _) = Client::greet(&daemon);
    let submitted = client.command(
        &daemon,
        &["submit", "--label", "branch=main", "--", "sleep", "20"],
        json!({"session": "ci", "env": {"vars": [["ONLY", "this one"]]}}),
    );
    assert_eq!(submitted["outcome"], "done", "{submitted}");
    assert_eq!(submitted["exit_status"], 0);
    assert_eq!(submitted["kind"], "submit");
    assert_eq!(submitted["text"], json!([]));
    assert_eq!(submitted["diagnostics"], json!([]));
    assert!(submitted.get("use").is_none());
    let job = &submitted["data"];
    let id = job["id"].as_u64().unwrap();
    assert_eq!(job["spec"]["argv"], json!(["sleep", "20"]));
    assert_eq!(job["spec"]["cwd"], daemon.work());
    assert_eq!(job["spec"]["session"], "ci");
    assert_eq!(job["spec"]["declared"]["labels"], json!({"branch": "main"}));
    assert_eq!(job["actor_uid"], unsafe { libc::geteuid() });
    assert_eq!(job["actor_pid"], std::process::id());
    let saved: Value = serde_json::from_slice(
        &std::fs::read(daemon.state.join(format!("jobs/{id}/env.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(saved, json!({"vars": [["ONLY", "this one"]]}));
    let shown: Value =
        serde_json::from_slice(&daemon.job(&["show", &id.to_string(), "--json"]).stdout)
            .unwrap_or_default();
    assert_eq!(shown["id"], id, "{shown}");
    assert_eq!(shown["spec"], job["spec"]);
    let audit = daemon.ok(&["audit", "--json", "--action", "submit"]);
    assert!(
        audit.contains(&format!("\"peer_pid\":{}", std::process::id()))
            || audit.contains(&format!("\"peer_pid\": {}", std::process::id())),
        "{audit}"
    );
    let empty = client.command(&daemon, &["create", "--", "true"], json!({}));
    let held = empty["data"]["id"].as_u64().unwrap();
    assert_eq!(empty["kind"], "create");
    assert_eq!(empty["data"]["spec"]["session"], "unnamed");
    let nothing: Value = serde_json::from_slice(
        &std::fs::read(daemon.state.join(format!("jobs/{held}/env.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(nothing, json!({"vars": []}));
    let cancelled = client.command(
        &daemon,
        &["cancel", &id.to_string(), "--format", "text"],
        json!({"session": "ci"}),
    );
    assert_eq!(cancelled["outcome"], "done", "{cancelled}");
    assert_eq!(cancelled["exit_status"], 0);
    assert_eq!(cancelled["kind"], "cancel");
    assert!(cancelled["data"].is_object(), "{cancelled}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while client.state(id) != "Cancelled" {
        assert!(Instant::now() < deadline, "{}", client.state(id));
        std::thread::sleep(Duration::from_millis(50));
    }
    let audit = daemon.ok(&["audit", "--json", "--action", "cancel"]);
    assert!(audit.contains("\"cancel\""), "{audit}");
    let again = client.command(
        &daemon,
        &["submit", "--idempotency-key", "one-key", "--", "true"],
        json!({}),
    );
    let repeated = client.command(
        &daemon,
        &["submit", "--idempotency-key", "one-key", "--", "true"],
        json!({}),
    );
    assert_eq!(again["data"]["id"], repeated["data"]["id"]);
    assert_eq!(repeated["exit_status"], 0);
    assert_eq!(repeated["diagnostics"].as_array().unwrap().len(), 1);
}

#[test]
fn client_interface_command_answers_refusals_usage_errors_and_uncarried_commands_as_outcomes() {
    let daemon = Daemon::start("outcomes");
    let (mut client, _) = Client::greet(&daemon);
    let queued = client.command(&daemon, &["submit", "--", "sleep", "20"], json!({}));
    let id = queued["data"]["id"].as_u64().unwrap().to_string();
    let refused = client.command(&daemon, &["release", &id], json!({}));
    let by_hand = daemon.job(&["release", &id]);
    assert_eq!(refused["outcome"], "done", "{refused}");
    assert_eq!(refused["exit_status"], by_hand.status.code().unwrap());
    assert_ne!(refused["exit_status"], 0);
    assert_eq!(refused["kind"], Value::Null);
    assert_eq!(refused["data"], Value::Null);
    let said: Vec<&str> = refused["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        said.join("\n"),
        String::from_utf8_lossy(&by_hand.stderr).trim_end()
    );
    for words in [
        vec!["cancel"],
        vec!["submit", "--no-such-option", "--", "true"],
        vec!["submit"],
        vec!["frobnicate", "1"],
        vec!["submit", "--stdin", "--", "cat"],
        vec!["retry", &id, "--current-env"],
        vec!["queue", "create"],
    ] {
        let answer = client.command(&daemon, &words, json!({}));
        assert_eq!(answer["outcome"], "usage_error", "{words:?}: {answer}");
        assert_eq!(answer["exit_status"], 125, "{words:?}");
        assert_eq!(answer["kind"], Value::Null, "{words:?}");
        assert_eq!(answer["data"], Value::Null, "{words:?}");
        assert!(
            answer["diagnostics"][0]
                .as_str()
                .is_some_and(|line| line.starts_with("job: ")),
            "{words:?}: {answer}"
        );
    }
    let listed = client.command(
        &daemon,
        &["queue", "set", "default", "--frob", "1"],
        json!({}),
    );
    assert_eq!(listed["outcome"], "usage_error", "{listed}");
    let said = listed["diagnostics"][0].as_str().unwrap();
    for named in [
        "`--frob`",
        "job queue set",
        "--max-running",
        "--job-memory-max",
        "--parallel",
    ] {
        assert!(said.contains(named), "{named}: {said}");
    }
    assert!(!said.contains("a queue takes"), "{said}");
    let typed = daemon.job(&["queue", "set", "default", "--frob", "1"]);
    assert_eq!(typed.status.code(), Some(125));
    assert_eq!(said, String::from_utf8_lossy(&typed.stderr).trim_end());
    let by_hand = daemon.job(&["cancel"]);
    assert_eq!(by_hand.status.code(), Some(125));
    let usage = client.command(&daemon, &["cancel"], json!({}));
    assert_eq!(
        usage["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        String::from_utf8_lossy(&by_hand.stderr).trim_end()
    );
    for (words, instead) in [
        (vec!["logs", id.as_str(), "--follow"], json!("output")),
        (vec!["list"], json!("jobs")),
        (vec!["queue", "list"], json!("jobs")),
        (vec!["show", id.as_str()], json!("job")),
        (vec!["status", id.as_str()], json!("job")),
        (vec!["group", "show", "x"], json!("tree")),
        (vec!["host"], json!("totals")),
        (vec!["help"], json!("commands")),
        (vec!["attach", id.as_str()], json!("native")),
        (vec!["config", "show"], json!("native")),
        (vec!["net", "list"], json!("native")),
        (vec!["net", "show", "office"], json!("native")),
        (vec!["net"], Value::Null),
        (vec!["run", "--", "true"], Value::Null),
        (vec!["wait", id.as_str()], Value::Null),
        (vec!["queue"], Value::Null),
        (vec!["daemon"], Value::Null),
    ] {
        let answer = client.command(&daemon, &words, json!({}));
        assert_eq!(
            answer,
            json!({
                "outcome": "not_a_command_request", "exit_status": 125, "kind": null,
                "data": null, "text": [], "diagnostics": [], "use": instead,
            }),
            "{words:?}"
        );
    }
    for args in [
        json!({}),
        json!({"words": []}),
        json!({"words": "cancel 1"}),
        json!({"words": ["cancel", 1]}),
        json!({"words": ["submit", "--", "true"]}),
        json!({"words": ["submit", "--", "true"], "cwd": "relative"}),
        json!({"words": ["cancel", "1"], "cwd": 7}),
        json!({"words": ["cancel", "1"], "env": [["A", "b"]]}),
        json!({"words": ["cancel", "1"], "env": {"vars": [["A"]]}}),
        json!({"words": ["cancel", "1"], "terminal": {"rows": 0, "cols": 80}}),
        json!({"words": ["cancel", "1"], "session": 5}),
        json!({"words": ["cancel", "1"], "dry_run": "yes"}),
    ] {
        let answer = client.ask("command", args.clone());
        sentence(&answer);
        assert!(answer.get("ok").is_none(), "{args}: {answer}");
    }
    assert_ne!(client.state(1), "Cancelled");
    let stopped = client.command(&daemon, &["cancel", &id], json!({}));
    assert_eq!(stopped["exit_status"], 0, "{stopped}");
}

#[test]
fn client_interface_command_dry_run_changes_nothing() {
    let daemon = Daemon::start("dry-run");
    let (mut client, _) = Client::greet(&daemon);
    let id = daemon.held();
    let word = id.to_string();
    let before = client.state(id);
    assert_eq!(before, "Held");
    for (words, more) in [
        (vec!["cancel", word.as_str()], json!({"dry_run": true})),
        (vec!["cancel", word.as_str(), "--dry-run"], json!({})),
        (
            vec!["cancel", word.as_str(), "--dry-run"],
            json!({"dry_run": false}),
        ),
        (
            vec!["cancel", word.as_str(), "--dry-run"],
            json!({"dry_run": true}),
        ),
        (vec!["release", word.as_str()], json!({"dry_run": true})),
    ] {
        let answer = client.command(&daemon, &words, more.clone());
        assert_eq!(answer["outcome"], "done", "{words:?} {more}: {answer}");
        assert_eq!(answer["exit_status"], 0, "{words:?} {more}: {answer}");
        assert_eq!(
            answer["data"]["dry_run"], true,
            "{words:?} {more}: {answer}"
        );
        assert_eq!(client.state(id), "Held", "{words:?} {more}");
    }
    let refused = client.command(&daemon, &["submit", "--", "true"], json!({"dry_run": true}));
    assert_eq!(refused["outcome"], "usage_error", "{refused}");
    assert_eq!(refused["exit_status"], 125);
    assert_eq!(client.ok("totals", json!({}))["counts"]["held"], 1);
    let released = client.command(&daemon, &["release", &word], json!({}));
    assert_eq!(released["exit_status"], 0, "{released}");
    assert_eq!(released["kind"], "release");
    assert_ne!(client.state(id), "Held");
}

#[test]
fn client_interface_command_keeps_the_saved_environment_on_edit_and_changes_objects() {
    let daemon = Daemon::start("edit");
    let (mut client, _) = Client::greet(&daemon);
    let created = client.command(
        &daemon,
        &["create", "--", "true"],
        json!({"env": {"vars": [["KEPT", "yes"]]}}),
    );
    let id = created["data"]["id"].as_u64().unwrap();
    let word = id.to_string();
    let env = |daemon: &Daemon| -> Value {
        serde_json::from_slice(
            &std::fs::read(daemon.state.join(format!("jobs/{id}/env.json"))).unwrap(),
        )
        .unwrap()
    };
    let edited = client.command(&daemon, &["edit", &word, "--", "false"], json!({}));
    assert_eq!(edited["outcome"], "done", "{edited}");
    assert_eq!(edited["exit_status"], 0, "{edited}");
    assert_eq!(edited["kind"], "edit");
    assert_eq!(edited["data"]["spec"]["argv"], json!(["false"]));
    assert_eq!(env(&daemon), json!({"vars": [["KEPT", "yes"]]}));
    let replaced = client.command(
        &daemon,
        &["edit", &word, "--", "false"],
        json!({"env": {"vars": [["NEW", "1"]]}}),
    );
    assert_eq!(replaced["exit_status"], 0, "{replaced}");
    assert_eq!(env(&daemon), json!({"vars": [["NEW", "1"]]}));
    let made = client.command(
        &daemon,
        &["queue", "create", "builds", "--max-running", "2"],
        json!({}),
    );
    assert_eq!(made["exit_status"], 0, "{made}");
    assert_eq!(made["kind"], "queue_create");
    let by_hand: Value =
        serde_json::from_str(&daemon.ok(&["queue", "show", "builds", "--format", "json"])).unwrap();
    assert_eq!(by_hand["data"]["objects"][0]["path"], "builds");
    assert_eq!(
        by_hand["data"]["objects"][0]["object"]["config"]["max_running"],
        2
    );
    assert_eq!(
        made["data"]["objects"][0]["object"]["id"],
        by_hand["data"]["objects"][0]["object"]["id"]
    );
    let paused = client.command(&daemon, &["queue", "pause", "builds"], json!({}));
    assert_eq!(paused["exit_status"], 0, "{paused}");
    assert_eq!(paused["kind"], "queue_pause");
    assert_eq!(paused["data"]["objects"][0]["object"]["paused"], true);
    let legacy = client.command(
        &daemon,
        &["queue", "set", "builds", "--parallel", "3"],
        json!({}),
    );
    assert_eq!(legacy["outcome"], "done", "{legacy}");
    assert_eq!(legacy["exit_status"], 0, "{legacy}");
    assert_eq!(legacy["kind"], Value::Null);
    assert_eq!(legacy["data"], Value::Null);
    assert_eq!(legacy["text"].as_array().unwrap().len(), 1, "{legacy}");
    let moved = client.command(&daemon, &["move", &word, "--queue", "builds"], json!({}));
    assert_eq!(moved["exit_status"], 0, "{moved}");
    assert_eq!(moved["data"]["spec"]["queue"], "builds");
    std::fs::write(daemon.state.join(format!("jobs/{id}/env.json")), "{").unwrap();
    let unreadable = client.command(&daemon, &["edit", &word, "--", "true"], json!({}));
    assert_eq!(unreadable["outcome"], "done", "{unreadable}");
    assert_ne!(unreadable["exit_status"], 0);
    assert!(
        unreadable["diagnostics"][0]
            .as_str()
            .is_some_and(|line| line.starts_with("job: "))
    );
    assert_eq!(
        client.ok("native", json!({"request": {"Status": {"id": id}}}))["answer"]["StillRunning"]["job"]
            ["spec"]["argv"],
        json!(["false"])
    );
    let missing = client.command(&daemon, &["edit", "999", "--", "true"], json!({}));
    assert_eq!(missing["outcome"], "done", "{missing}");
    assert_ne!(missing["exit_status"], 0);
    let audit = daemon.ok(&["audit", "--json"]);
    for action in ["edit", "object-create", "object-pause", "queue-set", "move"] {
        assert!(
            audit.contains(&format!("\"{action}\"")),
            "{action}: {audit}"
        );
    }
}

#[test]
fn client_interface_command_retries_with_the_saved_or_a_given_environment() {
    let daemon = Daemon::start("retry");
    let (mut client, _) = Client::greet(&daemon);
    let first = client.command(
        &daemon,
        &["submit", "--", "true"],
        json!({"env": {"vars": [["FIRST", "1"]]}}),
    );
    let id = first["data"]["id"].as_u64().unwrap();
    let word = id.to_string();
    let ended = |client: &mut Client| {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let state = client.state(id);
            if state == "Succeeded" || state == "Finished" {
                break;
            }
            assert!(Instant::now() < deadline, "{state}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let env = |daemon: &Daemon| -> Value {
        serde_json::from_slice(
            &std::fs::read(daemon.state.join(format!("jobs/{id}/env.json"))).unwrap(),
        )
        .unwrap()
    };
    ended(&mut client);
    let kept = client.command(&daemon, &["retry", &word], json!({}));
    assert_eq!(kept["outcome"], "done", "{kept}");
    assert_eq!(kept["exit_status"], 0, "{kept}");
    assert_eq!(kept["kind"], "retry");
    assert_eq!(kept["data"]["id"], id);
    assert_eq!(kept["data"]["attempt"], 2);
    assert_eq!(env(&daemon), json!({"vars": [["FIRST", "1"]]}));
    ended(&mut client);
    let replaced = client.command(
        &daemon,
        &["retry", &word],
        json!({"env": {"vars": [["SECOND", "2"]]}}),
    );
    assert_eq!(replaced["exit_status"], 0, "{replaced}");
    assert_eq!(replaced["data"]["attempt"], 3);
    assert_eq!(env(&daemon), json!({"vars": [["SECOND", "2"]]}));
    ended(&mut client);
    let audit = daemon.ok(&["audit", "--json", "--action", "retry"]);
    assert_eq!(audit.lines().count(), 2, "{audit}");
}

#[test]
fn client_interface_native_passes_reads_and_refuses_changes() {
    let daemon = Daemon::start("native");
    let (mut client, greeting) = Client::greet(&daemon);
    let id = daemon.held();
    let attempts = client.ok("native", json!({"request": {"Attempts": {"id": id}}}));
    assert_eq!(attempts["protocol"], greeting["hello"]["native_protocol"]);
    assert_eq!(attempts["answer"]["Attempts"]["schema_version"], 1);
    let by_hand: Value = serde_json::from_str(&daemon.native(&format!(
        "{{\"Versioned\":{{\"protocol\":{0},\"min\":{0},\"request\":{{\"Attempts\":{{\"id\":{id}}}}}}}}}",
        greeting["hello"]["native_protocol"]
    )))
    .unwrap();
    assert_eq!(
        attempts["answer"]["Attempts"]["attempts"][0]["id"],
        by_hand["Attempts"]["attempts"][0]["id"]
    );
    let missing = client.ok("native", json!({"request": {"Explain": {"id": 999}}}));
    assert!(
        missing["answer"]["Error"]["message"].as_str().is_some(),
        "{missing}"
    );
    let unknown = client.ok("native", json!({"request": {"NoSuchRequest": {}}}));
    assert!(
        unknown["answer"]["Error"]["message"]
            .as_str()
            .is_some_and(|text| text.contains("NoSuchRequest")),
        "{unknown}"
    );
    let attached = client.ok("native", json!({"request": {"Attach": {"id": id}}}));
    assert!(attached["answer"].is_object(), "{attached}");
    let query = client.ok(
        "native",
        json!({"request": {"LogQuery": {"id": id, "query": ["full"]}}}),
    );
    assert!(query["answer"].is_object(), "{query}");
    for request in [
        json!({"Release": {"id": id}}),
        json!({"Cancel": {"id": id, "session": "x"}}),
        json!({"Config": {"reload": true}}),
        json!({"Done": {"id": id}}),
        json!({"Output": {"id": id, "attempt": null, "follow": false}}),
        json!({"Versioned": {"protocol": 20, "min": 20, "request": "Host"}}),
    ] {
        let answer = client.ask("native", json!({"request": request}));
        sentence(&answer);
        assert!(answer.get("ok").is_none(), "{request}: {answer}");
    }
    sentence(&client.ask("native", json!({})));
    assert_eq!(client.state(id), "Held");
    let audit = daemon.ok(&["audit", "--json"]);
    assert!(!audit.contains("\"release\""), "{audit}");
}

#[test]
fn client_interface_closes_after_a_line_over_the_limit_and_takes_one_at_the_limit() {
    let daemon = Daemon::start("line-limit");
    let (mut client, _) = Client::greet(&daemon);
    let request = "{\"id\":1,\"op\":\"totals\"}";
    let padded = format!("{request}{}", " ".repeat(LINE_IN - 1 - request.len()));
    assert_eq!(padded.len() + 1, LINE_IN);
    client.send(&padded);
    assert_eq!(client.line().unwrap()["re"], 1);
    let over = format!("{request}{}", " ".repeat(LINE_IN - request.len()));
    let _ = client.writer.write_all(over.as_bytes());
    let _ = client.writer.write_all(b"\n");
    let refusal = client.line().unwrap();
    sentence(&refusal);
    assert!(refusal.get("re").is_none(), "{refusal}");
    assert!(client.closed());
    let mut endless = Client::raw(&daemon);
    endless.send(HELLO);
    endless.line().unwrap();
    let _ = endless.writer.write_all(&vec![b'x'; LINE_IN + 1]);
    sentence(&endless.line().unwrap());
    assert!(endless.closed());
}

#[test]
fn client_interface_answers_in_order_and_keeps_the_connection_after_an_unknown_request() {
    let daemon = Daemon::start("order");
    let (mut client, _) = Client::greet(&daemon);
    client
        .writer
        .write_all(
            b"{\"id\":41,\"op\":\"commands\",\"args\":{\"language\":\"en\"}}\n{\"id\":7,\"op\":\"totals\"}\n{\"id\":41,\"op\":\"events\"}\n{\"id\":3.5,\"op\":\"totals\",\"args\":[]}\n",
        )
        .unwrap();
    let first = client.line().unwrap();
    assert_eq!(first["re"], 41);
    assert!(first["ok"]["commands"].is_array());
    let second = client.line().unwrap();
    assert_eq!(second["re"], 7);
    assert!(second["ok"]["counts"].is_object());
    let third = client.line().unwrap();
    assert_eq!(third["re"], 41);
    assert!(sentence(&third).contains("events"));
    let fourth = client.line().unwrap();
    assert_eq!(fourth["re"], 3.5);
    sentence(&fourth);
    for absent in ["audit", "attach", "frobnicate"] {
        let answer = client.ask(absent, json!({}));
        assert!(sentence(&answer).contains(absent), "{answer}");
    }
    assert!(client.ok("totals", json!({}))["counts"].is_object());
    for unreadable in [
        "not json",
        "[1]",
        "{\"id\":1}",
        "{\"op\":\"totals\"}",
        "{\"id\":\"1\",\"op\":\"totals\"}",
        "",
    ] {
        let (mut client, _) = Client::greet(&daemon);
        client.send(unreadable);
        let answer = client.line().unwrap();
        sentence(&answer);
        assert!(answer.get("re").is_none(), "{unreadable}: {answer}");
        assert!(client.closed(), "{unreadable}");
    }
}

#[test]
fn client_interface_refuses_the_thirty_third_connection_of_one_user() {
    let daemon = Daemon::start("connections");
    let mut open: Vec<Client> = (0..32).map(|_| Client::greet(&daemon).0).collect();
    let mut further = Client::raw(&daemon);
    further.send(HELLO);
    let refusal = further.line().unwrap();
    sentence(&refusal);
    assert!(refusal.get("hello").is_none());
    assert!(further.closed());
    assert!(daemon.native("\"Ping\"").contains("Pong"));
    assert!(open[31].ok("totals", json!({}))["counts"].is_object());
    open.truncate(31);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut client = Client::raw(&daemon);
        client.send(HELLO);
        if client.line().unwrap().get("hello").is_some() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
}
