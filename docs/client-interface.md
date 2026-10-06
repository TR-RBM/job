# The client interface

This document describes how a program other than the `job` command talks to a running job service: a terminal interface, an editor extension, a status bar. It is the first of three sets. A client and a stand-in service can be written from it alone.

## Scope

In this set:

- a greeting with the version range, the capabilities, the service's clock and what the peer may do;
- a subscription: a snapshot, then every change of Jobs, Queues, Groups and health;
- live use of the Jobs a client declares an interest in;
- listings of Jobs as compact rows in pages, in four named orders, with filters, counts and a jump to an ID;
- the tree of Queues and Groups with depth and use against limits;
- totals of the host: counts per state, reserved against the pool, recorded output against its budget;
- one Job in full, with what the kernel confirmed when its command started;
- the processes of a running Job, read on demand;
- recorded output from a position, forwards or backwards, with stream selection and totals;
- job's command table as data;
- carrying out a job command given as the words a person would type: every action on Jobs, Queues and Groups, with the saved environment of a Job kept on edit and retry;
- a passage, outside the version promise, for the service's own requests that this document does not yet describe.

Not in this set, and answered as an unknown request when asked for:

- lifecycle events and the audit journal. They are files in the state directory and are not on the socket.
- Errors with an identifier and in a language the client chooses. An error is one sentence in the language of the service's environment.
- An identifier on a changing request, pressure of one Job, fair-share standing, operations in progress, live network links, resolving a draft without creating a Job. These are the second set.
- The terminal on this connection, bytes read per device, network bytes per Job, run history. These are the third set.

Never part of it:

- Traffic of a Job on the host's own network is not measured.
- The service computes nothing for one particular screen: no order other than the four it names, no layout, no column chosen per client.
- The service keeps no history beyond what it keeps already.

## Finding the socket

The service listens on one Unix stream socket. A client finds it as the `job` command does. Variables that are unset or empty count as absent; `XDG_` variables that are not absolute paths count as absent.

The state directory is the first of:

1. `$JOB_STATE_DIR`;
2. `/var/lib/job` for the system service;
3. `$XDG_STATE_HOME/job`;
4. `$HOME/.local/state/job`.

The system service is selected with `JOB_SYSTEM=1`, or by whatever option the client offers for it.

The socket is the first of:

1. `$JOB_RUNTIME_DIR/job.sock`;
2. `daemon.sock` in the state directory, when `JOB_STATE_DIR` is set;
3. `/run/job/job.sock` for the system service;
4. `$XDG_RUNTIME_DIR/job/job.sock`;
5. `daemon.sock` in the state directory.

A client tries that socket, then `daemon.sock` in the state directory if that is a different path, and under rule 5 also `/run/user/UID/job/job.sock` with its own numeric user ID. When none of these paths exists it reports the first. When exactly one exists it uses that. When several exist it uses the first that accepts a connection, and the first existing one when none does. It looks nowhere else.

## Framing

Each side sends lines. A line is one JSON object in UTF-8, without a byte order mark, ended by one line feed (byte 10). A line contains no other line feed.

- A line a client sends is at most 4 194 304 bytes, the line feed included.
- A line the service sends is at most 16 777 216 bytes.
- Numbers are integers unless a field says otherwise. Byte counts, times and identifiers stay below 2^53.
- A string is text. Bytes a Job wrote are never put in a string; they are carried in base64 (RFC 4648, standard alphabet, with padding) in a field named `data`.

The first line on a connection is the greeting. After it the client sends requests and the service sends answers and, where asked for, pushed lines.

A request:

```json
{"id":7,"op":"jobs","args":{"limit":50}}
```

`id` is a number the client chooses, unique among its unanswered requests on that connection. A request without an `id`, or with one that is not a number, cannot be answered with `re`; see Errors. `op` names the request. `args` is an object and may be left out when a request takes none.

An answer carries `re` with that number and exactly one of `ok` and `error`:

```json
{"re":7,"ok":{}}
{"re":7,"error":{"message":"there is no Queue `nightly`"}}
```

A pushed line carries `push` with its kind and no `re`:

```json
{"push":"heartbeat","now_ms":1790000005000,"seq":4411}
```


Answers on one connection arrive in the order of their requests. Pushed lines arrive between them. A request that takes long holds back the answers behind it; a client that does not want that uses a second connection. An `output` with `follow` is the case that never ends by itself: every request sent behind it on its connection waits until the Job ends or `end` stops the output, so a client that follows output in one part of its screen gives that `output` a connection of its own. The one request handled out of order is `end`, which stops a running `output`.

Two requests answer with more than one line: `subscribe`, whose answer is followed by pushed lines until the connection ends, and `output`, whose answer is followed by lines that carry `re` and one of `record`, `gap`, `status` and `end`. Both are described below. Everything is lines; there is no binary framing in this interface.

A connection carries at most one subscription. A `subscribe` on a connection that has one, and an `output` request on such a connection, are refused with an error; the subscription stays. After a `resync` the connection has no subscription and `subscribe` is accepted again.

## Versions

The client interface has a version number of its own, a positive integer that starts at 1. It is not the number of the service's internal request protocol, which changes with most releases and which this interface is there to hide.

A service speaks a range of versions, `min` to `max`. A client sends the range it speaks in the greeting. The service chooses the highest version in both ranges and says which. When the ranges do not overlap the service says so with its own range and closes the connection.

What raises the version: removing or renaming a request or a field, changing the type, unit or meaning of a field, and changing a guarantee stated here.

What does not raise it: a new request, a new field in an answer or a pushed line, a new optional member of `args`, a new kind of pushed line, a new capability, a new value of a field whose list of values this document calls open.

So a client must:

- ignore members it does not know in any object the service sends;
- ignore a pushed line whose `push` it does not know, after taking its `seq` into account where it has one;
- learn whether a request exists from the list of capabilities, never from the version of job.

A capability is a short name. The capabilities of this set are the names of its described requests, `subscribe`, `interest`, `jobs`, `job`, `tree`, `totals`, `processes`, `output`, `commands` and `command`, and three more:

| Capability | Present when |
|---|---|
| `cgroup` | limits are enforced through a cgroup. Without it CPU time, throttled time, OOM counts and the counters of shared ceilings are not measured. |
| `socket_group` | a socket group is configured, so that other users may be connected. |
| `native` | the service offers the request `native`. This capability is not part of the version promise: what passes through `native` can change with any release of job, and a service may stop offering it. A client looks for it before every use and has a described request to fall back on, or says that the function is not available. |

## The greeting

The client sends:

```json
{"hello":{"versions":{"min":1,"max":1},"client":"example-top 0.3.0"}}
```

| Field | Type | Meaning |
|---|---|---|
| `versions.min`, `versions.max` | number | the range of client interface versions the client speaks |
| `client` | string, optional | name and version of the client, at most 128 bytes. It is used in the service's log only. A longer one, or one that is not a string, is answered with `{"error":{"message":"..."}}` and the connection is closed. |

The service answers:

```json
{"hello":{"version":1,"versions":{"min":1,"max":1},"job_version":"0.9.0","native_protocol":20,"capabilities":["subscribe","interest","jobs","job","tree","totals","processes","output","commands","command","native","cgroup"],"now_ms":1790000000000,"started_ms":1789990000000,"hostname":"build-3","service":{"mode":"user","uid":1000,"socket_rule":"XDG_RUNTIME_DIR","socket_group":null},"backend":"cgroup","enforcement":"enforced","peer":{"uid":1000,"pid":4412,"via":"owner","may":{"read":true,"change":true,"events":false,"audit":false}},"limits":{"line_bytes_in":4194304,"line_bytes_out":16777216,"page_rows":1000,"page_rows_default":200,"interest_jobs":500,"connections":32,"subscriptions":4,"retained_changes":8192,"heartbeat_ms":5000,"live_interval_ms":1000,"output_piece_bytes":4194304,"processes":4096,"idle_ms":60000,"indexed_ended_jobs":50000,"ids":10000,"poll_ms":5000}}}
```

| Field | Type | From | Meaning |
|---|---|---|---|
| `version` | number | new | the client interface version in force on this connection |
| `versions` | object | new | the range the service speaks |
| `job_version` | string | `version` of the host answer | version of job |
| `native_protocol` | number | `protocol` of the host answer | number of the internal request protocol, needed only for `native` |
| `capabilities` | array of strings | new | see Versions |
| `now_ms` | number | new | the service's clock when it wrote this line |
| `started_ms` | number | `health.started_ms` | when this run of the service started. It identifies the run. |
| `hostname` | string | `hostname` | the host as the service names it |
| `service.mode` | string | `service.mode` | `user` or `system` |
| `service.uid` | number | `service.uid` | the user the service runs as |
| `service.socket_rule` | string | `service.socket_rule` | which of the socket rules placed the socket |
| `service.socket_group` | string or null | `service.socket_group` | the configured socket group; null when there is none |
| `backend` | string | `backend` | `cgroup` or `watch` |
| `enforcement` | string | `health.enforcement` | `enforced` or `monitoring_only` |
| `peer.uid`, `peer.pid` | number | the socket's peer credentials | who the service takes the client to be |
| `peer.via` | string | new | `owner` when the peer is the service's user, `group` when it was admitted through the socket group |
| `peer.may.read` | boolean | new | may use the reading requests |
| `peer.may.change` | boolean | new | may use `command` |
| `peer.may.events`, `peer.may.audit` | boolean | new | may read lifecycle events and the audit journal on the socket. Both are false in this set. |
| `limits` | object | new | see Limits |

When the ranges do not overlap the service sends this line and closes:

```json
{"unsupported":{"versions":{"min":1,"max":1},"job_version":"0.9.0"}}
```

A service that was built before this interface answers the greeting with `{"Error":{"message":"unreadable request: ..."}}` and closes. A client that reads an object with the member `Error` in answer to its greeting knows that the service has no client interface.

A peer that is not admitted gets `{"error":{"message":"..."}}` and the connection is closed.

## Times, durations and sizes

Every time is a number of milliseconds since the Unix epoch, read from the service's clock, and its field name ends in `_ms`. A field ending in `_ms` that is a length of time says so in its description.

A client never subtracts a time of the service from its own wall clock. It keeps an offset instead: when a line with `now_ms` arrives, `offset = now_ms - local`, where `local` is a monotonic clock of the client read at that moment. The service's time at any later moment is `local + offset`, and a duration shown on a screen is that minus the time the service wrote. `now_ms` is in the greeting, in every answer of `subscribe`, `jobs`, `job`, `tree`, `totals` and `processes`, and in every `totals` and `heartbeat` line, so the offset is renewed at least every five seconds on a subscription. A client takes the newest.

