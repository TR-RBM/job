# Workload suspension

## What it does

Reads and writes the cgroup v2 freezer, retaining desired state separately from observed completion. cgroup.freeze selects the local request; cgroup.events confirms the effective state, including ancestor freezes. Unsupported or malformed interfaces fail explicitly. Controls never create missing kernel files and never substitute process signals for subtree freezing.

The daemon persists intent before writing the kernel and reconciles pending requests on each tick and after recovery. Suspended work retains reservations, concurrency occupancy and elapsed deadlines. Cancellation requests thaw before TERM. Collection operations select a snapshot of active Jobs within the mixed subtree, require --recursive and return per-Job results. They do not affect future submissions or Queue pause state. Request generations detect opposing controls while a caller waits; the daemon mutex is released during confirmation waits.

Timing separates elapsed duration from observed frozen intervals and their difference, active duration. This is sampled wall-clock accounting, not CPU accounting. The stored cgroup path prevents silently recovering a suspension request against a different backend. Completed results are still collected when the original cgroup has already disappeared.

## How to test

Run tools/check through job for ordinary tests. Then run cargo test --test freezer -- --ignored --test-threads=1 through job inside a writable delegated cgroup. The live tests create their own subtree within the test Job, never reconfigure the installed service and kill/remove only their own subtree during cleanup. They verify kernel confirmation, ancestor holds, restart, durable pending intent, offline completion, retained admission occupancy, TERM handling, elapsed deadlines, mixed recursive selection and persistence failure before kernel mutation. Watch tests verify explicit refusal without changing execution.
