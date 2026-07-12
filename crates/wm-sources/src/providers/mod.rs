//! プロバイダ群と、全ソースを集約して `WeatherSnapshot` を作る高水準関数。

pub mod jma;
pub mod open_meteo;
pub mod owm;

pub use jma::Jma;
pub use open_meteo::OpenMeteo;
pub use owm::OpenWeatherMap;

use crate::error::Result;
use crate::now_unix;
use crate::traits::{PointMeasurements, WeatherProvider};
use wm_core::agg::{aggregate, aggregate_wind_dir, vote_condition, AggParams};
use wm_core::model::{AggregatedValue, WeatherCode, WeatherSnapshot};
use wm_core::{Measurement, SourceId};

/// 全プロバイダから取得し、`wm-core` で集約して 1 つの `WeatherSnapshot` にする。
///
/// 各プロバイダの取得失敗は握りつぶし（そのソースを欠損扱い）、
/// 取得できたソースだけで集約する。これにより 1 API が落ちても動作継続。
pub async fn fetch_and_aggregate(
    providers: &[Box<dyn WeatherProvider>],
    lat: f64,
    lon: f64,
) -> Result<WeatherSnapshot> {
    // 各プロバイダを並列取得（1つの API が遅くても全体を待たせない）。
    let futs: Vec<_> = providers.iter().map(|p| p.fetch_point(lat, lon)).collect();
    let results = futures::future::join_all(futs).await;

    // 取得できたソースだけ集約に回す（失敗ソースは欠損扱いで継続）。
    let mut points: Vec<PointMeasurements> = Vec::new();
    for r in results {
        if let Ok(pm) = r {
            points.push(pm);
        }
    }

    Ok(aggregate_points(&points, now_unix()))
}

/// 取得済みの各ソース観測値を `wm-core` で集約する純粋手続き（テスト可能）。
///
/// 時刻 `now`（unix秒）は呼び出し側から渡す。新鮮度減衰の基準になるため、
/// 観測値の `observed_at` と整合する時刻でなければ全重みが 0 に潰れる。
pub fn aggregate_points(points: &[PointMeasurements], now: u64) -> WeatherSnapshot {
    // 指標ごとに Measurement を集める。
    let collect = |sel: fn(&PointMeasurements) -> Option<Measurement>| -> Vec<Measurement> {
        points.iter().filter_map(sel).collect()
    };

    let temps = collect(|p| p.temp_c);
    let hums = collect(|p| p.humidity_pct);
    let winds = collect(|p| p.wind_ms);
    let wind_dirs = collect(|p| p.wind_dir_deg);
    let precips = collect(|p| p.precip_mmh);

    let temp_c = aggregate(&temps, now, &AggParams::for_slow());
    let humidity_pct = aggregate(&hums, now, &AggParams::for_slow());
    let wind_ms = aggregate(&winds, now, &AggParams::for_slow());
    let precip_mmh = aggregate(&precips, now, &AggParams::for_precip());

    // 風向は循環量なのでベクトル平均。AggregatedValue 形へ詰め替える。
    let wd = aggregate_wind_dir(&wind_dirs, now, &AggParams::for_slow());
    let wind_dir_deg = AggregatedValue {
        value: wd.dir_deg,
        cv: 1.0 - wd.concentration, // 集中度の逆を乖離度に
        confidence: wd.concentration,
        n_used: wd.n_used,
        n_excluded: 0,
    };

    // 天気状態：各ソースの wmo_code を WeatherCode に直して加重多数決。
    // JMA を先頭にして同票時に優先されるようにする。
    let mut votes: Vec<(SourceId, WeatherCode)> = Vec::new();
    for p in points.iter() {
        if let (Some(src), Some(code)) = (p.source, p.wmo_code) {
            votes.push((src, WeatherCode::from_wmo(code)));
        }
    }
    votes.sort_by_key(|(s, _)| match s {
        SourceId::Jma => 0,
        SourceId::OpenMeteo => 1,
        SourceId::OpenWeatherMap => 2,
    });
    let condition = if votes.is_empty() {
        WeatherCode::Unknown
    } else {
        vote_condition(&votes, &AggParams::for_slow())
    };

    WeatherSnapshot {
        temp_c,
        humidity_pct,
        wind_ms,
        wind_dir_deg,
        precip_mmh,
        condition,
        generated_at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pm(
        src: SourceId,
        temp: Option<f32>,
        wind: Option<f32>,
        dir: Option<f32>,
        code: Option<u8>,
    ) -> PointMeasurements {
        let now = 1000;
        PointMeasurements {
            temp_c: temp.map(|v| Measurement::new(src, v, now)),
            humidity_pct: None,
            wind_ms: wind.map(|v| Measurement::new(src, v, now)),
            wind_dir_deg: dir.map(|v| Measurement::new(src, v, now)),
            precip_mmh: None,
            wmo_code: code,
            source: Some(src),
        }
    }

    #[test]
    fn aggregates_three_sources() {
        let points = [
            pm(SourceId::Jma, Some(23.1), Some(3.0), Some(0.0), Some(0)),
            pm(SourceId::OpenMeteo, Some(23.4), Some(3.2), Some(10.0), Some(1)),
            // wmo_code は擬似 WMO（owm_to_pseudo_wmo 後の値）。OWM 801(few clouds)→1。
            pm(SourceId::OpenWeatherMap, Some(23.5), Some(2.8), Some(350.0), Some(1)),
        ];
        let snap = aggregate_points(&points, 1000);
        assert!(snap.temp_c.value > 23.0 && snap.temp_c.value < 23.6);
        assert!(snap.temp_c.n_used >= 2);
        // 風向は 0,10,350 の循環平均 → 0付近。
        let d = snap.wind_dir_deg.value;
        assert!(d < 10.0 || d > 350.0, "wind dir={}", d);
    }

    #[test]
    fn survives_missing_source() {
        // 1ソースだけ。
        let points = [pm(SourceId::Jma, Some(20.0), None, None, Some(0))];
        let snap = aggregate_points(&points, 1000);
        assert_eq!(snap.temp_c.n_used, 1);
        assert_eq!(snap.condition, WeatherCode::Clear);
    }
}
