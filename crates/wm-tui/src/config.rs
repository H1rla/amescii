//! 設定の読み込み。起動位置・APIキー・更新間隔。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub startup: Startup,
    #[serde(default)]
    pub sources: Sources,
    #[serde(default)]
    pub refresh: Refresh,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Startup {
    pub lat: f64,
    pub lon: f64,
    pub zoom: u8,
    /// 起動時に雨雲レイヤを表示するか。false で地図＋地名のみ。
    #[serde(default = "default_true")]
    pub show_radar: bool,
}

fn default_true() -> bool {
    true
}

impl Default for Startup {
    fn default() -> Self {
        // 東京駅。
        Self {
            lat: 35.681,
            lon: 139.767,
            zoom: 8,
            show_radar: true,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Sources {
    /// OpenWeatherMap のみキーが必要。空なら OWM をスキップ。
    #[serde(default)]
    pub owm_api_key: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Refresh {
    pub weather_secs: u64,
    pub radar_secs: u64,
}

impl Default for Refresh {
    fn default() -> Self {
        Self {
            weather_secs: 600,
            radar_secs: 300,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            startup: Startup::default(),
            sources: Sources::default(),
            refresh: Refresh::default(),
        }
    }
}

impl Config {
    /// 設定ファイルパス（~/.config/amescii/config.toml）。
    pub fn default_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "amescii")
            .map(|d| d.config_dir().join("config.toml"))
    }

    /// ファイルから読み込む。無ければデフォルト。
    pub fn load() -> Result<Self> {
        if let Some(path) = Self::default_path() {
            if path.exists() {
                let text = std::fs::read_to_string(&path)?;
                let cfg: Config = toml::from_str(&text)?;
                return Ok(cfg);
            }
        }
        Ok(Config::default())
    }

    /// 現在の設定を TOML ファイルに書き出す。
    /// ディレクトリが無ければ作成する。
    pub fn save(&self) -> Result<()> {
        if let Some(path) = Self::default_path() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let text = toml::to_string_pretty(self)?;
            std::fs::write(&path, text)?;
        }
        Ok(())
    }
}
