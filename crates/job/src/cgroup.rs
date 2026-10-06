use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::resources::{MILLI, Vector};

pub const WEIGHT_PER_CORE: u64 = 100;
pub const WANTED_CONTROLLERS: [&str; 4] = ["cpu", "memory", "pids", "io"];
const MAX_WEIGHT: u64 = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tree {
    pub root: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub memory_current: u64,
    pub memory_peak: u64,
    pub oom_group_kill: u64,
    pub oom_kill: u64,
    pub own_oom: Option<u64>,
    pub own_oom_kill: Option<u64>,
    pub pids_current: u64,
    pub pids_peak: u64,
    pub pids_max_events: u64,
    pub cpu_usage_us: u64,
    pub cpu_throttled_us: u64,
    pub written_back: u64,
    pub dirty: u64,
}

impl Counters {
    pub fn written(&self) -> u64 {
        self.written_back + self.dirty
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    pub managed: Vec<String>,
    pub oom_group: bool,
    pub memory_max: String,
    pub swap_max: String,
    pub pids_max: String,
    pub cpu_weight: String,
    pub cpu_max: String,
    pub memory_high: String,
    pub io_max: String,
    pub io_weight: String,
    pub io_bfq_weight: String,
}

impl Limits {
    pub fn for_reservation(need: Vector, memory_limit: Option<u64>) -> Limits {
        Limits {
            cpu_max: format!("max {}", crate::resource_policy::CPU_PERIOD_US),
            memory_high: "max".to_owned(),
            io_max: String::new(),
            io_weight: String::new(),
            io_bfq_weight: String::new(),
            managed: ["memory.max", "memory.swap.max", "pids.max", "cpu.weight"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            oom_group: true,
            memory_max: memory_limit.map_or_else(
                || "max".to_string(),
                |bytes| (bytes / page_size() * page_size()).to_string(),
            ),
            swap_max: "0".to_string(),
            pids_max: need.pids.to_string(),
            cpu_weight: (need.cores_milli * WEIGHT_PER_CORE / MILLI)
                .clamp(1, MAX_WEIGHT)
                .to_string(),
        }
    }

    pub fn for_job(job: &crate::model::Job) -> Self {
        let mut limits = if job.policy.legacy() {
            Self::for_reservation(
                job.reservation.vector,
                job.spec
                    .declared
                    .memory
                    .filter(|_| job.spec.declared.on.is_none()),
            )
        } else {
            Self::ordinary(&job.spec.declared)
        };
        if job.spec.declared.on.is_some() {
            return limits;
        }
        let resources = &job.spec.declared.resources;
        if resources.cpu_request_milli.is_some() && resources.cpu_weight.is_none() {
            limits.managed.retain(|file| file != "cpu.weight");
            limits.cpu_weight = "100".to_owned();
        }
        limits.apply_controls(resources);
        limits
    }

    pub fn for_controls(resources: &crate::resource_policy::Controls) -> Self {
        let mut limits = Self::ordinary(&crate::model::Declared::default());
        limits.apply_controls(resources);
        limits
    }

    fn apply_controls(&mut self, resources: &crate::resource_policy::Controls) {
        for (file, value) in [
            (
                "io.max",
                resources.io_max.as_ref().map(crate::io_policy::maxima_text),
            ),
            (
                "io.weight",
                resources
                    .io_weight
                    .as_ref()
                    .map(crate::io_policy::weights_text),
            ),
            (
                "io.bfq.weight",
                resources
                    .io_bfq_weight
                    .as_ref()
                    .map(crate::io_policy::weights_text),
            ),
            (
                "cpu.max",
                resources.cpu_limit_milli.map(|limit| limit.cpu()),
            ),
            (
                "cpu.weight",
                resources.cpu_weight.map(|weight| weight.to_string()),
            ),
            (
                "memory.high",
                resources.memory_high.map(|limit| limit.memory()),
            ),
            (
                "memory.max",
                resources.memory_max.map(|limit| limit.memory()),
            ),
            (
                "memory.swap.max",
                resources.memory_swap_max.map(|limit| limit.memory()),
            ),
            ("pids.max", resources.pids_max.map(|limit| limit.kernel())),
        ] {
            if let Some(value) = value {
                if !self.managed.iter().any(|name| name == file) {
                    self.managed.push(file.to_owned());
                }
                match file {
                    "io.max" => self.io_max = value,
                    "io.weight" => self.io_weight = value,
                    "io.bfq.weight" => self.io_bfq_weight = value,
                    "cpu.max" => self.cpu_max = value,
                    "cpu.weight" => self.cpu_weight = value,
                    "memory.high" => self.memory_high = value,
                    "memory.max" => {
                        self.oom_group = value != "max";
                        self.memory_max = value;
                    }
                    "memory.swap.max" => self.swap_max = value,
                    "pids.max" => self.pids_max = value,
                    _ => unreachable!(),
                }
            }
        }
    }

    fn ordinary(declared: &crate::model::Declared) -> Self {
        let local = declared.on.is_none();
        let mut limits = Self {
            cpu_max: format!("max {}", crate::resource_policy::CPU_PERIOD_US),
            memory_high: "max".to_owned(),
            io_max: String::new(),
            io_weight: String::new(),
            io_bfq_weight: String::new(),
            managed: Vec::new(),
            oom_group: local && declared.memory.is_some(),
            memory_max: "max".to_owned(),
            swap_max: "max".to_owned(),
            pids_max: "max".to_owned(),
            cpu_weight: "100".to_owned(),
        };
        if !local {
            return limits;
        }
        if let Some(memory) = declared.memory {
            limits.managed.push("memory.max".to_owned());
            limits.memory_max = (memory / page_size() * page_size()).to_string();
        }
        if let Some(pids) = declared.pids {
            limits.managed.push("pids.max".to_owned());
            limits.pids_max = pids.to_string();
        }
        if let Some(cores) = declared.cores_milli {
            limits.managed.push("cpu.weight".to_owned());
            limits.cpu_weight = (cores.saturating_mul(WEIGHT_PER_CORE) / MILLI)
                .clamp(1, MAX_WEIGHT)
                .to_string();
        }
        limits
    }

    pub fn with_recorded(
        mut self,
        values: &std::collections::BTreeMap<String, String>,
    ) -> io::Result<Self> {
        if values.is_empty() {
            return Ok(self);
        }
        self.managed.clear();
        for (file, value) in values {
            let target = match file.as_str() {
                "cpu.max" => &mut self.cpu_max,
                "cpu.weight" => &mut self.cpu_weight,
                "memory.high" => &mut self.memory_high,
                "memory.max" => &mut self.memory_max,
                "memory.swap.max" => &mut self.swap_max,
                "pids.max" => &mut self.pids_max,
                "io.max" => &mut self.io_max,
                "io.weight" => &mut self.io_weight,
                "io.bfq.weight" => &mut self.io_bfq_weight,
                _ => {
                    return Err(io::Error::other(crate::resource_policy::message(
                        "unknown recorded kernel resource control",
                    )));
                }
            };
            *target = value.clone();
            self.managed.push(file.clone());
        }
        Ok(self)
    }

    pub fn files(&self) -> Vec<(&'static str, &str)> {
        [
            ("io.max", self.io_max.as_str()),
            ("io.weight", self.io_weight.as_str()),
            ("io.bfq.weight", self.io_bfq_weight.as_str()),
            ("cpu.max", self.cpu_max.as_str()),
            ("memory.high", self.memory_high.as_str()),
            ("memory.max", self.memory_max.as_str()),
            ("memory.swap.max", self.swap_max.as_str()),
            ("pids.max", self.pids_max.as_str()),
            ("cpu.weight", self.cpu_weight.as_str()),
        ]
        .into_iter()
        .filter(|(file, _)| self.managed.iter().any(|name| name == file))
        .collect()
    }
}

pub fn is_daemon_cgroup(own: &Path, root: &Path) -> bool {
    own == root || own == root.join("daemon")
}

pub fn page_size() -> u64 {
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if size > 0 { size as u64 } else { 4096 }
}

pub fn keyed_value(text: &str, key: &str) -> u64 {
    text.lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            (fields.next() == Some(key))
                .then(|| fields.next())
                .flatten()
        })
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

pub fn io_written_bytes(io_stat: &str) -> u64 {
    io_stat
        .lines()
        .flat_map(|line| line.split_whitespace())
        .filter_map(|field| field.strip_prefix("wbytes="))
        .filter_map(|v| v.parse::<u64>().ok())
        .sum()
}

fn read(path: &Path, file: &str) -> String {
    fs::read_to_string(path.join(file)).unwrap_or_default()
}

fn read_number(path: &Path, file: &str) -> u64 {
    read(path, file).trim().parse().unwrap_or(0)
}

impl Tree {
    pub fn own() -> Option<PathBuf> {
        let own = fs::read_to_string("/proc/self/cgroup").ok()?;
        let own = own.lines().find_map(|l| l.strip_prefix("0::"))?;
        Some(Path::new("/sys/fs/cgroup").join(own.trim().trim_start_matches('/')))
    }

