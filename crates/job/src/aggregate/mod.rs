use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cgroup::Tree;
use crate::objects::Graph;
use crate::resource_policy::{Controls, kernel_file, message};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Domain {
    pub object_id: u64,
    pub object_path: String,
    pub limits: BTreeMap<String, String>,
}

pub fn key(key: &str) -> bool {
    kernel_file(key).is_some()
}

pub fn controls(config: &BTreeMap<String, Value>) -> Result<Controls, String> {
    let values: BTreeMap<_, _> = config.iter().filter(|(name, _)| key(name)).collect();
    if values.values().any(|value| value.is_null()) {
        return Err(message("use unset to remove an aggregate control"));
    }
    let controls: Controls = serde_json::from_value(serde_json::to_value(values).unwrap())
        .map_err(|error| error.to_string())?;
    controls.validate()?;
    Ok(controls)
}

pub fn chain(graph: &Graph, queue: u64) -> Vec<Domain> {
    graph
        .ancestors(queue)
        .into_iter()
        .rev()
        .filter_map(|id| {
            let node = &graph.nodes[&id];
            let controls = controls(&node.config).expect("validated object controls");
            let limits = crate::cgroup::Limits::for_controls(&controls);
            (!limits.managed.is_empty()).then(|| Domain {
                object_id: id,
                object_path: graph.path(id),
                limits: limits
                    .files()
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect(),
            })
        })
        .collect()
}

pub fn paths(tree: &Tree, domains: &[Domain]) -> Vec<PathBuf> {
    let mut path = tree.jobs();
    domains
        .iter()
        .map(|domain| {
            path.push(format!("domain-{}", domain.object_id));
            path.clone()
        })
        .collect()
}

pub fn leaf(tree: &Tree, domains: &[Domain], id: u64) -> PathBuf {
    paths(tree, domains)
        .last()
        .cloned()
        .unwrap_or_else(|| tree.jobs())
        .join(id.to_string())
}

pub fn available(tree: Option<&Tree>, domains: &[Domain], remote: bool) -> Result<(), String> {
    if remote && !domains.is_empty() {
        return Err(message("aggregate controls require local execution"));
    }
    for domain in domains {
        for (file, value) in &domain.limits {
            crate::io_policy::available(file, value)?;
            if tree.is_none_or(|tree| !tree.supports(file)) {
                return Err(format!(
                    "{}: {}: {file}",
                    message("requested resource control is unavailable"),
                    domain.object_path
                ));
            }
        }
    }
    Ok(())
}

pub fn changed(tree: &Tree, domains: &[Domain]) -> Option<String> {
    changed_except(tree, domains, |_, _, _| false)
}

pub fn changed_except(
    tree: &Tree,
    domains: &[Domain],
    accepts: impl Fn(&Path, &str, &str) -> bool,
) -> Option<String> {
    for (domain, path) in domains.iter().zip(paths(tree, domains)) {
        if !path.join("cgroup.events").exists() {
            return Some(format!(
                "{}: {}",
                message("aggregate resource domain is unavailable"),
                domain.object_path
            ));
        }
        for (file, expected) in &domain.limits {
            if let Some(actual) = crate::cgroup::changed_control(&path, file, expected)
                && !accepts(&path, file, &actual)
            {
                return Some(format!(
                    "{}: {}: {file}: {expected} -> {actual}",
                    message("aggregate resource control changed outside the service"),
                    domain.object_path
                ));
            }
        }
    }
    None
}

pub fn prepare(tree: &Tree, domains: &[Domain]) -> io::Result<()> {
    for (domain, path) in domains.iter().zip(paths(tree, domains)) {
        if path.exists() {
            for (file, expected) in &domain.limits {
                if crate::cgroup::changed_control(&path, file, expected).is_some() {
                    return Err(io::Error::other(message(
                        "aggregate resource control changed outside the service",
                    )));
                }
            }
            continue;
        }
        fs::create_dir(&path)?;
        for (file, value) in &domain.limits {
            crate::cgroup::write_control(&path, file, value)?;
        }
        let controllers = fs::read_to_string(path.join("cgroup.controllers"))?;
        let enable = controllers
            .split_whitespace()
            .filter(|name| ["cpu", "memory", "pids", "io"].contains(name))
            .map(|name| format!("+{name}"))
            .collect::<Vec<_>>()
            .join(" ");
        fs::write(path.join("cgroup.subtree_control"), enable)?;
    }
    Ok(())
}

fn remove_empty(path: &Path) -> io::Result<()> {
    let events = fs::read_to_string(path.join("cgroup.events"))?;
    if crate::cgroup::keyed_value(&events, "populated") != 0 {
        return Err(io::Error::other(message(
            "aggregate resource domain is still populated",
        )));
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name
                .strip_prefix("domain-")
                .unwrap_or(&name)
                .parse::<u64>()
                .is_err()
            {
                return Err(io::Error::other(message(
                    "unknown child in aggregate resource domain",
                )));
            }
            remove_empty(&entry.path())?;
        }
    }
    fs::remove_dir(path)
}

