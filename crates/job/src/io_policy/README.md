# Device I/O policy

## What it does

Defines optional device bandwidth/IOPS limits and explicit IOCost or BFQ weights. CLI options accept canonical whole-device major:minor numbers. Repeated options name distinct devices; a local policy replaces that complete inherited map. Unknown fields, duplicate devices, partitions, zero rates and unsupported weight backends are refused. Kernel comparison handles unordered rows, omitted unlimited limits and inherited weight defaults. Capability inspection reads sysfs and root IOCost state without changing device-wide policy.

## How to test

Run tools/check through job, followed by the explicitly selected delegated freezer suite. Unit tests cover parsing, kernel normalization and capability decisions. Live tests verify controller files, bounded direct-I/O throttling, aggregate limits and unsupported weight rejection without changing host device policy.