Every size is a number of bytes. CPU quantities are in thousandths of a core (`cores_milli`) or milliseconds of CPU time (`cpu_ms`).

## Values that are absent

A measured value is never replaced by zero. Where a number may be missing, the field holds the number or one of three words, and its description says which words can occur:

| Word | Meaning |
|---|---|
| `"unknown"` | the service should know this and does not: a file could not be read, or no sample has been taken yet |
| `"not_measured"` | the service does not measure this in the way it runs, for example CPU time without a cgroup |
| `"not_applicable"` | the value has no meaning for this object, for example the init process of a Job that has no PID namespace |

A limit is a number or the word `"unlimited"`.

Fields taken over from job's records keep job's own convention, in which `null` stands for "none": no exit code yet, no limit declared, no prediction. Their descriptions say what `null` means for each.

## Enumerated values and sources

Every enumerated value this document describes is lowercase, with underscores between words, wherever it appears: in rows, in the Job record, in the tree, in pushed lines. job's records and its own protocol write some of them otherwise; this interface does not pass those spellings on. The one place that keeps job's own spelling is `data` in the answer of `command`, which is the command's documented JSON output unchanged.

| Value | Spellings |
|---|---|
| state of a Job | `held`, `queued`, `starting`, `running`, `suspended`, `stopping`, `succeeded`, `failed`, `cancelled`, `lost` |
| `backend` | `cgroup`, `watch` |
| kind of an object | `queue`, `group` |
| `stop_kind`, `stop.kind` | `memory`, `processes`, `disk`, `filesystem_floor`, `host_memory`, `memory_pressure`, `wall_time`, `cancelled`, `working_directory_gone`, `limit_changed`, `daemon_lost` |
| `output_mode`, `mode` | `pipe`, `pty` |
| stream | `stdout`, `stderr`, `terminal`, `combined`, `diagnostic` |
| `enforcement` | `enforced`, `monitoring_only` |

The lists of `stop_kind` and of streams are open.

Where a reservation says how its figure was arrived at (`cores_source`, `memory_source`, `disk_source`), the value is an object with `kind`: `unset`, `declared`, `default`, `history` with `runs`, `repository_history` with `runs`, `rule` with `tool`, or `after_stop` with `limit` in bytes.

The source of a setting, the place a value in force comes from, has one form everywhere: in `priority_source`, in `resource_sources`, in `confirmed.sources` and in the settings of the tree.

| Form | Meaning |
|---|---|
| `{"from":"job"}` | declared on the Job itself |
| `{"from":"object","id":7,"path":"builds/linux"}` | set on this Queue or Group |
| `{"from":"profile","definition":"batch@3","sha256":"...","selected_by":{"from":"job"}}` | supplied by this revision of an execution profile or scheduling class; `selected_by` is a source in the same form, `job` or `object`, and says who chose the profile |
| `{"from":"service"}` | the service's configuration |
| `{"from":"compatibility"}` | a value job fills in for records and commands of earlier releases |

## Requests

### subscribe

Starts the subscription of this connection. See The subscription.

### interest

Declares the Jobs this subscription wants live figures for. See Live use.

### jobs

A page of Jobs as rows.

| Argument | Type | Meaning |
|---|---|---|
| `order` | string, optional | `activity` (the default), `id`, `started` or `ended` |
| `filter` | object, optional | see below; absent or empty matches every Job |
| `limit` | number, optional | rows wanted, 0 to `limits.page_rows`; default `limits.page_rows_default`. With 0 the answer holds the counts only. |
| `cursor` | string, optional | continue from a cursor of an earlier answer with the same order and filter |
| `direction` | string, optional | `forward` (the default) or `backward`; with a cursor only |
| `at` | number, optional | a Job ID: answer the page around that Job. Not together with `cursor`. |
| `ids_only` | boolean, optional | answer the IDs of the matching Jobs and no rows. Not together with `cursor`, `at` or `limit`. |

A listing covers the Jobs in the service's index: every Job that has not ended, and the `limits.indexed_ended_jobs` most recently ended, 50 000 in this version. An ended Job older than those is in no order, matches no filter and has no place a cursor can reach. It is reached by its ID with `job`, and its output with `output`. When an ended Job leaves the index because newer ones ended, no change is pushed for it; a client that still holds its row keeps it until it next asks for the page. Every answer says how many such Jobs there are, so that a total is never short without saying so.

The orders, each total over the Jobs in the index, defined on fields of the row:

| Order | Sequence |
|---|---|
| `id` | by `id`, ascending |
| `activity` | first Jobs that are `starting`, `running`, `suspended` or `stopping`, by `id` ascending; then `queued` Jobs by `predicted_start_ms` ascending, those without a prediction after those with one, then by `waiting_since_ms`, then by `id`; then `held` Jobs by `id` ascending; then ended Jobs by `finished_ms` descending, then by `id` descending |
| `started` | Jobs with `started_ms` by `started_ms` descending, then `id` descending; after them the Jobs that never started, by `id` descending |
| `ended` | Jobs with `finished_ms` by `finished_ms` descending, then `id` descending; after them the Jobs that have not ended, by `id` descending |

There is no other order, and a client does not sort rows by anything else.

The filter. Every member is optional, and a Job matches when it matches every member given:

| Member | Type | Matches when |
|---|---|---|
| `states` | array of strings | the row's `state` is one of them |
| `subtree` | string | the Job's Queue is the Queue or Group with this path, or lies below it |
| `labels` | object of strings | the row's `labels` hold every one of these keys with the same value |
| `actor_uid` | number | the row's `actor_uid` equals it |
| `session` | string | the row's `session` equals it |
| `text` | string, at most 256 bytes | it occurs, byte for byte, in the row's `argv` joined with single spaces, in `queue`, in `session`, or in a label written as `key=value` |

All of these are decided on the row as it is sent, so a client can decide for a row it receives later whether it matches. `subtree` is compared with the row's `queue`; the empty string is the root and matches every Job that has a Queue. A member of the filter that the service does not know is refused with an error, so that a filter is never applied in part.

The answer:

```json
{"id":3,"op":"jobs","args":{"order":"activity","filter":{"states":["running","queued"],"subtree":"builds"},"limit":2}}
{"re":3,"ok":{"seq":4410,"now_ms":1790000001200,"order":"activity","matching":{"total":37,"counts":{"running":4,"queued":33},"ended_not_searched":0},"offset":0,"rows":[{"id":812,"revision":9,"attempt":1,"state":"running","queue":"builds/linux","queue_id":7,"actor_uid":1000,"actor_pid":30112,"session":"ci","labels":{"branch":"main"},"argv":["cargo","build","--release"],"argv_truncated":false,"priority":50,"priority_source":{"from":"job"},"idempotency_key":null,"submitted_ms":1789999900000,"waiting_since_ms":1789999900000,"admitted_ms":1789999900400,"started_ms":1789999900450,"finished_ms":null,"suspended":{"total_ms":0,"since_ms":null},"wall_limit_ms":3600000,"termination_deadline_ms":null,"predicted_start_ms":null,"waited_for":null,"reserved":{"cores_milli":4000,"memory":8589934592,"pids":512},"memory_max":8589934592,"stop_kind":null,"exit_code":null,"signal":null,"start_error":null,"usage":null,"network":"host","terminal":false,"on":null,"cgroup":"/sys/fs/cgroup/user.slice/job/jobs/812","confirmed":{"namespaces":[],"pid_namespace":false,"no_new_privs":false,"cap_drop":null,"seccomp_denied":0,"cpu_affinity":null}},{"id":815,"revision":2,"attempt":1,"state":"queued","queue":"builds/linux","queue_id":7,"actor_uid":1000,"actor_pid":30190,"session":"ci","labels":{},"argv":["make","check"],"argv_truncated":false,"priority":null,"priority_source":null,"idempotency_key":null,"submitted_ms":1789999950000,"waiting_since_ms":1789999950000,"admitted_ms":null,"started_ms":null,"finished_ms":null,"suspended":{"total_ms":0,"since_ms":null},"wall_limit_ms":null,"termination_deadline_ms":null,"predicted_start_ms":1790000400000,"waited_for":"waiting for resources or an unknown running-job end","reserved":{"cores_milli":2000,"memory":2147483648,"pids":256},"memory_max":"not_applicable","stop_kind":null,"exit_code":null,"signal":null,"start_error":null,"usage":null,"network":null,"terminal":false,"on":null,"cgroup":null,"confirmed":"not_applicable"}],"before":null,"after":"djE6YWN0aXZpdHk6..."}}
```

| Field | Type | Meaning |
|---|---|---|
| `seq` | number | the change sequence number this answer is valid at; see The subscription. 0 from a service without `subscribe` among its capabilities. |
| `now_ms` | number | the service's clock |
| `order` | string | the order used |
| `matching.total` | number | how many Jobs in the index match the filter |
| `matching.counts` | object | how many of them are in each state; a state with none may be left out |
| `matching.ended_not_searched` | number | how many ended Jobs the service holds records of outside the index. They were not compared with the filter and are in neither `total` nor `counts`. Zero when the index holds every Job. When it is not zero, a client says beside any total that older ended Jobs were not searched. |
| `offset` | number | how many matching Jobs come before the first row of this page in this order; null when `rows` is empty |
| `rows` | array | the rows, in the order |
| `before` | string or null | cursor for the page before the first row; null when the first row is the first match |
| `after` | string or null | cursor for the page after the last row; null when the last row is the last match |
| `found` | boolean | only with `at`: whether that Job is in the index and matches the filter. When false, `rows` is empty. An ended Job outside the index is not found here even though `job` returns it. |
| `cursor_expired` | boolean | only when true: the cursor can no longer be used; `rows` is empty and both cursors are null |

With `at`, the page holds the Job with that ID and up to half of `limit` rows before it.

With `ids_only`, the answer holds `seq`, `now_ms`, `order`, `matching` and two fields in place of the rows and cursors:

```json
{"id":14,"op":"jobs","args":{"filter":{"states":["failed"],"subtree":"builds"},"ids_only":true}}
{"re":14,"ok":{"seq":4410,"now_ms":1790000001250,"order":"activity","matching":{"total":3,"counts":{"failed":3},"ended_not_searched":0},"ids":[790,744,612],"ids_more":0}}
```

