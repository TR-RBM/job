use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::model::{Job, Net, Spec};
use crate::store::{Store, write_json};

const KEY_LIMIT: usize = 128;
const ENVIRONMENT_POLICY: &str = "captured-at-first-submission";

pub const DIGEST_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub key: String,
    pub digest: String,
    pub legacy: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    key: String,
    id: u64,
    digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    digest_version: Option<u32>,
}

pub enum Lookup {
    Fresh,
    Existing(u64),
}

fn error(text: &str) -> io::Error {
    io::Error::other(super::message(text))
}

pub fn check(key: &str) -> Result<(), String> {
    if key.is_empty()
        || key.len() > KEY_LIMIT
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
    {
        return Err(super::message(
            "--idempotency-key needs 1 to 128 characters of A-Z a-z 0-9 . _ : -",
        ));
    }
    Ok(())
}

fn hash(key: &str) -> String {
    format!("{:x}", Sha256::digest(key.as_bytes()))
}

fn directory(store: &Store) -> PathBuf {
    store.root.join("idempotency")
}

fn file(store: &Store, key: &str) -> PathBuf {
    directory(store).join(format!("{}.json", hash(key)))
}

fn given(value: Value) -> Option<Value> {
    match value {
        Value::Null | Value::Bool(false) => None,
        Value::Array(items) if items.is_empty() => None,
        Value::Object(map) => {
            let kept: serde_json::Map<String, Value> = map
                .into_iter()
                .filter_map(|(name, inner)| given(inner).map(|inner| (name, inner)))
                .collect();
            (!kept.is_empty()).then_some(Value::Object(kept))
        }
        other => Some(other),
    }
}

fn canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut names: Vec<&String> = map.keys().collect();
            names.sort();
            out.push('{');
            for (index, name) in names.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(name.clone()).to_string());
                out.push(':');
                canonical(&map[name], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

fn requested(spec: &Spec, held: bool) -> Result<String, String> {
    let mut declared = spec.declared.clone();
    let pty = declared.terminal.take().is_some();
    if let Some(Net::Proxy(url)) = &mut declared.net {
        *url = crate::netsecret::split(url).0;
    }
    let options = serde_json::to_value(&declared).map_err(|error| error.to_string())?;
    let value = serde_json::json!({
        "digest_version": DIGEST_VERSION,
        "argv": spec.argv,
        "cwd": spec.cwd,
        "queue": spec.queue,
        "held": held,
        "pty": pty,
        "environment": ENVIRONMENT_POLICY,
        "options": given(options).unwrap_or_else(|| serde_json::json!({})),
    });
    let mut text = String::new();
    canonical(&value, &mut text);
    Ok(text)
}

fn legacy(spec: &Spec, held: bool) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&(
        &spec.argv,
        &spec.cwd,
        &spec.queue,
        &spec.declared,
        held,
        ENVIRONMENT_POLICY,
    ))
    .map_err(|error| error.to_string())
}

pub fn key(key: Option<String>, spec: &Spec, held: bool) -> Result<Option<Key>, String> {
    let Some(key) = key else {
        return Ok(None);
    };
    check(&key)?;
    Ok(Some(Key {
        key,
        digest: format!("{:x}", Sha256::digest(requested(spec, held)?)),
        legacy: format!("{:x}", Sha256::digest(legacy(spec, held)?)),
    }))
}

pub fn edited(previous: Option<&Job>, spec: &Spec, held: bool) -> Result<Option<Key>, String> {
    key(
        previous.and_then(|job| job.durability.idempotency_key.clone()),
        spec,
        held,
    )
}

pub fn lookup(store: &Store, key: &Key) -> io::Result<Lookup> {
    let bytes = match fs::read(file(store, &key.key)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Lookup::Fresh),
        Err(error) => return Err(error),
    };
    let entry: Entry =
        serde_json::from_slice(&bytes).map_err(|_| error("invalid idempotency index"))?;
    if entry.key != key.key || entry.id == 0 {
        return Err(error("invalid idempotency index"));
    }
    let Some(job) = store
        .load_job(entry.id)
        .filter(|job| job.durability.idempotency_key.as_deref() == Some(key.key.as_str()))
    else {
        return Ok(Lookup::Fresh);
    };
    let (recorded, version) = match &job.durability.spec_digest {
        Some(digest) => (digest, job.durability.spec_digest_version),
        None => (&entry.digest, entry.digest_version),
    };
    let wanted = if version == Some(DIGEST_VERSION) {
        &key.digest
    } else {
        &key.legacy
    };
    if recorded == wanted {
        Ok(Lookup::Existing(entry.id))
    } else {
        Err(io::Error::other(format!(
            "{}: {}",
            super::message(
                "this idempotency key already belongs to a Job with a different specification"
            ),
            job.id
        )))
    }
}

