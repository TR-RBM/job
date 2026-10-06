# Execution attempts

## What it does

Validates and stages immutable completed attempt archives. Current execution stays in jobs/ID; earlier attempts live in jobs/ID/attempts/N. The counter starts at one and archives must form a contiguous sequence ending before the current attempt. Archive records must identify their Job and attempt, be terminal and contain only regular files. Saved environments and results are validated when present. Old records without environments remain inspectable.

Retry first verifies that the previous supervisor has exited. Under the daemon mutex, it stages the completed attempt and a fresh current record/environment and atomically exchanges directories. Earlier archives are hardlinked into the staging tree without copying their log contents again. Files in archives are never overwritten; log retention can unlink expired logs. A failed staging operation leaves authoritative state untouched. Interrupted staging directories are not executable Jobs.

Inspection rebases each log path to the selected store, including restored backups. Retry uses an expected attempt number to reject concurrent or replayed requests for an already superseded attempt. This protects this transition; general protocol idempotency and launch/result crash transactions remain separate work.

## How to test

Run tools/check through job. Integration tests verify retries, held edits and restarts; saved and explicitly replaced environments; archived logs after backup/restore; FIFO admission; renamed and retired Queue identities; stale requests; closed Queues; live supervisors; lost-work acknowledgement; failed publication; damaged archives; and offline schema conversion.
