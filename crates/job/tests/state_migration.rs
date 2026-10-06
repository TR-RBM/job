use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Fixture {
    root: PathBuf,
    child: Option<Child>,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("job-migration-{}-{name}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let mut fixture = Self { root, child: None };
        fixture.start("source");
        fixture
    }

    fn start(&mut self, directory: &str) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_job"));
        command
            .arg("daemon")
            .env("LC_ALL", "C")
            .env_remove("JOB_CONFIG")
            .env("XDG_CONFIG_HOME", self.root.join("unused-config"))
            .env("JOB_CGROUP_ROOT", self.root.join("absent-cgroup"))
            .env("JOB_STATE_DIR", self.root.join(directory))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if self
            .root
            .join(directory)
            .join("migration-config.toml")
            .exists()
        {
            command.env(
                "JOB_CONFIG",
                self.root.join(directory).join("migration-config.toml"),
            );
        }
        self.child = Some(command.spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            assert!(
                self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "daemon exited"
            );
            if let Ok(mut stream) =
                UnixStream::connect(self.root.join(directory).join("daemon.sock"))
            {
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut response = String::new();
                if writeln!(stream, "\"Ping\"").is_ok()
                    && stream.read_to_string(&mut response).is_ok()
                    && response.contains("Pong")
                {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("daemon did not become ready");
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn cli(&self, directory: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_job"))
            .args(args)
            .env("JOB_CLI_COMPAT", "legacy")
            .env("LC_ALL", "C")
            .env("JOB_STATE_DIR", self.root.join(directory))
            .current_dir(&self.root)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> Output {
        let output = self.cli("source", args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn legacy(&mut self) {
        self.ok(&["queue", "create", "serial", "--max-running", "1"]);
        self.ok(&["run", "--", "echo original-output"]);
        self.ok(&["run", "--queue", "serial", "--", "true"]);
        self.ok(&["run", "--", "true"]);
        self.stop();
        let source = self.root.join("source");
        fs::remove_file(source.join("schema.json")).unwrap();
        fs::remove_file(source.join("objects.json")).unwrap();
        fs::create_dir(source.join("queues")).unwrap();
        fs::write(source.join("queues/serial.json"), br#"{"name":"serial","parallel":1,"paused":false,"draining":false,"created_ms":0,"settings":{}}"#).unwrap();
        for id in 1..=3 {
            let path = source.join(format!("jobs/{id}/job.json"));
            let mut job: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            for key in ["policy", "queue_id", "supervisor_boot_id"] {
                job.as_object_mut().unwrap().remove(key);
            }
            job["extension_metadata"] = serde_json::json!({"preserve":"original"});
            job["spec"]["queue"] = match id {
                1 => serde_json::Value::Null,
                2 => serde_json::json!("serial"),
                _ => serde_json::json!("removed-queue"),
            };
            fs::write(path, serde_json::to_vec(&job).unwrap()).unwrap();
        }
    }

    fn migrate_args(&self, dry_run: bool) -> Vec<&str> {
        let mut args = vec![
            "state",
            "migrate",
            "--source",
            "source",
            "--destination",
            "converted",
            "--backup",
            "backup",
            "--profile",
            "legacy",
        ];
        if dry_run {
            args.push("--dry-run");
        }
        args
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn scan(base: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                scan(base, &entry.path(), files);
            } else if entry.file_type().unwrap().is_file() && entry.file_name() != "daemon.lock" {
                files.insert(
                    entry.path().strip_prefix(base).unwrap().to_owned(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    scan(root, root, &mut result);
    result
}

#[test]
fn dry_run_leaves_state_unchanged_and_reports_historical_binding() {
    let mut fixture = Fixture::new("dry-run");
    fixture.legacy();
    let before = files(&fixture.root.join("source"));
    let output = fixture.ok(&fixture.migrate_args(true));
    let inventory: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(inventory["unbound_jobs"], serde_json::json!([1, 2, 3]));
    assert_eq!(
        inventory["planned_service_policy"]["config"]["profile"],
        "legacy"
    );
    assert_eq!(
        inventory["planned_service_policy"]["automatic_estimation"],
        true
    );
    assert_eq!(
        inventory["recovered_historical_queues"],
        serde_json::json!(["removed-queue"])
    );
    assert_eq!(before, files(&fixture.root.join("source")));
    assert!(!fixture.root.join("backup").exists());
    assert!(!fixture.root.join("converted").exists());
}

#[test]
fn migration_preserves_serial_queues_ids_logs_and_default_membership() {
    let mut fixture = Fixture::new("conversion");
    fixture.legacy();
    let before = files(&fixture.root.join("source"));
    fixture.ok(&fixture.migrate_args(false));
    assert_eq!(before, files(&fixture.root.join("source")));
    assert_eq!(before, files(&fixture.root.join("backup/data")));
    let converted: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.root.join("converted/jobs/1/job.json")).unwrap())
            .unwrap();
    assert_eq!(
        converted["extension_metadata"],
        serde_json::json!({"preserve":"original"})
    );
    fixture.start("converted");
    let status = fixture.cli("converted", &["status", "1", "--json"]);
    let record: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(record["id"], 1);
    assert_eq!(record["queue_id"], 2);
    assert_eq!(record["policy"], "legacy");
    let queue = fixture.cli("converted", &["queue", "show", "serial", "--json"]);
    let queue: serde_json::Value = serde_json::from_slice(&queue.stdout).unwrap();
    assert_eq!(queue["objects"][0]["object"]["config"]["max_running"], 1);
    let log = fixture.cli("converted", &["log", "1", "full"]);
    assert!(String::from_utf8_lossy(&log.stdout).contains("original-output"));
    let submitted = fixture.cli("converted", &["submit", "--", "true"]);
    assert_eq!(String::from_utf8_lossy(&submitted.stdout).trim(), "4");
    assert!(fixture.cli("converted", &["wait", "4"]).status.success());
}

#[test]
fn restoring_a_backup_reproduces_all_original_file_bytes() {
    let mut fixture = Fixture::new("restore");
    fixture.legacy();
    fixture.ok(&[
        "state",
        "backup",
        "--source",
        "source",
        "--destination",
        "backup",
    ]);
    fixture.ok(&[
        "state",
        "restore",
        "--source",
        "backup",
        "--destination",
        "restored",
    ]);
    assert_eq!(
        files(&fixture.root.join("source")),
        files(&fixture.root.join("restored"))
    );
}

#[test]
fn a_changed_backup_is_refused_without_publishing_a_destination() {
    let mut fixture = Fixture::new("corrupt-backup");
    fixture.legacy();
    fixture.ok(&[
        "state",
        "backup",
        "--source",
        "source",
        "--destination",
        "backup",
    ]);
    fs::write(
        fixture.root.join("backup/data/jobs/1/output.log"),
        b"modified",
    )
    .unwrap();
    let output = fixture.cli(
        "source",
        &[
            "state",
            "restore",
            "--source",
            "backup",
            "--destination",
            "restored",
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("integrity"));
    assert!(!fixture.root.join("restored").exists());
}

#[test]
fn maintenance_refuses_a_live_daemon() {
    let fixture = Fixture::new("locked");
    let output = fixture.cli(
        "source",
        &[
            "state",
            "backup",
            "--source",
            "source",
            "--destination",
            "backup",
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("another daemon"));
    assert!(!fixture.root.join("backup").exists());
}

#[test]
fn queued_work_prevents_offline_conversion() {
    let mut fixture = Fixture::new("queued");
    fixture.ok(&["queue", "pause", "default"]);
    fixture.ok(&["submit", "--", "true"]);
    fixture.stop();
    let output = fixture.ok(&fixture.migrate_args(true));
    let inventory: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(inventory["conversion_ready"], false);
    let output = fixture.cli("source", &fixture.migrate_args(false));
    assert!(!output.status.success());
    assert!(!fixture.root.join("backup").exists());
    assert!(!fixture.root.join("converted").exists());
}

#[test]
fn corrupted_records_are_not_silently_skipped() {
    let mut fixture = Fixture::new("corrupt-record");
    fixture.legacy();
    fs::write(fixture.root.join("source/jobs/2/job.json"), b"{").unwrap();
    let output = fixture.cli("source", &fixture.migrate_args(false));
    assert!(!output.status.success());
    assert!(!fixture.root.join("backup").exists());
}

#[test]
fn migration_never_overwrites_existing_destinations() {
    let mut fixture = Fixture::new("destination");
    fixture.legacy();
    fs::create_dir(fixture.root.join("converted")).unwrap();
    fs::write(fixture.root.join("converted/keep"), b"existing data").unwrap();
    let output = fixture.cli("source", &fixture.migrate_args(false));
    assert!(!output.status.success());
    assert_eq!(
        fs::read(fixture.root.join("converted/keep")).unwrap(),
        b"existing data"
    );
    assert!(!fixture.root.join("backup").exists());
}

#[test]
fn an_unknown_state_version_is_refused_before_conversion() {
    let mut fixture = Fixture::new("future");
    fixture.stop();
    fs::write(
        fixture.root.join("source/schema.json"),
        br#"{"schema_version":999,"resource_vocabulary":"legacy_declarations"}"#,
    )
    .unwrap();
    let output = fixture.cli("source", &fixture.migrate_args(false));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported state schema"));
    assert!(!fixture.root.join("backup").exists());
}

#[test]
fn symlinks_in_state_cannot_escape_the_backup_source() {
    let mut fixture = Fixture::new("symlink");
    fixture.legacy();
    fs::write(fixture.root.join("outside"), b"outside data").unwrap();
    std::os::unix::fs::symlink(
        fixture.root.join("outside"),
        fixture.root.join("source/link"),
    )
    .unwrap();
    let output = fixture.cli("source", &fixture.migrate_args(false));
    assert!(!output.status.success());
    assert!(!fixture.root.join("backup").exists());
    assert_eq!(
        fs::read(fixture.root.join("outside")).unwrap(),
        b"outside data"
    );
}

#[test]
fn nested_migration_destinations_are_refused() {
    let mut fixture = Fixture::new("nested");
    fixture.legacy();
    let output = fixture.cli(
        "source",
        &[
            "state",
            "migrate",
            "--source",
            "source",
            "--destination",
            "source/converted",
            "--backup",
            "backup",
            "--profile",
            "legacy",
        ],
    );
    assert!(!output.status.success());
    assert!(!fixture.root.join("backup").exists());
    assert!(!fixture.root.join("source/converted").exists());
}

#[test]
fn default_discovery_refuses_to_guess_between_exec_and_job() {
    let mut fixture = Fixture::new("ambiguous");
    fixture.stop();
    fs::rename(fixture.root.join("source"), fixture.root.join("exec")).unwrap();
    fs::create_dir(fixture.root.join("job")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_job"))
        .args(["host", "--json"])
        .env_remove("JOB_STATE_DIR")
        .env("XDG_STATE_HOME", &fixture.root)
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("legacy exec state exists"));
    assert!(fixture.root.join("exec/schema.json").exists());
    assert_eq!(fs::read_dir(fixture.root.join("job")).unwrap().count(), 0);
}

#[test]
fn restored_current_state_serves_its_own_logs() {
    let mut fixture = Fixture::new("restore-current");
    fixture.ok(&["run", "--", "echo restored-output"]);
    fixture.stop();
    fixture.ok(&[
        "state",
        "backup",
        "--source",
        "source",
        "--destination",
        "backup",
    ]);
    fixture.ok(&[
        "state",
        "restore",
        "--source",
        "backup",
        "--destination",
        "restored",
    ]);
    fs::rename(
        fixture.root.join("source"),
        fixture.root.join("source-offline"),
    )
    .unwrap();
    fixture.start("restored");
    let log = fixture.cli("restored", &["log", "1", "full"]);
    assert!(log.status.success());
    assert!(String::from_utf8_lossy(&log.stdout).contains("restored-output"));
}

#[test]
fn live_network_helpers_prevent_copying_a_control_context() {
    let mut fixture = Fixture::new("helpers");
    fixture.stop();
    let pid = std::process::id();
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let ticks: u64 = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap();
    fs::create_dir(fixture.root.join("source/links")).unwrap();
    let process = serde_json::json!({"pid":pid,"start_ticks":ticks});
    let link = serde_json::json!({"name":"test","holder":process,"relay":process,"rate":null});
    fs::write(
        fixture.root.join("source/links/test.json"),
        serde_json::to_vec(&link).unwrap(),
    )
    .unwrap();
    let inventory = fixture.ok(&["state", "validate", "--source", "source"]);
    let inventory: serde_json::Value = serde_json::from_slice(&inventory.stdout).unwrap();
    assert_eq!(inventory["active_helpers"], serde_json::json!([pid]));
    assert_eq!(inventory["conversion_ready"], false);
    let output = fixture.cli(
        "source",
        &[
            "state",
            "backup",
            "--source",
            "source",
            "--destination",
            "backup",
        ],
    );
    assert!(!output.status.success());
    assert!(!fixture.root.join("backup").exists());
}

#[test]
fn backup_restore_preserves_attempts_and_rebases_archived_logs() {
    let mut fixture = Fixture::new("attempt-backup");
    fixture.ok(&["run", "--", "echo archived-output"]);
    fixture.ok(&["retry", "1", "--hold"]);
    fixture.ok(&["edit", "1", "--", "echo current-output"]);
    fixture.ok(&["release", "1"]);
    fixture.ok(&["wait", "1"]);
    fixture.stop();
    let before = files(&fixture.root.join("source"));
    fixture.ok(&[
        "state",
        "backup",
        "--source",
        "source",
        "--destination",
        "backup",
    ]);
    fixture.ok(&[
        "state",
        "restore",
        "--source",
        "backup",
        "--destination",
        "restored",
    ]);
    assert_eq!(files(&fixture.root.join("restored")), before);
    fs::rename(
        fixture.root.join("source"),
        fixture.root.join("source-offline"),
    )
    .unwrap();
    fixture.start("restored");
    let old = fixture.cli("restored", &["log", "1", "--attempt", "1", "full"]);
    assert!(old.status.success());
    assert!(String::from_utf8_lossy(&old.stdout).contains("archived-output"));
    let current = fixture.cli("restored", &["log", "1", "full"]);
    assert!(current.status.success());
    assert!(String::from_utf8_lossy(&current.stdout).contains("current-output"));
    assert!(fixture.cli("restored", &["retry", "1"]).status.success());
    assert!(fixture.cli("restored", &["wait", "1"]).status.success());
}

#[test]
fn incomplete_attempt_archives_are_rejected_by_offline_validation() {
    let mut fixture = Fixture::new("attempt-corrupt");
    fixture.ok(&["run", "--", "true"]);
    fixture.ok(&["retry", "1"]);
    fixture.ok(&["wait", "1"]);
    fixture.stop();
    fs::rename(
        fixture.root.join("source/jobs/1/attempts/1"),
        fixture.root.join("source/jobs/1/attempts/3"),
    )
    .unwrap();
    assert!(
        !fixture
            .cli("source", &["state", "validate", "--source", "source"])
            .status
            .success()
    );
}

#[test]
fn schema_two_converts_to_a_first_attempt_without_changing_its_original_bytes() {
    let mut fixture = Fixture::new("attempt-schema");
    fixture.ok(&["run", "--", "echo legacy-attempt"]);
    fixture.stop();
    fs::write(
        fixture.root.join("source/schema.json"),
        br#"{"schema_version":2,"resource_vocabulary":"legacy_declarations"}"#,
    )
    .unwrap();
    let path = fixture.root.join("source/jobs/1/job.json");
    let mut job: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    job.as_object_mut().unwrap().remove("attempt");
    job.as_object_mut().unwrap().remove("attempt_submitted_ms");
    fs::write(path, serde_json::to_vec(&job).unwrap()).unwrap();
    let before = files(&fixture.root.join("source"));
    fixture.ok(&fixture.migrate_args(false));
    assert_eq!(files(&fixture.root.join("source")), before);
    fixture.start("converted");
    let output = fixture.cli("converted", &["attempts", "1", "--json"]);
    assert!(output.status.success());
    let attempts: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(attempts["attempts"][0]["attempt"], 1);
    assert!(fixture.cli("converted", &["retry", "1"]).status.success());
    assert!(fixture.cli("converted", &["wait", "1"]).status.success());
}

#[test]
fn removal_receipts_survive_backup_restore_and_ids_remain_retired() {
    let mut fixture = Fixture::new("removal-backup");
    fixture.ok(&["run", "--", "true"]);
    fixture.ok(&["remove", "1"]);
    fixture.stop();
    let before = files(&fixture.root.join("source"));
    fixture.ok(&[
        "state",
        "backup",
        "--source",
        "source",
        "--destination",
        "backup",
    ]);
    fixture.ok(&[
        "state",
        "restore",
        "--source",
        "backup",
        "--destination",
        "restored",
    ]);
    assert_eq!(files(&fixture.root.join("restored")), before);
    fixture.start("restored");
    assert!(!fixture.cli("restored", &["status", "1"]).status.success());
    let created = fixture.cli("restored", &["create", "--", "true"]);
    assert!(created.status.success());
    assert_eq!(String::from_utf8_lossy(&created.stdout).trim(), "2");
}

#[test]
fn validation_refuses_a_counter_that_would_reuse_a_removed_job_id() {
    let mut fixture = Fixture::new("removal-counter");
    fixture.ok(&["run", "--", "true"]);
    fixture.ok(&["remove", "1"]);
    fixture.stop();
    fs::write(fixture.root.join("source/next-id"), "1").unwrap();
    assert!(
        !fixture
            .cli("source", &["state", "validate", "--source", "source"])
            .status
            .success()
    );
}

#[test]
fn cancellation_audit_survives_removal_backup_and_restore() {
    let mut fixture = Fixture::new("cancellation-backup");
    fixture.ok(&["create", "--", "true"]);
    fixture.ok(&["cancel", "1"]);
    fixture.ok(&["remove", "1"]);
    fixture.stop();
    let before = files(&fixture.root.join("source"));
    assert!(
        fs::read_dir(fixture.root.join("source/cancellations"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "json"))
    );
    fixture.ok(&[
        "state",
        "backup",
        "--source",
        "source",
        "--destination",
        "backup",
    ]);
    fixture.ok(&[
        "state",
        "restore",
        "--source",
        "backup",
        "--destination",
        "restored",
    ]);
    assert_eq!(files(&fixture.root.join("restored")), before);
    fixture.start("restored");
    assert!(
        fixture
            .cli("restored", &["create", "--", "true"])
            .status
            .success()
    );
}

#[test]
fn pending_cancellation_refuses_backup_until_runtime_reconciliation() {
    let mut fixture = Fixture::new("cancellation-pending");
    fixture.ok(&["create", "--", "true"]);
    fixture.ok(&["cancel", "1"]);
    fixture.stop();
    let path = fs::read_dir(fixture.root.join("source/cancellations"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut operation: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    operation["complete"] = serde_json::json!(false);
    operation["results"] = serde_json::json!([]);
    fs::write(&path, serde_json::to_vec(&operation).unwrap()).unwrap();
    let before = files(&fixture.root.join("source"));
    let output = fixture.cli(
        "source",
        &[
            "state",
            "backup",
            "--source",
            "source",
            "--destination",
            "backup",
        ],
    );
    assert!(!output.status.success());
    assert_eq!(files(&fixture.root.join("source")), before);
    assert!(!fixture.root.join("backup").exists());
    fixture.start("source");
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let operation: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        if operation["complete"] == true {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    fixture.stop();
    fixture.ok(&[
        "state",
        "backup",
        "--source",
        "source",
        "--destination",
        "backup",
    ]);
}

#[test]
fn schema_six_keeps_legacy_resource_meanings_when_retried_after_conversion() {
    let mut fixture = Fixture::new("resource-schema");
    fixture.ok(&["create", "--cores", "0.5", "--mem", "1M", "--", "true"]);
    fixture.ok(&["cancel", "1"]);
    fixture.stop();
    fs::write(
        fixture.root.join("source/schema.json"),
        br#"{"schema_version":6,"resource_vocabulary":"legacy_declarations"}"#,
    )
    .unwrap();
    let path = fixture.root.join("source/jobs/1/job.json");
    let mut job: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    job.as_object_mut().unwrap().remove("resource_sources");
    job.as_object_mut().unwrap().remove("applied_resources");
    for spec in ["spec", "submitted_spec", "requested_spec", "effective_spec"] {
        if let Some(declared) = job
            .get_mut(spec)
            .and_then(|value| value.get_mut("declared"))
            .and_then(serde_json::Value::as_object_mut)
        {
            for field in [
                "cpu_request_milli",
                "cpu_limit_milli",
                "cpu_weight",
                "memory_request",
                "memory_high",
                "memory_max",
                "memory_swap_max",
            ] {
                declared.remove(field);
            }
        }
    }
    fs::write(&path, serde_json::to_vec(&job).unwrap()).unwrap();
    let before = files(&fixture.root.join("source"));
    fixture.ok(&fixture.migrate_args(false));
    assert_eq!(files(&fixture.root.join("source")), before);
    let schema: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.root.join("converted/schema.json")).unwrap())
            .unwrap();
    assert_eq!(schema["schema_version"], 18);
    assert_eq!(
        schema["resource_vocabulary"],
        "independent_resources_with_legacy_aliases"
    );
    fixture.start("converted");
    let retried = fixture.cli("converted", &["retry", "1", "--hold"]);
    assert!(
        retried.status.success(),
        "{}",
        String::from_utf8_lossy(&retried.stderr)
    );
    let output = fixture.cli("converted", &["status", "1", "--json"]);
    let job: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(job["spec"]["declared"]["cpu_request_milli"], 500);
    assert_eq!(job["spec"]["declared"]["cpu_weight"], 50);
    assert!(job["spec"]["declared"]["cpu_limit_milli"].is_null());
    assert_eq!(job["spec"]["declared"]["memory_request"], 1 << 20);
    assert_eq!(job["spec"]["declared"]["memory_max"], 1 << 20);
    assert!(job["submitted_spec"]["declared"]["cpu_limit_milli"].is_null());
}

#[test]
fn schema_seven_conversion_preserves_independent_controls_without_adding_domains() {
    let mut fixture = Fixture::new("aggregate-schema");
    fixture.ok(&["group", "create", "tree", "--job-memory-request", "1M"]);
    fixture.ok(&["queue", "create", "tree/q"]);
    fixture.ok(&[
        "create",
        "-q",
        "tree/q",
        "--cpu-request",
        "0.5",
        "--",
        "true",
    ]);
    fixture.ok(&["cancel", "1"]);
    fixture.stop();
    fs::write(fixture.root.join("source/schema.json"), br#"{"schema_version":7,"resource_vocabulary":"independent_resources_with_legacy_aliases"}"#).unwrap();
    let path = fixture.root.join("source/jobs/1/job.json");
    let mut job: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    job.as_object_mut().unwrap().remove("aggregate_domains");
    fs::write(&path, serde_json::to_vec(&job).unwrap()).unwrap();
    let before = files(&fixture.root.join("source"));
    fixture.ok(&fixture.migrate_args(false));
    assert_eq!(files(&fixture.root.join("source")), before);
    fixture.start("converted");
    let output = fixture.cli("converted", &["status", "1", "--json"]);
    let converted: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(converted["spec"]["declared"], job["spec"]["declared"]);
    assert!(
        converted["aggregate_domains"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let output = fixture.cli("converted", &["group", "show", "tree", "--json"]);
    let tree: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        tree["objects"][0]["object"]["config"]["job_memory_request"],
        1048576
    );
    assert!(
        tree["objects"][0]["aggregate_domains"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn schema_eight_conversion_retains_absent_io_and_aggregate_history() {
    let mut fixture = Fixture::new("device-io-schema");
    fixture.ok(&["create", "--memory-request", "1M", "--", "true"]);
    fixture.ok(&["cancel", "1"]);
    fixture.stop();
    fs::write(fixture.root.join("source/schema.json"), br#"{"schema_version":8,"resource_vocabulary":"independent_resources_with_legacy_aliases"}"#).unwrap();
    let path = fixture.root.join("source/jobs/1/job.json");
    let mut job: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for spec in ["spec", "submitted_spec", "requested_spec", "effective_spec"] {
        if let Some(declared) = job
            .get_mut(spec)
            .and_then(|value| value.get_mut("declared"))
            .and_then(serde_json::Value::as_object_mut)
        {
            for field in ["io_max", "io_weight", "io_bfq_weight"] {
                declared.remove(field);
            }
        }
    }
    fs::write(&path, serde_json::to_vec(&job).unwrap()).unwrap();
    let source = files(&fixture.root.join("source"));
    fixture.ok(&fixture.migrate_args(false));
    assert_eq!(files(&fixture.root.join("source")), source);
    fixture.start("converted");
    let output = fixture.cli("converted", &["status", "1", "--json"]);
    let converted: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(converted["aggregate_domains"], job["aggregate_domains"]);
    assert_eq!(converted["spec"]["declared"]["memory_request"], 1048576);
    for field in ["io_max", "io_weight", "io_bfq_weight"] {
        assert!(converted["spec"]["declared"][field].is_null());
    }
}

#[test]
fn schema_nine_conversion_preserves_records_without_inventing_live_updates() {
    let mut fixture = Fixture::new("live-update-schema");
    fixture.ok(&["create", "--memory-request", "1M", "--", "true"]);
    fixture.ok(&["cancel", "1"]);
    fixture.stop();
    fs::write(fixture.root.join("source/schema.json"), br#"{"schema_version":9,"resource_vocabulary":"independent_resources_with_legacy_aliases"}"#).unwrap();
    let before = files(&fixture.root.join("source"));
    fixture.ok(&fixture.migrate_args(false));
    assert_eq!(files(&fixture.root.join("source")), before);
    let schema: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.root.join("converted/schema.json")).unwrap())
            .unwrap();
    assert_eq!(schema["schema_version"], 18);
    assert!(!fixture.root.join("converted/resource-updates").exists());
    fixture.start("converted");
    let output = fixture.cli("converted", &["status", "1", "--json"]);
    let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(record["state"], "Cancelled");
    assert_eq!(record["spec"]["declared"]["memory_request"], 1048576);
}

#[test]
fn schema_ten_conversion_does_not_enable_admission_policy() {
    let mut fixture = Fixture::new("admission-schema");
    fixture.ok(&["create", "--", "true"]);
    fixture.ok(&["cancel", "1"]);
    fixture.stop();
    fs::write(fixture.root.join("source/schema.json"), br#"{"schema_version":10,"resource_vocabulary":"independent_resources_with_legacy_aliases"}"#).unwrap();
    let path = fixture.root.join("source/jobs/1/job.json");
    let mut record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for field in ["priority_source", "priority_changes", "admission_snapshot"] {
        record.as_object_mut().unwrap().remove(field);
    }
    for spec in ["spec", "submitted_spec", "requested_spec", "effective_spec"] {
        if let Some(declared) = record
            .get_mut(spec)
            .and_then(|v| v.get_mut("declared"))
            .and_then(serde_json::Value::as_object_mut)
        {
            declared.remove("priority");
        }
    }
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let before = files(&fixture.root.join("source"));
    fixture.ok(&fixture.migrate_args(false));
    assert_eq!(files(&fixture.root.join("source")), before);
    assert!(!fixture.root.join("converted/scheduling.json").exists());
    fixture.start("converted");
    let output = fixture.cli("converted", &["explain", "1", "--json"]);
    assert!(output.status.success());
    let explanation: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(explanation["ordering_enabled"], false);
    assert!(explanation["priority"].is_null());
    assert_eq!(explanation["eligible_wait_ms"], 0);
}

#[test]
fn malformed_scheduling_ledger_is_not_silently_discarded_by_migration() {
    let mut fixture = Fixture::new("invalid-admission-ledger");
    fixture.stop();
    let path = fixture.root.join("source/scheduling.json");
    for identity in ["0:1", "1:0", "01:1", "1:+1", "1:1:1"] {
        let value = serde_json::json!({"schema_version": 1, "boot_id": "previous-boot", "checkpoint_ms": 1, "credits": {identity: {"milliseconds": 20, "eligible": false}}});
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let output = fixture.cli("source", &["state", "validate", "--source", "source"]);
        assert!(!output.status.success(), "{identity}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("invalid scheduling ledger"));
    }
    let value = serde_json::json!({"schema_version": 1, "boot_id": fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap().trim(), "checkpoint_ms": u64::MAX, "credits": {}});
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(
        !fixture
            .cli("source", &["state", "validate", "--source", "source"])
            .status
            .success()
    );
}

#[test]
fn scheduling_ledger_survives_backup_and_restore_byte_for_byte() {
    let mut fixture = Fixture::new("admission-ledger-backup");
    fixture.ok(&["queue", "create", "work", "--aging", "1s"]);
    fixture.ok(&["run", "-q", "work", "--", "true"]);
    fixture.stop();
    let before = fs::read(fixture.root.join("source/scheduling.json")).unwrap();
    fixture.ok(&[
        "state",
        "backup",
        "--source",
        "source",
        "--destination",
        "backup",
    ]);
    fixture.ok(&[
        "state",
        "restore",
        "--source",
        "backup",
        "--destination",
        "restored",
    ]);
    assert_eq!(
        fs::read(fixture.root.join("restored/scheduling.json")).unwrap(),
        before
    );
    fixture.start("restored");
    let output = fixture.cli("restored", &["explain", "1", "--json"]);
    assert!(output.status.success());
    let explanation: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(explanation["aging"]["value"], 1000);
}

#[test]
fn schema_eleven_migration_preserves_aging_without_activating_fair_share() {
    let mut fixture = Fixture::new("fair-share-schema");
    fixture.ok(&["queue", "create", "work", "--aging", "1s"]);
    fixture.ok(&["run", "-q", "work", "--", "true"]);
    fixture.stop();
    let path = fixture.root.join("source/scheduling.json");
    let mut ledger: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    ledger["schema_version"] = serde_json::json!(1);
    ledger.as_object_mut().unwrap().remove("fair");
    fs::write(&path, serde_json::to_vec(&ledger).unwrap()).unwrap();
    fs::write(fixture.root.join("source/schema.json"), br#"{"schema_version":11,"resource_vocabulary":"independent_resources_with_legacy_aliases"}"#).unwrap();
    let before = files(&fixture.root.join("source"));
    fixture.ok(&fixture.migrate_args(false));
    assert_eq!(files(&fixture.root.join("source")), before);
    fixture.start("converted");
    let explanation = fixture.cli("converted", &["explain", "1", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&explanation.stdout).unwrap();
    assert!(value["fair_share"].as_array().unwrap().is_empty());
    assert_eq!(value["aging"]["value"], 1000);
}

#[test]
fn fair_share_accounting_is_validated_and_retained_by_backup() {
    let mut fixture = Fixture::new("fair-share-backup");
    fixture.ok(&[
        "group",
        "create",
        "team",
        "--fair-share",
        "cpu-request-time",
    ]);
    fixture.ok(&["queue", "create", "team/a"]);
    fixture.ok(&["run", "-q", "team/a", "--cpu-request", "0.1", "--", "true"]);
    fixture.stop();
    let path = fixture.root.join("source/scheduling.json");
    let before = fs::read(&path).unwrap();
    fixture.ok(&[
        "state",
        "backup",
        "--source",
        "source",
        "--destination",
        "backup",
    ]);
    fixture.ok(&[
        "state",
        "restore",
        "--source",
        "backup",
        "--destination",
        "restored",
    ]);
    assert_eq!(
        fs::read(fixture.root.join("restored/scheduling.json")).unwrap(),
        before
    );
    let mut ledger: serde_json::Value = serde_json::from_slice(&before).unwrap();
    let scope = ledger["fair"]["scopes"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .next()
        .unwrap();
    scope["branches"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .next()
        .unwrap()["weight"] = serde_json::json!(0);
    fs::write(&path, serde_json::to_vec(&ledger).unwrap()).unwrap();
    let output = fixture.cli("source", &["state", "validate", "--source", "source"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid fair-share accounting"));
}

#[test]
fn pressure_state_is_validated_and_retained_by_backup() {
    let mut fixture = Fixture::new("pressure-backup");
    fs::write(fixture.root.join("rules.json"), br#"[{"id":"memory","resource":"memory","metric":"full","window":"avg10","high_bp":100,"low_bp":10,"sustain_ms":1000,"minimum_hold_ms":1000,"recovery_ms":1000,"step_ms":1000,"required":false}]"#).unwrap();
    fixture.ok(&["queue", "create", "observed", "--pressure", "rules.json"]);
    fixture.ok(&["run", "-q", "observed", "--", "true"]);
    fixture.stop();
    let path = fixture.root.join("source/pressure.json");
    let before = fs::read(&path).unwrap();
    fixture.ok(&[
        "state",
        "backup",
        "--source",
        "source",
        "--destination",
        "backup",
    ]);
    fixture.ok(&[
        "state",
        "restore",
        "--source",
        "backup",
        "--destination",
        "restored",
    ]);
    assert_eq!(
        fs::read(fixture.root.join("restored/pressure.json")).unwrap(),
        before
    );
    let mut ledger: serde_json::Value = serde_json::from_slice(&before).unwrap();
    ledger["views"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .next()
        .unwrap()["phase"] = serde_json::json!("holding");
    fs::write(&path, serde_json::to_vec(&ledger).unwrap()).unwrap();
    let result = fixture.cli("source", &["state", "validate", "--source", "source"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("invalid pressure ledger"));
    fixture.start("restored");
    let result = fixture.cli("restored", &["run", "-q", "observed", "--", "true"]);
    assert!(result.status.success());
}

#[test]
fn schema_twelve_migration_leaves_pressure_policies_absent() {
    let mut fixture = Fixture::new("pressure-schema");
    fixture.ok(&["run", "--", "true"]);
    fixture.stop();
    fs::write(fixture.root.join("source/schema.json"), br#"{"schema_version":12,"resource_vocabulary":"independent_resources_with_legacy_aliases"}"#).unwrap();
    let before = files(&fixture.root.join("source"));
    fixture.ok(&fixture.migrate_args(false));
    assert_eq!(files(&fixture.root.join("source")), before);
    assert!(!fixture.root.join("converted/pressure.json").exists());
    fixture.start("converted");
    let output = fixture.cli("converted", &["pressure", "status"]);
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(report["rules"].as_object().unwrap().is_empty());
}

#[test]
fn versioned_presets_archive_and_attempts_survive_backup_and_reject_corruption() {
    let mut fixture = Fixture::new("presets-backup");
    fixture.stop();
    fs::write(
        fixture.root.join("source/migration-config.toml"),
        r#"
schema_version = 1
[[presets.classes]]
name = 'interactive'
revision = 1
priority = 100
[[presets.profiles]]
name = 'small'
revision = 1
scheduling_class = 'interactive@1'
[presets.profiles.values]
cpu_request_milli = 100
"#,
    )
    .unwrap();
    fixture.start("source");
    fixture.ok(&["run", "--execution-profile", "small@1", "--", "true"]);
    fixture.ok(&["retry", "1"]);
    fixture.ok(&["wait", "1", "--timeout", "5s"]);
    fixture.stop();
    let archive = fixture.root.join("source/presets.json");
    let original_archive = fs::read(&archive).unwrap();
    let record = fixture.root.join("source/jobs/1/job.json");
    let original_record = fs::read(&record).unwrap();
    fixture.ok(&[
        "state",
        "backup",
        "--source",
        "source",
        "--destination",
        "backup",
    ]);
    fixture.ok(&[
        "state",
        "restore",
        "--source",
        "backup",
        "--destination",
        "restored",
    ]);
    assert_eq!(
        fs::read(fixture.root.join("restored/presets.json")).unwrap(),
        original_archive
    );
    assert_eq!(
        fs::read(fixture.root.join("restored/jobs/1/job.json")).unwrap(),
        original_record
    );
    let mut changed: serde_json::Value = serde_json::from_slice(&original_record).unwrap();
    changed["preset_snapshot"]["defaults"]["cpu_request_milli"] = serde_json::json!(200);
    fs::write(&record, serde_json::to_vec(&changed).unwrap()).unwrap();
    let refused = fixture.cli("source", &["state", "validate", "--source", "source"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("invalid preset snapshot"));
    fs::write(&record, &original_record).unwrap();
    let mut changed: serde_json::Value = serde_json::from_slice(&original_archive).unwrap();
    changed["definitions"]["profile:small@1"]["extends"] = serde_json::json!(["small@1"]);
    fs::write(&archive, serde_json::to_vec(&changed).unwrap()).unwrap();
    let refused = fixture.cli("source", &["state", "validate", "--source", "source"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("cyclic"));
    fs::remove_file(&archive).unwrap();
    std::os::unix::fs::symlink(fixture.root.join("restored/presets.json"), &archive).unwrap();
    assert!(
        !fixture
            .cli("source", &["state", "validate", "--source", "source"])
            .status
            .success()
    );
    fs::remove_file(&archive).unwrap();
    assert!(
        !fixture
            .cli("source", &["state", "validate", "--source", "source"])
            .status
            .success()
    );
    fixture.start("restored");
    let attempts = fixture.cli("restored", &["attempts", "1", "--json"]);
    assert!(attempts.status.success());
    let attempts: serde_json::Value = serde_json::from_slice(&attempts.stdout).unwrap();
    assert_eq!(attempts["attempts"].as_array().unwrap().len(), 2);
    for attempt in attempts["attempts"].as_array().unwrap() {
        assert_eq!(
            attempt["preset_snapshot"]["execution_profile"]["reference"],
            "small@1"
        );
    }
}

#[test]
fn schema_thirteen_migration_does_not_manufacture_presets() {
    let mut fixture = Fixture::new("presets-schema");
    fixture.ok(&["run", "--", "true"]);
    fixture.stop();
    fs::write(fixture.root.join("source/schema.json"), br#"{"schema_version":13,"resource_vocabulary":"independent_resources_with_legacy_aliases"}"#).unwrap();
    let record = fixture.root.join("source/jobs/1/job.json");
    let mut original: serde_json::Value =
        serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    original.as_object_mut().unwrap().remove("preset_snapshot");
    fs::write(&record, serde_json::to_vec(&original).unwrap()).unwrap();
    let before = files(&fixture.root.join("source"));
    fixture.ok(&fixture.migrate_args(true));
    assert_eq!(files(&fixture.root.join("source")), before);
    assert!(!fixture.root.join("converted").exists());
    fixture.ok(&fixture.migrate_args(false));
    assert_eq!(files(&fixture.root.join("source")), before);
    assert!(!fixture.root.join("converted/presets.json").exists());
    let converted: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.root.join("converted/jobs/1/job.json")).unwrap())
            .unwrap();
    assert!(converted.get("preset_snapshot").is_none());
    fixture.start("converted");
    let record = fixture.cli("converted", &["status", "1", "--json"]);
    assert!(record.status.success());
    let record: serde_json::Value = serde_json::from_slice(&record.stdout).unwrap();
    assert!(record["preset_snapshot"].is_null());
}

#[test]
fn process_launch_controls_survive_archival_backup_and_reject_invalid_limits() {
    let mut fixture = Fixture::new("process-backup");
    fixture.ok(&[
        "run",
        "--rlimit",
        "nofile=64:96",
        "--cpu-affinity",
        "inherit",
        "--",
        "true",
    ]);
    fixture.ok(&["retry", "1"]);
    fixture.ok(&["wait", "1"]);
    fixture.stop();
    let original = files(&fixture.root.join("source"));
    fixture.ok(&[
        "state",
        "backup",
        "--source",
        "source",
        "--destination",
        "backup",
    ]);
    fixture.ok(&[
        "state",
        "restore",
        "--source",
        "backup",
        "--destination",
        "restored",
    ]);
    assert_eq!(files(&fixture.root.join("restored")), original);
    let archived = fixture.root.join("source/jobs/1/attempts/1/job.json");
    let mut invalid: serde_json::Value =
        serde_json::from_slice(&fs::read(&archived).unwrap()).unwrap();
    invalid["result"]["process_controls"]["rlimits"]["nofile"]["soft"] = serde_json::json!(100);
    fs::write(&archived, serde_json::to_vec(&invalid).unwrap()).unwrap();
    let check = fixture.cli("source", &["state", "validate", "--source", "source"]);
    assert!(!check.status.success());
    assert!(String::from_utf8_lossy(&check.stderr).contains("soft value exceeds"));
    fixture.start("restored");
    let output = fixture.cli("restored", &["status", "1", "--json"]);
    assert!(output.status.success());
    let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        record["result"]["process_controls"]["rlimits"]["nofile"],
        serde_json::json!({"soft":64,"hard":96})
    );
}

#[test]
fn schema_fifteen_conversion_does_not_invent_process_controls() {
    let mut fixture = Fixture::new("process-schema");
    fixture.ok(&["run", "--", "true"]);
    fixture.stop();
    fs::write(fixture.root.join("source/schema.json"),br#"{"schema_version":15,"resource_vocabulary":"independent_resources_with_legacy_aliases"}"#).unwrap();
    let path = fixture.root.join("source/jobs/1/job.json");
    let mut record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for key in ["spec", "submitted_spec", "requested_spec", "effective_spec"] {
        if let Some(declared) = record[key]["declared"].as_object_mut() {
            declared.retain(|key, _| {
                !key.starts_with("rlimit_") && key != "numa_policy" && key != "cpu_affinity"
            });
        }
    }
    record["result"]
        .as_object_mut()
        .unwrap()
        .remove("process_controls");
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let before = files(&fixture.root.join("source"));
    fixture.ok(&fixture.migrate_args(false));
    assert_eq!(before, files(&fixture.root.join("source")));
    fixture.start("converted");
    let output = fixture.cli("converted", &["status", "1", "--json"]);
    assert!(output.status.success());
    let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(record["spec"]["declared"]["cpu_affinity"].is_null());
    assert!(record["spec"]["declared"]["rlimit_nofile"].is_null());
    assert!(record["result"]["process_controls"].is_null());
}

#[test]
fn security_launch_controls_survive_archival_backup_and_reject_invalid_outcomes() {
    let mut fixture = Fixture::new("security-backup");
    fixture.ok(&["run", "--seccomp-deny", "getppid", "--", "true"]);
    fixture.ok(&["retry", "1"]);
    fixture.ok(&["wait", "1"]);
    fixture.stop();
    let original = files(&fixture.root.join("source"));
    fixture.ok(&[
        "state",
        "backup",
        "--source",
        "source",
        "--destination",
        "backup",
    ]);
    fixture.ok(&[
        "state",
        "restore",
        "--source",
        "backup",
        "--destination",
        "restored",
    ]);
    assert_eq!(files(&fixture.root.join("restored")), original);
    let archived = fixture.root.join("source/jobs/1/attempts/1/job.json");
    let mut invalid: serde_json::Value =
        serde_json::from_slice(&fs::read(&archived).unwrap()).unwrap();
    invalid["result"]["security_controls"]["seccomp_arch"] = serde_json::Value::Null;
    fs::write(&archived, serde_json::to_vec(&invalid).unwrap()).unwrap();
    let check = fixture.cli("source", &["state", "validate", "--source", "source"]);
    assert!(!check.status.success());
    assert!(String::from_utf8_lossy(&check.stderr).contains("invalid applied security controls"));
    fixture.start("restored");
    let output = fixture.cli("restored", &["status", "1", "--json"]);
    assert!(output.status.success());
    let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        record["result"]["security_controls"]["seccomp_deny"],
        "getppid"
    );
}

#[test]
fn schema_sixteen_conversion_does_not_invent_security_controls() {
    let mut fixture = Fixture::new("security-schema");
    fixture.ok(&["run", "--", "true"]);
    fixture.stop();
    fs::write(fixture.root.join("source/schema.json"),br#"{"schema_version":16,"resource_vocabulary":"independent_resources_with_legacy_aliases"}"#).unwrap();
    let path = fixture.root.join("source/jobs/1/job.json");
    let mut record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for key in ["spec", "submitted_spec", "requested_spec", "effective_spec"] {
        if let Some(declared) = record[key]["declared"].as_object_mut() {
            declared.retain(|key, _| {
                !["no_new_privs", "cap_drop", "seccomp_deny"].contains(&key.as_str())
            });
        }
    }
    record["result"]
        .as_object_mut()
        .unwrap()
        .remove("security_controls");
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let before = files(&fixture.root.join("source"));
    fixture.ok(&fixture.migrate_args(false));
    assert_eq!(before, files(&fixture.root.join("source")));
    fixture.start("converted");
    let output = fixture.cli("converted", &["status", "1", "--json"]);
    assert!(output.status.success());
    let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(record["spec"]["declared"]["seccomp_deny"].is_null());
    assert!(record["spec"]["declared"]["no_new_privs"].is_null());
    assert!(record["result"]["security_controls"].is_null());
}

#[test]
fn isolation_launch_controls_survive_archival_backup_and_reject_invalid_outcomes() {
    let mut fixture = Fixture::new("isolation-backup");
    let outside = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("isolation-backup-{}", std::process::id()));
    std::fs::create_dir_all(&outside).unwrap();
    fixture.ok(&[
        "run",
        "--dir",
        outside.to_str().unwrap(),
        "--namespaces",
        "user,mount,uts",
        "--root",
        "read-only",
        "--private-tmp",
        "yes",
        "--",
        "true",
    ]);
    fixture.ok(&["retry", "1"]);
    fixture.ok(&["wait", "1"]);
    fixture.stop();
    let _ = std::fs::remove_dir(&outside);
    let original = files(&fixture.root.join("source"));
    fixture.ok(&[
        "state",
        "backup",
        "--source",
        "source",
        "--destination",
        "backup",
    ]);
    fixture.ok(&[
        "state",
        "restore",
        "--source",
        "backup",
        "--destination",
        "restored",
    ]);
    assert_eq!(files(&fixture.root.join("restored")), original);
    let archived = fixture.root.join("source/jobs/1/attempts/1/job.json");
    let mut invalid: serde_json::Value =
        serde_json::from_slice(&fs::read(&archived).unwrap()).unwrap();
    assert_eq!(
        invalid["result"]["isolation_controls"]["root_read_only"],
        true
    );
    invalid["result"]["isolation_controls"]["namespaces"] = serde_json::json!(["uts"]);
    fs::write(&archived, serde_json::to_vec(&invalid).unwrap()).unwrap();
    let check = fixture.cli("source", &["state", "validate", "--source", "source"]);
    assert!(!check.status.success());
    assert!(String::from_utf8_lossy(&check.stderr).contains("invalid applied isolation controls"));
    fixture.start("restored");
    let output = fixture.cli("restored", &["status", "1", "--json"]);
    assert!(output.status.success());
    let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        record["result"]["isolation_controls"],
        serde_json::json!({"namespaces":["mount","user","uts"],"user_namespace_from_network":false,"root_read_only":true,"private_tmp":record["result"]["isolation_controls"]["private_tmp"],"writable":[]})
    );
    assert_eq!(
        record["result"]["isolation_controls"]["private_tmp"][0],
        "/tmp"
    );
}

#[test]
fn store_without_isolation_fields_loads_unchanged() {
    let mut fixture = Fixture::new("isolation-absent");
    fixture.ok(&["run", "--", "true"]);
    fixture.stop();
    let path = fixture.root.join("source/jobs/1/job.json");
    let mut record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for key in ["spec", "submitted_spec", "requested_spec", "effective_spec"] {
        if let Some(declared) = record[key]["declared"].as_object_mut() {
            declared.retain(|key, _| {
                !["namespaces", "root", "private_tmp", "writable"].contains(&key.as_str())
            });
        }
    }
    record["result"]
        .as_object_mut()
        .unwrap()
        .remove("isolation_controls");
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let before = files(&fixture.root.join("source"));
    fixture.ok(&["state", "validate", "--source", "source"]);
    assert_eq!(before, files(&fixture.root.join("source")));
    fixture.start("source");
    let output = fixture.cli("source", &["status", "1", "--json"]);
    assert!(output.status.success());
    let record: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(record["spec"]["declared"]["namespaces"].is_null());
    assert!(record["spec"]["declared"]["root"].is_null());
    assert!(record["result"]["isolation_controls"].is_null());
}

#[test]
fn output_quota_recordings_survive_backup_and_restore_and_reject_a_changed_quota() {
    let mut fixture = Fixture::new("output-quota-backup");
    fixture.ok(&[
        "run",
        "--output-head",
        "1M",
        "--output-tail",
        "1M",
        "--",
        "sh",
        "-c",
        "head -c 4500000 /dev/zero | tr '\\0' a; printf err >&2",
    ]);
    fixture.ok(&["retry", "1"]);
    fixture.ok(&["wait", "1"]);
    fixture.stop();
    let original = files(&fixture.root.join("source"));
    fixture.ok(&[
        "state",
        "backup",
        "--source",
        "source",
        "--destination",
        "backup",
    ]);
    fixture.ok(&[
        "state",
        "restore",
        "--source",
        "backup",
        "--destination",
        "restored",
    ]);
    assert_eq!(files(&fixture.root.join("restored")), original);
    let archived = fixture.root.join("source/jobs/1/attempts/1/streams.json");
    let mut changed: serde_json::Value =
        serde_json::from_slice(&fs::read(&archived).unwrap()).unwrap();
    assert_eq!(changed["quota"]["head_bytes"], 1_048_576);
    assert!(changed["retired_records"].as_u64().unwrap() > 0);
    changed["quota"]["tail_bytes"] = 2_097_152.into();
    fs::write(&archived, serde_json::to_vec(&changed).unwrap()).unwrap();
    let check = fixture.cli("source", &["state", "validate", "--source", "source"]);
    assert!(!check.status.success());
    assert!(String::from_utf8_lossy(&check.stderr).contains("invalid output recording"));
    fixture.start("restored");
    let output = fixture.cli(
        "restored",
        &["logs", "1", "--attempt", "1", "--raw", "--stream", "stdout"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.len() > 2_000_000 && output.stdout.len() < 4_500_000);
    assert!(String::from_utf8_lossy(&output.stderr).contains("stream=stdout"));
    let record: serde_json::Value =
        serde_json::from_slice(&fixture.cli("restored", &["status", "1", "--json"]).stdout)
            .unwrap();
    assert_eq!(record["spec"]["declared"]["output_head_bytes"], 1_048_576);
    assert_eq!(record["resource_sources"]["output_head_bytes"], "Job");
}

fn backup_and_restore(fixture: &Fixture) {
    fixture.ok(&[
        "state",
        "backup",
        "--source",
        "source",
        "--destination",
        "backup",
    ]);
    fixture.ok(&[
        "state",
        "restore",
        "--source",
        "backup",
        "--destination",
        "restored",
    ]);
}

fn modern(fixture: &Fixture, directory: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_job"))
        .args(args)
        .env_remove("JOB_CLI_COMPAT")
        .env("LC_ALL", "C")
        .env("JOB_STATE_DIR", fixture.root.join(directory))
        .current_dir(&fixture.root)
        .output()
        .unwrap()
}

#[test]
fn stream_recordings_survive_backup_restore_and_are_served_by_a_running_daemon() {
    let mut fixture = Fixture::new("streams-backup");
    fixture.ok(&["run", "--", "echo recorded-out; echo recorded-err >&2"]);
    fixture.stop();
    let before = files(&fixture.root.join("source"));
    assert!(before.keys().any(|path| path.ends_with("streams.json")));
    backup_and_restore(&fixture);
    assert_eq!(files(&fixture.root.join("restored")), before);
    fixture.start("restored");
    let stdout = modern(
        &fixture,
        "restored",
        &["logs", "1", "--stream", "stdout", "--raw"],
    );
    assert!(
        stdout.status.success(),
        "{}",
        String::from_utf8_lossy(&stdout.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&stdout.stdout), "recorded-out\n");
    let stderr = modern(
        &fixture,
        "restored",
        &["logs", "1", "--stream", "stderr", "--raw"],
    );
    assert_eq!(String::from_utf8_lossy(&stderr.stdout), "recorded-err\n");
    let next = fixture.cli("restored", &["run", "--", "echo after-restore"]);
    assert!(next.status.success());
}

#[test]
fn resource_update_receipts_survive_backup_restore_and_a_daemon_start() {
    let mut fixture = Fixture::new("update-receipt");
    fixture.ok(&["run", "--", "true"]);
    fixture.stop();
    let receipt_id = "0123456789abcdef0123456789abcdef";
    let receipt = serde_json::json!({
        "schema_version": 1, "actor_uid": unsafe { libc::getuid() }, "requested_ms": 17,
        "plan": {
            "operation": receipt_id, "target": {"Job": {"id": 1}}, "object_id": null,
            "members": [{"id": 1, "attempt": 1}], "boot_id": "an-earlier-boot",
            "path": "/sys/fs/cgroup/gone", "device": 1, "inode": 2, "patch": {},
            "allow_oom": false, "before": {}, "after": {}, "config_before": null,
            "config_after": null, "steps": []
        },
        "phase": "Applied", "completed_steps": 0, "error": null
    });
    fs::create_dir(fixture.root.join("source/resource-updates")).unwrap();
    fs::write(
        fixture
            .root
            .join(format!("source/resource-updates/{receipt_id}.json")),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    let before = files(&fixture.root.join("source"));
    backup_and_restore(&fixture);
    assert_eq!(files(&fixture.root.join("restored")), before);
    fixture.start("restored");
    let shown = fixture.cli("restored", &["resource-update", receipt_id, "--json"]);
    assert!(
        shown.status.success(),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let shown: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert!(shown.to_string().contains(receipt_id));
    assert!(
        fixture
            .cli("restored", &["run", "--", "true"])
            .status
            .success()
    );
    fixture.stop();
    fs::write(
        fixture
            .root
            .join(format!("restored/resource-updates/{receipt_id}.json")),
        b"{}",
    )
    .unwrap();
    assert!(
        !fixture
            .cli("restored", &["state", "validate", "--source", "restored"])
            .status
            .success()
    );
}

#[test]
fn suspended_records_refuse_backup_and_held_records_are_copied_and_stay_usable() {
    let mut fixture = Fixture::new("held-suspended");
    let held = fixture.ok(&["create", "--", "true"]);
    assert_eq!(String::from_utf8_lossy(&held.stdout).trim(), "1");
    fixture.ok(&["run", "--", "true"]);
    fixture.stop();
    let before = files(&fixture.root.join("source"));
    let attempt = |fixture: &Fixture, destination: &str, copied: bool| {
        let inventory = fixture.ok(&["state", "validate", "--source", "source"]);
        let inventory: serde_json::Value = serde_json::from_slice(&inventory.stdout).unwrap();
        assert_eq!(inventory["conversion_ready"], false);
        let output = fixture.cli(
            "source",
            &[
                "state",
                "backup",
                "--source",
                "source",
                "--destination",
                destination,
            ],
        );
        assert_eq!(
            output.status.success(),
            copied,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fixture.root.join(destination).exists(), copied);
        inventory["active_jobs"].clone()
    };
    assert_eq!(
        attempt(&fixture, "held-backup", true),
        serde_json::json!([1])
    );
    assert_eq!(files(&fixture.root.join("held-backup/data")), before);
    assert_eq!(files(&fixture.root.join("source")), before);
    let converted = fixture.cli(
        "source",
        &[
            "state",
            "migrate",
            "--source",
            "source",
            "--destination",
            "converted",
            "--backup",
            "converted-backup",
            "--profile",
            "ordinary",
        ],
    );
    assert!(!converted.status.success());
    assert!(!fixture.root.join("converted").exists());
    let path = fixture.root.join("source/jobs/2/job.json");
    let finished = fs::read(&path).unwrap();
    let mut job: serde_json::Value = serde_json::from_slice(&finished).unwrap();
    job["state"] = serde_json::json!("Suspended");
    job["finished_ms"] = serde_json::Value::Null;
    job["result"] = serde_json::Value::Null;
    job["suspension"]["requested"] = serde_json::json!(true);
    fs::write(&path, serde_json::to_vec(&job).unwrap()).unwrap();
    assert_eq!(
        attempt(&fixture, "backup", false),
        serde_json::json!([1, 2])
    );
    fs::write(&path, finished).unwrap();
    fixture.start("source");
    fixture.ok(&["release", "1"]);
    assert!(fixture.cli("source", &["wait", "1"]).status.success());
    fixture.stop();
    backup_and_restore(&fixture);
    fixture.start("restored");
    assert!(fixture.cli("restored", &["status", "1"]).status.success());
}

#[test]
fn links_of_an_earlier_boot_survive_backup_restore_and_a_daemon_start() {
    let mut fixture = Fixture::new("links-backup");
    fixture.ok(&["run", "--", "true"]);
    fixture.stop();
    fs::create_dir(fixture.root.join("source/links")).unwrap();
    let process = serde_json::json!({"pid": 1, "start_ticks": 1, "boot_id": vec![7u8; 16]});
    let link =
        serde_json::json!({"name": "kept", "holder": process, "relay": process, "rate": null});
    fs::write(
        fixture.root.join("source/links/kept.json"),
        serde_json::to_vec(&link).unwrap(),
    )
    .unwrap();
    fs::write(fixture.root.join("source/links/kept.log"), b"link log\n").unwrap();
    let inventory = fixture.ok(&["state", "validate", "--source", "source"]);
    let inventory: serde_json::Value = serde_json::from_slice(&inventory.stdout).unwrap();
    assert_eq!(inventory["active_helpers"], serde_json::json!([]));
    let before = files(&fixture.root.join("source"));
    backup_and_restore(&fixture);
    assert_eq!(files(&fixture.root.join("restored")), before);
    fixture.start("restored");
    assert!(
        fixture
            .cli("restored", &["run", "--", "true"])
            .status
            .success()
    );
    assert!(fixture.root.join("restored/links/kept.json").exists());
}

#[test]
fn audit_and_idempotency_files_survive_backup_restore_and_keep_working() {
    let mut fixture = Fixture::new("audit-idempotency");
    let first = modern(
        &fixture,
        "source",
        &["submit", "--idempotency-key", "backup.key", "--", "true"],
    );
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(modern(&fixture, "source", &["wait", "1"]).status.success());
    fixture.stop();
    let before = files(&fixture.root.join("source"));
    assert!(before.contains_key(Path::new("audit/current.jsonl")));
    assert_eq!(
        before
            .keys()
            .filter(|path| path.starts_with("idempotency"))
            .count(),
        1
    );
    backup_and_restore(&fixture);
    assert_eq!(files(&fixture.root.join("restored")), before);
    fixture.start("restored");
    let replay = modern(
        &fixture,
        "restored",
        &["submit", "--idempotency-key", "backup.key", "--", "true"],
    );
    assert_eq!(String::from_utf8_lossy(&replay.stdout).trim(), "1");
    let audit = modern(&fixture, "restored", &["audit", "--json"]);
    let entries: Vec<serde_json::Value> = String::from_utf8_lossy(&audit.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["seq"], 1);
    assert_eq!(entries[1]["seq"], 2);
    assert_eq!(entries[1]["result"], "replayed");
    fixture.stop();
    let index = fs::read_dir(fixture.root.join("restored/idempotency"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let original = fs::read(&index).unwrap();
    fs::write(&index, b"{\"key\":\"other\",\"id\":1,\"digest\":\"x\"}").unwrap();
    let validate = |fixture: &Fixture| {
        fixture
            .cli("restored", &["state", "validate", "--source", "restored"])
            .status
            .success()
    };
    assert!(!validate(&fixture));
    fs::write(&index, original).unwrap();
    assert!(validate(&fixture));
    fs::write(fixture.root.join("restored/audit/notes.txt"), b"x").unwrap();
    assert!(!validate(&fixture));
    fs::remove_file(fixture.root.join("restored/audit/notes.txt")).unwrap();
    let mut journal = fs::OpenOptions::new()
        .append(true)
        .open(fixture.root.join("restored/audit/current.jsonl"))
        .unwrap();
    journal.write_all(b"{\"seq\":3,\"torn").unwrap();
    drop(journal);
    assert!(validate(&fixture));
}

#[test]
fn leftover_submission_stages_survive_backup_restore_and_are_swept_at_start() {
    let mut fixture = Fixture::new("stage-backup");
    fixture.ok(&["run", "--", "true"]);
    fixture.stop();
    let stage = fixture.root.join("source/.transactions/9-1-1");
    fs::create_dir_all(&stage).unwrap();
    fs::write(stage.join("job.json"), b"{\"half\":").unwrap();
    let before = files(&fixture.root.join("source"));
    backup_and_restore(&fixture);
    assert_eq!(files(&fixture.root.join("restored")), before);
    assert!(
        fixture
            .root
            .join("restored/.transactions/9-1-1/job.json")
            .exists()
    );
    fixture.start("restored");
    assert!(!fixture.root.join("restored/.transactions/9-1-1").exists());
    let created = fixture.cli("restored", &["create", "--", "true"]);
    assert_eq!(String::from_utf8_lossy(&created.stdout).trim(), "2");
}

fn answers(fixture: &Fixture, directory: &str, jobs: &[&str]) -> BTreeMap<String, Vec<u8>> {
    let mut commands: Vec<Vec<&str>> = vec![
        vec!["list", "--format", "json"],
        vec!["list"],
        vec!["queue", "list", "--format", "json"],
        vec!["group", "list", "--format", "json"],
        vec!["queue", "show", "team/build", "--format", "json"],
        vec!["queue", "show", "team/parked", "--format", "json"],
        vec!["audit", "--json"],
    ];
    for id in jobs {
        commands.push(vec!["show", id, "--format", "json"]);
        commands.push(vec!["show", id]);
        commands.push(vec!["attempts", id, "--format", "json"]);
        for stream in ["stdout", "stderr", "all"] {
            commands.push(vec!["logs", id, "--raw", "--stream", stream]);
        }
        commands.push(vec!["logs", id, "--json"]);
    }
    commands.push(vec!["logs", "2", "--attempt", "1", "--raw"]);
    commands.push(vec!["logs", "2", "--attempt", "2", "--raw"]);
    let mut found = BTreeMap::new();
    for command in commands {
        let output = modern(fixture, directory, &command);
        assert!(
            output.stderr.is_empty(),
            "{command:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut bytes = output.stdout;
        bytes.extend_from_slice(format!("\nexit {:?}", output.status.code()).as_bytes());
        found.insert(command.join(" "), bytes);
    }
    found
}

#[test]
fn a_destroyed_mixed_service_is_restored_from_backup_and_answers_identically() {
    let mut fixture = Fixture::new("full-restore");
    let ok = |fixture: &Fixture, directory: &str, args: &[&str]| {
        let output = modern(fixture, directory, args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    ok(&fixture, "source", &["group", "create", "team"]);
    ok(&fixture, "source", &["group", "create", "team/empty"]);
    ok(&fixture, "source", &["queue", "create", "team/build"]);
    ok(
        &fixture,
        "source",
        &[
            "queue",
            "set",
            "team/build",
            "--max-running",
            "2",
            "--priority",
            "5",
        ],
    );
    ok(&fixture, "source", &["queue", "create", "team/parked"]);
    ok(&fixture, "source", &["queue", "pause", "team/parked"]);
    let first = ok(
        &fixture,
        "source",
        &[
            "submit",
            "--",
            "sh",
            "-c",
            "printf 'first-out\\n'; printf 'first-err\\n' >&2",
        ],
    );
    assert_eq!(first, "1");
    assert!(modern(&fixture, "source", &["wait", "1"]).status.success());
    let failing = [
        "submit",
        "-q",
        "team/build",
        "--idempotency-key",
        "restore.key",
        "--",
        "sh",
        "-c",
        "printf 'attempt-out\\n'; date +%N >&2; exit 3",
    ];
    assert_eq!(ok(&fixture, "source", &failing), "2");
    assert_eq!(
        modern(&fixture, "source", &["wait", "2"]).status.code(),
        Some(3)
    );
    ok(&fixture, "source", &["retry", "2"]);
    assert_eq!(
        modern(&fixture, "source", &["wait", "2"]).status.code(),
        Some(3)
    );
    let held = ok(
        &fixture,
        "source",
        &["create", "-q", "team/build", "--", "sh", "-c", "echo held"],
    );
    assert_eq!(held, "3");
    let queued = ok(
        &fixture,
        "source",
        &[
            "submit",
            "-q",
            "team/parked",
            "--",
            "sh",
            "-c",
            "echo queued",
        ],
    );
    assert_eq!(queued, "4");
    let waiting = modern(&fixture, "source", &["list", "--format", "json"]);
    let waiting: serde_json::Value = serde_json::from_slice(&waiting.stdout).unwrap();
    let states: Vec<_> = waiting["data"]["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| (entry["job"]["id"].clone(), entry["job"]["state"].clone()))
        .collect();
    assert_eq!(
        serde_json::json!(states),
        serde_json::json!([[3, "Held"], [4, "Queued"]])
    );
    fixture.stop();
    let inventory = ok(
        &fixture,
        "source",
        &["state", "validate", "--source", "source"],
    );
    let inventory: serde_json::Value = serde_json::from_str(&inventory).unwrap();
    assert_eq!(inventory["active_jobs"], serde_json::json!([3, 4]));
    ok(
        &fixture,
        "source",
        &[
            "state",
            "backup",
            "--source",
            "source",
            "--destination",
            "waiting",
        ],
    );
    ok(
        &fixture,
        "source",
        &[
            "state",
            "restore",
            "--source",
            "waiting",
            "--destination",
            "rehearsal",
        ],
    );
    assert_eq!(
        files(&fixture.root.join("rehearsal")),
        files(&fixture.root.join("source"))
    );
    fixture.start("rehearsal");
    let waiting = modern(&fixture, "rehearsal", &["list", "--format", "json"]);
    let waiting: serde_json::Value = serde_json::from_slice(&waiting.stdout).unwrap();
    let states: Vec<_> = waiting["data"]["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| (entry["job"]["id"].clone(), entry["job"]["state"].clone()))
        .collect();
    assert_eq!(
        serde_json::json!(states),
        serde_json::json!([[3, "Held"], [4, "Queued"]])
    );
    ok(&fixture, "rehearsal", &["release", "3"]);
    assert!(
        modern(&fixture, "rehearsal", &["wait", "3"])
            .status
            .success()
    );
    assert_eq!(ok(&fixture, "rehearsal", &["logs", "3", "--raw"]), "held");
    ok(&fixture, "rehearsal", &["queue", "resume", "team/parked"]);
    assert!(
        modern(&fixture, "rehearsal", &["wait", "4"])
            .status
            .success()
    );
    assert_eq!(ok(&fixture, "rehearsal", &["logs", "4", "--raw"]), "queued");
    assert_eq!(ok(&fixture, "rehearsal", &["submit", "--", "true"]), "5");
    fixture.stop();
    fixture.start("source");
    ok(&fixture, "source", &["cancel", "3"]);
    ok(&fixture, "source", &["cancel", "4"]);
    let jobs = ["1", "2", "3", "4"];
    let before = answers(&fixture, "source", &jobs);
    assert_eq!(before.len(), 37);
    let text = |answers: &BTreeMap<String, Vec<u8>>, command: &str| {
        String::from_utf8(answers[command].clone()).unwrap()
    };
    assert_eq!(
        text(&before, "logs 1 --raw --stream stdout"),
        "first-out\n\nexit Some(0)"
    );
    assert_eq!(
        text(&before, "logs 1 --raw --stream stderr"),
        "first-err\n\nexit Some(0)"
    );
    assert_ne!(
        before["logs 2 --attempt 1 --raw"],
        before["logs 2 --attempt 2 --raw"]
    );
    assert!(text(&before, "logs 2 --attempt 1 --raw").starts_with("attempt-out\n"));
    let attempts: serde_json::Value = serde_json::from_str(
        text(&before, "attempts 2 --format json")
            .rsplit_once("\nexit")
            .unwrap()
            .0,
    )
    .unwrap();
    assert_eq!(attempts["data"]["attempts"].as_array().unwrap().len(), 2);
    assert!(text(&before, "queue show team/build --format json").contains("\"max_running\":2"));
    assert!(text(&before, "queue show team/parked --format json").contains("\"paused\":true"));
    assert!(text(&before, "group list --format json").contains("team/empty"));
    assert!(text(&before, "show 3 --format json").contains("\"state\":\"Cancelled\""));
    fixture.stop();
    let original = files(&fixture.root.join("source"));
    assert!(original.contains_key(Path::new("audit/current.jsonl")));
    assert!(original.keys().any(|path| path.starts_with("idempotency")));
    assert!(
        original
            .keys()
            .any(|path| path.starts_with("jobs/2/attempts"))
    );
    ok(
        &fixture,
        "source",
        &[
            "state",
            "backup",
            "--source",
            "source",
            "--destination",
            "backup",
        ],
    );
    fs::rename(fixture.root.join("source"), fixture.root.join("destroyed")).unwrap();
    for path in original.keys() {
        fs::remove_file(fixture.root.join("destroyed").join(path)).unwrap();
    }
    assert!(files(&fixture.root.join("destroyed")).is_empty());
    assert!(!fixture.root.join("source").exists());
    fs::create_dir(fixture.root.join("elsewhere")).unwrap();
    let restored = modern(
        &fixture,
        "elsewhere",
        &[
            "state",
            "restore",
            "--source",
            "backup",
            "--destination",
            "elsewhere/recovered",
        ],
    );
    assert!(
        restored.status.success(),
        "{}",
        String::from_utf8_lossy(&restored.stderr)
    );
    assert_eq!(files(&fixture.root.join("elsewhere/recovered")), original);
    fixture.start("elsewhere/recovered");
    let after = answers(&fixture, "elsewhere/recovered", &jobs);
    let moved = fixture.root.join("elsewhere/recovered");
    let lost = fixture.root.join("source");
    let mut relocated = 0;
    let after: BTreeMap<String, Vec<u8>> = after
        .into_iter()
        .map(|(command, bytes)| {
            let text = String::from_utf8(bytes).unwrap();
            relocated += text.matches(moved.to_str().unwrap()).count();
            (
                command,
                text.replace(moved.to_str().unwrap(), lost.to_str().unwrap())
                    .into_bytes(),
            )
        })
        .collect();
    assert_eq!(relocated, 9);
    for (command, bytes) in &before {
        let other = &after[command];
        let at = bytes
            .iter()
            .zip(other.iter())
            .position(|(left, right)| left != right)
            .unwrap_or(bytes.len().min(other.len()));
        let near = |bytes: &[u8]| {
            String::from_utf8_lossy(&bytes[at.saturating_sub(80)..bytes.len().min(at + 160)])
                .into_owned()
        };
        assert!(
            bytes == other,
            "{command} differs at byte {at}:\n before: {}\n after:  {}",
            near(bytes),
            near(other)
        );
    }
    assert_eq!(after, before);
    assert_eq!(ok(&fixture, "elsewhere/recovered", &failing), "2");
    let next = ok(
        &fixture,
        "elsewhere/recovered",
        &[
            "submit",
            "-q",
            "team/build",
            "--",
            "sh",
            "-c",
            "echo after-restore",
        ],
    );
    assert_eq!(next, "5");
    assert!(
        modern(&fixture, "elsewhere/recovered", &["wait", "5"])
            .status
            .success()
    );
    assert_eq!(
        ok(&fixture, "elsewhere/recovered", &["logs", "5", "--raw"]),
        "after-restore"
    );
    ok(&fixture, "elsewhere/recovered", &["retry", "2"]);
    assert_eq!(
        modern(&fixture, "elsewhere/recovered", &["wait", "2"])
            .status
            .code(),
        Some(3)
    );
    let attempts = ok(
        &fixture,
        "elsewhere/recovered",
        &["attempts", "2", "--format", "json"],
    );
    let attempts: serde_json::Value = serde_json::from_str(&attempts).unwrap();
    assert_eq!(attempts["data"]["attempts"].as_array().unwrap().len(), 3);
    let parked = ok(
        &fixture,
        "elsewhere/recovered",
        &["submit", "-q", "team/parked", "--", "true"],
    );
    let status = modern(
        &fixture,
        "elsewhere/recovered",
        &["status", &parked, "--json"],
    );
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["state"], "Queued");
    ok(&fixture, "elsewhere/recovered", &["cancel", &parked]);
}

fn ticks_of(pid: u64) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    if fields.first() == Some(&"Z") {
        return None;
    }
    fields.get(19)?.parse().ok()
}

fn end_process(pid: u64, ticks: u64) {
    if ticks_of(pid) != Some(ticks) {
        return;
    }
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    let deadline = Instant::now() + Duration::from_secs(10);
    while ticks_of(pid) == Some(ticks) {
        assert!(Instant::now() < deadline, "process {pid} did not end");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn an_interrupted_store_is_backed_up_on_request_and_its_executions_are_recorded_as_lost() {
    let mut fixture = Fixture::new("interrupted");
    let ok = |fixture: &Fixture, directory: &str, args: &[&str]| {
        let output = modern(fixture, directory, args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    let status = |fixture: &Fixture, directory: &str, id: &str| -> serde_json::Value {
        serde_json::from_slice(&modern(fixture, directory, &["status", id, "--json"]).stdout)
            .unwrap()
    };
    ok(&fixture, "source", &["queue", "create", "parked"]);
    ok(&fixture, "source", &["queue", "pause", "parked"]);
    assert_eq!(
        ok(
            &fixture,
            "source",
            &[
                "submit",
                "--",
                "sh",
                "-c",
                "echo began; echo $$ > workload.pid; exec sleep 40",
            ],
        ),
        "1"
    );
    assert_eq!(
        ok(
            &fixture,
            "source",
            &["submit", "-q", "parked", "--", "sh", "-c", "echo waited"],
        ),
        "2"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let workload = loop {
        if let Some(pid) = fs::read_to_string(fixture.root.join("workload.pid"))
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
        {
            break pid;
        }
        assert!(Instant::now() < deadline, "the workload did not start");
        std::thread::sleep(Duration::from_millis(10));
    };
    let workload = (workload, ticks_of(workload).unwrap());
    let running = status(&fixture, "source", "1");
    assert_eq!(running["state"], "Running");
    let supervisor = (
        running["shim_pid"].as_u64().unwrap(),
        running["shim_start_ticks"].as_u64().unwrap(),
    );
    fixture.stop();
    let backup = |destination: &str, interrupted: bool| {
        let mut args = vec![
            "state",
            "backup",
            "--source",
            "source",
            "--destination",
            destination,
        ];
        if interrupted {
            args.push("--interrupted");
        }
        modern(&fixture, "source", &args)
    };
    let refused = backup("plain", false);
    assert_eq!(refused.status.code(), Some(125));
    let text = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(
        text.contains("Jobs recorded as starting, running, suspended or stopping: 1;")
            && text.contains("--interrupted"),
        "{text}"
    );
    assert!(!fixture.root.join("plain").exists());
    let alive = backup("alive", true);
    assert_eq!(alive.status.code(), Some(125));
    let text = String::from_utf8_lossy(&alive.stderr).into_owned();
    assert!(
        text.contains("the supervisors of these Jobs are still running: 1;"),
        "{text}"
    );
    assert!(!fixture.root.join("alive").exists());
    end_process(supervisor.0, supervisor.1);
    end_process(workload.0, workload.1);
    let refused = backup("plain", false);
    assert_eq!(refused.status.code(), Some(125));
    assert!(!fixture.root.join("plain").exists());
    let copied = backup("backup", true);
    assert!(
        copied.status.success(),
        "{}",
        String::from_utf8_lossy(&copied.stderr)
    );
    let original = files(&fixture.root.join("source"));
    ok(
        &fixture,
        "source",
        &[
            "state",
            "restore",
            "--source",
            "backup",
            "--destination",
            "restored",
        ],
    );
    assert_eq!(files(&fixture.root.join("restored")), original);
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.root.join("restored/jobs/1/job.json")).unwrap())
            .unwrap();
    assert_eq!(record["state"], "Running");
    fixture.start("restored");
    let deadline = Instant::now() + Duration::from_secs(10);
    let lost = loop {
        let found = status(&fixture, "restored", "1");
        if found["state"] != "Running" {
            break found;
        }
        assert!(Instant::now() < deadline, "{found}");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(lost["state"], "Lost");
    assert_eq!(lost["stop"]["kind"], "DaemonLost");
    assert_eq!(
        lost["stop"]["line"],
        "lost: its supervisor ended without writing a result"
    );
    assert_eq!(
        modern(&fixture, "restored", &["wait", "1", "--timeout", "2s"])
            .status
            .code(),
        Some(125)
    );
    assert_eq!(status(&fixture, "restored", "2")["state"], "Queued");
    ok(&fixture, "restored", &["queue", "resume", "parked"]);
    assert!(
        modern(&fixture, "restored", &["wait", "2"])
            .status
            .success()
    );
    assert_eq!(ok(&fixture, "restored", &["logs", "2", "--raw"]), "waited");
    assert_eq!(ok(&fixture, "restored", &["submit", "--", "true"]), "3");
}
