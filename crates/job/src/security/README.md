# Security launch controls

## What it does

Provides optional `no_new_privs`, capability reduction and a native-ABI seccomp deny list as launch controls. Typed values resolve Queue/Group and pinned-profile defaults with independently tracked origins. The filter and masks are prepared before fork and applied in the workload child only, last in its setup, without allocation, each step read back before exec. The bounding set is reduced where the launch context permits it and the result records whether it was. Remote work validates and applies on the execution host. Launch results keep the applied settings.

## How to test

Run the security_policy_ service integration cases through job. They cover the plain baseline, descendants, a denied and an allowed system call, capability sets with and without a launch-created user namespace, inheritance, reset, profiles, retry and refusals before the payload. stream_remote_supervisor_decodes_frames_and_separates_transport_stderr checks that the local transport stays unfiltered and on the host network while the destination applies both. State migration cases security_launch_controls_survive_archival_backup_and_reject_invalid_outcomes and schema_sixteen_conversion_does_not_invent_security_controls cover storage. No new unit tests.
