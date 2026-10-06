use std::path::{Path, PathBuf};

use crate::model::{HistoryEntry, Spec, StopKind};
use crate::resources::{MILLI, Reservation, Source, Vector};
use crate::shell;

pub const HISTORY_WINDOW: usize = 10;
const RUNTIME_WINDOW: usize = 2;
const MARGIN_PERCENT: u64 = 15;
const DEFAULT_PIDS: u64 = 4096;

pub struct Context {
    pub repository_cargo: Vec<HistoryEntry>,
    pub cores: u64,
    pub cargo_build_jobs: Option<u64>,
    pub pool: Vector,
}

pub fn command_words(argv: &[String]) -> Vec<String> {
    let is_shell = argv.first().is_some_and(|a| {
        matches!(
            Path::new(a).file_name().and_then(|n| n.to_str()),
            Some("sh" | "bash")
        )
    });
    if is_shell && argv.get(1).map(String::as_str) == Some("-c") {
        argv.get(2).map_or_else(Vec::new, |c| {
            c.split_whitespace().map(str::to_string).collect()
        })
    } else {
        argv.to_vec()
    }
}

pub fn command_text(argv: &[String]) -> String {
    command_words(argv).join(" ")
}

pub fn history_key(spec: &Spec) -> String {
    format!(
        "{} @ {}",
        routing_text(&spec.argv),
        repository_of(&spec.cwd).display()
    )
}

pub fn routing_text(argv: &[String]) -> String {
    let heavy: Vec<String> = commands_of(argv)
        .into_iter()
        .filter(|words| !crate::hook::is_cheap(words))
        .map(|words| words.join(" "))
        .collect();
    if heavy.is_empty() {
        command_text(argv)
    } else {
        heavy.join(" ; ")
    }
}

