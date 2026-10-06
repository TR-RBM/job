use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::objects::Graph;

use super::row::Row;
use super::tree;

pub const KEPT: usize = 8192;

static LAST: AtomicU64 = AtomicU64::new(0);
static WANTED: AtomicBool = AtomicBool::new(false);
static STORE: Mutex<Store> = Mutex::new(Store {
    lines: VecDeque::new(),
    nodes: None,
    health: None,
});

struct Store {
    lines: VecDeque<(u64, Arc<str>)>,
    nodes: Option<HashMap<u64, String>>,
    health: Option<String>,
}

pub enum Since {
    Lines(Vec<(u64, Arc<str>)>),
    Gone,
}

fn locked() -> MutexGuard<'static, Store> {
    STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn publish(store: &mut Store, kind: &str, members: &str) {
    let seq = LAST.load(Ordering::SeqCst) + 1;
    let line = format!(
        "{{\"push\":\"{kind}\",\"seq\":{seq},\"at_ms\":{},{members}}}",
        crate::shim::now_ms()
    );
    store.lines.push_back((seq, Arc::from(line)));
    while store.lines.len() > KEPT {
        store.lines.pop_front();
    }
    LAST.store(seq, Ordering::SeqCst);
}

pub fn wanted() -> bool {
    WANTED.load(Ordering::SeqCst)
}

pub fn last() -> u64 {
    LAST.load(Ordering::SeqCst)
}

pub fn job(row: &Row) {
    if !wanted() {
        return;
    }
    let mut members = String::from("\"row\":");
    row.write(&mut members);
    publish(&mut locked(), "job", &members);
}

pub fn job_removed(id: u64) {
    if wanted() {
        publish(&mut locked(), "job_removed", &format!("\"id\":{id}"));
    }
}

fn drawn(graph: &Graph) -> Vec<(u64, String)> {
    tree::ordered(graph)
        .into_iter()
        .map(|id| (id, tree::node(graph, id).to_string()))
        .collect()
}

pub fn begin(graph: &Graph) {
    let mut store = locked();
    if store.nodes.is_none() {
        store.nodes = Some(drawn(graph).into_iter().collect());
    }
    WANTED.store(true, Ordering::SeqCst);
}

pub fn objects(graph: &Graph) {
    if !wanted() {
        return;
    }
    let drawn = drawn(graph);
    let mut store = locked();
    let mut known = store.nodes.take().unwrap_or_default();
    let gone: Vec<u64> = known
        .keys()
        .filter(|id| !graph.nodes.contains_key(id))
        .copied()
        .collect();
    for (id, node) in drawn {
        if known.get(&id) != Some(&node) {
            publish(&mut store, "object", &format!("\"node\":{node}"));
            known.insert(id, node);
        }
    }
    for id in gone {
        known.remove(&id);
        publish(&mut store, "object_removed", &format!("\"id\":{id}"));
    }
    store.nodes = Some(known);
}

pub fn health(health: &str) {
    let mut store = locked();
    if store.health.as_deref() == Some(health) {
        return;
    }
    if store.health.is_some() {
        publish(&mut store, "health", &format!("\"health\":{health}"));
    }
    store.health = Some(health.to_owned());
}

pub fn since(after: u64, most: usize) -> Since {
    if after >= last() {
        return Since::Lines(Vec::new());
    }
    let store = locked();
    let Some((first, _)) = store.lines.front() else {
        return Since::Gone;
    };
    if after + 1 < *first {
        return Since::Gone;
    }
    Since::Lines(
        store
            .lines
            .iter()
            .skip((after + 1 - first) as usize)
            .take(most)
            .cloned()
            .collect(),
    )
}