    pub fn detect(root: &Path) -> Option<Tree> {
        let inside = is_daemon_cgroup(&Tree::own()?, root);
        let writable = unsafe {
            let path =
                std::ffi::CString::new(root.join("cgroup.procs").as_os_str().as_encoded_bytes())
                    .ok()?;
            libc::access(path.as_ptr(), libc::W_OK) == 0
        };
        (inside && writable).then(|| Tree {
            root: root.to_path_buf(),
        })
    }

    pub fn leave_root(&self) -> io::Result<()> {
        if Tree::own().as_deref() != Some(self.root.as_path()) {
            return Ok(());
        }
        let daemon = self.root.join("daemon");
        if !daemon.exists() {
            fs::create_dir(&daemon)?;
        }
        fs::write(daemon.join("cgroup.procs"), "0")
    }

    pub fn jobs(&self) -> PathBuf {
        self.root.join("jobs")
    }

    pub fn shims(&self) -> PathBuf {
        self.root.join("shims")
    }

    pub fn supports(&self, file: &str) -> bool {
        let controller = file.split('.').next().unwrap_or_default();
        read(&self.jobs(), "cgroup.subtree_control")
            .split_whitespace()
            .any(|name| name == controller)
            && std::ffi::CString::new(self.jobs().join(file).as_os_str().as_encoded_bytes())
                .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 })
    }

    pub fn prepare(&self) -> io::Result<()> {
        let available = read(&self.root, "cgroup.controllers");
        let enable: Vec<String> = WANTED_CONTROLLERS
            .iter()
            .filter(|c| available.split_whitespace().any(|a| a == **c))
            .map(|c| format!("+{c}"))
            .collect();
        let enable = enable.join(" ");
        fs::write(self.root.join("cgroup.subtree_control"), &enable)?;
        for dir in [self.jobs(), self.shims()] {
            if !dir.exists() {
                fs::create_dir(&dir)?;
            }
        }
        fs::write(self.jobs().join("cgroup.subtree_control"), &enable)
    }

    pub fn delegated_own() -> Option<Tree> {
        let own = Tree::own()?;
        let root = if own.file_name().is_some_and(|n| n == "daemon") {
            own.parent()?.to_path_buf()
        } else {
            own
        };
        let writable = |file: &str| {
            std::ffi::CString::new(root.join(file).as_os_str().as_encoded_bytes())
                .is_ok_and(|p| unsafe { libc::access(p.as_ptr(), libc::W_OK) } == 0)
        };
        (root != Path::new("/sys/fs/cgroup")
            && writable("cgroup.procs")
            && writable("cgroup.subtree_control"))
        .then_some(Tree { root })
    }

    pub fn ordinary_pool(&self, cores_milli: u64) -> Vector {
        let limit = |file: &str| read(&self.root, file).trim().parse::<u64>().ok();
        Vector {
            cores_milli,
            memory: limit("memory.max").unwrap_or_else(|| crate::host::read_meminfo().total),
            pids: limit("pids.max").unwrap_or_else(crate::host::pid_max),
        }
    }

    pub fn pool(&self, cores_milli: u64) -> Vector {
        let limit = |file: &str| read(&self.root, file).trim().parse::<u64>().ok();
        Vector {
            cores_milli,
            memory: limit("memory.max")
                .unwrap_or_else(|| crate::host::read_meminfo().total / 4 * 3),
            pids: limit("pids.max").unwrap_or_else(|| crate::host::pid_max() / 2),
        }
    }

    pub fn create_at(path: &Path, limits: &Limits) -> io::Result<PathBuf> {
        if !path.exists() {
            fs::create_dir(path)?;
        }
        if path.join("memory.oom.group").exists() {
            fs::write(
                path.join("memory.oom.group"),
                if limits.oom_group { "1" } else { "0" },
            )?;
        }
        for (file, value) in limits.files() {
            write_control(path, file, value)?;
        }
        Ok(path.to_path_buf())
    }

    pub fn changed_limit(path: &Path, limits: &Limits) -> Option<(String, String, String)> {
        limits.files().into_iter().find_map(|(file, expected)| {
            changed_control(path, file, expected)
                .map(|actual| (file.to_string(), expected.to_string(), actual))
        })
    }

    pub fn counters(path: &Path) -> Counters {
        let memory_events = read(path, "memory.events");
        let own_events = fs::read_to_string(path.join("memory.events.local")).ok();
        let pids_events = read(path, "pids.events");
        let cpu_stat = read(path, "cpu.stat");
        let memory_stat = read(path, "memory.stat");
        Counters {
            memory_current: read_number(path, "memory.current"),
            memory_peak: read_number(path, "memory.peak"),
            oom_group_kill: keyed_value(&memory_events, "oom_group_kill"),
            oom_kill: keyed_value(&memory_events, "oom_kill"),
            own_oom: own_events.as_deref().map(|text| keyed_value(text, "oom")),
            own_oom_kill: own_events
                .as_deref()
                .map(|text| keyed_value(text, "oom_kill")),
            pids_current: read_number(path, "pids.current"),
            pids_peak: read_number(path, "pids.peak"),
            pids_max_events: keyed_value(&pids_events, "max"),
            cpu_usage_us: keyed_value(&cpu_stat, "usage_usec"),
            cpu_throttled_us: keyed_value(&cpu_stat, "throttled_usec"),
            written_back: io_written_bytes(&read(path, "io.stat")),
            dirty: keyed_value(&memory_stat, "file_dirty"),
        }
    }

    pub fn freeze(path: &Path) -> io::Result<()> {
        fs::write(path.join("cgroup.freeze"), "1")
    }

    pub fn kill(path: &Path) -> io::Result<()> {
        fs::write(path.join("cgroup.kill"), "1")
    }

    pub fn is_populated(path: &Path) -> bool {
        keyed_value(&read(path, "cgroup.events"), "populated") == 1
    }

    pub fn processes(path: &Path) -> Vec<i32> {
        read(path, "cgroup.procs")
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect()
    }

    pub fn remove(path: &Path) -> io::Result<()> {
        fs::remove_dir(path)
    }
}

