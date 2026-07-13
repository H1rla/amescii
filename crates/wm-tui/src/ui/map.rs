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
    /// 描画中に `App::render_cache` を更新するため可変参照で受ける。
    pub app: &'a mut App,
}

impl<'a> Widget for MapWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" ameSCII ");
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
        //   rasterize_lines は入力（データ版数・BBox・セル数）が同じなら同じ結果を返す
        //   純粋関数。キーが一致する間はキャッシュ済みセルを使い、再ラスタライズしない。
        if self.app.basemap.is_some() {
            let key = self.app.basemap_key(inner.width, inner.height);
            if self.app.render_cache.basemap_key != Some(key) {
                self.app.render_cache.basemap_cells =
                    rasterize_basemap(self.app, inner.width, inner.height);
                self.app.render_cache.basemap_key = Some(key);
            }
            blit(buf, inner, &self.app.render_cache.basemap_cells);
        }

        // 合成順序 ②雨雲グリッド（タイムラインの現在コマ）を Braille 量子化して重畳
        //          （地図線を上書き＝雨を優先）。機能B：show_radar が false なら描かない。
        //   キーに frame_idx を含むので、再生中はコマが変わったときだけ再量子化する
        //   （同じコマを見ている間の再描画では計算しない）。
        if self.app.show_radar && self.app.current_frame().is_some() {
            let key = self.app.radar_key(inner.width, inner.height);
            if self.app.render_cache.radar_key != Some(key) {
                self.app.render_cache.radar_cells = self
                    .app
                    .current_frame()
                    .map(|f| quantize(&f.grid, inner.width, inner.height).into_iter().collect())
                    .unwrap_or_default();
                self.app.render_cache.radar_key = Some(key);
            }
            blit(buf, inner, &self.app.render_cache.radar_cells);
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
        //   zoom < 11：内蔵都市テーブル（places::layout_city_labels）。日本語名。
        //   zoom >= 11：地理院 label の日本語地名（annoChar/knj）を間引いて描画。
        // どちらも全角なので描画は draw_label_text（set_stringn 委譲）で共通。
        let bbox = self.app.current_bbox();
        if self.app.zoom >= JA_LABEL_ZOOM {
            if let Some(labels) = &self.app.name_labels_ja {
                draw_ja_labels(buf, inner, &bbox, labels);
            }
        } else {
            let labels = layout_city_labels(&bbox, self.app.zoom, inner.width, inner.height);
            for l in labels.iter() {
                draw_label_text(buf, inner, l.marker_col, l.row, l.name);
            }
        }
    }
}

/// 地図ベース層を Braille セル列へラスタライズする（キャッシュミス時のみ呼ぶ）。
///
/// 描画順：海岸線 → 行政界 → 道路 → 鉄道（後のものがセルで後勝ち）。
fn rasterize_basemap(app: &App, cols: u16, rows: u16) -> Vec<DrawCell> {
    let Some(lines) = &app.basemap else {
        return Vec::new();
    };
    let bbox = app.current_bbox();
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
    rasterize_lines(&polys, &bbox, cols, rows).into_iter().collect()
}

