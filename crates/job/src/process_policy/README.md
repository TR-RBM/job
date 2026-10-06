# Process launch controls

## What it does

Provides optional CPU affinity, NUMA task policies and per-resource rlimit soft/hard pairs. Typed values resolve Queue/Group and pinned-profile defaults with independently tracked origins. Prepared masks and limits apply only to the workload child, after cgroup/isolation setup, using allocation-free syscalls and exact readback. Remote work validates and applies on the execution host. Launch results retain the verified explicit settings; historical validation does not depend on present topology.

## How to test

Run process_policy_ service integration cases through job. Use JOB_REQUIRE_NUMA=1 with --nocapture on a capable host to require actual policy application. These cover preserved defaults, descendants, EMFILE/EFBIG, inheritance/reset/profiles/retry, host and kernel refusals, and executed-program readback. State migration cases cover optional-field migration and archive/backup validation. The remote transport integration checks workload limits do not affect its transport. Explicitly select process_placement_and_rlimits_coexist_with_cgroup_controls in the delegated freezer suite to check placement and limits alongside real cgroup controls. No new unit tests.
