# Explicit execution profiles and admission classes

## What it does

Validates named, immutable revisions in service configuration, composes execution defaults and resolves explicitly selected profiles and classes. Classes set admission priority only. Profiles can combine a class with independent resource controls and supported execution options. Nearest selections and individual field overrides are recorded with their definition digests in each attempt. Accepted definitions remain in a bounded durable archive for recovery and offline validation.

## How to test

Run the versioned_presets CLI/service integration cases through job. Validate configuration, inheritance, override bounds, opt-out, immutable revisions, restart and attempt history through the public commands. Delegated controller tests and offline migration checks qualify kernel effects and durable archive recovery. No new unit tests are introduced while the grandfathered share exceeds 6%.
