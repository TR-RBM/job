# Command hook

## What it does

Answers one question for a program that is about to run a shell command line for somebody else: may it run as it is, must it be refused, or should another command run in its place. `job hook` reads one JSON object on standard input and writes one JSON object on standard output. It never runs the command.

The question has `command`, and optionally `cwd` (the directory the command would run in; default the hook's own), `caller` (a label of the caller; it becomes the session label of a rewritten command and is written to the denial log), `caller_name` (the name the command policy compares with its lists) and `detached` (true when the caller already runs the command without waiting for it). Other fields are ignored.

The answer has `decision`, one of `allow`, `deny` and `rewrite`. `deny` carries `reason` and, when a rule of the command policy refused, `rule`. `rewrite` carries `command`, the line to run instead, `detach: true`, which says that the line is meant to be started without waiting for it, and `reason`. `allow` carries `reason` only when there is something to tell, such as a service that does not answer.

A command is refused when the command policy forbids it, and when a line that would be rewritten starts with `cd` or `export`, which would change nothing for the caller's shell once the rest runs detached. A command is rewritten when it is heavy: anything but a short list of file, text and query programs, most `git` commands and cheap `cargo` commands becomes `job run --session CALLER --budget none --shell bash --summary -- 'COMMAND'`, followed by a guard that keeps the exit status and leaves a working directory that no longer exists. A `job run` or `job wait` that would hold its caller is rewritten in the same way. `classify` gives the first of the two verdicts alone, `direct` or `route`, for `job classify`.

Input that is not a JSON object with a `command` ends with status 125 and a message on standard error, and nothing on standard output.

## How to test

`cargo test hook::` runs the unit tests of classification and of the answers. `cargo test --test linux_integration the_hook_answers` goes through the built binary with and without a test service; `cargo test --test linux_integration policy` covers the hook together with a policy file.
