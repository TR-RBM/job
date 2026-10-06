# Administration and installation

Run `job doctor` first and after every step: it checks the host and the service setup, prints one line per check with `ok`, `warn` or `fail`, and says what to do. It works without a running service and changes nothing. `jobd(8)` is the reference for everything on this page.

Each step says what it changes, whether it needs root, how to check it, what happens without it, and how to undo it. Steps 1 to 5 are needed for enforcement; without steps 3 and 4 the service still queues and answers, in monitoring mode, and every answer says `limits watched, not enforced`. The steps were carried out on one host: Artix Linux, kernel 7.2, runit, btrfs.

### 1. Kernel: process handles and cgroup v2

The service requires pidfd_open and pidfd_send_signal (Linux 5.3 or newer, with the system calls allowed). It probes them before accepting work. `job host --json` reports the capability. Running records from versions without a supervisor boot identity must be drained under that version before an upgrade; they cannot be adopted by guessing a process identity.

- **Change:** none on a current distribution. cgroup v2 must be mounted at `/sys/fs/cgroup` (unified hierarchy), and the root cgroup must pass `cpu`, `io`, `memory` and `pids` to its children. The service uses `cgroup.kill` (Linux 5.14) and `memory.peak` (5.19), and reads `pids.peak` where the kernel has it.
- **Check:** `stat -fc %T /sys/fs/cgroup` prints `cgroup2fs`; `cat /sys/fs/cgroup/cgroup.subtree_control` lists `cpu io memory pids`.
- **If a controller is missing (root):** `echo "+cpu +io +memory +pids" > /sys/fs/cgroup/cgroup.subtree_control`. A host booted with the legacy or hybrid hierarchy needs `systemd.unified_cgroup_hierarchy=1` (systemd) or a cgroup2 mount at boot.
- **Without it:** monitoring mode.

### 2. Build and install (root for a system-wide prefix)

