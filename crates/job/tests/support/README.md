# Delegated service fixture

## What it does

Starts a private job daemon inside a caller-owned delegated cgroup, with a private state directory. Provides CLI, restart and lifecycle observations. Cleanup kills only the fixture subtree and restores its temporary controller delegation. Used by the delegated kernel integration suite and pressure workload benchmark.

## How to test

Run `cargo test --test freezer -- --ignored --test-threads=1` through the installed job service with delegated controllers. Run the pressure benchmark as described in `docs/pressure-benchmark.md`.
