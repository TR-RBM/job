use serde::{Deserialize, Serialize};

use super::message;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DetachKey(Option<u8>);

impl Default for DetachKey {
    fn default() -> Self {
        Self(Some(0x1d))
    }
}

impl TryFrom<String> for DetachKey {
    type Error = String;
    fn try_from(text: String) -> Result<Self, String> {
        Self::parse(&text)
    }
}

impl From<DetachKey> for String {
    fn from(key: DetachKey) -> Self {
        match key.name() {
            Some(name) => format!("ctrl-{}", name.to_ascii_lowercase()),
            None => "none".to_owned(),
        }
    }
}

impl DetachKey {
    pub fn parse(text: &str) -> Result<Self, String> {
        let lower = text.to_ascii_lowercase();
        if lower == "none" {
            return Ok(Self(None));
        }
        let mut name = lower.strip_prefix("ctrl-").unwrap_or("").chars();
        match (name.next(), name.next()) {
            (Some(key @ ('a'..='z' | ']' | '\\' | '^' | '_')), None) => {
                Ok(Self(Some(key.to_ascii_uppercase() as u8 & 0x1f)))
            }
            _ => Err(message(
                "detach key `{key}` is not ctrl-LETTER, ctrl-], ctrl-\\, ctrl-^, ctrl-_ or none",
                &[("key", text.to_owned())],
            )),
        }
    }

    pub fn byte(self) -> Option<u8> {
        self.0
    }

    fn name(self) -> Option<char> {
        self.0.map(|byte| (byte | 0x40) as char)
    }

    pub fn label(self) -> Option<String> {
        self.name()
            .map(|name| message("Ctrl-{key}", &[("key", name.to_string())]))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detach_key: Option<DetachKey>,
}

impl Settings {
    pub fn unset(&self) -> bool {
        self.detach_key.is_none()
    }
}

pub fn resolve(
    option: Option<&str>,
    configured: impl FnOnce() -> Option<DetachKey>,
) -> Result<DetachKey, String> {
    if let Some(text) = option {
        return DetachKey::parse(text);
    }
    if let Some(text) = std::env::var("JOB_DETACH_KEY")
        .ok()
        .filter(|v| !v.is_empty())
    {
        return DetachKey::parse(&text);
    }
    Ok(configured().unwrap_or_default())
}
