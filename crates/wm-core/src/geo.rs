//! 地理座標変換：lat/lon ↔ Web Mercator タイル ↔ ピクセル。
//!
//! OSM/JMA タイルは Web Mercator (EPSG:3857) のスリッピーマップ方式。
//! 検証値は OpenStreetMap wiki "Slippy map tilenames" 準拠。
//! 三角関数・対数は `no_std` のため `libm` 経由。

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use core::f64::consts::PI;

/// 地理的バウンディングボックス（緯度経度）。
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GeoBBox {
    pub min_lat: f64,
    pub min_lon: f64,
    pub max_lat: f64,
    pub max_lon: f64,
}

impl GeoBBox {
    pub const fn new(min_lat: f64, min_lon: f64, max_lat: f64, max_lon: f64) -> Self {
        Self {
            min_lat,
            min_lon,
            max_lat,
            max_lon,
        }
    }

    /// 中心緯度経度。
    pub fn center(&self) -> (f64, f64) {
        (
            (self.min_lat + self.max_lat) * 0.5,
            (self.min_lon + self.max_lon) * 0.5,
        )
    }

    /// 日本本土を覆う既定 BBox（フォールバック用）。
    pub const fn japan() -> Self {
        Self::new(24.0, 122.0, 46.0, 146.0)
    }
}

/// タイル座標（整数）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct TileCoord {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

/// 緯度経度 → タイル座標（整数、切り捨て）。
///
/// ズーム z における経度→x, 緯度→y のスリッピーマップ標準式。
pub fn lonlat_to_tile(lat_deg: f64, lon_deg: f64, z: u8) -> TileCoord {
    let n = tiles_per_axis(z);
    // no_std のため度→ラジアン・ln・floor は std メソッドを使わず手計算/libm 経由。
    let lat_rad = lat_deg * PI / 180.0;

    let x = libm::floor((lon_deg + 180.0) / 360.0 * n);
    // y = (1 - asinh(tan φ)/π)/2 · n。asinh(tan φ) = ln(tan φ + sec φ)。
    let y = libm::floor(
        (1.0 - libm::log(libm::tan(lat_rad) + 1.0 / libm::cos(lat_rad)) / PI) / 2.0 * n,
    );

    TileCoord {
        z,
        x: clamp_tile(x, n),
        y: clamp_tile(y, n),
    }
}

/// タイル左上角の緯度経度（タイル境界の座標復元）。
pub fn tile_nw_corner(tile: TileCoord) -> (f64, f64) {
    tile_frac_to_lonlat(tile.z, tile.x as f64, tile.y as f64)
}

/// 分数タイル座標 (x, y) → 緯度経度。スリッピーマップ逆変換。
///
/// `x, y` はズーム `z` における分数タイル座標（整数タイル + タイル内オフセット）。
/// 例：MVT のタイルローカル座標 `(lx, ly) ∈ [0, extent)` は
/// `x = tile_x + lx/extent`, `y = tile_y + ly/extent` として渡す。
/// `tile_nw_corner` の一般化版（重複実装を避けるためこちらに集約）。no_std/libm。
pub fn tile_frac_to_lonlat(z: u8, x: f64, y: f64) -> (f64, f64) {
    let n = tiles_per_axis(z);
    let lon = x / n * 360.0 - 180.0;
    let lat_rad = libm::atan(libm::sinh(PI * (1.0 - 2.0 * y / n)));
    (lat_rad * 180.0 / PI, lon)
}

/// 緯度経度 → グローバルピクセル座標（タイル内オフセット計算用）。
///
/// 1タイル = `tile_size` px（通常256）。戻り値は (px_x, px_y) の浮動小数。
pub fn lonlat_to_pixel(lat_deg: f64, lon_deg: f64, z: u8, tile_size: f64) -> (f64, f64) {
    let n = tiles_per_axis(z);
    let lat_rad = lat_deg * PI / 180.0;
    let world = n * tile_size;
    let px = (lon_deg + 180.0) / 360.0 * world;
    let py = (1.0 - libm::log(libm::tan(lat_rad) + 1.0 / libm::cos(lat_rad)) / PI) / 2.0 * world;
    (px, py)
}

