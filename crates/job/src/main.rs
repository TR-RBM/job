macro_rules! println {
    () => { $crate::commands::output::write(false, true, format_args!("")) };
    ($($argument:tt)*) => { $crate::commands::output::write(false, true, format_args!($($argument)*)) };
}
#[allow(unused_macros)]
macro_rules! print {
    ($($argument:tt)*) => { $crate::commands::output::write(false, false, format_args!($($argument)*)) };
}
macro_rules! eprintln {
    () => { $crate::commands::output::write(true, true, format_args!("")) };
    ($($argument:tt)*) => { $crate::commands::output::write(true, true, format_args!($($argument)*)) };
}
macro_rules! eprint {
    ($($argument:tt)*) => { $crate::commands::output::write(true, false, format_args!($($argument)*)) };
}

mod admission;
mod aggregate;
mod answer;
mod attempts;
mod cancellation;
mod cgroup;
mod cli2;
mod cli_contract;
mod client;
mod clientif;
mod commands;
mod config;
mod confine;
mod daemon;
mod display;
mod doctor;
mod durability;
mod estimate;
mod filter;
mod freezer;
mod hook;
mod host;
mod idset;
mod io_policy;
mod isolate;
mod isolation;
mod lifecycle;
mod link;
mod logfile;
mod migration;
mod model;
mod netpolicy;
mod netsecret;
mod objects;
mod operations;
mod pacing;
mod paths;
mod policy;
mod policy_file;
mod presets;
mod pressure;
mod process;
mod process_policy;
mod procs;
mod query;
mod remote;
mod removal;
mod resource_policy;
mod resource_update;
mod resources;
mod schedule;
mod security;
mod service;
mod shell;
mod shim;
mod store;
mod streams;
mod templates;
mod terminal;
mod units;
mod wg;

use std::path::PathBuf;
use std::process::ExitCode;

use model::{Declared, Net, Parallel, QueueChange, Request, Response, State};
use store::Store;

const DEFAULT_BUDGET_MS: u64 = 120_000;
const ANSWER_MARGIN_MS: u64 = 5_000;
const EXIT_SERVICE_ERROR: u8 = 125;
const EXIT_STILL_RUNNING: u8 = 75;
const UNLIMITED_BUDGET_MS: u64 = u64::MAX / 4;
const RECONNECT_WINDOW_MS: u64 = 60_000;
const RECONNECT_INTERVAL_MS: u64 = 250;

const USAGE: &str = "usage:
  job run [options] -- COMMAND...     execute and wait; --pty opens a shared terminal
  job submit [options] -- COMMAND...  submit and print the job id
  job create [options] -- COMMAND...  create held work without starting it
  job edit ID [options] -- COMMAND... replace a held/queued command and environment
  job release ID                     release held work for scheduling
  job retry ID [--hold] [--current-env] [--queue PATH] [--allow-lost] [--json]
  job attempts ID [--json]            inspect all execution attempts
  job remove ID [--dry-run] [--allow-lost] [--json]  remove completed records
  job queue|group remove PATH [--recursive] [--dry-run] [--allow-lost] [--json]
  job queue|group cancel --recursive PATH [--dry-run] [--json]
  job suspend ID [--timeout DURATION] [--json]   freeze a local cgroup workload
  job continue ID [--timeout DURATION] [--json]  confirm thawed cgroup state
  job queue|group suspend|continue --recursive PATH [--timeout DURATION] [--json]
  job attach ID [--detach-key KEY|none]  join a shared terminal; detach: Ctrl-] then d unless set
  job wait ID [--timeout DURATION]    wait quietly and return its exit status
  job status ID                       the answer, or where the job stands
  job log ID [--attempt N] [errors | grep TEXT | lines A..B | tail N | full]
  job queue                           every queue, and every queued and running job
  job queue NAME                      one queue: its rules, what runs and what waits
  job queue create PATH [SETTINGS]   create an unconfigured Queue
  job group create PATH [SETTINGS]   create a Group of Groups and Queues
  job group list | show PATH         inspect hierarchy and effective configuration
  job group set [SETTINGS] PATH       set local defaults and constraints
  job queue/group move --group P PATH   move an inactive object
  job queue/group rename PATH NAME   rename without changing identity
  job queue/group unset PATH KEY...  restore inheritance
  job queue/group close | open PATH  reject or accept new submissions
  job queue add NAME [SETTINGS]       legacy creation spelling; no implicit concurrency
  job queue set NAME SETTINGS         change them; running jobs go on
      --parallel N|all                how many of its jobs run at once (default unlimited)
      --cores N  --mem SIZE           what its jobs hold together at most
      --dir DIR                       where its jobs run, unless a job says --dir
      --net default|none              the host's network, or none
      --bandwidth RATE                all its jobs together, each way, e.g. 10Mbit or 2MB/s
      --job-bandwidth RATE            each of its jobs, unless a job says --bandwidth
      --on USER@HOST [--ssh-key FILE] [--ssh-option KEY=VALUE]
                                      its jobs run on that host's job daemon
      --monitor M                     the monitor its jobs' windows belong on
      --host-cores N                  while one of its jobs runs, this host's jobs
                                      together use at most N cores
  job queue pause NAME | resume NAME  hold its waiting jobs, or let them start again
  job queue clear NAME                cancel every job waiting in it
  job queue rm NAME [--when-empty]    remove it, or take no new jobs and remove it when empty
  job cancel ID                       cancel a job
  job signal [-s SIGNAL] ID           signal the current workload processes
  job host [--json]                   this host: job's version and protocol, cores, memory,
                                      the pool, devices and queues
  job host --on USER@HOST [--json]    the same of another host, over ssh
  job screenshot [M] [-o FILE]        a PNG of monitor M (default: the whole screen)
  job screenshot --job ID [-o FILE]   a PNG of the job's window, once it has redrawn all of it
  job screenshot --pid PID [-o FILE]  the same for a process and its children
  job doctor [--json] [--system]      check this host and the service setup; see jobd(8)
  job config check FILE               validate service configuration
  job config init --profile ordinary|legacy FILE  create a configuration
  job config show [--json]             inspect effective service policy and its source
  job config reload                   reload the same profile while drained
  job state validate --source PATH    validate offline state and report migration readiness
  job state backup --source PATH --destination PATH  create a verified backup
  job state migrate --source PATH --destination PATH --backup PATH --profile ordinary|legacy [--dry-run]
  job state restore --source BACKUP --destination PATH  restore into a new directory
  job policy [NAME] < COMMANDS.jsonl  the command policy's verdict for each line, as text

