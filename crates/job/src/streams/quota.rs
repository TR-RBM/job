use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Source, text};
use crate::model::Job;
use crate::resource_policy::Origin;
use crate::units::format_bytes;

pub const MINIMUM: u64 = super::SEGMENT_BYTES;
pub const MAXIMUM: u64 = 1 << 30;
pub const BUILT_IN: u64 = super::HEAD_SEGMENTS as u64 * super::SEGMENT_BYTES;
pub const BUDGET: u64 = 1 << 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quota {
    pub head_bytes: u64,
    pub tail_bytes: u64,
}

impl Quota {
    pub fn of(controls: &Controls) -> Option<Self> {
        (controls.output_head_bytes.is_some() || controls.output_tail_bytes.is_some()).then(|| {
            Self {
                head_bytes: controls.output_head_bytes.unwrap_or(BUILT_IN),
                tail_bytes: controls.output_tail_bytes.unwrap_or(BUILT_IN),
            }
        })
    }

    pub fn valid(&self) -> bool {
        [self.head_bytes, self.tail_bytes]
            .iter()
            .all(|bytes| (MINIMUM..=MAXIMUM).contains(bytes))
    }
}

fn bounded(bytes: u64) -> Result<u64, String> {
    if (MINIMUM..=MAXIMUM).contains(&bytes) {
        Ok(bytes)
    } else {
        Err(text(
            "an output quota must be between {minimum} and {maximum}",
            &[
                ("minimum", format_bytes(MINIMUM)),
                ("maximum", format_bytes(MAXIMUM)),
            ],
        ))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Controls {
    pub output_head_bytes: Option<u64>,
    pub output_tail_bytes: Option<u64>,
}

impl Controls {
    pub fn fields(&self) -> BTreeMap<String, Value> {
        serde_json::from_value(serde_json::to_value(self).unwrap()).unwrap()
    }

    pub fn from_settings(settings: &crate::model::Settings) -> Self {
        Self {
            output_head_bytes: settings.job_output_head_bytes,
            output_tail_bytes: settings.job_output_tail_bytes,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        for bytes in [self.output_head_bytes, self.output_tail_bytes]
            .into_iter()
            .flatten()
        {
            bounded(bytes)?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Written {
    Number(u64),
    Text(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Written", into = "u64")]
pub struct Size(pub u64);

impl TryFrom<Written> for Size {
    type Error = String;
    fn try_from(written: Written) -> Result<Self, String> {
        match written {
            Written::Number(bytes) => Ok(Self(bytes)),
            Written::Text(size) => crate::units::parse_bytes(&size).map(Self),
        }
    }
}

impl From<Size> for u64 {
    fn from(size: Size) -> Self {
        size.0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Service {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_bytes: Option<Size>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail_bytes: Option<Size>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_bytes: Option<Size>,
}

impl Service {
    pub fn unset(&self) -> bool {
        *self == Self::default()
    }

    pub fn validate(&self) -> Result<(), String> {
        for size in [self.head_bytes, self.tail_bytes].into_iter().flatten() {
            bounded(size.0)?;
        }
        Ok(())
    }

    pub fn budget(&self) -> u64 {
        self.budget_bytes.map_or(BUDGET, |size| size.0)
    }

    fn field(&self, name: &str) -> Option<u64> {
        match name {
            "output_head_bytes" => self.head_bytes,
            _ => self.tail_bytes,
        }
        .map(|size| size.0)
    }
}

pub fn resolve(
    declared: &mut crate::model::Declared,
    effective: &BTreeMap<String, crate::objects::Effective>,
    service: &Service,
) -> Result<BTreeMap<String, Origin>, String> {
    let mut fields = declared.output.fields();
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
        } else if let Some(bytes) = service.field(name) {
            *value = Value::from(bytes);
            sources.insert(name.clone(), Origin::Service);
        }
    }
    declared.output =
        serde_json::from_value(serde_json::to_value(fields).unwrap()).map_err(|e| e.to_string())?;
    declared.output.validate()?;
    Ok(sources)
}

fn key(option: &str) -> Option<&'static str> {
    match option {
        "output-head" => Some("output_head_bytes"),
        "output-tail" => Some("output_tail_bytes"),
        _ => None,
    }
}

pub fn parse_option(option: &str, value: &str) -> Option<Result<(String, Value), String>> {
    let key = key(option)?;
    Some(
        crate::units::parse_bytes(value)
            .and_then(bounded)
            .map(|bytes| (key.to_owned(), Value::from(bytes))),
    )
}

pub fn unset_key(word: &str) -> Option<String> {
    word.strip_prefix("job-")
        .and_then(key)
        .map(|key| format!("job_{key}"))
}

pub fn default_option(word: &str) -> bool {
    matches!(word, "--job-output-head" | "--job-output-tail")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Total {
    pub stream: Source,
    pub written_bytes: u64,
    pub dropped_bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Retention {
    pub head_bytes: Option<u64>,
    pub tail_bytes: Option<u64>,
    pub streams: Vec<Total>,
    pub omitted_bytes: u64,
    pub trimmed_bytes: Option<u64>,
}

fn origin(job: &Job, field: &str) -> String {
    match job.resource_sources.get(field) {
        Some(Origin::Job) => text("this Job", &[]),
        Some(Origin::Object { path, .. }) => path.clone(),
        Some(Origin::Preset { definition, .. }) => definition.clone(),
        Some(Origin::Service) => text("service configuration", &[]),
        _ => text("built-in", &[]),
    }
}

pub fn stream_name(stream: Source) -> String {
    serde_json::to_value(stream)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

pub fn kept(quota: Option<Quota>) -> String {
    match quota {
        Some(quota) => text(
            "kept per stream: at least the first {head} and the last {tail}",
            &[
                ("head", format_bytes(quota.head_bytes)),
                ("tail", format_bytes(quota.tail_bytes)),
            ],
        ),
        None => text(
            "kept for all streams together: the first {head} and the last {tail}",
            &[
                ("head", format_bytes(BUILT_IN)),
                ("tail", format_bytes(BUILT_IN)),
            ],
        ),
    }
}

pub fn summary(job: &Job) -> Option<String> {
    let quota = Quota::of(&job.spec.declared.output);
    let retention = job
        .result
        .as_ref()
        .and_then(|result| result.output_retention.as_ref());
    let dropped: Vec<String> = retention
        .map(|retention| {
            retention
                .streams
                .iter()
                .filter(|total| quota.is_some() || total.dropped_bytes > 0)
                .map(|total| {
                    text(
                        "{stream} {bytes} bytes",
                        &[
                            ("stream", stream_name(total.stream)),
                            ("bytes", total.dropped_bytes.to_string()),
                        ],
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let omitted = retention.map_or(0, |retention| retention.omitted_bytes);
    let trimmed = retention.and_then(|retention| retention.trimmed_bytes);
    if quota.is_none() && dropped.is_empty() && omitted == 0 && trimmed.is_none() {
        return None;
    }
    let mut parts = vec![match quota {
        Some(quota) => text(
            "output quota per stream: first {head} ({head_source}), last {tail} ({tail_source})",
            &[
                ("head", format_bytes(quota.head_bytes)),
                ("head_source", origin(job, "output_head_bytes")),
                ("tail", format_bytes(quota.tail_bytes)),
                ("tail_source", origin(job, "output_tail_bytes")),
            ],
        ),
        None => text(
            "output quota for all streams together: first {head}, last {tail} (built-in)",
            &[
                ("head", format_bytes(BUILT_IN)),
                ("tail", format_bytes(BUILT_IN)),
            ],
        ),
    }];
    if !dropped.is_empty() {
        parts.push(text(
            "removed by retention: {streams}",
            &[("streams", dropped.join(", "))],
        ));
    }
    if omitted > 0 {
        parts.push(text(
            "{bytes} bytes were dropped before they reached this recording",
            &[("bytes", omitted.to_string())],
        ));
    }
    if let Some(bytes) = trimmed {
        parts.push(text(
            "the recording was removed by the service output budget ({bytes} bytes)",
            &[("bytes", bytes.to_string())],
        ));
    }
    Some(format!("note: {}", parts.join("; ")))
}

pub fn help() -> String {
    [
        "Output quota: --output-head SIZE; --output-tail SIZE",
        "Each recorded stream keeps at least its first and its last SIZE, between 1M and 1G; what lies between is removed and reported. Queue and Group defaults use the --job- prefix; the service configuration sets [output] head_bytes, tail_bytes and budget_bytes. There is no unlimited.",
    ]
    .map(|line| text(line, &[]))
    .join("\n")
}
