use std::io;
use std::path::{Path, PathBuf};

use super::message;
use crate::cgroup::Tree;

pub const LEGACY_ROOT: &str = "/sys/fs/cgroup/exec";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    Environment,
    Configuration,
    Delegated,
    Legacy,
}

impl Rule {
    pub fn name(self) -> &'static str {
        match self {
            Rule::Environment => "JOB_CGROUP_ROOT",
            Rule::Configuration => "[cgroup] root",
            Rule::Delegated => "delegated own cgroup",
            Rule::Legacy => "legacy /sys/fs/cgroup/exec",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Found {
    pub tree: Option<Tree>,
    pub rule: Option<Rule>,
    pub skipped: Vec<String>,
}

impl Found {
    pub fn describe(&self) -> String {
        match (&self.tree, self.rule) {
            (Some(tree), Some(rule)) => message(
                "cgroup root {root}, chosen by {rule}",
                &[
                    ("root", tree.root.display().to_string()),
                    ("rule", message(rule.name(), &[])),
                ],
            ),
            _ => {
                let mut text = message("no cgroup root, processes are watched instead", &[]);
                for reason in &self.skipped {
                    text.push_str("; ");
                    text.push_str(reason);
                }
                text
            }
        }
    }
}

pub fn foreign(root: &Path) -> Vec<i32> {
    let own = std::process::id() as i32;
    Tree::processes(root)
        .into_iter()
        .filter(|pid| *pid != own)
        .collect()
}

fn occupied(root: &Path, foreign: &[i32]) -> String {
    message(
        "{root} holds {count} other processes (first: {pid}); a cgroup root must hold only the service",
        &[
            ("root", root.display().to_string()),
            ("count", foreign.len().to_string()),
            ("pid", foreign[0].to_string()),
        ],
    )
}

fn explicit(root: PathBuf, rule: Rule) -> io::Result<Found> {
    let foreign = foreign(&root);
    if !foreign.is_empty() {
        return Err(io::Error::other(occupied(&root, &foreign)));
    }
    let tree = Tree::detect(&root);
    let skipped = if tree.is_none() {
        vec![message(
            "{rule} names {root}, which is not a writable cgroup holding this service",
            &[
                ("rule", message(rule.name(), &[])),
                ("root", root.display().to_string()),
            ],
        )]
    } else {
        Vec::new()
    };
    Ok(Found {
        rule: tree.is_some().then_some(rule),
        tree,
        skipped,
    })
}

pub fn discover(configured: Option<&Path>) -> io::Result<Found> {
    if let Some(root) = std::env::var_os("JOB_CGROUP_ROOT").filter(|v| !v.is_empty()) {
        return explicit(PathBuf::from(root), Rule::Environment);
    }
    if let Some(root) = configured {
        return explicit(root.to_path_buf(), Rule::Configuration);
    }
    let mut skipped = Vec::new();
    for (tree, rule) in [
        (Tree::delegated_own(), Rule::Delegated),
        (Tree::detect(Path::new(LEGACY_ROOT)), Rule::Legacy),
    ] {
        let Some(tree) = tree else {
            continue;
        };
        let foreign = foreign(&tree.root);
        if foreign.is_empty() {
            return Ok(Found {
                tree: Some(tree),
                rule: Some(rule),
                skipped,
            });
        }
        skipped.push(occupied(&tree.root, &foreign));
    }
    Ok(Found {
        tree: None,
        rule: None,
        skipped,
    })
}
