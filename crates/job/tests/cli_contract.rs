use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BASH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../completions/job.bash");
const FISH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../completions/job.fish");
const SYNTAX: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/reference/syntax.md"
);

struct Daemon {
    child: Child,
    base: PathBuf,
    state: PathBuf,
    work: PathBuf,
}

impl Daemon {
    fn start(name: &str) -> Daemon {
        let base = std::env::temp_dir().join(format!("job-cli-{}-{name}", std::process::id()));
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
        let mut command = plain(args);
        command
            .env("JOB_STATE_DIR", &self.state)
            .env("JOB_SESSION", "cli-contract")
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
            out(&output),
            err(&output)
        );
        out(&output)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn plain(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_job"));
    command
        .args(args)
        .env("LC_ALL", "C")
        .env_remove("LANG")
        .env_remove("LC_MESSAGES")
        .env_remove("JOB_CLI_COMPAT")
        .env(
            "JOB_STATE_DIR",
            std::env::temp_dir().join(format!("job-cli-{}-absent", std::process::id())),
        );
    command
}

fn offline(args: &[&str]) -> Output {
    plain(args).output().unwrap()
}

fn out(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn err(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|error| panic!("{error}: {text}"))
}

fn complete(script: &str, words: &str, state: &std::path::Path) -> Vec<String> {
    let program = format!(
        "job() {{ \"$JOB_BINARY\" \"$@\"; }}; source \"$1\"; COMP_WORDS=({words}); COMP_CWORD=$((${{#COMP_WORDS[@]}}-1)); _job_complete; printf '%s\\n' \"${{COMPREPLY[@]}}\""
    );
    let output = Command::new("bash")
        .args(["-c", &program, "bash", script])
        .env("JOB_BINARY", env!("CARGO_BIN_EXE_job"))
        .env("JOB_STATE_DIR", state)
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", err(&output));
    out(&output)
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

#[test]
fn contract_top_level_help_fits_one_screen() {
    let bare = offline(&[]);
    assert_eq!(bare.status.code(), Some(125));
    assert!(out(&bare).is_empty());
    for args in [&["help"][..], &["--help"], &["-h"]] {
        let output = offline(args);
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        let text = out(&output);
        assert_eq!(text.trim_end(), err(&bare).trim_end());
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.len() <= 24, "{} lines", lines.len());
        assert!(lines.iter().all(|line| line.chars().count() <= 80));
        assert_eq!(lines[0], "job - execute and schedule jobs");
        assert_eq!(lines[2], "Usage: job [OPTIONS] COMMAND [ARGS...]");
        assert!(lines.iter().any(|line| {
            line.starts_with("Execution:")
                && line.contains("run, submit")
                && line.contains("wait, cancel")
        }));
        assert!(lines.contains(&"Interaction:    attach (for Jobs started with --pty)"));
        assert!(lines.iter().any(|line| line.starts_with("Inspection:")
            && line.contains("list, show")
            && line.contains("explain, logs")));
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("Organization:") && line.contains("queue, group"))
        );
        assert!(lines.iter().any(|line| line.starts_with("Administration:")
            && line.contains("config")
            && line.contains("completion")));
        assert_eq!(
            lines[lines.len() - 1],
            "See 'job COMMAND --help' or 'man job'."
        );
        for hidden in ["daemon", "shim", "remote", "hook", "classify", "policy"] {
            assert!(!text.contains(hidden), "{hidden}");
        }
    }
}

#[test]
fn contract_help_is_translated_and_complete() {
    let german = plain(&["help"])
        .env("LC_ALL", "de_DE.UTF-8")
        .output()
        .unwrap();
    let text = out(&german);
    assert!(text.starts_with("job - Jobs ausführen und einplanen\n"));
    assert!(text.contains("Aufruf: job [OPTIONEN] BEFEHL [ARGUMENTE...]"));
    assert!(text.contains("run, submit"));
    let run = plain(&["run", "--help"])
        .env("LC_ALL", "de_DE.UTF-8")
        .output()
        .unwrap();
    assert!(out(&run).contains("einen Befehl ausführen und auf ihn warten"));
    assert!(out(&run).contains("Rückgabewert:"));
    let missing = offline(&["__complete", "untranslated"]);
    assert_eq!(out(&missing), "");
    let long = offline(&["help", "--all"]);
    assert!(long.status.success());
    assert!(out(&long).starts_with("usage:\n  job run [options] -- COMMAND..."));
    assert!(out(&long).lines().count() > 100);
}

#[test]
fn contract_command_help_needs_no_service() {
    let cases: [(&[&str], &str); 9] = [
        (
            &["run", "--help"],
            "Usage: job run [OPTIONS] -- COMMAND [ARG...]",
        ),
        (
            &["run", "--queue", "builds", "-h", "--", "true"],
            "Usage: job run [OPTIONS] -- COMMAND [ARG...]",
        ),
        (
            &["queue", "set", "--help"],
            "Usage: job queue set [OPTIONS] PATH",
        ),
        (&["group", "--help"], "job group COMMAND [ARGS...]"),
        (&["wait", "5", "--help"], "Usage: job wait [OPTIONS] ID"),
        (&["logs", "-h"], "Usage: job logs [OPTIONS] ID"),
        (
            &["help", "queue", "set"],
            "Usage: job queue set [OPTIONS] PATH",
        ),
        (&["completion", "--help"], "Usage: job completion SHELL"),
        (&["explain", "--help"], "Usage: job explain [OPTIONS] ID"),
    ];
    for (args, usage) in cases {
        let output = offline(args);
        assert_eq!(output.status.code(), Some(0), "{args:?}: {}", err(&output));
        let text = out(&output);
        assert!(text.contains(usage), "{args:?}: {text}");
        assert!(text.contains("Exit status:"), "{args:?}");
        assert!(text.contains("-h, --help"), "{args:?}");
    }
    let run = out(&offline(&["run", "--help"]));
    for line in [
        "  0    the Job succeeded",
        "  N    the exit status of the Job itself; 128+S when signal S ended it",
        "  1    the Job was cancelled or stopped by the service",
        "  75   the deadline passed while the Job was pending or running, or the service stayed unavailable",
        "  125  service failure, lost Job, start error or usage error",
    ] {
        assert!(run.contains(line), "{line}");
    }
    let after = offline(&["run", "--", "true", "--help"]);
    assert_eq!(after.status.code(), Some(125));
    assert!(err(&after).contains("daemon"));
    let value = offline(&["run", "--shell", "-h", "--", "true"]);
    assert_eq!(value.status.code(), Some(125));
    assert!(!out(&value).contains("Usage:"));
}

#[test]
fn contract_unknown_options_have_one_message() {
    let commands: [&[&str]; 31] = [
        &["wait"],
        &["logs"],
        &["retry"],
        &["suspend"],
        &["continue"],
        &["remove"],
        &["queue", "cancel"],
        &["group", "cancel"],
        &["queue", "suspend"],
        &["group", "continue"],
        &["queue", "remove"],
        &["attempts"],
        &["explain"],
        &["signal"],
        &["status"],
        &["show"],
        &["list"],
        &["host"],
        &["run"],
        &["submit"],
        &["create"],
        &["edit"],
        &["queue", "create"],
        &["queue", "set"],
        &["queue", "show"],
        &["queue", "list"],
        &["group", "set"],
        &["group", "rename"],
        &["group", "update"],
        &["update"],
        &["release"],
    ];
    for path in commands {
        let name = path.join(" ");
        let mut args = path.to_vec();
        args.extend(["--no-such-option", "7"]);
        let output = offline(&args);
        assert_eq!(output.status.code(), Some(125), "{name}");
        assert_eq!(
            err(&output),
            format!(
                "job: unknown option `--no-such-option` for job {name}; see job {name} --help\n"
            )
        );
        assert_eq!(out(&output), "");
    }
    let missing = offline(&["wait", "7", "--timeout"]);
    assert_eq!(
        err(&missing),
        "job: option `--timeout` of job wait needs a value; see job wait --help\n"
    );
    let ends = offline(&["remove", "--", "7"]);
    assert_eq!(
        err(&ends),
        "job: job remove takes no `--`; see job remove --help\n"
    );
    let german = plain(&["explain", "7", "--no-such-option"])
        .env("LC_ALL", "de_DE.UTF-8")
        .output()
        .unwrap();
    assert_eq!(
        err(&german),
        "job: unbekannte Option `--no-such-option` für job explain; siehe job explain --help\n"
    );
    let compatible = plain(&["status", "7", "--no-such-option"])
        .env("JOB_CLI_COMPAT", "legacy")
        .output()
        .unwrap();
    assert!(err(&compatible).contains("unknown option `--no-such-option` for job status"));
    let ended = offline(&["wait", "--", "7"]);
    assert!(err(&ended).contains("daemon") || ended.status.code() == Some(75));
}

#[test]
fn contract_table_options_reach_the_parsers() {
    for (path, tail) in [
        (&["run"][..], &["--", "true"][..]),
        (&["submit"], &["--", "true"]),
        (&["queue", "set"], &["target"]),
        (&["group", "create"], &["target"]),
    ] {
        let mut query = vec!["__complete", "options"];
        query.extend(path);
        let listed = out(&offline(&query));
        assert!(listed.lines().count() > 20, "{path:?}: {listed}");
        for line in listed.lines() {
            let (option, arity) = line.split_once('\t').unwrap();
            if option == "--help" {
                continue;
            }
            let mut args = path.to_vec();
            args.push(option);
            if arity == "value" {
                args.push(if option == "--format" { "json" } else { "1" });
            }
            args.extend(tail);
            let output = offline(&args);
            let message = err(&output);
            for refusal in [
                "unknown option",
                "unknown setting",
                "needs a value",
                "cannot read",
            ] {
                assert!(!message.contains(refusal), "{args:?}: {message}");
            }
        }
    }
    let run = out(&offline(&["__complete", "options", "run"]));
    for option in [
        "--cpu-affinity\tvalue",
        "--numa-policy\tvalue",
        "--rlimit\tvalue",
        "--no-new-privs\tvalue",
        "--cap-drop\tvalue",
        "--seccomp-deny\tvalue",
        "--pty\tflag",
        "--queue\tvalue",
    ] {
        assert!(run.lines().any(|line| line == option), "{option}");
    }
    let set = out(&offline(&["__complete", "options", "queue", "set"]));
    for option in [
        "--job-cpu-affinity\tvalue",
        "--job-rlimit\tvalue",
        "--job-seccomp-deny\tvalue",
        "--job-memory-max\tvalue",
        "--job-io-max\tvalue",
        "--max-running\tvalue",
    ] {
        assert!(set.lines().any(|line| line == option), "{option}");
    }
}

