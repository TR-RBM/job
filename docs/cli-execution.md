# Commands, shells and waiting

Arguments after `--` are executable arguments, including when there is only one. Quoting protects an argument from your current shell; it does not ask job to interpret that argument again.

```sh
job submit -- /path/to/program
job submit -- sh -c 'producer | consumer'
job submit --shell bash -- 'printf "%s\n" "$1"' 'one argument'
```

`--shell SHELL` runs `SHELL -c SCRIPT job ARG...`: `$0` is `job`, `$1` is the first remaining argument. The executable can be an absolute path or resolved through the Job's saved PATH. Shell selection is explicit; it is not selected from `$SHELL`. The resolved argv is saved with the Job. Already submitted Jobs, historical attempts and retries keep their saved interpretation.

## Wait without replaying output

```sh
job wait 42
job wait --timeout 30s 42
job wait --summary 42
job wait --json 42
```

Plain wait prints no successful or failed command output. It returns the outcome's exit status. `--summary` requests the diagnostic summary and retained-output excerpt. `--json` requests one result object and cannot be combined with `--summary`. The ID can precede or follow options. Timing uses a monotonic clock; native waiting performs bounded service requests, reconnects on transient failure and honors a finite timeout during disconnection. It does not cancel the Job. A retry that changes the attempt while the client is waiting requires inspecting the Job and waiting again.

| Outcome | Exit status | JSON outcome |
|---|---|---|
| Process exits | Process exit code | `completed` |
| Process terminates by signal | 128 + signal number | `completed` |
| Cancellation | 1 | `cancelled` |
| Other managed stop | 1 | `stopped` |
| Lost execution | 125 | `lost` |
| Executable cannot start | 125 | `start_error` |
| Service rejects the request | 125 | `service_error` |
| Wait deadline reached | 75 | `timeout` |
| Service remains unavailable | 75 | `unavailable` |

The JSON envelope includes `schema_version: 1`, `outcome`, `exit_status`, `job` and `error`. `job` is null for a service error or unknown execution; an unavailable result also identifies `job_id`. Outcome identifiers are not localized. An application may itself exit with 75 or 125; its outcome is still `completed`. Waiting-client termination leaves the Job managed by the service.

## Existing scripts

Rewrite a previous invocation such as `job run -- 'make && make check'` to `job run -- sh -c 'make && make check'` or use `--shell bash`. `--legacy-shell` opts into the previous rule for that command: a single argument is interpreted by Bash, while multiple arguments remain literal. It cannot be combined with `--shell`.

For a deliberate transition period, `JOB_CLI_COMPAT=legacy` preserves the old single-argument rule, summary output and unwrapped wait JSON. `JOB_CLI_COMPAT=unix`, or an absent variable, selects the new contract. Any other value is an error. This client setting is independent of the service's ordinary/legacy resource policy. Explicit shell selection still takes precedence over the environment compatibility rule. The optional [command hook](integrations.md) requests Bash and summaries explicitly.

Native pipe-mode `run` streams raw stdout and stderr separately while work runs. `--summary`, `--json` and legacy compatibility select final result views instead. `job logs ID --follow` follows the selected attempt; stream selection and safe versus raw rendering are described in [output recording](output-recording.md). Quiet wait does not replay any output.

## Signals, disconnection and cancellation

`job submit` leaves no client behind, so nothing that happens to a terminal reaches the Job. `run`, `wait`, `logs --follow` and `attach` keep a client. What happens to that client never cancels the Job; only `job cancel`, or `job run --on-interrupt cancel`, does.

| Event | `run` in pipe mode | `run --pty`, `attach` | `wait`, `logs --follow` |
|---|---|---|---|
| first SIGINT or SIGTERM | forwarded to the processes of the Job as the same signal; the client goes on waiting | the client detaches and returns 0; the Job goes on | the client ends by the signal; the Job is untouched |
| second SIGINT or SIGTERM | the client returns 75; the Job goes on | | |
| SIGHUP | the client returns 75; the Job goes on | the client detaches; the Job goes on | the client ends by the signal; the Job is untouched |
| closed output (EPIPE) | the client returns 75; the Job goes on | the client reports the lost connection and returns 125; the Job goes on | `logs` ends with 125; `wait` prints nothing and is unaffected |
| lost service connection | output is still read from the record; after 60 seconds without the service the client returns 75 | the client returns 125; attach again | `wait` tries again for up to 60 seconds or its timeout, then returns 75 with outcome `unavailable`; `logs` reads the record and needs no service |

