use std::collections::HashSet;

use crate::query::is_error_line;

pub const ERROR_LINES: usize = 10;
pub const WARNING_LINES: usize = 10;
pub const FAILED_TEST_LINES: usize = 10;
pub const LAST_LINES: usize = 5;
pub const LINE_CAP: usize = ERROR_LINES + WARNING_LINES + FAILED_TEST_LINES + LAST_LINES;
const CONTEXT_BEFORE: usize = 4;
const CONTEXT_AFTER: usize = 6;
const PANIC_MESSAGE_LINES: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Level {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub level: Level,
    pub code: Option<String>,
    pub message: String,
    pub location: Option<String>,
    pub label: Option<String>,
}

impl Diagnostic {
    pub fn render(&self) -> String {
        let level = match self.level {
            Level::Error => "error",
            Level::Warning => "warning",
        };
        let code = self
            .code
            .as_ref()
            .map(|c| format!("[{c}]"))
            .unwrap_or_default();
        let location = self
            .location
            .as_ref()
            .map(|l| format!("{l}: "))
            .unwrap_or_default();
        let label = match &self.label {
            Some(label) if !self.message.contains(label.as_str()) => format!(": {label}"),
            _ => String::new(),
        };
        format!("{location}{level}{code}: {}{label}", self.message)
    }
}

fn split_header(text: &str) -> Option<(Level, Option<String>, String)> {
    let (level, rest) = if let Some(rest) = text.strip_prefix("error") {
        (Level::Error, rest)
    } else {
        let rest = text.strip_prefix("warning")?;
        (Level::Warning, rest)
    };
    let (code, rest) = match rest.strip_prefix('[') {
        Some(inner) => {
            let (code, after) = inner.split_once(']')?;
            (Some(code.to_string()), after)
        }
        None => (None, rest),
    };
    let message = rest.strip_prefix(": ")?;
    Some((level, code, message.trim().to_string()))
}

fn location_of(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("--> ")?;
    Some(rest.trim().to_string())
}

fn short_form(line: &str) -> Option<Diagnostic> {
    for marker in [": error", ": warning"] {
        if let Some(at) = line.find(marker) {
            let location = &line[..at];
            let mut parts = location.rsplitn(3, ':');
            let column = parts.next()?;
            let row = parts.next()?;
            if column.parse::<u32>().is_err()
                || row.parse::<u32>().is_err()
                || location.contains(' ')
            {
                continue;
            }
            let (level, code, message) = split_header(&line[at + 2..])?;
            return Some(Diagnostic {
                level,
                code,
                message,
                location: Some(location.to_string()),
                label: None,
            });
        }
    }
    None
}

fn label_of(block: &[String]) -> Option<String> {
    block.iter().find_map(|line| {
        let after_bar = line.split_once('|')?.1;
        let carets = after_bar.find('^')?;
        let rest = after_bar[carets..].trim_start_matches('^').trim();
        (!rest.is_empty()).then(|| rest.to_string())
    })
}

pub fn rust_diagnostics(lines: &[String]) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        let diagnostic = if let Some((level, code, message)) = split_header(line) {
            let block_end = (i + 1..lines.len())
                .find(|&j| lines[j].trim().is_empty() || split_header(&lines[j]).is_some())
                .unwrap_or(lines.len());
            let block = &lines[i + 1..block_end];
            let location = block.iter().take(3).find_map(|l| location_of(l));
            let label = location.as_ref().and_then(|_| label_of(block));
            Some(Diagnostic {
                level,
                code,
                message,
                location,
                label,
            })
        } else {
            short_form(line)
        };
        if let Some(diagnostic) = diagnostic
            && seen.insert(diagnostic.clone())
        {
            found.push(diagnostic);
        }
        i += 1;
    }
    found
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FailedTest {
    pub name: String,
    pub location: Option<String>,
    pub message: Vec<String>,
}

