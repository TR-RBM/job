# job

**job is an open source execution and scheduling application for Linux.** Run commands, organize work into queues and groups, and control when and how processes execute. Use job from a terminal, a script, or an automated system.

This release runs work on one host and on explicitly named hosts over SSH. It does not place work across a cluster and has no calendar scheduling.

## Purpose

A service, `jobd`, starts each command under its own supervisor, records its output and outcome, and keeps it running independently of the terminal that submitted it. The `job` command talks to that service. You can submit work and leave, wait for it later, read its output, hold it, change it, retry it, and decide in which order and under which limits waiting work starts.

Nothing beyond that is switched on by itself. A Job without options gets no resource reservation, no limit and no sandbox. Priorities, fair share, resource limits, pressure rules, security controls, namespaces and network boundaries are each requested explicitly.

## A short example

Start the service in one terminal, or under a service manager as the [administration guide](docs/administration.md) describes:

```sh
jobd
```

Then, in another terminal:

```sh
job run -- make test
job submit -- make release
job wait 42
job logs 42
job group create development
job queue create development/builds
job submit --queue development/builds -- make
job list
job explain 43
```

`run` waits and passes the command's output and exit status through. `submit` prints a Job ID and returns; use that ID in place of `42`. `wait` is quiet and returns the Job's exit status. Arguments after `--` are executed as they are; for shell syntax write `sh -c '...'` or use `--shell bash`.

## Objects

| Object | Meaning | Contains |
|---|---|---|
| Job | A command, its state and its recorded outcomes | Execution attempts |
| Queue | A collection of Jobs | Jobs |
| Group | A collection for organization and optional shared policy | Queues and Groups |

```text
root
├── default                         Queue
├── development                     Group
│   ├── builds                      Queue
│   ├── tests                       Queue
│   └── releases                    Group
│       └── packaging               Queue
└── operations                      Group
    ├── backups                     Queue
    └── maintenance                 Queue
```

Every Job belongs to one Queue and every Queue and Group to one parent Group. The root Group and the `default` Queue exist without setup. A new Queue has no settings of its own and changes organization only. A Group here is not a Unix group, a process group or a kernel cgroup.

## Capabilities

- Run, submit, hold, edit, move, release, retry, cancel, signal, suspend and continue Jobs; every retry is a numbered attempt with its own output.
- Queues and Groups in one tree, with pause, close, concurrency ceilings and inherited defaults, labels for classification, and an explanation of where each setting comes from.
- Optional admission priority, aging, strict order, backfill and fair share between collections.
- Optional CPU, memory and device I/O requests, limits and weights, per Job or shared by a Queue or Group, changeable while work runs.
- Optional pressure observation and admission rules based on Linux PSI.
- Optional CPU affinity, NUMA policy and process resource limits.
- Optional `no_new_privs`, capability reduction, system call deny lists, write confinement, namespaces, a read-only root and a private temporary directory.
- Optional network boundaries: no network, a proxy, a WireGuard tunnel, a bandwidth ceiling.
- Separate recorded stdout and stderr with retention quotas, and shared persistent terminals.
- A journal of lifecycle events, queue depth and wait statistics, a health report, and an audit journal of every request that changes something.
- A socket that can be shared with one Unix group, for a team that trusts each other.
- Execution on another host over SSH.
- Structured JSON output, tab-separated listings, manual pages, and Bash and fish completion.

The [capability matrix](docs/capabilities.md) says for each of these how to request it, what it needs from the host, what happens when the host lacks it, and where it is tested. It also lists what is not available.

## Installation

job is installed from source with stable Rust and `make`:

```sh
make build
make install                      # PREFIX=/usr/local
make install PREFIX=$HOME/.local  # a private installation
job doctor
```

`DESTDIR` stages the same tree elsewhere for a package, as in `make install PREFIX=/usr DESTDIR=/tmp/pkg`. `make install` puts `job` and the service entry point `jobd` in `bin`; the manuals `job(1)`, `job.conf(5)`, `job(7)` and `jobd(8)` under `share/man`; Bash and fish completion; a systemd system unit and a systemd user unit; and under `share/doc/job` the licence, the administration guide, the migration guide and a runit run script. The other guides are read from the source tree. It creates no user, enables no service and changes no kernel setting. `make uninstall` removes exactly what was installed.

`job doctor` checks the host and the service setup and says what to do for each finding. The [administration guide](docs/administration.md) covers the service, cgroup delegation, file locations, sharing the socket with a Unix group, and configuration.

## Documentation

- [User guide](docs/user-guide.md): the path through the product, with links to the topic pages.
- [Administration guide](docs/administration.md) and [migration guide](docs/migration.md).
- [Capability matrix](docs/capabilities.md) and [security model](docs/security.md).
- [Operating the service](docs/operations.md), [reliability](docs/reliability.md) and [performance](docs/performance.md).
- [Syntax reference](docs/reference/syntax.md): every command and option, generated from the program.
- [Release notes](docs/release-notes.md), with the [known limits](docs/release-notes.md#known-limits) of this release.
- [Automation](docs/integrations.md): the diagnostic summary, classifying a command line, the command policy file and the hook.
- Manuals: `job(1)` for commands, `job.conf(5)` for configuration, `job(7)` for concepts, `jobd(8)` for the service. From a checkout, `man -l man/job.1` shows a page without installing.
- `job help`, and `job COMMAND --help` for one command.

## Contributing and security

[CONTRIBUTING.md](CONTRIBUTING.md) describes how to build, test and submit changes. [SECURITY.md](SECURITY.md) describes how to report a vulnerability, and [docs/security.md](docs/security.md) says what the service protects and what it does not.

## Licence

Apache-2.0, in [LICENSE](LICENSE).
