use std::collections::HashSet;

use crate::filter;
use crate::logfile::read_lines;
use crate::model::{Job, State};
use crate::resources::format_cores;
use crate::units::{format_bytes, format_duration_ms};

pub fn header(job: &Job) -> String {
    let waited = job
        .durability
        .admitted_ms
        .or(job.started_ms)
        .map(|s| s.saturating_sub(job.waiting_since()))
        .unwrap_or(0);
    let ran = match (job.started_ms, job.finished_ms) {
        (Some(s), Some(f)) => f.saturating_sub(s),
        _ => 0,
    };
    let queue = job
        .spec
        .queue
        .as_ref()
        .filter(|q| q.as_str() != "default")
        .map(|q| format!(" in queue {q}"))
        .unwrap_or_default();
    let mut parts = vec![
        format!("[job] job {}{queue}: {}", job.id, job.exit_description()),
        format!("ran {}", format_duration_ms(ran)),
        format!("peak {}", format_bytes(job.usage.peak_memory)),
    ];
    if job.timing.suspended_ms > 0 {
        parts[1] = format!(
            "{} {}",
            crate::freezer::message("elapsed"),
            format_duration_ms(job.timing.elapsed_ms)
        );
        parts.push(format!(
            "{} {}",
            crate::freezer::message("active"),
            format_duration_ms(job.timing.active_ms)
        ));
        parts.push(format!(
            "{} {}",
            crate::freezer::message("suspended"),
            format_duration_ms(job.timing.suspended_ms)
        ));
    }
    if waited >= 1000 {
        let reason = job
            .waited_for
            .as_ref()
            .map(|r| format!(" for {r}"))
            .unwrap_or_default();
        parts.push(format!("queued {}{reason}", format_duration_ms(waited)));
    }
    if job.usage.throttled_ms >= 1000 {
        parts.push(format!(
            "throttled {} at its {}",
            format_duration_ms(job.usage.throttled_ms),
            format_cores(job.reservation.vector.cores_milli)
        ));
    }
    if job.spec.declared.confine {
        parts.push("confined".to_string());
    }
    if matches!(job.spec.declared.net, Some(crate::model::Net::None)) {
        parts.push("no network".to_string());
    }
    if let Some(network) = &job.network {
        parts.push(network.selected.clone());
    }
    if let Some(monitor) = job.result.as_ref().and_then(|r| r.monitor.as_ref()) {
        parts.push(format!("on monitor {monitor}"));
    }
    if let Some(link) = &job.link {
        if let Some(egress) = link.egress.as_ref().filter(|e| e.needs_link()) {
            parts.push(egress.describe());
        }
        match (link.cap, link.link_rate) {
            (Some(cap), Some(total)) if cap < total => parts.push(format!(
                "network capped at {} each way within {}",
                crate::units::format_rate(cap),
                crate::units::format_rate(total)
            )),
            (Some(cap), _) => parts.push(format!(
                "network capped at {} each way",
                crate::units::format_rate(cap)
            )),
            _ => {}
        }
    }
    match job.result.as_ref().and_then(|r| r.remote.as_ref()) {
        Some(remote) => parts.push(remote.clone()),
        None => parts.push(job.backend.describe().to_string()),
    }
    parts.join(", ")
}

pub fn pending(job: &Job, now: u64, position: Option<usize>) -> String {
    let what = match job.state {
        State::Held => format!("[job] {}: {}", job.id, crate::lifecycle::message("held")),
        State::Starting => format!(
            "[job] {}: {}",
            job.id,
            crate::lifecycle::message("starting")
        ),
        State::Stopping => format!(
            "[job] {}: {}",
            job.id,
            crate::lifecycle::message("stopping")
        ),
        State::Suspended => format!(
            "[job] {}: {}",
            job.id,
            crate::lifecycle::message("suspended")
        ),
        State::Queued => {
            let place = position
                .map(|p| format!(", position {p}"))
                .unwrap_or_default();
            format!(
                "[job] job {} is queued{place} for {}",
                job.id,
                format_duration_ms(now.saturating_sub(job.waiting_since()))
            )
        }
        _ => format!(
            "[job] job {} is still running, for {}",
            job.id,
            format_duration_ms(
                now.saturating_sub(job.started_ms.or(job.durability.admitted_ms).unwrap_or(now))
            )
        ),
    };
    let mut reason = match (&job.state, &job.waited_for) {
        (State::Queued, Some(reason)) => format!(", waiting for {reason}"),
        _ => String::new(),
    };
    if let Some(error) = &job.suspension.error {
        reason.push_str(&format!(", {error}"));
    } else if job.suspension.pending {
        reason.push_str(&format!(
            ", {}",
            crate::freezer::message("pending kernel confirmation")
        ));
    }
    format!(
        "{what}{reason}. Wait for it in the background with: job wait {}",
        job.id
    )
}

pub fn render(job: &Job, known: &HashSet<String>) -> String {
    let mut out = vec![header(job)];
    if let Some(stop) = &job.stop {
        out.push(stop.line.clone());
    }
    if let Some(result) = &job.result
        && result.leftover_processes > 0
        && job.stop.is_none()
    {
        out.push(format!(
            "{} processes were still running when the command ended and were killed: {}",
            result.leftover_processes,
            result.leftover_names.join("; ")
        ));
    }
    if let Some(result) = &job.result {
        out.extend(result.remote_lines.iter().cloned());
        out.extend(result.notes.iter().map(|note| format!("note: {note}")));
        out.extend(result.windows.iter().cloned());
        if !result.kept_helpers.is_empty() {
            out.push(format!(
                "note: kept running for later jobs: {}",
                result.kept_helpers.join("; ")
            ));
        }
        out.extend(crate::streams::quota::summary(job));
        if result.output_held_open {
            out.push(
                "note: a process kept the output open after the command ended; the log may lack its end"
                    .to_string(),
            );
        }
    }
    let lines = read_lines(&job.log).unwrap_or_default();
    let lines = if job.spec.declared.terminal.is_some() {
        lines
            .iter()
            .map(|line| crate::terminal::printable(line))
            .collect::<Vec<_>>()
    } else {
        lines
    };

    if job.spec.declared.confine
        && lines
            .iter()
            .any(|l| crate::confine::REFUSAL_WORDS.iter().any(|w| l.contains(w)))
    {
        out.push(format!(
            "note: this job was confined, and its output reports a refused write; if it needed to write there, run it with --allow-write PATH (job log {} grep 'denied')",
            job.id
        ));
    }
    let command = crate::estimate::command_text(&job.spec.argv);
    let context = filter::Context {
        id: job.id,
        succeeded: job.succeeded(),
        command: &command,
        known,
    };
    out.extend(filter::relevant(&lines, &context));
    out.join("\n")
}
