# Live resource updates

## What it does

Plans explicit resource transitions, journals their kernel progress, pins the target cgroup identity, and reconciles writes interrupted before acknowledgment. The daemon publishes verified resource snapshots without rewriting launch specifications. I/O maps are replaced device by device, including resets of removed devices. Pending operations remain inspectable and prevent conflicting changes.

## How to test

Run tools/check through job. The resource_update unit tests exercise transition planning, journal validation and interrupted writes. The delegated freezer suite exercises the public commands against isolated live cgroup domains.
