use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Service {
    root: PathBuf,
    child: Child,
    jobs: Vec<String>,
}

impl Service {
    fn start(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("job-terminal-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let child = Self::spawn(&root);
        let mut service = Self {
            root,
            child,
            jobs: Vec::new(),
        };
        service.ready();
        service
    }

    fn spawn(root: &PathBuf) -> Child {
        Command::new(env!("CARGO_BIN_EXE_job"))
            .arg("daemon")
            .env("LC_ALL", "C")
            .env("JOB_STATE_DIR", root)
            .env("JOB_CGROUP_ROOT", root.join("absent-cgroup"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    fn ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.call(serde_json::json!("Ping")).is_some() {
                return;
            }
            assert!(self.child.try_wait().unwrap().is_none());
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("daemon did not start");
    }

    fn call(&self, request: serde_json::Value) -> Option<serde_json::Value> {
        let mut stream = UnixStream::connect(self.root.join("daemon.sock")).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
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

    fn submit(&mut self, command: &str) -> String {
        self.submit_with(&[], command)
    }

    fn submit_with(&mut self, options: &[&str], command: &str) -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_job"))
            .env("JOB_CLI_COMPAT", "legacy")
            .args(["submit", "--pty", "--time", "20s"])
            .args(options)
            .args(["--", command])
            .env("LC_ALL", "C")
            .env("JOB_STATE_DIR", &self.root)
            .env("JOB_SESSION", "owner")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let id = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        self.jobs.push(id.clone());
        id
    }

    fn attach(&self, id: &str, rows: u16, cols: u16) -> Terminal {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let response = self
                .call(serde_json::json!({"Attach":{"id":id.parse::<u64>().unwrap()}}))
                .unwrap();
            if let Some(path) = response["Attached"]["path"].as_str() {
                let stream = UnixStream::connect(path).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut terminal = Terminal {
                    stream,
                    screen: vt100::Parser::new(rows, cols, 0),
                };
                terminal.send(2, &[rows.to_be_bytes(), cols.to_be_bytes()].concat());
                return terminal;
            }
            assert!(response.get("StillRunning").is_some(), "{response}");
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("terminal did not start");
    }

    fn restart(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        self.child = Self::spawn(&self.root);
        self.ready();
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        for id in &self.jobs {
            let _ = self.call(
                serde_json::json!({"Cancel":{"id":id.parse::<u64>().unwrap(),"session":"cleanup"}}),
            );
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline
            && self.jobs.iter().any(|id| {
                self.call(serde_json::json!({"Status":{"id":id.parse::<u64>().unwrap()}}))
                    .is_some_and(|v| v.get("StillRunning").is_some())
            })
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct Terminal {
    stream: UnixStream,
    screen: vt100::Parser,
}

impl Terminal {
    fn send(&mut self, kind: u8, body: &[u8]) {
        self.stream
            .write_all(&[&[kind][..], &(body.len() as u32).to_be_bytes()[..], body].concat())
            .unwrap();
    }

    fn receive(&mut self) -> (u8, Vec<u8>) {
        let mut header = [0; 5];
        self.stream.read_exact(&mut header).unwrap();
        let mut body = vec![0; u32::from_be_bytes(header[1..].try_into().unwrap()) as usize];
        assert!(body.len() <= 1024 * 1024);
        self.stream.read_exact(&mut body).unwrap();
        if header[0] == 3 {
            self.screen.process(&body);
        }
        (header[0], body)
    }

    fn until(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.screen.screen().contents().contains(text) && Instant::now() < deadline {
            let (kind, _) = self.receive();
            assert_eq!(kind, 3, "terminal exited before {text}");
        }
        assert!(
            self.screen.screen().contents().contains(text),
            "{}",
            self.screen.screen().contents()
        );
    }
}

const ECHO: &str = "stty -echo; printf 'READY\\n'; while IFS= read -r line; do case \"$line\" in quit) exit 7;; size) stty size;; *) printf 'received:%s\\n' \"$line\";; esac; done";

#[test]
fn two_clients_write_and_reattach_to_the_same_terminal() {
    let mut service = Service::start("shared");
    let id = service.submit(ECHO);
    let mut first = service.attach(&id, 24, 80);
    let mut second = service.attach(&id, 24, 80);
    first.until("READY");
    second.until("READY");
    first.send(1, b"alpha\n");
    second.send(1, b"beta\n");
    for terminal in [&mut first, &mut second] {
        terminal.until("received:alpha");
        terminal.until("received:beta");
    }
    drop(first);
    drop(second);
    let mut again = service.attach(&id, 24, 80);
    again.until("received:alpha");
    again.until("received:beta");
    again.send(1, b"quit\n");
    loop {
        let (kind, body) = again.receive();
        if kind == 4 {
            assert_eq!(i32::from_be_bytes(body.try_into().unwrap()), 7);
            break;
        }
    }
}

#[test]
fn terminal_survives_daemon_restart() {
    let mut service = Service::start("restart");
    let id = service.submit(ECHO);
    let mut first = service.attach(&id, 24, 80);
    first.until("READY");
    service.restart();
    first.send(1, b"survived\n");
    first.until("received:survived");
    let mut second = service.attach(&id, 24, 80);
    second.until("received:survived");
}

#[test]
fn smallest_attached_terminal_sets_shared_size() {
    let mut service = Service::start("resize");
    let id = service.submit(ECHO);
    let mut large = service.attach(&id, 40, 120);
    large.until("READY");
    let mut small = service.attach(&id, 20, 60);
    small.until("READY");
    small.send(1, b"size\n");
    small.until("20 60");
    drop(small);
    large.send(1, b"size\n");
    large.until("40 120");
}

#[test]
fn attach_rejects_jobs_without_a_terminal() {
    let service = Service::start("no-terminal");
    let output = Command::new(env!("CARGO_BIN_EXE_job"))
        .args(["submit", "--", "true"])
        .env("LC_ALL", "C")
        .env("JOB_STATE_DIR", &service.root)
        .output()
        .unwrap();
    let id: u64 = String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let response = service
        .call(serde_json::json!({"Attach":{"id":id}}))
        .unwrap();
    assert!(
        response["Error"]["message"]
            .as_str()
            .unwrap()
            .contains("no terminal")
    );
}

#[test]
fn any_client_of_the_service_can_cancel_a_terminal_job() {
    let mut service = Service::start("cancel");
    let id = service.submit(ECHO);
    service.attach(&id, 24, 80).until("READY");
    let response = service.call(serde_json::json!({"Cancel":{"id":id.parse::<u64>().unwrap(),"session":"another-client"}})).unwrap();
    assert!(response.get("Cancelled").is_some(), "{response}");
}

#[test]
fn cli_detach_restores_local_terminal_and_keeps_job_running() {
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::process::CommandExt;

    let mut service = Service::start("cli");
    let (mut master_fd, mut slave_fd) = (-1, -1);
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    let mut master = unsafe { File::from_raw_fd(master_fd) };
    let slave = unsafe { File::from_raw_fd(slave_fd) };
    for fd in [master_fd, slave_fd] {
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    let size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(unsafe { libc::ioctl(slave_fd, libc::TIOCSWINSZ, &size) }, 0);
    let mut before: libc::termios = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::tcgetattr(slave_fd, &mut before) }, 0);
    let mut command = Command::new(env!("CARGO_BIN_EXE_job"));
    command
        .args(["run", "--pty", "--time", "20s", "--", ECHO])
        .env("JOB_CLI_COMPAT", "legacy")
        .env("LC_ALL", "C")
        .env("JOB_STATE_DIR", &service.root)
        .stdin(slave.try_clone().unwrap())
        .stdout(slave.try_clone().unwrap())
        .stderr(slave.try_clone().unwrap());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    drop(command);
    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !String::from_utf8_lossy(&output).contains("READY") && Instant::now() < deadline {
        let mut poll = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, 100) } > 0 {
            let mut bytes = [0; 8192];
            let n = master.read(&mut bytes).unwrap();
            output.extend_from_slice(&bytes[..n]);
        }
    }
    let output = String::from_utf8_lossy(&output);
    assert!(output.contains("READY"), "{output}");
    let id = output
        .split("[job] terminal ")
        .nth(1)
        .unwrap()
        .split(':')
        .next()
        .unwrap();
    service.jobs.push(id.to_owned());
    master.write_all(b"\x1dd").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("attach failed to detach");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success());
    let mut after: libc::termios = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::tcgetattr(slave_fd, &mut after) }, 0);
    assert_eq!(after.c_lflag, before.c_lflag);
    assert_eq!(after.c_iflag, before.c_iflag);
    assert_eq!(after.c_oflag, before.c_oflag);
    let mut attached = service.attach(id, 24, 80);
    attached.until("READY");
    attached.send(1, b"after-detach\n");
    attached.until("received:after-detach");
}

