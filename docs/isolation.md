# Namespaces, read-only root and private tmp

All settings on this page are optional. A new Queue has none, and an ordinary Job without them starts exactly as before.

```sh
job host
job run --namespaces user,uts,ipc -- command
job run --namespaces user,mount --root read-only --writable /srv/out -- command
job run --namespaces user,mount --private-tmp yes -- command
job run --namespaces user,mount,pid -- command
job queue set research/builds --job-namespaces user,mount --job-root read-only
job queue unset research/builds job-root
```

`job host` prints what this host offers; `job host --json` has the same under `isolation_controls`.

## What each option does

`--namespaces LIST` starts the program in new namespaces of the named kinds: `user`, `mount`, `pid`, `ipc`, `uts`, `cgroup`. The list is the whole request. Nothing is added for you.

- `user`: a new user namespace in which your user and group keep their numbers. The program holds no capabilities in it once it runs. A service that runs without privilege needs `user` in every list, because only a new user namespace lets it create the others.
- `mount`: a private copy of the mount table. Mounts and unmounts inside do not reach the host. On its own it changes nothing the program can see.
- `ipc`: separate System V IPC objects and POSIX message queues.
- `uts`: a separate host name and domain name.
- `cgroup`: the Job's own cgroup appears as the root of the cgroup tree.
- `pid`: the Job's processes get process numbers of their own, see only each other, and all end when the Job ends. It needs `mount` in the same list, because the Job gets a `/proc` of its own. See [PID namespace](#pid-namespace).

`--root read-only` makes every mount the Job sees read-only: `/`, your home directory, the working directory, `/proc`, `/sys` and `/dev/shm` included. Writing to devices such as `/dev/null`, to pipes and to the recorded output still works. It needs `--namespaces user,mount`.

`--writable PATH,...` keeps the named paths writable under a read-only root. Each path is absolute, exists when the Job starts, and what the program writes there is written to the real place. Up to 32 paths; a path cannot contain a comma. It needs `--root read-only`. A path is resolved once, when the Job starts: if it is or passes through a symbolic link, the directory it leads to is the one made writable, and `result.isolation_controls.writable` names that directory, not the link. If the path changes between that moment and the mount, the Job does not start.

`--private-tmp yes` gives the Job an empty `/tmp` and `/var/tmp` of its own, both the same directory. Nothing the Job writes there reaches the host's `/tmp`, and the directory is deleted when the Job ends, also where the Job took its own write permission away from directories in it. It needs `--namespaces user,mount`. A working directory under `/tmp` or `/var/tmp` is refused together with it at submission, because the Job would otherwise still stand in the host's directory; choose `--dir` elsewhere. Each attempt gets a directory of its own (`jobs/ID/tmp-ATTEMPT` in the state directory). If it cannot be deleted, for example because it is nested deeper than 128 directories, it is moved to `private-tmp-leftovers/` in the state directory, the Job's result carries a note that names the place, and a retry starts all the same. Nothing deletes that directory later; remove it by hand.

`inherit` as a value takes no setting and suppresses a Queue or Group default.

## What they do not do

- A mount namespace is not a container. The Job sees the whole host file system, read-only or not, every process unless `pid` is in the list, and the host's network unless `--net` says otherwise.
- Jobs of the same Unix user are not separated from each other. One Job can still signal another and read and write its files.
- A PID namespace hides processes; it is not a security boundary by itself. See below.
- The network namespace is not part of this list. Use `--net none` or a proxy or tunnel.
- A read-only root does not hide anything; every file stays readable as before.
- With `--confine`, the writable paths and the private tmp are added to what the Job may write. `--allow-write PATH` is not needed for them.
- With a read-only `/proc` a program cannot set up a user namespace of its own, because it cannot write its identity maps. A program that needs `/dev/shm` needs `--writable /dev/shm`.
- With a private tmp, anything that was in `/tmp` is hidden, including a `TMPDIR` below it. A writable path or a working directory under `/tmp` or `/var/tmp` is refused together with `--private-tmp yes`.
- Under a service that runs as an ordinary user the program cannot undo a read-only root: it holds no capabilities, and in a user namespace it creates itself the mounts are locked read-only. It can still mount a temporary file system over a path inside such a namespace of its own, which changes only what it sees.
- Under a service that runs as root the program is root in its user namespace and would keep the capability to remount `/` writable or unmount the private tmp. Such a service therefore refuses `--root read-only` and `--private-tmp yes` at submission unless the Job's capability reduction removes `sys_admin`: add `--cap-drop sys_admin` or `--cap-drop all`, on the Job or as a Queue default. This rule was not run on a root service.

