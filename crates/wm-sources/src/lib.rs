//! # wm-sources
//!
//! std 層。各気象 API を叩き、JSON/PNG を `wm-core` のデータ型へ変換する。
//!
//! 責務の境界（移植性の維持）:
//! - HTTP・JSON パース・PNG デコードはすべてここ（std依存）で行う。
//! - 現在時刻の取得（`SystemTime::now`）もここで行い、`wm-core` には `u64` を渡す。
//! - 変換後は `Measurement` / `Grid` という抽象型だけを返し、後段は `wm-core` に委ねる。

pub mod basemap;
pub mod cache;
pub mod error;
pub mod providers;
pub mod radar;
pub mod traits;

pub use cache::{SharedCache, TileCache};
pub use error::SourceError;
pub use traits::{RadarProvider, WeatherProvider};

use std::time::{SystemTime, UNIX_EPOCH};

/// 現在 unix 秒。`wm-core` に渡す時刻はすべてここ経由で取得する。
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
