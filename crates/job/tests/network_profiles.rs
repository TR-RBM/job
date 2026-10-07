use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const NETPROBE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/netprobe.py");
const SOCKS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/socks5.py");
const LINK_TOOLS: [&str; 9] = [
    "python3",
    "curl",
    "slirp4netns",
    "nft",
    "tc",
    "ip",
    "unshare",
    "nsenter",
    "sleep",
];
const NAMESPACE_TOOLS: [&str; 5] = ["python3", "curl", "ip", "unshare", "sleep"];

struct Daemon {
    child: Child,
    base: PathBuf,
    state: PathBuf,
    work: PathBuf,
}

impl Daemon {
    fn area(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("job-net-{}-{name}", std::process::id()));
        std::fs::create_dir_all(base.join("work")).unwrap();
        base
    }

    fn start(name: &str, network: &str) -> Daemon {
        let base = Self::area(name);
        Self::configure(&base, network);
        let child = Self::spawn(&base);
        let mut daemon = Daemon {
            child,
            state: base.join("state"),
            work: base.join("work"),
            base,
        };
        daemon.ready();
        daemon
    }

    fn configure(base: &Path, network: &str) {
        std::fs::write(
            base.join("config.toml"),
            format!("schema_version = 1\nprofile = 'legacy'\n{network}"),
        )
        .unwrap();
    }

    fn spawn(base: &Path) -> Child {
        let mut command = Command::new(env!("CARGO_BIN_EXE_job"));
        if let Ok(path) = std::fs::read_to_string(base.join("path")) {
            command.env("PATH", path);
        }
        command
            .arg("daemon")
            .env("JOB_CGROUP_ROOT", base.join("absent-cgroup"))
            .env("JOB_CONFIG", base.join("config.toml"))
            .env("JOB_STATE_DIR", base.join("state"))
            .env("LC_ALL", "C")
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                std::fs::File::create(base.join("daemon.err")).unwrap(),
            ))
            .spawn()
            .unwrap()
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(self.state.join("daemon.sock"));
    }

    fn restart(&mut self) {
        self.stop();
        self.child = Self::spawn(&self.base);
        self.ready();
    }

    fn answers(&self) -> bool {
        let Ok(mut stream) = UnixStream::connect(self.state.join("daemon.sock")) else {
            return false;
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut response = String::new();
        writeln!(stream, "\"Ping\"").is_ok()
            && stream.read_to_string(&mut response).is_ok()
            && response.contains("Pong")
    }

    fn ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "daemon exited during startup: {}",
                self.errors()
            );
            if self.answers() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("daemon did not become ready");
    }

    fn errors(&self) -> String {
        std::fs::read_to_string(self.base.join("daemon.err")).unwrap_or_default()
    }

    fn job(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_job"))
            .args(args)
            .env("JOB_CLI_COMPAT", "legacy")
            .env("JOB_STATE_DIR", &self.state)
            .env("JOB_SESSION", "integration")
            .env("LC_ALL", "C")
            .current_dir(&self.work)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.job(args);
        assert!(output.status.success(), "{args:?}: {}", both(&output));
        text(&output)
    }

    fn refused(&self, args: &[&str]) -> String {
        let output = self.job(args);
        assert!(!output.status.success(), "{args:?}: {}", both(&output));
        both(&output)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.work.join(name)
    }

    fn record(&self, id: &str) -> serde_json::Value {
        serde_json::from_slice(&self.job(&["status", id, "--json"]).stdout).unwrap()
    }

    fn host(&self) -> serde_json::Value {
        serde_json::from_slice(&self.job(&["host", "--json"]).stdout).unwrap()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn both(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(tool).is_file()))
}