options:
  -q, --queue NAME the queue to run in (default: the shared pool)
  --dir DIR        run in DIR (default: the queue's directory, else here)
  --net default|none   the host's network (default), or none but loopback
  --bandwidth RATE the job's network, each way, e.g. 5Mbit or 1MB/s
  --on USER@HOST   run it on another host's job daemon, over ssh
                   (--ssh-key FILE, --ssh-option KEY=VALUE as with ssh -o)
  --send PATH      copy PATH from here into its directory there before it starts
  --fetch PATH     copy PATH from its directory there back here after it ends
  --monitor M      a monitor by name (HDMI-0), left, right, primary or number; the job
                   gets DISPLAY, JOB_MONITOR=WxH+X+Y and JOB_MONITOR_NAME
  --session NAME   the calling session's label (default $JOB_SESSION)
  --cores N        cores to reserve          --mem SIZE    memory, e.g. 4G
  --pids N         processes and threads     --disk SIZE   bytes it may write
  --device NAME    an exclusive device       --time DUR    stop it after DUR
  --pty            persistent shared terminal (run attaches; submit detaches)
  --confine        the job may write only in its own tree, the repository's .git,
                   /tmp, /dev and ~/.cargo, ~/.cache, ~/.rustup (Landlock)
  --allow-write P  also allow writing under P (implies --confine)
  --budget DUR     how long `run` waits (default 2m; none waits to completion)
  --json           print the job record as JSON

Arguments after -- execute literally. Use --shell SHELL or an explicit sh -c command.";

struct Options {
    idempotency_key: Option<String>,
    session: String,
    queue: Option<String>,
    declared: Declared,
    budget_ms: u64,
    json: bool,
    summary: bool,
    compatibility: bool,
    on_interrupt: Option<cli2::interrupt::Policy>,
    command: Vec<String>,
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        idempotency_key: None,
        session: clientif::invocation::session(),
        queue: None,
        declared: Declared::default(),
        budget_ms: DEFAULT_BUDGET_MS,
        json: false,
        summary: cli_contract::legacy()?,
        compatibility: cli_contract::legacy()?,
        on_interrupt: None,
        command: Vec::new(),
    };
    let mut selected_shell = None;
    let mut legacy_shell = options.compatibility;
    let mut explicit_legacy_shell = false;
    let mut explicit_summary = false;
    let mut ssh_key: Option<PathBuf> = None;
    let mut ssh_options: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        if flag == "--" {
            options.command = args[i + 1..].to_vec();
            break;
        }
        if flag == "--pty" {
            options.declared.terminal = Some(clientif::invocation::terminal());
            i += 1;
            continue;
        }
        if flag == "--legacy-shell" {
            legacy_shell = true;
            explicit_legacy_shell = true;
            i += 1;
            continue;
        }
        if flag == "--summary" {
            options.summary = true;
            explicit_summary = true;
            i += 1;
            continue;
        }
        if flag == "--stdin" {
            options.declared.stdin = true;
            i += 1;
            continue;
        }
        if flag == "--confine" {
            options.declared.confine = true;
            i += 1;
            continue;
        }
        if flag == "--json" {
            options.json = true;
            i += 1;
            continue;
        }
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("{flag} needs a value"))?;
        if let Some(result) = isolation::parse_option(flag.trim_start_matches("--"), value) {
            let (key, value) = result?;
            isolation::insert(&mut options.declared.isolation, key, value)?;
            i += 2;
            continue;
        }
        if let Some(result) = security::parse_option(flag.trim_start_matches("--"), value) {
            let (key, value) = result?;
            let mut fields = options.declared.security.fields();
            if fields.get(&key).is_some_and(|v| !v.is_null()) {
                return Err(security::message("duplicate security control"));
            }
            fields.insert(key, value);
            options.declared.security =
                serde_json::from_value(serde_json::to_value(fields).unwrap())
                    .map_err(|e| e.to_string())?;
            i += 2;
            continue;
        }
        if let Some(result) = streams::quota::parse_option(flag.trim_start_matches("--"), value) {
            let (key, value) = result?;
            let mut fields = options.declared.output.fields();
            if fields.get(&key).is_some_and(|v| !v.is_null()) {
                return Err(streams::message("duplicate output quota"));
            }
            fields.insert(key, value);
            options.declared.output = serde_json::from_value(serde_json::to_value(fields).unwrap())
                .map_err(|e| e.to_string())?;
            i += 2;
            continue;
        }
        if let Some(result) = process_policy::parse_option(flag.trim_start_matches("--"), value) {
            let (key, value) = result?;
            let mut fields = options.declared.process.fields();
            if fields.get(&key).is_some_and(|v| !v.is_null()) {
                return Err(process_policy::message("duplicate process control"));
            }
            fields.insert(key, value);
            options.declared.process =
                serde_json::from_value(serde_json::to_value(fields).unwrap())
                    .map_err(|e| e.to_string())?;
            i += 2;
            continue;
        }
        if let Some(result) = resource_policy::parse_option(flag.trim_start_matches("--"), value) {
            let (key, value) = result?;
            let mut fields = options.declared.resources.fields();
            io_policy::insert(&mut fields, key, value)?;
            options.declared.resources =
                serde_json::from_value(serde_json::to_value(fields).unwrap())
                    .map_err(|error| error.to_string())?;
            options.declared.resources.validate()?;
            i += 2;
            continue;
        }
        let number = |v: &str| {
            v.parse::<u64>()
                .map_err(|_| format!("{flag}: `{v}` is not a number"))
        };
        match flag {
            "--shell" => selected_shell = Some(value.clone()),
            "--idempotency-key" => {
                durability::idempotency::check(value)?;
                options.idempotency_key = Some(value.clone());
            }
            "--execution-profile" => {
                presets::reference(value)?;
                options.declared.execution_profile = Some(value.clone());
            }
            "--class" => {
                presets::reference(value)?;
                options.declared.scheduling_class = Some(value.clone());
            }
            "--priority" => options.declared.priority = Some(admission::priority(value)?),
            "--label" => cli2::labels::insert(&mut options.declared.labels, value)?,
            "--on-interrupt" => options.on_interrupt = Some(cli2::interrupt::parse(value)?),
            "--session" => options.session = value.clone(),
            "-q" | "--queue" => options.queue = Some(value.clone()),
            "--cores" => options.declared.cores_milli = Some(parse_cores(value)?),
            "--dir" => options.declared.dir = Some(place(value)?),
            "--on" => {
                options.declared.on = Some(model::Remote {
                    target: value.clone(),
                    key: None,
                    options: Vec::new(),
                })
            }
            "--ssh-key" => ssh_key = Some(absolute(value)?),
            "--ssh-option" => ssh_options.push(value.clone()),
            "--send" => options.declared.send.push(PathBuf::from(value)),
            "--fetch" => options.declared.fetch.push(PathBuf::from(value)),
            "--net" => options.declared.net = Some(Net::parse(value)?.absolute(&absolute(".")?)),
            "--net-secret-file" => options.declared.net_secret_file = Some(absolute(value)?),
            "--bandwidth" => options.declared.bandwidth = Some(units::parse_rate(value)?),
            "--monitor" => options.declared.monitor = Some(value.clone()),
            "--mem" => options.declared.memory = Some(units::parse_bytes(value)?),
            "--pids" => options.declared.pids = Some(number(value)?),
            "--disk" | "--write-budget" => options.declared.disk = Some(units::parse_bytes(value)?),
            "--device" => options.declared.devices.push(value.clone()),
            "--allow-write" => {
                options.declared.confine = true;
                options.declared.allow_write.push(PathBuf::from(value));
            }
            "--time" => options.declared.wall_ms = Some(units::parse_duration_ms(value)?),
            "--budget" if value == "none" => options.budget_ms = UNLIMITED_BUDGET_MS,
            "--budget" => options.budget_ms = units::parse_duration_ms(value)?,
            other => return Err(format!("unknown option {other}\n{USAGE}")),
        }
        i += 2;
    }
    if ssh_key.is_some() || !ssh_options.is_empty() {
        match &mut options.declared.on {
            Some(remote) => {
                remote.key = ssh_key;
                remote.options = ssh_options;
            }
            None => return Err("--ssh-key and --ssh-option go with --on USER@HOST".to_string()),
        }
    }
    if selected_shell.is_some() && explicit_legacy_shell {
        return Err(cli_contract::message(
            "--shell and --legacy-shell are mutually exclusive",
        ));
    }
    if options.json && explicit_summary {
        return Err(cli_contract::message(
            "--summary and --json are mutually exclusive",
        ));
    }
    options.command =
        cli_contract::argv(&options.command, selected_shell.as_deref(), legacy_shell)?;
    Ok(options)
}

fn place(path: &str) -> Result<PathBuf, String> {
    if path.starts_with('~') || path.starts_with('/') {
        Ok(PathBuf::from(path))
    } else {
        absolute(path)
    }
}

fn socket() -> PathBuf {
    paths::client_socket().unwrap_or_else(|_| Store::default_root().join(paths::STATE_SOCKET))
}

fn call(request: Request) -> Result<Response, String> {
    if let Some(answer) = clientif::invocation::exchange(&request) {
        return answer;
    }
    commands::output::contacted();
    Store::validate_default_selection().map_err(|e| e.to_string())?;
    let socket = socket();
    client::call(&socket, &request).map_err(|e| {
        format!(
            "job: the daemon does not answer at {} ({e}); run the command directly, or start the daemon with `jobd`; `job doctor` checks the setup",
            socket.display()
        )
    })
}

fn finish(response: Response, json: bool) -> ExitCode {
    match response {
        Response::Cancellation { operation } => cancellation_result(&operation, json),
        Response::Finished { job } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&job).unwrap_or_default());
            } else {
                let store = Store {
                    root: Store::default_root(),
                };
                println!(
                    "{}",
                    answer::render(&job, &templates::known(&store, &job.key))
                );
                if let Some(line) = cli2::labels::line(&job.spec.declared.labels) {
                    println!("{line}");
                }
            }
            let code = match (&job.stop, job.result.as_ref().and_then(|r| r.exit_code)) {
                (None, Some(code)) => code.clamp(0, 255) as u8,
                _ => 1,
            };
            ExitCode::from(code)
        }
        Response::StillRunning { job } | Response::Submitted { job } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&job).unwrap_or_default());
            } else {
                println!("{}", answer::pending(&job, shim::now_ms(), None));
                if let Some(line) = streams::quota::summary(&job) {
                    println!("{line}");
                }
                if let Some(line) = cli2::labels::line(&job.spec.declared.labels) {
                    println!("{line}");
                }
            }
            ExitCode::from(EXIT_STILL_RUNNING)
        }
        Response::Error { message } => {
            eprintln!("job: {message}");
            ExitCode::from(EXIT_SERVICE_ERROR)
        }
        other => {
            eprintln!("job: unexpected answer {other:?}");
            ExitCode::from(EXIT_SERVICE_ERROR)
        }
    }
}

fn wait_for(id: u64, timeout_ms: u64, json: bool, summary: bool, compatibility: bool) -> ExitCode {
    let started = std::time::Instant::now();
    let mut lost_since: Option<std::time::Instant> = None;
    let mut expected_attempt = None;
    loop {
        if let Some(code) = cli2::interrupt::poll(id, json) {
            return code;
        }
        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let remaining = timeout_ms.saturating_sub(elapsed);
        match client::call(
            &socket(),
            &Request::Wait {
                id,
                timeout_ms: if compatibility {
                    remaining
                } else {
                    remaining.min(1000)
                },
            },
        ) {
            Ok(response) => {
                if !compatibility
                    && let Response::Finished { job } | Response::StillRunning { job } = &response
                {
                    if expected_attempt.is_some_and(|attempt| attempt != job.attempt) {
                        return cli_contract::wait_result(
                            Response::Error {
                                message: cli_contract::message(
                                    "the attempt changed while waiting; inspect the Job and wait again",
                                ),
                            },
                            json,
                            summary,
                            false,
                        );
                    }
                    expected_attempt = Some(job.attempt);
                }
                if let Some(since) = lost_since.take() {
                    eprintln!(
                        "[job] note: the daemon could not be reached for {} while waiting; the wait went on",
                        units::format_duration_ms(
                            u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
                        )
                    );
                }
                if !compatibility
                    && matches!(&response, Response::StillRunning { .. })
                    && started.elapsed().as_millis() < u128::from(timeout_ms)
                {
                    continue;
                }
                return cli_contract::wait_result(response, json, summary, compatibility);
            }
            Err(_) => {
                let since = lost_since.get_or_insert_with(std::time::Instant::now);
                if since.elapsed().as_millis() >= u128::from(RECONNECT_WINDOW_MS)
                    || (!compatibility && elapsed >= timeout_ms)
                {
                    if !compatibility {
                        return cli_contract::unavailable(id, json);
                    }
                    eprintln!(
                        "[job] job {id}: the connection to the daemon was lost while waiting and did not come back within {}; the job may have run, or may still be running. Wait for it again with: job wait {id}",
                        units::format_duration_ms(RECONNECT_WINDOW_MS)
                    );
                    return ExitCode::from(EXIT_STILL_RUNNING);
                }
                std::thread::sleep(std::time::Duration::from_millis(if compatibility {
                    RECONNECT_INTERVAL_MS
                } else {
                    RECONNECT_INTERVAL_MS.min(remaining)
                }));
            }
        }
    }
}

fn parse_id(text: Option<&String>) -> Result<u64, String> {
    let text = text.ok_or("a job id is needed")?;
    text.parse()
        .map_err(|_| format!("`{text}` is not a job id"))
}

