//! Braille 点字量子化：`Grid` → `DrawCell` 列。
//!
//! 端末1文字 = Braille 1セル = 横2×縦4 = 8ドット。各ドットを格子へ対応づけ、
//! 閾値を超えたドットのビットを立てて U+2800+pattern の点字文字を作る。
//! 前景色はセル内の代表値（最大値）を colormap で RGB 化する。

use super::colormap::{cloud_to_rgb, precip_to_rgb, Rgb};
use super::DrawCell;
use crate::grid::{Grid, GridKind};
use heapless::Vec;

/// 1回の量子化で生成できる最大セル数。
/// 端末を広めに見積もって 200桁 × 60行 = 12000。
pub const MAX_CELLS: usize = 12_000;

pub type CellVec = Vec<DrawCell, MAX_CELLS>;

/// Braille ドット (dx,dy) → ビットマスク（Unicode Braille 規格）。
///
/// 配置:
/// ```text
/// (0,0)=0x01  (1,0)=0x08
/// (0,1)=0x02  (1,1)=0x10
/// (0,2)=0x04  (1,2)=0x20
/// (0,3)=0x40  (1,3)=0x80
/// ```
///
/// `lines.rs` でも同じビット規格を使うため `pub(crate)` で共有する（重複実装禁止）。
#[inline]
pub(crate) fn dot_bit(dx: u8, dy: u8) -> u8 {
    match (dx, dy) {
        (0, 0) => 0x01,
        (0, 1) => 0x02,
        (0, 2) => 0x04,
        (0, 3) => 0x40,
        (1, 0) => 0x08,
        (1, 1) => 0x10,
        (1, 2) => 0x20,
        (1, 3) => 0x80,
        _ => 0x00,
    }
}

/// ビットパターン → Braille 文字。`lines.rs` と共有（`pub(crate)`）。
#[inline]
pub(crate) fn braille_char(pattern: u8) -> char {
    // U+2800 + pattern。0..=255 は必ず有効な Braille コードポイント。
    char::from_u32(0x2800 + pattern as u32).unwrap_or('⠀')
}

/// 量子化の閾値判定：値が「描画対象」か。
#[inline]
fn dot_on(kind: GridKind, value: f32) -> bool {
    match kind {
        GridKind::PrecipMmH => value >= 0.1,
        GridKind::CloudPct => value >= 10.0,
    }
}

/// 代表値 → RGB。kind に応じて配色を切り替える。
#[inline]
fn value_to_rgb(kind: GridKind, value: f32) -> Option<Rgb> {
    match kind {
        GridKind::PrecipMmH => precip_to_rgb(value),
        GridKind::CloudPct => cloud_to_rgb(value),
    }
}

/// `Grid` を `cols × rows` の Braille セル群へ量子化する。
///
/// - `cols`, `rows`: 描画領域の文字数（端末のマップ領域サイズ）。
/// - 1セルは横2×縦4ドット。全体で `cols*2 × rows*4` ドットを格子へマップ。
/// - 降水/雲のない（閾値未満の）セルは出力しない（背景＝地図を透かす）。
///
/// 戻り値は描画すべきセルのみ。容量超過分は切り捨てる。
pub fn quantize(grid: &Grid, cols: u16, rows: u16) -> CellVec {
    let mut out: CellVec = Vec::new();

    if cols == 0 || rows == 0 {
        return out;
    }

    let dots_x = cols as f32 * 2.0;
    let dots_y = rows as f32 * 4.0;

    for row in 0..rows {
        for col in 0..cols {
            let mut pattern: u8 = 0;
            let mut rep_value = f32::MIN; // セル内代表値（最大）

            for dy in 0..4u8 {
                for dx in 0..2u8 {
                    // このドットのグローバルドット座標。
                    let gx = col as f32 * 2.0 + dx as f32;
                    let gy = row as f32 * 4.0 + dy as f32;
                    // 正規化 [0,1)。
                    let u = gx / dots_x;
                    let v = gy / dots_y;
                    let val = grid.sample_nearest(u, v);

                    if dot_on(grid.kind, val) {
                        pattern |= dot_bit(dx, dy);
                        if val > rep_value {
                            rep_value = val;
                        }
                    }
                }
            }

            // 1ドットも立たなければこのセルは描画しない。
            if pattern == 0 {
                continue;
            }

            let fg = match value_to_rgb(grid.kind, rep_value) {
                Some(c) => c,
                None => continue,
            };

            let cell = DrawCell {
                col,
                row,
                braille: braille_char(pattern),
                fg,
            };
            if out.push(cell).is_err() {
                // 容量超過：以降は描画しきれないので打ち切る。
                return out;
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::GeoBBox;
    use crate::grid::GridKind;

    fn bbox() -> GeoBBox {
        GeoBBox::new(35.0, 139.0, 36.0, 140.0)
    }

    #[test]
    fn empty_grid_no_cells() {
        let g = Grid::new_zeroed(8, 8, GridKind::PrecipMmH, bbox()).unwrap();
        let cells = quantize(&g, 4, 2);
        assert!(cells.is_empty(), "all-zero grid should draw nothing");
    }

    #[test]
    fn full_rain_fills_cell() {
        // 全セルに強い雨 → 全ドット点灯 → U+28FF（全点）になるはず。
        let mut g = Grid::new_zeroed(2, 4, GridKind::PrecipMmH, bbox()).unwrap();
        for y in 0..4 {
            for x in 0..2 {
                g.set(x, y, 50.0);
            }
        }
        let cells = quantize(&g, 1, 1);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].braille, '⣿'); // U+28FF 全ドット
        // 50mm/h は赤。
        assert_eq!(cells[0].fg, Rgb::new(0xFF, 0x28, 0x28));
    }

    #[test]
    fn dot_bit_mapping() {
        // 規格通りのビット割り当て確認。
        assert_eq!(dot_bit(0, 0), 0x01);
        assert_eq!(dot_bit(1, 3), 0x80);
        assert_eq!(dot_bit(0, 3), 0x40);
    }

    #[test]
    fn braille_codepoint() {
        assert_eq!(braille_char(0x00), '⠀'); // U+2800 空
        assert_eq!(braille_char(0xFF), '⣿'); // U+28FF 全点
        assert_eq!(braille_char(0x01), '⠁'); // U+2801 左上のみ
    }

    #[test]
    fn cloud_kind_uses_cloud_threshold() {
        // 雲量 5% は閾値(10)未満 → 描画されない。
        let mut g = Grid::new_zeroed(2, 4, GridKind::CloudPct, bbox()).unwrap();
        for y in 0..4 {
            for x in 0..2 {
                g.set(x, y, 5.0);
            }
        }
        let cells = quantize(&g, 1, 1);
        assert!(cells.is_empty());
    }
}
