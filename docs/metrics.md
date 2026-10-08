# Metrics

The service reports what it holds, what it reserves and how long Jobs wait and run as metrics in the Prometheus text exposition format, version 0.0.4. You get them in two ways:

- `job metrics` prints them to standard output. That suits node_exporter's textfile collector and a quick look.
- The service serves them over HTTP at `/metrics` when the configuration names an address. That is off unless you switch it on.

Both give the same families with the same names; the command asks the running service, so the figures are the service's own.

## From the command line

```sh
job metrics
```

`job metrics` takes no options and no operands. It prints the current figures and ends with status 0.

When the service does not answer, it prints the single line `job_up 0` with its `# HELP` and `# TYPE` lines, writes the reason to standard error and ends with status 125. A file written from that output says the service is down instead of keeping the last figures.

For node_exporter's textfile collector, write to a temporary file in the collector's directory and rename it, so that the collector never reads half a file:

```sh
job metrics > /var/lib/node_exporter/job.prom.$$; mv /var/lib/node_exporter/job.prom.$$ /var/lib/node_exporter/job.prom
```

Run that from a timer or from cron as the user whose service you want to describe; for the system service add `--system` before `metrics`. Do not chain the `mv` on the exit status if you want the down state recorded.

## Over HTTP

Add a `[metrics]` table to the service configuration, `job.conf(5)`:

```toml
[metrics]
listen = "127.0.0.1:9877"
```

`listen` is an IP address and a port: `127.0.0.1:9877`, `[::1]:9877`, or `0.0.0.0:9877` for every IPv4 address of the host. Host names are refused, so that the start does not depend on name resolution. There is no default address and no default port; without `listen` nothing listens. Prometheus keeps a list of default ports for exporters, and every port from 9100 to 9999 on it is taken, so job does not claim one. Port 9877 in these examples is listed there for another exporter; choose a port that is free on your host. Port 0 lets the kernel choose one, which is only useful for tests.

The service binds the address when it starts, before it opens its socket, and names it on standard error:

```
job daemon: metrics at http://127.0.0.1:9877/metrics
```

If the address cannot be bound, for example because another program uses the port, the service does not start and says why. The address is read at start only: `job config reload` refuses a configuration whose `[metrics]` table differs and says that a restart is needed.

A Prometheus scrape configuration for it:

```yaml
scrape_configs:
  - job_name: job
    static_configs:
      - targets: ["127.0.0.1:9877"]
```

### What the listener answers

- `GET /metrics`, with or without a query string: `200 OK` with `Content-Type: text/plain; version=0.0.4; charset=utf-8`.
- Any other method or path: `404 Not Found`.
- A request line that is not `METHOD TARGET HTTP/1.x`, or a request head longer than 8192 bytes: `400 Bad Request`.

Each connection carries one request and is closed after the answer (`Connection: close`). The request body is never read.

### Bounds

| Bound | Value | Why |
| --- | --- | --- |
| Open connections at once | 8 | a few Prometheus servers and a person with curl; a ninth connection is closed at once without an answer |
| Request head | 8192 bytes | a scrape request is a few hundred bytes |
| Time to send the request head | 5 seconds | a client that sends nothing is closed after it without an answer |
| Time to take the answer | 10 seconds per write | a client that stops reading does not hold a thread |
| Reuse of an answer | 1 second | scrapes that arrive within a second of each other get the same text, so the service gathers its figures over HTTP at most once a second |

The listener runs in threads of its own. Gathering the figures takes the service's lock once, while it counts the Jobs and walks the tree of Queues and Groups; reading the cgroup files and writing the answer happen after the lock is released. A slow or stalled HTTP client never holds the lock. What one gathering costs is in [performance](performance.md#metrics).

## Security

The listener has no authentication and no TLS. Everyone who can connect to the address can read every metric. On a loopback address that is every user of the host; on any other address it is everyone who can reach the host on that port, and the service says so on standard error at start:

```
job daemon: the metrics address 0.0.0.0:9877 is not a loopback address; everyone who can reach it can read the metrics
```

The metrics hold no command lines, no output, no environment and no Job IDs. They do hold the paths of Queues and Groups, how many Jobs are in each, how long they waited and ran, the version of job and the memory of the host. If that is more than the network may see, keep the address on loopback and let a local Prometheus or node_exporter carry the figures further, or put a proxy with authentication in front of it. The listener cannot change anything: it answers one read-only path.

`job metrics` goes through the service's socket like every other command and is admitted the same way: the service user, and members of the socket group if one is configured.

## The metrics

The type of each family is given in its `# TYPE` line. "Since the service started" means a counter that starts at 0 when the service starts; Prometheus treats the drop to 0 after a restart as a reset. `job_service_start_time_seconds` tells you when that happened.

### Service

| Name | Type | Labels | Meaning |
| --- | --- | --- | --- |
| `job_up` | gauge | | 1 when the service answered; 0 only in the output of `job metrics` when it did not |
| `job_service_info` | gauge | `version`, `protocol`, `state_schema`, `backend`, `enforcement` | always 1; `backend` is `cgroup` or `watch`, `enforcement` is `enforced` or `monitoring_only` |
| `job_service_start_time_seconds` | gauge | | Unix time at which the service started |
| `job_journal_writable` | gauge | `journal`: `audit`, `events` | 1 when the journal can be written, 0 when its last write failed |
| `job_starter_failures_total` | counter | | passes of the starter that failed, since the service started |
| `job_cancellations_failing` | gauge | | cancellations the service is still retrying |
| `job_cancellation_retries_total` | counter | | retries of cancellations, since the service started |
| `job_unreadable_records` | gauge | | Job records the service cannot read |
| `job_state_free_bytes` | gauge | | free space in the file system of the state directory; absent when it cannot be measured |

These are the figures of the health section of `job host`.

### Jobs

| Name | Type | Labels | Meaning |
| --- | --- | --- | --- |
| `job_jobs` | gauge | `state` | Jobs the service holds a record of, by state: `held`, `queued`, `starting`, `running`, `suspended`, `stopping`, `succeeded`, `failed`, `cancelled`, `lost` |

Ended Jobs are counted as long as their records exist; `job remove` lowers those counts. The counts agree with `job list --all`.

### Capacity, reservations and use

| Name | Type | Meaning |
| --- | --- | --- |
| `job_pool_cores`, `job_pool_memory_bytes`, `job_pool_processes` | gauge | what the service admits Jobs against, as `pool` in `job host --json` |
| `job_reserved_cores`, `job_reserved_memory_bytes`, `job_reserved_processes` | gauge | what Jobs that are starting, running, suspended or stopping have reserved; 0 for Jobs that requested nothing |
| `job_host_memory_bytes`, `job_host_memory_available_bytes` | gauge | `MemTotal` and `MemAvailable` of `/proc/meminfo` |
| `job_output_recorded_bytes` | gauge | recorded output that counts against the output budget; absent until the service has measured it |
| `job_output_budget_bytes` | gauge | the output budget, `budget_bytes` under `[output]` |
| `job_use_memory_bytes` | gauge | `memory.current` of the cgroup that holds all Jobs |
| `job_use_processes` | gauge | `pids.current` of that cgroup |
| `job_use_cpu_seconds_total` | counter | `usage_usec` of its `cpu.stat`: CPU time of all Jobs, including those that ended, since the service created the cgroup |
| `job_use_cpu_throttled_seconds_total` | counter | `throttled_usec` of its `cpu.stat` |
| `job_use_oom_kills_total` | counter | `oom_kill` of its `memory.events` |

The `job_use_` families exist only when the service manages a delegated cgroup. In monitoring mode there is no cgroup that holds all Jobs, and the families are left out rather than shown as 0. Cores are given as a decimal number of cores; the service keeps them in thousandths.

