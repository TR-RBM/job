# Performance

This page says what was measured for this release, how to repeat it, and which targets the release sets from the measurements. The numbers come from one run on one machine. They are not guarantees for other machines, other file systems or larger sizes, and nothing here is a claim about scale.

## What limits the service

Read this first; it explains every number below. Six limits were measured first and then reduced by a change to how the service makes its records durable; this section says what holds now. The table under Measured values has the numbers before and after that change.

- **A submission costs six synchronous writes before its answer.** One `fdatasync` for the audit intent, four `fsync` issued together for the three files of the Job and their directory, one `fsync` that makes the Job exist. On the measured disk one such call takes 11 to 14 ms and a submission about 60 ms. Clients that submit at the same time share the audit sync; the rest is done one submission at a time, so four clients got 19 submissions per second where one got 17.
- **A launch costs three synchronous writes of the service and an end two, one Job at a time, and the supervisor of each Job makes seven of its own.** That bounds the rate at which Jobs start and end at about eight per second on this disk, with or without a concurrency limit.
- **The service answers between two starts.** A request that lets many Jobs start returns after recording its own change. While 500 Jobs were being started, another client's `job queue show` took 80 ms at the median and 0.33 s at most. A read waits for the start or the end that is in progress.
- **Pausing and resuming a Queue write the Queue, not its Jobs.** 0.08 s with 300 Jobs waiting.
- **Recursive cancellation syncs each record and each Job directory it changes, 64 at a time, and nothing else on the file system; recursive removal syncs once per operation.** Both hold the service for their whole run: 1.3 s and 0.7 s for 1 000 held Jobs, 11 s and 5.1 s for 10 000.
- **With priorities, aging or fair share switched on, every Job costs about 25 synchronous writes of the service**, most of them the scheduling ledger, which is written at every request, pass and tick. Two Jobs per second is what such a Queue did here.
- **Recorded output passes through the supervisor at about 170 MiB per second**, 210 with a small retention quota. The same writer into a plain pipe needs 0.2 seconds for 256 MiB; as a Job it needs 1.2 to 1.5 seconds. Before the recorder was changed it needed 16 to 19 seconds, 13 to 16 MiB per second, because every MiB cost three syncs. Output now reaches the disk with the kernel's writeback while the Job runs and is synced when its streams end; see [output recording](output-recording.md).
- **`started_ms` is the start of the command.** `admitted_ms` is the time of the scheduling pass that admitted the Job. In a burst they differ by the time the Job waited for its turn to be started.

## What is measured

`crates/job/benches/release.rs` starts private services in the `ordinary` profile, each with its own state under the temporary directory, in monitoring mode (the watch backend) unless `--backend cgroup` is given. Clients are real `job` processes with a five-variable environment. Times called client times are from the start of the client process until it has exited; record times are the service's own millisecond timestamps. Quantiles are nearest rank.

| Group | Workload | Reported |
|---|---|---|
| `submit` | 500 `job submit -q QUEUE -- true` into a paused Queue, one after another; 500 more from four clients at once | client time per submission: median, p95, max; submissions per second |
| `dispatch` | `job queue resume` of those 500 Jobs with no concurrency limit, and of the other 500 with `--max-running 4` | time from the resume to each command's start, interval between starts, completions per second, duration of the resume call, and how long a second client's `job queue show` took meanwhile |
| `idle` | 100 times `job submit -- true` then `job wait ID` on an idle service | client time for both calls; record time from submission to command start and to finish |
| `large` | 1 000 and 10 000 held Jobs in one Queue | resident memory of the service idle, with the Jobs, after restart and after removal; time of `job list`, `job list --json`, `job queue`, `job queue show`, `job explain`, `job status`; restart time until the socket answers; time of `job queue cancel --recursive` and `job queue remove --recursive`; bytes of state |
| `output` | one Job writing 256 MiB of 64-byte lines to stdout, with the default retention and with `--output-head 1M --output-tail 1M` | run time, bytes on disk in the Job's directory, time and size of `job logs ID --raw`, time of `job log ID tail 50` |

