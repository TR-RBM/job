# Capability matrix

This page lists what job can do, how to ask for it, what it needs from the host, what happens when the host lacks it, and where it is tested. `job host` and `job host --json` report what one host offers, and `job doctor` checks a host and says what to do about each finding.

## How to read the tables

**Needs** names kernel features, cgroup controllers, programs and privileges. "Delegated cgroup" means that the service manages a cgroup v2 subtree it was given, as the [administration guide](administration.md) describes. Without one the service still runs every command and records it, in monitoring mode: it observes process trees instead of using cgroups, and its answers say `limits watched, not enforced`. `[cgroup] required = true` in the configuration forbids that mode; see [operating the service](operations.md#monitoring-mode).

**When the host lacks it** is one of:

- refused at submission: the request is rejected with the reason and no Job is created;
- start error: the Job is created, the program is not executed, and the Job records why;
- reported: the capability is shown as unavailable and nothing else changes;
- watched, not enforced: the service observes the value and reacts when it sees an overrun, without a kernel limit.

**Tested by** names a test in `crates/job/tests/` as `file: test name`, the unit tests of a module, or says that the capability was tried by hand and has no automated test. Tests in `freezer.rs`, and two in `linux_integration.rs`, need a delegated cgroup and run only through `tools/check-delegated`; [contributing](../CONTRIBUTING.md) says how. Everything was run on one host: x86_64, Linux 7.2, Artix with runit, as an ordinary user. What was never run is collected in the [release notes](release-notes.md#known-limits).

## Lifecycle

| Capability | Request | Needs | When the host lacks it | Tested by |
|---|---|---|---|---|
| Run and wait, submit | `job run`, `job submit`, `job wait` | Linux 5.3 for process handles (`pidfd_open`, `pidfd_send_signal`) | the service does not accept work | `watch_backend.rs`: `a_finished_job_answers_with_its_exit_status_and_its_output`, `unix_cli_executes_literal_argv_and_selects_shells_explicitly`, `unix_wait_is_quiet_and_distinguishes_outcomes_from_command_exit_codes` |
| Held work, edit, release | `job create`, `job edit`, `job release` | nothing further | not applicable | `watch_backend.rs`: `a_held_job_can_be_edited_recovered_and_released_without_losing_its_submission` |
| Retry and attempts | `job retry`, `job attempts`, `job logs --attempt N` | nothing further | not applicable | `watch_backend.rs`: `retries_keep_identity_original_submission_and_prior_logs_through_restart_and_edit`, `lost_attempts_require_explicit_acknowledgement_before_retry` |
| Cancel one Job or a subtree | `job cancel`, `job queue cancel`, `job group cancel --recursive` | nothing further; with a delegated cgroup every process of the Job is reached | in monitoring mode processes that left the Job's process tree are not reached | `watch_backend.rs`: `recursive_cancellation_captures_mixed_work_without_starting_selected_waiters`; `freezer.rs`: `recursive_cancellation_thaws_frozen_work_and_preserves_admission_holds` |
| Signal | `job signal -s SIGNAL ID` | process handles | as for run | `watch_backend.rs`: `an_explicit_signal_reaches_the_workload_after_daemon_recovery` |
| Suspend and continue | `job suspend`, `job continue`, and the Queue and Group forms with `--recursive` | delegated cgroup with the freezer (`cgroup.freeze`) | refused; the Job keeps running | `freezer.rs`: `confirmed_suspension_survives_restart_keeps_admission_and_allows_graceful_cancel`; `watch_backend.rs`: `watch_backend_refuses_suspension_without_changing_the_running_job` |
| Time limit | `--time DURATION` | nothing further | not applicable | `freezer.rs`: `elapsed_deadlines_continue_while_a_workload_is_frozen` |
| Remove records | `job remove`, `job queue remove`, `job group remove` | nothing further | not applicable | `watch_backend.rs`: `removal_purges_attempt_data_and_attributed_derivatives_but_preserves_work_files` |
| Move a waiting Job to another Queue | `job move ID --queue PATH` | nothing further | not applicable | `cli_contract.rs`: `cli2_move_keeps_identity_and_waiting_credit` |
| Standard input for an attached run | `job run --stdin` | nothing further; not with `--pty` or `--on` | not applicable | `cli_contract.rs`: `cli2_run_passes_stdin_only_when_asked` |
| Interrupt handling of an attached run | `job run --on-interrupt forward\|detach\|cancel`; forwarding is the default | nothing further | not applicable | `cli_contract.rs`: `cli2_run_forwards_the_first_interrupt_and_detaches_on_the_second` |
| Repeatable submission | `--idempotency-key KEY` | nothing further | not applicable | `durability.rs`: `durable_parallel_clients_with_one_key_get_one_job` |
| Recovery after a service crash | none; always on | nothing further | not applicable | `durability.rs` (44 cases) |

## Scheduling

All of this is decided by the service and needs nothing from the kernel. None of it is active unless configured.

| Capability | Request | Needs | When the host lacks it | Tested by |
|---|---|---|---|---|
| Queues and Groups in one tree | `job queue create`, `job group create`, `rename`, `move` | nothing | not applicable | `watch_backend.rs`: `mixed_groups_keep_queue_identity_through_move_rename_and_restart`, `a_new_queue_does_not_serialize_its_jobs` |
| Concurrency ceiling | `--max-running N` on a Queue or Group | nothing | not applicable | `watch_backend.rs`: `ancestor_concurrency_applies_across_sibling_queues` |
| Pause and close | `pause`, `resume`, `close`, `open` | nothing | not applicable | `watch_backend.rs`: `group_pause_and_close_preserve_independent_queue_state` |
| Admission priority | `--priority N`, `--priority-min`, `--priority-max`, `job reprioritize` | nothing | not applicable | `watch_backend.rs`: `admission_priority_selects_waiting_jobs_without_changing_kernel_weights` |
| Aging | `--aging DURATION` | nothing | not applicable | `watch_backend.rs`: `admission_aging_survives_restart_and_excludes_paused_time` |
| Strict order | `--strict-fifo true` | nothing | not applicable | `watch_backend.rs`: `admission_strict_fifo_overrides_urgency_through_a_group` |
| Backfill | `--backfill conservative` | a time limit or estimate for the Jobs involved | a Job of unknown duration can prevent backfill | unit tests of `admission`: `a_short_backfill_preserves_the_older_large_jobs_start`, `backfill_respects_a_shared_ancestor_budget` |
| Fair share between children | `--fair-share BASIS`, `--share-weight N` | declared requests on the Jobs | a Job without the required request is refused at submission | `watch_backend.rs`: `fair_share_keeps_collection_debt_across_restart_and_selects_before_urgency`, `fair_share_requires_declared_resources_and_basis_changes_require_draining` |
| Profiles and classes | `--execution-profile NAME@REV`, `--class NAME@REV`, `job profile`, `job class` | definitions in the service configuration | an unknown name or revision is refused | `watch_backend.rs`: `versioned_presets_resolve_hierarchy_overrides_and_explicit_absence` |
| Explanation of waiting | `job explain ID` | nothing | not applicable | `watch_backend.rs`: `admission_strict_fifo_overrides_urgency_through_a_group`; `cli_contract.rs`: `cli2_explain_names_the_object_that_supplies_or_blocks_a_setting` |
| Explanation of settings | `job explain PATH [KEY]` | nothing | not applicable | `cli_contract.rs`: `cli2_explain_names_the_object_that_supplies_or_blocks_a_setting` |
| Labels | `--label KEY=VALUE` on Jobs, Queues and Groups, and as a filter of `list`, `queue list` and `group list` | nothing; labels never affect policy | not applicable | `cli_contract.rs`: `cli2_labels_classify_jobs_queues_and_groups` |

Details of labels, moving and explanations: [commands, shells and waiting](cli-execution.md#labels).

## Resources

| Capability | Request | Needs | When the host lacks it | Tested by |
|---|---|---|---|---|
| Admission requests | `--cpu-request`, `--memory-request` | nothing; counted by the service only | not applicable | `freezer.rs`: `resource_requests_leave_kernel_limits_and_weights_unchanged` |
| CPU limit and weight | `--cpu-limit`, `--cpu-weight` | delegated cgroup with the `cpu` controller | refused at submission | `freezer.rs`: `explicit_kernel_limits_do_not_create_requests_and_cpu_quota_throttles`; `watch_backend.rs`: `explicit_kernel_controls_are_refused_without_a_delegated_backend` |
| Memory limits | `--memory-high`, `--memory-max`, `--memory-swap-max` | delegated cgroup with the `memory` controller | refused at submission | `freezer.rs`: `aggregate_memory_max_is_shared_across_jobs_and_reports_oom`; `watch_backend.rs` as above |
| Ceiling shared by a Queue or Group | the same options on `queue` or `group` `create`, `set` | delegated cgroup | refused before the object is created | `freezer.rs`: `aggregate_domains_share_ceilings_and_skip_empty_objects`; `watch_backend.rs`: `aggregate_controls_refuse_watch_backend_before_creating_objects` |
| Device I/O rate limits | `--io-max MAJOR:MINOR,KEY=VALUE...` | delegated cgroup with the `io` controller; a whole-disk device | refused at submission | `freezer.rs`: `device_io_max_enforces_bounded_direct_writes_without_creating_reservations`; `watch_backend.rs`: `io_policies_are_validated_and_do_not_silently_fall_back_to_watching` |
| Device I/O weights | `--io-weight`, `--io-bfq-weight` | as above, and IOCost enabled for the device, or the BFQ scheduler on it | refused at submission | `freezer.rs`: `io_capabilities_refuse_inactive_weight_backends_and_partitions_before_creation` |
| Change controls of running work | `job update`, `job queue update`, `job group update`, `job resource-update` | delegated cgroup | refused | `freezer.rs`: `live_update_changes_controls_and_preserves_the_launch_specification`, `live_memory_reduction_requires_explicit_oom_consent` |
| Process count limit | `--pids-max N`, `--job-pids-max`, and `--pids-max` on a Queue or Group | delegated cgroup with the `pids` controller | refused at submission | `cli_contract.rs`: `cli2_process_count_and_write_budget_have_their_own_names` |
| Write budget | `--write-budget SIZE` | nothing; the service counts bytes written. It is not a file system quota | not applicable | the same test |
| Earlier combined options | `--cores`, `--mem`, `--pids`, `--disk` | delegated cgroup for enforcement | in the `legacy` profile watched, not enforced: the service stops a Job it sees past its declared memory or process count; in the ordinary profile `--mem` and `--pids` are refused with a capability error | `watch_backend.rs`: `a_job_past_its_memory_is_stopped_with_a_record_line`; `operations.rs`: `ops_monitoring_mode_refuses_explicit_limits_with_a_capability_error`, `ops_legacy_profile_keeps_watching_limits_in_monitoring_mode`; `freezer.rs`: `legacy_cores_remain_a_request_and_weight_without_a_cpu_quota` |

`job host --json` reports the controls under `resource_controls`, the absence of a file system quota under `filesystem_quota`, the freezer under `freezer`, and for each block device under `io_devices` whether `io_max`, `io_weight` and `io_bfq_weight` can be used and why not.

## Pressure

| Capability | Request | Needs | When the host lacks it | Tested by |
|---|---|---|---|---|
| Host pressure | `job pressure` | a kernel with PSI (`/proc/pressure`) | reported as unavailable; never shown as zero | `watch_backend.rs`: `pressure_reports_host_availability_without_enabling_a_policy` |
| Pressure of one Job | `job pressure ID` | delegated cgroup and PSI | reported as unavailable; host figures are never substituted | `freezer.rs`: `pressure_reads_the_running_attempts_kernel_scope_and_retires_it_on_exit`; `watch_backend.rs`: `pressure_never_substitutes_host_data_for_a_watch_or_held_job` |
| Admission rules | `[[pressure]]` in the service configuration; `--pressure FILE` on a Queue or Group | PSI; for Queue and Group rules a delegated cgroup | a rule marked `required` holds new starts while its measurement is missing; an optional rule shows the gap and stays as it is | `freezer.rs`: `pressure_admission_holds_only_its_subtree_and_recovers_gradually`; `watch_backend.rs`: `required_pressure_holds_ancestors_preserves_aging_and_recovers_after_removal`, `optional_missing_pressure_is_visible_and_persistence_failure_holds_starts` |
| Kernel notifications for rules | none; used where possible | writable PSI files | periodic sampling goes on | `freezer.rs`: `pressure_notification_permission_failure_keeps_sampling_and_releases_descriptors` |
| Offline inspection | `job pressure parse FILE`, `job pressure replay FILE` | nothing; no service needed | not applicable | `watch_backend.rs`: `pressure_parse_reads_saved_metrics_and_preserves_partial_unavailability`, `pressure_replay_explains_spikes_recovery_missing_data_and_boot_boundaries_offline` |
| Emergency stop under memory shortage | the `legacy` service profile only | `/proc/meminfo`; PSI for the pressure condition | without PSI only the available-memory condition applies | not tested as a whole; the floor arithmetic has unit tests in `host.rs` |

The thresholds of pressure rules were chosen for tests. No value on this page or in [pressure](pressure.md) is a recommendation for production. The emergency stop is described under [Emergency stop in the legacy profile](pressure.md#emergency-stop-in-the-legacy-profile).

## Placement

| Capability | Request | Needs | When the host lacks it | Tested by |
|---|---|---|---|---|
| CPU affinity | `--cpu-affinity LIST` | the CPUs must be allowed to the service | refused at submission | `watch_backend.rs`: `process_policy_affinity_and_rlimits_apply_only_to_workload_and_descendants` |
| NUMA memory policy | `--numa-policy POLICY` | the nodes must be allowed to the service | refused at submission | `watch_backend.rs`: `process_policy_numa_modes_are_read_back_by_the_executed_program`, on a host with one memory node |
| Process resource limits | `--rlimit NAME=SOFT[:HARD]` | a hard limit can be raised only as far as the service's own | refused at submission, or a start error when the kernel refuses at launch | `watch_backend.rs`: `process_policy_rejects_invalid_requests_and_kernel_refusals_before_payload` |

`job host --json` reports `allowed_cpus`, `allowed_memory_nodes` and `inherited_rlimits` under `process_controls`.

## Security

| Capability | Request | Needs | When the host lacks it | Tested by |
|---|---|---|---|---|
| No new privileges | `--no-new-privs yes` | nothing further | start error | `watch_backend.rs`: `security_policy_applies_only_to_requesting_workload_and_descendants` |
| Capability reduction | `--cap-drop NAME,...` or `all` | names known to the kernel; the bounding set is reduced only in a new user namespace or with a privileged service | an unknown name is refused at submission; the result says whether the bounding set was reduced | `watch_backend.rs`: `security_policy_capability_reduction_reports_what_the_launch_context_permits` |
| System call deny list | `--seccomp-deny NAME,...` | seccomp filters; x86_64 or aarch64 | refused at submission | `watch_backend.rs`: `security_policy_rejects_invalid_and_contradictory_requests_before_payload`; aarch64 never run |
| Write confinement | `--confine`, `--allow-write PATH` | Landlock | start error: the program is not executed | `watch_backend.rs`: `a_confined_job_writes_in_its_own_tree_and_is_refused_elsewhere` |
| Access through a Unix group | `[socket] group` in the service configuration | a runtime directory separate from the state directory | the service does not start | `linux_integration.rs`: `another_user_is_admitted_only_through_the_socket_group_and_recorded_as_actor`, `a_private_socket_refuses_another_user_even_when_the_files_are_opened_up` |
| Output and terminals for members of the socket group | none; `job run`, `job logs`, `job log` and `job attach` work unchanged | `[socket] group` | not applicable | `operations.rs`: `ops_a_group_member_gets_output_from_the_service_and_the_state_stays_private`, `ops_a_group_member_attaches_to_a_terminal_in_the_runtime_directory` |
| Audit journal | `job audit`, with `--target`, `--action`, `--since`; `[audit]` in the configuration | the caller reads the state directory, so it is the service user | a member of the socket group cannot run it | `durability.rs`: `durable_audit_records_submit_signal_suspend_cancel_and_retry_with_the_peer`, `review_audit_intent_survives_a_crash_between_the_change_and_its_result`, `review_audit_records_the_program_and_a_digest_instead_of_the_arguments` |

`job host --json` reports `supported_cap_names`, `supported_syscalls` and the service's own `service_no_new_privs` and `service_seccomp_mode` under `security_controls`. What these controls do not do is in [security controls](security-controls.md) and the [security model](security.md).

## Isolation

| Capability | Request | Needs | When the host lacks it | Tested by |
|---|---|---|---|---|
| User, mount, IPC, UTS and cgroup namespaces | `--namespaces KIND,...` | the kernel has the kind; unprivileged user namespaces for a service that runs without privilege | refused at submission | `watch_backend.rs`: `isolation_policy_enters_only_the_requested_namespaces`, `isolation_policy_refuses_unavailable_and_incomplete_requests_before_payload` |
| PID namespace with an init and a fresh `/proc` | `--namespaces user,mount,pid` | the kernel has PID namespaces and lets the launch mount a fresh `/proc`; `mount` in the list; unprivileged user namespaces and `user` in the list for a service that runs without privilege | refused at submission, saying what to add or the kernel's error | `watch_backend.rs`: `pidns_command_runs_under_an_init_in_its_own_namespace_with_a_fresh_proc`, `pidns_refuses_requests_without_mount_or_user_and_says_what_to_add`; `freezer.rs`: `pidns_leftovers_exit_status_and_cancel_are_recorded_from_the_cgroup` |
| Read-only root with writable paths | `--root read-only`, `--writable PATH,...` | `user` and `mount` namespaces; `mount_setattr` (Linux 5.12) | refused at submission | `watch_backend.rs`: `isolation_policy_mount_policies_limit_writes_to_named_paths_and_private_tmp` |
| Private temporary directory | `--private-tmp yes` | `user` and `mount` namespaces | refused at submission | the same test |

`job host --json` reports `namespace_kinds`, `requestable_namespaces`, `pid_namespace_requestable`, `pid_namespace_error`, `unprivileged_user_namespaces`, `mount_setattr` and `landlock_abi` under `isolation_controls`. A refusal by the kernel inside the launch, which would be a start error naming the step, was never provoked in a test.

## Networking

| Capability | Request | Needs | When the host lacks it | Tested by |
|---|---|---|---|---|
| Host network | `--net default`, or no option | nothing | not applicable | not tested as a boundary; it is the absence of one |
| No network | `--net none` | unprivileged user namespaces | refused at submission | `watch_backend.rs`: `network_boundary_no_network_leaves_only_loopback_without_ipv4_ipv6_or_dns` |
| SOCKS5, HTTP or HTTPS proxy | `--net socks5://HOST:PORT`, `http://`, `https://`, `--net-secret-file FILE` | user namespaces; `unshare`, `nsenter`, `sleep`, `ip` (iproute2 4.18 or later), `tc`, `nft`, `slirp4netns` | refused at submission, naming what is missing | `watch_backend.rs`: `network_boundary_a_proxy_job_has_no_ipv6_udp_icmp_or_dns_of_its_own`, `network_boundary_missing_tools_refuse_the_job_with_their_names`; only SOCKS5 was run |
| WireGuard tunnel | `--net wireguard:FILE` | as for a proxy, and WireGuard in the kernel | refused at submission | `watch_backend.rs`: `network_boundary_a_wireguard_job_reaches_only_its_tunnel_and_nothing_once_the_peer_is_gone` |
| Bandwidth ceiling | `--bandwidth RATE`; `--bandwidth` and `--job-bandwidth` with `job queue set` | as for a proxy | refused at submission | `watch_backend.rs`: `network_boundary_a_bandwidth_holds_udp_as_well_as_tcp`; the rates were also measured by hand with iperf3 |
| One network shared by a Queue | `--net` on `job queue create` or `set`; `--bandwidth` on `job queue set` | as for a proxy | refused | `watch_backend.rs`: `network_boundary_jobs_of_one_queue_share_a_holder_and_cannot_reach_each_other` |
| Proxy credentials kept out of records | `--net-secret-file FILE` | a private file of the service user | a file with wider permissions is refused | `watch_backend.rs`: `network_boundary_proxy_credentials_stay_out_of_answers_records_and_arguments` |

| An existing network namespace | `--net ns:NAME`, with `[network.namespaces.NAME]` in the service configuration | the namespace file; the privilege to join it, or `user_namespace` naming a user namespace the service's user made | refused at submission with the kernel's reason | `network_profiles.rs`: `network_namespace_jobs_join_the_namespace_the_configuration_names_and_share_it`, `network_namespace_refusals_name_the_reason`; a namespace made by root was not joined by a privileged service |
| Named routing profile | `--net profile:NAME`, with `[network.profiles.NAME]` in the service configuration | as for a proxy, when the profile filters, shapes or routes; nothing for a plain `egress = "host"` | refused at submission, naming what is missing | `network_profiles.rs`: `network_profile_allow_and_deny_rules_decide_what_a_job_reaches`, `network_profile_sharing_decides_how_many_holders_serve_its_jobs`, `network_profile_egress_none_namespace_and_proxy_keep_credentials_out_of_sight`, `network_profile_definitions_are_resolved_at_launch_and_references_are_guarded`, `network_profile_jobs_keep_their_shared_network_across_a_service_restart`; a WireGuard profile was not run |

A Job behind a proxy, a tunnel or a bandwidth ceiling has IPv4 only, and so has a Job behind a profile that filters. `job host --json` lists the configured namespaces and profiles under `network`, each with whether this host can use it. `job host --json` reports `missing_link_tools`, `ipv6_in_linked_network` and `network_modes` under `isolation_controls`.

## Output

| Capability | Request | Needs | When the host lacks it | Tested by |
|---|---|---|---|---|
| Separate stdout and stderr, live | `job run`, `job logs --follow`, `--stream` | nothing | not applicable | `watch_backend.rs`: `stream_run_delivers_binary_stdout_and_stderr_before_exit`, `stream_follow_survives_daemon_restart_and_pins_retry_attempt` |
| Bounded retention with visible gaps | none; always on | nothing | not applicable | `watch_backend.rs`: `stream_retention_is_bounded_and_reports_gaps` |
| Per-stream quotas | `--output-head SIZE`, `--output-tail SIZE`, `[output]` in the configuration | nothing | not applicable | `watch_backend.rs`: `output_quota_keeps_head_and_tail_of_each_stream_and_names_the_gap` |
| Service output budget | `budget_bytes` under `[output]` | nothing | not applicable | `watch_backend.rs`: `output_quota_service_budget_trims_oldest_completed_recordings_visibly` |
| Structured output | `--json`, `--format json` | nothing | not applicable | `cli_contract.rs`: `contract_format_json_wraps_the_same_data`, `cli2_structured_output_covers_control_commands_and_usage_errors` |
| Listings of completed Jobs, tabular output | `job list --all`, `--state`, `--limit`, `--format tsv`; `--format tsv` on `queue list` and `group list` | nothing | not applicable | `cli_contract.rs`: `cli2_list_shows_completed_jobs_as_table_json_and_tsv` |

## Terminals

| Capability | Request | Needs | When the host lacks it | Tested by |
|---|---|---|---|---|
| Persistent shared terminal | `job run --pty`, `job submit --pty`, `job attach ID` | pseudo-terminals; the client is admitted to the service | a client that is not admitted is refused | `terminal_sessions.rs`: `two_clients_write_and_reattach_to_the_same_terminal`, `terminal_survives_daemon_restart` |
| Detach key | `--detach-key KEY`, `JOB_DETACH_KEY`, `detach_key` under `[terminal]` | nothing | not applicable | `terminal_sessions.rs`: `detach_key_option_replaces_the_default_and_passes_the_old_key_through` |

Terminals over `--on` are not supported. Attaching over an SSH login to the host of the service is described in the [user guide](user-guide.md#terminals) and has no automated test.

## Remote

| Capability | Request | Needs | When the host lacks it | Tested by |
|---|---|---|---|---|
| Run on another host | `--on USER@HOST`, `--ssh-key`, `--ssh-option`; `--on` on a Queue | `ssh` here; job installed and its service running there, with the same remote protocol | refused, naming both protocol versions | `watch_backend.rs`: `a_remote_request_is_answered_by_the_hosts_daemon`, `a_remote_request_of_another_protocol_is_refused_with_both_versions`, with a local stand-in for the other host |
| Copy files there and back | `--send PATH`, `--fetch PATH` | as above | as above | not tested in the current suite |
| Ask another host what it offers | `job host --on USER@HOST` | as above | refused | `watch_backend.rs`: `a_remote_request_is_answered_by_the_hosts_daemon` |

## Administration

| Capability | Request | Needs | When the host lacks it | Tested by |
|---|---|---|---|---|
| Host and setup check | `job doctor` | nothing; works without the service | not applicable | `linux_integration.rs`: `doctor_works_without_a_service_and_speaks_german` |
| Conventional file locations | none; `JOB_CONFIG`, `JOB_STATE_DIR`, `JOB_RUNTIME_DIR`, `JOB_CACHE_DIR` override | a home directory or the variables | the command fails and names the variable to set | `linux_integration.rs`: `user_paths_follow_the_xdg_fallbacks_and_overrides` |
| Discovery of the delegated cgroup | none; `JOB_CGROUP_ROOT` or `root` under `[cgroup]` to name it | a cgroup v2 subtree that holds only the service | monitoring mode, with the reason given | `linux_integration.rs`: `the_cgroup_root_is_discovered_from_delegation_and_named_at_startup`, `a_cgroup_root_holding_other_processes_is_refused` |
| Configuration check, view and reload | `job config check`, `job config init`, `job config show`, `job config reload` | reload needs a drained service | reload is refused while work is active or waiting | `watch_backend.rs`: `service_profile_changes_require_a_drained_restart` |
| State validation, backup, restore, migration | `job state validate`, `backup`, `restore`, `migrate` | a stopped service and no active work | refused | `state_migration.rs` (48 cases) |
| Staged installation | `make install` with `PREFIX` and `DESTDIR` | `make`, a built binary | not applicable | `tools/install-check` |
| Lifecycle events | `job events`, with `--job`, `--queue`, `--since`, `--follow`; `[events]` in the configuration | the caller reads the state directory | a member of the socket group cannot run it | `operations.rs`: `ops_events_record_every_job_transition_with_its_exit`, `ops_events_follow_prints_new_records_across_rotation_without_a_gap` |
| Queue depth and wait statistics | `job queue show PATH`, `job group show PATH` | nothing | not applicable | `operations.rs`: `ops_queue_and_group_show_report_depth_and_wait_statistics` |
| Health | `job host`, the `health` check of `job doctor` | nothing | not applicable | `operations.rs`: `ops_host_reports_health_and_doctor_reads_it` |
| Limits around the service | `surrounding_limits` in `job host --json`, `job doctor` | a readable cgroup of the service | reported as not discoverable | `operations.rs`: `ops_host_and_doctor_report_the_limits_around_the_service` |
| Metrics in the Prometheus text format | `job metrics`; `listen` under `[metrics]` for HTTP at `/metrics` | a TCP port for the listener; the `job_use_` families need a delegated cgroup | without a cgroup the `job_use_` families are left out; an address that cannot be bound stops the start | `metrics.rs`: `metrics_command_agrees_with_list_and_host`, `metrics_endpoint_serves_only_get_metrics`, `metrics_listener_is_absent_unless_configured`, `metrics_address_in_use_stops_the_start`; `freezer.rs`: `metrics_report_the_use_of_the_jobs_cgroup` |
| Monitoring mode forbidden | `required = true` under `[cgroup]` | a delegated cgroup | the service refuses to start and says what is missing | `operations.rs`: `ops_required_cgroup_refuses_to_start_without_one_and_says_what_is_missing`, `ops_monitoring_mode_is_announced_and_reported` |
| The system service from another account | `job --system COMMAND`, `JOB_SYSTEM=1` | a system service and admission to its socket | no service is reached | `linux_integration.rs`: `review_the_system_option_selects_the_system_instance`; no system service was run as root |
| systemd system and user units | installed by `make install` | systemd; 254 or later for `DelegateSubgroup=` | not known | not tested: the units were never run under systemd |
| runit service | the example `runit/run` | runit, root to install it | not applicable | the shipped script was checked for shell syntax only; `service_manager.rs`: `runit_restart_term_kill_and_down_up_keep_running_jobs_adopted_and_controllable` runs `jobd` under a private `runsv` |
| Help, syntax reference, completion | `job help`, `job completion bash`, `job completion fish`, `job completion zsh` | nothing | not applicable | `cli_contract.rs`: `contract_completion_scripts_are_generated`, `contract_zsh_completion_offers_what_bash_offers`, `contract_syntax_reference_is_generated` |

Events, depth, health, the limits around the service and monitoring mode are described in [operating the service](operations.md); the audit journal in [reliability](reliability.md#the-audit-journal); metrics in [metrics](metrics.md).

## Desktop

These are optional and outside the core execution model.

| Capability | Request | Needs | When the host lacks it | Tested by |
|---|---|---|---|---|
| Place a Job's windows on a monitor | `--monitor M` | an X11 display | start error naming the monitors that exist | tried by hand on one desktop with two monitors; no automated test |
| Capture a monitor or a Job's window | `job screenshot` | an X11 display | not determined | tried by hand on one desktop with two monitors; no automated test |
| Hold an exclusive device | `--device NAME` | the device is known to the service | a name the service does not know is refused at submission | not tested in the current suite |

## Not available

These are not in this release. Requests for them are refused or have no option at all.

- Joining a network namespace the service configuration does not name. A client cannot give a path.
- Routes, interfaces or address translation chosen by a profile. A routing profile selects one exit and filters destinations; it does not build a topology.
- OpenVPN. `--net openvpn:FILE` is refused.
- IPv6 behind a proxy, a tunnel or a bandwidth ceiling.
- Filesystem quotas. `--write-budget` counts bytes written and is not a quota on occupied space.
- Preemption. A running Job is never stopped to make room for another.
- Dependencies between Jobs.
- Delayed, recurring or calendar-based submission.
- Automatic retries.
- Roles and per-object permissions. Every admitted client has the same control.
- Execution as different Unix users. All work runs as the user of the service.
- `job audit` and `job events` for a socket group member who is not the service user.
- A configurable choice of which Job is stopped under shortage. Only the `legacy` profile stops Jobs, by its fixed rule.
- Terminals for Jobs on other hosts.
- Placement across several hosts, high availability and checkpointing.
