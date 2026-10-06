# Network namespaces and routing profiles

## What it does

Holds the `[network]` part of the service configuration and everything that follows from it. `[network.namespaces.NAME]` authorizes an existing network namespace for `--net ns:NAME`: the module checks that the file is a network namespace, tries the join in a short-lived child, and prepares the join the launched process performs, directly or through the owning user namespace, with a comparison of the namespace reached against the file named. `[network.profiles.NAME]` defines a routing profile for `--net profile:NAME`: the module validates it, turns `allow` and `deny` into nftables matches for the filtering namespace of `link.rs`, decides from `sharing` which filtering namespace a Job uses, checks a Job's own bandwidth and secret file against it, resolves the definition again at launch and records what was applied with its digest. It also answers `job net list` and `job net show`, the `network` part of `job host --json`, two checks of `job doctor`, and the references from Queues, Groups and pinned execution profiles.

Credentials are never held here: a profile names a secret file by path, and `netsecret` reads it at launch.

## How to test

Run `cargo test --test network_profiles` through job. The cases start a test service with a configuration, hold a user and network namespace pair with `unshare`, listen on loopback only, and skip with a printed reason where `slirp4netns`, `nft`, `tc`, `ip` or user namespaces are missing. No unit tests.
