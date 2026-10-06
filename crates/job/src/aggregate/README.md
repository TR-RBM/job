# Aggregate resource domains

## What it does

Maps explicit collection CPU, memory and device I/O controls to nested cgroup v2 domains. Unconfigured Groups and Queues add no kernel domain. Each attempt retains its domain chain; every ancestor ceiling remains in force. Per-Job defaults remain separate. Domain topology changes require an inactive affected subtree; explicit live updates can change existing domains. Local domains cannot constrain remote workloads, which are refused when aggregate policy applies.

## How to test

Run tools/check through job, followed by the explicitly selected delegated freezer suite. Inspect nested domain files, shared ancestry, recovery, lifecycle targeting and empty-topology equivalence.