pub fn control_matches(file: &str, expected: &str, actual: &str) -> bool {
    if crate::io_policy::is_file(file) {
        crate::io_policy::matches(file, expected, actual)
    } else {
        actual.trim() == expected
    }
}

pub fn changed_control(path: &Path, file: &str, expected: &str) -> Option<String> {
    if let Err(reason) = crate::io_policy::available(file, expected) {
        return Some(reason);
    }
    match fs::read_to_string(path.join(file)) {
        Ok(actual) if control_matches(file, expected, &actual) => None,
        Ok(actual) => Some(actual.trim().to_owned()),
        Err(error) => Some(error.to_string()),
    }
}

pub fn write_control(path: &Path, file: &str, value: &str) -> io::Result<()> {
    crate::io_policy::available(file, value).map_err(io::Error::other)?;
    let values: Vec<&str> = if crate::io_policy::is_file(file) {
        value.lines().collect()
    } else {
        vec![value]
    };
    for value in values {
        let mut target = fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path.join(file))?;
        target.write_all(value.as_bytes())?;
    }
    if let Some(actual) = changed_control(path, file, value) {
        return Err(io::Error::other(format!(
            "{}: {file}: {actual}",
            crate::resource_policy::message(
                "kernel resource value differs from the requested value"
            )
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_tracking_has_no_managed_resource_controls() {
        let limits = Limits::ordinary(&crate::model::Declared::default());
        assert!(limits.files().is_empty());
        assert!(!limits.oom_group);
        assert_eq!(limits.swap_max, "max");
        assert_eq!(limits.pids_max, "max");
    }

    #[test]
    fn declaring_memory_does_not_implicitly_disable_swap() {
        let limits = Limits::ordinary(&crate::model::Declared {
            memory: Some(64 << 20),
            ..Default::default()
        });
        assert_eq!(limits.files(), vec![("memory.max", "67108864")]);
    }

    #[test]
    fn only_the_root_or_its_daemon_cgroup_is_the_services_own() {
        let root = Path::new("/sys/fs/cgroup/exec");
        assert!(is_daemon_cgroup(
            Path::new("/sys/fs/cgroup/exec/daemon"),
            root
        ));
        assert!(is_daemon_cgroup(root, root));
        assert!(!is_daemon_cgroup(
            Path::new("/sys/fs/cgroup/exec/jobs/12"),
            root
        ));
        assert!(!is_daemon_cgroup(
            Path::new("/sys/fs/cgroup/exec/shims"),
            root
        ));
        assert!(!is_daemon_cgroup(Path::new("/sys/fs/cgroup/11"), root));
    }

    #[test]
    fn keyed_files_give_the_named_counter() {
        let events = "low 0\nhigh 0\nmax 3\noom 1\noom_kill 1\noom_group_kill 1\n";
        assert_eq!(keyed_value(events, "oom_group_kill"), 1);
        assert_eq!(keyed_value(events, "max"), 3);
        assert_eq!(keyed_value(events, "absent"), 0);
    }

    #[test]
    fn written_bytes_sum_over_devices() {
        let io_stat = "254:0 rbytes=10 wbytes=4096 rios=1 wios=1 dbytes=0 dios=0\n7:0 rbytes=0 wbytes=100 rios=0 wios=1 dbytes=0 dios=0\n";
        assert_eq!(io_written_bytes(io_stat), 4196);
    }

    #[test]
    fn a_memory_limit_is_written_in_whole_pages() {
        let limits = Limits::for_reservation(
            Vector {
                cores_milli: 1000,
                memory: page_size() * 3 + 1,
                pids: 1,
            },
            Some(page_size() * 3 + 1),
        );
        assert_eq!(limits.memory_max, (page_size() * 3).to_string());
    }

    #[test]
    fn an_estimated_reservation_leaves_memory_to_the_pool() {
        let limits = Limits::for_reservation(
            Vector {
                cores_milli: 1000,
                memory: 1 << 30,
                pids: 20,
            },
            None,
        );
        assert_eq!(limits.memory_max, "max");
    }

    #[test]
    fn limits_follow_the_reservation() {
        let limits = Limits::for_reservation(
            Vector {
                cores_milli: 1500,
                memory: 100 << 20,
                pids: 20,
            },
            Some(100 << 20),
        );
        assert_eq!(limits.memory_max, (100u64 << 20).to_string());
        assert_eq!(limits.swap_max, "0");
        assert_eq!(limits.pids_max, "20");
        assert_eq!(limits.cpu_weight, "150");
    }

    #[test]
    fn a_limit_changed_from_inside_the_job_is_named() {
        let dir = std::env::temp_dir().join(format!("job-cgroup-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let limits = Limits::for_reservation(
            Vector {
                cores_milli: 1000,
                memory: 1 << 20,
                pids: 20,
            },
            Some(1 << 20),
        );
        for (file, value) in limits.files() {
            fs::write(dir.join(file), value).unwrap();
        }
        assert_eq!(Tree::changed_limit(&dir, &limits), None);
        fs::write(dir.join("memory.oom.group"), "1").unwrap();
        fs::write(dir.join("memory.max"), "max").unwrap();
        assert_eq!(
            Tree::changed_limit(&dir, &limits),
            Some((
                "memory.max".to_string(),
                (1u64 << 20).to_string(),
                "max".to_string()
            ))
        );
        for (file, _) in limits.files() {
            fs::remove_file(dir.join(file)).unwrap();
        }
        fs::remove_file(dir.join("memory.oom.group")).unwrap();
        fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn a_missing_managed_cpu_controller_is_reported_as_a_changed_limit() {
        let dir = std::env::temp_dir().join(format!("job-missing-control-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let mut limits = Limits::for_reservation(Vector::default(), None);
        limits.managed = vec!["cpu.max".to_owned()];
        limits.cpu_max = "10000 100000".to_owned();
        let (file, expected, actual) = Tree::changed_limit(&dir, &limits).unwrap();
        assert_eq!(file, "cpu.max");
        assert_eq!(expected, "10000 100000");
        assert!(!actual.is_empty());
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn an_external_swap_restriction_is_not_silently_accepted() {
        let dir = std::env::temp_dir().join(format!("job-swap-control-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let mut limits = Limits::for_reservation(Vector::default(), None);
        limits.managed = vec!["memory.swap.max".to_owned()];
        limits.swap_max = "max".to_owned();
        fs::write(dir.join("memory.swap.max"), "0").unwrap();
        assert_eq!(
            Tree::changed_limit(&dir, &limits),
            Some((
                "memory.swap.max".to_owned(),
                "max".to_owned(),
                "0".to_owned()
            ))
        );
        fs::remove_file(dir.join("memory.swap.max")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn recorded_launch_values_survive_changed_resource_derivation() {
        let current = Limits::for_reservation(
            Vector {
                cores_milli: 2000,
                memory: 1 << 20,
                pids: 20,
            },
            None,
        );
        let recorded = std::collections::BTreeMap::from([
            ("cpu.weight".to_owned(), "50".to_owned()),
            ("memory.max".to_owned(), "1048576".to_owned()),
        ]);
        let recovered = current.with_recorded(&recorded).unwrap();
        assert_eq!(recovered.cpu_weight, "50");
        assert_eq!(recovered.memory_max, "1048576");
        assert_eq!(recovered.files().len(), 2);
        assert!(!recovered.managed.iter().any(|file| file == "pids.max"));
        assert!(
            recovered
                .with_recorded(&std::collections::BTreeMap::from([(
                    "unexpected.file".to_owned(),
                    "1".to_owned()
                )]))
                .is_err()
        );
    }
}