fn missing(tools: &[&str]) -> Option<String> {
    if let Some(missing) = tools.iter().find(|t| !on_path(t)) {
        return Some(format!("{missing} is not installed"));
    }
    let namespaces = Command::new("unshare")
        .args(["--user", "--map-root-user", "--net", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    (!namespaces).then(|| "this host does not let a user create namespaces".to_string())
}

fn held_id(answer: &str) -> String {
    answer
        .split(|c: char| !c.is_ascii_digit())
        .find(|word| !word.is_empty())
        .unwrap_or_else(|| panic!("no Job ID in {answer}"))
        .to_string()
}

fn free_port(offset: u32) -> String {
    (41000 + (std::process::id() * 7 + offset) % 20000).to_string()
}

struct Helper(Child);

impl Helper {
    fn start(program: &str, args: &[&str], dir: &Path) -> Helper {
        Helper(
            Command::new(program)
                .args(args)
                .current_dir(dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    fn stop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.stop();
    }
}

fn appears(path: &Path, seconds: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn listens(port: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("nothing listens on 127.0.0.1:{port}");
}

fn web_server(dir: &Path, port: &str) -> Helper {
    std::fs::write(dir.join("page.txt"), "served from outside\n").unwrap();
    let helper = Helper::start(
        "python3",
        &["-m", "http.server", "--bind", "127.0.0.1", port],
        dir,
    );
    listens(port);
    helper
}

fn udp_listener(dir: &Path, port: &str) -> (Helper, PathBuf) {
    let count = dir.join(format!("udp-{port}.count"));
    let helper = Helper::start(
        "python3",
        &[NETPROBE, "listen", port, count.to_str().unwrap(), "30"],
        dir,
    );
    assert!(appears(&count, 5), "the UDP listener did not start");
    (helper, count)
}

fn received(count: &Path) -> u64 {
    std::fs::read_to_string(count)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

struct Held {
    helper: Helper,
    net: PathBuf,
    user: PathBuf,
}

impl Held {
    fn start(dir: &Path, name: &str) -> Held {
        let helper = Helper::start(
            "unshare",
            &[
                "--user",
                "--map-root-user",
                "--net",
                "sh",
                "-c",
                "ip link set lo up; exec sleep 120",
            ],
            dir,
        );
        let pid = helper.0.id();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !std::fs::read_link(format!("/proc/{pid}/exe"))
            .is_ok_and(|exe| exe.file_name().is_some_and(|n| n == "sleep"))
        {
            assert!(
                Instant::now() < deadline,
                "the namespace holder did not start"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let net = dir.join(format!("{name}.net"));
        let user = dir.join(format!("{name}.user"));
        std::os::unix::fs::symlink(format!("/proc/{pid}/ns/net"), &net).unwrap();
        std::os::unix::fs::symlink(format!("/proc/{pid}/ns/user"), &user).unwrap();
        Held { helper, net, user }
    }

    fn namespace(&self) -> String {
        std::fs::read_link(format!("/proc/{}/ns/net", self.helper.0.id()))
            .unwrap()
            .to_string_lossy()
            .to_string()
    }
}

fn namespaces(base: &Path, held: &Held) -> String {
    let resolver = base.join("lab.resolv.conf");
    std::fs::write(&resolver, "nameserver 192.0.2.53\n").unwrap();
    let own = base.join("host.net");
    std::os::unix::fs::symlink(format!("/proc/{}/ns/net", std::process::id()), &own).unwrap();
    let plain = base.join("plain.file");
    std::fs::write(&plain, "not a namespace\n").unwrap();
    format!(
        "[network.namespaces.lab]\npath = '{net}'\nuser_namespace = '{user}'\nresolv_conf = '{resolver}'\ndescription = 'the test laboratory'\n\
         [network.namespaces.bare]\npath = '{net}'\n\
         [network.namespaces.rooted]\npath = '{own}'\n\
         [network.namespaces.plain]\npath = '{plain}'\n\
         [network.namespaces.swapped]\npath = '{user}'\n\
         [network.namespaces.absent]\n",
        net = held.net.display(),
        user = held.user.display(),
        resolver = resolver.display(),
        own = own.display(),
        plain = plain.display(),
    )
}

#[test]
fn network_namespace_jobs_join_the_namespace_the_configuration_names_and_share_it() {
    if let Some(reason) = missing(&NAMESPACE_TOOLS) {
        eprintln!("skipped: {reason}");
        return;
    }
    let base = Daemon::area("ns-join");
    let held = Held::start(&base, "lab");
    let daemon = Daemon::start("ns-join", &namespaces(&base, &held));
    let port = free_port(1);
    std::fs::write(daemon.file("inside.txt"), "served inside the namespace\n").unwrap();
    let server = text(&daemon.job(&[
        "submit",
        "--net",
        "ns:lab",
        "--",
        &format!(
            "python3 -m http.server --bind 127.0.0.1 {port} >/dev/null 2>&1 & \
             n=0; while [ ! -e stop ] && [ $n -lt 300 ]; do sleep 0.1; n=$((n+1)); done; kill $!"
        ),
    ]))
    .trim()
    .to_string();
    let answer = daemon.ok(&[
        "run",
        "--net",
        "ns:lab",
        "--",
        &format!(
            "echo namespace=$(readlink /proc/self/ns/net); echo uid=$(id -u); cat /etc/resolv.conf; \
             n=0; until curl -sS -m 2 http://127.0.0.1:{port}/inside.txt || [ $n -ge 30 ]; do sleep 0.2; n=$((n+1)); done; \
             echo interfaces=$(tail -n +3 /proc/net/dev | cut -d: -f1 | tr -d ' ' | tr '\\n' ' ')"
        ),
    ]);
    std::fs::write(daemon.file("stop"), "").unwrap();
    daemon.job(&["wait", &server, "--timeout", "40s"]);
    assert!(
        answer.contains(&format!("namespace={}\n", held.namespace())),
        "{answer}"
    );
    assert!(
        answer.contains(&format!("uid={}\n", unsafe { libc::getuid() })),
        "{answer}"
    );
    assert!(answer.contains("nameserver 192.0.2.53\n"), "{answer}");
    assert!(answer.contains("served inside the namespace"), "{answer}");
    assert!(answer.contains("interfaces=lo\n"), "{answer}");
    assert!(
        std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_err(),
        "the listener inside the namespace is reachable from the host"
    );
    let record = daemon.record(&server);
    assert_eq!(record["spec"]["declared"]["net"]["Namespace"], "lab");
    assert_eq!(record["network"]["selected"], "ns:lab");
    assert_eq!(record["network"]["scope"], "namespace");
    assert_eq!(
        record["network"]["namespace"]["path"],
        held.net.to_str().unwrap()
    );
    assert!(record["link"].is_null(), "{record}");
    assert!(
        !record["spec"].to_string().contains(".net"),
        "the declared network holds a path: {}",
        record["spec"]
    );
}

#[test]
fn network_namespace_a_queue_or_a_preset_names_the_namespace_for_its_jobs() {
    if let Some(reason) = missing(&NAMESPACE_TOOLS) {
        eprintln!("skipped: {reason}");
        return;
    }
    let base = Daemon::area("ns-queue");
    let held = Held::start(&base, "lab");
    let network = namespaces(&base, &held);
    let presets = "[[presets.profiles]]\nname = 'inlab'\nrevision = 1\n[presets.profiles.values]\nnet = 'ns:lab'\n";
    let daemon = Daemon::start("ns-queue", &format!("{presets}{network}"));
    daemon.ok(&["queue", "create", "labq", "--net", "ns:lab"]);
    let script = "echo namespace=$(readlink /proc/self/ns/net)";
    let expected = format!("namespace={}\n", held.namespace());
    let queued = daemon.ok(&["run", "-q", "labq", "--", script]);
    assert!(queued.contains(&expected), "{queued}");
    let preset = daemon.ok(&["run", "--execution-profile", "inlab@1", "--", script]);
    assert!(preset.contains(&expected), "{preset}");
    let other = daemon.refused(&["run", "-q", "labq", "--net", "none", "--", "true"]);
    assert!(other.contains("run it outside the queue"), "{other}");
    let unknown = daemon.refused(&["queue", "set", "labq", "--net", "ns:nowhere"]);
    assert!(
        unknown.contains("there is no network namespace nowhere"),
        "{unknown}"
    );
    let shaped = daemon.refused(&["queue", "set", "labq", "--bandwidth", "5Mbit"]);
    assert!(
        shaped.contains("does not own the devices of a namespace"),
        "{shaped}"
    );
    Daemon::configure(&daemon.base, presets);
    let dangling = daemon.refused(&["config", "reload"]);
    assert!(
        dangling.contains("profile:inlab@1: there is no network namespace lab"),
        "{dangling}"
    );
    Daemon::configure(&daemon.base, "");
    let reload = daemon.refused(&["config", "reload"]);
    assert!(
        reload.contains("labq is set to ns:lab")
            && reload.contains("there is no network namespace lab"),
        "{reload}"
    );
    let still = daemon.ok(&["run", "-q", "labq", "--", script]);
    assert!(still.contains(&expected), "{still}");
}

#[test]
fn network_namespace_refusals_name_the_reason() {
    if let Some(reason) = missing(&NAMESPACE_TOOLS) {
        eprintln!("skipped: {reason}");
        return;
    }
    let base = Daemon::area("ns-refuse");
    let mut held = Held::start(&base, "lab");
    let daemon = Daemon::start("ns-refuse", &namespaces(&base, &held));
    for (net, expected) in [
        ("ns:nowhere", "there is no network namespace nowhere"),
        ("ns:/proc/1/ns/net", "is not a name"),
        (
            "ns:rooted",
            "the service may not join this namespace: it belongs to a user namespace the service is not in; run the service with the needed privilege or give user_namespace",
        ),
        (
            "ns:bare",
            "the service may not join this namespace: it belongs to a user namespace the service is not in",
        ),
        ("ns:plain", "is not a namespace file"),
        ("ns:swapped", "is a namespace, but not a network namespace"),
        ("ns:absent", "cannot open /run/netns/absent"),
    ] {
        let said = daemon.refused(&["run", "--net", net, "--", "true"]);
        assert!(said.contains(expected), "{net}: {said}");
    }
    let shaped = daemon.refused(&[
        "run",
        "--net",
        "ns:lab",
        "--bandwidth",
        "5Mbit",
        "--",
        "true",
    ]);
    assert!(
        shaped.contains("job does not own that namespace's devices"),
        "{shaped}"
    );
    let secret = daemon.file("proxy.secret");
    std::fs::write(&secret, "user:password\n").unwrap();
    std::fs::set_permissions(&secret, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let credentials = daemon.refused(&[
        "run",
        "--net",
        "ns:lab",
        "--net-secret-file",
        secret.to_str().unwrap(),
        "--",
        "true",
    ]);
    assert!(credentials.contains("belongs to a proxy"), "{credentials}");

    let host = daemon.host();
    let listed = host["network"]["namespaces"].as_array().unwrap();
    let joinable = |name: &str| {
        listed
            .iter()
            .find(|entry| entry["name"] == name)
            .unwrap_or_else(|| panic!("{name} is not listed: {host}"))["joinable"]
            .as_bool()
            .unwrap()
    };
    assert!(joinable("lab"));
    for name in ["bare", "rooted", "plain", "swapped", "absent"] {
        assert!(!joinable(name), "{name}");
    }
    let list = daemon.ok(&["net", "list"]);
    assert!(
        list.contains("ns:lab") && list.contains("joinable"),
        "{list}"
    );
    assert!(
        list.contains("ns:rooted") && list.contains("not joinable"),
        "{list}"
    );
    let shown = daemon.ok(&["net", "show", "lab"]);
    assert!(
        shown.contains("the test laboratory")
            && shown.contains("shared with every Job that selects it")
            && shown.contains("job adds no filter"),
        "{shown}"
    );
    let doctor = both(&daemon.job(&["doctor"]));
    assert!(
        doctor.contains("network_namespaces") && doctor.contains("network namespace lab: joinable"),
        "{doctor}"
    );
    for kind in ["netns", "net"] {
        let completed = daemon.ok(&["__complete", kind, ""]);
        assert!(completed.contains("lab"), "{kind}: {completed}");
    }

    let id = held_id(&daemon.ok(&["create", "--net", "ns:lab", "--", "true"]));
    held.helper.stop();
    daemon.job(&["release", &id]);
    daemon.job(&["wait", &id, "--timeout", "30s"]);
    let record = daemon.record(&id);
    let error = record["result"]["start_error"].as_str().unwrap_or_default();
    assert!(
        error.contains("network namespace lab: cannot open"),
        "{record}"
    );

    let config = daemon.file("bad.toml");
    for (body, expected) in [
        (
            "[network.namespaces.x]\npath = '/proc/1/ns/net'\n",
            "outside /proc",
        ),
        (
            "[network.namespaces.x]\npath = '/run/netns/../x'\n",
            "without `..`",
        ),
        (
            "[network.namespaces.x]\npath = '/run/netns/x'\nfile = 'y'\n",
            "unknown field",
        ),
        (
            "[network.namespaces.'a b']\npath = '/run/netns/x'\n",
            "is not a name",
        ),
    ] {
        std::fs::write(&config, format!("schema_version = 1\n{body}")).unwrap();
        let said = daemon.refused(&["config", "check", config.to_str().unwrap()]);
        assert!(said.contains(expected), "{body}: {said}");
    }
}

fn files_holding(root: &Path, needle: &str, skip: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            files_holding(&path, needle, skip, found);
        } else if kind.is_file()
            && path != skip
            && std::fs::read(&path).is_ok_and(|bytes| {
                bytes
                    .windows(needle.len())
                    .any(|window| window == needle.as_bytes())
            })
        {
            found.push(path);
        }
    }
}

fn command_lines_holding(needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if let Ok(bytes) = std::fs::read(entry.path().join("cmdline")) {
            let line = String::from_utf8_lossy(&bytes).replace('\0', " ");
            if line.contains(needle) {
                found.push(format!("{} {line}", name.to_string_lossy()));
            }
        }
    }
    found
}

fn in_namespace(holder: u64, script: &str) -> Output {
    Command::new("nsenter")
        .args([
            "-t",
            &holder.to_string(),
            "-U",
            "-n",
            "--preserve-credentials",
            "--",
            "sh",
            "-e",
            "-c",
            script,
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn holders(daemon: &Daemon) -> Vec<(String, u64)> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(daemon.state.join("links")) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            let link: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let pid = link["holder"]["pid"].as_u64().unwrap();
            if Path::new(&format!("/proc/{pid}")).exists() {
                found.push((link["name"].as_str().unwrap().to_string(), pid));
            }
        }
    }
    found.sort();
    found
}

fn submitted(daemon: &Daemon, args: &[&str]) -> String {
    daemon.ok(args).trim().to_string()
}

fn profile_of<'a>(host: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    host["network"]["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("profile {name} is not listed: {host}"))
}

#[test]
fn network_profile_allow_and_deny_rules_decide_what_a_job_reaches() {
    if let Some(reason) = missing(&LINK_TOOLS) {
        eprintln!("skipped: {reason}");
        return;
    }
    let base = Daemon::area("profile-rules");
    let [open, closed, denied, datagram, udp_open, udp_closed] =
        [10, 11, 12, 13, 14, 15].map(free_port);
    let daemon = Daemon::start(
        "profile-rules",
        &format!(
            "[network.profiles.narrow]\negress = 'host'\ndescription = 'two doors'\n\
             allow = ['127.0.0.1:{open}/tcp', '127.0.0.1:{denied}', '127.0.0.1:{datagram}/udp', '127.0.0.1:{udp_open}/udp']\n\
             deny = ['127.0.0.1:{denied}/tcp']\ndns = ['192.0.2.53', '127.0.0.1']\n\
             [network.profiles.wide]\negress = 'host'\ndns = ['192.0.2.54']\n"
        ),
    );
    assert_eq!(daemon.base, base);
    let _servers: Vec<Helper> = [&open, &closed, &denied, &datagram]
        .iter()
        .map(|port| web_server(&daemon.work, port))
        .collect();
    let (_first, count_open) = udp_listener(&daemon.work, &udp_open);
    let (_second, count_closed) = udp_listener(&daemon.work, &udp_closed);
    let probe = format!(
        "cat /etc/resolv.conf; \
         curl -sS -m 5 http://198.18.0.2:{open}/page.txt; echo open=$?; \
         curl -sS -m 3 http://198.18.0.2:{closed}/page.txt; echo closed=$?; \
         curl -sS -m 3 http://198.18.0.2:{denied}/page.txt; echo denied=$?; \
         curl -sS -m 3 http://198.18.0.2:{datagram}/page.txt; echo datagram=$?; \
         python3 {NETPROBE} send 198.18.0.2 {udp_open} 5 100; \
         python3 {NETPROBE} send 198.18.0.2 {udp_closed} 5 100; \
         echo v6-routes=$(ip -6 route show default | wc -l)"
    );
    let id = submitted(
        &daemon,
        &["submit", "--net", "profile:narrow", "--", &probe],
    );
    daemon.job(&["wait", &id, "--timeout", "60s"]);
    std::thread::sleep(Duration::from_millis(300));
    let answer = daemon.ok(&["log", &id, "full"]);
    assert!(
        answer.contains("nameserver 192.0.2.53\nnameserver 198.18.0.2\n"),
        "{answer}"
    );
    assert!(answer.contains("served from outside\nopen=0"), "{answer}");
    assert_eq!(answer.matches("served from outside").count(), 1, "{answer}");
    for expected in ["closed=7", "denied=7", "datagram=7", "v6-routes=0"] {
        assert!(answer.contains(expected), "{expected} is missing: {answer}");
    }
    assert_eq!(received(&count_open), 500, "allowed UDP did not arrive");
    assert_eq!(
        received(&count_closed),
        0,
        "UDP passed a rule that names TCP ports only"
    );

    let record = daemon.record(&id);
    let host = daemon.host();
    let listed = profile_of(&host, "narrow");
    assert_eq!(record["spec"]["declared"]["net"]["Profile"], "narrow");
    assert_eq!(record["network"]["profile"], "narrow");
    assert_eq!(record["network"]["digest"], listed["digest"]);
    assert_eq!(record["network"]["digest"].as_str().unwrap().len(), 64);
    assert_eq!(record["network"]["scope"], "job");
    assert_eq!(record["network"]["rules"], listed["rules"]);
    let rules = record["network"]["rules"].to_string();
    for expected in [
        format!("ip daddr 198.18.0.2 tcp dport {denied} reject"),
        format!("ip daddr 198.18.0.2 tcp dport {open} accept"),
        format!("ip daddr 198.18.0.2 udp dport {udp_open} accept"),
    ] {
        assert!(rules.contains(&expected), "{expected} is missing: {rules}");
    }
    assert_eq!(listed["enforceable"], true, "{listed}");
    assert_eq!(listed["sharing"], "per-job");
    let explained = daemon.ok(&["explain", &id]);
    assert!(
        explained.contains("profile:narrow")
            && explained.contains(&record["network"]["digest"].as_str().unwrap()[..12])
            && explained.contains("one network per Job"),
        "{explained}"
    );
    let shown = daemon.ok(&["net", "show", "profile:narrow"]);
    assert!(
        shown.contains("two doors")
            && shown.contains(&format!("tcp dport {open} accept"))
            && shown.contains("enforceable"),
        "{shown}"
    );
    assert!(
        daemon
            .ok(&["__complete", "netprofile", ""])
            .contains("narrow")
    );

    let wide = daemon.ok(&[
        "run",
        "--net",
        "profile:wide",
        "--",
        &format!(
            "cat /etc/resolv.conf; curl -sS -m 5 http://198.18.0.2:{closed}/page.txt; echo wide=$?"
        ),
    ]);
    assert!(wide.contains("nameserver 192.0.2.54\n"), "{wide}");
    assert!(wide.contains("served from outside\nwide=0"), "{wide}");
    eprintln!(
        "not tested: resolving a name through the profile's resolver; port 53 cannot be bound without privilege here, so only the resolver file is asserted"
    );
}

#[test]
fn network_profile_sharing_decides_how_many_holders_serve_its_jobs() {
    if let Some(reason) = missing(&LINK_TOOLS) {
        eprintln!("skipped: {reason}");
        return;
    }
    let web = free_port(20);
    let inner = free_port(21);
    let daemon = Daemon::start(
        "profile-sharing",
        "[network.profiles.pool]\negress = 'host'\nsharing = 'shared'\nbandwidth = '1Mbit'\n\
         [network.profiles.each]\negress = 'host'\nsharing = 'per-job'\ndeny = ['192.0.2.0/24']\n\
         [network.profiles.lane]\negress = 'host'\nsharing = 'per-queue'\nbandwidth = '100Mbit'\n",
    );
    let _web = web_server(&daemon.work, &web);
    std::fs::write(daemon.file("blob.bin"), vec![b'x'; 250 * 1024]).unwrap();
    daemon.ok(&["queue", "create", "qa"]);
    daemon.ok(&["queue", "create", "qb"]);
    let waiting = |tag: &str, body: &str| {
        format!(
            "{body} touch {tag}.ready; n=0; while [ ! -e {tag}.go ] && [ $n -lt 300 ]; do sleep 0.1; n=$((n+1)); done; \
             date +%s.%N > {tag}.start; curl -sS -m 30 -o /dev/null http://198.18.0.2:{web}/blob.bin; echo {tag}-fetch=$?; date +%s.%N > {tag}.end"
        )
    };
    let listening = format!(
        "ip -4 -o addr show dev eth0 | awk '{{print $4}}' | cut -d/ -f1 > a.v4; echo neighbour > served.txt; \
         python3 -m http.server --bind 0.0.0.0 {inner} >/dev/null 2>&1 & \
         n=0; until curl -sS -m 2 http://127.0.0.1:{inner}/served.txt > a.self 2>/dev/null || [ $n -ge 30 ]; do sleep 0.2; n=$((n+1)); done;"
    );
    let first = submitted(
        &daemon,
        &[
            "submit",
            "-q",
            "qa",
            "--net",
            "profile:pool",
            "--",
            &waiting("a", &listening),
        ],
    );
    assert!(
        appears(&daemon.file("a.ready"), 30),
        "the first job did not start"
    );
    let address = std::fs::read_to_string(daemon.file("a.v4")).unwrap();
    let reach = format!(
        "curl -sS -m 3 http://{}:{inner}/served.txt; echo neighbour-reached=$?;",
        address.trim()
    );
    let second = submitted(
        &daemon,
        &[
            "submit",
            "-q",
            "qb",
            "--net",
            "profile:pool",
            "--",
            &waiting("b", &reach),
        ],
    );
    assert!(
        appears(&daemon.file("b.ready"), 30),
        "the second job did not start"
    );
    let shared = holders(&daemon);
    assert_eq!(shared.len(), 1, "{shared:?}");
    assert!(shared[0].0.starts_with("profile@pool@"), "{shared:?}");
    let (one, two) = (daemon.record(&first), daemon.record(&second));
    assert_eq!(one["link"]["name"], two["link"]["name"]);
    assert_eq!(one["link"]["name"], shared[0].0.as_str());
    assert_ne!(
        one["link"]["job"]["holder"]["pid"],
        two["link"]["job"]["holder"]["pid"]
    );
    assert_eq!(one["network"]["scope"], "profile");
    let relays = command_lines_holding(&format!(
        "slirp4netns --configure --mtu=1500 --cidr=198.18.0.0/24 --userns-path=/proc/{}/ns/user",
        shared[0].1
    ));
    assert_eq!(relays.len(), 1, "{relays:?}");
    std::fs::write(daemon.file("a.go"), "").unwrap();
    std::fs::write(daemon.file("b.go"), "").unwrap();
    daemon.job(&["wait", &first, "--timeout", "60s"]);
    daemon.job(&["wait", &second, "--timeout", "60s"]);
    assert_eq!(
        std::fs::read_to_string(daemon.file("a.self")).unwrap(),
        "neighbour\n"
    );
    let log = daemon.ok(&["log", &second, "full"]);
    assert!(
        !log.contains("neighbour\n"),
        "one Job read the other's page: {log}"
    );
    assert!(
        log.contains("neighbour-reached=") && !log.contains("neighbour-reached=0"),
        "{log}"
    );
    assert!(log.contains("b-fetch=0"), "{log}");
    assert!(daemon.ok(&["log", &first, "full"]).contains("a-fetch=0"));
    let stamp = |name: &str| -> f64 {
        std::fs::read_to_string(daemon.file(name))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    };
    let begun = stamp("a.start").min(stamp("b.start"));
    let ended = stamp("a.end").max(stamp("b.end"));
    let rate = 2.0 * 250.0 * 1024.0 * 8.0 / (ended - begun);
    assert!(
        (400_000.0..=1_300_000.0).contains(&rate),
        "two Jobs of the shared profile moved {rate} bit/s together under a budget of 1 Mbit/s"
    );
    assert!(holders(&daemon).is_empty(), "{:?}", holders(&daemon));

    let hold = |tag: &str| {
        format!(
            "touch {tag}.ready; n=0; while [ ! -e release ] && [ $n -lt 1200 ]; do sleep 0.1; n=$((n+1)); done"
        )
    };
    let mut ids = Vec::new();
    for (tag, queue, profile) in [
        ("c", "qa", "profile:each"),
        ("d", "qa", "profile:each"),
        ("e", "qa", "profile:lane"),
        ("f", "qa", "profile:lane"),
        ("g", "qb", "profile:lane"),
    ] {
        let id = submitted(
            &daemon,
            &["submit", "-q", queue, "--net", profile, "--", &hold(tag)],
        );
        assert!(
            appears(&daemon.file(&format!("{tag}.ready")), 120),
            "job {tag} did not start: {}",
            daemon.record(&id)
        );
        ids.push(id);
    }
    let names: Vec<String> = holders(&daemon).into_iter().map(|(name, _)| name).collect();
    let own: Vec<&String> = names
        .iter()
        .filter(|name| name.starts_with("job-"))
        .collect();
    let lanes: Vec<&String> = names
        .iter()
        .filter(|name| name.starts_with("profile@lane@"))
        .collect();
    assert_eq!(own.len(), 2, "{names:?}");
    assert_eq!(lanes.len(), 2, "{names:?}");
    assert_eq!(names.len(), 4, "{names:?}");
    let lane = |id: &str| {
        daemon.record(id)["link"]["name"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(lane(&ids[2]), lane(&ids[3]));
    assert_ne!(lane(&ids[2]), lane(&ids[4]));
    assert_eq!(daemon.record(&ids[2])["network"]["scope"], "queue");
    std::fs::write(daemon.file("release"), "").unwrap();
    for id in &ids {
        daemon.job(&["wait", id, "--timeout", "60s"]);
    }
    assert!(holders(&daemon).is_empty(), "{:?}", holders(&daemon));
}

#[test]
fn network_profile_egress_none_namespace_and_proxy_keep_credentials_out_of_sight() {
    if let Some(reason) = missing(&LINK_TOOLS) {
        eprintln!("skipped: {reason}");
        return;
    }
    let mark = ["SECRET", "MARK"].concat();
    let base = Daemon::area("profile-egress");
    let held = Held::start(&base, "lab");
    let web = free_port(30);
    let port = free_port(31);
    let secret = base.join("proxy.secret");
    std::fs::write(&secret, format!("user:{mark}\n")).unwrap();
    std::fs::set_permissions(&secret, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let daemon = Daemon::start(
        "profile-egress",
        &format!(
            "[network.namespaces.lab]\npath = '{net}'\nuser_namespace = '{user}'\n\
             [network.profiles.off]\negress = 'none'\n\
             [network.profiles.inlab]\negress = 'ns:lab'\n\
             [network.profiles.plain]\negress = 'host'\n\
             [network.profiles.capped]\negress = 'host'\nbandwidth = '10Mbit'\njob_bandwidth = '5Mbit'\n\
             [network.profiles.via]\negress = 'socks5://127.0.0.1:{port}'\nsecret_file = '{secret}'\nsharing = 'shared'\n\
             [network.profiles.open]\negress = 'socks5://127.0.0.1:{port}'\n",
            net = held.net.display(),
            user = held.user.display(),
            secret = secret.display(),
        ),
    );
    let _web = web_server(&daemon.work, &web);
    let log = daemon.file("socks.log");
    let _proxy = Helper::start(
        "python3",
        &[SOCKS, &port, log.to_str().unwrap(), "60"],
        &daemon.work,
    );
    listens(&port);

    let off = daemon.ok(&[
        "run",
        "--net",
        "profile:off",
        "--",
        &format!(
            "echo interfaces=$(tail -n +3 /proc/net/dev | cut -d: -f1 | tr -d ' ' | tr '\\n' ' '); \
             curl -sS -m 2 http://127.0.0.1:{web}/page.txt; echo off=$?"
        ),
    ]);
    assert!(
        off.contains("interfaces=lo\n") && off.contains("off=7"),
        "{off}"
    );
    let inlab = daemon.ok(&[
        "run",
        "--net",
        "profile:inlab",
        "--",
        "echo namespace=$(readlink /proc/self/ns/net)",
    ]);
    assert!(
        inlab.contains(&format!("namespace={}\n", held.namespace())),
        "{inlab}"
    );
    let plain = daemon.ok(&[
        "run",
        "--net",
        "profile:plain",
        "--",
        "echo namespace=$(readlink /proc/self/ns/net)",
    ]);
    let own = std::fs::read_link("/proc/self/ns/net").unwrap();
    assert!(
        plain.contains(&format!("namespace={}\n", own.display())),
        "{plain}"
    );

    let expected_length = format!("socks5h://user:{mark}@198.18.0.2:{port}").len();
    let id = submitted(
        &daemon,
        &[
            "submit",
            "--net",
            "profile:via",
            "--",
            &format!(
                "echo proxy-length=${{#ALL_PROXY}}; curl -sS -m 5 http://127.0.0.1:{web}/page.txt; echo through=$?; \
                 curl --noproxy '*' -sS -m 2 http://198.18.0.2:{web}/page.txt; echo around=$?; \
                 touch via.ready; n=0; while [ ! -e via.done ] && [ $n -lt 300 ]; do sleep 0.1; n=$((n+1)); done"
            ),
        ],
    );
    assert!(
        appears(&daemon.file("via.ready"), 30),
        "the proxy job did not start"
    );
    let lines = command_lines_holding(&mark);
    let said = [
        both(&daemon.job(&["status", &id, "--json"])),
        both(&daemon.job(&["status", &id])),
        both(&daemon.job(&["explain", &id, "--json"])),
        both(&daemon.job(&["host", "--json"])),
        both(&daemon.job(&["net", "list", "--json"])),
        both(&daemon.job(&["net", "show", "via"])),
        both(&daemon.job(&["config", "show", "--json"])),
        both(&daemon.job(&["queue", "--json"])),
        both(&daemon.job(&["audit"])),
        both(&daemon.job(&["events"])),
        both(&daemon.job(&["doctor", "--json"])),
    ]
    .concat();
    std::fs::write(daemon.file("via.done"), "").unwrap();
    daemon.job(&["wait", &id, "--timeout", "60s"]);
    assert!(
        lines.is_empty(),
        "process arguments hold the password: {lines:?}"
    );
    assert!(
        !said.contains(&mark),
        "an answer holds the password: {said}"
    );
    assert!(said.contains(secret.to_str().unwrap()), "{said}");
    let output = daemon.ok(&["log", &id, "full"]);
    assert!(
        output.contains(&format!("proxy-length={expected_length}\n")),
        "{output}"
    );
    assert!(
        output.contains("served from outside\nthrough=0"),
        "{output}"
    );
    assert!(output.contains("around=7"), "{output}");
    let mut found = Vec::new();
    files_holding(&daemon.base, &mark, &secret, &mut found);
    assert!(found.is_empty(), "files hold the password: {found:?}");

    let own_secret = daemon.file("own.secret");
    std::fs::write(&own_secret, "someone:else\n").unwrap();
    std::fs::set_permissions(
        &own_secret,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
    let own_secret = own_secret.to_str().unwrap();
    for (args, expected) in [
        (
            vec!["run", "--net", "profile:nowhere", "--", "true"],
            "there is no network profile nowhere in the service configuration",
        ),
        (
            vec![
                "run",
                "--net",
                "profile:via",
                "--net-secret-file",
                own_secret,
                "--",
                "true",
            ],
            "the profile names its own secret_file",
        ),
        (
            vec![
                "run",
                "--net",
                "profile:off",
                "--net-secret-file",
                own_secret,
                "--",
                "true",
            ],
            "does not lead through one",
        ),
        (
            vec![
                "run",
                "--net",
                "profile:capped",
                "--bandwidth",
                "8Mbit",
                "--",
                "true",
            ],
            "--bandwidth 8 Mbit/s is wider than the 5 Mbit/s profile:capped allows a Job",
        ),
        (
            vec![
                "run",
                "--net",
                "profile:off",
                "--bandwidth",
                "1Mbit",
                "--",
                "true",
            ],
            "its network is not one job can shape",
        ),
        (
            vec![
                "run",
                "--net",
                "profile:via",
                "--bandwidth",
                "1Mbit",
                "--",
                "true",
            ],
            "the profile sets no bandwidth to divide",
        ),
        (
            vec!["queue", "create", "bad", "--net", "profile:nowhere"],
            "there is no network profile nowhere",
        ),
    ] {
        let refused = daemon.refused(&args);
        assert!(refused.contains(expected), "{args:?}: {refused}");
    }
    let narrower = daemon.ok(&[
        "run",
        "--net",
        "profile:capped",
        "--bandwidth",
        "2Mbit",
        "--",
        &format!("curl -sS -m 5 http://198.18.0.2:{web}/page.txt"),
    ]);
    assert!(
        narrower.contains("served from outside") && narrower.contains("network capped at 2 Mbit/s"),
        "{narrower}"
    );
    let own = daemon.ok(&[
        "run",
        "--net",
        "profile:open",
        "--net-secret-file",
        own_secret,
        "--",
        "echo proxy-length=${#ALL_PROXY}",
    ]);
    let expected = format!("socks5h://someone:else@198.18.0.2:{port}").len();
    assert!(own.contains(&format!("proxy-length={expected}\n")), "{own}");

    let config = daemon.file("bad.toml");
    for (body, expected) in [
        (
            "egress = 'host'\nallow = ['2001:db8::/32']\n",
            "names an IPv6 range",
        ),
        (
            "egress = 'host'\nallow = ['[2001:db8::1]:443/tcp']\n",
            "names an IPv6 range",
        ),
        (
            "egress = 'host'\ndns = ['2001:db8::53']\n",
            "is an IPv6 address",
        ),
        (
            "egress = 'host'\nallow = ['10.0.0.0/8:0/tcp']\n",
            "write a port",
        ),
        (
            "egress = 'host'\nroutes = ['10.0.0.0/8']\n",
            "unknown field",
        ),
        (
            "egress = 'host'\nsecret_file = '/x'\n",
            "belongs to a proxy",
        ),
        (
            "egress = 'none'\nbandwidth = '1Mbit'\n",
            "there is no network to shape",
        ),
        ("egress = 'ns:lab'\n", "there is no network namespace lab"),
        (
            "egress = 'profile:x'\n",
            "egress cannot name another profile",
        ),
        ("egress = 'openvpn:/x'\n", "openvpn: not yet"),
        (
            "egress = 'socks5://user:password@proxy.example:1080'\n",
            "put them in a file and name it with secret_file",
        ),
        (
            "egress = 'socks5://proxy.example:1080'\nallow = ['10.0.0.0/8']\n",
            "filter destinations at the proxy",
        ),
        (
            "egress = 'socks5://proxy.example:1080'\ndns = ['10.0.0.1']\n",
            "names are resolved by the proxy",
        ),
        (
            "egress = 'host'\nsharing = 'shared'\njob_bandwidth = '1Mbit'\n",
            "job_bandwidth without bandwidth",
        ),
        (
            "egress = 'host'\nsharing = 'sometimes'\n",
            "unknown variant",
        ),
    ] {
        std::fs::write(
            &config,
            format!("schema_version = 1\n[network.profiles.x]\n{body}"),
        )
        .unwrap();
        let said = daemon.refused(&["config", "check", config.to_str().unwrap()]);
        assert!(said.contains(expected), "{body}: {said}");
        assert!(!said.contains("password@"), "{said}");
    }
}

#[test]
fn network_profile_definitions_are_resolved_at_launch_and_references_are_guarded() {
    if let Some(reason) = missing(&LINK_TOOLS) {
        eprintln!("skipped: {reason}");
        return;
    }
    let web = free_port(40);
    let other = free_port(41);
    let presets = "[[presets.profiles]]\nname = 'routed'\nrevision = 1\n[presets.profiles.values]\nnet = 'profile:door'\n";
    let door = |port: &str| {
        format!("[network.profiles.door]\negress = 'host'\nallow = ['127.0.0.1:{port}/tcp']\n")
    };
    let spare = "[network.profiles.spare]\negress = 'none'\n";
    let mut daemon = Daemon::start("profile-launch", &format!("{presets}{}{spare}", door(&web)));
    let _web = web_server(&daemon.work, &web);
    let _other = Helper::start(
        "python3",
        &["-m", "http.server", "--bind", "127.0.0.1", &other],
        &daemon.work,
    );
    listens(&other);
    let probe = format!(
        "curl -sS -m 3 http://198.18.0.2:{web}/page.txt; echo first=$?; curl -sS -m 3 http://198.18.0.2:{other}/page.txt; echo second=$?"
    );
    let preset = daemon.ok(&["run", "--execution-profile", "routed@1", "--", &probe]);
    assert!(
        preset.contains("first=0") && preset.contains("second=7"),
        "{preset}"
    );
    daemon.ok(&["queue", "create", "routed", "--net", "profile:door"]);
    let queued = submitted(&daemon, &["submit", "-q", "routed", "--", &probe]);
    daemon.job(&["wait", &queued, "--timeout", "60s"]);
    let before = daemon.record(&queued);
    assert_eq!(before["network"]["profile"], "door");
    assert!(daemon.ok(&["log", &queued, "full"]).contains("second=7"));
    let inline = daemon.refused(&["config", "check", {
        std::fs::write(
            daemon.file("inline.toml"),
            "schema_version = 1\n[[presets.profiles]]\nname = 'p'\nrevision = 1\n[presets.profiles.values]\nnet = { Proxy = 'socks5://user:password@proxy.example:1080' }\n",
        )
        .unwrap();
        "inline.toml"
    }]);
    assert!(
        inline.contains("profiles support only Host, None, ns:NAME or profile:NAME"),
        "{inline}"
    );

    Daemon::configure(&daemon.base, spare);
    let reload = daemon.refused(&["config", "reload"]);
    assert!(
        reload.contains("routed is set to profile:door")
            && reload.contains("there is no network profile door"),
        "{reload}"
    );

    let changing = held_id(&daemon.ok(&["create", "--net", "profile:door", "--", &probe]));
    let vanishing = held_id(&daemon.ok(&["create", "--net", "profile:spare", "--", "true"]));
    let preview = daemon.ok(&["explain", &changing, "--json"]);
    assert!(
        preview.contains(before["network"]["digest"].as_str().unwrap()),
        "{preview}"
    );
    daemon.ok(&["queue", "set", "routed", "--net", "default"]);
    daemon.stop();
    Daemon::configure(&daemon.base, &format!("{presets}{}", door(&other)));
    daemon.child = Daemon::spawn(&daemon.base);
    daemon.ready();
    daemon.ok(&["release", &changing]);
    daemon.ok(&["release", &vanishing]);
    daemon.job(&["wait", &changing, "--timeout", "60s"]);
    let waited = both(&daemon.job(&["wait", &vanishing, "--timeout", "60s"]));
    let after = daemon.record(&changing);
    assert_ne!(after["network"]["digest"], before["network"]["digest"]);
    assert_eq!(
        after["network"]["digest"],
        profile_of(&daemon.host(), "door")["digest"]
    );
    let output = daemon.ok(&["log", &changing, "full"]);
    assert!(
        output.contains("first=7") && output.contains("second=0"),
        "{output}"
    );
    let gone = daemon.record(&vanishing);
    let error = gone["result"]["start_error"].as_str().unwrap_or_default();
    assert!(
        error.contains("there is no network profile spare in the service configuration")
            && error.contains("the Job cannot start without it"),
        "{gone}"
    );
    assert!(
        waited.contains("there is no network profile spare"),
        "{waited}"
    );
}

#[test]
fn network_profile_jobs_keep_their_shared_network_across_a_service_restart() {
    if let Some(reason) = missing(&LINK_TOOLS) {
        eprintln!("skipped: {reason}");
        return;
    }
    let web = free_port(50);
    let mut daemon = Daemon::start(
        "profile-restart",
        &format!(
            "[network.profiles.pool]\negress = 'host'\nsharing = 'shared'\nbandwidth = '100Mbit'\nallow = ['127.0.0.1:{web}/tcp']\n"
        ),
    );
    let _web = web_server(&daemon.work, &web);
    let first = submitted(
        &daemon,
        &[
            "submit",
            "--net",
            "profile:pool",
            "--",
            &format!(
                "ip -4 -o addr show dev eth0 | awk '{{print $4}}' | cut -d/ -f1 > a.v4; echo neighbour > served.txt; \
                 python3 -m http.server --bind 0.0.0.0 {web} >/dev/null 2>&1 & \
                 touch a.ready; n=0; while [ ! -e a.done ] && [ $n -lt 300 ]; do sleep 0.1; n=$((n+1)); done; kill $!"
            ),
        ],
    );
    assert!(
        appears(&daemon.file("a.ready"), 30),
        "the first job did not start"
    );
    let before = holders(&daemon);
    assert_eq!(before.len(), 1, "{before:?}");
    let index = daemon.record(&first)["link"]["job"]["index"]
        .as_u64()
        .unwrap();
    let shown = || {
        let output = in_namespace(before[0].1, &format!("ip -d link show dev q{index}"));
        assert!(output.status.success(), "{}", both(&output));
        text(&output)
    };
    assert!(shown().contains("isolated on"), "{}", shown());
    let cleared = in_namespace(
        before[0].1,
        &format!("ip link set q{index} type bridge_slave isolated off"),
    );
    assert!(cleared.status.success(), "{}", both(&cleared));
    let rules = text(&in_namespace(before[0].1, "nft list chain ip job gate"));
    assert!(
        rules.contains(&format!("tcp dport {web} accept")),
        "{rules}"
    );
    daemon.restart();
    assert!(shown().contains("isolated on"), "{}", shown());
    let address = std::fs::read_to_string(daemon.file("a.v4")).unwrap();
    let second = daemon.ok(&[
        "run",
        "--net",
        "profile:pool",
        "--",
        &format!(
            "curl -sS -m 5 http://198.18.0.2:{web}/page.txt; echo outside=$?; \
             curl -sS -m 3 http://{}:{web}/served.txt; echo neighbour-reached=$?",
            address.trim()
        ),
    ]);
    assert_eq!(holders(&daemon), before);
    std::fs::write(daemon.file("a.done"), "").unwrap();
    daemon.job(&["wait", &first, "--timeout", "60s"]);
    assert!(
        second.contains("served from outside\noutside=0"),
        "{second}"
    );
    assert!(!second.contains("neighbour\n"), "{second}");
    assert!(
        second.contains("neighbour-reached=") && !second.contains("neighbour-reached=0"),
        "{second}"
    );
    assert!(holders(&daemon).is_empty(), "{:?}", holders(&daemon));
}

#[test]
fn network_names_of_a_remote_job_are_left_to_the_destination() {
    use std::os::unix::fs::PermissionsExt;
    let base = Daemon::area("names-remote");
    let fake = base.join("fake");
    std::fs::create_dir_all(&fake).unwrap();
    std::fs::write(
        fake.join("ssh"),
        format!(
            "#!/bin/sh\ncat >> {dir}/request\nexit 255\n",
            dir = fake.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(fake.join("ssh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        base.join("path"),
        format!(
            "{}:{}",
            fake.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    )
    .unwrap();
    let daemon = Daemon::start("names-remote", "");
    for net in ["profile:only-there", "ns:only-there"] {
        let here = daemon.refused(&["run", "--net", net, "--", "true"]);
        assert!(here.contains("in the service configuration"), "{here}");
        let id = submitted(
            &daemon,
            &[
                "submit",
                "--on",
                "far@elsewhere",
                "--net",
                net,
                "--",
                "true",
            ],
        );
        daemon.job(&["wait", &id, "--timeout", "30s"]);
        assert!(daemon.record(&id)["network"].is_null());
    }
    let request = std::fs::read_to_string(fake.join("request")).unwrap_or_default();
    let sent: Vec<serde_json::Value> = request
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    assert_eq!(sent.len(), 2, "{request}");
    assert_eq!(
        sent[0]["op"]["Submit"]["declared"]["net"]["Profile"],
        "only-there"
    );
    assert_eq!(
        sent[1]["op"]["Submit"]["declared"]["net"]["Namespace"],
        "only-there"
    );
}
