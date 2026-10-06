# Security model

This page says whom job trusts, what it protects, what it does not protect, and what each optional control adds. Read it before relying on job to keep one workload away from another.

The short form: job is a tool for people and programs that trust each other to share one service. It keeps their work organized and guards against accidents. It is not a boundary against a hostile program that runs as the same Unix user as the service, and this release does not claim isolation between mutually untrusted users.

## The parts

- The service, `jobd`, runs as one Unix user. Every Job it starts runs as that same user.
- Each Job has a supervisor process, also of that user, which starts the command, records its output and reports its end.
- Clients talk to the service through one Unix socket.
- Records, retained output, saved environments and the audit journal are files in the state directory of the service user.

## Who is trusted

The socket is the access boundary, and there is one level of access.

By default the runtime directory has mode 0700 and the socket 0600, and the service also checks the user of each connecting process and refuses any other user. With `[socket] group` in the configuration the socket is shared with one Unix group: the service user and the members of that group are admitted.

Everyone admitted has the same, complete control over every Job, Queue, Group and terminal of that service: submit, edit, cancel, signal, remove, reconfigure. There are no roles, no owners and no per-object permissions. The session and creator labels on a Job describe it; they authorize nothing.

Because every Job runs as the service user, submitting a Job is the ability to run any program as that user. Admitting someone to the socket therefore gives them that account. Treat membership of the socket group like a login to the service account.

The administrator of the host is trusted completely, as is anyone who can become the service user by other means.

## What the service protects

Against mistakes and interference by accident, among clients and Jobs that mean no harm:

- A client that is not admitted cannot connect. This is tested for a second user with and without a socket group (`linux_integration.rs`).
- Settings of one Job, Queue or Group change only through a request to the service. Requests are checked, and a refused request changes nothing.
- A request that changes something is recorded in the audit journal with the user ID and process ID of the client, taken from the socket and not from what the client says. The intent is written before the change, and a request whose intent cannot be written is refused.
- Records are written so that a crash of the service does not lose an acknowledged submission or a known exit status; see [reliability](reliability.md).
- Kernel limits that the service set and something else changed are noticed: the affected Job is stopped with a line that names the changed file. This detects a change. It does not prevent one.
- Proxy credentials are kept out of status output, logs, process arguments and the audit journal; see [Credentials](#credentials).

## What it does not protect

A Job is a program of the service user. Unless one of the controls below is requested for it, and within the limits of that control, a Job can do what any process of that user can do. In particular a hostile Job can:

- read the state directory: the records, retained output and saved environments of every other Job, the audit journal, and the private files that hold proxy credentials;
- write and delete there, including other Jobs' records and the audit journal;
- connect to the control socket and act as a fully admitted client: cancel, edit, remove or reconfigure anything, and submit further Jobs without the controls it was given itself;
- send signals to other Jobs, to their supervisors and to the service itself;
- trace other Jobs with `ptrace` and read their memory, unless the system call is denied for it or the host's own ptrace restrictions forbid it;
- read and write every file the service user can, including that user's SSH keys and shell configuration;
- raise or remove its own resource limits, by asking the service through the socket like any client, or by writing the control files of the delegated cgroup, which belong to the service user;
- attach to any shared terminal and type into it.

Two Jobs of the same service are therefore not protected from each other. A Queue or a Group does not change that: they organize work and carry settings, and neither is a container.

The audit journal is a record for honest use. It is a file of the service user with no protection against that user, so it does not prove anything to someone who suspects the service account itself.

Resource limits are a guard against overload by accident. They are enforced by the kernel when the service manages a delegated cgroup, and a cooperative Job cannot exceed them, but a Job that sets out to remove its limit can ask the service to change it, as described above. In monitoring mode, which is what a service without a delegated cgroup runs in, nothing is enforced by the kernel at all.

## What each control adds

Each control is optional, is requested per Job or as a default on a Queue or Group, and removes exactly what it names. A control that was requested and cannot be established stops the Job from starting; job does not run it without the control.

| Control | What it adds | What it leaves open |
|---|---|---|
| `--no-new-privs yes` | The program and its descendants cannot gain privileges through set-user-ID or file capabilities. | Everything the service user may already do. |
| `--cap-drop` | Removes capabilities. | A service that runs as an ordinary user starts programs without capabilities already, so little changes. |
| `--seccomp-deny LIST` | The named system calls fail. Denying `ptrace` stops this Job from tracing others. | Every call not named. A deny list is not a sandbox. |
| `--confine`, `--allow-write` | The Job can write only in its own tree, a few standard places and the allowed paths. This keeps it from altering the state directory, unless its own tree contains that directory. | Reading is not restricted: the Job can still read the state directory. Whether confinement keeps a Job from using the control socket was not examined, so do not rely on it for that. |
| `--namespaces`, `--root read-only`, `--private-tmp` | Separate IPC, host name, mount table or cgroup view; a tree the Job cannot write; a temporary directory of its own. | The Job still sees the whole file system, every process and the host network. |
| `--namespaces user,mount,pid` | The Job sees only its own processes, cannot signal others by number, and all its processes end with it. | It hides processes and is not a boundary against the same Unix user: processes outside still see and signal the Job's, and the Job still reaches every file and socket its user may open, the control socket included. |
| `--net none`, proxy, tunnel | The Job reaches no network, or only the proxy or the tunnel. | The control socket is a file, not a network address; a network boundary does not close it. |
| `--net profile:NAME` | The Job reaches only what the administrator's profile forwards: its exit, narrowed by its `allow` and `deny` rules. | The rules judge IPv4 addresses and ports, not names or content. Every admitted client can select every profile. |
| `--net ns:NAME` | Nothing. The Job is placed in a namespace that exists already. | It is a way into that namespace's network for every admitted client: Jobs there reach each other and whatever else is in it, and job filters nothing. Name only namespaces that every client of the service may use. |
| Resource limits | A ceiling enforced by the kernel. | See above: a guard against accidents. |

Details and the exact limits of each: [security controls](security-controls.md), [isolation](isolation.md), [networking](networking.md), [resource controls](resources.md).

No combination of these was assembled and tested as a sandbox against a hostile program. If you need to run code you do not trust, run it under a separate Unix user, in a container or in a virtual machine, and let job start that.

## Credentials

- Put a proxy's user and password in a private file and name it with `--net-secret-file`. The service records only the path. A password written inside the `--net` address is moved into a private file at submission and shown as `***` afterwards, but it was visible in the process list and the shell history of the command that submitted it.
- The Job that uses a proxy receives the credentials in its environment, because its programs need them. Other clients do not see them in any answer. Anyone who can read the state directory as the service user can.
- A WireGuard key is read from its file and handed to the kernel; only the file's path is recorded.
- A Job's environment is saved with its record so that the Job can be retried. Secrets in environment variables are therefore stored in the state directory, readable by the service user. The audit journal records the names of environment variables, not their values, and of a command only the program name, the number of arguments and a digest of them.
- `job state backup` copies all of this. Keep backups as private as the state directory. The backup manifest detects accidental damage; it is not a signature.
- SSH keys for `--on` are used by `ssh` on the local host and are not copied by job.

Details: [networking](networking.md#proxy-credentials) and [reliability](reliability.md#the-audit-journal).

## The audit journal

Every request that changes something leaves two lines in `audit/current.jsonl` in the state directory: an intent line written to disk before anything changes, and a result line written before the client is answered. `job audit` shows the pair as one record, and `--target`, `--action` and `--since` select from the journal. A record names the action, the target, the user ID and process ID of the client, the parameters with secrets removed, and the result. The arguments of a command are not recorded: the record holds the program name, the number of arguments and their SHA-256. Requests to attach, peers the service turned away and unreadable requests are recorded too, the last two at a bounded rate. Previews with `--dry-run` are not recorded. The journal rotates by size and keeps a configured number of files; the first record of a new file says what was dropped.

With a private socket every client is the service user, so the process ID is what tells clients apart. With a socket group the user ID identifies the member.

If the service dies between a change and its result line, the intent line remains and `job audit` shows the record with the result `outcome unknown`. If the intent line cannot be written, the request is refused and nothing changes.

`job audit` and `job events` read files in the state directory, so only the service user can run them.

## A system-wide service with a socket group

A system service runs under a dedicated account and shares its socket with one Unix group. What this changes:

- Members of the group are other Unix users. They cannot read the state directory directly, because it stays private to the service account, and the audit journal names them by their own user ID.
- They can submit, inspect, change and cancel work, read output and attach to terminals. Output reaches them through the service, which reads the retained output of the one attempt that was asked for and sends it over the control socket; terminals are reached through sockets in `terminals/` below the runtime directory, which the service user and the group can open. They cannot run `job audit` or `job events`.
- All work still runs as the one service account, whoever submitted it. A member can submit a Job that reads the state directory, another member's output, or anything else the service account can read. The group does not separate its members from each other.
- A member's own files are not available to their Jobs unless the service account can read them.

A socket group is a way for a team that trusts each other to share one service. It is not multi-user isolation. Running Jobs as the user who submitted them, roles and per-object permissions are not part of this release.

A system service was not run as root and not under systemd during development; see the [known limits](release-notes.md#known-limits).

## Reporting a vulnerability

See [SECURITY.md](../SECURITY.md).