Bounds: every loop has a fixed count; each measurement stops at 110 seconds and then says so in its record (`reached_target_within_cap`, `completed_within_cap`, `returned_within_cap`); the output Job writes a fixed 256 MiB twice; the largest state is about 160 MiB; in all well under 1 GiB is written. No Job runs longer than its payload, and a private service is ended when its group ends.

Ten thousand Jobs are not created through the command line inside the cap; at the first release a submission took 0.2 seconds, and the harness was not changed. The `large` group therefore creates 100 held Jobs through `job create`, stops the service, copies one of those records to the remaining IDs, lets `job state validate` accept the store and starts the service again. What is measured afterwards (memory, listing, restart, cancellation, removal) is the service's own work on those records.

## How to repeat it

```sh
uptime
job run --cores 8 --mem 4G --time 30m --budget none -- tools/bench /tmp/job-release-$(date +%s).jsonl
```

Run it through an installed job service so that it holds a reservation. The output file must not exist. The first line of the file describes the machine: CPU model and count, memory, file system and mount options of the temporary directory, kernel, load average, the cgroup limits it ran under and the source commit. Pick a quiet moment and keep the load average with the result. A full run takes about six minutes.

```sh
jq -c 'select(.record == "measurement") | {name, client_ms, completions_per_second}' /tmp/job-release-*.jsonl
```

`--only submit|dispatch|idle|large|output` runs one group. `--backend cgroup`, inside a delegated cgroup, runs the services with cgroup enforcement.

## Measured values

One run, 2026-10-06, on a virtual machine with 32 CPUs (AMD EPYC-Milan), 31 GiB memory, btrfs with zstd compression on a virtual disk, Linux 7.2.6, load average 3.1 at the start, with other work going on on the same disk. Watch backend, release build. The same harness on the commit before the durability change, the same night at a load average of 3.8, is the column "before".

| Measurement | Before | Now |
|---|---|---|
| Submit, one client, 500 | median 174 ms, p95 187 ms, max 299 ms; 6.0 per second | median 61 ms, p95 65 ms, max 129 ms; 17 per second |
| Submit, four clients, 500 | median 891 ms, p95 1 278 ms; 4.0 per second | median 203 ms, p95 285 ms, max 377 ms; 19 per second |
| Dispatch, 500 `true`, no limit: completions | 6.8 per second | 8.1 per second |
| … commands started every | 143 ms (median), p95 161 ms | 119 ms (median), p95 175 ms, max 289 ms |
| … `job queue resume` itself | 73.4 s | 0.17 s |
| … another client's `job queue show` meanwhile | up to 73.3 s | median 80 ms, p95 207 ms, max 328 ms |
| Dispatch, 500 `true`, `--max-running 4`: completions | 3.3 per second, 141 s for all | 8.2 per second, 61 s for all |
| … `job queue resume` itself | 20.7 s | 0.06 s |
| … another client's `job queue show` meanwhile | median 1.0 s, max 20.6 s | median 112 ms, p95 202 ms, max 282 ms |
| Idle service, `job submit -- true` then `job wait` | client median 344 ms, p95 431 ms | client median 221 ms, p95 234 ms, max 399 ms |
| … submission to command start | median 171 ms, p95 211 ms | median 79 ms, p95 87 ms |
| Service memory, idle | 8.2 MiB resident | 8.5 MiB |
| … with 1 000 held Jobs | 24 MiB | 24 MiB (about 15 KiB per Job) |
| … with 10 000 held Jobs | 160 MiB | 160 MiB |
| … after cancelling and removing 1 000 | 34 MiB | 47 MiB |
| … after cancelling and removing 10 000 | not reached | 177 MiB |
| `job list` | 44 ms at 1 000; 421 ms at 10 000 | 48 ms; 416 ms |
| `job queue` | 43 ms; 429 ms | 46 ms; 410 ms |
| `job queue show`, `job explain`, `job status` | 1.5 to 3.0 ms | 1.6 to 2.5 ms |
| Restart until the socket answers | 236 ms with 1 000 records; 2 480 ms with 10 000 | 273 ms; 2 439 ms |
| `job queue cancel --recursive`, 1 000 held | 40.0 s | 1.3 s (second run of the group, with 64 sync threads) |
| `job queue remove --recursive`, 1 000 cancelled | 17.0 s | 0.63 s |
| `job queue cancel --recursive`, 10 000 held | not finished after 110 s | 11.2 s (the same run) |
| `job queue remove --recursive`, 10 000 cancelled | not run | 4.8 s |
| `job queue pause` and `resume`, 300 Jobs waiting (by hand, debug build) | 7.5 s and 7.7 s | 0.08 s and 0.08 s |
| Output, 256 MiB, default retention | 19.3 s (13 MiB per second) | 1.5 s (168 MiB per second), measured with the recorder change; 130 MB on disk |
| Output, 256 MiB, head 1M and tail 1M | 16.4 s (16 MiB per second) | 1.2 s (210 MiB per second), measured with the recorder change; 5.2 to 5.6 MB on disk |
| The same writer into a pipe, without job | 0.19 s | 0.19 s |