#[test]
fn contract_completion_scripts_are_generated() {
    for (shell, path) in [("bash", BASH), ("fish", FISH)] {
        let output = offline(&["completion", shell]);
        assert!(output.status.success());
        let german = plain(&["completion", shell])
            .env("LC_ALL", "de_DE.UTF-8")
            .output()
            .unwrap();
        assert_eq!(out(&output), out(&german));
        assert_eq!(
            out(&output),
            std::fs::read_to_string(path).unwrap(),
            "write {path} again with: job completion {shell}"
        );
    }
    assert!(
        Command::new("bash")
            .args(["-n", BASH])
            .status()
            .unwrap()
            .success()
    );
    let refused = offline(&["completion", "zsh"]);
    assert_eq!(refused.status.code(), Some(125));
    assert_eq!(
        err(&refused),
        "job: job completion cannot read `zsh`; see job completion --help\n"
    );
    let absent = std::env::temp_dir().join(format!("job-cli-{}-absent", std::process::id()));
    assert_eq!(complete(BASH, "job ru", &absent), ["run"]);
    assert_eq!(
        complete(BASH, "job queue s", &absent),
        ["show", "set", "suspend"]
    );
    assert_eq!(
        complete(BASH, "job run --net ''", &absent),
        ["default", "none"]
    );
    assert_eq!(
        complete(BASH, "job logs --stream s", &absent),
        ["stdout", "stderr"]
    );
    assert_eq!(
        complete(BASH, "job run --cap-drop sys_a", &absent),
        ["sys_admin"]
    );
    assert_eq!(
        complete(BASH, "job queue set --job-seccomp-deny ptr", &absent),
        ["ptrace"]
    );
    assert_eq!(complete(BASH, "job signal -s K", &absent), ["KILL"]);
    assert_eq!(complete(BASH, "job screenshot --p", &absent), ["--pid"]);
    assert_eq!(
        complete(BASH, "job completion ''", &absent),
        ["bash", "fish"]
    );
    assert_eq!(complete(BASH, "job wait ''", &absent), Vec::<String>::new());
    assert_eq!(
        complete(BASH, "job run -- ''", &absent),
        Vec::<String>::new()
    );
}

