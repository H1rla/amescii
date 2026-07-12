//! 線分（緯度経度の点列）→ Braille セルへのラスタライズ。
//!
//! 雨雲の `braille.rs` と同じく、画面を 2x4 ドットの Braille グリッドとして扱う。
//! Bresenham でドット単位に線を引き、点灯ドットを Braille セルへ畳む。
//!
//! **移植性の肝**：MVT デコード（std 依存）は `wm-sources/basemap.rs`、
//! 線を画面へ投影して点を打つロジックはここ（`wm-core`, no_std）に置く。
//!
//! **雨雲との投影一致**：緯度経度→画面の投影は `braille.rs`/`radar.rs` と同じ
//! Web Mercator の BBox 正規化を使う。正規化により zoom は相殺されるので、
//! 同じ `GeoBBox` を渡せば雨雲セルと同じ (col,row) に同じ地点が来る。

use super::braille::{braille_char, dot_bit};
use super::{DrawCell, Rgb};
use crate::geo::{lonlat_to_pixel, GeoBBox};
use heapless::Vec;

/// 1本の線（点列）。座標は緯度経度 (lat, lon)。
pub struct PolyLine<'a> {
    pub points: &'a [(f64, f64)],
    pub color: Rgb,
}

/// 線描画の最大セル数（雨雲と別枠）。
pub const MAX_LINE_CELLS: usize = 12_000;
pub type LineCellVec = Vec<DrawCell, MAX_LINE_CELLS>;

/// セルごとの点灯ビットと代表色のアキュムレータ。
#[derive(Clone, Copy)]
struct CellAccum {
    pattern: u8,
    color: Rgb,
}

/// 緯度経度 → BBox 正規化座標 (u, v) ∈ おおよそ [0,1]。
///
/// `radar.rs` と同じ `lonlat_to_pixel` を使い、BBox 北西=(0,0)・南東=(1,1) に正規化。
/// 正規化で world スケール（zoom・tile_size）が相殺されるため、投影 zoom の値は
/// 結果に影響しない（雨雲と一致させるための要点）。`pub(crate)` でテストから参照。
pub(crate) fn project_norm(bbox: &GeoBBox, lat: f64, lon: f64) -> (f64, f64) {
    // zoom は正規化で消えるので任意。f64 精度を稼ぐため大きめの world を選ぶ。
    const PROJ_ZOOM: u8 = 12;
    const TILE: f64 = 256.0;
    let (px, py) = lonlat_to_pixel(lat, lon, PROJ_ZOOM, TILE);
    let (px_min, py_min) = lonlat_to_pixel(bbox.max_lat, bbox.min_lon, PROJ_ZOOM, TILE); // 北西
    let (px_max, py_max) = lonlat_to_pixel(bbox.min_lat, bbox.max_lon, PROJ_ZOOM, TILE); // 南東
    let sx = px_max - px_min;
    let sy = py_max - py_min;
    let u = if sx.abs() > f64::EPSILON {
        (px - px_min) / sx
    } else {
        0.0
    };
    let v = if sy.abs() > f64::EPSILON {
        (py - py_min) / sy
    } else {
        0.0
    };
    (u, v)
}

/// 緯度経度を、表示 BBox を覆う `cols×rows` 文字セルのセル (col,row) へ投影する。
///
/// `project_norm`（lines/radar/places と同一の BBox 正規化）→ floor でセル化。
/// 画面外（正規化が [0,1] を外れる、または範囲外セル）は `None`。
/// 地名ラベルなど「点を画面セルへ置く」用途で外（wm-tui）から使う公開 API。
/// これを使えばラベルが地図線・雨雲と必ず同じセルに乗る（投影一致）。
pub fn lonlat_to_cell(bbox: &GeoBBox, lat: f64, lon: f64, cols: u16, rows: u16) -> Option<(u16, u16)> {
    if cols == 0 || rows == 0 {
        return None;
    }
    let (u, v) = project_norm(bbox, lat, lon);
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return None;
    }
    let col = (u * cols as f64) as u16;
    let row = (v * rows as f64) as u16;
    if col >= cols || row >= rows {
        return None;
    }
    Some((col, row))
}