| Field | Type | Meaning |
|---|---|---|
| `ids` | array of numbers | the IDs of the matching Jobs in the order, at most `limits.ids`, 10 000 in this version |
| `ids_more` | number | how many more Jobs matched and are not in `ids`. A client that acts on a selection says so when this is not zero, and does not take `ids` for the whole. |

A cursor is opaque. A client stores it and sends it back unchanged, and reads nothing from it. It is bound to the order and the filter of the request that produced it and to the run of the service; sent with another order or filter it is refused as an error, since that is a mistake of the client. It expires when the service restarts, and the service may expire one that is older than ten minutes. A client learns that from `cursor_expired` and asks again without a cursor, with `at` set to the row it was at when it wants to stay there.

A page asked for with a cursor can be empty, when nothing lies in that direction any more. Its `before` is then the same place when rows lie before it and null when none do, and its `after` likewise.

A cursor names a place in the order, not a row. Rows that changed between two pages can appear in both or in neither; the client makes the pages consistent by applying the changes of its subscription that have a `seq` above the page's `seq`.

The row. Every field is always present.

| Field | Type | From | Meaning |
|---|---|---|---|
| `id` | number | `id` | the Job |
| `revision` | number | new | a counter of this Job that rises by at least one every time the service saves the Job's record. Two rows of one Job with the same `revision` are the same; the higher one is newer. A client that keeps the full record of a Job asks for it again when the `revision` of its row rises. It also rises by one, without a save, when the service learns from the supervisor that the command has started, so that a row with `started_ms` set always has a higher `revision` than the row of the same attempt without it. It is 0 for a record that a release before this interface saved and that was not saved since. Two fields of the row of a Job that has not ended are current without a save and so can differ between two rows of one `revision`: `predicted_start_ms` and `waited_for`. |
| `attempt` | number | `attempt` | the current attempt, from 1 |
| `state` | string | the state names of `job list --state` | `held`, `queued`, `starting`, `running`, `suspended`, `stopping`, `succeeded`, `failed`, `cancelled` or `lost` |
| `queue` | string or null | `spec.queue` | path of the Queue; null only in records from before Queues had paths |
| `queue_id` | number or null | `queue_id` | identifier of the Queue, the same as `object.id` in the tree |
| `actor_uid`, `actor_pid` | number or null | `actor_uid`, `actor_pid` | who submitted; null when the record does not say |
| `session` | string | `spec.session` | the session label |
| `labels` | object of strings | `spec.declared.labels` | the labels; empty when there are none |
| `argv` | array of strings | `spec.argv` | the command. The service cuts it after 1024 bytes in total. |
| `argv_truncated` | boolean | new | whether `argv` was cut |
| `priority` | number or null | `spec.declared.priority` | the declared priority; null when none was declared |
| `priority_source` | source or null | `priority_source` | where the priority comes from; null when there is none |
| `idempotency_key` | string or null | `idempotency_key` | null when the Job was submitted without one |
| `submitted_ms` | number | `submitted_ms` | when the Job was first submitted |
| `waiting_since_ms` | number | `released_ms`, else `attempt_submitted_ms`, else `submitted_ms` | since when this attempt has been waiting |
| `admitted_ms`, `started_ms`, `finished_ms` | number or null | the same names | null when that has not happened |
| `suspended.total_ms` | number | `suspension.total_ms` | length of time spent suspended in finished suspensions |
| `suspended.since_ms` | number or null | `suspension.since_ms` | start of the suspension in force; null when not suspended |
| `wall_limit_ms` | number or null | `reservation.wall_limit_ms` | length of the time limit; null when there is none |
| `termination_deadline_ms` | number or null | `termination_deadline_ms` | when a stopping Job is killed; null otherwise |
| `predicted_start_ms` | number or null | `predicted_start_ms` of the listing entry | null when the service has no prediction. It is current in the rows of `jobs`, of `job` and of the snapshot's page, and always null in the row of a pushed `job` line: a subscriber learns predictions only by asking for the page of queued rows again, see Sequenced changes. |
| `waited_for` | string or null | `waited_for` | the service's sentence about what the Job waits for. It is current in the rows of `jobs`, of `job` and of the snapshot's page; in the row of a pushed `job` line it is what the service held at that save. It reaches a subscriber between saves only by asking for the page again. |
| `reserved` | object | `reservation.vector` | `cores_milli`, `memory` in bytes, `pids` |
| `memory_max` | number, `"unlimited"` or `"not_applicable"` | `applied_resources["memory.max"]` | what the kernel holds; `not_applicable` when no value was applied |
| `stop_kind` | string or null | `stop.kind` | why the service stopped the Job, one of the values under Enumerated values; null when it did not |
| `exit_code`, `signal` | number or null | `exit_code`, `signal` | null when there is none |
| `start_error` | string or null | `start_error` | why the command did not start |
| `usage` | object or null | `usage` | for an ended Job: `peak_memory`, `peak_pids`, `written`, `cpu_ms`, `throttled_ms`. The last two are `"not_measured"` for a Job that ran without a cgroup. Null for a Job that has not ended. |
| `network` | string or null | `network.selected` | the network the Job was given; null before it is resolved |
| `terminal` | boolean | whether `spec.declared.terminal` is set | the Job has a terminal that can be attached to |
| `on` | string or null | `spec.declared.on.target` | the remote target the Job runs on; null for a Job on the service's host |
| `cgroup` | string or null | `workload_cgroup` | the cgroup of the Job; null when it has none |
| `confirmed` | object, `"not_applicable"` or `"unknown"` | new | a short form of what the kernel confirmed, see `job`: `namespaces` (array of strings), `pid_namespace` (boolean), `no_new_privs` (boolean), `cap_drop` (as in the Job record, or null), `seccomp_denied` (number of system calls denied), `cpu_affinity` (as in the Job record, or null) |

A client computes running time from `started_ms`, `finished_ms`, `suspended` and the service's clock.

### job

One Job in full.

```json
{"id":4,"op":"job","args":{"id":812}}
{"re":4,"ok":{"seq":4410,"now_ms":1790000001300,"row":{"id":812,"state":"running"},"job":{"id":812,"attempt":1,"state":"running","workload_cgroup":"/sys/fs/cgroup/user.slice/job/jobs/812","shim_pid":30120},"predicted_start_ms":null,"confirmed":{"at_ms":1789999900460,"isolation_controls":{"namespaces":["pid"],"user_namespace_from_network":false,"root_read_only":false,"private_tmp":[],"writable":[],"pid_namespace":{"proc":"/proc","init_pid":30131,"command_pid":30133}},"security_controls":null,"process_controls":{"cpu_affinity":null,"numa_policy":null,"rlimits":{}},"sources":{"namespaces":{"from":"object","id":7,"path":"builds/linux"}}},"live":{"at_ms":1790000001000,"cpu_at_ms":1790000001000,"attempt":1,"memory":734003200,"peak_memory":912261120,"pids":41,"peak_pids":58,"written":10485760,"cpu_ms":381200,"throttled_ms":0,"oom_kill":0,"oom_group_kill":0,"pids_max_events":0}}}
```

The example shortens `row` and `job`; a service sends both in full.

| Argument | Type | Meaning |
|---|---|---|
| `id` | number | the Job |
| `attempt` | number, optional | an earlier attempt; the current one when left out |

| Field | Type | Meaning |
|---|---|---|
| `missing` | string | only when the Job or the attempt does not exist: `job` or `attempt`; or `record` when a directory of that ID exists and its record cannot be read. No other field is present. This is an `ok` answer, not an error. |
| `seq`, `now_ms` | number | as in `jobs` |
| `row` | object | the row of the Job, as in `jobs`. It is built from the record for an ended Job outside the listing index. |
| `job` | object | the Job record, see below |
| `predicted_start_ms` | number or null | as in the row |
| `confirmed` | object or word | what the kernel confirmed; see below |
| `live` | object or null | the newest sample of a Job that is starting, running, suspended or stopping, with the fields of Live use and its times `at_ms` and `cpu_at_ms`; null for any other Job. The figures are read when the request arrives, whether or not the Job is in an interest set. |

`confirmed` is what the supervisor read back from the kernel when the command started. It is available from that moment, not only at the end of the Job.

| Field | Type | From | Meaning |
|---|---|---|---|
| `at_ms` | number | new | when the supervisor wrote it |
| `isolation_controls` | object or null | `isolation_controls` | `namespaces`, `user_namespace_from_network`, `root_read_only`, `private_tmp`, `writable`, and `pid_namespace` with `proc`, `init_pid` and `command_pid`; null when no isolation control was applied |
| `security_controls` | object or null | `security_controls` | `no_new_privs`, `cap_drop`, `cap_bounding_reduced`, `seccomp_deny`, `seccomp_arch`; null when none was applied |
| `process_controls` | object or null | `process_controls` | `cpu_affinity`, `numa_policy`, `rlimits`; null when none was applied |
| `sources` | object | `resource_sources` | for each setting behind a confirmed control, by the setting's name, its source |

In place of the object, `confirmed` is `"not_applicable"` for an attempt whose command never started, and `"unknown"` for one that started but whose confirmation the service cannot read, which is the case for Jobs started by a release before this interface. `isolation_controls.pid_namespace` is absent when the Job has no PID namespace; `command_pid` in it is null until the init process has started the command. It is also `"unknown"` for a Job that runs on a remote target, whose command the service of the other host starts.

The Job record is the record job keeps, with every enumerated value and every source written as Enumerated values and sources says. `state` is one of the ten states of the row. Its members:

| Member | Type | Meaning |
|---|---|---|
| `id`, `attempt` | number | the Job and its current attempt |
| `revision` | number | as in the row |
| `state` | string | as in the row |
| `spec` | object | what runs: `argv` (array of strings), `cwd` (string), `session` (string), `queue` (string or null) and `declared` |
| `spec.declared` | object | the settings in force. `priority`, `cores_milli`, `memory`, `pids`, `disk`, `wall_ms`, `cpu_request_milli`, `cpu_weight`, `memory_request`, `bandwidth` (numbers or null); `cpu_limit_milli`, `memory_high`, `memory_max`, `memory_swap_max`, `pids_max` (a number, `unlimited`, or null); `io_max`, `io_weight`, `io_bfq_weight`; `execution_profile`, `scheduling_class`, `monitor`, `dir` (strings or null); `devices`, `allow_write`, `send`, `fetch` (arrays of strings); `confine`, `stdin` (booleans); `net` (the word a person writes for `--net`, such as `host`, `none`, `profile:NAME`, or null); `on` (`target`, `key`, `options`, or null); `terminal` (`rows`, `cols`, or null); `namespaces`, `root`, `private_tmp`, `writable`; `no_new_privs`, `cap_drop`, `seccomp_deny`; `cpu_affinity`, `numa_policy` and `rlimit_as` to `rlimit_stack`; `output_head_bytes`, `output_tail_bytes`; `labels` (object of strings). Null or absent means that nothing was declared for that setting. |
| `submitted_spec`, `requested_spec`, `effective_spec` | object or null | the spec as first submitted, as last asked for, and as resolved |
| `key` | string | the key under which earlier runs of this command are remembered |
| `idempotency_key`, `spec_digest`, `spec_digest_version` | string, string, number; absent when not set | the idempotency key and the digest of the spec |
| `actor_uid`, `actor_pid` | number; absent when not recorded | who submitted |
| `admitted_ms` | number; absent before admission | when the Job was admitted |
| `submitted_ms`, `attempt_submitted_ms`, `released_ms`, `started_ms`, `finished_ms` | number or null | times of the lifecycle |
| `timing` | object | `elapsed_ms`, `active_ms`, `suspended_ms`: lengths of time, computed when the answer was written |
| `suspension` | object | `generation`, `requested`, `pending`, `since_ms`, `total_ms`, `error`, `actor_uid` |
| `reservation` | object | `vector` (`cores_milli`, `memory`, `pids`), `disk`, `devices`, `cores_source`, `memory_source`, `disk_source` (see Enumerated values), `predicted_ms`, `wall_limit_ms` |
| `policy` | string | the profile of the service the Job was submitted under: `ordinary` or `legacy` |
| `queue_id` | number or null | the Queue |
| `backend` | string | how limits are held for this Job |
| `priority_source` | source or null | where the priority comes from |
| `priority_changes` | array | `attempt`, `actor_uid`, `at_ms`, `before`, `after` |
| `applied_resources` | object of strings | cgroup file name to the value the kernel holds, for example `"memory.max":"8589934592"` |
| `resource_sources` | object | setting name to its source |
| `aggregate_domains` | array | the shared ceilings above the Job: `object_id`, `object_path`, `limits` (file name to value) |
| `workload_cgroup` | string or null | the cgroup path of the Job; null without a cgroup |
| `shim_pid` | number or null | the process ID of the supervisor; null before it is started |
| `shim_start_ticks`, `supervisor_boot_id` | number or null, string or null | which process and which boot that ID belongs to |
| `termination_deadline_ms` | number or null | when a stopping Job is killed |
| `waited_for` | string or null | what a waiting Job waits for |
| `stop` | object or null | `kind` and `line`, when the service stopped the Job |
| `result` | object or null | at the end: `exit_code`, `signal`, `start_error`, `finished_ms`, `usage`, `oom_kill`, `oom_group_kill`, `pids_max_events`, `leftover_processes`, `leftover_names`, `kept_helpers`, `output_bytes`, `output_error`, `output_held_open`, `output_retention`, `isolation_controls`, `security_controls`, `process_controls`, `notes`, `remote`, `remote_lines`, `monitor`, `windows` |
| `usage` | object | `peak_memory`, `peak_pids`, `cpu_ms`, `throttled_ms`, `written`. In this record they are zero until the Job ends, and `cpu_ms` and `throttled_ms` are zero for a Job without a cgroup. Use `row.usage` and `live`, which say "not measured". |
| `output_mode` | string or null | `pipe` or `pty` |
| `preset_snapshot`, `admission_snapshot` | object or null | the profile and class definitions in force, and the last explanation of admission. Both are passed on as job keeps them, in job's own spellings; this document does not describe their members. |
| `network`, `link` | object; absent or null when there is none | the network the Job was given, passed on as job keeps it, in job's own spellings |
| `log` | string | path of the output recording on the service's host |

Credentials are removed from every record before it is sent.

### tree

Every Queue and Group, with depth and use.

```json
{"id":5,"op":"tree"}
{"re":5,"ok":{"seq":4410,"now_ms":1790000001400,"nodes":[{"object":{"id":1,"parent":null,"name":"","kind":"group","owner_uid":1000,"created_ms":1789000000000,"paused":false,"closed":false,"draining":false,"config":{}},"path":"","effective":{},"aggregate_domains":[],"paused_by":[],"closed_by":[],"depth":{"held":0,"queued":33,"starting":0,"running":4,"suspended":0,"stopping":0,"oldest_queued_age_ms":51400,"oldest_queued_since_ms":1789999950000,"started_last_hour":{"window_ms":3600000,"sample_limit":4096,"count":12,"median_ms":800,"max_ms":64000}},"use":{"running":{"count":4,"limits":[]},"budgets":[],"domains":[]}},{"object":{"id":6,"parent":1,"name":"builds","kind":"group","owner_uid":1000,"created_ms":1789000100000,"paused":false,"closed":false,"draining":false,"config":{"max_running":4,"memory":34359738368,"memory_max":"32G"}},"path":"builds","effective":{"max_running":{"value":4,"source":{"from":"object","id":6,"path":"builds"}}},"aggregate_domains":[{"object_id":6,"object_path":"builds","limits":{"memory.max":"34359738368"}}],"paused_by":[],"closed_by":[],"depth":{"held":0,"queued":33,"starting":0,"running":4,"suspended":0,"stopping":0,"oldest_queued_age_ms":51400,"oldest_queued_since_ms":1789999950000,"started_last_hour":{"window_ms":3600000,"sample_limit":4096,"count":12,"median_ms":800,"max_ms":64000}},"use":{"running":{"count":4,"limits":[{"limit":4,"count":4,"set_by":{"id":6,"path":"builds"}}]},"budgets":[{"resource":"memory","limit":34359738368,"reserved":25769803776,"set_by":{"id":6,"path":"builds"}}],"domains":[{"object_id":6,"object_path":"builds","cgroup":"/sys/fs/cgroup/user.slice/job/jobs/domain-6","limits":{"memory.max":"34359738368"},"counters":{"at_ms":1790000001000,"memory":2936012800,"peak_memory":5368709120,"pids":164,"peak_pids":230,"written":41943040,"cpu_ms":1524800,"throttled_ms":0,"oom_kill":0,"oom_group_kill":0,"pids_max_events":0}}]}}]}}
```

`nodes` holds every Queue and Group, parents before children. A node is the view job returns for a Queue or Group today, with two additions.

| Field | Type | From | Meaning |
|---|---|---|---|
| `object.id` | number | the same | identifier; it does not change when the object is renamed or moved |
| `object.parent` | number or null | the same | the Group above; null for the root |
| `object.name` | string | the same | last part of the path |
| `object.kind` | string | the same | `queue` or `group` |
| `object.owner_uid`, `object.created_ms` | number | the same | who created it and when |
| `object.paused`, `object.closed`, `object.draining` | boolean | the same | flags set on this object itself |
| `object.config` | object | the same | the settings set on this object |
| `path` | string | the same | the full path; empty for the root |
| `effective` | object | `effective`, with `source_id` and `source_path` as one source | setting name to `value` and `source`: the value in force here and where it comes from |
| `aggregate_domains` | array | the same | the shared ceilings that apply here: `object_id`, `object_path`, `limits` |
| `paused_by`, `closed_by` | array of strings | the same | paths of the objects whose pause or close holds here; empty when none |
| `depth` | object | the same | Jobs at or below this object per state (`held`, `queued`, `starting`, `running`, `suspended`, `stopping`); `oldest_queued_age_ms`, a length of time valid at `now_ms`, null when nothing is queued; `started_last_hour` with `window_ms`, `sample_limit`, `count`, `median_ms`, `max_ms` |
| `depth.oldest_queued_since_ms` | number or null | new | since when the oldest queued Job has waited; null when nothing is queued |
| `use` | object | new | see below |

`use`:

| Field | Type | Meaning |
|---|---|---|
| `running.count` | number | Jobs at or below this object that are starting, running, suspended or stopping |
| `running.limits` | array | every `max-running` that binds here, from this object upwards. Each has `limit` (number), `count` (the Jobs counted against it, which are those at or below the object that sets it) and `set_by` with `id` and `path`. Empty when nothing limits. The first entry is the effective `max-running`. |
| `budgets` | array | every admission budget that binds here, from this object upwards: `resource` (`cores_milli` or `memory`), `limit`, `reserved` (the sum reserved by the Jobs counted against it) and `set_by` |
| `domains` | array | every shared ceiling that applies here, outermost first: `object_id`, `object_path`, `cgroup` (path), `limits` (file name to the value the kernel holds) and `counters` |
| `domains[].counters` | object | `at_ms`; `memory` and `peak_memory` in bytes; `pids`, `peak_pids`; `written` in bytes; `cpu_ms`, `throttled_ms` as lengths of CPU time; `oom_kill`, `oom_group_kill`, `pids_max_events` as counts since the cgroup was made. Each is a number or `"unknown"` when the file could not be read. |

Without the capability `cgroup` there are no shared ceilings and `domains` is empty.

### totals

Counts and capacity of the host. The same object is pushed on a subscription.

```json
{"id":6,"op":"totals"}
{"re":6,"ok":{"seq":4410,"now_ms":1790000001500,"counts":{"held":0,"queued":33,"starting":0,"running":4,"suspended":0,"stopping":0,"succeeded":10377,"failed":22,"cancelled":5,"lost":0},"capacity":{"pool":{"cores_milli":24000,"memory":50465865728,"pids":16384},"reserved":{"cores_milli":16000,"memory":25769803776,"pids":2048},"running":4,"waiting":33,"memory_total":67430813696,"memory_available":40802189312,"output":{"recorded_bytes":734003200,"budget_bytes":1073741824},"state_free_bytes":211106232532}}}
```

| Field | Type | From | Meaning |
|---|---|---|---|
| `seq` | number | new | the number of the last sequenced change the service produced in this run, 0 when there has been none. A service without `subscribe` among its capabilities always answers 0. |
| `counts` | object | `health.jobs` | Jobs per state, over every Job record the service holds, those outside the listing index included. All ten states are present. |
| `capacity.pool` | object | `pool` | what the service may hand out: `cores_milli`, `memory`, `pids` |
| `capacity.reserved` | object | new | the sum reserved by Jobs that are starting, running, suspended or stopping |
| `capacity.running`, `capacity.waiting` | number | `running`, `waiting` | Jobs that hold a reservation, and Jobs that are held or queued |
| `capacity.memory_total`, `capacity.memory_available` | number | the same | memory of the host |
| `capacity.output.recorded_bytes` | number or `"unknown"` | new | bytes of recorded output the service holds, as of its latest pass over the recordings. It is `"unknown"` until the service has measured its recordings for the first time, which happens after the first Job ends in a run. |
| `capacity.output.budget_bytes` | number | new | the configured budget for recorded output |
| `capacity.state_free_bytes` | number or `"unknown"` | `health.state_free_bytes` | free space in the state directory |