pub fn cleanup(tree: &Tree, graph: &Graph, protected: &BTreeSet<PathBuf>) {
    let mut desired = BTreeMap::new();
    for id in graph.nodes.keys() {
        let domains = chain(graph, *id);
        for (domain, path) in domains.iter().zip(paths(tree, &domains)) {
            desired.insert(path, domain.clone());
        }
    }
    prune(&tree.jobs(), &desired, protected);
}

fn prune(parent: &Path, desired: &BTreeMap<PathBuf, Domain>, protected: &BTreeSet<PathBuf>) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir())
            || !entry
                .file_name()
                .to_string_lossy()
                .strip_prefix("domain-")
                .is_some_and(|id| id.parse::<u64>().is_ok())
        {
            continue;
        }
        let path = entry.path();
        let matches = desired
            .get(&path)
            .is_some_and(|domain| defaults_match(&path, domain));
        if !matches && !protected.contains(&path) && !Tree::is_populated(&path) {
            let _ = remove_empty(&path);
        } else {
            prune(&path, desired, protected);
        }
    }
}

fn defaults_match(path: &Path, domain: &Domain) -> bool {
    let cpu = format!("max {}", crate::resource_policy::CPU_PERIOD_US);
    [
        ("cpu.max", cpu.as_str()),
        ("cpu.weight", "100"),
        ("memory.high", "max"),
        ("memory.max", "max"),
        ("memory.swap.max", "max"),
        ("pids.max", "max"),
        ("io.max", ""),
        ("io.weight", ""),
        ("io.bfq.weight", ""),
    ]
    .into_iter()
    .all(|(file, default)| {
        let expected = domain.limits.get(file).map_or(default, String::as_str);
        match fs::read_to_string(path.join(file)) {
            Ok(value) => crate::cgroup::control_matches(file, expected, &value),
            Err(error) => {
                error.kind() == io::ErrorKind::NotFound && !domain.limits.contains_key(file)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{Kind, Operation};

    #[test]
    fn empty_topology_adds_no_domains_or_default_weight() {
        let mut graph = Graph::fresh();
        graph.create("a", Kind::Group, BTreeMap::new()).unwrap();
        graph.create("a/b", Kind::Group, BTreeMap::new()).unwrap();
        let queue = graph.create("a/b/q", Kind::Queue, BTreeMap::new()).unwrap();
        assert!(chain(&graph, queue).is_empty());
        assert!(graph.view(queue).effective.is_empty());
        assert_eq!(
            leaf(
                &Tree {
                    root: PathBuf::from("/scope")
                },
                &chain(&graph, queue),
                42
            ),
            PathBuf::from("/scope/jobs/42")
        );
    }

    #[test]
    fn local_unlimited_preserves_every_ancestor_constraint() {
        let mut graph = Graph::fresh();
        let group = graph
            .create(
                "a",
                Kind::Group,
                BTreeMap::from([("memory_max".into(), serde_json::json!(1048576))]),
            )
            .unwrap();
        graph.create("a/b", Kind::Group, BTreeMap::new()).unwrap();
        let queue = graph
            .create(
                "a/b/q",
                Kind::Queue,
                BTreeMap::from([
                    ("memory_max".into(), serde_json::json!("unlimited")),
                    ("job_cpu_weight".into(), serde_json::json!(7)),
                ]),
            )
            .unwrap();
        let domains = chain(&graph, queue);
        assert_eq!(domains.len(), 2);
        assert_eq!(domains[0].object_id, group);
        assert_eq!(domains[0].limits["memory.max"], "1048576");
        assert_eq!(domains[1].limits["memory.max"], "max");
        assert!(!domains[1].limits.contains_key("cpu.weight"));
        assert!(!graph.view(queue).effective.contains_key("memory_max"));
        assert_eq!(graph.view(queue).effective["job_cpu_weight"].value, 7);
        graph
            .change(
                Kind::Queue,
                &Operation::Unset {
                    path: "a/b/q".into(),
                    keys: vec!["memory_max".into()],
                },
            )
            .unwrap();
        assert_eq!(chain(&graph, queue), domains[..1]);
    }

    #[test]
    fn legacy_queue_settings_preserve_aggregate_constraints() {
        let mut graph = Graph::fresh();
        let id = graph
            .create(
                "q",
                Kind::Queue,
                BTreeMap::from([("cpu_limit_milli".into(), serde_json::json!(1000))]),
            )
            .unwrap();
        let mut queue = graph.queue(id);
        queue.paused = true;
        graph.update_queue(id, &queue);
        assert_eq!(graph.nodes[&id].config["cpu_limit_milli"], 1000);
    }
}
