# Command line and lifecycle completions

## What it does

Holds what the audit of the redesign plan's sections 2, 3, 4, 5 and 7 found missing on the command line. `listing` lists Jobs in any state as a table, JSON or tab-separated values and lists Queues and Groups the same way. `moving` moves a held or queued Job to another Queue. `settings` explains the settings of a Queue or Group. `labels` validates and stores labels. `interrupt` and `run` define what a signal, a hangup or a closed output does to `job run`. `stdin` carries the client's standard input to a Job. `capability` reports that there is no file system quota. `usage` holds small parser helpers for `cancel`, `release` and `submit`. `service.rs` is compiled as a child of the daemon module and answers the one request variant these commands add. A text or TSV listing asks for brief rows (`Call::Rows`, `Query::brief`): the service leaves out the copies of the specification, the snapshots, the resource origins and the history key, which no row prints; JSON listings carry the whole record.

## How to test

Run `cargo test --test cli_contract cli2_` through job. The cases start the built binary and a test daemon and send real signals to a real client. The cgroup readback of `pids.max` is `cargo test --test freezer cli2_ -- --ignored`, which needs a writable delegated cgroup and therefore also runs through job. No unit tests.