Listing, memory, state size and restart did not change and grow in proportion to the number of records. The output rows are from the measurement of the change to the recorder; they were not measured again together with the durability change.

With the default retention the Job's directory holds about 130 MB for a 64 MiB cap, because the merged log and the stream journal each keep their own copy.

## Targets for this release

On this class of machine: a virtual machine with a virtual disk, btrfs, where one `fsync` takes 11 to 14 ms. Each target is the measured value rounded up generously, by a factor of two to four, because the machine is shared and one run was taken. A run that misses one on a comparable, quiet machine is a regression to explain before release. On a faster disk the numbers should be better; they were not measured there.

| What | Target |
|---|---|
| One submission, service otherwise idle | p95 under 0.25 s, every one under 1 s |
| Submissions accepted | at least 8 per second in total, however many clients submit |
| `true` Jobs started and completed from a waiting Queue | at least 4 per second, with or without `--max-running 4` |
| One `true` Job on an idle service, submit and wait | p95 under 0.8 s |
| `job queue resume` and `job queue pause`, however many Jobs wait or become startable | under 1 s with 500 waiting |
| A read (`job show`, `list`, `status`, `host`, `explain`, `queue show`) of one object while Jobs are being started | p95 under 1 s, every one under 2 s |
| `job queue cancel --recursive` and `job queue remove --recursive` | each under 5 s for 1 000 Jobs, under 40 s for 10 000 |
| Service memory | under 16 MiB idle; under 32 KiB per waiting Job on top, so under 48 MiB at 1 000 and under 320 MiB at 10 000; under 64 MiB after cancelling and removing 1 000 |
| `job list` and `job queue` | under 0.15 s at 1 000 waiting Jobs, under 1 s at 10 000 |
| `job queue show`, `job explain`, `job status` of one object | under 20 ms at 10 000 waiting Jobs, while the service is not launching or cancelling |
| Restart until the service answers | under 1 s with 1 000 records, under 5 s with 10 000 |
| Output recording | at least 100 MiB per second through one Job; at most 160 MB on disk per Job with the default retention; `job logs --raw` of a full default recording under 0.5 s |

No target is set for the time the service takes to answer while it cancels or removes thousands of Jobs for another client; it does not answer until the operation is done, which is the seconds in the table. No target is set for Queues with priorities, aging or fair share; two Jobs per second were measured there and that is not a number to promise. The statement for this release: the service takes more than ten submissions per second, was run with Queues of 500 waiting Jobs and stores of 10 000 held ones, and starts about eight short Jobs per second on this disk.

## What was not measured

- The cgroup backend for anything but the idle round trip, another file system, a physical disk, a machine under memory pressure.
- More than 10 000 records, more than one Queue of waiting Jobs, running Jobs in the thousands.
- Jobs with large environments: clients here pass five variables. A record with a full desktop environment is several times larger.
- Output on stderr, PTY recordings, many Jobs writing at once, and reading logs while they are written.
- Variation between runs: each group ran once before and once after the durability change. A run of the dispatch group at a load average of 10 gave 6.4 and 6.1 completions per second and reads of at most 0.58 s.
- A burst of thousands of startable Jobs, a burst under priorities or fair share, and the time a read waits during a recursive cancellation.
