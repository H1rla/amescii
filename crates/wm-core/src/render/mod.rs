//! 描画の中間表現と量子化ロジック。
//!
//! `wm-core` は「画面にどう描くか」を知らない。代わりに `DrawCell` の列を
//! 出力し、各プラットフォーム（wm-tui=Ratatui, wm-esp32=embedded-graphics）が
//! それを解釈する。この境界のおかげで描画バックエンドを差し替えられる。

pub mod braille;
pub mod colormap;
pub mod lines;

pub use colormap::Rgb;
pub use lines::{lonlat_to_cell, rasterize_lines, LineCellVec, PolyLine};

/// 描画対象の1セル（端末の1文字ぶん = Braille 2×4ドット）。
///
/// プラットフォームはこれを各々の描画プリミティブへ変換する:
/// - Ratatui: `Cell::set_char(braille).set_fg(Color::Rgb(...))`
/// - embedded-graphics: Braille パターンをピクセル/グリフとして描画
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DrawCell {
    /// マップ領域内の桁（0始まり）。
    pub col: u16,
    /// マップ領域内の行（0始まり）。
    pub row: u16,
    /// Braille 文字（U+2800..=U+28FF）。
    pub braille: char,
    /// truecolor 前景色。
    pub fg: Rgb,
}
