use std::collections::BTreeMap;

use serde_json::Value;

use super::message;

pub const MAX: usize = 32;
pub const KEY_BYTES: usize = 63;
pub const VALUE_BYTES: usize = 255;
pub const CONFIG_KEY: &str = "labels";
pub const UNSET_PREFIX: &str = "label-";

fn key_ok(key: &str) -> bool {
    (1..=KEY_BYTES).contains(&key.len())
        && key.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}

fn value_ok(value: &str) -> bool {
    value.len() <= VALUE_BYTES && !value.chars().any(char::is_control)
}

pub fn pair(text: &str) -> Result<(String, String), String> {
    let refused = || {
        message("`{word}` is not a label; write KEY=VALUE with a key of 1 to 63 characters from a-z, 0-9, dot, underscore and dash and a value of at most 255 bytes without control characters")
            .replace("{word}", &text.chars().filter(|c| !c.is_control()).take(80).collect::<String>())
    };
    let (key, value) = text.split_once('=').ok_or_else(refused)?;
    if !key_ok(key) || !value_ok(value) {
        return Err(refused());
    }
    Ok((key.to_owned(), value.to_owned()))
}

pub fn insert(labels: &mut BTreeMap<String, String>, text: &str) -> Result<(), String> {
    let (key, value) = pair(text)?;
    if labels.insert(key.clone(), value).is_some() {
        return Err(message("label `{word}` is given twice").replace("{word}", &key));
    }
    validate(labels)
}

pub fn validate(labels: &BTreeMap<String, String>) -> Result<(), String> {
    if labels.len() > MAX {
        return Err(message("an object carries at most 32 labels"));
    }
    match labels
        .iter()
        .find(|(key, value)| !key_ok(key) || !value_ok(value))
    {
        Some((key, _)) => Err(message("invalid label `{word}`").replace(
            "{word}",
            &key.chars()
                .filter(|c| !c.is_control())
                .take(80)
                .collect::<String>(),
        )),
        None => Ok(()),
    }
}

fn stored(value: &Value) -> Result<BTreeMap<String, String>, String> {
    let labels: BTreeMap<String, String> = serde_json::from_value(value.clone())
        .map_err(|_| message("labels are stored as a map of text"))?;
    validate(&labels)?;
    Ok(labels)
}

pub fn validate_config(config: &BTreeMap<String, Value>) -> Result<(), String> {
    config
        .get(CONFIG_KEY)
        .map_or(Ok(()), |value| stored(value).map(|_| ()))
}

pub fn merge(
    config: &mut BTreeMap<String, Value>,
    incoming: &mut BTreeMap<String, Value>,
) -> Result<(), String> {
    let Some(added) = incoming.remove(CONFIG_KEY) else {
        return Ok(());
    };
    let mut labels = config
        .get(CONFIG_KEY)
        .map_or_else(|| Ok(BTreeMap::new()), stored)?;
    labels.extend(stored(&added)?);
    validate(&labels)?;
    config.insert(
        CONFIG_KEY.to_owned(),
        serde_json::to_value(labels).map_err(|error| error.to_string())?,
    );
    Ok(())
}

pub fn unset(config: &mut BTreeMap<String, Value>, key: &str) -> bool {
    let Some(name) = key.strip_prefix(UNSET_PREFIX) else {
        return false;
    };
    if let Some(mut labels) = config.get(CONFIG_KEY).and_then(|value| stored(value).ok()) {
        labels.remove(name);
        if labels.is_empty() {
            config.remove(CONFIG_KEY);
        } else if let Ok(value) = serde_json::to_value(labels) {
            config.insert(CONFIG_KEY.to_owned(), value);
        }
    }
    true
}

pub fn line(labels: &BTreeMap<String, String>) -> Option<String> {
    (!labels.is_empty()).then(|| {
        format!(
            "{}: {}",
            message("labels"),
            labels
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}
