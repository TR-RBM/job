use super::*;
use crate::objects::{DEFAULT_QUEUE, Kind};
use crate::resources::Vector;

pub(super) fn job(id: u64, priority: Option<i32>, cores: u64) -> Job {
    serde_json::from_value(serde_json::json!({
        "id": id, "attempt": 1, "queue_id": DEFAULT_QUEUE, "policy": "ordinary",
        "state": "Queued", "submitted_ms": id, "released_ms": id,
        "spec": {"argv": ["true"], "cwd": "/", "session": "test", "declared": {"priority": priority, "devices": []}},
        "key": "test", "backend": "Watch", "log": "/unused", "usage": crate::model::Usage::default(),
        "reservation": {"vector": {"cores_milli": cores, "memory": 0, "pids": 0}, "disk": 0, "devices": [],
            "cores_source": "Declared", "memory_source": "Unset", "disk_source": "Unset"}
    })).unwrap()
}

pub(super) fn ledger() -> Ledger {
    Ledger {
        schema_version: 1,
        boot_id: "boot".into(),
        checkpoint_ms: 0,
        credits: BTreeMap::new(),
        fair: fair::Accounting::default(),
    }
}

#[test]
fn aging_duration_preserves_exact_milliseconds_beyond_float_precision() {
    assert_eq!(
        aging_duration("9007199254740993ms").unwrap(),
        9_007_199_254_740_993
    );
    assert_eq!(aging_duration("0.001s").unwrap(), 1);
    assert_eq!(aging_duration("1.5min").unwrap(), 90_000);
    assert_eq!(aging_duration("18446744073709551615ms").unwrap(), u64::MAX);
}

#[test]
fn aging_duration_rejects_rounding_overflow_and_malformed_values() {
    for text in [
        "0",
        "0.5ms",
        "18446744073709551616ms",
        "18446744073709551615s",
        "-1s",
        "1e3s",
        "NaN",
        "1.s",
        ".5s",
        "1..2s",
    ] {
        assert!(aging_duration(text).is_err(), "{text}");
    }
}

#[test]
fn wall_time_planning_includes_termination_grace_and_dispatch_tick() {
    let graph = aging_graph();
    let mut running = job(1, None, 1000);
    running.state = State::Running;
    running.started_ms = Some(0);
    running.reservation.wall_limit_ms = Some(100);
    let queued = job(2, Some(1), 1000);
    let jobs = BTreeMap::from([(1, running), (2, queued)]);
    let plan = planner::plan(
        &graph,
        &ledger(),
        &jobs,
        Vector {
            cores_milli: 1000,
            memory: 0,
            pids: 0,
        },
        10,
        &BTreeMap::new(),
    );
    assert_eq!(
        plan[0].start,
        Some(100 + crate::daemon::GRACE_MS + crate::daemon::TICK.as_millis() as u64)
    );
}

fn aging_graph() -> Graph {
    let mut graph = Graph::fresh();
    graph
        .nodes
        .get_mut(&DEFAULT_QUEUE)
        .unwrap()
        .config
        .insert("aging_ms".into(), Value::from(10));
    graph
}

#[test]
fn low_priority_eventually_outranks_continuous_maximum_priority_arrivals() {
    let graph = aging_graph();
    let mut ledger = ledger();
    let low = job(1, Some(MIN_PRIORITY), 1000);
    let mut jobs = BTreeMap::from([(1, low.clone())]);
    ledger.advance(&graph, &jobs, "boot", 0).unwrap();
    for arrival in 1..=2001 {
        let high = job(arrival + 1, Some(MAX_PRIORITY), 1000);
        jobs.retain(|&id, _| id == 1);
        jobs.insert(high.id, high.clone());
        ledger.advance(&graph, &jobs, "boot", arrival * 10).unwrap();
        if arrival == 2001 {
            assert!(ledger.score(&graph, &low) > ledger.score(&graph, &high));
        }
    }
}

#[test]
fn paused_time_is_excluded_and_released_resource_wait_keeps_credit() {
    let mut graph = aging_graph();
    let mut ledger = ledger();
    let jobs = BTreeMap::from([(1, job(1, None, 1000))]);
    ledger.advance(&graph, &jobs, "boot", 0).unwrap();
    ledger.advance(&graph, &jobs, "boot", 100).unwrap();
    graph.nodes.get_mut(&DEFAULT_QUEUE).unwrap().paused = true;
    ledger.advance(&graph, &jobs, "boot", 100).unwrap();
    ledger.advance(&graph, &jobs, "boot", 1000).unwrap();
    graph.nodes.get_mut(&DEFAULT_QUEUE).unwrap().paused = false;
    ledger.advance(&graph, &jobs, "boot", 1000).unwrap();
    ledger.advance(&graph, &jobs, "boot", 1100).unwrap();
    assert_eq!(ledger.credit(&jobs[&1]), 200);
}

