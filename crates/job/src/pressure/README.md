# Pressure observations

## What it does

Reads CPU, memory and I/O PSI from host or pinned workload cgroup directories, preserving availability, all averages and total stall microseconds. Host CPU full is explicitly undefined. The observation command can inspect saved raw pressure files without connecting to a daemon and changes no policy.

Explicit service and ancestor rules optionally gate new starts. The controller samples at a nominal one-second interval, with earlier kernel notification wakeups where available. It persists holds and gradual recovery before publication, preserves independent ancestor constraints, and reports bounded structured transition events. Missing data never counts as recovery; no rules means no monitor or pressure journal. Monitor descriptors are bounded, identity-pinned and optional, with visible sampling fallback. Offline trace replay uses the live state transition method to explain policy behavior. Complete workload performance qualification remains open.

## How to test

Use the public pressure command against the host and delegated workload cgroups, parse saved raw snapshots and replay `docs/examples/pressure-trace.json` through the CLI. Run integration tests and tools/check through job. Delegated tests observe real notification delivery, denied registration with readable measurements, descriptor retirement and capacity fallback. No unit tests are added while the grandfathered share exceeds the 6% limit.
