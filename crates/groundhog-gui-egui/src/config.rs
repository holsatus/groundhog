//! Persisted application configuration and where it lives.
//!
//! The configuration holds links, planning settings and recording. The egui
//! frontend keeps its own file, so two frontends can run at once without
//! racing each other.

use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
};

use anyhow::Context;
use directories::ProjectDirs;
use groundhog_logic::{
    link::LinkConfig,
    persist::{SaveQueue, read_or_set_aside, write_atomic},
};

static PROJECT_DIRS: LazyLock<Option<ProjectDirs>> =
    LazyLock::new(|| directories::ProjectDirs::from("org", "Holsatus", "Groundhog"));

/// Current on-disk configuration schema version.
const CONFIG_VERSION: u32 = 1;

fn default_config_version() -> u32 {
    CONFIG_VERSION
}

/// Persisted application preferences.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Dark or light egui theme.
    pub dark: bool,
    pub scale_factor: f32,
    /// Record every received frame to a `.tlog` file per session.
    pub record_telemetry: bool,
    /// The altitude above home of the first waypoint of a plan. Later ones
    /// take the altitude of the waypoint before them.
    pub default_waypoint_altitude_m: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark: true,
            scale_factor: 1.0,
            record_telemetry: true,
            default_waypoint_altitude_m:
                groundhog_logic::domain::mission::DEFAULT_WAYPOINT_ALTITUDE_M,
        }
    }
}

/// Where application data (telemetry logs) is kept.
pub fn data_dir() -> Option<PathBuf> {
    PROJECT_DIRS
        .as_ref()
        .map(|dirs| dirs.data_dir().to_path_buf())
}

/// Persists configurations one at a time, in order, off the UI thread (see
/// [`SaveQueue`]).
pub type ConfigWriter = SaveQueue<Configuration>;

/// Start the configuration writer on the current async runtime. The returned
/// receiver yields the outcome of each write.
pub fn spawn_writer() -> (
    ConfigWriter,
    tokio::sync::mpsc::UnboundedReceiver<Result<(), String>>,
) {
    SaveQueue::spawn(|configuration: Configuration| {
        configuration
            .write_to_file()
            .map_err(|error| error.to_string())
    })
}

/// The persisted application configuration.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct Configuration {
    /// Schema version, used to detect configs written by a newer build.
    #[serde(default = "default_config_version")]
    pub version: u32,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LinkConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_picker_path: Option<PathBuf>,

    #[serde(default)]
    pub settings: Settings,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            links: Vec::new(),
            file_picker_path: None,
            settings: Settings::default(),
        }
    }
}

impl Configuration {
    fn config_path() -> anyhow::Result<PathBuf> {
        PROJECT_DIRS
            .as_ref()
            .map(|dirs| dirs.config_dir().join("config-egui.ron"))
            .context("Unable to construct path to config-egui.ron")
    }

    pub fn initialize() -> anyhow::Result<Self> {
        Self::initialize_at(&Self::config_path()?)
    }

    pub fn write_to_file(&self) -> anyhow::Result<()> {
        self.write_to_file_at(&Self::config_path()?)
    }

    /// Load the configuration at `path`.
    ///
    /// A missing file is a fresh install: defaults are written and returned.
    /// Any other read error is propagated without touching the file. A file
    /// that reads but does not parse is moved aside (so it survives) before
    /// defaults are written.
    fn initialize_at(path: &Path) -> anyhow::Result<Self> {
        let Some(config) = read_or_set_aside(path, |text| ron::from_str::<Configuration>(text))?
        else {
            let config = Self::default();
            config.write_to_file_at(path)?;
            return Ok(config);
        };

        if config.version > CONFIG_VERSION {
            log::warn!(
                "Configuration at {} is version {} from a newer build (current {}); loading anyway",
                path.display(),
                config.version,
                CONFIG_VERSION
            );
        }
        Ok(config)
    }

    fn write_to_file_at(&self, path: &Path) -> anyhow::Result<()> {
        let serialized = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::new())?;
        write_atomic(path, serialized.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_round_trips() {
        let mut config = Configuration::default();
        config.links.push(LinkConfig {
            name: "Primary".to_string(),
            auto_connect: true,
            high_latency: false,
            auto_reconnect: true,
            build: groundhog_logic::link::LinkBuild::Udp {
                sock_addr: std::net::SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 14550),
                peer: None,
            },
        });

        let text = ron::ser::to_string_pretty(&config, ron::ser::PrettyConfig::new()).unwrap();
        let loaded: Configuration = ron::from_str(&text).unwrap();

        assert_eq!(loaded.version, CONFIG_VERSION);
        assert_eq!(loaded.links.len(), 1);
        assert_eq!(loaded.settings, config.settings);
    }
}