#[test]
fn terminal_control_c_reaches_the_foreground_process_group() {
    let mut service = Service::start("signal");
    let id = service.submit("printf 'READY\\n'; exec sleep 15");
    let mut terminal = service.attach(&id, 24, 80);
    terminal.until("READY");
    terminal.send(1, b"\x03");
    loop {
        let (kind, body) = terminal.receive();
        if kind == 4 {
            assert_eq!(
                i32::from_be_bytes(body.try_into().unwrap()),
                128 + libc::SIGINT
            );
            break;
        }
    }
}

#[test]
fn output_is_drained_without_any_attached_client() {
    let mut service = Service::start("unattached-output");
    let id = service.submit("stty -echo; seq 1 20000; printf '\\nDRAINED\\n'; read line");
    let log = service.root.join("jobs").join(&id).join("output.log");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if std::fs::read(&log).is_ok_and(|bytes| bytes.windows(7).any(|s| s == b"DRAINED")) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        std::fs::read(&log)
            .unwrap()
            .windows(7)
            .any(|s| s == b"DRAINED")
    );
    service.attach(&id, 24, 80).until("DRAINED");
}

#[test]
fn old_daemon_is_rejected_before_a_terminal_job_is_submitted() {
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;

    let service = Service::start("old-daemon");
    let mut host = service.call(serde_json::json!("Host")).unwrap();
    host["Host"]["info"]
        .as_object_mut()
        .unwrap()
        .remove("terminal_protocol");
    let root = service.root.join("old");
    std::fs::create_dir(&root).unwrap();
    let listener = UnixListener::bind(root.join("daemon.sock")).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&request).unwrap(),
            serde_json::json!({"Versioned": {"protocol": 20, "min": 20, "request": "Host"}})
        );
        writeln!(stream, "{host}").unwrap();
    });
    let output = Command::new(env!("CARGO_BIN_EXE_job"))
        .args(["submit", "--pty", "--", "true"])
        .env("LC_ALL", "C")
        .env("JOB_STATE_DIR", root)
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    server.join().unwrap();
    assert_eq!(output.status.code(), Some(125));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("does not support this terminal protocol")
    );
}

