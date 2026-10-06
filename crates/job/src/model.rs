use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::resources::{Reservation, Vector};

pub const PROTOCOL: u32 = 20;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declared {
    #[serde(default, flatten)]
    pub isolation: crate::isolation::Controls,
    #[serde(default, flatten)]
    pub output: crate::streams::quota::Controls,
    #[serde(default, flatten)]
    pub security: crate::security::Controls,
    #[serde(default, flatten)]
    pub process: crate::process_policy::Controls,
    #[serde(default)]
    pub execution_profile: Option<String>,
    #[serde(default)]
    pub scheduling_class: Option<String>,
    #[serde(default)]
    pub priority: Option<i32>,
    #[serde(default, flatten)]
    pub resources: crate::resource_policy::Controls,
    #[serde(default)]
    pub terminal: Option<crate::terminal::Size>,
    pub cores_milli: Option<u64>,
    pub memory: Option<u64>,
    pub pids: Option<u64>,
    pub disk: Option<u64>,
    pub devices: Vec<String>,
    pub wall_ms: Option<u64>,
    #[serde(default)]
    pub confine: bool,
    #[serde(default)]
    pub allow_write: Vec<PathBuf>,
    #[serde(default)]
    pub dir: Option<PathBuf>,
    #[serde(default)]
    pub net: Option<Net>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_secret_file: Option<PathBuf>,
    #[serde(default)]
    pub bandwidth: Option<u64>,
    #[serde(default)]
    pub on: Option<Remote>,
    #[serde(default)]
    pub send: Vec<PathBuf>,
    #[serde(default)]
    pub fetch: Vec<PathBuf>,
    #[serde(default)]
    pub monitor: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub labels: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stdin: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    pub target: String,
    #[serde(default)]
    pub key: Option<PathBuf>,
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Net {
    Host,
    None,
    Proxy(String),
    WireGuard(PathBuf),
    OpenVpn(PathBuf),
    Namespace(String),
    Profile(String),
}

impl Net {
    pub fn parse(text: &str) -> Result<Net, String> {
        let proxy = ["socks5://", "socks5h://", "http://", "https://"];
        match text {
            "default" | "host" => Ok(Net::Host),
            "none" => Ok(Net::None),
            _ if proxy.iter().any(|p| text.starts_with(p)) => {
                crate::link::proxy_target(text)?;
                Ok(Net::Proxy(text.to_string()))
            }
            _ => match text.split_once(':') {
                Some(("ns", name)) => crate::netpolicy::named(name).map(Net::Namespace),
                Some(("profile", name)) => crate::netpolicy::named(name).map(Net::Profile),
                Some(("wireguard", path)) if !path.is_empty() => {
                    Ok(Net::WireGuard(PathBuf::from(path)))
                }
                Some(("openvpn", path)) if !path.is_empty() => {
                    Ok(Net::OpenVpn(PathBuf::from(path)))
                }
                _ => Err(format!(
                    "--net: `{}` is not a network; write default, none, socks5://HOST:PORT, http://HOST:PORT, https://HOST:PORT, wireguard:FILE, openvpn:FILE, ns:NAME or profile:NAME",
                    crate::netsecret::redact(text)
                )),
            },
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Net::Host => "the host's network".to_string(),
            Net::None => "no network".to_string(),
            Net::Proxy(url) => format!(
                "network only through the proxy {}",
                crate::netsecret::split(url).0
            ),
            Net::WireGuard(path) => format!("network only through wireguard:{}", path.display()),
            Net::OpenVpn(path) => format!("network only through openvpn:{}", path.display()),
            Net::Namespace(name) => crate::netpolicy::describe("ns", name),
            Net::Profile(name) => crate::netpolicy::describe("profile", name),
        }
    }

    pub fn named(&self) -> bool {
        matches!(self, Net::Namespace(_) | Net::Profile(_))
    }

    pub fn needs_link(&self) -> bool {
        matches!(self, Net::Proxy(_) | Net::WireGuard(_) | Net::OpenVpn(_))
    }

    pub fn absolute(self, base: &std::path::Path) -> Net {
        match self {
            Net::WireGuard(path) => Net::WireGuard(base.join(path)),
            Net::OpenVpn(path) => Net::OpenVpn(base.join(path)),
            other => other,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub job_namespaces: Option<crate::isolation::Namespaces>,
    #[serde(default)]
    pub job_root: Option<crate::isolation::Root>,
    #[serde(default)]
    pub job_private_tmp: Option<bool>,
    #[serde(default)]
    pub job_writable: Option<crate::isolation::Writable>,
    #[serde(default)]
    pub job_output_head_bytes: Option<u64>,
    #[serde(default)]
    pub job_output_tail_bytes: Option<u64>,
    #[serde(default)]
    pub job_no_new_privs: Option<bool>,
    #[serde(default)]
    pub job_cap_drop: Option<crate::security::CapDrop>,
    #[serde(default)]
    pub job_seccomp_deny: Option<crate::security::Deny>,
    #[serde(default)]
    pub job_cpu_affinity: Option<crate::process_policy::Affinity>,
    #[serde(default)]
    pub job_numa_policy: Option<crate::process_policy::Numa>,
    #[serde(default)]
    pub job_rlimit_as: Option<crate::process_policy::Pair>,
    #[serde(default)]
    pub job_rlimit_core: Option<crate::process_policy::Pair>,
    #[serde(default)]
    pub job_rlimit_cpu: Option<crate::process_policy::Pair>,
    #[serde(default)]
    pub job_rlimit_data: Option<crate::process_policy::Pair>,
    #[serde(default)]
    pub job_rlimit_fsize: Option<crate::process_policy::Pair>,
    #[serde(default)]
    pub job_rlimit_memlock: Option<crate::process_policy::Pair>,
    #[serde(default)]
    pub job_rlimit_msgqueue: Option<crate::process_policy::Pair>,
    #[serde(default)]
    pub job_rlimit_nice: Option<crate::process_policy::Pair>,
    #[serde(default)]
    pub job_rlimit_nofile: Option<crate::process_policy::Pair>,
    #[serde(default)]
    pub job_rlimit_nproc: Option<crate::process_policy::Pair>,
    #[serde(default)]
    pub job_rlimit_rtprio: Option<crate::process_policy::Pair>,
    #[serde(default)]
    pub job_rlimit_rttime: Option<crate::process_policy::Pair>,
    #[serde(default)]
    pub job_rlimit_sigpending: Option<crate::process_policy::Pair>,
    #[serde(default)]
    pub job_rlimit_stack: Option<crate::process_policy::Pair>,

    #[serde(default)]
    pub job_execution_profile: Option<String>,
    #[serde(default)]
    pub job_scheduling_class: Option<String>,
    #[serde(default)]
    pub job_io_max: Option<crate::io_policy::Maxima>,
    #[serde(default)]
    pub job_io_weight: Option<crate::io_policy::Weights>,
    #[serde(default)]
    pub job_io_bfq_weight: Option<crate::io_policy::Weights>,
    #[serde(default)]
    pub job_cpu_request_milli: Option<u64>,
    #[serde(default)]
    pub job_cpu_limit_milli: Option<crate::resource_policy::Limit>,
    #[serde(default)]
    pub job_cpu_weight: Option<u64>,
    #[serde(default)]
    pub job_memory_request: Option<u64>,
    #[serde(default)]
    pub job_memory_high: Option<crate::resource_policy::Limit>,
    #[serde(default)]
    pub job_memory_max: Option<crate::resource_policy::Limit>,
    #[serde(default)]
    pub job_memory_swap_max: Option<crate::resource_policy::Limit>,
    #[serde(default)]
    pub job_pids_max: Option<crate::resource_policy::Limit>,
    #[serde(default)]
    pub dir: Option<PathBuf>,
    #[serde(default)]
    pub cores_milli: Option<u64>,
    #[serde(default)]
    pub memory: Option<u64>,
    #[serde(default)]
    pub net: Option<Net>,
    #[serde(default)]
    pub net_secret_file: Option<PathBuf>,
    #[serde(default)]
    pub bandwidth: Option<u64>,
    #[serde(default)]
    pub job_bandwidth: Option<u64>,
    #[serde(default)]
    pub on: Option<Remote>,
    #[serde(default)]
    pub monitor: Option<String>,
    #[serde(default)]
    pub host_cores_milli: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkRef {
    pub name: String,
    pub job: crate::link::JobLink,
    pub cap: Option<u64>,
    pub link_rate: Option<u64>,
    #[serde(default)]
    pub egress: Option<Net>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default)]
    pub resolver: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spec {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub session: String,
    pub declared: Declared,
    #[serde(default)]
    pub queue: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queue {
    pub name: String,
    pub parallel: Option<u64>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub draining: bool,
    pub created_ms: u64,
    #[serde(default)]
    pub settings: Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Parallel {
    All,
    Jobs(u64),
}

impl Parallel {
    pub fn limit(self) -> Option<u64> {
        match self {
            Parallel::All => None,
            Parallel::Jobs(n) => Some(n),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueChange {
    pub parallel: Option<Parallel>,
    pub paused: Option<bool>,
    #[serde(default)]
    pub settings: Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backend {
    Cgroup,
    Watch,
}

impl Backend {
    pub fn describe(self) -> &'static str {
        match self {
            Backend::Cgroup => "limits enforced by cgroup",
            Backend::Watch => "limits watched, not enforced",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopKind {
    Memory,
    Processes,
    Disk,
    FilesystemFloor,
    HostMemory,
    MemoryPressure,
    WallTime,
    Cancelled,
    WorkingDirectoryGone,
    LimitChanged,
    DaemonLost,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stop {
    pub kind: StopKind,
    pub line: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub peak_memory: u64,
    pub peak_pids: u64,
    pub cpu_ms: u64,
    pub throttled_ms: u64,
    pub written: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShimResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(default)]
    pub isolation_controls: Option<crate::isolation::kernel::Applied>,
    #[serde(default)]
    pub output_retention: Option<crate::streams::quota::Retention>,
    #[serde(default)]
    pub security_controls: Option<crate::security::kernel::Applied>,
    #[serde(default)]
    pub process_controls: Option<crate::process_policy::kernel::Applied>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub finished_ms: u64,
    pub leftover_processes: u64,
    #[serde(default)]
    pub leftover_names: Vec<String>,
    #[serde(default)]
    pub kept_helpers: Vec<String>,
    #[serde(default)]
    pub output_held_open: bool,
    pub usage: Usage,
    pub oom_group_kill: u64,
    #[serde(default)]
    pub oom_kill: u64,
    pub pids_max_events: u64,
    pub output_bytes: u64,
    #[serde(default)]
    pub output_error: Option<String>,
    pub start_error: Option<String>,
    #[serde(default)]
    pub remote: Option<String>,
    #[serde(default)]
    pub remote_lines: Vec<String>,
    #[serde(default)]
    pub monitor: Option<String>,
    #[serde(default)]
    pub windows: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Held,
    Queued,
    Starting,
    Running,
    Suspended,
    Stopping,
    Succeeded,
    Failed,
    Cancelled,
    Lost,
    Finished,
}

impl State {
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Lost | Self::Finished
        )
    }

    pub fn active(&self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Running | Self::Suspended | Self::Stopping
        )
    }

    pub fn editable(&self) -> bool {
        matches!(self, Self::Held | Self::Queued)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    #[serde(default, flatten)]
    pub durability: crate::durability::Record,
    #[serde(default)]
    pub output_mode: Option<crate::streams::Mode>,
    #[serde(default)]
    pub preset_snapshot: Option<crate::presets::Snapshot>,
    #[serde(default)]
    pub admission_snapshot: Option<crate::admission::Explanation>,
    #[serde(default)]
    pub priority_source: Option<crate::resource_policy::Origin>,
    #[serde(default)]
    pub priority_changes: Vec<crate::admission::PriorityChange>,
    #[serde(default)]
    pub applied_resources: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub resource_sources: std::collections::BTreeMap<String, crate::resource_policy::Origin>,
    #[serde(default)]
    pub workload_cgroup: Option<PathBuf>,
    #[serde(default)]
    pub aggregate_domains: Vec<crate::aggregate::Domain>,
    #[serde(default)]
    pub suspension: crate::freezer::Suspension,
    #[serde(default)]
    pub timing: crate::freezer::Timing,
    #[serde(default = "first_attempt")]
    pub attempt: u64,
    #[serde(default)]
    pub attempt_submitted_ms: Option<u64>,
    #[serde(default)]
    pub submitted_spec: Option<Spec>,
    #[serde(default)]
    pub requested_spec: Option<Spec>,
    #[serde(default)]
    pub effective_spec: Option<Spec>,
    #[serde(default)]
    pub released_ms: Option<u64>,
    #[serde(default = "crate::config::legacy_record")]
    pub policy: crate::config::Profile,
    #[serde(default)]
    pub queue_id: Option<u64>,
    pub id: u64,
    pub spec: Spec,
    pub key: String,
    pub reservation: Reservation,
    pub backend: Backend,
    pub state: State,
    pub submitted_ms: u64,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
    pub shim_pid: Option<i32>,
    pub shim_start_ticks: Option<u64>,
    #[serde(default)]
    pub supervisor_boot_id: Option<String>,
    #[serde(default)]
    pub termination_deadline_ms: Option<u64>,
    #[serde(default)]
    pub waited_for: Option<String>,
    pub stop: Option<Stop>,
    pub result: Option<ShimResult>,
    pub usage: Usage,
    pub log: PathBuf,
    #[serde(default)]
    pub link: Option<LinkRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<crate::netpolicy::Applied>,
}

fn first_attempt() -> u64 {
    1
}

impl Job {
    pub fn update_timing(&mut self, now: u64) {
        let end = self.finished_ms.unwrap_or(now);
        let elapsed = self.started_ms.map_or(0, |start| end.saturating_sub(start));
        let suspended = self
            .suspension
            .total_ms
            .saturating_add(
                self.suspension
                    .since_ms
                    .map_or(0, |start| end.saturating_sub(start)),
            )
            .min(elapsed);
        self.timing = crate::freezer::Timing {
            elapsed_ms: elapsed,
            active_ms: elapsed.saturating_sub(suspended),
            suspended_ms: suspended,
        };
    }
    pub fn waiting_since(&self) -> u64 {
        self.released_ms
            .or(self.attempt_submitted_ms)
            .unwrap_or(self.submitted_ms)
    }
    pub fn outcome(&self) -> State {
        match self.stop.as_ref().map(|stop| stop.kind) {
            Some(StopKind::Cancelled) => State::Cancelled,
            Some(StopKind::DaemonLost) => State::Lost,
            Some(_) => State::Failed,
            None if self.succeeded() => State::Succeeded,
            None => State::Failed,
        }
    }
    pub fn succeeded(&self) -> bool {
        self.stop.is_none() && self.result.as_ref().is_some_and(|r| r.exit_code == Some(0))
    }

    pub fn exit_description(&self) -> String {
        match &self.result {
            Some(ShimResult {
                start_error: Some(error),
                ..
            }) => format!("did not start: {error}"),
            Some(ShimResult {
                exit_code: Some(code),
                ..
            }) => format!("exit {code}"),
            Some(ShimResult {
                signal: Some(signal),
                ..
            }) => format!("killed by signal {signal}"),
            _ => "no exit status".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    #[serde(default)]
    pub job_id: Option<u64>,
    #[serde(default)]
    pub attempt: Option<u64>,
    pub key: String,
    pub finished_ms: u64,
    pub wall_ms: u64,
    pub usage: Usage,
    pub stop: Option<StopKind>,
    pub memory_limit: u64,
    pub succeeded: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Env {
    pub vars: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    PressureControl,
    Pressure {
        id: Option<u64>,
    },
    Explain {
        id: u64,
    },
    Reprioritize {
        id: u64,
        attempt: u64,
        priority: i32,
    },
    ResourceUpdate {
        target: crate::resource_update::Target,
        patch: std::collections::BTreeMap<String, serde_json::Value>,
        allow_oom: bool,
        expected: Option<Box<crate::resource_update::Plan>>,
    },
    ResourceUpdateStatus {
        operation: String,
        #[serde(default)]
        abandon: bool,
    },
    Versioned {
        protocol: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<u32>,
        request: Box<Request>,
    },
    CancelSelection {
        target: crate::cancellation::Target,
        recursive: bool,
        expected: Option<crate::cancellation::Selection>,
    },
    Remove {
        target: crate::removal::Target,
        recursive: bool,
        allow_lost: bool,
        expected: Option<crate::removal::Selection>,
    },
    Freeze {
        target: crate::freezer::Target,
        recursive: bool,
        frozen: bool,
        timeout_ms: u64,
    },
    Retry {
        id: u64,
        expected_attempt: u64,
        held: bool,
        env: Option<Env>,
        queue: Option<String>,
        allow_lost: bool,
    },
    Attempts {
        id: u64,
    },
    Create {
        spec: Box<Spec>,
        env: Env,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    Edit {
        id: u64,
        spec: Box<Spec>,
        env: Env,
    },
    Release {
        id: u64,
    },
    Signal {
        id: u64,
        signal: i32,
    },
    Config {
        reload: bool,
    },
    Object {
        kind: crate::objects::Kind,
        operation: crate::objects::Operation,
    },
    Attach {
        id: u64,
    },
    Submit {
        spec: Box<Spec>,
        env: Env,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    Wait {
        id: u64,
        timeout_ms: u64,
    },
    Status {
        id: u64,
    },
    Queue,
    Cancel {
        id: u64,
        session: String,
    },
    Done {
        id: u64,
    },
    Ping,
    QueueAdd {
        name: String,
        change: QueueChange,
    },
    QueueSet {
        name: String,
        change: QueueChange,
    },
    QueueRemove {
        name: String,
        drain: bool,
    },
    QueueClear {
        name: String,
    },
    Host,
    Output {
        id: u64,
        #[serde(default)]
        attempt: Option<u64>,
        #[serde(default)]
        follow: bool,
    },
    LogQuery {
        id: u64,
        #[serde(default)]
        attempt: Option<u64>,
        #[serde(default)]
        query: Vec<String>,
    },
    Extended {
        call: crate::cli2::Call,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    #[serde(default)]
    pub isolation_controls: Option<crate::isolation::kernel::Capabilities>,
    #[serde(default)]
    pub security_controls: crate::security::kernel::Capabilities,
    #[serde(default)]
    pub process_controls: crate::process_policy::kernel::Capabilities,
    #[serde(default)]
    pub output_protocol: Option<u32>,
    #[serde(default)]
    pub io_devices: Vec<crate::io_policy::Capability>,
    #[serde(default)]
    pub resource_controls: std::collections::BTreeMap<String, bool>,
    #[serde(default)]
    pub freezer: bool,
    #[serde(default)]
    pub pidfd: bool,
    #[serde(default)]
    pub terminal_protocol: Option<u32>,
    #[serde(default)]
    pub service: Option<crate::service::Info>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable_records: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<crate::pacing::Activity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<crate::operations::health::Health>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surrounding_limits: Option<crate::operations::limits::Surrounding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem_quota: Option<crate::cli2::capability::Feature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<crate::netpolicy::Report>,
    pub protocol: u32,
    pub version: String,
    pub hostname: String,
    pub kernel: String,
    pub cores: u64,
    pub memory_total: u64,
    pub memory_available: u64,
    pub backend: Backend,
    pub pool: Vector,
    pub devices: Vec<String>,
    pub queues: Vec<Queue>,
    pub running: usize,
    pub waiting: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueView {
    pub jobs: Vec<QueueEntry>,
    #[serde(default)]
    pub queues: Vec<Queue>,
    pub backend: Backend,
    pub pool: crate::resources::Vector,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub job: Job,
    pub predicted_start_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Response {
    PressureControl {
        ledger: crate::pressure::control::Ledger,
        error: Option<String>,
        sampling_interval_ms: u64,
        notifications: crate::pressure::notify::Report,
    },
    Pressure {
        snapshot: crate::pressure::Snapshot,
    },
    Explanation {
        explanation: crate::admission::Explanation,
    },
    ResourceUpdatePreview {
        plan: Box<crate::resource_update::Plan>,
    },
    ResourceUpdate {
        operation: Box<crate::resource_update::Operation>,
    },
    CancellationPreview {
        schema_version: u32,
        selection: crate::cancellation::Selection,
    },
    Cancellation {
        operation: crate::cancellation::Operation,
    },
    RemovalInProgress {
        receipt: crate::removal::Receipt,
        message: String,
    },
    RemovalPreview {
        preview: crate::removal::Preview,
    },
    Removed {
        receipt: crate::removal::Receipt,
    },
    Controlled {
        schema_version: u32,
        results: Vec<crate::freezer::Outcome>,
    },
    RetryPending,
    Attempts {
        schema_version: u32,
        attempts: Vec<Job>,
    },
    Signalled {
        delivered: usize,
    },
    Configuration {
        effective: crate::config::Effective,
    },
    Objects {
        schema_version: u32,
        objects: Vec<crate::objects::View>,
    },
    Attached {
        path: PathBuf,
        terminal_protocol: u32,
    },
    Submitted {
        job: Job,
    },
    Finished {
        job: Job,
    },
    StillRunning {
        job: Job,
    },
    Queue {
        view: QueueView,
    },
    Cancelled {
        job: Job,
    },
    Pong {
        backend: Backend,
        pool: crate::resources::Vector,
    },
    Error {
        message: String,
    },
    Unsupported {
        min: u32,
        max: u32,
        version: String,
    },
    Done {
        line: String,
    },
    Host {
        info: Box<HostInfo>,
    },
    Output {
        attempt: u64,
        #[serde(default)]
        mode: Option<crate::streams::Mode>,
        #[serde(default)]
        output_protocol: u32,
    },
    Extended {
        answer: Box<crate::cli2::Answer>,
    },
}
