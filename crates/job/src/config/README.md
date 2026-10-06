# Service configuration

## What it does

Parses versioned TOML service configuration, rejects unknown keys, discovers the configuration through JOB_CONFIG or the XDG configuration directory, and describes the effective ordinary or legacy profile. Initializing a file refuses overwrites and syncs it with owner-only permissions. Configuration inspection queries the daemon rather than assuming the client's environment matches it.

Ordinary execution supplies no estimates or automatic host-pressure actions. Legacy behavior is an explicit compatibility profile. Persisted Jobs record their profile; old records deserialize as legacy. A daemon refuses recovery of active records under another profile. Reload requires drained work and the same profile. Profile changes require a restart.

## How to test

Run `cargo test config::` and `cargo test --test watch_backend` through job. The latter verifies empty reservations after repeated executions and daemon restarts, ordinary dispatch without backfill reservations, ancestor admission budgets and profile reload restrictions. Use `job config check FILE` and `job config show --json` for operator inspection.
