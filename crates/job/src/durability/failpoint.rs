use std::io;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

pub const NAMES: [&str; 20] = [
    "launch-confirm-delay",
    "audit-partial-write",
    "audit-before-end",
    "liveness-unknown",
    "submit-after-index",
    "submit-after-publish",
    "launch-after-starting",
    "launch-after-spawn",
    "launch-identity-save",
    "launch-after-identity",
    "finalize-save",
    "shim-after-exit-record",
    "shim-before-result",
    "shim-result-save",
    "cancel-batch-after-stage",
    "cancel-batch-after-publish",
    "cancel-batch-sync",
    "cancel-batch-rename",
    "cancel-batch-mid-rename",
    "starter-panic",
];

struct Point {
    name: String,
    hit: Option<u64>,
    seen: AtomicU64,
}

static POINTS: OnceLock<Vec<Point>> = OnceLock::new();

fn parse(text: &str) -> Vec<Point> {
    text.split(',')
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .map(|word| {
            let (name, hit) = match word.split_once('@') {
                Some((name, hit)) => (name, hit.parse().ok()),
                None => (word, None),
            };
            Point {
                name: name.to_owned(),
                hit,
                seen: AtomicU64::new(0),
            }
        })
        .collect()
}

fn points() -> &'static [Point] {
    POINTS.get_or_init(|| {
        std::env::var("JOB_FAILPOINT")
            .map(|text| parse(&text))
            .unwrap_or_default()
    })
}

fn armed(name: &str) -> bool {
    points().iter().any(|point| {
        point.name == name && {
            let count = point.seen.fetch_add(1, Ordering::SeqCst) + 1;
            point.hit.is_none_or(|hit| hit == count)
        }
    })
}

pub fn check() -> Result<(), String> {
    match points()
        .iter()
        .find(|point| !NAMES.contains(&point.name.as_str()))
    {
        Some(point) => Err(format!(
            "{}: {}",
            super::message("unknown failpoint in JOB_FAILPOINT"),
            point.name
        )),
        None => Ok(()),
    }
}

pub fn failpoint(name: &str) {
    if armed(name) {
        unsafe {
            libc::kill(libc::getpid(), libc::SIGKILL);
            libc::_exit(137);
        }
    }
}

pub fn fails(name: &str) -> io::Result<()> {
    if armed(name) {
        Err(io::Error::other(format!(
            "{}: {name}",
            super::message("injected failure at failpoint")
        )))
    } else {
        Ok(())
    }
}

pub fn panics(name: &str) {
    if armed(name) {
        panic!(
            "{}: {name}",
            super::message("injected failure at failpoint")
        );
    }
}

pub fn pause(name: &str) {
    if armed(name) {
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
}