impl FailedTest {
    pub fn render(&self) -> String {
        let mut line = format!("FAILED {}", self.name);
        if let Some(location) = &self.location {
            line.push_str(&format!(" at {location}"));
        }
        if !self.message.is_empty() {
            line.push_str(": ");
            line.push_str(&self.message.join("; "));
        }
        line
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TestReport {
    pub failed: Vec<FailedTest>,
    pub passed: u64,
    pub failed_count: u64,
    pub ignored: u64,
    pub targets: u64,
    pub stopped_at_first_failing_target: bool,
}

fn count_before(text: &str, word: &str) -> u64 {
    text.split(';')
        .find_map(|part| {
            part.trim()
                .strip_suffix(word)?
                .trim()
                .rsplit(' ')
                .next()?
                .parse()
                .ok()
        })
        .unwrap_or(0)
}

fn panic_location(line: &str) -> Option<String> {
    let rest = line.split_once("panicked at ")?.1;
    Some(rest.trim().trim_end_matches(':').to_string())
}

pub fn test_report(lines: &[String]) -> TestReport {
    let mut report = TestReport::default();
    let mut names_in_lists: Vec<String> = Vec::new();
    let mut in_failure_list = false;
    let mut last_rerun_hint = None;
    let mut targets_failed_summary = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].as_str();
        if let Some(rest) = line.strip_prefix("test result: ") {
            report.targets += 1;
            report.passed += count_before(rest, "passed");
            report.failed_count += count_before(rest, "failed");
            report.ignored += count_before(rest, "ignored");
        }
        if line.starts_with("error: test failed, to rerun pass") {
            last_rerun_hint = Some(i);
        }
        if line.starts_with("error: ") && line.contains(" targets failed") {
            targets_failed_summary = true;
        }
        if line == "failures:" {
            in_failure_list = true;
            i += 1;
            continue;
        }
        if in_failure_list {
            match line.strip_prefix("    ") {
                Some(name) if !name.trim().is_empty() && !name.contains(' ') => {
                    names_in_lists.push(name.trim().to_string())
                }
                _ if line.trim().is_empty() => {}
                _ => in_failure_list = false,
            }
        }
        if let Some(name) = line
            .strip_prefix("---- ")
            .and_then(|r| r.strip_suffix(" stdout ----"))
        {
            let block_end = (i + 1..lines.len())
                .find(|&j| lines[j].starts_with("---- ") || lines[j] == "failures:")
                .unwrap_or(lines.len());
            let block = &lines[i + 1..block_end];
            let panic_at = block.iter().position(|l| l.contains("panicked at "));
            let (location, message) = match panic_at {
                Some(p) => (
                    panic_location(&block[p]),
                    block[p + 1..]
                        .iter()
                        .take_while(|l| !l.trim().is_empty() && !l.starts_with("note: "))
                        .take(PANIC_MESSAGE_LINES)
                        .map(|l| l.trim().to_string())
                        .collect(),
                ),
                None => (
                    None,
                    block
                        .iter()
                        .filter(|l| !l.trim().is_empty())
                        .take(PANIC_MESSAGE_LINES)
                        .map(|l| l.trim().to_string())
                        .collect(),
                ),
            };
            if !report.failed.iter().any(|t| t.name == name) {
                report.failed.push(FailedTest {
                    name: name.to_string(),
                    location,
                    message,
                });
            }
        }
        i += 1;
    }
    for name in names_in_lists {
        if !report.failed.iter().any(|t| t.name == name) {
            report.failed.push(FailedTest {
                name,
                ..FailedTest::default()
            });
        }
    }
    report.stopped_at_first_failing_target = last_rerun_hint.is_some() && !targets_failed_summary;
    report
}

pub fn mask(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut digits = false;
    for c in line.chars() {
        if c.is_ascii_digit() {
            if !digits {
                out.push('0');
            }
            digits = true;
        } else {
            digits = false;
            out.push(c);
        }
    }
    out
}

pub fn generic_blocks(
    lines: &[String],
    known: &HashSet<String>,
    budget: usize,
) -> Vec<(usize, usize)> {
    let hits: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| is_error_line(l) && !known.contains(&mask(l)))
        .map(|(i, _)| i)
        .collect();
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    for hit in hits {
        let start = hit.saturating_sub(CONTEXT_BEFORE);
        let end = (hit + CONTEXT_AFTER + 1).min(lines.len());
        match blocks.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => blocks.push((start, end)),
        }
    }
    let mut chosen = Vec::new();
    let mut used = 0;
    for block in blocks.into_iter().rev() {
        let size = block.1 - block.0;
        if used + size > budget {
            if used == 0 {
                chosen.push((block.1 - budget, block.1));
            }
            break;
        }
        used += size;
        chosen.push(block);
    }
    chosen.reverse();
    chosen
}

pub struct Context<'a> {
    pub id: u64,
    pub succeeded: bool,
    pub command: &'a str,
    pub known: &'a HashSet<String>,
}