/// BBox を覆うのに必要なタイル範囲 (x_min,y_min)..=(x_max,y_max) を返す。
pub fn bbox_to_tile_range(bbox: &GeoBBox, z: u8) -> (TileCoord, TileCoord) {
    // 北西角（max_lat, min_lon）→ 左上タイル、南東角 → 右下タイル。
    let nw = lonlat_to_tile(bbox.max_lat, bbox.min_lon, z);
    let se = lonlat_to_tile(bbox.min_lat, bbox.max_lon, z);
    (nw, se)
}

#[inline]
fn tiles_per_axis(z: u8) -> f64 {
    // 2^z。z は実用上 0..=19。
    (1u64 << z) as f64
}

#[inline]
fn clamp_tile(v: f64, n: f64) -> u32 {
    if v < 0.0 {
        0
    } else if v >= n {
        (n as u32).saturating_sub(1)
    } else {
        v as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // OSM wiki の既知検証値: z=16 で (lat=47.2342, lon=11.0500ish) → tile。
    // ここでは安定な既知値で確認する。
    #[test]
    fn zero_zoom_single_tile() {
        // z=0 は世界が1タイル。常に (0,0)。
        let t = lonlat_to_tile(35.68, 139.76, 0);
        assert_eq!(t.x, 0);
        assert_eq!(t.y, 0);
    }

    #[test]
    fn greenwich_equator_midpoint() {
        // 経度0,緯度0 は z=1 で x=1,y=1（4タイルの右下境界）。
        let t = lonlat_to_tile(0.0, 0.0, 1);
        assert_eq!(t.x, 1);
        assert_eq!(t.y, 1);
    }

    #[test]
    fn tokyo_tile_z8() {
        // 東京駅近辺 z=8。OSM式での既知値域に入ることを確認。
        let t = lonlat_to_tile(35.681, 139.767, 8);
        assert_eq!(t.z, 8);
        // z=8 では x≈227, y≈100 付近。
        assert!(t.x >= 226 && t.x <= 228, "x={}", t.x);
        assert!(t.y >= 99 && t.y <= 101, "y={}", t.y);
    }

    #[test]
    fn roundtrip_corner() {
        // タイル → 角の緯度経度 → 同じタイルに戻る。
        let t = TileCoord { z: 10, x: 909, y: 403 };
        let (lat, lon) = tile_nw_corner(t);
        let t2 = lonlat_to_tile(lat, lon, 10);
        // 角はそのタイルの左上なので一致するはず（浮動小数誤差で隣に行く場合は±1許容）。
        assert!((t2.x as i64 - t.x as i64).abs() <= 1);
        assert!((t2.y as i64 - t.y as i64).abs() <= 1);
    }

    #[test]
    fn bbox_range_orders_correctly() {
        let bbox = GeoBBox::new(35.0, 139.0, 36.0, 140.0);
        let (nw, se) = bbox_to_tile_range(&bbox, 8);
        // 北西の y は南東の y 以下、x も以下。
        assert!(nw.x <= se.x);
        assert!(nw.y <= se.y);
    }

    #[test]
    fn tile_frac_roundtrip() {
        // 分数タイル座標 → 緯度経度 → タイル整数化で元のタイルに戻る。
        let z = 8;
        let (lat, lon) = tile_frac_to_lonlat(z, 227.5, 100.5); // タイル(227,100)の中心
        // 東京周辺の妥当な範囲に入る。
        assert!(lat > 35.0 && lat < 37.0, "lat={lat}");
        assert!(lon > 139.0 && lon < 141.0, "lon={lon}");
        let t = lonlat_to_tile(lat, lon, z);
        assert_eq!(t.x, 227);
        assert_eq!(t.y, 100);
    }

    #[test]
    fn pixel_within_world() {
        let (px, py) = lonlat_to_pixel(35.681, 139.767, 8, 256.0);
        let world = (1u64 << 8) as f64 * 256.0;
        assert!(px >= 0.0 && px < world);
        assert!(py >= 0.0 && py < world);
    }
}