### processes

The processes of one Job, read when asked.

```json
{"id":8,"op":"processes","args":{"id":812}}
{"re":8,"ok":{"at_ms":1790000001600,"now_ms":1790000001601,"attempt":1,"active":true,"source":"cgroup","truncated":false,"processes":[{"pid":30133,"parent":30131,"name":"cargo","threads":1,"rss":20971520,"start_ticks":88123411},{"pid":30170,"parent":30133,"name":"rustc","threads":17,"rss":713031680,"start_ticks":88123502}]}}
```

| Field | Type | Meaning |
|---|---|---|
| `missing` | string | only when there is no such Job: `job`; or `record` when a directory of that ID exists and its record cannot be read |
| `at_ms` | number | when the service read the process table |
| `attempt` | number | the attempt the processes belong to |
| `active` | boolean | false when the Job is not starting, running, suspended or stopping; `processes` is then empty |
| `source` | string | `cgroup` when the list is the membership of the Job's cgroup, `descendants` when it is the descendants of the supervisor |
| `truncated` | boolean | true when there were more than `limits.processes`; the list then holds the first by process ID |
| `processes[].pid`, `.parent` | number | process ID and parent, as the service's host numbers them |
| `processes[].name` | string | the kernel's command name |
| `processes[].threads` | number | threads |
| `processes[].rss` | number | resident memory in bytes |
| `processes[].start_ticks` | number | start time in clock ticks since boot, which tells two processes with one ID apart |

The list is sorted by `pid`. It is never part of a subscription.

### output

Recorded output of one attempt.

| Argument | Type | Meaning |
|---|---|---|
| `id` | number | the Job |
| `attempt` | number, optional | the attempt; the current one when left out |
| `from` | string or object, optional | `"start"` (the default): the first record that is kept. `"end"`: after the last record. `{"sequence":N}`: the record with sequence number N. |
| `direction` | string, optional | `forward` (the default) or `backward` |
| `streams` | array of strings, optional | only these streams: `stdout`, `stderr`, `terminal`, `combined`, `diagnostic`. All when left out. |
| `follow` | boolean, optional | keep sending as the Job writes. Forward only. |
| `max_bytes` | number, optional | stop after this many bytes of `data`, counted before base64; 1 to `limits.output_piece_bytes`, default 1 048 576. A record is never cut: the service stops before the record that would pass the bound, and sends one record even when that record alone is larger. Without effect on what follows the existing output when `follow` is true. |

The first line is the answer, the following lines carry the same `re`:

```json
{"id":9,"op":"output","args":{"id":812,"from":"end","direction":"backward","streams":["stdout","stderr"],"max_bytes":65536}}
{"re":9,"ok":{"attempt":1,"mode":"pipe","first_sequence":1200,"next_sequence":1903,"complete":false,"quota":{"head_bytes":33554432,"tail_bytes":33554432},"trimmed_bytes":null,"totals":[{"stream":"stdout","written_bytes":88080384,"kept_bytes":67108864},{"stream":"stderr","written_bytes":4096,"kept_bytes":4096}]}}
{"re":9,"gap":{"sequence":1890,"at_ms":1790000000100,"records":310,"bytes":20316160,"stream":"stdout"}}
{"re":9,"record":{"sequence":1901,"at_ms":1790000001100,"stream":"stdout","data":"ICAgQ29tcGlsaW5nIHNlcmRlIHYxLjAK"}}
{"re":9,"record":{"sequence":1902,"at_ms":1790000001350,"stream":"stderr","data":"d2FybmluZzogdW51c2VkIHZhcmlhYmxlCg=="}}
{"re":9,"status":{"complete":false,"terminal":false,"exit_status":0,"error":null,"quota":{"head_bytes":33554432,"tail_bytes":33554432},"trimmed":null}}
{"re":9,"end":{"reason":"limit","first_sequence":1890,"next_sequence":1903,"more":true}}
```

The answer:

| Field | Type | From | Meaning |
|---|---|---|---|
| `missing` | string | new | only when the Job or attempt does not exist: `job` or `attempt`; or `record` when a directory of that ID exists and its record cannot be read. Nothing follows. |
| `attempt` | number | the same | the attempt served |
| `mode` | string or null | the same | `pipe` or `pty`; null for a recording from before streams were recorded apart |
| `first_sequence` | number or null | new | sequence number of the first record that is kept; null when nothing is kept |
| `next_sequence` | number | `next_sequence` of the recording | the number the next record will get |
| `complete` | boolean | the same | whether the recording is closed |
| `quota` | object or null | the same | `head_bytes` and `tail_bytes` kept per stream; null when the built-in window applies |
| `trimmed_bytes` | number or null | the same | bytes removed because the service's budget for recordings was exceeded; null when none |
| `totals[].stream` | string | the same | the stream |
| `totals[].written_bytes` | number or `"not_measured"` | `written_bytes` | bytes the Job wrote to it |
| `totals[].kept_bytes` | number or `"not_measured"` | `written_bytes` less `dropped_bytes` | bytes of it still recorded. Both are numbers for a Job that has ended, and for a running Job whose output has a quota (`quota` is not null). Both are `not_measured` for a running Job without a quota: the supervisor counts per stream in its own memory and gives the service the counts only when the Job ends, so the lines of such a Job are returned while its totals are not yet known. They are also `not_measured` for a recording that was made without counts per stream. A stream the Job never wrote to has no entry once the counts are known. |

After the answer:

| Line | Fields | Meaning |
|---|---|---|
| `record` | `sequence`, `at_ms`, `stream`, `data` | one piece of output, at most 65 536 bytes before base64, with the time the service captured it. `at_ms` is null in a recording from before streams were recorded apart, whose `mode` is null and whose only stream is `combined`. |
| `gap` | `sequence`, `at_ms`, `records`, `bytes`, `stream` | at this place output is missing: so many records and bytes of this stream, or of all streams when `stream` is null |
| `status` | `complete`, `terminal`, `exit_status`, `error`, `quota`, `trimmed` | the state of the recording and the Job: whether the recording is closed, whether the Job has ended, the exit status `job` would return for it, an error of the recording or null, the quota, and bytes just trimmed or null. Sent after each batch of records, and at least once a second while following. |
| `end` | `reason`, `first_sequence`, `next_sequence`, `more` | the last line for this request. `reason` is `complete` (everything asked for was sent, and when following the Job has ended), `limit` (`max_bytes` was reached), `stopped` (the client sent `end`) or `error` (the last `status` names it). `first_sequence` is the lowest sequence number of a `record` or `gap` line that was sent, and null when none was sent. `next_sequence` is always a number, the position at which the read stopped looking: forward, one above the last record it passed, whether that record was sent or belonged to a stream that was not asked for, and the position it started from when it passed none; backward, one above the last line sent, and the position asked from when none was sent. `more` says whether records remain in the direction asked. |

Positions:

- Sequence numbers count the records of one attempt over all streams, from 0. With `streams` given, the numbers a client sees have holes. A hole is not a gap. Only a `gap` line says that output is missing, and a client draws a gap where the line arrived and never closes it up.
- Forward, records arrive in ascending order from `from`. To continue, ask again from `{"sequence": end.next_sequence}`.
- Backward, the service chooses the newest records below `from` that fit into `max_bytes` and sends them in ascending order. To read further back, ask again with `from` set to `{"sequence": end.first_sequence}`.
- A position that is no longer kept is not an error: the service sends a `gap` for what is missing and goes on with the first record kept.
- A read that sent no record and no gap ends with `first_sequence` null. Forward, asking again from its `next_sequence` asks the same question again, which is how a client waits without `follow`. Backward, `more` is false and nothing lies further back.
- `"end"` is the place after the last record the service finds in the recording when the request arrives.
- A follow ends with `end` when the Job has ended and its recording is closed. A client ends it earlier with the request `end`, or by closing the connection.
- A client that reads slower than a Job writes is not buffered for. The recording is on disk with a bounded window; when the reader's position falls out of it, the next line is a `gap` with the count of what was skipped.

### end

Stops a running `output` on the same connection.

```json
{"id":15,"op":"end","args":{"of":9}}
{"re":9,"end":{"reason":"stopped","first_sequence":1890,"next_sequence":1950,"more":true}}
{"re":15,"ok":{"stopped":true}}
```

`of` is the `id` of the `output` request. The service reads and handles `end` while that output is running, ahead of any other request that waits behind it. It is the only request handled out of order. While an output runs the service reads at most 64 waiting requests, or 8 388 608 bytes of them, ahead; an `end` that waits behind more than that is handled when the output has ended, and answers `stopped` false.

Guarantees:

- The stream's own `end` line, with reason `stopped`, is written before the answer to the request `end`.
- After the answer, no line with the `re` of that stream is written. Lines of it that were written before the answer may still arrive before it; a client drops them or shows them, as it likes, and knows from `next_sequence` where the stream stopped.
- `stopped` is false, and nothing else happens, when no output with that `id` is running: it has ended by itself, or never existed. That is not an error, because the two can cross.
- Requests that waited behind the output are answered after it, in their order.

### commands

job's command table.

```json
{"id":10,"op":"commands","args":{"language":"en"}}
{"re":10,"ok":{"language":"en","global":[{"arity":"flag","help":"use the system service, as JOB_SYSTEM=1 does","long":"--system","meta":"","short":null,"value":{"kind":"free"}}],"commands":[{"available":true,"end_of_options":false,"exits":[["0","success"],["125","usage error or service failure"]],"groups":[{"heading":"Options","legacy":false,"options":[{"arity":"flag","help":"print the result as JSON","long":"--json","meta":"","short":null,"value":{"kind":"free"}},{"arity":"one","help":"json prints one versioned envelope with schema_version, kind and data","long":"--format","meta":"text|json","short":null,"value":{"kind":"words","words":["text","json"]}},{"arity":"flag","help":"show this help","long":"--help","meta":"","short":"-h","value":{"kind":"free"}}]}],"kind":"queue_pause","notes":[],"operands":[{"help":"the name of a Queue","name":"NAME","repeat":false,"required":true,"value":{"kind":"queue_path"}}],"path":["queue","pause"],"section":"organization","summary":"hold the waiting Jobs of a Queue","trailing_command":false,"usage":"job queue pause [OPTIONS] NAME"}]}}
```

