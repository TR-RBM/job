use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::resource_policy::{Limit, message};

pub type Maxima = BTreeMap<String, Rates>;
pub type Weights = BTreeMap<String, u64>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Rates {
    pub rbps: Limit,
    pub wbps: Limit,
    pub riops: Limit,
    pub wiops: Limit,
}

impl Default for Rates {
    fn default() -> Self {
        Self {
            rbps: Limit::Unlimited,
            wbps: Limit::Unlimited,
            riops: Limit::Unlimited,
            wiops: Limit::Unlimited,
        }
    }
}

impl Rates {
    fn values(&self) -> [(&'static str, Limit); 4] {
        [
            ("rbps", self.rbps),
            ("wbps", self.wbps),
            ("riops", self.riops),
            ("wiops", self.wiops),
        ]
    }

    fn validate(&self) -> Result<(), String> {
        for (key, value) in self.values() {
            if let Limit::Value(value) = value
                && (value == 0
                    || (key.ends_with("iops") && value >= u32::MAX as u64)
                    || value == u64::MAX)
            {
                return Err(message(
                    "I/O rates require positive finite values or unlimited",
                ));
            }
        }
        Ok(())
    }

    pub fn kernel(&self) -> String {
        self.values()
            .iter()
            .map(|(key, value)| format!("{key}={}", value.kernel()))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub fn device(value: &str) -> Result<String, String> {
    let error = || message("I/O device requires a canonical MAJOR:MINOR number");
    let (major, minor) = value.split_once(':').ok_or_else(error)?;
    let parse = |text: &str| {
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(error());
        }
        text.parse::<u32>().map_err(|_| error())
    };
    let canonical = format!("{}:{}", parse(major)?, parse(minor)?);
    if canonical != value {
        return Err(error());
    }
    Ok(canonical)
}

pub fn validate(
    maxima: Option<&Maxima>,
    weight: Option<&Weights>,
    bfq: Option<&Weights>,
) -> Result<(), String> {
    if let Some(maxima) = maxima {
        if maxima.is_empty() {
            return Err(message(
                "an I/O policy needs at least one device; use unset to remove it",
            ));
        }
        for (id, rates) in maxima {
            device(id)?;
            rates.validate()?;
        }
    }
    for (weights, max) in [(weight, 10_000), (bfq, 1_000)] {
        if let Some(weights) = weights {
            if weights.is_empty() {
                return Err(message(
                    "an I/O policy needs at least one device; use unset to remove it",
                ));
            }
            for (id, weight) in weights {
                device(id)?;
                if !(1..=max).contains(weight) {
                    return Err(format!(
                        "{}: 1..{max}",
                        message("I/O weight is outside the backend range")
                    ));
                }
            }
        }
    }
    Ok(())
}

pub fn parse_max(value: &str) -> Result<Value, String> {
    let mut parts = value.split(',');
    let id = device(parts.next().unwrap_or_default())?;
    let mut fields = BTreeMap::new();
    for field in parts {
        let (key, value) = field
            .split_once('=')
            .ok_or_else(|| message("io-max requires DEVICE,rbps=RATE,wbps=RATE,riops=N,wiops=N"))?;
        if !["rbps", "wbps", "riops", "wiops"].contains(&key) || fields.contains_key(key) {
            return Err(message("unknown or repeated I/O rate field"));
        }
        let value = if value == "unlimited" {
            serde_json::json!("unlimited")
        } else if key.ends_with("iops") {
            if !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(message("IOPS requires a positive integer or unlimited"));
            }
            serde_json::json!(
                value
                    .parse::<u64>()
                    .map_err(|_| message("IOPS requires a positive integer or unlimited"))?
            )
        } else {
            let bytes = crate::resource_policy::parse_bytes(value).map_err(|_| {
                message("I/O bandwidth requires an exact whole-byte rate or unlimited")
            })?;
            serde_json::json!(bytes)
        };
        fields.insert(key, value);
    }
    if fields.is_empty() {
        return Err(message("io-max needs at least one rate"));
    }
    let rates: Rates =
        serde_json::from_value(serde_json::to_value(fields).unwrap()).map_err(|e| e.to_string())?;
    rates.validate()?;
    Ok(serde_json::json!({id: rates}))
}

pub fn parse_weight(value: &str, bfq: bool) -> Result<Value, String> {
    let (id, weight) = value
        .split_once('=')
        .ok_or_else(|| message("io-weight requires DEVICE=WEIGHT"))?;
    let id = device(id)?;
    if !weight.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(message("io-weight requires DEVICE=WEIGHT"));
    }
    let weight = weight
        .parse::<u64>()
        .map_err(|_| message("io-weight requires DEVICE=WEIGHT"))?;
    let weights = BTreeMap::from([(id, weight)]);
    validate(None, (!bfq).then_some(&weights), bfq.then_some(&weights))?;
    Ok(serde_json::to_value(weights).unwrap())
}