pub fn reserve(store: &Store, key: &Key, id: u64) -> io::Result<()> {
    let directory = directory(store);
    match fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => fs::File::open(&store.root)?.sync_all()?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    write_json(
        &file(store, &key.key),
        &Entry {
            key: key.key.clone(),
            id,
            digest: key.digest.clone(),
            digest_version: Some(DIGEST_VERSION),
        },
    )
    .inspect_err(|_| {
        let _ = sweep(store);
    })
}

pub fn release(store: &Store, key: &Key) {
    let _ = fs::remove_file(file(store, &key.key));
}

pub fn record(key: Option<&Key>, previous: Option<&Job>) -> super::Record {
    let peer = super::peer::current();
    super::Record {
        idempotency_key: key
            .map(|key| key.key.clone())
            .or_else(|| previous.and_then(|job| job.durability.idempotency_key.clone())),
        spec_digest: key
            .map(|key| key.digest.clone())
            .or_else(|| previous.and_then(|job| job.durability.spec_digest.clone())),
        spec_digest_version: match key {
            Some(_) => Some(DIGEST_VERSION),
            None => previous.and_then(|job| job.durability.spec_digest_version),
        },
        actor_uid: peer.map(|peer| peer.uid),
        actor_pid: peer.map(|peer| peer.pid),
        revision: previous.and_then(|job| job.durability.revision),
        ..Default::default()
    }
}

fn leftover(name: &OsStr) -> bool {
    name.to_str()
        .and_then(|name| name.rsplit_once(".tmp"))
        .is_some_and(|(stem, pid)| {
            !stem.is_empty() && !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn listing(store: &Store) -> io::Result<Vec<fs::DirEntry>> {
    let directory = directory(store);
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(error("invalid idempotency index")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    }
    fs::read_dir(&directory)?.collect()
}

pub fn leftovers(store: &Store) -> io::Result<Vec<String>> {
    let mut names: Vec<String> = listing(store)?
        .into_iter()
        .filter(|entry| {
            leftover(&entry.file_name()) && entry.file_type().is_ok_and(|kind| kind.is_file())
        })
        .map(|entry| format!("idempotency/{}", entry.file_name().to_string_lossy()))
        .collect();
    names.sort();
    Ok(names)
}

pub fn sweep(store: &Store) -> io::Result<usize> {
    let names = leftovers(store)?;
    for name in &names {
        match fs::remove_file(store.root.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    if !names.is_empty() {
        fs::File::open(directory(store))?.sync_all()?;
    }
    Ok(names.len())
}

fn entries(store: &Store) -> io::Result<Vec<(PathBuf, Entry)>> {
    let mut found = Vec::new();
    for entry in listing(store)? {
        if !entry.file_type()?.is_file() {
            return Err(error("invalid idempotency index"));
        }
        if leftover(&entry.file_name()) {
            continue;
        }
        let parsed: Entry = serde_json::from_slice(&fs::read(entry.path())?)
            .map_err(|_| error("invalid idempotency index"))?;
        if check(&parsed.key).is_err()
            || parsed.id == 0
            || entry.file_name() != format!("{}.json", hash(&parsed.key)).as_str()
        {
            return Err(error("invalid idempotency index"));
        }
        found.push((entry.path(), parsed));
    }
    Ok(found)
}

pub fn validate(store: &Store) -> io::Result<()> {
    entries(store).map(|_| ())
}

pub fn forget(store: &Store, ids: &BTreeSet<u64>) -> io::Result<()> {
    let Ok(listed) = listing(store) else {
        return Ok(());
    };
    let mut removed = false;
    for entry in listed {
        let recorded = fs::read(entry.path())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Entry>(&bytes).ok());
        if recorded.is_some_and(|recorded| ids.contains(&recorded.id)) {
            match fs::remove_file(entry.path()) {
                Ok(()) => removed = true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    if removed {
        fs::File::open(directory(store))?.sync_all()?;
    }
    Ok(())
}
