# Resource requests and controls

All settings below are optional. An ordinary Job without them receives no synthetic reservation, quota or weight. Its tracking cgroup retains kernel defaults and remains subject to the service manager and other surrounding cgroups.

| Job option | Meaning | Serialized unit / kernel file |
| --- | --- | --- |
| `--cpu-request N` | Capacity accounted for admission | `cpu_request_milli`, integer milli-CPUs |
| `--cpu-limit N` | CPU bandwidth ceiling | `cpu_limit_milli`, `cpu.max` |
| `--cpu-weight N` | Relative CPU share under contention | `cpu_weight`, `cpu.weight` |
| `--memory-request SIZE` | Memory accounted for admission | `memory_request`, bytes |
| `--memory-high SIZE` | Reclaim and throttling threshold | `memory_high`, `memory.high` |
| `--memory-max SIZE` | Hard memory ceiling | `memory_max`, `memory.max` |
| `--memory-swap-max SIZE` | Swap ceiling, separate from memory | `memory_swap_max`, `memory.swap.max` |

Requests neither reserve physical pages nor guarantee dedicated CPUs. They participate in the available pool and configured ancestor admission budgets. A request does not set a limit or weight. A limit or weight does not create an admission request. Kernel CPU controls are not admission priority or CPU affinity; they apply to the kernel scheduler classes that support those controls. Memory high may be exceeded and does not itself invoke OOM killing; memory max provides containment. See the [Linux cgroup v2 documentation](https://docs.kernel.org/admin-guide/cgroup-v2.html) for controller behavior.

```sh
job run --cpu-request 2 --memory-request 1G -- make
job run --cpu-limit 0.5 --cpu-weight 25 -- command
job run --memory-high 512M --memory-max 1G --memory-swap-max 0 -- command
```

CPU amounts are nonnegative decimals with a leading digit and at most three fractional digits. Quota uses a fixed 100000 microsecond period; the minimum finite CPU limit is 0.01. CPU weight is an integer from 1 through 10000. Memory sizes are whole bytes, optionally expressed with binary K/M/G/T suffixes; `0.5G` is exact and accepted, whereas `0.5B` is rejected. Arithmetic does not silently saturate or round decimal input. Memory controls round upward to kernel page boundaries, and status records the applied values.

Limits accept `unlimited`. An omitted value leaves the setting unset and eligible for inheritance. Explicit unlimited overrides a per-Job default; it cannot remove a limit imposed by a surrounding cgroup. Requests may be zero; they do not accept unlimited. Explicit kernel controls, including unlimited, require a writable delegated controller. An unavailable controller causes submission to fail before creating the Job; process watching is not a substitute. Availability is listed under `resource_controls` in `job host --json`. Creation rechecks and reads back actual controller values before starting the workload.

## Defaults in Queues and Groups

Use the `--job-` prefix to configure defaults for individual Jobs:

```sh
job group create development --job-cpu-request 1 --job-memory-request 256M
job queue create development/builds --job-cpu-limit 2
job queue set development/builds --job-cpu-weight 50
job queue show development/builds --json
job queue unset development/builds job-cpu-limit
```

Each field resolves from the nearest explicit ancestor. Explicit Job options override these defaults. The same controls with `--job-` work for Group/Queue create and set. A collection's `show --json` includes source IDs and paths. A Job's `resource_sources` records its resolution; `spec.declared` holds resolved controls, `requested_spec` preserves the latest submission/edit input, and `submitted_spec` retains the original input. `effective_spec` snapshots the launch specification. `applied_resources` records per-Job kernel values after successful setup; it is not an inventory of all external ancestor constraints or continuously refreshed kernel telemetry.

Changing a default affects future submissions and explicit retries/edits. Existing waiting and running attempts retain their resolved defaults. `unset` restores inheritance for subsequent resolutions. No additional kernel sharing domain is created merely by organizing empty Groups or Queues.

These per-Job defaults are distinct from aggregate limits for a collection. Shared enforcement uses explicit aggregate domains, described below; a Group limit is never copied into every Job. Use the explicit live-update commands below to change running controls. Current `--cores`/`--mem` collection settings remain aggregate admission budgets, not aggregate kernel quotas. Admission priority, fairness and PSI are separately tracked in the redesign plan.

## Compatibility and recovery

The old Job option `--cores N` continues to set a CPU request and derived CPU weight; it never becomes a CPU quota. `--mem SIZE` continues to pair a memory request with memory max and retains its previous downward page normalization. Differing overlapping explicit values are rejected. If the same control is also explicitly named, its native backend requirement still applies. On a service without a delegated cgroup, `--mem` and `--pids` are watched by the service in the legacy profile and refused with a capability error in the ordinary profile; see [monitoring mode](operations.md#monitoring-mode).

The old Job option `--pids N` likewise continues to count N processes and threads at admission and to limit the Job to N. `--pids-max N|unlimited` is the explicit limit: it writes the cgroup control `pids.max`, reserves nothing and needs a delegated `pids` controller. `--job-pids-max` is the per-Job default of a Queue or Group, and `--pids-max` on a Queue or Group bounds its subtree as one domain. `--pids` with a different `--pids-max` is rejected. `--write-budget SIZE` names what `--disk SIZE` does: the service counts the bytes a Job writes and stops it past the budget. This is not a file system quota, and `job host --json` reports `filesystem_quota` as unsupported.

The explicit legacy service profile retains its estimator and baseline policies. An explicit new CPU request does not derive a CPU weight even in that profile. Remote dispatch forwards settings for destination validation without turning compatibility declarations into new explicit controls.

Active local Jobs with explicit kernel controls require their original backend on recovery. Service restart retains existing controller values and attempts; it does not reapply changed collection defaults. Unsupported live changes require a new attempt. State schema 9 identifies independent resources alongside preserved legacy declarations; earlier stores require explicit offline migration. Historical declarations and backups retain their original meaning.

Every request of the command carries the protocol range it speaks, and the service refuses a request outside its own range before it reads it, so a service of another release cannot take a submission and silently discard fields it does not know. Remote requests also require matching protocol versions. See [reliability](reliability.md#protocol-versions).


## Shared Queue and Group controls

Collection `--cpu-limit`, `--cpu-weight`, `--memory-high`, `--memory-max` and `--memory-swap-max` configure a shared kernel domain. The same options with `--job-` supply individual Job defaults. Existing collection `--cores` and `--mem` remain admission budgets.

```sh
job group create production --cpu-limit 4 --memory-max 8G
job queue create production/web
job group create production/builds
job queue create production/builds/compile --job-memory-max 2G
job submit --queue production/web -- command
job submit --queue production/builds/compile -- command
job group show production --json
```

Both workloads share the production ceiling. The empty builds Group and its Queue add no kernel domains; the compile Job gets its own 2 GiB memory ceiling. `aggregate_domains` lists every applicable ancestor, its stable object ID, source path and kernel values. Job status records the launch snapshot and `workload_cgroup` identifies the leaf. Per-Job `applied_resources` describes only that leaf's explicit controls, not the ancestor ceilings.

Kernel paths use `jobs/domain-ID/.../JOB-ID`. Only explicit aggregate rules create domains. Each is a scheduling domain with the kernel's default weights unless explicitly configured; its descendants compete within that domain. Weights are relative to siblings, not percentages of the host. The service and supervisors run outside these domains. A collection does not implicitly become an indivisible OOM victim: its `memory.oom.group` remains zero. Kernel OOM events on individual workloads are recorded without claiming that a particular ancestor was the cause.

A Job is recorded as stopped for memory when the kernel killed one of its processes for memory or its own memory limit was reached. One case is not a stop: a Job that makes cgroups of its own below its cgroup, has a process killed there at a limit it set itself, and whose command then exits 0. Such a Job is recorded by its exit status, and its result carries a note with the number of processes killed below it. `job status ID --json` shows the counts: `oom_kill` for the Job's whole subtree, `own_oom` and `own_oom_kill` for its own cgroup. A cgroup a Job makes below its own is the Job's to remove before it ends.

Every configured ancestor remains binding. A local unlimited limit cannot bypass a parent ceiling. `job group unset production cpu-limit` removes the local CPU policy; other rules keep that domain present. Removing the last aggregate rule collapses the domain for subsequent attempts. An empty logical topology therefore produces the same flat workload hierarchy as the default Queue.

Ordinary set/unset refuses a change affecting active domain topology or policy. Wait for the affected work to finish, or use an explicit update for an existing domain. Per-Job defaults can still change for future submissions. Queued work checks current aggregate policy at admission; original specifications remain intact. Current aggregate constraints are never silently weakened for remote execution: remote submissions under them are refused, and already queued remote work remains waiting with a reason.

Object configuration is committed before kernel materialization. At launch, stale empty domain trees are cleaned, configured ancestors are created with readback verification, and the Job enters its leaf. A populated domain is verified without rewriting it. Failed setup prevents execution. Restart requires active attempts to retain their original domains and matching kernel values. External changes to managed aggregate controls stop affected workloads with a visible reason. Unchanged configured domains retain their identity, counters and resource charges between Jobs. Stale empty domains are removed after work ends, including idle children of populated ancestors; admission is not allowed to reuse a populated domain with different rules.


## Device I/O policies

Use `job host --json` to inspect `io_devices`. Each entry identifies a major:minor device number, kernel name, whole-device status, selected scheduler, IOCost state, BFQ low-latency state, supported controls and reasons for unavailable controls. The summary `resource_controls` marks an I/O control available only when at least one listed device supports it. `io.max` support still requires successful kernel setup/readback on the selected device before a command can start.

The following example uses `254:0`; replace it with a whole-block-device number from your host. A partition number, missing device or inactive weight backend is rejected. Device numbers address the current host and must be reviewed after storage topology changes; they are not persistent disk UUIDs. Controls apply where I/O reaches that block device, not to an arbitrary path or filesystem. Stacked devices and multi-device filesystems need explicit administrator-selected device policies.

```sh
job run --io-max 254:0,wbps=4M,wiops=100 -- command
job group set production --io-max 254:0,rbps=16M,wbps=8M
job queue set production/builds --job-io-max 254:0,wbps=2M
job run --io-weight 254:0=200 -- command
job run --io-bfq-weight 254:0=200 -- command
```

`rbps` and `wbps` are whole bytes per second with the same binary size suffixes used by memory controls. `riops` and `wiops` are whole operations per second. Omitted directions are unlimited; spell an explicit unlimited direction as `wbps=unlimited`. Finite byte rates range from 1 through 18446744073709551614; finite IOPS from 1 through 4294967294. Zero and kernel sentinel values are rejected rather than reinterpreted. A rate is not a filesystem space quota. Cached reads may cause no device I/O, buffered writes may reach the device later, and short bursts can exceed an interval's nominal rate.

Repeat an option for distinct devices. Duplicate device entries in one command fail; include all desired directions in one entry. Each I/O option is one complete policy map: an explicit Job map overrides the inherited map, and the nearest per-Job default map replaces a more distant map. Different options resolve independently. A collection `set` replaces its local map; supply every device you want to retain. `unset ... io-max` removes a local aggregate map; `unset ... job-io-max` restores inheritance of a per-Job default. Ancestor aggregate policies always remain binding, including when a leaf requests unlimited.

`--io-weight DEVICE=N` selects the IOCost controller, whose range is 1–10000. The administrator must already have enabled IOCost for that device. `--io-bfq-weight DEVICE=N` selects BFQ, whose range is 1–1000; BFQ must be the active device scheduler and its automatic `low_latency` weight raising must be off. Values are not rescaled between backends. The service never switches device schedulers or changes root IOCost/latency policy. Relative weights are not admission priorities, guaranteed throughput percentages or hard IOPS limits. Both controls require contention and a supported workload/device path to influence service.

At launch, I/O values are written one device at a time and compared semantically with kernel readback. Unordered device rows and omitted all-unlimited rows are handled explicitly. Status records the desired policy and applied canonical values. Missing devices, backend deactivation, modified managed values and unavailable controller files are visible errors. Active recovery compares recorded controls with the kernel instead of adopting external changes. Ordinary collection set/unset still refuses changes to active aggregate policies; use explicit updates below.

Controller semantics and limits follow the [cgroup v2 documentation](https://docs.kernel.org/admin-guide/cgroup-v2.html), [BFQ documentation](https://docs.kernel.org/block/bfq-iosched.html), and the kernel's [throttle parser](https://github.com/torvalds/linux/blob/master/block/blk-throttle.c). Actual weight-share qualification on an enabled IOCost/BFQ device remains part of the deployment matrix.

## Live resource changes

```sh
job update 42 --cpu-limit 2 --memory-high 1G --dry-run
job update 42 --cpu-limit 2 --memory-high 1G
job group update production --cpu-weight 200
job queue update production/web --memory-max 4G --allow-oom
job resource-update OPERATION --json
```

`update` accepts CPU limit/weight, memory high/max/swap-max, and device I/O max/weight/BFQ-weight. Jobs must be local and running or suspended. A collection must already have a materialized resource domain; adding/removing active domains requires draining. Collection updates change the shared domain, its configuration for future starts and affected attempt snapshots. They do not copy the aggregate limit into each Job.

`--dry-run` returns a preview without writes. A normal command obtains a preview and commits it after revalidation. The receipt identifies the boot, cgroup device/inode, affected attempts, prior/desired values, individual writes and progress. Repeating the same committed operation is idempotent; a new CLI update creates a new operation. Exit 75 means pending: inspect the printed ID with `resource-update`. Exit 0 means verified and published. An ended target returns 1. After connection loss, query the operation ID reported in the diagnostic before submitting another update.

Every finite decrease of `memory.max` requires `--allow-oom`, including changing unlimited to finite. This can reclaim memory and terminate processes in the cgroup. `memory.high` can cause substantial throttling. Nonblocking memory writes avoid doing synchronous reclaim in the service; readback confirms the configured limit, not that usage is already below it. Kernel documentation explicitly permits delayed enforcement. The current `memory.oom.group` setting is preserved.

Commands, environments, reservation accounting, submitted/requested specifications and effective launch specifications remain unchanged. Current leaf controls appear in `applied_resources`; current ancestor controls appear in `aggregate_domains`; operation receipts retain the change history. Existing ancestor ceilings remain binding. Use unlimited limits or normal weights to relax a live control. Removing managed fields or the last collection domain uses ordinary unset after draining. Replacing an I/O map resets omitted devices explicitly.

Writes across files/devices are not atomic. Intent and each acknowledged write are persisted. A restart reconciles an interrupted write against its before/after values and finishes metadata publication after verification. Unexpected values are never overwritten as part of replay. An incomplete operation blocks new admission and conflicting administrative changes; status, attach, signals, cancellation and freezer controls remain available. Kernel writes run in a worker so reclaim cannot hold the service mutex. Receipts remain after Job removal and are included in offline backups; pending operations prevent backup/migration. Full power-loss and deployment qualification remain release work.

If a persistent controller error prevents completion, inspect the receipt and restore the required controller/device availability. To discard an incomplete operation, first cancel or finish its affected workloads, then use `job resource-update OPERATION --abandon`. This requires an empty target domain and no active update worker. It records an ended operation without rolling back completed writes; ordinary inactive set/unset and cleanup can then reconcile the domain. A verified update finishes metadata publication instead of being abandoned.