Before replacing an installation of the earlier tool, read what is [open before release](plans/implementation-status.md#open-before-release) and follow the [migration guide](migration.md). State is never converted in place.

- **Change:** `make build` (Rust 1.98 or later), then `make install`. `PREFIX` defaults to `/usr/local`; `DESTDIR` stages the same tree elsewhere for a package, as in `make install PREFIX=/usr DESTDIR=/tmp/pkg`. `install` does not build: it installs `target/release/job`, or the file named by `JOB_BIN`.
- **What it installs:** `bin/job` and the link `bin/jobd`; `share/man/man1/job.1`, `man5/job.conf.5`, `man7/job.7`, `man8/jobd.8`; `share/bash-completion/completions/job` and `share/fish/vendor_completions.d/job.fish`; `lib/systemd/system/jobd.service` and `lib/systemd/user/jobd.service`; and under `share/doc/job/` this guide, the migration guide, `LICENSE` and the runit example `runit/run`. `tools/install-manifest` is the complete list with modes.
- **What it does not do:** it creates no user or group, enables and starts no service, and sets no kernel parameter. Those are the steps below, each on purpose.
- **Replacing a running installation:** safe. The service starts every Job's supervisor from its own image, so it keeps starting Jobs of the old version until it is restarted; running Jobs survive that restart and are adopted. `job doctor` reports a service and command of different versions.
- **Check:** `job help` prints the usage; `man jobd` opens the manual; `job doctor`.
- **Undo:** `make uninstall` with the same `PREFIX` and `DESTDIR` removes exactly the installed files, after step 3 is undone. State and configuration are not touched.
- **Packaging:** build with `make build`, stage with `make install PREFIX=/usr DESTDIR=...`, and leave user creation, enabling and sysctl settings to the administrator. On a distribution that keeps units in `/usr/lib/systemd` nothing else is needed.

### 3. The service and its delegated cgroup

The service is `jobd`. It runs in the foreground, logs to standard error and executes all work as its own Unix user. There are two ways to run it.

| Purpose | User service | System service (`jobd --system`) |
|---|---|---|
| Configuration | `$XDG_CONFIG_HOME/job/config.toml` | `/etc/job/config.toml` |
| State | `$XDG_STATE_HOME/job/` | `/var/lib/job/` |
| Socket | `$XDG_RUNTIME_DIR/job/job.sock` | `/run/job/job.sock` |
| Cache (unused so far) | `$XDG_CACHE_HOME/job/` | `/var/cache/job/` |

The XDG fallbacks apply (`~/.config`, `~/.local/state`, `~/.cache`). `JOB_CONFIG`, `JOB_STATE_DIR`, `JOB_RUNTIME_DIR` and `JOB_CACHE_DIR` override one location each. Where no location can be derived, the command fails and names the variable to set; nothing falls back to `/tmp`.

**Where the socket is.** The first rule that applies decides:

1. `JOB_RUNTIME_DIR` is set: `job.sock` in it.
2. `JOB_STATE_DIR` is set: `daemon.sock` inside that state directory. A state directory relocated on purpose never shares the socket of the account's ordinary service; this is also what keeps test services apart.
3. System service: `/run/job/job.sock`.
4. `XDG_RUNTIME_DIR` is set: `$XDG_RUNTIME_DIR/job/job.sock`.
5. Otherwise (runit, no login session): `daemon.sock` in the state directory, as in earlier versions.

Clients apply the same rules, then also try `daemon.sock` in their state directory. A client of a user service with neither `XDG_RUNTIME_DIR` nor an override, such as a login over ssh without a session manager, also tries `/run/user/UID/job/job.sock`. In every other case the service and its clients must see the same variables. The lock `daemon.lock`, the records, the working directories of remote Jobs (`remote-work`) and the terminal sockets stay in the state directory: a remote Job's directory is its working directory and is named in its record, so it is not disposable cache.

**Which cgroup it manages.** The service takes the first that applies and names the rule in its startup line, in `job doctor` and in `job host --json`: an explicit root (`JOB_CGROUP_ROOT`, or `root` under `[cgroup]` in the configuration); else the delegated cgroup it was started in; else the earlier fixed path `/sys/fs/cgroup/exec`. A root must hold no process but the service. An explicit root that holds others stops the start; a discovered one is skipped and the service runs in monitoring mode instead, saying why.

**Not yet run under systemd; treat as a starting point.** The two unit files were written from systemd's documentation and installed by `make install`, and no systemd has ever loaded them: the project's hosts run runit, and neither `systemd-analyze verify` nor the systemd manual pages were available where the release was qualified. `job doctor` therefore reports `service_manager` as `warn` on a systemd host, in this release, whatever else it finds. The service relies on these directives behaving as described below:

| Directive | What the service relies on |
|---|---|
| `Delegate=yes` | the unit's cgroup is handed to the service account, which may create child cgroups and enable controllers in it |
| `DelegateSubgroup=daemon` (systemd 254 or later) | the service process is placed in `daemon/` below the unit's cgroup, so the delegated cgroup itself holds no process and Jobs get siblings of `daemon/` |
| `KillMode=process` | a stop or restart signals only the service process; supervisors and Jobs in the same unit cgroup go on |
| `RuntimeDirectory=job` | `/run/job` exists with the service account as owner before the service starts. By default systemd removes it when the unit stops and creates it again on start, which is harmless here because only the client socket lives in it; records and terminal sockets are in the state directory |
| `StateDirectory=job` | `/var/lib/job` exists, owned by the service account, and is kept across stops |

Verify once on first use, system service shown (add `--user` and drop `--system` for the user unit):

1. `systemctl enable --now jobd`, then `systemctl status jobd`: active, with one process in the `daemon` subgroup.
2. `job doctor --system`: no `fail`; `cgroup_root` names the delegated cgroup and `enforcement` says cgroup. The `service_manager` warning is expected.
3. `job --system run -- sleep 300 &`, then `job --system list`: the Job is running, and `systemd-cgls -u jobd` shows it beside `daemon`.
4. `systemctl restart jobd`.
5. `job --system list`: the same Job is still running with the same start time, and `job --system wait ID` returns its own status when it ends. If the Job was killed by the restart, `KillMode=process` did not take effect; if the service came back watching processes instead of enforcing, the delegation did not survive the restart. Stop here and report it.

**systemd, system service** (root):

- **Change:** create the account the unit names, `useradd --system --home-dir /var/lib/job --shell /usr/sbin/nologin job`, or change `User=` and `Group=` in a drop-in (`systemctl edit jobd`); then `systemctl enable --now jobd`.
- **What the unit does:** `ExecStart=PREFIX/bin/jobd --system`; `RuntimeDirectory=job` and `StateDirectory=job` create `/run/job` and `/var/lib/job` for that account; `Delegate=yes` hands the unit's cgroup to it and `DelegateSubgroup=daemon` (systemd 254 or later) places the service process in `daemon/` below it, which is the layout the service discovers. No `JOB_CGROUP_ROOT` is set. `KillMode=process` makes a stop or restart end the service only, so running Jobs survive and are adopted; stopping the unit does not stop Jobs. `TasksMax=infinity` avoids an implicit per-unit task ceiling. Enclosing slice and host limits still apply.
- **Clients:** other accounts write `job --system COMMAND` or set `JOB_SYSTEM=1`, and need the socket group below.
- **Check:** `systemctl status jobd`; `job doctor --system`; `job --system queue` (the option before the command selects the system service for one invocation, as `JOB_SYSTEM=1` does).
- **Undo:** `systemctl disable --now jobd`. Jobs keep running; cancel them first.

**systemd, user service** (no root):

- **Change:** `systemctl --user enable --now jobd`. With a private prefix, `make install PREFIX=$HOME/.local` and link `~/.local/lib/systemd/user/jobd.service` into `~/.config/systemd/user/`.
- **What the unit does:** the same delegation and stop behaviour, and `JOB_RUNTIME_DIR=%t/job`, so the socket is `$XDG_RUNTIME_DIR/job/job.sock`.
- **Lingering:** a user service ends with the account's last session. Root runs `loginctl enable-linger USER` once to keep it; `job doctor` reports whether it is enabled.
- **Check:** `job doctor`; `job run -- true`.
- **Undo:** `systemctl --user disable --now jobd`.

**runit** (root):

- **Change:** `install -d /etc/runit/sv/job && install -m 0755 PREFIX/share/doc/job/runit/run /etc/runit/sv/job/run`, write `/etc/runit/sv/job/conf` with at least `SERVICE_USER=name`, then `ln -s /etc/runit/sv/job /etc/runit/runsvdir/default/job`. The script names no user itself. `conf` may also set `SERVICE_GROUP`, `SOCKET_GROUP` (an extra group for the socket group below), `CG` (default `/sys/fs/cgroup/job`) and `BIN`.
- **What the run script does at every start:** creates the cgroup `CG` and `CG/daemon`, hands their `cgroup.procs`, `cgroup.subtree_control` and `cgroup.threads` to the service user, moves itself into `CG/daemon`, and starts `jobd --foreground` as that user with `HOME` set. The service discovers `CG` from its own position. It imposes no resource caps.
- **Check:** `sv status job` prints `run`; `job doctor`; `job queue` prints `limits enforced by cgroup; pool …` as its first line.
- **Undo:** `rm -i /etc/runit/runsvdir/default/job`, then remove the empty cgroup directories once no Job runs.
- **An existing installation** that uses `/sys/fs/cgroup/exec` and `job daemon` keeps working: `job daemon` remains as an alias of `jobd`, and that path is found by discovery or as the legacy fallback.

**Without a service manager:** run `jobd` in a terminal as your own user. Started from a shell it usually finds no cgroup of its own and watches processes instead of enforcing limits.

**Monitoring mode:** a service without a delegated cgroup watches processes and enforces no limit. It says so in its start line, in `job host`, in `job doctor` (check `enforcement`, a warning) and in every Job answer ("limits watched, not enforced"). To forbid that mode, set

```toml
[cgroup]
required = true
```

and the service refuses to start without a usable delegated cgroup, naming what is missing. With the default `required = false` and the ordinary profile, a Job that asks for `--mem`, `--pids` or a cgroup control is refused with a capability error; the legacy profile keeps watching `--mem` and `--pids`. See [operations](operations.md).

### Sharing the socket with a Unix group

By default only the service user is admitted: the runtime directory is 0700, the socket 0600, and the service refuses any other peer. To share it, set in the configuration

```toml
[socket]
group = "jobusers"
```

and restart the service. The runtime directory becomes 0750 and the socket 0660, both owned by the group, and a peer is admitted when it is the service user or its primary or supplementary groups contain the group. The service user must be a member of the group. An unknown group stops the start, and so does a socket that would lie in the private state directory: use a runtime directory (`--system`, `XDG_RUNTIME_DIR` or `JOB_RUNTIME_DIR`).

Every admitted client has the same control over every Job, Queue, Group and terminal. Work still runs as the service user. Records that name an actor, such as a priority change, store the peer's user ID.

The state directory stays 0700. A client that cannot read it asks the service: `job run`, `job logs` and `job log` receive the retained output of one attempt over the control socket, read-only and with follow, and `job attach` connects to the Job's terminal socket, which a service with a socket group places in `terminals/` below the runtime directory (directory 0750, socket 0660, both owned by the group). `job audit` and `job events` read the state directory directly and stay with the service user. `job doctor` shows which path applies to the caller.

### Suspend and continue existing work

`job host --json` reports freezer interface availability. `job suspend ID` and `job continue ID` require a local cgroup workload. Success means the kernel confirmed the requested effective state. A service in monitoring mode and local proxies for remote Jobs refuse the operation. Recursive collection controls require `--recursive` and return per-Job results; they can partially succeed. Queue/Group pause remains an independent admission hold.

A confirmation timeout returns 75 and leaves accepted intent active. Check `job status ID --json` for `suspension.requested`, `pending`, `error` and `timing`. An inherited freeze can prevent continuation; job does not thaw an administrator's ancestor cgroup. Suspend retains resources and elapsed deadlines. Cancel requests thaw before TERM, then uses the existing escalation deadline. Restart with the same delegated cgroup root when suspended work remains; otherwise recovery refuses to silently abandon its control state. A completed supervisor result can still be collected after its workload cgroup disappears.

Elapsed and active durations are sampled wall-clock measurements. Active duration subtracts observed suspended intervals; CPU consumption is reported separately in usage. No application checkpoint or migration is implied by suspension.

### 4. Configuration and upgrades

Fresh state uses the ordinary service profile. An unconfigured Job receives no synthetic reservation, process limit, swap restriction or pressure policy. The service still tracks its lifecycle and output. Explicit resource requests participate in admission. The earlier options `--mem`, `--pids`, `--disk` and `--cores` keep their combined request and control meanings; `--pids-max` and `--write-budget` are the separate names. See [resource controls](resources.md#compatibility-and-recovery), a page of the source tree.

Use `job config init --profile ordinary PATH` or `job config init --profile legacy PATH` to create a configuration without overwriting an existing file. Set `JOB_CONFIG=PATH` in the service environment. Run `job config check PATH` before starting it, and `job config show --json` to inspect the running configuration and its source. See [job.conf(5)](../man/job.conf.5).

Legacy state requires an explicit profile. Choose `legacy` to preserve automatic estimates, backfilling, process defaults and emergency host protection. Any active or queued records must retain their original profile through recovery. To change profile, finish or cancel those Jobs, stop the service, update configuration and restart. `job config reload` validates and reloads the same profile only while drained; profile changes require a restart. Use the [offline migration and restore workflow](migration.md) to validate, back up and convert existing state. A conversion of the state of a real installed service was not performed during development; keep the backup the conversion makes.

The ordinary profile does not raise or remove limits imposed by a parent cgroup, service unit, shell or administrator. Check those separately. Configuration discovery uses `JOB_CONFIG`, then `$XDG_CONFIG_HOME/job/config.toml`, then `~/.config/job/config.toml`; a system service reads `/etc/job/config.toml`. A missing explicitly named file is an error. `job doctor` validates the file the service would read. The socket group and the cgroup root change only by a restart, not by `job config reload`.

### 5. The state directory

Default discovery refuses legacy exec state or simultaneous exec/job stores. Migrate explicitly and select the converted directory with JOB_STATE_DIR while the original remains present. New private state roots are owner-only.

- **Change:** none. The service keeps records, retained output and history in `$XDG_STATE_HOME/job`, by default `~/.local/state/job` of the service user, or in `/var/lib/job` for a system service, so they survive the end of that user's last login, which removes `/run/user/<uid>` and the socket in it. How much output is kept per Job is set by its output quota, and the service keeps 1 GiB in all unless `budget_bytes` under `[output]` in the configuration says otherwise; see [output recording](output-recording.md), a page of the source tree.
- **Check:** `job doctor` prints the state directory, its schema version, whether the lock is held, and the socket with its owner, group and mode.
- **Journals:** `audit/` holds the requests that changed something (`job audit`), `events/` the state changes of Jobs, Queues and Groups (`job events`). Both rotate at 8 MiB and keep eight files unless `[audit]` or `[events]` in the configuration say otherwise; both are part of a backup. `job host` and the `health` check of `job doctor` report whether they can be written and how much space the state directory has left.

### 6. A command policy (optional)

- **Change:** none unless you want one. job ships no rules about which commands may run. A command policy is a file you write, `policy.json`, that `job hook` and `job policy` consult; without it nothing is forbidden. Nothing else in this guide depends on it.
- **Root:** no for a user's file, `~/.config/job/policy.json` (or under `$XDG_CONFIG_HOME/job`). Yes for the system's file, `/etc/job/policy.json`, which applies to every user who has no file of their own:

  ```sh
  install -d ~/.config/job
  install -m 0644 policy.json ~/.config/job/policy.json
  ```

- **Check:** `job policy --show` prints the file in use and the rules in force, or the reason the file is refused. `job doctor` has the same as its `policy` check.
- **Takes effect:** with the next command that is checked. The file is read at every call; the service does not read it and nothing is reloaded.
- **Undo:** remove the file.

The format, the rules and the hook that applies them are described in [automation](integrations.md), a page of the source tree, and in `job.conf(5)`.

### 7. For bandwidth caps (optional)

- **Change:** the packages that provide `unshare` and `nsenter` (util-linux), `ip` and `tc` (iproute2), `nft` (nftables) and `slirp4netns`; the kernel must let an unprivileged user create user namespaces. No root at run time.
- **Check:** `job run --bandwidth 2Mbit -- true` answers `network capped at 2 Mbit/s each way`.
- **Without it:** a job or queue with a bandwidth is refused at submission, naming what is missing; everything else works.

### 8. Network namespaces and routing profiles (optional)

Both are named in the service configuration and selected by clients with `--net ns:NAME` and `--net profile:NAME`. Every client admitted to the service can select every name, so name only what all of them may use. The keys are described in [networking](networking.md#an-existing-network-namespace).

**A namespace for a service that runs with privilege.** Make it the usual way and name it:

```sh
ip netns add lab
ip -n lab link set lo up
```

```toml
[network.namespaces.lab]
description = "the measurement bench"
```

The file is `/run/netns/lab`. A service without `CAP_SYS_ADMIN` in the initial user namespace cannot join it and says so; nothing was run as root during development, so this path is written from the kernel's rules and untested.

**A namespace for a service of an ordinary user.** The service's user makes a user namespace and a network namespace in it, keeps them alive with a process, and gives them stable names. An unprivileged user cannot bind-mount namespace files where other processes see them, so the names are symbolic links kept by the same script that starts the holder:

```sh
mkdir -p ~/.local/state/job-netns
unshare --user --map-root-user --net sh -c 'ip link set lo up; exec sleep infinity' &
holder=$!
ln -sfn /proc/$holder/ns/net  ~/.local/state/job-netns/lab.net
ln -sfn /proc/$holder/ns/user ~/.local/state/job-netns/lab.user
printf 'nameserver 192.0.2.53\n' > ~/.local/state/job-netns/lab.resolv.conf
```

```toml
[network.namespaces.lab]
path = "/home/me/.local/state/job-netns/lab.net"
user_namespace = "/home/me/.local/state/job-netns/lab.user"
resolv_conf = "/home/me/.local/state/job-netns/lab.resolv.conf"
```

Run the holder under the same supervisor as the service, and renew the links whenever it restarts: a link that points at an ended process makes the namespace unjoinable, which `job net list` shows, and a link left pointing at a process number that was given to another process would name that process's namespace. The configuration itself refuses a `path` under `/proc` for that reason; the link is the administrator's statement that the script keeps it true.

**A routing profile.** This one lets its Jobs reach one mirror over HTTPS and one resolver, 20 Mbit/s for all of them together:

```toml
[network.profiles.updates]
description = "package mirror only"
egress = "host"
allow = ["192.0.2.10:443/tcp", "192.0.2.53:53"]
dns = ["192.0.2.53"]
bandwidth = "20Mbit"
sharing = "shared"
```

Then:

```sh
job config check ~/.config/job/config.toml
job config reload          # needs a drained service; otherwise restart it
job net list
job net show updates
job run --net profile:updates -- curl -sS https://192.0.2.10/
job queue set mirror --net profile:updates
```

- **Check:** `job net show updates` prints the packet filter in order and ends its first line with `enforceable`; `job doctor` has the checks `network_namespaces` and `network_profiles`.
- **Needs:** for a profile that filters, shapes or routes, the tools of section 7. A plain `egress = "host"`, `"none"` or `"ns:NAME"` needs none of them.
- **Changing or removing:** edit the file and reload or restart. A Job that waited uses the definition current when it starts; if its profile is gone it ends with a start error that names it. A reload or a start is refused while a Queue, a Group or an execution profile still names what was removed.
- **Credentials:** a proxy's user and password go into `secret_file`, a file of the service user with mode 600, never into `egress`.

### 9. Optional

- **earlyoom**, where it runs, kills at its own thresholds; the legacy profile's host floor of 12 % available memory lies above earlyoom's 6 % so that the service, which knows which job to stop and says why, acts first.
