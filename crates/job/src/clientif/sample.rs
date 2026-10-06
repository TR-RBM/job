use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::daemon::Shared;
use crate::daemon::client_service;

use super::{changes, record, totals, tree};

pub const SUBSCRIPTIONS: usize = 4;
pub const INTERVAL: Duration = Duration::from_millis(1000);

pub struct Entry {
    pub attempt: u64,
    pub mark: String,
    pub body: String,
}

pub struct Board {
    pub tick: u64,
    pub at_ms: u64,
    pub totals: String,
    pub uses: Vec<(u64, String, String)>,
    pub live: HashMap<u64, Entry>,
}

type Interest = Arc<Mutex<BTreeSet<u64>>>;

struct Watchers {
    next: u64,
    sets: HashMap<u64, (u32, Interest)>,
    running: bool,
}

static TICK: AtomicU64 = AtomicU64::new(0);
static BOARD: Mutex<Option<Arc<Board>>> = Mutex::new(None);
static WATCHERS: Mutex<Option<Watchers>> = Mutex::new(None);

fn watchers() -> MutexGuard<'static, Option<Watchers>> {
    let mut guard = WATCHERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        *guard = Some(Watchers {
            next: 1,
            sets: HashMap::new(),
            running: false,
        });
    }
    guard
}

pub struct Watch {
    id: u64,
    pub interest: Interest,
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(watchers) = watchers().as_mut() {
            watchers.sets.remove(&self.id);
        }
    }
}

pub fn watch(shared: &Arc<Shared>, uid: u32, interest: BTreeSet<u64>) -> Option<Watch> {
    let mut guard = watchers();
    let watchers = guard.as_mut()?;
    if watchers
        .sets
        .values()
        .filter(|(owner, _)| *owner == uid)
        .count()
        >= SUBSCRIPTIONS
    {
        return None;
    }
    let id = watchers.next;
    watchers.next += 1;
    let interest = Arc::new(Mutex::new(interest));
    watchers.sets.insert(id, (uid, Arc::clone(&interest)));
    if !watchers.running {
        watchers.running = true;
        let shared = Arc::clone(shared);
        std::thread::spawn(move || run(&shared));
    }
    Some(Watch { id, interest })
}

pub fn tick() -> u64 {
    TICK.load(Ordering::SeqCst)
}

pub fn board() -> Option<Arc<Board>> {
    BOARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

pub fn used(node: &Value) -> (u64, String, String) {
    let id = node["object"]["id"].as_u64().unwrap_or(0);
    (
        id,
        tree::mark(node),
        format!(
            "{{\"id\":{id},\"depth\":{},\"use\":{}}}",
            node["depth"], node["use"]
        ),
    )
}

fn pass(shared: &Shared, wanted: &BTreeSet<u64>) -> Board {
    let now = crate::shim::now_ms();
    let (held, ended, _) = totals::held(shared);
    let (nodes, _, _) = tree::nodes(shared);
    let subjects = client_service::subjects(shared, wanted);
    changes::health(&totals::health(&held));
    let mut table = None;
    let live = subjects
        .iter()
        .map(|subject| {
            let mut figures = record::figures(subject, now, &mut table);
            let at = figures.remove("cpu_at_ms").unwrap_or(Value::Null);
            let mark = Value::Object(figures.clone()).to_string();
            let mut body = format!(
                "{{\"id\":{},\"attempt\":{},\"cpu_at_ms\":{at}",
                subject.id, subject.attempt
            );
            for (name, value) in &figures {
                body.push_str(&format!(",\"{name}\":{value}"));
            }
            body.push('}');
            (
                subject.id,
                Entry {
                    attempt: subject.attempt,
                    mark,
                    body,
                },
            )
        })
        .collect();
    Board {
        tick: TICK.load(Ordering::SeqCst) + 1,
        at_ms: now,
        totals: totals::body(&held, ended),
        uses: nodes.iter().map(used).collect(),
        live,
    }
}

fn run(shared: &Shared) {
    loop {
        let started = Instant::now();
        let wanted: BTreeSet<u64> = {
            let mut guard = watchers();
            let Some(watchers) = guard.as_mut() else {
                return;
            };
            if watchers.sets.is_empty() {
                watchers.running = false;
                *BOARD
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
                return;
            }
            watchers
                .sets
                .values()
                .flat_map(|(_, interest)| {
                    interest
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone()
                })
                .collect()
        };
        let board = pass(shared, &wanted);
        let tick = board.tick;
        *BOARD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(board));
        TICK.store(tick, Ordering::SeqCst);
        std::thread::sleep(INTERVAL.saturating_sub(started.elapsed()));
    }
}
