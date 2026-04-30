use std::{io::Write as _, path::PathBuf, sync::LazyLock};

use directories::ProjectDirs;

use crate::{BoxError, connection::LinkConfig};


static PROJECT_DIRS: LazyLock<ProjectDirs> =
    LazyLock::new(|| directories::ProjectDirs::from("org", "Holsatus", "Groundhog").unwrap());


#[derive(Debug, serde::Serialize, serde::Deserialize, Default, Clone)]
pub struct Configuration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_config: Option<LinkConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_picker_path: Option<PathBuf>,
}

impl Configuration {
    pub fn initialize() -> Result<Self, BoxError> {
        let project_path = PROJECT_DIRS.config_dir();
        let config_path = project_path.join("config.ron");
    
        Ok(match std::fs::read_to_string(&config_path) {
            Ok(contents) => {
                log::debug!("Loaded configuration file from: {config_path:?}");
                ron::from_str::<'_, Configuration>(&contents)?
            }
            Err(_) => {
                let config = Self::default();
                config.write_to_file()?;
                config
            }
        })
    }
    
    pub fn write_to_file(&self) -> Result<(), BoxError> {
        let project_path = PROJECT_DIRS.config_dir();
        let config_path = project_path.join("config.ron");
    
        std::fs::create_dir_all(project_path)?;
        let mut file = std::fs::File::create(&config_path)?;
    
        let serialized = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::new())?;
    
        file.write_all(serialized.as_bytes())?;
    
        Ok(())
    }
}


