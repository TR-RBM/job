# Offline state maintenance

## What it does

Validates supported state, inventories legacy Job/Queue bindings, creates content-verified backups, converts drained state into a new directory and restores original backup bytes. Uses the daemon's exclusive lock, explicit profile selection, SHA-256 manifests, private staging directories and rename without replacement. The original source is preserved. The state schema identifies the current legacy resource vocabulary without claiming the resource redesign is already complete.

Runtime schema validation rejects unversioned existing stores and corrupt records. Fresh state initializes schema 14 with optional versioned presets, pressure/scheduling policy, device I/O, aggregate domains, independent resource controls and preserved legacy aliases. Earlier supported schemas require explicit offline conversion. Pending cancellation operations are valid for runtime replay but prevent maintenance backup and conversion. Backup copies held and queued Jobs; records of executing Jobs need `--interrupted` and supervisors that have ended. CLI diagnostics have English and German catalogue entries. Paths default to the XDG job directory; simultaneous legacy exec state requires explicit selection or migration. See [migration guide](../../../../docs/migration.md) for operational boundaries and recovery.

## How to test

Run `cargo test --test state_migration` through job, followed by the repository gate. Tests start private services, create records, convert legacy fixtures, validate original backups and exercise recovery and failure cases. They do not alter the installed service or its state.
