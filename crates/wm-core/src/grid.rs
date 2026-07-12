//! `Grid`：雨量・雲量を表す2次元数値格子。**移植性の要**。
//!
//! 雨雲レーダー（JMAナウキャストPNG由来）も雲量（Open-Meteo由来）も、
//! この `Grid` に正規化してから `render` へ渡す。`Grid` の「作り方」は
//! プラットフォーム依存（PC=PNG読み, ESP32=数値API）だが、`Grid` を受けた
//! 後段ロジックは共通。

use crate::geo::GeoBBox;
use heapless::Vec;

// グリッド最大寸法。embedded（ESP32）では RAM 節約のため小さくする。
// 256x256x4B = 256KB（PC可）/ 64x64x4B = 16KB（ESP32-C3可）。
#[cfg(not(feature = "embedded"))]
pub const GRID_MAX_W: usize = 256;
#[cfg(not(feature = "embedded"))]
pub const GRID_MAX_H: usize = 256;

#[cfg(feature = "embedded")]
pub const GRID_MAX_W: usize = 64;
#[cfg(feature = "embedded")]
pub const GRID_MAX_H: usize = 64;

/// 格子セルの最大数。
pub const GRID_CAP: usize = GRID_MAX_W * GRID_MAX_H;

/// 格子の値が何を表すか。色マッピングの選択に使う。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GridKind {
    /// 雨量 mm/h（JMAナウキャスト or 数値降水）。
    PrecipMmH,
    /// 雲量 %（Open-Meteo cloud_cover）。
    CloudPct,
}

/// 2次元数値格子（行優先）。固定容量 `heapless::Vec`、alloc 不要。
pub struct Grid {
    pub width: u16,
    pub height: u16,
    pub kind: GridKind,
    /// この格子が覆う地理範囲。セル↔緯度経度対応に使う。
    pub bbox: GeoBBox,
    data: Vec<f32, GRID_CAP>,
}

impl Grid {
    /// 全セル 0.0 で初期化した格子を作る。
    ///
    /// `width * height` が容量を超える場合は `None`。
    pub fn new_zeroed(width: u16, height: u16, kind: GridKind, bbox: GeoBBox) -> Option<Self> {
        let n = width as usize * height as usize;
        if n > GRID_CAP || width == 0 || height == 0 {
            return None;
        }
        let mut data: Vec<f32, GRID_CAP> = Vec::new();
        // 0.0 で埋める。
        for _ in 0..n {
            // 容量チェック済みなので unwrap 相当だが no_std で安全に。
            if data.push(0.0).is_err() {
                return None;
            }
        }
        Some(Self {
            width,
            height,
            kind,
            bbox,
            data,
        })
    }

    #[inline]
    fn idx(&self, x: u16, y: u16) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(y as usize * self.width as usize + x as usize)
    }

    /// セル値を取得（範囲外は None）。
    #[inline]
    pub fn get(&self, x: u16, y: u16) -> Option<f32> {
        self.idx(x, y).map(|i| self.data[i])
    }

    /// セル値を設定（範囲外は false）。
    #[inline]
    pub fn set(&mut self, x: u16, y: u16, v: f32) -> bool {
        match self.idx(x, y) {
            Some(i) => {
                self.data[i] = v;
                true
            }
            None => false,
        }
    }

    /// 最近傍サンプリング：正規化座標 (u,v)∈[0,1) のセル値。
    ///
    /// Braille 量子化で、表示セル内のドット位置を格子へ対応づけるのに使う。
    #[inline]
    pub fn sample_nearest(&self, u: f32, v: f32) -> f32 {
        if self.width == 0 || self.height == 0 {
            return 0.0;
        }
        let x = (u.clamp(0.0, 0.999_999) * self.width as f32) as u16;
        let y = (v.clamp(0.0, 0.999_999) * self.height as f32) as u16;
        self.get(x, y).unwrap_or(0.0)
    }

    /// 値域の最大値（凡例やデバッグ用）。
    pub fn max_value(&self) -> f32 {
        let mut m = f32::MIN;
        for &v in self.data.iter() {
            if v > m {
                m = v;
            }
        }
        if m == f32::MIN {
            0.0
        } else {
            m
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bbox() -> GeoBBox {
        GeoBBox::new(35.0, 139.0, 36.0, 140.0)
    }

    #[test]
    fn create_and_access() {
        let mut g = Grid::new_zeroed(4, 3, GridKind::PrecipMmH, bbox()).unwrap();
        assert_eq!(g.get(0, 0), Some(0.0));
        assert!(g.set(2, 1, 5.5));
        assert_eq!(g.get(2, 1), Some(5.5));
        assert_eq!(g.get(4, 0), None); // 範囲外
    }

    #[test]
    fn oversized_rejected() {
        let too_big = Grid::new_zeroed(GRID_MAX_W as u16, (GRID_MAX_H + 1) as u16, GridKind::CloudPct, bbox());
        assert!(too_big.is_none());
    }

    #[test]
    fn nearest_sampling() {
        let mut g = Grid::new_zeroed(2, 2, GridKind::CloudPct, bbox()).unwrap();
        g.set(0, 0, 1.0);
        g.set(1, 0, 2.0);
        g.set(0, 1, 3.0);
        g.set(1, 1, 4.0);
        assert_eq!(g.sample_nearest(0.1, 0.1), 1.0);
        assert_eq!(g.sample_nearest(0.9, 0.1), 2.0);
        assert_eq!(g.sample_nearest(0.9, 0.9), 4.0);
    }

    #[test]
    fn max_value_works() {
        let mut g = Grid::new_zeroed(3, 3, GridKind::PrecipMmH, bbox()).unwrap();
        g.set(1, 1, 42.0);
        assert_eq!(g.max_value(), 42.0);
    }
}
