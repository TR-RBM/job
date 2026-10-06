use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

mod support;
use support::Service;

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn local_admission_barriers_preserve_competing_order_without_holding_independent_queues() {
    let service = Service::start("independent-admission");
    service.ok(&["group", "create", "limited", "--max-running", "1"]);
    service.ok(&["queue", "create", "limited/work"]);
    service.ok(&["queue", "create", "limited/sibling"]);
    service.ok(&[
        "queue",
        "create",
        "independent",
        "--backfill",
        "conservative",
    ]);
    let busy = service.ok(&[
        "submit",
        "-q",
        "limited/work",
        "--time",
        "30s",
        "--",
        "touch busy; sleep 25",
    ]);
    service.wait_file("busy");
    let protected = service.ok(&[
        "submit",
        "-q",
        "limited/work",
        "--priority",
        "100",
        "--cpu-request",
        "1",
        "--time",
        "5s",
        "--",
        "touch protected; sleep 1",
    ]);
    let competing = service.ok(&[
        "submit",
        "-q",
        "independent",
        "--priority",
        "0",
        "--cpu-request",
        "0.1",
        "--memory-request",
        "1M",
        "--",
        "touch competing",
    ]);
    let memory_only = service.ok(&[
        "submit",
        "-q",
        "independent",
        "--priority",
        "0",
        "--memory-request",
        "1M",
        "--",
        "touch memory-only",
    ]);
    let shared = service.ok(&[
        "submit",
        "-q",
        "limited/sibling",
        "--priority",
        "0",
        "--",
        "touch shared",
    ]);
    let independent = service.ok(&[
        "submit",
        "-q",
        "independent",
        "--priority",
        "0",
        "--",
        "touch independent",
    ]);
    service.wait_file("independent");
    service.ok(&["wait", &independent]);
    for id in [&protected, &competing, &memory_only, &shared] {
        assert_eq!(service.status(id)["state"], "Queued");
    }
    let explanation: serde_json::Value =
        serde_json::from_str(&service.ok(&["explain", &competing, "--json"])).unwrap();
    assert!(
        explanation["blocking_reason"]
            .as_str()
            .unwrap()
            .contains(&format!("protected opportunity for Job {protected}"))
    );
    let chained: serde_json::Value =
        serde_json::from_str(&service.ok(&["explain", &memory_only, "--json"])).unwrap();
    assert!(
        chained["blocking_reason"]
            .as_str()
            .unwrap()
            .contains(&format!("protected opportunity for Job {competing}"))
    );
    service.ok(&["cancel", &busy]);
    service.cli(&["wait", &busy]);
    for id in [&protected, &competing, &memory_only, &shared] {
        service.ok(&["wait", id]);
    }
    assert!(
        service.status(&competing)["started_ms"].as_u64().unwrap()
            >= service.status(&protected)["started_ms"].as_u64().unwrap()
    );
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn confirmed_suspension_survives_restart_keeps_admission_and_allows_graceful_cancel() {
    let mut service = Service::start("recovery");
    let info: serde_json::Value = serde_json::from_str(&service.ok(&["host", "--json"])).unwrap();
    assert_eq!(info["freezer"], true);
    service.ok(&["queue", "create", "serial", "--max-running", "1"]);
    let id = service.ok(&[
        "submit",
        "-q",
        "serial",
        "--time",
        "20s",
        "--",
        "trap 'echo graceful; exit 0' TERM; while :; do echo tick >> ticks; sleep .02; done",
    ]);
    service.wait_file("ticks");
    service.ok(&["suspend", &id]);
    assert!(service.events(&id).contains("frozen 1"));
    let bytes = fs::read(service.root.join("ticks")).unwrap();
    let second = service.ok(&["submit", "-q", "serial", "--", "touch second"]);
    assert_eq!(service.status(&second)["state"], "Queued");
    service.restart();
    assert_eq!(service.status(&id)["state"], "Suspended");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(fs::read(service.root.join("ticks")).unwrap(), bytes);
    let suspended = service.status(&id);
    assert!(suspended["timing"]["suspended_ms"].as_u64().unwrap() >= 300);
    assert_eq!(
        suspended["timing"]["elapsed_ms"].as_u64().unwrap(),
        suspended["timing"]["active_ms"].as_u64().unwrap()
            + suspended["timing"]["suspended_ms"].as_u64().unwrap()
    );
    assert!(!service.root.join("second").exists());
    service.ok(&["cancel", &id]);
    let result = service.cli(&["wait", &id, "--timeout", "3s"]);
    assert!(!result.status.success());
    assert_ne!(result.status.code(), Some(75));
    assert!(service.ok(&["log", &id, "full"]).contains("graceful"));
    service.ok(&["wait", &second, "--timeout", "3s"]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn recursive_controls_report_mixed_subtrees_and_do_not_pause_future_admission() {
    let service = Service::start("recursive");
    service.ok(&["group", "create", "tree"]);
    service.ok(&["group", "create", "tree/nested"]);
    service.ok(&["queue", "create", "tree/direct"]);
    service.ok(&["queue", "create", "tree/nested/leaf"]);
    let one = service.ok(&[
        "submit",
        "-q",
        "tree/direct",
        "--time",
        "20s",
        "--",
        "touch one; sleep 15",
    ]);
    let two = service.ok(&[
        "submit",
        "-q",
        "tree/nested/leaf",
        "--time",
        "20s",
        "--",
        "touch two; sleep 15",
    ]);
    service.wait_file("one");
    service.wait_file("two");
    assert!(!service.cli(&["group", "suspend", "tree"]).status.success());
    let results: serde_json::Value =
        serde_json::from_str(&service.ok(&["group", "suspend", "--recursive", "tree", "--json"]))
            .unwrap();
    assert_eq!(results["results"].as_array().unwrap().len(), 2);
    assert!(
        results["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|result| result["confirmed"] == true)
    );
    assert_eq!(service.status(&one)["state"], "Suspended");
    assert_eq!(service.status(&two)["state"], "Suspended");
    service.ok(&["run", "-q", "tree/direct", "--", "touch later"]);
    service.ok(&["queue", "pause", "tree/nested/leaf"]);
    service.ok(&["group", "continue", "tree", "--recursive"]);
    assert_eq!(service.status(&one)["state"], "Running");
    assert_eq!(service.status(&two)["state"], "Running");
    let queued = service.ok(&["submit", "-q", "tree/nested/leaf", "--", "true"]);
    assert_eq!(service.status(&queued)["state"], "Queued");
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn recursive_cancellation_thaws_frozen_work_and_preserves_admission_holds() {
    let service = Service::start("cancel-tree");
    service.ok(&["group", "create", "tree"]);
    service.ok(&["queue", "create", "tree/q", "--max-running", "1"]);
    let running = service.ok(&[
        "submit",
        "-q",
        "tree/q",
        "--time",
        "20s",
        "--",
        "trap 'echo graceful; exit 0' TERM; touch ready; while :; do sleep .02; done",
    ]);
    service.wait_file("ready");
    service.ok(&["suspend", &running]);
    assert!(service.events(&running).contains("frozen 1"));
    let queued = service.ok(&["submit", "-q", "tree/q", "--", "touch forbidden"]);
    service.ok(&["group", "pause", "tree"]);
    let output = service.ok(&["group", "cancel", "tree", "--recursive", "--json"]);
    let operation: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(operation["complete"], true);
    for id in [&running, &queued] {
        let output = service.cli(&["wait", id, "--timeout", "3s"]);
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(service.status(id)["state"], "Cancelled");
    }
    assert!(service.ok(&["log", &running, "full"]).contains("graceful"));
    assert!(!service.root.join("forbidden").exists());
    let next = service.ok(&["submit", "-q", "tree/q", "--", "true"]);
    assert_eq!(service.status(&next)["state"], "Queued");
    service.ok(&["group", "resume", "tree"]);
    service.ok(&["wait", &next, "--timeout", "3s"]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn resource_requests_leave_kernel_limits_and_weights_unchanged() {
    let service = Service::start("requests");
    service.ok(&["group", "create", "empty"]);
    service.ok(&["queue", "create", "empty/q"]);
    let id = service.ok(&[
        "submit",
        "-q",
        "empty/q",
        "--cpu-request",
        "0.25",
        "--memory-request",
        "1M",
        "--",
        "touch ready; sleep 3",
    ]);
    service.wait_file("ready");
    let path = service.cgroup.join("jobs").join(&id);
    for (name, expected) in [
        ("cpu.weight", "100"),
        ("cpu.max", "max 100000"),
        ("memory.high", "max"),
        ("memory.max", "max"),
        ("memory.swap.max", "max"),
        ("pids.max", "max"),
    ] {
        assert_eq!(
            fs::read_to_string(path.join(name)).unwrap().trim(),
            expected,
            "{name}"
        );
    }
    let job = service.status(&id);
    assert_eq!(job["applied_resources"], serde_json::json!({}));
    assert_eq!(job["reservation"]["vector"]["cores_milli"], 250);
    assert_eq!(job["reservation"]["vector"]["memory"], 1 << 20);
    assert_eq!(job["workload_cgroup"], path.to_str().unwrap());
    service.ok(&["cancel", &id]);
    service.cli(&["wait", &id, "--timeout", "3s"]);
}

#[test]
#[ignore = "requires delegated cgroup v2 through job"]
fn versioned_presets_apply_explicit_controls_while_classes_leave_kernel_defaults() {
    let mut service = Service::start("presets-kernel");
    fs::write(
        service.root.join("config.toml"),
        r#"
schema_version = 1
[[presets.classes]]
name = 'interactive'
revision = 1
priority = 100
[[presets.profiles]]
name = 'bounded'
revision = 1
scheduling_class = 'interactive@1'
[presets.profiles.values]
cpu_request_milli = 100
cpu_limit_milli = 500
cpu_weight = 37
memory_high = 16777216
memory_max = 33554432
memory_swap_max = 0
pids = 16
wall_ms = 20000
"#,
    )
    .unwrap();
    service.ok(&["config", "reload"]);
    service.ok(&["group", "create", "empty"]);
    service.ok(&[
        "queue",
        "create",
        "empty/q",
        "--job-execution-profile",
        "bounded@1",
    ]);
    let bounded = service.ok(&[
        "submit",
        "-q",
        "empty/q",
        "--",
        "touch bounded-ready; sleep 15",
    ]);
    let class_only = service.ok(&[
        "submit",
        "--class",
        "interactive@1",
        "--time",
        "20s",
        "--",
        "touch class-ready; sleep 15",
    ]);
    service.wait_file("bounded-ready");
    service.wait_file("class-ready");
    for (id, values) in [
        (
            &bounded,
            [
                ("cpu.max", "50000 100000"),
                ("cpu.weight", "37"),
                ("memory.high", "16777216"),
                ("memory.max", "33554432"),
                ("memory.swap.max", "0"),
                ("pids.max", "16"),
            ],
        ),
        (
            &class_only,
            [
                ("cpu.max", "max 100000"),
                ("cpu.weight", "100"),
                ("memory.high", "max"),
                ("memory.max", "max"),
                ("memory.swap.max", "max"),
                ("pids.max", "max"),
            ],
        ),
    ] {
        let path = service.cgroup.join("jobs").join(id);
        for (name, expected) in values {
            assert_eq!(
                fs::read_to_string(path.join(name)).unwrap().trim(),
                expected,
                "{id}/{name}"
            );
        }
        assert_eq!(service.status(id)["spec"]["declared"]["priority"], 100);
    }
    assert!(
        service.status(&bounded)["aggregate_domains"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let snapshot = service.status(&bounded)["preset_snapshot"].clone();
    service.restart();
    assert_eq!(service.status(&bounded)["preset_snapshot"], snapshot);
    service.ok(&["update", &bounded, "--cpu-weight", "43"]);
    assert_eq!(
        fs::read_to_string(
            service
                .cgroup
                .join("jobs")
                .join(&bounded)
                .join("cpu.weight")
        )
        .unwrap()
        .trim(),
        "43"
    );
    assert_eq!(service.status(&bounded)["preset_snapshot"], snapshot);
    for id in [bounded, class_only] {
        service.ok(&["cancel", &id]);
        service.cli(&["wait", &id, "--timeout", "3s"]);
    }
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn explicit_kernel_limits_do_not_create_requests_and_cpu_quota_throttles() {
    let service = Service::start("limits");
    let id = service.ok(&[
        "submit",
        "--cpu-limit",
        "0.1",
        "--cpu-weight",
        "37",
        "--memory-high",
        "16M",
        "--memory-max",
        "32M",
        "--memory-swap-max",
        "0",
        "--",
        "touch ready; timeout 2s bash -c 'while :; do :; done'",
    ]);
    service.wait_file("ready");
    let job = service.status(&id);
    assert_eq!(job["reservation"]["vector"]["cores_milli"], 0);
    assert_eq!(job["reservation"]["vector"]["memory"], 0);
    let path = service.cgroup.join("jobs").join(&id);
    for (name, expected) in [
        ("cpu.max", "10000 100000".to_owned()),
        ("cpu.weight", "37".to_owned()),
        ("memory.high", (16u64 << 20).to_string()),
        ("memory.max", (32u64 << 20).to_string()),
        ("memory.swap.max", "0".to_owned()),
    ] {
        assert_eq!(
            fs::read_to_string(path.join(name)).unwrap().trim(),
            expected,
            "{name}"
        );
        assert_eq!(job["applied_resources"][name], expected);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let stat = fs::read_to_string(path.join("cpu.stat")).unwrap_or_default();
        let throttled = stat
            .lines()
            .find_map(|line| line.strip_prefix("throttled_usec "))
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        if throttled > 0 {
            break;
        }
        assert!(Instant::now() < deadline, "quota did not throttle: {stat}");
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = service.cli(&["wait", &id, "--timeout", "3s", "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(job["result"]["exit_code"], 124);
    assert!(job["stop"].is_null(), "{job}");
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn resource_default_overrides_unlimited_unset_and_restart_remain_explicit() {
    let mut service = Service::start("resource-defaults");
    service.ok(&[
        "group",
        "create",
        "tree",
        "--job-cpu-limit",
        "0.25",
        "--job-cpu-weight",
        "42",
        "--job-memory-high",
        "16M",
        "--job-memory-max",
        "32M",
    ]);
    service.ok(&["queue", "create", "tree/q", "--job-cpu-limit", "unlimited"]);
    let id = service.ok(&[
        "submit",
        "-q",
        "tree/q",
        "--memory-high",
        "16777217",
        "--",
        "touch ready; sleep 5",
    ]);
    service.wait_file("ready");
    let path = service.cgroup.join("jobs").join(&id);
    assert_eq!(
        fs::read_to_string(path.join("cpu.max")).unwrap().trim(),
        "max 100000"
    );
    assert_eq!(
        fs::read_to_string(path.join("cpu.weight")).unwrap().trim(),
        "42"
    );
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
    let rounded = 16777217u64.div_ceil(page) * page;
    assert_eq!(
        fs::read_to_string(path.join("memory.high")).unwrap().trim(),
        rounded.to_string()
    );
    assert_eq!(
        service.status(&id)["applied_resources"]["memory.high"],
        rounded.to_string()
    );
    service.ok(&["queue", "unset", "tree/q", "job-cpu-limit"]);
    service.restart();
    assert_eq!(
        fs::read_to_string(path.join("cpu.max")).unwrap().trim(),
        "max 100000"
    );
    let prior = service.status(&id);
    assert_eq!(
        prior["resource_sources"]["cpu_limit_milli"]["Object"]["path"],
        "tree/q"
    );
    let next = service.ok(&["submit", "-q", "tree/q", "--", "touch next; sleep 3"]);
    service.wait_file("next");
    assert_eq!(
        fs::read_to_string(service.cgroup.join("jobs").join(&next).join("cpu.max"))
            .unwrap()
            .trim(),
        "25000 100000"
    );
    for id in [&id, &next] {
        service.ok(&["cancel", id]);
        service.cli(&["wait", id, "--timeout", "3s"]);
    }
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn legacy_cores_remain_a_request_and_weight_without_a_cpu_quota() {
    let service = Service::start("legacy-cores");
    let id = service.ok(&[
        "submit",
        "--cores",
        "2",
        "--mem",
        "32M",
        "--",
        "touch ready; sleep 3",
    ]);
    service.wait_file("ready");
    let path = service.cgroup.join("jobs").join(&id);
    assert_eq!(
        fs::read_to_string(path.join("cpu.weight")).unwrap().trim(),
        "200"
    );
    assert_eq!(
        fs::read_to_string(path.join("cpu.max")).unwrap().trim(),
        "max 100000"
    );
    let job = service.status(&id);
    assert_eq!(job["reservation"]["vector"]["cores_milli"], 2000);
    assert_eq!(job["resource_sources"]["cpu_weight"], "Compatibility");
    assert_eq!(job["spec"]["declared"]["memory_request"], 32 << 20);
    assert_eq!(job["spec"]["declared"]["memory_max"], 32 << 20);
    service.ok(&["cancel", &id]);
    service.cli(&["wait", &id, "--timeout", "3s"]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn an_ancestor_freeze_prevents_false_continue_confirmation() {
    let service = Service::start("ancestor");
    let id = service.ok(&["submit", "--time", "20s", "--", "touch ready; sleep 15"]);
    service.wait_file("ready");
    service.ok(&["suspend", &id]);
    fs::write(service.cgroup.join("jobs/cgroup.freeze"), "1").unwrap();
    let continued = service.cli(&["continue", &id, "--timeout", "200ms", "--json"]);
    assert_eq!(continued.status.code(), Some(75));
    let result: serde_json::Value = serde_json::from_slice(&continued.stdout).unwrap();
    assert_eq!(result["results"][0]["confirmed"], false);
    assert_eq!(service.status(&id)["state"], "Suspended");
    fs::write(service.cgroup.join("jobs/cgroup.freeze"), "0").unwrap();
    service.ok(&["continue", &id]);
    assert!(service.events(&id).contains("frozen 0"));
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn elapsed_deadlines_continue_while_a_workload_is_frozen() {
    let service = Service::start("deadline");
    let id = service.ok(&["submit", "--time", "1s", "--", "touch ready; sleep 15"]);
    service.wait_file("ready");
    service.ok(&["suspend", &id]);
    let waited = service.cli(&["wait", &id, "--timeout", "4s"]);
    assert!(!waited.status.success());
    assert_ne!(waited.status.code(), Some(75));
    let job = service.status(&id);
    assert_eq!(job["state"], "Failed");
    assert!(job["timing"]["suspended_ms"].as_u64().unwrap() > 0);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn a_failed_intent_write_does_not_freeze_the_workload() {
    let service = Service::start("persistence");
    let id = service.ok(&["submit", "--time", "20s", "--", "touch ready; sleep 15"]);
    service.wait_file("ready");
    let blocked = service
        .root
        .join("state/jobs")
        .join(&id)
        .join(format!("job.tmp{}", service.child.id()));
    fs::create_dir(&blocked).unwrap();
    let result = service.cli(&["suspend", &id, "--json"]);
    assert!(!result.status.success());
    let result: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(result["results"][0]["accepted"], false);
    assert!(service.events(&id).contains("frozen 0"));
    fs::remove_dir(blocked).unwrap();
    service.ok(&["suspend", &id]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn recovery_applies_durable_freeze_intent_that_was_not_yet_confirmed() {
    let mut service = Service::start("intent-recovery");
    let id = service.ok(&["submit", "--time", "20s", "--", "touch ready; sleep 15"]);
    service.wait_file("ready");
    service.child.kill().unwrap();
    service.child.wait().unwrap();
    let path = service.root.join("state/jobs").join(&id).join("job.json");
    let mut job: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    job["suspension"]["requested"] = serde_json::json!(true);
    job["suspension"]["pending"] = serde_json::json!(true);
    job["suspension"]["generation"] = serde_json::json!(1);
    fs::write(path, serde_json::to_vec(&job).unwrap()).unwrap();
    service.child = Service::spawn(&service.root, &service.cgroup);
    service.ready();
    let deadline = Instant::now() + Duration::from_secs(3);
    while service.status(&id)["state"] != "Suspended" {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(service.events(&id).contains("frozen 1"));
    service.ok(&["continue", &id]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn recovery_collects_completion_when_a_suspended_workload_ended_while_offline() {
    let mut service = Service::start("offline-completion");
    let id = service.ok(&["submit", "--time", "20s", "--", "touch ready; sleep 15"]);
    service.wait_file("ready");
    service.ok(&["suspend", &id]);
    service.child.kill().unwrap();
    service.child.wait().unwrap();
    fs::write(
        service.cgroup.join("jobs").join(&id).join("cgroup.kill"),
        "1",
    )
    .unwrap();
    service.wait_file(&format!("state/jobs/{id}/result.json"));
    service.child = Service::spawn(&service.root, &service.cgroup);
    service.ready();
    let result = service.cli(&["wait", &id, "--timeout", "3s"]);
    assert_ne!(result.status.code(), Some(75));
    assert_eq!(service.status(&id)["state"], "Failed");
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn an_opposing_control_supersedes_a_waiter_without_blocking_the_daemon() {
    let service = Service::start("opposing-control");
    let id = service.ok(&["submit", "--time", "20s", "--", "touch ready; sleep 15"]);
    service.wait_file("ready");
    service.ok(&["suspend", &id]);
    fs::write(service.cgroup.join("jobs/cgroup.freeze"), "1").unwrap();
    let waiter = Command::new(env!("CARGO_BIN_EXE_job"))
        .args(["continue", &id, "--timeout", "5s", "--json"])
        .env("JOB_STATE_DIR", service.root.join("state"))
        .env("LC_ALL", "C")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while service.status(&id)["suspension"]["requested"] != false {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    service.ok(&["suspend", &id]);
    let output = waiter.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["results"][0]["confirmed"], false);
    assert_eq!(
        response["results"][0]["error"],
        "previous control request was superseded"
    );
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn aggregate_domains_share_ceilings_and_skip_empty_objects() {
    let service = Service::start("aggregate-hierarchy");
    service.ok(&[
        "group",
        "create",
        "pool",
        "--cpu-limit",
        "0.5",
        "--memory-max",
        "64M",
    ]);
    service.ok(&["group", "create", "pool/organization"]);
    service.ok(&["queue", "create", "pool/direct"]);
    service.ok(&[
        "queue",
        "create",
        "pool/organization/nested",
        "--memory-max",
        "unlimited",
        "--job-cpu-weight",
        "23",
    ]);
    let one = service.ok(&["submit", "-q", "pool/direct", "--", "touch one; sleep 20"]);
    let two = service.ok(&[
        "submit",
        "-q",
        "pool/organization/nested",
        "--",
        "touch two; sleep 20",
    ]);
    service.wait_file("one");
    service.wait_file("two");
    let a = service.status(&one);
    let b = service.status(&two);
    let first = PathBuf::from(a["workload_cgroup"].as_str().unwrap());
    let second = PathBuf::from(b["workload_cgroup"].as_str().unwrap());
    let pool = first.parent().unwrap();
    assert_eq!(second.parent().unwrap().parent().unwrap(), pool);
    assert_eq!(
        fs::read_to_string(pool.join("cpu.max")).unwrap().trim(),
        "50000 100000"
    );
    assert_eq!(
        fs::read_to_string(pool.join("memory.max")).unwrap().trim(),
        "67108864"
    );
    assert_eq!(
        fs::read_to_string(pool.join("memory.oom.group"))
            .unwrap()
            .trim(),
        "0"
    );
    assert_eq!(
        fs::read_to_string(first.join("memory.max")).unwrap().trim(),
        "max"
    );
    assert_eq!(
        fs::read_to_string(second.parent().unwrap().join("memory.max"))
            .unwrap()
            .trim(),
        "max"
    );
    assert_eq!(
        fs::read_to_string(second.join("cpu.weight"))
            .unwrap()
            .trim(),
        "23"
    );
    assert_eq!(a["aggregate_domains"].as_array().unwrap().len(), 1);
    assert_eq!(b["aggregate_domains"].as_array().unwrap().len(), 2);
    assert_eq!(a["aggregate_domains"][0], b["aggregate_domains"][0]);
    assert!(a["applied_resources"].as_object().unwrap().is_empty());
    let inspect: serde_json::Value =
        serde_json::from_str(&service.ok(&["queue", "show", "pool/organization/nested", "--json"]))
            .unwrap();
    assert_eq!(
        inspect["objects"][0]["aggregate_domains"],
        b["aggregate_domains"]
    );
    assert!(
        fs::read_to_string(pool.join("cgroup.procs"))
            .unwrap()
            .trim()
            .is_empty()
    );
    service.ok(&["suspend", &two]);
    assert_eq!(service.status(&two)["state"], "Suspended");
    assert_eq!(service.status(&one)["state"], "Running");
    service.ok(&["continue", &two]);
    service.ok(&["signal", "-s", "TERM", &one]);
    service.cli(&["wait", &one]);
    assert_eq!(service.status(&two)["state"], "Running");
    service.ok(&["group", "cancel", "pool", "--recursive"]);
    service.cli(&["wait", &two]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn aggregate_policy_changes_require_idle_work_and_rebuild_for_retry() {
    let mut service = Service::start("aggregate-reconfigure");
    service.ok(&["group", "create", "pool", "--cpu-limit", "1"]);
    service.ok(&["queue", "create", "pool/q"]);
    let id = service.ok(&["submit", "-q", "pool/q", "--", "touch ready; sleep 20"]);
    service.wait_file("ready");
    let before = service.status(&id);
    for args in [
        vec!["group", "set", "pool", "--cpu-limit", "0.5"],
        vec!["group", "unset", "pool", "cpu-limit"],
        vec!["queue", "set", "pool/q", "--memory-max", "32M"],
    ] {
        let result = service.cli(&args);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("inactive subtree"));
    }
    service.ok(&["queue", "set", "pool/q", "--job-cpu-weight", "11"]);
    service.ok(&["queue", "pause", "pool/q"]);
    service.ok(&["suspend", &id]);
    service.restart();
    let recovered = service.status(&id);
    assert_eq!(recovered["aggregate_domains"], before["aggregate_domains"]);
    assert_eq!(recovered["workload_cgroup"], before["workload_cgroup"]);
    assert_eq!(recovered["state"], "Suspended");
    service.ok(&["cancel", &id]);
    service.cli(&["wait", &id]);
    service.ok(&["group", "unset", "pool", "cpu-limit"]);
    service.ok(&["queue", "resume", "pool/q"]);
    service.ok(&["retry", &id]);
    let after = service.status(&id);
    assert!(after["aggregate_domains"].as_array().unwrap().is_empty());
    assert_eq!(
        PathBuf::from(after["workload_cgroup"].as_str().unwrap())
            .parent()
            .unwrap(),
        service.cgroup.join("jobs")
    );
    assert_eq!(after["spec"]["declared"]["cpu_weight"], 11);
    let attempts: serde_json::Value =
        serde_json::from_str(&service.ok(&["attempts", &id, "--json"])).unwrap();
    assert_eq!(
        attempts["attempts"][0]["aggregate_domains"],
        before["aggregate_domains"]
    );
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn aggregate_cpu_quota_throttles_shared_work_without_per_job_quotas() {
    let service = Service::start("aggregate-throttle");
    service.ok(&["queue", "create", "limited", "--cpu-limit", "0.1"]);
    let one = service.ok(&[
        "submit",
        "-q",
        "limited",
        "--",
        "touch one; timeout 4s bash -c 'while :; do :; done'",
    ]);
    let two = service.ok(&[
        "submit",
        "-q",
        "limited",
        "--",
        "touch two; timeout 4s bash -c 'while :; do :; done'",
    ]);
    service.wait_file("one");
    service.wait_file("two");
    let a = service.status(&one);
    let b = service.status(&two);
    let first = PathBuf::from(a["workload_cgroup"].as_str().unwrap());
    let second = PathBuf::from(b["workload_cgroup"].as_str().unwrap());
    assert_eq!(first.parent(), second.parent());
    for leaf in [&first, &second] {
        assert_eq!(
            fs::read_to_string(leaf.join("cpu.max")).unwrap().trim(),
            "max 100000"
        );
    }
    let domain = first.parent().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let stat = fs::read_to_string(domain.join("cpu.stat")).unwrap();
        if stat.lines().any(|line| {
            line.strip_prefix("throttled_usec ")
                .is_some_and(|value| value.parse::<u64>().unwrap() > 0)
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "{stat}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn aggregate_control_drift_stops_affected_jobs() {
    let service = Service::start("aggregate-drift");
    service.ok(&["queue", "create", "limited", "--cpu-weight", "43"]);
    let id = service.ok(&["submit", "-q", "limited", "--", "touch ready; sleep 20"]);
    service.wait_file("ready");
    let status = service.status(&id);
    let leaf = PathBuf::from(status["workload_cgroup"].as_str().unwrap());
    fs::write(leaf.parent().unwrap().join("cpu.weight"), "44").unwrap();
    service.cli(&["wait", &id]);
    let status = service.status(&id);
    assert_eq!(status["stop"]["kind"], "LimitChanged");
    assert!(
        status["stop"]["line"]
            .as_str()
            .unwrap()
            .contains("aggregate resource control changed")
    );
}

#[test]
#[ignore = "bounded helper used by the aggregate memory integration test"]
fn aggregate_memory_fixture() {
    let Ok(marker) = std::env::var("JOB_AGGREGATE_MEMORY_FIXTURE") else {
        return;
    };
    let mut memory = vec![0u8; 40 * 1024 * 1024];
    memory.fill(1);
    fs::write(marker, b"ready").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !Path::new("release-memory").exists() {
        std::hint::black_box(&memory);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn aggregate_memory_max_is_shared_across_jobs_and_reports_oom() {
    let service = Service::start("aggregate-memory");
    service.ok(&[
        "queue",
        "create",
        "limited",
        "--memory-max",
        "64M",
        "--memory-swap-max",
        "0",
    ]);
    let executable = std::env::current_exe().unwrap();
    let submit = |marker| {
        service.ok(&[
            "submit",
            "-q",
            "limited",
            "--",
            "env",
            marker,
            executable.to_str().unwrap(),
            "--exact",
            "aggregate_memory_fixture",
            "--ignored",
            "--nocapture",
        ])
    };
    let one = submit("JOB_AGGREGATE_MEMORY_FIXTURE=one");
    service.wait_file("one");
    let first = service.status(&one);
    let leaf = PathBuf::from(first["workload_cgroup"].as_str().unwrap());
    assert_eq!(
        fs::read_to_string(leaf.join("memory.max")).unwrap().trim(),
        "max"
    );
    let two = submit("JOB_AGGREGATE_MEMORY_FIXTURE=two");
    let deadline = Instant::now() + Duration::from_secs(5);
    let victim = loop {
        if service.status(&one)["stop"]["kind"] == "Memory" {
            break one.clone();
        }
        if service.status(&two)["stop"]["kind"] == "Memory" {
            break two.clone();
        }
        assert!(
            Instant::now() < deadline,
            "shared memory ceiling did not report an OOM victim"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let status = service.status(&victim);
    assert!(status["result"]["oom_kill"].as_u64().unwrap() > 0);
    assert!(
        status["stop"]["line"]
            .as_str()
            .unwrap()
            .contains("kernel OOM kill")
    );
    fs::write(service.root.join("release-memory"), b"release").unwrap();
    service.cli(&["wait", &one]);
    service.cli(&["wait", &two]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn recovery_refuses_changed_aggregate_controls_before_serving_requests() {
    let mut service = Service::start("aggregate-recovery-refusal");
    service.ok(&["queue", "create", "limited", "--cpu-weight", "43"]);
    let id = service.ok(&["submit", "-q", "limited", "--", "touch ready; sleep 20"]);
    service.wait_file("ready");
    let before = service.status(&id);
    let leaf = PathBuf::from(before["workload_cgroup"].as_str().unwrap());
    service.child.kill().unwrap();
    service.child.wait().unwrap();
    fs::write(leaf.parent().unwrap().join("cpu.weight"), "44").unwrap();
    service.child = Service::spawn(&service.root, &service.cgroup);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = service.child.try_wait().unwrap() {
            assert!(!status.success());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        fs::read_to_string(service.root.join("daemon.err"))
            .unwrap()
            .contains("original aggregate resource domains")
    );
    fs::write(leaf.parent().unwrap().join("cpu.weight"), "43").unwrap();
    service.child = Service::spawn(&service.root, &service.cgroup);
    service.ready();
    assert_eq!(service.status(&id)["state"], "Running");
    assert_eq!(
        service.status(&id)["workload_cgroup"],
        before["workload_cgroup"]
    );
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn aggregate_changes_in_an_idle_branch_preserve_running_siblings() {
    let service = Service::start("aggregate-idle-branch");
    service.ok(&["group", "create", "pool", "--cpu-limit", "1"]);
    service.ok(&["queue", "create", "pool/running"]);
    service.ok(&["queue", "create", "pool/idle"]);
    let id = service.ok(&[
        "submit",
        "-q",
        "pool/running",
        "--",
        "touch ready; sleep 20",
    ]);
    service.wait_file("ready");
    let before = service.status(&id);
    service.ok(&["queue", "set", "pool/idle", "--memory-max", "64M"]);
    service.ok(&["run", "-q", "pool/idle", "--", "true"]);
    service.ok(&["queue", "pause", "pool/idle"]);
    let waiting = service.ok(&["submit", "-q", "pool/idle", "--", "touch later; sleep 20"]);
    service.ok(&["queue", "set", "pool/idle", "--memory-max", "32M"]);
    let refused = service.cli(&[
        "create",
        "-q",
        "pool/idle",
        "--on",
        "localhost",
        "--",
        "true",
    ]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("aggregate controls require local execution")
    );
    service.ok(&["queue", "resume", "pool/idle"]);
    service.wait_file("later");
    assert_eq!(
        service.status(&id)["workload_cgroup"],
        before["workload_cgroup"]
    );
    assert_eq!(service.status(&id)["state"], "Running");
    assert_eq!(
        service.status(&waiting)["aggregate_domains"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let parent = service.status(&waiting)["aggregate_domains"][0].clone();
    assert_eq!(parent, before["aggregate_domains"][0]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn unchanged_aggregate_domains_keep_kernel_identity_between_jobs() {
    use std::os::unix::fs::MetadataExt;
    let service = Service::start("aggregate-domain-lifetime");
    service.ok(&["queue", "create", "limited", "--cpu-weight", "43"]);
    let first = service.ok(&["submit", "-q", "limited", "--", "true"]);
    service.cli(&["wait", &first]);
    let status = service.status(&first);
    let leaf = PathBuf::from(status["workload_cgroup"].as_str().unwrap());
    let domain = leaf.parent().unwrap();
    let original_inode = fs::metadata(domain).unwrap().ino();
    service.ok(&["run", "-q", "limited", "--", "true"]);
    assert_eq!(fs::metadata(domain).unwrap().ino(), original_inode);
    service.ok(&["queue", "unset", "limited", "cpu-weight"]);
    service.ok(&["run", "-q", "limited", "--", "true"]);
    assert!(!domain.exists());
}

fn filesystem_device(service: &Service) -> String {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let mut dev = fs::metadata(&service.root).unwrap().dev();
    let mut id = format!("{}:{}", libc::major(dev), libc::minor(dev));
    if !Path::new("/sys/dev/block").join(&id).exists() {
        let output = Command::new("findmnt")
            .args(["--noheadings", "--output", "SOURCE", "--target"])
            .arg(&service.root)
            .output()
            .unwrap();
        assert!(output.status.success());
        let source = String::from_utf8(output.stdout).unwrap();
        let source = source.trim().split('[').next().unwrap();
        let metadata =
            fs::metadata(source).expect("live I/O test requires a block-backed filesystem");
        assert!(metadata.file_type().is_block_device());
        dev = metadata.rdev();
        id = format!("{}:{}", libc::major(dev), libc::minor(dev));
    }
    let path = Path::new("/sys/dev/block")
        .join(&id)
        .canonicalize()
        .unwrap();
    if path.join("partition").exists() {
        fs::read_to_string(path.parent().unwrap().join("dev"))
            .unwrap()
            .trim()
            .to_owned()
    } else {
        id
    }
}

fn io_source(service: &Service) {
    let mut bytes = vec![0u8; 1048576];
    fs::File::open("/dev/urandom")
        .unwrap()
        .read_exact(&mut bytes)
        .unwrap();
    let path = service.root.join("io-source");
    fs::write(&path, bytes).unwrap();
    fs::File::open(path).unwrap().sync_all().unwrap();
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn device_io_max_enforces_bounded_direct_writes_without_creating_reservations() {
    let service = Service::start("io-job-limit");
    io_source(&service);
    let device = filesystem_device(&service);
    let limit = format!("{device},rbps=1M,wbps=256K,wiops=100");
    let id = service.ok(&[
        "submit",
        "--io-max",
        &limit,
        "--",
        "timeout",
        "12s",
        "dd",
        "if=io-source",
        "of=io-data",
        "bs=64K",
        "count=16",
        "oflag=direct",
        "conv=fdatasync",
        "status=none",
    ]);
    let status = service.status(&id);
    let leaf = PathBuf::from(status["workload_cgroup"].as_str().unwrap());
    let control = fs::read_to_string(leaf.join("io.max")).unwrap();
    assert!(
        control.contains(&format!(
            "{device} rbps=1048576 wbps=262144 riops=max wiops=100"
        )),
        "{control}"
    );
    assert_eq!(status["reservation"]["vector"]["cores_milli"], 0);
    assert_eq!(status["reservation"]["vector"]["memory"], 0);
    assert!(
        status["applied_resources"]["io.max"]
            .as_str()
            .unwrap()
            .contains("wbps=262144")
    );
    service.ok(&["wait", &id]);
    let status = service.status(&id);
    assert_eq!(status["state"], "Succeeded");
    assert!(
        status["timing"]["elapsed_ms"].as_u64().unwrap() >= 2000,
        "{status}"
    );
    assert_eq!(
        fs::metadata(service.root.join("io-data")).unwrap().len(),
        1048576
    );
    assert!(status["usage"]["written"].as_u64().unwrap() >= 1048576);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn aggregate_io_ceiling_is_shared_while_job_defaults_remain_independent() {
    let mut service = Service::start("io-aggregate");
    io_source(&service);
    let device = filesystem_device(&service);
    let maximum = format!("{device},wbps=512K");
    let per_job = format!("{device},rbps=1M,wbps=unlimited");
    service.ok(&["group", "create", "pool", "--io-max", &maximum]);
    service.ok(&["queue", "create", "pool/q", "--job-io-max", &per_job]);
    let one = service.ok(&["submit", "-q", "pool/q", "--", "touch one; timeout 12s dd if=io-source of=one-data bs=64K count=16 oflag=direct conv=fdatasync status=none"]);
    let two = service.ok(&["submit", "-q", "pool/q", "--", "touch two; timeout 12s dd if=io-source of=two-data bs=64K count=16 oflag=direct conv=fdatasync status=none"]);
    service.wait_file("one");
    service.wait_file("two");
    let a = service.status(&one);
    let b = service.status(&two);
    let leaf = PathBuf::from(a["workload_cgroup"].as_str().unwrap());
    assert_eq!(a["aggregate_domains"], b["aggregate_domains"]);
    assert!(
        fs::read_to_string(leaf.parent().unwrap().join("io.max"))
            .unwrap()
            .contains("wbps=524288")
    );
    assert!(
        fs::read_to_string(leaf.join("io.max"))
            .unwrap()
            .contains("wbps=max")
    );
    service.restart();
    service.ok(&["wait", &one]);
    service.ok(&["wait", &two]);
    let longest = [&one, &two]
        .iter()
        .map(|id| service.status(id)["timing"]["elapsed_ms"].as_u64().unwrap())
        .max()
        .unwrap();
    assert!(longest >= 2000);
    service.ok(&["group", "unset", "pool", "io-max"]);
    service.ok(&["queue", "unset", "pool/q", "job-io-max"]);
    service.ok(&["run", "-q", "pool/q", "--", "true"]);
    assert!(!leaf.parent().unwrap().exists());
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn io_capabilities_refuse_inactive_weight_backends_and_partitions_before_creation() {
    let service = Service::start("io-capabilities");
    let host: serde_json::Value = serde_json::from_str(&service.ok(&["host", "--json"])).unwrap();
    let devices = host["io_devices"].as_array().unwrap();
    assert!(!devices.is_empty());
    for info in devices {
        let device = info["device"].as_str().unwrap();
        for (flag, field) in [
            ("--io-weight", "io_weight"),
            ("--io-bfq-weight", "io_bfq_weight"),
        ] {
            if info[field] == false {
                let output = service.cli(&["create", flag, &format!("{device}=100"), "--", "true"]);
                assert!(!output.status.success(), "{device}: {field}");
            }
        }
        if info["whole_disk"] == false {
            assert!(
                !service
                    .cli(&[
                        "create",
                        "--io-max",
                        &format!("{device},wbps=1M"),
                        "--",
                        "true"
                    ])
                    .status
                    .success()
            );
        }
    }
    assert!(
        !service
            .cli(&[
                "create",
                "--io-max",
                "4294967295:4294967295,wbps=1M",
                "--",
                "true"
            ])
            .status
            .success()
    );
    assert_eq!(service.ok(&["create", "--", "true"]), "1");
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn per_job_io_drift_is_not_adopted_after_daemon_restart() {
    let mut service = Service::start("io-recovery-drift");
    let device = filesystem_device(&service);
    let id = service.ok(&[
        "submit",
        "--io-max",
        &format!("{device},wbps=1M"),
        "--",
        "touch ready; sleep 20",
    ]);
    service.wait_file("ready");
    let before = service.status(&id);
    let leaf = PathBuf::from(before["workload_cgroup"].as_str().unwrap());
    service.child.kill().unwrap();
    service.child.wait().unwrap();
    fs::write(leaf.join("io.max"), format!("{device} wbps=2097152")).unwrap();
    service.child = Service::spawn(&service.root, &service.cgroup);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = service.child.try_wait().unwrap() {
            assert!(!status.success());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        fs::read_to_string(service.root.join("daemon.err"))
            .unwrap()
            .contains("original resource-control backend")
    );
    fs::write(leaf.join("io.max"), format!("{device} wbps=1048576")).unwrap();
    service.child = Service::spawn(&service.root, &service.cgroup);
    service.ready();
    assert_eq!(service.status(&id)["state"], "Running");
    fs::write(leaf.join("io.max"), format!("{device} wbps=max")).unwrap();
    service.cli(&["wait", &id]);
    assert_eq!(service.status(&id)["stop"]["kind"], "LimitChanged");
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn device_read_iops_limits_operations_independently_of_byte_rate() {
    let service = Service::start("io-read-iops");
    let device = filesystem_device(&service);
    io_source(&service);
    let id = service.ok(&[
        "submit",
        "--io-max",
        &format!("{device},riops=4"),
        "--",
        "timeout",
        "12s",
        "dd",
        "if=io-source",
        "of=/dev/null",
        "bs=64K",
        "count=16",
        "iflag=direct",
        "status=none",
    ]);
    let status = service.status(&id);
    assert!(
        status["applied_resources"]["io.max"]
            .as_str()
            .unwrap()
            .contains("rbps=max wbps=max riops=4 wiops=max")
    );
    service.ok(&["wait", &id]);
    let elapsed = service.status(&id)["timing"]["elapsed_ms"]
        .as_u64()
        .unwrap();
    assert!(elapsed >= 2000, "direct reads finished in {elapsed} ms");
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn recovery_of_a_pre_cgroup_launch_crash_requeues_work_that_never_executed() {
    let mut service = Service::start("io-launch-crash");
    let device = filesystem_device(&service);
    let id = service.ok(&[
        "create",
        "--io-max",
        &format!("{device},wbps=1M"),
        "--",
        "touch executed",
    ]);
    service.child.kill().unwrap();
    service.child.wait().unwrap();
    let path = service.root.join("state/jobs").join(&id).join("job.json");
    let mut job: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    job["state"] = serde_json::json!("Starting");
    job["supervisor_boot_id"] = serde_json::json!(
        fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .unwrap()
            .trim()
    );
    job["workload_cgroup"] = serde_json::json!(service.cgroup.join("jobs").join(&id));
    job["launch_gated"] = serde_json::json!(true);
    fs::write(&path, serde_json::to_vec(&job).unwrap()).unwrap();
    assert!(!service.root.join("executed").exists());
    service.child = Service::spawn(&service.root, &service.cgroup);
    service.ready();
    service.cli(&["wait", &id]);
    let status = service.status(&id);
    assert_eq!(status["state"], "Succeeded");
    assert_eq!(status["attempt"], 1);
    assert!(status["applied_resources"]["io.max"].is_string());
    assert!(service.root.join("executed").exists());
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn recovery_of_an_ungated_launch_crash_still_records_lost_work() {
    let mut service = Service::start("ungated-launch-crash");
    let id = service.ok(&["create", "--", "touch executed"]);
    service.child.kill().unwrap();
    service.child.wait().unwrap();
    let path = service.root.join("state/jobs").join(&id).join("job.json");
    let mut job: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    job["state"] = serde_json::json!("Starting");
    job["supervisor_boot_id"] = serde_json::json!(
        fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .unwrap()
            .trim()
    );
    job["workload_cgroup"] = serde_json::json!(service.cgroup.join("jobs").join(&id));
    fs::write(&path, serde_json::to_vec(&job).unwrap()).unwrap();
    service.child = Service::spawn(&service.root, &service.cgroup);
    service.ready();
    service.cli(&["wait", &id]);
    assert_eq!(service.status(&id)["state"], "Lost");
    assert!(!service.root.join("executed").exists());
}

fn resource_rpc(service: &Service, request: serde_json::Value) -> serde_json::Value {
    let mut stream = UnixStream::connect(service.root.join("state/daemon.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    writeln!(
        stream,
        "{}",
        serde_json::json!({"Versioned": {"protocol": 20, "request": request}})
    )
    .unwrap();
    let mut line = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(stream), &mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn update_receipt(service: &Service, plan: &serde_json::Value, phase: &str, completed: usize) {
    let directory = service.root.join("state/resource-updates");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join(format!("{}.json", plan["operation"].as_str().unwrap())),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1, "actor_uid": unsafe {libc::geteuid()}, "requested_ms": 1,
            "plan": plan, "phase": phase, "completed_steps": completed, "error": null
        }))
        .unwrap(),
    )
    .unwrap();
}

fn completed_update(service: &Service, operation: &str) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let result = service.cli(&["resource-update", operation, "--json"]);
        let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        if value["phase"] == "Applied" {
            return value;
        }
        assert!(Instant::now() < deadline, "{value}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn live_update_changes_controls_and_preserves_the_launch_specification() {
    let mut service = Service::start("live-update-job");
    let id = service.ok(&[
        "submit",
        "--cpu-request",
        "0.5",
        "--",
        "touch ready; sleep 30",
    ]);
    service.wait_file("ready");
    let before = service.status(&id);
    let receipt: serde_json::Value = serde_json::from_str(&service.ok(&[
        "update",
        &id,
        "--cpu-limit",
        "0.25",
        "--cpu-weight",
        "200",
        "--memory-high",
        "32M",
        "--json",
    ]))
    .unwrap();
    assert_eq!(receipt["phase"], "Applied");
    let path = PathBuf::from(before["workload_cgroup"].as_str().unwrap());
    assert_eq!(
        fs::read_to_string(path.join("cpu.max")).unwrap().trim(),
        "25000 100000"
    );
    let after = service.status(&id);
    for field in [
        "spec",
        "submitted_spec",
        "requested_spec",
        "effective_spec",
        "reservation",
    ] {
        assert_eq!(before[field], after[field], "{field}");
    }
    assert_eq!(after["applied_resources"]["cpu.weight"], "200");
    service.restart();
    assert_eq!(service.status(&id)["state"], "Running");
    assert_eq!(
        service.status(&id)["applied_resources"]["cpu.weight"],
        "200"
    );
    service.ok(&["cancel", &id]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn live_memory_reduction_requires_explicit_oom_consent() {
    let service = Service::start("live-update-memory");
    let id = service.ok(&["submit", "--", "touch ready; sleep 30"]);
    service.wait_file("ready");
    let refused = service.cli(&["update", &id, "--memory-max", "64M"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--allow-oom"));
    let path = PathBuf::from(service.status(&id)["workload_cgroup"].as_str().unwrap());
    assert_eq!(
        fs::read_to_string(path.join("memory.max")).unwrap().trim(),
        "max"
    );
    service.ok(&["update", &id, "--memory-max", "64M", "--allow-oom"]);
    assert_eq!(
        fs::read_to_string(path.join("memory.max")).unwrap().trim(),
        "67108864"
    );
    service.ok(&["update", &id, "--memory-max", "unlimited"]);
    assert_eq!(
        fs::read_to_string(path.join("memory.max")).unwrap().trim(),
        "max"
    );
    service.ok(&["cancel", &id]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn live_group_update_changes_one_shared_domain_and_survives_restart() {
    let mut service = Service::start("live-update-group");
    service.ok(&["group", "create", "work", "--cpu-limit", "1"]);
    service.ok(&["queue", "create", "work/a"]);
    service.ok(&["queue", "create", "work/b"]);
    let one = service.ok(&["submit", "-q", "work/a", "--", "touch one; sleep 30"]);
    let two = service.ok(&["submit", "-q", "work/b", "--", "touch two; sleep 30"]);
    service.wait_file("one");
    service.wait_file("two");
    service.ok(&[
        "group",
        "update",
        "work",
        "--cpu-limit",
        "0.5",
        "--cpu-weight",
        "150",
    ]);
    for id in [&one, &two] {
        let status = service.status(id);
        assert_eq!(
            status["aggregate_domains"][0]["limits"]["cpu.max"],
            "50000 100000"
        );
        let path = PathBuf::from(status["workload_cgroup"].as_str().unwrap());
        assert_eq!(
            fs::read_to_string(path.parent().unwrap().join("cpu.max"))
                .unwrap()
                .trim(),
            "50000 100000"
        );
        assert_eq!(
            fs::read_to_string(path.join("cpu.max")).unwrap().trim(),
            "max 100000"
        );
    }
    service.restart();
    assert_eq!(service.status(&one)["state"], "Running");
    assert_eq!(service.status(&two)["state"], "Running");
    let refused = service.cli(&["queue", "update", "work/a", "--cpu-weight", "200"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("existing resource domain"));
    service.ok(&["group", "cancel", "work", "--recursive"]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn live_update_replays_a_kernel_write_whose_progress_was_not_saved() {
    let mut service = Service::start("live-update-replay");
    let id = service.ok(&[
        "submit",
        "--cpu-weight",
        "100",
        "--",
        "touch ready; sleep 30",
    ]);
    service.wait_file("ready");
    let preview: serde_json::Value = serde_json::from_str(&service.ok(&[
        "update",
        &id,
        "--cpu-weight",
        "200",
        "--memory-high",
        "32M",
        "--dry-run",
        "--json",
    ]))
    .unwrap();
    let plan = &preview["plan"];
    service.child.kill().unwrap();
    service.child.wait().unwrap();
    update_receipt(&service, plan, "Pending", 0);
    let step = &plan["steps"][0];
    fs::write(
        Path::new(plan["path"].as_str().unwrap()).join(step["file"].as_str().unwrap()),
        step["write"].as_str().unwrap(),
    )
    .unwrap();
    service.child = Service::spawn(&service.root, &service.cgroup);
    service.ready();
    completed_update(&service, plan["operation"].as_str().unwrap());
    let status = service.status(&id);
    assert_eq!(status["state"], "Running");
    assert_eq!(status["applied_resources"]["cpu.weight"], "200");
    assert_eq!(status["applied_resources"]["memory.high"], "33554432");
    service.ok(&["cancel", &id]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn live_update_commit_rejects_a_stale_preview_and_repeats_idempotently() {
    let service = Service::start("live-update-stale");
    let id = service.ok(&[
        "submit",
        "--cpu-weight",
        "100",
        "--",
        "touch ready; sleep 30",
    ]);
    service.wait_file("ready");
    let preview: serde_json::Value = serde_json::from_str(&service.ok(&[
        "update",
        &id,
        "--cpu-weight",
        "200",
        "--dry-run",
        "--json",
    ]))
    .unwrap();
    let plan = &preview["plan"];
    let commit = serde_json::json!({"ResourceUpdate": {"target": plan["target"], "patch": plan["patch"], "allow_oom": false, "expected": plan}});
    let response = resource_rpc(&service, commit.clone());
    assert!(response.get("ResourceUpdate").is_some(), "{response}");
    completed_update(&service, plan["operation"].as_str().unwrap());
    let repeated = resource_rpc(&service, commit);
    assert_eq!(repeated["ResourceUpdate"]["operation"]["phase"], "Applied");
    let stale: serde_json::Value = serde_json::from_str(&service.ok(&[
        "update",
        &id,
        "--cpu-weight",
        "300",
        "--dry-run",
        "--json",
    ]))
    .unwrap();
    service.ok(&["update", &id, "--cpu-weight", "400"]);
    let plan = &stale["plan"];
    let refused = resource_rpc(
        &service,
        serde_json::json!({"ResourceUpdate": {"target": plan["target"], "patch": plan["patch"], "allow_oom": false, "expected": plan}}),
    );
    assert!(refused.get("Error").is_some(), "{refused}");
    assert_eq!(
        service.status(&id)["applied_resources"]["cpu.weight"],
        "400"
    );
    service.ok(&["cancel", &id]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn live_device_io_update_replaces_the_configured_rates() {
    let service = Service::start("live-update-io");
    let device = filesystem_device(&service);
    let id = service.ok(&[
        "submit",
        "--io-max",
        &format!("{device},wbps=1M"),
        "--",
        "touch ready; sleep 30",
    ]);
    service.wait_file("ready");
    service.ok(&["update", &id, "--io-max", &format!("{device},rbps=2M")]);
    let status = service.status(&id);
    assert_eq!(
        status["applied_resources"]["io.max"],
        format!("{device} rbps=2097152 wbps=max riops=max wiops=max")
    );
    let path = Path::new(status["workload_cgroup"].as_str().unwrap());
    let actual = fs::read_to_string(path.join("io.max")).unwrap();
    assert!(actual.contains("rbps=2097152 wbps=max"), "{actual}");
    assert_eq!(status["state"], "Running");
    service.ok(&["cancel", &id]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn live_group_update_recovers_partially_published_metadata() {
    let mut service = Service::start("live-update-publish");
    service.ok(&["queue", "create", "work", "--cpu-weight", "100"]);
    let id = service.ok(&["submit", "-q", "work", "--", "touch ready; sleep 30"]);
    service.wait_file("ready");
    let preview: serde_json::Value = serde_json::from_str(&service.ok(&[
        "queue",
        "update",
        "work",
        "--cpu-weight",
        "200",
        "--dry-run",
        "--json",
    ]))
    .unwrap();
    let plan = &preview["plan"];
    service.child.kill().unwrap();
    service.child.wait().unwrap();
    for step in plan["steps"].as_array().unwrap() {
        fs::write(
            Path::new(plan["path"].as_str().unwrap()).join(step["file"].as_str().unwrap()),
            step["write"].as_str().unwrap(),
        )
        .unwrap();
    }
    update_receipt(
        &service,
        plan,
        "Verified",
        plan["steps"].as_array().unwrap().len(),
    );
    let graph_path = service.root.join("state/objects.json");
    let mut graph: serde_json::Value =
        serde_json::from_slice(&fs::read(&graph_path).unwrap()).unwrap();
    graph["nodes"][plan["object_id"].to_string()]["config"] = plan["config_after"].clone();
    fs::write(graph_path, serde_json::to_vec(&graph).unwrap()).unwrap();
    service.child = Service::spawn(&service.root, &service.cgroup);
    service.ready();
    completed_update(&service, plan["operation"].as_str().unwrap());
    let status = service.status(&id);
    assert_eq!(status["state"], "Running");
    assert_eq!(
        status["aggregate_domains"][0]["limits"]["cpu.weight"],
        "200"
    );
    service.ok(&["cancel", &id]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn live_update_keeps_suspended_jobs_suspended_and_rejects_requests() {
    let service = Service::start("live-update-suspended");
    let id = service.ok(&["submit", "--", "touch ready; sleep 30"]);
    service.wait_file("ready");
    service.ok(&["suspend", &id]);
    service.ok(&["update", &id, "--cpu-weight", "200"]);
    assert_eq!(service.status(&id)["state"], "Suspended");
    assert!(
        !service
            .cli(&["update", &id, "--cpu-request", "1"])
            .status
            .success()
    );
    service.ok(&["continue", &id]);
    assert_eq!(service.status(&id)["state"], "Running");
    service.ok(&["cancel", &id]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn live_update_conflict_keeps_control_available_and_can_be_abandoned_after_draining() {
    let mut service = Service::start("live-update-conflict");
    service.ok(&["queue", "create", "work", "--cpu-weight", "100"]);
    let id = service.ok(&["submit", "-q", "work", "--", "touch ready; sleep 30"]);
    service.wait_file("ready");
    let preview: serde_json::Value = serde_json::from_str(&service.ok(&[
        "queue",
        "update",
        "work",
        "--cpu-weight",
        "300",
        "--dry-run",
        "--json",
    ]))
    .unwrap();
    let plan = &preview["plan"];
    service.child.kill().unwrap();
    service.child.wait().unwrap();
    let path = Path::new(plan["path"].as_str().unwrap());
    fs::write(path.join("cpu.weight"), "300").unwrap();
    update_receipt(&service, plan, "Pending", 1);
    fs::write(path.join("cpu.weight"), "100").unwrap();
    service.child = Service::spawn(&service.root, &service.cgroup);
    service.ready();
    let operation = plan["operation"].as_str().unwrap();
    let status = service.cli(&["resource-update", operation, "--json"]);
    assert_eq!(status.status.code(), Some(75));
    assert!(!service.cli(&["submit", "--", "true"]).status.success());
    assert_eq!(service.status(&id)["state"], "Running");
    assert!(
        !service
            .cli(&["resource-update", operation, "--abandon"])
            .status
            .success()
    );
    service.ok(&["cancel", &id]);
    service.cli(&["wait", &id]);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = service.cli(&["resource-update", operation, "--abandon", "--json"]);
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout)
            && value["phase"] == "Ended"
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    service.ok(&["run", "--", "true"]);
}

#[test]
#[ignore = "bounded helper used by the live memory update test"]
fn live_memory_update_fixture() {
    if std::env::var_os("JOB_LIVE_MEMORY_FIXTURE").is_none() {
        return;
    }
    let mut memory = vec![0u8; 32 * 1024 * 1024];
    memory.fill(1);
    fs::write("memory-ready", b"ready").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !Path::new("charge-memory").exists() {
        std::hint::black_box(&memory);
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut additional = vec![0u8; 16 * 1024 * 1024];
    additional.fill(1);
    std::hint::black_box((&memory, &additional));
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn live_memory_reduction_below_usage_reports_the_kernel_oom_outcome() {
    let service = Service::start("live-update-oom");
    let executable = std::env::current_exe().unwrap();
    let id = service.ok(&[
        "submit",
        "--memory-swap-max",
        "0",
        "--",
        "env",
        "JOB_LIVE_MEMORY_FIXTURE=1",
        executable.to_str().unwrap(),
        "--exact",
        "live_memory_update_fixture",
        "--ignored",
        "--nocapture",
    ]);
    service.wait_file("memory-ready");
    service.ok(&["update", &id, "--memory-max", "8M", "--allow-oom"]);
    fs::write(service.root.join("charge-memory"), b"charge").unwrap();
    service.cli(&["wait", &id]);
    let status = service.status(&id);
    assert!(
        status["result"]["oom_kill"].as_u64().unwrap() > 0,
        "{status}"
    );
    assert_eq!(status["applied_resources"]["memory.max"], "8388608");
    service.ok(&["host"]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn admission_priority_and_live_kernel_weights_remain_independent() {
    let service = Service::start("admission-weights");
    service.ok(&["queue", "create", "work"]);
    service.ok(&["queue", "pause", "work"]);
    let id = service.ok(&[
        "submit",
        "-q",
        "work",
        "--priority",
        "0",
        "--cpu-weight",
        "200",
        "--",
        "touch ready; sleep 30",
    ]);
    service.ok(&["reprioritize", &id, "--priority", "200"]);
    service.ok(&["queue", "resume", "work"]);
    service.wait_file("ready");
    let status = service.status(&id);
    let path = Path::new(status["workload_cgroup"].as_str().unwrap());
    assert_eq!(
        fs::read_to_string(path.join("cpu.weight")).unwrap().trim(),
        "200"
    );
    let io_before = fs::read_to_string(path.join("io.weight")).ok();
    service.ok(&["update", &id, "--cpu-weight", "300"]);
    assert_eq!(
        fs::read_to_string(path.join("cpu.weight")).unwrap().trim(),
        "300"
    );
    assert_eq!(fs::read_to_string(path.join("io.weight")).ok(), io_before);
    let explanation: serde_json::Value =
        serde_json::from_str(&service.ok(&["explain", &id, "--json"])).unwrap();
    assert_eq!(explanation["priority"], 200);
    assert!(
        !service
            .cli(&["reprioritize", &id, "--priority", "300"])
            .status
            .success()
    );
    service.ok(&["cancel", &id]);
    service.cli(&["wait", &id]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn fair_share_creates_no_kernel_sharing_domains_or_weights() {
    let service = Service::start("fair-share-kernel");
    service.ok(&[
        "group",
        "create",
        "team",
        "--fair-share",
        "cpu-request-time",
    ]);
    service.ok(&["queue", "create", "team/a", "--share-weight", "2"]);
    service.ok(&["queue", "create", "team/b", "--share-weight", "1"]);
    let a = service.ok(&[
        "submit",
        "-q",
        "team/a",
        "--cpu-request",
        "0.1",
        "--",
        "touch a; sleep 30",
    ]);
    let b = service.ok(&[
        "submit",
        "-q",
        "team/b",
        "--cpu-request",
        "0.1",
        "--",
        "touch b; sleep 30",
    ]);
    service.wait_file("a");
    service.wait_file("b");
    for id in [&a, &b] {
        let status = service.status(id);
        assert!(status["aggregate_domains"].as_array().unwrap().is_empty());
        let path = Path::new(status["workload_cgroup"].as_str().unwrap());
        assert_eq!(path.parent(), Some(service.cgroup.join("jobs").as_path()));
        assert_eq!(
            fs::read_to_string(path.join("cpu.weight")).unwrap().trim(),
            "100"
        );
        assert!(status["spec"]["declared"]["cpu_weight"].is_null());
        assert!(status["spec"]["declared"]["io_weight"].is_null());
    }
    service.ok(&["queue", "set", "team/a", "--share-weight", "3"]);
    service.ok(&["suspend", &a]);
    service.ok(&["continue", &a]);
    service.ok(&["group", "cancel", "team", "--recursive"]);
    service.cli(&["wait", &a]);
    service.cli(&["wait", &b]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn pressure_reads_the_running_attempts_kernel_scope_and_retires_it_on_exit() {
    use std::os::unix::fs::MetadataExt;
    let service = Service::start("pressure-snapshot");
    let id = service.ok(&["submit", "--", "touch ready; sleep 30"]);
    service.wait_file("ready");
    let before = service.status(&id);
    let path = Path::new(before["workload_cgroup"].as_str().unwrap());
    let metadata = fs::metadata(path).unwrap();
    let snapshot: serde_json::Value =
        serde_json::from_str(&service.ok(&["pressure", &id, "--json"])).unwrap();
    assert_eq!(snapshot["cgroup"]["device"], metadata.dev());
    assert_eq!(snapshot["cgroup"]["inode"], metadata.ino());
    assert_eq!(snapshot["attempt"], 1);
    for observation in snapshot["observations"].as_array().unwrap() {
        let resource = observation["resource"].as_str().unwrap();
        assert_eq!(
            observation["path"],
            path.join(format!("{resource}.pressure")).to_str().unwrap()
        );
        let raw = fs::read_to_string(path.join(format!("{resource}.pressure"))).unwrap();
        for metric in ["some", "full"] {
            assert_eq!(observation[metric]["availability"], "available");
            let total: u64 = raw
                .lines()
                .find(|line| line.starts_with(metric))
                .unwrap()
                .split_whitespace()
                .find_map(|field| field.strip_prefix("total="))
                .unwrap()
                .parse()
                .unwrap();
            assert!(observation[metric]["total_us"].as_u64().unwrap() <= total);
        }
    }
    let after = service.status(&id);
    assert_eq!(before["applied_resources"], after["applied_resources"]);
    assert_eq!(before["aggregate_domains"], after["aggregate_domains"]);
    service.ok(&["cancel", &id]);
    service.cli(&["wait", &id]);
    let ended: serde_json::Value =
        serde_json::from_str(&service.ok(&["pressure", &id, "--json"])).unwrap();
    assert!(ended["cgroup"].is_null());
    assert_eq!(
        ended["observations"][0]["some"]["availability"],
        "unavailable"
    );
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn pressure_admission_holds_only_its_subtree_and_recovers_gradually() {
    let mut service = Service::start("pressure-admission");
    let rules = service.root.join("pressure-rules.json");
    fs::write(&rules, serde_json::to_vec(&serde_json::json!([{"id":"cpu-local","resource":"cpu","metric":"some","window":"avg10","high_bp":1,"low_bp":0,"sustain_ms":1000,"minimum_hold_ms":2000,"recovery_ms":1000,"step_ms":1000,"required":true}])).unwrap()).unwrap();
    service.ok(&[
        "group",
        "create",
        "loaded",
        "--pressure",
        rules.to_str().unwrap(),
    ]);
    service.ok(&["queue", "create", "loaded/a"]);
    let remote = service.cli(&[
        "submit",
        "-q",
        "loaded/a",
        "--on",
        "pressure-test.invalid",
        "--",
        "true",
    ]);
    assert!(!remote.status.success());
    assert!(
        String::from_utf8_lossy(&remote.stderr)
            .contains("remote workload PSI is unavailable locally")
    );
    let busy = service.ok(&[
        "submit",
        "-q",
        "loaded/a",
        "--cpu-limit",
        "0.05",
        "--",
        "touch busy; timeout 25s sh -c 'while :; do :; done'",
    ]);
    service.wait_file("busy");
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        let report: serde_json::Value =
            serde_json::from_str(&service.ok(&["pressure", "status"])).unwrap();
        if report["rules"]
            .as_object()
            .unwrap()
            .values()
            .any(|v| v["phase"] == "holding")
        {
            assert_eq!(report["observation_mode"], "notifications_and_sampling");
            let monitors = report["notifications"]["monitors"].as_array().unwrap();
            assert_eq!(monitors.len(), 1);
            assert_eq!(
                monitors[0]["source"]["job_attempt"][0],
                busy.parse::<u64>().unwrap()
            );
            assert_eq!(monitors[0]["threshold_us"], 200);
            break;
        }
        assert!(Instant::now() < until, "{report}");
        std::thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(service.status(&busy)["state"], "Running");
    let notified = Instant::now() + Duration::from_secs(8);
    loop {
        let report: serde_json::Value =
            serde_json::from_str(&service.ok(&["pressure", "status"])).unwrap();
        if report["notifications"]["notifications"].as_u64().unwrap() > 0 {
            break;
        }
        assert!(Instant::now() < notified, "{report}");
        std::thread::sleep(Duration::from_millis(30));
    }
    let a = service.ok(&[
        "submit",
        "-q",
        "loaded/a",
        "--priority",
        "1000",
        "--",
        "touch recovered-a; sleep 6",
    ]);
    let b = service.ok(&[
        "submit",
        "-q",
        "loaded/a",
        "--",
        "touch recovered-b; sleep 6",
    ]);
    service.ok(&["run", "--", "true"]);
    assert!(!service.root.join("recovered-a").exists());
    service.restart();
    assert_eq!(service.status(&a)["state"], "Queued");
    service.ok(&["cancel", &busy]);
    service.cli(&["wait", &busy]);
    service.wait_file("recovered-a");
    let report: serde_json::Value =
        serde_json::from_str(&service.ok(&["pressure", "status"])).unwrap();
    let view = report["rules"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(view["phase"], "recovering");
    assert_eq!(view["temporary_max_running"], 1);
    assert!(!service.root.join("recovered-b").exists());
    service.wait_file("recovered-b");
    let events: serde_json::Value =
        serde_json::from_str(&service.ok(&["pressure", "events"])).unwrap();
    assert!(
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["view"]["phase"] == "holding"
                && e["view"]["signal"]["availability"] == "available")
    );
    assert!(
        service.status(&a)["aggregate_domains"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    service.ok(&["cancel", &a]);
    service.ok(&["cancel", &b]);
    service.cli(&["wait", &a]);
    service.cli(&["wait", &b]);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn pressure_notification_permission_failure_keeps_sampling_and_releases_descriptors() {
    use std::os::unix::fs::PermissionsExt;
    let service = Service::start("pressure-notification-fallback");
    service.ok(&["queue", "create", "observed"]);
    let id = service.ok(&["submit", "-q", "observed", "--", "touch ready; sleep 20"]);
    service.wait_file("ready");
    let record = service.status(&id);
    let leaf = Path::new(record["workload_cgroup"].as_str().unwrap());
    let pressure = leaf.join("cpu.pressure");
    fs::set_permissions(&pressure, fs::Permissions::from_mode(0o444)).unwrap();
    let rules = service.root.join("rules.json");
    fs::write(&rules, br#"[{"id":"cpu","resource":"cpu","metric":"some","window":"avg10","high_bp":10000,"low_bp":0,"sustain_ms":1000,"minimum_hold_ms":1000,"recovery_ms":1000,"step_ms":1000,"required":true}]"#).unwrap();
    service.ok(&[
        "queue",
        "set",
        "observed",
        "--pressure",
        rules.to_str().unwrap(),
    ]);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let report: serde_json::Value =
            serde_json::from_str(&service.ok(&["pressure", "status"])).unwrap();
        if let Some(monitor) = report["notifications"]["monitors"]
            .as_array()
            .unwrap()
            .first()
        {
            assert_eq!(report["observation_mode"], "sampling");
            assert_eq!(monitor["registered"], false);
            assert!(
                monitor["error"]
                    .as_str()
                    .unwrap()
                    .contains("Permission denied")
            );
            assert!(monitor["retry_boot_ms"].as_u64().is_some());
            assert_eq!(
                report["rules"]
                    .as_object()
                    .unwrap()
                    .values()
                    .next()
                    .unwrap()["signal"]["availability"],
                "available"
            );
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(20));
    }
    service.ok(&["run", "-q", "observed", "--", "true"]);
    fs::set_permissions(&pressure, fs::Permissions::from_mode(0o644)).unwrap();
    service.ok(&["queue", "unset", "observed", "pressure"]);
    service.ok(&[
        "queue",
        "set",
        "observed",
        "--pressure",
        rules.to_str().unwrap(),
    ]);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let report: serde_json::Value =
            serde_json::from_str(&service.ok(&["pressure", "status"])).unwrap();
        if report["notifications"]["monitors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["registered"] == true)
        {
            break;
        }
        assert!(Instant::now() < until, "{report}");
        std::thread::sleep(Duration::from_millis(20));
    }
    fs::write(leaf.join("cgroup.pressure"), "0").unwrap();
    let disabled: serde_json::Value =
        serde_json::from_str(&service.ok(&["pressure", &id, "--json"])).unwrap();
    assert_eq!(
        disabled["observations"][0]["some"]["availability"],
        "unavailable"
    );
    assert!(
        disabled["observations"][0]["some"]["reason"]
            .as_str()
            .unwrap()
            .contains("disabled"),
        "{disabled}"
    );
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let report: serde_json::Value =
            serde_json::from_str(&service.ok(&["pressure", "status"])).unwrap();
        if report["rules"]
            .as_object()
            .unwrap()
            .values()
            .any(|r| r["phase"] == "holding")
        {
            assert!(
                report["notifications"]["monitors"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            break;
        }
        assert!(Instant::now() < until, "{report}");
        std::thread::sleep(Duration::from_millis(20));
    }
    fs::write(leaf.join("cgroup.pressure"), "1").unwrap();
    service.ok(&["cancel", &id]);
    service.cli(&["wait", &id]);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let report: serde_json::Value =
            serde_json::from_str(&service.ok(&["pressure", "status"])).unwrap();
        if report["notifications"]["monitors"]
            .as_array()
            .unwrap()
            .is_empty()
        {
            break;
        }
        assert!(Instant::now() < until, "{report}");
        std::thread::sleep(Duration::from_millis(20));
    }
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let retained = fs::read_dir(format!("/proc/{}/fd", service.child.id()))
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| fs::read_link(entry.path()).ok())
            .any(|target| target.starts_with(leaf));
        if !retained {
            break;
        }
        assert!(
            Instant::now() < until,
            "retired monitor descriptor remains open"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn pressure_notification_capacity_is_bounded_without_blocking_admission() {
    let service = Service::start("pressure-notification-capacity");
    let rules: Vec<_> = (0..32).map(|i| serde_json::json!({"id":format!("cpu-{i}"),"resource":"cpu","metric":"some","window":"avg10","high_bp":10000-i,"low_bp":0,"sustain_ms":1000,"minimum_hold_ms":1000,"recovery_ms":1000,"step_ms":1000,"required":true})).collect();
    let file = service.root.join("rules.json");
    fs::write(&file, serde_json::to_vec(&rules).unwrap()).unwrap();
    service.ok(&[
        "queue",
        "create",
        "many",
        "--pressure",
        file.to_str().unwrap(),
    ]);
    let mut ids = Vec::new();
    for i in 0..10 {
        ids.push(service.ok(&[
            "submit",
            "-q",
            "many",
            "--",
            &format!("touch ready-{i}; sleep 20"),
        ]));
        service.wait_file(&format!("ready-{i}"));
    }
    let until = Instant::now() + Duration::from_secs(8);
    loop {
        let report: serde_json::Value =
            serde_json::from_str(&service.ok(&["pressure", "status"])).unwrap();
        if report["notifications"]["omitted_sources"] == 64 {
            assert_eq!(report["notifications"]["capacity"], 256);
            assert_eq!(
                report["notifications"]["monitors"]
                    .as_array()
                    .unwrap()
                    .len(),
                256
            );
            assert_eq!(report["notifications"]["omitted_sources"], 64);
            break;
        }
        assert!(Instant::now() < until, "{report}");
        std::thread::sleep(Duration::from_millis(20));
    }
    service.ok(&["queue", "cancel", "many", "--recursive"]);
    for id in ids {
        service.cli(&["wait", &id]);
    }
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn process_placement_and_rlimits_coexist_with_cgroup_controls() {
    let mut service = Service::start("process-placement");
    let host: serde_json::Value = serde_json::from_str(&service.ok(&["host", "--json"])).unwrap();
    let cpu = host["process_controls"]["allowed_cpus"]
        .as_str()
        .unwrap()
        .split([',', '-'])
        .next()
        .unwrap();
    let id = service.ok(&[
        "submit",
        "--cpu-affinity",
        cpu,
        "--rlimit",
        "nofile=64:96",
        "--cpu-limit",
        "0.5",
        "--cpu-weight",
        "200",
        "--",
        "sh",
        "-c",
        "echo $$ > workload-pid; ulimit -n > actual-nofile; touch ready; sleep 15",
    ]);
    service.wait_file("ready");
    let pid = fs::read_to_string(service.root.join("workload-pid")).unwrap();
    let actual = fs::read_to_string(format!("/proc/{}/status", pid.trim())).unwrap();
    let affinity = actual
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))
        .unwrap()
        .trim();
    assert_eq!(affinity, cpu);
    assert_eq!(
        fs::read_to_string(service.root.join("actual-nofile"))
            .unwrap()
            .trim(),
        "64"
    );
    let record = service.status(&id);
    let cgroup = Path::new(record["workload_cgroup"].as_str().unwrap());
    assert!(
        fs::read_to_string(cgroup.join("cgroup.procs"))
            .unwrap()
            .lines()
            .any(|p| p == pid.trim())
    );
    assert_eq!(
        fs::read_to_string(cgroup.join("cpu.max")).unwrap().trim(),
        "50000 100000"
    );
    assert_eq!(
        fs::read_to_string(cgroup.join("cpu.weight"))
            .unwrap()
            .trim(),
        "200"
    );
    assert!(
        !service
            .cli(&["update", &id, "--cpu-affinity", cpu])
            .status
            .success()
    );
    service.ok(&["cancel", &id]);
    service.cli(&["wait", &id]);
    let finished = service.status(&id);
    assert_eq!(finished["result"]["process_controls"]["cpu_affinity"], cpu);
    assert_eq!(
        finished["result"]["process_controls"]["rlimits"]["nofile"]["hard"],
        96
    );
    service.restart();
    assert_eq!(
        service.status(&id)["result"]["process_controls"],
        finished["result"]["process_controls"]
    );
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn cli2_pids_max_is_written_read_back_and_kept_for_the_earlier_option() {
    let service = Service::start("pids");
    let host: serde_json::Value = serde_json::from_str(&service.ok(&["host", "--json"])).unwrap();
    assert_eq!(host["resource_controls"]["pids.max"], true);
    assert_eq!(host["filesystem_quota"]["supported"], false);
    let read = |id: &str| {
        fs::read_to_string(service.cgroup.join("jobs").join(id).join("pids.max"))
            .unwrap()
            .trim()
            .to_owned()
    };
    let explicit = service.ok(&[
        "submit",
        "--pids-max",
        "64",
        "--",
        "touch explicit; sleep 3",
    ]);
    service.wait_file("explicit");
    let job = service.status(&explicit);
    assert_eq!(read(&explicit), "64");
    assert_eq!(job["applied_resources"]["pids.max"], "64");
    assert_eq!(job["reservation"]["vector"]["pids"], 0);
    assert_eq!(job["resource_sources"]["pids_max"], "Job");

    let earlier = service.ok(&["submit", "--pids", "32", "--", "touch earlier; sleep 3"]);
    service.wait_file("earlier");
    let job = service.status(&earlier);
    assert_eq!(read(&earlier), "32");
    assert_eq!(job["reservation"]["vector"]["pids"], 32);
    assert_eq!(job["spec"]["declared"]["pids"], 32);
    assert_eq!(job["spec"]["declared"]["pids_max"], 32);
    assert_eq!(job["resource_sources"]["pids_max"], "Compatibility");

    let open = service.ok(&[
        "submit",
        "--pids-max",
        "unlimited",
        "--",
        "touch open; sleep 3",
    ]);
    service.wait_file("open");
    assert_eq!(read(&open), "max");

    let updated = service.cli(&["update", &explicit, "--pids-max", "128"]);
    assert!(
        updated.status.success(),
        "{}",
        String::from_utf8_lossy(&updated.stderr)
    );
    assert_eq!(read(&explicit), "128");

    service.ok(&["group", "create", "pool", "--pids-max", "100"]);
    service.ok(&["queue", "create", "pool/q", "--job-pids-max", "10"]);
    let inherited = service.ok(&["submit", "-q", "pool/q", "--", "touch inherited; sleep 3"]);
    service.wait_file("inherited");
    let job = service.status(&inherited);
    let leaf = PathBuf::from(job["workload_cgroup"].as_str().unwrap());
    assert_eq!(
        fs::read_to_string(leaf.join("pids.max")).unwrap().trim(),
        "10"
    );
    assert_eq!(
        fs::read_to_string(leaf.parent().unwrap().join("pids.max"))
            .unwrap()
            .trim(),
        "100"
    );
    assert_eq!(
        job["resource_sources"]["pids_max"]["Object"]["path"],
        "pool/q"
    );
    for id in [explicit, earlier, open, inherited] {
        service.ok(&["cancel", &id]);
        service.cli(&["wait", &id, "--timeout", "3s"]);
    }
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn signals_under_process_churn_stay_inside_the_cgroup_of_the_job() {
    let service = Service::start("churn");
    let aside = service.root.join("bystander");
    fs::create_dir(&aside).unwrap();
    let mut bystander = Command::new("sh")
        .args([
            "-c",
            "echo ready > ready; i=0; while [ $i -lt 40000 ] && [ ! -e stop ]; do /usr/bin/true; rc=$?; [ $rc -eq 0 ] || echo child-$rc >> hits; i=$((i+1)); echo $i > progress; done; echo $i > done",
        ])
        .current_dir(&aside)
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let id = service.ok(&[
        "submit",
        "--time",
        "90s",
        "--",
        "n=0; trap 'n=$((n+1)); echo $n > trapped' USR1; echo ready > ready; i=0; while [ $i -lt 20000 ]; do /usr/bin/true & /usr/bin/true & wait; i=$((i+1)); echo $i > progress; done",
    ]);
    service.wait_file("ready");
    service.wait_file("bystander/ready");
    let number = |name: &str| -> usize {
        for _ in 0..200 {
            if let Some(number) = fs::read_to_string(service.root.join(name))
                .ok()
                .and_then(|text| text.trim().parse().ok())
            {
                return number;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("{name} holds no number");
    };
    let leaf = PathBuf::from(service.status(&id)["workload_cgroup"].as_str().unwrap());
    let rounds = 50;
    let mut delivered = 0;
    for round in 0..rounds {
        let response = resource_rpc(
            &service,
            serde_json::json!({"Signal": {"id": id.parse::<u64>().unwrap(), "signal": libc::SIGUSR1}}),
        );
        let count = response["Signalled"]["delivered"]
            .as_u64()
            .unwrap_or_else(|| panic!("{response}"));
        assert!((1..=8).contains(&count), "round {round}: {count}");
        delivered += count;
        std::thread::sleep(Duration::from_millis(20));
    }
    let children = number("progress") * 2;
    let trapped = number("trapped");
    let others = number("bystander/progress");
    assert!(children >= 300, "{children}");
    assert!(others >= 100, "{others}");
    assert!(
        (rounds * 3 / 4..=rounds).contains(&trapped),
        "{trapped} of {rounds}"
    );
    assert!(bystander.try_wait().unwrap().is_none());
    assert!(
        fs::read_to_string(leaf.join("cgroup.events"))
            .unwrap()
            .contains("populated 1")
    );
    service.ok(&["cancel", &id]);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let populated = fs::read_to_string(leaf.join("cgroup.events"))
            .is_ok_and(|events| events.contains("populated 1"));
        if service.status(&id)["state"] == "Cancelled" && !populated {
            break;
        }
        assert!(Instant::now() < deadline, "the tree outlived the cancel");
        std::thread::sleep(Duration::from_millis(20));
    }
    let after = fs::read(service.root.join("progress")).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(fs::read(service.root.join("progress")).unwrap(), after);
    assert!(bystander.try_wait().unwrap().is_none());
    fs::write(aside.join("stop"), b"").unwrap();
    assert!(bystander.wait().unwrap().success());
    assert!(aside.join("done").exists());
    assert!(!aside.join("hits").exists());
    println!(
        "cgroup: {children} job children and {others} bystander children during {rounds} signals, payload counted {trapped}, service reported {delivered} deliveries"
    );
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn review_pressure_hold_activates_while_jobs_start_every_half_second() {
    let service = Service::start("pressure-stream");
    let rules = service.root.join("pressure-rules.json");
    fs::write(&rules, serde_json::to_vec(&serde_json::json!([{"id":"cpu-local","resource":"cpu","metric":"some","window":"avg10","high_bp":1,"low_bp":0,"sustain_ms":1000,"minimum_hold_ms":2000,"recovery_ms":1000,"step_ms":1000,"required":false}])).unwrap()).unwrap();
    service.ok(&[
        "group",
        "create",
        "loaded",
        "--pressure",
        rules.to_str().unwrap(),
    ]);
    service.ok(&["queue", "create", "loaded/a"]);
    service.ok(&[
        "submit",
        "-q",
        "loaded/a",
        "--cpu-limit",
        "0.05",
        "--",
        "touch busy; timeout 30s sh -c 'while :; do :; done'",
    ]);
    service.wait_file("busy");
    let begun = Instant::now();
    let mut started = 0u32;
    let mut held = false;
    while begun.elapsed() < Duration::from_secs(14) {
        if begun.elapsed() >= Duration::from_millis(500) * started {
            service.ok(&["submit", "-q", "loaded/a", "--", "sleep", "4"]);
            started += 1;
        }
        let report: serde_json::Value =
            serde_json::from_str(&service.ok(&["pressure", "status"])).unwrap();
        if report["rules"]
            .as_object()
            .unwrap()
            .values()
            .any(|view| view["phase"] == "holding")
        {
            held = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    service.ok(&["queue", "cancel", "loaded/a", "--recursive"]);
    assert!(
        held,
        "no hold after {started} Jobs started every 500 ms under sustained pressure"
    );
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn idset_suspend_and_continue_of_a_range_freeze_and_thaw_each_running_job() {
    let service = Service::start("idset-freeze");
    let script = "while :; do echo tick >> ticks; sleep .02; done";
    let first = service.ok(&["submit", "--time", "20s", "--", script]);
    let second = service.ok(&["submit", "--time", "20s", "--", script]);
    let held = service.ok(&["create", "--", "true"]);
    service.wait_file("ticks");
    let deadline = Instant::now() + Duration::from_secs(5);
    while service.status(&second)["state"] != "Running"
        || service.status(&first)["state"] != "Running"
    {
        assert!(Instant::now() < deadline, "the Jobs did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    let set = format!("{first}-{held}");
    let output = service.cli(&["suspend", &set, "9999"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!(
            "{first}: suspended\n{second}: suspended\n{held}: refused: only running or suspended Jobs can be controlled\n9999: no such Job\n"
        )
    );
    assert!(service.events(&first).contains("frozen 1"));
    assert!(service.events(&second).contains("frozen 1"));
    assert_eq!(service.status(&first)["state"], "Suspended");
    let running = format!("{first},{second}");
    let report: serde_json::Value =
        serde_json::from_str(&service.ok(&["continue", &running, "--json"])).unwrap();
    assert_eq!(report["kind"], "job_set");
    assert_eq!(report["action"], "continue");
    assert_eq!(report["results"][0]["result"], "continued");
    assert_eq!(report["results"][1]["state"], "running");
    assert!(service.events(&first).contains("frozen 0"));
    assert!(service.events(&second).contains("frozen 0"));
    assert_eq!(
        service.ok(&["signal", "-s", "KILL", &running]),
        format!("{first}: signalled\n{second}: signalled")
    );
    let waited = service.cli(&["wait", &running, "--timeout", "10s"]);
    assert_ne!(waited.status.code(), Some(0));
    assert_ne!(waited.status.code(), Some(75));
}

fn pidns_on_host(marker: &str) -> bool {
    fs::read_dir("/proc").unwrap().flatten().any(|entry| {
        fs::read(entry.path().join("cmdline")).is_ok_and(|bytes| {
            String::from_utf8_lossy(&bytes)
                .split('\0')
                .any(|part| part == marker)
        })
    })
}

fn pidns_removed(service: &Service, id: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while service.cgroup.join("jobs").join(id).exists() {
        assert!(Instant::now() < deadline, "the cgroup of {id} stayed");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn pidns_gone(marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while pidns_on_host(marker) {
        assert!(Instant::now() < deadline, "{marker} stayed");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn pidns_suspend_and_continue_hold_the_init_and_the_command_in_the_cgroup() {
    let service = Service::start("pidns-freeze");
    let id = service.ok(&[
        "submit",
        "--namespaces",
        "user,mount,pid",
        "--time",
        "20s",
        "--",
        "trap 'echo graceful $$; exit 0' TERM; while :; do echo tick >> ticks; sleep .02; done",
    ]);
    service.wait_file("ticks");
    let members =
        fs::read_to_string(service.cgroup.join("jobs").join(&id).join("cgroup.procs")).unwrap();
    let names: Vec<String> = members
        .split_whitespace()
        .filter_map(|pid| fs::read_to_string(format!("/proc/{pid}/comm")).ok())
        .map(|name| name.trim().to_owned())
        .collect();
    assert!(names.contains(&"job-init".to_owned()), "{names:?}");
    assert!(names.len() >= 2, "{names:?}");
    service.ok(&["suspend", &id]);
    assert!(service.events(&id).contains("frozen 1"));
    let bytes = fs::read(service.root.join("ticks")).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(fs::read(service.root.join("ticks")).unwrap(), bytes);
    assert_eq!(service.status(&id)["state"], "Suspended");
    service.ok(&["continue", &id]);
    let deadline = Instant::now() + Duration::from_secs(3);
    while fs::read(service.root.join("ticks")).unwrap() == bytes {
        assert!(Instant::now() < deadline, "the command did not continue");
        std::thread::sleep(Duration::from_millis(20));
    }
    service.ok(&["suspend", &id]);
    service.ok(&["cancel", &id]);
    let result = service.cli(&["wait", &id, "--timeout", "5s"]);
    assert!(!result.status.success());
    assert_ne!(result.status.code(), Some(75));
    assert!(service.ok(&["log", &id, "full"]).contains("graceful 2"));
    pidns_removed(&service, &id);
}

#[test]
#[ignore = "requires a writable delegated cgroup; run through job"]
fn pidns_leftovers_exit_status_and_cancel_are_recorded_from_the_cgroup() {
    let service = Service::start("pidns-cgroup");
    let plain = service.ok(&["submit", "--", "sleep 23.4320 & exit 7"]);
    let inside = service.ok(&[
        "submit",
        "--namespaces",
        "user,mount,pid",
        "--",
        "sleep 23.4321 & exit 7",
    ]);
    for (id, marker) in [(&plain, "23.4320"), (&inside, "23.4321")] {
        let result = service.cli(&["wait", id, "--timeout", "10s"]);
        assert_eq!(result.status.code(), Some(7));
        let record = service.status(id);
        assert_eq!(record["state"], "Failed", "{record}");
        assert_eq!(record["result"]["exit_code"], 7);
        assert_eq!(record["result"]["leftover_processes"], 1, "{record}");
        assert!(
            record["result"]["leftover_names"][0]
                .as_str()
                .unwrap()
                .starts_with(&format!("sleep {marker} (pid ")),
            "{record}"
        );
        pidns_gone(marker);
        pidns_removed(&service, id);
    }
    let peak = |id: &str| {
        service.status(id)["result"]["usage"]["peak_pids"]
            .as_u64()
            .unwrap()
    };
    assert!(peak(&inside) > peak(&plain));
    assert!(peak(&inside) <= peak(&plain) + 2);
    let killed = service.ok(&[
        "submit",
        "--namespaces",
        "user,mount,pid",
        "--",
        "sh",
        "-c",
        "kill -SEGV $$",
    ]);
    service.cli(&["wait", &killed, "--timeout", "10s"]);
    let record = service.status(&killed);
    assert_eq!(record["result"]["signal"], libc::SIGSEGV, "{record}");
    let stubborn = service.ok(&[
        "submit",
        "--namespaces",
        "user,mount,pid",
        "--time",
        "40s",
        "--",
        "trap '' TERM; sleep 22.4321 & : > ready; wait",
    ]);
    service.wait_file("ready");
    let begun = Instant::now();
    service.ok(&["cancel", &stubborn]);
    service.cli(&["wait", &stubborn, "--timeout", "20s"]);
    let took = begun.elapsed();
    let record = service.status(&stubborn);
    assert_eq!(record["state"], "Cancelled", "{record}");
    assert_eq!(record["result"]["signal"], libc::SIGKILL, "{record}");
    assert!(took >= Duration::from_secs(9), "{took:?}");
    assert!(took < Duration::from_secs(16), "{took:?}");
    pidns_gone("22.4321");
    pidns_removed(&service, &stubborn);
}
