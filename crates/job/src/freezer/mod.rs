use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suspension {
    pub generation: u64,
    pub requested: bool,
    pub pending: bool,
    pub since_ms: Option<u64>,
    pub total_ms: u64,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_uid: Option<u32>,
}

impl Suspension {
    pub fn observe(&mut self, frozen: bool, now: u64) {
        if frozen {
            self.since_ms.get_or_insert(now);
        } else if let Some(start) = self.since_ms.take() {
            self.total_ms = self.total_ms.saturating_add(now.saturating_sub(start));
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timing {
    pub elapsed_ms: u64,
    pub active_ms: u64,
    pub suspended_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    Job {
        id: u64,
    },
    Object {
        kind: crate::objects::Kind,
        path: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub id: u64,
    pub attempt: u64,
    pub generation: u64,
    pub state: crate::model::State,
    pub accepted: bool,
    pub confirmed: bool,
    pub error: Option<String>,
}

pub fn requested(path: &Path) -> io::Result<bool> {
    match fs::read_to_string(path.join("cgroup.freeze"))?.trim() {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(io::Error::other(message("invalid cgroup freezer value"))),
    }
}

pub fn observed(path: &Path) -> io::Result<bool> {
    let events = fs::read_to_string(path.join("cgroup.events"))?;
    for line in events.lines() {
        let mut fields = line.split_whitespace();
        if fields.next() == Some("frozen") {
            return match (fields.next(), fields.next()) {
                (Some("0"), None) => Ok(false),
                (Some("1"), None) => Ok(true),
                _ => Err(io::Error::other(message("invalid cgroup freezer value"))),
            };
        }
    }
    Err(io::Error::other(message(
        "cgroup freezer confirmation is unavailable",
    )))
}

pub fn write(path: &Path, frozen: bool) -> io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path.join("cgroup.freeze"))?
        .write_all(if frozen { b"1" } else { b"0" })
}

const CATALOGUE: &[(&str, &str)] = &[
    ("elapsed", "vergangen"),
    ("active", "aktiv"),
    ("suspended", "angehalten"),
    (
        "invalid cgroup freezer value",
        "ungültiger cgroup-Freezer-Wert",
    ),
    (
        "cgroup freezer confirmation is unavailable",
        "die cgroup-Freezer-Bestätigung ist nicht verfügbar",
    ),
    (
        "suspension requires a local cgroup v2 workload",
        "das Anhalten erfordert einen lokalen Workload in cgroup v2",
    ),
    (
        "only running or suspended Jobs can be controlled",
        "du kannst nur laufende oder angehaltene Jobs so steuern",
    ),
    (
        "collection suspension requires --recursive",
        "zum Anhalten oder Fortsetzen einer Sammlung brauchst du --recursive",
    ),
    (
        "no such control target",
        "dieses Steuerungsziel existiert nicht",
    ),
    (
        "previous control request was superseded",
        "die vorherige Steuerungsanfrage wurde ersetzt",
    ),
    (
        "execution ended before confirmation",
        "die Ausführung endete vor der Bestätigung",
    ),
    (
        "request saved; kernel control failed",
        "Anfrage gespeichert; Kernel-Steuerung fehlgeschlagen",
    ),
    (
        "kernel state changed; recording failed",
        "Kernel-Zustand geändert; Speicherung fehlgeschlagen",
    ),
    (
        "pending kernel confirmation",
        "Kernel-Bestätigung steht aus",
    ),
    ("confirmed", "bestätigt"),
    (
        "recovery requires the original cgroup backend for suspended work",
        "zur Wiederherstellung angehaltener Arbeit brauchst du das ursprüngliche cgroup-Backend",
    ),
    (
        "usage: job suspend|continue ID [--timeout DURATION] [--json]",
        "Aufruf: job suspend|continue ID [--timeout DAUER] [--json]",
    ),
    (
        "usage: job queue|group suspend|continue --recursive PATH [--timeout DURATION] [--json]",
        "Aufruf: job queue|group suspend|continue --recursive PFAD [--timeout DAUER] [--json]",
    ),
];

pub fn message(english: &str) -> String {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty())
        .unwrap_or_default();
    if locale.starts_with("de") {
        CATALOGUE
            .iter()
            .find(|(key, _)| *key == english)
            .map_or(english, |(_, value)| *value)
            .to_owned()
    } else {
        english.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requesting_a_freeze_does_not_confirm_it() {
        let directory =
            std::env::temp_dir().join(format!("job-freezer-confirm-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("cgroup.freeze"), "0").unwrap();
        fs::write(directory.join("cgroup.events"), "populated 1\nfrozen 0\n").unwrap();
        write(&directory, true).unwrap();
        assert!(requested(&directory).unwrap());
        assert!(!observed(&directory).unwrap());
        fs::write(directory.join("cgroup.events"), "populated 1\nfrozen 1\n").unwrap();
        assert!(observed(&directory).unwrap());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn missing_freezer_support_cannot_be_reported_as_thawed() {
        let directory =
            std::env::temp_dir().join(format!("job-freezer-missing-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("cgroup.events"), "populated 1\n").unwrap();
        assert!(observed(&directory).is_err());
        assert!(write(&directory, false).is_err());
        assert!(!directory.join("cgroup.freeze").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn suspension_accounting_accumulates_only_observed_intervals() {
        let mut suspension = Suspension::default();
        suspension.observe(true, 10);
        suspension.observe(true, 20);
        suspension.observe(false, 40);
        suspension.observe(false, 50);
        suspension.observe(true, 70);
        suspension.observe(false, 100);
        assert_eq!(suspension.total_ms, 60);
        assert_eq!(suspension.since_ms, None);
    }
}
