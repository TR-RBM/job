use std::process::ExitCode;

use super::{NamespaceReport, ProfileReport, Report, Sharing, message, scope_words};
use crate::model::{Request, Response};

fn plain(key: &str) -> String {
    message(key, &[])
}

fn sharing(sharing: Sharing) -> &'static str {
    match sharing {
        Sharing::PerJob => "per-job",
        Sharing::PerQueue => "per-queue",
        Sharing::Shared => "shared",
    }
}

fn namespace_line(namespace: &NamespaceReport) -> String {
    let state = if namespace.joinable {
        plain("joinable")
    } else {
        plain("not joinable")
    };
    format!(
        "ns:{}  {}  {state}",
        namespace.name,
        namespace.path.display()
    )
}

fn profile_line(profile: &ProfileReport) -> String {
    let state = if profile.enforceable {
        plain("enforceable")
    } else {
        plain("not enforceable here")
    };
    format!(
        "profile:{}  egress {}  sharing {}  {state}",
        profile.name,
        profile.egress,
        sharing(profile.sharing)
    )
}

fn listed(report: &Report) -> String {
    let mut lines = Vec::new();
    if report.namespaces.is_empty() && report.profiles.is_empty() {
        lines.push(plain(
            "the service configuration names no network namespace and no network profile",
        ));
    }
    lines.extend(report.namespaces.iter().map(namespace_line));
    lines.extend(report.profiles.iter().map(profile_line));
    lines.join("\n")
}

fn rate(label: &str, value: Option<u64>, lines: &mut Vec<String>) {
    if let Some(value) = value {
        lines.push(format!("{label}: {}", crate::units::format_rate(value)));
    }
}

fn list(label: &str, values: &[String], lines: &mut Vec<String>) {
    if !values.is_empty() {
        lines.push(format!("{label}: {}", values.join(" ")));
    }
}

fn namespace_shown(namespace: &NamespaceReport) -> String {
    let mut lines = vec![namespace_line(namespace)];
    if let Some(description) = &namespace.description {
        lines.push(format!("{}: {description}", plain("description")));
    }
    if let Some(user) = &namespace.user_namespace {
        lines.push(format!("user_namespace: {}", user.display()));
    }
    if let Some(resolver) = &namespace.resolv_conf {
        lines.push(format!("resolv_conf: {}", resolver.display()));
    }
    lines.push(format!(
        "{}: {}",
        plain("network"),
        scope_words(namespace.scope)
    ));
    lines.push(plain(
        "job adds no filter, no name resolution and no bandwidth limit to this namespace",
    ));
    if let Some(reason) = &namespace.reason {
        lines.push(format!("{}: {reason}", plain("not joinable")));
    }
    lines.join("\n")
}

fn profile_shown(profile: &ProfileReport) -> String {
    let mut lines = vec![profile_line(profile)];
    if let Some(description) = &profile.description {
        lines.push(format!("{}: {description}", plain("description")));
    }
    lines.push(format!("sha256: {}", profile.digest));
    lines.push(format!(
        "{}: {}",
        plain("network"),
        scope_words(profile.scope)
    ));
    if let Some(path) = &profile.secret_file {
        lines.push(format!("secret_file: {}", path.display()));
    }
    rate("bandwidth", profile.bandwidth, &mut lines);
    rate("job_bandwidth", profile.job_bandwidth, &mut lines);
    list("dns", &profile.dns, &mut lines);
    list("allow", &profile.allow, &mut lines);
    list("deny", &profile.deny, &mut lines);
    if !profile.rules.is_empty() {
        lines.push(plain("packet filter, in order:"));
        lines.extend(profile.rules.iter().map(|rule| format!("  {rule}")));
    }
    if !profile.missing.is_empty() {
        lines.push(format!(
            "{}: {}",
            plain("not enforceable here"),
            profile.missing.join("; ")
        ));
    }
    lines.join("\n")
}

fn json<T: serde::Serialize>(value: &T) -> Result<ExitCode, String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|e| e.to_string())?
    );
    Ok(ExitCode::SUCCESS)
}

pub fn command(arguments: &[String]) -> Result<ExitCode, String> {
    let usage = || plain("usage: job net list [--json] | show NAME [--json]");
    let as_json = arguments.iter().any(|argument| argument == "--json");
    let words: Vec<&str> = arguments
        .iter()
        .map(String::as_str)
        .filter(|argument| *argument != "--json")
        .collect();
    if words.first() == Some(&"--help") {
        println!("{}", super::help());
        return Ok(ExitCode::SUCCESS);
    }
    let report = match (words.as_slice(), crate::call(Request::Host)?) {
        (["list"] | ["show", _], Response::Host { info }) => info.network.unwrap_or_default(),
        (["list"] | ["show", _], Response::Error { message }) => return Err(message),
        _ => return Err(usage()),
    };
    match words.as_slice() {
        ["list"] if as_json => json(&report),
        ["list"] => {
            println!("{}", listed(&report));
            Ok(ExitCode::SUCCESS)
        }
        ["show", name] => {
            let wanted = name
                .strip_prefix("ns:")
                .map(|name| (Some(true), name))
                .or_else(|| {
                    name.strip_prefix("profile:")
                        .map(|name| (Some(false), name))
                })
                .unwrap_or((None, name));
            let namespace = report
                .namespaces
                .iter()
                .find(|found| wanted.0 != Some(false) && found.name == wanted.1);
            let profile = report
                .profiles
                .iter()
                .find(|found| wanted.0 != Some(true) && found.name == wanted.1);
            match (namespace, profile) {
                (None, None) => Err(message(
                    "the service configuration names no network namespace and no network profile {name}; `job net list` shows what it names",
                    &[("name", (*name).to_string())],
                )),
                (Some(_), Some(_)) => Err(message(
                    "{name} is both a network namespace and a network profile; write ns:{name} or profile:{name}",
                    &[("name", (*name).to_string())],
                )),
                (Some(namespace), None) if as_json => json(namespace),
                (None, Some(profile)) if as_json => json(profile),
                (Some(namespace), None) => {
                    println!("{}", namespace_shown(namespace));
                    Ok(ExitCode::SUCCESS)
                }
                (None, Some(profile)) => {
                    println!("{}", profile_shown(profile));
                    Ok(ExitCode::SUCCESS)
                }
            }
        }
        _ => Err(usage()),
    }
}