## PID namespace

```sh
job run --namespaces user,mount,pid -- sh -c 'echo $$; ls /proc'
job signal -s USR1 ID
job cancel ID
```

With `pid` in the list the program runs in a PID namespace of its own.

What it gives:

- The program sees only the Job's processes. Its own number is small (2 for the command), `ps` and `/proc` list nothing else, and it cannot signal a process outside by number.
- Every process of the Job ends with the Job. When the Job is over, the kernel kills whatever is left in the namespace, including processes that detached themselves into the background.
- Orphaned processes are reaped, so a Job that starts many short-lived background processes leaves no zombies behind.

What it does not give:

- It is not a security boundary against the same Unix user. Processes outside still see the Job's processes under their host numbers and can signal them, and the Job still reaches every file, socket and pipe its user may open, the service's control socket included.
- It does not hide the host's file system, network or users. Combine it with the other options for that.
- It does not limit the number of processes. That is `--pids-max`.

How the Job is built:

```text
jobd                          the service
└─ job shim                   the supervisor, in the service's PID namespace
   └─ job-init                number 1 inside the Job's namespace
      └─ command              number 2 inside; leads its own process group
         └─ its children      orphans are adopted and reaped by job-init
```

`job-init` is part of job itself; nothing else is installed. It is a copy of the supervisor that never executes another program: it keeps one socket to the supervisor, closes everything else, and then only waits. It shows as `job-init` in `/proc/1/comm`; its command line is the supervisor's. It does four things:

- It starts the command as its child and tells the supervisor the command's exit status or fatal signal exactly as the kernel reported it. `job status ID --json` therefore shows the same `exit_code` and `signal` as without the namespace, and a program that could not be executed is the same start error.
- It reaps every orphan.
- It forwards a signal that a process inside the namespace sends to number 1 to the command's process group. A signal from the service is not forwarded: `job signal` and `job cancel` deliver to every process of the Job themselves, as they do without the namespace, so the command receives each signal once. `KILL` and `STOP` sent to number 1 from inside are ignored by the kernel.
- After the command has exited it stays until the supervisor has recorded which processes were left, and is then killed, which ends everything else in the namespace. If the supervisor itself is gone, it exits once the command has exited.

What stays the same:

- `job signal -s SIG ID` reaches the command and every other process of the Job. `job cancel ID` sends `TERM`, waits the grace period and kills the whole tree. `job suspend` and `job continue` freeze and thaw the Job's cgroup, `job-init` included.
- Processes still running when the command exits are counted and named in `result.leftover_processes` and `result.leftover_names` and then killed, as before. The one difference: helper programs that are otherwise kept (`sccache`, `gpg-agent`, `keyboxd`, `ssh-agent`, `dirmngr`) cannot outlive the namespace, so they are listed as leftover processes and `result.kept_helpers` stays empty.
- With `--pty` the command is the session leader and owns the terminal. Job control in a shell, Ctrl-C, Ctrl-Z and window size changes work as without the namespace.
- `job run --stdin`, both output streams and the recorded log are unchanged.
- The launch gate is unchanged: the supervisor does not create the namespace before the service has recorded it.

What differs:

- `job-init` is one more process in the Job's cgroup. It counts against `--pids-max` and appears in `usage.peak_pids`. It counts as one process of the user for `--rlimit nproc`.
- `--no-new-privs`, `--cap-drop`, `--seccomp-deny`, `--rlimit`, CPU and memory placement and `--confine` are applied to the command, after `job-init` exists. `job-init` is not under them, so a deny list cannot break it; while it waits it uses only `poll` (`ppoll` on aarch64), `read`, `wait4`, `kill`, `sendto` and `exit_group`. It is marked not dumpable and its memory belongs to the service's user namespace, so the command cannot read its memory, attach to it or read `/proc/1/environ`.
- Without `--pty` the command leads a process group in the session of `job-init` instead of the supervisor's session. Neither has a controlling terminal.
- `/proc` is mounted anew inside the Job and shows only the namespace. With `--root read-only` it is read-only like every other mount; `/proc/self` and reading work as before.
- `result.isolation_controls.pid_namespace` records `proc` (where the fresh `/proc` is mounted), `init_pid` (1) and `command_pid` (the command's number inside).

A Job in a PID namespace can run `job` itself against the service, because the control socket is a file it can still reach. The service sees the client's process number as the kernel translates it into the service's namespace, not the small number the client has inside, and records that one in the audit trail.

A service that runs without privilege needs `user` in the list as for every other kind. A host whose `/proc` is partly covered, as inside some containers, does not let a user namespace mount a fresh `/proc`; `job host` then reports that PID namespaces are not available, with the kernel's error under `pid_namespace_error`, and a Job that asks for `pid` is refused at submission.

## Order and failure

job enters the namespaces after the network setup and before `--confine`, placement, limits, capability reduction and seccomp. With `pid` the order inside that step is: create the namespaces, start `job-init`, make the mounts private, mount the fresh `/proc`, then the writable paths, the private tmp and the read-only root, then the readback, then start the command. With `--net none`, a proxy, a tunnel or a bandwidth limit the launch is already in a user namespace, and `user` then names that one instead of creating another; the result says so in `user_namespace_from_network`.

Before the program starts, job reads back that each requested namespace differs from the service's, that `/` is read-only and each writable path is not, and that `/tmp` is the Job's own directory. With `pid` it also reads back that the PID namespace differs from the service's, that `job-init` is number 1 and that the fresh `/proc` lists no other process. If the kernel refuses a step or a readback does not match, the program is not executed and the Job records a start error that names the step.

After the mounts job enters the working directory again by its path, so the program never keeps a handle on a directory from before them.

A request the host cannot meet is refused at submission and nothing is created: a namespace kind the kernel lacks, user namespaces switched off, a missing `mount_setattr`, `pid` without `mount`, a host that cannot mount a fresh `/proc`, a writable path that does not exist, a working directory that a private tmp would hide, a root service without the capability reduction named above.

## Inheritance, profiles and history

Queue and Group defaults use `--job-namespaces`, `--job-root`, `--job-private-tmp` and `--job-writable`. They are defaults: a Job may give its own value, or `inherit` to take none. `job queue unset PATH job-namespaces` removes a local default and restores what the ancestors supply. Each field inherits on its own, and the combination is checked when a Job is submitted: a Queue may hold `--job-root read-only` alone, and a Job there is refused until its namespaces include `user,mount`.

Pinned profiles accept the same fields:

```toml
[[presets.profiles]]
name = "read-only"
revision = 1
[presets.profiles.values]
namespaces = "mount,user"
root = "read-only"
private_tmp = true
writable = "/srv/out"
```

`job status ID --json` records where each value came from under `resource_sources` and what was applied under `result.isolation_controls`: `namespaces`, `user_namespace_from_network`, `root_read_only`, `private_tmp`, `writable` and, with `pid`, `pid_namespace`. A retry resolves the defaults current at that time; earlier attempts keep their own record.

## The host's capabilities

`isolation_controls` in `job host --json` holds:

- `namespace_kinds`: for `user`, `mount`, `ipc`, `uts`, `cgroup`, `net` and `pid`, whether the kernel has the kind.
- `requestable_namespaces`: the kinds `--namespaces` accepts on this host.
- `pid_namespace_requestable`: whether a Job can be started in a PID namespace here. It is found by trying once: a bounded child creates the namespaces and mounts a fresh `/proc`. `pid_namespace_error` holds the step and the kernel's error when it failed.
- `unprivileged_user_namespaces`, `mount_setattr` and `mount_setattr_error`.
- `landlock_abi`: the Landlock version behind `--confine`, or null.
- `missing_link_tools`: programs a proxy, tunnel or bandwidth network needs and this host lacks.
- `ipv6_in_linked_network`: false. A Job behind a proxy, tunnel or bandwidth limit has IPv4 only. A Job on the host's network has what the host has.
- `network_modes`: for `host`, `none`, `proxy`, `wireguard`, `bandwidth` and `openvpn`, whether the mode is available, whether the Job's network namespace is the host's or its own (`per_job`), and whether a link behind it exists (`per_queue_or_per_job`: one link for the whole Queue when the Queue sets the network or a bandwidth, otherwise one per Job).

## Remote execution

With `--on HOST` the controls are checked and applied on that host. The local SSH transport stays in the local service's namespaces.