/// `DrawCell` 列をバッファへ転写する（地図線・雨雲で共用）。領域外はクリップ。
fn blit(buf: &mut Buffer, inner: Rect, cells: &[DrawCell]) {
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
/// 内蔵都市・地理院地名の両方で共用する。
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

/// 地図レイヤ種別 → 色（暗めにして雨雲を引き立てる）。MAP_AND_TIMELINE.md §色 準拠。
fn color_for(kind: BaseLineKind) -> Rgb {
    match kind {
        BaseLineKind::Coastline => Rgb::new(90, 120, 150), // 海岸線/水域輪郭：青灰
        BaseLineKind::Boundary => Rgb::new(80, 80, 90),    // 行政界：暗い灰
        BaseLineKind::Road => Rgb::new(110, 95, 80),       // 道路：くすんだ茶灰
        BaseLineKind::Railway => Rgb::new(100, 100, 105),  // 鉄道：灰
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wm_core::geo::GeoBBox;
    use wm_core::{Grid, GridKind};
    use wm_sources::basemap::BaseLine;
    use wm_sources::radar::RadarFrame;

    const AREA: Rect = Rect { x: 0, y: 0, width: 40, height: 20 };
    /// 枠線ぶんを除いた描画領域（Block::inner と一致）。
    const COLS: u16 = AREA.width - 2;
    const ROWS: u16 = AREA.height - 2;

    /// キャッシュに差し込む目印。再計算されれば消えるので「計算をスキップしたか」を
    /// カウンタ無しで観測できる。
    const SENTINEL: DrawCell = DrawCell {
        col: 0,
        row: 0,
        braille: '⣿',
        fg: Rgb { r: 1, g: 2, b: 3 },
    };

    /// 雨雲 1 コマ。`dot` の位置に雨を立てる（コマごとに変えると再量子化を観測できる）。
    fn frame(bbox: GeoBBox, dot: (u16, u16), valid_unix: u64) -> RadarFrame {
        let mut grid = Grid::new_zeroed(COLS * 2, ROWS * 4, GridKind::PrecipMmH, bbox).unwrap();
        grid.set(dot.0, dot.1, 20.0);
        RadarFrame {
            valid_unix,
            is_forecast: false,
            grid: Box::new(grid),
        }
    }

    /// 地図線＋雨雲2コマを載せた App。都市が1つも入らない太平洋上に置き、
    /// ラベル描画をテストの観測対象から外す（描かれるのは地図線・雨雲・◎ のみ）。
    fn test_app() -> App {
        let mut app = App::new(30.0, 145.0, 8, true);
        app.map_cols = COLS;
        app.map_rows = ROWS;
        let bbox = app.current_bbox();
        // BBox を斜めに横切る線＝そこそこの数のセルが立つ。
        let line = BaseLine {
            points: vec![
                (bbox.min_lat, bbox.min_lon),
                (bbox.max_lat, bbox.max_lon),
            ],
            kind: BaseLineKind::Coastline,
        };
        app.set_basemap(vec![line]);
        app.set_frames(vec![
            frame(bbox, (10, 10), 1000),
            frame(bbox, (30, 30), 1600),
        ]);
        app.frame_idx = 0;
        app
    }

    fn render(app: &mut App) -> Buffer {
        let mut buf = Buffer::empty(AREA);
        MapWidget { app }.render(AREA, &mut buf);
        buf
    }

    #[test]
    fn first_render_populates_cache() {
        let mut app = test_app();
        render(&mut app);
        assert_eq!(
            app.render_cache.basemap_key,
            Some(app.basemap_key(COLS, ROWS))
        );
        assert_eq!(app.render_cache.radar_key, Some(app.radar_key(COLS, ROWS)));
        assert!(!app.render_cache.basemap_cells.is_empty(), "地図線が立つ");
        assert!(!app.render_cache.radar_cells.is_empty(), "雨雲セルが立つ");
    }

    #[test]
    fn idle_redraw_recomputes_nothing() {
        // アイドル中の再描画：入力が何も変わらないので両方ともキャッシュ命中。
        let mut app = test_app();
        render(&mut app);
        app.render_cache.basemap_cells = vec![SENTINEL];
        app.render_cache.radar_cells = vec![SENTINEL];

        render(&mut app);

        assert_eq!(app.render_cache.basemap_cells, vec![SENTINEL], "地図線は再計算されない");
        assert_eq!(app.render_cache.radar_cells, vec![SENTINEL], "雨雲は再計算されない");
    }

    #[test]
    fn playback_keeps_basemap_cache_hot() {
        // 再生中（frame_idx が 500ms ごとに変わる）：雨雲だけ再量子化し、
        // 地図線はキャッシュのまま＝ここが今回の主目的。
        let mut app = test_app();
        render(&mut app);
        let basemap_key = app.render_cache.basemap_key.unwrap();
        let radar_key = app.render_cache.radar_key.unwrap();
        app.render_cache.basemap_cells = vec![SENTINEL];

        app.advance_play(); // frame_idx 0 → 1
        let buf = render(&mut app);

        assert_eq!(app.render_cache.basemap_key, Some(basemap_key));
        assert_eq!(
            app.render_cache.basemap_cells,
            vec![SENTINEL],
            "コマ送りで地図線が再計算されている"
        );
        // 目印がバッファに出ている＝キャッシュ済みセルが実際に描画に使われている。
        assert_eq!(buf[(1, 1)].symbol(), "⣿");
        // 雨雲側はコマが変わったので再量子化される。
        assert_ne!(app.render_cache.radar_key, Some(radar_key));
        assert_ne!(app.render_cache.radar_cells, vec![SENTINEL]);
    }

    #[test]
    fn pan_invalidates_basemap_cache() {
        // パンでは basemap_version が上がらない（同じデータを別 BBox へ投影し直すだけ）。
        // キーに中心座標を入れてあるので再ラスタライズされる＝地図が固まらない。
        let mut app = test_app();
        render(&mut app);
        app.render_cache.basemap_cells = vec![SENTINEL];

        app.pan(0.0, 0.3); // 東へパン（データ取得は伴わない）
        render(&mut app);

        assert_ne!(
            app.render_cache.basemap_cells,
            vec![SENTINEL],
            "パン後も古い投影のセルを使い回している"
        );
    }

    #[test]
    fn resize_invalidates_both_caches() {
        let mut app = test_app();
        render(&mut app);
        app.render_cache.basemap_cells = vec![SENTINEL];
        app.render_cache.radar_cells = vec![SENTINEL];

        // 端末リサイズ：描画領域が変わると出力セル数が変わる。
        let small = Rect::new(0, 0, 30, 14);
        app.map_cols = small.width - 2;
        app.map_rows = small.height - 2;
        let mut buf = Buffer::empty(small);
        MapWidget { app: &mut app }.render(small, &mut buf);

        assert_ne!(app.render_cache.basemap_cells, vec![SENTINEL]);
        assert_ne!(app.render_cache.radar_cells, vec![SENTINEL]);
    }

    #[test]
    fn new_radar_data_invalidates_cache() {
        // コマ数も frame_idx も同じだが中身が別データ、というケース。
        // radar_version が無いと検知できない（frame_idx だけでは同一キーになる）。
        let mut app = test_app();
        render(&mut app);
        let radar_key = app.render_cache.radar_key.unwrap();
        app.render_cache.radar_cells = vec![SENTINEL];

        let bbox = app.current_bbox();
        app.set_frames(vec![frame(bbox, (10, 10), 2000), frame(bbox, (30, 30), 2600)]);
        app.frame_idx = 0; // set_frames と同じコマ位置に戻す
        render(&mut app);

        assert_ne!(app.render_cache.radar_key, Some(radar_key), "版数で無効化される");
        assert_ne!(app.render_cache.radar_cells, vec![SENTINEL]);
    }
}
