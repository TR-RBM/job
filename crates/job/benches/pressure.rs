use std::fs;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[allow(dead_code)]
#[path = "../tests/support/mod.rs"]
mod support;
use support::Service;

fn cpu_time() -> Duration {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut time) },
        0
    );
    Duration::new(
        time.tv_sec.try_into().unwrap(),
        time.tv_nsec.try_into().unwrap(),
    )
}

fn burn(milliseconds: u64, marker: &Path) {
    assert!(milliseconds <= 4000);
    fs::write(marker, b"started").unwrap();
    let start = cpu_time();
    let wall = Instant::now();
    let mut value = 1_u64;
    while cpu_time() - start < Duration::from_millis(milliseconds) {
        assert!(wall.elapsed() < Duration::from_secs(45));
        for _ in 0..10000 {
            value = std::hint::black_box(value.wrapping_mul(1664525).wrapping_add(1013904223));
        }
    }
    std::hint::black_box(value);
}

fn memory(mib: usize, milliseconds: u64, marker: &Path) {
    assert!(mib <= 24 && milliseconds <= 8000);
    fs::write(marker, b"started").unwrap();
    let wall = Instant::now();
    let page: usize = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }
        .try_into()
        .unwrap();
    assert!(page > 0);
    let mut bytes = vec![0_u8; mib * 1024 * 1024];
    for byte in bytes.iter_mut().step_by(page) {
        unsafe { std::ptr::write_volatile(byte, 1) };
        assert!(wall.elapsed() < Duration::from_secs(45));
    }
    let resident = Instant::now();
    while resident.elapsed() < Duration::from_millis(milliseconds) {
        for byte in bytes.iter_mut().step_by(page) {
            unsafe { std::ptr::write_volatile(byte, 1) };
        }
        assert!(wall.elapsed() < Duration::from_secs(45));
        std::thread::sleep(Duration::from_millis(20));
    }
    std::hint::black_box(bytes);
}

fn workload(resource: &str, kind: &str, marker: &Path) -> Vec<String> {
    let mut args = if resource == "memory" && matches!(kind, "seed" | "batch" | "external") {
        vec![
            "--memory-mib".to_owned(),
            if kind == "seed" { "24" } else { "16" }.to_owned(),
            "--hold-ms".to_owned(),
            if kind == "batch" { "2000" } else { "8000" }.to_owned(),
        ]
    } else {
        vec![
            "--burn-ms".to_owned(),
            match kind {
                "seed" => "4000",
                "batch" => "100",
                "external" => "3000",
                _ => "10",
            }
            .to_owned(),
        ]
    };
    args.push(marker.to_str().unwrap().to_owned());
    args
}

fn counters(path: &Path) -> Value {
    let values: serde_json::Map<String, Value> = fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| {
            let (key, value) = line.split_once(' ').unwrap();
            (key.to_owned(), json!(value.parse::<u64>().unwrap()))
        })
        .collect();
    Value::Object(values)
}

