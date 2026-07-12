//! プロバイダ trait 定義。

use crate::error::Result;
use async_trait::async_trait;
use wm_core::{GeoBBox, Grid, Measurement, SourceId};

/// 地点天気を取得するプロバイダ（気温・湿度・風・降水など）。
#[async_trait]
pub trait WeatherProvider: Send + Sync {
    fn id(&self) -> SourceId;

    /// 指定地点の観測/予報を取得し、`Measurement` 群へ変換して返す。
    ///
    /// 返す `Measurement` は指標ごとに `source` が自身の `id()` になる。
    /// 指標の種類は呼び出し側が `MeasurementSet` 等で仕分ける（providers/mod 参照）。
    async fn fetch_point(&self, lat: f64, lon: f64) -> Result<PointMeasurements>;
}

/// 1地点・1ソースぶんの各指標観測値。
#[derive(Clone, Debug, Default)]
pub struct PointMeasurements {
    pub temp_c: Option<Measurement>,
    pub humidity_pct: Option<Measurement>,
    pub wind_ms: Option<Measurement>,
    pub wind_dir_deg: Option<Measurement>,
    pub precip_mmh: Option<Measurement>,
    /// WMO weather code（あれば）。集約の多数決に使う。
    pub wmo_code: Option<u8>,
    pub source: Option<SourceId>,
}

/// 雨雲/雲量レーダーを取得するプロバイダ。
#[async_trait]
pub trait RadarProvider: Send + Sync {
    /// 指定 BBox・ズームの雨量/雲量を `Grid` として返す。
    async fn fetch_radar(&self, bbox: GeoBBox, zoom: u8) -> Result<Grid>;
}