pub fn insert(
    fields: &mut BTreeMap<String, Value>,
    key: String,
    value: Value,
) -> Result<(), String> {
    if matches!(
        key.strip_prefix("job_").unwrap_or(&key),
        "io_max" | "io_weight" | "io_bfq_weight"
    ) && let Some(existing) = fields.get_mut(&key).and_then(Value::as_object_mut)
    {
        for (device, value) in value.as_object().unwrap() {
            if existing.contains_key(device) {
                return Err(message("each device may appear only once per I/O option"));
            }
            existing.insert(device.clone(), value.clone());
        }
        return Ok(());
    }
    fields.insert(key, value);
    Ok(())
}

pub fn maxima_text(maxima: &Maxima) -> String {
    maxima
        .iter()
        .map(|(device, rates)| format!("{device} {}", rates.kernel()))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn weights_text(weights: &Weights) -> String {
    weights
        .iter()
        .map(|(device, weight)| format!("{device} {weight}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn is_file(file: &str) -> bool {
    ["io.max", "io.weight", "io.bfq.weight"].contains(&file)
}

fn rows(text: &str) -> Option<BTreeMap<&str, Vec<&str>>> {
    let mut rows = BTreeMap::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let Some(id) = words.next() else { continue };
        if id != "default" && device(id).is_err() {
            return None;
        }
        if rows.insert(id, words.collect()).is_some() {
            return None;
        }
    }
    Some(rows)
}

pub fn matches(file: &str, expected: &str, actual: &str) -> bool {
    let (Some(wanted), Some(current)) = (rows(expected), rows(actual)) else {
        return false;
    };
    for (id, fields) in &current {
        if !wanted.contains_key(id) {
            if file == "io.max" {
                if fields.iter().any(|field| {
                    !["rbps=max", "wbps=max", "riops=max", "wiops=max"].contains(field)
                }) {
                    return false;
                }
            } else if fields.as_slice() != ["100"] {
                return false;
            }
        }
    }
    if file != "io.max" && current.get("default").map(Vec::as_slice) != Some(["100"].as_slice()) {
        return false;
    }
    for (device, fields) in wanted {
        if file == "io.max" {
            let mut limits = BTreeMap::from([
                ("rbps", "max"),
                ("wbps", "max"),
                ("riops", "max"),
                ("wiops", "max"),
            ]);
            if let Some(words) = current.get(device) {
                let mut seen = std::collections::BTreeSet::new();
                for word in words {
                    let Some((key, value)) = word.split_once('=') else {
                        return false;
                    };
                    if !seen.insert(key) {
                        return false;
                    }
                    limits.insert(key, value);
                }
            }
            for field in fields {
                let Some((key, value)) = field.split_once('=') else {
                    return false;
                };
                if limits.get(key) != Some(&value) {
                    return false;
                }
            }
        } else {
            let found = current.get(device).or_else(|| current.get("default"));
            if found != Some(&fields) {
                return false;
            }
        }
    }
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    pub device: String,
    pub name: String,
    pub whole_disk: bool,
    pub scheduler: Option<String>,
    pub iocost_enabled: bool,
    pub bfq_low_latency: Option<bool>,
    pub io_max: bool,
    pub io_weight: bool,
    pub io_bfq_weight: bool,
    pub reasons: BTreeMap<String, String>,
}

fn cost_enabled(text: &str, device: &str) -> bool {
    text.lines().any(|line| {
        let mut fields = line.split_whitespace();
        fields.next() == Some(device) && fields.any(|field| field == "enable=1")
    })
}

fn inspect(sysfs: &Path, qos: &str, id: &str, supported: [bool; 3]) -> Result<Capability, String> {
    device(id)?;
    let path = sysfs
        .join(id)
        .canonicalize()
        .map_err(|_| format!("{}: {id}", message("I/O device is unavailable")))?;
    if fs::read_to_string(path.join("dev"))
        .ok()
        .as_deref()
        .map(str::trim)
        != Some(id)
    {
        return Err(message(
            "I/O device identity differs from the requested device",
        ));
    }
    let whole_disk = !path.join("partition").exists();
    let scheduler = fs::read_to_string(path.join("queue/scheduler"))
        .ok()
        .and_then(|text| {
            text.split_whitespace().find_map(|word| {
                word.strip_prefix('[')
                    .and_then(|word| word.strip_suffix(']'))
                    .map(str::to_owned)
            })
        });
    let iocost_enabled = cost_enabled(qos, id);
    let bfq_low_latency = fs::read_to_string(path.join("queue/iosched/low_latency"))
        .ok()
        .and_then(|value| match value.trim() {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        });
    let active = [
        true,
        iocost_enabled,
        scheduler.as_deref() == Some("bfq") && bfq_low_latency == Some(false),
    ];
    let mut reasons = BTreeMap::new();
    let files = ["io.max", "io.weight", "io.bfq.weight"];
    let unsupported = [
        "I/O maximum controller is unavailable",
        "IOCost is not enabled for this device",
        "BFQ requires the active scheduler and low_latency=0",
    ];
    for index in 0..files.len() {
        if !whole_disk || !supported[index] || !active[index] {
            let reason = if !whole_disk {
                "I/O controls require a whole block device, not a partition"
            } else if !supported[index] {
                "requested resource control is unavailable"
            } else {
                unsupported[index]
            };
            reasons.insert(files[index].to_owned(), message(reason));
        }
    }
    Ok(Capability {
        device: id.to_owned(),
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        whole_disk,
        scheduler,
        iocost_enabled,
        bfq_low_latency,
        io_max: !reasons.contains_key("io.max"),
        io_weight: !reasons.contains_key("io.weight"),
        io_bfq_weight: !reasons.contains_key("io.bfq.weight"),
        reasons,
    })
}

pub fn capabilities(tree: Option<&crate::cgroup::Tree>) -> Vec<Capability> {
    let supported = ["io.max", "io.weight", "io.bfq.weight"]
        .map(|file| tree.is_some_and(|tree| tree.supports(file)));
    let qos = fs::read_to_string("/sys/fs/cgroup/io.cost.qos").unwrap_or_default();
    let Ok(entries) = fs::read_dir("/sys/dev/block") else {
        return Vec::new();
    };
    let mut values: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            inspect(
                Path::new("/sys/dev/block"),
                &qos,
                &entry.file_name().to_string_lossy(),
                supported,
            )
            .ok()
        })
        .collect();
    values.sort_by(|a, b| a.device.cmp(&b.device));
    values
}

