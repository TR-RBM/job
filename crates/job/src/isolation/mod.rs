use crate::resource_policy::Origin;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

pub mod kernel;
mod messages;
pub mod pidns;
pub use messages::{help, message, text};

pub const KINDS: &[&str] = &["cgroup", "ipc", "mount", "pid", "user", "uts"];
pub const OPTIONS: &[&str] = &["namespaces", "root", "private-tmp", "writable"];
const MAX_PATHS: usize = 32;
const MAX_PATH_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Namespaces(String);
impl TryFrom<String> for Namespaces {
    type Error = String;
    fn try_from(value: String) -> Result<Self, String> {
        if value == "inherit" {
            return Ok(Self(value));
        }
        if value.len() > 256 {
            return Err(message("invalid namespace list"));
        }
        let names: BTreeSet<_> = value.split(',').collect();
        if names.contains("net") {
            return Err(message(
                "the network namespace is requested with --net, not with --namespaces",
            ));
        }
        if names.iter().any(|name| !KINDS.contains(name)) {
            return Err(message("invalid namespace list"));
        }
        Ok(Self(names.into_iter().collect::<Vec<_>>().join(",")))
    }
}
impl From<Namespaces> for String {
    fn from(value: Namespaces) -> Self {
        value.0
    }
}
impl Namespaces {
    pub fn active(&self) -> bool {
        self.0 != "inherit"
    }
    pub fn kinds(&self) -> impl Iterator<Item = &str> {
        self.0.split(',').filter(|_| self.active())
    }
    pub fn has(&self, kind: &str) -> bool {
        self.kinds().any(|name| name == kind)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Root {
    #[serde(rename = "read-only")]
    ReadOnly,
    #[serde(rename = "inherit")]
    Inherit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Writable(String);
impl TryFrom<String> for Writable {
    type Error = String;
    fn try_from(value: String) -> Result<Self, String> {
        if value == "inherit" {
            return Ok(Self(value));
        }
        let paths: BTreeSet<_> = value.split(',').collect();
        if paths.len() > MAX_PATHS
            || paths.iter().any(|path| {
                let parsed = Path::new(path);
                path.len() > MAX_PATH_BYTES
                    || path.contains('\0')
                    || !parsed.is_absolute()
                    || parsed.parent().is_none()
                    || parsed
                        .components()
                        .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
            })
        {
            return Err(message(
                "--writable takes up to 32 absolute paths separated by commas, without . or .. and not / itself",
            ));
        }
        Ok(Self(paths.into_iter().collect::<Vec<_>>().join(",")))
    }
}
impl From<Writable> for String {
    fn from(value: Writable) -> Self {
        value.0
    }
}
impl Writable {
    pub fn active(&self) -> bool {
        self.0 != "inherit"
    }
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.0.split(',').filter(|_| self.active())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Controls {
    pub namespaces: Option<Namespaces>,
    pub root: Option<Root>,
    pub private_tmp: Option<bool>,
    pub writable: Option<Writable>,
}
impl Controls {
    pub fn fields(&self) -> BTreeMap<String, Value> {
        serde_json::from_value(serde_json::to_value(self).unwrap()).unwrap()
    }
    pub fn kinds(&self) -> impl Iterator<Item = &str> {
        self.namespaces.iter().flat_map(Namespaces::kinds)
    }
    pub fn has(&self, kind: &str) -> bool {
        self.kinds().any(|name| name == kind)
    }
    pub fn read_only(&self) -> bool {
        self.root == Some(Root::ReadOnly)
    }
    pub fn private(&self) -> bool {
        self.private_tmp == Some(true)
    }
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.writable.iter().flat_map(Writable::paths)
    }
    pub fn mounts(&self) -> bool {
        self.read_only() || self.private() || self.paths().next().is_some()
    }
    pub fn active(&self) -> bool {
        self.kinds().next().is_some() || self.mounts()
    }
    pub fn enters_user_namespace(&self) -> bool {
        self.has("user")
    }
    pub fn validate(&self) -> Result<(), String> {
        let needs = |option: &str| {
            Err(text(
                "{option} needs both mount and user in --namespaces; add --namespaces user,mount",
                &[("option", option.to_owned())],
            ))
        };
        if self.has("pid") && !self.has("mount") {
            return Err(message(
                "pid needs mount in --namespaces, because the job gets its own /proc; add mount",
            ));
        }
        let entered = self.has("mount") && self.has("user");
        if self.read_only() && !entered {
            return needs("--root read-only");
        }
        if self.private() && !entered {
            return needs("--private-tmp yes");
        }
        if self.paths().next().is_some() {
            if !entered {
                return needs("--writable");
            }
            if !self.read_only() {
                return Err(message(
                    "--writable needs --root read-only; without it every path is already writable as before",
                ));
            }
        }
        if self.private()
            && let Some(path) = self.paths().find(|path| {
                ["/tmp", "/var/tmp"]
                    .iter()
                    .any(|hidden| Path::new(path).starts_with(hidden))
            })
        {
            return Err(text(
                "--private-tmp yes hides {path}; choose a writable path outside /tmp and /var/tmp",
                &[("path", path.to_owned())],
            ));
        }
        Ok(())
    }
}

pub fn resolve(
    declared: &mut crate::model::Declared,
    effective: &BTreeMap<String, crate::objects::Effective>,
) -> Result<BTreeMap<String, Origin>, String> {
    let mut fields = declared.isolation.fields();
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
    declared.isolation =
        serde_json::from_value(serde_json::to_value(fields).unwrap()).map_err(|e| e.to_string())?;
    declared.isolation.validate()?;
    Ok(sources)
}

pub fn parse_option(option: &str, value: &str) -> Option<Result<(String, Value), String>> {
    let key = match option {
        "namespaces" => "namespaces",
        "root" => "root",
        "private-tmp" => "private_tmp",
        "writable" => "writable",
        _ => return None,
    };
    Some((|| {
        let value = match key {
            "namespaces" => serde_json::to_value(Namespaces::try_from(value.to_owned())?).unwrap(),
            "root" => match value {
                "read-only" => serde_json::to_value(Root::ReadOnly).unwrap(),
                "inherit" => serde_json::to_value(Root::Inherit).unwrap(),
                _ => return Err(message("--root requires read-only or inherit")),
            },
            "private_tmp" => match value {
                "yes" => Value::Bool(true),
                "inherit" => Value::Bool(false),
                _ => return Err(message("--private-tmp requires yes or inherit")),
            },
            _ => serde_json::to_value(Writable::try_from(value.to_owned())?).unwrap(),
        };
        Ok((key.to_owned(), value))
    })())
}

pub fn insert(controls: &mut Controls, key: String, value: Value) -> Result<(), String> {
    let mut fields = controls.fields();
    if fields.get(&key).is_some_and(|v| !v.is_null()) {
        return Err(message("duplicate isolation control"));
    }
    fields.insert(key, value);
    *controls =
        serde_json::from_value(serde_json::to_value(fields).unwrap()).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn validate_job(job: &crate::model::Job) -> Result<(), String> {
    let applied = job
        .result
        .as_ref()
        .and_then(|r| r.isolation_controls.as_ref());
    if applied.is_some_and(|applied| {
        let has = |kind: &str| applied.namespaces.iter().any(|name| name == kind);
        let mounts = applied.root_read_only
            || !applied.private_tmp.is_empty()
            || !applied.writable.is_empty();
        applied.namespaces.is_empty()
            || applied
                .namespaces
                .iter()
                .any(|name| !KINDS.contains(&name.as_str()))
            || (mounts && !(has("mount") && has("user")))
            || (has("pid") != applied.pid_namespace.is_some())
            || (has("pid") && !has("mount"))
            || (!applied.writable.is_empty() && !applied.root_read_only)
            || applied
                .private_tmp
                .iter()
                .chain(&applied.writable)
                .any(|path| !Path::new(path).is_absolute())
    }) {
        return Err(message("invalid applied isolation controls"));
    }
    Ok(())
}

pub fn describe_host(info: &crate::model::HostInfo) -> Vec<String> {
    let list = |names: Vec<String>| {
        if names.is_empty() {
            message("none")
        } else {
            names.join(", ")
        }
    };
    let yes = |value: bool| message(if value { "yes" } else { "no" });
    let security = &info.security_controls;
    let process = &info.process_controls;
    let mut lines = vec![
        text(
            "security controls: no_new_privs {nnp}, seccomp deny lists {seccomp} ({syscalls} system calls), {caps} capability names",
            &[
                (
                    "nnp",
                    yes(security.no_new_privs_error.is_none()
                        && security.service_no_new_privs.is_some()),
                ),
                (
                    "seccomp",
                    yes(security.seccomp_query_error.is_none() && security.seccomp_arch.is_some()),
                ),
                ("syscalls", security.supported_syscalls.len().to_string()),
                ("caps", security.supported_cap_names.len().to_string()),
            ],
        ),
        text(
            "process controls: CPU affinity {affinity}, NUMA policy {numa}, {limits} resource limits",
            &[
                ("affinity", yes(process.allowed_cpus.is_some())),
                ("numa", yes(process.allowed_memory_nodes.is_some())),
                ("limits", process.inherited_rlimits.len().to_string()),
            ],
        ),
    ];
    let Some(isolation) = &info.isolation_controls else {
        return lines;
    };
    lines.push(text(
        "isolation: namespaces {kinds}; user namespaces without privilege {user}; read-only root and private tmp {mounts}; pid namespaces with an init and a fresh /proc {pid}",
        &[
            (
                "kinds",
                list(isolation.requestable_namespaces.clone()),
            ),
            ("user", yes(isolation.unprivileged_user_namespaces)),
            ("mounts", yes(isolation.mount_setattr)),
            ("pid", yes(isolation.pid_namespace_requestable)),
        ],
    ));
    lines.push(text(
        "isolation: Landlock ABI {landlock}; missing network tools: {tools}; IPv6 in a job's linked network {ipv6}",
        &[
            (
                "landlock",
                isolation
                    .landlock_abi
                    .map_or_else(|| message("none"), |abi| abi.to_string()),
            ),
            ("tools", list(isolation.missing_link_tools.clone())),
            ("ipv6", yes(isolation.ipv6_in_linked_network)),
        ],
    ));
    lines.push(text(
        "network namespaces: {modes}",
        &[(
            "modes",
            isolation
                .network_modes
                .iter()
                .map(|(name, mode)| {
                    format!(
                        "{name} {}",
                        message(match (mode.available, mode.namespace.as_str()) {
                            (false, _) => "unavailable",
                            (true, "host") => "shared with the host",
                            (true, _) if mode.link == "none" => "per job",
                            (true, _) => "per job behind a link per Queue or per job",
                        })
                    )
                })
                .collect::<Vec<_>>()
                .join("; "),
        )],
    ));
    lines
}
