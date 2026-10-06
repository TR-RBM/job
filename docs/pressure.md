# Pressure observation and admission

`job pressure` reports Linux Pressure Stall Information (PSI) for the host. `job pressure ID` reads the recorded cgroup of the current local active Job attempt. Both accept `--json`. An unknown Job ID is an error; a known attempt without local measurements reports unavailable values.

## Measurements

Each CPU, memory and I/O observation has separate `some` and `full` readings. `some` measures time during which at least some work was stalled on the resource; `full` describes simultaneous stalls of all non-idle work in the measured scope. Host CPU `full` is undefined and is always labelled as such, even when the kernel prints a zero placeholder.

The kernel's avg10, avg60 and avg300 are averages over their respective time scales in seconds. The text output shows percentages. Schema-one JSON uses `avg10_bp`, `avg60_bp` and `avg300_bp` as integer basis points: 100 means 1%, and 10000 means 100%. `total_us` preserves cumulative stall microseconds as an unsigned 64-bit integer. Consumers must preserve integer precision; a stall total can exceed the exact integer range of a floating-point JSON consumer. Totals are not instantaneous pressure or CPU utilization.

Each reading has `availability` equal to `available`, `unavailable` or `undefined`. Unavailable readings carry a reason instead of numbers. A valid `some` measurement remains available even when `full` is missing or malformed. Source paths identify attempted reads when the parent directory could be opened.