fn submit(options: &Options) -> Result<model::Job, String> {
    if !options.compatibility
        && !options.summary
        && !options.json
        && options.declared.terminal.is_none()
    {
        match call(Request::Host)? {
            Response::Host { info } if info.output_protocol == Some(streams::PROTOCOL) => {}
            _ => {
                return Err(streams::message(
                    "update the service before requesting live output",
                ));
            }
        }
    }
    if options.declared.terminal.is_some() {
        match call(Request::Host)? {
            Response::Host { info } if info.terminal_protocol == Some(terminal::PROTOCOL) => {}
            _ => {
                return Err(terminal::message(
                    "the running daemon does not support this terminal protocol; update it before submitting a PTY job",
                    &[],
                ));
            }
        }
    }
    if options.command.is_empty() {
        return Err(format!("no command after --\n{USAGE}"));
    }
    let cwd = clientif::invocation::cwd().map_err(|e| format!("no working directory: {e}"))?;
    let spec = client::spec(
        &options.command,
        cwd,
        options.session.clone(),
        options.declared.clone(),
        options.queue.clone(),
    );
    match call(Request::Submit {
        spec: Box::new(spec),
        env: client::env(),
        idempotency_key: options.idempotency_key.clone(),
    })? {
        Response::Submitted { job } => {
            durability::cli::replay_note(&job);
            Ok(job)
        }
        Response::Error { message } => Err(message),
        other => Err(format!("unexpected answer {other:?}")),
    }
}

fn retry_command(args: &[String]) -> Result<ExitCode, String> {
    let id = parse_id(args.first())?;
    let mut held = false;
    let mut env = None;
    let mut queue = None;
    let mut allow_lost = false;
    let mut json = false;
    let mut options = args[1..].iter();
    while let Some(option) = options.next() {
        match option.as_str() {
            "--hold" => held = true,
            "--current-env" => env = Some(client::env()),
            "--queue" | "-q" => queue = Some(options.next().ok_or_else(|| lifecycle::message("usage: job retry ID [--hold] [--current-env] [--queue PATH] [--allow-lost] [--json]"))?.clone()),
            "--allow-lost" => allow_lost = true,
            "--json" => json = true,
            _ => return Err(lifecycle::message("usage: job retry ID [--hold] [--current-env] [--queue PATH] [--allow-lost] [--json]")),
        }
    }
    let expected_attempt = match call(Request::Status { id })? {
        Response::Finished { job } => job.attempt,
        Response::Error { message } => return Err(message),
        _ => {
            return Err(lifecycle::message(
                "retry requires the expected completed attempt",
            ));
        }
    };
    let request = Request::Retry {
        id,
        expected_attempt,
        held,
        env,
        queue,
        allow_lost,
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match call(request.clone())? {
            Response::Submitted { job } => {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&job).map_err(|e| e.to_string())?
                    );
                } else {
                    println!("{}", job.id);
                }
                return Ok(ExitCode::SUCCESS);
            }
            Response::RetryPending if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            Response::RetryPending => {
                return Err(lifecycle::message(
                    "previous supervisor has not exited; retry was not created",
                ));
            }
            Response::Error { message } => return Err(message),
            other => return Err(format!("unexpected answer {other:?}")),
        }
    }
}

fn freezer_command(
    args: &[String],
    kind: Option<objects::Kind>,
    frozen: bool,
) -> Result<ExitCode, String> {
    let usage = || {
        freezer::message(if kind.is_some() {
            "usage: job queue|group suspend|continue --recursive PATH [--timeout DURATION] [--json]"
        } else {
            "usage: job suspend|continue ID [--timeout DURATION] [--json]"
        })
    };
    let mut path = None;
    let mut recursive = false;
    let mut json = false;
    let mut timeout_ms = 5_000;
    let mut arguments = args.iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--recursive" if kind.is_some() => recursive = true,
            "--json" => json = true,
            "--timeout" => {
                timeout_ms = units::parse_duration_ms(arguments.next().ok_or_else(usage)?)?
            }
            value if !value.starts_with('-') && path.is_none() => path = Some(argument.clone()),
            _ => return Err(usage()),
        }
    }
    let path = path.ok_or_else(usage)?;
    let target = match kind {
        Some(kind) => freezer::Target::Object { kind, path },
        None => freezer::Target::Job {
            id: parse_id(Some(&path))?,
        },
    };
    match call(Request::Freeze {
        target,
        recursive,
        frozen,
        timeout_ms,
    })? {
        Response::Controlled {
            schema_version,
            results,
        } => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"schema_version": schema_version, "results": results})
                );
            } else {
                for result in &results {
                    let status = result.error.clone().unwrap_or_else(|| {
                        freezer::message(if result.confirmed {
                            "confirmed"
                        } else {
                            "pending kernel confirmation"
                        })
                    });
                    if kind.is_some() {
                        println!("{}\t{status}", result.id);
                    } else if !result.confirmed {
                        eprintln!("{}: {status}", result.id);
                    }
                }
            }
            Ok(if results.iter().any(|result| result.error.is_some()) {
                ExitCode::FAILURE
            } else if results.iter().any(|result| !result.confirmed) {
                ExitCode::from(EXIT_STILL_RUNNING)
            } else {
                ExitCode::SUCCESS
            })
        }
        Response::Error { message } => Err(message),
        other => Err(format!("unexpected answer {other:?}")),
    }
}

fn remove_command(args: &[String], kind: Option<objects::Kind>) -> Result<ExitCode, String> {
    let usage = || {
        removal::message(if kind.is_some() {
            "usage: job queue|group remove PATH [--recursive] [--dry-run] [--allow-lost] [--json]"
        } else {
            "usage: job remove ID [--dry-run] [--allow-lost] [--json]"
        })
    };
    let mut path = None;
    let mut recursive = false;
    let mut dry_run = false;
    let mut allow_lost = false;
    let mut json = false;
    for argument in args {
        match argument.as_str() {
            "--recursive" if kind.is_some() => recursive = true,
            "--dry-run" => dry_run = true,
            "--allow-lost" => allow_lost = true,
            "--json" => json = true,
            value if !value.starts_with('-') && path.is_none() => path = Some(argument.clone()),
            _ => return Err(usage()),
        }
    }
    let path = path.ok_or_else(usage)?;
    let target = match kind {
        Some(kind) => removal::Target::Object { kind, path },
        None => removal::Target::Job {
            id: parse_id(Some(&path))?,
        },
    };
    let mut expected = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match call(Request::Remove {
            target: target.clone(),
            recursive,
            allow_lost,
            expected: expected.clone(),
        })? {
            Response::Removed { receipt } => {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&receipt).map_err(|e| e.to_string())?
                    );
                }
                return Ok(ExitCode::SUCCESS);
            }
            Response::RemovalInProgress { receipt, message } => {
                if std::time::Instant::now() >= deadline {
                    if json {
                        println!(
                            "{}",
                            serde_json::json!({"schema_version": 1, "committed": true, "cleanup_pending": true, "receipt": receipt, "message": message})
                        );
                    } else {
                        eprintln!("{message}");
                    }
                    return Ok(ExitCode::from(EXIT_STILL_RUNNING));
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Response::RemovalPreview { preview } => {
                let pending = !preview.ready && preview.issues.iter().all(|issue| issue.pending);
                if dry_run || (!preview.ready && !pending) || std::time::Instant::now() >= deadline
                {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&preview).map_err(|e| e.to_string())?
                        );
                    } else {
                        println!(
                            "{}",
                            removal::message(if preview.ready { "ready" } else { "blocked" })
                        );
                        for job in &preview.selection.jobs {
                            println!(
                                "job\t{}\t{} {}",
                                job.id,
                                removal::message("attempt"),
                                job.attempt
                            );
                        }
                        for object in &preview.selection.objects {
                            println!("{:?}\t{}\t{}", object.kind, object.id, object.path);
                        }
                        for issue in &preview.issues {
                            eprintln!(
                                "{}: {}",
                                issue
                                    .job_id
                                    .map_or_else(|| "-".to_owned(), |id| id.to_string()),
                                issue.message
                            );
                        }
                        for helper in &preview.helpers {
                            println!("{}\t{helper}", removal::message("helper"));
                        }
                    }
                    return Ok(if dry_run || preview.ready {
                        ExitCode::SUCCESS
                    } else if pending {
                        ExitCode::from(EXIT_STILL_RUNNING)
                    } else {
                        ExitCode::FAILURE
                    });
                }
                expected.get_or_insert(preview.selection);
                if pending {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
            Response::Error { message } => return Err(message),
            other => return Err(format!("unexpected answer {other:?}")),
        }
    }
}

fn cancellation_result(operation: &cancellation::Operation, json: bool) -> ExitCode {
    if json {
        println!("{}", serde_json::to_string_pretty(operation).unwrap());
    } else {
        println!(
            "{}: {}",
            cancellation::message(if operation.complete {
                "cancellation recorded"
            } else {
                "cancellation is pending"
            }),
            operation.operation
        );
        for result in &operation.results {
            println!(
                "{}: {:?}{}",
                result.id,
                result.state,
                result
                    .error
                    .as_ref()
                    .map(|e| format!("; {e}"))
                    .unwrap_or_default()
            );
        }
        if let Some(error) = &operation.persistence_error {
            eprintln!("{error}");
        }
    }
    ExitCode::from(if !operation.complete {
        75
    } else {
        u8::from(
            operation
                .results
                .iter()
                .any(|result| result.error.is_some()),
        )
    })
}

