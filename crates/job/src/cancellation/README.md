# Cancellation

## What it does

Captures current attempts in an explicitly recursive collection selection, validates membership on commitment, and stores cancellation operations before side effects. The daemon gates admission for pending selections, saves each lifecycle transition before publication, and retries partial application after errors and restart. Single-Job cancellation and legacy Queue clear use the same engine. Audit records survive Job record removal and contain no commands, environment or log output.

## How to test

Run tools/check through job. The watch integration tests exercise mixed subtrees, read-only previews, later arrivals, stale attempts and moves, idempotent replay, failed intent publication, partial record writes, signal ordering, preservation of successful partial results, and restart recovery. The delegated freezer suite covers graceful cancellation of suspended workloads.