pub fn repository_of(cwd: &Path) -> PathBuf {
    for dir in cwd.ancestors() {
        let git = dir.join(".git");
        if git.is_dir() {
            return git;
        }
        if let Ok(text) = std::fs::read_to_string(&git)
            && let Some(target) = text.trim().strip_prefix("gitdir:")
        {
            let target = dir.join(target.trim());
            let common = target.join("commondir");
            return match std::fs::read_to_string(&common) {
                Ok(relative) => target.join(relative.trim()),
                Err(_) => target,
            };
        }
    }
    cwd.to_path_buf()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CargoRule {
    pub subcommand: String,
    pub release: bool,
    pub jobs: Option<u64>,
}

impl CargoRule {
    pub fn same_kind(&self, other: &CargoRule) -> bool {
        self.subcommand == other.subcommand && self.release == other.release
    }
}

fn commands_of(argv: &[String]) -> Vec<Vec<String>> {
    let is_shell = argv.first().is_some_and(|a| {
        matches!(
            Path::new(a).file_name().and_then(|n| n.to_str()),
            Some("sh" | "bash")
        )
    });
    match (is_shell, argv.get(1).map(String::as_str), argv.get(2)) {
        (true, Some("-c"), Some(line)) => shell::simple_commands(line),
        _ => vec![shell::unwrap_command(argv)],
    }
}

fn cargo_rule_of(words: &[String]) -> Option<CargoRule> {
    const HEAVY: [&str; 8] = [
        "build", "test", "check", "clippy", "run", "bench", "doc", "rustc",
    ];
    if words.first().is_some_and(|w| w.ends_with("tools/check")) {
        return Some(CargoRule {
            subcommand: "tools/check".to_string(),
            release: false,
            jobs: None,
        });
    }
    if shell::program(words) != "cargo" {
        return None;
    }
    let subcommand = words
        .iter()
        .skip(1)
        .find(|w| !w.starts_with('-') && !w.starts_with('+'))?;
    if !HEAVY.contains(&subcommand.as_str()) {
        return None;
    }
    let jobs = words.windows(2).find_map(|pair| match pair[0].as_str() {
        "-j" | "--jobs" => pair[1].parse().ok(),
        _ => None,
    });
    let jobs = jobs.or_else(|| {
        words.iter().find_map(|w| {
            w.strip_prefix("--jobs=")
                .or_else(|| w.strip_prefix("-j"))?
                .parse()
                .ok()
        })
    });
    let release = words.iter().any(|w| w == "--release" || w == "-r")
        || words
            .windows(2)
            .any(|p| p[0] == "--profile" && p[1] == "release");
    Some(CargoRule {
        subcommand: subcommand.clone(),
        release,
        jobs,
    })
}

pub fn cargo_rule(argv: &[String]) -> Option<CargoRule> {
    commands_of(argv)
        .iter()
        .find_map(|words| cargo_rule_of(words))
}

pub fn cargo_rule_of_key(key: &str) -> Option<CargoRule> {
    let command = key.rsplit_once(" @ ").map_or(key, |(command, _)| command);
    cargo_rule(&["bash".to_string(), "-c".to_string(), command.to_string()])
}

fn with_margin(bytes: u64) -> u64 {
    bytes + bytes / 100 * MARGIN_PERCENT
}

pub fn estimate(spec: &Spec, history: &[HistoryEntry], context: &Context) -> Reservation {
    let recent: Vec<&HistoryEntry> = history.iter().rev().take(HISTORY_WINDOW).collect();
    let complete: Vec<&HistoryEntry> = recent
        .iter()
        .copied()
        .filter(|e| e.stop.is_none())
        .collect();
    let cargo = cargo_rule(&spec.argv);
    let core_share = |total: u64| total / context.cores.max(1);

    let (cores_milli, cores_source) = match (spec.declared.cores_milli, &cargo) {
        (Some(declared), _) => (declared, Source::Declared),
        (None, Some(rule)) => (
            rule.jobs
                .or(context.cargo_build_jobs)
                .unwrap_or(context.cores)
                .clamp(1, context.cores)
                * MILLI,
            Source::Rule {
                tool: "cargo".to_string(),
            },
        ),
        (None, None) => (MILLI, Source::Default),
    };

    let last_stop = |kind: StopKind| recent.first().filter(|entry| entry.stop == Some(kind));
    let repository: Vec<&HistoryEntry> = context
        .repository_cargo
        .iter()
        .filter(|e| e.stop.is_none())
        .rev()
        .take(HISTORY_WINDOW)
        .collect();
    let repository_source = Source::RepositoryHistory {
        runs: repository.len(),
    };
    let cargo_source = Source::Rule {
        tool: "cargo".to_string(),
    };

    let cores_share = |total: u64| core_share(total) * cores_milli / MILLI;
    let (memory, memory_source) = if let Some(declared) = spec.declared.memory {
        (declared, Source::Declared)
    } else if let Some(stop) = last_stop(StopKind::Memory) {
        (
            stop.memory_limit.saturating_mul(2),
            Source::AfterStop {
                limit: stop.memory_limit,
            },
        )
    } else if !complete.is_empty() {
        (
            with_margin(
                complete
                    .iter()
                    .map(|e| e.usage.peak_memory)
                    .max()
                    .unwrap_or(0),
            ),
            Source::History {
                runs: complete.len(),
            },
        )
    } else if cargo.is_some() && !repository.is_empty() {
        (
            with_margin(
                repository
                    .iter()
                    .map(|e| e.usage.peak_memory)
                    .max()
                    .unwrap_or(0),
            ),
            repository_source.clone(),
        )
    } else if cargo.is_some() {
        (cores_share(context.pool.memory), cargo_source)
    } else {
        (core_share(context.pool.memory), Source::Default)
    };

    let (disk, disk_source) = match spec.declared.disk {
        Some(declared) => (declared, Source::Declared),
        None => (0, Source::Default),
    };

    let finished: Vec<u64> = recent
        .iter()
        .filter(|e| e.stop.is_none())
        .take(RUNTIME_WINDOW)
        .map(|e| e.wall_ms)
        .collect();
    let predicted_ms =
        (!finished.is_empty()).then(|| finished.iter().sum::<u64>() / finished.len() as u64);

    Reservation {
        vector: Vector {
            cores_milli,
            memory: memory.min(context.pool.memory),
            pids: spec.declared.pids.unwrap_or(DEFAULT_PIDS),
        },
        disk,
        devices: spec.declared.devices.clone(),
        cores_source,
        memory_source,
        disk_source,
        predicted_ms,
        wall_limit_ms: spec.declared.wall_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Declared, Usage};

    const GIB: u64 = 1 << 30;

    fn spec(command: &str) -> Spec {
        Spec {
            argv: vec!["bash".to_string(), "-c".to_string(), command.to_string()],
            cwd: PathBuf::from("/nonexistent/repo"),
            session: "test".to_string(),
            declared: Declared::default(),
            queue: None,
        }
    }

    fn context() -> Context {
        Context {
            repository_cargo: Vec::new(),
            cores: 4,
            cargo_build_jobs: None,
            pool: Vector {
                cores_milli: 4000,
                memory: 36 * GIB,
                pids: 1 << 20,
            },
        }
    }

    fn run(peak_memory: u64, wall_ms: u64, stop: Option<StopKind>) -> HistoryEntry {
        HistoryEntry {
            job_id: None,
            attempt: None,
            key: String::new(),
            finished_ms: 0,
            wall_ms,
            usage: Usage {
                peak_memory,
                written: GIB,
                ..Usage::default()
            },
            stop,
            memory_limit: 4 * GIB,
            succeeded: stop.is_none(),
        }
    }

    #[test]
    fn an_unknown_command_gets_one_core_and_one_cores_share() {
        let r = estimate(&spec("python3 render.py"), &[], &context());
        assert_eq!(r.vector.cores_milli, 1000);
        assert_eq!(r.vector.memory, 9 * GIB);
        assert_eq!(r.disk, 0);
        assert_eq!(r.vector.pids, 4096);
        assert_eq!(r.predicted_ms, None);
        assert_eq!(r.memory_source, Source::Default);
    }

    #[test]
    fn cargo_without_history_gets_every_core_and_the_whole_pool() {
        let r = estimate(&spec("cargo test --workspace"), &[], &context());
        assert_eq!(r.vector.cores_milli, 4000);
        assert_eq!(r.vector.memory, 36 * GIB);
    }

    #[test]
    fn cargo_with_jobs_gets_that_many_cores() {
        assert_eq!(
            estimate(&spec("cargo build -j 2"), &[], &context())
                .vector
                .cores_milli,
            2000
        );
        assert_eq!(
            estimate(&spec("cargo +stable build --jobs=3"), &[], &context())
                .vector
                .cores_milli,
            3000
        );
    }

    #[test]
    fn cargo_follows_cargo_build_jobs_from_the_jobs_environment() {
        let mut context = context();
        context.cargo_build_jobs = Some(2);
        assert_eq!(
            estimate(&spec("cargo test"), &[], &context)
                .vector
                .cores_milli,
            2000
        );
        assert_eq!(
            estimate(&spec("cargo test -j 3"), &[], &context)
                .vector
                .cores_milli,
            3000
        );
    }

    #[test]
    fn history_gives_the_largest_recent_peak_with_a_margin() {
        let history = [run(2 * GIB, 1000, None), run(GIB, 3000, None)];
        let r = estimate(&spec("cargo test"), &history, &context());
        assert_eq!(r.vector.memory, 2 * GIB + 2 * GIB / 100 * 15);
        assert_eq!(r.memory_source, Source::History { runs: 2 });
        assert_eq!(r.predicted_ms, Some(2000));
    }

    #[test]
    fn a_memory_stop_doubles_the_limit_it_was_stopped_at() {
        let history = [
            run(GIB, 1000, None),
            run(4 * GIB, 500, Some(StopKind::Memory)),
        ];
        let r = estimate(&spec("make"), &history, &context());
        assert_eq!(r.vector.memory, 8 * GIB);
        assert_eq!(r.memory_source, Source::AfterStop { limit: 4 * GIB });
        assert_eq!(r.predicted_ms, Some(1000));
    }

    #[test]
    fn cargo_after_cd_and_wrappers_follows_the_cargo_rule() {
        let mut context = context();
        context.cargo_build_jobs = Some(4);
        let r = estimate(
            &spec("cd /home/x/tool && time cargo test -p tool-asm 2>&1"),
            &[],
            &context,
        );
        assert_eq!(r.vector.cores_milli, 4000);
        assert_eq!(
            r.cores_source,
            Source::Rule {
                tool: "cargo".to_string()
            }
        );
    }

    #[test]
    fn a_first_cargo_command_learns_from_other_cargo_runs_in_its_repository() {
        let mut context = context();
        context.repository_cargo = vec![run(3 * GIB, 60_000, None), run(GIB, 1000, None)];
        let r = estimate(&spec("cargo build --release"), &[], &context);
        assert_eq!(r.vector.memory, 3 * GIB + 3 * GIB / 100 * 15);
        assert_eq!(r.memory_source, Source::RepositoryHistory { runs: 2 });
        assert_eq!(r.disk, 0);
    }

    #[test]
    fn a_first_cargo_command_without_any_history_gets_the_share_of_its_cores() {
        let mut context = context();
        context.cargo_build_jobs = Some(1);
        let r = estimate(&spec("cargo build --release"), &[], &context);
        assert_eq!(r.vector.cores_milli, 1000);
        assert_eq!(r.vector.memory, 9 * GIB);
        assert_eq!(r.disk, 0);
        assert_eq!(r.disk_source, Source::Default);
    }

    #[test]
    fn a_run_cut_short_teaches_nothing_about_peaks() {
        let mut cut_short = run(2 * GIB, 21_000, Some(StopKind::Disk));
        cut_short.usage.written = 880 << 20;
        let history = [run(5 * GIB, 60_000, None), cut_short];
        let r = estimate(&spec("cargo test --workspace"), &history, &context());
        assert_eq!(r.vector.memory, 5 * GIB + 5 * GIB / 100 * 15);
        assert_eq!(r.memory_source, Source::History { runs: 1 });
        let mut context = context();
        context.repository_cargo = vec![run(GIB, 1, Some(StopKind::Memory))];
        let r = estimate(&spec("cargo build"), &[], &context);
        assert_eq!(r.vector.memory, 36 * GIB);
    }

    #[test]
    fn cargo_runs_are_of_one_kind_only_with_the_same_subcommand_and_profile() {
        let release =
            cargo_rule_of_key("cargo build --release --workspace 2>&1 @ /repo/.git").unwrap();
        let test = cargo_rule_of_key("cargo test --workspace 2>&1 @ /repo/.git").unwrap();
        let debug = cargo_rule_of_key("cd x && cargo build -p y @ /repo/.git").unwrap();
        assert!(release.release && !test.release && !debug.release);
        assert!(!release.same_kind(&test));
        assert!(!release.same_kind(&debug));
        assert!(
            test.same_kind(&cargo_rule_of_key("cargo test -p tool-numbers @ /repo/.git").unwrap())
        );
    }

    #[test]
    fn a_decorated_line_and_the_plain_command_share_one_history() {
        let decorated = spec("tools/check 2>&1 | tail -12; echo \"check exit ${PIPESTATUS[0]}\"");
        let plain = spec("tools/check");
        assert_eq!(history_key(&decorated), history_key(&plain));
        assert_eq!(
            history_key(&spec(
                "cd /repo && cargo test -p x 2>&1 | grep -E 'FAILED|result'"
            )),
            history_key(&spec("cargo test -p x"))
        );
        assert_ne!(
            history_key(&spec("cargo test -p x")),
            history_key(&spec("cargo test -p y"))
        );
        assert_eq!(history_key(&spec("ls -la")), "ls -la @ /nonexistent/repo");
    }

    #[test]
    fn tools_check_reserves_cores_like_cargo_under_its_own_kind() {
        let mut context = context();
        context.cargo_build_jobs = Some(4);
        let r = estimate(&spec("tools/check 2>&1 | tail -12"), &[], &context);
        assert_eq!(r.vector.cores_milli, 4000);
        let check = cargo_rule_of_key("tools/check @ /repo/.git").unwrap();
        let cargo_check = cargo_rule_of_key("cargo check @ /repo/.git").unwrap();
        assert!(!check.same_kind(&cargo_check));
    }

    #[test]
    fn declared_values_win() {
        let mut s = spec("cargo test");
        s.declared = Declared {
            cores_milli: Some(500),
            memory: Some(GIB),
            pids: Some(20),
            disk: Some(GIB),
            devices: vec!["gpu0".to_string()],
            wall_ms: Some(60_000),
            ..Declared::default()
        };
        let r = estimate(&s, &[run(8 * GIB, 1, None)], &context());
        assert_eq!(
            r.vector,
            Vector {
                cores_milli: 500,
                memory: GIB,
                pids: 20
            }
        );
        assert_eq!(r.disk, GIB);
        assert_eq!(r.devices, vec!["gpu0".to_string()]);
        assert_eq!(r.wall_limit_ms, Some(60_000));
    }

    #[test]
    fn only_heavy_cargo_subcommands_follow_the_cargo_rule() {
        assert!(cargo_rule(&[String::from("cargo"), String::from("fmt")]).is_none());
        assert!(cargo_rule(&[String::from("/usr/bin/cargo"), String::from("clippy")]).is_some());
        assert!(cargo_rule(&[String::from("make")]).is_none());
    }
}