/// 複数の線を、指定 BBox・セル数の画面へラスタライズする。
///
/// - `bbox`: 画面が覆う地理範囲（`App::current_bbox` と同じもの）。
/// - `cols/rows`: マップ領域の文字数。1セル=2x4ドット。
/// - 出力は点灯セルのみ。同一セルに複数線が来たら後勝ち（呼び出し側で描画順を
///   制御：海岸線→行政界→道路→鉄道 の順に呼べば鉄道が上）。
pub fn rasterize_lines(lines: &[PolyLine], bbox: &GeoBBox, cols: u16, rows: u16) -> LineCellVec {
    let mut out: LineCellVec = Vec::new();
    if cols == 0 || rows == 0 {
        return out;
    }
    let n_cells = cols as usize * rows as usize;
    if n_cells > MAX_LINE_CELLS {
        return out; // 画面が大きすぎる：描画しない（パニックより安全側）。
    }

    // セルアキュムレータを 0 初期化。
    let mut acc: Vec<CellAccum, MAX_LINE_CELLS> = Vec::new();
    for _ in 0..n_cells {
        if acc
            .push(CellAccum {
                pattern: 0,
                color: Rgb::new(0, 0, 0),
            })
            .is_err()
        {
            return out;
        }
    }

    let dots_x = cols as i32 * 2;
    let dots_y = rows as i32 * 4;
    // クリップ矩形は有効ドット範囲 [0, dots-1]（包含）。
    let xmax = (dots_x - 1) as f64;
    let ymax = (dots_y - 1) as f64;

    for line in lines.iter() {
        // 各セグメント p0→p1 を投影・クリップ・Bresenham。
        for seg in line.points.windows(2) {
            let (u0, v0) = project_norm(bbox, seg[0].0, seg[0].1);
            let (u1, v1) = project_norm(bbox, seg[1].0, seg[1].1);
            let x0 = u0 * dots_x as f64;
            let y0 = v0 * dots_y as f64;
            let x1 = u1 * dots_x as f64;
            let y1 = v1 * dots_y as f64;

            if let Some((cx0, cy0, cx1, cy1)) = clip_segment(x0, y0, x1, y1, xmax, ymax) {
                // ドット座標は切り捨て（floor）で整数化する。雨雲側の
                // sample_nearest も floor なので、同じ地点が同じセルに落ちる。
                // クリップ後の座標は [0, dots-1] なので `as i32` の 0 方向切り捨て
                // は floor と一致する（no_std で f64::floor を使わずに済む）。
                bresenham(
                    cx0 as i32,
                    cy0 as i32,
                    cx1 as i32,
                    cy1 as i32,
                    |x, y| light_dot(&mut acc, cols, dots_x, dots_y, x, y, line.color),
                );
            }
        }
    }

    // アキュムレータ → DrawCell。
    for cy in 0..rows {
        for cx in 0..cols {
            let idx = cy as usize * cols as usize + cx as usize;
            let a = acc[idx];
            if a.pattern != 0 {
                let cell = DrawCell {
                    col: cx,
                    row: cy,
                    braille: braille_char(a.pattern),
                    fg: a.color,
                };
                if out.push(cell).is_err() {
                    return out;
                }
            }
        }
    }

    out
}

/// 1ドットを点灯（範囲外は無視）。セルの 8bit パターンへ畳み、代表色は後勝ち。
#[inline]
fn light_dot(
    acc: &mut [CellAccum],
    cols: u16,
    dots_x: i32,
    dots_y: i32,
    x: i32,
    y: i32,
    color: Rgb,
) {
    if x < 0 || y < 0 || x >= dots_x || y >= dots_y {
        return;
    }
    let cell_x = (x / 2) as usize;
    let cell_y = (y / 4) as usize;
    let idx = cell_y * cols as usize + cell_x;
    if idx < acc.len() {
        acc[idx].pattern |= dot_bit((x % 2) as u8, (y % 4) as u8);
        acc[idx].color = color;
    }
}

// ─────────────────── Cohen-Sutherland クリップ（float） ───────────────────
// 画面外の点を含む線でも Bresenham が暴走しないよう、矩形 [0,xmax]×[0,ymax] に
// 線分をクリップしてから整数化する。

const OC_LEFT: u8 = 1;
const OC_RIGHT: u8 = 2;
const OC_BOTTOM: u8 = 4;
const OC_TOP: u8 = 8;

#[inline]
fn outcode(x: f64, y: f64, xmax: f64, ymax: f64) -> u8 {
    let mut c = 0;
    if x < 0.0 {
        c |= OC_LEFT;
    } else if x > xmax {
        c |= OC_RIGHT;
    }
    if y < 0.0 {
        c |= OC_BOTTOM;
    } else if y > ymax {
        c |= OC_TOP;
    }
    c
}

/// 線分を矩形へクリップ。完全に外なら `None`。
fn clip_segment(
    mut x0: f64,
    mut y0: f64,
    mut x1: f64,
    mut y1: f64,
    xmax: f64,
    ymax: f64,
) -> Option<(f64, f64, f64, f64)> {
    loop {
        let o0 = outcode(x0, y0, xmax, ymax);
        let o1 = outcode(x1, y1, xmax, ymax);
        if o0 | o1 == 0 {
            return Some((x0, y0, x1, y1)); // 両端が内側
        }
        if o0 & o1 != 0 {
            return None; // 同じ外側領域 → 完全に外
        }
        let o = if o0 != 0 { o0 } else { o1 };
        let (x, y);
        if o & OC_TOP != 0 {
            x = x0 + (x1 - x0) * (ymax - y0) / (y1 - y0);
            y = ymax;
        } else if o & OC_BOTTOM != 0 {
            x = x0 + (x1 - x0) * (0.0 - y0) / (y1 - y0);
            y = 0.0;
        } else if o & OC_RIGHT != 0 {
            y = y0 + (y1 - y0) * (xmax - x0) / (x1 - x0);
            x = xmax;
        } else {
            y = y0 + (y1 - y0) * (0.0 - x0) / (x1 - x0);
            x = 0.0;
        }
        if o == o0 {
            x0 = x;
            y0 = y;
        } else {
            x1 = x;
            y1 = y;
        }
    }
}