#[test]
fn a_retry_does_not_inherit_the_previous_attempts_credit() {
    let graph = aging_graph();
    let mut ledger = ledger();
    let mut jobs = BTreeMap::from([(1, job(1, None, 1000))]);
    ledger.advance(&graph, &jobs, "boot", 0).unwrap();
    ledger.advance(&graph, &jobs, "boot", 100).unwrap();
    jobs.get_mut(&1).unwrap().attempt = 2;
    ledger.advance(&graph, &jobs, "boot", 200).unwrap();
    assert_eq!(ledger.credit(&jobs[&1]), 0);
}

#[test]
fn changing_an_aging_interval_reinterprets_credit_and_disabled_time_is_excluded() {
    let mut graph = aging_graph();
    let mut ledger = ledger();
    let jobs = BTreeMap::from([(1, job(1, None, 0))]);
    ledger.advance(&graph, &jobs, "boot", 0).unwrap();
    ledger.advance(&graph, &jobs, "boot", 100).unwrap();
    graph
        .nodes
        .get_mut(&DEFAULT_QUEUE)
        .unwrap()
        .config
        .insert("aging_ms".into(), Value::from(20));
    assert_eq!(ledger.score(&graph, &jobs[&1]), 5);
    graph
        .nodes
        .get_mut(&DEFAULT_QUEUE)
        .unwrap()
        .config
        .remove("aging_ms");
    ledger.advance(&graph, &jobs, "boot", 100).unwrap();
    ledger.advance(&graph, &jobs, "boot", 1000).unwrap();
    assert_eq!(ledger.credit(&jobs[&1]), 100);
    assert_eq!(ledger.score(&graph, &jobs[&1]), 0);
    graph
        .nodes
        .get_mut(&DEFAULT_QUEUE)
        .unwrap()
        .config
        .insert("aging_ms".into(), Value::from(10));
    ledger.advance(&graph, &jobs, "boot", 1000).unwrap();
    ledger.advance(&graph, &jobs, "boot", 1100).unwrap();
    assert_eq!(ledger.credit(&jobs[&1]), 200);
}

#[test]
fn held_time_and_queue_changes_do_not_mint_waiting_credit() {
    let mut graph = aging_graph();
    let target = graph
        .create(
            "target",
            Kind::Queue,
            BTreeMap::from([("aging_ms".into(), Value::from(10))]),
        )
        .unwrap();
    let mut ledger = ledger();
    let mut jobs = BTreeMap::from([(1, job(1, None, 0))]);
    jobs.get_mut(&1).unwrap().state = State::Held;
    ledger.advance(&graph, &jobs, "boot", 0).unwrap();
    ledger.advance(&graph, &jobs, "boot", 1000).unwrap();
    assert_eq!(ledger.credit(&jobs[&1]), 0);
    jobs.get_mut(&1).unwrap().state = State::Queued;
    ledger.advance(&graph, &jobs, "boot", 1000).unwrap();
    ledger.advance(&graph, &jobs, "boot", 1100).unwrap();
    jobs.get_mut(&1).unwrap().queue_id = Some(target);
    ledger.advance(&graph, &jobs, "boot", 1100).unwrap();
    assert_eq!(ledger.credit(&jobs[&1]), 100);
    ledger.advance(&graph, &jobs, "boot", 1200).unwrap();
    assert_eq!(ledger.credit(&jobs[&1]), 200);
}

#[test]
fn restart_preserves_same_boot_credit_and_reboot_preserves_only_confirmed_credit() {
    let graph = aging_graph();
    let mut ledger = ledger();
    let jobs = BTreeMap::from([(1, job(1, None, 1000))]);
    ledger.advance(&graph, &jobs, "boot", 0).unwrap();
    ledger.advance(&graph, &jobs, "boot", 100).unwrap();
    let mut restored: Ledger =
        serde_json::from_slice(&serde_json::to_vec(&ledger).unwrap()).unwrap();
    restored.advance(&graph, &jobs, "boot", 200).unwrap();
    assert_eq!(restored.credit(&jobs[&1]), 200);
    restored.advance(&graph, &jobs, "next-boot", 1000).unwrap();
    assert_eq!(restored.credit(&jobs[&1]), 200);
}

#[test]
fn overflow_is_an_error_and_never_wraps_into_a_low_score() {
    let graph = aging_graph();
    let mut ledger = ledger();
    let jobs = BTreeMap::from([(1, job(1, Some(MAX_PRIORITY), 0))]);
    ledger.credits.insert(
        "1:1".into(),
        Credit {
            milliseconds: u64::MAX,
            eligible: true,
        },
    );
    assert!(ledger.advance(&graph, &jobs, "boot", 1).is_err());
    assert!(ledger.score(&graph, &jobs[&1]) > MAX_PRIORITY as i128);
}

