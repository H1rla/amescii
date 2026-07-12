//! Open-Meteo プロバイダ。JMA seamless モデルを別実装で提供する第2予報源。
//!
//! ドキュメント: https://open-meteo.com/en/docs
//! 認証不要。`current=` で現況、`hourly=...&models=jma_seamless` でモデル指定。

use crate::error::{Result, SourceError};
use crate::now_unix;
use crate::traits::{PointMeasurements, WeatherProvider};
use async_trait::async_trait;
use serde::Deserialize;
use wm_core::{Measurement, SourceId};

pub struct OpenMeteo {
    client: reqwest::Client,
}

impl OpenMeteo {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[derive(Deserialize)]
struct OmResponse {
    current: Option<OmCurrent>,
}

#[derive(Deserialize)]
struct OmCurrent {
    #[serde(default)]
    temperature_2m: Option<f32>,
    #[serde(default)]
    relative_humidity_2m: Option<f32>,
    #[serde(default)]
    wind_speed_10m: Option<f32>,
    #[serde(default)]
    wind_direction_10m: Option<f32>,
    #[serde(default)]
    precipitation: Option<f32>,
    #[serde(default)]
    weather_code: Option<u8>,
}

#[async_trait]
impl WeatherProvider for OpenMeteo {
    fn id(&self) -> SourceId {
        SourceId::OpenMeteo
    }

    async fn fetch_point(&self, lat: f64, lon: f64) -> Result<PointMeasurements> {
        // JMA seamless モデルを明示し、現況の各指標を取得。
        // wind_speed は m/s に統一（wind_speed_unit=ms）。
        let vars = "temperature_2m,relative_humidity_2m,wind_speed_10m,\
wind_direction_10m,precipitation,weather_code";
        let url = format!(
            "https://api.open-meteo.com/v1/forecast?latitude={lat:.4}&longitude={lon:.4}\
&current={vars}&wind_speed_unit=ms&models=jma_seamless&timezone=Asia%2FTokyo"
        );

        let resp: OmResponse = self.client.get(&url).send().await?.json().await?;
        let cur = resp.current.ok_or(SourceError::NoData)?;
        let now = now_unix();

        let mk = |v: Option<f32>| v.map(|x| Measurement::new(SourceId::OpenMeteo, x, now));

        Ok(PointMeasurements {
            temp_c: mk(cur.temperature_2m),
            humidity_pct: mk(cur.relative_humidity_2m),
            wind_ms: mk(cur.wind_speed_10m),
            wind_dir_deg: mk(cur.wind_direction_10m),
            precip_mmh: mk(cur.precipitation),
            wmo_code: cur.weather_code,
            source: Some(SourceId::OpenMeteo),
        })
    }
}
