use super::*;
use crate::admission::tests::{job, ledger};
use serde_json::Value;

fn graph() -> (Graph, u64, u64) {
    let mut graph = Graph::fresh();
    graph
        .nodes
        .get_mut(&ROOT)
        .unwrap()
        .config
        .insert("fair_share".into(), Value::from("cpu-request-time"));
    let a = graph.create("a", Kind::Queue, BTreeMap::new()).unwrap();
    let b = graph.create("b", Kind::Queue, BTreeMap::new()).unwrap();
    (graph, a, b)
}

fn work(id: u64, queue: u64, rate: u64, active: bool) -> Job {
    let mut job = job(id, None, rate);
    job.queue_id = Some(queue);
    if active {
        job.state = State::Running;
    }
    job
}

#[test]
fn exact_accounting_is_independent_of_checkpoint_frequency() {
    let mut once = Branch::new(3);
    let mut often = once.clone();
    once.charge(10000).unwrap();
    for _ in 0..10000 {
        often.charge(1).unwrap();
    }
    assert_eq!(once, often);
    assert_eq!(once.service.0, 3333);
    assert_eq!(once.remainder, 1);
}

#[test]
fn unequal_cpu_requests_receive_equal_normalized_service_not_equal_start_counts() {
    let (graph, a, b) = graph();
    let mut accounting = Accounting::default();
    let mut jobs = BTreeMap::from([(1, work(1, a, 4000, true)), (2, work(2, b, 1000, true))]);
    accounting.advance(&graph, &jobs, 0).unwrap();
    accounting.advance(&graph, &jobs, 100).unwrap();
    jobs.get_mut(&1).unwrap().state = State::Queued;
    jobs.get_mut(&2).unwrap().state = State::Queued;
    accounting.advance(&graph, &jobs, 0).unwrap();
    let ordered = accounting
        .order(&graph, &ledger(), jobs.values().collect())
        .unwrap();
    assert_eq!(ordered[0].id, 2);
    let scope = &accounting.scopes[&scope_key(ROOT, Resource::CpuRequestTime)];
    assert_eq!(scope.branches[&a].service.0, 400000);
    assert_eq!(scope.branches[&b].service.0, 100000);
}

#[test]
fn weights_divide_service_and_memory_accounting_uses_bytes() {
    let (mut graph, a, b) = graph();
    graph
        .nodes
        .get_mut(&ROOT)
        .unwrap()
        .config
        .insert("fair_share".into(), Value::from("memory-request-time"));
    graph
        .nodes
        .get_mut(&a)
        .unwrap()
        .config
        .insert("share_weight".into(), Value::from(2));
    let mut jobs = BTreeMap::from([(1, work(1, a, 0, true)), (2, work(2, b, 0, true))]);
    jobs.get_mut(&1).unwrap().reservation.vector.memory = 2000;
    jobs.get_mut(&2).unwrap().reservation.vector.memory = 1000;
    let mut accounting = Accounting::default();
    accounting.advance(&graph, &jobs, 0).unwrap();
    accounting.advance(&graph, &jobs, 100).unwrap();
    let scope = &accounting.scopes[&scope_key(ROOT, Resource::MemoryRequestTime)];
    assert_eq!(scope.branches[&a].service.0, scope.branches[&b].service.0);
    assert!(validate_job(&graph, &jobs[&1]).is_ok());
    jobs.get_mut(&1).unwrap().reservation.vector.memory = 0;
    assert!(validate_job(&graph, &jobs[&1]).is_err());
}

#[test]
fn a_batch_of_simultaneous_starts_does_not_all_go_to_one_queue() {
    let (graph, a, b) = graph();
    let jobs: Vec<_> = (1..=6)
        .map(|id| work(id, if id <= 3 { a } else { b }, 1000, false))
        .collect();
    let ordered = Accounting::default()
        .order(&graph, &ledger(), jobs.iter().collect())
        .unwrap();
    assert_eq!(
        ordered
            .iter()
            .map(|j| j.queue_id.unwrap())
            .collect::<Vec<_>>(),
        [a, b, a, b, a, b]
    );
}

#[test]
fn active_reservations_influence_new_starts_before_a_tick_elapses() {
    let (graph, a, b) = graph();
    let jobs = BTreeMap::from([
        (1, work(1, a, 1000, true)),
        (2, work(2, a, 1000, false)),
        (3, work(3, b, 1000, false)),
    ]);
    let mut accounting = Accounting::default();
    accounting.advance(&graph, &jobs, 0).unwrap();
    let ordered = accounting
        .order(&graph, &ledger(), vec![&jobs[&2], &jobs[&3]])
        .unwrap();
    assert_eq!(ordered[0].id, 3);
}