Local `cgroup.pressure=0` disables accounting and is reported as unavailable, even if pressure files remain readable. job never enables it implicitly. This switch is not hierarchical; disabling a parent does not disable its children. Older kernels without this switch are checked through their pressure files. See the [cgroup v2 interface](https://docs.kernel.org/admin-guide/cgroup-v2.html) for its semantics.

Snapshots include the Job/attempt where applicable, wall-clock sample time in milliseconds, boot-relative time and boot ID when available, and the observed cgroup's device/inode. Resources are read sequentially; the timestamp marks the start of sampling, not an atomic kernel snapshot. The directory is pinned while its files are read. The identity describes the sampled directory and is not proof of the cause of pressure.

Watch-backend, remote, unstarted and completed attempts have no local workload pressure measurement. These cases never substitute host PSI. Local readings disappear after the workload cgroup is removed; this command does not archive samples or offer historical attempt pressure.

## Saved files

Inspect a saved raw kernel pressure file without a daemon:

```sh
job pressure parse saved-memory-pressure.txt --resource memory
job pressure parse saved-cpu-pressure.txt --resource cpu --host
```

The output is schema-one JSON with an `observation` object. `--host` selects host CPU semantics. Inputs are capped at 4096 bytes and final-component symlinks are refused. Unreadable, oversized or non-UTF-8 input fails the command; malformed metrics are represented as unavailable in a successful observation. Unknown fields are allowed, but duplicate fields, invalid numbers and missing required values are rejected for the affected metric.

## Scheduling boundary

Observation changes no policy, creates no resource domain and registers no threshold monitor. Empty Queues retain ordinary execution. Optional admission rules described below must be explicitly configured. The legacy service profile's existing emergency behavior is a separate policy.

Pressure describes affected work. It does not establish which workload caused contention, and it does not guarantee advance warning of an out-of-memory event. See the [Linux PSI documentation](https://docs.kernel.org/accounting/psi.html) for kernel semantics.

## Explicit admission rules

Service configuration accepts `[[pressure]]` TOML entries, which read host PSI and apply to every new service start, including remote submissions. Queue and Group `create`/`set` accept `--pressure FILE`, containing a JSON array of rules. `queue unset PATH pressure` or `group unset PATH pressure` removes only that object's rules. Existing ancestor rules remain binding; empty child arrays cannot clear them.

Each rule requires all these fields:

| Field | Meaning |
| --- | --- |
| `id` | Unique within its policy array; 1–64 ASCII letters, digits, hyphens or underscores |
| `resource` | `cpu`, `memory` or `io` |
| `metric` | `some` or `full`; host CPU full is rejected |
| `window` | `avg10`, `avg60` or `avg300`, selecting a kernel average |
| `high_bp` / `low_bp` | Activation and recovery thresholds, integer basis points, `0 <= low < high <= 10000` |
| `sustain_ms` | Continuous observed high-pressure time before a hold |
| `minimum_hold_ms` | Minimum time a hold remains before recovery |
| `recovery_ms` | Continuous observed low-pressure time before recovery or another recovery step |
| `step_ms` | Minimum interval between recovery steps |
| `required` | Whether unavailable measurements must prevent new starts |

Each duration must be an integer from 1000 through 9223372036854775807 milliseconds. Each array accepts at most 32 rules. Unknown fields, duplicate IDs and invalid thresholds are errors. There is no default rule or deployment threshold. In particular, CPU utilization alone is not used, and CPU waiting does not impose a blanket policy without explicit configuration.

For example, the bounded CPU qualification uses this local rule to make its throttled test workload trigger a hold. These values test behavior; they are not production recommendations:

```json
[
  {
    "id": "cpu-local",
    "resource": "cpu",
    "metric": "some",
    "window": "avg10",
    "high_bp": 1,
    "low_bp": 0,
    "sustain_ms": 1000,
    "minimum_hold_ms": 2000,
    "recovery_ms": 1000,
    "step_ms": 1000,
    "required": true
  }
]
```

After saving rules chosen for your workload:

```sh
job group set compute --pressure pressure-rules.json
job pressure status
job explain 42 --json
job pressure events
```

Service host rules use the same fields in TOML. Check them with `job config check FILE`. Configuration reload retains its existing drained-service requirement. Local rule changes take effect on the next dispatch evaluation. Replacing or removing a rule explicitly resets its transient state. A rule follows its immutable object ID through rename/move; its affected Jobs follow current ancestry.

## Local measurement and availability

Local rules read the existing active descendant workload leaves and use the maximum selected average. This is an **any-descendant pressure predicate**, not an aggregate PSI measurement. Each reported signal identifies the winning Job/attempt and the number of unavailable descendant readings. An empty Queue or Group creates no kernel domain merely for monitoring.

A supported local scope with no active Jobs is reported as `empty`, with no invented numeric value, and can recover. Capability is checked on the existing workload parent. Remote and watch-backend active work has unavailable local PSI. New remote submissions under a required local rule are rejected because no local measurement can be obtained; already queued remote work remains gated if such a rule is added later. Required rules hold if any relevant measurement is missing; optional rules may activate from available high readings, but missing descendants prevent recovery. If all readings are unavailable, an optional open rule remains open and an existing restrictive state remains in place. This reports the capability gap without substituting host data.

## Activation and recovery

The controller uses a nominal one-second sampling interval on the daemon tick, with earlier notification wakeups and immediate reevaluation after explicit rule changes. Sampling is not a real-time deadline. The first observed high sample starts the activation timer; a low or intermediate sample resets it. A sampling gap longer than two seconds resets continuous activation/recovery evidence. Crossing the sustained-high duration enters `holding` with temporary concurrency zero.

After both the minimum hold and continuous low-pressure duration, the rule enters `recovering`. Its temporary `max-running` starts at the currently active count plus one. While low pressure continues, it increases by at most one per step interval. Once the target covers current active and queued demand, a subsequent step clears the target and returns to `open`. Intermediate pressure stops increases; sustained high pressure holds again. Missing data never grants a recovery step. This bounds concurrency, not the number of short Jobs that can start per interval.

Starting, running, suspended and stopping Jobs count as active. Existing limits, fairness, pauses, strict FIFO and ancestor gates continue to apply. The controller never edits saved concurrency settings, suspends or kills work, or changes kernel weights. Queued Jobs keep their identity, order and eligible aging credit. Priority cannot bypass pressure. A permanent hold or indefinitely running work can prevent a start.

## Persistence and diagnosis

`pressure.json` stores controller state and the latest 256 structured transition events. The service atomically writes it before publishing changed gates. A write failure stops new admission while cancellation and other lifecycle controls remain available. Once persistence succeeds, admission is reconsidered. Same-boot restart retains restrictive state but requires fresh continuous signal evidence. Reboot restarts hold timing and converts a retained recovery ceiling into a hold before reevaluating it. Configuration changes remain explicit administrative overrides.

`job pressure status [--json]` emits versioned JSON with all rules, source scopes, signals, phases, temporary ceilings, the sampling interval and persistence errors. `job pressure events [--json]` includes monotonically increasing sequence numbers, wall time, boot identity, previous phase/target and the transition's full policy/measurement context. Events older than the ring are discarded; export them when longer retention is required. Reconsideration and timer fields are boot-relative milliseconds and must be read with the report's boot ID.

`job explain ID --json` includes the applicable pressure views. Waiting status carries the blocking reason. Launched attempts retain their pressure explanation snapshot; these are launch observations, not current measurements of completed work. Full pressure observation remains available separately through `job pressure ID`.

Comparisons of throughput, wait time, interactive latency and oscillation remain open. The bounded live and trace qualification demonstrates admission behavior, not a tuned production policy or OOM guarantee. No emergency victim is selected by these rules.

## Kernel notifications and fallback

Explicit rules prefer kernel PSI notifications where writable pressure files support them. A notification requests an earlier measurement; it does not directly hold a Job. Each monitor uses a two-second kernel tracking window, with stall microseconds calculated as `high_bp * 2000000 / 10000`. This window is valid for unprivileged PSI monitors. The admission rule still reads its selected avg10/avg60/avg300 and applies all its existing timing conditions.

The daemon polls outside its control mutex. Owned file handles remain valid while a poll is outstanding; removed registrations close after those outstanding references are released. A source is pinned to the observed cgroup directory identity and Job attempt. Final-component symlinks and unexpected filesystems are refused before a trigger is written. Equivalent sources and thresholds share a descriptor, including rules on different ancestors.

Monitor source `cgroup_identity` is `[device, inode]`; `job_attempt` is `[Job ID, attempt]`. Host sources have null values for both. Unavailable selected measurements retire their monitor registrations and remain visible in the rule's signal and the raw pressure observation.

At most 256 distinct sources are retained in the monitor registry. The remaining sources use periodic sampling. A failed registration, lost source or polling error records a reason and retries registration after 30 seconds if the source remains applicable. A new attempt or changed threshold has a separate identity. Removing the policy or finishing the workload retires its monitors. There are no notification registrations when rules are absent.

`job pressure status` reports `notifications_and_sampling` when at least one monitor is registered, otherwise `sampling`. Its `notifications` object includes the tracking window, capacity, omitted source count, runtime notification total and individual source records. Each record names its referring policy keys, Job/attempt, observed identity, high threshold, exact stall threshold, registration state, error, retry time and received notification count. Counters restart with the daemon and are diagnostic telemetry, not durable audit counts. A mixed registry can have both registered and unavailable sources.

Notification-driven observation batches are separated by at least 250 milliseconds. Bursts are coalesced without postponing the ordinary ticker. Periodic measurements continue for recovery and fallback, so silence from the notification channel never counts as pressure recovery. `required` requires valid measurements; denied notification permissions alone do not hold work if sampling remains available.

## Replay a pressure trace

```sh
job pressure replay docs/examples/pressure-trace.json --json
```

This offline diagnostic uses the same state transition method as live admission. It lets you inspect how an explicit rule responds before enabling it, without contacting the service, registering monitors or executing work. The bundled example is a synthetic qualification trace covering a short spike, sustained pressure, minimum hold, intermediate pressure, gradual recovery, unavailable/partial observations, same-boot restart, reboot and a sampling gap.

Input is one JSON object with `schema_version: 1`, `host` (boolean), one `rule` using the fields above and a nonempty `frames` array. Each frame contains:

| Field | Meaning |
| --- | --- |
| `boot_id` | A nonempty identifier of at most 128 bytes |
| `at_ms` | Boot-relative observation time in milliseconds |
| `signal` | The selected average signal: `available` with `value_bp` and `missing`, `unavailable` with `reason`, or local `empty` |
| `active` | Number of active Jobs in the scope |
| `demand` | Active plus queued demand, at least `active` |
| `restart` | Optional boolean; true resets continuous evidence for a daemon restart |

The available signal may include paired `job_id` and `attempt` values for a local winning leaf. Omitted identity values describe synthetic or unattributed local input. Host signals cannot claim a winning Job or missing descendants. A local available signal requires active work and fewer missing readings than active Jobs; an empty local signal requires active zero. Values outside 0–10000 basis points are refused. Same-boot time must strictly increase, and a trace cannot return to a previously ended boot. A changed boot ID applies the same reboot transition as live recovery. Unknown fields and malformed data fail rather than becoming a zero-pressure observation.

Input is limited to one MiB and 4096 frames; final-component symlinks are refused. Output is schema-one JSON containing a sample after each frame: boot identity, active/demand counts, the full rule view and `admission_held`. That boolean only describes whether this pressure rule would block one additional start. Local replay scope ID zero and path `replay` are synthetic and refer to no saved object. The command does not simulate other scheduler gates, predict throughput or supply calibrated thresholds.

## Workload comparison

Use the [bounded workload benchmark](pressure-benchmark.md) to compare actual throughput, admission waiting, interactive response and controller transitions with and without explicit CPU pressure policies. It records raw observations and uses isolated delegated resource limits. Replay explains transitions; measured workload comparisons determine whether a proposed policy helps its intended use case.

## Emergency stop in the legacy profile

Everything above holds new starts and never ends running work. The `legacy` service profile has a separate, older policy that does end work: when the host runs short of memory or of space on a file system, the service stops one running Job. The ordinary profile has none of it. `job config show --json` reports whether it is active, as `emergency_host_termination` and `host_and_filesystem_floors`. Its thresholds are fixed in the program and cannot be configured.

The service checks every 250 milliseconds. There are three conditions.

**Available memory below the floor.** The floor is 12 % of the host's total memory. The condition holds when `MemAvailable` in `/proc/meminfo` is below that floor and, at the same time, the free swap on disk is at or below 12 % of the total swap on disk. Swap on a zram device is not counted as swap on disk, so on a host with no disk swap the memory figure alone decides.

**Sustained memory pressure.** The condition holds when the host's memory pressure, the `full` line's `avg10` in `/proc/pressure/memory`, has been at 60 % or more at every check for 30 seconds. One check below 60 % starts the 30 seconds again. On a kernel without that file this condition never holds.

**A file system below its floor.** The floor is 5 % of the size of the file system. The condition holds for a file system that has less free space than that and holds the working directory of at least one running Job.

### Which Job is stopped

For the two memory conditions the victim is the running Job with the largest measured memory use at that moment, among Jobs that are not already being stopped. With a delegated cgroup the measure is the Job's `memory.current`; in watch mode it is the sum of the resident memory of the Job's process tree. Reservations, requests, limits, priority, Queue, age and running time play no part. Where several Jobs have the same measure, which of them is chosen is not defined. A Job that runs on another host through `--on` is measured by its local `ssh` process only.

For a file system the victim is, among the running Jobs whose working directory is on it, the one that has written the most bytes.

### How it is stopped

The Job's processes are killed at once, without a TERM signal and without a grace period. With a delegated cgroup the whole cgroup is killed. The Job ends as stopped by the service, `job wait` returns 1 for it, and its record carries one line with the reason and the figures, for example:

```text
stopped: the host's available memory fell to 3.10 GiB, below the floor of 3.72 GiB; this job held 9.80 GiB, the most of any job
stopped: memory pressure stayed at 72 % (full, avg10) for 30s; this job held 9.80 GiB, the most of any job
```

Only one Job at a time is stopped for memory. While that Job has not ended and been recorded, neither memory condition selects another. After it has ended, a condition that still holds selects the next largest; the 30 seconds of the pressure condition start again after each stop. File-system stops are not limited in this way: at each check, every file system below its floor loses its largest writer.

### Limits of this policy

- It reacts after the shortage exists. It does not prevent the kernel's out-of-memory killer from acting first, and the kernel chooses its own victim.
- The largest Job is not necessarily the cause. Memory used outside job is not considered, and when no Job is running nothing is stopped.
- It measures use, not what was allowed: a Job well inside its own `--memory-max` can be the victim.
- There is no exemption. A Job cannot be marked as protected from it.
- The policy as a whole has no automated test. The floor arithmetic is covered by unit tests in `crates/job/src/host.rs`; the selection and the stop are not exercised by the integration suite.