#[test]
fn an_interactive_shell_accepts_commands_after_reattachment() {
    let mut service = Service::start("interactive-shell");
    let id = service.submit("export PS1='PROMPT> '; exec bash --noprofile --norc -i");
    let mut first = service.attach(&id, 24, 80);
    first.until("PROMPT>");
    drop(first);
    let mut second = service.attach(&id, 24, 80);
    second.until("PROMPT>");
    second.send(1, b"printf 'SHELL_%s\\n' OK\n");
    second.until("SHELL_OK");
    second.send(1, b"exit\n");
    loop {
        let (kind, body) = second.receive();
        if kind == 4 {
            assert_eq!(i32::from_be_bytes(body.try_into().unwrap()), 0);
            break;
        }
    }
}

const BYTES: &str = "stty raw -echo; printf 'READY\\r\\n'; i=0; while [ $i -lt 24 ]; do b=$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\n'); printf 'got:%s;' \"$b\"; i=$((i+1)); done; sleep 15";

struct Client {
    child: Child,
    master: std::fs::File,
    seen: Vec<u8>,
}

impl Client {
    fn attach(root: &std::path::Path, id: &str, options: &[&str], key: Option<&str>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_job"));
        command
            .arg("attach")
            .arg(id)
            .args(options)
            .env("LC_ALL", "C")
            .env("JOB_STATE_DIR", root)
            .env_remove("JOB_DETACH_KEY");
        if let Some(key) = key {
            command.env("JOB_DETACH_KEY", key);
        }
        Self::spawn(command)
    }

