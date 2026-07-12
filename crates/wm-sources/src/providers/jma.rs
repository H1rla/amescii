//! 気象庁 (JMA) プロバイダ。日本の公式・国内基準（静的重み最大）。
//!
//! JMA は地点 API ではなく「府県予報区」単位の JSON を配信する。
//! forecast JSON: https://www.jma.go.jp/bosai/forecast/data/forecast/{area_code}.json
//!
//! 注意：JMA forecast JSON は気温/降水確率/天気を区域コード単位で返し、
//! 風速・湿度の数値は含まれないことが多い。このプロバイダは取得できる指標
//! （気温・天気）だけを Measurement 化し、欠損は None にする。風速・湿度・
//! 降水量(mm/h)は Open-Meteo / OWM 側で補完する設計。
//!
//! 雨雲レーダー（雨量グリッド）は別途 radar.rs の JmaNowcast が担う。

use crate::error::{Result, SourceError};
use crate::now_unix;
use crate::traits::{PointMeasurements, WeatherProvider};
use async_trait::async_trait;
use serde::Deserialize;
use wm_core::{Measurement, SourceId, WeatherCode};

pub struct Jma {
    client: reqwest::Client,
    /// 対象の府県予報区コード（例: "130000" = 東京都）。
    area_code: String,
}

impl Jma {
    pub fn new(client: reqwest::Client, area_code: impl Into<String>) -> Self {
        Self {
            client,
            area_code: area_code.into(),
        }
    }

    /// 緯度経度から最寄りの府県予報区コードを引く（簡易テーブル）。
    /// 本実装では緯度経度→area_code の対応表を別データで持つ想定。
    /// ここでは代表都市のみのフォールバックを用意する。
    pub fn area_for(lat: f64, lon: f64) -> &'static str {
        // ごく簡易な最近傍（主要都市）。実運用では area.json を使う。
        const TABLE: [(f64, f64, &str); 8] = [
            (43.06, 141.35, "016000"), // 札幌（石狩）
            (38.27, 140.87, "040000"), // 仙台（宮城）
            (35.69, 139.69, "130000"), // 東京
            (35.18, 136.91, "230000"), // 名古屋（愛知）
            (34.69, 135.50, "270000"), // 大阪
            (34.39, 132.46, "340000"), // 広島
            (33.59, 130.40, "400000"), // 福岡
            (26.21, 127.68, "471000"), // 那覇（沖縄本島）
        ];
        let mut best = TABLE[2].2;
        let mut best_d = f64::MAX;
        for (la, lo, code) in TABLE.iter() {
            let d = (la - lat) * (la - lat) + (lo - lon) * (lo - lon);
            if d < best_d {
                best_d = d;
                best = code;
            }
        }
        best
    }
}

// JMA forecast JSON は配列。先頭要素に timeSeries があり、その中に
// areas[].temps / weatherCodes などが入る。構造が深いので必要部分だけ拾う。
#[derive(Deserialize)]
struct JmaForecastRoot(Vec<JmaForecast>);

#[derive(Deserialize)]
struct JmaForecast {
    #[serde(rename = "timeSeries")]
    time_series: Vec<JmaTimeSeries>,
}

#[derive(Deserialize)]
struct JmaTimeSeries {
    areas: Vec<JmaArea>,
}

#[derive(Deserialize)]
struct JmaArea {
    /// 天気コード（JMA 独自）。存在する timeSeries にのみある。
    #[serde(default, rename = "weatherCodes")]
    weather_codes: Option<Vec<String>>,
    /// 気温（存在する timeSeries にのみある）。
    #[serde(default)]
    temps: Option<Vec<String>>,
}

#[async_trait]
impl WeatherProvider for Jma {
    fn id(&self) -> SourceId {
        SourceId::Jma
    }

    async fn fetch_point(&self, _lat: f64, _lon: f64) -> Result<PointMeasurements> {
        let url = format!(
            "https://www.jma.go.jp/bosai/forecast/data/forecast/{}.json",
            self.area_code
        );
        let root: JmaForecastRoot = self.client.get(&url).send().await?.json().await?;
        let now = now_unix();

        let (temp_c, condition_code) = extract_temp_and_code(&root, now);

        if temp_c.is_none() && condition_code.is_none() {
            return Err(SourceError::NoData);
        }

        // JMA 天気コードを WMO 擬似コードへ逆変換して wmo_code に載せる
        // （集約の多数決は WeatherCode ベースなので、ここで直接持たせる手もある）。
        let wmo = condition_code.map(weather_to_pseudo_wmo);

        Ok(PointMeasurements {
            temp_c,
            humidity_pct: None, // JMA forecast JSON には通常含まれない
            wind_ms: None,
            wind_dir_deg: None,
            precip_mmh: None, // 降水"量"はレーダー側で扱う
            wmo_code: wmo,
            source: Some(SourceId::Jma),
        })
    }
}

