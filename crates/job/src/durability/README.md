# Durability

## What it does

Closes the crash windows around launch and result, and adds idempotent submission, request protocol negotiation, peer identity and the audit journal. The store stays one directory per Job with files written by temporary file, fsync, rename and parent fsync, plus one JSON journal per operation.

`failpoint.rs` reads `JOB_FAILPOINT=name[,name]` once. `failpoint(name)` ends the process with SIGKILL at that point; `fails(name)` returns an injected write error instead. `name@N` arms only the N-th time the point is reached. With the variable unset both do nothing, and the daemon refuses to start on a name it does not know. The names:

| Name | Process | Effect |
|---|---|---|
| `submit-after-index` | daemon | ends after the idempotency index is written, before the Job is published |
| `submit-after-publish` | daemon | ends after the Job is published, before the reply |
| `launch-after-starting` | daemon | ends after the `Starting` record, before the cgroup and the supervisor |
| `launch-after-spawn` | daemon | ends after the supervisor was spawned, before its identity is saved |
| `launch-identity-save` | daemon | the identity save fails |
| `launch-after-identity` | daemon | ends after the identity save, before the launch gate opens |
| `finalize-save` | daemon | the terminal record save fails |
| `shim-after-exit-record` | supervisor | ends after `exit.json`, before cleanup |
| `shim-before-result` | supervisor | ends after cleanup, before `result.json` |
| `shim-result-save` | supervisor | the result save fails |
| `audit-partial-write` | daemon | a journal write stops after half the line and fails |
| `audit-before-end` | daemon | ends after a described request was handled, before its result line |
| `liveness-unknown` | daemon | the supervisor liveness check answers "unknown" |
| `launch-confirm-delay` | supervisor | waits 1.5 s between the gate byte and `started.json` |
| `cancel-batch-after-stage` | daemon | ends after every record of a cancel batch was staged and synced, before the renames |
| `cancel-batch-sync` | daemon | the sync of the staged records of a cancel batch fails |
| `cancel-batch-rename` | daemon | one rename of a cancel batch fails |
| `cancel-batch-mid-rename` | daemon | ends before one rename of a cancel batch |
| `starter-panic` | daemon | the starter thread panics in a pass, holding the service lock |
| `cancel-batch-after-publish` | daemon | ends after the records of a cancel batch were renamed and synced, before the events and the completed operation |

`pause(name)` is the third kind: it sleeps instead of failing, and exists for `launch-confirm-delay`.

`launch.rs` holds the launch gate: the daemon passes the supervisor the read end of a pipe on its own descriptor (`--gate FD`) and writes one byte only after the record with the supervisor's pid, start ticks and boot identity is on disk. A supervisor that reads end of file exits without running the command and without a result. On the byte it writes `started.json` and only then runs the command. A gated launch with no recorded supervisor, or with a recorded supervisor of this boot that ended without `started.json`, never executed and is queued again, three times at most before it ends as a start error.

`exit.rs` writes `exit.json` (`attempt`, `exit_code`, `signal`, `at_ms`) as soon as the command has been waited for, rejects a result or exit record of another attempt and keeps it under that attempt's archive as `late-result.json` or `late-exit.json`, retries a failed result or terminal record write once and logs it. `started.json` and `exit.json` are renamed into place without fsync: they only have to survive the death of a process, and after a reboot a missing marker is never read as "never started".

`idempotency.rs` keeps `idempotency/<sha256(key)>.json` with `key`, `id` and `digest`. The digest (version 2) is the SHA-256 of a JSON object with sorted keys: `digest_version`, `argv`, `cwd`, `queue`, `held`, `pty`, `environment` and `options`, where `options` is the declarations without the terminal size, without proxy user and password, and without every null, false, empty list and empty object. The session label and environment values are not part of it. The Job record's digest and `spec_digest_version` are what a replay is compared with; a record without a version is compared with the earlier digest over the whole declarations. `edited` computes the key of an edited Job. `<hash>.tmp<pid>` files are ignored by the reader, listed by `leftovers`, removed by `sweep` at service start, and `forget` skips whatever is not an entry. `peer.rs` reads SO_PEERCRED and carries uid and pid for the handling thread. `audit.rs` appends a `begin` line before and an `end` line after each mutating request to `audit/current.jsonl`, both fsynced and with one `seq`, pairs them when reading, bounds every client string and the line, records `attach`, refused peers and unreadable or unknown requests (the last three rate-limited), replaces argv by program, count and SHA-256, rotates, and ends a fragment left by a failed write with a newline. `negotiation.rs` wraps every client request in its protocol range; `receive` reads the envelope untyped, decides on the range and only then reads the request. `exchange` in `mod.rs` is the one path from a request line to its response: negotiation, intent, handler, result. `cli.rs` is `job audit` and the help text.

## How to test

Run `tools/check` through job. `crates/job/tests/durability.rs` holds the `durable_*` tests: each crash window with its failpoint, stale results, the exit record, the audit journal with rotation and a torn line, idempotency keys and raw-socket negotiation. The `review_*` tests there cover a stray temporary file in the index, negotiation with an invented request, replay from a resized terminal, the key after an edit, the journal's intent after a crash, a failed journal write, bounded strings, attach and unreadable requests, argv and embedded credentials, an unknown liveness answer, and the output budget beside an unreadable record. `crates/job/tests/state_migration.rs` covers backup and restore of stream recordings, resource-update receipts, links, audit and idempotency files and leftover submission stages. The cgroup variant of launch recovery is in the delegated suite: `cargo test --test freezer -- --ignored --test-threads=1 recovery_of_`.
