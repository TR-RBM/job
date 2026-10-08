mod messages;
pub use messages::message;

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    #[default]
    Ordinary,
    Legacy,
}

impl Profile {
    pub fn legacy(self) -> bool {
        self == Self::Legacy
    }
}

pub fn legacy_record() -> Profile {
    Profile::Legacy
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub presets: crate::presets::Catalog,
    pub schema_version: u32,
    #[serde(default)]
    pub profile: Profile,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pressure: Vec<crate::pressure::control::Rule>,
    #[serde(default, skip_serializing_if = "crate::streams::quota::Service::unset")]
    pub output: crate::streams::quota::Service,
    #[serde(default, skip_serializing_if = "crate::terminal::key::Settings::unset")]
    pub terminal: crate::terminal::key::Settings,
    #[serde(default, skip_serializing_if = "crate::service::Socket::unset")]
    pub socket: crate::service::Socket,
    #[serde(default, skip_serializing_if = "crate::service::Cgroup::unset")]
    pub cgroup: crate::service::Cgroup,
    #[serde(
        default,
        skip_serializing_if = "crate::durability::audit::Settings::is_default"
    )]
    pub audit: crate::durability::audit::Settings,
    #[serde(
        default,
        skip_serializing_if = "crate::operations::journal::Settings::is_default"
    )]
    pub events: crate::operations::journal::Settings,
    #[serde(default, skip_serializing_if = "crate::netpolicy::Network::unset")]
    pub network: crate::netpolicy::Network,
    #[serde(default, skip_serializing_if = "crate::metrics::Settings::unset")]
    pub metrics: crate::metrics::Settings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            presets: Default::default(),
            profile: Profile::Ordinary,
            pressure: Vec::new(),
            output: Default::default(),
            terminal: Default::default(),
            socket: Default::default(),
            cgroup: Default::default(),
            audit: Default::default(),
            events: Default::default(),
            network: Default::default(),
            metrics: Default::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effective {
    pub config: Config,
    pub source: Option<PathBuf>,
    pub automatic_estimation: bool,
    pub conservative_backfill: bool,
    pub implicit_process_limit: Option<u64>,
    pub implicit_swap_limit: Option<u64>,
    pub emergency_host_termination: bool,
    pub host_and_filesystem_floors: bool,
}

impl Config {
    pub fn parse(text: &str) -> io::Result<Self> {
        let config: Self = toml::from_str(text).map_err(io::Error::other)?;
        if config.schema_version != 1 {
            return Err(io::Error::other(message(
                "unsupported configuration schema {version}",
                &[("version", config.schema_version.to_string())],
            )));
        }
        config.presets.definitions().map_err(io::Error::other)?;
        crate::pressure::control::validate(&config.pressure, true).map_err(io::Error::other)?;
        config.output.validate().map_err(io::Error::other)?;
        config.audit.validate().map_err(io::Error::other)?;
        config.events.validate().map_err(io::Error::other)?;
        config.network.validate().map_err(io::Error::other)?;
        config.metrics.validate().map_err(io::Error::other)?;
        config
            .network
            .validate_presets(&config.presets)
            .map_err(io::Error::other)?;
        Ok(config)
    }

    pub fn path() -> io::Result<PathBuf> {
        use crate::paths;
        paths::config_file(paths::mode(&paths::process), &paths::process).map_err(io::Error::other)
    }

    pub fn load() -> io::Result<(Self, Option<PathBuf>)> {
        let path = Self::path()?;
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok((Self::parse(&text)?, Some(path))),
            Err(e)
                if e.kind() == io::ErrorKind::NotFound
                    && !crate::paths::config_explicit(&crate::paths::process) =>
            {
                Ok((Self::default(), None))
            }
            Err(e) => Err(io::Error::other(format!("{}: {e}", path.display()))),
        }
    }

    pub fn effective(&self, source: Option<PathBuf>) -> Effective {
        let legacy = self.profile.legacy();
        Effective {
            config: self.clone(),
            source,
            automatic_estimation: legacy,
            conservative_backfill: legacy,
            implicit_process_limit: legacy.then_some(4096),
            implicit_swap_limit: legacy.then_some(0),
            emergency_host_termination: legacy,
            host_and_filesystem_floors: legacy,
        }
    }

    pub fn initialize(path: &Path, profile: Profile) -> io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let text = toml::to_string_pretty(&Self {
            profile,
            ..Self::default()
        })
        .map_err(io::Error::other)?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_profile_enables_no_implicit_resource_policy() {
        let effective = Config::default().effective(None);
        assert!(!effective.automatic_estimation);
        assert!(!effective.conservative_backfill);
        assert_eq!(effective.implicit_process_limit, None);
        assert_eq!(effective.implicit_swap_limit, None);
        assert!(!effective.emergency_host_termination);
        assert!(!effective.host_and_filesystem_floors);
    }

    #[test]
    fn legacy_profile_is_an_explicit_configuration_choice() {
        let config = Config::parse("schema_version = 1\nprofile = 'legacy'\n").unwrap();
        assert!(config.effective(None).automatic_estimation);
        assert_eq!(config.effective(None).implicit_process_limit, Some(4096));
    }

    #[test]
    fn unknown_configuration_fields_and_versions_are_refused() {
        assert!(Config::parse("schema_version = 2").is_err());
        assert!(Config::parse("schema_version = 1\nprofiel = 'legacy'").is_err());
    }
}
