# Execution workload benchmarks

## What it does

The pressure benchmark compares explicit admission rules against the ordinary scheduler using private daemons and bounded CPU or memory workloads. It records public CLI observations, durable Job results and kernel counters in JSON Lines. Helpers have explicit work and residency bounds and independent wall-clock deadlines.

The release benchmark `release.rs` measures submission, dispatch, service memory, large Queues, restart and output recording on private services in the ordinary profile, prints one JSON object per measurement and asserts no speed. Every loop has a fixed count and a wall-clock cap of 110 seconds; a measurement that reaches the cap says so in its record.

## How to test

Follow `docs/pressure-benchmark.md` for the pressure benchmark and `docs/performance.md` for the release benchmark, which `tools/bench OUTPUT.jsonl` runs. Each benchmark is an explicit operational qualification command and is not run by the ordinary unit-test gate. Formatting and clippy cover it with the other targets.
