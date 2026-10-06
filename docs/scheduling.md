# Admission ordering

An empty Queue organizes Jobs without enabling priority, aging, backfill or kernel weights. Baseline dispatch considers arrival order and skips work that cannot currently start. Concurrency and resource gates apply only as configured. Priority is optional and affects waiting work; it never changes the kernel's CPU or I/O weights and never preempts running work.

## Configure a policy

```sh
job group create production --priority-min -100 --priority-max 300
job queue create production/builds --priority 50 --aging 30s --backfill conservative
job submit -q production/builds --priority 100 --cpu-request 2 --time 5min -- make
job explain 42
job explain 42 --json
job reprioritize 42 --priority 200
job queue unset production/builds aging
```

Use the identifier returned by submission in place of `42`. Queue and Group `set` accept the same scheduling fields. `unset` removes the local value and reveals any inherited value. A priority default is resolved at submission, edit or retry; changing that default does not rewrite already submitted Jobs. Explicit reprioritization accepts held or queued Jobs and records the attempt, Unix actor UID, time and previous/new priority. Original submission remains intact; retry resolves the latest requested priority.

Base priority is an integer from -1000 through 1000; higher values rank first. An absent priority compares as zero but remains absent in inspection. Every ancestor's `priority-min` and `priority-max` applies independently. Children cannot widen a parent's allowed range. Tightening a bound can hold an existing queued Job until its priority or the bound is changed; it does not silently clamp values or affect a running Job.

## Aging

`--aging DURATION` adds one ranking point per complete interval of eligible waiting time. Accepted units are `ms`, `s`, `m`, `min` and `h`; a bare number means seconds. Values must be positive, exactly representable as whole milliseconds and fit an unsigned 64-bit millisecond count. Sub-millisecond values and overflow are rejected.

```text
effective priority = base priority + floor(eligible waiting milliseconds / aging interval)
```

Credit accrues after release while queued, aging is enabled and no ancestor is paused. Resource waits count. Held time and administrative pauses do not. Renames, moves and edits preserve an attempt's accumulated credit; retry begins a new attempt with zero credit. Disabling aging retains stored credit but stops accrual. Changing the interval reinterprets retained credit using the new interval. Unsetting an interval can reveal an inherited interval rather than disable aging.

The service persists credit using boot identity and Linux `CLOCK_BOOTTIME`. Same-boot restart includes the gap only if both saved and recovered states are eligible. Across a reboot, only confirmed credit is retained. Host suspend counts as elapsed waiting on the same boot. Wall-clock corrections do not change accumulated credit. Scores use signed 128-bit arithmetic with no aging cap; the stored millisecond counter uses checked addition. A persistence or arithmetic error stops admission and is visible through `explain`.

With sufficient eligible waiting, a low-priority Job can outrank newly arriving maximum-priority Jobs. This does not guarantee a start if occupants run indefinitely, the Job does not fit, or administrative holds remain.

## FIFO and backfill

`--strict-fifo true` on any ancestor requires release/arrival order across that subtree before urgency is considered. A child's `false` cannot disable a parent's `true`. A paused head can therefore hold its whole FIFO scope. Strict FIFO may leave resources idle.

`--backfill off` preserves an ordered Job's opportunity by holding later competing work while it waits for capacity. Competition means positive requests for the same host resource dimension, a common reserved device, or a shared ancestor with a configured concurrency ceiling. Jobs without a shared constraint can start independently; an unrelated Queue does not inherit another Group's start barrier. Protection also applies through intermediate protected waiters with overlapping resource sets. `--backfill conservative` allows later competing work only when its planned interval leaves earlier placements feasible. The planner checks host capacity, device reservations and every ancestor's concurrency/request budget. Kernel weights and hard ceilings are separate from these request budgets.

Planning uses an explicit execution time limit plus the service's termination grace and dispatch interval, or a legacy estimate where available. Without either, duration is unknown. These are predictions, not promised start times: overruns, delayed termination and external changes require replanning. An unknown end can prevent backfill. Baseline work without ordering policy retains ordinary skipping behavior; strict FIFO remains stronger than priority or backfill within its scope.

## Explain decisions

