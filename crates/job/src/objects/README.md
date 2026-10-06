# Objects

## What it does

Stores a mixed tree of Groups and Queues in a versioned snapshot. IDs are immutable and never reused; paths derive from parent IDs and names. The root Group and default Queue are structural objects. New objects store empty local configuration. Explicit unlimited concurrency is distinct from an absent value. Inspection includes local settings, inherited defaults with their source, and independent ancestor pause/close states.

Graph changes validate sibling names, parent types and cycles before publication. Snapshot replacement syncs the file and parent directory. The daemon publishes its new in-memory view only after storage succeeds. Legacy serial Queue files import as explicit concurrency settings and remain untouched. Removed legacy Queues retain an identity snapshot for historical Job references. Full-store migration dry runs, backup/restore and transactionally coordinated Job records are still part of the ongoing redesign.

Existing execution code consumes an effective flat Queue projection while hierarchy-aware admission checks enforce ancestor pause and concurrency. Running subtrees cannot be moved or renamed. New removal refuses existing Job records and nonempty containers. Legacy draining/removal preserves its compatibility behavior with retired object metadata.

## How to test

Run `cargo test objects::` and `cargo test --test watch_backend` through the host job service. CLI tests create mixed trees, inspect empty settings, move objects without changing their IDs, reject cycles and retain independent holds. Migration tests import existing serial queues and reject damaged metadata.
