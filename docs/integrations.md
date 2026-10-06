# Automation

job is used the same way by people, scripts and automated systems: through the `job` command and its documented output. This page describes four additional pieces for a program that runs shell commands on somebody's behalf, such as a build bot or an agent: the diagnostic summary, the classification of a command line, the command policy, and the hook that puts the last two together. None of them is needed for anything else in the documentation, and none is active unless you set it up. The commands `job hook`, `job classify` and `job policy` are left out of `job help`; the [syntax reference](reference/syntax.md) lists them under Service and tools.

job contains nothing that is specific to one calling product. A product that wants to call the hook needs a small adapter that translates its own format to the question below and the answer back. Such adapters are separate projects and are not part of job.

## The diagnostic summary

`job run --summary` and `job wait --summary ID` print a short view of a finished Job instead of its output: one header line with the exit status, the running time and peak memory, a line saying why if the service stopped the Job, and then the part of the output that matters.

- Output of at most 35 lines is shown whole.
- In longer output the summary recognizes Rust compiler diagnostics and the results of Rust's test harness by their content, and prints each error as one line with file, line and column, each failed test as one line, and one line of totals.
- For other output of a failed command it shows lines around error words, then the last lines.
- The last line says how long the log is and how to ask for the rest.

The summary never changes the command or its arguments. The full output stays available:

```sh
job log 42 errors
job log 42 grep TEXT
job log 42 lines 120..160
job log 42 tail 50
job log 42 full
job logs 42
```

The recognition of compiler and test output is specific to Rust's tools. For anything else the summary is a generic excerpt. Scripts that need a dependable format should use `--json` or `--format json`, not the summary.

## The session label

`--session NAME` attaches a label to a Job, and `job cancel --session NAME` passes one with a cancellation. Without the option the label is taken from the environment variable `JOB_SESSION`, and without that it is `unnamed`. The label describes who submitted a Job. It authorizes nothing.

## Classifying a command line

`job classify` says whether a command line is heavy enough to be worth sending through the service. It reads one line per command on standard input, either the command itself or a JSON object with `command`, and prints one line per command. It runs nothing.

```sh
echo 'cargo test' | job classify
echo '{"command": "ls -la && git status"}' | job classify
```

The first prints `route`, a tab, and the reason, `it runs cargo`. The second prints `direct`.

A line is `direct` when every program in it is on a short list of file, text and query programs (`ls`, `cat`, `grep`, `sed`, `find`, `cp`, `mv`, `rm` and the like), a `git` command other than `gc`, `clone`, `fsck`, `repack` and `bisect`, a `cargo` command that compiles nothing, or `job` itself. Anything else is `route`: a build, a test run, an interpreter, a loop, an unknown program. The line is read as a shell would read it, so pipelines, `&&`, loops, command substitutions and `bash -c '...'` are looked into.

## The command policy

The command policy is a list of rules against commands that are destructive when written carelessly. **job ships no rules.** Without a policy file the policy is empty and forbids nothing. The rules are yours: you write the file, you edit it, and job reads it again at every call, so a change takes effect with the next command that is checked. Nothing is rebuilt or reloaded.

The policy judges the text of a command that is about to run. It is a guard against mistakes, not a security feature: it does not restrict what a running Job can do. The controls that do are in [security controls](security-controls.md).

### Where the file is

The file is `policy.json` in the configuration directory, beside `config.toml`:

| For | File |
|---|---|
| a user | `$XDG_CONFIG_HOME/job/policy.json`, by default `~/.config/job/policy.json` |
| the system | `/etc/job/policy.json` |

A user's own file is used when it exists, otherwise the system's. With `JOB_SYSTEM=1` only the system's file is read. When `JOB_CONFIG` names the configuration file, only `policy.json` in the same directory is read. One file applies; two files are never merged.

### The format

The file is one JSON object. Every field is optional.

| Field | Type | Meaning |
|---|---|---|
| `rules` | object | the rules in force: each key is a rule name from the table below, each value an object with the two texts `forbids` and `instead` |
| `protected_paths` | array of strings | the operands the rule `rm-protected` guards, compared as written, with a trailing `/` ignored |
| `work_root` | absolute path | a directory whose entries each belong to the caller of the same name; used by `other-worktree` |
| `repos_root` | absolute path | a directory of shared checkouts that belong to no caller; used by `other-worktree` |
| `sudo_callers` | array of strings | the caller names the rule `sudo` lets through |
| `push` | array of objects | each with `caller`, a caller name, and `remotes`, the remote names the rule `git-push` lets that caller push to |

A rule is in force only when `rules` names it. `forbids` and `instead` are the two halves of the refusal a caller reads: `job: denied by rule NAME: FORBIDS. Instead: INSTEAD.` Write them for whoever, or whatever, will read the refusal.

