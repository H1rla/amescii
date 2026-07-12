//! 外れ値検出と重み付き統計量。

use heapless::Vec;

/// 集約に使う中間表現：値・合成重み・除外フラグ。
#[derive(Clone, Copy)]
pub struct WeightedSample {
    pub value: f32,
    pub weight: f32,
    pub excluded: bool,
}

/// 集約で扱える最大ソース数（現状3だが余裕を持たせる）。
pub const MAX_SAMPLES: usize = 8;

pub type SampleVec = Vec<WeightedSample, MAX_SAMPLES>;

/// 重み付き平均と重み付き標準偏差を返す（除外サンプルは無視）。
///
/// 戻り値 `(mean, std, weight_sum, n_used)`。
pub fn weighted_stats(samples: &SampleVec) -> (f32, f32, f32, u8) {
    let mut wsum = 0.0f32;
    let mut wval = 0.0f32;
    let mut n: u8 = 0;

    for s in samples.iter() {
        if s.excluded {
            continue;
        }
        wsum += s.weight;
        wval += s.weight * s.value;
        n += 1;
    }

    if wsum <= f32::EPSILON || n == 0 {
        return (f32::NAN, 0.0, 0.0, 0);
    }

    let mean = wval / wsum;

    let mut wvar = 0.0f32;
    for s in samples.iter() {
        if s.excluded {
            continue;
        }
        let d = s.value - mean;
        wvar += s.weight * d * d;
    }
    let var = wvar / wsum;
    let std = libm::sqrtf(var);

    (mean, std, wsum, n)
}

/// MAD ベースの修正 z-score による外れ値検出。
///
/// 「全体平均・全体σ」での通常の z-score は、外れ値自身がσを膨らませて
/// 自分を隠す（masking）。特に n=3 の小標本では明白な外れ値（例: 23.1/23.4
/// に対する 28.0）でも z が z_thresh=2.0 を構造的に超えられず検出に失敗する。
/// そこで中央値と MAD（中央絶対偏差）を使う頑健な修正 z-score を採用する。
///   Iglewicz & Hoaglin (1993):  Mi = 0.6745 · (xi - median) / MAD
/// MAD≈0（過半数が同値）の場合は平均絶対偏差 (MeanAD) にフォールバックする。
/// 重みは最終値の加重平均には使うが、外れ値判定は値の分布のみで頑健に行う。
///
/// 戻り値：新たに除外した個数。
pub fn mark_outliers(samples: &mut SampleVec, z_thresh: f32) -> u8 {
    // 生存サンプルの値を集める。
    let mut vals: SampleValues = SampleValues::new();
    for s in samples.iter() {
        if !s.excluded {
            let _ = vals.push(s.value);
        }
    }
    let n = vals.len();
    // 2点以下では「どちらが外れ値か」を統計的に決められない → 除外しない。
    if n < 3 {
        return 0;
    }

    let median = median_of(&mut vals[..]);

    // MAD = median(|xi - median|)
    let mut devs: SampleValues = SampleValues::new();
    for &v in vals.iter() {
        let _ = devs.push(libm::fabsf(v - median));
    }
    let mad = median_of(&mut devs[..]);

    // スケール推定量と正規化係数。MAD≈0 のときは MeanAD にフォールバック。
    // 係数は正規分布で σ に一致する Iglewicz-Hoaglin の定数。
    let (scale, k) = if mad > f32::EPSILON {
        (mad, 0.6745f32)
    } else {
        let mut sum = 0.0f32;
        for &v in vals.iter() {
            sum += libm::fabsf(v - median);
        }
        (sum / n as f32, 1.0f32 / 1.253314f32)
    };

    // ばらつきが無い（全値ほぼ同一）なら誰も除外しない。
    if scale <= f32::EPSILON {
        return 0;
    }

    let mut excluded = 0u8;
    for s in samples.iter_mut() {
        if s.excluded {
            continue;
        }
        let mz = k * libm::fabsf(s.value - median) / scale;
        if mz > z_thresh {
            s.excluded = true;
            excluded += 1;
        }
    }
    excluded
}

/// 中央値計算用の f32 バッファ（生存サンプルの値・偏差を保持）。
type SampleValues = Vec<f32, MAX_SAMPLES>;

/// スライスを破壊的にソートして中央値を返す（呼び出し側はコピーを渡す）。
/// no_std のため `sort_unstable_by`（core, alloc 不要）を使用。
fn median_of(v: &mut [f32]) -> f32 {
    v.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
    let n = v.len();
    if n == 0 {
        return f32::NAN;
    }
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// 変動係数 CV = std / |mean|。mean がほぼ 0 のときは 0 を返す。
#[inline]
pub fn coefficient_of_variation(mean: f32, std: f32) -> f32 {
    let denom = libm::fabsf(mean);
    if denom <= f32::EPSILON {
        0.0
    } else {
        std / denom
    }
}
