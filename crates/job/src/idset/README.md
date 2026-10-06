# Sets of Job IDs

## What it does

Holds the one grammar for a set of Job IDs (an ID, an inclusive range, a comma list, several operands; at most 10000 IDs as written), the request and the per-ID report that `cancel`, `remove`, `release`, `retry`, `suspend`, `continue`, `signal`, `reprioritize`, `move` and `wait` exchange with the service for a set, and the client that prints the report as lines, as one summary line when more than 20 IDs behaved the same, or as JSON, and turns it into the exit status. `service.rs` is compiled as a child of the daemon module: it resolves the set against the existing Jobs under the service lock, hands the Jobs that can be cancelled to one cancellation operation and those that can be removed to one removal transaction, runs the single-Job functions of the other commands in ID order, and waits for a set. `crates/job/src/commands/sets.rs` decides from the command table when an invocation is a set and reads its options. One plain ID never reaches this module.

## How to test

Run `cargo test --test cli_contract idset_` and `cargo test --test watch_backend idset_` through job. The first covers the grammar, usage errors without a service, help and completion; the second goes through a test service for cancel, remove, release, retry, reprioritize, move, signal, wait, `list --id`, dry runs, unchanged single-ID answers, the audit record and a crash in the middle of a set cancel. Suspending and continuing a set needs a delegated cgroup: `cargo test --test freezer -- --ignored --test-threads=1 idset_`. No unit tests.
