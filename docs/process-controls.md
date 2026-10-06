# CPU placement, NUMA and process limits

All settings on this page are optional. A new Queue has none. An ordinary Job without them inherits the operating-system settings of the execution supervisor. They are launch settings, separate from admission reservations, cgroup quotas and kernel weights.

```sh
job host --json
job run --cpu-affinity 0-3,8 -- command
job run --numa-policy bind:0 -- command
job run --rlimit nofile=1024:4096 --rlimit core=0 -- command
job group set research --job-rlimit core=0
job queue set research/builds --job-cpu-affinity 0-3 --job-rlimit nofile=1024:4096
job queue unset research/builds job-rlimit-nofile
```

Choose CPU and node IDs from the execution host. `host --json` exposes `process_controls.allowed_cpus`, `allowed_memory_nodes`, query errors and `inherited_rlimits`. A successful NUMA query does not guarantee permission to set every policy; actual application and readback are required before the program executes. An explicitly unavailable setting fails; job never silently narrows the requested mask. A queued Job is checked again when starting.

## CPU and NUMA semantics

`--cpu-affinity LIST` accepts comma-separated IDs and inclusive ranges, normalizes overlap/order and rejects malformed, empty or out-of-range lists. The implementation supports CPU IDs up to 65535; availability is the supervisor's current affinity. `inherit` suppresses an inherited job default and preserves the supervisor's affinity. The setting survives fork and exec. It is not an exclusive reservation or a cpuset boundary: a workload may change its own affinity within the kernel's enclosing constraints. [sched_setaffinity(2)](https://man7.org/linux/man-pages/man2/sched_setaffinity.2.html)

`--numa-policy` accepts:

| Value | Launch behavior |
|---|---|
| `inherit` | Preserve the supervisor's inherited task policy. |
| `default` | Explicitly clear a task policy and use the kernel default. |
| `local` | Prefer allocation local to the executing CPU. |
| `bind:LIST` | Apply binding to the specified memory nodes. |
| `interleave:LIST` | Apply interleaved allocations across the nodes. |
| `preferred:N` | Prefer one node, with fallback permitted. |

NUMA policies affect future allocations; they do not migrate existing pages or select CPUs. Explicit node lists must be allowed and memory-bearing at preparation. They use physical IDs with `MPOL_F_STATIC_NODES`; later cpuset/hotplug changes can alter the kernel's behavior, including local fallback when no requested node remains usable. This is task placement, not an immutable memory isolation guarantee. The workload may replace its task policy. [set_mempolicy(2)](https://man7.org/linux/man-pages/man2/set_mempolicy.2.html), [kernel NUMA policy](https://docs.kernel.org/admin-guide/mm/numa_memory_policy.html)

CPU masks and NUMA masks have different kernel size limits. NUMA buffers stay within a page and `get_mempolicy` verifies allowed nodes and applied policy. This host qualifies policy set/readback on node 0; multi-node distribution and locality performance need a multi-node deployment. [get_mempolicy(2)](https://man7.org/linux/man-pages/man2/get_mempolicy.2.html)

## Rlimit syntax and units

`--rlimit NAME=SOFT:HARD` sets one resource. A single value sets both bounds. Each bound accepts `inherit`, `unlimited` or a nonnegative value; the numeric infinity sentinel is refused. Byte resources accept the existing binary K/M/G/T size suffixes. Repeated options select different resources; duplicate definitions of the same resource in one command are rejected.

| Resource | Unit and scope |
|---|---|
| `as`, `core`, `data`, `fsize`, `memlock`, `stack` | Bytes; per-process/kernel-specific limit semantics. |
| `msgqueue` | Bytes, accounting for the real UID. |
| `cpu` | CPU seconds per process, distinct from elapsed wall time and CPU quota. |
| `nofile` | Descriptor-number ceiling, not a cgroup-wide descriptor budget. |
| `nproc` | Threads counted for the real UID; privileged exceptions apply. |
| `sigpending` | Queued signals counted for the real UID. |
| `nice` | Kernel nice ceiling encoding, 0–40; not an admission priority. |
| `rtprio` | Realtime priority ceiling, 0–99; does not itself select realtime scheduling. |
| `rttime` | Microseconds of realtime CPU time under the kernel's blocking/reset rules. |

Soft must not exceed hard after inherited bounds are resolved. Raising a soft limit within hard is normally permitted; hard-limit increases require the relevant operating-system authority. A workload can raise its own soft value up to hard, so a soft value alone is not an immutable ceiling. Limits survive fork and exec. `rss` and `locks` are rejected because modern Linux does not enforce them. These settings retain Linux's precise per-resource behavior; in particular, `nproc` does not replace `pids.max`. [getrlimit(2)](https://man7.org/linux/man-pages/man2/getrlimit.2.html)

## Inheritance, profiles and history

Defaults use `--job-cpu-affinity`, `--job-numa-policy` and repeated `--job-rlimit`. Each rlimit resource is an independent field, so a Queue's nofile setting leaves a Group's core setting intact. Unsetting `job-rlimit-nofile` restores ancestor/default resolution. Explicit `nofile=inherit` preserves operating-system bounds for that resource instead; it does not erase other resources.

Pinned profiles support these fields:

```toml
[[presets.profiles]]
name = "worker"
revision = 1
[presets.profiles.values]
cpu_affinity = "0-3"
numa_policy = "bind:0"
rlimit_nofile = { soft = 1024, hard = 4096 }
rlimit_core = { soft = 0, hard = 0 }
```

The existing nearest-field/profile rules and explicit Job overrides apply. `resource_sources` reports origins; submitted/requested/effective specifications retain the declarations. Successful execution records verified explicit settings in `result.process_controls`; partial `inherit` bounds have resolved numeric/unlimited values there. A failed pre-exec setup does not claim an applied snapshot. Applications may subsequently modify settings where Linux permits it, so this is launch evidence, not ongoing measurement.

Changing defaults affects subsequently resolved submissions/retries. Edit held/queued Jobs to change their requested controls. `job update` remains for supported live cgroup controls; it does not rewrite placement or process limits of running descendants. Remote declarations are passed to the execution host, and the local SSH transport retains its own settings. State schema 16 and protocol 18 identify this contract. Migration leaves old controls absent and validates history without requiring historic CPUs/nodes to exist locally.
