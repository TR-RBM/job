# User guide

This guide follows the path of a user through job: run something, wait for it, read its output, control it, organize it, and then add the optional policies. Most sections are short and point to the page that holds the details. The [syntax reference](reference/syntax.md) lists every command and option; `job COMMAND --help` prints the same for one command.

job needs its service, `jobd`, to be running. The [administration guide](administration.md) describes how to run it. Everything below is typed as a user who is admitted to that service.

Examples use `42` as a Job ID. Use the ID that `submit` or `create` printed.

## Run and submit

```sh
job run -- make test
job submit -- make release
job submit -- sh -c 'producer | consumer'
job submit --shell bash -- 'make && make check'
job run --dir /srv/project --time 30min -- make
```

`run` starts a command and waits for it. It passes the command's stdout to stdout and its stderr to stderr while it runs, and returns the command's exit status. `submit` prints the Job ID and returns at once. Both take the same options.

Arguments after `--` are the program and its arguments, exactly as written. job does not interpret them again. For pipes, `&&` and other shell syntax, run a shell yourself or name one with `--shell`. A Job inherits the working directory and the environment of the command that submitted it; `--dir` names another directory. `--time` stops a Job that runs longer than the given duration.

A Job reads end of file on standard input. `job run --stdin` passes the standard input of the command to the Job, and a Job with a terminal reads from it; see [Terminals](#terminals).

```sh
printf 'one\ntwo\n' | job run --stdin -- sort -r
job run --on-interrupt detach -- make test
```

Ctrl-C on `job run` is passed on: the first SIGINT or SIGTERM is forwarded to the processes of the Job and `run` goes on waiting; a second one ends `run` with status 75 and leaves the Job running. `--on-interrupt detach` leaves the Job running at the first signal, and `--on-interrupt cancel` cancels it. A hangup or a closed output never ends the Job. `submit` leaves no client behind, so nothing that happens to your terminal reaches the Job.

Details: [commands, shells and waiting](cli-execution.md), with the table of [signals and disconnection](cli-execution.md#signals-disconnection-and-cancellation) and [standard input](cli-execution.md#standard-input).

## Wait and exit statuses

```sh
job wait 42
job wait --timeout 30s 42
job wait --summary 42
job wait --json 42
```

`wait` prints nothing and returns the Job's outcome as its exit status. `--summary` prints a diagnostic summary with an excerpt of the output, and `--json` prints one result object. Waiting never cancels the Job, and ending the waiting command leaves the Job running.

| Status | Meaning |
|---|---|
| 0 | the Job succeeded |
| the Job's own status, or 128 plus a signal number | the Job ended by itself |
| 1 | the Job was cancelled or stopped by the service |
| 75 | a deadline passed while the Job was pending or running, or the service stayed unavailable |
| 125 | service failure, lost Job, start error or usage error |

A program may itself exit with 1, 75 or 125. The `outcome` field of the JSON result tells these cases apart. The full table is in [commands, shells and waiting](cli-execution.md#exit-statuses).

## Look at Jobs

```sh
job list
job list --queue development/builds
job list --all
job list --state failed,cancelled --limit 20
job list --all --format tsv
job show 42
job show 42 --json
job explain 42
```

`list` prints held, queued and running Jobs. `--all` includes completed Jobs, and `--state` selects states by name. The most recent 200 matching Jobs are listed unless `--limit` says otherwise. `--format tsv` prints tab-separated columns under a header line, for scripts that do not want JSON; `queue list` and `group list` take it too. `show` prints one Job, and with `--json` its full record: the original submission, the resolved settings, where each setting came from, and the result. `status` is an earlier spelling of `show`. `explain` says why a Job waits and when it may start.

Details: [list and show](cli-execution.md#list-and-show).

## Labels

```sh
job submit --label team=build --label ticket=4711 -- make
job queue set development/builds --label owner=ci
job list --label team=build
job queue list --label owner=ci
```

A Job, a Queue and a Group can carry labels, pairs of a key and a value, and the three listings select by them. Labels classify. They never affect scheduling, limits or any other policy, and a Queue does not pass its labels on to its Jobs. Details: [labels](cli-execution.md#labels).

## Logs and output recording

```sh
job logs 42
job logs 42 --follow
job logs 42 --stream stderr
job logs 42 --raw
job logs 42 --grep error
job log 42 tail 50
```

The service records stdout and stderr separately, with the time each piece arrived. `logs` prints the retained output and `--follow` keeps printing until the Job ends. By default terminal control characters and non-ASCII bytes are shown escaped; `--raw` prints the bytes unchanged. `log` is the earlier query interface over the combined text: `errors`, `grep TEXT`, `lines A..B`, `tail N` or `full`.

Output is kept within bounds. `--output-head SIZE` and `--output-tail SIZE` say how much of the beginning and of the end of each stream is kept, and the service has a total budget. Where bytes were removed, `logs` says so at that place.

Details: [output recording](output-recording.md).

## Hold, edit and release

```sh
job create -- bash -c 'echo original'
job edit 42 -- bash -c 'echo updated'
job release 42
job wait 42
```

`create` records a Job and holds it. A held Job waits until it is released. `edit` replaces the complete command, its options and its environment before execution; the original submission stays visible in `job show 42 --json`. A running Job cannot be edited.

```sh
job move 42 --queue development/releases
```

`move` puts a held or queued Job into another Queue without touching its command. The Job keeps its number and the time it has waited; the settings of the new Queue are applied and checked as at submission. Details: [moving a waiting Job](cli-execution.md#moving-a-waiting-job).

## Retry and attempts

```sh
job retry 42
job wait 42
job attempts 42
job logs 42 --attempt 1
```

`retry` runs a completed Job again under the same ID. Each retry is a new attempt with its own number, admission time, output and outcome, and earlier attempts stay readable. A retry uses the saved environment and resolves its settings against the current defaults of its Queue, so an inherited setting may differ from the earlier attempt. `--current-env` takes the environment of your shell instead, `--queue PATH` chooses another Queue, and `--hold` creates the attempt held so that you can edit it first.

A Job whose execution state is unknown is `lost`. job never reruns lost work by itself, because the command may have run partly or completely. `job retry 42 --allow-lost` is your decision. What `lost` means, and what survives a crash of the service, is in [reliability](reliability.md).

## Cancel, signal, suspend and continue

```sh
job cancel 42
job signal -s HUP 42
job suspend 42
job continue 42
```

`cancel` asks the service to end a Job: the processes receive TERM and, after a grace period, are killed. Cancellation is accepted first and completes asynchronously; use `wait` to see the end. `signal` sends one signal to the Job's processes and changes nothing else.

`suspend` freezes all processes of a running Job and `continue` lets them run again. Success means the kernel confirmed the state. A suspended Job keeps its memory and its place among running Jobs, and its time limit keeps running. Suspension needs a service that manages a delegated cgroup; without one, and for Jobs running on another host, the request is refused. A status of 75 means the confirmation is still pending.

These commands, and `release`, `retry`, `reprioritize`, `move`, `remove` and `wait`, also take a set of Jobs: a range, a list, or several operands.

```sh
job cancel 1-10
job cancel 1,4,7-9 --dry-run
job wait 1-10 --timeout 5m
```

A set answers one line per Job and exits with 1 when a Job was refused; IDs inside a range that do not exist are skipped. `--dry-run` shows what would happen and changes nothing. The rules are in [sets of Jobs](cli-execution.md#sets-of-jobs).

Details: [cancellation](cancellation.md), and the suspension section of the [administration guide](administration.md#suspend-and-continue-existing-work).

## Remove records

```sh
job remove 42 --dry-run
job remove 42
```

`remove` deletes the record of a completed Job with all its attempts, retained output and saved environments. It never cancels work: held, queued and running Jobs must be cancelled first. Files the Job wrote in its working directory stay. Details: [record removal](record-removal.md).

## Queues and Groups

```sh
job group create development
job group create development/releases
job queue create development/builds
job submit --queue development/builds -- make
job queue show development/builds
job queue list
job group list
```

A Queue collects Jobs and a Group collects Queues and other Groups. Paths name them, with `/` between the levels. Jobs submitted without `--queue` go to the `default` Queue. A new Queue has no settings: it does not serialize its Jobs and does not limit them.

### Set and unset

```sh
job group set --max-running 4 development
job queue set development/builds --priority 50
job queue unset development/builds priority
job group unset development max-running
```

`set` stores a setting on one Queue or Group. `unset` removes it, so that the value of the ancestors applies again; the key is the option name without its dashes.

### Pause, resume, close and open

```sh
job group pause development
job group resume development
job queue close development/builds
job queue open development/builds
```

`pause` holds new starts: waiting Jobs stay queued and running Jobs go on. `close` refuses new submissions while existing work can finish. A Queue paused on its own stays paused when its parent Group is resumed.

### Rename, move and remove

```sh
job queue rename development/builds compile
job queue move --group development/releases development/compile
job queue remove development/releases/compile --dry-run
job group remove development --recursive
```

Renaming and moving keep the identity of the object and of its Jobs; both are refused while the subtree has running work. `remove` deletes an object and the records of its completed Jobs. It never cancels work, and a Group that still contains something needs `--recursive`.

### Act on everything below a path

```sh
job group cancel --recursive development --dry-run
job group cancel --recursive development
job group suspend --recursive development
job group continue --recursive development
```

These act on the Jobs that exist at that moment and report each result. They do not change later submissions; that is what `pause` and `close` are for.

## Inheritance and effective settings

A setting on a Group applies to everything below it. There are two kinds:

- A constraint, such as `--max-running` or a shared `--memory-max`, binds the whole subtree. A child cannot lift it.
- A default for Jobs, written with a `--job-` prefix such as `--job-memory-max` or `--job-seccomp-deny`, is taken by each Job that does not say otherwise. A nearer Queue or Group overrides a farther one, and the Job's own option overrides both.

```sh
job queue show development/builds --json
job show 42 --json
```

```sh
job explain development/builds
job explain development/builds memory-max
```

`queue show` and `group show` print the local settings of an object and, for each inherited one, where it comes from. `job explain PATH` goes through the settings of a Queue or Group and says for each what is configured, what is in effect, which object supplies it, and why a setting cannot take effect as written; `job explain PATH KEY` does that for one setting, named by its option without dashes. Details: [explaining settings](cli-execution.md#explaining-settings). A Job's record keeps the resolved values and their sources under `resource_sources`. Defaults are resolved when a Job is submitted, edited or retried; changing a default does not change Jobs that already run.

Versioned [execution profiles and scheduling classes](execution-profiles.md) bundle such defaults under a name and revision: `job submit --execution-profile build@1 -- make`. `job profile list` and `job class list` show what the service configuration defines.

## Priorities and fair share

Without configuration, waiting Jobs start as soon as nothing holds them. Ordering policies are opt-in.

```sh
job queue create builds --priority 50 --aging 30s --backfill conservative
job submit -q builds --priority 100 --cpu-request 2 --time 5min -- make
job reprioritize 42 --priority 200
job explain 42
```

Admission priority, from -1000 through 1000, decides which waiting Job starts first. It is not a CPU weight and does not affect running Jobs. `--aging` raises the effective priority of a Job the longer it waits. `--strict-fifo true` starts Jobs strictly in submission order. `--backfill conservative` lets a small Job start ahead when that delays no other.

```sh
job group create compute --fair-share cpu-request-time
job queue create compute/builds --share-weight 2
job queue create compute/batch --share-weight 1
job submit -q compute/builds --cpu-request 2 -- make
```

With fair share, a Group prefers the child that has received less service relative to its weight. Service is the requested CPU, or with `memory-request-time` the requested memory, multiplied by the time it was held. It is a relative target under contention, not a guaranteed percentage.

Details: [admission ordering](scheduling.md).

## Resource requests, limits and weights

These are three separate things:

```sh
job run --cpu-request 2 --memory-request 1G -- make
job run --cpu-limit 0.5 --cpu-weight 25 -- command
job run --memory-high 512M --memory-max 1G --memory-swap-max 0 -- command
job queue set development/builds --job-cpu-limit 2
job group create production --cpu-limit 4 --memory-max 8G
```

- A request is counted when the service decides whether a Job may start. It reserves no physical memory and guarantees no CPU.
- A limit is enforced by the kernel through cgroup v2 while the Job runs.
- A weight is a relative share under contention.

`--pids-max N` limits the number of processes and threads of a Job; it is a limit like `--memory-max` and reserves nothing. `--write-budget SIZE` stops a Job once it has written more than SIZE bytes. It counts what the Job writes and is not a file system quota. Details: [process count and write budget](cli-execution.md#process-count-and-write-budget).

On a Queue or Group, `--cpu-limit`, `--memory-max` and the other unprefixed controls are one ceiling shared by all Jobs below, while the `--job-` forms are defaults for each Job. Device I/O has `--io-max`, `--io-weight` and `--io-bfq-weight`.

Kernel controls need a service that manages a delegated cgroup with the controller in question. `job host` shows what this host offers; a control the host cannot apply is refused when the Job is submitted. A service without a delegated cgroup runs in monitoring mode and says `limits watched, not enforced` in every answer; see [monitoring mode](operations.md#monitoring-mode). An administrator can forbid that mode with `required = true` under `[cgroup]` in the service configuration; the service then refuses to start without a cgroup.

```sh
job update 42 --cpu-limit 2 --memory-high 1G --dry-run
job update 42 --cpu-limit 2 --memory-high 1G
job group update production --cpu-weight 200
```

`update` changes the controls of running work without restarting it. Lowering `--memory-max` can kill processes and therefore needs `--allow-oom`. A status of 75 means the change is still being applied; `job resource-update OPERATION` shows it.

The earlier options `--cores`, `--mem`, `--pids` and `--disk` keep their earlier combined meanings; `--disk` is the earlier spelling of `--write-budget`. In monitoring mode with the ordinary service profile, `--mem` and `--pids` are refused, because the limit they promise cannot be set. Details: [resource controls](resources.md).

## Pressure

```sh
job pressure
job pressure 42
job pressure status
job pressure events
```

`pressure` shows the kernel's CPU, memory and I/O pressure figures for the host or for one running Job. Observation changes nothing. Optional rules, in the service configuration or with `--pressure FILE` on a Queue or Group, can hold new starts while pressure stays high and restore concurrency step by step afterwards. No rule exists by default. Details: [pressure observation and admission](pressure.md).

## Placement and process limits

```sh
job run --cpu-affinity 0-3 -- command
job run --numa-policy bind:0 -- command
job run --rlimit nofile=1024:4096 -- command
job queue set builds --job-rlimit core=0
```

`--cpu-affinity` names the CPUs the processes may run on, `--numa-policy` sets the memory policy, and `--rlimit` sets one process resource limit, soft and hard. These are settings of the processes, not of a cgroup, and placement is not containment. `job host --json` lists the CPUs and memory nodes available. Details: [placement and process limits](process-controls.md).

## Security controls

```sh
job run --no-new-privs yes -- command
job run --cap-drop all -- command
job run --seccomp-deny ptrace,bpf -- command
job run --confine --allow-write /srv/out -- command
```

`--no-new-privs` forbids gaining privileges through exec. `--cap-drop` removes capabilities. `--seccomp-deny` makes the named system calls fail. `--confine` lets a Job write only in its own tree and a few standard places, plus each `--allow-write PATH`.

Each control removes exactly what it names. None of them is a sandbox, and none separates two Jobs of the same Unix user from each other. Read [security controls](security-controls.md) for the limits of each and the [security model](security.md) for what the service as a whole does and does not protect.

## Isolation

```sh
job run --namespaces user,uts,ipc -- command
job run --namespaces user,mount --root read-only --writable /srv/out -- command
job run --namespaces user,mount --private-tmp yes -- command
```

`--namespaces` starts the program in new namespaces of the named kinds: `user`, `mount`, `pid`, `ipc`, `uts`, `cgroup`. The list is the whole request and nothing is added. `--root read-only` makes every mount the Job sees read-only except the `--writable` paths, and `--private-tmp yes` gives it an empty temporary directory of its own. With `--namespaces user,mount,pid` the program sees only its own processes under a small init that is part of job, and they all end with the Job; that hides processes and is not a security boundary. Details: [isolation](isolation.md).

## Networking

```sh
job run --net none -- command
job run --net socks5://proxy.example:1080 --net-secret-file ~/.config/job/proxy.secret -- command
job run --net wireguard:/home/me/tunnel.conf -- command
job run --bandwidth 5Mbit -- command
```

A Job uses the host's network unless it or its Queue says otherwise. `--net none` leaves loopback only. A proxy or a WireGuard tunnel puts the Job in a network namespace from which nothing else is reachable; when the proxy or the tunnel fails, the Job has no network. `--bandwidth` holds the Job to a rate each way. A Queue can set a network for all its Jobs. Details, the table of what each mode carries, and the handling of proxy credentials: [networking](networking.md).

## Terminals

```sh
job run --pty -- bash
job submit --pty -- java -jar server.jar nogui
job attach 42
```

`--pty` gives a Job a persistent terminal. `run --pty` attaches at once; `submit --pty` starts it detached. `attach` joins the terminal of a running Job, also over SSH: `ssh -t user@host job attach 42`.

Several clients may be attached at the same time. All see the same screen and all may type; there is no owner of the input and no separate login. Ctrl-C and Ctrl-Z go to the shared program and so affect everyone. Every client admitted to the service may attach, also a member of a shared socket group who is another Unix user (see the [administration guide](administration.md#sharing-the-socket-with-a-unix-group)).

Detach with Ctrl-] followed by `d`. Press Ctrl-] twice to send it to the program. The program keeps running after you detach or lose the connection. The detach key can be changed, in this order of precedence: `job attach 42 --detach-key ctrl-a`, the environment variable `JOB_DETACH_KEY`, or `detach_key` in the `[terminal]` table of the service configuration. A key is `ctrl-` and a letter or one of `]`, `\`, `^`, `_`; the second key stays `d`. With `none` there is no detach key: you then detach only by closing your own terminal.

A new attachment receives the current screen. The shared size is the smallest attached size. Earlier output is available through `job logs`, where stdout and stderr of a terminal are one combined stream. What you type is not recorded separately, though the terminal's echo can appear in the output.

The terminal belongs to the Job's supervisor, so restarting the service alone keeps it. A reboot, or a service manager that kills the whole process tree on stop, ends it. The terminal handles common text applications; it does not implement every xterm extension, clipboard access or graphics. `--pty` cannot be combined with `--on`: connect to the other host with SSH and use its service there.

## Other hosts

```sh
job host --on user@host
job run --on user@host -- make test
job run --on user@host --send ./input --fetch output -- ./process
job queue set remote-builds --on user@host
```

`--on USER@HOST` runs a Job through the job service of another host, reached with `ssh`. The other host needs job installed and its service running, in a release that speaks the same remote protocol; otherwise the request is refused and both versions are named. `--ssh-key` and `--ssh-option` configure the connection. `--send PATH` copies files there before the start and `--fetch PATH` copies results back after the end.

A remote Job holds no CPU or memory on the local host. Its limits, network, security and isolation settings are checked and applied on the other host; the local `ssh` process runs unrestricted on the local network. Suspension and live resource updates must be done on the host that executes the Job. A Queue with `--on` sends all its Jobs to that host.

## Structured output

```sh
job show 42 --json
job show 42 --format json
job logs 42 --json
```

Commands that print a result take `--json`. `--format json` wraps the same data in one versioned line with `schema_version`, `kind` and `data`, so that a script can tell documents apart. `logs --json` prints a stream of records, one per line. Field names and outcome identifiers are never translated. Details: [structured output](cli-execution.md#structured-output).

To make a submission safe to repeat from a script, give it a key: `job submit --idempotency-key nightly-1 -- make release`. The same key with the same command returns the Job already recorded instead of adding a second one. See [reliability](reliability.md#idempotency-keys).

## Events, queue depth and health

```sh
job events --job 42
job events --queue development --follow --format json
job queue show development/builds
job group show /
job host
job doctor
```

The service writes one record for every state change of a Job and for every pause, close and pressure hold of a Queue or Group. `job events` reads that journal, selects by Job, by subtree or by time, and with `--follow` keeps printing new records. `queue show` and `group show` report the depth of the subtree: how many Jobs are held, queued, starting, running, suspended and stopping, how long the oldest queued Job has waited, and the median and longest wait of the Jobs started in the last hour. `job host` ends with a health section and with the limits a service manager or container set around the service, which the service cannot lift. `job doctor` checks the host and the service setup, works without a running service and says what to do about each finding.

Details: [operating the service](operations.md).

## The audit journal

```sh
job audit
job audit --target 42
job audit --action cancel --since 1791230000000 --json
```

Every request that changes something is recorded with the user ID and process ID of the client that made it: an intent before the change and a result after it. The arguments of commands are not recorded, only the program name, their number and a digest. `job audit` and `job events` read files in the state directory and are for the service user; a member of a socket group cannot run them. Details: [the audit journal](reliability.md#the-audit-journal).

## A system service

```sh
job --system list
JOB_SYSTEM=1 job submit -- make
```

`--system`, written before the command, selects the system service of the host instead of your own user service for one invocation; `JOB_SYSTEM=1` does the same for everything in an environment. Your account must be admitted to that service. See the [administration guide](administration.md#3-the-service-and-its-delegated-cgroup).

## Help and completion

```sh
job help
job queue set --help
job completion bash > ~/.local/share/bash-completion/completions/job
job completion fish > ~/.config/fish/completions/job.fish
```

`make install` installs the completion files already; the two commands above are for a manual setup. Completion offers commands, options and listed values without the service, and asks the service briefly for Job IDs and paths.

## Where to read on

- [Capability matrix](capabilities.md): what needs what, and what is not available.
- [Operating the service](operations.md), [reliability](reliability.md) and [performance](performance.md).
- [Security model](security.md).
- [Administration guide](administration.md) and [migration guide](migration.md).
- [Release notes](release-notes.md).
- Manuals `job(1)`, `job.conf(5)`, `job(7)` and `jobd(8)`.