fn cancellation_command(args: &[String], kind: objects::Kind) -> Result<ExitCode, String> {
    let usage = || {
        cancellation::message("usage: job queue|group cancel --recursive PATH [--dry-run] [--json]")
    };
    let mut path = None;
    let mut recursive = false;
    let mut dry_run = false;
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--recursive" => recursive = true,
            "--dry-run" => dry_run = true,
            "--json" => json = true,
            value if !value.starts_with('-') && path.is_none() => path = Some(value.to_owned()),
            _ => return Err(usage()),
        }
    }
    if !recursive {
        return Err(usage());
    }
    let target = cancellation::Target::Object {
        kind,
        path: path.ok_or_else(usage)?,
    };
    let selection = match call(Request::CancelSelection {
        target: target.clone(),
        recursive,
        expected: None,
    })? {
        Response::CancellationPreview { selection, .. } => selection,
        Response::Error { message } => return Err(message),
        other => return Ok(finish(other, json)),
    };
    if dry_run {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"schema_version": 1, "selection": selection})
                )
                .unwrap()
            );
        } else {
            for member in &selection.members {
                println!("{}: {:?}", member.id, member.state);
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    match call(Request::CancelSelection {
        target,
        recursive,
        expected: Some(selection),
    })? {
        Response::Cancellation { operation } => Ok(cancellation_result(&operation, json)),
        other => Ok(finish(other, json)),
    }
}

fn main_result() -> Result<ExitCode, String> {
    if service::invoked_as_daemon(std::env::args_os().next().as_deref()) {
        let args: Vec<String> = std::env::args().skip(1).collect();
        return service::run(&args);
    }
    carry_out(&commands::arguments())
}

fn carry_out(args: &[String]) -> Result<ExitCode, String> {
    if let Some(code) = commands::dispatch(args)? {
        return Ok(code);
    }
    let Some((command, rest)) = args.split_first() else {
        return Err(USAGE.to_string());
    };
    match command.as_str() {
        "queue" | "group" if rest.first().is_some_and(|action| action == "cancel") => {
            cancellation_command(
                &rest[1..],
                if command == "queue" {
                    objects::Kind::Queue
                } else {
                    objects::Kind::Group
                },
            )
        }
        "remove" => remove_command(rest, None),
        "queue" | "group" if rest.first().is_some_and(|action| action == "remove") => {
            remove_command(
                &rest[1..],
                Some(if command == "queue" {
                    objects::Kind::Queue
                } else {
                    objects::Kind::Group
                }),
            )
        }
        "suspend" | "continue" => freezer_command(rest, None, command == "suspend"),
        "queue" | "group"
            if rest
                .first()
                .is_some_and(|action| action == "suspend" || action == "continue") =>
        {
            freezer_command(
                &rest[1..],
                Some(if command == "queue" {
                    objects::Kind::Queue
                } else {
                    objects::Kind::Group
                }),
                rest[0] == "suspend",
            )
        }
        "create" | "edit" => {
            let (existing, arguments) = if command == "edit" {
                (Some(parse_id(rest.first())?), &rest[1..])
            } else {
                (None, rest)
            };
            let options = parse_options(arguments)?;
            if existing.is_some() && options.idempotency_key.is_some() {
                return Err(durability::message(
                    "--idempotency-key goes with run, submit and create",
                ));
            }
            if options.command.is_empty() {
                return Err(format!("no command after --\n{USAGE}"));
            }
            let spec = Box::new(client::spec(
                &options.command,
                clientif::invocation::cwd().map_err(|e| e.to_string())?,
                options.session,
                options.declared,
                options.queue,
            ));
            let request = match existing {
                Some(id) => Request::Edit {
                    id,
                    spec,
                    env: client::env(),
                },
                None => Request::Create {
                    spec,
                    env: client::env(),
                    idempotency_key: options.idempotency_key.clone(),
                },
            };
            match call(request)? {
                Response::Submitted { job } => {
                    durability::cli::replay_note(&job);
                    if options.json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&job).map_err(|e| e.to_string())?
                        );
                    } else {
                        println!("{}", job.id);
                    }
                    Ok(ExitCode::SUCCESS)
                }
                Response::Error { message } => Err(message),
                other => Err(format!("unexpected answer {other:?}")),
            }
        }
        "release" => {
            let (rest, json) = cli2::usage::without_json(rest);
            if rest.len() != 1 {
                return Err(lifecycle::message("usage: job release ID"));
            }
            match call(Request::Release {
                id: parse_id(rest.first())?,
            })? {
                Response::Submitted { job } if json => cli2::usage::record(&job),
                Response::Submitted { .. } => Ok(ExitCode::SUCCESS),
                Response::Error { message } => Err(message),
                other => Err(format!("unexpected answer {other:?}")),
            }
        }
        "retry" => retry_command(rest),
        "profile" | "class" => {
            if rest.iter().any(|v| v == "--help" || v == "-h") {
                println!("{}", presets::help());
                return Ok(ExitCode::SUCCESS);
            }
            let words: Vec<_> = rest
                .iter()
                .filter(|v| v.as_str() != "--json")
                .map(String::as_str)
                .collect();
            if !matches!(words.as_slice(), [] | ["list"] | ["show", _]) {
                return Err(presets::help());
            }
            if let ["show", reference] = words.as_slice() {
                presets::reference(reference)?;
            }
            let effective = match call(Request::Config { reload: false })? {
                Response::Configuration { effective } => effective,
                Response::Error { message } => return Err(message),
                other => return Err(format!("unexpected answer {other:?}")),
            };
            let definitions = effective.config.presets.definitions()?;
            let prefix = if command == "class" {
                "class:"
            } else {
                "profile:"
            };
            let values: Vec<_> = definitions.iter().filter(|(key, _)| key.starts_with(prefix)).filter(|(key, _)| words.len() != 2 || **key == format!("{prefix}{}", words[1])).map(|(key, definition)| serde_json::json!({"reference":key,"sha256":definition.digest(),"definition":definition})).collect();
            if words.len() == 2 && values.is_empty() {
                return Err(presets::message("unknown preset revision"));
            }
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"schema_version":1,"definitions":values})
                )
                .map_err(|e| e.to_string())?
            );
            Ok(ExitCode::SUCCESS)
        }
        "pressure" => {
            if rest == ["--help"] {
                println!("{}", pressure::help());
                return Ok(ExitCode::SUCCESS);
            }
            if rest.first().is_some_and(|s| s == "replay") {
                if rest.len() != 2 && !(rest.len() == 3 && rest[2] == "--json") {
                    return Err(pressure::message(
                        "usage: job pressure replay FILE [--json]",
                    ));
                }
                let report = pressure::replay::run(std::path::Path::new(&rest[1]))
                    .map_err(|e| e.to_string())?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
                );
                return Ok(ExitCode::SUCCESS);
            }
            if rest.first().is_some_and(|s| s == "status" || s == "events") {
                if rest.len() != 1 && rest != [rest[0].clone(), "--json".to_owned()] {
                    return Err(pressure::help());
                }
                return match call(Request::PressureControl)? {
                    Response::PressureControl {
                        ledger,
                        error,
                        sampling_interval_ms,
                        notifications,
                    } => {
                        let result = if rest[0] == "events" {
                            serde_json::json!({"schema_version":1,"latest_sequence":ledger.sequence,"events":ledger.events,"error":error})
                        } else {
                            let mode = if notifications.monitors.iter().any(|m| m.registered) {
                                "notifications_and_sampling"
                            } else {
                                "sampling"
                            };
                            serde_json::json!({"schema_version":1,"boot_id":ledger.boot_id,"sampling_interval_ms":sampling_interval_ms,"observation_mode":mode,"notifications":notifications,"rules":ledger.views,"error":error})
                        };
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
                        );
                        Ok(ExitCode::SUCCESS)
                    }
                    Response::Error { message } => Err(message),
                    other => Err(format!("unexpected answer {other:?}")),
                };
            }
            if rest.first().is_some_and(|s| s == "parse") {
                if !(rest.len() == 4 || (rest.len() == 5 && rest[4] == "--host"))
                    || rest[2] != "--resource"
                {
                    return Err(pressure::message(
                        "usage: job pressure parse FILE --resource cpu|memory|io [--host]",
                    ));
                }
                let resource = match rest[3].as_str() {
                    "cpu" => pressure::Resource::Cpu,
                    "memory" => pressure::Resource::Memory,
                    "io" => pressure::Resource::Io,
                    _ => {
                        return Err(pressure::message(
                            "usage: job pressure parse FILE --resource cpu|memory|io [--host]",
                        ));
                    }
                };
                let observation =
                    pressure::parse_file(std::path::Path::new(&rest[1]), resource, rest.len() == 5)
                        .map_err(|e| e.to_string())?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"schema_version":1,"observation":observation})
                    )
                    .map_err(|e| e.to_string())?
                );
                return Ok(ExitCode::SUCCESS);
            }
            let mut id = None;
            let mut json = false;
            for word in rest {
                if word == "--json" && !json {
                    json = true;
                } else if id.is_none() && !word.starts_with('-') {
                    id = Some(parse_id(Some(word))?);
                } else {
                    return Err(pressure::message("usage: job pressure [ID] [--json]"));
                }
            }
            match call(Request::Pressure { id })? {
                Response::Pressure { snapshot } => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&snapshot).map_err(|e| e.to_string())?
                        );
                    } else {
                        println!("{}", snapshot.describe());
                    }
                    Ok(ExitCode::SUCCESS)
                }
                Response::Error { message } => Err(message),
                other => Err(format!("unexpected answer {other:?}")),
            }
        }
        "reprioritize" => {
            if rest == ["--help"] {
                println!("{}", admission::help());
                return Ok(ExitCode::SUCCESS);
            }
            if rest.len() != 3 || rest[1] != "--priority" {
                return Err(admission::message(
                    "usage: job reprioritize ID --priority N",
                ));
            }
            let id = parse_id(rest.first())?;
            let priority = admission::priority(&rest[2])?;
            let job = match call(Request::Status { id })? {
                Response::StillRunning { job } | Response::Finished { job } => job,
                Response::Error { message } => return Err(message),
                other => return Err(format!("unexpected answer {other:?}")),
            };
            match call(Request::Reprioritize {
                id,
                attempt: job.attempt,
                priority,
            })? {
                Response::Submitted { .. } => Ok(ExitCode::SUCCESS),
                Response::Error { message } => Err(message),
                other => Err(format!("unexpected answer {other:?}")),
            }
        }
        "explain" => {
            if rest == ["--help"] {
                println!("{}", admission::help());
                return Ok(ExitCode::SUCCESS);
            }
            if rest.is_empty() || rest.len() > 2 || rest.get(1).is_some_and(|s| s != "--json") {
                return Err(admission::message("usage: job explain ID [--json]"));
            }
            match call(Request::Explain {
                id: parse_id(rest.first())?,
            })? {
                Response::Explanation { explanation } => {
                    if rest.len() == 2 {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&explanation).map_err(|e| e.to_string())?
                        );
                    } else {
                        println!("{}", explanation.describe());
                    }
                    Ok(ExitCode::SUCCESS)
                }
                Response::Error { message } => Err(message),
                other => Err(format!("unexpected answer {other:?}")),
            }
        }
        "update" => resource_update_command(rest, None),
        "resource-update" => {
            if rest.is_empty()
                || rest.len() > 3
                || rest[1..].iter().any(|s| s != "--json" && s != "--abandon")
            {
                return Err(resource_update::message(
                    "usage: job resource-update OPERATION [--abandon] [--json]",
                ));
            }
            match call(Request::ResourceUpdateStatus {
                operation: rest[0].clone(),
                abandon: rest.iter().any(|s| s == "--abandon"),
            })? {
                Response::ResourceUpdate { operation } => {
                    show_resource_update(&operation, rest.iter().any(|s| s == "--json"))
                }
                Response::Error { message } => Err(message),
                other => Err(format!("unexpected answer {other:?}")),
            }
        }
        "attempts" => {
            if rest.is_empty() || rest.len() > 2 || rest.get(1).is_some_and(|arg| arg != "--json") {
                return Err(lifecycle::message("usage: job attempts ID [--json]"));
            }
            match call(Request::Attempts {
                id: parse_id(rest.first())?,
            })? {
                response @ Response::Attempts { .. } if rest.len() == 2 => {
                    if let Response::Attempts {
                        schema_version,
                        attempts,
                    } = response
                    {
                        println!(
                            "{}",
                            serde_json::json!({"schema_version": schema_version, "attempts": attempts})
                        );
                    }
                    Ok(ExitCode::SUCCESS)
                }
                Response::Attempts { attempts, .. } => {
                    for job in attempts {
                        match streams::quota::summary(&job) {
                            Some(line) => println!("{}\t{:?}\t{line}", job.attempt, job.state),
                            None => println!("{}\t{:?}", job.attempt, job.state),
                        }
                    }
                    Ok(ExitCode::SUCCESS)
                }
                Response::Error { message } => Err(message),
                other => Err(format!("unexpected answer {other:?}")),
            }
        }
        "state" => migration::command(rest)
            .map(|()| ExitCode::SUCCESS)
            .map_err(|e| e.to_string()),
        "signal" => signal_command(rest),
        "audit" => durability::cli::audit_command(rest),
        "events" => operations::cli::command(rest),
        "config" => config_command(rest),
        "group" => object_command(objects::Kind::Group, rest),
        "run" => {
            let options = parse_options(rest)?;
            if options.declared.terminal.is_some() {
                terminal::require_terminal()?;
                if options.json {
                    return Err(terminal::message(
                        "--pty cannot be combined with --json",
                        &[],
                    ));
                }
            }
            cli2::run::prepare(
                options.on_interrupt,
                options.declared.terminal.is_some(),
                options.compatibility,
                &options.session,
            )?;
            cli2::stdin::check(&options.declared)?;
            let submitted = submit(&options)?;
            let id = submitted.id;
            if options.declared.terminal.is_some() {
                return terminal::attach(&socket(), id, None).map(ExitCode::from);
            }
            if options.declared.stdin {
                cli2::stdin::pump(id);
            }
            if !options.json && !options.summary && !options.compatibility {
                return streams::cli::live(&submitted, options.budget_ms);
            }
            let timeout_ms = options.budget_ms.saturating_sub(ANSWER_MARGIN_MS);
            Ok(wait_for(
                id,
                timeout_ms,
                options.json,
                true,
                options.compatibility,
            ))
        }
        "attach" => {
            let (id, detach_key) = match rest {
                [id] => (id, None),
                [id, option, key] | [option, key, id] if option == "--detach-key" => {
                    (id, Some(key.as_str()))
                }
                _ => {
                    return Err(terminal::message(
                        "usage: job attach ID [--detach-key KEY|none]",
                        &[],
                    ));
                }
            };
            terminal::attach(&socket(), parse_id(Some(id))?, detach_key).map(ExitCode::from)
        }
        "submit" => {
            let options = parse_options(rest)?;
            let submitted = submit(&options)?;
            if commands::output::enveloped() {
                return cli2::usage::record(&submitted);
            }
            let id = submitted.id;
            println!("{id}");
            Ok(ExitCode::SUCCESS)
        }
        "wait" => {
            if rest == ["--help"] || rest == ["-h"] {
                println!("{}", cli_contract::help());
                return Ok(ExitCode::SUCCESS);
            }
            let options = cli_contract::WaitOptions::parse(rest)?;
            Ok(wait_for(
                options.id,
                options.timeout_ms,
                options.json,
                options.summary,
                options.compatibility,
            ))
        }
        "status" => {
            let id = parse_id(rest.first())?;
            Ok(finish(
                call(Request::Status { id })?,
                rest.iter().any(|a| a == "--json"),
            ))
        }
        "logs" => streams::cli::logs(rest),
        "log" => operations::output::log_command(rest),
        "queue" => queue_command(rest),
        "link-wireguard" => {
            let [interface, config, endpoints @ ..] = rest else {
                return Err("link-wireguard INTERFACE CONFIG [ENDPOINT|-]...".to_string());
            };
            let config = wg::read(std::path::Path::new(config))?;
            let endpoints: Vec<Option<std::net::SocketAddrV4>> =
                endpoints.iter().map(|e| e.parse().ok()).collect();
            wg::configure(interface, &config, &endpoints)
                .map_err(|e| format!("cannot configure {interface}: {e}"))?;
            Ok(ExitCode::SUCCESS)
        }
        "remote" => remote::serve(),
        "host" if rest.first().is_some_and(|a| a == "--on") => {
            let target = rest.get(1).ok_or("--on needs USER@HOST")?;
            let key = rest
                .windows(2)
                .find(|w| w[0] == "--ssh-key")
                .map(|w| absolute(&w[1]))
                .transpose()?;
            let remote = model::Remote {
                target: target.clone(),
                key,
                options: rest
                    .windows(2)
                    .filter(|w| w[0] == "--ssh-option")
                    .map(|w| w[1].clone())
                    .collect(),
            };
            let bytes = remote::call(&remote, remote::Op::Host, None)?;
            let info: model::HostInfo = serde_json::from_slice(&bytes)
                .map_err(|e| format!("{target} answered what this job cannot read: {e}"))?;
            if rest.iter().any(|a| a == "--json") {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&info).unwrap_or_default()
                );
            } else {
                println!("{}", render_host(&info));
            }
            Ok(ExitCode::SUCCESS)
        }
        "host" => match call(Request::Host)? {
            Response::Host { info } => {
                if rest.iter().any(|a| a == "--json") {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&info).unwrap_or_default()
                    );
                } else {
                    println!("{}", render_host(&info));
                }
                Ok(ExitCode::SUCCESS)
            }
            other => Ok(finish(other, false)),
        },
        "screenshot" => screenshot(rest),
        "cancel" => {
            let (word, others) = cli2::usage::leading_id(rest, &["--session"]);
            let id = parse_id(word.as_ref())?;
            let options = parse_options(&others)?;
            match call(Request::Cancel {
                id,
                session: options.session,
            })? {
                Response::Cancelled { job } if options.json => cli2::usage::record(&job),
                Response::Cancelled { job } => {
                    println!(
                        "{}",
                        job.stop
                            .map(|s| s.line)
                            .unwrap_or_else(|| format!("job {id} is being stopped"))
                    );
                    Ok(ExitCode::SUCCESS)
                }
                Response::Cancellation { operation } => {
                    Ok(cancellation_result(&operation, options.json))
                }
                other => Ok(finish(other, false)),
            }
        }
        "daemon" => service::run(rest),
        "doctor" => doctor::command(rest),
        "net" => netpolicy::command(rest),
        "shim" => {
            let value = |name: &str| rest.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
            let root = value("--state").ok_or("shim: --state is needed")?;
            let id = parse_id(value("--id").as_ref())?;
            let cgroup = value("--cgroup").map(PathBuf::from);
            operations::access::supervised(value("--terminal-socket"));
            let store = Store {
                root: PathBuf::from(root),
            };
            service::access::adopt(rest);
            if !durability::launch::admitted(&store, id, value("--gate").as_deref()) {
                return Ok(ExitCode::from(3));
            }
            Ok(ExitCode::from(shim::run(&store, id, cgroup) as u8))
        }
        "hook" => {
            let mut input = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
            let asked = hook::parse(&input)?;
            let answer = match policy_file::load() {
                Err(error) => {
                    log_denial(&asked, "policy-file");
                    hook::Answer::deny(Some("policy-file"), policy_file::broken(&error))
                }
                Ok(loaded) => std::panic::catch_unwind(|| {
                    if let Some(denial) = hook::police(&asked, &loaded.policy) {
                        log_denial(&asked, &denial.rule);
                        return hook::Answer::deny(Some(&denial.rule), denial.reason);
                    }
                    hook::respond(&asked, || {
                        matches!(call(Request::Ping), Ok(Response::Pong { .. }))
                    })
                })
                .unwrap_or_else(|_| match &loaded.file {
                    Some(file) if policy::fallback_check(&asked.command) => {
                        log_denial(&asked, "fallback");
                        hook::Answer::deny(
                            Some("fallback"),
                            policy_file::message(
                                "job: the hook failed while checking this command, and the command matches a known-dangerous pattern, so it is refused; the rules are in {path}",
                                &[("path", file.display().to_string())],
                            ),
                        )
                    }
                    _ => hook::Answer::allow(None),
                }),
            };
            println!(
                "{}",
                serde_json::to_string(&answer).map_err(|e| e.to_string())?
            );
            Ok(ExitCode::SUCCESS)
        }
        "policy" => {
            if rest.iter().any(|word| word == "--show") {
                return policy_file::show();
            }
            let policy = policy_file::load()?.policy;
            let cwd = std::env::current_dir().unwrap_or_default();
            let name = rest.first().cloned();
            for line in std::io::stdin().lines().map_while(Result::ok) {
                let value = serde_json::from_str::<serde_json::Value>(&line).ok();
                let field = |key: &str| {
                    value
                        .as_ref()
                        .and_then(|v| v.get(key))
                        .and_then(|c| c.as_str())
                        .map(str::to_string)
                };
                let command = field("command").unwrap_or_else(|| line.clone());
                let caller = policy::Caller {
                    name: field("caller_name")
                        .or_else(|| field("name"))
                        .or_else(|| name.clone()),
                    cwd: field("cwd").map_or_else(|| cwd.clone(), PathBuf::from),
                };
                match policy.check(&command, &caller) {
                    Some(denial) => println!("deny\t{}", denial.rule),
                    None => println!("allow"),
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        "classify" => {
            for line in std::io::stdin().lines().map_while(Result::ok) {
                let command = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|v| {
                        v.get("command")
                            .and_then(|c| c.as_str())
                            .map(str::to_string)
                    })
                    .unwrap_or(line);
                match hook::classify(&command) {
                    hook::Verdict::Direct => println!("direct"),
                    hook::Verdict::Route { reason } => println!("route\t{reason}"),
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            println!("\n{}", resource_policy::help());
            println!("\n{}", resource_update::help());
            println!("\n{}", admission::help());
            println!("\n{}", pressure::help());
            println!("\n{}", presets::help());
            println!("\n{}", cli_contract::help());
            println!("\n{}", streams::cli::help());
            println!("\n{}", process_policy::help());
            println!("\n{}", security::help());
            println!("\n{}", isolation::help());
            println!("\n{}", streams::quota::help());
            println!("\n{}", terminal::help());
            println!("\n{}", netsecret::help());
            println!("\n{}", durability::cli::help());
            Ok(ExitCode::SUCCESS)
        }
        other => Err(format!("unknown command `{other}`\n{USAGE}")),
    }
}

fn queue_command(rest: &[String]) -> Result<ExitCode, String> {
    if rest.first().is_some_and(|v| {
        matches!(
            v.as_str(),
            "create"
                | "list"
                | "show"
                | "rename"
                | "move"
                | "unset"
                | "close"
                | "open"
                | "remove"
                | "update"
        )
    }) {
        return object_command(objects::Kind::Queue, rest);
    }
    if rest.first().is_some_and(|v| v == "set")
        && (rest.get(1).is_some_and(|v| v.starts_with("--"))
            || rest.iter().any(|v| {
                v == "--max-running"
                    || v == "--label"
                    || v == "--pressure"
                    || v == "--job-execution-profile"
                    || v == "--job-class"
                    || v.starts_with("--job-cpu-")
                    || v.starts_with("--job-memory-")
                    || v == "--job-pids-max"
                    || v == "--job-numa-policy"
                    || v == "--job-rlimit"
                    || matches!(
                        v.as_str(),
                        "--job-no-new-privs" | "--job-cap-drop" | "--job-seccomp-deny"
                    )
                    || v.strip_prefix("--job-")
                        .is_some_and(|key| isolation::OPTIONS.contains(&key))
                    || streams::quota::default_option(v)
                    || v.starts_with("--job-io-")
                    || v.strip_prefix("--")
                        .and_then(admission::option_key)
                        .is_some()
                    || v.strip_prefix("--")
                        .and_then(resource_policy::option_key)
                        .is_some_and(aggregate::key)
            }))
    {
        return object_command(objects::Kind::Queue, rest);
    }
    if rest
        .first()
        .is_some_and(|verb| verb == "pause" || verb == "resume")
        && rest.iter().any(|word| word == "--json")
    {
        return object_command(objects::Kind::Queue, rest);
    }
    let words: Vec<&str> = rest
        .iter()
        .map(String::as_str)
        .filter(|word| !(clientif::invocation::active() && *word == "--json"))
        .collect();
    let request = match words.as_slice() {
        [] => return show_queues(None),
        ["add", name, flags @ ..] => Request::QueueAdd {
            name: name.to_string(),
            change: queue_change(flags)?,
        },
        ["set", name, flags @ ..] => {
            commands::listed(&["queue", "set"], flags)?;
            let change = queue_change(flags)?;
            if change == QueueChange::default() {
                return Err(format!(
                    "nothing to change; write: job queue set {name} --parallel N"
                ));
            }
            Request::QueueSet {
                name: name.to_string(),
                change,
            }
        }
        ["pause", name] | ["resume", name] => Request::QueueSet {
            name: name.to_string(),
            change: QueueChange {
                paused: Some(words[0] == "pause"),
                ..QueueChange::default()
            },
        },
        ["clear", name] => Request::QueueClear {
            name: name.to_string(),
        },
        ["rm", name] => Request::QueueRemove {
            name: name.to_string(),
            drain: false,
        },
        ["rm", name, "--when-empty"] => Request::QueueRemove {
            name: name.to_string(),
            drain: true,
        },
        [name] if !name.starts_with('-') && daemon::check_queue_name(name).is_ok() => {
            return show_queues(Some(name));
        }
        _ => {
            return Err(format!(
                "cannot read `job queue {}`\n{USAGE}",
                words.join(" ")
            ));
        }
    };
    match call(request)? {
        Response::Done { line } => {
            println!("{line}");
            Ok(ExitCode::SUCCESS)
        }
        other => Ok(finish(other, false)),
    }
}

fn screenshot(rest: &[String]) -> Result<ExitCode, String> {
    let mut monitor: Option<String> = None;
    let mut job: Option<u64> = None;
    let mut pid: Option<i32> = None;
    let mut output: Option<PathBuf> = None;
    let mut display_name: Option<String> = None;
    let mut i = 0;
    while i < rest.len() {
        let value = rest.get(i + 1);
        match rest[i].as_str() {
            "--job" => job = Some(parse_id(value)?),
            "--pid" => {
                pid = Some(
                    value
                        .ok_or("--pid needs a process id")?
                        .parse()
                        .map_err(|_| "--pid needs a process id".to_string())?,
                )
            }
            "-o" | "--output" => output = Some(absolute(value.ok_or("-o needs a file")?)?),
            "--display" => display_name = value.cloned(),
            other if !other.starts_with('-') && monitor.is_none() => {
                monitor = Some(other.to_string());
                i += 1;
                continue;
            }
            other => return Err(format!("cannot read `{other}`\n{USAGE}")),
        }
        i += 2;
    }
    let display = display::open(display_name.as_deref()).map_err(|e| e.to_string())?;
    let stamp = shim::now_ms();
    let owner = match (job, pid) {
        (Some(id), _) => {
            let store = Store {
                root: Store::default_root(),
            };
            let record = store
                .load_job(id)
                .ok_or_else(|| format!("there is no job {id}"))?;
            let shim = record
                .shim_pid
                .filter(|_| record.state.active())
                .ok_or_else(|| format!("job {id} is not running, so it has no window"))?;
            Some((format!("job {id}"), format!("job-{id}"), shim))
        }
        (None, Some(pid)) => Some((format!("process {pid}"), format!("pid-{pid}"), pid)),
        (None, None) => None,
    };
    if let Some((what, stem, shim)) = owner {
        let mut pids: Vec<i32> = procs::descendants(shim, &procs::all())
            .iter()
            .map(|s| s.pid)
            .collect();
        pids.push(shim);
        let windows = display.windows_of(&pids).map_err(|e| e.to_string())?;
        let Some(&window) = windows.first() else {
            return Err(format!("{what} has no window on {}", display.name));
        };
        let path = output.unwrap_or_else(|| PathBuf::from(format!("{stem}-{stamp}.png")));
        let (width, height) = display
            .capture_window(window, &path)
            .map_err(|e| e.to_string())?;
        println!(
            "wrote {}: window 0x{window:x} of {what}, {width}x{height}{}",
            path.display(),
            if windows.len() > 1 {
                format!("; it has {} windows, this is the first", windows.len())
            } else {
                String::new()
            }
        );
        return Ok(ExitCode::SUCCESS);
    }
    let monitors = display.monitors().map_err(|e| e.to_string())?;
    let chosen = match &monitor {
        Some(wanted) => display::choose(&monitors, wanted)?.clone(),
        None => {
            let right = monitors
                .iter()
                .map(|m| i32::from(m.x) + i32::from(m.width))
                .max();
            let bottom = monitors
                .iter()
                .map(|m| i32::from(m.y) + i32::from(m.height))
                .max();
            display::Monitor {
                name: "screen".to_string(),
                x: 0,
                y: 0,
                width: right.unwrap_or(0) as u16,
                height: bottom.unwrap_or(0) as u16,
                primary: false,
            }
        }
    };
    let path =
        output.unwrap_or_else(|| PathBuf::from(format!("screenshot-{}-{stamp}.png", chosen.name)));
    display
        .capture_monitor(&chosen, &path)
        .map_err(|e| e.to_string())?;
    println!(
        "wrote {}: {} {} of {}",
        path.display(),
        if chosen.name == "screen" {
            "the whole screen"
        } else {
            "monitor"
        },
        if chosen.name == "screen" {
            chosen.geometry()
        } else {
            format!("{} {}", chosen.name, chosen.geometry())
        },
        display.name
    );
    Ok(ExitCode::SUCCESS)
}

fn queue_change(flags: &[&str]) -> Result<QueueChange, String> {
    let mut change = QueueChange::default();
    let mut rest = flags;
    while let [flag, value, tail @ ..] = rest {
        match *flag {
            "--parallel" if *value == "all" => change.parallel = Some(Parallel::All),
            "--parallel" => match value.parse::<u64>() {
                Ok(n) if n >= 1 => change.parallel = Some(Parallel::Jobs(n)),
                _ => {
                    return Err(format!(
                        "--parallel: `{value}` is not a number of jobs; write a whole number from 1, or all"
                    ));
                }
            },
            "--dir" => change.settings.dir = Some(place(value)?),
            "--on" => {
                change.settings.on = Some(model::Remote {
                    target: value.to_string(),
                    key: None,
                    options: Vec::new(),
                })
            }
            "--ssh-option" => match &mut change.settings.on {
                Some(remote) => remote.options.push(value.to_string()),
                None => return Err("--ssh-option goes after --on USER@HOST".to_string()),
            },
            "--ssh-key" => match &mut change.settings.on {
                Some(remote) => remote.key = Some(absolute(value)?),
                None => return Err("--ssh-key goes after --on USER@HOST".to_string()),
            },
            "--cores" => change.settings.cores_milli = Some(parse_cores(value)?),
            "--mem" => change.settings.memory = Some(units::parse_bytes(value)?),
            "--net" => change.settings.net = Some(Net::parse(value)?.absolute(&absolute(".")?)),
            "--net-secret-file" => change.settings.net_secret_file = Some(absolute(value)?),
            "--bandwidth" => change.settings.bandwidth = Some(units::parse_rate(value)?),
            "--monitor" => change.settings.monitor = Some(value.to_string()),
            "--host-cores" => change.settings.host_cores_milli = Some(parse_cores(value)?),
            "--job-bandwidth" => change.settings.job_bandwidth = Some(units::parse_rate(value)?),
            other => {
                return Err(format!(
                    "cannot read `{other}`; a queue takes --parallel N|all, --dir DIR, --cores N, --mem SIZE, --net default|none, --bandwidth RATE, --job-bandwidth RATE, --monitor M and --host-cores N"
                ));
            }
        }
        rest = tail;
    }
    if let [flag] = rest {
        return Err(format!("{flag} needs a value"));
    }
    Ok(change)
}

fn absolute(path: &str) -> Result<PathBuf, String> {
    let cwd = clientif::invocation::cwd().map_err(|e| format!("no working directory: {e}"))?;
    Ok(cwd.join(path))
}

fn parse_cores(value: &str) -> Result<u64, String> {
    let cores: f64 = value
        .parse()
        .map_err(|_| format!("--cores: `{value}` is not a number"))?;
    Ok((cores * 1000.0).round() as u64)
}

fn show_queues(name: Option<&str>) -> Result<ExitCode, String> {
    match call(Request::Queue)? {
        Response::Queue { view } => {
            if let Some(name) = name
                && !view.queues.iter().any(|q| q.name == name)
            {
                return Err(format!(
                    "there is no queue {name}; create it with: job queue add {name}"
                ));
            }
            println!("{}", render_queue(&view, name));
            Ok(ExitCode::SUCCESS)
        }
        other => Ok(finish(other, false)),
    }
}

fn render_host(info: &model::HostInfo) -> String {
    let mut lines = vec![
        format!(
            "host {}: job {}, protocol {}, Linux {}",
            info.hostname, info.version, info.protocol, info.kernel
        ),
        format!(
            "{}, {} memory, {} available now",
            resources::format_cores(info.cores * resources::MILLI),
            units::format_bytes(info.memory_total),
            units::format_bytes(info.memory_available)
        ),
        format!(
            "{}; pool {}, {} memory, {} processes",
            info.backend.describe(),
            resources::format_cores(info.pool.cores_milli),
            units::format_bytes(info.pool.memory),
            info.pool.pids
        ),
        format!(
            "devices: {}",
            if info.devices.is_empty() {
                "none".to_string()
            } else {
                info.devices.join(", ")
            }
        ),
    ];
    for device in info.io_devices.iter().filter(|device| device.whole_disk) {
        lines.push(io_policy::describe(device));
    }
    lines.extend(isolation::describe_host(info));
    lines.extend(streams::budget::describe(&info.unreadable_records));
    for queue in &info.queues {
        lines.push(format!(
            "queue {}: {}",
            queue.name,
            daemon::describe_queue(queue)
        ));
    }
    lines.push(format!(
        "{} running, {} waiting",
        info.running, info.waiting
    ));
    lines.extend(operations::render_host(info));
    lines.join("\n")
}

fn log_denial(call: &hook::Call, rule: &str) {
    let entry = serde_json::json!({
        "at_ms": shim::now_ms(),
        "session": call.caller,
        "cwd": call.cwd,
        "rule": rule,
        "command": netsecret::redact(&call.command),
    });
    let path = Store::default_root().join("denials.jsonl");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = std::io::Write::write_all(&mut file, format!("{entry}\n").as_bytes());
    }
}

fn render_queue(view: &model::QueueView, only: Option<&str>) -> String {
    let now = shim::now_ms();
    let mut lines = vec![format!(
        "{}; pool {}, {} memory",
        view.backend.describe(),
        resources::format_cores(view.pool.cores_milli),
        units::format_bytes(view.pool.memory)
    )];
    for queue in view
        .queues
        .iter()
        .filter(|q| only.is_none_or(|name| q.name == name))
    {
        let count = |state: State| {
            view.jobs
                .iter()
                .filter(|e| {
                    (if state == State::Running {
                        e.job.state.active()
                    } else {
                        e.job.state.editable()
                    }) && e.job.spec.queue.as_ref() == Some(&queue.name)
                })
                .count()
        };
        lines.push(format!(
            "queue {}: {}; {} running, {} waiting",
            queue.name,
            daemon::describe_queue(queue),
            count(State::Running),
            count(State::Queued)
        ));
    }
    let jobs: Vec<&model::QueueEntry> = view
        .jobs
        .iter()
        .filter(|e| only.is_none_or(|name| e.job.spec.queue.as_deref() == Some(name)))
        .collect();
    if jobs.is_empty() {
        lines.push("no jobs are queued or running".to_string());
    }
    for entry in jobs {
        let job = &entry.job;
        let when =
            match (&job.state, entry.predicted_start_ms) {
                (State::Held, _) => lifecycle::message("held"),
                (State::Starting, _) => lifecycle::message("starting"),
                (State::Stopping, _) => lifecycle::message("stopping"),
                (State::Suspended, _) => lifecycle::message("suspended"),
                (State::Running, _) => format!(
                    "running {}",
                    units::format_duration_ms(now.saturating_sub(
                        job.started_ms.or(job.durability.admitted_ms).unwrap_or(now)
                    ))
                ),
                (_, Some(start)) => format!(
                    "starts in about {}",
                    units::format_duration_ms(start.saturating_sub(now))
                ),
                (_, None) => "start unknown".to_string(),
            };
        let when = match (&job.state, &job.waited_for) {
            (State::Queued, Some(reason)) => format!("{when}, waiting for {reason}"),
            _ => when,
        };
        let queue = job
            .spec
            .queue
            .as_ref()
            .map(|q| format!("in {q}, "))
            .unwrap_or_default();
        lines.push(format!(
            "{:>5}  {:<14}  {queue}{}  [{}]  {}",
            job.id,
            job.spec.session.chars().take(14).collect::<String>(),
            when,
            job.reservation.describe(),
            estimate::command_text(&job.spec.argv)
        ));
    }
    lines.join("\n")
}

fn main() -> ExitCode {
    match commands::output::run(main_result) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("job: {message}");
            ExitCode::from(EXIT_SERVICE_ERROR)
        }
    }
}

