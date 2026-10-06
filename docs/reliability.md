# Reliability

This page says what job guarantees when the service, a supervisor or the client connection fails, and where the guarantees end.

## What survives a crash of the service

A submission the service answered is on disk before the answer is sent. After a crash and restart the Job is there with its ID, its command and its environment, and runs if it had not run.

Six sync calls stand between a submission and its answer, and each is needed:

| Call | What it makes durable |
|---|---|
| `fdatasync` of `audit/current.jsonl` | the intent line, before anything changes |
| `fsync` of `job.json`, `env.json`, `submitted-env.json` in the stage directory | the record, the environment that runs, the environment as submitted |
| `fsync` of the stage directory | the three names |
| `fsync` of `jobs/`, after the stage directory was renamed to `jobs/ID` | the Job itself; from here on it survives a crash |

The four in the middle are issued at once. The ID counter `next-id` is not synced: after a loss of power the next ID is the largest of the counter, the highest Job directory and the highest removed ID, so no ID that was answered or recorded is used again. An ID that was reserved for a submission that was never recorded can be.

A submission whose answer never arrived may or may not be recorded. Repeat it with the same idempotency key (below) and you get the recorded Job or a new one, never two.

Running Jobs keep running. Each has its own supervisor process, which the service finds again by pid, start time and boot identity.

## No command runs unrecorded

The service writes the Job record as `Starting`, starts a supervisor, writes the supervisor's identity into the Job record, and only then tells the supervisor to go. The `Starting` record is written with an `fsync` of the file; the identity record with an `fsync` of the file and one of the Job's directory, which covers both, before the supervisor is told to go. If the service dies before that, the supervisor exits without starting the command. After a restart such a Job is queued again, keeps its place and its waiting credit, and runs once.

## Start time

`admitted_ms` in the record is when the service admitted the Job. `started_ms` is when its command started: the time the supervisor wrote into `started.json` just before it ran the command. It has one value for the life of the record. The service reads it from that file, at the latest a quarter of a second after the start, and until then the field is absent; a Job whose command never started never has one. Run time, active time, suspended time and the limit of `--time` count from `started_ms`. The waiting time in the event journal and in `job queue show` ends at `admitted_ms`, and so does the time a Job is counted as occupying its reservation. In a burst of starts the two differ by the time the Job waited for its turn to be started; `job status` shows such a Job as queued, waiting for `its turn to start`.

## Exit status

The supervisor writes the exit status the moment the command ends, before it cleans up. If the supervisor is killed during cleanup, or cannot write the full result, the Job still ends with its real exit status and a note that cleanup did not complete: leftover processes, usage and the end of the output were not recorded.

## What lost means

`lost` means one thing: the supervisor confirmed the launch, then ended, and neither a result nor an exit status was found. The command may have run partly or completely. A reboot of the host while a Job runs ends that way. job does not rerun lost work by itself; `job retry ID --allow-lost` is your decision.

A Job that never started is not lost. It is queued again, or ends with a start error that says it never started.

## What job does not promise

No exactly-once effects. A command can send mail, write a file or call a service before a failure is seen, and a retry does it again. Make the command safe to repeat, or check before retrying.

No protection against losing power between a rename and the directory's fsync on a file system that does not order metadata. job fsyncs the file and the directory; what the disk does after that is outside it.

The estimator's history file is appended without fsync. It holds samples for estimates, not outcomes; a lost last line costs one sample.

The lifecycle event of an ended Job is written when the Job's record has been synced and is itself synced right after the request that ended the Job has been answered, or after the tick that found it ended. A loss of power in between leaves the record, which is what counts, and no event.

The reason a Job waits (`waited_for`) is kept in memory and written with the record only when the record is written for another cause. The service computes it again when it starts.

## Many Jobs at once

Recursive cancellation of eight or more waiting Jobs writes every new record beside the old one as `job.cancelling`, syncs each of those files, renames each into place and syncs each Job directory, sixty-four at a time; nothing else on the file system is synced. The cancellation operation is recorded before and marked complete after, when every record and every `cancelled` event is on disk. A crash in between leaves each record whole, cancelled or not; when the service starts it removes leftover `job.cancelling` files, finishes the operation and writes the `cancelled` event of every member that has none. If a batch cannot be written the Jobs stay as they were, the operation's answer and `persistence_error` or its results say why, and the service tries again after 0.5 s, 1 s, 2 s and so on up to 30 s, or at once when the same cancellation is asked for again; `job host` counts such operations as `cancellations_failing`. Recursive removal syncs `jobs/` once after all selected Job directories were moved away, under its removal journal as before.

## Idempotency keys

    job submit --idempotency-key nightly-2026-10-05 -- make release
    job run --idempotency-key deploy:42 -- ./deploy
    job create --idempotency-key batch.7 -- ./prepare

The key is 1 to 128 characters of `A-Z a-z 0-9 . _ : -`. The same key with the same command, directory, Queue and options returns the Job already recorded; `submit` prints its ID and says on standard error that nothing was added, `run` waits for it, and `--json` shows `"replayed": true`. The same key with a different specification is refused. Removing the Job record with `job remove ID` frees the key.

What is compared is a digest of what you asked for, version 2, stored in the record as `spec_digest` with `spec_digest_version`:

- the command as the service receives it, argument by argument (`--shell` is part of it, because the client turns it into the command);
- the working directory and the Queue;
- whether the Job was asked for held (`create`) or to run (`submit`, `run`);
- whether `--pty` was given, not the size of the terminal;
- every option you gave, by its name in the record and its value. An option you did not give is left out, so a later version of job that knows more options computes the same digest for the same request.

