# Resource policy

## What it does

Defines optional device I/O maps alongside independent CPU requests, CPU quota and weight, memory requests, memory high/max and swap max. Unset controls remain absent; explicitly unlimited limits have a separate representation. CPU input uses exact milli-CPU decimal parsing and memory input uses exact whole-byte sizes with binary suffixes. Limits are validated before submission. Effective per-Job defaults resolve field by field from the object hierarchy, with origin IDs and paths saved separately from original requested specifications.

Legacy cores and mem declarations retain their documented paired request/weight and request/max meanings. They never become CPU quotas. New defaults use job-prefixed keys, distinct from aggregate enforcement policies. The daemon refuses unavailable explicit kernel controls, and cgroup setup verifies readback before starting a workload. Status retains applied kernel values, including page rounding. Aggregate policies map to separate shared domains. Device I/O policy maps resolve as complete fields; live resource updates remain tracked in the implementation audit.

New resource-bearing local submissions, creates and edits use a versioned envelope so an old service cannot ignore the controls. Remote forwarding distinguishes compatibility mappings from explicitly requested controls. Managed controller disappearance and changed values are detected during execution.

## How to test

Run tools/check through job and explicitly run the delegated freezer integration suite. Unit tests cover exact parsing and numeric bounds. Watch tests cover request accounting, inherited defaults, source reporting, original specifications, retry and unavailable controls. Live tests inspect actual kernel files and quota throttling, and verify requests leave kernel controls unchanged.
