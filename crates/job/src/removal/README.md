# Record removal

## What it does

Prepares a versioned exact selection, validates completed attempts and process identities, and removes Job state through a durable deletion journal. Recursive container removal follows current Job membership and preserves retired object identities for other attempts. Root/default objects are protected. Removal never cancels work.

The daemon serializes commitment, blocks conflicting administrative mutations while cleanup is pending, keeps supervising existing execution, and replays the journal before normal startup validation. Private retired directories keep partially purged records outside the executable namespace. Filesystem parents and payload entries are checked; renames do not replace existing targets. Receipts contain selected identities rather than execution payload and are retained for idempotent request replay and ID allocation.

Attributed estimator samples and all selected attempts' output templates are purged. Unattributed legacy samples and retired object metadata remain explicitly documented. Source working directories are outside the deletion scope. Network helper cleanup uses boot identity and pidfds. Backup refuses pending deletion and otherwise preserves receipts byte-for-byte.

## How to test

Run tools/check through job. Integration tests exercise complete attempt deletion, preserved work files and unrelated Jobs, read-only previews, active-work refusal, exact selection and replay, mixed recursive removal, source publication failure, mid-transaction restart, backup/restore and retired IDs, historical Queue references, live supervisor refusal, lost-attempt acknowledgement, helper cleanup, symlink rejection and populated-cgroup refusal.
