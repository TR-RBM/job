use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Queue, Settings};
use crate::store::Store;

pub const ROOT: u64 = 1;
pub const DEFAULT_QUEUE: u64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Group,
    Queue,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: u64,
    pub parent: Option<u64>,
    pub name: String,
    pub kind: Kind,
    pub owner_uid: u32,
    pub created_ms: u64,
    pub paused: bool,
    pub closed: bool,
    pub draining: bool,
    pub config: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Graph {
    pub schema_version: u32,
    pub next_id: u64,
    pub nodes: BTreeMap<u64, Node>,
    #[serde(default)]
    pub retired: BTreeMap<u64, View>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effective {
    pub value: Value,
    pub source_id: u64,
    pub source_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    pub object: Node,
    pub path: String,
    pub effective: BTreeMap<String, Effective>,
    #[serde(default)]
    pub aggregate_domains: Vec<crate::aggregate::Domain>,
    pub paused_by: Vec<String>,
    pub closed_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<crate::operations::depth::Depth>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    List,
    Show {
        path: String,
    },
    Create {
        path: String,
        config: BTreeMap<String, Value>,
    },
    Set {
        path: String,
        config: BTreeMap<String, Value>,
    },
    Unset {
        path: String,
        keys: Vec<String>,
    },
    Rename {
        path: String,
        name: String,
    },
    Move {
        path: String,
        parent: String,
    },
    Pause {
        path: String,
        paused: bool,
    },
    Close {
        path: String,
        closed: bool,
    },
    Remove {
        path: String,
    },
}

impl Graph {
    pub fn fresh() -> Self {
        let mut graph = Self {
            schema_version: 1,
            next_id: 3,
            nodes: BTreeMap::new(),
            retired: BTreeMap::new(),
        };
        for (id, parent, name, kind) in [
            (ROOT, None, "", Kind::Group),
            (DEFAULT_QUEUE, Some(ROOT), "default", Kind::Queue),
        ] {
            graph.nodes.insert(
                id,
                Node {
                    id,
                    parent,
                    name: name.to_owned(),
                    kind,
                    owner_uid: unsafe { libc::getuid() },
                    created_ms: crate::shim::now_ms(),
                    paused: false,
                    closed: false,
                    draining: false,
                    config: BTreeMap::new(),
                },
            );
        }
        graph
    }

    pub fn load(store: &Store) -> io::Result<Self> {
        let path = store.root.join("objects.json");
        let graph = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let mut graph = Self::fresh();
                for queue in store.load_queues()? {
                    let id = if queue.name == "default" {
                        DEFAULT_QUEUE
                    } else {
                        graph
                            .create(&queue.name, Kind::Queue, BTreeMap::new())
                            .map_err(io::Error::other)?
                    };
                    graph.update_queue(id, &queue);
                }
                graph
            }
            Err(e) => return Err(e),
        };
        graph.validate().map_err(io::Error::other)?;
        Ok(graph)
    }

    pub fn save(&self, store: &Store) -> io::Result<()> {
        self.validate().map_err(io::Error::other)?;
        crate::store::write_json(&store.root.join("objects.json"), self)?;
        crate::operations::journal::observe_objects(self);
        Ok(())
    }

    pub fn resolve(&self, path: &str) -> Result<u64, String> {
        if path == "/" || path.is_empty() {
            return Ok(ROOT);
        }
        if let Some(id) = path.strip_prefix('#').and_then(|s| s.parse::<u64>().ok()) {
            return self
                .nodes
                .contains_key(&id)
                .then_some(id)
                .ok_or_else(|| format!("no object {path}"));
        }
        let mut parent = ROOT;
        for name in path.trim_start_matches('/').split('/') {
            check_name(name)?;
            parent = self
                .nodes
                .values()
                .find(|n| n.parent == Some(parent) && n.name == name)
                .map(|n| n.id)
                .ok_or_else(|| format!("no object {path}"))?;
        }
        Ok(parent)
    }

    pub fn path(&self, id: u64) -> String {
        if id == ROOT {
            return "/".to_owned();
        }
        let mut names = self
            .ancestors(id)
            .into_iter()
            .filter(|&id| id != ROOT)
            .map(|id| self.nodes[&id].name.clone())
            .collect::<Vec<_>>();
        names.reverse();
        names.join("/")
    }

    pub fn ancestors(&self, id: u64) -> Vec<u64> {
        let mut chain = Vec::new();
        let mut current = Some(id);
        while let Some(id) = current {
            if chain.contains(&id) {
                break;
            }
            let Some(node) = self.nodes.get(&id) else {
                break;
            };
            chain.push(id);
            current = node.parent;
        }
        chain
    }

    pub fn within(&self, id: u64, ancestor: u64) -> bool {
        self.ancestors(id).contains(&ancestor)
    }

    pub fn view(&self, id: u64) -> View {
        let mut effective = BTreeMap::new();
        let mut paused_by = Vec::new();
        let mut closed_by = Vec::new();
        for ancestor in self.ancestors(id) {
            let node = &self.nodes[&ancestor];
            for (key, value) in &node.config {
                if crate::aggregate::key(key)
                    || ["fair_share", "share_weight", "pressure", "labels"].contains(&key.as_str())
                {
                    continue;
                }
                effective.entry(key.clone()).or_insert_with(|| Effective {
                    value: value.clone(),
                    source_id: ancestor,
                    source_path: self.path(ancestor),
                });
            }
            if node.paused {
                paused_by.push(self.path(ancestor));
            }
            if node.closed || node.draining {
                closed_by.push(self.path(ancestor));
            }
        }
        View {
            object: self.nodes[&id].clone(),
            path: self.path(id),
            effective,
            aggregate_domains: crate::aggregate::chain(self, id),
            paused_by,
            closed_by,
            depth: None,
        }
    }

    pub fn create(
        &mut self,
        path: &str,
        kind: Kind,
        config: BTreeMap<String, Value>,
    ) -> Result<u64, String> {
        let path = path.trim_start_matches('/');
        let (parent, name) = path.rsplit_once('/').unwrap_or(("/", path));
        let parent = self.resolve(parent)?;
        self.destination(None, parent, name)?;
        validate_config(&config)?;
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or("object IDs exhausted")?;
        self.nodes.insert(
            id,
            Node {
                id,
                parent: Some(parent),
                name: name.to_owned(),
                kind,
                owner_uid: unsafe { libc::getuid() },
                created_ms: crate::shim::now_ms(),
                paused: false,
                closed: false,
                draining: false,
                config,
            },
        );
        Ok(id)
    }

    fn destination(&self, moving: Option<u64>, parent: u64, name: &str) -> Result<(), String> {
        check_name(name)?;
        if self
            .nodes
            .get(&parent)
            .is_none_or(|n| n.kind != Kind::Group)
        {
            return Err("the parent must be a Group".to_owned());
        }
        if moving.is_some_and(|id| self.within(parent, id)) {
            return Err("moving an object into its own subtree would create a cycle".to_owned());
        }
        if self
            .nodes
            .values()
            .any(|n| n.parent == Some(parent) && n.name == name && Some(n.id) != moving)
        {
            return Err(format!("a sibling named {name} already exists"));
        }
        Ok(())
    }

    pub fn change(&mut self, kind: Kind, operation: &Operation) -> Result<Option<u64>, String> {
        let path = match operation {
            Operation::List => return Ok(None),
            Operation::Create { path, config } => {
                return self.create(path, kind, config.clone()).map(Some);
            }
            Operation::Show { path }
            | Operation::Set { path, .. }
            | Operation::Unset { path, .. }
            | Operation::Rename { path, .. }
            | Operation::Move { path, .. }
            | Operation::Pause { path, .. }
            | Operation::Close { path, .. }
            | Operation::Remove { path } => path,
        };
        let id = self.resolve(path)?;
        if self.nodes[&id].kind != kind {
            return Err(format!("{path} is not a {kind:?}"));
        }
        if matches!(
            operation,
            Operation::Rename { .. } | Operation::Move { .. } | Operation::Remove { .. }
        ) && [ROOT, DEFAULT_QUEUE].contains(&id)
        {
            return Err(
                "the root Group and default Queue cannot be moved, renamed or removed".to_owned(),
            );
        }
        match operation {
            Operation::Set { config, .. } => {
                validate_config(config)?;
                let mut config = config.clone();
                let node = self.nodes.get_mut(&id).unwrap();
                crate::cli2::labels::merge(&mut node.config, &mut config)?;
                node.config.extend(config);
            }
            Operation::Unset { keys, .. } => {
                let node = self.nodes.get_mut(&id).unwrap();
                for key in keys {
                    if !crate::cli2::labels::unset(&mut node.config, key) {
                        node.config.remove(key);
                    }
                }
            }
            Operation::Rename { name, .. } => {
                self.destination(Some(id), self.nodes[&id].parent.unwrap(), name)?;
                self.nodes.get_mut(&id).unwrap().name = name.clone();
            }
            Operation::Move { parent, .. } => {
                let parent = self.resolve(parent)?;
                self.destination(Some(id), parent, &self.nodes[&id].name)?;
                self.nodes.get_mut(&id).unwrap().parent = Some(parent);
            }
            Operation::Pause { paused, .. } => self.nodes.get_mut(&id).unwrap().paused = *paused,
            Operation::Close { closed, .. } => self.nodes.get_mut(&id).unwrap().closed = *closed,
            Operation::Remove { .. } => {
                if self.nodes.values().any(|n| n.parent == Some(id)) {
                    return Err("object is not empty".to_owned());
                }
                self.retired.insert(id, self.view(id));
                self.nodes.remove(&id);
                return Ok(None);
            }
            _ => {}
        }
        Ok(Some(id))
    }

    pub fn queue(&self, id: u64) -> Queue {
        let view = self.view(id);
        let settings: Settings = serde_json::from_value(Value::Object(
            view.effective
                .iter()
                .filter(|(key, _)| key.as_str() != "max_running")
                .map(|(key, value)| (key.clone(), value.value.clone()))
                .collect(),
        ))
        .unwrap_or_default();
        Queue {
            name: view.path,
            parallel: view
                .effective
                .get("max_running")
                .and_then(|v| v.value.as_u64()),
            paused: !view.paused_by.is_empty(),
            draining: view.object.draining,
            created_ms: view.object.created_ms,
            settings,
        }
    }

    pub fn queues(&self) -> BTreeMap<String, Queue> {
        self.nodes
            .values()
            .filter(|n| n.kind == Kind::Queue)
            .map(|n| {
                let q = self.queue(n.id);
                (q.name.clone(), q)
            })
            .collect()
    }

    pub fn update_queue(&mut self, id: u64, queue: &Queue) {
        let node = self.nodes.get_mut(&id).unwrap();
        let aggregate: BTreeMap<_, _> = node
            .config
            .iter()
            .filter(|(key, _)| {
                crate::aggregate::key(key)
                    || crate::admission::key(key)
                    || key.as_str() == "pressure"
                    || key.as_str() == crate::cli2::labels::CONFIG_KEY
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let fields = serde_json::to_value(&queue.settings).unwrap();
        node.config = fields
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        node.config.extend(aggregate);
        if let Some(parallel) = queue.parallel {
            node.config
                .insert("max_running".to_owned(), Value::from(parallel));
        }
        node.paused = queue.paused;
        node.draining = queue.draining;
        node.created_ms = queue.created_ms;
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err(format!("unsupported object schema {}", self.schema_version));
        }
        if self
            .nodes
            .get(&ROOT)
            .is_none_or(|n| n.kind != Kind::Group || n.parent.is_some())
            || self.nodes.get(&DEFAULT_QUEUE).is_none_or(|n| {
                n.kind != Kind::Queue || n.parent != Some(ROOT) || n.name != "default"
            })
        {
            return Err("invalid root Group or default Queue".to_owned());
        }
        for (&id, view) in &self.retired {
            if id != view.object.id || id >= self.next_id || self.nodes.contains_key(&id) {
                return Err("invalid retired object identity".to_owned());
            }
        }
        let mut siblings = BTreeSet::new();
        for (&id, node) in &self.nodes {
            if node.id != id || id >= self.next_id {
                return Err("invalid object identity".to_owned());
            }
            validate_config(&node.config)?;
            if id == ROOT {
                continue;
            }
            check_name(&node.name)?;
            let parent = node.parent.ok_or("object has no parent")?;
            if self
                .nodes
                .get(&parent)
                .is_none_or(|n| n.kind != Kind::Group)
                || !self.ancestors(id).contains(&ROOT)
            {
                return Err("object graph has a cycle or invalid parent".to_owned());
            }
            if !siblings.insert((parent, &node.name)) {
                return Err("duplicate sibling name".to_owned());
            }
        }
        crate::admission::validate_bounds(self)?;
        Ok(())
    }
}

pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 128
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        Err(format!("invalid object name {name:?}"))
    } else {
        Ok(())
    }
}

fn validate_config(config: &BTreeMap<String, Value>) -> Result<(), String> {
    crate::admission::validate_config(config)?;
    for key in ["job_execution_profile", "job_scheduling_class"] {
        if let Some(value) = config.get(key) {
            crate::presets::reference(
                value
                    .as_str()
                    .ok_or_else(|| crate::presets::message("use a pinned name@revision or none"))?,
            )?;
        }
    }
    if let Some(value) = config.get("pressure") {
        crate::pressure::control::validate_local(value)?;
    }
    crate::cli2::labels::validate_config(config)?;
    let settings = serde_json::to_value(Settings::default()).unwrap();
    let mut resource_fields = crate::resource_policy::Controls::default().fields();
    resource_fields.extend(crate::process_policy::Controls::default().fields());
    resource_fields.extend(crate::security::Controls::default().fields());
    resource_fields.extend(crate::isolation::Controls::default().fields());
    resource_fields.extend(crate::streams::quota::Controls::default().fields());
    for (key, value) in config {
        if value.is_null()
            && key
                .strip_prefix("job_")
                .is_some_and(|name| resource_fields.contains_key(name))
        {
            return Err(crate::resource_policy::message(
                "use unset to remove a resource default",
            ));
        }
        if key == "max_running" {
            if value.as_u64().is_none_or(|n| n == 0) && value != "unlimited" {
                return Err("max-running requires a positive integer or unlimited".to_owned());
            }
        } else if !crate::aggregate::key(key)
            && !crate::admission::key(key)
            && key != "pressure"
            && key != crate::cli2::labels::CONFIG_KEY
            && !settings.as_object().unwrap().contains_key(key)
        {
            return Err(format!("unknown setting {key}"));
        }
    }
    let parsed = serde_json::from_value::<Settings>(Value::Object(
        config
            .iter()
            .filter(|(key, _)| key.as_str() != "max_running")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    ))
    .map_err(|e| e.to_string())?;
    crate::resource_policy::Controls::from_settings(&parsed).validate()?;
    crate::process_policy::Controls::from_settings(&parsed).validate()?;
    crate::security::Controls::from_settings(&parsed).validate()?;
    crate::streams::quota::Controls::from_settings(&parsed).validate()?;
    crate::aggregate::controls(config)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_tree_has_stable_identity_after_move_and_rename() {
        let mut graph = Graph::fresh();
        let development = graph
            .create("development", Kind::Group, BTreeMap::new())
            .unwrap();
        let queue = graph
            .create("development/builds", Kind::Queue, BTreeMap::new())
            .unwrap();
        graph
            .create("development/releases", Kind::Group, BTreeMap::new())
            .unwrap();
        assert!(graph.nodes[&queue].config.is_empty());
        graph
            .change(
                Kind::Queue,
                &Operation::Move {
                    path: "development/builds".into(),
                    parent: "development/releases".into(),
                },
            )
            .unwrap();
        graph
            .change(
                Kind::Group,
                &Operation::Rename {
                    path: "development".into(),
                    name: "engineering".into(),
                },
            )
            .unwrap();
        assert_eq!(graph.resolve("engineering/releases/builds").unwrap(), queue);
        assert_eq!(graph.resolve("engineering").unwrap(), development);
        graph.validate().unwrap();
    }

    #[test]
    fn moving_a_group_into_its_descendant_is_refused() {
        let mut graph = Graph::fresh();
        graph.create("a", Kind::Group, BTreeMap::new()).unwrap();
        graph.create("a/b", Kind::Group, BTreeMap::new()).unwrap();
        let before = graph.clone();
        assert!(
            graph
                .change(
                    Kind::Group,
                    &Operation::Move {
                        path: "a".into(),
                        parent: "a/b".into()
                    }
                )
                .is_err()
        );
        assert_eq!(graph, before);
    }

    #[test]
    fn resuming_a_parent_preserves_child_holds() {
        let mut graph = Graph::fresh();
        graph.create("a", Kind::Group, BTreeMap::new()).unwrap();
        let queue = graph.create("a/b", Kind::Queue, BTreeMap::new()).unwrap();
        for (kind, path) in [(Kind::Group, "a"), (Kind::Queue, "a/b")] {
            graph
                .change(
                    kind,
                    &Operation::Pause {
                        path: path.into(),
                        paused: true,
                    },
                )
                .unwrap();
        }
        graph
            .change(
                Kind::Group,
                &Operation::Pause {
                    path: "a".into(),
                    paused: false,
                },
            )
            .unwrap();
        assert_eq!(graph.view(queue).paused_by, vec!["a/b"]);
    }

    #[test]
    fn unlimited_and_unset_remain_distinct_in_storage() {
        let mut graph = Graph::fresh();
        graph
            .create(
                "a",
                Kind::Group,
                BTreeMap::from([("max_running".into(), Value::from(4))]),
            )
            .unwrap();
        let id = graph.create("a/b", Kind::Queue, BTreeMap::new()).unwrap();
        graph
            .change(
                Kind::Queue,
                &Operation::Set {
                    path: "a/b".into(),
                    config: BTreeMap::from([("max_running".into(), Value::from("unlimited"))]),
                },
            )
            .unwrap();
        assert_eq!(graph.view(id).effective["max_running"].value, "unlimited");
        graph
            .change(
                Kind::Queue,
                &Operation::Unset {
                    path: "a/b".into(),
                    keys: vec!["max_running".into()],
                },
            )
            .unwrap();
        assert!(graph.nodes[&id].config.is_empty());
        assert_eq!(graph.view(id).effective["max_running"].source_path, "a");
    }

    #[test]
    fn old_serial_queue_import_is_explicit_and_preserves_the_source() {
        let root = std::env::temp_dir().join(format!("job-objects-import-{}", std::process::id()));
        let store = Store::open(root.clone()).unwrap();
        std::fs::create_dir_all(root.join("queues")).unwrap();
        let bytes = br#"{"name":"serial","parallel":1,"created_ms":10}"#;
        std::fs::write(root.join("queues/serial.json"), bytes).unwrap();
        let graph = Graph::load(&store).unwrap();
        let id = graph.resolve("serial").unwrap();
        assert_eq!(graph.nodes[&id].config["max_running"], 1);
        graph.save(&store).unwrap();
        assert_eq!(Graph::load(&store).unwrap(), graph);
        assert_eq!(
            std::fs::read(root.join("queues/serial.json")).unwrap(),
            bytes
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn damaged_legacy_queue_is_not_silently_discarded() {
        let root = std::env::temp_dir().join(format!("job-objects-damaged-{}", std::process::id()));
        let store = Store::open(root.clone()).unwrap();
        std::fs::create_dir_all(root.join("queues")).unwrap();
        std::fs::write(root.join("queues/broken.json"), b"{").unwrap();
        assert!(Graph::load(&store).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
