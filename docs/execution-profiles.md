# Execution profiles and scheduling classes

Profiles are optional named defaults. Creating a Queue without options attaches no profile or class. A scheduling class sets admission priority; an execution profile can explicitly combine that class with resource controls or execution options.

## Define and inspect

Add definitions to the service's `job.conf` TOML document:

```toml
schema_version = 1

[[presets.classes]]
name = "interactive"
revision = 1
priority = 100

[[presets.profiles]]
name = "build-base"
revision = 1
[presets.profiles.values]
cpu_request_milli = 1000
memory_request = 1073741824

[[presets.profiles]]
name = "build"
revision = 1
extends = ["build-base@1"]
scheduling_class = "interactive@1"
[presets.profiles.values]
cpu_limit_milli = 2000
cpu_weight = 100
memory_high = 2147483648
memory_max = 3221225472
```

```sh
job config check /path/to/job.conf
job config reload
job profile list --json
job profile show build@1 --json
job class show interactive@1 --json
job submit --execution-profile build@1 -- make
job submit --class interactive@1 -- command
```

Reload requires a drained service. Listing and showing definitions read the accepted running configuration, not an unaccepted edited file. Structured output includes schema version 1, definition bodies and SHA-256 digests. Names contain 1–64 ASCII letters, digits, hyphens or underscores. Revisions are positive integers. References must include a canonical revision, such as `build@1`; `build`, `build@01` and `none@1` are invalid.

Classes accept `name`, `revision` and `priority` only. Priority is between -1000 and 1000 and remains subject to all ancestor priority bounds. Aging, fair-share policy and pressure rules remain explicitly configured on their existing scopes; a class does not silently enable them.

Profile values accept independent resource fields: `cpu_request_milli`, `cpu_limit_milli`, `cpu_weight`, `memory_request`, `memory_high`, `memory_max`, `memory_swap_max`, `io_max`, `io_weight` and `io_bfq_weight`. Their types and validation match the [resource controls](resources.md). Memory values are bytes; CPU request/limit values are millicores. Limit fields also accept `"unlimited"`. Launch values `cpu_affinity`, `numa_policy` and per-resource `rlimit_NAME` pairs are also supported; see [process controls](process-controls.md). Security values `no_new_privs` (boolean), `cap_drop` and `seccomp_deny` (comma-separated names) are supported as well; see [security controls](security-controls.md). Rlimit resources inherit independently, with the same nearest-field precedence. Additional supported values are positive `pids`, positive `wall_ms`, boolean `confine`, and `net` equal to `"Host"`, `"None"`, `"profile:NAME"` or `"ns:NAME"`. The last two name a routing profile or a network namespace of the service configuration, see [networking](networking.md#named-routing-profiles); the name must exist when the configuration is read, and the Job resolves it again when it starts. Credential-bearing proxy/VPN definitions are not accepted in these snapshots: a proxy is reached through a routing profile, whose credentials stay in its `secret_file`. Unknown fields, null values and invalid types are rejected.

## Compose and override

`extends` is an ordered list of pinned profiles. Parents apply first, in listed order; later parents win overlapping fields. A profile's own fields apply last. Its `scheduling_class` can select a pinned class or `"none"` to disable a parent profile's class. Omitting a field inherits its composed value.

```sh
job group create builds --job-execution-profile build@1
job queue create builds/quick
job queue set builds/quick --job-class interactive@1
job submit -q builds/quick --cpu-weight 200 -- make
job submit -q builds/quick --execution-profile none --class none -- command
job queue unset builds/quick job-class
```

Resolution follows these rules:

1. Explicit Job fields, including legacy resource aliases, take precedence over profile values.
2. A profile selected on a Job overrides collection field defaults. An inherited profile loses to fields set on the same or a closer collection.
3. The nearest explicit profile selector wins. Class selectors are resolved independently; a closer profile's class overrides a farther class selector, and a class selector wins at equal distance.
4. `none` disables that selector's inheritance. It does not erase independent priority/resource defaults or ancestor restrictions. `unset` removes a collection selector and restores inheritance.
5. All ancestor budgets, priority bounds, pause/close states, pressure gates and kernel limits continue to apply.

Use `job status ID --json` for the attempt's `preset_snapshot`, requested specification and effective fields; `job explain ID --json` includes admission origins. A snapshot records composed defaults even when an explicit field superseded them; its `applied` map identifies which defaults actually supplied values. Live resource changes and reprioritization have separate recorded origins.

## Revision history and recovery

Once accepted by the service, a revision is immutable. Publish a new revision and change the desired selectors. Existing attempts retain their snapshots; held edits and retries resolve their requested options against the current collection defaults. Historical definitions remain in `presets.json` when removed from active configuration. Removing a definition still referenced by a collection is refused. A new submission cannot select a removed revision just because it remains archived.

Backups include the archive and all attempt snapshots. State schema 14 requires offline conversion from previous versions; see [migration](migration.md). Archive corruption or snapshot disagreement is an error during startup and offline validation. The archive has no automatic pruning: 4096 definitions or 16 MiB requires an explicit future retention workflow. Active configuration allows 128 definitions; composition allows 16 parents, 16 levels and 1024 expansion visits.