`job explain ID` reports the priority source, all ancestor priority bounds, retained waiting credit, effective score, aging interval/source, backfill mode, FIFO predecessor and current blocking reason. `--json` returns schema-versioned data; effective priority is a decimal string to retain exactness. Predicted starts are Unix milliseconds and may be absent. For an attempt launched with ordering enabled, later inspection uses its saved launch-policy snapshot; it does not reinterpret history using today's configuration. `job status ID --json` includes reprioritization records.

## Hierarchical fair share

Enable a scope on a Group and weight its immediate children explicitly:

```sh
job group create research --fair-share cpu-request-time
job group create research/builds --share-weight 2 --fair-share memory-request-time
job queue create research/builds/interactive --share-weight 1
job queue create research/builds/batch --share-weight 3
job queue create research/other --share-weight 1
job submit -q research/builds/interactive --cpu-request 2 --memory-request 1G -- make
```

`fair-share` is local to the Group; it is not an inherited default or valid on a Queue. `share-weight` is local to an immediate child and accepts integers 1–10000. An absent child weight means 1 only within a configured scope. Both kinds of child compete in the same parent scope. Adding children inside `research/builds` cannot multiply its outer share of `research`.

| Mode | Accounted service | Required resolved reservation |
|---|---|---|
| `cpu-request-time` | Requested milli-CPU × milliseconds | Positive CPU request |
| `memory-request-time` | Requested bytes × milliseconds | Positive memory request |
| `off` or unset | No local fair-share scope | None added by that Group |

Requests may come from explicit per-Job declarations, resolved defaults or estimation in an explicitly selected legacy profile. The fair-share policy never invents requests. All ancestor scopes apply, so the example requires both resources. A missing required reservation is refused on submission; existing queued work is held with an explanation until edited. Empty topology with no configured scope remains unaffected.

At a configured scope, prefer the competing child with the lowest cumulative service divided by weight. Recurse through nested scopes, then compare urgency/aging among the eligible Jobs in the selected branch. Unconfigured branches merge their candidates by urgency. Strict FIFO and other admission gates remain binding. Unused shares can be borrowed; weights are relative targets under contention and introduce no kernel limits, priorities or controller domains.

Accounting charges retained reservations in starting, running, suspended and stopping states until the service releases them. This includes supervision/reconciliation delay, not just application CPU activity. Same-boot daemon downtime charges the last recorded reservations until recovery observes their release. A reboot retains confirmed history without inventing a cross-boot interval. Memory may remain charged while suspended; the CPU-request-time mode also charges the retained CPU reservation even when the process uses no CPU. This is explicitly reservation fairness rather than utilization fairness.

The horizon is cumulative participation history, not a rolling time window. Each scope retains a monotonic minimum-service watermark. New or rejoining children enter at no less than that watermark and retain any debt above it. Idle time therefore does not accumulate an unlimited burst entitlement. Retry does not erase a Queue's or Group's past service. Names can change without resetting IDs. Moving inactive work into another scope subjects it to that scope's rejoin floor; active movement remains unsupported.

Counters use checked 128-bit arithmetic and exact per-weight remainders. JSON encodes whole service counters as decimal strings. Weight changes retain historical normalized service, round any residual up by less than one service unit, then apply the new divisor to future service. Changing/enabling/disabling a scope's accounting mode requires its subtree to have no active Jobs; queued or held work can remain. `group unset PATH fair-share` removes only that local scope and leaves ancestors binding.

The planner adds a temporary lookahead charge for one dispatch interval of each active reservation and each provisionally ordered start. This distributes simultaneous starts before elapsed service is available. The lookahead is not written as consumed service. Explanations show the interval, current active rate, scope/child IDs and paths, weight, whole service, fractional remainder and watermark. Existing launch snapshots retain the settings used at launch.

These shares do not guarantee start times or converge while all needed capacity is held by indefinitely running Jobs. Large or long non-preemptive reservations can temporarily exceed their target; later starts rebalance accounted service. Protected backfill continues to check host and ancestor feasibility. Persistence or counter errors stop admission while lifecycle controls remain available. Scheduling classes and PSI admission remain under development.
