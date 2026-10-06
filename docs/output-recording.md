# Output recording and live clients

```sh
job run -- sh -c 'printf "stdout\n"; printf "stderr\n" >&2'
job submit -- make
job logs 42 --follow
job logs 42 --stream stderr
job logs 42 --attempt 1 --raw > saved-output
job logs 42 --json
```

## Execution and reading

Native pipe `run` delivers original stdout bytes to stdout and stderr bytes to stderr as the supervisor observes them. It returns the command outcome. Application buffering still applies: job cannot flush the application's own buffers. `--summary`, `--json` and `JOB_CLI_COMPAT=legacy` retain final result views. Plain `wait` remains quiet. `log` retains the previous combined-log query interface.

`logs` without follow reads through a snapshot of the last complete record available at invocation. `--follow` continues until the selected attempt ends. The attempt defaults to the current one at attachment and stays pinned across retry and archival. `--attempt N` selects retained history. Removing the Job while reading produces an error, not a switch to other work.

`--stream stdout` and `--stream stderr` require a new pipe recording. `combined` selects workload output in observation order; `all` also includes transport diagnostics. `diagnostic` selects transport diagnostics alone. A PTY merges output in the terminal and is recorded as `terminal`; historic logs are marked `combined`. Neither can retrospectively supply separated descriptors. Capture order across pipes is observation order, not proof of the application's original write order. Local capture times clamp backward clock movement; forwarded remote timestamps remain those of the originating host and may differ from local diagnostic time. Sequence numbers define recorded order across clock domains.

Default logs rendering preserves ASCII, newline and tab, escaping other bytes as `\xNN`, including ESC, carriage returns, invalid UTF-8 and non-ASCII bytes. This prevents retained terminal control sequences from executing. Use `--raw` for exact bytes, including UTF-8 text and terminal programs. Native `run` is raw by design. `--json` emits one schema-one JSON object per record with Job ID, attempt, sequence, capture time (`at_ms`), stream and a byte array. Historic capture times are null; their synthetic read sequence is not a historical event ID.

`--since-ms N` and `--until-ms N` select inclusive Unix epoch milliseconds. Historic logs reject time selection. `--grep TEXT` filters independently per source, across chunk boundaries; lines longer than 65536 bytes are treated as bounded fragments. A match crossing a fragment boundary is not guaranteed. Filtering and JSON are mutually exclusive. Retention gaps remain visible even with source or time selection.

## Limits and failures

Without an output quota the journal stores up to 64 segments of at most one MiB each, keeping the first 32 and latest 32 segments of all streams together. A retired segment is deleted once the metadata that no longer lists it is published, so further segments, at most 64 per recording, can exist for up to a tenth of a second. Record headers consume capacity. The legacy combined head/tail log is retained separately for compatibility, so total output storage is larger than the journal alone.

## Output quotas

```sh
job run --output-head 4M --output-tail 16M -- make
job queue set --job-output-head 1M --job-output-tail 1M builds
job queue unset builds job-output-tail
job status 42
```

`--output-head SIZE` and `--output-tail SIZE` say how much of the beginning and of the end of each recorded stream is kept. Sizes use K, M and G as binary units and lie between 1M, one journal segment, and 1G. There is no `unlimited`: a recording without a bound can fill the disk the service and every other Job depend on, so a large explicit size is the way to keep more. The upper bound of 1G per part keeps the segment list in a recording's metadata, which is rewritten at every segment change and read by every poll of a follower, below one MiB, and matches the default service budget.

Each of the two values resolves on its own, through the same precedence as the other launch defaults (see `execution-profiles.md`): the Job's option, the Queue and Group defaults (`--job-output-head`, `--job-output-tail`) and pinned execution profile values (`output_head_bytes`, `output_tail_bytes`, in bytes). Where none of them sets a value, the service configuration does:

```toml
[output]
head_bytes = "4M"
tail_bytes = "16M"
budget_bytes = "2G"
```

Where no level sets either value, the recording is made exactly as before and its metadata has the previous shape. Where only one value is set, the other is the built-in 32 MiB. The resolved values are stored in the Job's declaration and their origins in `resource_sources` (`Job`, `Object` with the Queue or Group path, `Preset`, or `Service`). A retry resolves again; earlier attempts keep what they ran with.

With a quota, every stream (stdout, stderr, terminal, diagnostic) has its own head and its own tail, so a noisy stdout cannot push out the beginning or the end of stderr. The head holds the first bytes up to the size, counted as stored: each recorded chunk costs a 32 byte header, so a program that writes many small pieces keeps fewer payload bytes than one that writes large ones. The tail keeps at least the last SIZE as stored and at most two segments (2 MiB) more, because whole segments are retired. The combined `output.log` uses the same two sizes for the combined text.

`job status ID` prints a note with the quota, where each value came from, and the bytes removed per stream; `job status ID --json` has the values under `spec.declared`, the origins under `resource_sources` and the counts under `result.output_retention` (`streams` with `written_bytes` and `dropped_bytes`, `omitted_bytes` for bytes lost before they reached this recording, `trimmed_bytes` after a budget trim). The counts appear when the attempt ends. `job attempts ID` shows the same note per attempt. A Job without a quota shows the note only when something was removed.

