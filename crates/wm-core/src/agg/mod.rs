//! 集約のエントリポイント。
//!
//! アルゴリズム全体は docs/AGGREGATION.md を参照。
//! 流れ：合成重み算出 → 第1パス統計 → 外れ値検出 → 第2パス統計 → CV → 信頼度。

mod outlier;
mod weight;

pub use weight::{freshness_weight, AggParams};

use crate::model::{AggregatedValue, Measurement, SourceId};
use outlier::{
    coefficient_of_variation, mark_outliers, weighted_stats, SampleVec, WeightedSample,
};

/// 後方互換のための型エイリアス（re-export 名）。
pub type Aggregated = AggregatedValue;

/// 複数ソースの同一指標を信頼度つき加重平均で集約する。
///
/// - `measurements`: 同じ指標の観測値群（ソースは混在してよい）
/// - `now`: 現在時刻（unix秒）。**外部から渡す**（wm-core は時刻を取得しない）。
/// - `params`: 重み・閾値などのパラメータ。
pub fn aggregate(measurements: &[Measurement], now: u64, params: &AggParams) -> AggregatedValue {
    // 合成重みつきサンプルを構築。
    let mut samples: SampleVec = SampleVec::new();
    for m in measurements.iter() {
        if !m.value.is_finite() {
            continue;
        }
        let w_static = params.static_weight(m.source);
        let w_fresh = freshness_weight(now, m.observed_at, params.tau_secs);
        let weight = w_static * w_fresh;
        // heapless::Vec への push は容量超過で Err。超過分は捨てる。
        let _ = samples.push(WeightedSample {
            value: m.value,
            weight,
            excluded: false,
        });
    }

    if samples.is_empty() {
        return AggregatedValue::default();
    }

    // 第1パス：有限値の番兵として加重平均だけ確認する（外れ値判定は
    // mark_outliers が中央値・MAD ベースで頑健に行うため σ はここでは不要）。
    let (mean1, _std1, _wsum1, _n1) = weighted_stats(&samples);
    if !mean1.is_finite() {
        return AggregatedValue::default();
    }

    // 外れ値検出（MAD ベース修正 z-score）。
    let n_excluded = mark_outliers(&mut samples, params.z_thresh);

    // 第2パス：生き残りで再計算。
    let (mean2, std2, _wsum2, n_used) = weighted_stats(&samples);
    let value = if mean2.is_finite() { mean2 } else { mean1 };

    let cv = coefficient_of_variation(value, std2);
    let confidence = confidence_score(cv, n_used, n_excluded, params);

    AggregatedValue {
        value,
        cv,
        confidence,
        n_used,
        n_excluded,
    }
}

/// 信頼度スコア = 一致度 × カバレッジ × 外れ値ペナルティ。
fn confidence_score(cv: f32, n_used: u8, n_excluded: u8, p: &AggParams) -> f32 {
    let agreement = (1.0 - cv / p.cv_max).clamp(0.0, 1.0);
    let coverage = (n_used as f32) / (p.n_expected as f32);
    let penalty = 1.0 - 0.15 * (n_excluded as f32);
    (agreement * coverage * penalty).clamp(0.0, 1.0)
}

// ───────────────────────── 風向（循環量）の集約 ─────────────────────────

/// 風向集約の結果。
#[derive(Clone, Copy, Debug)]
pub struct WindDirResult {
    /// 平均風向（度、0..360、0=北）。
    pub dir_deg: f32,
    /// 集中度 R（0..1）。1で完全一致、0でバラバラ。
    pub concentration: f32,
    pub n_used: u8,
}

/// 風向をベクトル平均で集約する（350°と10°の平均が0°になる）。
///
/// 角度は「度・0=北・時計回り」を仮定。三角関数は `libm` 経由。
pub fn aggregate_wind_dir(
    dirs: &[Measurement],
    now: u64,
    params: &AggParams,
) -> WindDirResult {
    let mut x = 0.0f32;
    let mut y = 0.0f32;
    let mut wsum = 0.0f32;
    let mut n = 0u8;

    for m in dirs.iter() {
        if !m.value.is_finite() {
            continue;
        }
        let w = params.static_weight(m.source)
            * freshness_weight(now, m.observed_at, params.tau_secs);
        // 度 → ラジアン。
        let rad = m.value * core::f32::consts::PI / 180.0;
        x += w * libm::cosf(rad);
        y += w * libm::sinf(rad);
        wsum += w;
        n += 1;
    }

    if wsum <= f32::EPSILON || n == 0 {
        return WindDirResult {
            dir_deg: f32::NAN,
            concentration: 0.0,
            n_used: 0,
        };
    }

    let xb = x / wsum;
    let yb = y / wsum;
    let mut deg = libm::atan2f(yb, xb) * 180.0 / core::f32::consts::PI;
    if deg < 0.0 {
        deg += 360.0;
    }
    let r = libm::sqrtf(xb * xb + yb * yb);

    WindDirResult {
        dir_deg: deg,
        concentration: r,
        n_used: n,
    }
}

