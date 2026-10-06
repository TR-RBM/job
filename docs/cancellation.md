# Cancel Jobs and collections

```sh
job cancel 42
job queue cancel --recursive builds --dry-run --json
job queue cancel --recursive builds --json
job group cancel --recursive production --json
job wait 42
```

Cancellation stops work and retains its records. Use remove separately to delete eligible completed records. Collection cancellation requires an explicit recursive flag, including for a Queue, and traverses mixed Group/Queue subtrees. It selects current nonterminal Job attempts: held, queued, starting, running, suspended and stopping. The default Queue and root Group can be cancellation scopes.

The CLI captures a selection and commits it against immutable identities. A changed attempt or membership before commitment fails. Natural completion between selection and commitment is reported without changing its outcome. Later submissions are outside the captured set. Collection configuration, pause/close state and future admission remain unchanged; pause a collection separately when future starts must also stop.

The service records a durable operation before applying it. Held/queued Jobs become cancelled without running; active Jobs receive managed termination, including thaw before TERM and escalation at the existing termination deadline. Repeating the same selection returns the original operation. It does not extend a running Job's grace period or cancel a replacement attempt. Legacy queue clear still selects waiting work only, using the same durable engine.

## Results and recovery

JSON operations include schema version, operation ID, actor UID, request time, captured selection, results and persistence error. Each result contains Job ID, attempt, observed state, pending and error. Complete means cancellation application has been recorded, not that every process has exited. Use wait or status for process completion. Completed operation results describe application time rather than a continuously refreshed view.

Exit 0 means all captured attempts were handled; exit 75 means the operation is pending and the daemon will retry. A partial record-write failure cannot allow selected waiting work to start. Exit 1 means a completed operation contains a per-Job error, for example a superseded attempt encountered during recovery. Errors before commitment use the ordinary service-error exit. A disconnected client may have an unknown commitment outcome; the protocol accepts the original selection for idempotent replay.

Private records in the state's cancellations directory retain identities, states, errors, actor and time. They contain no command, environment or output and survive removal of Job records. Configurable audit retention remains future work. Backup and migration require all cancellation operations to finish; runtime startup validates and replays pending operations. Cancellation operations were introduced in state schema 6; see the [migration guide](migration.md) for the current format and supported offline conversion.

Reconciliation skips members whose successful application was already recorded, even if another member remains pending. A crash between signal delivery and saving that progress can cause a signal to be delivered again during recovery; signal delivery and a filesystem transaction cannot be made atomic. The saved termination deadline still prevents replay from extending the grace period.

## A set of Jobs

`job cancel 1-10`, `job cancel 1,4,7-9` and `job cancel 1-3 8` cancel a set of Jobs in one request and record one operation with the Jobs as members and the set as written, as a recursive cancellation does. Jobs that have ended are reported per ID and do not stop the others; `--dry-run` shows the selection and changes nothing. The grammar and the answer are in [sets of Jobs](cli-execution.md#sets-of-jobs).
