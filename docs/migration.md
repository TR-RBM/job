# State migration and restoration

This guide is for moving an installation of the earlier tool, which kept its state in `~/.local/state/exec`, to this release, and for backing up and restoring the state of this release. Conversion is an explicit, offline step. The program never converts state in place or on its own.

The conversion is tested on constructed stores of every earlier schema. A conversion of the state of a real installed service was not performed; see the [known limits](release-notes.md#known-limits).

## Versions

This release writes state schema 18 and speaks protocol 20.

- The `job` command and the service must come from the same release. The service speaks protocol 20 only. A command of another protocol is refused with both ranges and exit status 125, and the service decides that before it reads the request. A command older than this negotiation cannot read the refusal and reports an unreadable answer.
- Between two hosts, `--on` is refused when the remote protocols differ, and the message names both.
- The offline commands `job state validate`, `backup`, `migrate` and `restore` read schemas 1 through 18 and unversioned stores of the earlier tool. The service itself starts only on schema 18.
- An earlier program cannot use a schema 18 directory. To go back, restore the backup that the conversion made; see [Back up and restore](#back-up-and-restore).

## What changes for an existing installation

- **The service is `jobd`.** `job daemon` remains as an earlier spelling. Change service definitions to `jobd`, or `jobd --system` for a system service.
- **The socket moved.** It is `job.sock` in a runtime directory where one exists: `$XDG_RUNTIME_DIR/job/` for a user service, `/run/job/` for a system service, or the directory named by `JOB_RUNTIME_DIR`. A service without a runtime directory, such as one started by runit without a login session, and a service whose state directory is named by `JOB_STATE_DIR`, still bind `daemon.sock` in the state directory, and clients look there too. The service and its clients must see the same variables. The service holds a lock on `job.lock` beside `job.sock` while it runs, so that a second service cannot take over the same runtime directory. The complete rules are in the [administration guide](administration.md#3-the-service-and-its-delegated-cgroup).
- **Nothing is reserved or limited by default** in the `ordinary` service profile. The `legacy` profile, chosen at conversion, keeps estimates, backfill, the default process limit and the emergency stop.
- **A Queue made without a ceiling has none.** `job queue add NAME` without `--parallel` used to create a Queue that ran one Job at a time. It now creates a Queue without a ceiling, as `job queue create NAME` does. Write `--parallel 1` or `--max-running 1` where a new Queue must run its Jobs one after another. Queues that existed before the conversion keep one at a time as an explicit `max-running` of one.
- **Arguments are literal and `wait` is quiet.** See the [release notes](release-notes.md#what-changed-incompatibly).

## What the state directory holds

Beside the records an earlier store had (`jobs/ID/job.json`, `result.json`, `output.log`, the saved environments, `objects.json`, `links/`, `history.jsonl`, `next-id`, `schema.json`), a schema 18 directory can hold:

| Path | Content |
|---|---|
| `audit/current.jsonl` and rotated `audit/SEQ.jsonl` | the audit journal of requests that change something, read by `job audit` |
| `events/current.jsonl` and rotated files | the journal of state changes, read by `job events` |
| `idempotency/HASH.json` | one entry per idempotency key |
| `net-secrets/` | private files with proxy credentials of Queues and Groups |
| `private-tmp-leftovers/` | private temporary directories of Jobs that could not be removed; nothing deletes them, and the Job's result names the directory |
| `jobs/ID/started.json` | written by the supervisor before it runs the command |
| `jobs/ID/exit.json` | the exit status, written when the command ends and before cleanup |
| `jobs/ID/net-secret` | proxy credentials of one Job, mode 600 |
| `jobs/ID/streams.json` and stream segments | the separate recording of stdout and stderr |
| `jobs/ID/attempts/N/` | archived earlier attempts |
| `scheduling.json`, `pressure.json`, `presets.json` | the scheduling ledger, pressure controller state and the archive of profile revisions |
| `cancellations/`, `removals/`, `resource-updates/` | journals of operations that span several records |

All of them are optional: a store without them is complete, and a converted store gains them as the service works. All of them are part of a backup. The runtime directory holds `job.sock`, `job.lock` and, with a socket group, `terminals/`; these are not state and are not backed up.

A cancellation of a set of Jobs records the set as written in its operation file; a program from before that change refuses such a record, so finish or let end every cancellation before starting an earlier program on the same state. The earlier state `Finished` is never written. A record that still carries it is read as succeeded or failed by its exit status, or as cancelled or lost when its stop says so.

## Prepare

Run maintenance as the Unix user that owns the service state. Close admission, finish or cancel waiting and running Jobs, and stop the previous service and its network helpers. Maintenance takes the same exclusive lock as the service. It refuses a running service, active Jobs, queued Jobs and live network helpers; it does not terminate them for you. Depending on the service manager, stopping only the service may leave supervisors and network helpers alive.

Choose three separate paths: the existing source, a new backup and a new destination. Their parent directories must already exist. Backup and destination must not exist and none may be inside another. Existing destinations are never overwritten. Source data remains unchanged; inspecting a store can create its lock file if one is absent.

## Inspect and convert

These examples assume `~/.local/state/exec` contains the old installation and the destination paths do not exist:

```sh
job state validate --source ~/.local/state/exec
job state migrate --source ~/.local/state/exec \
    --destination ~/.local/state/job \
    --backup ~/.local/state/job-before-upgrade \
    --profile legacy --dry-run
```

The JSON inventory reports the schema, Job and container counts, active Jobs and network helpers, unbound Job records, recovered historical Queue names, whether conversion can proceed and, during a migration preview, the selected profile and its effective compatibility rules. A successful dry run can report `conversion_ready: false`; the actual conversion then refuses to proceed. Validation checks records, Queue references, next-ID consistency and history. Unknown schemas, damaged JSON and unexpected filesystem objects are errors. Migration does not silently discard malformed records.

Choose the service profile explicitly. `legacy` retains automatic estimation, backfilling, default process and swap policies and emergency host protections. `ordinary` removes those implicit application policies for future execution. Enclosing service-manager limits and administrator settings are outside the state directory; inventory and retain those separately. See [job.conf(5)](../man/job.conf.5).

Remove `--dry-run` to back up and convert. The backup is complete and published before conversion starts. Job IDs, recorded results, output bytes and additional metadata are preserved. The destination also records a migration report with source, backup, actor and inventory. Unqueued records receive the default Queue ID; old serial Queues receive explicit concurrency one. A deleted Queue referenced by historical Jobs receives a retired identity with unknown settings left empty, rather than a new active Queue. Resolution follows recorded names because pre-hierarchy records had no immutable Queue IDs. Dry-run binding is deterministic.

State schema 18 identifies its vocabulary as `independent_resources_with_legacy_aliases`. It retains optional pressure rules, the validated pressure controller journal and pressure launch explanations, and retains optional versioned preset definitions and attempt snapshots, retains per-attempt output modes, bounded stream journals and recording errors, and adds optional placement/rlimit declarations and verified launch outcomes, and since schema 17 optional no_new_privs, capability and seccomp declarations with their applied outcomes, and since schema 18 optional namespace and mount-policy declarations with their applied outcomes. Older records gain no automatic process, security or isolation controls. Schema 18 also covers the optional per-Job start and exit records, the audit journal under `audit/`, the event journal under `events/` and the idempotency index under `idempotency/`; a store without them is complete. Historical IDs and policies are syntax-validated without requiring the original host topology. Older merged logs remain merged; migration does not invent stream identities or timestamps. Missing pressure and preset configuration remain absent; migration enables neither. The preset archive and snapshots are validated during startup, backup and offline maintenance. Retain external service configuration separately: generated migration configuration selects the requested compatibility profile and does not infer host rules or active preset selections from historical state. Restore the intended explicit host rules and preset catalogue before restarting managed execution; startup refuses collection references absent from active configuration.

Hierarchical fair-share accounting remains in the versioned scheduling ledger. Scheduling ledger schema 1 loads with empty fair-share history and advances to schema 2 on its next write; offline conversion does not manufacture a policy or service credit. Missing admission fields remain unset. Ledger corruption, invalid weights and malformed decimal service counters are validation errors. The schema retains optional admission priority, origins, reprioritization records, eligible-wait credit, durable live resource-update receipts, optional device I/O policy maps, aggregate domain chains, kernel OOM counters, independent per-Job controls, recorded origins and applied values, captured cancellation operations, removal receipts, attributed estimator samples, freezer intent, numbered attempts and explicit lifecycle states. Offline conversion accepts schemas 7–12 with the independent resource vocabulary, schemas 1–6 with `legacy_declarations`, and unversioned legacy stores. Schema 7 records without aggregate policy retain flat workload paths. Historical declarations remain preserved; missing new fields mean unset. Retrying resolves the old `--cores` declaration as request plus weight, and `--mem` as request plus memory max. Neither is reinterpreted as a CPU quota or filesystem capacity limit. See [resource controls](resources.md) and [admission ordering](scheduling.md).

Old records without an attempt number represent attempt one; absent suspension data means no recorded suspension. Unattributed legacy estimator samples retain their unknown provenance. Existing archives, resource-update receipts, cancellation operations and removal receipts are preserved, and restored historical logs resolve within the selected store. Validation includes retired Job IDs when checking the allocation counter. Maintenance refuses pending resource updates, removal or cancellation; complete those operations with the owning service before backup or conversion. Retrying an imported Job whose environment was not retained requires explicit `--current-env`.

Start the converted service with:

```sh
JOB_STATE_DIR=~/.local/state/job \
JOB_CONFIG=~/.local/state/job/migration-config.toml jobd
```

Apply those environment settings to the service manager for an actual installation. Migration does not alter service units, sysctls or existing cgroup limits. Default discovery uses `$XDG_STATE_HOME/job` or `~/.local/state/job` and refuses to guess while a legacy `exec` store exists. Explicit `JOB_STATE_DIR` selects one context.

## Back up and restore

`job state backup` copies the store of a stopped service. Held and queued Jobs are copied and run on the restored service. It refuses a store whose service is running, a store with live network helpers, and a store with a Job that claims a live execution (starting, running, suspended or stopping) unless `--interrupted` is given; a service started on a store restored from such a backup records those executions as lost. `--interrupted` is itself refused while a recorded supervisor of this boot is still alive. Converting a store to a newer schema with `job state migrate` still needs a drained store. It also refuses a store with a pending resource update, removal or cancellation.

```sh
job state backup --source /path/to/state --destination /path/to/backup
job state restore --source /path/to/backup --destination /path/to/restored-state
```

Backups contain original files under `data/` and a manifest with SHA-256 hashes, lengths and permission bits. They include logs, outcomes, histories and retained environment files. Keep them private: backup roots are owner-only. Service locks and Unix sockets are runtime artifacts and are excluded. Symlinks and other special files are refused. Restoring verifies the complete file inventory and hashes before publishing the destination. The manifest detects accidental damage; it is not an authenticated signature against someone who can rewrite the entire backup.

Restoration does not perform schema conversion. A legacy backup is restored in the legacy format, for use with its matching release or a subsequent explicit migration. The current binary requires migrated state; it does not auto-convert an old restored directory. Legacy binaries may require restoration at the original path because their records contain absolute log paths. The current binary resolves logs within the selected store: a store restored into another directory serves its output from there, and the log paths in its answers lie below its new location, not below the directory it was backed up from.

To roll back, stop new submissions, drain new work and stop the new service and helpers. Preserve any work created after migration separately. Restore into an unused directory, validate it, and deliberately select that directory with the matching binary and configuration. Restoration never merges histories or discards newer work automatically.

## Failure boundaries

Each published backup or destination is a fully written directory, atomically renamed without replacement. Files and directories are synced before publication. Unsupported filesystem rename semantics fail explicitly. A crash before publication may leave a private `.job-stage-*` directory next to the intended destination; it is not a valid published backup or state directory. Inspect it after stopping maintenance before removing it. The original source remains available. A failure after backup publication can leave a valid backup with no converted destination.

The checked-in integration tests demonstrate byte-for-byte restoration, preserved serial behavior and IDs, usable restored logs, corrupted-backup refusal, source immutability, active-service and workload refusal, path checks and ambiguous discovery. `a_destroyed_mixed_service_is_restored_from_backup_and_answers_identically` restores a service with Groups, Queues, completed Jobs, journals and settings from its backup and compares its answers. The conversion of the state of a real installed service, and a restart under systemd, were not performed; see the [release notes](release-notes.md#known-limits).
