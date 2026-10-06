use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::Declared;
use crate::objects::Effective;

mod messages;
pub use messages::message;

pub const CPU_PERIOD_US: u64 = 100_000;
pub const MIN_CPU_MILLI: u64 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "WireLimit", into = "WireLimit")]
pub enum Limit {
    Value(u64),
    Unlimited,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum WireLimit {
    Value(u64),
    Text(String),
}

impl TryFrom<WireLimit> for Limit {
    type Error = String;
    fn try_from(value: WireLimit) -> Result<Self, Self::Error> {
        match value {
            WireLimit::Value(value) => Ok(Self::Value(value)),
            WireLimit::Text(value) if value == "unlimited" => Ok(Self::Unlimited),
            _ => Err(message("a limit requires a nonnegative value or unlimited")),
        }
    }
}

impl From<Limit> for WireLimit {
    fn from(value: Limit) -> Self {
        match value {
            Limit::Value(value) => Self::Value(value),
            Limit::Unlimited => Self::Text("unlimited".to_owned()),
        }
    }
}

impl Limit {
    pub fn kernel(self) -> String {
        match self {
            Self::Value(value) => value.to_string(),
            Self::Unlimited => "max".to_owned(),
        }
    }

    pub fn memory(self) -> String {
        match self {
            Self::Value(value) => (value.div_ceil(crate::cgroup::page_size())
                * crate::cgroup::page_size())
            .to_string(),
            Self::Unlimited => self.kernel(),
        }
    }

