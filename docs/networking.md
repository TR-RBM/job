# Networks, their boundaries and proxy credentials

A Job runs in the host's network unless it or its Queue says otherwise. Nothing on this page changes a Job that names no network and no bandwidth.

```sh
job run --net none -- command
job run --net socks5://proxy.example:1080 --net-secret-file ~/.config/job/proxy.secret -- command
job run --net http://proxy.example:3128 -- command
job run --net wireguard:/home/me/tunnel.conf -- command
job run --bandwidth 5Mbit -- command
job queue create crawl --net socks5://proxy.example:1080 --net-secret-file ~/.config/job/proxy.secret
job queue set crawl --bandwidth 20Mbit --job-bandwidth 5Mbit
job run -q crawl -- command
job run --net ns:lab -- command
job run --net profile:updates -- command
job net list
```

A Queue takes `--net` and `--net-secret-file` when it is created or with `job queue set`. The bandwidth of a Queue, `--bandwidth` for all its Jobs together and `--job-bandwidth` for each of them, is set with `job queue set`; `job queue create` and Groups do not take it.

## What each setting gives a Job

| Setting | Network namespace | IPv4 | IPv6 | Name resolution | UDP | ICMP |
|---|---|---|---|---|---|---|
| `--net default`, no bandwidth | the host's | as the host | as the host | as the host | as the host | as the host |
| `--net none` | its own, loopback only | none | none | none | none | none |
| `--net socks5://…`, `http://…`, `https://…` | its own, behind a filtering namespace | TCP to the proxy's address and port only | none | by the proxy; the Job's resolver file names 127.0.0.1, where nothing listens | none leaves | none leaves |
| `--net wireguard:FILE` | its own, behind a filtering namespace | through the tunnel only | none | the `DNS` lines of FILE, reached through the tunnel; none if FILE has none | through the tunnel | not available to the Job |
| a bandwidth with the default network | its own, behind a filtering namespace | TCP and UDP through slirp4netns, held to the rate | none | slirp4netns's resolver at 198.18.0.3, which asks the host's | held to the rate like TCP | not available to the Job |
| `--net ns:NAME` | an existing one the configuration names, shared | whatever that namespace has | whatever it has | the host's resolver file, or the namespace's `resolv_conf` | whatever it has | whatever it has |
| `--net profile:NAME` | as its `egress` says; see [named routing profiles](#named-routing-profiles) | narrowed by `allow` and `deny` | none | the profile's `dns` | as the egress, narrowed by the rules | not available to the Job |

The proxy variables a Job receives (`ALL_PROXY` and `all_proxy`, and for an HTTP proxy also `HTTP_PROXY`, `HTTPS_PROXY` and their lower-case forms) only tell programs where the proxy is. They isolate nothing. The boundary is the Job's network namespace and the packet filter of the filtering namespace in front of it: a program that ignores the variables reaches nothing. `socks5://` is handed to the Job as `socks5h://`, so that names are resolved by the proxy.

When the proxy stops, or the WireGuard peer stops answering, the Job has no network. Nothing falls back to a direct connection.

A proxy on this host's loopback is reached by the Job at 198.18.0.2; job rewrites the address in the variables.

The tunnel carries IPv4 only: IPv6 addresses and allowed ranges in a WireGuard file are ignored. `openvpn:FILE` is refused.

## Example: a Job whose traffic goes through Tor

A Tor client offers a SOCKS5 proxy, by default on `127.0.0.1:9050`. Naming that proxy as the Job's network sends everything the Job can send through Tor, and leaves it nothing else:

```sh
job run --net socks5://127.0.0.1:9050 -- curl https://check.torproject.org/api/ip
```

```text
{"IsTor":true,"IP":"185.220.101.110"}
```

The same request from the host itself, without job, answers `"IsTor":false` with the host's own address.

What the Job gets:

- A network namespace of its own. The only connection that leaves it is TCP to the proxy's address and port.
- The variables `ALL_PROXY` and `all_proxy`, set to `socks5h://198.18.0.2:9050`: the address under which the Job reaches the host's loopback, and a scheme that lets the proxy resolve names, so no name is looked up outside Tor. Addresses ending in `.onion` work for the same reason.
- No direct name resolution, no UDP, no IPv6 and no ICMP.

A program that honours those variables, as `curl`, `git` over HTTPS and most HTTP libraries do, works unchanged. A program that ignores them and connects directly gets no connection at all; it does not reach the network by another way. When the Tor client stops while the Job runs, the Job's connections fail and stay failed.

For a whole Queue, so that every Job submitted to it goes through Tor without saying so:

```sh
job queue create tor --net socks5://127.0.0.1:9050
job run --queue tor -- curl https://check.torproject.org/api/ip
```

For a name people can use without knowing the address, the administrator declares a profile in the service configuration, see [named routing profiles](#named-routing-profiles):

```toml
[network.profiles.tor]
description = "everything through the local Tor client"
egress = "socks5://127.0.0.1:9050"
sharing = "shared"
```

```sh
job net show tor
job run --net profile:tor -- curl https://check.torproject.org/api/ip
```

The Tor client itself is not started by job; run it as your system does. It has to be running before the Job starts.

What this does and does not give: it keeps a Job's traffic from leaving any other way than through the proxy. It does not make a program anonymous by itself: what the program sends through the proxy, such as cookies, account names or a browser's fingerprint, is the program's affair, and the Tor Project's own advice applies. The network a Job uses is fixed when the Job starts; a Job that is already running cannot be moved to another network.

## One network per Job, or one shared by a Queue

- `--net default` without a bandwidth: the Job shares the host's network with everything else on the host.
- `--net none`: one network namespace per Job, with nothing in it but loopback.
- A proxy, a tunnel or a bandwidth on a Job, outside a Queue that sets one: one filtering namespace per Job, named `job-ID`, with its own slirp4netns process, and removed when the Job ends.
- A Queue that sets a proxy, a tunnel or a bandwidth: one filtering namespace for the Queue, named after it and kept while the Queue has that setting. Each of its Jobs still has a network namespace and an address of its own (198.19.0.2 to 198.19.0.254, so at most 253 Jobs at a time). What the Jobs share is the exit (one slirp4netns process, one WireGuard interface with one key, one proxy), the rate budget and the filter. They cannot reach each other: their ports on the shared bridge are isolated from one another. A Job in such a Queue cannot ask for a different network; run it outside the Queue.
- `--net ns:NAME`: no namespace is made. Every Job that selects the name shares the existing namespace.
- `--net profile:NAME`: the profile's `sharing` says it, per Job, per Queue or one for all its Jobs.
- With `--on HOST` the network setting is applied on that host.

`job status ID --json` shows the filtering namespace a Job used under `link.name`.

## An existing network namespace

`--net ns:NAME` starts a Job inside a network namespace that already exists. NAME is an entry the administrator wrote into the service configuration; a client never gives a path. That list is what "authorized" means here: every client admitted to the service can select every name the configuration holds, and nothing else.

```toml
[network]
namespace_directory = "/run/netns"

[network.namespaces.lab]
description = "the measurement bench"

[network.namespaces.bench]
path = "/srv/job/netns/bench.net"
user_namespace = "/srv/job/netns/bench.user"
resolv_conf = "/srv/job/netns/bench.resolv.conf"
```

```sh
job run --net ns:lab -- command
job queue create bench --net ns:bench
job net list
job net show lab
```

- `path` is the namespace file. Without it the file is `NAME` in `namespace_directory`, which is `/run/netns` unless set, the place where `ip netns add` puts its files. A relative `path` is taken inside that directory. An absolute `path` may lie anywhere, since the configuration is the administrator's, except under `/proc`: a path there names a process, and a process number is given to another process later. `..` is refused.
- `user_namespace` names the file of the user namespace that owns the network namespace. The service joins it first and the network namespace second, then gives the Job the service's own user again. An ordinary-user service needs this, and it works only for a user namespace that the service's user created.
- `resolv_conf` is a file that the Job sees as `/etc/resolv.conf`. Without it the Job reads the host's file, whose resolvers may not be reachable from that namespace.
- `description` is shown by `job net show`.

Who may join what is the kernel's rule, not job's. Joining a network namespace needs `CAP_SYS_ADMIN` in the user namespace that owns it. A namespace made by root with `ip netns add` belongs to the initial user namespace, so only a service that runs with that privilege can join it; an ordinary-user service is refused with the kernel's reason:

    network namespace lab: the service may not join this namespace: it belongs to a user namespace the service is not in; run the service with the needed privilege or give user_namespace (Operation not permitted)

The service checks at submission that the name is configured, that the file exists and is a network namespace, and that it can join it, by trying in a short-lived child process. At launch it joins, and before the command starts it compares the namespace the process is in with the file the configuration names; a mismatch is a start error. The Job record keeps the name under `spec.declared.net`; the path the launch used is recorded under `network.namespace`.

What sharing means: every Job that selects the name is in the same network namespace as every other such Job and as whatever else lives there. They can reach each other. job adds no packet filter, no name resolution of its own and no bandwidth limit to that namespace, because job does not own its devices: `--bandwidth` together with `ns:NAME` is refused, a Queue's bandwidth beside it is refused, and so is `--net-secret-file`, since there is no proxy and the rules for proxy credentials do not apply. A Job in a Queue that sets `ns:NAME` cannot ask for a different network.

With `--on HOST` the name is looked up in the configuration of that host's service.

## Named routing profiles

A routing profile is an outbound policy with a name. The administrator defines it in the service configuration, and a Job, a Queue, a Group or a pinned execution profile selects it with `profile:NAME` instead of spelling the pieces.

```toml
[network.profiles.updates]
description = "package mirrors only"
egress = "host"
allow = ["192.0.2.10:443/tcp", "192.0.2.0/24:53/udp"]
deny = ["192.0.2.66"]
dns = ["192.0.2.53"]
bandwidth = "20Mbit"
job_bandwidth = "5Mbit"
sharing = "shared"

[network.profiles.crawl]
egress = "socks5://proxy.example:1080"
secret_file = "/etc/job/crawl.secret"
sharing = "per-queue"
```

```sh
job run --net profile:updates -- command
job queue set crawl --net profile:crawl
job net show updates
```

| Key | Meaning |
|---|---|
| `egress` | where traffic leaves: `"host"`, `"none"`, `"socks5://HOST:PORT"`, `"http://HOST:PORT"`, `"https://HOST:PORT"`, `"wireguard:/ABSOLUTE/FILE"` or `"ns:NAME"`. `openvpn:` and another profile are refused |
| `secret_file` | for a proxy only: a file with `USER:PASSWORD`, under the same rules as `--net-secret-file`. A user and password inside `egress` are refused, because the configuration is shown by `job config show` |
| `bandwidth` | the rate each way for the whole network of the profile, for example `"10Mbit"` |
| `job_bandwidth` | the rate each way for each Job; not above `bandwidth` |
| `dns` | up to three IPv4 resolver addresses, written into the Job's `/etc/resolv.conf` |
| `allow`, `deny` | up to 64 rules each, `ADDRESS[/PREFIX][:PORT[-PORT]][/tcp|/udp]`, IPv4 only |
| `sharing` | `"per-job"` (the default), `"per-queue"` or `"shared"` |
| `description` | shown by `job net show` |

Unknown keys are refused.

What each egress gives, and what the other keys may add to it:

| `egress` | The Job's network | `allow`, `deny` | `dns` | `bandwidth` |
|---|---|---|---|---|
| `host`, nothing else set | the host's network, shared with everything on the host; job filters nothing | — | — | — |
| `host` with any of the other keys | a namespace of its own behind a filtering namespace; IPv4 TCP and UDP through slirp4netns | enforced by the packet filter | written to the resolver file; without it slirp4netns's resolver at 198.18.0.3 | enforced |
| a proxy | a namespace of its own; only TCP to the proxy's address and port | refused: the Job reaches nothing but the proxy, so filter destinations at the proxy | refused: names are resolved by the proxy | enforced |
| `wireguard:FILE` | a namespace of its own; only the tunnel | enforced on what enters the tunnel | replaces the `DNS` lines of FILE | enforced |
| `none` | loopback only, one namespace per Job | refused: there is nothing to filter | refused | refused |
| `ns:NAME` | the configured namespace, shared as described above | refused: job does not own that namespace | refused | refused |

The packet filter is nftables in the filtering namespace, in this order: packets of connections already allowed pass; a packet that matches a `deny` rule is rejected; if the profile has `allow` rules, a packet that matches one passes; if it has none, everything the egress forwards passes; everything else is rejected. So `allow` narrows what the egress would forward, and `deny` wins over `allow`. A rule without a port covers every port, and a rule without `/tcp` or `/udp` covers both and, without a port, every protocol. The rules judge the address a Job connects to, after no translation: a rule for a name's address does not follow the name.

An address in 127.0.0.0/8 in a rule or in `dns` means the host's loopback, which a Job behind a filtering namespace reaches at 198.18.0.2; job writes that address into the filter and the resolver file. When a profile has `allow` rules, name resolution needs a rule too: allow the resolver's address on port 53.

These networks carry IPv4 only. An IPv6 range in `allow` or `deny` and an IPv6 address in `dns` are refused when the configuration is read; nothing pretends to filter what is not forwarded.

### One network per Job, or one for many

| `sharing` | Filtering namespace | Shared among | Each Job |
|---|---|---|---|
| `per-job` | one for each Job, named `job-ID`, removed when the Job ends | nothing | its own exit, filter and rate |
| `per-queue` | one for the Jobs of one Queue that use the profile | the exit, the filter and the `bandwidth` | its own network namespace and address; `job_bandwidth` each |
| `shared` | one for all Jobs that use the profile, in any Queue | the exit, the filter and the `bandwidth` | its own network namespace and address; `job_bandwidth` each |

Jobs that share a filtering namespace cannot reach each other: their ports on its bridge are isolated, as for a Queue's network. A shared filtering namespace exists while a Job uses it and is removed when the last one ends. It holds at most 253 Jobs at a time. `sharing` has no meaning for `egress = "none"`, `"ns:NAME"` or a plain `"host"` and is refused there. With `per-queue` or `shared`, `job_bandwidth` needs `bandwidth`.

### Selecting a profile

- `--net profile:NAME` on `job run`, `submit`, `create` and `edit`.
- `--net profile:NAME` on `job queue create`, `job queue set`, `job group create` and `job group set`. The Jobs below inherit it, and a Job in a Queue that sets a profile cannot ask for a different network. A Queue's own `--bandwidth` beside a profile is refused; set it in the profile.
- `net = "profile:NAME"` or `net = "ns:NAME"` under `[presets.profiles.values]` of a pinned execution profile. A preset still cannot carry a proxy address or credentials.
- `--bandwidth RATE` on a Job narrows the profile's rate for that Job. A wider rate than `job_bandwidth`, or than `bandwidth` where that is not set, is refused; so is any rate on a shared profile without `bandwidth`.
- `--net-secret-file` together with a profile that has `secret_file` is refused. A proxy profile without `secret_file` takes the Job's own file.

### What is recorded, and what a change does

The Job record keeps the name. When a Job starts, the service looks the profile up again and records what it applied under `network` in `job status ID --json`: the name, `digest`, the SHA-256 of the definition as configured (the secret file's path is part of it, its content is not), `scope`, the egress, the rates, `dns`, `allow`, `deny`, the filter rules in order under `rules`, and the filtering namespace under `holder`. `job explain ID` shows the name, the digest and the scope; before the Job starts it shows what the current definition would give.

A profile changes only when the service reads its configuration again, at a start or at `job config reload`, which needs a drained service. A Job that waited across that uses the definition current at its start and records that digest. If the profile is gone by then, the Job does not start: its record holds a start error that names the profile. Running Jobs keep the network they started with. A reload that removes a profile or a namespace which a Queue, a Group or an execution profile in the same file still names is refused, and the service does not start with such a configuration.

`job net list` prints the configured namespaces and profiles with whether this host can use them, `job net show NAME` one of them with its rules, and `job host --json` holds the same under `network`. `job doctor` reports them in the checks `network_namespaces` and `network_profiles`. A profile's `secret_file` appears as a path; no answer, record, journal or process argument holds its content.

## Proxy credentials

Put the proxy's user and password in a file and name it:

```sh
install -m 600 /dev/null ~/.config/job/proxy.secret
printf '%s\n' 'user:password' > ~/.config/job/proxy.secret
job run --net socks5://proxy.example:1080 --net-secret-file ~/.config/job/proxy.secret -- command
```

The file holds one line `USER:PASSWORD`; write `@`, `/` and spaces percent-encoded. It must be a regular file that belongs to the user the job service runs as, with no permission for its group or for others. job reads it when the Job starts and records only its path. A Queue or a Group takes `--net-secret-file` beside `--net`, and its Jobs inherit both. Setting a new `--net` on a Queue without `--net-secret-file` drops the old file from it.

An address with the user and password inside, `socks5://user:password@proxy.example:1080`, is still accepted. job moves them at submission into a private file, mode 600, in the Job's state directory (`jobs/ID/net-secret`), or under `net-secrets/` in the state directory for a Queue or Group, and records the address as `socks5://***@proxy.example:1080` beside that file's path. An address stored in a Queue or Group before this version is moved the same way when the service starts, and so is the address in the record of every Job that has not ended and in the files under `links/`. The user and password inside an address must be `USER:PASSWORD` with `@`, `/` and spaces percent-encoded, as in the file; anything else is refused at submission.

Who can read the credentials:

- The Job itself can: its proxy variables carry them, because the programs in it need them. Anything the Job starts inherits them.
- Other clients of the service cannot. Answers, `job status`, `job queue`, `job host`, their `--json` forms, error messages, the process arguments of the service and its helpers, and the files under `links/` show `***` or nothing.
- The service user can, in the secret file, as with any file of that user. The private files are removed with the Job's record; a retried Job's earlier attempt keeps its copy under `jobs/ID/attempts/N/` until the record is removed.
- A file under `net-secrets/` that no Queue, Group or waiting or running Job names any more is deleted at the next change to a Queue or Group. A finished Job that inherited such a file cannot be retried after that; submit it again.
- `job state backup` copies the private files with their mode, so a backup holds the credentials the state held.
- With `--on HOST` the credentials travel inside the request on the SSH connection's input, never as an argument, and the other host moves them into its own private file.

What job cannot hide: the arguments of the `job` command you type. `job run --net socks5://user:password@…` is visible in the process list while that command runs, and in your shell history. Use `--net-secret-file`. The record of a Job that ended under an earlier version, and the archived attempts of any Job from then, still hold on disk the address they were given, because ended attempts are not rewritten; every answer of the service shows them with `***`, and the files have mode 600. Remove such records with `job remove ID`.

The audit journal records a request with `***` in place of the user and password, whatever characters the password holds. Everything before the last `@` of an address counts as user and password, which is the rule the service itself uses to read the address; an argument of a command that looks like an address is recorded the same way.

An idempotency key compares submissions without the user and password of the proxy address. Two submissions under one key that differ only there are the same submission, and the Job recorded first keeps its credentials.

The service checks a network setting itself and does not rely on the `job` command: a proxy address must use `socks5`, `socks5h`, `http` or `https` and name a port, and a WireGuard file and a secret file must be absolute paths.

When the service starts and finds a Queue's filtering namespace from before the restart, it isolates every Job port on its bridge again. If that fails, the service says so on its standard error at start, and no further Job joins that network until the isolation succeeds; a Job that would join it gets a start error that names the network.

A WireGuard file is read by job when the network is made; the key is given to the kernel directly and only the file's path appears in records and arguments.

## What it needs

`unshare`, `nsenter`, `sleep`, `ip`, `tc`, `nft` and `slirp4netns` on the service's PATH, and a kernel that lets an unprivileged user create user namespaces. `ip` must know `bridge_slave isolated` (iproute2 4.18 or later). Where a tool or user namespaces are missing, a Job that asks for a proxy, a tunnel or a bandwidth is refused at submission with the names of what is missing; `--net none` needs user namespaces only.

`--net ns:NAME` needs none of these tools. A profile needs them when it gives a Job a filtering namespace; `job net show NAME` says what is missing.

The network boundaries are tested on loopback, with a SOCKS5 proxy and with a WireGuard tunnel between two namespaces of one host. What was not tested is listed in the [release notes](release-notes.md#known-limits).
