use crate::resource_policy::Origin;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub mod kernel;
mod messages;
mod seccomp;
pub use messages::{help, message};
pub use seccomp::NAMES as SYSCALLS;

pub const CAPABILITIES: &[&str] = &[
    "chown",
    "dac_override",
    "dac_read_search",
    "fowner",
    "fsetid",
    "kill",
    "setgid",
    "setuid",
    "setpcap",
    "linux_immutable",
    "net_bind_service",
    "net_broadcast",
    "net_admin",
    "net_raw",
    "ipc_lock",
    "ipc_owner",
    "sys_module",
    "sys_rawio",
    "sys_chroot",
    "sys_ptrace",
    "sys_pacct",
    "sys_admin",
    "sys_boot",
    "sys_nice",
    "sys_resource",
    "sys_time",
    "sys_tty_config",
    "mknod",
    "lease",
    "audit_write",
    "audit_control",
    "setfcap",
    "mac_override",
    "mac_admin",
    "syslog",
    "wake_alarm",
    "block_suspend",
    "audit_read",
    "perfmon",
    "bpf",
    "checkpoint_restore",
];

fn canonical(text: String, allowed: &[&str], all: bool) -> Result<String, String> {
    if text == "inherit" || (all && text == "all") {
        return Ok(text);
    }
    if text.len() > 8192 {
        return Err(message("invalid security control list"));
    }
    let names: BTreeSet<_> = text.split(',').collect();
    if names.len() > 128 || names.iter().any(|name| !allowed.contains(name)) {
        return Err(message("invalid security control list"));
    }
    Ok(names.into_iter().collect::<Vec<_>>().join(","))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CapDrop(String);
impl TryFrom<String> for CapDrop {
    type Error = String;
    fn try_from(text: String) -> Result<Self, String> {
        canonical(text, CAPABILITIES, true).map(Self)
    }
}
impl From<CapDrop> for String {
    fn from(value: CapDrop) -> Self {
        value.0
    }
}
impl CapDrop {
    pub fn active(&self) -> bool {
        self.0 != "inherit"
    }
    pub fn mask(&self, last: u32) -> Result<u64, String> {
        if last >= 64 {
            return Err(message("kernel capability range is unsupported"));
        }
        if self.0 == "all" {
            return Ok(u64::MAX >> (63 - last));
        }
        let mut mask = 0;
        for name in self.0.split(',').filter(|_| self.active()) {
            let bit = CAPABILITIES
                .iter()
                .position(|known| *known == name)
                .unwrap() as u32;
            if bit > last {
                return Err(message("requested capability is unavailable"));
            }
            mask |= 1u64 << bit;
        }
        Ok(mask)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Deny(String);
impl TryFrom<String> for Deny {
    type Error = String;
    fn try_from(text: String) -> Result<Self, String> {
        canonical(text, seccomp::NAMES, false).map(Self)
    }
}
impl From<Deny> for String {
    fn from(value: Deny) -> Self {
        value.0
    }
}
impl Deny {
    pub fn active(&self) -> bool {
        self.0 != "inherit"
    }
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.split(',').filter(|_| self.active())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Controls {
    pub no_new_privs: Option<bool>,
    pub cap_drop: Option<CapDrop>,
    pub seccomp_deny: Option<Deny>,
}
impl Controls {
    pub fn fields(&self) -> BTreeMap<String, Value> {
        serde_json::from_value(serde_json::to_value(self).unwrap()).unwrap()
    }
    pub fn from_settings(settings: &crate::model::Settings) -> Self {
        Self {
            no_new_privs: settings.job_no_new_privs,
            cap_drop: settings.job_cap_drop.clone(),
            seccomp_deny: settings.job_seccomp_deny.clone(),
        }
    }
    pub fn requires_nnp(&self) -> bool {
        self.no_new_privs == Some(true)
            || self.cap_drop.as_ref().is_some_and(CapDrop::active)
            || self.seccomp_deny.as_ref().is_some_and(Deny::active)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.no_new_privs == Some(false) && self.requires_nnp() {
            return Err(message(
                "capability reduction and seccomp require no_new_privs",
            ));
        }
        Ok(())
    }
}

pub fn resolve(
    declared: &mut crate::model::Declared,
    effective: &BTreeMap<String, crate::objects::Effective>,
) -> Result<BTreeMap<String, Origin>, String> {
    let mut fields = declared.security.fields();
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
    declared.security =
        serde_json::from_value(serde_json::to_value(fields).unwrap()).map_err(|e| e.to_string())?;
    declared.security.validate()?;
    Ok(sources)
}

pub fn parse_option(option: &str, value: &str) -> Option<Result<(String, Value), String>> {
    let key = match option {
        "no-new-privs" => "no_new_privs",
        "cap-drop" => "cap_drop",
        "seccomp-deny" => "seccomp_deny",
        _ => return None,
    };
    Some((|| {
        let value = match key {
            "no_new_privs" => match value {
                "yes" => Value::Bool(true),
                "inherit" => Value::Bool(false),
                _ => return Err(message("no-new-privs requires yes or inherit")),
            },
            "cap_drop" => serde_json::to_value(CapDrop::try_from(value.to_owned())?).unwrap(),
            _ => serde_json::to_value(Deny::try_from(value.to_owned())?).unwrap(),
        };
        Ok((key.to_owned(), value))
    })())
}

pub fn validate_job(job: &crate::model::Job) -> Result<(), String> {
    for spec in std::iter::once(&job.spec)
        .chain(job.submitted_spec.iter())
        .chain(job.requested_spec.iter())
        .chain(job.effective_spec.iter())
    {
        spec.declared.security.validate()?;
    }
    let applied = job
        .result
        .as_ref()
        .and_then(|r| r.security_controls.as_ref());
    if applied.is_some_and(|applied| {
        !applied.no_new_privs
            || applied.cap_drop.as_ref().is_some_and(|v| !v.active())
            || (applied.cap_bounding_reduced && applied.cap_drop.is_none())
            || applied.seccomp_deny.as_ref().is_some_and(|v| !v.active())
            || applied.seccomp_deny.is_some() != applied.seccomp_arch.is_some()
            || applied
                .seccomp_arch
                .is_some_and(|arch| ![0xc000003e, 0xc00000b7].contains(&arch))
    }) {
        return Err(message("invalid applied security controls"));
    }
    Ok(())
}