    pub fn cpu(self) -> String {
        match self {
            Self::Value(milli) => format!(
                "{} {CPU_PERIOD_US}",
                milli * (CPU_PERIOD_US / crate::resources::MILLI)
            ),
            Self::Unlimited => format!("max {CPU_PERIOD_US}"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Controls {
    pub io_max: Option<crate::io_policy::Maxima>,
    pub io_weight: Option<crate::io_policy::Weights>,
    pub io_bfq_weight: Option<crate::io_policy::Weights>,
    pub cpu_request_milli: Option<u64>,
    pub cpu_limit_milli: Option<Limit>,
    pub cpu_weight: Option<u64>,
    pub memory_request: Option<u64>,
    pub memory_high: Option<Limit>,
    pub memory_max: Option<Limit>,
    pub memory_swap_max: Option<Limit>,
    pub pids_max: Option<Limit>,
}

impl Controls {
    pub fn validate(&self) -> Result<(), String> {
        crate::io_policy::validate(
            self.io_max.as_ref(),
            self.io_weight.as_ref(),
            self.io_bfq_weight.as_ref(),
        )?;
        if self
            .cpu_weight
            .is_some_and(|weight| !(1..=10_000).contains(&weight))
        {
            return Err(message("cpu-weight must be between 1 and 10000"));
        }
        if let Some(Limit::Value(milli)) = self.cpu_limit_milli
            && (milli < MIN_CPU_MILLI
                || milli
                    .checked_mul(CPU_PERIOD_US / crate::resources::MILLI)
                    .is_none())
        {
            return Err(message(
                "cpu-limit must be at least 0.01 CPU and fit the quota range",
            ));
        }
        for limit in [self.memory_high, self.memory_max, self.memory_swap_max] {
            if let Some(Limit::Value(bytes)) = limit
                && bytes.checked_add(crate::cgroup::page_size() - 1).is_none()
            {
                return Err(message("memory limit cannot be rounded to a page safely"));
            }
        }
        Ok(())
    }

    pub fn fields(&self) -> BTreeMap<String, serde_json::Value> {
        serde_json::from_value(serde_json::to_value(self).unwrap()).unwrap()
    }

    pub fn from_settings(settings: &crate::model::Settings) -> Self {
        Self {
            io_max: settings.job_io_max.clone(),
            io_weight: settings.job_io_weight.clone(),
            io_bfq_weight: settings.job_io_bfq_weight.clone(),
            cpu_request_milli: settings.job_cpu_request_milli,
            cpu_limit_milli: settings.job_cpu_limit_milli,
            cpu_weight: settings.job_cpu_weight,
            memory_request: settings.job_memory_request,
            memory_high: settings.job_memory_high,
            memory_max: settings.job_memory_max,
            memory_swap_max: settings.job_memory_swap_max,
            pids_max: settings.job_pids_max,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    Preset {
        definition: String,
        sha256: String,
        selected_by: crate::presets::Source,
    },
    Job,
    Compatibility,
    Service,
    Object {
        id: u64,
        path: String,
    },
}

pub fn kernel_file(name: &str) -> Option<&'static str> {
    match name {
        "io_max" => Some("io.max"),
        "io_weight" => Some("io.weight"),
        "io_bfq_weight" => Some("io.bfq.weight"),
        "cpu_limit_milli" => Some("cpu.max"),
        "cpu_weight" => Some("cpu.weight"),
        "memory_high" => Some("memory.high"),
        "memory_max" => Some("memory.max"),
        "memory_swap_max" => Some("memory.swap.max"),
        "pids_max" => Some("pids.max"),
        _ => None,
    }
}

pub fn resolve(
    declared: &mut Declared,
    effective: &BTreeMap<String, Effective>,
) -> Result<BTreeMap<String, Origin>, String> {
    declared.resources.validate()?;
    let mut fields = declared.resources.fields();
    let mut aliases = BTreeMap::new();
    if let Some(cores) = declared.cores_milli {
        aliases.insert("cpu_request_milli", serde_json::json!(cores));
        aliases.insert(
            "cpu_weight",
            serde_json::json!(
                (cores.saturating_mul(crate::cgroup::WEIGHT_PER_CORE) / crate::resources::MILLI)
                    .clamp(1, 10_000)
            ),
        );
    }
    if let Some(memory) = declared.memory {
        aliases.insert("memory_request", serde_json::json!(memory));
        aliases.insert(
            "memory_max",
            serde_json::json!(memory / crate::cgroup::page_size() * crate::cgroup::page_size()),
        );
    }
    if let Some(pids) = declared.pids {
        aliases.insert("pids_max", serde_json::json!(pids));
    }
    let mut sources = BTreeMap::new();
    for (name, value) in &mut fields {
        if let Some(alias) = aliases.get(name.as_str()) {
            if !value.is_null() && value != alias {
                return Err(format!(
                    "{}: {name}",
                    message("legacy resource option conflicts with an explicit value")
                ));
            }
            let origin = if value.is_null() {
                Origin::Compatibility
            } else {
                Origin::Job
            };
            *value = alias.clone();
            sources.insert(name.clone(), origin);
        } else if !value.is_null() {
            sources.insert(name.clone(), Origin::Job);
        } else if let Some(default) = effective.get(&format!("job_{name}")) {
            *value = default.value.clone();
            sources.insert(
                name.clone(),
                Origin::Object {
                    id: default.source_id,
                    path: default.source_path.clone(),
                },
            );
        }
    }
    declared.resources = serde_json::from_value(serde_json::to_value(fields).unwrap())
        .map_err(|error| error.to_string())?;
    declared.resources.validate()?;
    Ok(sources)
}

pub fn option_key(option: &str) -> Option<&'static str> {
    Some(match option {
        "io-max" => "io_max",
        "io-weight" => "io_weight",
        "io-bfq-weight" => "io_bfq_weight",
        "cpu-request" => "cpu_request_milli",
        "cpu-limit" => "cpu_limit_milli",
        "cpu-weight" => "cpu_weight",
        "memory-request" => "memory_request",
        "memory-high" => "memory_high",
        "memory-max" => "memory_max",
        "memory-swap-max" => "memory_swap_max",
        "pids-max" => "pids_max",
        _ => return None,
    })
}

pub fn for_remote(declared: &Declared, sources: &BTreeMap<String, Origin>) -> Declared {
    let mut declared = declared.clone();
    declared.execution_profile = None;
    declared.scheduling_class = None;
    let mut fields = declared.resources.fields();
    for (field, origin) in sources {
        if *origin == Origin::Compatibility {
            fields.insert(field.clone(), serde_json::Value::Null);
        }
    }
    declared.resources = serde_json::from_value(serde_json::to_value(fields).unwrap()).unwrap();
    declared
}

pub fn help() -> String {
    format!(
        "{}\n  --cpu-request N\n  --cpu-limit N|unlimited\n  --cpu-weight N\n  --memory-request SIZE\n  --memory-high SIZE|unlimited\n  --memory-max SIZE|unlimited\n  --memory-swap-max SIZE|unlimited\n  --pids-max N|unlimited\n  --io-max MAJOR:MINOR,rbps=RATE,wbps=RATE,riops=N,wiops=N\n  --io-weight MAJOR:MINOR=WEIGHT\n  --io-bfq-weight MAJOR:MINOR=WEIGHT\n{}\n{}\n{}",
        message("Independent resource controls (all optional):"),
        message(
            "Requests affect admission only. Limits and weights require delegated cgroups. Collection per-Job defaults use --job- before these option names; unset restores inheritance."
        ),
        message(
            "Collection controls without job- apply to the entire subtree. Empty objects create no kernel domains. Inspect all ancestor constraints with queue/group show --json. Changes to active domains require a separate live-update operation."
        ),
        message(
            "I/O rates use bytes/s and operations/s on whole block devices. io-weight requires enabled IOCost; io-bfq-weight requires active BFQ with low_latency=0. Inspect io_devices in host --json. Device-wide policy is never changed automatically."
        )
    )
}

pub fn parse_option(
    option: &str,
    value: &str,
) -> Option<Result<(String, serde_json::Value), String>> {
    let key = option_key(option)?;
    let result = (|| -> Result<serde_json::Value, String> {
        if key == "io_max" {
            return crate::io_policy::parse_max(value);
        }
        if key == "io_weight" || key == "io_bfq_weight" {
            return crate::io_policy::parse_weight(value, key == "io_bfq_weight");
        }
        if matches!(
            key,
            "cpu_limit_milli" | "memory_high" | "memory_max" | "memory_swap_max" | "pids_max"
        ) && value == "unlimited"
        {
            return Ok(serde_json::json!("unlimited"));
        }
        let value = match key {
            "cpu_request_milli" | "cpu_limit_milli" => parse_cpu(value)?,
            "cpu_weight" => value
                .parse::<u64>()
                .map_err(|_| message("cpu-weight must be between 1 and 10000"))?,
            "pids_max" => value
                .parse::<u64>()
                .ok()
                .filter(|count| *count > 0)
                .ok_or_else(|| message("pids-max requires a positive whole number or unlimited"))?,
            _ => parse_bytes(value)?,
        };
        Ok(serde_json::json!(value))
    })();
    Some(result.map(|value| (key.to_owned(), value)))
}

fn parse_cpu(value: &str) -> Result<u64, String> {
    let invalid =
        || message("CPU values require a nonnegative decimal with at most three fractional digits");
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || !whole.bytes().all(|c| c.is_ascii_digit())
        || fraction.len() > 3
        || !fraction.bytes().all(|c| c.is_ascii_digit())
    {
        return Err(invalid());
    }
    let whole: u64 = whole.parse().map_err(|_| invalid())?;
    let fraction: u64 = format!("{fraction:0<3}").parse().map_err(|_| invalid())?;
    whole
        .checked_mul(crate::resources::MILLI)
        .and_then(|whole| whole.checked_add(fraction))
        .ok_or_else(invalid)
}

pub fn parse_bytes(value: &str) -> Result<u64, String> {
    let invalid = || message("memory values require an exact whole-byte size within range");
    let split = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(value.len());
    let (number, suffix) = value.split_at(split);
    let scale: u64 = match suffix.to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => 1 << 30,
        "t" | "tb" | "tib" => 1 << 40,
        _ => return Err(invalid()),
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty() || fraction.len() > 18 || !fraction.bytes().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    let whole = whole
        .parse::<u64>()
        .map_err(|_| invalid())?
        .checked_mul(scale)
        .ok_or_else(invalid)?;
    let fractional = if fraction.is_empty() {
        0
    } else {
        let numerator =
            u128::from(fraction.parse::<u64>().map_err(|_| invalid())?) * u128::from(scale);
        let denominator = 10u128.pow(fraction.len() as u32);
        if numerator % denominator != 0 {
            return Err(invalid());
        }
        u64::try_from(numerator / denominator).map_err(|_| invalid())?
    };
    whole.checked_add(fractional).ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_input_does_not_round_or_saturate_invalid_values() {
        assert_eq!(parse_cpu("1.025").unwrap(), 1025);
        assert_eq!(parse_cpu("0").unwrap(), 0);
        for value in [
            "-1",
            "NaN",
            "inf",
            "1e3",
            "0.0001",
            "18446744073709552",
            "1.2.3",
        ] {
            assert!(parse_cpu(value).is_err(), "{value}");
        }
    }

    #[test]
    fn memory_input_preserves_large_integer_bytes_exactly() {
        assert_eq!(parse_bytes("9007199254740993").unwrap(), 9007199254740993);
        assert_eq!(parse_bytes("0.5GiB").unwrap(), 1 << 29);
        for value in [
            "0.5B",
            "-1M",
            "18446744073709551616",
            "18446744073709551615T",
            "NaN",
        ] {
            assert!(parse_bytes(value).is_err(), "{value}");
        }
    }

    #[test]
    fn remote_forwarding_preserves_compatibility_without_discarding_explicit_controls() {
        let mut declared = Declared {
            cores_milli: Some(500),
            resources: Controls {
                memory_high: Some(Limit::Value(1 << 20)),
                ..Controls::default()
            },
            ..Declared::default()
        };
        let sources = resolve(&mut declared, &BTreeMap::new()).unwrap();
        let mut remote = for_remote(&declared, &sources);
        assert_eq!(remote.cores_milli, Some(500));
        assert_eq!(remote.resources.cpu_request_milli, None);
        assert_eq!(remote.resources.cpu_weight, None);
        assert_eq!(remote.resources.memory_high, declared.resources.memory_high);
        assert_eq!(resolve(&mut remote, &BTreeMap::new()).unwrap(), sources);
        assert_eq!(remote.resources, declared.resources);
    }

    #[test]
    fn limit_validation_rejects_unsafe_kernel_conversions() {
        for limit in [0, MIN_CPU_MILLI - 1, u64::MAX] {
            let controls = Controls {
                cpu_limit_milli: Some(Limit::Value(limit)),
                ..Controls::default()
            };
            assert!(controls.validate().is_err());
        }
        let controls = Controls {
            memory_max: Some(Limit::Value(u64::MAX)),
            ..Controls::default()
        };
        assert!(controls.validate().is_err());
        assert!(
            Controls {
                cpu_limit_milli: Some(Limit::Unlimited),
                memory_swap_max: Some(Limit::Value(0)),
                ..Controls::default()
            }
            .validate()
            .is_ok()
        );
    }
}
