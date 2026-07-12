//! データモデル。すべて `Copy` で `no_std` 安全。

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// 気象データの取得元。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum SourceId {
    /// 気象庁。日本の公式・国内基準。
    Jma,
    /// Open-Meteo（JMA seamless モデルの別実装）。
    OpenMeteo,
    /// OpenWeatherMap（欧州系グローバルモデル）。
    OpenWeatherMap,
}

impl SourceId {
    /// 表示用の短い名前。
    pub const fn short(self) -> &'static str {
        match self {
            SourceId::Jma => "JMA",
            SourceId::OpenMeteo => "O-M",
            SourceId::OpenWeatherMap => "OWM",
        }
    }

    /// 集約に使う全ソース（期待ソース集合）。
    pub const ALL: [SourceId; 3] = [
        SourceId::Jma,
        SourceId::OpenMeteo,
        SourceId::OpenWeatherMap,
    ];
}

/// 1ソース・1指標の観測値。
///
/// `observed_at` は unix 秒。wm-core はこの値を生成せず、外部から受け取る。
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Measurement {
    pub source: SourceId,
    pub value: f32,
    pub observed_at: u64,
}

impl Measurement {
    pub const fn new(source: SourceId, value: f32, observed_at: u64) -> Self {
        Self {
            source,
            value,
            observed_at,
        }
    }
}

/// 天気状態コード（WMO weather code を簡略化した分類）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum WeatherCode {
    Clear,
    PartlyCloudy,
    Cloudy,
    Fog,
    Drizzle,
    Rain,
    Snow,
    Thunderstorm,
    Unknown,
}

impl WeatherCode {
    pub const fn label_ja(self) -> &'static str {
        match self {
            WeatherCode::Clear => "晴れ",
            WeatherCode::PartlyCloudy => "晴れ時々曇り",
            WeatherCode::Cloudy => "曇り",
            WeatherCode::Fog => "霧",
            WeatherCode::Drizzle => "霧雨",
            WeatherCode::Rain => "雨",
            WeatherCode::Snow => "雪",
            WeatherCode::Thunderstorm => "雷雨",
            WeatherCode::Unknown => "不明",
        }
    }

    /// WMO weather code から分類（Open-Meteo の weathercode 等で使用）。
    pub fn from_wmo(code: u8) -> Self {
        match code {
            0 => WeatherCode::Clear,
            1..=2 => WeatherCode::PartlyCloudy,
            3 => WeatherCode::Cloudy,
            45 | 48 => WeatherCode::Fog,
            51..=57 => WeatherCode::Drizzle,
            61..=67 | 80..=82 => WeatherCode::Rain,
            71..=77 | 85..=86 => WeatherCode::Snow,
            95..=99 => WeatherCode::Thunderstorm,
            _ => WeatherCode::Unknown,
        }
    }
}

/// 1指標の集約結果。
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct AggregatedValue {
    /// 加重平均値。
    pub value: f32,
    /// 変動係数 σ/μ。ソース間の乖離度。
    pub cv: f32,
    /// 信頼度 0.0..=1.0。
    pub confidence: f32,
    /// 集約に使われたソース数。
    pub n_used: u8,
    /// 外れ値として除外されたソース数。
    pub n_excluded: u8,
}

impl Default for AggregatedValue {
    fn default() -> Self {
        Self {
            value: f32::NAN,
            cv: 0.0,
            confidence: 0.0,
            n_used: 0,
            n_excluded: 0,
        }
    }
}

/// 全指標を集約したスナップショット。
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct WeatherSnapshot {
    pub temp_c: AggregatedValue,
    pub humidity_pct: AggregatedValue,
    pub wind_ms: AggregatedValue,
    /// 風向（度、0=北、時計回り）。循環量なのでベクトル平均で算出。
    pub wind_dir_deg: AggregatedValue,
    pub precip_mmh: AggregatedValue,
    pub condition: WeatherCode,
    /// このスナップショットの生成時刻（unix秒、外部から付与）。
    pub generated_at: u64,
}
