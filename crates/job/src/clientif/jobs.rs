use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::daemon::Shared;
use crate::daemon::client_service;

use super::args::{self, Args};
use super::row::{Row, STATES};
use super::{base64, index, message};

const OP: &str = "jobs";
const ORDERS: [&str; 4] = ["activity", "id", "started", "ended"];
const TEXT: usize = 256;
const TOP: u64 = u64::MAX;

pub const PAGE: u64 = 1000;
pub const PAGE_DEFAULT: u64 = 200;
pub const IDS: usize = 10_000;

type Key = [u64; 5];

#[derive(Default)]
struct Filter {
    states: Option<Vec<&'static str>>,
    subtree: Option<String>,
    labels: Vec<(String, String)>,
    actor_uid: Option<u64>,
    session: Option<String>,
    text: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Before,
    After,
}

struct Place {
    side: Side,
    key: Key,
}

enum Cursor {
    Expired,
    At(Place),
}

pub struct Asked {
    order: &'static str,
    filter: Filter,
    limit: usize,
}

fn key(order: &str, row: &Row) -> Key {
    let late = |time: u64| TOP - time;
    match order {
        "id" => [row.id, 0, 0, 0, 0],
        "started" => match row.started_ms {
            Some(started) => [0, late(started), late(row.id), 0, 0],
            None => [1, late(row.id), 0, 0, 0],
        },
        "ended" => match row.finished_ms {
            Some(finished) => [0, late(finished), late(row.id), 0, 0],
            None => [1, late(row.id), 0, 0, 0],
        },
        _ => match row.state {
            "starting" | "running" | "suspended" | "stopping" => [0, row.id, 0, 0, 0],
            "queued" => [
                1,
                u64::from(row.predicted_start_ms.is_none()),
                row.predicted_start_ms.unwrap_or(0),
                row.waiting_since_ms,
                row.id,
            ],
            "held" => [2, row.id, 0, 0, 0],
            _ => [3, late(row.finished_ms.unwrap_or(0)), late(row.id), 0, 0],
        },
    }
}

impl Filter {
    fn read(value: Option<&Value>) -> Result<Self, String> {
        let wrong = |member: &str, wanted: &str| {
            message("`{member}` in the filter of jobs must be {wanted}")
                .replace("{member}", member)
                .replace("{wanted}", &message(wanted))
        };
        let members = match value {
            None => return Ok(Self::default()),
            Some(Value::Object(members)) => members,
            Some(_) => return Err(wrong("filter", "an object")),
        };
        let mut filter = Self::default();
        for (member, value) in members {
            match (member.as_str(), value) {
                ("states", Value::Array(names)) => {
                    let mut states = Vec::with_capacity(names.len());
                    for name in names {
                        let known = name
                            .as_str()
                            .and_then(|name| STATES.iter().find(|state| **state == name))
                            .ok_or_else(|| {
                                message("`states` in the filter of jobs holds {given}, which is not a state; the states are {states}")
                                    .replace("{given}", &name.to_string())
                                    .replace("{states}", &STATES.join(", "))
                            })?;
                        states.push(*known);
                    }
                    states.sort_unstable();
                    states.dedup();
                    filter.states = Some(states);
                }
                ("states", _) => return Err(wrong(member, "an array of strings")),
                ("subtree", Value::String(path)) => {
                    filter.subtree = Some(path.trim_matches('/').to_owned());
                }
                ("session", Value::String(session)) => filter.session = Some(session.clone()),
                ("subtree" | "session", _) => return Err(wrong(member, "a string")),
                ("labels", Value::Object(labels)) => {
                    for (name, wanted) in labels {
                        let Value::String(wanted) = wanted else {
                            return Err(wrong(member, "an object of strings"));
                        };
                        filter.labels.push((name.clone(), wanted.clone()));
                    }
                }
                ("labels", _) => return Err(wrong(member, "an object of strings")),
                ("actor_uid", value) => {
                    filter.actor_uid = Some(
                        value
                            .as_u64()
                            .ok_or_else(|| wrong(member, "a whole number that is not negative"))?,
                    );
                }
                ("text", Value::String(text)) if text.len() <= TEXT => {
                    filter.text = Some(text.clone());
                }
                ("text", _) => return Err(wrong(member, "a string of at most 256 bytes")),
                _ => {
                    return Err(message(
                        "the filter of jobs has no member `{member}`; it takes states, subtree, labels, actor_uid, session and text",
                    )
                    .replace("{member}", &member.chars().take(64).collect::<String>()));
                }
            }
        }
        Ok(filter)
    }

