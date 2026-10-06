# Removing execution records

Removal is explicit, and cancellation is a separate operation. A running process is not killed because its record was selected for deletion. Held and queued work must also be cancelled before removal.

Use `job queue cancel --recursive PATH` or `job group cancel --recursive PATH` to stop current work separately; see [cancellation](cancellation.md). Minimal cancellation audit records survive Job record removal, alongside removal receipts. They retain identities, states, actor, time and errors, without command, environment or output.

```sh
job remove 42 --dry-run --json
job remove 42 --json
job queue remove builds --recursive --dry-run --json
job queue remove builds --recursive
job group remove projects --recursive --dry-run --json
```

## Selection

An ordinary container removal requires no child objects and no current Job records. Recursive removal selects the mixed subtree and all Jobs currently assigned to its Queues. It does not select a Job in another Queue merely because an older attempt used the target. Root and default Queue are structural and cannot be removed.

The preview includes stable object IDs and paths, Job IDs, current attempt numbers and outcomes, helper names, readiness and blocking reasons. A successful dry-run command means the preview was produced; inspect `ready`. The CLI submits the same selection for mutation. A retry, move or new submission that changes that selection causes refusal. Close the relevant container first when a stable selection is needed while other clients submit work.

All selected Jobs must be terminal. Every attempt is checked for known live supervisors and populated recorded workload cgroups. Any lost attempt requires `--allow-lost`, including a lost historical attempt followed by a successful retry. This flag acknowledges potentially untracked work; it never overrides known live work and does not stop escaped processes.

## Data retained and removed

Removal deletes each selected Job's complete state directory, including archived attempts, logs and saved environments. It deletes derived output-template files and estimator samples carrying that Job ID. New estimator samples carry Job and attempt identity. Unattributed legacy samples, including their command keys and measurements, remain because their provenance cannot be established safely; the preview reports `unattributed_history_samples_retained` for the store.

Files produced in the workload's working directory are outside this operation. Minimal receipts remain in `removals/job-ID.json` or `removals/object-ID.json`, naming the service Unix actor, requested timestamp and selected identities, attempt numbers, outcomes and object paths. Receipts contain no submitted command, environment or log output. They also prevent removed IDs from being reused. Historical Queue/Group identity snapshots remain in the object graph for surviving references; those snapshots retain their historical settings. Removing a container cleans up its recorded idle network helpers using their verified process identities.

This is logical record deletion, not secure erasure. Existing backups and already open file descriptors are unaffected.

## Failure and recovery

Preflight and intent-write failures leave records untouched. Once a private removal journal is durably published, the operation is committed to completion. The service retires selected directories outside the authoritative Jobs namespace, updates the graph and attributed history, removes derived templates and payloads, and publishes a minimal receipt. Cleanup is idempotent; the same expected selection returns its existing receipt after completion.

A confirmation wait lasts up to five seconds. Exit 75 before commitment means a supervisor is still exiting and no deletion was started. Exit 75 with `committed: true` and `cleanup_pending: true` means the journal was accepted but cleanup has not completed. Correct the reported filesystem or helper problem; the running daemon retries automatically, and a restarted daemon replays the journal before accepting work. Conflicting administrative mutations are refused while cleanup is blocked. Existing execution is still monitored, and inspection, signalling, cancellation and freezer control remain available.

Offline validation, backup and migration refuse an unfinished removal journal. Complete it with the owning service before maintenance. Do not manually delete the journal or retired directories to hide an error. The journal is what lets the service reconcile an interrupted operation without resurrecting records or overwriting unrelated object changes.

Parent directories and removal data are checked for symlinks and unsupported objects. State-directory corruption causes an explicit refusal. A loss of power and a full run under a service manager were not exercised.

## A set of Jobs

`job remove 1-10` removes every completed Job of the set in one transaction with one receipt, named after the lowest removed ID. A Job that has not ended, or a lost one without `--allow-lost`, is reported for its ID and stays; the others are removed. `--dry-run` shows what would be removed. The grammar and the answer are in [sets of Jobs](cli-execution.md#sets-of-jobs).
