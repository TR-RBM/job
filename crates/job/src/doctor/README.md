# Doctor

## What it does

`job doctor [--json] [--system]` examines the host and the service setup from the client side and changes nothing. Each check yields a name, a status (`ok`, `warn`, `fail`), what was found and what to do. It covers the file locations, the configuration, the state directory, its schema version and a legacy store beside it, the service lock (read from `/proc/locks` by the lock file's inode, never taking or creating it), the socket's path, owner, group and mode, whether the service answers and runs the same version and protocol, access through a socket group, cgroup v2 mount, own cgroup, the cgroup root and the rule that chose it, controllers, block devices, the freezer, pressure files, user namespaces, Landlock, seccomp, `no_new_privs`, capabilities, process handles, network tools, the service manager and lingering. With the service running, cgroup facts come from the service; otherwise they describe a service started from the calling shell. The exit status is 1 when a check failed. On a systemd host, or inside a unit, `service_manager` is a warning in this release: the unit files were never run under systemd.

## How to test

`cargo test --test linux_integration` through job: `doctor_works_without_a_service_and_speaks_german`, `a_state_schema_this_program_does_not_serve_fails_the_doctor`, and the service cases listed in `../service/README.md`, which read the running service through it. `review_doctor_never_makes_a_starting_daemon_fail` runs it in a bounded loop beside twenty service starts. No unit tests.
