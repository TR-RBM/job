# Command policy file

## What it does

Finds, reads and validates the file that holds the command policy, and describes it for `job policy --show` and for the `policy` check of `job doctor`. The policy that is built in is empty and forbids nothing.

The file is `policy.json` beside the configuration file: `$XDG_CONFIG_HOME/job/policy.json` for a user, by default `~/.config/job/policy.json`, and `/etc/job/policy.json` for the system. A user's client reads its own file and, when there is none, the system's. When `JOB_CONFIG` names the configuration, only `policy.json` in the same directory is read. One file applies; files are not merged.

Every field is optional. `rules` maps a rule name to `forbids` and `instead`, the two texts of a refusal; a rule that is not named is not in force. `protected_paths` lists the paths the rule `rm-protected` guards, `work_root` and `repos_root` are the directories the rule `other-worktree` compares with, `sudo_callers` names the callers the rule `sudo` lets through, and `push` lists, per caller, the remotes the rule `git-push` lets through.

A file is refused, with its path and the reason, when it is not JSON, has a field that does not exist, names a rule that does not exist, leaves a text of a rule empty, gives a relative directory, or names `other-worktree` without a directory or `rm-protected` without a path. While a file is refused, `job policy` ends with an error and `job hook` refuses every command with that reason, so that a guard that was asked for is never silently absent.

## How to test

`cargo test --test linux_integration policy` through job: `without_a_policy_file_the_command_policy_forbids_nothing_and_says_so`, `a_policy_file_beside_the_configuration_is_applied_and_reported`, `a_user_policy_file_is_found_under_xdg_config_home` and `an_invalid_policy_file_is_refused_with_the_reason`. The rules themselves are unit tests in `policy.rs`, which read `tests/fixtures/policy.json`.
