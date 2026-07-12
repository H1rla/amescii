//! OpenWeatherMap プロバイダ。欧州系グローバルモデルの第3予報源。
//!
//! Current Weather Data API: https://api.openweathermap.org/data/2.5/weather
//! APIキー必須（無料枠で可）。

use crate::error::{Result, SourceError};
use crate::now_unix;
use crate::traits::{PointMeasurements, WeatherProvider};
use async_trait::async_trait;
use serde::Deserialize;
use wm_core::{Measurement, SourceId};

pub struct OpenWeatherMap {
    client: reqwest::Client,
    api_key: String,
}

impl OpenWeatherMap {
    pub fn new(client: reqwest::Client, api_key: impl Into<String>) -> Self {
        Self {
            client,
            api_key: api_key.into(),
        }
    }
}

#[derive(Deserialize)]
struct OwmResponse {
    main: Option<OwmMain>,
    wind: Option<OwmWind>,
    rain: Option<OwmRain>,
    weather: Option<Vec<OwmWeather>>,
}

#[derive(Deserialize)]
struct OwmMain {
    temp: Option<f32>,
    humidity: Option<f32>,
}

#[derive(Deserialize)]
struct OwmWind {
    speed: Option<f32>,
    deg: Option<f32>,
}

#[derive(Deserialize)]
struct OwmRain {
    /// 直近1時間の降水量 mm。
    #[serde(rename = "1h")]
    one_h: Option<f32>,
}

#[derive(Deserialize)]
struct OwmWeather {
    /// OWM 独自の condition code。
    id: Option<u16>,
}

#[async_trait]
impl WeatherProvider for OpenWeatherMap {
    fn id(&self) -> SourceId {
        SourceId::OpenWeatherMap
    }

    async fn fetch_point(&self, lat: f64, lon: f64) -> Result<PointMeasurements> {
        if self.api_key.is_empty() {
            return Err(SourceError::MissingApiKey("OpenWeatherMap"));
        }
        // units=metric で摂氏・m/s。
        let key = &self.api_key;
        let url = format!(
            "https://api.openweathermap.org/data/2.5/weather?lat={lat:.4}&lon={lon:.4}\
&units=metric&appid={key}"
        );

        let resp: OwmResponse = self.client.get(&url).send().await?.json().await?;
        let now = now_unix();
        let mk = |v: Option<f32>| v.map(|x| Measurement::new(SourceId::OpenWeatherMap, x, now));

        let main = resp.main.unwrap_or(OwmMain {
            temp: None,
            humidity: None,
        });
        let wind = resp.wind.unwrap_or(OwmWind {
            speed: None,
            deg: None,
        });
        let precip = resp.rain.and_then(|r| r.one_h);
        // OWM condition code を WMO 風コードへ大まかに対応づけ（providers/mod で吸収）。
        let owm_code = resp
            .weather
            .as_ref()
            .and_then(|w| w.first())
            .and_then(|w| w.id);

        Ok(PointMeasurements {
            temp_c: mk(main.temp),
            humidity_pct: mk(main.humidity),
            wind_ms: mk(wind.speed),
            wind_dir_deg: mk(wind.deg),
            precip_mmh: mk(precip),
            // OWM コードは WMO ではないので、ここでは大分類だけ別関数で変換する。
            wmo_code: owm_code.map(owm_to_pseudo_wmo),
            source: Some(SourceId::OpenWeatherMap),
        })
    }
}

/// OWM condition code を WeatherCode::from_wmo が解釈できる擬似 WMO コードへ写す。
/// 厳密一致ではなく大分類の対応。
fn owm_to_pseudo_wmo(owm: u16) -> u8 {
    match owm {
        200..=232 => 95, // thunderstorm
        300..=321 => 51, // drizzle
        500..=531 => 61, // rain
        600..=622 => 71, // snow
        701..=781 => 45, // atmosphere → fog 扱い
        800 => 0,        // clear
        801..=802 => 1,  // few/scattered clouds → partly cloudy
        803..=804 => 3,  // broken/overcast → cloudy
        _ => 255,        // unknown
    }
}