    fn spawn(mut command: Command) -> Self {
        use std::os::fd::FromRawFd;
        use std::os::unix::process::CommandExt;

        let (mut master_fd, mut slave_fd) = (-1, -1);
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master_fd,
                    &mut slave_fd,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let master = unsafe { std::fs::File::from_raw_fd(master_fd) };
        let slave = unsafe { std::fs::File::from_raw_fd(slave_fd) };
        for fd in [master_fd, slave_fd] {
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        let size = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(unsafe { libc::ioctl(slave_fd, libc::TIOCSWINSZ, &size) }, 0);
        command
            .stdin(slave.try_clone().unwrap())
            .stdout(slave.try_clone().unwrap())
            .stderr(slave.try_clone().unwrap());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        Self {
            child,
            master,
            seen: Vec::new(),
        }
    }

    fn until(&mut self, text: &str) {
        use std::os::fd::AsRawFd;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !String::from_utf8_lossy(&self.seen).contains(text) && Instant::now() < deadline {
            let mut poll = libc::pollfd {
                fd: self.master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut poll, 1, 100) } > 0 {
                let mut bytes = [0; 8192];
                match self.master.read(&mut bytes) {
                    Ok(n) => self.seen.extend_from_slice(&bytes[..n]),
                    Err(_) => break,
                }
            }
        }
        assert!(
            String::from_utf8_lossy(&self.seen).contains(text),
            "{}",
            String::from_utf8_lossy(&self.seen)
        );
    }

    fn exited(&mut self) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().unwrap() {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn configured_service(name: &str, config: &str) -> Service {
    let root = std::env::temp_dir().join(format!("job-terminal-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.toml"), config).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_job"))
        .arg("daemon")
        .env("LC_ALL", "C")
        .env("JOB_STATE_DIR", &root)
        .env("JOB_CONFIG", root.join("config.toml"))
        .env("JOB_CGROUP_ROOT", root.join("absent-cgroup"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut service = Service {
        root,
        child,
        jobs: Vec::new(),
    };
    service.ready();
    service
}

#[test]
fn detach_key_option_replaces_the_default_and_passes_the_old_key_through() {
    let mut service = Service::start("detach-option");
    let id = service.submit(BYTES);
    let mut watcher = service.attach(&id, 24, 80);
    watcher.until("READY");
    let mut client = Client::attach(
        &service.root,
        &id,
        &["--detach-key", "ctrl-a"],
        Some("ctrl-b"),
    );
    client.until("detach with Ctrl-A then d");
    client.until("READY");
    client.master.write_all(b"\x1dd").unwrap();
    watcher.until("got:1d;got:64;");
    client.master.write_all(b"\x02d").unwrap();
    watcher.until("got:1d;got:64;got:02;got:64;");
    client.master.write_all(b"\x01\x01").unwrap();
    watcher.until("got:1d;got:64;got:02;got:64;got:01;");
    assert!(client.child.try_wait().unwrap().is_none());
    client.master.write_all(b"\x01d").unwrap();
    assert!(client.exited().unwrap().success());
    watcher.send(1, b"z");
    watcher.until("got:1d;got:64;got:02;got:64;got:01;got:7a;");
    let refused = Command::new(env!("CARGO_BIN_EXE_job"))
        .args(["attach", &id, "--detach-key", "ctrl-1"])
        .env("LC_ALL", "C")
        .env("JOB_STATE_DIR", &service.root)
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("is not ctrl-LETTER"));
}

#[test]
fn detach_key_from_configuration_applies_without_an_option() {
    let mut service = configured_service(
        "detach-config",
        "schema_version = 1\n[terminal]\ndetach_key = 'ctrl-a'\n",
    );
    let id = service.submit(BYTES);
    let mut watcher = service.attach(&id, 24, 80);
    watcher.until("READY");
    let mut client = Client::attach(&service.root, &id, &[], None);
    client.until("detach with Ctrl-A then d");
    client.until("READY");
    client.master.write_all(b"\x1dd").unwrap();
    watcher.until("got:1d;got:64;");
    client.master.write_all(b"\x01d").unwrap();
    assert!(client.exited().unwrap().success());
    watcher.send(1, b"z");
    watcher.until("got:1d;got:64;got:7a;");
    std::fs::write(
        service.root.join("bad.toml"),
        "schema_version = 1\n[terminal]\ndetach_key = 'f1'\n",
    )
    .unwrap();
    let refused = Command::new(env!("CARGO_BIN_EXE_job"))
        .args(["config", "check"])
        .arg(service.root.join("bad.toml"))
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    assert!(!refused.status.success());
}

#[test]
fn detach_key_none_passes_every_key_through_until_the_terminal_closes() {
    let mut service = configured_service(
        "detach-none",
        "schema_version = 1\n[terminal]\ndetach_key = 'ctrl-a'\n",
    );
    let id = service.submit(BYTES);
    let mut watcher = service.attach(&id, 24, 80);
    watcher.until("READY");
    let mut client = Client::attach(&service.root, &id, &[], Some("none"));
    client.until("no detach key is set");
    client.until("READY");
    client.master.write_all(b"\x1dd\x01d").unwrap();
    watcher.until("got:1d;got:64;got:01;got:64;");
    assert!(client.child.try_wait().unwrap().is_none());
    drop(std::mem::replace(
        &mut client.master,
        std::fs::File::open("/dev/null").unwrap(),
    ));
    assert!(client.exited().is_some());
    watcher.send(1, b"z");
    watcher.until("got:1d;got:64;got:01;got:64;got:7a;");
}

fn job_state(service: &Service, id: &str) -> (String, u64) {
    let output = Command::new(env!("CARGO_BIN_EXE_job"))
        .args(["status", id, "--json"])
        .env("LC_ALL", "C")
        .env("JOB_STATE_DIR", &service.root)
        .output()
        .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stderr)));
    (
        status["state"].as_str().unwrap().to_owned(),
        status["shim_pid"].as_u64().unwrap(),
    )
}

