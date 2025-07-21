use std::{fs, path::Path};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone)]
pub struct Config {
    pub monitor: usize,
    pub fps: u32,
    pub resolution: [u32; 2],
    pub clip_path: String,
}

impl Config {
    pub fn with_clip_path(clip_path: &Path) -> Self {
        Self {
            monitor: 0,
            fps: 60,
            resolution: [720, 1280],
            clip_path: clip_path.to_string_lossy().into_owned(),
        }
    }

    pub fn new() -> Self {
        let project_directory =
            ProjectDirs::from("", "", "clipper").expect("could not make config folder");
        fs::create_dir_all(project_directory.config_dir())
            .expect("failed to create config directory");

        let mut config_path = project_directory.config_dir().to_path_buf();
        config_path.push("config.toml");

        fs::read_to_string(config_path.clone())
            .ok()
            .and_then(|s| toml::from_str(&s).expect("couldn't parse toml"))
            .unwrap_or_else(|| {
                let default = Self::with_clip_path(project_directory.data_dir());
                let toml = toml::to_string_pretty(&default).expect("failed to encode config");
                fs::write(config_path, toml).expect("failed to write config");
                default
            })
    }
}
