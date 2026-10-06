mod cli;
mod join;
mod messages;
mod rule;

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::model::{Declared, Job, Net, Settings};

pub use cli::command;
pub use join::Joined;
pub use messages::{help, message};
use rule::Rule;

const DIRECTORY: &str = "/run/netns";
const MOST: usize = 64;
const RESOLVERS: usize = 3;
const NET_KEY: &str = "net";
const ESTABLISHED: &str = "ct state established,related accept";
const REJECT: &str = "iifname \"br0\" reject with icmp type admin-prohibited";

pub fn named(name: &str) -> Result<String, String> {
    let valid = !name.is_empty()
        && name.len() <= MOST
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if valid {
        Ok(name.to_string())
    } else {
        Err(message(
            "`{name}` is not a name: use up to 64 letters, digits, - and _",
            &[("name", name.to_string())],
        ))
    }
}

pub fn describe(kind: &str, name: &str) -> String {
    let key = if kind == "ns" {
        "the network namespace {name}"
    } else {
        "the network profile {name}"
    };
    message(key, &[("name", name.to_string())])
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Sharing {
    #[default]
    PerJob,
    PerQueue,
    Shared,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Host,
    Job,
    Queue,
    Profile,
    Namespace,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Namespace {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_namespace: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolv_conf: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub egress: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_bandwidth: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dns: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharing: Option<Sharing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace_directory: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub namespaces: BTreeMap<String, Namespace>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, Profile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub name: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_namespace: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolv_conf: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    pub selected: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharing: Option<Sharing>,
    pub egress: Net,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<Place>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_bandwidth: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dns: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
}

pub struct Route {
    pub name: String,
    pub rate: Option<u64>,
    pub cap: Option<u64>,
    pub egress: Net,
    pub filter: crate::link::Filter,
    pub resolver: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamespaceReport {
    pub name: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_namespace: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolv_conf: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub scope: Scope,
    pub joinable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileReport {
    pub name: String,
    pub digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub egress: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_file: Option<PathBuf>,
    pub sharing: Sharing,
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_bandwidth: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dns: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
    pub enforceable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    #[serde(default)]
    pub namespaces: Vec<NamespaceReport>,
    #[serde(default)]
    pub profiles: Vec<ProfileReport>,
}

struct Plan {
    net: Net,
    rate: Option<u64>,
    job_rate: Option<u64>,
    dns: Vec<Ipv4Addr>,
    allow: Vec<Rule>,
    deny: Vec<Rule>,
    sharing: Sharing,
}

impl Plan {
    fn linked(&self) -> bool {
        match self.net {
            Net::Proxy(_) | Net::WireGuard(_) => true,
            Net::Host => {
                self.rate.is_some()
                    || self.job_rate.is_some()
                    || !self.dns.is_empty()
                    || !self.allow.is_empty()
                    || !self.deny.is_empty()
            }
            _ => false,
        }
    }

    fn scope(&self) -> Scope {
        match (&self.net, self.linked(), self.sharing) {
            (Net::Namespace(_), _, _) => Scope::Namespace,
            (Net::None, _, _) => Scope::Job,
            (_, false, _) => Scope::Host,
            (_, true, Sharing::PerJob) => Scope::Job,
            (_, true, Sharing::PerQueue) => Scope::Queue,
            (_, true, Sharing::Shared) => Scope::Profile,
        }
    }

    fn filter(&self) -> crate::link::Filter {
        crate::link::Filter {
            deny: self.deny.iter().map(Rule::matcher).collect(),
            allow: self.allow.iter().map(Rule::matcher).collect(),
        }
    }

    fn rules(&self) -> Vec<String> {
        if !self.linked() {
            return Vec::new();
        }
        let base = match &self.net {
            Net::Proxy(url) => match crate::link::proxy_target(url) {
                Ok(target) => format!(
                    "iifname \"br0\" ip daddr {} tcp dport {} accept;",
                    target.host, target.port
                ),
                Err(_) => return Vec::new(),
            },
            Net::WireGuard(_) => crate::link::TUNNEL_GATE.to_string(),
            _ => crate::link::HOST_GATE.to_string(),
        };
        std::iter::once(ESTABLISHED.to_string())
            .chain(
                crate::link::gate(&base, &self.filter())
                    .split(';')
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string),
            )
            .chain(std::iter::once(REJECT.to_string()))
            .collect()
    }

    fn missing(&self) -> Vec<String> {
        let mut missing: Vec<String> = Vec::new();
        if self.linked() {
            missing.extend(crate::link::missing_tools().into_iter().map(str::to_string));
        }
        if (self.linked() || self.net == Net::None) && !crate::host::user_namespaces() {
            missing.push(message("user namespaces", &[]));
        }
        missing
    }
}

fn lexical(path: &Path) -> bool {
    path.is_absolute()
        && !path.starts_with("/proc")
        && path
            .components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
}

fn shown(path: &Path) -> String {
    path.display().to_string()
}

impl Network {
    pub fn unset(&self) -> bool {
        *self == Self::default()
    }

    fn directory(&self) -> PathBuf {
        self.namespace_directory
            .clone()
            .unwrap_or_else(|| PathBuf::from(DIRECTORY))
    }

    fn place(&self, name: &str, namespace: &Namespace) -> Result<Place, String> {
        let path = match &namespace.path {
            Some(path) => self.directory().join(path),
            None => self.directory().join(name),
        };
        let stable = |field: &str, path: &Path| {
            if lexical(path) {
                Ok(())
            } else {
                Err(message(
                    "network namespace {name}: {field} = {path} must be an absolute path without `..` and outside /proc; a path under /proc names a process, not a namespace",
                    &[
                        ("name", name.to_string()),
                        ("field", field.to_string()),
                        ("path", shown(path)),
                    ],
                ))
            }
        };
        stable("path", &path)?;
        if let Some(user) = &namespace.user_namespace {
            stable("user_namespace", user)?;
        }
        if let Some(resolver) = &namespace.resolv_conf
            && !resolver.is_absolute()
        {
            return Err(message(
                "network namespace {name}: resolv_conf = {path} must be an absolute path",
                &[("name", name.to_string()), ("path", shown(resolver))],
            ));
        }
        Ok(Place {
            name: name.to_string(),
            path,
            user_namespace: namespace.user_namespace.clone(),
            resolv_conf: namespace.resolv_conf.clone(),
        })
    }

    fn namespace(&self, name: &str) -> Result<Place, String> {
        match self.namespaces.get(name) {
            Some(namespace) => self.place(name, namespace),
            None => Err(message(
                "there is no network namespace {name} in the service configuration; `job net list` shows what it names",
                &[("name", name.to_string())],
            )),
        }
    }

    fn profile(&self, name: &str) -> Result<(&Profile, Plan), String> {
        match self.profiles.get(name) {
            Some(profile) => Ok((profile, self.plan(name, profile)?)),
            None => Err(message(
                "there is no network profile {name} in the service configuration; `job net list` shows what it names",
                &[("name", name.to_string())],
            )),
        }
    }

    fn plan(&self, name: &str, profile: &Profile) -> Result<Plan, String> {
        let said = |key: &str, values: &[(&str, String)]| {
            format!(
                "{}: {}",
                message("network profile {name}", &[("name", name.to_string())]),
                message(key, values)
            )
        };
        let net =
            Net::parse(&profile.egress).map_err(|error| said("{error}", &[("error", error)]))?;
        let unfit = |field: &str, key: &str| {
            Err(said(
                key,
                &[
                    ("field", field.to_string()),
                    ("egress", crate::netsecret::redact(&profile.egress)),
                ],
            ))
        };
        let rate = |field: &str, text: &Option<String>| match text {
            Some(text) => crate::units::parse_rate(text).map(Some).map_err(|error| {
                said(
                    "{field}: {error}",
                    &[("field", field.to_string()), ("error", error)],
                )
            }),
            None => Ok(None),
        };
        let mut plan = Plan {
            net,
            rate: rate("bandwidth", &profile.bandwidth)?,
            job_rate: rate("job_bandwidth", &profile.job_bandwidth)?,
            dns: Vec::new(),
            allow: Vec::new(),
            deny: Vec::new(),
            sharing: profile.sharing.unwrap_or_default(),
        };
        if profile.dns.len() > RESOLVERS || profile.allow.len() > MOST || profile.deny.len() > MOST
        {
            return Err(said(
                "at most 3 dns addresses and 64 allow and 64 deny rules",
                &[],
            ));
        }
        for text in &profile.dns {
            plan.dns.push(
                rule::address(text).map_err(|error| said("dns: {error}", &[("error", error)]))?,
            );
        }
        for text in &profile.allow {
            plan.allow.push(
                Rule::parse(text).map_err(|error| said("allow: {error}", &[("error", error)]))?,
            );
        }
        for text in &profile.deny {
            plan.deny.push(
                Rule::parse(text).map_err(|error| said("deny: {error}", &[("error", error)]))?,
            );
        }
        let shaped = plan.rate.is_some() || plan.job_rate.is_some();
        let filtered = !plan.allow.is_empty() || !plan.deny.is_empty();
        match &plan.net {
            Net::Host | Net::WireGuard(_) => {
                if profile.secret_file.is_some() {
                    return unfit(
                        "secret_file",
                        "{field} belongs to a proxy; egress {egress} takes no user and password",
                    );
                }
            }
            Net::Proxy(url) => {
                if crate::netsecret::split(url).1.is_some() {
                    return Err(said(
                        "egress carries a user and password; put them in a file and name it with secret_file",
                        &[],
                    ));
                }
                crate::netsecret::check(Some(&plan.net), profile.secret_file.as_deref())
                    .map_err(|error| said("{error}", &[("error", error)]))?;
                if !plan.dns.is_empty() {
                    return unfit(
                        "dns",
                        "{field} cannot be set with egress {egress}: names are resolved by the proxy, and the Job reaches nothing but the proxy",
                    );
                }
                if filtered {
                    return unfit(
                        "allow and deny",
                        "{field} cannot be set with egress {egress}: the Job reaches nothing but the proxy, so filter destinations at the proxy",
                    );
                }
            }
            Net::None | Net::Namespace(_) => {
                let reason = if plan.net == Net::None {
                    "{field} cannot be set with egress {egress}: there is no network to shape, filter or resolve names on"
                } else {
                    "{field} cannot be set with egress {egress}: job does not own that namespace's devices, filter or resolver"
                };
                for (field, set) in [
                    ("secret_file", profile.secret_file.is_some()),
                    ("bandwidth", shaped),
                    ("dns", !plan.dns.is_empty()),
                    ("allow and deny", filtered),
                    ("sharing", profile.sharing.is_some()),
                ] {
                    if set {
                        return unfit(field, reason);
                    }
                }
                if let Net::Namespace(namespace) = &plan.net {
                    self.namespace(namespace)
                        .map_err(|error| said("{error}", &[("error", error)]))?;
                }
            }
            Net::OpenVpn(_) => {
                return Err(said("openvpn: not yet; use wireguard:FILE or a proxy", &[]));
            }
            Net::Profile(_) => {
                return Err(said("egress cannot name another profile", &[]));
            }
        }
        if let Net::WireGuard(path) = &plan.net
            && !path.is_absolute()
        {
            return Err(said("egress wireguard:FILE needs an absolute path", &[]));
        }
        if let (Some(each), Some(all)) = (plan.job_rate, plan.rate)
            && each > all
        {
            return Err(said("job_bandwidth is wider than bandwidth", &[]));
        }
        if plan.job_rate.is_some() && plan.rate.is_none() && plan.sharing != Sharing::PerJob {
            return Err(said(
                "job_bandwidth without bandwidth needs sharing = \"per-job\"; a shared network holds its Jobs within one bandwidth",
                &[],
            ));
        }
        Ok(plan)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.namespaces.len() > MOST || self.profiles.len() > MOST {
            return Err(message(
                "at most 64 network namespaces and 64 network profiles",
                &[],
            ));
        }
        if let Some(directory) = &self.namespace_directory
            && !lexical(directory)
        {
            return Err(message(
                "namespace_directory = {path} must be an absolute path without `..` and outside /proc",
                &[("path", shown(directory))],
            ));
        }
        for (name, namespace) in &self.namespaces {
            named(name)?;
            self.place(name, namespace)?;
        }
        for (name, profile) in &self.profiles {
            named(name)?;
            self.plan(name, profile)?;
        }
        Ok(())
    }

    fn known(&self, net: &Net) -> Result<(), String> {
        match net {
            Net::Namespace(name) => self.namespace(name).map(|_| ()),
            Net::Profile(name) => self.profile(name).map(|_| ()),
            _ => Ok(()),
        }
    }

    pub fn validate_presets(&self, catalog: &crate::presets::Catalog) -> Result<(), String> {
        for profile in &catalog.profiles {
            if let Some(value) = profile.values.get(NET_KEY)
                && let Ok(net) = serde_json::from_value::<Net>(preset_value(NET_KEY, value))
            {
                self.known(&net).map_err(|error| {
                    format!("profile:{}@{}: {error}", profile.name, profile.revision)
                })?;
            }
        }
        Ok(())
    }

    pub fn validate_graph(&self, graph: &crate::objects::Graph) -> Result<(), String> {
        for node in graph.nodes.values() {
            if node.config.get("on").is_some_and(|on| !on.is_null()) {
                continue;
            }
            if let Some(net) = node
                .config
                .get(NET_KEY)
                .and_then(|value| serde_json::from_value::<Option<Net>>(value.clone()).ok())
                .flatten()
            {
                self.known(&net).map_err(|error| {
                    message(
                        "{path} is set to {net}: {error}",
                        &[
                            ("path", graph.view(node.id).path),
                            ("net", selection(&net)),
                            ("error", error),
                        ],
                    )
                })?;
            }
        }
        Ok(())
    }

    pub fn settle(&self, settings: &Settings) -> Result<(), String> {
        let Some(net) = settings
            .net
            .as_ref()
            .filter(|net| net.named() && settings.on.is_none())
        else {
            return Ok(());
        };
        self.known(net)?;
        if settings.net_secret_file.is_some() {
            return Err(message(
                "--net-secret-file cannot be set beside {net}: a profile names its own secret_file, and a namespace takes none",
                &[("net", selection(net))],
            ));
        }
        if settings.bandwidth.is_some() || settings.job_bandwidth.is_some() {
            return Err(unshaped(net));
        }
        Ok(())
    }
}

fn unshaped(net: &Net) -> String {
    message(
        "a Queue's bandwidth cannot be set beside {net}: a profile sets its own bandwidth, and job does not own the devices of a namespace it joins",
        &[("net", selection(net))],
    )
}

fn selection(net: &Net) -> String {
    match net {
        Net::Namespace(name) => format!("ns:{name}"),
        Net::Profile(name) => format!("profile:{name}"),
        other => other.describe(),
    }
}

pub fn preset_value(key: &str, value: &Value) -> Value {
    match value.as_str().filter(|_| key == NET_KEY) {
        Some(text) if text.starts_with("ns:") || text.starts_with("profile:") => Net::parse(text)
            .ok()
            .and_then(|net| serde_json::to_value(net).ok())
            .unwrap_or_else(|| value.clone()),
        _ => value.clone(),
    }
}

pub fn preset_net(value: &Value) -> bool {
    value == "Host"
        || value == "None"
        || serde_json::from_value::<Net>(preset_value(NET_KEY, value)).is_ok_and(|net| net.named())
}

impl Profile {
    pub fn digest(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(self).unwrap_or_default())
        )
    }
}

fn joined(place: &Place) -> Result<Joined, String> {
    let fault = |path: &Path, kind: &str, fault: join::Fault| {
        let values = [
            ("name", place.name.clone()),
            ("path", shown(path)),
            ("kind", kind.to_string()),
        ];
        match fault {
            join::Fault::Unreadable(error) => message(
                "network namespace {name}: cannot open {path}: {error}",
                &[
                    ("name", place.name.clone()),
                    ("path", shown(path)),
                    ("error", error.to_string()),
                ],
            ),
            join::Fault::NotNamespace => message(
                "network namespace {name}: {path} is not a namespace file",
                &values,
            ),
            join::Fault::OtherKind => message(
                "network namespace {name}: {path} is a namespace, but not a {kind} namespace",
                &values,
            ),
        }
    };
    let check = |path: &Path, kind: join::Kind, word: &str| {
        join::open(path, kind)
            .map(|_| ())
            .map_err(|error| fault(path, word, error))
    };
    check(&place.path, join::Kind::Network, "network")?;
    if let Some(user) = &place.user_namespace {
        check(user, join::Kind::User, "user")?;
    }
    if let Some(resolver) = &place.resolv_conf
        && !resolver.is_file()
    {
        return Err(message(
            "network namespace {name}: resolv_conf = {path} is not a file",
            &[("name", place.name.clone()), ("path", shown(resolver))],
        ));
    }
    Joined::prepare(
        &place.path,
        place.user_namespace.as_deref(),
        place.resolv_conf.as_deref(),
    )
    .map_err(|error| fault(&place.path, "network", error))
}

fn refusal(place: &Place, refusal: join::Refusal) -> String {
    let error = std::io::Error::from_raw_os_error(refusal.errno).to_string();
    let values = [("name", place.name.clone()), ("error", error)];
    let denied = refusal.errno == libc::EPERM;
    let key = match (refusal.step, denied, place.user_namespace.is_some()) {
        (join::Step::User, true, _) => {
            "network namespace {name}: the service may not join the user namespace named in user_namespace: it was made by another user, or the service is not in its parent ({error})"
        }
        (join::Step::User, _, _) => {
            "network namespace {name}: cannot join the user namespace named in user_namespace: {error}"
        }
        (join::Step::Network, true, false) => {
            "network namespace {name}: the service may not join this namespace: it belongs to a user namespace the service is not in; run the service with the needed privilege or give user_namespace ({error})"
        }
        (join::Step::Network, true, true) => {
            "network namespace {name}: the service may not join this namespace: it does not belong to the user namespace named in user_namespace ({error})"
        }
        (join::Step::Network, _, _) => "network namespace {name}: cannot join it: {error}",
        (join::Step::Verify, _, _) => {
            "network namespace {name}: after joining, the process was not in the namespace the configuration names ({error})"
        }
        (join::Step::Resolver, _, _) => {
            "network namespace {name}: cannot put resolv_conf over /etc/resolv.conf: {error}"
        }
        (join::Step::Identity, _, _) => {
            "network namespace {name}: cannot return to the service's user inside the namespace: {error}"
        }
    };
    message(key, &values)
}

pub fn enter(place: &Place) -> Result<Joined, String> {
    let joined = joined(place)?;
    joined.probe().map_err(|error| refusal(place, error))?;
    Ok(joined)
}

pub fn admit(
    network: &Network,
    declared: &Declared,
    settings: &Settings,
    queue: &str,
) -> Result<(), String> {
    let Some(net) = &declared.net else {
        return Ok(());
    };
    if let Some(theirs) = settings.net.as_ref().filter(|theirs| theirs.named())
        && net != theirs
    {
        return Err(message(
            "queue {queue}'s jobs have {theirs}; this job asks for {mine}; run it outside the queue",
            &[
                ("queue", queue.to_string()),
                ("theirs", theirs.describe()),
                ("mine", net.describe()),
            ],
        ));
    }
    if !net.named() {
        return Ok(());
    }
    if settings.bandwidth.is_some() || settings.job_bandwidth.is_some() {
        return Err(unshaped(net));
    }
    let foreign = |name: &str| {
        if declared.bandwidth.is_some() {
            return Err(message(
                "--bandwidth cannot be used with ns:{name}: job does not own that namespace's devices, so it cannot shape them",
                &[("name", name.to_string())],
            ));
        }
        enter(&network.namespace(name)?).map(|_| ())
    };
    match net {
        Net::Namespace(name) => foreign(name),
        Net::Profile(name) => {
            let (profile, plan) = network.profile(name)?;
            let values = [("name", name.clone())];
            match (&plan.net, &profile.secret_file, &declared.net_secret_file) {
                (_, Some(_), Some(_)) => {
                    return Err(message(
                        "--net-secret-file cannot be used with profile:{name}: the profile names its own secret_file",
                        &values,
                    ));
                }
                (Net::Proxy(_), Some(path), None) => {
                    crate::netsecret::read(path)?;
                }
                (Net::Proxy(_), None, _) => {}
                (_, _, Some(_)) => {
                    return Err(message(
                        "--net-secret-file belongs to a proxy; profile:{name} does not lead through one",
                        &values,
                    ));
                }
                _ => {}
            }
            if let Some(asked) = declared.bandwidth {
                let limit = plan.job_rate.or(plan.rate);
                if !plan.linked() && plan.net != Net::Host {
                    return Err(message(
                        "--bandwidth cannot be used with profile:{name}: its network is not one job can shape",
                        &values,
                    ));
                }
                if let Some(limit) = limit.filter(|limit| asked > *limit) {
                    return Err(message(
                        "--bandwidth {asked} is wider than the {limit} profile:{name} allows a Job",
                        &[
                            ("name", name.clone()),
                            ("asked", crate::units::format_rate(asked)),
                            ("limit", crate::units::format_rate(limit)),
                        ],
                    ));
                }
                if plan.rate.is_none() && plan.sharing != Sharing::PerJob {
                    return Err(message(
                        "--bandwidth cannot be used with profile:{name}: its network is shared and the profile sets no bandwidth to divide",
                        &values,
                    ));
                }
            }
            let mut missing = plan.missing();
            if declared.bandwidth.is_some() && !plan.linked() {
                missing.extend(crate::link::missing_tools().into_iter().map(str::to_string));
            }
            if !missing.is_empty() {
                return Err(message(
                    "this host cannot enforce profile:{name}: it lacks {missing}",
                    &[("name", name.clone()), ("missing", missing.join(", "))],
                ));
            }
            match &plan.net {
                Net::Namespace(namespace) => foreign(namespace),
                Net::WireGuard(path) => crate::wg::read(path).map(|_| ()),
                _ => Ok(()),
            }
        }
        _ => Ok(()),
    }
}

pub fn apply(network: &Network, job: &Job) -> Result<Option<Applied>, String> {
    let declared = &job.spec.declared;
    if declared.on.is_some() {
        return Ok(None);
    }
    let gone = |error: String| {
        message(
            "{error}; it was there when the Job was submitted, and the Job cannot start without it",
            &[("error", error)],
        )
    };
    let plain = |selected: String, scope: Scope, egress: Net| Applied {
        selected,
        profile: None,
        digest: None,
        scope,
        sharing: None,
        egress,
        holder: None,
        namespace: None,
        secret_file: None,
        bandwidth: None,
        job_bandwidth: None,
        cap: None,
        dns: Vec::new(),
        allow: Vec::new(),
        deny: Vec::new(),
        rules: Vec::new(),
    };
    match &declared.net {
        Some(Net::Namespace(name)) => {
            let place = network.namespace(name).map_err(gone)?;
            Ok(Some(Applied {
                namespace: Some(place),
                ..plain(
                    format!("ns:{name}"),
                    Scope::Namespace,
                    Net::Namespace(name.clone()),
                )
            }))
        }
        Some(Net::Profile(name)) => {
            let (profile, plan) = network.profile(name).map_err(gone)?;
            let digest = profile.digest();
            let short = &digest[..8];
            let own = declared.bandwidth;
            let linked = plan.linked() || own.is_some();
            let scope = if linked && plan.scope() == Scope::Host {
                Scope::Job
            } else {
                plan.scope()
            };
            let holder = linked.then(|| match scope {
                Scope::Queue => format!(
                    "profile@{name}@{short}@q{}",
                    job.queue_id.unwrap_or(crate::objects::DEFAULT_QUEUE)
                ),
                Scope::Profile => format!("profile@{name}@{short}"),
                _ => format!("job-{}", job.id),
            });
            let cap = [own, plan.job_rate, plan.rate].into_iter().flatten().min();
            let namespace = match &plan.net {
                Net::Namespace(namespace) => Some(network.namespace(namespace).map_err(gone)?),
                _ => None,
            };
            Ok(Some(Applied {
                profile: Some(name.clone()),
                digest: Some(digest),
                sharing: linked.then_some(plan.sharing),
                holder,
                namespace,
                secret_file: profile.secret_file.clone(),
                bandwidth: plan.rate,
                job_bandwidth: plan.job_rate,
                cap,
                dns: plan.dns.iter().map(Ipv4Addr::to_string).collect(),
                allow: plan.allow.iter().map(Rule::canonical).collect(),
                deny: plan.deny.iter().map(Rule::canonical).collect(),
                rules: plan.rules(),
                ..plain(format!("profile:{name}"), scope, plan.net.clone())
            }))
        }
        _ => Ok(None),
    }
}

pub fn view(network: &Network, job: &Job) -> Option<Applied> {
    if job.state.editable() || job.network.is_none() {
        apply(network, job).ok().flatten().or(job.network.clone())
    } else {
        job.network.clone()
    }
}

impl Applied {
    pub fn route(&self) -> Option<Route> {
        let name = self.holder.clone()?;
        let rules = |texts: &[String]| {
            texts
                .iter()
                .filter_map(|text| Rule::parse(text).ok())
                .map(|rule| rule.matcher())
                .collect()
        };
        let resolver: String = self
            .dns
            .iter()
            .filter_map(|text| text.parse::<Ipv4Addr>().ok())
            .map(|address| {
                if address.is_loopback() {
                    crate::link::HOST_LOOPBACK
                } else {
                    address
                }
            })
            .map(|address| format!("nameserver {address}\n"))
            .collect();
        Some(Route {
            rate: match self.scope {
                Scope::Job => self.bandwidth.or(self.cap),
                _ => self.bandwidth,
            },
            cap: self.cap,
            egress: self.egress.clone(),
            filter: crate::link::Filter {
                deny: rules(&self.deny),
                allow: rules(&self.allow),
            },
            resolver: (!resolver.is_empty()).then_some(resolver),
            name,
        })
    }

    pub fn isolated(&self) -> bool {
        self.egress == Net::None
    }

    pub fn secret(&self, declared: &Declared) -> Declared {
        let mut declared = declared.clone();
        if declared.net_secret_file.is_none() {
            declared.net_secret_file = self.secret_file.clone();
        }
        declared
    }

    pub fn describe(&self) -> String {
        let mut parts = vec![self.selected.clone()];
        if let Some(digest) = &self.digest {
            parts.push(format!("sha256 {}", &digest[..digest.len().min(12)]));
        }
        parts.push(scope_words(self.scope));
        for (label, values) in [
            ("dns", &self.dns),
            ("allow", &self.allow),
            ("deny", &self.deny),
        ] {
            if !values.is_empty() {
                parts.push(format!("{label} {}", values.join(" ")));
            }
        }
        parts.join("; ")
    }
}

fn scope_words(scope: Scope) -> String {
    message(
        match scope {
            Scope::Host => "the host's network, shared with everything on the host",
            Scope::Job => "one network per Job",
            Scope::Queue => {
                "one exit and rate budget per Queue; each Job in a network namespace of its own"
            }
            Scope::Profile => {
                "one exit and rate budget for all Jobs of the profile; each Job in a network namespace of its own"
            }
            Scope::Namespace => {
                "an existing network namespace, shared with every Job that selects it and with whatever else is in it"
            }
        },
        &[],
    )
}

pub fn report(network: &Network) -> Option<Report> {
    if network.unset() {
        return None;
    }
    let namespaces = network
        .namespaces
        .iter()
        .filter_map(|(name, namespace)| {
            let place = network.place(name, namespace).ok()?;
            let reason = enter(&place).err();
            Some(NamespaceReport {
                name: name.clone(),
                path: place.path,
                user_namespace: place.user_namespace,
                resolv_conf: place.resolv_conf,
                description: namespace.description.clone(),
                scope: Scope::Namespace,
                joinable: reason.is_none(),
                reason,
            })
        })
        .collect();
    let profiles = network
        .profiles
        .iter()
        .filter_map(|(name, profile)| {
            let plan = network.plan(name, profile).ok()?;
            let mut missing = plan.missing();
            if let Net::Namespace(namespace) = &plan.net
                && let Err(reason) = network.namespace(namespace).and_then(|place| enter(&place))
            {
                missing.push(reason);
            }
            if let Some(path) = &profile.secret_file
                && let Err(reason) = crate::netsecret::read(path)
            {
                missing.push(reason);
            }
            Some(ProfileReport {
                name: name.clone(),
                digest: profile.digest(),
                description: profile.description.clone(),
                egress: crate::netsecret::redact(&profile.egress),
                secret_file: profile.secret_file.clone(),
                sharing: plan.sharing,
                scope: plan.scope(),
                bandwidth: plan.rate,
                job_bandwidth: plan.job_rate,
                dns: plan.dns.iter().map(Ipv4Addr::to_string).collect(),
                allow: plan.allow.iter().map(Rule::canonical).collect(),
                deny: plan.deny.iter().map(Rule::canonical).collect(),
                rules: plan.rules(),
                enforceable: missing.is_empty(),
                missing,
            })
        })
        .collect();
    Some(Report {
        namespaces,
        profiles,
    })
}

pub fn findings(report: Option<&Report>) -> Vec<(&'static str, bool, String, String)> {
    let Some(report) = report else {
        return Vec::new();
    };
    let mut found = Vec::new();
    if !report.namespaces.is_empty() {
        let detail: Vec<String> = report
            .namespaces
            .iter()
            .map(|namespace| match &namespace.reason {
                Some(reason) => reason.clone(),
                None => message(
                    "network namespace {name}: joinable",
                    &[("name", namespace.name.clone())],
                ),
            })
            .collect();
        found.push((
            "network_namespaces",
            report.namespaces.iter().all(|namespace| namespace.joinable),
            detail.join("; "),
            message(
                "prepare the namespace or correct [network.namespaces] in the service configuration; Jobs that do not select it are not affected",
                &[],
            ),
        ));
    }
    if !report.profiles.is_empty() {
        let detail: Vec<String> = report
            .profiles
            .iter()
            .map(|profile| {
                if profile.enforceable {
                    format!("profile:{}: {}", profile.name, message("enforceable", &[]))
                } else {
                    format!(
                        "profile:{}: {}: {}",
                        profile.name,
                        message("not enforceable here", &[]),
                        profile.missing.join("; ")
                    )
                }
            })
            .collect();
        found.push((
            "network_profiles",
            report.profiles.iter().all(|profile| profile.enforceable),
            detail.join("; "),
            message(
                "install what is missing or correct [network.profiles] in the service configuration; Jobs that do not select the profile are not affected",
                &[],
            ),
        ));
    }
    found
}