#[test]
fn two_client_processes_write_without_takeover_and_killed_clients_leave_the_job_running() {
    use std::os::unix::fs::PermissionsExt;

    let mut service = Service::start("two-processes");
    let id = service.submit(ECHO);
    let login = service.root.join("login-bin");
    std::fs::create_dir(&login).unwrap();
    let ssh = login.join("ssh");
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\n[ \"$1\" = -t ] || exit 97\n[ \"$2\" = test-host ] || exit 98\nshift 2\nexec env -i PATH=/usr/bin:/bin TERM=xterm LC_ALL=C JOB_STATE_DIR='{}' \"$@\"\n",
            service.root.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    let through_login = |id: &str| {
        let mut command = Command::new(&ssh);
        command.args(["-t", "test-host", env!("CARGO_BIN_EXE_job"), "attach", id]);
        Client::spawn(command)
    };
    let mut local = Client::attach(&service.root, &id, &[], None);
    let mut remote = through_login(&id);
    local.until("READY");
    remote.until("READY");
    let (_, supervisor) = job_state(&service, &id);
    for (round, word) in ["alpha", "beta", "gamma", "delta"].iter().enumerate() {
        let writer = if round % 2 == 0 {
            &mut local
        } else {
            &mut remote
        };
        writer
            .master
            .write_all(format!("{word}\n").as_bytes())
            .unwrap();
        local.until(&format!("received:{word}"));
        remote.until(&format!("received:{word}"));
    }
    assert!(local.child.try_wait().unwrap().is_none());
    assert!(remote.child.try_wait().unwrap().is_none());
    remote.child.kill().unwrap();
    remote.child.wait().unwrap();
    drop(remote);
    local.master.write_all(b"after-kill\n").unwrap();
    local.until("received:after-kill");
    assert_eq!(job_state(&service, &id), ("Running".to_owned(), supervisor));
    let mut again = through_login(&id);
    again.until("received:after-kill");
    again.master.write_all(b"rejoined\n").unwrap();
    again.until("received:rejoined");
    local.until("received:rejoined");
    local.child.kill().unwrap();
    local.child.wait().unwrap();
    again.child.kill().unwrap();
    again.child.wait().unwrap();
    drop(local);
    drop(again);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(job_state(&service, &id), ("Running".to_owned(), supervisor));
    let mut last = Client::attach(&service.root, &id, &[], None);
    last.until("received:rejoined");
    last.master.write_all(b"quit\n").unwrap();
    assert_eq!(last.exited().unwrap().code(), Some(7));
    let deadline = Instant::now() + Duration::from_secs(5);
    while job_state(&service, &id).0 == "Running" {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(job_state(&service, &id).0, "Failed");
}

#[test]
fn a_terminal_on_another_host_is_refused_and_attach_has_no_remote_option() {
    let service = Service::start("no-remote-terminal");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_job"))
            .args(args)
            .env("LC_ALL", "C")
            .env("JOB_STATE_DIR", &service.root)
            .output()
            .unwrap()
    };
    let submitted = run(&["submit", "--pty", "--on", "test-host", "--", "sh"]);
    assert_eq!(submitted.status.code(), Some(125));
    assert!(
        String::from_utf8_lossy(&submitted.stderr)
            .contains("--pty is local to a job service; connect with SSH and run job there"),
        "{}",
        String::from_utf8_lossy(&submitted.stderr)
    );
    let attached = run(&["attach", "--on", "test-host", "1"]);
    assert_eq!(attached.status.code(), Some(125));
    assert!(!String::from_utf8_lossy(&attached.stderr).is_empty());
    let listed = run(&["list", "--json"]);
    assert!(listed.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&listed.stdout).unwrap()["jobs"],
        serde_json::json!([])
    );
}

