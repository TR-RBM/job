use crate::logfile::read_lines;
use crate::model::Job;

pub const ERROR_WORDS: [&str; 13] = [
    "fatal",
    "fail",
    "panic",
    "error",
    "exit",
    "kill",
    "no such file",
    "err:",
    "err!",
    "failures:",
    "missing",
    "exception",
    "cannot",
];

pub fn is_error_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    ERROR_WORDS.iter().any(|w| lower.contains(w))
}

fn numbered(lines: &[String], range: std::ops::Range<usize>) -> Vec<String> {
    range
        .filter_map(|i| lines.get(i).map(|l| format!("{:>6}  {l}", i + 1)))
        .collect()
}

pub fn run(job: &Job, words: &[String]) -> Result<String, String> {
    let lines = read_lines(&job.log)
        .map_err(|e| format!("the log of job {} cannot be read: {e}", job.id))?;
    let lines = if job.spec.declared.terminal.is_some() {
        lines
            .iter()
            .map(|line| crate::terminal::printable(line))
            .collect::<Vec<_>>()
    } else {
        lines
    };
    let out: Vec<String> = match words
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] | ["full"] => lines.clone(),
        ["errors"] => lines
            .iter()
            .enumerate()
            .filter(|(_, l)| is_error_line(l))
            .map(|(i, l)| format!("{:>6}  {l}", i + 1))
            .collect(),
        ["grep", pattern] => lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.contains(pattern))
            .map(|(i, l)| format!("{:>6}  {l}", i + 1))
            .collect(),
        ["tail", n] => {
            let n: usize = n
                .parse()
                .map_err(|_| format!("`{n}` is not a number of lines"))?;
            numbered(&lines, lines.len().saturating_sub(n)..lines.len())
        }
        ["lines", range] => {
            let (a, b) = range
                .split_once("..")
                .ok_or_else(|| format!("`{range}` is not a range such as 10..40"))?;
            let a: usize = a
                .parse()
                .map_err(|_| format!("`{a}` is not a line number"))?;
            let b: usize = b
                .parse()
                .map_err(|_| format!("`{b}` is not a line number"))?;
            numbered(&lines, a.saturating_sub(1)..b.min(lines.len()))
        }
        other => {
            return Err(format!(
                "unknown query `{}`; use errors, grep PATTERN, lines A..B, tail N or full",
                other.join(" ")
            ));
        }
    };
    if out.is_empty() {
        return Ok(format!(
            "job {}: nothing in {} lines matches",
            job.id,
            lines.len()
        ));
    }
    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_lines_are_found_by_the_keywords_of_logsage() {
        assert!(is_error_line("error[E0308]: mismatched types"));
        assert!(is_error_line("thread 'main' panicked at src/lib.rs:6:5"));
        assert!(!is_error_line("   Compiling job v0.1.0"));
    }
}
