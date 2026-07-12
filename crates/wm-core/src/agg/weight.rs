//! 重み計算：静的重み × 新鮮度減衰。

use crate::model::SourceId;

/// 集約パラメータ。すべてユーザー調整可能にしておく。
#[derive(Clone, Copy, Debug)]
pub struct AggParams {
    /// JMA の静的重み。
    pub w_jma: f32,
    /// Open-Meteo の静的重み。
    pub w_open_meteo: f32,
    /// OpenWeatherMap の静的重み。
    pub w_owm: f32,
    /// 新鮮度減衰の時定数（秒）。指標により変える。
    pub tau_secs: f32,
    /// 外れ値判定の閾値。中央値・MAD ベースの修正 z-score に対する上限。
    /// Iglewicz-Hoaglin 推奨の 3.5 を既定とする（古典的 z-score の 2.0 とは
    /// スケールが異なる点に注意）。詳細は agg/outlier.rs を参照。
    pub z_thresh: f32,
    /// 信頼度が 0 になる CV の上限。
    pub cv_max: f32,
    /// 期待ソース数（カバレッジ算出用）。
    pub n_expected: u8,
}

impl AggParams {
    /// 降水・雨量向け：実況性重視で短い時定数。
    pub const fn for_precip() -> Self {
        Self {
            w_jma: 1.0,
            w_open_meteo: 0.9,
            w_owm: 0.8,
            tau_secs: 1800.0, // 30分
            z_thresh: 3.5, // MAD 修正 z-score の閾値（Iglewicz-Hoaglin）
            cv_max: 0.20,
            n_expected: 3,
        }
    }

    /// 気温・湿度・風向け：ゆっくり変化するので長い時定数。
    pub const fn for_slow() -> Self {
        Self {
            w_jma: 1.0,
            w_open_meteo: 0.9,
            w_owm: 0.8,
            tau_secs: 5400.0, // 90分
            z_thresh: 3.5, // MAD 修正 z-score の閾値（Iglewicz-Hoaglin）
            cv_max: 0.20,
            n_expected: 3,
        }
    }

    /// ソースの静的重みを引く。
    pub fn static_weight(&self, s: SourceId) -> f32 {
        match s {
            SourceId::Jma => self.w_jma,
            SourceId::OpenMeteo => self.w_open_meteo,
            SourceId::OpenWeatherMap => self.w_owm,
        }
    }
}

impl Default for AggParams {
    fn default() -> Self {
        Self::for_slow()
    }
}

/// 新鮮度重み：観測からの経過時間で指数減衰。
///
/// `w = exp(-age / tau)`。`no_std` のため `libm::expf` を使用。
#[inline]
pub fn freshness_weight(now: u64, observed_at: u64, tau_secs: f32) -> f32 {
    // 未来時刻（時計ずれ）は age=0 にクランプ。
    let age = now.saturating_sub(observed_at) as f32;
    libm::expf(-age / tau_secs)
}
