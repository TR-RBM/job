# Admission ordering

## What it does

Resolves optional admission priority, validates ancestor bounds, persists eligible waiting credit and plans protected dispatch opportunities through the complete logical hierarchy. Optional Group scopes account CPU-request-time or memory-request-time, choose sibling subtrees by weighted service and retain rejoin floors. Explanations keep urgency, waiting credit, reservation-time shares, FIFO constraints and kernel weights distinct.

## How to test

Run tools/check through job. Existing unit tests use deterministic clocks and resource timelines; watch-backend integration tests exercise public priority, aging, fair-share scope/weight changes, FIFO, explanation and restart behavior. The delegated freezer suite checks that admission shares do not introduce kernel sharing domains or weights. New unit-test additions are restricted by the repository's documented 6% limit and grandfathered baseline.
