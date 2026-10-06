use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const JOB: &str = env!("CARGO_BIN_EXE_job");

const ROUNDS: usize = 50;

const CHURN: &str = "n=0; trap 'n=$((n+1)); echo $n > trapped' USR1; echo ready > ready; i=0; while [ $i -lt 20000 ]; do /usr/bin/true & /usr/bin/true & wait; i=$((i+1)); echo $i > progress; done";

const BYSTANDER: &str = "echo ready > ready; i=0; while [ $i -lt 40000 ] && [ ! -e stop ]; do /usr/bin/true; rc=$?; [ $rc -eq 0 ] || echo child-$rc >> hits; i=$((i+1)); echo $i > progress; done; echo $i > done";

const NAMESPACE: [&str; 6] = [
    "--user",
    "--map-root-user",
    "--pid",
    "--fork",
    "--kill-child",
    "--mount-proc",
];

fn narrow_namespace_refusal() -> Option<String> {
    let probe = Command::new("unshare")
        .args(NAMESPACE)
        .args([
            "sh",
            "-c",
            "echo 400 > /proc/sys/kernel/pid_max && [ $(cat /proc/sys/kernel/pid_max) -eq 400 ]",
        ])
        .output();
    match probe {
        Err(error) => Some(format!("unshare cannot be run: {error}")),
        Ok(output) if !output.status.success() => Some(format!(
            "a private process namespace with its own pid_max is not available: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Ok(_) => None,
    }
}

struct Scene {
    base: PathBuf,
    daemon: Child,
    bystander: Option<Child>,
}

impl Scene {
    fn start(name: &str, narrow: bool) -> Self {
        let base = std::env::temp_dir().join(format!("job-churn-{}-{name}", std::process::id()));
        for directory in ["work", "bystander"] {
            std::fs::create_dir_all(base.join(directory)).unwrap();
        }
        std::fs::write(
            base.join("config.toml"),
            "schema_version = 1\nprofile = 'ordinary'\n",
        )
        .unwrap();
        let mut command = if narrow {
            let mut command = Command::new("unshare");
            command.args(NAMESPACE).args([
                "sh",
                "-c",
                &format!(
                    "echo 400 > /proc/sys/kernel/pid_max || exit 90; (cd '{}' && exec sh -c \"$BYSTANDER\") & exec '{JOB}' daemon",
                    base.join("bystander").display()
                ),
            ]);
            command.env("BYSTANDER", BYSTANDER);
            command
        } else {
            let mut command = Command::new(JOB);
            command.arg("daemon");
            command
        };
        let daemon = command
            .env("JOB_CGROUP_ROOT", base.join("absent-cgroup"))
            .env("JOB_CONFIG", base.join("config.toml"))
            .env("JOB_STATE_DIR", base.join("state"))
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let bystander = (!narrow).then(|| {
            Command::new("sh")
                .args(["-c", BYSTANDER])
                .current_dir(base.join("bystander"))
                .stdin(Stdio::null())
                .spawn()
                .unwrap()
        });
        let mut scene = Self {
            base,
            daemon,
            bystander,
        };
        scene.ready();
        scene
    }

    fn ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            assert!(
                self.daemon.try_wait().unwrap().is_none(),
                "daemon exited during startup"
            );
            if self
                .call(serde_json::json!("Ping"))
                .is_some_and(|value| value.get("Pong").is_some())
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("daemon did not become ready");
    }

    fn call(&self, request: serde_json::Value) -> Option<serde_json::Value> {
        let mut stream = UnixStream::connect(self.base.join("state/daemon.sock")).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let request = if request.is_string() {
            request
        } else {
            serde_json::json!({"Versioned": {"protocol": 20, "request": request}})
        };
        writeln!(stream, "{request}").ok()?;
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn job(&self, args: &[&str]) -> Output {
        Command::new(JOB)
            .args(args)
            .env_remove("JOB_CLI_COMPAT")
            .env("JOB_STATE_DIR", self.base.join("state"))
            .env("JOB_SESSION", "churn")
            .env("LC_ALL", "C")
            .current_dir(self.base.join("work"))
            .output()
            .unwrap()
    }

    fn number(&self, file: &str) -> Option<usize> {
        for _ in 0..200 {
            if let Some(number) = std::fs::read_to_string(self.base.join(file))
                .ok()
                .and_then(|text| text.trim().parse().ok())
            {
                return Some(number);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    fn wait_file(&self, file: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !self.base.join(file).exists() {
            assert!(Instant::now() < deadline, "missing {file}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = std::fs::write(self.base.join("bystander/stop"), b"");
        let _ = self.job(&["cancel", "1"]);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !inside(&self.base.join("work")).is_empty() {
            std::thread::sleep(Duration::from_millis(20));
        }
        if let Some(bystander) = &mut self.bystander {
            let _ = bystander.kill();
            let _ = bystander.wait();
        }
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn inside(directory: &Path) -> Vec<u32> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if std::fs::read_link(entry.path().join("cwd")).is_ok_and(|cwd| cwd == directory) {
            found.push(pid);
        }
    }
    found
}

fn signals_reach_only_the_job(name: &str, narrow: bool) {
    let mut scene = Scene::start(name, narrow);
    let work = scene.base.join("work");
    let submitted = scene.job(&["submit", "--time", "90s", "--", "sh", "-c", CHURN]);
    assert!(
        submitted.status.success(),
        "{}",
        String::from_utf8_lossy(&submitted.stderr)
    );
    let id: u64 = String::from_utf8_lossy(&submitted.stdout)
        .trim()
        .parse()
        .unwrap();
    assert_eq!(id, 1);
    scene.wait_file("work/ready");
    scene.wait_file("bystander/ready");
    let mut delivered = 0;
    for round in 0..ROUNDS {
        if round % 5 == 0 {
            let sent = scene.job(&["signal", "-s", "USR1", "1"]);
            assert!(
                sent.status.success(),
                "{}",
                String::from_utf8_lossy(&sent.stderr)
            );
            assert!(sent.stdout.is_empty());
        } else {
            let response = scene
                .call(serde_json::json!({"Signal": {"id": id, "signal": libc::SIGUSR1}}))
                .unwrap();
            let count = response["Signalled"]["delivered"]
                .as_u64()
                .unwrap_or_else(|| panic!("{response}"));
            assert!(
                (1..=8).contains(&count),
                "round {round} reported {count} deliveries for a shell and two children"
            );
            delivered += count;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let children = scene.number("work/progress").unwrap() * 2;
    let trapped = scene.number("work/trapped").unwrap();
    let others = scene.number("bystander/progress").unwrap();
    assert!(
        !scene.base.join("bystander/done").exists(),
        "the bystander ended before the last signal"
    );
    assert!(
        children >= 300,
        "only {children} children during the signals"
    );
    assert!(others >= 100, "only {others} bystander children");
    if narrow {
        assert!(
            children + others >= 400,
            "{children} and {others} children did not wrap a range of 100 process IDs"
        );
    }
    assert!(
        (ROUNDS * 3 / 4..=ROUNDS).contains(&trapped),
        "the payload counted {trapped} of {ROUNDS} signals"
    );
    assert!(delivered >= (ROUNDS - ROUNDS / 5) as u64);
    assert!(!inside(&work).is_empty());
    let cancelled = scene.job(&["cancel", "1"]);
    assert!(
        cancelled.status.success(),
        "{}",
        String::from_utf8_lossy(&cancelled.stderr)
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let status: serde_json::Value =
            serde_json::from_slice(&scene.job(&["status", "1", "--json"]).stdout).unwrap();
        if status["state"] == "Cancelled" && inside(&work).is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "after cancel: state {}, processes left {:?}",
            status["state"],
            inside(&work)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let after = std::fs::read(scene.base.join("work/progress")).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        std::fs::read(scene.base.join("work/progress")).unwrap(),
        after
    );
    assert!(
        !scene.base.join("bystander/done").exists(),
        "the bystander ended before the cancellation"
    );
    assert!(!inside(&scene.base.join("bystander")).is_empty());
    std::fs::write(scene.base.join("bystander/stop"), b"").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let alive = !inside(&scene.base.join("bystander")).is_empty();
        if scene.base.join("bystander/done").exists() {
            break;
        }
        assert!(
            Instant::now() < deadline && alive,
            "the bystander ended without finishing its loop; a signal ended it"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    if let Some(bystander) = &mut scene.bystander {
        assert!(bystander.wait().unwrap().success());
    }
    let hits = std::fs::read_to_string(scene.base.join("bystander/hits")).unwrap_or_default();
    assert!(hits.is_empty(), "the bystander was signalled: {hits}");
    println!(
        "{name}: {children} job children and {others} bystander children during {ROUNDS} signals, payload counted {trapped}, service reported {delivered} deliveries in {} wire rounds",
        ROUNDS - ROUNDS / 5
    );
}

#[test]
fn signals_under_rapid_process_churn_reach_only_the_job_and_cancel_ends_its_tree() {
    signals_reach_only_the_job("host", false);
}

#[test]
fn signals_with_process_ids_reused_every_hundred_children_spare_a_bystander() {
    if let Some(reason) = narrow_namespace_refusal() {
        println!("skipped: {reason}");
        return;
    }
    signals_reach_only_the_job("narrow", true);
}
