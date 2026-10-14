use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub port: u16,
    pub paths: Vec<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 8000,
            paths: Vec::new(),
        }
    }
}

impl Config {
    pub fn file_path() -> PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("lan-share.json")))
            .unwrap_or_else(|| PathBuf::from("lan-share.json"))
    }

    pub fn load() -> Self {
        let path = Self::file_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str::<Config>(&text) {
            Ok(config) => config.sanitized(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::file_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|err| format!("无法创建配置目录：{err}"))?;
        }
        let text =
            serde_json::to_string_pretty(self).map_err(|err| format!("无法保存配置：{err}"))?;
        std::fs::write(&path, text).map_err(|err| format!("无法写入配置：{err}"))
    }

    fn sanitized(mut self) -> Self {
        if self.port == 0 {
            self.port = 8000;
        }
        self.paths.retain(|path| !path.as_os_str().is_empty());
        self
    }
}
