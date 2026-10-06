use crate::resource_policy::{Limit, Origin};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub mod kernel;
mod messages;
pub use messages::{help, message};
pub const MAX_ID: u32 = 65_535;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Ids(pub Vec<u32>);
impl TryFrom<String> for Ids {
    type Error = String;
    fn try_from(text: String) -> Result<Self, String> {
        if text.len() > 65_536 {
            return Err(message("invalid CPU or NUMA node list"));
        }
        let mut out = BTreeSet::new();
        for part in text.split(',') {
            let (a, b) = part.split_once('-').unwrap_or((part, part));
            let parse = |v: &str| -> Result<u32, String> {
                if v.is_empty() || !v.bytes().all(|c| c.is_ascii_digit()) {
                    return Err(message("invalid CPU or NUMA node list"));
                }
                v.parse::<u32>()
                    .ok()
                    .filter(|n| *n <= MAX_ID)
                    .ok_or_else(|| message("invalid CPU or NUMA node list"))
            };
            let (a, b) = (parse(a)?, parse(b)?);
            if a > b {
                return Err(message("invalid CPU or NUMA node list"));
            }
            out.extend(a..=b);
        }
        if out.is_empty() {
            return Err(message("invalid CPU or NUMA node list"));
        }
        Ok(Self(out.into_iter().collect()))
    }
}
impl From<Ids> for String {
    fn from(ids: Ids) -> Self {
        let mut parts = Vec::new();
        let mut index = 0;
        while index < ids.0.len() {
            let first = ids.0[index];
            let mut last = first;
            index += 1;
            while index < ids.0.len() && ids.0[index] == last + 1 {
                last += 1;
                index += 1;
            }
            parts.push(if first == last {
                first.to_string()
            } else {
                format!("{first}-{last}")
            });
        }
        parts.join(",")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Affinity {
    Inherit,
    Set(Ids),
}
impl TryFrom<String> for Affinity {
    type Error = String;
    fn try_from(text: String) -> Result<Self, String> {
        if text == "inherit" {
            Ok(Self::Inherit)
        } else {
            Ids::try_from(text).map(Self::Set)
        }
    }
}
impl From<Affinity> for String {
    fn from(v: Affinity) -> Self {
        match v {
            Affinity::Inherit => "inherit".into(),
            Affinity::Set(ids) => ids.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Numa {
    pub mode: String,
    pub nodes: Option<Ids>,
}
impl TryFrom<String> for Numa {
    type Error = String;
    fn try_from(text: String) -> Result<Self, String> {
        if matches!(text.as_str(), "inherit" | "default" | "local") {
            return Ok(Self {
                mode: text,
                nodes: None,
            });
        }
        if let Some((mode, nodes)) = text.split_once(':')
            && matches!(mode, "bind" | "interleave" | "preferred")
        {
            let nodes = Ids::try_from(nodes.to_owned())?;
            if mode != "preferred" || nodes.0.len() == 1 {
                return Ok(Self {
                    mode: mode.to_owned(),
                    nodes: Some(nodes),
                });
            }
        }
        Err(message("invalid NUMA policy"))
    }
}
impl From<Numa> for String {
    fn from(v: Numa) -> Self {
        v.nodes.map_or(v.mode.clone(), |ids| {
            format!("{}:{}", v.mode, String::from(ids))
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Value", into = "Value")]
pub enum Bound {
    Inherit,
    Limit(Limit),
}
impl TryFrom<Value> for Bound {
    type Error = String;
    fn try_from(v: Value) -> Result<Self, String> {
        if v == "inherit" {
            return Ok(Self::Inherit);
        }
        let limit: Limit = serde_json::from_value(v).map_err(|e| e.to_string())?;
        if matches!(limit,Limit::Value(n) if u128::from(n) >= u128::from(libc::RLIM_INFINITY)) {
            return Err(message("rlimit value is outside the kernel range"));
        }
        Ok(Self::Limit(limit))
    }
}
impl From<Bound> for Value {
    fn from(v: Bound) -> Self {
        match v {
            Bound::Inherit => Value::from("inherit"),
            Bound::Limit(limit) => serde_json::to_value(limit).unwrap(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pair {
    pub soft: Bound,
    pub hard: Bound,
}
impl Pair {
    pub fn resolve(self, inherited: libc::rlimit) -> Result<libc::rlimit, String> {
        let resolve = |bound: Bound, old: libc::rlim_t| match bound {
            Bound::Inherit => old,
            Bound::Limit(Limit::Unlimited) => libc::RLIM_INFINITY,
            Bound::Limit(Limit::Value(n)) => n as libc::rlim_t,
        };
        let result = libc::rlimit {
            rlim_cur: resolve(self.soft, inherited.rlim_cur),
            rlim_max: resolve(self.hard, inherited.rlim_max),
        };
        if result.rlim_cur > result.rlim_max {
            return Err(message("rlimit soft value exceeds hard value"));
        }
        Ok(result)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Controls {
    pub cpu_affinity: Option<Affinity>,
    pub numa_policy: Option<Numa>,
    pub rlimit_as: Option<Pair>,
    pub rlimit_core: Option<Pair>,
    pub rlimit_cpu: Option<Pair>,
    pub rlimit_data: Option<Pair>,
    pub rlimit_fsize: Option<Pair>,
    pub rlimit_memlock: Option<Pair>,
    pub rlimit_msgqueue: Option<Pair>,
    pub rlimit_nice: Option<Pair>,
    pub rlimit_nofile: Option<Pair>,
    pub rlimit_nproc: Option<Pair>,
    pub rlimit_rtprio: Option<Pair>,
    pub rlimit_rttime: Option<Pair>,
    pub rlimit_sigpending: Option<Pair>,
    pub rlimit_stack: Option<Pair>,
}

impl Controls {
    pub fn fields(&self) -> BTreeMap<String, Value> {
        serde_json::from_value(serde_json::to_value(self).unwrap()).unwrap()
    }
    pub fn from_settings(settings: &crate::model::Settings) -> Self {
        let fields = serde_json::to_value(settings).unwrap();
        let values: serde_json::Map<String, Value> = Self::default()
            .fields()
            .keys()
            .map(|k| (k.clone(), fields[format!("job_{k}")].clone()))
            .collect();
        serde_json::from_value(Value::Object(values)).unwrap()
    }
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in self.fields() {
            if name.starts_with("rlimit_") && !value.is_null() {
                let pair: Pair = serde_json::from_value(value).map_err(|e| e.to_string())?;
                if !matches!(pair.soft, Bound::Inherit) && !matches!(pair.hard, Bound::Inherit) {
                    pair.resolve(libc::rlimit {
                        rlim_cur: 0,
                        rlim_max: 0,
                    })?;
                }
                for bound in [pair.soft, pair.hard] {
                    if let Bound::Limit(Limit::Value(n)) = bound
                        && ((name == "rlimit_nice" && n > 40)
                            || (name == "rlimit_rtprio" && n > 99))
                    {
                        return Err(message("rlimit scheduling value is outside its range"));
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn resolve(
    declared: &mut crate::model::Declared,
    effective: &BTreeMap<String, crate::objects::Effective>,
) -> Result<BTreeMap<String, Origin>, String> {
    let mut fields = declared.process.fields();
    let mut sources = BTreeMap::new();
    for (name, value) in &mut fields {
        if !value.is_null() {
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
    declared.process =
        serde_json::from_value(serde_json::to_value(fields).unwrap()).map_err(|e| e.to_string())?;
    declared.process.validate()?;
    Ok(sources)
}

pub fn parse_option(option: &str, value: &str) -> Option<Result<(String, Value), String>> {
    let result = || {
        let (key, value) = match option {
            "cpu-affinity" => (
                "cpu_affinity".to_owned(),
                serde_json::to_value(Affinity::try_from(value.to_owned())?).unwrap(),
            ),
            "numa-policy" => (
                "numa_policy".to_owned(),
                serde_json::to_value(Numa::try_from(value.to_owned())?).unwrap(),
            ),
            "rlimit" => {
                let (name, pair) = value
                    .split_once('=')
                    .ok_or_else(|| message("rlimit requires NAME=SOFT:HARD"))?;
                if kernel::resource(name).is_none() {
                    return Err(message("unsupported rlimit resource"));
                }
                let (soft, hard) = pair.split_once(':').unwrap_or((pair, pair));
                let bound = |text: &str| -> Result<Bound, String> {
                    if text == "inherit" {
                        return Ok(Bound::Inherit);
                    }
                    if text == "unlimited" {
                        return Ok(Bound::Limit(Limit::Unlimited));
                    }
                    let n = if matches!(
                        name,
                        "as" | "core" | "data" | "fsize" | "memlock" | "msgqueue" | "stack"
                    ) {
                        crate::resource_policy::parse_bytes(text)?
                    } else {
                        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
                            return Err(message(
                                "rlimit requires a nonnegative integer in native units",
                            ));
                        }
                        text.parse::<u64>()
                            .map_err(|_| message("rlimit value is outside the kernel range"))?
                    };
                    Bound::try_from(Value::from(n))
                };
                (
                    format!("rlimit_{name}"),
                    serde_json::to_value(Pair {
                        soft: bound(soft)?,
                        hard: bound(hard)?,
                    })
                    .unwrap(),
                )
            }
            _ => unreachable!(),
        };
        let controls: Controls =
            serde_json::from_value(serde_json::json!({key.clone():value.clone()}))
                .map_err(|e| e.to_string())?;
        controls.validate()?;
        Ok((key, value))
    };
    matches!(option, "cpu-affinity" | "numa-policy" | "rlimit").then(result)
}

pub fn validate_job(job: &crate::model::Job) -> Result<(), String> {
    for spec in std::iter::once(&job.spec)
        .chain(job.submitted_spec.iter())
        .chain(job.requested_spec.iter())
        .chain(job.effective_spec.iter())
    {
        spec.declared.process.validate()?;
    }
    if let Some(applied) = job
        .result
        .as_ref()
        .and_then(|result| result.process_controls.as_ref())
    {
        let mut fields = BTreeMap::new();
        if applied
            .numa_policy
            .as_ref()
            .is_some_and(|policy| policy.mode == "inherit")
        {
            return Err(message("invalid applied process controls"));
        }
        for (name, pair) in &applied.rlimits {
            if kernel::resource(name).is_none()
                || matches!(pair.soft, Bound::Inherit)
                || matches!(pair.hard, Bound::Inherit)
            {
                return Err(message("invalid applied process controls"));
            }
            fields.insert(
                format!("rlimit_{name}"),
                serde_json::to_value(pair).unwrap(),
            );
        }
        let controls: Controls = serde_json::from_value(serde_json::to_value(fields).unwrap())
            .map_err(|e| e.to_string())?;
        controls.validate()?;
    }
    Ok(())
}
