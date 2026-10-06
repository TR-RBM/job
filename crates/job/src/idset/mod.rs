use serde::{Deserialize, Serialize};

pub mod client;
mod messages;

pub use messages::message;

pub const LIMIT: u64 = 10_000;
pub const COMPACT_FROM: usize = 20;
pub const KIND: &str = "job_set";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Set {
    pub written: String,
    pub ids: Vec<u64>,
    pub explicit: Vec<u64>,
}

fn refused(key: &str, word: &str) -> String {
    message(key)
        .replace("{word}", word)
        .replace("{limit}", &LIMIT.to_string())
}

fn number(text: &str, item: &str) -> Result<u64, String> {
    let invalid = || {
        refused(
            "`{word}` is not a Job ID; write a positive number, a range such as 1-10 or a list such as 1,4,7-9",
            item,
        )
    };
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    text.parse::<u64>()
        .ok()
        .filter(|id| *id != 0)
        .ok_or_else(invalid)
}

pub fn written(operands: &[&str]) -> bool {
    operands.len() > 1
        || operands
            .iter()
            .any(|word| word.contains('-') || word.contains(','))
}

pub fn parse(operands: &[&str]) -> Result<Set, String> {
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    let mut explicit: Vec<u64> = Vec::new();
    let mut named: u64 = 0;
    for operand in operands {
        for item in operand.split(',') {
            if item.is_empty() {
                return Err(refused(
                    "`{word}` has an empty item; write IDs and ranges with one comma between them",
                    operand,
                ));
            }
            let (first, last) = match item.split_once('-') {
                Some((first, last)) if first.is_empty() || last.is_empty() => {
                    return Err(refused(
                        "`{word}` is an open range; write both ends, as in 5-10",
                        item,
                    ));
                }
                Some((first, last)) => (number(first, item)?, number(last, item)?),
                None => {
                    let id = number(item, item)?;
                    explicit.push(id);
                    (id, id)
                }
            };
            if first > last {
                return Err(refused(
                    "`{word}` is a reversed range; write the lower ID first",
                    item,
                ));
            }
            named = named.saturating_add((last - first).saturating_add(1));
            if named > LIMIT {
                return Err(refused(
                    "the set names more than {limit} IDs; split it into several calls",
                    "",
                ));
            }
            ranges.push((first, last));
        }
    }
    if ranges.is_empty() {
        return Err(message("a set of Job IDs is needed"));
    }
    let mut ids: Vec<u64> = ranges
        .into_iter()
        .flat_map(|(first, last)| first..=last)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    explicit.sort_unstable();
    explicit.dedup();
    Ok(Set {
        written: operands.join(" "),
        ids,
        explicit,
    })
}

pub fn ranges(ids: &[u64]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut index = 0;
    while index < ids.len() {
        let mut end = index;
        while end + 1 < ids.len() && ids[end + 1] == ids[end] + 1 {
            end += 1;
        }
        parts.push(if end == index {
            ids[index].to_string()
        } else {
            format!("{}-{}", ids[index], ids[end])
        });
        index = end + 1;
    }
    parts.join(", ")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Cancel {
        session: String,
    },
    Remove {
        allow_lost: bool,
    },
    Release,
    Retry {
        held: bool,
        env: Option<crate::model::Env>,
        queue: Option<String>,
        allow_lost: bool,
    },
    Freeze {
        frozen: bool,
        timeout_ms: u64,
    },
    Signal {
        signal: i32,
    },
    Reprioritize {
        priority: i32,
    },
    Move {
        queue: String,
    },
    Wait {
        timeout_ms: u64,
    },
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Action::Cancel { .. } => "cancel",
            Action::Remove { .. } => "remove",
            Action::Release => "release",
            Action::Retry { .. } => "retry",
            Action::Freeze { frozen: true, .. } => "suspend",
            Action::Freeze { .. } => "continue",
            Action::Signal { .. } => "signal",
            Action::Reprioritize { .. } => "reprioritize",
            Action::Move { .. } => "move",
            Action::Wait { .. } => "wait",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    pub action: Action,
    pub operands: Vec<String>,
    #[serde(default)]
    pub dry_run: bool,
}

impl Call {
    pub fn read_only(&self) -> bool {
        self.dry_run || matches!(self.action, Action::Wait { .. })
    }

    pub fn audited(&self) -> Option<(&'static str, String)> {
        (!self.read_only()).then(|| (self.action.name(), self.operands.join(" ")))
    }
}

pub fn unhindered(request: &crate::model::Request) -> bool {
    match request {
        crate::model::Request::Versioned { request, .. } => unhindered(request),
        crate::model::Request::Extended {
            call: crate::cli2::Call::Set { call },
        } => matches!(
            call.action,
            Action::Cancel { .. } | Action::Signal { .. } | Action::Freeze { .. }
        ),
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Pending,
    Refused,
    Failed,
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    pub outcome: Outcome,
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_status: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub kind: String,
    pub action: String,
    pub set: String,
    pub dry_run: bool,
    pub selected: usize,
    pub results: Vec<Entry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
}

impl Report {
    pub fn exit_status(&self) -> u8 {
        if self.action == "wait" {
            return self
                .results
                .iter()
                .map(|entry| match entry.outcome {
                    Outcome::Done => entry.exit_status.unwrap_or(1),
                    Outcome::Pending => crate::EXIT_STILL_RUNNING,
                    _ => 1,
                })
                .find(|status| *status != 0)
                .unwrap_or(u8::from(self.selected == 0));
        }
        if self.selected == 0
            || self.results.iter().any(|entry| {
                matches!(
                    entry.outcome,
                    Outcome::Refused | Outcome::Failed | Outcome::Missing
                )
            })
        {
            1
        } else if self
            .results
            .iter()
            .any(|entry| entry.outcome == Outcome::Pending)
        {
            crate::EXIT_STILL_RUNNING
        } else {
            0
        }
    }

    pub fn counts(&self) -> String {
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for entry in &self.results {
            match counts.iter_mut().find(|(word, _)| *word == entry.result) {
                Some((_, count)) => *count += 1,
                None => counts.push((&entry.result, 1)),
            }
        }
        counts
            .iter()
            .map(|(word, count)| format!("{word} {count}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn audited(&self) -> (String, Option<String>) {
        let counts = self.counts();
        let word = match self.exit_status() {
            0 => "ok",
            crate::EXIT_STILL_RUNNING => "pending",
            _ => "error",
        };
        (
            if counts.is_empty() {
                format!("{word}: no existing Job")
            } else {
                format!("{word}: {counts}")
            },
            self.operation.clone(),
        )
    }
}
