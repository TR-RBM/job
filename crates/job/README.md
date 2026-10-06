# job

## What it does

One program containing the service, the per-Job supervisor and the command-line client. Invoked as `jobd` (or `job daemon`) it is the service; invoked as `job` it is the client; the service starts a supervisor from its own image for every Job. State schema 18, protocol 20.

The user documentation is in `docs/` at the repository root, starting with `docs/user-guide.md`. `docs/reference/syntax.md` is generated from the command table in `src/commands/` and lists every command and option. This page says where things are in the code.

### Commands

- `job run -- COMMAND...` submits literal argv and passes stdout and stderr through while it waits. `--shell SHELL` selects a shell explicitly; `--summary` and `--json` select final result views; `JOB_CLI_COMPAT=legacy` keeps the earlier single-argument Bash rule and summary output. The first SIGINT or SIGTERM is forwarded to the Job unless `--on-interrupt` says otherwise, and `--stdin` passes standard input.
- `job submit`, `job create`, `job edit`, `job release`, `job retry`, `job move`, `job wait`, `job cancel`, `job signal`, `job suspend`, `job continue`, `job update` and `job remove` manage a Job before, during and after execution.
- `job list`, `job show` (earlier `job status`), `job explain`, `job attempts`, `job logs`, `job log`, `job events`, `job audit`, `job pressure` and `job host` inspect.
- `job queue ...` and `job group ...` organize Queues and Groups and carry settings; `job profile` and `job class` show versioned presets.
- `job attach ID` joins the shared terminal of a Job started with `--pty`.
- `job config`, `job state`, `job doctor`, `job completion` and `job help` are for setup and maintenance. `job state` and `job doctor` work without the service.
- `job screenshot` and `--monitor` are the optional desktop functions.

### Automation

`job hook`, `job classify` and `job policy` serve a program that runs shell commands for somebody else and are not part of ordinary execution. `job hook` answers one JSON question about a command line with allow, deny or rewrite: it rewrites a heavy command to `job run` and refuses commands the command policy forbids. `job classify` prints whether a line is heavy; `job policy [NAME]` prints the policy's verdict for commands given as lines, and `job policy --show` the policy file in use. The policy is read at run time from `policy.json` in the configuration directory, and none is built in. They are left out of everyday help and described in `docs/integrations.md`, together with the diagnostic summary (`filter.rs`, `templates.rs`).

### Service profiles and enforcement

The ordinary service profile uses only declared requests, starts eligible work without predicted backfill slots, and enables no implicit process, memory, swap or pressure policy. The explicit legacy profile reserves cores, memory, processes, written bytes and exclusive devices per Job from declarations, from earlier runs of the same command, from the cargo rule or from a default, starts a Job only where its reservation fits, and stops a Job that passes what was declared or that the host can no longer hold, with one line saying why. `job config show --json` reports the active profile and its source.

With a delegated cgroup v2 tree, limits are enforced by the kernel. Without one the service runs in monitoring mode: it watches each Job's process tree and every answer says `limits watched, not enforced`. The cgroup root comes from `JOB_CGROUP_ROOT` or `[cgroup] root`, else from the delegated cgroup the service was started in, else from the earlier fixed path `/sys/fs/cgroup/exec`.

State lives in `$JOB_STATE_DIR`, else `$XDG_STATE_HOME/job`, else `~/.local/state/job`, or `/var/lib/job` for a system service. Default discovery refuses to choose while a store of the earlier tool is present; `job state migrate` converts it offline.

Exit status of `job run` and `job wait`: the command's own on a normal exit; 128 plus the signal number when a signal ended it; 1 for cancellation and other managed stops; 75 when a deadline passed, the service stayed unavailable for 60 seconds, or `run` left the Job running after an interrupt; 125 when the service failed, the Job was lost or could not start, or the arguments were refused.

### Source files

| File | Content |
|---|---|
| `main.rs` | the client: argument parsing for the commands not yet under `commands/`, and dispatch |
| `daemon.rs` | the service's request handlers, admission loop, launch, recovery and the emergency stop of the legacy profile |
| `shim.rs` | the supervisor: starts the command, records output and the result, serves the terminal |
| `model.rs` | the protocol and the persisted types |
| `store.rs` | the state directory: Job records, links, history, the lock |
| `schedule.rs`, `estimate.rs` | reservation-based admission and the estimator of the legacy profile |
| `client.rs`, `query.rs`, `answer.rs` | the socket client, log queries and the text of an answer |
| `filter.rs`, `templates.rs`, `logfile.rs` | the diagnostic summary and the combined head and tail log |
| `cgroup.rs`, `procs.rs` | the cgroup backend and the process-tree watcher |
| `resources.rs`, `host.rs` | reservation vectors with their sources, and the host's memory, swap and pressure figures |
| `confine.rs` | write confinement with Landlock |
| `isolate.rs`, `link.rs`, `wg.rs` | network namespaces, the filtering namespace with slirp4netns, proxy and bandwidth, and the WireGuard tunnel |
| `remote.rs` | execution on another host over `ssh` |
| `display.rs` | monitors, window placement and screenshots on X11 |
| `hook/`, `shell.rs`, `policy.rs`, `policy_file/` | the command hook, its shell parser, the rules of the command policy and the file they are read from |
| `units.rs` | sizes, rates and durations |