| Argument | Type | Meaning |
|---|---|---|
| `language` | string, optional | `en` or `de` for the texts; the language of the service's environment when left out |

| Field | Type | Meaning |
|---|---|---|
| `language` | string | the language of the texts |
| `global` | array of options | options written before the command |
| `commands[].path` | array of strings | the words of the command, for example `["queue","set"]` |
| `commands[].section` | string | `execution`, `interaction`, `inspection`, `organization`, `administration` or `legacy` |
| `commands[].summary` | string | one line of help |
| `commands[].usage` | string | the usage line of the syntax reference |
| `commands[].groups[]` | object | `heading` (text), `legacy` (boolean: options kept for older scripts) and `options` |
| option | object | `long` (string), `short` (string or null), `arity` (`flag`, `one` or `many`), `value`, `meta` (the placeholder of the help, empty for a flag) and `help` (text) |
| `value.kind` | string | what the value is: `free`, `words`, `job_id`, `any_job_id`, `job_id_set`, `ended_job_id_set`, `queue_path`, `group_path`, `file`, `dir`, `network`, `preset`, `signal` or `program`. The list is open. |
| `value.words` | array of strings | with `words`: the listed values. A word ending in `:` or `=` is the start of a value the person completes. |
| `commands[].operands[]` | object | `name`, `value`, `required`, `repeat`, `help` |
| `commands[].trailing_command` | boolean | the command line of a Job follows after `--` |
| `commands[].end_of_options` | boolean | `--` ends the options |
| `commands[].exits` | array of pairs | exit status and its meaning |
| `commands[].kind` | string or null | the `kind` of the command's `--format json` envelope; null when it has none |
| `commands[].available` | boolean | false for a command that is documented and not built |
| `commands[].notes` | array of strings | further lines of help |

The example is one command of the answer as a service sends it. Groups that share a heading are sent as one group. Commands that are internal to job are left out. The table is the same for the whole run of a service; a client asks once.

### command

Carries out one job command, given as the words a person would type after `job`. The service parses the words with job's own command table and parsers and carries them out as the `job` command would.

```json
{"id":11,"op":"command","args":{"words":["retry","790","--queue","builds/linux"],"session":"ci"}}
{"re":11,"ok":{"outcome":"done","exit_status":0,"kind":"retry","data":{"id":790,"attempt":2},"text":[],"diagnostics":[]}}
```

The example shortens `data`.

| Argument | Type | Meaning |
|---|---|---|
| `words` | array of strings | what follows `job` on a command line, one string per argument, for example `["queue","set","builds","--max-running","4"]`. No shell reads them: a string is one argument as it is. |
| `cwd` | string, optional | an absolute path on the service's host: the working directory of the command. Required for `submit`, `create` and `edit`. Relative paths in `words` are resolved against it. |
| `env` | object, optional | `{"vars":[["NAME","value"],...]}`: the environment to save with the Job. See below for what leaving it out means. |
| `terminal` | object, optional | `{"rows":R,"cols":C}`: the size of the terminal for a Job submitted with `--pty`. Without effect otherwise; 24 by 80 when left out. |
| `session` | string, optional | the session label, which the `job` command takes from `JOB_SESSION`. `unnamed` when left out. `--session` in `words` goes before it. |
| `dry_run` | boolean, optional | carry out nothing and answer what would happen, as `--dry-run` does. Accepted only for commands whose entry in `commands` lists `--dry-run`; refused as a usage error for the others in this set. `--dry-run` among `words` means the same; either one is enough, both together are accepted, and `dry_run` given as `false` does not cancel a `--dry-run` in `words`. |

The first word, or the first two for `queue`, `group` and `config`, must be one of:

`submit`, `create`, `edit`, `release`, `retry`, `cancel`, `signal`, `suspend`, `continue`, `reprioritize`, `move`, `remove`, `update`, `resource-update`, `queue` and `group` with every subcommand that changes something (`create`, `set`, `unset`, `rename`, `move`, `pause`, `resume`, `close`, `open`, `update`, `cancel`, `suspend`, `continue`, `remove`, and the earlier `add`, `rm` and `clear` of `queue`), and `config reload`. The subcommands that only read (`list`, `show`) are not carried; they are answered with `not_a_command_request`.

The environment:

- On `submit` and `create`, `env` is the environment the Job is saved with. Left out, the environment is empty. The service adds nothing of its own.
- On `edit` and `retry`, `env` left out keeps the environment saved with the Job, and the command is refused when that cannot be read. `env` given replaces it; on `retry` that is what `--current-env` does for the `job` command, and the option itself is refused here, since the service has no environment of the client.

Paths. Every path in `words` is a path on the service's host: `--dir`, `--ssh-key`, `--net wireguard:FILE`, `--net-secret-file`, `--pressure FILE`. The service reads those files itself. A client that runs on another host than the service says so to the person before it sends one.

What cannot be carried and is refused as a usage error: `--stdin`, because the standard input of the client is not on this connection. `--format` and `--json` in `words` are accepted and change nothing; the answer always has the form below.

The answer has the same form as every other answer, with its content in `ok`:

| Field | Type | Meaning |
|---|---|---|
| `outcome` | string | `done`: the command was parsed and carried out, and `exit_status` says how it went. `usage_error`: the words are not a valid command; nothing was carried out. `not_a_command_request`: the first word is a job command that this request does not carry; nothing was carried out. The list is open. |
| `exit_status` | number | what the `job` command would exit with: 0 for success, 125 for a usage error, and for the rest the statuses that `commands` lists under `exits` for the command |
| `kind` | string or null | the `kind` of the command's `--format json` output. Null when the command failed, and when it printed text in place of a JSON form. |
| `data` | value or null | the `data` of the command's `--format json` output, exactly as the command prints it. One `kind` covers two shapes for the commands that take a set of Jobs: with a single ID `data` is the Job record, and with a set, a range, a list or `--dry-run` it is an object with `results`, one entry per ID. A client looks for `results` before it reads `data` as a record. Its shape belongs to the command and is documented with job's command line; the spellings of this document do not apply inside it. Null when `kind` is null, and when the command succeeded and printed nothing. It is never a sentence: what a command prints as text is in `text`. |
| `text` | array of strings | lines the command wrote to standard output outside its JSON form; empty for most commands. `kind` and `data` are null when it is not empty. |
| `diagnostics` | array of strings | what the command wrote to standard error, line by line, in the language of the service's environment. A refusal by the service is here, with `exit_status` not 0. |
| `use` | string or null | only with `not_a_command_request`: the name of the one request to use instead. Null means that no request of this interface stands in for the command. It names the request the description gives, whether or not the service lists it among its capabilities. |

A refusal is not an error of this interface. A command the service refuses, for example the release of a Job that is not held, is `outcome` `done` with the exit status and the sentence the `job` command would give. For a single Job that status is 125:

```json
{"id":16,"op":"command","args":{"words":["release","812"]}}
{"re":16,"ok":{"outcome":"done","exit_status":125,"kind":null,"data":null,"text":[],"diagnostics":["job: only held Jobs can be released"]}}
{"id":17,"op":"command","args":{"words":["cancel"]}}
{"re":17,"ok":{"outcome":"usage_error","exit_status":125,"kind":null,"data":null,"text":[],"diagnostics":["job: a job id is needed"]}}
{"id":18,"op":"command","args":{"words":["logs","812","--follow"]}}
{"re":18,"ok":{"outcome":"not_a_command_request","exit_status":125,"kind":null,"data":null,"text":[],"diagnostics":[],"use":"output"}}
```

When a command fails, `kind` and `data` are null. The failure is in `exit_status` and `diagnostics`. The `error` envelope that `--format json` prints for a failed command is not carried.

Some carried commands have no JSON form: `config reload` and the earlier `queue add`, `queue rm` and `queue clear`. Their output arrives in `text`, with `kind` and `data` null. The same holds for `queue set` in its earlier spelling, with `--parallel` and the other options of that spelling, which prints one sentence:

```json
{"id":20,"op":"command","args":{"words":["queue","set","builds","--parallel","3"]}}
{"re":20,"ok":{"outcome":"done","exit_status":0,"kind":null,"data":null,"text":["queue builds changed: 3 jobs at a time, in the order they came"],"diagnostics":[]}}
```

The sentences in the examples stand for whatever the service says. The `error` form remains for a request that is malformed: no `words`, a `cwd` that is not absolute, a member of the wrong type.

What `use` names for the commands this request does not carry:

| First word | `use` |
|---|---|
| `list`, `queue list` | `jobs` |
| `status`, `show` | `job` |
| `queue show`, `group show`, `group list` | `tree` |
| `host` | `totals` |
| `logs` | `output` |
| `help`, `completion` | `commands` |
| `attach`, `explain`, `attempts`, `pressure`, `config` other than `reload`, `net list`, `net show` | `native` |
| `run`, `wait`, and everything else | null |

The service treats the command exactly as it treats the same command from `job` itself: the same checks, the same refusals while a resource update or a removal is pending, the same record in the audit journal with the peer's user ID and process ID. `--idempotency-key` in `words` works as it does there: a repeated `submit` with the same key and the same specification returns the first Job, and a client that got no answer to a `submit` sends it again with the same key to learn what happened.

A command that makes several requests of the service, such as `retry`, which reads the current attempt first, is not one transaction, here as there.

### native

Carries one request of the service's internal protocol and returns its answer, both in the internal form.

```json
{"id":13,"op":"native","args":{"request":{"Attempts":{"id":790}}}}
{"re":13,"ok":{"protocol":20,"answer":{"Attempts":{"schema_version":1,"attempts":[]}}}}
```

`native` is the passage to what the service can tell and this document does not yet describe. It is offered only where the capability `native` is listed, and it is not part of the version promise. The shapes are those of the internal protocol whose number is `native_protocol` in the greeting; they can change with any release of job, and a client that uses `native` checks `native_protocol` and refuses a number it was not written for.

Every use of `native` is temporary. Each internal request below is to be replaced by a described request in a later set, and a request moves from this list into this document without a new version. A client keeps its uses of `native` in one place so that each can be replaced when its described request appears among the capabilities.

The internal requests a client is expected to pass through it in this set. This is the list of what remains to be described.

