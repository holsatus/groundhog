use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
};

use directories::ProjectDirs;

use crate::{foundation::BoxError, layout::LayoutConfig, link::LinkBuild, theme::ThemeMode};

static PROJECT_DIRS: LazyLock<ProjectDirs> =
    LazyLock::new(|| directories::ProjectDirs::from("org", "Holsatus", "Groundhog").unwrap());

/// Current on-disk configuration schema version.
const CONFIG_VERSION: u32 = 1;

fn default_config_version() -> u32 {
    CONFIG_VERSION
}

/// Persisted application preferences.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Which palette to use.
    pub theme: ThemeMode,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct Configuration {
    /// Schema version, used to detect configs written by a newer build.
    #[serde(default = "default_config_version")]
    pub version: u32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_config: Option<LinkBuild>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_picker_path: Option<PathBuf>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<LayoutConfig>,

    #[serde(default)]
    pub settings: Settings,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            link_config: None,
            file_picker_path: None,
            layout: None,
            settings: Settings::default(),
        }
    }
}

impl Configuration {
    fn config_path() -> PathBuf {
        PROJECT_DIRS.config_dir().join("config.ron")
    }

    pub fn initialize() -> Result<Self, BoxError> {
        Self::initialize_at(&Self::config_path())
    }

    pub fn write_to_file(&self) -> Result<(), BoxError> {
        self.write_to_file_at(&Self::config_path())
    }

    /// Load the configuration at `path`.
    ///
    /// A missing file is a fresh install: defaults are written and returned.
    /// Any other read error is propagated without touching the file. A file
    /// that reads but does not parse is moved aside (so it survives) before
    /// defaults are written.
    fn initialize_at(path: &Path) -> Result<Self, BoxError> {
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let config = Self::default();
                config.write_to_file_at(path)?;
                return Ok(config);
            }
            Err(error) => return Err(error.into()),
        };

        match ron::from_str::<Configuration>(&contents) {
            Ok(config) => {
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
            Err(error) => {
                let backup = path.with_extension("ron.corrupt");
                log::error!(
                    "Could not parse configuration at {}: {error}; backing it up to {}",
                    path.display(),
                    backup.display()
                );

                if let Err(rename_error) = std::fs::rename(path, &backup) {
                    log::error!(
                        "Could not back up configuration to {}: {rename_error}",
                        backup.display()
                    );
                }

                let config = Self::default();
                config.write_to_file_at(path)?;
                Ok(config)
            }
        }
    }

    /// Write the configuration at `path` atomically: serialize to a sibling
    /// temp file, then rename it over the target.
    fn write_to_file_at(&self, path: &Path) -> Result<(), BoxError> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }

        let serialized = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::new())?;

        let tmp_path = path.with_extension("ron.tmp");
        std::fs::write(&tmp_path, serialized.as_bytes())?;
        std::fs::rename(&tmp_path, path)?;

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
    fn version_less_config_loads_with_current_version() {
        let path = temp_config_path("old");
        // An old config with no `version` field.
        std::fs::write(&path, "(settings: (theme: \"Dark\"))").unwrap();

        let config = Configuration::initialize_at(&path).unwrap();
        assert_eq!(config.version, CONFIG_VERSION);

        cleanup(&path);
    }
}
