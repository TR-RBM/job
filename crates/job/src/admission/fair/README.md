# Fair-share tests

## What it does

Exercises the hierarchical reservation-time accountant with deterministic clocks and synthetic resource requests. It distinguishes proportional service, temporary lookahead, rejoin floors and admission urgency.

## How to test

Run `cargo test admission::fair::` through job. CLI and daemon integration coverage remains required in the parent module and integration suites.
