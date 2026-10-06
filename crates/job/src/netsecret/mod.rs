mod messages;

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::model::{Declared, Job, Net, Settings};

pub use messages::{help, message};

pub const MARK: &str = "***";
const PRIVATE_FILE: &str = "net-secret";
const SHARED_DIRECTORY: &str = "net-secrets";
const NET_KEY: &str = "net";
const FILE_KEY: &str = "net_secret_file";
const LONGEST: u64 = 4096;

pub fn split(url: &str) -> (String, Option<String>) {
    let Some((scheme, rest)) = url.split_once("://") else {
        return (url.to_string(), None);
    };
    match rest.rsplit_once('@') {
        Some((who, place)) => (
            format!("{scheme}://{MARK}@{place}"),
            (who != MARK).then(|| who.to_string()),
        ),
        None => (url.to_string(), None),
    }
}

pub fn hidden(text: &str) -> Option<String> {
    let (scheme, _) = text.split_once("://")?;
    if scheme.is_empty()
        || !scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+.-".contains(&byte))
    {
        return None;
    }
    let (shown, secret) = split(text);
    secret.map(|_| shown)
}

fn well_formed(secret: &str) -> bool {
    let (user, password) = secret.split_once(':').unwrap_or(("", ""));
    let plain = |c: char| !c.is_whitespace() && !c.is_control() && c != '@' && c != '/';
    !user.is_empty() && !password.is_empty() && secret.chars().all(plain)
}

pub fn check(net: Option<&Net>, file: Option<&Path>) -> Result<(), String> {
    let relative = |option: &str, path: &Path| {
        message(
            "{option}: {path} must be an absolute path",
            &[
                ("option", option.to_string()),
                ("path", path.display().to_string()),
            ],
        )
    };
    match net {
        Some(Net::Proxy(url)) => {
            if !matches!(Net::parse(url)?, Net::Proxy(_)) {
                return Err(message(
                    "--net: `{net}` is not a network; write default, none, socks5://HOST:PORT, http://HOST:PORT, https://HOST:PORT, wireguard:FILE, openvpn:FILE, ns:NAME or profile:NAME",
                    &[("net", redact(url))],
                ));
            }
            if split(url).1.is_some_and(|secret| !well_formed(&secret)) {
                return Err(message(
                    "--net: write the proxy's user and password as USER:PASSWORD before the @, with @, / and spaces percent-encoded",
                    &[],
                ));
            }
        }
        Some(Net::WireGuard(path) | Net::OpenVpn(path)) if !path.is_absolute() => {
            return Err(relative("--net", path));
        }
        _ => {}
    }
    match file {
        Some(path) if !path.is_absolute() => Err(relative("--net-secret-file", path)),
        _ => Ok(()),
    }
}

pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (before, after) = rest.split_at(at + 3);
        out.push_str(before);
        let quoted = after
            .find(['\'', '"', '`'])
            .unwrap_or(after.len())
            .min(after.find("://").unwrap_or(after.len()));
        let (span, _) = after.split_at(quoted);
        let Some(at) = span.rfind('@') else {
            let end = span.find(char::is_whitespace).unwrap_or(span.len());
            out.push_str(&after[..end]);
            rest = &after[end..];
            continue;
        };
        let place = &span[at + 1..];
        let end = place.find(char::is_whitespace).unwrap_or(place.len());
        out.push_str(MARK);
        out.push('@');
        out.push_str(&place[..end]);
        rest = &after[at + 1 + end..];
    }
    out.push_str(rest);
    out
}

pub fn read(path: &Path) -> Result<String, String> {
    let shown = || path.display().to_string();
    let unreadable = |error: io::Error| {
        message(
            "--net-secret-file: cannot read {path}: {error}",
            &[("path", shown()), ("error", error.to_string())],
        )
    };
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(unreadable)?;
    let meta = file.metadata().map_err(unreadable)?;
    if !meta.is_file() {
        return Err(message(
            "--net-secret-file: {path} is not a regular file",
            &[("path", shown())],
        ));
    }
    if meta.uid() != unsafe { libc::geteuid() } {
        return Err(message(
            "--net-secret-file: {path} belongs to another user; it must belong to the user the job service runs as",
            &[("path", shown())],
        ));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(message(
            "--net-secret-file: {path} can be read by its group or by others; run: chmod 600 {path}",
            &[("path", shown())],
        ));
    }
    let mut text = String::new();
    file.take(LONGEST)
        .read_to_string(&mut text)
        .map_err(unreadable)?;
    let line = text.strip_suffix('\n').unwrap_or(&text);
    if !well_formed(line) {
        return Err(message(
            "--net-secret-file: {path} must hold one line USER:PASSWORD; write @, / and spaces percent-encoded",
            &[("path", shown())],
        ));
    }
    Ok(line.to_string())
}

