# Process identity

## What it does

Owns Linux pidfds, sends signals through them and polls for exit. Opening a handle can require an expected process start time. The daemon captures a supervisor handle before reaping can release its PID and retains it until the attempt completes. Recovery requires both the recorded boot identity and start time. Resource exhaustion while reopening a handle aborts recovery instead of labelling a still-running supervisor lost.

Daemon startup requires pidfd_open and pidfd_send_signal, including permission to use them. `job host --json` reports `pidfd: true`. There is no fallback to sending signals through reusable numeric PIDs. Procfs still supplies process-tree discovery and accounting, and cgroup.kill remains the whole-cgroup termination mechanism. Process discovery in watch mode remains a snapshot and cannot contain descendants that escape ancestry.

`job signal [-s SIGNAL] ID` signals the current workload processes, excluding the execution supervisor. It defaults to TERM, accepts common signal names with an optional SIG prefix or Linux signal numbers, and is quiet on success. The wire response reports the number of deliveries. A process that exits during delivery is skipped. Other errors are returned with notice that earlier deliveries may already have succeeded. A signal is not a cancellation request and does not freeze a whole cgroup; lifecycle state follows the observed result.

## How to test

Run `cargo test process::` and `cargo test --test watch_backend` through job. Tests cover exit readiness, stale handles after reaping and process churn, mismatched start identity, another boot, and signalling a workload after daemon restart. Terminal restart integration checks that reconnect survives supervisor adoption. No test forces host PID reuse or changes kernel PID allocation.