| Internal request | For |
|---|---|
| `Explain` | why a waiting Job waits, its predicted start, priority, bounds and aging |
| `Attempts` | the earlier attempts of a Job |
| `Pressure` | pressure of the host or of one Job |
| `PressureControl` | the pressure rules and their state |
| `Config` with `reload` false | the configuration in force, with profiles and classes |
| `LogQuery` | job's extraction of errors and lines from recorded output |
| `Extended` with `Settings` | the settings of a Queue or Group: configured, effective, from where |
| `Object` with `Show` | one Queue or Group |
| `Host` | what the host offers: kernel, controllers, security, isolation and process controls, I/O devices, surrounding limits, the service's paths; and under `info.network` the configured network namespaces and routing profiles, which is what `net list` and `net show NAME` print. `info.network` is absent when the service's configuration names none. |
| `ResourceUpdateStatus` | the steps of a resource update |
| `Attach` | the path of a Job's terminal socket |

`native` carries reading requests and `Attach`, and nothing else. Changing requests are not on this list, because `command` carries every action. A changing request sent through `native` is answered with an `error` of this interface. It is not recorded in the audit journal, because nothing was attempted. A reading request that is not on the list is carried on the same terms as the listed ones; a client has no reason to send one.

The service treats a request inside `native` exactly as it treats the same request from the `job` command, and nothing less: the same checks of the request and of the peer, the same refusals while a resource update or a removal is pending, the same record in the audit journal with the peer's user ID and process ID where the service records that request at all. `native` is a different envelope around the request and no other way in.

Not carried, and answered with an `error` of this interface: every changing request, the internal output stream (use `output`) and the internal version envelope. A refusal of the inner request arrives as `{"Error":{"message":"..."}}` in `answer`, not as an `error` of this interface.

## The subscription

### Starting

```json
{"id":1,"op":"subscribe","args":{"rows":{"order":"activity","limit":50},"interest":[812,811,809,804]}}
```

| Argument | Type | Meaning |
|---|---|---|
| `rows` | object, optional | `order`, `filter` and `limit` as in `jobs`, for the first page in the snapshot. Default: order `activity`, no filter, `limits.page_rows_default` rows. |
| `interest` | array of numbers, optional | the first interest set; empty when left out |
| `after` | object, optional | `{"started_ms":S,"seq":N}`: continue after change N of the run that started at S, without a snapshot |

The answer is one of three.

A snapshot:

```json
{"re":1,"ok":{"snapshot":{"seq":4410,"started_ms":1789990000000,"now_ms":1790000001000,"totals":{"counts":{"held":0,"queued":33,"starting":0,"running":4,"suspended":0,"stopping":0,"succeeded":10377,"failed":22,"cancelled":5,"lost":0},"capacity":{"pool":{"cores_milli":24000,"memory":50465865728,"pids":16384},"reserved":{"cores_milli":16000,"memory":25769803776,"pids":2048},"running":4,"waiting":33,"memory_total":67430813696,"memory_available":40802189312,"output":{"recorded_bytes":734003200,"budget_bytes":1073741824},"state_free_bytes":211106232532}},"health":{"started_ms":1789990000000,"state_schema":18,"backend":"cgroup","enforcement":"enforced","cgroup_required":false,"supervisors_adopted":0,"last_recovery_ms":1789990000200,"recovery_changed_records":0,"audit_writable":true,"events_writable":true,"starter_failures":0,"cancellations_failing":0,"cancellation_retries":0,"unreadable_records":[]},"tree":{"nodes":[]},"page":{"order":"activity","matching":{"total":10441,"counts":{"queued":33,"running":4,"succeeded":10377,"failed":22,"cancelled":5},"ended_not_searched":0},"offset":0,"rows":[],"before":null,"after":"djE6YWN0aXZpdHk6..."},"interest":4}}}
```

The example leaves `tree.nodes` and `page.rows` empty; a service fills them.

| Field | Type | Meaning |
|---|---|---|
| `seq` | number | the snapshot holds the effect of every change up to this number; the next sequenced line has `seq + 1` |
| `started_ms` | number | the run of the service the sequence numbers belong to |
| `now_ms` | number | the service's clock |
| `totals` | object | `counts` and `capacity` as in `totals` |
| `health` | object | see Health |
| `tree` | object | `nodes` as in `tree`, with depth and use |
| `page` | object | the first page as in `jobs`, without `seq` and `now_ms`, which are those of the snapshot |
| `interest` | number | how many Jobs the interest set holds |

The snapshot does not hold every Job. It holds one page. A client asks for further pages with `jobs` and for one Job with `job`.

A continuation, when `after` was given and the service still has every change after it:

```json
{"re":1,"ok":{"resumed":{"seq":4410,"started_ms":1789990000000,"now_ms":1790000001000}}}
```

`seq` in `resumed` is the last number assigned when the answer was written. The changes after `after.seq` follow as pushed lines, then one `totals`, one `use` with every Queue and Group, and the first `live` samples of the interest set; these three come with the service's next sample, up to a second later.

A refusal to continue:

```json
{"re":1,"ok":{"resync":{"reason":"restarted"}}}
```

`reason` is `restarted` when `after.started_ms` is not this run, and `expired` when the service no longer has every change after `after.seq` or has assigned no such number yet. No subscription is in force afterwards. See Resync.

### Sequenced changes

Each of these carries `seq`. Sequence numbers start at 1 with each run of the service and rise by exactly one from one sequenced line to the next, whatever its kind. The service numbers changes from the first `subscribe` of a run on: until then no change is kept, and `seq` is 0 in every answer and in that first snapshot. A sequenced line is sent to every subscriber, in order, without a hole. The first of them may follow the answer to `subscribe` at once and arrive in the same read as that answer, so a client applies the snapshot before it looks at the next line, and never in a later turn of its event loop.

| `push` | Fields | Sent when |
|---|---|---|
| `job` | `seq`, `at_ms`, `row` | a Job was created, the service saved its record, or the service learned that its command has started. Every save raises `revision` of the row, so a `job` line is sent even when no other field of the row changed. One line is held back: the save that records the supervisor of a starting Job leaves the Job `running` without `started_ms`, and no line is sent for it; the line follows when the service has learned the start, within about a quarter of a second, with `started_ms` set and the next `revision`. A `job` line therefore never shows a Job `running`, `suspended` or `stopping` with `started_ms` null. A change of `predicted_start_ms` or `waited_for` alone is not a save and sends nothing. |
| `job_removed` | `seq`, `at_ms`, `id` | the record of a Job was removed |
| `object` | `seq`, `at_ms`, `node` | a Queue or Group was created, or its node changed: settings, name, parent, paused, closed, draining, and with them `path`, `effective`, `aggregate_domains`, `paused_by`, `closed_by`. One line per node that changed, descendants included, parents first. `node` is as in `tree` without `depth` and `use`. |
| `object_removed` | `seq`, `at_ms`, `id` | a Queue or Group was removed |
| `health` | `seq`, `at_ms`, `health` | a field of the health object changed. The service compares once a second while a subscription exists, so the line can follow the change by a second. |

```json
{"push":"job","seq":4411,"at_ms":1790000002210,"row":{"id":815,"revision":3,"state":"starting"}}
{"push":"object","seq":4412,"at_ms":1790000003000,"node":{"object":{"id":7,"parent":6,"name":"linux","kind":"queue","owner_uid":1000,"created_ms":1789000200000,"paused":true,"closed":false,"draining":false,"config":{}},"path":"builds/linux","effective":{"max_running":{"value":4,"source":{"from":"object","id":6,"path":"builds"}}},"aggregate_domains":[{"object_id":6,"object_path":"builds","limits":{"memory.max":"34359738368"}}],"paused_by":["builds/linux"],"closed_by":[]}}
{"push":"job_removed","seq":4413,"at_ms":1790000003500,"id":640}
```

The `row` in the example is shortened; a service sends the whole row.

What a client does with them:

- `job`: when it holds a row with that `id`, it replaces it. When it does not, it decides from the filter and the order whether the row belongs between the first and the last row it holds; when so it inserts it, when not it leaves it out. Outside the range it holds it cannot know where the row lies, and need not.
- A row carries `queue_id`. When an `object` line changes the path of a Queue, the service sends no `job` line for the Jobs in it; a client that shows the path takes it from the tree.
- `predicted_start_ms` and `waited_for` change with every pass of admission, for every waiting Job. They do not cause a `job` line, and a `job` line does not carry the prediction: its row has `predicted_start_ms` null and the `waited_for` of that save. A client keeps the prediction it holds for a row when a `job` line for that row arrives while the Job stays `queued`, and drops it when the state changes. Both are current in every answer of `jobs` and `job` and in the snapshot's page, and that is the only way they reach a subscriber: a client that shows them asks again for the page of queued rows it shows, not more often than every `limits.poll_ms`.
- An answer of `jobs`, `job`, `tree` and `totals` carries the `seq` it is valid at. A sequenced line with a higher `seq` is newer than the answer; one with the same or a lower `seq` is already in it.
- A client that sees a `seq` that is not one above the last treats it as a `resync`.

### Sampled lines

These carry values the service samples. They have no sequence number of their own, are sent at most once a second each, are never kept for a later continuation, and can be dropped by the service under load without notice; the next one carries the current values.

| `push` | Fields | Content |
|---|---|---|
| `totals` | `now_ms`, `seq`, `counts`, `capacity` | as in `totals`; sent when a count or a capacity figure changed. `seq` is the last sequence number assigned. |
| `use` | `at_ms`, `objects` | `objects` is an array of `id`, `depth` and `use` as in `tree`, for the Queues and Groups of which a count in `depth` or a figure in `use` changed. `oldest_queued_age_ms` and the `at_ms` of counters change with the clock alone and do not count as a change. |
| `live` | `at_ms`, `jobs` | see Live use |

```json
{"push":"totals","now_ms":1790000003000,"seq":4413,"counts":{"held":0,"queued":32,"starting":0,"running":5,"suspended":0,"stopping":0,"succeeded":10377,"failed":22,"cancelled":5,"lost":0},"capacity":{"pool":{"cores_milli":24000,"memory":50465865728,"pids":16384},"reserved":{"cores_milli":18000,"memory":27917287424,"pids":2304},"running":5,"waiting":32,"memory_total":67430813696,"memory_available":40001189312,"output":{"recorded_bytes":734103200,"budget_bytes":1073741824},"state_free_bytes":211106132532}}
```

### Heartbeat

When the service has sent nothing on a subscription for `limits.heartbeat_ms`, 5000 in this version, it sends:

```json
{"push":"heartbeat","now_ms":1790000008000,"seq":4413}
```