fn write_private(path: &Path, secret: &str) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(secret.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    match path.parent() {
        Some(parent) => fs::File::open(parent)?.sync_all(),
        None => Ok(()),
    }
}

fn kept(path: PathBuf, secret: &str) -> Result<PathBuf, String> {
    write_private(&path, secret).map_err(|error| {
        message(
            "cannot keep the proxy's user and password in {path}: {error}",
            &[
                ("path", path.display().to_string()),
                ("error", error.to_string()),
            ],
        )
    })?;
    Ok(path)
}

fn shared_file(root: &Path) -> io::Result<PathBuf> {
    let dir = root.join(SHARED_DIRECTORY);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)?;
    let mut random = [0u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let name: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(dir.join(name))
}

pub fn take(net: &mut Option<Net>) -> Option<String> {
    let Some(Net::Proxy(url)) = net else {
        return None;
    };
    let (shown, secret) = split(url);
    *url = shown;
    secret
}

fn both() -> String {
    message(
        "give the proxy's user and password either inside --net or in --net-secret-file, not both",
        &[],
    )
}

pub fn private_path(directory: &Path) -> PathBuf {
    directory.join(PRIVATE_FILE)
}

pub fn take_for_job(
    declared: &mut Declared,
    existing: Option<&Path>,
) -> Result<Option<String>, String> {
    check(declared.net.as_ref(), declared.net_secret_file.as_deref())?;
    let own = existing.map(private_path);
    let carried = own.is_some() && declared.net_secret_file == own;
    let secret = take(&mut declared.net);
    if secret.is_some() && declared.net_secret_file.is_some() && !carried {
        return Err(both());
    }
    if !carried {
        return Ok(secret);
    }
    let path = declared.net_secret_file.take().unwrap_or_default();
    match secret {
        Some(secret) => Ok(Some(secret)),
        None => read(&path).map(Some),
    }
}

pub fn stage(directory: &Path, secret: Option<&str>) -> io::Result<()> {
    let path = private_path(directory);
    match secret {
        Some(secret) => write_private(&path, secret),
        None => match fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        },
    }
}

pub fn settle_record(job: &mut Job, directory: &Path) -> Result<bool, String> {
    let own = private_path(directory);
    let mut changed = false;
    let mut found = None;
    let specs = [
        Some(&mut job.spec),
        job.requested_spec.as_mut(),
        job.submitted_spec.as_mut(),
        job.effective_spec.as_mut(),
    ];
    for spec in specs.into_iter().flatten() {
        if let Some(secret) = take(&mut spec.declared.net) {
            changed = true;
            if spec.declared.net_secret_file.is_none() {
                spec.declared.net_secret_file = Some(own.clone());
                found.get_or_insert(secret);
            }
        }
    }
    if let Some(link) = &mut job.link {
        changed |= take(&mut link.egress).is_some();
        for (_, value) in &mut link.env {
            if let Some(shown) = hidden(value) {
                *value = shown;
                changed = true;
            }
        }
    }
    if let Some(secret) = found {
        kept(own, &secret)?;
    }
    Ok(changed)
}

fn conceal(value: &mut Value, linked: bool) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map.iter_mut() {
                match inner {
                    Value::String(url) if key == "Proxy" => *url = split(url).0,
                    _ => conceal(inner, linked || key == "link"),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| conceal(item, linked)),
        Value::String(text) if linked => {
            if let Some(shown) = hidden(text) {
                *text = shown;
            }
        }
        _ => {}
    }
}

pub fn rendered<T: serde::Serialize>(answer: &T) -> serde_json::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(answer)?;
    let named = b"\"Proxy\"";
    if !bytes.windows(named.len()).any(|window| window == named) {
        return Ok(bytes);
    }
    let mut value: Value = serde_json::from_slice(&bytes)?;
    conceal(&mut value, false);
    serde_json::to_vec(&value)
}

pub fn resolve(declared: &mut Declared, settings: &Settings) -> Result<(), String> {
    if declared.net_secret_file.is_none()
        && (declared.net.is_none() || declared.net == settings.net)
    {
        declared.net_secret_file = settings.net_secret_file.clone();
    }
    let Some(path) = &declared.net_secret_file else {
        return Ok(());
    };
    let net = declared.net.as_ref().or(settings.net.as_ref());
    if !matches!(net, Some(Net::Proxy(_) | Net::Profile(_))) {
        return Err(message(
            "--net-secret-file belongs to a proxy; give --net socks5://HOST:PORT, http://HOST:PORT or https://HOST:PORT with it",
            &[],
        ));
    }
    read(path).map(|_| ())
}