/// 整数 Bresenham。各通過点で `put` を呼ぶ。始点=終点なら1点だけ。
fn bresenham<F: FnMut(i32, i32)>(mut x0: i32, mut y0: i32, x1: i32, y1: i32, mut put: F) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        put(x0, y0);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Grid, GridKind};
    use crate::render::braille::quantize;

    fn bbox() -> GeoBBox {
        GeoBBox::new(35.0, 139.0, 36.0, 140.0)
    }

    #[test]
    fn horizontal_line_lights_expected_row() {
        // BBox 中央緯度で水平線（経度方向）。中央付近の行だけ点灯し、上下端は消灯。
        let bb = bbox();
        let mid_lat = 35.5;
        let line = PolyLine {
            points: &[(mid_lat, 139.05), (mid_lat, 139.95)],
            color: Rgb::new(90, 120, 150),
        };
        let cells = rasterize_lines(&[line], &bb, 20, 10);
        assert!(!cells.is_empty(), "水平線が1セルも出ないのは異常");

        // 全点灯セルがほぼ同じ row 帯に集まる（水平線なので）。
        let rows: heapless::Vec<u16, 256> = cells.iter().map(|c| c.row).collect();
        let min_r = rows.iter().copied().min().unwrap();
        let max_r = rows.iter().copied().max().unwrap();
        assert!(max_r - min_r <= 1, "水平線が縦に散っている: {min_r}..{max_r}");
        // 中央緯度なので中央付近の行。
        assert!(min_r >= 3 && max_r <= 6, "row帯={min_r}..{max_r}");
    }

    #[test]
    fn diagonal_is_connected() {
        // 対角線：Bresenham で穴が空かない（隣接セルが連続）。
        let bb = bbox();
        let line = PolyLine {
            points: &[(35.95, 139.05), (35.05, 139.95)],
            color: Rgb::new(100, 100, 105),
        };
        let cells = rasterize_lines(&[line], &bb, 20, 10);
        assert!(cells.len() >= 5, "対角線のセルが少なすぎる: {}", cells.len());
        // col 昇順に並べたとき、隣接セルの col 差が高々1（連続）であること。
        let mut sorted: heapless::Vec<(u16, u16), 512> =
            cells.iter().map(|c| (c.col, c.row)).collect();
        sorted.sort_unstable();
        for w in sorted.windows(2) {
            let dc = w[1].0 as i32 - w[0].0 as i32;
            assert!(dc <= 1, "col に飛び: {:?} -> {:?}", w[0], w[1]);
        }
    }

    #[test]
    fn out_of_bbox_does_not_panic() {
        // BBox を大きくはみ出す点を含む線。クリップされ、パニックしない。
        let bb = bbox();
        let line = PolyLine {
            points: &[(80.0, 10.0), (35.5, 139.5), (-80.0, 270.0)],
            color: Rgb::new(110, 95, 80),
        };
        let cells = rasterize_lines(&[line], &bb, 30, 12);
        // 中央付近を通る区間があるので何かは出る（出なくても panic しなければ可）。
        let _ = cells.len();
    }

    #[test]
    fn line_and_rain_share_projection() {
        // 「地図線と雨雲が同じ投影で一致する」回帰テスト。
        // 同じ点を、(a) 雨雲 Grid→quantize と (b) 線 rasterize で描き、
        // 同じ (col,row) セルが点灯することを確認する。
        let bb = bbox();
        let cols = 16u16;
        let rows = 8u16;
        let gw = cols * 2; // グリッド解像度=ドット数にして1:1対応にする
        let gh = rows * 4;

        // BBox 内の代表点。
        let (lat, lon) = (35.62, 139.37);
        let (u, v) = project_norm(&bb, lat, lon);
        let dotx = (u * (cols as f64 * 2.0)) as u16;
        let doty = (v * (rows as f64 * 4.0)) as u16;

        // (a) 雨雲：その点のドットに対応するグリッドセルへ強い雨を置く。
        let mut grid = Grid::new_zeroed(gw, gh, GridKind::PrecipMmH, bb).unwrap();
        grid.set(dotx, doty, 50.0);
        let rain = quantize(&grid, cols, rows);
        assert_eq!(rain.len(), 1, "雨雲は1セルだけ点灯のはず");
        let rcell = (rain[0].col, rain[0].row);

        // (b) 線：同じ点を通る極小線分（2点同一）。
        let line = PolyLine {
            points: &[(lat, lon), (lat, lon)],
            color: Rgb::new(90, 120, 150),
        };
        let lc = rasterize_lines(&[line], &bb, cols, rows);
        assert!(!lc.is_empty(), "線が点灯しない");
        // 線の点灯セルに雨雲セルと同じ (col,row) が含まれる＝投影一致。
        assert!(
            lc.iter().any(|c| (c.col, c.row) == rcell),
            "雨雲セル {:?} が線の点灯セルに無い: {:?}",
            rcell,
            lc.iter().map(|c| (c.col, c.row)).collect::<std::vec::Vec<_>>()
        );
    }
}
