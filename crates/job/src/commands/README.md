# Shared command definitions

## What it does

Holds one static table of every `job` command with its options, operands, exit statuses and output kind, and derives from it the one-screen help, per-command help, the syntax reference, the Bash and fish completion scripts, the uniform check of option names, and the `--format json` envelope. It also provides `job list`, `job show`, `job completion` and the hidden bounded query `job __complete`. Existing parsers keep parsing values; this module decides which options exist. `output::write` is what `println!` and `eprintln!` mean in this crate (the macros are defined at the top of `main.rs`): a closed standard output or standard error ends a client with 125 and without a panic, and is ignored in the service and its supervisors.

## How to test

Run `cargo test --test cli_contract` through job. The cases start the built binary with and without a test daemon and cover help in English and German, the uniform refusal of unknown options, every table option of `run`, `submit`, `queue set` and `group create` reaching its parser, generated completion files and the syntax reference being equal to the checked-in ones, completion in a real Bash, the bounded query against a running, an absent and a silent service, `list` and `show`, and the envelope around `wait`, `attempts` and `queue show`. No unit tests.