### Module directories

Each has its own `README.md`.

| Directory | Content |
|---|---|
| `admission/` | admission priority, aging, strict order, protected backfill and hierarchical fair share |
| `aggregate/` | shared cgroup domains for the limits of a Queue or Group |
| `attempts/` | numbered attempts and their immutable archives |
| `cancellation/` | durable cancellation of one Job or a captured subtree |
| `cli2/` | labels, moving a waiting Job, complete listings, explanation of settings, interrupts and standard input of `job run` |
| `cli_contract/` | the compatibility switch, literal arguments, shell selection and quiet wait |
| `commands/` | the one command table and what is generated from it: help, completion, option checking, the JSON envelope and the syntax reference |
| `config/` | the versioned TOML service configuration and the service profiles |
| `doctor/` | `job doctor`, the check of the host and the service setup |
| `durability/` | the launch gate, exit records, idempotency keys, protocol negotiation, peer identity, the audit journal and the failpoints |
| `freezer/` | confirmed suspension through the cgroup freezer |
| `io_policy/` | device I/O limits and weights |
| `isolation/` | namespaces, read-only root, private temporary directories and writable paths |
| `lifecycle/` | held work, editing and release, and the message catalogue of the lifecycle |
| `migration/` | offline validation, backup, conversion and restoration of state |
| `netsecret/` | proxy credentials in private files and their redaction |
| `hook/` | the command hook: one JSON question about a command line, answered with allow, deny or rewrite |
| `objects/` | the tree of Groups and Queues with immutable IDs |
| `operations/` | the lifecycle event journal, queue depth and wait statistics, health, surrounding limits, monitoring mode and output served to socket-group members |
| `paths/` | configuration, state, runtime and cache locations for a user and a system service |
| `policy_file/` | the command policy file: where it is, how it is validated and described |
| `presets/` | versioned execution profiles and scheduling classes |
| `pressure/` | PSI observation, pressure admission rules, notifications and replay |
| `process/` | pidfds, signals and supervisor identity |
| `process_policy/` | CPU affinity, NUMA policy and process resource limits |
| `removal/` | journaled removal of completed records |
| `resource_policy/` | independent CPU and memory requests, limits and weights |
| `resource_update/` | journaled changes to the controls of running work |
| `security/` | `no_new_privs`, capability reduction and system call deny lists |
| `service/` | the `jobd` entry point, the socket, the runtime lock and the socket group |
| `streams/` | separate recording of stdout and stderr, output quotas and the service budget |
| `terminal/` | persistent shared terminals and the attach client |

### Tests and benchmarks

| Target | Content |
|---|---|
| `tests/watch_backend.rs` | the command line against a private service without a cgroup |
| `tests/freezer.rs` | the same inside a delegated cgroup; every case is ignored and runs through `tools/check-delegated` |
| `tests/state_migration.rs` | offline validation, conversion, backup and restore |
| `tests/durability.rs` | crash windows, idempotency, negotiation, the audit journal |
| `tests/cli_contract.rs` | the command table, help, completion, envelopes and the `cli2_` cases |
| `tests/linux_integration.rs` | paths, the socket, `job doctor`, cgroup discovery, the socket group |
| `tests/operations.rs` | events, depth, health, surrounding limits, monitoring mode, served output |
| `tests/terminal_sessions.rs` | shared terminals with real attach clients |
| `tests/process_churn.rs` | signals under rapid process churn and reused process IDs |
| `tests/service_manager.rs` | restart, term, kill and down/up under a private `runsv`; static check of the systemd units |
| `tests/support/` | the fixture for a service in a delegated cgroup |
| `tests/fixtures/` | recorded cargo and libtest output for the summary's tests, and two network helpers |
| `benches/pressure.rs` | bounded workload comparisons under pressure rules |
| `benches/release.rs` | the release benchmark, run by `tools/bench` |

## How to test

`tools/check` at the repository root runs `tools/install-check`, formatting, clippy, every test and then `tools/check-delegated`, which runs the ignored cgroup cases where a delegated cgroup is writable and skips with one line elsewhere (`JOB_REQUIRE_DELEGATED=1` makes the skip a failure). Run it through an installed job service, so that it has a reservation and a delegated cgroup: `job run --cores 6 --mem 8G -- tools/check`.

The integration tests start services with private state directories and run bounded programs only. The filter's unit tests read real cargo and libtest output kept in `tests/fixtures/`. `tools/bench OUTPUT.jsonl` runs the release benchmark; it asserts no speed and is not part of the gate.
