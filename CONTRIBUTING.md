# Contributing

This page describes how to build and test job and which rules its code and documents follow. Contributions are accepted under the Apache-2.0 licence of the project; by submitting a change you agree that it is licensed that way.

This repository receives releases as snapshots: each release is one commit that holds the whole tree of that release. The history between two releases is not published here. A change you send is reviewed against the latest snapshot and appears, when it is accepted, in the next one.

## Build

job is one Rust workspace with one crate, `crates/job`, which builds the single binary `job`. `jobd` is the same binary under another name.

```sh
cargo build
cargo build --release     # what `make build` runs
```

You need Linux and stable Rust 1.98 or later. Nightly features are not used. The dependencies are the ones in `crates/job/Cargo.toml`; a change that adds a dependency says why it is needed.

To try a build without installing it, start the service from the build directory with a state directory of its own:

```sh
export JOB_STATE_DIR=$(mktemp -d)
target/debug/job daemon &
target/debug/job run -- true
```

With `JOB_STATE_DIR` set, the socket lies inside that directory, so a test service never meets an installed one. `job daemon` is the earlier spelling of `jobd` and is used here because a build directory has no `jobd` link.

## The gate

```sh
tools/check
```

A change is ready when `tools/check` passes on it. `make check` runs the same. It does four things and stops at the first that fails:

1. `tools/install-check` installs into a temporary directory with a stub binary, compares the installed files and their modes with `tools/install-manifest`, checks the service units and the runit example, uninstalls and checks that nothing is left. It then renders the four manual pages with groff, and with mandoc where it is installed, and fails on any warning, and checks the syntax of the Bash, fish and zsh completion files.
2. `cargo fmt --all --check`: the code is formatted with rustfmt's defaults.
3. `cargo clippy --quiet --all-targets -- -D warnings`: every clippy warning is an error.
4. `cargo test --quiet`: all unit and integration tests.

If you add, remove or rename an installed file, change the `Makefile` and `tools/install-manifest` together.

## Tests

Integration tests are in `crates/job/tests/`. They start the built binary and a private test service and go through the real command line:

| File | Covers |
|---|---|
| `watch_backend.rs` | most behaviour, with a service that has no cgroup |
| `freezer.rs` | everything that needs kernel enforcement through a cgroup |
| `durability.rs` | crashes of the service and of supervisors at named points |
| `state_migration.rs` | validation, backup, restore and conversion of state |
| `terminal_sessions.rs` | shared terminals |
| `cli_contract.rs` | help, completion, the syntax reference, option checks, JSON envelopes |
| `linux_integration.rs` | file locations, `jobd`, `doctor`, the socket group, cgroup discovery |
| `metrics.rs` | `job metrics` and the HTTP listener of the service |

Run one file or one group of cases with `cargo test --test durability` or `cargo test --test watch_backend network_boundary_`.

Test programs must be bounded by their own construction: no unbounded loops, no process bombs, no network beyond loopback. A test of a limit uses a workload that ends harmlessly if the limit does not hold.

### Tests that need a delegated cgroup

The cases in `freezer.rs`, and two in `linux_integration.rs`, write to cgroup v2 control files. They are marked ignored, so `cargo test` and `tools/check` skip them, and they must be selected:

```sh
cargo test --test freezer -- --ignored --test-threads=1
cargo test --test linux_integration -- --ignored --test-threads=1
```

They use the cgroup the test process itself runs in. That cgroup must not be the root cgroup, must be writable by the user who runs the tests, and must have nothing enabled in its `cgroup.subtree_control`. The tests create their own children below it and change nothing outside it. One way to arrange this is to run the command as a Job of an installed job service that manages a delegated cgroup:

```sh
job run -- cargo test --test freezer -- --ignored --test-threads=1
```

Run them when your change touches resource controls, suspension, pressure rules on Queues and Groups, or cgroup discovery, and say with your change whether you did.

### The unit-test limit

Unit-test source is limited to 6 % of the code. The repository is above that limit, so no new unit tests are added: no new `#[cfg(test)] mod tests` and no new `src/**/tests.rs`. Existing unit tests are corrected when the behaviour they check changes. New behaviour is tested by integration tests.

## Code rules

These are the rules the existing code follows. Reviewers hold changes to them; only formatting and clippy are checked by a tool.

- **Rust only**, apart from the `Makefile`, the service scripts under `deploy/` and the tools under `tools/`, which are POSIX shell.
- **No comments in code.** Names and structure carry the meaning.
- **Messages go through a catalogue.** Each module directory has a `messages.rs` with a table of English texts and their German translations and a `message()` function that picks by `LC_ALL`, `LC_MESSAGES` and `LANG`. `crates/job/src/security/messages.rs` is the model. The English text is the key. German addresses the reader as `du` and uses plain words. Identifiers a program reads, such as JSON field names, option names and outcome names, are never translated. Some older top-level files, among them `daemon.rs`, still build English messages in place; new text does not.
- **Commands and options are declared once**, in `crates/job/src/commands/table.rs`, with a help text in `crates/job/src/commands/messages.rs`. Help, the option check, completion and the syntax reference come from that table. After changing it, regenerate the four generated files and never edit them by hand:

  ```sh
  target/debug/job completion bash > completions/job.bash
  target/debug/job completion fish > completions/job.fish
  target/debug/job completion zsh > completions/job.zsh
  target/debug/job help --syntax > docs/reference/syntax.md
  ```

  `cli_contract.rs` fails when they differ from what the program prints.
- **One README per module.** Each module directory has a `README.md` with the headings What it does and How to test.
- **Stored and transmitted data stay readable.** A new field in a record or in a request is optional with a default, so that older records load. A change that old records or old clients cannot tolerate raises the state schema or the protocol version, which needs a migration path and a note in the release notes.
- **Existing behaviour stays.** Without a new option, nothing about ordinary execution changes, and existing commands, their output and their tests keep working. The earlier spellings listed in the [release notes](docs/release-notes.md) are kept throughout the first major version.
- **Keep changes to the large shared files small**: `main.rs`, `daemon.rs`, `model.rs`, `shim.rs`, `store.rs`. Put logic in a module of its own and add a thin call.

## Documents

- **Reasons and results.** A change of behaviour or design says, in the text that comes with it, what it changes and why, what was run with which results, and what was not tested. A list of what was not tested is expected, not optional.
- **User documentation.** A user-visible change comes with its page or section under `docs/`, its entry in `man/job.1` or the other manuals, and a line in `docs/release-notes.md`. A new capability gets a row in `docs/capabilities.md` that says what it needs from the host and where it is tested.
- **Style.** Plain sentences, short headings, no decoration. Write for a Linux user or administrator who was in no earlier conversation. Never describe something planned as if it existed.

## Commits and changes

- A commit message is one line of at most 72 characters that says what the change does to the code, in the imperative: `Add optional namespaces, read-only root and private tmp`. A body follows after a blank line only where the diff cannot show something a later reader needs.
- A commit message names no person, no conversation and no review state.
- Keep one change to one subject. Something else you notice goes into a separate change or into a note to the maintainers.
- Never commit credentials or private keys.

Before you send a change, check:

1. `tools/check` passes, and you ran the delegated-cgroup tests if the change touches what they cover.
2. The reasons, the results and the user documentation are in the change.
3. Generated files were regenerated, not edited.
4. Every command and option in the documentation you wrote exists in `docs/reference/syntax.md`.

## Security

Do not report a vulnerability in a public change or issue. See [SECURITY.md](SECURITY.md).
