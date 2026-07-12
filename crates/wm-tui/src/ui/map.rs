//! 地図ウィジェット：wm-core の Braille 量子化結果を Ratatui バッファへ描画。
//!
//! truecolor の雨雲セルを地図領域に重畳する。地図ベース層は本実装では
//! 単色背景＋中心マーカーのみ（OSM タイル輪郭描画は将来拡張）。

use crate::app::{App, JA_LABEL_ZOOM};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Widget},
};
use unicode_width::UnicodeWidthStr;
use wm_core::places::layout_city_labels;
use wm_core::render::braille::quantize;
use wm_core::render::{lonlat_to_cell, rasterize_lines, DrawCell, PolyLine, Rgb};
use wm_sources::basemap::{BaseLineKind, NameLabelJa};

/// 1画面に描く日本語地名の上限（高ズームは注記が多いので間引く）。
const MAX_JA_LABELS: usize = 40;

pub struct MapWidget<'a> {
    pub app: &'a App,
}

impl<'a> Widget for MapWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" weathermap ");
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        // 背景：淡色のドット下地（地図があるように見せる最小限）。
        for y in inner.top()..inner.bottom() {
            for x in inner.left()..inner.right() {
                let cell = &mut buf[(x, y)];
                cell.set_char(' ');
                cell.set_bg(Color::Reset);
            }
        }

        // 合成順序 ①地図ベース層（暗色の線）。雨雲より先に描き、雨雲で上書きさせる。
        if let Some(lines) = &self.app.basemap {
            let bbox = self.app.current_bbox();
            // 描画順：海岸線 → 行政界 → 道路 → 鉄道（後のものがセルで後勝ち）。
            let mut polys: Vec<PolyLine> = Vec::new();
            for kind in [
                BaseLineKind::Coastline,
                BaseLineKind::Boundary,
                BaseLineKind::Road,
                BaseLineKind::Railway,
            ] {
                let color = color_for(kind);
                for bl in lines.iter().filter(|b| b.kind == kind) {
                    polys.push(PolyLine {
                        points: &bl.points,
                        color,
                    });
                }
            }
            let cells = rasterize_lines(&polys, &bbox, inner.width, inner.height);
            for dc in cells.iter() {
                let x = inner.left() + dc.col;
                let y = inner.top() + dc.row;
                if x < inner.right() && y < inner.bottom() {
                    let cell = &mut buf[(x, y)];
                    cell.set_char(dc.braille);
                    cell.set_fg(Color::Rgb(dc.fg.r, dc.fg.g, dc.fg.b));
                }
            }
        }

        // 合成順序 ②雨雲グリッド（タイムラインの現在コマ）を Braille 量子化して重畳
        //          （地図線を上書き＝雨を優先）。機能B：show_radar が false なら描かない。
        if self.app.show_radar {
            if let Some(frame) = self.app.current_frame() {
                let cells: Vec<DrawCell> =
                    quantize(&frame.grid, inner.width, inner.height).into_iter().collect();
                for dc in cells {
                    let x = inner.left() + dc.col;
                    let y = inner.top() + dc.row;
                    if x < inner.right() && y < inner.bottom() {
                        let cell = &mut buf[(x, y)];
                        cell.set_char(dc.braille);
                        cell.set_fg(Color::Rgb(dc.fg.r, dc.fg.g, dc.fg.b));
                    }
                }
            }
        }

        // 合成順序 ③中心マーカー ◎ を「ラベルより先に」描く。
        //   こうすると後続のラベル文字が ◎ の上に乗り、地名の先頭文字が ◎ で
        //   潰れない（前回報告の "◎okyo" 問題の修正）。中心に地名が重なる場合は
        //   ◎ よりラベルを優先＝場所名が読める。
        let cx = inner.left() + inner.width / 2;
        let cy = inner.top() + inner.height / 2;
        if cx < inner.right() && cy < inner.bottom() {
            let cell = &mut buf[(cx, cy)];
            cell.set_char('◎');
            cell.set_fg(Color::White);
        }

        // 合成順序 ④地名ラベル（雨雲・地図の上＝場所を見失わないため）。
        //   zoom < 11：内蔵英語都市（places::layout_city_labels）。
        //   zoom >= 11：地理院 label の日本語地名（annoChar/knj）を間引いて描画。
        let bbox = self.app.current_bbox();
        if self.app.zoom >= JA_LABEL_ZOOM {
            if let Some(labels) = &self.app.name_labels_ja {
                draw_ja_labels(buf, inner, &bbox, labels);
            }
        } else {
            let labels = layout_city_labels(&bbox, self.app.zoom, inner.width, inner.height);
            for l in labels.iter() {
                draw_label(buf, inner, l.marker_col, l.row, l.name);
            }
        }
    }
}

