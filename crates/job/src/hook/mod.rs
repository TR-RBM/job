mod messages;
pub use messages::message;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::policy::{Caller, Denial, Policy};
use crate::shell::{self, program, unwrap_command};

const CHEAP: [&str; 73] = [
    "job",
    "install",
    "sv",
    "set",
    "unset",
    "local",
    "read",
    ":",
    "find",
    "du",
    "paste",
    "comm",
    "nl",
    "fold",
    "sha256sum",
    "ls",
    "cat",
    "head",
    "tail",
    "wc",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "sed",
    "awk",
    "cut",
    "sort",
    "uniq",
    "tr",
    "echo",
    "printf",
    "pwd",
    "cd",
    "which",
    "type",
    "command",
    "file",
    "stat",
    "readlink",
    "realpath",
    "basename",
    "dirname",
    "date",
    "true",
    "false",
    "test",
    "[",
    "mkdir",
    "touch",
    "ln",
    "cp",
    "mv",
    "rm",
    "rmdir",
    "chmod",
    "diff",
    "cmp",
    "jq",
    "column",
    "tee",
    "df",
    "free",
    "uptime",
    "ps",
    "id",
    "whoami",
    "hostname",
    "uname",
    "nproc",
    "env",
    "export",
    "exit",
];
const HEAVY_GIT: [&str; 5] = ["gc", "clone", "fsck", "repack", "bisect"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Direct,
    Route { reason: String },
}

pub fn is_cheap(words: &[String]) -> bool {
    verdict_of(words) == Verdict::Direct
}

fn verdict_of(words: &[String]) -> Verdict {
    let words = unwrap_command(words);
    let name = program(&words);
    if matches!(name.as_str(), "bash" | "sh")
        && let Some(inner) = words.iter().skip_while(|w| *w != "-c").nth(1)
    {
        return classify(inner);
    }
    let cheap = match name.as_str() {
        "" => true,
        "cargo" => crate::estimate::cargo_rule(&words).is_none(),
        "git" => !words
            .iter()
            .skip(1)
            .any(|w| HEAVY_GIT.contains(&w.as_str())),
        other => CHEAP.contains(&other),
    };
    if cheap {
        Verdict::Direct
    } else {
        Verdict::Route {
            reason: format!("it runs {}", name.replace(['\n', '\t'], " ")),
        }
    }
}

pub fn classify(command: &str) -> Verdict {
    let parsed = shell::parse(command);
    parsed
        .commands
        .iter()
        .map(|words| verdict_of(words))
        .find(|v| *v != Verdict::Direct)
        .unwrap_or(Verdict::Direct)
}

pub fn waits_on_the_service(command: &str) -> bool {
    shell::simple_commands(command).iter().any(|words| {
        program(words) == "job" && matches!(words.get(1).map(String::as_str), Some("run" | "wait"))
    })
}

pub fn keeping_status_and_cwd(command: &str) -> String {
    format!(
        "{command}; __exec_status=$?; [ -d \"$PWD\" ] || {{ echo \"[job] note: the working directory $PWD no longer exists; the shell moved to $HOME\"; cd \"$HOME\"; }}; (exit $__exec_status)"
    )
}

pub fn without_budget_limit(command: &str) -> String {
    if command.contains("--budget") {
        return command.to_string();
    }
    command.replacen("job run ", "job run --budget none ", 1)
}

