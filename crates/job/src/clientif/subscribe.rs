use std::collections::{BTreeSet, HashMap};
use std::io;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::daemon::Shared;
use crate::daemon::client_service;

use super::args::Args;
use super::changes::{self, Since};
use super::sample::{self, Watch};
use super::{jobs, message, send, totals, tree};

pub const INTEREST: usize = 500;
pub const HEARTBEAT: Duration = Duration::from_millis(5000);
const BATCH: usize = 256;

pub struct Subscription {
    watch: Watch,
    written: u64,
    tick: u64,
    totals: String,
    uses: HashMap<u64, String>,
    live: HashMap<u64, (u64, String)>,
    quiet: Instant,
}

fn ids(value: Option<&Value>, member: &str) -> Result<BTreeSet<u64>, String> {
    let wrong = || {
        message("`{member}` must be an array of at most {limit} Job IDs")
            .replace("{member}", member)
            .replace("{limit}", &INTEREST.to_string())
    };
    let listed = match value {
        None => return Ok(BTreeSet::new()),
        Some(Value::Array(listed)) if listed.len() <= INTEREST => listed,
        Some(_) => return Err(wrong()),
    };
    listed
        .iter()
        .map(|id| id.as_u64().ok_or_else(wrong))
        .collect()
}

fn watched(shared: &Arc<Shared>, uid: u32, interest: BTreeSet<u64>) -> Result<Watch, String> {
    sample::watch(shared, uid, interest).ok_or_else(|| {
        message("this user already has {limit} subscriptions; end one, or use its connection")
            .replace("{limit}", &sample::SUBSCRIPTIONS.to_string())
    })
}

fn resync(reason: &str) -> String {
    format!("{{\"resync\":{{\"reason\":\"{reason}\"}}}}")
}

impl Subscription {
    fn pushed(&mut self, out: &mut UnixStream, line: &str) -> io::Result<()> {
        self.quiet = Instant::now();
        send(out, line)
    }

    fn sampled(&mut self, out: &mut UnixStream) -> io::Result<()> {
        let Some(board) = sample::board() else {
            return Ok(());
        };
        self.tick = board.tick;
        if board.totals != self.totals {
            self.totals.clone_from(&board.totals);
            let line = format!(
                "{{\"push\":\"totals\",\"sampled\":true,\"now_ms\":{},\"seq\":{},{}}}",
                crate::shim::now_ms(),
                changes::last(),
                board.totals
            );
            self.pushed(out, &line)?;
        }
        let changed: Vec<&str> = board
            .uses
            .iter()
            .filter(|(id, mark, _)| self.uses.get(id) != Some(mark))
            .map(|(_, _, body)| body.as_str())
            .collect();
        if !changed.is_empty() {
            let line = format!(
                "{{\"push\":\"use\",\"sampled\":true,\"at_ms\":{},\"objects\":[{}]}}",
                board.at_ms,
                changed.join(",")
            );
            self.uses = board
                .uses
                .iter()
                .map(|(id, mark, _)| (*id, mark.clone()))
                .collect();
            self.pushed(out, &line)?;
        }
        let wanted = self
            .watch
            .interest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        self.live
            .retain(|id, _| wanted.contains(id) && board.live.contains_key(id));
        let mut entries = Vec::new();
        for id in &wanted {
            let Some(entry) = board.live.get(id) else {
                continue;
            };
            let known = self.live.get(id);
            if known.is_none_or(|(attempt, mark)| *attempt != entry.attempt || *mark != entry.mark)
            {
                self.live.insert(*id, (entry.attempt, entry.mark.clone()));
                entries.push(entry.body.as_str());
            }
        }
        if !entries.is_empty() {
            let line = format!(
                "{{\"push\":\"live\",\"sampled\":true,\"at_ms\":{},\"jobs\":[{}]}}",
                board.at_ms,
                entries.join(",")
            );
            self.pushed(out, &line)?;
        }
        Ok(())
    }