    fn digest(&self) -> String {
        let written = format!(
            "{:?}",
            (
                &self.states,
                &self.subtree,
                &self.labels,
                &self.actor_uid,
                &self.session,
                &self.text
            )
        );
        Sha256::digest(written.as_bytes())[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn matches(&self, row: &Row, joined: &mut String) -> bool {
        if let Some(states) = &self.states
            && !states.contains(&row.state)
        {
            return false;
        }
        if let Some(subtree) = &self.subtree {
            let Some(queue) = &row.queue else {
                return false;
            };
            let below = queue
                .strip_prefix(subtree.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));
            if !subtree.is_empty() && !below {
                return false;
            }
        }
        if self
            .labels
            .iter()
            .any(|(name, wanted)| row.labels.get(name) != Some(wanted))
        {
            return false;
        }
        if let Some(uid) = self.actor_uid
            && row.actor_uid.map(u64::from) != Some(uid)
        {
            return false;
        }
        if let Some(session) = &self.session
            && row.session != *session
        {
            return false;
        }
        if let Some(text) = &self.text
            && !row.holds(text, joined)
        {
            return false;
        }
        true
    }
}

fn sealed(order: &str, digest: &str, place: &Place) -> String {
    let key: Vec<String> = place.key.iter().map(u64::to_string).collect();
    base64::encoded(
        format!(
            "v1:{order}:{}:{digest}:{}:{}",
            crate::operations::health::start(),
            if place.side == Side::Before { "b" } else { "a" },
            key.join(".")
        )
        .as_bytes(),
    )
}

fn opened(text: &str, order: &str, digest: &str) -> Result<Cursor, String> {
    let unknown = || {
        message(
            "`cursor` in the arguments of jobs is not a cursor of this service; send one from an earlier answer unchanged",
        )
    };
    let bytes = base64::decoded(text).ok_or_else(unknown)?;
    let written = String::from_utf8(bytes).map_err(|_| unknown())?;
    let parts: Vec<&str> = written.split(':').collect();
    let [version, bound, run, filter, side, key] = parts.as_slice() else {
        return Err(unknown());
    };
    if *version != "v1" {
        return Err(unknown());
    }
    let numbers: Vec<u64> = key
        .split('.')
        .map(|number| number.parse::<u64>())
        .collect::<Result<_, _>>()
        .map_err(|_| unknown())?;
    let key: Key = numbers.try_into().map_err(|_| unknown())?;
    let side = match *side {
        "b" => Side::Before,
        "a" => Side::After,
        _ => return Err(unknown()),
    };
    if *bound != order || *filter != digest {
        return Err(message(
            "the cursor belongs to another order or filter; send it with the order and filter of the answer it came from, or ask without a cursor",
        ));
    }
    if run.parse::<u64>().ok() != Some(crate::operations::health::start()) {
        return Ok(Cursor::Expired);
    }
    Ok(Cursor::At(Place { side, key }))
}

pub fn asked(args: &Args, op: &str) -> Result<Asked, String> {
    let order = args::word(args, op, "order", &ORDERS)?
        .and_then(|given| ORDERS.iter().find(|order| **order == given).copied())
        .unwrap_or("activity");
    let filter = Filter::read(args.get("filter"))?;
    let limit = match args::number(args, op, "limit")? {
        Some(limit) if limit > PAGE => {
            return Err(message(
                "`limit` in the arguments of {op} is above {limit}, the most rows a page holds",
            )
            .replace("{op}", op)
            .replace("{limit}", &PAGE.to_string()));
        }
        Some(limit) => limit,
        None => PAGE_DEFAULT,
    };
    Ok(Asked {
        order,
        filter,
        limit: limit as usize,
    })
}

struct Matched<'a> {
    ordered: Vec<(Key, &'a Row)>,
    matching: Value,
}

fn matched<'a>(asked: &Asked, live: &'a [Row], index: &'a index::Index) -> Matched<'a> {
    let mut counts = [0u64; STATES.len()];
    let mut ordered: Vec<(Key, &Row)> = Vec::new();
    let ended = index
        .ended()
        .map(|row| row.as_ref())
        .filter(|row| live.binary_search_by_key(&row.id, |row| row.id).is_err());
    let mut joined = String::new();
    for row in live.iter().chain(ended) {
        if !asked.filter.matches(row, &mut joined) {
            continue;
        }
        if let Some(at) = STATES.iter().position(|state| *state == row.state) {
            counts[at] += 1;
        }
        ordered.push((key(asked.order, row), row));
    }
    ordered.sort_unstable_by_key(|(key, _)| *key);
    let named: Map<String, Value> = STATES
        .iter()
        .zip(counts)
        .filter(|(_, count)| *count > 0)
        .map(|(state, count)| ((*state).to_owned(), Value::from(count)))
        .collect();
    Matched {
        matching: json!({
            "total": ordered.len(),
            "counts": named,
            "ended_not_searched": index.outside(),
        }),
        ordered,
    }
}

pub struct Gathered<T> {
    pub live: Vec<Row>,
    pub index: index::Guard,
    pub seq: u64,
    pub more: T,
}

pub fn gathered<T>(
    shared: &Shared,
    more: impl FnOnce(&client_service::Seen<'_>, &index::Index) -> T,
) -> Gathered<T> {
    client_service::seen(shared, |seen| {
        let mut index = index::locked();
        let live = seen
            .jobs
            .values()
            .map(|job| {
                let confirmed = index.confirmed(seen.store, job);
                let (started, revision) = index.started(seen.store, job);
                let mut row = Row::of(
                    job,
                    revision,
                    seen.predicted.get(&job.id).copied().flatten(),
                    confirmed,
                );
                row.started_ms = started;
                row
            })
            .collect();
        let more = more(seen, &index);
        Gathered {
            live,
            seq: super::changes::last(),
            index,
            more,
        }
    })
}

pub fn first(asked: &Asked, live: &[Row], index: &index::Index) -> String {
    let found = matched(asked, live, index);
    let page = paged(asked, &found, 0, asked.limit.min(found.ordered.len()));
    written(asked, &found, &page, None)
}

struct Page {
    start: usize,
    end: usize,
    before: Option<String>,
    after: Option<String>,
    found: Option<bool>,
    expired: bool,
}

fn paged(asked: &Asked, found: &Matched<'_>, start: usize, end: usize) -> Page {
    let digest = asked.filter.digest();
    let rows = &found.ordered[start..end];
    let place = |side: Side, key: Key| sealed(asked.order, &digest, &Place { side, key });
    Page {
        start,
        end,
        before: rows
            .first()
            .filter(|_| start > 0)
            .map(|(key, _)| place(Side::Before, *key)),
        after: rows
            .last()
            .filter(|_| end < found.ordered.len())
            .map(|(key, _)| place(Side::After, *key)),
        found: None,
        expired: false,
    }
}

fn head(asked: &Asked, found: &Matched<'_>, seq: Option<u64>) -> String {
    let stamp = seq.map_or_else(String::new, |seq| {
        format!("\"seq\":{seq},\"now_ms\":{},", crate::shim::now_ms())
    });
    format!(
        "{{{stamp}\"order\":\"{}\",\"matching\":{}",
        asked.order, found.matching
    )
}

fn written(asked: &Asked, found: &Matched<'_>, page: &Page, seq: Option<u64>) -> String {
    let rows = &found.ordered[page.start..page.end];
    let mut line = head(asked, found, seq);
    line.push_str(&format!(
        ",\"offset\":{},\"rows\":[",
        json!((!rows.is_empty()).then_some(page.start))
    ));
    for (at, (_, row)) in rows.iter().enumerate() {
        if at > 0 {
            line.push(',');
        }
        row.write(&mut line);
    }
    line.push_str(&format!(
        "],\"before\":{},\"after\":{}",
        json!(page.before),
        json!(page.after)
    ));
    if let Some(found) = page.found {
        line.push_str(&format!(",\"found\":{found}"));
    }
    if page.expired {
        line.push_str(",\"cursor_expired\":true");
    }
    line.push('}');
    line
}

pub fn answer(shared: &Shared, args: &Args) -> Result<String, String> {
    let ids_only = args::flag(args, OP, "ids_only")?;
    let cursor = args::text(args, OP, "cursor")?;
    let at = args::number(args, OP, "at")?;
    let direction = args::word(args, OP, "direction", &["forward", "backward"])?;
    if ids_only {
        for member in ["cursor", "at", "limit"] {
            if args.contains_key(member) {
                return Err(args::together(OP, "ids_only", member));
            }
        }
    }
    if cursor.is_some() && at.is_some() {
        return Err(args::together(OP, "cursor", "at"));
    }
    if direction.is_some() && cursor.is_none() {
        return Err(message(
            "`direction` in the arguments of jobs goes with a cursor only; leave it out, or send the cursor of an earlier answer",
        ));
    }
    let asked = asked(args, OP)?;
    let cursor = cursor
        .map(|text| opened(text, asked.order, &asked.filter.digest()))
        .transpose()?;
    let Gathered {
        live, index, seq, ..
    } = gathered(shared, |_, _| ());
    let found = matched(&asked, &live, &index);
    let total = found.ordered.len();
    if ids_only {
        let ids: Vec<u64> = found
            .ordered
            .iter()
            .take(IDS)
            .map(|(_, row)| row.id)
            .collect();
        return Ok(format!(
            "{},\"ids\":{},\"ids_more\":{}}}",
            head(&asked, &found, Some(seq)),
            json!(ids),
            total - ids.len()
        ));
    }
    let page = match (cursor, at) {
        (Some(Cursor::Expired), _) => Page {
            expired: true,
            ..paged(&asked, &found, 0, 0)
        },
        (Some(Cursor::At(place)), _) => {
            let edge = found.ordered.partition_point(|(key, _)| match place.side {
                Side::Before => *key < place.key,
                Side::After => *key <= place.key,
            });
            let (start, end) = if direction == Some("backward") {
                (edge.saturating_sub(asked.limit), edge)
            } else {
                (edge, (edge + asked.limit).min(total))
            };
            let mut page = paged(&asked, &found, start, end);
            if start == end {
                let same = sealed(asked.order, &asked.filter.digest(), &place);
                page.before = (edge > 0).then(|| same.clone());
                page.after = (edge < total).then_some(same);
            }
            page
        }
        (None, Some(id)) => {
            let position = found.ordered.iter().position(|(_, row)| row.id == id);
            let page = match position {
                Some(position) => {
                    let start = position - position.min(asked.limit / 2);
                    paged(&asked, &found, start, (start + asked.limit).min(total))
                }
                None => paged(&asked, &found, 0, 0),
            };
            Page {
                found: Some(position.is_some()),
                ..page
            }
        }
        (None, None) => paged(&asked, &found, 0, asked.limit.min(total)),
    };
    Ok(written(&asked, &found, &page, Some(seq)))
}