/// forecast JSON ルートから最初に見つかる気温・天気コードを拾う純粋関数。
///
/// JMA forecast JSON は深くネストし、`temps` と `weatherCodes` は別々の
/// timeSeries に分かれて入る（実データ: block[0].timeSeries[0] に weatherCodes、
/// block[0].timeSeries[2] に temps）。位置を決め打ちせず全 timeSeries を走査し、
/// 最初に見つかった値を採用する。HTTP から切り離してテスト可能にしてある。
fn extract_temp_and_code(
    root: &JmaForecastRoot,
    now: u64,
) -> (Option<Measurement>, Option<WeatherCode>) {
    let mut temp_c = None;
    let mut condition_code: Option<WeatherCode> = None;

    for fc in root.0.iter() {
        for ts in fc.time_series.iter() {
            for area in ts.areas.iter() {
                if temp_c.is_none() {
                    if let Some(temps) = &area.temps {
                        if let Some(first) = temps.iter().find(|s| !s.is_empty()) {
                            if let Ok(v) = first.parse::<f32>() {
                                temp_c = Some(Measurement::new(SourceId::Jma, v, now));
                            }
                        }
                    }
                }
                if condition_code.is_none() {
                    if let Some(codes) = &area.weather_codes {
                        if let Some(first) = codes.iter().find(|s| !s.is_empty()) {
                            condition_code = Some(jma_code_to_weather(first));
                        }
                    }
                }
            }
        }
    }

    (temp_c, condition_code)
}

/// JMA 天気コード（文字列）→ WeatherCode。
/// JMA コードは 100番台=晴, 200番台=曇, 300番台=雨, 400番台=雪 が大まかな規則。
fn jma_code_to_weather(code: &str) -> WeatherCode {
    let n: u32 = code.parse().unwrap_or(0);
    match n {
        100..=199 => {
            // 100=晴, 101=晴時々曇 など
            if n == 100 {
                WeatherCode::Clear
            } else {
                WeatherCode::PartlyCloudy
            }
        }
        200..=299 => WeatherCode::Cloudy,
        300..=399 => WeatherCode::Rain,
        400..=499 => WeatherCode::Snow,
        _ => WeatherCode::Unknown,
    }
}

/// WeatherCode → WMO 擬似コード（集約の多数決で from_wmo に通すため）。
fn weather_to_pseudo_wmo(c: WeatherCode) -> u8 {
    match c {
        WeatherCode::Clear => 0,
        WeatherCode::PartlyCloudy => 1,
        WeatherCode::Cloudy => 3,
        WeatherCode::Fog => 45,
        WeatherCode::Drizzle => 51,
        WeatherCode::Rain => 61,
        WeatherCode::Snow => 71,
        WeatherCode::Thunderstorm => 95,
        WeatherCode::Unknown => 255,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 気象庁 forecast/130000.json の実構造を縮約したサンプル。
    // block[0].timeSeries[0] に weatherCodes、[2] に temps が入る点を再現し、
    // 余分なキー（weathers/winds/pops/reliabilities/tempsMin... や block[1]）が
    // あっても serde が無視して必要部分だけ拾えることを検証する。
    const SAMPLE: &str = r#"[
      {
        "publishingOffice": "気象庁",
        "reportDatetime": "2026-06-28T05:00:00+09:00",
        "timeSeries": [
          {
            "timeDefines": ["2026-06-28T05:00:00+09:00","2026-06-29T00:00:00+09:00","2026-06-30T00:00:00+09:00"],
            "areas": [
              {"area":{"name":"東京地方","code":"130010"},
               "weatherCodes":["313","203","200"],
               "weathers":["雨","くもり","くもり"],
               "winds":["北の風","北の風","南の風"],
               "waves":["０．５メートル","０．５メートル","０．５メートル"]}
            ]
          },
          {
            "timeDefines": ["2026-06-28T06:00:00+09:00","2026-06-28T12:00:00+09:00"],
            "areas": [
              {"area":{"name":"東京地方","code":"130010"},"pops":["10","20"]}
            ]
          },
          {
            "timeDefines": ["2026-06-28T00:00:00+09:00","2026-06-28T09:00:00+09:00"],
            "areas": [
              {"area":{"name":"東京","code":"44132"},"temps":["21","23"]}
            ]
          }
        ]
      },
      {
        "publishingOffice": "気象庁",
        "reportDatetime": "2026-06-28T05:00:00+09:00",
        "timeSeries": [
          {
            "timeDefines": ["2026-06-29T00:00:00+09:00"],
            "areas": [
              {"area":{"name":"東京地方","code":"130010"},
               "weatherCodes":["201"],"pops":["30"],"reliabilities":["A"]}
            ]
          },
          {
            "timeDefines": ["2026-06-29T00:00:00+09:00"],
            "areas": [
              {"area":{"name":"東京","code":"44132"},
               "tempsMin":["20"],"tempsMax":["28"]}
            ]
          }
        ],
        "tempAverage": {"areas":[]},
        "precipAverage": {"areas":[]}
      }
    ]"#;

    #[test]
    fn parses_real_jma_structure() {
        let root: JmaForecastRoot =
            serde_json::from_str(SAMPLE).expect("real-shape JMA JSON must deserialize");
        let (temp, code) = extract_temp_and_code(&root, 1000);

        // temps[0]="21" を気温として採用。
        let t = temp.expect("temperature should be extracted from timeSeries[2]");
        assert_eq!(t.value, 21.0);
        assert_eq!(t.source, SourceId::Jma);

        // weatherCodes[0]="313" は 300番台＝雨。
        assert_eq!(code, Some(WeatherCode::Rain));
    }

    #[test]
    fn empty_strings_are_skipped() {
        // temps の先頭が空文字でも次の有効値を拾う。
        let json = r#"[{"timeSeries":[
          {"areas":[{"weatherCodes":["","100"],"temps":["","","19"]}]}
        ]}]"#;
        let root: JmaForecastRoot = serde_json::from_str(json).unwrap();
        let (temp, code) = extract_temp_and_code(&root, 0);
        assert_eq!(temp.unwrap().value, 19.0);
        assert_eq!(code, Some(WeatherCode::Clear)); // 100=晴
    }
}