Every other command that prints, such as `job list --json | head -1`, ends with 125 and without a message as soon as its standard output or standard error is closed; nothing about the service or a Job changes. The same holds for the final summary of `run` once the Job has ended. The client keeps `SIGPIPE` ignored and acts on the failed write instead, because `run` and `logs --follow` need the error to leave the Job behind in an orderly way. Any other failure to write the output, a full disk behind a redirection for example, is named on standard error and also ends with 125.

With `--pty` the terminal is in raw mode, so Ctrl-C typed there is input for the Job and no signal to the client; the detach key leaves the terminal.

`--on-interrupt forward|detach|cancel` chooses what the first SIGINT or SIGTERM does to `job run` in pipe mode. `forward` is the default. `detach` returns 75 at once and leaves the Job running. `cancel` requests cancellation and goes on waiting, so the client returns 1 once the Job has ended. A second signal always returns 75 without waiting. A signal forwarded to a Job that has not started reaches nothing, and the client says so.

Whenever the client leaves a Job behind it prints one line on standard error:

```text
[job] interrupted; Job 42 goes on; wait for it with job wait 42, cancel it with job cancel 42
```

`job run --json` then prints `"outcome":"detached"` with `"exit_status":75` and `job_id`. Under `JOB_CLI_COMPAT=legacy` and without `--on-interrupt`, `run` installs no handlers: a signal ends the client and the Job goes on, as before. Before this was defined, a signal ended the client silently and a closed output returned 125. Commands other than `run` and `logs` used to end a closed output with a Rust panic message and status 101.

## Standard input

A Job in pipe mode reads end of file on standard input: its input is `/dev/null`. `job run --stdin` passes the standard input of the client to the Job and closes it when the client reaches end of file or ends.

```sh
printf 'one\ntwo\n' | job run --stdin -- sort -r
```

The input is not recorded. If no client connects within ten seconds of the start, for example because the client was gone while the Job waited, or because `job retry` started a later attempt, the Job reads end of file. `--stdin` is refused with `--pty`, where the terminal is the input, and with `--on`. `submit`, `create` and `edit` do not take it.

## Labels

```sh
job submit --label team=build --label ticket=4711 -- make
job queue create development/builds --label tier=1
job queue set development/builds --label owner=ci
job queue unset development/builds label-owner
job list --label team=build
job queue list --label tier=1
```

A Job, a Queue and a Group carry at most 32 labels each. A key has 1 to 63 characters from `a-z`, `0-9`, dot, underscore and dash; a value has at most 255 bytes and no control characters. Labels classify without giving an object a second parent. They never affect scheduling, limits or any other policy, and a Queue or Group does not pass its labels on to what it contains. `set` adds to the labels already there; `unset PATH label-KEY` removes one and `unset PATH labels` removes all. Labels appear in `show`, `status` and `list` and in every JSON record.

## Moving a waiting Job

```sh
job move 42 --queue development/releases
```

`job move` puts a held or queued Job into another Queue. The destination must exist and be open, and its defaults, constraints and priority bounds are applied and checked as at submission. The Job keeps its number, attempt, submission time and accumulated waiting credit; a move never adds credit. A starting, running or ended Job is refused with a message that names its state. Each request is recorded by `job audit` under the action `move`. `job edit` remains the way to replace the command itself.

## Explaining settings

```sh
job explain 42
job explain development/builds
job explain development/builds memory-max --json
```

`job explain ID` says why a Job waits. When a change of policy leaves a queued Job unable to start, the reason names the resource and the object, for example `memory admission budget 104857600 at development`.

`job explain PATH [KEY]` explains the settings of a Queue or Group, one of them or all:

```text
Queue development/builds (#4)
cores: configured 4000; effective 1000; supplied by development
  ancestor development sets a lower cores: 1000; the lower one applies
max-running: configured unlimited; effective 2; supplied by development
  unlimited here, but ancestor development still bounds this subtree: max-running 2
```

A key is an option name without dashes. Values are shown as they are stored: bytes, thousandths of a CPU, milliseconds. A reason follows where a setting cannot take effect as written: an ancestor sets a lower ceiling, an explicit `unlimited` is still bounded by an ancestor, a default for each Job exceeds what the subtree may hold together, a default request exceeds an admission budget above it, or the host lacks the cgroup controller. `--json` prints `key`, `option`, `configured`, `effective`, `source`, `takes_effect` and `reasons` for each setting.

## Process count and write budget

`--pids-max N|unlimited` limits the processes and threads of a Job through the cgroup control `pids.max`, in the same family as `--memory-max`: it reserves nothing, needs a delegated `pids` controller, is read back into `applied_resources`, has the per-Job default `--job-pids-max` and the subtree form `--pids-max` on a Queue or Group, and can be changed with `job update`. `job host --json` lists `pids.max` under `resource_controls`. The earlier `--pids N` keeps its meaning: it counts N at admission and limits the Job to N, also on a host without cgroups, where the service watches the count. Giving both with different values is refused.

`--write-budget SIZE` stops a Job once it has written more than SIZE bytes; `--disk` is the earlier spelling. It is a count, not a file system quota. The service sets no quota, and `job host --json` reports `"filesystem_quota": {"supported": false, "reason": "..."}`.

## Help

`job`, `job help` and `job --help` print one screen: the commands by section and where to read on. `job COMMAND --help`, also with `-h` and for two-word commands such as `job queue set --help`, prints the usage line, every option with its value, the operands and the exit statuses of that command. It needs no running service and works wherever the option stands before `--`. `job help COMMAND` prints the same. `job help --all` prints the long text of every part, and `job help --syntax` prints the reference kept in [reference/syntax.md](reference/syntax.md).

All of it comes from one command definition, so help, completion and the reference list the same options. An option a command does not have is refused the same way everywhere:

```text
job: unknown option `--colour` for job wait; see job wait --help
```

Options stand before operands or after them; `--` ends the options only where a command follows it (`run`, `submit`, `create`, `edit`) and for `wait`. `log` keeps its own reading of arguments, because a word after `grep` may begin with a dash, and so do the earlier Queue spellings `queue`, `queue add`, `queue clear`, `queue rm` and `queue set NAME`. `cancel`, `doctor`, `audit`, the `state` subcommands and `queue pause|resume` are checked like the rest; `cancel` no longer accepts and ignores the options of `run`.

## Completion

```sh
job completion bash > ~/.local/share/bash-completion/completions/job
job completion fish > ~/.config/fish/completions/job.fish
job completion zsh > ~/.zfunc/_job    # with fpath+=(~/.zfunc) before compinit in ~/.zshrc
```

The scripts complete commands, subcommands, options and listed values such as `--net`, `--stream`, `--cap-drop` and signal names without the service. For a Job ID or a Queue or Group path they ask the service through a query that gives up after about a third of a second and then offers nothing. Commands that act on waiting or running work are offered the held, queued and running Jobs. `show`, `status`, `logs`, `log` and `attempts` are offered completed Jobs as well, and `retry` and `remove` only completed ones, the most recent 200.

## Sets of Jobs

`cancel`, `remove`, `release`, `retry`, `suspend`, `continue`, `signal`, `reprioritize`, `move` and `wait` take a set of Jobs where they took one ID:

```sh
job cancel 1-10
job cancel 1,4,7-9
job cancel 1-3 8 12-14
job cancel 1-10 --dry-run
job remove 1-10
job release 1-3
job wait 1-3 --timeout 5m
job list --id 2-4
```

A set is an ID, an inclusive range `A-B`, a comma list of IDs and ranges, or several such operands, which are united. Duplicates are removed and the order is ascending. There is no open range: `5-` is refused, so that a typo cannot select everything. A reversed range such as `10-1`, an empty item, zero, anything that is not digits, and a set that names more than 10000 IDs are usage errors with status 125, and nothing happens. The bound counts the IDs as written.

