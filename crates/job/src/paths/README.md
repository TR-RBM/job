# File locations

## What it does

Derives the configuration file, the state, runtime and cache directories and the service socket from an environment passed in as a lookup function, for a user service (XDG variables with the specification's fallbacks) and a system service (`/etc/job`, `/var/lib/job`, `/run/job`, `/var/cache/job`). `JOB_CONFIG`, `JOB_STATE_DIR`, `JOB_RUNTIME_DIR` and `JOB_CACHE_DIR` override one location each; `JOB_SYSTEM=1` selects the system layout. Unset, empty and relative XDG values are ignored. Where no location can be derived the result is an error that names the variables to set; nothing falls back to `/tmp`.

The socket follows the first matching rule: `JOB_RUNTIME_DIR`; an explicit `JOB_STATE_DIR`, which keeps `daemon.sock` inside that state directory so that a relocated state never shares the account's ordinary socket; the system runtime directory; `XDG_RUNTIME_DIR`; otherwise the state directory. A client tries that socket and then `daemon.sock` in its state directory; a user client under the last rule also tries `/run/user/UID/job/job.sock`. It connects to probe only when more than one of them exists.

## How to test

`cargo test --test linux_integration` through job: `user_paths_follow_the_xdg_fallbacks_and_overrides`, `system_paths_and_missing_home_are_reported`, `the_runtime_socket_is_separate_and_a_stale_state_socket_is_removed`, `a_client_falls_back_to_the_socket_in_the_state_directory` and `a_default_user_service_binds_under_xdg_runtime_dir`. They read the locations through `job doctor --json` and through real test services. No unit tests.
