# Output recordings

## What it does

Records pipe stdout and stderr separately, combined terminal output and transport diagnostics. A versioned binary journal has bounded head/tail segment retention, capture timestamps and record sequences. Readers pin attempts, follow rotation and report retention gaps. Native run preserves original descriptors and bytes; logs supports snapshot/follow, source/attempt/time selection, bounded line filtering, safe rendering and explicit raw or JSON output. Remote transport frames retain source identity and an explicit end marker.

An output quota (`quota.rs`) resolves a head and a tail size per stream from the Job, its Queue and Groups, a profile and the service configuration, with recorded origins. With a quota the journal keeps one head chain and one tail chain of segments per stream (`split.rs`) and the reader merges them by sequence, naming the stream and the bytes of each gap. Without one the journal is written exactly as before. The service budget (`budget.rs`) counts segments and the combined log and removes whole completed recordings, oldest first, leaving a marked metadata file and a marked Job record. A Job directory whose record exists and cannot be read is counted whole, never trimmed and reported once in the service log and in `job host`.

## How to test

Run the stream_ CLI/service integration scenarios in watch_backend through job. They cover live binary output, selected streams, terminal escaping, follow/retry/restart, retention, disconnect/timeout, remote framing, incomplete recordings, PTY combined semantics and backup/restore. The output_quota_ scenarios cover a quota with gaps on stdout and stderr, the unchanged default, origins through Group, Queue, Job, unset and retry, a recording without quota metadata, the budget trim and a remote Job; output_quota_recordings_survive_backup_and_restore_and_reject_a_changed_quota in state_migration covers storage. Run tools/check and the terminal and delegated suites after supervisor changes. No new unit tests are added.