Not compared: the terminal size, the session label, the values of the environment, and the user and password inside a proxy address. The first submission's environment is the one that runs.

`job edit ID` on a held or queued Job that has a key computes the digest again from the edited specification. The key keeps naming that Job: a repeat with the edited specification returns it, a repeat with the original one is refused. After an edit a held Job matches `create` and a queued one matches `submit` and `run`. A retry keeps the key and the digest the Job had.

A record written before digest version 2 is compared the old way, over the whole set of declarations including the terminal size, until the Job is edited or removed.

A file left in `idempotency/` by an interrupted write, named `<hash>.tmp<pid>`, is ignored by the service, removed when the service starts, and listed by `job state validate` as `leftover_temporary_files`. It never stops a removal or a submission.

## The audit journal

Every request that changes something leaves two lines in `audit/current.jsonl` below the state directory: an intent line (`"phase": "begin"`) fsynced before anything changes, and a result line (`"phase": "end"`, same `seq`) written before the client is answered and synced right after the answer has been sent. Requests that arrive together share one sync of their intent lines. `job audit` shows the pair as one record. An intent without a result, which is what a crash between the change and its result line leaves, or a loss of power before the result line was synced, is shown with the result `outcome unknown`: the change may or may not have happened, and the Job or object record says which.

If the intent line cannot be written, the request is refused and nothing changes: a service that cannot record does not change state. Reads, the supervisors' own messages and `attach` are never held back by the journal.

    job audit
    job audit --target 42
    job audit --action cancel --since 1791230000000 --json

A record has `seq`, `at_ms`, `action`, `target`, `peer_uid`, `peer_pid`, `session`, `params`, `result` and `operation`, and `truncated` when something was cut. `peer_uid` and `peer_pid` come from the socket, not from the request. `--json` prints one object per line with `schema_version`.

What is kept out of the journal:

- The arguments of a command. They often carry secrets (a token after `--header`, a password in a connection string), so `params.spec.argv` holds the program name, the number of arguments and the SHA-256 of the whole argument list as a JSON array, which is enough to tell whether two submissions ran the same command.
- The values of the environment; only the names are kept.
- User and password in any address of the form `scheme://user:password@host`, wherever it occurs inside a string, in `params`, `session`, `target` and an error text.

Every string a client supplies is bounded: `session` at 128 bytes, `target` at 256, `params` at 4 KiB, and the whole line at 8 KiB.

Actions: `submit`, `create`, `edit`, `release`, `retry`, `move`, `cancel`, `remove`, `signal`, `suspend`, `continue`, `reprioritize`, `update`, `update-abandon`, `config-reload`, `object-create`, `object-set`, `object-unset`, `object-rename`, `object-move`, `object-pause`, `object-resume`, `object-close`, `object-open`, `object-remove`, `queue-add`, `queue-set`, `queue-remove`, `queue-clear`, and `audit-rotation`. Previews (`--dry-run`) are not recorded.

Four more actions record what did not change anything. `attach` says who asked to attach to which Job; what is typed in the terminal is not recorded, and neither is the connection to the terminal itself. `connection-refused` records a peer the service turned away, with its user ID, process ID and the reason. `unreadable-request` records a request that is not readable, with its length and its first 64 bytes in hexadecimal. `unknown-request` records a request by a name this service does not know. The last three together are limited to 30 records a minute; the first one over the limit writes `audit-suppressed` with `"state": "started"`, and the next record after the minute writes `audit-suppressed` with the number left out.

The journal rotates at 8 MiB to `audit/<first seq>.jsonl` and keeps eight files, the current one included. The first record of a new file is `audit-rotation` and names the files and sequence numbers that were dropped. In the configuration file:

    [audit]
    keep_files = 8
    rotate_bytes = 8388608

A line cut short by a crash or by a failed write is skipped by the reader wherever it stands in a file, and the reader says how many lines it could not read. After a failed write the service ends the fragment with a newline before the next record, so the next record is whole. Cancellations, removals, resource updates and reprioritizations also carry the actor in their own records.

With a private socket every client runs as the service user, so `peer_uid` is that user and `peer_pid` tells clients apart. With a socket group, `peer_uid` is the user ID of the member who made the request. The journal is a file in the private state directory: `job audit` reads it directly and works only for the service user.

## Protocol versions

The client sends the range of request protocols it speaks with every request; the service answers a request outside its own range, or a changing request without a version, with the range and version it speaks. The client then prints both and exits 125. The service decides this from the envelope alone, before it reads the request inside, so a newer client whose request this service cannot read still gets the range. A request inside the range by a name the service does not know is answered with `this service does not know request NAME` and the range and version the service speaks. Restart the service from the release the client comes from. Supervisors of an older service may still report their end, and `Ping` and `Host` are answered without a version.

## Two services, one socket directory

A service takes a lock on `job.lock` in its runtime directory before it touches the socket there, holds it while it runs, and writes its state directory into the file. A second service pointed at the same runtime directory with another state directory stops with a message naming the first one's state directory, and the first stays reachable. The lock on `daemon.lock` in the state directory still keeps two services off one state.

`job doctor` reads who holds `daemon.lock` from `/proc/locks`. It never takes the lock, so it cannot make a starting service fail.

## Supervisors whose state is unknown

If the service cannot tell whether a supervisor is alive, it leaves the Job as it is and looks again a quarter of a second later. Only a supervisor that is certainly gone, with no `started.json`, puts a Job back in the Queue.

## Output budget

The output budget only removes the output of Jobs whose record says they ended. A Job directory whose record cannot be read is counted in the total with everything recorded in it, is never trimmed, and is named once in the service's log and in `job host` (`unreadable_records` in `job host --json`).