fn capped(items: Vec<String>, cap: usize, noun: &str) -> Vec<String> {
    let total = items.len();
    let mut out: Vec<String> = items.into_iter().take(cap).collect();
    if total > cap {
        out.push(format!("+{} more {noun}", total - cap));
    }
    out
}

pub fn relevant(lines: &[String], context: &Context) -> Vec<String> {
    if lines.len() <= LINE_CAP {
        return lines.to_vec();
    }
    let diagnostics = rust_diagnostics(lines);
    let tests = test_report(lines);
    let render = |level: Level, with_location: bool| -> Vec<String> {
        diagnostics
            .iter()
            .filter(|d| d.level == level && d.location.is_some() == with_location)
            .map(Diagnostic::render)
            .collect()
    };
    let located_errors = render(Level::Error, true);
    let explained = !located_errors.is_empty() || !tests.failed.is_empty();
    let mut out = Vec::new();
    out.extend(capped(located_errors, ERROR_LINES, "errors"));
    if !explained {
        out.extend(capped(render(Level::Error, false), ERROR_LINES, "errors"));
    }
    out.extend(capped(
        tests.failed.iter().map(FailedTest::render).collect(),
        FAILED_TEST_LINES,
        "failed tests",
    ));
    if tests.targets > 0 {
        out.push(format!(
            "tests: {} passed, {} failed, {} ignored, in {} targets",
            tests.passed, tests.failed_count, tests.ignored, tests.targets
        ));
    }
    if tests.stopped_at_first_failing_target && !context.command.contains("--no-fail-fast") {
        out.push(
            "cargo stops at the first failing test target unless given --no-fail-fast, so later targets, if any, did not run"
                .to_string(),
        );
    }
    out.extend(capped(
        render(Level::Warning, true),
        WARNING_LINES,
        "warnings",
    ));
    let structured = !out.is_empty();
    let mut shown_until = 0;
    if !structured && !context.succeeded {
        for (start, end) in generic_blocks(lines, context.known, LINE_CAP - LAST_LINES) {
            out.push(format!("[lines {}..{}]", start + 1, end));
            out.extend(lines[start..end].iter().cloned());
            shown_until = end;
        }
    }
    let tail_start = lines.len().saturating_sub(LAST_LINES).max(shown_until);
    if tail_start < lines.len() {
        out.push(format!("[last lines, from {}]", tail_start + 1));
        out.extend(lines[tail_start..].iter().cloned());
    }
    out.push(format!(
        "[the log has {} lines: job log {} errors | lines A..B | full]",
        lines.len(),
        context.id
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<String> {
        let path = format!("{}/tests/fixtures/{name}.txt", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn context(known: &HashSet<String>) -> Context<'_> {
        Context {
            id: 7,
            succeeded: false,
            command: "cargo test",
            known,
        }
    }

    #[test]
    fn full_errors_carry_code_location_and_label() {
        let found = rust_diagnostics(&fixture("build-fail-full"));
        let errors: Vec<String> = found
            .iter()
            .filter(|d| d.location.is_some())
            .map(Diagnostic::render)
            .collect();
        assert_eq!(
            errors,
            vec![
                "src/lib.rs:2:52: error[E0308]: mismatched types: expected `u32`, found `&str`",
                "src/lib.rs:3:17: error[E0425]: cannot find function `undefined_fn` in this scope: not found in this scope",
            ]
        );
    }

    #[test]
    fn short_errors_read_the_same_as_full_ones() {
        let found = rust_diagnostics(&fixture("build-fail-short"));
        let located: Vec<String> = found
            .iter()
            .filter(|d| d.location.is_some())
            .map(Diagnostic::render)
            .collect();
        assert_eq!(
            located[0],
            "src/lib.rs:2:52: error[E0308]: mismatched types: expected `u32`, found `&str`"
        );
        assert_eq!(located.len(), 2);
    }

    #[test]
    fn cargo_summaries_are_errors_without_a_location() {
        let found = rust_diagnostics(&fixture("build-fail-full"));
        assert!(
            found
                .iter()
                .any(|d| d.location.is_none() && d.message.starts_with("could not compile"))
        );
    }

    #[test]
    fn warnings_are_found_in_both_forms() {
        assert_eq!(
            rust_diagnostics(&fixture("build-warn-full"))
                .iter()
                .filter(|d| d.location.is_some())
                .count(),
            3
        );
        assert_eq!(
            rust_diagnostics(&fixture("clippy-short"))
                .iter()
                .filter(|d| d.location.is_some())
                .count(),
            3
        );
    }

    #[test]
    fn failed_tests_carry_their_panic_location_and_message() {
        let report = test_report(&fixture("test-fail-full"));
        assert_eq!(
            report.failed[0].render(),
            "FAILED tests::fails_eq at src/lib.rs:6:53: assertion `left == right` failed; left: 4; right: 5"
        );
        assert_eq!(
            report.failed[1].render(),
            "FAILED tests::fails_panic at src/lib.rs:7:67: index out of bounds: the len is 0 but the index is 3"
        );
        assert_eq!(
            (report.passed, report.failed_count, report.targets),
            (1, 2, 1)
        );
    }

    #[test]
    fn a_run_that_stopped_at_the_first_failing_target_says_so() {
        assert!(test_report(&fixture("test-multibin-failfast")).stopped_at_first_failing_target);
        let all = test_report(&fixture("test-multibin-nofailfast"));
        assert!(!all.stopped_at_first_failing_target);
        assert_eq!(all.failed.len(), 3);
        assert_eq!(all.targets, 3);
    }

    #[test]
    fn a_long_failing_test_run_answers_in_a_handful_of_lines() {
        let mut lines = fixture("test-multibin-nofailfast");
        let noise: Vec<String> = (0..200)
            .map(|i| format!("test module::case_{i} ... ok"))
            .collect();
        lines.splice(0..0, noise);
        let known = HashSet::new();
        let answer = relevant(&lines, &context(&known));
        assert!(answer.len() <= 15, "{answer:#?}");
        assert!(
            answer
                .iter()
                .any(|l| l.starts_with("FAILED integ_fails at tests/it.rs:1:78")),
            "{answer:#?}"
        );
        assert!(
            answer
                .iter()
                .any(|l| l.starts_with("tests: 1 passed, 3 failed")),
            "{answer:#?}"
        );
    }

    #[test]
    fn a_compile_error_in_a_test_run_is_named_with_its_location() {
        let mut lines = fixture("test-compile-error-short");
        lines.splice(
            0..0,
            (0..100).map(|i| format!("   Compiling crate{i} v0.1.0")),
        );
        let known = HashSet::new();
        let answer = relevant(&lines, &context(&known));
        assert_eq!(
            answer[0],
            "tests/it.rs:2:16: error[E0308]: mismatched types: expected `u8`, found `&str`"
        );
    }

    #[test]
    fn other_commands_show_the_blocks_around_error_words_nearest_the_end() {
        let mut lines: Vec<String> = (0..300).map(|i| format!("step {i} done")).collect();
        lines[100] = "ERROR: early problem".to_string();
        lines[250] = "fatal: the real problem".to_string();
        let known = HashSet::new();
        let answer = relevant(
            &lines,
            &Context {
                id: 1,
                succeeded: false,
                command: "make",
                known: &known,
            },
        );
        assert!(answer.contains(&"fatal: the real problem".to_string()));
        assert!(answer.contains(&"ERROR: early problem".to_string()));
        assert!(answer.len() <= LINE_CAP + 5, "{}", answer.len());
    }

    #[test]
    fn lines_seen_in_successful_runs_are_not_error_hits() {
        let mut lines: Vec<String> = (0..300).map(|i| format!("step {i} done")).collect();
        lines[100] = "warning: missing optional file 17".to_string();
        lines[250] = "fatal: the real problem".to_string();
        let known: HashSet<String> = [mask("warning: missing optional file 3")]
            .into_iter()
            .collect();
        let answer = relevant(
            &lines,
            &Context {
                id: 1,
                succeeded: false,
                command: "make",
                known: &known,
            },
        );
        assert!(
            !answer.iter().any(|l| l.contains("missing optional")),
            "{answer:#?}"
        );
        assert!(answer.contains(&"fatal: the real problem".to_string()));
    }

    #[test]
    fn a_short_output_is_shown_whole() {
        let lines = vec!["one".to_string(), "error: two".to_string()];
        let known = HashSet::new();
        assert_eq!(relevant(&lines, &context(&known)), lines);
    }

    #[test]
    fn masking_turns_every_number_into_one_zero() {
        assert_eq!(
            mask("finished in 12.50s, 3 of 400"),
            "finished in 0.0s, 0 of 0"
        );
    }
}
