# Pacing

## What it does

Keeps the service answering while it works for someone else, and counts what that work costs.

`Launches` is the start budget of one admission pass. The pass of a submission, of a release and of the tick may start one Job, every other pass none; a Job that could start beyond that is labelled `its turn to start` and left to the starter thread, which the pass wakes through `TURNS`. The reason the Job had before is kept and put back when its turn comes. The starter takes the service's lock for one start at a time and, between two starts, gives way to every request that is waiting for the lock, for at most 20 ms. A brief pass stops at the first Job it has to defer, so its cost does not grow with the number of Jobs behind it.

`Group` is a group commit: writers append under their own lock, then ask for a sync up to their position; one of them syncs and the others wait for that sync. The audit journal uses it for intent lines.

`file`, `data`, `directory` and `together` are the service's sync calls. Each counts itself, and `job_record_written` counts a Job record put in place. `job host --json` shows the counters as `activity`, with `starts_pending` true while the starter has Jobs left to start. Each request thread also counts its own calls, and `last_change` in `activity` is what the last changing request spent before its answer; the tests read that, because the totals also move with the tick and with flushes after a reply. The audit intent counts as one call of its request whichever thread performed the shared sync. `together` syncs several files and directories at once from short-lived threads, which lets the file system commit them in one log write.

`Trims` decides when the output budget has to be scanned: at the first end of a Job, when the output recorded by Jobs that ended since the last scan plus that of the running Jobs could reach the budget, and ten seconds after the last scan. The recorded output of the running Jobs is added up at most once a second.

`settle_records` makes a batch of Job records durable without a sync per record under the lock in sequence: every new record is written beside the old one as `job.cancelling`, those files are synced from sixty-four threads, each is renamed into place, and the Job directories are synced the same way. Only what the batch wrote is synced. A crash in between leaves every record whole, old or new, and the cancellation operation that owns the batch applies it again at the next start. `sweep_staged` removes leftover staged files when the service starts. `Retries` spaces the attempts of an operation that failed (0.5 s doubling to 30 s) and feeds `cancellations_failing` and `cancellation_retries` of the health report. An operation that is tried again or found at a start is recovering: its staged files are removed, the directories of its ended members are synced, and `journal::ensure_terminal` writes the terminal event of every member that has none.

`starter_failed` counts a panic of the starter thread, which is caught per pass, and writes the first to the service's output.

## How to test

Run `tools/check` through job. `crates/job/tests/responsiveness.rs` covers a resume that returns before its starts, reads answered during a burst, the start time of a Job late in a burst, the sync and record-write counts of a submission, pause and resume with waiting Jobs, a wait reason after restart, the sync counts of recursive cancellation and removal, a crash at each failpoint of a cancel batch, a batch that fails once and one that always fails, a panic of the starter, one start time while a Job runs and after it ended, and waiting statistics across a restart. The delegated suite holds the test of a pressure hold under a stream of starts.
