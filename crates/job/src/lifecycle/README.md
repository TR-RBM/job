# Job lifecycle

## What it does

Provides the language catalogue for the expanded lifecycle. Jobs can be created held, edited before execution and released. The model separates terminal outcomes from exit status, retains original submitted and latest requested specifications, and records an effective specification at launch. Original and latest environments remain in private state files; status does not return their contents.

Held work does not enter admission. Queued editing preserves its release time; edited work is revalidated before publication. Editing replaces the complete requested specification and environment, with current defaults resolved explicitly. The original submission remains intact. Release requires an open Queue; a paused Queue can accept release while keeping the Job queued. Running and terminal Jobs cannot be edited.

Submission and pre-execution editing publish the record and environment as one directory transaction. Starting and stopping transitions are persisted. Cancellation before execution leaves started_ms absent and contributes no execution history. Stopping keeps resource accounting and a persisted escalation deadline through recovery. Terminal states are succeeded, failed, cancelled or lost; legacy Finished records remain readable. The attempts module implements explicit retry and preserved attempt history. The freezer module implements confirmed suspension, continuation and observed time accounting for local cgroup workloads.

## How to test

Run the full repository gate through job. Watch-backend integration tests create and edit held work from another session label, restart and release it, inspect original and effective specifications, reject invalid and storage-failed edits, cancel held work without execution and recover a stopping Job without resetting its deadline.