`job logs` prints one notice on stderr at the place where bytes are missing, for example `job: output records were removed by retention (stream=stdout, records=41, bytes=2457600; kept per stream: at least the first 1 MiB and the last 1 MiB)`. With `--json` the gap record carries a `gap` object with `stream`, `records`, `bytes` and `quota`. Following continues across a gap. A follower that falls behind a fast writer can meet more than one gap; a snapshot of a finished recording has at most one per stream.

## Service budget

`budget_bytes` in the `[output]` table bounds the output the service keeps in total; the default is 1 GiB. The service counts the journal segments and `output.log` of every attempt, including Jobs still running, each time a Job ends. While the total is above the budget it removes whole recordings of completed attempts, oldest Job first. A running Job's recording is counted and never removed, so the total can exceed the budget while large Jobs run; their own quotas bound them. A budget smaller than one finished recording removes that recording as soon as its Job ends.

A removed recording stays visible: `job logs ID` prints `job: the output of this attempt was removed to keep the service within its output budget (bytes=N)` and ends successfully without output, `job status ID` carries a note, and `result.output_retention.trimmed_bytes` and `trimmed_bytes` in `streams.json` hold the count. `job log ID` reports that the log cannot be read.

Slow clients consume retained files; they do not block the supervisor's collection. Falling behind retention produces an explicit gap record or stderr notice. Gap records contain two little-endian unsigned 64-bit values: omitted record count and omitted byte count. Zero means unavailable or inapplicable for that counter: terminal queue loss knows only the byte count, and a follower of a recording without a quota that fell behind inside the tail knows only the record count. Aggregate retired byte and record counts remain in `streams.json`.

The supervisor reads what a pipe holds, up to one MiB at a time, and writes it to the journal with one write per segment, before it reads again. The pipe keeps the kernel's default size of 64 KiB. Bytes inside the segment a reader already knows are therefore visible at once. A new segment becomes visible when the supervisor next publishes `streams.json`: at once if 100 ms have passed since the last publication, otherwise at the next pass of its loop after that, which runs at least every 50 ms. `job run` and `job logs --follow` therefore show output at most about 150 ms after the Job wrote it, plus their own polling. Terminal recordings publish at every segment as before. A reader of a recording with a quota that meets a jump in the sequence waits for the next publication before it reports a gap, so that output in a segment not yet published is never taken for retired output.

When output reaches the disk: recorded output is not part of what an acknowledged submission promises. While a Job runs, its output is written to the file system without a sync and reaches the disk with the kernel's ordinary writeback, typically within half a minute. When the streams end, the supervisor syncs every retained segment and the metadata before the result is recorded, so the output of a Job whose result is recorded is on disk. After a host crash the recording of a Job that was running may lack its last half minute or hold a segment shorter than its metadata says; such a recording is incomplete, state validation accepts it, and `job logs` prints what precedes the damage and reports `output recording is incomplete`. With an output quota the reader reports an invalid recording instead.

Recording failures are saved in metadata or the Job result's `output_error`. A completed command can have incomplete output; live readers report the recording error with status 125. Incomplete final records after a supervisor crash are not delivered as valid bytes. A service restart does not restart the supervisor or replace its recording. Backup/restore includes journal metadata and segments for all retained attempts. State validation checks identity, shape, bounds and file inventory; readers additionally validate framing and sequences. These checks detect structural damage, not arbitrary bit changes to payloads.

A closed output never cancels the Job: `job run` returns 75 and says that the Job goes on, and `job logs` ends with status 125. A finite `run --budget` returns 75 while work continues, identifying the Job on stderr. The first SIGINT or SIGTERM to a pipe-mode `job run`, which is what Ctrl-C sends, is forwarded to the processes of the Job unless `--on-interrupt` says otherwise; ending a `logs` or `wait` client does not signal the workload. The table is in [commands, shells and waiting](cli-execution.md#signals-disconnection-and-cancellation). Attached PTYs retain their existing terminal-input behavior. A blocked client output descriptor can delay that client's timeout; it cannot block the separate collector.

## Remote recordings and compatibility

A remote Job is recorded twice: by the destination's service, and again by the local supervisor from the frames it receives. The quota applies to each recording where it is made. The local service forwards the values it resolved, so the destination records with them as the Job's own; where nothing was resolved locally, the destination applies its own Queue and service defaults. A destination older than this feature ignores the values and records with the built-in retention. Gaps of the destination's recording arrive as gap frames and are counted locally under `omitted_bytes`; a gap frame does not name its stream.

Current protocol 20 retains the framed remote output introduced in protocol 17 with source tags, capture timestamps and an explicit completion marker. SSH stderr is recorded as diagnostics, never as workload stderr. Missing completion or malformed/partial frames marks output incomplete. Both peers require the matching protocol; live pipe submission checks the service's output capability before accepting work. Older merged recordings remain available without invented separation.

Current state schema 18 retains the output metadata introduced in schema 15.

## Reading as a member of a socket group

The recordings are files in the private state directory. A client that can read it, which is the service user, reads them directly and needs no running service. A member of a socket group cannot read it and gets the same output from the service over the control socket: `job run`, `job logs` with all its options and `job log` behave the same. See [operating the service](operations.md#output-and-terminals-for-a-socket-group).
