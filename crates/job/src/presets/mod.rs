use std::collections::BTreeMap;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::model::Declared;
use crate::objects::{Effective, Graph};
use crate::resource_policy::{Controls, Origin};
use crate::store::{Store, write_json};

mod messages;
pub use messages::{help, message};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Class {
    pub name: String,
    pub revision: u64,
    pub priority: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    pub revision: u64,
    #[serde(default)]
    pub extends: Vec<String>,
    #[serde(default)]
    pub scheduling_class: Option<String>,
    #[serde(default)]
    pub values: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Definition {
    Class(Class),
    Profile(Profile),
}

impl Definition {
    fn validate(&self) -> Result<(), String> {
        let key = self.key();
        reference(key.split_once(':').unwrap().1)?;
        match self {
            Self::Class(v) => {
                crate::admission::priority(&v.priority.to_string())?;
            }
            Self::Profile(v) => {
                validate_values(&v.values)?;
                if v.extends.len() > 16 {
                    return Err(message("profile composition is too deep"));
                }
                for parent in &v.extends {
                    reference(parent)?;
                    if parent == "none" {
                        return Err(message("profile parent cannot be none"));
                    }
                }
                if let Some(class) = &v.scheduling_class {
                    reference(class)?;
                }
            }
        }
        Ok(())
    }

    pub fn key(&self) -> String {
        match self {
            Self::Class(v) => format!("class:{}@{}", v.name, v.revision),
            Self::Profile(v) => format!("profile:{}@{}", v.name, v.revision),
        }
    }

    pub fn digest(&self) -> String {
        format!("{:x}", Sha256::digest(serde_json::to_vec(self).unwrap()))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    #[serde(default)]
    pub classes: Vec<Class>,
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

pub fn reference(text: &str) -> Result<(), String> {
    if text == "none" {
        return Ok(());
    }
    let valid = text.split_once('@').is_some_and(|(name, revision)| {
        !name.is_empty()
            && name.len() <= 64
            && name != "none"
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            && revision
                .parse::<u64>()
                .is_ok_and(|n| n > 0 && n.to_string() == revision)
    });
    if valid {
        Ok(())
    } else {
        Err(message("use a pinned name@revision or none"))
    }
}

fn validate_values(values: &BTreeMap<String, Value>) -> Result<(), String> {
    let mut fields = Controls::default().fields();
    fields.extend(crate::process_policy::Controls::default().fields());
    fields.extend(crate::security::Controls::default().fields());
    fields.extend(crate::isolation::Controls::default().fields());
    fields.extend(crate::streams::quota::Controls::default().fields());
    for (key, value) in values {
        if value.is_null()
            || (!fields.contains_key(key)
                && !["pids", "wall_ms", "net", "confine"].contains(&key.as_str()))
        {
            return Err(format!("{}: {key}", message("unsupported profile value")));
        }
        if ["pids", "wall_ms"].contains(&key.as_str()) && value.as_u64().is_none_or(|n| n == 0) {
            return Err(message("profile counts and durations must be positive"));
        }
        if key == "net" && !crate::netpolicy::preset_net(value) {
            return Err(message(
                "profiles support only Host, None, ns:NAME or profile:NAME network defaults",
            ));
        }
    }
    let mut declared = serde_json::to_value(Declared::default()).unwrap();
    for (key, value) in values {
        declared[key] = crate::netpolicy::preset_value(key, value);
    }
    let parsed: Declared = serde_json::from_value(declared).map_err(|e| e.to_string())?;
    parsed.resources.validate()?;
    parsed.process.validate()?;
    parsed.security.validate()?;
    parsed.output.validate()
}

impl Catalog {
    pub fn definitions(&self) -> Result<BTreeMap<String, Definition>, String> {
        if self.classes.len() + self.profiles.len() > 128 {
            return Err(message("too many preset definitions"));
        }
        let mut all = BTreeMap::new();
        for definition in self
            .classes
            .iter()
            .cloned()
            .map(Definition::Class)
            .chain(self.profiles.iter().cloned().map(Definition::Profile))
        {
            let key = definition.key();
            definition.validate()?;
            if all.insert(key, definition).is_some() {
                return Err(message("duplicate preset revision"));
            }
        }
        for p in &self.profiles {
            let mut resolution = Resolved::default();
            expand(
                &all,
                &format!("{}@{}", p.name, p.revision),
                &mut Vec::new(),
                &mut resolution,
            )?;
        }
        Ok(all)
    }

    pub fn validate_graph(&self, graph: &Graph) -> Result<(), String> {
        let all = self.definitions()?;
        for node in graph.nodes.values() {
            for (field, kind) in [
                ("job_execution_profile", "profile"),
                ("job_scheduling_class", "class"),
            ] {
                if let Some(value) = node.config.get(field) {
                    let value = value
                        .as_str()
                        .ok_or_else(|| message("use a pinned name@revision or none"))?;
                    reference(value)?;
                    if value != "none" && !all.contains_key(&format!("{kind}:{value}")) {
                        return Err(format!(
                            "{}: {kind}:{value}",
                            message("unknown preset revision")
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct Resolved {
    steps: usize,
    values: BTreeMap<String, Value>,
    providers: BTreeMap<String, String>,
    definitions: BTreeMap<String, Definition>,
    class: Option<String>,
}

fn class_definition<'a>(
    all: &'a BTreeMap<String, Definition>,
    reference: &str,
) -> Result<Option<&'a Class>, String> {
    if reference == "none" {
        return Ok(None);
    }
    match all.get(&format!("class:{reference}")) {
        Some(Definition::Class(class)) => Ok(Some(class)),
        _ => Err(format!(
            "{}: class:{reference}",
            message("unknown preset revision")
        )),
    }
}

fn expand(
    all: &BTreeMap<String, Definition>,
    reference: &str,
    path: &mut Vec<String>,
    out: &mut Resolved,
) -> Result<(), String> {
    out.steps += 1;
    if out.steps > 1024 {
        return Err(message("profile composition exceeds the expansion budget"));
    }
    let key = format!("profile:{reference}");
    if path.len() >= 16 || path.contains(&key) {
        return Err(message("profile composition is cyclic or too deep"));
    }
    let Some(Definition::Profile(profile)) = all.get(&key) else {
        return Err(format!("{}: {key}", message("unknown preset revision")));
    };
    path.push(key.clone());
    for parent in &profile.extends {
        expand(all, parent, path, out)?;
    }
    path.pop();
    for (field, value) in &profile.values {
        out.values.insert(field.clone(), value.clone());
        out.providers.insert(field.clone(), key.clone());
    }
    if profile.scheduling_class.is_some() {
        out.class = profile.scheduling_class.clone();
    }
    if let Some(reference) = &profile.scheduling_class
        && let Some(class) = class_definition(all, reference)?
    {
        out.definitions.insert(
            format!("class:{reference}"),
            Definition::Class(class.clone()),
        );
    }
    out.definitions
        .insert(key, Definition::Profile(profile.clone()));
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    Job,
    Object { id: u64, path: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub reference: String,
    pub source: Source,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    pub execution_profile: Option<Selection>,
    pub scheduling_class: Option<Selection>,
    pub definitions: BTreeMap<String, Definition>,
    pub defaults: BTreeMap<String, Value>,
    pub providers: BTreeMap<String, String>,
    pub applied: BTreeMap<String, Origin>,
}

fn selected(
    job: &Option<String>,
    effective: &BTreeMap<String, Effective>,
    key: &str,
) -> Option<Selection> {
    job.as_ref()
        .map(|value| Selection {
            reference: value.clone(),
            source: Source::Job,
        })
        .or_else(|| {
            effective.get(key).and_then(|v| {
                v.value.as_str().map(|value| Selection {
                    reference: value.to_owned(),
                    source: Source::Object {
                        id: v.source_id,
                        path: v.source_path.clone(),
                    },
                })
            })
        })
}

fn rank(source: &Source, ancestors: &[u64]) -> usize {
    match source {
        Source::Job => 0,
        Source::Object { id, .. } => {
            ancestors
                .iter()
                .position(|v| v == id)
                .unwrap_or(usize::MAX - 1)
                + 1
        }
    }
}

pub fn resolve(
    catalog: &Catalog,
    graph: &Graph,
    queue: u64,
    declared: &mut Declared,
) -> Result<Option<Snapshot>, String> {
    let effective = graph.view(queue).effective;
    let profile = selected(
        &declared.execution_profile,
        &effective,
        "job_execution_profile",
    );
    let mut class = selected(
        &declared.scheduling_class,
        &effective,
        "job_scheduling_class",
    );
    if profile.is_none() && class.is_none() {
        return Ok(None);
    }
    let all = catalog.definitions()?;
    let ancestors = graph.ancestors(queue);
    let mut out = Resolved::default();
    if let Some(profile) = &profile {
        reference(&profile.reference)?;
        if profile.reference != "none" {
            expand(&all, &profile.reference, &mut Vec::new(), &mut out)?;
        }
        if let Some(reference) = &out.class
            && class
                .as_ref()
                .is_none_or(|c| rank(&profile.source, &ancestors) < rank(&c.source, &ancestors))
        {
            class = Some(Selection {
                reference: reference.clone(),
                source: profile.source.clone(),
            });
        }
    }
    if let Some(selected) = &class {
        reference(&selected.reference)?;
        if let Some(definition) = class_definition(&all, &selected.reference)? {
            let key = format!("class:{}", selected.reference);
            out.values
                .insert("priority".to_owned(), Value::from(definition.priority));
            out.providers.insert("priority".to_owned(), key.clone());
            out.definitions
                .insert(key, Definition::Class(definition.clone()));
        }
    }
    let mut values = serde_json::to_value(&*declared).unwrap();
    let mut controls = Controls::default().fields();
    controls.extend(crate::process_policy::Controls::default().fields());
    controls.extend(crate::security::Controls::default().fields());
    controls.extend(crate::isolation::Controls::default().fields());
    controls.extend(crate::streams::quota::Controls::default().fields());
    let mut applied = BTreeMap::new();
    for (field, value) in &out.values {
        let selection = if field == "priority" {
            class.as_ref().unwrap()
        } else {
            profile.as_ref().unwrap()
        };
        let key = if controls.contains_key(field) {
            format!("job_{field}")
        } else {
            field.clone()
        };
        let inherited_wins = effective.get(&key).is_some_and(|v| {
            rank(
                &Source::Object {
                    id: v.source_id,
                    path: v.source_path.clone(),
                },
                &ancestors,
            ) <= rank(&selection.source, &ancestors)
        });
        let alias = (declared.cores_milli.is_some()
            && ["cpu_request_milli", "cpu_weight"].contains(&field.as_str()))
            || (declared.memory.is_some()
                && ["memory_request", "memory_max"].contains(&field.as_str()));
        let explicit = !values[field].is_null() && !(field == "confine" && values[field] == false);
        if !explicit && !alias && !inherited_wins {
            values[field] = crate::netpolicy::preset_value(field, value);
            let provider = &out.providers[field];
            applied.insert(
                field.clone(),
                Origin::Preset {
                    definition: provider.clone(),
                    sha256: out.definitions[provider].digest(),
                    selected_by: selection.source.clone(),
                },
            );
        }
    }
    *declared = serde_json::from_value(values).map_err(|e| e.to_string())?;
    declared.execution_profile = profile.as_ref().map(|s| s.reference.clone());
    declared.scheduling_class = class.as_ref().map(|s| s.reference.clone());
    Ok(Some(Snapshot {
        schema_version: 1,
        execution_profile: profile,
        scheduling_class: class,
        definitions: out.definitions,
        defaults: out.values,
        providers: out.providers,
        applied,
    }))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    schema_version: u32,
    definitions: BTreeMap<String, Definition>,
}

impl Registry {
    pub fn load(store: &Store) -> io::Result<Self> {
        let registry: Self = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(store.root.join("presets.json"))
        {
            Ok(file) => {
                if !file.metadata()?.is_file() {
                    return Err(io::Error::other(message("invalid preset registry")));
                }
                let mut bytes = Vec::new();
                file.take(16_777_217).read_to_end(&mut bytes)?;
                if bytes.len() > 16_777_216 {
                    return Err(io::Error::other(message("preset registry is full")));
                }
                serde_json::from_slice(&bytes)?
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self {
                schema_version: 1,
                definitions: BTreeMap::new(),
            },
            Err(e) => return Err(e),
        };
        if registry.schema_version != 1
            || registry.definitions.len() > 4096
            || registry
                .definitions
                .iter()
                .any(|(key, value)| *key != value.key())
        {
            return Err(io::Error::other(message("invalid preset registry")));
        }
        for definition in registry.definitions.values() {
            definition.validate().map_err(io::Error::other)?;
            if let Definition::Profile(v) = definition {
                expand(
                    &registry.definitions,
                    &format!("{}@{}", v.name, v.revision),
                    &mut Vec::new(),
                    &mut Resolved::default(),
                )
                .map_err(io::Error::other)?;
            }
        }
        Ok(registry)
    }

    pub fn enroll(store: &Store, catalog: &Catalog) -> io::Result<()> {
        let mut registry = Self::load(store)?;
        let mut changed = false;
        for (key, definition) in catalog.definitions().map_err(io::Error::other)? {
            if let Some(old) = registry.definitions.get(&key) {
                if *old != definition {
                    return Err(io::Error::other(format!(
                        "{}: {key}",
                        message("preset revision is immutable; use a new revision")
                    )));
                }
            } else {
                registry.definitions.insert(key, definition);
                changed = true;
            }
        }
        if registry.definitions.len() > 4096
            || serde_json::to_vec_pretty(&registry)?.len() > 16_777_216
        {
            return Err(io::Error::other(message("preset registry is full")));
        }
        if changed {
            write_json(&store.root.join("presets.json"), &registry)?;
        }
        Ok(())
    }

    pub fn validate_snapshot(&self, snapshot: &Snapshot) -> io::Result<()> {
        if snapshot.schema_version != 1
            || (snapshot.execution_profile.is_none() && snapshot.scheduling_class.is_none())
            || snapshot
                .definitions
                .iter()
                .any(|(key, value)| self.definitions.get(key) != Some(value))
        {
            return Err(io::Error::other(message("invalid preset snapshot")));
        }
        for selection in [&snapshot.execution_profile, &snapshot.scheduling_class]
            .into_iter()
            .flatten()
        {
            reference(&selection.reference).map_err(io::Error::other)?;
            if let Source::Object { id, path } = &selection.source
                && (*id == 0 || path.is_empty())
            {
                return Err(io::Error::other(message("invalid preset snapshot")));
            }
        }
        let mut expected = Resolved::default();
        if let Some(profile) = &snapshot.execution_profile
            && profile.reference != "none"
        {
            expand(
                &snapshot.definitions,
                &profile.reference,
                &mut Vec::new(),
                &mut expected,
            )
            .map_err(io::Error::other)?;
        }
        if let Some(class) = &snapshot.scheduling_class
            && let Some(value) = class_definition(&snapshot.definitions, &class.reference)
                .map_err(io::Error::other)?
        {
            expected
                .values
                .insert("priority".to_owned(), Value::from(value.priority));
            expected
                .providers
                .insert("priority".to_owned(), format!("class:{}", class.reference));
            expected.definitions.insert(
                format!("class:{}", class.reference),
                Definition::Class(value.clone()),
            );
        }
        let invalid_origin = snapshot.applied.iter().any(|(field, origin)| {
            let Some(provider) = snapshot.providers.get(field) else {
                return true;
            };
            let selection = if field == "priority" {
                &snapshot.scheduling_class
            } else {
                &snapshot.execution_profile
            };
            let Origin::Preset {
                definition,
                sha256,
                selected_by,
            } = origin
            else {
                return true;
            };
            definition != provider
                || snapshot
                    .definitions
                    .get(definition)
                    .is_none_or(|v| v.digest() != *sha256)
                || selection.as_ref().is_none_or(|s| s.source != *selected_by)
        });
        if expected.definitions != snapshot.definitions
            || expected.values != snapshot.defaults
            || expected.providers != snapshot.providers
            || invalid_origin
        {
            return Err(io::Error::other(message("invalid preset snapshot")));
        }
        Ok(())
    }
}