/// 日本語地名ラベル群を投影・間引きして描画する。
///
/// - 投影は wm-core の `lonlat_to_cell`（地図線・雨雲・英語都市と同一）。画面外は除外。
/// - 短い注記（"皇居"/"中央区" 等の上位地名のことが多い）を優先し、同じ行で水平に
///   重なる後続は捨てる。1画面 `MAX_JA_LABELS` 件まで。
/// - 全角は端末で 2 セル幅。描画は `set_stringn` に任せる（ratatui が幅を処理）。
fn draw_ja_labels(buf: &mut Buffer, inner: Rect, bbox: &wm_core::geo::GeoBBox, labels: &[NameLabelJa]) {
    // 画面内に投影できるものだけ候補化（col, row, text, 表示幅）。
    let mut cand: std::vec::Vec<(u16, u16, &str, u16)> = std::vec::Vec::new();
    for l in labels {
        if let Some((col, row)) = lonlat_to_cell(bbox, l.lat, l.lon, inner.width, inner.height) {
            let w = UnicodeWidthStr::width(l.text.as_str()) as u16;
            cand.push((col, row, l.text.as_str(), w));
        }
    }
    // 短い順（上位地名優先）に貪欲配置。
    cand.sort_by_key(|c| c.3);

    let mut placed: std::vec::Vec<(u16, u16, u16)> = std::vec::Vec::new(); // (col,row,span)
    let mut drawn = 0usize;
    for (col, row, text, w) in cand {
        if drawn >= MAX_JA_LABELS {
            break;
        }
        let span = 2 + w; // "· " + text の占有幅
        if ja_overlaps(&placed, col, row, span) {
            continue;
        }
        placed.push((col, row, span));
        draw_label_text(buf, inner, col, row, text);
        drawn += 1;
    }
}

/// 同じ行で水平範囲 [col, col+span) が既配置と被るか。
fn ja_overlaps(placed: &[(u16, u16, u16)], col: u16, row: u16, span: u16) -> bool {
    let a1 = col.saturating_add(span);
    placed.iter().any(|&(pcol, prow, pspan)| {
        prow == row && col < pcol.saturating_add(pspan) && pcol < a1
    })
}

/// マーカー `·` ＋ 空白 ＋ テキストを 1 行に書く（全角は ratatui が幅2で扱う）。
/// 黒背景＋白文字で地図・雨雲の上でも読めるようにし、右端はクリップ。
fn draw_label_text(buf: &mut Buffer, inner: Rect, col: u16, row: u16, text: &str) {
    let y = inner.top() + row;
    if y >= inner.bottom() {
        return;
    }
    let mx = inner.left() + col;
    if mx >= inner.right() {
        return;
    }
    let max_w = (inner.right() - mx) as usize;
    let s = format!("· {text}");
    let style = Style::default().fg(Color::White).bg(Color::Black);
    buf.set_stringn(mx, y, s, max_w, style);
}

/// 地名ラベルを 1 件描く：マーカー `·` ＋ 空白 ＋ 名前。
///
/// 地図線・雨雲の上に重ねるので、背景を 1 段暗く（黒）して白文字で可読性を確保する。
/// 右端を超える分はクリップ。座標はマップ内側領域 `inner` 基準のセル (marker_col,row)。
fn draw_label(buf: &mut Buffer, inner: Rect, marker_col: u16, row: u16, name: &str) {
    let y = inner.top() + row;
    if y >= inner.bottom() {
        return;
    }
    // マーカー。
    let mx = inner.left() + marker_col;
    if mx < inner.right() {
        let cell = &mut buf[(mx, y)];
        cell.set_char('·');
        cell.set_fg(Color::White);
        cell.set_bg(Color::Black);
    }
    // 名前（マーカーの 2 セル右から）。ASCII 前提で 1 文字 1 セル。
    let start = marker_col + 2;
    for (i, ch) in name.chars().enumerate() {
        let x = inner.left() + start + i as u16;
        if x >= inner.right() {
            break; // 右端クリップ
        }
        let cell = &mut buf[(x, y)];
        cell.set_char(ch);
        cell.set_fg(Color::White);
        cell.set_bg(Color::Black);
    }
}

/// 地図レイヤ種別 → 色（暗めにして雨雲を引き立てる）。MAP_AND_TIMELINE.md §色 準拠。
fn color_for(kind: BaseLineKind) -> Rgb {
    match kind {
        BaseLineKind::Coastline => Rgb::new(90, 120, 150), // 海岸線/水域輪郭：青灰
        BaseLineKind::Boundary => Rgb::new(80, 80, 90),    // 行政界：暗い灰
        BaseLineKind::Road => Rgb::new(110, 95, 80),       // 道路：くすんだ茶灰
        BaseLineKind::Railway => Rgb::new(100, 100, 105),  // 鉄道：灰
    }
}