The service resolves the whole set in one request while it holds its lock. An ID inside a range that does not exist is skipped silently. An ID named on its own that does not exist is reported for that ID. A Job that cannot take the action (already ended for `cancel`, not held for `release`, not completed for `remove`, and so on) is reported with the reason and does not stop the others.

One plain ID answers exactly as before. A set answers one line per ID:

```
1: cancelled
2: already ended (succeeded)
3: stopping
9: no such Job
```

When more than 20 IDs behaved the same, the answer is one summary line instead, for example `cancelled: 1-27, 29; already ended (succeeded): 28; no such Job: 40`. `--json` prints one object:

```json
{"schema_version": 1, "kind": "job_set", "action": "cancel", "set": "1-3,9", "dry_run": false, "selected": 3,
 "results": [{"id": 1, "outcome": "done", "result": "cancelled", "state": "cancelled", "attempt": 1},
             {"id": 9, "outcome": "missing", "result": "no_such_job"}],
 "operation": "6b9e..."}
```

`outcome` is `done`, `pending`, `refused`, `failed` or `missing`; `result` is a word for what happened (`cancelled`, `stopping`, `already_ended`, `removed`, `released`, `retried`, `suspended`, `continued`, `signalled`, `reprioritized`, `moved`, `refused`, `failed`, `pending`, `no_such_job`, and for `wait` the state the Job ended in or `still_running`); `reason` carries the text of a refusal. `selected` counts the existing Jobs the set selected. `--format json` wraps the same object in the envelope, with the command as `kind`.

The exit status of a set is 0 when every selected existing Job took the action, 1 when at least one was refused or failed, when an ID named on its own does not exist, or when the set selects no existing Job at all (which standard error also says), 75 when nothing was refused and a change is still pending, and 125 for a usage error or a service failure.

`--dry-run` resolves the set, answers what would happen to each Job with `(dry run, nothing changed)` on every line, and changes nothing. For `cancel` and `remove` it runs the checks of the real request; for the other commands it looks at the state of each Job, and a refusal that depends on something else, such as a closed Queue, appears only when the command runs. `--dry-run` with one plain ID gives the set answer too, except on `remove`, whose preview of one ID stays as it was.

`cancel` records the whole set as one cancellation operation, the same kind `queue cancel --recursive` records, so a crash of the service in the middle is completed at the next start. Running Jobs are stopped the graceful way and answer `stopping`; `job wait` on the same set sees their end. `remove` removes every Job of the set that can be removed in one transaction with one receipt. `release`, `retry`, `signal`, `reprioritize` and `move` are done Job by Job in ID order inside the service; each Job's change is saved on its own. `suspend` and `continue` wait for the kernel to confirm each Job, within `--timeout`.

`wait` with a set waits until every Job of it has ended and prints one line per Job and none of their output; `--summary` belongs to one Job and is refused. `--timeout` bounds the whole wait. The exit status is the first one in ID order that is not 0; a Job still running at the deadline counts as 75 and a missing ID named on its own as 1.

`status`, `show`, `explain`, `logs`, `log`, `attempts`, `attach`, `edit` and `update` keep one ID, because each answers with one record or one stream or changes one specification. `job list --id SET` lists the Jobs of a set, in every state unless `--state` says otherwise.

The audit journal holds one record for a set request, with the set as written as its target and the count of each result, for example `error: cancelled 7, already_ended 1, no_such_job 1`. Dry runs are not recorded. Lifecycle events are one per Job, as always.

## List and show

`job list` prints the held, queued and running Jobs, one per line, as `job queue` does below its Queue lines; `--queue PATH` keeps the Jobs of one Queue and `--json` prints `{"schema_version":1,"jobs":[...]}`. `job show ID` is `job status ID`.

```sh
job list --all
job list --state failed,cancelled --limit 20
job list --label team=build --queue development/builds
job list --all --format tsv
job list --id 2-4,9
```

`--all` includes completed Jobs. `--state` selects among `held`, `queued`, `starting`, `running`, `suspended`, `stopping`, `succeeded`, `failed`, `cancelled` and `lost`, separated by commas. The most recent 200 matching Jobs are listed; `--limit N` changes that, up to 10000. When more Jobs match than were listed, the listing says so on standard error and JSON carries `"more": true`. The service reads at most 50000 completed records for one listing.