pub fn rewrite(command: &str, session: &str, budget: Option<u64>) -> String {
    let budget = budget.map_or_else(|| "none".to_string(), |ms| format!("{ms}ms"));
    format!(
        "job run --session {} --budget {budget} --shell bash --summary -- {}",
        shell::quote(session),
        shell::quote(command)
    )
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Call {
    pub command: String,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub caller: Option<String>,
    #[serde(default)]
    pub caller_name: Option<String>,
    #[serde(default)]
    pub detached: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny,
    Rewrite,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Answer {
    pub decision: Decision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detach: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Answer {
    pub fn allow(reason: Option<String>) -> Answer {
        Answer {
            decision: Decision::Allow,
            command: None,
            detach: None,
            rule: None,
            reason,
        }
    }

    pub fn deny(rule: Option<&str>, reason: String) -> Answer {
        Answer {
            decision: Decision::Deny,
            command: None,
            detach: None,
            rule: rule.map(str::to_owned),
            reason: Some(reason),
        }
    }

    fn rewrite(command: String, reason: String) -> Answer {
        Answer {
            decision: Decision::Rewrite,
            command: Some(command),
            detach: Some(true),
            rule: None,
            reason: Some(reason),
        }
    }
}

pub fn parse(input: &str) -> Result<Call, String> {
    let refuse = |error: String| {
        message(
            "hook: standard input is not one JSON object with a command: {error}",
            &[("error", error)],
        )
    };
    let value: serde_json::Value =
        serde_json::from_str(input).map_err(|error| refuse(error.to_string()))?;
    if !value.is_object() {
        return Err(refuse(message("not an object", &[])));
    }
    serde_json::from_value(value).map_err(|error| refuse(error.to_string()))
}

pub fn police(call: &Call, policy: &Policy) -> Option<Denial> {
    let caller = Caller {
        name: call.caller_name.clone(),
        cwd: call
            .cwd
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/")),
    };
    policy.check(&call.command, &caller)
}

pub fn respond(call: &Call, daemon_answers: impl Fn() -> bool) -> Answer {
    let command = call.command.as_str();
    let (prefix, routed) = shell::split_state_prefix(command);
    let waits = waits_on_the_service(command);
    let verdict = classify(routed);
    if (waits || verdict != Verdict::Direct) && !prefix.is_empty() && !call.detached {
        let prefix = prefix.trim_end_matches(['&', ';', '\n', ' ']);
        return Answer::deny(
            None,
            message(
                "job: this line would run detached, where `{prefix}` changes nothing for the caller's shell, so later commands would run in the old directory or environment. Run `{prefix}` as its own call first, then `{rest}`.",
                &[("prefix", prefix.to_owned()), ("rest", routed.to_owned())],
            ),
        );
    }
    if waits {
        if call.detached {
            return Answer::allow(None);
        }
        return Answer::rewrite(
            keeping_status_and_cwd(&without_budget_limit(command)),
            message(
                "job: a `job run` or `job wait` holds its caller until the Job ends; run the rewritten command without waiting for it. Its answer begins with `[job] job N`.",
                &[],
            ),
        );
    }
    let Verdict::Route { reason } = verdict else {
        return Answer::allow(None);
    };
    if !daemon_answers() {
        return Answer::allow(Some(message(
            "job: this command would go through the job service because {reason}, but the service does not answer, so it is left to run directly, without a reservation.",
            &[("reason", reason)],
        )));
    }
    let job_line = rewrite(routed, call.caller.as_deref().unwrap_or("unnamed"), None);
    let line = if prefix.is_empty() {
        job_line
    } else {
        format!("{prefix} {job_line}")
    };
    Answer::rewrite(
        keeping_status_and_cwd(&line),
        message(
            "job: routed to the job service because {reason}; run the rewritten command without waiting for it. Its answer begins with `[job] job N`.",
            &[("reason", reason)],
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routed(command: &str) -> bool {
        matches!(classify(command), Verdict::Route { .. })
    }

    #[test]
    fn reading_and_looking_run_directly() {
        for command in [
            "ls -la",
            "git status && git log --oneline -5",
            "grep -rn foo src | head -20",
            "cat Cargo.toml",
            "cd /tmp && pwd",
            "job wait 17",
            "job run -- cargo test",
            "cargo fmt; job queue | tail -1",
            "cargo --version && cargo metadata --format-version 1 | head -c 100",
            "sudo install -m 0755 target/release/job /usr/local/bin/job",
            "N=$((10#$N+1)); echo $N",
            "time ls",
            "for r in one two three; do git -C $r config user.name; done",
            "sudo sv status job; ls -la /etc/runit/sv/job",
            "sudo sv restart job",
            "git branch | while read b; do git ls-tree $b | head -1; done",
            "sudo -n true",
            "bash -c 'git status'",
            "set -e; find . -name '*.rs' | xargs grep -l foo",
        ] {
            assert!(!routed(command), "{command}");
        }
    }

    #[test]
    fn builds_tests_loops_sleeps_and_unknown_programs_are_routed() {
        for command in [
            "cargo test",
            "cargo build --release 2>&1 | tail -5",
            "tools/check",
            "sleep 280; cat report.txt",
            "for f in *.rs; do cargo check; done",
            "while pgrep -f build; do sleep 5; done",
            "until test -f done.txt; do cargo check; done",
            "python3 render.py",
            "git gc --aggressive",
            "echo $(make -n)",
            "time cargo build",
            "timeout 60 ./target/release/probe",
            "flock /tmp/x.lock cargo test",
            "bash -c 'make all'",
            "RUST_LOG=1 nice -n 10 cargo bench",
        ] {
            assert!(routed(command), "{command}");
        }
    }

    fn call(command: &str) -> Call {
        Call {
            command: command.to_owned(),
            ..Call::default()
        }
    }

    #[test]
    fn a_routed_command_is_rewritten_to_run_detached_under_the_callers_label() {
        let asked = Call {
            caller: Some("abc".to_owned()),
            ..call("cargo test")
        };
        let answer = respond(&asked, || true);
        assert_eq!(answer.decision, Decision::Rewrite);
        assert_eq!(
            answer.command.as_deref(),
            Some(
                keeping_status_and_cwd(
                    "job run --session 'abc' --budget none --shell bash --summary -- 'cargo test'"
                )
                .as_str()
            )
        );
        assert_eq!(answer.detach, Some(true));
        assert!(answer.reason.unwrap().contains("because it runs cargo"));
    }

    #[test]
    fn a_job_run_or_wait_that_would_hold_its_caller_is_rewritten_to_run_detached() {
        for (command, expected) in [
            (
                "job run --cores 8 --mem 6G -- 'cargo build --release'",
                "job run --budget none --cores 8 --mem 6G -- 'cargo build --release'",
            ),
            ("job wait 17", "job wait 17"),
            (
                "job run --budget 30s -- make",
                "job run --budget 30s -- make",
            ),
        ] {
            let answer = respond(&call(command), || true);
            assert_eq!(answer.decision, Decision::Rewrite);
            assert_eq!(answer.command, Some(keeping_status_and_cwd(expected)));
            assert_eq!(answer.detach, Some(true));
        }
    }

    #[test]
    fn the_guard_keeps_the_commands_status_when_its_directory_vanished() {
        let base = std::env::temp_dir().join(format!("job-guard-{}", std::process::id()));
        for (status, expected) in [("true", Some(0)), ("false", Some(1))] {
            std::fs::create_dir_all(&base).unwrap();
            let line = keeping_status_and_cwd(&format!("rmdir {} && {status}", base.display()));
            let output = std::process::Command::new("bash")
                .arg("-c")
                .arg(format!(
                    "cd {} && {line}; echo \"cwd=$PWD\"; (exit $__exec_status)",
                    base.display()
                ))
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&output.stdout);
            assert_eq!(output.status.code(), expected, "{text}");
            assert!(
                text.contains("no longer exists; the shell moved to"),
                "{text}"
            );
            assert!(!text.contains(&format!("cwd={}", base.display())), "{text}");
        }
    }

    #[test]
    fn a_forbidden_command_is_denied_with_the_rule_and_the_alternative() {
        let asked = Call {
            cwd: Some(PathBuf::from("/home/agent/work/x/y")),
            caller: Some("no-such-caller".to_owned()),
            ..call("cd target && rm -rf debug")
        };
        let path = format!("{}/tests/fixtures/policy.json", env!("CARGO_MANIFEST_DIR"));
        let policy = crate::policy_file::parse(
            &std::fs::read_to_string(&path).unwrap(),
            std::path::Path::new(&path),
        )
        .unwrap();
        let denial = police(&asked, &policy).unwrap();
        assert_eq!(denial.rule, "rm-recursive-force");
        assert!(denial.reason.contains("Instead: remove a file with rm -i"));
        let answer = Answer::deny(Some(&denial.rule), denial.reason);
        assert_eq!(
            serde_json::to_value(&answer).unwrap()["decision"],
            serde_json::json!("deny")
        );
    }

    #[test]
    fn a_leading_cd_before_routed_work_is_refused_with_the_two_calls_to_make() {
        for (command, first, rest) in [
            (
                "cd /home/agent/work/lead/x && python3 edit.py",
                "cd /home/agent/work/lead/x",
                "python3 edit.py",
            ),
            ("export A=1; cargo test", "export A=1", "cargo test"),
            ("cd x && job run -- make", "cd x", "job run -- make"),
        ] {
            let answer = respond(&call(command), || true);
            let reason = answer.reason.unwrap();
            assert_eq!(answer.decision, Decision::Deny);
            assert!(
                reason.contains(&format!(
                    "Run `{first}` as its own call first, then `{rest}`"
                )),
                "{reason}"
            );
        }
        assert_eq!(
            respond(&call("cd x && git status"), || true),
            Answer::allow(None)
        );
    }

    #[test]
    fn job_queries_and_submit_are_allowed_unchanged() {
        for command in [
            "job queue",
            "job status 3",
            "job log 3 errors",
            "job cancel 3",
            "job submit -- make",
        ] {
            assert_eq!(
                respond(&call(command), || true),
                Answer::allow(None),
                "{command}"
            );
        }
        let asked = Call {
            detached: true,
            ..call("job wait 3")
        };
        assert_eq!(respond(&asked, || true), Answer::allow(None));
    }

    #[test]
    fn without_a_daemon_the_command_is_allowed_and_the_caller_is_told() {
        let answer = respond(&call("cargo test"), || false);
        assert_eq!(answer.decision, Decision::Allow);
        assert!(answer.command.is_none());
        assert!(answer.reason.unwrap().contains("run directly"));
    }

    #[test]
    fn cheap_commands_are_allowed_without_a_reason() {
        assert_eq!(respond(&call("ls"), || true), Answer::allow(None));
    }
}
