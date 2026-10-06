# Terminal sessions

## What it does

Owns a persistent Linux PTY inside the execution supervisor. Clients of the same Unix user connect through a private socket discovered with `Attach`. Every attachment can write. The supervisor parses output with `vt100`, sends a screen snapshot followed by diffs, answers device/status queries centrally, and retains the terminal without attachments. Client input is serialized in arrival order, with no line ownership.

The shared dimensions are the minimum rows and columns of attached clients. With no clients, the last dimensions remain. The initial detached size is 24 rows by 80 columns. Dimensions are bounded to 200 rows by 500 columns; there are at most 64 attachments. Each client has at most 2 MiB of pending output, 64 KiB of buffered input, and a maximum frame body of 1 MiB. A slow or invalid peer is disconnected. The PTY input queue is bounded to 64 KiB. The output logger has a separate bounded queue of 32 chunks, each at most 8192 bytes. Logging may omit data if storage cannot keep up; the final log records the omitted count when storage permits.

The daemon advertises `terminal_protocol: 1` in Host responses and includes it in Attach responses. Clients check this capability before submitting a PTY job, preventing silent downgrade with older daemons. The local terminal transport is version 1: one byte message kind, four bytes big-endian body length, then body. Client messages are input (1) and size (2: two big-endian u16 values, rows then columns). Server messages are rendered output (3) and exit (4: big-endian i32). Size must be the first client message. No input is resent after disconnect. A new attachment receives a snapshot through the same ordered output queue as subsequent diffs. Output frames contain only the parser's generated display sequences, not arbitrary escape sequences from the workload.

The parser implements a subset of xterm terminal behavior. It does not provide graphics, clipboard integration, persistent scrollback, or complete xterm compatibility. Raw merged output is retained in the bounded job log. CLI log/status display escapes control characters for PTY jobs. Input is not separately recorded, but terminal echo and program output may contain entered text.

The socket and peer credentials restrict access to the service's Unix user. SSH access under that account uses the same daemon/state directory. Different Unix accounts are not admitted. There are no application roles, writer leases or additional credentials. Existing resource scheduling and limits remain in force.

The detach key is a client matter and not part of the terminal protocol. It resolves from `job attach --detach-key`, then `JOB_DETACH_KEY`, then `[terminal] detach_key` of the service configuration, which the client asks the daemon for, then Ctrl-]. Accepted keys are `ctrl-` with a letter or `]`, `\`, `^`, `_`, and `none`. The key followed by `d` detaches, the key twice sends it once, the key followed by anything else sends both. With `none` nothing is intercepted and closing the local terminal is the only way to detach.

## How to test

Run `cargo test terminal::` and `cargo test --test terminal_sessions` through the host's job service. The integration tests exercise two writers, shared resize, reconnect, daemon restart, cancellation, rejection of nonterminal jobs, and CLI detach with termios restoration, and the `detach_key_` cases cover the option, the environment, the configuration and `none` through a real PTY client. Test workloads have explicit bounded runtimes and private state directories.

For an interactive check, start a development daemon in a private `JOB_STATE_DIR`, run `job run --pty -- bash`, detach with Ctrl-] then d, and reconnect from two terminals using `job attach ID`. Stop the shell with `exit`.
