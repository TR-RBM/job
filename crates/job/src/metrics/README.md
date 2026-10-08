# Metrics

## What it does

Writes the service's figures in the Prometheus text exposition format, version 0.0.4, for `job metrics` and for an optional HTTP listener in the service. The families, their labels and their meaning are described in `docs/metrics.md`.

- `mod.rs`: the `[metrics]` table of the configuration (`listen`, an IP address and a port, checked when the configuration is read), binding the listener when the service starts, the answer to the request `Metrics` of the service's protocol, and the command `job metrics`, which prints `job_up 0` when the service does not answer.
- `gather.rs`: the figures. It takes the service's lock once, through `client_service::seen`, for the counts of Jobs, the reservations, the pool and the tree of Queues and Groups as the client interface's `tree` draws it; health, `/proc/meminfo`, the free space and the counters of the cgroup that holds all Jobs are read after the lock is released.
- `observed.rs`: the waiting-time and run-time histograms and the count of ended attempts, per Queue path, kept in memory since the service started. The event journal calls `transition` for every state change it records, so a change is counted once and changes found at recovery are not counted. At most 1024 Queue paths are kept; further ones are counted as `(other)`.
- `http.rs`: the listener. One accept thread, at most 8 connections at once, each in a thread of its own; a request head of at most 8192 bytes read within 5 seconds; `GET /metrics` answered with the text, any other method or path with 404, an unreadable request line with 400; one request per connection. An answer is reused for one second.
- `text.rs`: the format: `# HELP` and `# TYPE` lines, samples with escaped label values.
- `messages.rs`: the sentences of this module in English and German. Metric names and HELP texts are part of the format and stay English.

## How to test

Run `cargo test --test metrics` through job. The cases start a private service: `job metrics` compared with `job list --all` and `job host --json`, with a Queue that has a limit, a held, a queued, a running and an ended Job, and again after cancellation; the HTTP listener on a port the kernel chose, with `GET /metrics`, a query string, other paths and methods, a malformed and an oversized request, a client that sends nothing, the limit of connections, and the address in `job config show`; no listening socket of the service without `[metrics]`; `listen` values accepted and refused by `job config check`; a start refused when the address is in use; `job config reload` refused for a changed listener; `job metrics` without a service, with an operand and with `--help`. Every exposition is parsed strictly: HELP and TYPE once per family before its samples, valid names and escaped label values, counters ending in `_total`, cumulative histogram buckets with `+Inf` equal to the count, no label named `job` or `id`. `cargo test --test freezer metrics_report -- --ignored` checks the `job_use_` families against the cgroup inside a delegated cgroup. No unit tests.