#[test]
fn new_and_rejoining_queues_join_at_the_watermark_without_erasing_debt() {
    let (mut graph, a, b) = graph();
    let jobs = BTreeMap::from([(1, work(1, a, 1000, true))]);
    let mut accounting = Accounting::default();
    accounting.advance(&graph, &jobs, 0).unwrap();
    accounting.advance(&graph, &jobs, 100).unwrap();
    let fresh = graph.create("fresh", Kind::Queue, BTreeMap::new()).unwrap();
    let mut queued = jobs;
    queued.insert(2, work(2, b, 1000, false));
    queued.insert(3, work(3, fresh, 1000, false));
    accounting.advance(&graph, &queued, 0).unwrap();
    let scope = &accounting.scopes[&scope_key(ROOT, Resource::CpuRequestTime)];
    assert_eq!(scope.branches[&b].service.0, 100000);
    assert_eq!(scope.branches[&fresh].service.0, 100000);
    queued.remove(&2);
    accounting.advance(&graph, &queued, 0).unwrap();
    accounting.advance(&graph, &queued, 100).unwrap();
    queued.insert(2, work(2, b, 1000, false));
    accounting.advance(&graph, &queued, 0).unwrap();
    assert!(
        accounting.scopes[&scope_key(ROOT, Resource::CpuRequestTime)].branches[&b]
            .service
            .0
            >= 100000
    );
}

#[test]
fn child_queue_proliferation_does_not_multiply_the_groups_outer_share() {
    let mut graph = Graph::fresh();
    graph
        .nodes
        .get_mut(&ROOT)
        .unwrap()
        .config
        .insert("fair_share".into(), Value::from("cpu-request-time"));
    let group = graph
        .create(
            "team",
            Kind::Group,
            BTreeMap::from([("fair_share".into(), Value::from("cpu-request-time"))]),
        )
        .unwrap();
    let peer = graph.create("peer", Kind::Queue, BTreeMap::new()).unwrap();
    let mut jobs = BTreeMap::new();
    for id in 1..=10 {
        let queue = graph
            .create(&format!("team/q{id}"), Kind::Queue, BTreeMap::new())
            .unwrap();
        jobs.insert(id, work(id, queue, 1000, true));
    }
    jobs.insert(11, work(11, peer, 10000, true));
    let mut accounting = Accounting::default();
    accounting.advance(&graph, &jobs, 0).unwrap();
    accounting.advance(&graph, &jobs, 100).unwrap();
    let outer = &accounting.scopes[&scope_key(ROOT, Resource::CpuRequestTime)];
    assert_eq!(
        outer.branches[&group].service.0,
        outer.branches[&peer].service.0
    );
    assert_eq!(outer.branches[&group].service.0, 1000000);
}

#[test]
fn reweighting_retains_prior_normalized_service_and_changes_only_future_rates() {
    let (mut graph, a, _) = graph();
    let jobs = BTreeMap::from([(1, work(1, a, 1000, true))]);
    let mut accounting = Accounting::default();
    accounting.advance(&graph, &jobs, 0).unwrap();
    accounting.advance(&graph, &jobs, 100).unwrap();
    graph
        .nodes
        .get_mut(&a)
        .unwrap()
        .config
        .insert("share_weight".into(), Value::from(2));
    accounting.advance(&graph, &jobs, 0).unwrap();
    accounting.advance(&graph, &jobs, 100).unwrap();
    assert_eq!(
        accounting.scopes[&scope_key(ROOT, Resource::CpuRequestTime)].branches[&a]
            .service
            .0,
        150000
    );
}

#[test]
fn retained_reservations_are_charged_while_suspended_but_paused_waiters_are_not() {
    let (mut graph, a, b) = graph();
    let mut jobs = BTreeMap::from([(1, work(1, a, 1000, true)), (2, work(2, b, 1000, false))]);
    jobs.get_mut(&1).unwrap().state = State::Suspended;
    graph.nodes.get_mut(&b).unwrap().paused = true;
    let mut accounting = Accounting::default();
    accounting.advance(&graph, &jobs, 0).unwrap();
    accounting.advance(&graph, &jobs, 100).unwrap();
    let scope = &accounting.scopes[&scope_key(ROOT, Resource::CpuRequestTime)];
    assert_eq!(scope.branches[&a].service.0, 100000);
    assert_eq!(scope.branches[&b].service.0, 0);
    assert!(!scope.branches[&b].competing);
}

#[test]
fn accounting_rejects_overflow_instead_of_wrapping_service() {
    let mut branch = Branch::new(1);
    branch.service = Counter(u128::MAX);
    assert!(branch.charge(1).is_err());
    assert_eq!(
        serde_json::from_str::<Counter>(&serde_json::to_string(&Counter(u128::MAX)).unwrap())
            .unwrap(),
        Counter(u128::MAX)
    );
}
