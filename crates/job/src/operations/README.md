# Operations: events, health, mode and shared access

## What it does

Keeps the append-only lifecycle event journal and reads it for `job events`. Job transitions are observed where a Job record is saved, admission transitions where the object graph and the pressure ledger are saved, so no call site decides what an event is. Computes the depth and wait statistics shown by `queue show` and `group show`, the health section and the surrounding cgroup and resource limits shown by `job host` and `job doctor`. States the operating mode of a service without a cgroup and refuses limits that mode cannot enforce. Serves retained output over the control socket to clients that cannot read the state directory, and places terminal sockets in the runtime directory when a socket group is configured.

## How to test

Run `cargo test --test operations` through job. The cases start the built binary with a test daemon without a cgroup and cover every Job transition with its exit, hold, release, cancel, retry and a launch requeued by recovery, Queue and Group pause, close and pressure holds, follow across rotation, a torn last line, depth and wait statistics, health before and after a restart, surrounding limits against the cgroup of the test itself, a required cgroup that is missing, refusals in monitoring mode under both profiles, and output and a terminal for a peer with a subordinate user ID. The last two print why they were skipped where no second user can be made. No unit tests.