/// 16方位の日本語表記に変換（サイドバー表示用）。
pub fn compass_16_ja(deg: f32) -> &'static str {
    if !deg.is_finite() {
        return "--";
    }
    const NAMES: [&str; 16] = [
        "北", "北北東", "北東", "東北東", "東", "東南東", "南東", "南南東", "南", "南南西",
        "南西", "西南西", "西", "西北西", "北西", "北北西",
    ];
    let idx = (((deg + 11.25) % 360.0) / 22.5) as usize % 16;
    NAMES[idx]
}

// ───────────────────────── 天気状態の多数決 ─────────────────────────

use crate::model::WeatherCode;

/// (source, code) の組から加重多数決で代表コードを決める。
/// 同票時は最初に最大へ到達したものが残る（呼び出し側で JMA を先頭にすること推奨）。
pub fn vote_condition(votes: &[(SourceId, WeatherCode)], params: &AggParams) -> WeatherCode {
    // WeatherCode 9 種に対する得票を固定配列で集計。
    // index: Clear,PartlyCloudy,Cloudy,Fog,Drizzle,Rain,Snow,Thunderstorm,Unknown
    let mut score = [0.0f32; 9];
    for (src, code) in votes.iter() {
        let w = params.static_weight(*src);
        score[code_index(*code)] += w;
    }
    let mut best = 0usize;
    let mut best_score = -1.0f32;
    for (i, s) in score.iter().enumerate() {
        if *s > best_score {
            best_score = *s;
            best = i;
        }
    }
    index_code(best)
}

const fn code_index(c: WeatherCode) -> usize {
    match c {
        WeatherCode::Clear => 0,
        WeatherCode::PartlyCloudy => 1,
        WeatherCode::Cloudy => 2,
        WeatherCode::Fog => 3,
        WeatherCode::Drizzle => 4,
        WeatherCode::Rain => 5,
        WeatherCode::Snow => 6,
        WeatherCode::Thunderstorm => 7,
        WeatherCode::Unknown => 8,
    }
}

const fn index_code(i: usize) -> WeatherCode {
    match i {
        0 => WeatherCode::Clear,
        1 => WeatherCode::PartlyCloudy,
        2 => WeatherCode::Cloudy,
        3 => WeatherCode::Fog,
        4 => WeatherCode::Drizzle,
        5 => WeatherCode::Rain,
        6 => WeatherCode::Snow,
        7 => WeatherCode::Thunderstorm,
        _ => WeatherCode::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SourceId::*;

    fn m(source: crate::model::SourceId, value: f32, observed_at: u64) -> Measurement {
        Measurement::new(source, value, observed_at)
    }

    #[test]
    fn excludes_outlier() {
        let now = 1000;
        let data = [
            m(Jma, 23.1, 1000),
            m(OpenMeteo, 23.4, 1000),
            m(OpenWeatherMap, 28.0, 1000),
        ];
        let r = aggregate(&data, now, &AggParams::for_slow());
        assert_eq!(r.n_excluded, 1, "28.0 should be excluded");
        assert_eq!(r.n_used, 2);
        assert!((r.value - 23.2).abs() < 0.3, "value={}", r.value);
        assert!(r.confidence > 0.4, "confidence={}", r.confidence);
    }

    #[test]
    fn all_agree_high_confidence() {
        let now = 1000;
        let data = [
            m(Jma, 20.0, 1000),
            m(OpenMeteo, 20.1, 1000),
            m(OpenWeatherMap, 19.9, 1000),
        ];
        let r = aggregate(&data, now, &AggParams::for_slow());
        assert_eq!(r.n_excluded, 0);
        assert_eq!(r.n_used, 3);
        assert!(r.cv < 0.02, "cv={}", r.cv);
        assert!(r.confidence > 0.9, "confidence={}", r.confidence);
    }

    #[test]
    fn stale_data_downweighted() {
        let now = 10_000;
        // JMA は新鮮、OWM は2時間前。
        let fresh = [m(Jma, 10.0, 10_000), m(OpenWeatherMap, 20.0, 10_000)];
        let stale = [m(Jma, 10.0, 10_000), m(OpenWeatherMap, 20.0, 10_000 - 7200)];
        let rf = aggregate(&fresh, now, &AggParams::for_slow());
        let rs = aggregate(&stale, now, &AggParams::for_slow());
        // stale の方が OWM の寄与が小さい → 値が JMA(10) 寄りに下がる。
        assert!(rs.value < rf.value, "stale={}, fresh={}", rs.value, rf.value);
    }

    #[test]
    fn wind_direction_wraps() {
        let now = 0;
        let data = [m(Jma, 350.0, 0), m(OpenMeteo, 10.0, 0)];
        let r = aggregate_wind_dir(&data, now, &AggParams::for_slow());
        // 350 と 10 の循環平均は 0(=360) 付近。
        let d = r.dir_deg;
        assert!(d < 5.0 || d > 355.0, "dir={}", d);
        assert!(r.concentration > 0.9, "R={}", r.concentration);
    }

    #[test]
    fn empty_is_default() {
        let r = aggregate(&[], 0, &AggParams::for_slow());
        assert_eq!(r.n_used, 0);
        assert!(r.value.is_nan());
    }

    #[test]
    fn compass_directions() {
        assert_eq!(compass_16_ja(0.0), "北");
        assert_eq!(compass_16_ja(90.0), "東");
        assert_eq!(compass_16_ja(180.0), "南");
        assert_eq!(compass_16_ja(270.0), "西");
    }
}