fn show_resource_update(op: &resource_update::Operation, json: bool) -> Result<ExitCode, String> {
    use resource_update::{Phase, message};
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(op).map_err(|e| e.to_string())?
        );
    } else {
        let state = match op.phase {
            Phase::Applied => "applied",
            Phase::Ended => "ended",
            _ => "pending",
        };
        println!(
            "{} {}: {} ({}/{})",
            message("resource update"),
            op.plan.operation,
            message(state),
            op.completed_steps,
            op.plan.steps.len()
        );
        if let Some(error) = &op.error {
            eprintln!("{error}");
        }
    }
    Ok(match op.phase {
        Phase::Applied => ExitCode::SUCCESS,
        Phase::Ended => ExitCode::FAILURE,
        _ => ExitCode::from(EXIT_STILL_RUNNING),
    })
}

fn resource_update_command(
    args: &[String],
    kind: Option<objects::Kind>,
) -> Result<ExitCode, String> {
    if args == ["--help"] {
        println!("{}", resource_update::help());
        return Ok(ExitCode::SUCCESS);
    }
    let usage = || {
        resource_update::message(if kind.is_some() {
            "usage: job queue|group update PATH [CONTROLS] [--allow-oom] [--dry-run] [--json]"
        } else {
            "usage: job update ID [CONTROLS] [--allow-oom] [--dry-run] [--json]"
        })
    };
    let mut path = None;
    let mut patch = std::collections::BTreeMap::new();
    let (mut allow_oom, mut dry_run, mut json) = (false, false, false);
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--allow-oom" => allow_oom = true,
            "--dry-run" => dry_run = true,
            "--json" => json = true,
            value if value.starts_with("--") => {
                index += 1;
                let raw = args.get(index).ok_or_else(usage)?;
                let (key, value) =
                    resource_policy::parse_option(&value[2..], raw).ok_or_else(usage)??;
                if !aggregate::key(&key) {
                    return Err(usage());
                }
                io_policy::insert(&mut patch, key.to_owned(), value)?;
            }
            _ if path.is_none() => path = Some(args[index].clone()),
            _ => return Err(usage()),
        }
        index += 1;
    }
    let path = path.ok_or_else(usage)?;
    let target = match kind {
        Some(kind) => resource_update::Target::Object { kind, path },
        None => resource_update::Target::Job {
            id: parse_id(Some(&path))?,
        },
    };
    let plan = match call(Request::ResourceUpdate {
        target: target.clone(),
        patch: patch.clone(),
        allow_oom,
        expected: None,
    })? {
        Response::ResourceUpdatePreview { plan } => plan,
        Response::Error { message } => return Err(message),
        other => return Err(format!("unexpected answer {other:?}")),
    };
    if dry_run {
        if json {
            println!("{}", serde_json::json!({"schema_version": 1, "plan": plan}));
        } else {
            println!(
                "{} {}: {}",
                resource_update::message("resource update"),
                plan.operation,
                resource_update::message("preview")
            );
            for step in &plan.steps {
                println!("{}: {} -> {}", step.file, step.before, step.after);
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    let operation_id = plan.operation.clone();
    let diagnostic = |error| {
        format!(
            "{} {operation_id}: {error}",
            resource_update::message("resource update")
        )
    };
    let mut response = call(Request::ResourceUpdate {
        target,
        patch,
        allow_oom,
        expected: Some(plan),
    })
    .map_err(&diagnostic)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match response {
            Response::ResourceUpdate { operation } => {
                if !operation.pending()
                    || operation.error.is_some()
                    || std::time::Instant::now() >= deadline
                {
                    return show_resource_update(&operation, json);
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
                response = call(Request::ResourceUpdateStatus {
                    operation: operation.plan.operation,
                    abandon: false,
                })
                .map_err(&diagnostic)?;
            }
            Response::Error { message } => return Err(diagnostic(message)),
            other => return Err(format!("unexpected answer {other:?}")),
        }
    }
}

fn object_command(kind: objects::Kind, rest: &[String]) -> Result<ExitCode, String> {
    commands::check_object(kind, rest)?;
    if rest.first().is_some_and(|s| s == "update") {
        return resource_update_command(&rest[1..], Some(kind));
    }
    use objects::Operation;
    use std::collections::BTreeMap;
    let verb = rest.first().map(String::as_str).unwrap_or("list");
    let mut operands = Vec::new();
    let mut config = BTreeMap::new();
    let mut parent = None;
    let mut labels = BTreeMap::new();
    let mut index = 1;
    let json = rest.iter().any(|v| v == "--json");
    while index < rest.len() {
        let word = &rest[index];
        if word == "--json" {
            index += 1;
            continue;
        }
        if word.starts_with("--") {
            let value = rest
                .get(index + 1)
                .ok_or_else(|| format!("{word} needs a value"))?;
            if word == "--group" {
                parent = Some(value.clone());
            } else if word == "--label" {
                cli2::labels::insert(&mut labels, value)?;
            } else if word == "--job-execution-profile" || word == "--job-class" {
                presets::reference(value)?;
                config.insert(
                    if word == "--job-class" {
                        "job_scheduling_class"
                    } else {
                        "job_execution_profile"
                    }
                    .to_owned(),
                    serde_json::Value::from(value.clone()),
                );
            } else if let Some(result) = word
                .strip_prefix("--job-")
                .and_then(|key| process_policy::parse_option(key, value))
            {
                let (key, value) = result?;
                if config.insert(format!("job_{key}"), value).is_some() {
                    return Err(process_policy::message("duplicate process control"));
                }
            } else if let Some(result) = word
                .strip_prefix("--job-")
                .and_then(|key| security::parse_option(key, value))
            {
                let (key, value) = result?;
                if config.insert(format!("job_{key}"), value).is_some() {
                    return Err(security::message("duplicate security control"));
                }
            } else if let Some(result) = word
                .strip_prefix("--job-")
                .and_then(|key| isolation::parse_option(key, value))
            {
                let (key, value) = result?;
                if config.insert(format!("job_{key}"), value).is_some() {
                    return Err(isolation::message("duplicate isolation control"));
                }
            } else if let Some(result) = word
                .strip_prefix("--job-")
                .and_then(|key| streams::quota::parse_option(key, value))
            {
                let (key, value) = result?;
                if config.insert(format!("job_{key}"), value).is_some() {
                    return Err(streams::message("duplicate output quota"));
                }
            } else if word == "--pressure" {
                let rules: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(clientif::invocation::file(value)?)
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                pressure::control::validate_local(&rules)?;
                config.insert("pressure".to_owned(), rules);
            } else if let Some(result) = word
                .strip_prefix("--")
                .and_then(|name| admission::parse(name, value))
            {
                let (key, value) = result?;
                config.insert(key, value);
            } else if let Some(result) = word
                .strip_prefix("--job-")
                .and_then(|name| resource_policy::parse_option(name, value))
            {
                let (key, value) = result?;
                io_policy::insert(&mut config, format!("job_{key}"), value)?;
            } else if let Some(result) = word
                .strip_prefix("--")
                .filter(|name| resource_policy::option_key(name).is_some_and(aggregate::key))
                .and_then(|name| resource_policy::parse_option(name, value))
            {
                let (key, value) = result?;
                io_policy::insert(&mut config, key.to_owned(), value)?;
            } else {
                let key = match word.as_str() {
                    "--max-running" | "--parallel" => "max_running",
                    "--cores" => "cores_milli",
                    "--mem" => "memory",
                    "--dir" => "dir",
                    "--net" => "net",
                    "--net-secret-file" => "net_secret_file",
                    other => return Err(format!("unknown setting {other}")),
                };
                let parsed = match key {
                    "max_running" if value == "unlimited" || value == "all" => {
                        serde_json::Value::from("unlimited")
                    }
                    "max_running" => serde_json::Value::from(
                        value
                            .parse::<u64>()
                            .map_err(|_| "max-running needs a positive integer")?,
                    ),
                    "cores_milli" => serde_json::Value::from(parse_cores(value)?),
                    "memory" => serde_json::Value::from(units::parse_bytes(value)?),
                    "dir" => serde_json::to_value(place(value)?).map_err(|e| e.to_string())?,
                    "net" => serde_json::to_value(Net::parse(value)?).map_err(|e| e.to_string())?,
                    "net_secret_file" => {
                        serde_json::to_value(absolute(value)?).map_err(|e| e.to_string())?
                    }
                    _ => unreachable!(),
                };
                config.insert(key.to_owned(), parsed);
            }
            index += 2;
        } else {
            operands.push(word.clone());
            index += 1;
        }
    }
    if !labels.is_empty() {
        config.insert(
            cli2::labels::CONFIG_KEY.to_owned(),
            serde_json::to_value(labels).map_err(|e| e.to_string())?,
        );
    }
    let path = || {
        operands
            .first()
            .cloned()
            .ok_or_else(|| "an object path is required".to_owned())
    };
    let operation = match verb {
        "list" => Operation::List,
        "show" => Operation::Show { path: path()? },
        "create" => Operation::Create {
            path: match parent {
                Some(parent) => format!("{}/{}", parent.trim_end_matches('/'), path()?),
                None => path()?,
            },
            config,
        },
        "set" => Operation::Set {
            path: path()?,
            config,
        },
        "unset" => Operation::Unset {
            path: path()?,
            keys: operands
                .iter()
                .skip(1)
                .map(|s| {
                    if s == "job-class" {
                        return "job_scheduling_class".to_owned();
                    }
                    if s.starts_with(cli2::labels::UNSET_PREFIX) {
                        return s.clone();
                    }
                    if let Some(key) = streams::quota::unset_key(s) {
                        return key;
                    }
                    s.strip_prefix("job-")
                        .and_then(resource_policy::option_key)
                        .map_or_else(
                            || {
                                admission::option_key(s)
                                    .or_else(|| resource_policy::option_key(s))
                                    .filter(|key| aggregate::key(key) || admission::key(key))
                                    .map_or_else(|| s.replace('-', "_"), str::to_owned)
                            },
                            |key| format!("job_{key}"),
                        )
                })
                .collect(),
        },
        "rename" => Operation::Rename {
            path: path()?,
            name: operands.get(1).cloned().ok_or("a new name is required")?,
        },
        "move" => Operation::Move {
            path: path()?,
            parent: parent.ok_or("move requires --group PATH")?,
        },
        "pause" | "resume" => Operation::Pause {
            path: path()?,
            paused: verb == "pause",
        },
        "close" | "open" => Operation::Close {
            path: path()?,
            closed: verb == "close",
        },
        "remove" | "rm" => Operation::Remove { path: path()? },
        _ => return Err(format!("unknown object operation {verb}")),
    };
    match call(Request::Object { kind, operation })? {
        Response::Objects {
            schema_version,
            objects,
        } => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"schema_version":schema_version,"objects":objects})
                    )
                    .map_err(|e| e.to_string())?
                );
            } else {
                for object in objects {
                    println!(
                        "{}\t{}\t{:?}\t{}",
                        object.object.id,
                        object.path,
                        object.object.kind,
                        serde_json::to_string(&object.object.config).map_err(|e| e.to_string())?
                    );
                    for line in object.depth.iter().flat_map(operations::depth::render) {
                        println!("{line}");
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        other => Ok(finish(other, json)),
    }
}

fn config_command(rest: &[String]) -> Result<ExitCode, String> {
    let words = rest.iter().map(String::as_str).collect::<Vec<_>>();
    match words.as_slice() {
        ["check", path] => {
            config::Config::parse(&std::fs::read_to_string(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            println!(
                "{}",
                config::message(
                    "configuration valid: {path}",
                    &[("path", (*path).to_owned())]
                )
            );
        }
        ["init", "--profile", profile, path] => {
            let profile = match *profile {
                "ordinary" => config::Profile::Ordinary,
                "legacy" => config::Profile::Legacy,
                _ => return Err(config::message("profile must be ordinary or legacy", &[])),
            };
            config::Config::initialize(std::path::Path::new(path), profile)
                .map_err(|e| e.to_string())?;
            println!(
                "{}",
                config::message(
                    "created {path}; set JOB_CONFIG to this file when starting the daemon",
                    &[("path", (*path).to_owned())]
                )
            );
        }
        ["show"] | ["show", "--json"] | ["reload"] => {
            match call(Request::Config {
                reload: words[0] == "reload",
            })? {
                Response::Configuration { effective } => println!(
                    "{}",
                    serde_json::to_string_pretty(&effective).map_err(|e| e.to_string())?
                ),
                Response::Error { message } => return Err(message),
                other => {
                    return Err(config::message(
                        "unexpected configuration response {response}",
                        &[("response", format!("{other:?}"))],
                    ));
                }
            }
        }
        _ => {
            return Err(config::message(
                "usage: job config check FILE | init --profile ordinary|legacy FILE | show [--json] | reload",
                &[],
            ));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn signal_command(rest: &[String]) -> Result<ExitCode, String> {
    let (id, signal) = match rest {
        [id] => (id, libc::SIGTERM),
        [option, signal, id] if option == "-s" || option == "--signal" => {
            (id, process::parse_signal(signal)?)
        }
        _ => return Err(process::message("usage: job signal [-s SIGNAL] ID", &[])),
    };
    let id = parse_id(Some(id))?;
    match call(Request::Signal { id, signal })? {
        Response::Signalled { .. } => Ok(ExitCode::SUCCESS),
        Response::Error { message } => Err(message),
        other => Err(process::message(
            "unexpected signal response: {response}",
            &[("response", format!("{other:?}"))],
        )),
    }
}
