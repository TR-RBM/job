# Release notes

## First redesigned release

These notes compare this release with the earlier tool, which was developed under the name `exec` and kept its state in `~/.local/state/exec`. The comparison point in the source history is commit `862c8f9`. This release has state schema 18 and protocol 20.

The earlier tool reserved cores and memory for every command, estimated what an unknown command would need, ran each Queue one Job at a time, and answered with a filtered summary of the output. This release is a general execution and scheduling service: nothing is reserved, limited or summarized unless you ask, and the earlier behaviour is available as an explicit profile.

Read [What changed incompatibly](#what-changed-incompatibly) before you upgrade.

## What is new

**For programs that show the service**

- A client interface on the service's socket, with a version number of its own: a greeting that names what the service answers, a subscription that gives a snapshot and then every change of a Job, Queue or Group in order, listings of Jobs by cursor in four orders, one Job in full with what the kernel confirmed at its start, the tree with its use, processes, output from a position, live use of the Jobs a client names, the command table as data, and `command`, which carries out a job command given as its words. It is described field by field in [the client interface](client-interface.md). job's own command does not use it and is unchanged.

**Objects and lifecycle**

- Groups that contain Queues and other Groups, with rename and move that keep identities. A new Queue has no settings and does not serialize its Jobs.
- Held work: `job create`, `job edit`, `job release`.
- Retries as numbered attempts of the same Job, each with its own output: `job retry`, `job attempts`.
- Confirmed suspension: `job suspend`, `job continue`, for one Job or a subtree.
- Cancellation of a subtree with a preview, and removal of completed records: `job queue cancel`, `job group cancel`, `job remove`.
- `job signal` for any signal. Process control uses Linux process handles (pidfds).
- `pause` and `resume`, `close` and `open` on Queues and Groups.
- `job list` with `--all`, `--state`, `--limit` and a tab-separated format, and `job show`: [list and show](cli-execution.md#list-and-show).
- `job move` puts a held or queued Job into another Queue: [moving a waiting Job](cli-execution.md#moving-a-waiting-job).
- Labels on Jobs, Queues and Groups, and listings selected by label: [labels](cli-execution.md#labels).
- Sets of Jobs: `job cancel 1-10`, `job cancel 1,4,7-9`, and the same for `remove`, `release`, `retry`, `suspend`, `continue`, `signal`, `reprioritize`, `move` and `wait`, with one answer line per Job, `--dry-run`, and `job list --id SET`: [sets of Jobs](cli-execution.md#sets-of-jobs).
- `job run --stdin` passes standard input, and `--on-interrupt` chooses what an interrupt of `job run` does: [signals and disconnection](cli-execution.md#signals-disconnection-and-cancellation), [standard input](cli-execution.md#standard-input).

**Scheduling**

- Opt-in admission priority with bounds, aging, strict order and conservative backfill, and `job explain` and `job reprioritize`.
- Opt-in fair share between the children of a Group.
- Versioned execution profiles and scheduling classes.
- `job explain PATH [KEY]` says which Queue or Group supplies or blocks a setting: [explaining settings](cli-execution.md#explaining-settings).

**Resources**

- Separate requests, limits and weights for CPU and memory, in place of one combined reservation.
- Ceilings shared by a Queue or Group.
- Device I/O limits and weights.
- Changes to the controls of running work: `job update`.
- `--pids-max` as a limit on processes and threads, and `--write-budget` as the name of the count of bytes written: [process count and write budget](cli-execution.md#process-count-and-write-budget).
- Pressure observation, and opt-in pressure rules that hold new starts: `job pressure`.

**Process, security and isolation controls**

- CPU affinity, NUMA policy and process resource limits.
- `no_new_privs`, capability reduction and system call deny lists: [security controls](security-controls.md).
- User, mount, IPC, UTS and cgroup namespaces, a read-only root with writable paths, and a private temporary directory: [isolation](isolation.md).
- PID namespaces: `--namespaces user,mount,pid` runs the program under a small init that is part of job, with a `/proc` of its own; the Job sees only its own processes and they all end with it: [PID namespace](isolation.md#pid-namespace).
- Documented network boundaries for `--net none`, proxies, WireGuard tunnels and bandwidth ceilings; Jobs of one Queue that share a network cannot reach each other; proxy credentials in a private file with `--net-secret-file`: [networking](networking.md#proxy-credentials).

**Output and terminals**

- stdout and stderr recorded separately with times; `job logs` with `--follow`, `--stream`, `--raw` and `--json`.
- Output quotas per stream, `--output-head` and `--output-tail`, and a total budget for the service: [output recording](output-recording.md#output-quotas).
- Persistent terminals shared by several clients: `--pty`, `job attach`, a configurable detach key: [terminals](user-guide.md#terminals).
- Output and terminals for members of a socket group, served by the service while the state directory stays private: [operating the service](operations.md#output-and-terminals-for-a-socket-group).

**Reliability**

- A documented durability contract: an acknowledged submission survives a crash of the service, no command runs before its supervisor is recorded, and a known exit status is not lost.
- Idempotency keys for submissions, `--idempotency-key`: [idempotency keys](reliability.md#idempotency-keys).
- An audit journal of every request that changes something, with an intent before the change and a result after it, and without the arguments of commands: `job audit`, [the audit journal](reliability.md#the-audit-journal).
- Version negotiation between the command and the service.

**Operation**

- A journal of lifecycle events: `job events`.
- Queue depth and wait statistics in `job queue show` and `job group show`.
- Health and the limits set around the service in `job host` and `job doctor`.
- Monitoring mode is announced wherever it applies, and `required = true` under `[cgroup]` forbids it.

All four are described in [operating the service](operations.md).

**Networks**

- `--net ns:NAME` starts a Job in an existing network namespace that the service configuration names under `[network.namespaces]`. A client selects a name, never a path.
- `--net profile:NAME` applies a routing profile from `[network.profiles]`: one exit (the host, none, a proxy, a WireGuard tunnel or a configured namespace) with destination rules, resolvers and bandwidth, per Job, per Queue or shared by all its Jobs.
- `job net list` and `job net show NAME` print both, with the filter rules and whether this host can enforce them.

They are described in [networking](networking.md).

**Installation and administration**

- `jobd` as the service entry point, with systemd units and a runit example.
- Conventional file locations for a user service and a system service.
- Discovery of the delegated cgroup instead of one fixed path.
- A socket that can be shared with one Unix group.
- `job doctor`, `job config`, and `job state` for validation, backup, restore and migration: [administration](administration.md), [migration](migration.md).
- `--system` before the command selects the system service for one invocation.
- A staged `make install` with `PREFIX` and `DESTDIR`, four manual pages, and Bash and fish completion generated from the same command table as the help.
- `--format json` with a versioned envelope, and documented exit statuses.

The [capability matrix](capabilities.md) lists each of these with what it needs from the host.

## What changed incompatibly

### Arguments are executed literally

Earlier, a single argument after `--` was handed to Bash: `job run -- 'make && make check'` ran a shell line. Now the arguments after `--` are the program and its arguments, also when there is only one.

Rewrite such commands as `job run -- sh -c 'make && make check'` or `job run --shell bash -- 'make && make check'`. To keep the earlier rule for one command, add `--legacy-shell`. To keep it for everything a script or session runs, set `JOB_CLI_COMPAT=legacy` in its environment. Jobs submitted before the upgrade keep the interpretation they were stored with. See [commands, shells and waiting](cli-execution.md#existing-scripts).

### `wait` is quiet

Earlier, `job wait ID` printed a summary of the result. Now it prints nothing and returns the outcome as its exit status. Use `job wait --summary ID` for the summary or `job wait --json ID` for a result object. `JOB_CLI_COMPAT=legacy` restores the earlier output, including the earlier shape of the JSON.

### `run` passes output through, on two streams

Earlier, `job run` printed one filtered summary when the command ended. Now it passes the command's stdout to stdout and its stderr to stderr while the command runs. `--summary` selects the earlier summary. Recorded output is separate per stream; recordings made before the upgrade stay merged.

### Interrupting `job run`

Earlier, Ctrl-C on `job run` ended the client and the Job went on without a word. Now the first SIGINT or SIGTERM is forwarded to the processes of the Job and `run` goes on waiting; a second one returns 75 and leaves the Job running, with one line on standard error that names the Job. A script or harness that interrupts `job run` to stop waiting must pass `--on-interrupt detach`; `--on-interrupt cancel` cancels the Job instead. A closed output now returns 75 instead of 125. Under `JOB_CLI_COMPAT=legacy` and without `--on-interrupt`, `run` behaves as before. See [signals and disconnection](cli-execution.md#signals-disconnection-and-cancellation).

### The service is `jobd`

The service is started as `jobd`. `job daemon` still works as an earlier spelling. Service definitions should be changed to `jobd`, or `jobd --system` for a system service.

### Where the socket is

Earlier, the socket was always `daemon.sock` in the state directory. Now it is `job.sock` in a runtime directory where one exists: `$XDG_RUNTIME_DIR/job/` for a user service and `/run/job/` for a system service. A service that has no runtime directory, such as one started by runit without a login session, still binds `daemon.sock` in its state directory, and clients look there too. The service and its clients must agree; the rules are in the [administration guide](administration.md#3-the-service-and-its-delegated-cgroup).

### State is not converted automatically

State schema 18 is not read from an `exec` state directory in place. While an `exec` store is present, the program refuses to pick a state directory by itself, and it never converts on its own. Conversion is an explicit, offline step that first makes a verified backup: see [How to upgrade](#how-to-upgrade). Running Jobs cannot be carried across; drain them under the earlier version first.

The command and the service must come from the same release. A command of another protocol version is refused with both version ranges and exit status 125. Between two hosts, `--on` is likewise refused when their remote protocols differ, and the message names both.

### Nothing is reserved or limited by default

In the ordinary service profile, which every fresh installation uses, a Job without options has no estimated reservation, no process limit, no swap restriction and no emergency stop, and a Queue runs its Jobs side by side.

The `legacy` service profile keeps the earlier behaviour. You choose it explicitly when you convert existing state or create a configuration with `job config init --profile legacy FILE`. It preserves:

- automatic estimates of what a command needs, from its earlier runs;
- conservative backfill;
- a default limit of 4096 processes and threads per Job, and no swap for Jobs;
- floors of 12 % of the host's memory and 5 % of each file system that admission keeps free;
- the emergency stop of the largest Job under memory or disk shortage, described in [pressure](pressure.md#emergency-stop-in-the-legacy-profile).

`job config show --json` reports which of these are in effect. Converted Queues that ran one Job at a time keep that as an explicit `max-running` of one.

### Resource options

`--cores`, `--mem`, `--pids` and `--disk` keep their earlier combined meanings: `--cores` is a request and a CPU weight and never a CPU quota, and `--mem` is a request and a memory ceiling. New work should use the separate options `--cpu-request`, `--cpu-limit`, `--cpu-weight`, `--memory-request`, `--memory-high`, `--memory-max`, `--memory-swap-max`, `--pids-max` and `--write-budget`. See [resource controls](resources.md#compatibility-and-recovery).

One case changed: on a service without a delegated cgroup and with the ordinary profile, `--mem` and `--pids` are refused with a capability error when the Job would become runnable, because the limit they promise cannot be set there. The `legacy` profile keeps watching them. See [monitoring mode](operations.md#monitoring-mode).

### The command hook and the command policy

`job hook` no longer reads and writes the format of one calling product. It reads one JSON object with `command` and optionally `cwd`, `caller`, `caller_name` and `detached`, and writes one with `decision`, which is `allow`, `deny` or `rewrite`; see [automation](integrations.md). A caller that sent its own format needs an adapter, which is a separate project. Input in another format ends with status 125.

No command rules are compiled in any more. The command policy is empty until you install a file, `policy.json`, in the configuration directory; until then `job hook` and `job policy` forbid nothing. `job policy --show` prints the file in use.

The session label of a Job is `--session`, otherwise `JOB_SESSION`, otherwise `unnamed`. No other environment variable is read for it.

### Queue commands

`job queue create`, `set`, `unset`, `remove` and `--max-running` replace `job queue add`, `job queue rm` and `--parallel`. The earlier spellings still work, with one difference: a Queue made without `--parallel` used to run one Job at a time and now has no ceiling, whichever spelling creates it. Write `--parallel 1` or `--max-running 1` where a new Queue must run its Jobs one after another. Queues that existed before the conversion keep one at a time as an explicit setting.

### Earlier spellings that remain

These keep working throughout the first major version.

| Earlier | Now |
|---|---|
| `--legacy-shell`, `JOB_CLI_COMPAT=legacy` | `sh -c`, `--shell`, `--summary` |
| `job queue add`, `job queue rm` | `job queue create`, `job queue remove` |
| `job queue clear`, which cancels the waiting Jobs of a Queue | no equal replacement; `job queue cancel` also cancels running Jobs |
| `--parallel` | `--max-running` |
| `--cores`, `--mem`, `--pids` | the separate request, limit and weight options, and `--pids-max` |
| `--disk` | `--write-budget` |
| `job status` | `job show` |
| `job daemon` | `jobd` |
| `daemon.sock` in the state directory | `job.sock` in the runtime directory |
| the cgroup `/sys/fs/cgroup/exec` | discovery, `JOB_CGROUP_ROOT` or `[cgroup] root` |

## How to upgrade

Follow the [migration guide](migration.md). In outline:

1. Let running and waiting Jobs finish or cancel them, and stop the earlier service and its network helpers.
2. Install this release.
3. Check the old state: `job state validate --source ~/.local/state/exec`.
4. Preview the conversion with `job state migrate` and `--dry-run`, choosing `--profile legacy` or `--profile ordinary`.
5. Run the same command without `--dry-run`. It writes a verified backup first and never changes the source.
6. Point the service at the converted directory and its configuration, change the service definition to `jobd`, start it, and run `job doctor`.
7. Change scripts as described above, or set `JOB_CLI_COMPAT=legacy` for them for the time being.

To go back, stop the new service, restore the backup into an unused directory with `job state restore`, and start the earlier program on it. Work done after the conversion is not merged back.

## Known limits

This section collects what is open, what was measured and what was never tested or verified. All tests and measurements ran on one host: a virtual machine, x86_64, Linux 7.2, Artix Linux with runit, btrfs, as an ordinary user.

**Open before release**

- systemd was never run. The units, delegation, the placement of a restarted service beside surviving Jobs, the runtime and state directories and `systemctl restart` are written from systemd's documentation. On systemd older than 254 a restart with surviving Jobs is expected to fail.
- Nothing was run as root: no system service under `/etc/job`, `/var/lib/job` and `/run/job`, no dedicated service account, no privileged service. The shipped runit script needs root and was checked for shell syntax only.
- aarch64 was never built or run. The system call filter is written for it.
- A real reboot and a loss of power were not exercised. A test changes the recorded boot identity and ends the processes instead.
- A real SSH host was not used. Remote execution is tested against a stand-in on the same machine.
- There are no release artifacts: no package, no signature, no changelog.
- `SECURITY.md` names no maintainer contact yet.
- A conversion of the state of a real installed service was not performed. The migration tests use constructed stores.
- No security review of the whole release is recorded.

**Performance, as measured**

One run on the host above, where one `fsync` takes about 11 ms; the figures and how to repeat them are in [performance](performance.md).

- The service starts and ends Jobs one at a time, about eight short Jobs per second on the measured machine, because each start and end is made durable with synchronous writes. Other requests are answered between two starts: while 500 Jobs were being started, another client's `job queue show` took 80 ms at the median and 0.4 s at most.
- A submission takes about 60 ms (six synchronous writes); the service accepts 17 to 19 submissions per second.
- With priorities, aging or fair share switched on, a Job costs about 25 synchronous writes of the service, most of them the scheduling ledger, so such a Queue runs about two short Jobs per second.
- `job queue cancel --recursive` and `job queue remove --recursive` hold the service for their whole run: about 1.3 s and 0.7 s for 1 000 held Jobs, 11 s and 5 s for 10 000.
- Recorded output passes through the supervisor at about 170 MiB per second; the same writer into a pipe is about eight times faster.
- With the default retention a Job's directory can hold 131 MB of output for a 64 MiB cap, because the merged log and the stream journal each keep a copy.
- In a burst of starts, `started_ms` is the time of the scheduling pass, not of the command.
- `job list` took 43 ms with 1 000 waiting Jobs and 377 ms with 10 000; the service used 23 MiB and 155 MiB of memory; a restart took 0.2 and 2.1 seconds.

The service is for tens of submissions per minute and for Queues of hundreds of waiting Jobs. With thousands waiting, `pause`, `resume`, recursive cancellation and a burst of starts each stop it from answering for minutes. No target is set for the time to answer one client while the service works for another. The cgroup backend was measured for the idle round trip only, and each group ran once.

**Platforms and service managers**

- The check for lingering was seen only on a host without systemd.
- The second user in the socket-group tests was a subordinate user ID inside a user namespace, not a second login account. Admission through a supplementary group of another user was not exercised.
- A restart of the service with running Jobs was run under a private `runsv`, without a cgroup. An upgrade of a real installation was not run.
- No other distribution, kernel, file system or C library was run.
- The manuals were checked with groff only; mandoc was not available. No test compares `job(1)` with the command table.
- `[cgroup] required = true` was run without a cgroup, where it refuses the start, and not with one.

**Durability**

- The crash tests end the service or a supervisor with the kernel alive.
- The crash tests for the exit record run without a cgroup; that path with a cgroup, where the service kills what is left, was not exercised.
- A command older than the protocol negotiation cannot read the service's refusal.
- A full disk under the audit or event journal was not provoked; the write failure of the audit journal is injected.
- `job state backup` refuses a store with held, queued, running or suspended Jobs, so waiting work cannot be backed up.

**Resources and placement**

- The host has one NUMA memory node. Placement across several nodes was not tested.
- No host with an active I/O weight backend was available; applied I/O weights were never observed.
- The legacy profile's emergency stop has no test of its selection and stop.
- Pressure rules were tried with bounded test workloads. No threshold is calibrated for production, and I/O pressure and host-wide pressure from outside job were not tested.
- The limits around the service are checked to be reported; no test sets a quota or a CPU set above the service.

**Security and isolation**

- No combination of controls was tested as a sandbox against a hostile program; see the [security model](security.md).
- A program of another ABI under a system call filter (32-bit on x86_64) was not run.
- Capability reduction and namespaces were combined with `--net none` only, not with a proxy, a tunnel or a bandwidth ceiling.
- Hosts without unprivileged user namespaces or without `mount_setattr` were not available; their refusals were not provoked.
- A refusal by the kernel in the middle of a launch was never provoked, so the start error that names the step was never seen.
- `--confine` on another host has no test of its own, and whether it keeps a Job from the control socket was not examined.
- A private temporary directory that cannot be removed is moved to `private-tmp-leftovers/` in the state directory and nothing deletes it.

**Networking**

- HTTP and HTTPS proxies were not run; SOCKS5 was. UDP, ICMP and name resolution through a WireGuard tunnel were not tested.
- A Job behind a proxy, a tunnel or a bandwidth ceiling has no IPv6.
- An `ip` program too old for isolated bridge ports was not tried.
- A proxy password typed inside `--net` is visible in the arguments of the submitting command while it runs.

**Output and terminals**

- A terminal Job with an output quota was not tested.
- Quota sizes near the upper bound, and following a recording of thousands of segments, were not measured.
- The terminal handles common text applications, not every xterm extension, clipboard access or graphics. No test shows the screen of a full-screen program restored for a client that attaches later.
- `job attach` to a Job on another host is not built. Attaching over an SSH login to the host of the service has no test with a real SSH server.
- `job audit` and `job events` are not available to a member of the socket group who is not the service user.

**Interfaces**

- The fish completion was checked for syntax, not driven in a fish session.
- `--format json` was exercised for a part of the commands; the others use the same wrapping and were not run one by one.
- `job run --on-interrupt` was not run against a Job on another host, and `job run --pty` and `job attach` were not run under signals.
- A listing over more than 50000 completed records was not run.
- The German texts were written and not reviewed by a second person.
- One test, `fair_share_balances_weighted_service_under_a_bounded_continuous_backlog`, depends on disk speed and fails when other work loads the disk.

What is not in this release at all is listed under [Not available](capabilities.md#not-available).
