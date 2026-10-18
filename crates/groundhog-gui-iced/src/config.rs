use std::path::{Path, PathBuf};

use anyhow::Context;
use groundhog_logic::{
    link::LinkConfig,
    persist::{SaveQueue, read_or_set_aside, write_atomic},
};

use crate::{layout::Workspace, shell::ShellMode, ui::theme::ThemeMode};

/// Current on-disk configuration schema version.
const CONFIG_VERSION: u32 = 2;

fn default_config_version() -> u32 {
    CONFIG_VERSION
}

/// Persisted application preferences.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Which palette to use.
    pub theme: ThemeMode,
    /// How the window is laid out.
    pub shell: ShellMode,
    /// Touch-sized controls in every layout, not only the floating one.
    pub touch_controls: bool,
    /// How much larger than the platform's own scale the interface is drawn.
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
            theme: ThemeMode::Dark,
            shell: ShellMode::default(),
            touch_controls: false,
            scale_factor: 1.0,
            record_telemetry: true,
            default_waypoint_altitude_m:
                groundhog_logic::domain::mission::DEFAULT_WAYPOINT_ALTITUDE_M,
        }
    }
}

/// Where application data (telemetry logs) is kept.
pub fn data_dir() -> Option<PathBuf> {
    crate::platform::data_dir()
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
///
/// This is the composition root for the on-disk schema: it owns the top-level
/// fields and delegates each sub-type's persisted representation to that
/// sub-type (for example, [`Workspace`] arranges the panes and
/// [`LinkConfig`] describes a link). Those types therefore live alongside the
/// data they describe rather than in this module.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct Configuration {
    /// Schema version, used to detect configs written by a newer build.
    #[serde(default = "default_config_version")]
    pub version: u32,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LinkConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_picker_path: Option<PathBuf>,

    /// The panes' arrangement. (Configurations from before the three-column
    /// workspace stored a `layout` instead, which is ignored.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Workspace>,

    #[serde(default)]
    pub settings: Settings,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            links: Vec::new(),
            file_picker_path: None,
            workspace: None,
            settings: Settings::default(),
        }
    }
}

impl Configuration {
    fn config_path() -> anyhow::Result<PathBuf> {
        crate::platform::config_dir()
            .map(|dir| dir.join("config.ron"))
            .context("Unable to construct path to config.ron")
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
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A unique config path inside its own temp directory.
    fn temp_config_path(tag: &str) -> PathBuf {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "groundhog-cfg-{tag}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("config.ron")
    }

    fn cleanup(path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    fn serialized(config: &Configuration) -> String {
        ron::ser::to_string_pretty(config, ron::ser::PrettyConfig::new()).unwrap()
    }

    #[test]
    fn missing_file_writes_default_and_round_trips() {
        let path = temp_config_path("missing");

        let config = Configuration::initialize_at(&path).unwrap();
        assert_eq!(config.version, CONFIG_VERSION);
        assert!(path.exists());

        let loaded = Configuration::initialize_at(&path).unwrap();
        assert_eq!(serialized(&config), serialized(&loaded));

        cleanup(&path);
    }

    #[test]
    fn corrupt_file_is_backed_up_and_default_written() {
        let path = temp_config_path("corrupt");
        let original = "this is not valid ron {{{{";
        std::fs::write(&path, original).unwrap();

        let config = Configuration::initialize_at(&path).unwrap();
        assert_eq!(config.version, CONFIG_VERSION);

        let backup = path.with_extension("ron.corrupt");
        assert!(backup.exists(), "corrupt file should be preserved");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        assert!(path.exists(), "a fresh default should be written");

        cleanup(&path);
    }

    #[test]
    fn links_round_trip_through_serialize() {
        use groundhog_logic::link::LinkBuild;
        use std::net::{Ipv4Addr, SocketAddrV4};

        let mut config = Configuration::default();
        config.links.push(LinkConfig {
            name: "Primary".to_string(),
            auto_connect: true,
            high_latency: true,
            auto_reconnect: false,
            build: LinkBuild::Udp {
                sock_addr: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 14550),
                peer: Some(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 2), 14551)),
            },
        });

        let text = serialized(&config);
        let loaded: Configuration = ron::from_str(&text).unwrap();

        assert_eq!(loaded.links.len(), 1);
        assert_eq!(loaded.links[0].name, "Primary");
        assert!(loaded.links[0].auto_connect);
        assert!(loaded.links[0].high_latency);
        assert_eq!(serialized(&config), serialized(&loaded));

        // The peer survives the round trip.
        let Some(LinkBuild::Udp { peer, .. }) = Some(loaded.links[0].build.clone()) else {
            panic!("the link is a UDP build")
        };
        assert_eq!(
            peer,
            Some(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 2), 14551))
        );

        // A config written before peers existed loads as a listening link.
        let text_without_peer = r#"(
            links: [(
                name: "Old",
                auto_connect: false,
                high_latency: false,
                build: Udp(sock_addr: "127.0.0.1:14550"),
            )],
        )"#;
        let loaded: Configuration = ron::from_str(text_without_peer).unwrap();
        let Some(LinkBuild::Udp { peer, .. }) = Some(loaded.links[0].build.clone()) else {
            panic!("the old link is a UDP build")
        };
        assert_eq!(peer, None, "an absent peer defaults to a listening link");
    }

    #[test]
    fn version_less_config_loads_with_current_version() {
        let path = temp_config_path("old");
        // An old config with no `version` field.
        std::fs::write(&path, "(settings: (theme: \"Dark\"))").unwrap();

        let config = Configuration::initialize_at(&path).unwrap();
        assert_eq!(config.version, CONFIG_VERSION);

        cleanup(&path);
    }
}
