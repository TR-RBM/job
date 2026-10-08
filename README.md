# job

[![CI](https://github.com/TR-RBM/job/actions/workflows/ci.yml/badge.svg)](https://github.com/TR-RBM/job/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/TR-RBM/job)](https://github.com/TR-RBM/job/releases/latest)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![Rust 1.98+](https://img.shields.io/badge/rust-1.98%2B-orange)](Cargo.toml)
[![Platform: Linux](https://img.shields.io/badge/platform-Linux%20x86--64%20%7C%20ARM64-lightgrey)](#installation)

**job is an open source execution and scheduling application for Linux.** Run commands, organize work into queues and groups, and control when and how processes execute. Use job from a terminal, a script, or an automated system.

A command given to job becomes a Job: a service starts it, keeps it running independently of the terminal it came from, and records what it printed and how it ended. You can leave and come back, wait for it from a script, read its output while it is written, and hold, change, retry or cancel it.

Beyond running it, you decide for one Job, or for a whole Queue or Group of them:

- **When it runs.** At once, held until you release it, after the work ahead of it, by priority, or as a fair share between teams, with a ceiling on how many run together.
- **With how much.** Reserved and limited CPU, memory and device I/O, the CPUs and the NUMA policy it uses, and its process limits, changeable while it runs.
- **In what surroundings.** Namespaces of its own in which it sees only its own processes, a read-only file system, a private temporary directory, no new privileges, fewer capabilities, and system calls it may not make.
- **On which network.** The host's, none at all, only through a proxy or a WireGuard tunnel, or a network the administrator prepared and named, each with an optional bandwidth ceiling.
- **On which machine.** The host the service runs on, or another host that runs job and is reached over SSH.
- **What is kept.** Standard output and standard error apart with a size you choose, every attempt with its outcome, a terminal you can detach from and return to, the events of every Job, and a journal of who changed what.

Each of these is asked for, by an option on the Job or a setting on its Queue or Group. A plain `job run -- command` adds none of them: no reservation, no limit, no sandbox.

This release runs work on one host and on explicitly named hosts over SSH. It does not place work across a cluster and has no calendar scheduling.

## How it is built

A service, `jobd`, starts each command under its own supervisor process and keeps its records on disk; a Job that is running goes on when the service is restarted, and is picked up again. The `job` command talks to that service over a Unix socket and answers as text, as JSON or as tab-separated lines, so a script or another program uses the same command a person does. One user can run a service for themselves, or one service can be shared by a Unix group whose members trust each other.

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

A Job is in one of ten states: `held`, `queued`, `starting`, `running`, `suspended`, `stopping`, `succeeded`, `failed`, `cancelled` or `lost`.

## What you can do, and where it is described

Each line names a task, shows how it looks, and links to the page that explains it in full. Everything after the first two rows is optional and off until asked for.

### Running and following work

| Task | For example | Described in |
|---|---|---|
| Run a command and wait, or submit it and leave | `job run -- make`, `job submit -- make` | [Commands, shells and waiting](docs/cli-execution.md) |
| Wait for one Job or several, with a time limit | `job wait 42`, `job wait 42-45 --timeout 10m` | [Commands, shells and waiting](docs/cli-execution.md) |
| Read output while it is written, stdout and stderr apart | `job logs --follow 42`, `job logs 42 --stream stderr` | [Output recording and live clients](docs/output-recording.md) |
| Limit how much output is kept | `job run --output-tail 4M -- make` | [Output recording and live clients](docs/output-recording.md) |
| Work in a program that keeps its terminal | `job run --pty -- bash`, `job attach 42` | [User guide](docs/user-guide.md) |
| Hold a Job, change it, release it, retry it | `job create -- make`, `job edit 42 -- make all`, `job release 42`, `job retry 42` | [User guide](docs/user-guide.md) |
| Cancel, signal, suspend and continue | `job cancel 42`, `job signal -s HUP 42`, `job suspend 42` | [Cancel Jobs and collections](docs/cancellation.md) |
| Act on many Jobs at once | `job cancel 1-10`, `job release 1,4,7-9`, `job cancel --dry-run 1-10` | [Commands, shells and waiting](docs/cli-execution.md) |
| Delete records that are no longer needed | `job remove 1-10` | [Removing execution records](docs/record-removal.md) |
| Run on another host over SSH | `job run --on user@host -- make` | [User guide](docs/user-guide.md) |

### Finding out what happens and why

| Task | For example | Described in |
|---|---|---|
| List Jobs, including ended ones, by state or label | `job list --all --state failed`, `job list --label team=build` | [Commands, shells and waiting](docs/cli-execution.md) |
| See one Job with its attempts | `job show 42`, `job attempts 42` | [User guide](docs/user-guide.md) |
| Ask why a Job waits, or where a setting comes from | `job explain 42`, `job explain development/builds` | [Admission ordering](docs/scheduling.md) |
| Follow state changes as they happen | `job events --follow` | [Operating the service](docs/operations.md) |
| See who changed what | `job audit` | [Reliability](docs/reliability.md) |
| Check the host and the service | `job doctor`, `job host` | [Operating the service](docs/operations.md) |
| Read answers from a script | `job list --format json`, `job list --format tsv` | [Commands, shells and waiting](docs/cli-execution.md) |

### Organizing and ordering work

| Task | For example | Described in |
|---|---|---|
| Build a tree of Queues and Groups, with labels | `job group create development`, `job queue create development/builds` | [User guide](docs/user-guide.md) |
| Stop new starts or new submissions for a subtree | `job queue pause development/builds`, `job group close development` | [User guide](docs/user-guide.md) |
| Limit how many Jobs run at once | `job queue set development/builds --max-running 4` | [User guide](docs/user-guide.md) |
| Move a waiting Job to another Queue | `job move 42 --queue development/tests` | [Commands, shells and waiting](docs/cli-execution.md) |
| Order waiting work by priority, aging, strict order or fair share | `job submit --priority 100 -- make` | [Admission ordering](docs/scheduling.md) |
| Name a set of settings once and reuse it | `job submit --execution-profile build@2 -- make` | [Execution profiles and scheduling classes](docs/execution-profiles.md) |

### Resources

| Task | For example | Described in |
|---|---|---|
| Reserve, limit and weight CPU, memory and device I/O | `job run --memory-max 4G --cpu-limit 2 -- make` | [Resource requests and controls](docs/resources.md) |
| Give a Queue or Group a shared ceiling, or defaults for its Jobs | `job queue set development/builds --job-memory-max 4G` | [Resource requests and controls](docs/resources.md) |
| Change limits while work runs | `job update 42 --memory-max 8G` | [Resource requests and controls](docs/resources.md) |
| Hold new starts while the host is under pressure | rules in the service configuration | [Pressure observation and admission](docs/pressure.md) |
| Choose CPUs, a NUMA policy and process limits | `job run --cpu-affinity 0-3 --rlimit nofile=1024 -- make` | [CPU placement, NUMA and process limits](docs/process-controls.md) |

### Containment

| Task | For example | Described in |
|---|---|---|
| Forbid gaining privileges, drop capabilities, refuse system calls | `job run --no-new-privs yes --cap-drop all --seccomp-deny ptrace -- cmd` | [no_new_privs, capabilities and seccomp](docs/security-controls.md) |
| Limit where a Job may write | `job run --confine -- cmd` | [Security model](docs/security.md) |
| Give a Job its own namespaces, a read-only root, a private tmp | `job run --namespaces user,mount,pid --root read-only --private-tmp yes -- cmd` | [Namespaces, read-only root and private tmp](docs/isolation.md) |
| Cut a Job off from the network, or send it through a proxy or a tunnel | `job run --net none -- cmd` | [Networks, their boundaries and proxy credentials](docs/networking.md) |
| Send everything a Job sends through Tor, and nothing any other way | `job run --net socks5://127.0.0.1:9050 -- curl https://check.torproject.org/api/ip` | [Example: a Job whose traffic goes through Tor](docs/networking.md#example-a-job-whose-traffic-goes-through-tor) |
| Use a network the administrator prepared and named | `job run --net profile:updates -- cmd`, `job net list` | [Networks, their boundaries and proxy credentials](docs/networking.md) |
| Cap bandwidth for a Job or a Queue | `job run --bandwidth 10Mbit -- cmd` | [Networks, their boundaries and proxy credentials](docs/networking.md) |

What these controls protect against, and what they do not, is in the [security model](docs/security.md). In short: every client admitted to the service has equal control, and two Jobs of the same Unix user are not protected from each other by default.

### Running the service

| Task | For example | Described in |
|---|---|---|
| Install, start under a service manager, delegate a cgroup | `make install`, `jobd` | [Administration and installation](docs/administration.md) |
| Configure the service | `job config check FILE`, `job config show` | `job.conf(5)`, [Administration and installation](docs/administration.md) |
| Share one service with a Unix group | `[socket] group` in the configuration | [Administration and installation](docs/administration.md) |
| Back up and restore the state, or convert it from an earlier version | `job state backup --source DIR --destination DIR` | [State migration and restoration](docs/migration.md) |
| Know what survives a crash or a restart | | [Reliability](docs/reliability.md) |
| Know how fast it is and what bounds it | | [Performance](docs/performance.md) |
| Submit safely from a script that may repeat itself | `job submit --idempotency-key build-1234 -- make` | [Reliability](docs/reliability.md) |
| Use job from another program | a diagnostic summary, a command policy file, a hook | [Automation](docs/integrations.md) |
| Write a program that shows and steers the service while it runs | one connection: a snapshot, then every change as it happens, listings, output, and job's commands as words | [Client interface](docs/client-interface.md) |

The [capability matrix](docs/capabilities.md) says for each capability how to request it, what it needs from the host, what happens when the host lacks it, and where it is tested. It also lists what is not available. The [release notes](docs/release-notes.md) say what is new, what changed incompatibly, and the [known limits](docs/release-notes.md#known-limits) of this release.

## Installation

Every [release](https://github.com/TR-RBM/job/releases/latest) carries these files for x86-64 and ARM64, and `SHA256SUMS` beside them:

| System | File | Install |
|---|---|---|
| Debian, Ubuntu | `job_<version>-1_<arch>.deb` | `sudo apt install ./job_*.deb` |
| Fedora, RHEL | `job-<version>-1.<arch>.rpm` | `sudo dnf install ./job-*.rpm` |
| Arch Linux, Artix | `job-<version>-1-x86_64.pkg.tar.zst` | `sudo pacman -U job-*.pkg.tar.zst` |
| Any Linux | `job-v<version>-<arch>-linux.tar.gz` | unpack, then `sudo make install` in the unpacked folder |

The binaries are linked statically against musl and run on any distribution. musl reads users and groups from `/etc/passwd` and `/etc/group` only, so a socket group that exists only in a directory service such as LDAP or SSSD is named by its number in `job.conf`, or job is built from source. The packages install under `/usr`, the archive under `/usr/local` or `PREFIX`, with the same files as an installation from source; `make uninstall` in the unpacked folder removes them.

From source, with stable Rust 1.98 or later and `make`:

```sh
make build
make install                      # PREFIX=/usr/local
make install PREFIX=$HOME/.local  # a private installation
job doctor
```

`DESTDIR` stages the same tree elsewhere for a package, as in `make install PREFIX=/usr DESTDIR=/tmp/pkg`. `make install` puts `job` and the service entry point `jobd` in `bin`; the manuals `job(1)`, `job.conf(5)`, `job(7)` and `jobd(8)` under `share/man`; Bash and fish completion; a systemd system unit and a systemd user unit; and under `share/doc/job` the licence, the administration guide, the migration guide and a runit run script. The other guides are read from the source tree. It creates no user, enables no service and changes no kernel setting. `make uninstall` removes exactly what was installed.

`job doctor` checks the host and the service setup and says what to do for each finding. It works without a running service.

What the host needs: Linux on x86-64 or ARM64 with the unified cgroup hierarchy. Enforced limits, suspension and I/O control need a cgroup delegated to the service; without one the service runs and says that limits are watched, not enforced. The systemd units are shipped but were not yet run under systemd; the [administration guide](docs/administration.md) has a checklist for the first use.

## Documentation

Guides:

- [User guide](docs/user-guide.md): the path through the product, with links to the topic pages.
- [Administration and installation](docs/administration.md) and [state migration and restoration](docs/migration.md).
- [Operating the service](docs/operations.md), [reliability](docs/reliability.md) and [performance](docs/performance.md).
- [Security model](docs/security.md) and [capability matrix](docs/capabilities.md).
- [Release notes](docs/release-notes.md).

Topic pages:

- [Commands, shells and waiting](docs/cli-execution.md)
- [Output recording and live clients](docs/output-recording.md)
- [Cancel Jobs and collections](docs/cancellation.md) and [removing execution records](docs/record-removal.md)
- [Admission ordering](docs/scheduling.md) and [execution profiles and scheduling classes](docs/execution-profiles.md)
- [Resource requests and controls](docs/resources.md)
- [Pressure observation and admission](docs/pressure.md) and [comparing pressure policies](docs/pressure-benchmark.md)
- [CPU placement, NUMA and process limits](docs/process-controls.md)
- [no_new_privs, capabilities and seccomp](docs/security-controls.md)
- [Namespaces, read-only root and private tmp](docs/isolation.md)
- [Networks, their boundaries and proxy credentials](docs/networking.md)
- [Automation](docs/integrations.md)
- [Client interface: the service's protocol for interface programs](docs/client-interface.md)

Reference:

- [Syntax reference](docs/reference/syntax.md): every command and option, generated from the program.
- Manuals: `job(1)` for commands, `job.conf(5)` for configuration, `job(7)` for concepts, `jobd(8)` for the service. From a checkout, `man -l man/job.1` shows a page without installing.
- `job help`, and `job COMMAND --help` for one command, which also lists its exit statuses.

## Contributing and security

[CONTRIBUTING.md](CONTRIBUTING.md) describes how to build, test and submit changes. [SECURITY.md](SECURITY.md) describes how to report a vulnerability, and [docs/security.md](docs/security.md) says what the service protects and what it does not.

## Licence

Apache-2.0, in [LICENSE](LICENSE).
