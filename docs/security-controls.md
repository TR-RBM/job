# no_new_privs, capabilities and seccomp

All settings on this page are optional. A new Queue has none, and an ordinary Job without them starts exactly as before.

```sh
job host --json
job run --no-new-privs yes -- command
job run --cap-drop all -- command
job run --cap-drop net_raw,sys_admin -- command
job run --seccomp-deny ptrace,bpf,perf_event_open -- command
job queue set research/builds --job-seccomp-deny ptrace --job-no-new-privs yes
job queue unset research/builds job-seccomp-deny
```

`job host --json` lists the names this host accepts under `security_controls.supported_cap_names` and `security_controls.supported_syscalls`. The fields `service_no_new_privs` and `service_seccomp_mode` there describe the service process itself, which every Job inherits from.

## What each control does

`--no-new-privs yes` sets the Linux `no_new_privs` attribute before the program starts. Afterwards neither the program nor anything it starts can gain privileges through set-user-ID, set-group-ID or file capabilities. It cannot be unset. `inherit` leaves the attribute as the service has it. [prctl(2)](https://man7.org/linux/man-pages/man2/PR_SET_NO_NEW_PRIVS.2const.html)

`--cap-drop LIST` removes the named capabilities from the effective, permitted, inheritable and ambient sets; `all` names every capability the kernel knows. It also sets `no_new_privs`. The bounding set is reduced too where job is allowed to: when the Job's network setting makes job start it in a new user namespace (`--net none`, a proxy, a tunnel, a bandwidth limit), or when the service runs privileged. The result field `cap_bounding_reduced` says which happened. [capabilities(7)](https://man7.org/linux/man-pages/man7/capabilities.7.html)

`--seccomp-deny LIST` installs a filter under which each named system call fails with "Operation not permitted". It also sets `no_new_privs`. The filter is inherited by every descendant and cannot be removed. [seccomp(2)](https://man7.org/linux/man-pages/man2/seccomp.2.html)

## What they do not do

- A service running as an ordinary user starts programs without capabilities already. There `--cap-drop` changes nothing a program can observe except, in a new user namespace, an emptied bounding set.
- A program may create a user namespace of its own and holds capabilities inside it. Deny `unshare` if that matters. Denying `clone` or `clone3` also stops it, and stops threads and child processes with it.
- A deny list is not a sandbox. Each name is one system call: denying `mount` leaves `fsopen`, `fsmount` and `move_mount`; denying `socket` leaves `io_uring_setup`; denying `sendto` leaves `sendmsg`. Name every call you mean.
- `clone3` is answered with "Function not implemented" rather than "Operation not permitted", because the C library only falls back to `clone` on that answer.
- A program of another ABI than the host's native one (a 32-bit program on x86_64, x32) is killed when it makes a system call under a filter. Filters are supported on x86_64 and aarch64.
- These controls do not separate two Jobs of the same Unix user from each other.

## Inheritance, profiles and history

Queue and Group defaults use `--job-no-new-privs`, `--job-cap-drop` and `--job-seccomp-deny`. They are defaults: a Job may give its own value, or `inherit` to take none. `job queue unset PATH job-seccomp-deny` removes a local default and restores what the ancestors supply. Requesting `--cap-drop` or `--seccomp-deny` together with `--no-new-privs inherit` is refused, since both need the attribute.

Pinned profiles accept the same fields:

```toml
[[presets.profiles]]
name = "locked"
revision = 1
[presets.profiles.values]
no_new_privs = true
cap_drop = "all"
seccomp_deny = "bpf,perf_event_open,ptrace"
```

`job status ID --json` records where each value came from under `resource_sources` and what was applied under `result.security_controls`: `no_new_privs`, `cap_drop`, `cap_bounding_reduced`, `seccomp_deny` and the audit architecture of the filter. A retry resolves the defaults current at that time; earlier attempts keep their own record.

## Failure and remote execution

A name the host does not know is refused at submission and nothing is created. If the kernel refuses a control when the Job starts, the program is not executed and the Job records a start error.

With `--on HOST` the controls, `--confine` and `--net none` are applied on that host. The local SSH transport runs unfiltered and on the local network.
