# Operating the service

This page covers what an operator reads from a running service: the record of state changes, queue depth and waiting time, health, the limits set around the service, and what a service without a cgroup does. Installation is in [administration](administration.md), the audit journal of requests in [reliability](reliability.md).

## Lifecycle events

The service writes one record for every state change into `events/` below the state directory. `job events` reads it directly, like `job audit`, and needs no running service.

```sh
job events
job events --job 42 --format json
job events --queue team/build --since 1790000000000
job events --follow --format json
```

A Job record has these fields:

| Field | Meaning |
| --- | --- |
| `seq` | number of the record, rising by one, never reused |
| `at_ms` | Unix time in milliseconds when the record was written |
| `kind` | `job` |
| `job`, `attempt` | the Job and its attempt |
| `queue_id`, `queue_path` | the Queue of the Job |
| `from`, `to` | the state before and after; `from` is null for a new Job |
| `reason` | why: `submitted`, `released`, `admitted`, `retry`, the stop line, `exit 3` |
| `exit_code`, `signal` | set on a final state when the command exited or was killed |
| `wait_ms` | on `starting`: how long the Job waited since it became runnable |

`to` is one of `held`, `queued`, `starting`, `running`, `suspended`, `stopping`, `succeeded`, `failed`, `cancelled`, `lost` and `requeued`. `requeued` means the Job is queued again after it had left the Queue: a retry of a finished attempt, or a launch that recovery could not confirm. Its state is queued.

Admission records have `kind` `queue`, `group` or `host`, with `object_id` and `object_path` instead of the Job fields. `to` is `paused` or `resumed`, `closed` or `open`, and for pressure rules `pressure_hold`, `pressure_recovering` or `pressure_open`, with the rule in `reason`. A record of kind `journal` with `to` `rotated` marks a rotation.

`--job ID` selects one Job, `--queue PATH` a Queue or Group and everything below it, `--since MS` records from that time on. `--format json` prints one object per line and adds `schema_version`, currently 1. Text output is tab-separated with a header line.

`--follow` prints what is there and then every new record until you interrupt it. It looks for new records five times a second and reads on across a rotation without leaving a gap in `seq`, as long as the files it still needs are kept. A follower that falls further behind than `keep_files` files reach says on standard error how many records were removed by rotation before it read them, and goes on with the oldest record still kept.

### Retention and durability

The current file is closed at 8 MiB and eight files are kept; older ones are deleted. Change that in the configuration:

```toml
[events]
keep_files = 8
rotate_bytes = 8388608
```

`job config reload` applies a change. A record is at most a few kilobytes: `reason` is cut at 512 bytes and paths at 1024.

A record is written after the state it describes is on disk, so the journal never says more than the records. Each record is written in one piece. It is forced to disk with every final state of a Job, with every admission record and at every rotation. A crash of the service loses nothing that was written. A crash of the host can lose the records after the last forced one; the Job records themselves are not affected. A line cut short by a crash is skipped, counted on standard error, and the next start continues with the next `seq`. If a record cannot be written, the service goes on, says so on standard error, and `job host` reports `events_writable: false`.

The journal is read by the service user. A member of a socket group cannot read it.

## Queue depth and waiting time

`job queue show PATH` and `job group show PATH` report the subtree below the path. `job group show /` covers the whole service.

```
jobs: 1 held, 4 queued, 0 starting, 2 running, 0 suspended, 0 stopping
oldest queued Job waits since: 3m12s
started in the last hour: 17 Jobs, median wait 41s, longest wait 6m02s
```

With `--json` the same is in `objects[0].depth`: `held`, `queued`, `starting`, `running`, `suspended`, `stopping`, `oldest_queued_age_ms`, and `started_last_hour` with `count`, `median_ms`, `max_ms`, `window_ms` and `sample_limit`.

The waiting time of a Job runs from the moment it became runnable (submitted, released or retried) to the moment it was admitted. The statistics cover Jobs admitted in the last hour. The service keeps at most the 4096 most recent starts for this, so on a service that starts more than that in an hour the numbers describe the latest 4096. After a restart they are rebuilt from the records. The median of an even number of waits is the mean of the two middle ones. `job queue list` does not compute depth.

## Health

`job host` ends with a health section, and `job host --json` carries it as `health`:

| Field | Meaning |
| --- | --- |
| `started_ms`, `uptime_ms` | when this service process started |
| `state_schema`, `protocol` | the versions it runs with |
| `backend`, `enforcement` | `Cgroup` and `enforced`, or `Watch` and `monitoring_only` |
| `cgroup_required` | the setting `[cgroup] required` |
| `jobs` | number of Job records in each state |
| `supervisors_adopted` | supervisors of running Jobs taken over at the last start |
| `last_recovery_ms`, `recovery_changed_records` | when the last start recovered, and how many Job records it changed |
| `state_free_bytes` | space left on the file system of the state directory |
| `audit_writable`, `events_writable` | whether the last write to each journal succeeded |
| `starter_failures` | how often the thread that starts Jobs failed in a pass since the service started; it goes on after each, and the first is written to the service's output |
| `cancellations_failing`, `cancellation_retries` | cancellation operations that could not be completed and are being tried again, and how many attempts failed since the service started |

`job host --json` also carries `activity`: `sync_calls`, the `fsync` and `fdatasync` calls the service itself made since it started (its supervisors are separate processes and not counted); `job_record_writes`, the Job records it put in place; `starts_pending`, true while admitted Jobs wait for their turn to be started; and `last_change`, the `action` of the last request that changed something with the `sync_calls` and `job_record_writes` that request itself made before it was answered. A submission shows 6 and 1 there.

`job doctor` reads it in the check `health`: ok with a one-line summary, fail when a journal cannot be written or less than 64 MiB are left, warn when the service does not answer.

## Limits around the service

The service cannot lift a limit that a service manager, a container or an administrator set around it. It reports what it can discover as `surrounding_limits` in `job host --json`, in two lines of `job host`, and in the check `surrounding_limits` of `job doctor`:

- `memory_max`, `memory_high`, `cpu_max`, `pids_max`: the tightest value among the cgroup the service runs in and all its ancestors, as the kernel prints it, and in `set_by` the cgroup that sets it. `value` is `max` when no level limits it and null when the file cannot be read. When two levels set the same value, the outer one is named.
- `cpuset_cpus_effective`: the CPUs the service may use; `set_by` is null when that is every CPU of the host.
- `rlimits`: soft and hard value of each resource limit of the service process; null means unlimited. Jobs inherit them unless a Job sets its own.
- `discoverable`: false when the cgroup of the service cannot be read at all.

Without a running service `job doctor` reports the limits of the command itself and says so.

## Monitoring mode

A service that finds no delegated cgroup cannot enforce limits. It then runs in monitoring mode: it starts, tracks and stops Jobs and watches what they use. This mode is announced, never silent:

- the start line says `monitoring mode: no delegated cgroup, limits are watched, not enforced`;
- `job host` says `monitoring only`, and `health.enforcement` is `monitoring_only`;
- `job doctor` warns in the check `enforcement`;
- every Job answer ends with `limits watched, not enforced`.

To forbid the mode, set

```toml
[cgroup]
required = true
```

Then the service refuses to start without a usable delegated cgroup and says what is missing. The default is `false`, so that an installation without a cgroup keeps running. A change needs a restart.

A boundary that cannot be established is not accepted quietly. In monitoring mode with the ordinary profile:

- `--memory-max`, `--memory-high`, `--memory-swap-max`, `--cpu-limit`, `--cpu-weight` and the I/O controls are refused when the Job is submitted, as before;
- `--mem` and `--pids` are refused with a capability error when the Job would become runnable: at `run` and `submit`, at `release` of a held Job, and at a retry that is not held. A held Job can still be created and inspected.

Requests (`--cpu-request`, `--memory-request`, `--cores`), `--time` and `--write-budget` (earlier `--disk`) are not cgroup boundaries and are accepted. With the legacy profile `--mem` and `--pids` keep their earlier meaning: the service watches the Job and stops it when it passes them.

## Output and terminals for a socket group

With `[socket] group` set, members of that group control the service through the socket. The state directory stays private. A client that cannot read it gets output from the service instead: `job run`, `job logs` (with `--follow`, `--stream`, `--json` and the other options) and `job log` work unchanged. The service reads the retained output of the one attempt that was asked for and sends it over the control socket; the request cannot change anything. `job attach` and `job run --pty` reach the Job's terminal through a socket in `terminals/` below the runtime directory, which only the service user and the group can open.

`job audit` and `job events` read the state directory and stay with the service user.