#[test]
fn pidns_shell_has_job_control_signals_and_window_size_under_the_init() {
    let mut service = Service::start("pidns-shell");
    let id = service.submit_with(
        &["--namespaces", "user,mount,pid"],
        "export PS1='PROMPT> '; exec bash --noprofile --norc -i",
    );
    let mut terminal = service.attach(&id, 24, 80);
    terminal.until("PROMPT>");
    terminal.send(1, b"echo pid=$$ parent=$PPID tty=$(tty | cut -c1-8)\n");
    terminal.until("pid=2 parent=1 tty=/dev/pts");
    terminal.send(1, b"sleep 19 &\n");
    terminal.until("[1]");
    terminal.send(1, b"echo before=$(jobs -p | wc -l)\n");
    terminal.until("before=1");
    terminal.send(1, b"fg\n");
    std::thread::sleep(Duration::from_millis(300));
    terminal.send(1, b"\x03");
    terminal.send(1, b"echo after=$(jobs -p | wc -l)\n");
    terminal.until("after=0");
    terminal.send(1, b"sleep 18\n");
    std::thread::sleep(Duration::from_millis(300));
    terminal.send(1, b"\x1a");
    terminal.until("Stopped");
    terminal.send(1, b"fg\n");
    std::thread::sleep(Duration::from_millis(300));
    terminal.send(1, b"\x03");
    terminal.send(1, b"echo cleared=$(jobs -p | wc -l)\n");
    terminal.until("cleared=0");
    terminal.send(
        1,
        b"sh -c 'trap \"echo winch=\\$((40+2)); exit\" WINCH; echo armed=$((1+1)); i=0; while [ $i -lt 50 ]; do sleep 0.1; i=$((i+1)); done'\n",
    );
    terminal.until("armed=2");
    terminal.send(2, &[30u16.to_be_bytes(), 100u16.to_be_bytes()].concat());
    terminal.until("winch=42");
    terminal.send(1, b"echo size=$(stty size | tr ' ' x)\n");
    terminal.until("size=30x100");
    terminal.send(1, b"exit 7\n");
    loop {
        let (kind, body) = terminal.receive();
        if kind == 4 {
            assert_eq!(i32::from_be_bytes(body.try_into().unwrap()), 7);
            break;
        }
    }
}

#[test]
fn pidns_terminal_control_c_ends_the_command_with_the_interrupt_signal() {
    let mut service = Service::start("pidns-signal");
    let id = service.submit_with(
        &["--namespaces", "user,mount,pid"],
        "printf 'READY\\n'; exec sleep 15",
    );
    let mut terminal = service.attach(&id, 24, 80);
    terminal.until("READY");
    terminal.send(1, b"\x03");
    loop {
        let (kind, body) = terminal.receive();
        if kind == 4 {
            assert_eq!(
                i32::from_be_bytes(body.try_into().unwrap()),
                128 + libc::SIGINT
            );
            break;
        }
    }
}