| Rule | Refuses |
|---|---|
| `rm-recursive-force` | `rm` with both a recursive and a force flag, in every spelling, also behind `xargs` and `find -exec` |
| `rm-protected` | a recursive `rm` of an operand listed in `protected_paths` |
| `empty-variable` | `rm`, `mv`, `truncate`, `shred`, recursive `cp`, `chmod` and `chown`, `dd of=` and `find -delete` whose path holds a variable or a substitution, unless it is written `${NAME:?}` |
| `find-root` | `find` starting at `/` |
| `kill-by-pattern` | `pkill` and `killall` |
| `no-verify` | `git` with `--no-verify`, and `git commit -n` |
| `pipe-to-shell` | a line that downloads with `curl` or `wget` and pipes into a shell |
| `sudo` | `sudo` and `doas`, unless the caller's name is in `sudo_callers` |
| `git-push` | `git push`, unless `push` names the caller and the remote |
| `other-worktree` | a `git` command that changes state (`checkout`, `reset`, `commit`, `merge`, `stash` and others) in a directory under `repos_root`, or under an entry of `work_root` that does not carry the caller's name |

The last three rules compare with the name of the caller, which `job hook` receives as `caller_name` and `job policy` as its operand. A caller without a name is never in a list.

A small example with two rules:

```json
{
  "rules": {
    "find-root": {
      "forbids": "find starting at /",
      "instead": "find in the one directory you mean"
    },
    "pipe-to-shell": {
      "forbids": "downloading with curl or wget and piping it into a shell",
      "instead": "download to a file, read it, then run it on purpose"
    }
  }
}
```

### Checking the file

```sh
job policy --show
```

prints the file in use and the rules in force, or that there is no file and nothing is forbidden. `job doctor` reports the same in its `policy` check.

A file is refused, with its path and the reason, when it is not JSON, has a field that does not exist, names a rule that does not exist, leaves `forbids` or `instead` empty, gives a relative directory, names `other-worktree` without `work_root` or `repos_root`, or names `rm-protected` without `protected_paths`. While the file is refused, `job policy` ends with status 125, the `policy` check of `job doctor` fails, and `job hook` refuses every command with the reason. A guard that somebody asked for is never silently absent.

### Asking for a verdict

`job policy [NAME]` prints the verdict for each line on standard input and runs nothing. A line is a command, or a JSON object with `command` and optionally `cwd` and `caller_name`. `NAME` is the caller's name for lines that carry none.

```sh
echo 'find / -name core' | job policy
```

With the example above this prints `deny`, a tab, and `find-root`. A command no rule refuses prints `allow`.

## The hook

`job hook` answers one question for a program that is about to run a shell command line: may it run as it is, must it be refused, or should another line run in its place. It reads one JSON object on standard input, writes one JSON object on standard output and ends with status 0. It never runs the command.

### The question

| Field | Type | Meaning |
|---|---|---|
| `command` | string, required | the command line, as a shell would be given it |
| `cwd` | string | the directory the command would run in; default: the directory `job hook` runs in |
| `caller` | string | a label for the caller, such as a run or conversation ID; it becomes the session label of a rewritten command and is written to the denial log; default `unnamed` |
| `caller_name` | string | the name the command policy compares with `sudo_callers`, `push` and the entries of `work_root` |
| `detached` | boolean | true when the caller already runs this command without waiting for it; default false |

Other fields are ignored.

### The answer

| Field | Present | Meaning |
|---|---|---|
| `decision` | always | `allow`, `deny` or `rewrite` |
| `reason` | with `deny` and `rewrite`; with `allow` only when there is something to tell | a sentence for whoever asked |
| `rule` | with `deny` by the command policy | the name of the rule; `policy-file` when the policy file is refused |
| `command` | with `rewrite` | the line to run instead |
| `detach` | with `rewrite`, always true | the line is meant to be started without waiting for it; its output is the Job's summary |

```sh
echo '{"command": "ls"}' | job hook
```

prints `{"decision":"allow"}`.

```sh
echo '{"command": "cargo test", "caller": "run-7"}' | job hook
```

prints, with a running service, a `rewrite` whose `command` is `job run --session 'run-7' --budget none --shell bash --summary -- 'cargo test'` followed by a guard that keeps the exit status and moves the shell out of a working directory that no longer exists.

The hook decides in this order:

1. If the policy file is refused, the answer is `deny`.
2. If the command policy forbids the command, the answer is `deny` with the rule. The denial is appended to `denials.jsonl` in the state directory, with addresses of proxies shown without user and password.
3. If the line would be rewritten and starts with `cd` or `export`, the answer is `deny`, and the reason names the two calls to make instead. Detached, the `cd` would change nothing for the caller's shell.
4. A `job run` or `job wait` that would hold the caller is rewritten: `--budget none` is added to a `job run` that names no budget, and the guard is appended. With `detached` true it is allowed as it is.
5. A line that `job classify` calls `route` is rewritten to a `job run` as above. If the service does not answer within two seconds, the answer is `allow` with a reason that says so.
6. Everything else is `allow`.

Input that is not a JSON object with a `command` ends with status 125, a message on standard error and nothing on standard output.

## What these are not

- They are not required. Every capability in the [user guide](user-guide.md) works without them.
- They are not a security boundary. A hook that a caller's own configuration installs can be removed by whoever controls that configuration.
- The summary and the list of programs that `job classify` calls direct may change between minor releases; the release notes will say so. The fields of the hook's question and answer and the format of the policy file are kept.