#[test]
fn descendant_bounds_cannot_widen_an_ancestor_limit() {
    let mut graph = Graph::fresh();
    graph
        .nodes
        .get_mut(&crate::objects::ROOT)
        .unwrap()
        .config
        .insert("priority_max".into(), Value::from(100));
    graph
        .nodes
        .get_mut(&DEFAULT_QUEUE)
        .unwrap()
        .config
        .insert("priority_max".into(), Value::from(500));
    assert!(bounds(&graph, DEFAULT_QUEUE, 101).is_err());
    assert!(bounds(&graph, DEFAULT_QUEUE, 100).is_ok());
}

#[test]
fn an_unrelated_empty_policy_does_not_enable_ordering() {
    let mut graph = Graph::fresh();
    graph
        .create(
            "unused",
            Kind::Queue,
            BTreeMap::from([("aging_ms".into(), Value::from(1))]),
        )
        .unwrap();
    assert!(!enabled(&graph, &BTreeMap::from([(1, job(1, None, 0))])));
}

#[test]
fn strict_fifo_precedes_a_younger_jobs_higher_priority() {
    let mut graph = Graph::fresh();
    graph
        .nodes
        .get_mut(&DEFAULT_QUEUE)
        .unwrap()
        .config
        .insert("strict_fifo".into(), Value::Bool(true));
    let jobs = BTreeMap::from([(1, job(1, Some(-100), 1000)), (2, job(2, Some(100), 1000))]);
    let plan = planner::plan(
        &graph,
        &ledger(),
        &jobs,
        Vector {
            cores_milli: 2000,
            memory: 0,
            pids: 0,
        },
        100,
        &BTreeMap::new(),
    );
    assert!(
        plan.iter()
            .find(|d| d.id == 2)
            .unwrap()
            .reason
            .as_ref()
            .unwrap()
            .contains("FIFO")
    );
    assert_eq!(plan.iter().find(|d| d.id == 1).unwrap().start, Some(100));
}

#[test]
fn a_short_backfill_preserves_the_older_large_jobs_start() {
    let mut graph = Graph::fresh();
    graph
        .nodes
        .get_mut(&DEFAULT_QUEUE)
        .unwrap()
        .config
        .insert("backfill".into(), Value::from("conservative"));
    let mut running = job(1, None, 1000);
    running.state = State::Running;
    running.started_ms = Some(0);
    running.reservation.predicted_ms = Some(100);
    let large = job(2, Some(100), 2000);
    let mut short = job(3, Some(0), 1000);
    short.reservation.predicted_ms = Some(20);
    let mut long = job(4, Some(0), 1000);
    long.reservation.predicted_ms = Some(200);
    let jobs = [running, large, short, long]
        .into_iter()
        .map(|j| (j.id, j))
        .collect();
    let plan = planner::plan(
        &graph,
        &ledger(),
        &jobs,
        Vector {
            cores_milli: 2000,
            memory: 0,
            pids: 0,
        },
        10,
        &BTreeMap::new(),
    );
    assert_eq!(plan.iter().find(|d| d.id == 2).unwrap().start, Some(100));
    assert_eq!(plan.iter().find(|d| d.id == 3).unwrap().start, Some(10));
    assert_ne!(plan.iter().find(|d| d.id == 4).unwrap().start, Some(10));
}

#[test]
fn backfill_respects_a_shared_ancestor_budget() {
    let mut graph = Graph::fresh();
    let group = graph
        .create(
            "work",
            Kind::Group,
            BTreeMap::from([
                ("cores_milli".into(), Value::from(2000)),
                ("backfill".into(), Value::from("conservative")),
            ]),
        )
        .unwrap();
    let one = graph
        .create("work/one", Kind::Queue, BTreeMap::new())
        .unwrap();
    let two = graph
        .create("work/two", Kind::Queue, BTreeMap::new())
        .unwrap();
    let mut running = job(1, None, 1000);
    running.queue_id = Some(one);
    running.state = State::Running;
    running.started_ms = Some(0);
    running.reservation.predicted_ms = Some(100);
    let mut large = job(2, Some(100), 2000);
    large.queue_id = Some(one);
    let mut long = job(3, Some(0), 1000);
    long.queue_id = Some(two);
    long.reservation.predicted_ms = Some(200);
    let jobs = [running, large, long]
        .into_iter()
        .map(|j| (j.id, j))
        .collect();
    let plan = planner::plan(
        &graph,
        &ledger(),
        &jobs,
        Vector {
            cores_milli: 4000,
            memory: 0,
            pids: 0,
        },
        10,
        &BTreeMap::new(),
    );
    assert_eq!(plan.iter().find(|d| d.id == 2).unwrap().start, Some(100));
    assert_ne!(
        plan.iter().find(|d| d.id == 3).unwrap().start,
        Some(10),
        "ancestor {group} cannot be oversubscribed at the protected start"
    );
}