    pub fn pump(&mut self, out: &mut UnixStream) -> io::Result<bool> {
        loop {
            match changes::since(self.written, BATCH) {
                Since::Gone => {
                    send(out, "{\"push\":\"resync\",\"reason\":\"slow\"}")?;
                    return Ok(false);
                }
                Since::Lines(lines) if lines.is_empty() => break,
                Since::Lines(lines) => {
                    for (seq, line) in lines {
                        self.pushed(out, &line)?;
                        self.written = seq;
                    }
                }
            }
        }
        if sample::tick() != self.tick {
            self.sampled(out)?;
        }
        if self.quiet.elapsed() >= HEARTBEAT {
            let line = format!(
                "{{\"push\":\"heartbeat\",\"sampled\":true,\"now_ms\":{},\"seq\":{}}}",
                crate::shim::now_ms(),
                changes::last()
            );
            self.pushed(out, &line)?;
        }
        Ok(true)
    }

    pub fn interest(&mut self, args: &Args) -> Result<String, String> {
        let wanted = match args.get("jobs") {
            None => {
                return Err(message(
                    "interest needs `jobs`: an array of Job IDs, which replaces the set",
                ));
            }
            listed => ids(listed, "jobs")?,
        };
        let count = wanted.len();
        *self
            .watch
            .interest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = wanted;
        Ok(format!("{{\"jobs\":{count}}}"))
    }
}

fn resumed(
    shared: &Arc<Shared>,
    uid: u32,
    after: &Value,
    interest: BTreeSet<u64>,
) -> Result<(String, Option<Subscription>), String> {
    let (Some(started), Some(seq)) = (after["started_ms"].as_u64(), after["seq"].as_u64()) else {
        return Err(message(
            "`after` in the arguments of subscribe must be {\"started_ms\":S,\"seq\":N} with whole numbers",
        ));
    };
    let run = crate::operations::health::start();
    if started != run {
        return Ok((resync("restarted"), None));
    }
    let last = changes::last();
    if !changes::wanted() || seq > last || matches!(changes::since(seq, 1), Since::Gone) {
        return Ok((resync("expired"), None));
    }
    let watch = watched(shared, uid, interest)?;
    Ok((
        format!(
            "{{\"resumed\":{{\"seq\":{last},\"started_ms\":{run},\"now_ms\":{}}}}}",
            crate::shim::now_ms()
        ),
        Some(Subscription {
            watch,
            written: seq,
            tick: sample::tick(),
            totals: String::new(),
            uses: HashMap::new(),
            live: HashMap::new(),
            quiet: Instant::now(),
        }),
    ))
}

pub fn subscribe(
    shared: &Arc<Shared>,
    uid: u32,
    args: &Args,
) -> Result<(String, Option<Subscription>), String> {
    let empty = Args::new();
    let rows = match args.get("rows") {
        None => &empty,
        Some(Value::Object(rows)) => rows,
        Some(_) => {
            return Err(message(
                "`rows` in the arguments of subscribe must be an object with order, filter and limit",
            ));
        }
    };
    let asked = jobs::asked(rows, "subscribe")?;
    let interest = ids(args.get("interest"), "interest")?;
    if let Some(after) = args.get("after") {
        return resumed(shared, uid, after, interest);
    }
    let count = interest.len();
    let watch = watched(shared, uid, interest)?;
    let now = crate::shim::now_ms();
    let gathered = jobs::gathered(shared, |seen, index| {
        changes::begin(seen.objects);
        (
            tree::drawn(seen, now),
            client_service::held(seen),
            index.counts(),
        )
    });
    let page = jobs::first(&asked, &gathered.live, &gathered.index);
    let seq = gathered.seq;
    let (mut nodes, held, ended) = gathered.more;
    drop(gathered.index);
    tree::filled(&mut nodes);
    let counted = totals::body(&held, ended);
    let health = totals::health(&held);
    changes::health(&health);
    let uses: HashMap<u64, String> = nodes
        .iter()
        .map(|node| {
            let (id, mark, _) = sample::used(node);
            (id, mark)
        })
        .collect();
    let answer = format!(
        "{{\"snapshot\":{{\"seq\":{seq},\"started_ms\":{},\"now_ms\":{now},\"totals\":{{{counted}}},\"health\":{health},\"tree\":{{\"nodes\":{}}},\"page\":{page},\"interest\":{count}}}}}",
        crate::operations::health::start(),
        Value::Array(nodes),
    );
    Ok((
        answer,
        Some(Subscription {
            watch,
            written: seq,
            tick: sample::tick(),
            totals: counted,
            uses,
            live: HashMap::new(),
            quiet: Instant::now(),
        }),
    ))
}