`--id SET` keeps the Jobs of a [set](#sets-of-jobs); with it and without `--state`, Jobs in every state are listed.

With `--all`, `--state`, `--label`, `--limit`, `--id` or `--format tsv` the text form is a table:

```text
ID  STATE      QUEUE    SESSION  EXIT  LABELS       COMMAND
1   succeeded  default  build    0     -            make
2   failed     default  build    3     team=build   make check
3   held       default  build    -     -            make install
```

`--format tsv` prints a header line and one line per Job, with tab-separated columns in this order:

| Column | Content |
|---|---|
| `id` | the Job number |
| `attempt` | the attempt number |
| `state` | one of the ten state names |
| `queue` | the Queue path |
| `session` | the session label |
| `priority` | the admission priority, empty when unset |
| `submitted_ms`, `started_ms`, `finished_ms` | Unix time in milliseconds, empty when it has not happened |
| `exit_status` | what `job wait` would return for an ended Job, empty otherwise |
| `labels` | a JSON object |
| `command` | a JSON array of the arguments |

`job queue list --format tsv` and `job group list --format tsv` print `id`, `path`, `kind`, `paused`, `closed` and `labels` in the same way. Column names, state names and kinds are identifiers and are never translated. No column holds a tab, because the two free-text columns are JSON.

A record written before the outcomes were told apart carries the state `Finished`. It is listed as `succeeded` or `failed` by its exit status, or as `cancelled` or `lost` when its stop says so.

## Structured output

Every command that takes `--json` and prints one document also takes `--format json`. It prints one line:

```json
{"schema_version":1,"kind":"queue_show","data":{"objects":[],"schema_version":1}}
```

`kind` names the command (`run`, `wait`, `status`, `show`, `list`, `attempts`, `queue_show`, `group_set` and so on; the reference names each) and `data` is what `--json` prints for that command. `--json` alone prints what it printed before. `--format text` is the default. `logs --json` prints a stream of records, one JSON object per line, and has no envelope. `audit --json` is such a stream too; `audit --format json` prints one envelope whose `data` holds `schema_version`, `entries` and `unreadable`. `submit --json` prints the Job number as before and `submit --format json` wraps the Job record. `cancel`, `release`, `move`, `doctor` and `queue|group pause|resume` have an envelope as well. `list`, `queue list` and `group list` also take `--format tsv`.

When the command fails before it has a document, `run` and `wait` keep their result shape with `outcome` set to `usage_error` or `service_error`, `exit_status` 125 and the message in `error`. Other commands print `kind` `error` with `command`, `outcome`, `exit_status` and `error`. The message also goes to standard error. `usage_error` means the arguments were refused before any request reached a service: by the shared check, by a missing or surplus operand, or by a parser that could not read a value. `service_error` means a service was asked and refused, or could not be reached. Identifiers are never translated.

## Colour

Output carries no colour and no terminal formatting, whether it goes to a terminal or is redirected. No option and no environment variable changes that. The bytes a Job writes are passed on as they are.

## Exit statuses

| Status | Meaning | Commands |
|---|---|---|
| 0 | success | all |
| the Job's own status, 128 + signal | the Job ended by itself | `run`, `wait`, `show`, `status`, `attach` |
| 1 | the Job was cancelled or stopped; a request was refused or a member failed; a Job of a [set](#sets-of-jobs) was refused or does not exist | `run`, `wait`, `show`, `status`, `cancel`, `suspend`, `continue`, `remove`, `update`, `resource-update` and their Queue and Group forms; with a set also `release`, `retry`, `signal`, `reprioritize` and `move` |
| 75 | a deadline passed, the Job or a change is still pending, the service stayed unavailable, or `run` left the Job running after an interrupt, a hangup or a closed output | the same commands |
| 125 | service failure, lost Job, start error, or usage error | all |

A Job may itself exit with 1, 75 or 125; the `outcome` of `run --format json` and `wait --format json` tells the cases apart. `job COMMAND --help` lists the statuses of one command.