pub fn available(file: &str, expected: &str) -> Result<(), String> {
    if !is_file(file) {
        return Ok(());
    }
    let qos = fs::read_to_string("/sys/fs/cgroup/io.cost.qos").unwrap_or_default();
    for id in rows(expected)
        .ok_or_else(|| message("invalid recorded I/O policy"))?
        .keys()
    {
        let capability = inspect(Path::new("/sys/dev/block"), &qos, id, [true; 3])?;
        if let Some(reason) = capability.reasons.get(file) {
            return Err(format!("{id}: {reason}"));
        }
    }
    Ok(())
}

pub fn describe(capability: &Capability) -> String {
    let controls = ["io.max", "io.weight", "io.bfq.weight"]
        .iter()
        .map(|file| {
            let state = capability
                .reasons
                .get(*file)
                .cloned()
                .unwrap_or_else(|| message("available"));
            format!("{file}: {state}")
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "{} {} ({}): {controls}",
        message("I/O device"),
        capability.device,
        capability.name
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_fields_use_exact_bytes_and_keep_unmentioned_directions_unlimited() {
        let policy: Maxima =
            serde_json::from_value(parse_max("8:0,wbps=0.5M,riops=100").unwrap()).unwrap();
        assert_eq!(
            maxima_text(&policy),
            "8:0 rbps=max wbps=524288 riops=100 wiops=max"
        );
        for value in [
            "8:0",
            "08:0,wbps=1M",
            "8:0,rbps=0",
            "8:0,riops=0.5",
            "8:0,wiops=4294967295",
            "8:0,wbps=18446744073709551615",
            "8:0,rate=1",
            "8:0,wbps=1,wbps=2",
            "8:0,wbps=0.5B",
        ] {
            assert!(parse_max(value).is_err(), "{value}");
        }
    }

    #[test]
    fn backend_weight_ranges_are_explicit_and_not_rescaled() {
        assert!(parse_weight("8:0=10000", false).is_ok());
        assert!(parse_weight("8:0=10000", true).is_err());
        assert!(parse_weight("8:0=1000", true).is_ok());
        assert!(parse_weight("8:0=0", false).is_err());
        assert!(parse_weight("8:0=10001", false).is_err());
        assert!(parse_weight("8:0=+1", false).is_err());
    }

    #[test]
    fn repeated_options_collect_distinct_devices_and_reject_duplicates() {
        let mut fields = BTreeMap::new();
        insert(
            &mut fields,
            "job_io_max".into(),
            parse_max("8:0,wbps=1M").unwrap(),
        )
        .unwrap();
        insert(
            &mut fields,
            "job_io_max".into(),
            parse_max("8:16,rbps=2M").unwrap(),
        )
        .unwrap();
        assert_eq!(fields["job_io_max"].as_object().unwrap().len(), 2);
        assert!(
            insert(
                &mut fields,
                "job_io_max".into(),
                parse_max("8:0,wbps=2M").unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn kernel_comparison_ignores_order_and_omitted_unlimited_rows() {
        let expected = "8:0 rbps=max wbps=524288 riops=max wiops=100\n8:16 rbps=max wbps=max riops=max wiops=max";
        assert!(matches(
            "io.max",
            expected,
            "8:0 wiops=100 riops=max rbps=max wbps=524288\n"
        ));
        assert!(!matches(
            "io.max",
            expected,
            "8:0 rbps=max wbps=524289 riops=max wiops=100"
        ));
        assert!(!matches(
            "io.max",
            expected,
            "8:0 rbps=max wbps=524288 riops=max wiops=100\n8:32 wbps=1"
        ));
        assert!(matches(
            "io.max",
            "",
            "8:0 rbps=max wbps=max riops=max wiops=max"
        ));
        assert!(!matches(
            "io.max",
            "",
            "8:0 rbps=1 wbps=max riops=max wiops=max"
        ));
    }

    #[test]
    fn weight_readback_checks_device_overrides_and_default_drift() {
        for file in ["io.weight", "io.bfq.weight"] {
            assert!(matches(file, "8:0 100\n8:16 23", "default 100\n8:16 23"));
            assert!(!matches(file, "8:0 100", "default 99"));
            assert!(!matches(file, "8:0 100", "default 100\n8:32 20"));
            assert!(!matches(file, "8:0 100", "default 100\n8:0 100\n8:0 23"));
        }
    }

    #[test]
    fn backend_capabilities_require_activation_and_whole_devices() {
        let root = std::env::temp_dir().join(format!("job-io-capability-{}", std::process::id()));
        let disk = root.join("8:0");
        fs::create_dir_all(disk.join("queue/iosched")).unwrap();
        fs::write(disk.join("dev"), "8:0\n").unwrap();
        fs::write(disk.join("queue/scheduler"), "[none] bfq").unwrap();
        let capability = inspect(&root, "", "8:0", [true; 3]).unwrap();
        assert!(capability.io_max);
        assert!(!capability.io_weight && !capability.io_bfq_weight);
        assert!(
            inspect(&root, "8:0 enable=1 ctrl=auto", "8:0", [true; 3])
                .unwrap()
                .io_weight
        );
        assert!(
            !inspect(&root, "8:0 enable=1", "8:0", [false; 3])
                .unwrap()
                .io_weight
        );
        fs::write(disk.join("queue/scheduler"), "none [bfq]").unwrap();
        fs::write(disk.join("queue/iosched/low_latency"), "1").unwrap();
        assert!(!inspect(&root, "", "8:0", [true; 3]).unwrap().io_bfq_weight);
        fs::write(disk.join("queue/iosched/low_latency"), "0").unwrap();
        assert!(inspect(&root, "", "8:0", [true; 3]).unwrap().io_bfq_weight);
        fs::write(disk.join("partition"), "1").unwrap();
        assert!(
            !inspect(&root, "8:0 enable=1", "8:0", [true; 3])
                .unwrap()
                .io_max
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn io_default_maps_are_resolved_as_complete_fields_with_recorded_origins() {
        let mut graph = crate::objects::Graph::fresh();
        let group = graph
            .create(
                "a",
                crate::objects::Kind::Group,
                BTreeMap::from([
                    ("job_io_max".into(), parse_max("8:0,wbps=1M").unwrap()),
                    (
                        "job_io_weight".into(),
                        parse_weight("8:0=21", false).unwrap(),
                    ),
                ]),
            )
            .unwrap();
        let queue = graph
            .create(
                "a/q",
                crate::objects::Kind::Queue,
                BTreeMap::from([("job_io_max".into(), parse_max("8:16,rbps=2M").unwrap())]),
            )
            .unwrap();
        let mut declared = crate::model::Declared::default();
        let sources =
            crate::resource_policy::resolve(&mut declared, &graph.view(queue).effective).unwrap();
        assert!(
            declared
                .resources
                .io_max
                .as_ref()
                .unwrap()
                .contains_key("8:16")
        );
        assert!(
            !declared
                .resources
                .io_max
                .as_ref()
                .unwrap()
                .contains_key("8:0")
        );
        assert_eq!(
            sources["io_max"],
            crate::resource_policy::Origin::Object {
                id: queue,
                path: "a/q".into()
            }
        );
        assert_eq!(
            sources["io_weight"],
            crate::resource_policy::Origin::Object {
                id: group,
                path: "a".into()
            }
        );
        assert!(declared.resources.cpu_request_milli.is_none());
        assert!(declared.resources.memory_request.is_none());
    }
}