pub fn settle(settings: &mut Settings, root: &Path) -> Result<(), String> {
    check(settings.net.as_ref(), settings.net_secret_file.as_deref())?;
    let secret = take(&mut settings.net);
    if secret.is_some() && settings.net_secret_file.is_some() {
        return Err(both());
    }
    if let Some(secret) = secret {
        let path = shared_file(root).map_err(|error| {
            message(
                "cannot keep the proxy's user and password in {path}: {error}",
                &[
                    ("path", root.join(SHARED_DIRECTORY).display().to_string()),
                    ("error", error.to_string()),
                ],
            )
        })?;
        settings.net_secret_file = Some(kept(path, &secret)?);
    }
    if let Some(path) = &settings.net_secret_file {
        read(path)?;
    }
    Ok(())
}

pub fn shadow(config: &mut BTreeMap<String, Value>) {
    if config.contains_key(NET_KEY) && !config.contains_key(FILE_KEY) {
        config.insert(FILE_KEY.to_string(), Value::Null);
    }
}

pub fn settle_config(config: &mut BTreeMap<String, Value>, root: &Path) -> Result<(), String> {
    let mut settings = Settings {
        net: config
            .get(NET_KEY)
            .and_then(|value| serde_json::from_value(value.clone()).ok()),
        net_secret_file: config
            .get(FILE_KEY)
            .and_then(|value| serde_json::from_value(value.clone()).ok()),
        ..Settings::default()
    };
    settle(&mut settings, root)?;
    if let Some(net) = &settings.net {
        config.insert(
            NET_KEY.to_string(),
            serde_json::to_value(net).map_err(|e| e.to_string())?,
        );
    }
    if let Some(path) = &settings.net_secret_file {
        config.insert(
            FILE_KEY.to_string(),
            serde_json::to_value(path).map_err(|e| e.to_string())?,
        );
    }
    shadow(config);
    Ok(())
}

fn redact_value(value: &mut Value) {
    if let Ok(mut net) = serde_json::from_value::<Option<Net>>(value.clone())
        && take(&mut net).is_some()
        && let Ok(shown) = serde_json::to_value(net)
    {
        *value = shown;
    }
}

pub fn settle_stored(graph: &mut crate::objects::Graph, root: &Path) -> Result<(), String> {
    for node in graph.nodes.values_mut() {
        let mut net = node
            .config
            .get(NET_KEY)
            .and_then(|value| serde_json::from_value::<Option<Net>>(value.clone()).ok())
            .flatten();
        if take(&mut net).is_some() {
            node.config.remove(FILE_KEY);
            settle_config(&mut node.config, root)?;
        }
    }
    for view in graph.retired.values_mut() {
        if let Some(value) = view.object.config.get_mut(NET_KEY) {
            redact_value(value);
        }
        if let Some(effective) = view.effective.get_mut(NET_KEY) {
            redact_value(&mut effective.value);
        }
    }
    Ok(())
}

pub fn sweep(root: &Path, used: &[PathBuf]) {
    let Ok(entries) = fs::read_dir(root.join(SHARED_DIRECTORY)) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !used.contains(&path) {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn job_environment(
    declared: &Declared,
    variables: &[(String, String)],
) -> Result<Vec<(String, String)>, String> {
    let secret = match (&declared.net_secret_file, &declared.net) {
        (Some(path), _) => read(path)?,
        (None, Some(Net::Proxy(url))) => match split(url).1 {
            Some(secret) => secret,
            None => return Ok(variables.to_vec()),
        },
        _ => return Ok(variables.to_vec()),
    };
    Ok(variables
        .iter()
        .map(|(name, value)| {
            let with = match value.split_once("://") {
                Some((scheme, place)) => format!("{scheme}://{secret}@{place}"),
                None => value.clone(),
            };
            (name.clone(), with)
        })
        .collect())
}

pub fn for_remote(mut declared: Declared) -> Result<Declared, String> {
    let Some(path) = declared.net_secret_file.take() else {
        return Ok(declared);
    };
    let secret = read(&path)?;
    if let Some(Net::Proxy(url)) = &mut declared.net {
        let shown = split(url).0;
        if let Some((scheme, place)) = shown.split_once("://") {
            let place = place.strip_prefix(MARK).map_or(place, |p| &p[1..]);
            *url = format!("{scheme}://{secret}@{place}");
        }
    }
    Ok(declared)
}