struct Peer(Child);

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn external(service: &Service, seed: &str, resource: &str) -> Peer {
    let status = service.status(seed);
    let leaf = Path::new(status["workload_cgroup"].as_str().unwrap());
    let group = leaf.parent().unwrap().join("benchmark-external");
    fs::create_dir(&group).unwrap();
    let procs =
        std::ffi::CString::new(group.join("cgroup.procs").as_os_str().as_encoded_bytes()).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(workload(
            resource,
            "external",
            &service.root.join("external-start"),
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(move || {
            let fd = libc::open(procs.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let result = libc::write(fd, b"0".as_ptr().cast(), 1);
            let error = std::io::Error::last_os_error();
            libc::close(fd);
            if result != 1 {
                return Err(error);
            }
            Ok(())
        });
    }
    Peer(command.spawn().unwrap())
}

fn submit(service: &Service, kind: &str, index: usize, resource: &str) -> (String, String) {
    let marker = format!("{kind}-{index}");
    let binary = std::env::current_exe().unwrap();
    let mut args = vec![
        "submit",
        "-q",
        if kind == "outside" {
            "outside"
        } else if kind == "interactive" {
            "pool/interactive"
        } else {
            "pool/batch"
        },
        "--priority",
        if kind == "interactive" { "500" } else { "0" },
        "--time",
        "50s",
        "--",
        binary.to_str().unwrap(),
    ];
    let work = workload(resource, kind, Path::new(&marker));
    args.extend(work.iter().map(String::as_str));
    let id = service.ok(&args);
    (id, kind.to_owned())
}

fn distribution(mut values: Vec<u64>) -> Value {
    assert!(!values.is_empty());
    values.sort_unstable();
    let quantile = |percent: usize| values[(values.len() * percent).div_ceil(100) - 1];
    json!({"count":values.len(), "mean_ms":values.iter().sum::<u64>() as f64 / values.len() as f64,
        "p50_ms":quantile(50), "p95_ms":quantile(95), "max_ms":values.last().unwrap()})
}

fn run(mode: &str, repetition: usize, with_external: bool, resource: &str) -> Value {
    let service = Service::start(&format!("bench-{mode}-{repetition}-{with_external}"));
    service.ok(&[
        "group",
        "create",
        "pool",
        "--cpu-limit",
        "0.5",
        "--max-running",
        "4",
    ]);
    if resource == "memory" {
        service.ok(&[
            "group",
            "set",
            "pool",
            "--memory-high",
            "48M",
            "--memory-max",
            "128M",
        ]);
    }
    service.ok(&["queue", "create", "pool/batch"]);
    service.ok(&["queue", "create", "pool/interactive"]);
    service.ok(&["queue", "create", "outside"]);
    let rule = match mode {
        "none" => Value::Null,
        "sensitive" | "tolerant" => {
            json!({"id":"benchmark", "resource":resource, "metric":"some", "window":"avg10",
            "high_bp":if mode == "sensitive" {100} else {9000}, "low_bp":if mode == "sensitive" {50} else {7000},
            "sustain_ms":1000, "minimum_hold_ms":2000, "recovery_ms":1000, "step_ms":1000, "required":true})
        }
        _ => unreachable!(),
    };
    if !rule.is_null() {
        let path = service.root.join("rule.json");
        fs::write(&path, serde_json::to_vec(&json!([rule])).unwrap()).unwrap();
        service.ok(&["group", "set", "pool", "--pressure", path.to_str().unwrap()]);
    }
    let start = Instant::now();
    let mut jobs = vec![submit(&service, "seed", 0, resource)];
    service.wait_file("seed-0");
    let seed = service.status(&jobs[0].0);
    let domain = Path::new(seed["workload_cgroup"].as_str().unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    assert_eq!(
        fs::read_to_string(domain.join("cpu.max")).unwrap().trim(),
        "50000 100000"
    );
    if resource == "memory" {
        assert_eq!(
            fs::read_to_string(domain.join("memory.high"))
                .unwrap()
                .trim(),
            "50331648"
        );
        assert_eq!(
            fs::read_to_string(domain.join("memory.max"))
                .unwrap()
                .trim(),
            "134217728"
        );
    }
    let mut peer = with_external.then(|| external(&service, &jobs[0].0, resource));
    let mut snapshots = Vec::new();
    let warmup = Instant::now();
    while warmup.elapsed() < Duration::from_secs(3) {
        let status: Value = serde_json::from_str(&service.ok(&["pressure", "status"])).unwrap();
        snapshots.push(json!({"elapsed_ms":start.elapsed().as_millis(), "status":status}));
        std::thread::sleep(Duration::from_millis(250));
    }
    for i in 0..12 {
        jobs.push(submit(&service, "batch", i, resource));
        if i % 4 == 0 {
            jobs.push(submit(&service, "interactive", i / 4, resource));
        }
    }
    jobs.push(submit(&service, "outside", 0, resource));
    let mut finished = Vec::new();
    while !jobs.is_empty() {
        assert!(start.elapsed() < Duration::from_secs(90));
        jobs.retain(|(id, kind)| {
            let status = service.status(id);
            if !status["finished_ms"].is_null() {
                assert_eq!(status["state"], "Succeeded", "{status}");
                if kind == "outside" {
                    assert!(status["waited_for"].is_null(), "{status}");
                }
                finished.push(json!({"kind":kind, "job":status}));
                false
            } else {
                true
            }
        });
        let status: Value = serde_json::from_str(&service.ok(&["pressure", "status"])).unwrap();
        snapshots.push(json!({"elapsed_ms":start.elapsed().as_millis(), "status":status}));
        if !jobs.is_empty() {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    if let Some(peer) = &mut peer {
        loop {
            if let Some(status) = peer.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(90));
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let events: Value = serde_json::from_str(&service.ok(&["pressure", "events"])).unwrap();
    let mut previous = "open";
    let mut holding_entries = 0;
    let mut phase_changes = 0;
    for event in events["events"].as_array().unwrap() {
        let signal = &event["view"]["signal"];
        assert_ne!(signal["availability"], "unavailable", "{event}");
        if signal["availability"] == "available" {
            assert_eq!(signal["missing"], 0, "{event}");
        }
        let phase = event["view"]["phase"].as_str().unwrap();
        if phase != previous {
            phase_changes += 1;
            holding_entries += usize::from(phase == "holding");
            previous = phase;
        }
    }
    let selected = |kind: &str, end: &str, begin: &str| -> Vec<u64> {
        finished
            .iter()
            .filter(|v| kind == "all" || v["kind"] == kind)
            .map(|v| {
                v["job"][end]
                    .as_u64()
                    .unwrap()
                    .checked_sub(v["job"][begin].as_u64().unwrap())
                    .unwrap()
            })
            .collect()
    };
    let first = finished
        .iter()
        .map(|v| v["job"]["submitted_ms"].as_u64().unwrap())
        .min()
        .unwrap();
    let last = finished
        .iter()
        .map(|v| v["job"]["finished_ms"].as_u64().unwrap())
        .max()
        .unwrap();
    let makespan = last.checked_sub(first).unwrap();
    let memory_events = counters(&domain.join("memory.events"));
    assert_eq!(memory_events["oom"], 0, "{memory_events}");
    assert_eq!(memory_events["oom_kill"], 0, "{memory_events}");
    assert_eq!(memory_events["max"], 0, "{memory_events}");
    json!({"mode":mode, "resource":resource, "repetition":repetition, "external_peer":with_external, "rule":rule,
        "kernel":{"cpu_stat":counters(&domain.join("cpu.stat")), "memory_events":memory_events,
            "memory_peak_bytes":fs::read_to_string(domain.join("memory.peak")).ok().and_then(|v| v.trim().parse::<u64>().ok())},
        "metrics":{"makespan_ms":makespan, "completed_jobs":finished.len(),
            "jobs_per_second":finished.len() as f64 * 1000.0 / makespan as f64,
            "admission_wait":distribution(selected("all","started_ms","submitted_ms")),
            "batch_response":distribution(selected("batch","finished_ms","submitted_ms")),
            "interactive_response":distribution(selected("interactive","finished_ms","submitted_ms")),
            "outside_response":distribution(selected("outside","finished_ms","submitted_ms")),
            "holding_entries":holding_entries, "phase_changes":phase_changes},
        "jobs":finished, "pressure_samples":snapshots, "pressure_events":events})
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--burn-ms") {
        assert_eq!(args.len(), 3);
        burn(args[1].parse().unwrap(), Path::new(&args[2]));
        return;
    }
    if args.first().is_some_and(|a| a == "--memory-mib") {
        assert_eq!(args.len(), 5);
        assert_eq!(args[2], "--hold-ms");
        memory(
            args[1].parse().unwrap(),
            args[3].parse().unwrap(),
            Path::new(&args[4]),
        );
        return;
    }
    let args: Vec<_> = args.iter().filter(|a| a.as_str() != "--bench").collect();
    assert!(
        args.len() == 4 || args.len() == 6,
        "--output FILE --repetitions 1..10 [--resource cpu|memory]"
    );
    let resource = if args.len() == 6 {
        assert_eq!(args[4], "--resource");
        args[5].as_str()
    } else {
        "cpu"
    };
    assert!(matches!(resource, "cpu" | "memory"));
    assert_eq!(args[0], "--output");
    assert_eq!(args[2], "--repetitions");
    let repetitions: usize = args[3].parse().unwrap();
    assert!((1..=10).contains(&repetitions));
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(args[1])
        .unwrap();
    let environment = json!({"record":"environment", "schema_version":1,
        "kernel":String::from_utf8(Command::new("uname").arg("-srvm").output().unwrap().stdout).unwrap().trim(),
        "boot_id":fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap().trim(),
        "cpus_allowed":fs::read_to_string("/proc/self/status").unwrap().lines().find(|l| l.starts_with("Cpus_allowed_list:")).unwrap(),
        "repetitions":repetitions, "scope_cpu_limit":"0.5", "scope_max_running":4,
        "resource":resource, "seed_cpu_ms":(resource == "cpu").then_some(4000),
        "batch_count":12, "batch_cpu_ms":(resource == "cpu").then_some(100), "interactive_count":3,
        "interactive_cpu_ms":10, "external_cpu_ms":(resource == "cpu").then_some(3000), "warmup_ms":3000,
        "memory_workload":(resource == "memory").then(|| json!({"scope_high_bytes":50331648,"scope_max_bytes":134217728,
            "seed_mib":24,"seed_hold_ms":8000,"batch_mib":16,"batch_hold_ms":2000,"external_mib":16,"external_hold_ms":8000})),
        "clock":"job realtime milliseconds; benchmark observation timestamps are monotonic",
        "quantiles":"nearest rank", "unit":"milliseconds unless named otherwise"});
    writeln!(output, "{environment}").unwrap();
    output.sync_data().unwrap();
    for repetition in 0..repetitions {
        let modes = if repetition % 2 == 0 {
            ["none", "sensitive", "tolerant"]
        } else {
            ["tolerant", "sensitive", "none"]
        };
        for with_external in [false, true] {
            for mode in modes {
                let result = run(mode, repetition, with_external, resource);
                writeln!(output, "{}", json!({"record":"run", "result":result})).unwrap();
                output.sync_data().unwrap();
                println!(
                    "{}",
                    json!({"mode":mode,"repetition":repetition,"external_peer":with_external,"metrics":result["metrics"]})
                );
            }
        }
    }
}