### Queues and Groups

Every Queue and Group of the tree has these series, labelled with `kind` (`queue` or `group`) and `path`, the path as `job group list` shows it; the root Group is `/`.

| Name | Type | Labels | Meaning |
| --- | --- | --- | --- |
| `job_object_jobs` | gauge | `kind`, `path`, `state` | Jobs that have not ended in the Queue, or in the whole subtree of the Group, by state: `held`, `queued`, `starting`, `running`, `suspended`, `stopping` |
| `job_object_oldest_queued_age_seconds` | gauge | `kind`, `path` | how long the oldest queued Job of it has waited; absent when none is queued |
| `job_object_running_limit` | gauge | `kind`, `path` | the limit on Jobs running at once that this Queue or Group sets itself (`--max-running`); absent where it sets none |
| `job_object_cores_limit`, `job_object_reserved_cores` | gauge | `kind`, `path` | the cores this Queue or Group sets as what its Jobs hold together at most, and what its Jobs have reserved; absent where it sets none |
| `job_object_memory_limit_bytes`, `job_object_reserved_memory_bytes` | gauge | `kind`, `path` | the same for memory |

These are the figures `job queue show` and the client interface's `tree` give. A limit is reported on the object that sets it, not repeated on the objects below it. Running Jobs against a limit:

```
sum without (state) (job_object_jobs{state=~"starting|running|suspended|stopping"})
  / on (kind, path) job_object_running_limit
```

### Waiting and run time

| Name | Type | Labels | Meaning |
| --- | --- | --- | --- |
| `job_wait_duration_seconds` | histogram | `queue` | how long a Job waited from becoming runnable (submitted, released or retried) until it was admitted; one observation per attempt that starts, since the service started |
| `job_run_duration_seconds` | histogram | `queue` | wall time from the start of the command to the end of the attempt, suspended time included; one observation per attempt whose command started and that reached a final state, since the service started |
| `job_ended_total` | counter | `queue`, `state` | attempts that reached a final state since the service started: `succeeded`, `failed`, `cancelled`, `lost` |

The waiting time is the `wait_ms` of the `starting` record in `job events`. An attempt that is cancelled before it starts counts in `job_ended_total` but has no run time.

The buckets of both histograms, in seconds, are 0.1, 0.5, 1, 5, 10, 30, 60, 300, 600, 1800, 3600, 10800, 43200 and 86400, and `+Inf`. They span what job runs: a test that ends in a fraction of a second to a build or a render of a day. Use `histogram_quantile` for percentiles, for example the 90th percentile of the waiting time of each Queue over the last hour:

```
histogram_quantile(0.9, sum by (queue, le) (rate(job_wait_duration_seconds_bucket[1h])))
```

They are histograms and not summaries because a histogram can be added up across Queues and across hosts, Prometheus computes the quantiles, and the service only counts.

These three families count what happened while the service ran. Transitions that happened before it started, including those found when it recovered its state, are not counted again. They are kept in memory and start from 0 after a restart.

## Cardinality

No label carries a Job ID, a command, a user or a session. The number of series is bounded by the tree:

- `job_jobs`: 10 series.
- per Queue or Group: 6 `job_object_jobs` series and at most 6 others;
- per Queue that Jobs waited or ran in since the service started: 17 series of each histogram (15 buckets, the sum and the count) and at most 4 of `job_ended_total`.

The histograms and `job_ended_total` keep a Queue path after the Queue is removed or renamed, until the service restarts. They keep at most 1024 different paths; observations for any further path are counted under `queue="(other)"`, which no Queue can be named, since names do not allow parentheses.

## What is not there

- No figures per Job. Use `job show`, `job list --format json` or the [client interface](client-interface.md) for those.
- No figures of the service process itself (its CPU time, memory, open files).
- No OpenMetrics format; the answer is always the text format 0.0.4, which Prometheus accepts whatever it asks for.
- No TLS and no authentication on the listener.
