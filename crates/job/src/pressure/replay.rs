use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::control::{Rule, Signal, View, validate};
use super::message;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Trace {
    schema_version: u32,
    host: bool,
    rule: Rule,
    frames: Vec<Frame>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    boot_id: String,
    at_ms: u64,
    signal: Signal,
    active: u64,
    demand: u64,
    #[serde(default)]
    restart: bool,
}

#[derive(Serialize)]
pub struct Sample {
    boot_id: String,
    active: u64,
    demand: u64,
    admission_held: bool,
    view: View,
}

#[derive(Serialize)]
pub struct Report {
    schema_version: u32,
    samples: Vec<Sample>,
}

pub fn run(path: &Path) -> io::Result<Report> {
    let mut bytes = Vec::new();
    File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?
        .take(1_048_577)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1_048_576 {
        return Err(io::Error::other(message(
            "pressure trace exceeds the size limit",
        )));
    }
    let trace: Trace = serde_json::from_slice(&bytes)?;
    validate(std::slice::from_ref(&trace.rule), trace.host).map_err(io::Error::other)?;
    if trace.schema_version != 1 || trace.frames.is_empty() || trace.frames.len() > 4096 {
        return Err(io::Error::other(message("invalid pressure trace")));
    }
    let mut view = View::new(
        (!trace.host).then_some(0),
        trace.rule,
        trace.frames[0].at_ms,
    );
    view.scope_path = if trace.host { "host" } else { "replay" }.to_owned();
    let mut samples = Vec::new();
    let mut prior_boot = None;
    let mut prior_time = 0;
    let mut boots = BTreeSet::new();
    for frame in trace.frames {
        if frame.boot_id.is_empty()
            || frame.boot_id.len() > 128
            || frame.demand < frame.active
            || frame.active == u64::MAX
        {
            return Err(io::Error::other(message("invalid pressure trace")));
        }
        if let Signal::Available {
            value_bp,
            job_id,
            attempt,
            missing,
        } = &frame.signal
        {
            if *value_bp > 10000
                || job_id.is_some() != attempt.is_some()
                || *job_id == Some(0)
                || *attempt == Some(0)
                || (trace.host && (job_id.is_some() || *missing != 0))
                || (!trace.host && (frame.active == 0 || *missing >= frame.active))
            {
                return Err(io::Error::other(message("invalid pressure trace")));
            }
        } else if (trace.host || frame.active != 0) && matches!(frame.signal, Signal::Empty) {
            return Err(io::Error::other(message("invalid pressure trace")));
        }
        let reboot = prior_boot.as_ref().is_some_and(|old| *old != frame.boot_id);
        if prior_boot.as_ref().is_some_and(|old| *old == frame.boot_id) && frame.at_ms <= prior_time
        {
            return Err(io::Error::other(message(
                "pressure trace time must increase within a boot",
            )));
        }
        if prior_boot.as_ref() != Some(&frame.boot_id) && !boots.insert(frame.boot_id.clone()) {
            return Err(io::Error::other(message(
                "pressure trace revisits an earlier boot",
            )));
        }
        if reboot || frame.restart {
            view.restart(frame.at_ms, reboot);
        }
        view.advance(frame.signal, frame.at_ms, frame.active, frame.demand);
        prior_boot = Some(frame.boot_id.clone());
        prior_time = frame.at_ms;
        samples.push(Sample {
            boot_id: frame.boot_id,
            active: frame.active,
            demand: frame.demand,
            admission_held: view
                .temporary_max_running
                .is_some_and(|cap| frame.active >= cap),
            view: view.clone(),
        });
    }
    Ok(Report {
        schema_version: 1,
        samples,
    })
}