#[test]
fn contract_dynamic_completion_is_bounded() {
    let down = Instant::now();
    for kind in ["job", "queue", "group", "object", "nothing"] {
        let output = offline(&["__complete", kind, ""]);
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(out(&output), "");
        assert_eq!(err(&output), "");
    }
    assert!(down.elapsed() < Duration::from_secs(5));
    let daemon = Daemon::start("complete");
    daemon.ok(&["group", "create", "development"]);
    daemon.ok(&["queue", "create", "--group", "development", "builds"]);
    let held = daemon.ok(&["create", "--queue", "development/builds", "--", "true"]);
    let held = held.trim();
    assert_eq!(daemon.ok(&["__complete", "job", ""]).trim(), held);
    assert_eq!(daemon.ok(&["__complete", "job", held]).trim(), held);
    assert_eq!(daemon.ok(&["__complete", "job", "x"]), "");
    let queues = daemon.ok(&["__complete", "queue", ""]);
    assert!(queues.lines().any(|line| line == "development/builds"));
    assert_eq!(
        daemon.ok(&["__complete", "queue", "dev"]),
        "development/builds\n"
    );
    assert_eq!(daemon.ok(&["__complete", "group", "dev"]), "development\n");
    let objects = daemon.ok(&["__complete", "object", "dev"]);
    assert_eq!(objects, "development\ndevelopment/builds\n");
    assert_eq!(complete(BASH, "job wait ''", &daemon.state), [held]);
    assert_eq!(
        complete(BASH, "job run --queue dev", &daemon.state),
        ["development/builds"]
    );
    assert_eq!(
        complete(BASH, "job queue show dev", &daemon.state),
        ["development/builds"]
    );
    assert_eq!(
        complete(BASH, "job queue move --group dev", &daemon.state),
        ["development"]
    );
    for index in 0..230 {
        daemon.ok(&["queue", "create", &format!("many{index:03}")]);
    }
    let many = daemon.ok(&["__complete", "queue", "many"]);
    assert_eq!(many.lines().count(), 200);
    std::fs::create_dir_all(daemon.base.join("stalled")).unwrap();
    let stalled =
        std::os::unix::net::UnixListener::bind(daemon.base.join("stalled/daemon.sock")).unwrap();
    let started = Instant::now();
    let output = plain(&["__complete", "job", ""])
        .env("JOB_STATE_DIR", daemon.base.join("stalled"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(out(&output), "");
    assert!(started.elapsed() >= Duration::from_millis(200));
    assert!(started.elapsed() < Duration::from_secs(3));
    drop(stalled);
}

#[test]
fn contract_list_and_show_are_aliases() {
    let daemon = Daemon::start("inspect");
    daemon.ok(&["queue", "create", "builds"]);
    let empty = daemon.ok(&["list"]);
    assert_eq!(empty, "no jobs are queued or running\n");
    let held = daemon.ok(&["create", "--queue", "builds", "--", "true"]);
    let held = held.trim();
    let other = daemon.ok(&["create", "--", "true"]);
    let other = other.trim();
    let all = daemon.ok(&["list"]);
    assert_eq!(all.lines().count(), 2);
    assert!(all.lines().all(|line| line.contains("held")));
    let queued = daemon.ok(&["queue"]);
    for line in all.lines() {
        assert!(queued.lines().any(|known| known == line), "{line}");
    }
    let builds = daemon.ok(&["list", "--queue", "builds"]);
    assert_eq!(builds.lines().count(), 1);
    assert!(builds.trim_start().starts_with(held));
    assert_eq!(daemon.ok(&["list", "-q", "/builds"]), builds);
    let listed = json(&daemon.ok(&["list", "--json"]));
    assert_eq!(listed["schema_version"], 1);
    let mut ids: Vec<String> = listed["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["job"]["id"].to_string())
        .collect();
    ids.sort();
    let mut expected = [held.to_owned(), other.to_owned()];
    expected.sort();
    assert_eq!(ids, expected);
    let enveloped = json(&daemon.ok(&["list", "--queue", "builds", "--format", "json"]));
    assert_eq!(enveloped["kind"], "list");
    assert_eq!(enveloped["data"]["jobs"][0]["job"]["id"].to_string(), held);
    let absent = daemon.job(&["list", "--queue", "nowhere"]);
    assert_eq!(absent.status.code(), Some(125));
    assert_eq!(err(&absent), "job: there is no Queue `nowhere`\n");
    let operand = daemon.job(&["list", "builds"]);
    assert_eq!(
        err(&operand),
        "job: job list takes no operand `builds`; see job list --help\n"
    );
    let status = daemon.job(&["status", held, "--json"]);
    let show = daemon.job(&["show", held, "--json"]);
    assert_eq!(show.status.code(), status.status.code());
    assert_eq!(show.stdout, status.stdout);
    let reversed = daemon.job(&["show", "--json", held]);
    assert_eq!(reversed.stdout, status.stdout);
    let text = daemon.job(&["show", held]);
    assert_eq!(text.stdout, daemon.job(&["status", held]).stdout);
    let twice = daemon.job(&["show", held, other]);
    assert_eq!(
        err(&twice),
        "job: job show takes exactly one ID; see job show --help\n"
    );
}

#[test]
fn contract_format_json_wraps_the_same_data() {
    let daemon = Daemon::start("format");
    daemon.ok(&["queue", "create", "builds"]);
    let id = daemon.ok(&["submit", "--queue", "builds", "--", "true"]);
    let id = id.trim();
    let plain_wait = daemon.job(&["wait", "--json", id]);
    assert_eq!(plain_wait.status.code(), Some(0), "{}", err(&plain_wait));
    let wait = out(&plain_wait);
    assert!(wait.starts_with("{\"error\":null,\"exit_status\":0,\"job\":{"));
    assert!(wait.ends_with(",\"outcome\":\"completed\",\"schema_version\":1}\n"));
    let wrapped = daemon.job(&["wait", "--format", "json", id]);
    assert_eq!(wrapped.status.code(), Some(0));
    assert_eq!(
        out(&wrapped),
        format!(
            "{{\"schema_version\":1,\"kind\":\"wait\",\"data\":{}}}\n",
            wait.trim_end()
        )
    );
    assert_eq!(out(&daemon.job(&["wait", "--json", id])), wait);
    assert_eq!(
        out(&daemon.job(&["wait", "--format", "json", "--json", id])),
        out(&wrapped)
    );
    assert_eq!(out(&daemon.job(&["wait", "--format", "text", id])), "");

    let attempts = out(&daemon.job(&["attempts", id, "--json"]));
    assert!(attempts.starts_with("{\"attempts\":[{"));
    assert!(attempts.ends_with("],\"schema_version\":1}\n"));
    assert_eq!(
        out(&daemon.job(&["attempts", id, "--format", "json"])),
        format!(
            "{{\"schema_version\":1,\"kind\":\"attempts\",\"data\":{}}}\n",
            attempts.trim_end()
        )
    );

    let show = out(&daemon.job(&["queue", "show", "builds", "--json"]));
    assert!(show.starts_with("{\n  \"objects\": [\n    {\n"));
    assert!(show.ends_with("  ],\n  \"schema_version\": 1\n}\n"));
    assert_eq!(
        out(&daemon.job(&["queue", "show", "builds", "--json"])),
        show
    );
    let wrapped = out(&daemon.job(&["queue", "show", "builds", "--format", "json"]));
    assert_eq!(wrapped.lines().count(), 1);
    assert!(wrapped.starts_with("{\"schema_version\":1,\"kind\":\"queue_show\",\"data\":{"));
    assert_eq!(json(&wrapped)["data"], json(&show));

    let status = json(&out(&daemon.job(&["status", id, "--format", "json"])));
    assert_eq!(status["schema_version"], 1);
    assert_eq!(status["kind"], "status");
    assert_eq!(status["data"]["id"].to_string(), id);
    let run = daemon.job(&["run", "--format", "json", "--", "sh", "-c", "exit 3"]);
    assert_eq!(run.status.code(), Some(3));
    let run = json(&out(&run));
    assert_eq!(run["kind"], "run");
    assert_eq!(run["data"]["outcome"], "completed");
    assert_eq!(run["data"]["exit_status"], 3);

    let usage = daemon.job(&["wait", "--format", "json", id, "--no-such-option"]);
    assert_eq!(usage.status.code(), Some(125));
    let envelope = json(&out(&usage));
    assert_eq!(envelope["kind"], "wait");
    assert_eq!(envelope["data"]["outcome"], "usage_error");
    assert_eq!(envelope["data"]["exit_status"], 125);
    assert_eq!(envelope["data"]["job"], serde_json::Value::Null);
    assert!(
        envelope["data"]["error"]
            .as_str()
            .unwrap()
            .contains("--no-such-option")
    );
    let word = json(&out(&daemon.job(&["wait", "--format", "json", "word"])));
    assert_eq!(word["data"]["outcome"], "usage_error");
    let empty = json(&out(&daemon.job(&["run", "--format", "json", "--"])));
    assert_eq!(empty["kind"], "run");
    assert_eq!(empty["data"]["outcome"], "usage_error");
    let unknown = daemon.job(&["wait", "--format", "json", "999999"]);
    assert_eq!(unknown.status.code(), Some(125));
    assert_eq!(json(&out(&unknown))["data"]["outcome"], "service_error");
    let attempts = daemon.job(&["attempts", "999999", "--format", "json"]);
    assert_eq!(attempts.status.code(), Some(125));
    let envelope = json(&out(&attempts));
    assert_eq!(envelope["kind"], "error");
    assert_eq!(envelope["data"]["command"], "attempts");
    assert_eq!(envelope["data"]["outcome"], "service_error");
    assert_eq!(envelope["data"]["exit_status"], 125);
    assert!(!err(&attempts).is_empty());
    let format = daemon.job(&["wait", "--format", "xml", id]);
    assert_eq!(format.status.code(), Some(125));
    assert_eq!(
        err(&format),
        "job: `xml` is not an output format of job wait; write text or json\n"
    );
    assert_eq!(out(&format), "");
    let literal = daemon.job(&["run", "--", "echo", "--format", "json"]);
    assert_eq!(literal.status.code(), Some(0), "{}", err(&literal));
    assert!(out(&literal).contains("--format json"));

    let down = offline(&["status", "7", "--format", "json"]);
    assert_eq!(down.status.code(), Some(125));
    let envelope = json(&out(&down));
    assert_eq!(envelope["kind"], "error");
    assert_eq!(envelope["data"]["outcome"], "service_error");
}

#[test]
fn contract_syntax_reference_is_generated() {
    let output = offline(&["help", "--syntax"]);
    assert!(output.status.success());
    let text = out(&output);
    assert_eq!(
        text,
        std::fs::read_to_string(SYNTAX).unwrap(),
        "write {SYNTAX} again with: job help --syntax"
    );
    for heading in [
        "### job run",
        "### job queue set",
        "### job group cancel",
        "### job completion",
        "### job daemon",
        "### job queue add",
    ] {
        assert!(text.lines().any(|line| line == heading), "{heading}");
    }
    assert!(!text.contains("### job shim"));
    assert!(!text.contains("### job __complete"));
    assert!(text.contains("| 75 | the change is still pending |"));
    assert!(text.contains("| 125 | service failure, lost Job, start error or usage error |"));
}

fn field(line: &str, index: usize) -> &str {
    line.split('\t').nth(index).unwrap_or("")
}

#[test]
fn cli2_list_shows_completed_jobs_as_table_json_and_tsv() {
    let daemon = Daemon::start("cli2-list");
    let done = daemon.ok(&["submit", "--", "true"]);
    let done = done.trim();
    assert_eq!(daemon.job(&["wait", done]).status.code(), Some(0));
    let failed = daemon.ok(&["submit", "--", "sh", "-c", "exit 3"]);
    let failed = failed.trim();
    assert_eq!(daemon.job(&["wait", failed]).status.code(), Some(3));
    let held = daemon.ok(&["create", "--", "true"]);
    let held = held.trim();

    let active = daemon.ok(&["list"]);
    assert_eq!(active.lines().count(), 1);
    assert!(active.trim_start().starts_with(held));

    let all = daemon.ok(&["list", "--all"]);
    let lines: Vec<&str> = all.lines().collect();
    assert_eq!(lines.len(), 4, "{all}");
    assert!(lines[0].starts_with("ID") && lines[0].contains("STATE"));
    assert!(lines[1].starts_with(done) && lines[1].contains("succeeded"));
    assert!(lines[2].starts_with(failed) && lines[2].contains("failed"));
    assert!(lines[3].starts_with(held) && lines[3].contains("held"));

    let one = daemon.job(&["list", "--state", "succeeded,failed", "--limit", "1"]);
    assert_eq!(one.status.code(), Some(0));
    assert_eq!(out(&one).lines().count(), 2);
    assert!(out(&one).lines().nth(1).unwrap().starts_with(failed));
    assert!(err(&one).contains("more Jobs match than were listed"));

    let enveloped = json(&daemon.ok(&["list", "--all", "--format", "json"]));
    assert_eq!(enveloped["kind"], "list");
    assert_eq!(enveloped["data"]["jobs"].as_array().unwrap().len(), 3);
    assert_eq!(enveloped["data"]["more"], false);
    assert_eq!(enveloped["data"]["jobs"][1]["job"]["state"], "Failed");
    let plain_json = json(&daemon.ok(&["list", "--state", "held", "--json"]));
    assert_eq!(plain_json["jobs"][0]["job"]["id"].to_string(), held);

    let tsv = daemon.ok(&["list", "--all", "--format", "tsv"]);
    let rows: Vec<&str> = tsv.lines().collect();
    assert_eq!(
        rows[0],
        "id\tattempt\tstate\tqueue\tsession\tpriority\tsubmitted_ms\tstarted_ms\tfinished_ms\texit_status\tlabels\tcommand"
    );
    assert_eq!(rows.len(), 4);
    assert!(rows.iter().all(|row| row.split('\t').count() == 12));
    assert_eq!(field(rows[2], 0), failed);
    assert_eq!(field(rows[2], 2), "failed");
    assert_eq!(field(rows[2], 3), "default");
    assert_eq!(field(rows[2], 9), "3");
    assert_eq!(
        json(field(rows[2], 11)),
        serde_json::json!(["sh", "-c", "exit 3"])
    );
    assert_eq!(field(rows[3], 9), "");
    let german = daemon
        .command(&["list", "--all", "--format", "tsv"])
        .env("LC_ALL", "de_DE.UTF-8")
        .output()
        .unwrap();
    assert_eq!(out(&german), tsv);
    let objects = daemon.ok(&["queue", "list", "--format", "tsv"]);
    assert_eq!(
        objects,
        "id\tpath\tkind\tpaused\tclosed\tlabels\n2\tdefault\tqueue\tfalse\tfalse\t{}\n"
    );
    let groups = daemon.ok(&["group", "list", "--format", "tsv"]);
    assert!(groups.starts_with("id\tpath\tkind\tpaused\tclosed\tlabels\n1\t/\tgroup\t"));

    let state = daemon.job(&["list", "--state", "done", "--format", "json"]);
    assert_eq!(state.status.code(), Some(125));
    assert_eq!(json(&out(&state))["data"]["outcome"], "usage_error");
    let both = daemon.job(&["list", "--all", "--state", "held"]);
    assert_eq!(
        err(&both),
        "job: --all and --state are mutually exclusive\n"
    );
    let limit = daemon.job(&["list", "--limit", "0"]);
    assert_eq!(limit.status.code(), Some(125));
    assert!(err(&limit).contains("--limit: `0` is not a number from 1 through 10000"));
    let format = daemon.job(&["list", "--format", "xml"]);
    assert_eq!(
        err(&format),
        "job: `xml` is not an output format of job list; write text, json or tsv\n"
    );
    let mixed = daemon.job(&["list", "--format", "tsv", "--json"]);
    assert_eq!(mixed.status.code(), Some(125));

    assert_eq!(daemon.ok(&["__complete", "job", ""]).trim(), held);
    let mut any: Vec<String> = daemon
        .ok(&["__complete", "anyjob", ""])
        .lines()
        .map(str::to_owned)
        .collect();
    any.sort();
    let mut expected = vec![done.to_owned(), failed.to_owned(), held.to_owned()];
    expected.sort();
    assert_eq!(any, expected);
    let ended = daemon.ok(&["__complete", "ended", ""]);
    assert!(ended.lines().any(|line| line == done));
    assert!(ended.lines().any(|line| line == failed));
    assert!(!ended.lines().any(|line| line == held));
    for words in ["job show ''", "job logs ''", "job attempts ''"] {
        let mut offered = complete(BASH, words, &daemon.state);
        offered.sort();
        assert_eq!(offered, expected, "{words}");
    }
    let mut retried = complete(BASH, "job retry ''", &daemon.state);
    retried.sort();
    let mut finished = vec![done.to_owned(), failed.to_owned()];
    finished.sort();
    assert_eq!(retried, finished);
    assert_eq!(complete(BASH, "job wait ''", &daemon.state), [held]);
}

#[test]
fn cli2_move_keeps_identity_and_waiting_credit() {
    let daemon = Daemon::start("cli2-move");
    daemon.ok(&[
        "queue",
        "create",
        "source",
        "--max-running",
        "1",
        "--aging",
        "1s",
    ]);
    daemon.ok(&["queue", "create", "target"]);
    daemon.ok(&["queue", "create", "shut"]);
    daemon.ok(&["queue", "create", "narrow", "--priority-max", "5"]);
    daemon.ok(&["queue", "close", "shut"]);
    daemon.ok(&["queue", "pause", "target"]);
    let running = daemon.ok(&["submit", "-q", "source", "--", "sleep", "20"]);
    let running = running.trim();
    let waiting = daemon.ok(&["submit", "-q", "source", "--priority", "9", "--", "true"]);
    let waiting = waiting.trim();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let credit = json(&daemon.ok(&["explain", waiting, "--json"]))["eligible_wait_ms"]
            .as_u64()
            .unwrap();
        if credit >= 200 {
            break;
        }
        assert!(Instant::now() < deadline, "no waiting credit accrued");
        std::thread::sleep(Duration::from_millis(50));
    }
    daemon.ok(&["queue", "pause", "source"]);
    let before = json(&daemon.ok(&["explain", waiting, "--json"]));
    let record = json(&daemon.ok(&["list", "--state", "queued", "--json"]));
    let record = &record["jobs"][0]["job"];
    assert!(before["eligible_wait_ms"].as_u64().unwrap() >= 200);

    let refused = daemon.job(&["move", running, "--queue", "target"]);
    assert_eq!(refused.status.code(), Some(125));
    assert_eq!(
        err(&refused),
        format!("job: Job {running} is running; only held and queued Jobs can be moved\n")
    );
    let absent = daemon.job(&["move", waiting, "--queue", "nowhere"]);
    assert_eq!(err(&absent), "job: there is no Queue `nowhere`\n");
    let closed = daemon.job(&["move", waiting, "--queue", "shut"]);
    assert_eq!(closed.status.code(), Some(125));
    assert!(
        err(&closed).contains("takes no new jobs"),
        "{}",
        err(&closed)
    );
    let bound = daemon.job(&["move", waiting, "--queue", "narrow"]);
    assert_eq!(bound.status.code(), Some(125));
    assert!(err(&bound).contains("priority"), "{}", err(&bound));
    let usage = daemon.job(&["move", waiting, "--format", "json"]);
    assert_eq!(usage.status.code(), Some(125));
    assert_eq!(json(&out(&usage))["data"]["outcome"], "usage_error");
    assert_eq!(
        json(&daemon.ok(&["explain", waiting, "--json"]))["eligible_wait_ms"],
        before["eligible_wait_ms"]
    );

    assert_eq!(
        daemon.ok(&["move", waiting, "--queue", "target"]),
        format!("Job {waiting} is in Queue target\n")
    );
    let after = json(&daemon.ok(&["explain", waiting, "--json"]));
    assert_eq!(after["eligible_wait_ms"], before["eligible_wait_ms"]);
    assert_eq!(after["state"], "Queued");
    assert_eq!(after["attempt"], before["attempt"]);
    let moved = json(&daemon.ok(&["list", "--state", "queued", "--json"]));
    let moved = &moved["jobs"][0]["job"];
    assert_eq!(moved["id"].to_string(), waiting);
    assert_eq!(moved["spec"]["queue"], "target");
    assert_eq!(moved["submitted_ms"], record["submitted_ms"]);
    assert_eq!(moved["released_ms"], record["released_ms"]);
    assert_eq!(
        moved["attempt_submitted_ms"],
        record["attempt_submitted_ms"]
    );
    assert_eq!(moved["spec"]["declared"]["priority"], 9);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        json(&daemon.ok(&["explain", waiting, "--json"]))["eligible_wait_ms"],
        before["eligible_wait_ms"]
    );

    let held = daemon.ok(&["create", "-q", "source", "--", "true"]);
    let held = held.trim();
    let wrapped = json(&daemon.ok(&["move", held, "-q", "target", "--format", "json"]));
    assert_eq!(wrapped["kind"], "move");
    assert_eq!(wrapped["data"]["state"], "Held");
    assert_eq!(wrapped["data"]["spec"]["queue"], "target");
    assert_eq!(
        daemon.ok(&["move", held, "-q", "target"]),
        format!("Job {held} is in Queue target\n")
    );

    let audit = daemon.ok(&["audit", "--action", "move"]);
    assert!(
        audit.lines().any(|line| {
            field(line, 2) == "move" && field(line, 3) == waiting && field(line, 7) == "ok"
        }),
        "{audit}"
    );
    assert!(
        audit
            .lines()
            .any(|line| { field(line, 3) == running && field(line, 7).starts_with("error: ") })
    );
    daemon.ok(&["cancel", running]);
    assert_eq!(daemon.job(&["wait", running]).status.code(), Some(1));
    let ended = daemon.job(&["move", running, "--queue", "target"]);
    assert_eq!(
        err(&ended),
        format!("job: Job {running} is cancelled; only held and queued Jobs can be moved\n")
    );
}

#[test]
fn cli2_labels_classify_jobs_queues_and_groups() {
    let daemon = Daemon::start("cli2-labels");
    daemon.ok(&["group", "create", "team", "--label", "owner=build"]);
    daemon.ok(&[
        "queue",
        "create",
        "team/fast",
        "--label",
        "tier=1",
        "--label",
        "owner=ci",
    ]);
    daemon.ok(&["queue", "create", "slow"]);
    daemon.ok(&["queue", "pause", "team/fast"]);
    let first = daemon.ok(&[
        "submit",
        "-q",
        "team/fast",
        "--label",
        "kind=build",
        "--label",
        "note=a b,c=d",
        "--",
        "true",
    ]);
    let first = first.trim();
    let second = daemon.ok(&["create", "--label", "kind=test", "--", "true"]);
    let second = second.trim();

    let status = json(&out(&daemon.job(&["show", second, "--json"])));
    assert_eq!(status["spec"]["declared"]["labels"]["kind"], "test");
    let shown = daemon.job(&["show", first]);
    assert!(
        out(&shown).contains("labels: kind=build, note=a b,c=d\n"),
        "{}",
        out(&shown)
    );
    let unlabelled = daemon.ok(&["create", "--", "true"]);
    let plain_show = daemon.job(&["show", unlabelled.trim()]);
    assert!(!out(&plain_show).contains("labels"));
    assert!(
        json(&out(&daemon.job(&["show", unlabelled.trim(), "--json"])))["spec"]["declared"]
            .get("labels")
            .is_none()
    );

    let builds = daemon.ok(&["list", "--label", "kind=build"]);
    assert_eq!(builds.lines().count(), 2);
    assert!(builds.lines().nth(1).unwrap().starts_with(first));
    assert!(builds.contains("kind=build,note=a b,c=d"));
    let both = daemon.ok(&[
        "list",
        "--label",
        "kind=build",
        "--label",
        "note=a b,c=d",
        "--format",
        "tsv",
    ]);
    assert_eq!(both.lines().count(), 2);
    assert_eq!(
        json(field(both.lines().nth(1).unwrap(), 10)),
        serde_json::json!({"kind": "build", "note": "a b,c=d"})
    );
    assert_eq!(
        daemon.ok(&["list", "--label", "kind=none"]),
        "no Jobs match\n"
    );

    let fast = json(&daemon.ok(&["queue", "show", "team/fast", "--json"]));
    assert_eq!(
        fast["objects"][0]["object"]["config"]["labels"]["tier"],
        "1"
    );
    assert!(fast["objects"][0]["effective"].get("labels").is_none());
    let queues = daemon.ok(&["queue", "list", "--label", "tier=1"]);
    assert_eq!(queues.lines().count(), 1);
    assert!(queues.contains("\tteam/fast\t"));
    let queues = json(&daemon.ok(&["queue", "list", "--label", "tier=1", "--format", "json"]));
    assert_eq!(queues["kind"], "queue_list");
    assert_eq!(queues["data"]["objects"].as_array().unwrap().len(), 1);
    let groups = daemon.ok(&["group", "list", "--label", "owner=build", "--format", "tsv"]);
    assert_eq!(groups.lines().count(), 2);
    assert_eq!(field(groups.lines().nth(1).unwrap(), 1), "team");
    assert_eq!(
        field(groups.lines().nth(1).unwrap(), 5),
        "{\"owner\":\"build\"}"
    );

    daemon.ok(&[
        "queue",
        "set",
        "team/fast",
        "--label",
        "tier=2",
        "--label",
        "extra=x",
    ]);
    daemon.ok(&["queue", "unset", "team/fast", "label-owner"]);
    let fast = json(&daemon.ok(&["queue", "show", "team/fast", "--json"]));
    assert_eq!(
        fast["objects"][0]["object"]["config"]["labels"],
        serde_json::json!({"extra": "x", "tier": "2"})
    );
    daemon.ok(&["queue", "resume", "team/fast"]);
    assert_eq!(daemon.job(&["wait", first]).status.code(), Some(0));
    let fast = json(&daemon.ok(&["queue", "show", "team/fast", "--json"]));
    assert_eq!(
        fast["objects"][0]["object"]["config"]["labels"]["tier"],
        "2"
    );
    daemon.ok(&["queue", "unset", "team/fast", "labels"]);
    let fast = json(&daemon.ok(&["queue", "show", "team/fast", "--json"]));
    assert!(
        fast["objects"][0]["object"]["config"]
            .get("labels")
            .is_none()
    );

    let long = format!("k={}", "v".repeat(256));
    let upper = daemon.job(&["submit", "--label", "Kind=x", "--", "true"]);
    assert_eq!(upper.status.code(), Some(125));
    assert!(err(&upper).contains("`Kind=x` is not a label"));
    for bad in ["novalue", "=x", long.as_str(), "k=a\tb"] {
        let refused = daemon.job(&["queue", "set", "slow", "--label", bad]);
        assert_eq!(refused.status.code(), Some(125), "{bad}");
        assert!(err(&refused).contains("is not a label"), "{bad}");
    }
    let twice = daemon.job(&["submit", "--label", "a=1", "--label", "a=2", "--", "true"]);
    assert_eq!(err(&twice), "job: label `a` is given twice\n");
    let mut many = vec!["queue".to_owned(), "set".to_owned(), "slow".to_owned()];
    for index in 0..33 {
        many.extend(["--label".to_owned(), format!("k{index}=v")]);
    }
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let refused = daemon.job(&many);
    assert_eq!(refused.status.code(), Some(125));
    assert_eq!(err(&refused), "job: an object carries at most 32 labels\n");
    daemon.ok(&many[..many.len() - 2]);
    let over = daemon.job(&["queue", "set", "slow", "--label", "one-more=v"]);
    assert!(err(&over).contains("an object carries at most 32 labels"));
    let help = daemon.ok(&["run", "--help"]);
    assert!(help.contains("it never affects scheduling or policy"));
}

struct Attached {
    child: Child,
    stdout: std::io::BufReader<std::process::ChildStdout>,
    stderr: std::io::BufReader<std::process::ChildStderr>,
}

impl Attached {
    fn start(daemon: &Daemon, args: &[&str]) -> Attached {
        let mut child = daemon
            .command(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let stderr = std::io::BufReader::new(child.stderr.take().unwrap());
        Attached {
            child,
            stdout,
            stderr,
        }
    }

    fn out_line(&mut self) -> String {
        let mut line = String::new();
        std::io::BufRead::read_line(&mut self.stdout, &mut line).unwrap();
        line
    }

    fn err_line(&mut self) -> String {
        let mut line = String::new();
        std::io::BufRead::read_line(&mut self.stderr, &mut line).unwrap();
        line
    }

    fn signal(&self, signal: i32) {
        assert_eq!(unsafe { libc::kill(self.child.id() as i32, signal) }, 0);
    }

    fn end(mut self) -> (std::process::ExitStatus, String, String) {
        let mut out = String::new();
        let mut err = String::new();
        self.stdout.read_to_string(&mut out).unwrap();
        self.stderr.read_to_string(&mut err).unwrap();
        (self.child.wait().unwrap(), out, err)
    }
}

fn states(daemon: &Daemon) -> Vec<(String, String)> {
    daemon
        .ok(&["list", "--all", "--format", "tsv"])
        .lines()
        .skip(1)
        .map(|line| (field(line, 0).to_owned(), field(line, 2).to_owned()))
        .collect()
}

fn state_of(daemon: &Daemon, id: &str) -> String {
    states(daemon)
        .into_iter()
        .find(|(known, _)| known == id)
        .map(|(_, state)| state)
        .unwrap_or_default()
}

fn newest(daemon: &Daemon) -> String {
    states(daemon).last().unwrap().0.clone()
}

fn settled(daemon: &Daemon, id: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let state = state_of(daemon, id);
        if ["succeeded", "failed", "cancelled", "lost"].contains(&state.as_str()) {
            return state;
        }
        assert!(Instant::now() < deadline, "job {id} stays {state}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

const IGNORES: &str = "trap '' INT TERM HUP; echo ready; sleep 20";

#[test]
fn cli2_run_forwards_the_first_interrupt_and_detaches_on_the_second() {
    use std::os::unix::process::ExitStatusExt;
    let daemon = Daemon::start("cli2-signal");
    let mut run = Attached::start(
        &daemon,
        &[
            "run",
            "--budget",
            "30s",
            "--",
            "sh",
            "-c",
            "trap 'echo caught; exit 7' INT; echo ready; i=0; while [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done",
        ],
    );
    assert_eq!(run.out_line(), "ready\n");
    let id = newest(&daemon);
    run.signal(libc::SIGINT);
    let (status, out, err) = run.end();
    assert_eq!(status.code(), Some(7), "{err}");
    assert_eq!(out, "caught\n");
    assert_eq!(
        err,
        format!(
            "[job] SIGINT forwarded to Job {id}; interrupt again to leave it running and return\n"
        )
    );
    assert_eq!(settled(&daemon, &id), "failed");

    let mut run = Attached::start(
        &daemon,
        &["run", "--budget", "30s", "--", "sh", "-c", IGNORES],
    );
    assert_eq!(run.out_line(), "ready\n");
    let id = newest(&daemon);
    run.signal(libc::SIGTERM);
    assert_eq!(
        run.err_line(),
        format!(
            "[job] SIGTERM forwarded to Job {id}; interrupt again to leave it running and return\n"
        )
    );
    assert_eq!(state_of(&daemon, &id), "running");
    run.signal(libc::SIGINT);
    let (status, _, err) = run.end();
    assert_eq!(status.code(), Some(75));
    assert_eq!(
        err,
        format!(
            "[job] interrupted; Job {id} goes on; wait for it with job wait {id}, cancel it with job cancel {id}\n"
        )
    );
    assert_eq!(state_of(&daemon, &id), "running");
    daemon.ok(&["cancel", &id]);

    let mut run = Attached::start(
        &daemon,
        &["run", "--budget", "30s", "--", "sh", "-c", IGNORES],
    );
    assert_eq!(run.out_line(), "ready\n");
    let id = newest(&daemon);
    run.signal(libc::SIGHUP);
    let (status, _, err) = run.end();
    assert_eq!(status.code(), Some(75));
    assert!(
        err.contains(&format!("the terminal hung up; Job {id} goes on")),
        "{err}"
    );
    assert_eq!(state_of(&daemon, &id), "running");
    daemon.ok(&["cancel", &id]);

    let mut run = Attached::start(
        &daemon,
        &[
            "run",
            "--budget",
            "30s",
            "--on-interrupt",
            "detach",
            "--",
            "sh",
            "-c",
            IGNORES,
        ],
    );
    assert_eq!(run.out_line(), "ready\n");
    let id = newest(&daemon);
    run.signal(libc::SIGTERM);
    let (status, _, err) = run.end();
    assert_eq!(status.code(), Some(75));
    assert!(err.starts_with("[job] interrupted; Job "), "{err}");
    assert_eq!(state_of(&daemon, &id), "running");
    daemon.ok(&["cancel", &id]);

    let mut run = Attached::start(
        &daemon,
        &[
            "run",
            "--budget",
            "30s",
            "--on-interrupt",
            "cancel",
            "--",
            "sh",
            "-c",
            "echo ready; sleep 20",
        ],
    );
    assert_eq!(run.out_line(), "ready\n");
    let id = newest(&daemon);
    run.signal(libc::SIGINT);
    let (status, _, err) = run.end();
    assert_eq!(status.code(), Some(1), "{err}");
    assert!(err.contains(&format!("cancelling Job {id}")), "{err}");
    assert_eq!(settled(&daemon, &id), "cancelled");

    let run = Attached::start(
        &daemon,
        &[
            "run",
            "--budget",
            "30s",
            "--json",
            "--on-interrupt",
            "detach",
            "--",
            "sh",
            "-c",
            IGNORES,
        ],
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !states(&daemon).iter().any(|(_, state)| state == "running") {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let id = newest(&daemon);
    run.signal(libc::SIGINT);
    let (status, out, _) = run.end();
    assert_eq!(status.code(), Some(75));
    let result = json(&out);
    assert_eq!(result["outcome"], "detached");
    assert_eq!(result["exit_status"], 75);
    assert_eq!(result["job_id"].to_string(), id);
    assert_eq!(state_of(&daemon, &id), "running");

    for (command, signal) in [
        (vec!["wait", id.as_str()], libc::SIGINT),
        (vec!["logs", "--follow", id.as_str()], libc::SIGTERM),
    ] {
        let mut waiter = Attached::start(&daemon, &command);
        if command[0] == "logs" {
            assert_eq!(waiter.out_line(), "ready\n");
        } else {
            std::thread::sleep(Duration::from_millis(300));
        }
        waiter.signal(signal);
        let (status, _, _) = waiter.end();
        assert_eq!(status.signal(), Some(signal), "{command:?}");
        assert_eq!(state_of(&daemon, &id), "running", "{command:?}");
    }
    daemon.ok(&["cancel", &id]);

    let mut run = Attached::start(
        &daemon,
        &[
            "run",
            "--budget",
            "30s",
            "--",
            "sh",
            "-c",
            "i=0; while [ $i -lt 200 ]; do echo line; sleep 0.05; i=$((i+1)); done",
        ],
    );
    assert_eq!(run.out_line(), "line\n");
    let id = newest(&daemon);
    let Attached {
        mut child,
        stdout,
        mut stderr,
    } = run;
    drop(stdout);
    let mut err = String::new();
    stderr.read_to_string(&mut err).unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(75));
    assert!(
        err.contains(&format!("the output was closed; Job {id} goes on")),
        "{err}"
    );
    assert_eq!(state_of(&daemon, &id), "running");
    daemon.ok(&["cancel", &id]);

    let bad = daemon.job(&["run", "--on-interrupt", "sometimes", "--", "true"]);
    assert_eq!(bad.status.code(), Some(125));
    assert_eq!(
        crate::err(&bad),
        "job: --on-interrupt: `sometimes` is not a choice; write forward, detach or cancel\n"
    );
    let submit = daemon.job(&["submit", "--on-interrupt", "forward", "--", "true"]);
    assert!(crate::err(&submit).contains("unknown option `--on-interrupt` for job submit"));
}

#[test]
fn cli2_explain_names_the_object_that_supplies_or_blocks_a_setting() {
    let daemon = Daemon::start("cli2-explain");
    daemon.ok(&[
        "group",
        "create",
        "team",
        "--cores",
        "1",
        "--mem",
        "1G",
        "--max-running",
        "2",
    ]);
    daemon.ok(&[
        "queue",
        "create",
        "team/fast",
        "--max-running",
        "unlimited",
        "--cores",
        "4",
        "--job-cpu-request",
        "2",
        "--job-memory-max",
        "2G",
        "--aging",
        "1s",
    ]);
    let all = json(&daemon.ok(&["explain", "team/fast", "--format", "json"]));
    assert_eq!(all["kind"], "explain");
    assert_eq!(all["data"]["path"], "team/fast");
    assert_eq!(all["data"]["kind"], "queue");
    let entry = |key: &str| {
        all["data"]["settings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["key"] == key)
            .unwrap_or_else(|| panic!("{key}: {all}"))
            .clone()
    };
    let cores = entry("cores_milli");
    assert_eq!(cores["option"], "cores");
    assert_eq!(cores["configured"], 4000);
    assert_eq!(cores["effective"], 1000);
    assert_eq!(cores["source"], "team");
    assert_eq!(cores["takes_effect"], false);
    assert_eq!(
        cores["reasons"][0],
        "ancestor team sets a lower cores: 1000; the lower one applies"
    );
    let running = entry("max_running");
    assert_eq!(running["configured"], "unlimited");
    assert_eq!(running["effective"], 2);
    assert_eq!(
        running["reasons"][0],
        "unlimited here, but ancestor team still bounds this subtree: max-running 2"
    );
    let memory = entry("memory");
    assert_eq!(memory["configured"], serde_json::Value::Null);
    assert_eq!(memory["effective"], 1u64 << 30);
    assert_eq!(memory["source"], "team");
    assert_eq!(memory["takes_effect"], true);
    let aging = entry("aging_ms");
    assert_eq!(aging["source"], "team/fast");
    assert_eq!(aging["takes_effect"], true);
    let request = entry("job_cpu_request_milli");
    assert_eq!(
        request["reasons"][0],
        "a Job with this request is never admitted: the admission budget cores at team is 1000"
    );
    let limit = entry("job_memory_max");
    assert_eq!(limit["effective"], 2u64 << 30);
    assert!(
        limit["reasons"][0]
            .as_str()
            .unwrap()
            .starts_with("this host cannot apply memory.max:"),
        "{limit}"
    );

    let one = daemon.ok(&["explain", "team/fast", "max-running"]);
    assert_eq!(
        one,
        "Queue team/fast (#4)\nmax-running: configured unlimited; effective 2; supplied by team\n  unlimited here, but ancestor team still bounds this subtree: max-running 2\n"
    );
    let unset = daemon.ok(&["explain", "team", "job-memory-high"]);
    assert_eq!(
        unset,
        "Group team (#3)\njob-memory-high: configured not set; effective not set\n"
    );
    let german = daemon
        .command(&["explain", "team/fast", "cores", "--json"])
        .env("LC_ALL", "de_DE.UTF-8")
        .output()
        .unwrap();
    assert_eq!(json(&out(&german))["settings"][0]["key"], "cores_milli");
    let absent = daemon.job(&["explain", "nowhere"]);
    assert_eq!(absent.status.code(), Some(125));
    assert_eq!(err(&absent), "job: there is no Queue or Group `nowhere`\n");

    daemon.ok(&["queue", "create", "team/slow"]);
    let waiting = daemon.ok(&[
        "submit",
        "-q",
        "team/slow",
        "--cpu-request",
        "0.5",
        "--memory-request",
        "512M",
        "--",
        "true",
    ]);
    let waiting = waiting.trim();
    assert_eq!(daemon.job(&["wait", waiting]).status.code(), Some(0));
    daemon.ok(&["queue", "pause", "team/slow"]);
    let blocked = daemon.ok(&[
        "submit",
        "-q",
        "team/slow",
        "--cpu-request",
        "0.5",
        "--memory-request",
        "512M",
        "--",
        "true",
    ]);
    let blocked = blocked.trim();
    daemon.ok(&["group", "set", "team", "--mem", "100M"]);
    daemon.ok(&["queue", "resume", "team/slow"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    let reason = loop {
        let explanation = json(&daemon.ok(&["explain", blocked, "--json"]));
        let reason = explanation["blocking_reason"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        if reason.contains("admission budget") {
            assert_eq!(explanation["state"], "Queued");
            break reason;
        }
        assert!(Instant::now() < deadline, "{explanation}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(reason, "memory admission budget 104857600 at team");
    let text = daemon.ok(&["explain", blocked]);
    assert!(
        text.contains("memory admission budget 104857600 at team"),
        "{text}"
    );
    let listed = daemon.ok(&["list", "--queue", "team/slow"]);
    assert!(
        listed.contains("waiting for memory admission budget 104857600 at team"),
        "{listed}"
    );
    let budget = daemon.ok(&["explain", "team/slow", "mem"]);
    assert!(
        budget.contains("effective 104857600; supplied by team"),
        "{budget}"
    );
}

#[test]
fn cli2_process_count_and_write_budget_have_their_own_names() {
    let daemon = Daemon::start("cli2-pids");
    let host = json(&daemon.ok(&["host", "--json"]));
    assert_eq!(host["resource_controls"]["pids.max"], false);
    assert_eq!(host["filesystem_quota"]["supported"], false);
    assert!(
        host["filesystem_quota"]["reason"]
            .as_str()
            .unwrap()
            .starts_with("the service sets no file system quota; --write-budget counts")
    );

    let refused = daemon.job(&["submit", "--pids-max", "64", "--", "true"]);
    assert_eq!(refused.status.code(), Some(125));
    assert_eq!(
        err(&refused),
        "job: requested resource control is unavailable: pids.max\n"
    );
    for bad in ["0", "many", "-1"] {
        let refused = daemon.job(&["submit", "--pids-max", bad, "--", "true"]);
        assert_eq!(
            err(&refused),
            "job: pids-max requires a positive whole number or unlimited\n",
            "{bad}"
        );
    }
    let earlier = daemon.ok(&[
        "create",
        "--pids",
        "32",
        "--write-budget",
        "1M",
        "--",
        "true",
    ]);
    let record = json(&out(&daemon.job(&["show", earlier.trim(), "--json"])));
    assert_eq!(record["spec"]["declared"]["pids"], 32);
    assert_eq!(record["spec"]["declared"]["pids_max"], 32);
    assert_eq!(record["resource_sources"]["pids_max"], "Compatibility");
    assert_eq!(record["reservation"]["vector"]["pids"], 32);
    assert_eq!(record["spec"]["declared"]["disk"], 1 << 20);
    let disk = daemon.ok(&["create", "--disk", "1M", "--", "true"]);
    let disk = json(&out(&daemon.job(&["show", disk.trim(), "--json"])));
    assert_eq!(disk["spec"]["declared"]["disk"], 1 << 20);
    let conflict = daemon.job(&["submit", "--pids", "32", "--pids-max", "64", "--", "true"]);
    assert_eq!(conflict.status.code(), Some(125));
    assert!(
        err(&conflict)
            .contains("legacy resource option conflicts with an explicit value: pids_max"),
        "{}",
        err(&conflict)
    );

    daemon.ok(&["queue", "create", "bounded", "--job-pids-max", "10"]);
    let shown = daemon.ok(&["explain", "bounded", "job-pids-max"]);
    assert!(shown.contains("job-pids-max: configured 10; effective 10; supplied by bounded"));
    assert!(
        shown.contains("this host cannot apply pids.max:"),
        "{shown}"
    );
    let inherited = daemon.job(&["submit", "-q", "bounded", "--", "true"]);
    assert_eq!(inherited.status.code(), Some(125));
    assert!(err(&inherited).contains("pids.max"));
    let subtree = daemon.job(&["group", "create", "capped", "--pids-max", "100"]);
    assert_eq!(subtree.status.code(), Some(125));
    assert!(err(&subtree).contains("pids.max"), "{}", err(&subtree));
    let help = daemon.ok(&["run", "--help"]);
    assert!(help.contains("--pids-max N|unlimited"));
    assert!(help.contains("--write-budget SIZE"));
    assert!(help.contains("earlier spelling of --write-budget"));
}

#[test]
fn cli2_structured_output_covers_control_commands_and_usage_errors() {
    let daemon = Daemon::start("cli2-format");
    daemon.ok(&["queue", "create", "builds"]);
    daemon.ok(&["group", "create", "team"]);

    let plain_id = daemon.ok(&["submit", "--json", "--", "true"]);
    assert!(plain_id.trim().parse::<u64>().is_ok(), "{plain_id}");
    let submitted = json(&daemon.ok(&["submit", "--format", "json", "--", "true"]));
    assert_eq!(submitted["kind"], "submit");
    assert_eq!(
        submitted["data"]["spec"]["argv"],
        serde_json::json!(["true"])
    );
    assert!(submitted["data"]["id"].is_u64());

    let held = daemon.ok(&["create", "-q", "builds", "--", "true"]);
    let held = held.trim();
    let released = json(&daemon.ok(&["release", held, "--format", "json"]));
    assert_eq!(released["kind"], "release");
    assert_eq!(released["data"]["id"].to_string(), held);
    assert!(released["data"]["released_ms"].is_u64());
    let quiet = daemon.ok(&["create", "--", "true"]);
    assert_eq!(daemon.ok(&["release", quiet.trim()]), "");

    let paused = json(&daemon.ok(&["queue", "pause", "builds", "--format", "json"]));
    assert_eq!(paused["kind"], "queue_pause");
    assert_eq!(paused["data"]["objects"][0]["path"], "builds");
    assert_eq!(paused["data"]["objects"][0]["object"]["paused"], true);
    let resumed = json(&daemon.ok(&["queue", "resume", "--format", "json", "builds"]));
    assert_eq!(resumed["kind"], "queue_resume");
    assert_eq!(resumed["data"]["objects"][0]["object"]["paused"], false);
    assert!(
        daemon
            .ok(&["queue", "pause", "builds"])
            .starts_with("queue builds changed: ")
    );
    assert!(
        daemon
            .ok(&["queue", "resume", "builds"])
            .starts_with("queue builds changed: ")
    );
    let group = json(&daemon.ok(&["group", "pause", "team", "--format", "json"]));
    assert_eq!(group["kind"], "group_pause");
    assert_eq!(group["data"]["objects"][0]["object"]["paused"], true);
    daemon.ok(&["group", "resume", "team"]);

    let first = daemon.ok(&["create", "--", "true"]);
    let cancelled = daemon.job(&["cancel", first.trim(), "--format", "json"]);
    let envelope = json(&out(&cancelled));
    assert_eq!(envelope["kind"], "cancel", "{envelope}");
    assert!(envelope["data"].is_object(), "{envelope}");
    let second = daemon.ok(&["create", "--", "true"]);
    let leading = daemon.job(&[
        "cancel",
        "--format",
        "json",
        "--session",
        "other",
        second.trim(),
    ]);
    assert_eq!(json(&out(&leading))["kind"], "cancel");
    let third = daemon.ok(&["create", "--", "true"]);
    let text = daemon.job(&["cancel", third.trim(), "--session", "other"]);
    assert_eq!(text.status.code(), Some(0), "{}", err(&text));
    assert!(!out(&text).trim_start().starts_with('{'));
    let unknown = daemon.job(&["cancel", "7", "--cores", "2"]);
    assert_eq!(
        err(&unknown),
        "job: unknown option `--cores` for job cancel; see job cancel --help\n"
    );
    let missing = json(&out(&daemon.job(&["cancel", "999999", "--format", "json"])));
    assert_eq!(missing["data"]["outcome"], "service_error");

    let doctor = daemon.job(&["doctor", "--format", "json"]);
    let report = json(&out(&doctor));
    assert_eq!(report["kind"], "doctor");
    assert!(report["data"]["checks"].is_array());
    assert_eq!(
        err(&daemon.job(&["doctor", "--verbose"])),
        "job: unknown option `--verbose` for job doctor; see job doctor --help\n"
    );

    let audit = json(&daemon.ok(&["audit", "--action", "create", "--format", "json"]));
    assert_eq!(audit["kind"], "audit");
    assert!(audit["data"]["entries"].as_array().unwrap().len() >= 4);
    assert_eq!(audit["data"]["entries"][0]["action"], "create");
    assert_eq!(audit["data"]["unreadable"], 0);
    let stream = daemon.ok(&["audit", "--action", "create", "--json"]);
    assert!(stream.lines().count() >= 4);
    assert!(stream.lines().all(|line| json(line)["action"] == "create"));
    assert_eq!(
        err(&daemon.job(&["audit", "--colour"])),
        "job: unknown option `--colour` for job audit; see job audit --help\n"
    );
    assert_eq!(
        err(&offline(&["state", "validate", "--colour"])),
        "job: unknown option `--colour` for job state validate; see job state validate --help\n"
    );

    let id = daemon.ok(&["create", "--", "true"]);
    let id = id.trim();
    let refusals: [&[&str]; 18] = [
        &["cancel"],
        &["release"],
        &["retry"],
        &["retry", "word"],
        &["remove"],
        &["suspend"],
        &["continue", "--timeout", "soon", id],
        &["attempts"],
        &["explain"],
        &["update"],
        &["update", id, "--cpu-limit", "much"],
        &["resource-update"],
        &["show"],
        &["move", id],
        &["queue", "show"],
        &["queue", "rename", "builds"],
        &["group", "move", "team"],
        &["audit", "--since", "yesterday"],
    ];
    for words in refusals {
        let mut args = words.to_vec();
        args.extend(["--format", "json"]);
        let output = daemon.job(&args);
        assert_eq!(output.status.code(), Some(125), "{words:?}");
        let envelope = json(&out(&output));
        assert_eq!(
            envelope["data"]["outcome"], "usage_error",
            "{words:?}: {envelope}"
        );
        assert!(!err(&output).is_empty(), "{words:?}");
    }
    let refused = json(&out(&daemon.job(&["retry", id, "--format", "json"])));
    assert_eq!(refused["data"]["outcome"], "service_error", "{refused}");
    let logs = daemon.ok(&["logs", "--help"]);
    assert!(logs.contains("--json prints a stream of records, one JSON object per line"));
}

fn piped(daemon: &Daemon, args: &[&str], input: &[u8]) -> Output {
    let mut child = daemon
        .command(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let output = child.wait_with_output().unwrap();
    writer.join().unwrap();
    output
}

#[test]
fn cli2_run_passes_stdin_only_when_asked() {
    let daemon = Daemon::start("cli2-stdin");
    let lines = piped(
        &daemon,
        &["run", "--stdin", "--", "cat"],
        b"one\ntwo\nthree\n",
    );
    assert_eq!(lines.status.code(), Some(0), "{}", err(&lines));
    assert_eq!(out(&lines), "one\ntwo\nthree\n");
    let record = json(&daemon.ok(&["list", "--all", "--json"]));
    assert_eq!(record["jobs"][0]["job"]["spec"]["declared"]["stdin"], true);

    let payload: Vec<u8> = (0..300_000u32).map(|index| (index % 251) as u8).collect();
    let counted = piped(&daemon, &["run", "--stdin", "--", "wc", "-c"], &payload);
    assert_eq!(counted.status.code(), Some(0), "{}", err(&counted));
    assert_eq!(out(&counted).trim(), "300000");

    let without = piped(
        &daemon,
        &["run", "--", "sh", "-c", "cat; echo end"],
        b"unseen\n",
    );
    assert_eq!(without.status.code(), Some(0), "{}", err(&without));
    assert_eq!(out(&without), "end\n");
    let record = json(&daemon.ok(&["list", "--all", "--json"]));
    assert!(
        record["jobs"][2]["job"]["spec"]["declared"]
            .get("stdin")
            .is_none()
    );

    let empty = daemon
        .command(&["run", "--stdin", "--", "sh", "-c", "cat; echo end"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(empty.status.code(), Some(0), "{}", err(&empty));
    assert_eq!(out(&empty), "end\n");

    let summary = piped(
        &daemon,
        &[
            "run",
            "--stdin",
            "--json",
            "--",
            "sh",
            "-c",
            "read word; test \"$word\" = go",
        ],
        b"go\n",
    );
    assert_eq!(summary.status.code(), Some(0), "{}", err(&summary));

    let remote = daemon.job(&["run", "--stdin", "--on", "nobody@localhost", "--", "cat"]);
    assert_eq!(remote.status.code(), Some(125));
    assert_eq!(
        err(&remote),
        "job: --stdin is not carried to another host; leave out --on\n"
    );
    let submit = daemon.job(&["submit", "--stdin", "--", "cat"]);
    assert_eq!(
        err(&submit),
        "job: unknown option `--stdin` for job submit; see job submit --help\n"
    );
}

#[test]
fn cli2_records_in_the_earlier_finished_state_list_by_their_exit_status() {
    let daemon = Daemon::start("cli2-finished");
    let good = daemon.ok(&["submit", "--", "true"]);
    let good = good.trim();
    assert_eq!(daemon.job(&["wait", good]).status.code(), Some(0));
    let bad = daemon.ok(&["submit", "--", "sh", "-c", "exit 4"]);
    let bad = bad.trim();
    assert_eq!(daemon.job(&["wait", bad]).status.code(), Some(4));
    for (id, state) in [(good, "Succeeded"), (bad, "Failed")] {
        let path = daemon.state.join("jobs").join(id).join("job.json");
        let text = std::fs::read_to_string(&path).unwrap();
        let known = format!("\"state\": \"{state}\"");
        assert!(text.contains(&known), "{text}");
        std::fs::write(&path, text.replace(&known, "\"state\": \"Finished\"")).unwrap();
    }
    let rows = daemon.ok(&["list", "--all", "--format", "tsv"]);
    let rows: Vec<&str> = rows.lines().skip(1).collect();
    assert_eq!((field(rows[0], 0), field(rows[0], 2)), (good, "succeeded"));
    assert_eq!((field(rows[1], 0), field(rows[1], 2)), (bad, "failed"));
    assert_eq!(field(rows[1], 9), "4");
    let failed = daemon.ok(&["list", "--state", "failed", "--format", "tsv"]);
    assert_eq!(failed.lines().count(), 2);
    let listed = json(&daemon.ok(&["list", "--all", "--json"]));
    assert_eq!(listed["jobs"][0]["job"]["state"], "Succeeded");
    assert_eq!(listed["jobs"][1]["job"]["state"], "Failed");
    assert_eq!(
        json(&out(&daemon.job(&["show", bad, "--json"])))["state"],
        "Failed"
    );
}

fn closed(daemon: &Daemon, args: &[&str]) -> (Option<i32>, String) {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let output = daemon
        .command(args)
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    (output.status.code(), err(&output))
}

fn first_line_only(daemon: &Daemon, args: &[&str]) -> (Option<i32>, String, usize) {
    let mut child = daemon
        .command(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut first = Vec::new();
    let mut byte = [0u8; 1];
    while stdout.read(&mut byte).unwrap() == 1 && byte[0] != b'\n' {
        first.push(byte[0]);
    }
    drop(stdout);
    let output = child.wait_with_output().unwrap();
    (output.status.code(), err(&output), first.len())
}

#[test]
fn contract_a_closed_output_ends_every_command_with_125_and_without_a_panic() {
    let daemon = Daemon::start("closed-output");
    daemon.ok(&["queue", "create", "parked"]);
    daemon.ok(&["queue", "pause", "parked"]);
    let long = "x".repeat(2000);
    for _ in 0..40 {
        daemon.ok(&["submit", "-q", "parked", "--", "echo", &long]);
    }
    let id = daemon.ok(&["submit", "--", "sh", "-c", "seq 1 20000"]);
    let id = id.trim();
    assert!(daemon.job(&["wait", id]).status.success());
    let commands: [&[&str]; 9] = [
        &["list", "--all", "--format", "json"],
        &["list"],
        &["status", id, "--json"],
        &["logs", id],
        &["logs", id, "--raw"],
        &["audit", "--json"],
        &["events"],
        &["host", "--json"],
        &["help", "--all"],
    ];
    for args in commands {
        let whole = daemon.job(args);
        assert!(whole.status.success(), "{args:?}: {}", err(&whole));
        assert!(!whole.stdout.is_empty(), "{args:?}");
        let (code, stderr) = closed(&daemon, args);
        assert!(!stderr.contains("panicked"), "{args:?}: {stderr}");
        assert_eq!(code, Some(125), "{args:?}: {stderr}");
    }
    for args in [&["list", "--all", "--json"][..], &["logs", id, "--raw"]] {
        assert!(daemon.job(args).stdout.len() > 100_000, "{args:?}");
        let (code, stderr, read) = first_line_only(&daemon, args);
        assert!(read > 0, "{args:?}");
        assert!(!stderr.contains("panicked"), "{args:?}: {stderr}");
        assert_eq!(code, Some(125), "{args:?}: {stderr}");
    }
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let refused = daemon
        .command(&["status", "999999"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(writer)
        .status()
        .unwrap();
    assert_eq!(refused.code(), Some(125));
    let run = daemon.job(&["run", "--", "echo", "still-works"]);
    assert_eq!(out(&run), "still-works\n");
}

#[test]
fn contract_doctor_warns_that_systemd_is_unverified() {
    let daemon = Daemon::start("doctor-systemd");
    let check = |inside: bool| {
        let mut command = daemon.command(&["doctor", "--json"]);
        if inside {
            command.env("INVOCATION_ID", "0123456789abcdef0123456789abcdef");
        } else {
            command.env_remove("INVOCATION_ID");
        }
        let report = json(&out(&command.output().unwrap()));
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["name"] == "service_manager")
            .cloned()
            .unwrap()
    };
    let inside = check(true);
    assert_eq!(inside["status"], "warn", "{inside}");
    assert!(
        inside["detail"]
            .as_str()
            .unwrap()
            .contains("this release was never run under systemd"),
        "{inside}"
    );
    assert!(
        inside["fix"]
            .as_str()
            .unwrap()
            .contains("systemctl restart jobd"),
        "{inside}"
    );
    if !std::path::Path::new("/run/systemd/system").is_dir() {
        assert_eq!(check(false)["status"], "ok");
    }
}

const IDSET_COMMANDS: [&[&str]; 10] = [
    &["cancel"],
    &["remove"],
    &["release"],
    &["retry"],
    &["suspend"],
    &["continue"],
    &["signal"],
    &["reprioritize", "--priority", "5"],
    &["move", "--queue", "default"],
    &["wait"],
];

fn idset_args<'a>(command: &[&'a str], rest: &[&'a str]) -> Vec<&'a str> {
    command.iter().chain(rest).copied().collect()
}

#[test]
fn idset_a_malformed_set_is_a_usage_error_before_the_service_is_asked() {
    for command in IDSET_COMMANDS {
        for (set, reason) in [
            (
                "10-1",
                "`10-1` is a reversed range; write the lower ID first",
            ),
            ("5-", "`5-` is an open range; write both ends, as in 5-10"),
            (
                "1,,3",
                "`1,,3` has an empty item; write IDs and ranges with one comma between them",
            ),
            (
                "0,2",
                "`0` is not a Job ID; write a positive number, a range such as 1-10 or a list such as 1,4,7-9",
            ),
            (
                "2,x",
                "`x` is not a Job ID; write a positive number, a range such as 1-10 or a list such as 1,4,7-9",
            ),
            (
                "1-10001",
                "the set names more than 10000 IDs; split it into several calls",
            ),
        ] {
            let output = offline(&idset_args(command, &[set]));
            assert_eq!(output.status.code(), Some(125), "{command:?} {set}");
            assert_eq!(out(&output), "", "{command:?} {set}");
            assert_eq!(
                err(&output),
                format!("job: {reason}\n"),
                "{command:?} {set}"
            );
        }
        let output = offline(&idset_args(command, &["1-10000", "10001"]));
        assert_eq!(
            err(&output),
            "job: the set names more than 10000 IDs; split it into several calls\n",
            "{command:?}"
        );
        let output = offline(&idset_args(command, &["3-1", "--format", "json"]));
        assert_eq!(output.status.code(), Some(125), "{command:?}");
        let envelope = json(&out(&output));
        assert_eq!(envelope["schema_version"], 1, "{command:?}");
        let data = &envelope["data"];
        assert_eq!(data["outcome"], "usage_error", "{command:?}: {envelope}");
        assert_eq!(data["exit_status"], 125, "{command:?}");
    }
    let output = plain(&["cancel", "3-1"])
        .env("LC_ALL", "de_DE.UTF-8")
        .output()
        .unwrap();
    assert_eq!(
        err(&output),
        "job: `3-1` ist ein umgekehrter Bereich; schreib die kleinere ID zuerst\n"
    );
    let output = offline(&["list", "--id", "3-1"]);
    assert_eq!(output.status.code(), Some(125));
    assert_eq!(
        err(&output),
        "job: `3-1` is a reversed range; write the lower ID first\n"
    );
}

#[test]
fn idset_a_well_formed_set_reaches_the_service() {
    for command in IDSET_COMMANDS {
        for sets in [
            &["1-10"][..],
            &["1,4,7-9"],
            &["1-3", "8", "12-14"],
            &["7", "7"],
            &["1-10000"],
        ] {
            let output = offline(&idset_args(command, sets));
            assert_eq!(output.status.code(), Some(125), "{command:?} {sets:?}");
            assert!(
                err(&output).contains("the daemon does not answer"),
                "{command:?} {sets:?}: {}",
                err(&output)
            );
        }
    }
    let unknown = offline(&["cancel", "1-3", "--cores", "2"]);
    assert_eq!(
        err(&unknown),
        "job: unknown option `--cores` for job cancel; see job cancel --help\n"
    );
    let summary = offline(&["wait", "1-3", "--summary"]);
    assert_eq!(summary.status.code(), Some(125));
    assert_eq!(
        err(&summary),
        "job: --summary shows the output of one Job; leave it out when waiting for a set\n"
    );
    let bare = offline(&["move", "1-3"]);
    assert_eq!(
        err(&bare),
        "job: job move needs --queue PATH; see job move --help\n"
    );
    let bare = offline(&["reprioritize", "1-3"]);
    assert_eq!(
        err(&bare),
        "job: job reprioritize needs --priority N; see job reprioritize --help\n"
    );
}

#[test]
fn idset_help_and_completion_name_the_set_operand() {
    for command in IDSET_COMMANDS {
        let help = out(&offline(&[command[0], "--help"]));
        assert!(
            help.contains(&format!("job {} [OPTIONS] ID[-ID][,ID...]...", command[0])),
            "{help}"
        );
        assert!(
            help.contains(
                "one Job, an inclusive range such as 1-10, or a comma list such as 1,4,7-9"
            ),
            "{help}"
        );
        if command[0] != "wait" {
            assert!(help.contains("--dry-run"), "{help}");
        }
    }
    for command in [
        "status", "show", "explain", "logs", "attempts", "attach", "edit", "update",
    ] {
        let help = out(&offline(&[command, "--help"]));
        assert!(!help.contains("ID[-ID]"), "{command}: {help}");
    }
    let german = plain(&["cancel", "--help"])
        .env("LC_ALL", "de_DE.UTF-8")
        .output()
        .unwrap();
    assert!(
        out(&german).contains("ein Job, ein Bereich mit beiden Enden wie 1-10"),
        "{}",
        out(&german)
    );
    assert!(out(&offline(&["list", "--help"])).contains("--id ID[-ID][,ID...]"));
    let daemon = Daemon::start("idset-complete");
    let first = daemon.ok(&["create", "--", "true"]);
    let second = daemon.ok(&["create", "--", "true"]);
    let held = [first.trim().to_owned(), second.trim().to_owned()];
    assert_eq!(complete(BASH, "job cancel ''", &daemon.state), held);
    assert_eq!(complete(BASH, "job cancel 1 ''", &daemon.state), held);
    assert_eq!(complete(BASH, "job release ''", &daemon.state), held);
    assert_eq!(complete(BASH, "job list --id ''", &daemon.state), held);
}

#[test]
fn pidns_run_passes_stdin_and_keeps_the_streams_apart() {
    let daemon = Daemon::start("pidns-stdin");
    let pid = ["run", "--stdin", "--namespaces", "user,mount,pid", "--"];
    let lines = piped(
        &daemon,
        &[&pid[..], &["cat"]].concat(),
        b"one\ntwo\nthree\n",
    );
    assert_eq!(lines.status.code(), Some(0), "{}", err(&lines));
    assert_eq!(out(&lines), "one\ntwo\nthree\n");
    let payload: Vec<u8> = (0..300_000u32).map(|index| (index % 251) as u8).collect();
    let counted = piped(&daemon, &[&pid[..], &["wc", "-c"]].concat(), &payload);
    assert_eq!(counted.status.code(), Some(0), "{}", err(&counted));
    assert_eq!(out(&counted).trim(), "300000");
    let without = piped(
        &daemon,
        &[
            "run",
            "--namespaces",
            "user,mount,pid",
            "--",
            "sh",
            "-c",
            "cat; echo end $$; echo problem >&2; exit 3",
        ],
        b"unseen\n",
    );
    assert_eq!(without.status.code(), Some(3), "{}", err(&without));
    assert_eq!(out(&without), "end 2\n");
    assert!(err(&without).contains("problem"), "{}", err(&without));
    let record = json(&daemon.ok(&["list", "--all", "--json"]));
    let id = record["jobs"][2]["job"]["id"].to_string();
    let stdout = daemon.job(&["logs", &id, "--stream", "stdout", "--raw"]);
    assert_eq!(out(&stdout), "end 2\n");
    let stderr = daemon.job(&["logs", &id, "--stream", "stderr", "--raw"]);
    assert_eq!(out(&stderr), "problem\n");
}

#[test]
fn run_of_a_program_that_cannot_be_started_names_the_reason() {
    let daemon = Daemon::start("unstartable");
    let missing = daemon.job(&["run", "--", "/no-such-program-here"]);
    assert_eq!(missing.status.code(), Some(125), "{}", err(&missing));
    assert!(
        err(&missing).contains("/no-such-program-here"),
        "{}",
        err(&missing)
    );
    assert!(!err(&missing).contains("recording"), "{}", err(&missing));
    assert_eq!(out(&missing), "");
}