`seq` is the last sequence number assigned. A client that has read no line for three times that interval takes the connection for dead, closes it and connects again. `capacity.memory_available` and `capacity.state_free_bytes` move on most hosts from one second to the next, so a `totals` line is sent about once a second there and a heartbeat is rare; a client does not wait for one.

### Health

| Field | Type | From | Meaning |
|---|---|---|---|
| `started_ms` | number | `health.started_ms` | start of this run |
| `state_schema` | number | the same | version of the state directory's layout |
| `backend`, `enforcement`, `cgroup_required` | string, string, boolean | the same | how limits are held, and whether the configuration requires a cgroup |
| `supervisors_adopted`, `last_recovery_ms`, `recovery_changed_records` | number, number or null, number | the same | what the last start found |
| `audit_writable`, `events_writable` | boolean | the same | whether the two journals can be written |
| `starter_failures`, `cancellations_failing`, `cancellation_retries` | number | the same | counts since the service started |
| `unreadable_records` | array of numbers | `unreadable_records` of the host answer | the IDs of the Job directories whose record cannot be read. Such a directory is counted in `capacity.output.recorded_bytes` with everything recorded in it and is never trimmed by the output budget; the Job is in no listing and `job` answers for it with `{"missing":"record"}`, which is told apart from `{"missing":"job"}` for an ID that never existed or was removed. |

Uptime is the service's clock less `started_ms`. Counts per state and free space are in `totals`.

### Resync

`resync` ends a subscription. It arrives as the answer to `subscribe`, as shown above, or as a pushed line:

```json
{"push":"resync","reason":"slow"}
```

`reason` is `slow` when the subscriber fell behind, and the list is open.

On `resync` a client must:

1. stop applying changes, and treat everything it took from this subscription as stale: rows, tree, totals, health and live figures;
2. send `subscribe` again without `after` on the same connection, or on a new one;
3. replace what it held with the snapshot, and ask again for the pages it shows.

It must not go on as if nothing had happened, and it must not send `after`.

### Reading too slowly

The service does not buffer for a subscriber. It keeps the most recent `limits.retained_changes` sequenced lines, 8192 in this version, in one store for all subscribers, and remembers for each subscriber only the last sequence number it wrote. A subscriber for which the next line to write is no longer in the store gets `resync` with reason `slow`. A write that the client does not take within 30 seconds ends the connection. The same store answers `after`.

### After a lost connection or a restart

A client connects, greets, and compares `started_ms` of the greeting with the one it knew. When they are equal it may subscribe with `after` set to the last `seq` it applied. When they differ the service restarted: sequence numbers, cursors and the counters in health belong to the old run, and the client subscribes without `after`.

## Live use

Live figures are sent only for the Jobs in the interest set of a subscription.

```json
{"id":2,"op":"interest","args":{"jobs":[812,811,809,804,815]}}
{"re":2,"ok":{"jobs":5}}
```

`jobs` replaces the whole set. It holds at most `limits.interest_jobs` IDs, 500 in this version; a longer list is refused with an error and the set stays as it was. An ID named twice counts once, and the answer says how many IDs the set holds. An ID that does not exist or does not run is accepted and produces nothing until a Job with that ID runs. A client sends the new set whenever what it shows changes. The request needs a subscription on the same connection.

Once a second the service sends one line with the Jobs of the set whose figures changed since it last sent them to this subscriber:

```json
{"push":"live","at_ms":1790000002000,"jobs":[{"id":812,"attempt":1,"cpu_at_ms":1790000002000,"memory":735051776,"peak_memory":912261120,"pids":43,"peak_pids":58,"written":10485760,"cpu_ms":385100,"throttled_ms":0,"oom_kill":0,"oom_group_kill":0,"pids_max_events":0}]}
```

| Field | Type | From | Meaning |
|---|---|---|---|
| `at_ms` | number | new | when the figures other than the CPU figures were sampled |
| `jobs[].cpu_at_ms` | number or `"not_measured"` | new | when `cpu_ms` and `throttled_ms` of this entry were sampled |
| `id`, `attempt` | number | | the Job and the attempt sampled |
| `memory` | number | the service's sample of `memory.current` | memory in use now, in bytes |
| `peak_memory` | number | `usage.peak_memory` as it grows | highest memory so far |
| `pids` | number | new; `pids.current` | processes and threads now |
| `peak_pids` | number | `usage.peak_pids` as it grows | highest count so far |
| `written` | number | `usage.written` as it grows | bytes written to disk so far |
| `cpu_ms` | number or `"not_measured"` | `usage.cpu_ms` as it grows | CPU time used so far, as a length of time |
| `throttled_ms` | number or `"not_measured"` | `usage.throttled_ms` as it grows | time the Job was held back by its CPU limit |
| `oom_kill`, `oom_group_kill` | number or `"not_measured"` | `oom_kill`, `oom_group_kill` as they grow | OOM kills in the Job's cgroup so far |
| `pids_max_events` | number or `"not_measured"` | `pids_max_events` as it grows | times the process limit was hit |

Guarantees and their limits:

- At most one entry per Job per second.
- A Job that enters the set gets an entry with the next line after its first sample, whether or not anything changed. Until then a client has no figure for it and shows none; it does not show a figure it remembers from before.
- When a Job leaves the set, or ends, no further entry comes. The final figures of an ended Job are in `usage` of its row.
- The service reads the figures of the Jobs in some interest set once a second, while at least one subscription exists, and of no other Job. `cpu_at_ms` says when the CPU figures of an entry were read; it is at most a second old.
- The rate of CPU use is the difference of two `cpu_ms` divided by the difference of their `cpu_at_ms`. A client computes it, and only from two entries; the service sends no rate.
- Without the capability `cgroup`, `memory` is the resident memory of the Job's processes, `pids` their threads, `written` what they wrote, and the four cgroup counters are `not_measured`.
- Any figure is `"unknown"` in an entry when its file could not be read for that sample.

## Errors

In this set an error is one sentence:

```json
{"re":7,"error":{"message":"there is no Queue `nightly`"}}
```

`message` is a sentence of the service, in the language of its environment. What a job command refuses is not in this form; it is in the answer of `command`. A client shows it as it is and does not match on its words. Two members are reserved for the second set and may be absent: `id`, a string that names the kind of error, and `values`, an object with the values the sentence was built from. A client must accept an error with or without them.

Outcomes a client has to act on are not errors and are not sentences: a Job that does not exist is `missing`, an expired cursor is `cursor_expired`, an impossible continuation is `resync`, a Job that does not run is `active: false`.

A line the service cannot read as JSON, or without `op`, or without an `id` that is a number, is answered with `{"error":{"message":"..."}}` without `re`, and the connection is closed. An unknown `op` is answered as an error with `re`; the connection stays open. So is a request whose `args` is not an object.

## Permissions

The socket decides who may connect: the user the service runs as, and, when a socket group is configured, the members of that group. The service checks the peer's credentials on every connection; a file mode alone admits nobody.

Everyone who is admitted may use every request of this document. A member of the socket group reads what the service's user reads through it: every Job of every user with its command, labels, working directory and recorded output, every Queue and Group, health and totals. No request returns the environment of a Job, and credentials in network settings are removed before a record is sent. A member may also change: `command` acts with the rights of the service, reads the files its words name as the service's user, and the audit journal records the peer's user ID and process ID.

Lifecycle events and the audit journal are not on the socket in this set. `peer.may.events` and `peer.may.audit` are false for every peer, the service's user included, and there is no request for them.

## Limits

All are in `limits` of the greeting. A client reads them there and does not assume the numbers below.

| Name | Value | When exceeded |
|---|---|---|
| `line_bytes_in` | 4 194 304 | the service sends an error without `re` and closes the connection |
| `line_bytes_out` | 16 777 216 | the service never sends a longer line; a request whose answer would be longer is refused with an error |
| `page_rows` | 1000 | a larger `limit` is refused with an error |
| `page_rows_default` | 200 | |
| `interest_jobs` | 500 | a longer set is refused with an error; the set stays as it was |
| `connections` | 32 per user ID | a further connection is answered with an error line in place of the greeting and closed |
| `subscriptions` | 4 per user ID | a further `subscribe` is refused with an error; the connection stays open |
| `retained_changes` | 8192 | a subscriber further behind gets `resync` |
| `heartbeat_ms` | 5000 | |
| `live_interval_ms` | 1000 | |
| `output_piece_bytes` | 4 194 304 | a larger `max_bytes` is refused with an error |
| `processes` | 4096 | the answer is cut and says `truncated` |
| `indexed_ended_jobs` | 50 000 | ended Jobs beyond the most recent this many are not in listings; a listing says how many, see `jobs` |
| `ids` | 10 000 | `ids_only` answers at most this many IDs and says in `ids_more` how many it left out |
| `poll_ms` | 5000 | the shortest interval at which a client repeats a request to keep a view fresh. Three repeats are foreseen in this set: `jobs` for the page of queued rows it shows, and `Pressure` and `PressureControl` through `native`. Each not more often than this. A service may refuse a faster repeat with an error. |
| `idle_ms` | 60 000 | a connection without a subscription and without a following output that sends no request for this long is closed by the service. A connection whose followed `output` ended by itself, or was stopped with `end`, is such a connection again from that moment. |

No time limit of this interface applies before the greeting. The service cannot tell which protocol a connection speaks before its first line is complete, so what applies until then is whatever the service applies to every connection.

A write that a client does not take within 30 seconds ends the connection. This holds for every connection of this interface, with or without a subscription.

## What a client must not assume

- That the version of job says what the service can do. The capabilities do.
- That an object has only the members described here, or that pushed lines have only the kinds described here.
- That a missing figure is zero. A word in the place of a number means what Values that are absent says.
- That its own clock agrees with the service's.
- That a cursor has a meaning, lasts, or can be used with another order or filter.
- That a snapshot or a page holds every Job, or that the rows it holds are all rows that match.
- That `matching.total` counts every Job the service has a record of. `matching.ended_not_searched` says how many it left out.
- That a service offers `native`, or that a use of it will work with the next release.
- That nothing happened while it was not connected, or after `resync`.
- That sampled lines arrive every second, or at all while nothing changes.
- That a hole in the sequence numbers of output is lost output.
- That a sentence from the service has a fixed wording.
- That a request it sent and got no answer to was not carried out. For a `submit` the idempotency key tells; for the rest this set has no way to find out, and the second set adds one.
- That `data` in the answer of `command` follows the spellings of this document. It is the command's own output.
- That the shapes behind `native` stay as they are.
- That a member of the socket group may do less than the service's user.
