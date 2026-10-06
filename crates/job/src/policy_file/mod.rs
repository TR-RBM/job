mod messages;
pub use messages::message;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::paths;
use crate::policy::{Policy, RULE_NAMES};

#[derive(Clone, Debug)]
pub struct Loaded {
    pub policy: Policy,
    pub file: Option<PathBuf>,
    pub searched: Vec<PathBuf>,
}

fn shown(path: &Path) -> String {
    path.display().to_string()
}

fn refuse(path: &Path, key: &str, values: &[(&str, String)]) -> String {
    format!("{}: {}", shown(path), message(key, values))
}

pub fn parse(text: &str, path: &Path) -> Result<Policy, String> {
    let policy: Policy = serde_json::from_str(text).map_err(|error| {
        refuse(
            path,
            "not a valid policy file: {error}",
            &[("error", error.to_string())],
        )
    })?;
    let mut names: Vec<&String> = policy.rules.keys().collect();
    names.sort();
    for name in names {
        if !RULE_NAMES.contains(&name.as_str()) {
            return Err(refuse(
                path,
                "unknown rule {rule}; the rules are {rules}",
                &[("rule", name.clone()), ("rules", RULE_NAMES.join(", "))],
            ));
        }
        let rule = &policy.rules[name];
        if rule.forbids.trim().is_empty() || rule.instead.trim().is_empty() {
            return Err(refuse(
                path,
                "rule {rule} needs a text for forbids and for instead",
                &[("rule", name.clone())],
            ));
        }
    }
    for (field, value) in [
        ("work_root", &policy.work_root),
        ("repos_root", &policy.repos_root),
    ] {
        if value.as_ref().is_some_and(|path| !path.is_absolute()) {
            return Err(refuse(
                path,
                "{field} must be an absolute path",
                &[("field", field.to_owned())],
            ));
        }
    }
    if policy.rules.contains_key("other-worktree")
        && policy.work_root.is_none()
        && policy.repos_root.is_none()
    {
        return Err(refuse(
            path,
            "rule other-worktree needs work_root or repos_root",
            &[],
        ));
    }
    if policy.rules.contains_key("rm-protected") && policy.protected_paths.is_empty() {
        return Err(refuse(path, "rule rm-protected needs protected_paths", &[]));
    }
    Ok(policy)
}

pub fn load() -> Result<Loaded, String> {
    let searched = paths::policy_files(paths::mode(&paths::process), &paths::process);
    for path in &searched {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                return Ok(Loaded {
                    policy: parse(&text, path)?,
                    file: Some(path.clone()),
                    searched,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(refuse(
                    path,
                    "cannot read the policy file: {error}",
                    &[("error", error.to_string())],
                ));
            }
        }
    }
    Ok(Loaded {
        policy: Policy::builtin(),
        file: None,
        searched,
    })
}

fn places(searched: &[PathBuf]) -> String {
    searched
        .iter()
        .map(|path| shown(path))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn describe(loaded: &Loaded) -> String {
    match &loaded.file {
        None => message(
            "no policy file at {places}; the command policy forbids nothing",
            &[("places", places(&loaded.searched))],
        ),
        Some(path) => {
            let active: Vec<&str> = RULE_NAMES
                .iter()
                .copied()
                .filter(|name| loaded.policy.rules.contains_key(*name))
                .collect();
            if active.is_empty() {
                message(
                    "{path} is valid and names no rule; the command policy forbids nothing",
                    &[("path", shown(path))],
                )
            } else {
                message(
                    "{path} is valid; rules in force: {rules}",
                    &[("path", shown(path)), ("rules", active.join(", "))],
                )
            }
        }
    }
}

pub fn broken(error: &str) -> String {
    message(
        "job: the command policy cannot be read, so every command is refused until the file is corrected or removed: {error}",
        &[("error", error.to_owned())],
    )
}

pub fn fix() -> String {
    message(
        "correct the file or remove it; job policy --show validates it",
        &[],
    )
}

pub fn show() -> Result<ExitCode, String> {
    let loaded = load()?;
    println!("{}", describe(&loaded));
    Ok(ExitCode::SUCCESS)
}
