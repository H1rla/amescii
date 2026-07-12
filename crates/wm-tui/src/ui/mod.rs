//! UI 全体レイアウト。

pub mod legend;
pub mod map;
pub mod sidebar;
pub mod timeline;

use crate::app::App;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    Frame,
};

/// 1フレームを描画する。地図（左）＋サイドバー＆凡例（右）。
///
/// 描画時に App の map_cols / map_rows を実領域に合わせて更新する
/// （次回の BBox 計算に使うため）。
pub fn draw(f: &mut Frame, app: &mut App) {
    let size = f.area();

    // 横分割：地図 70% / 右パネル 30%（最小幅確保）。
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(40), Constraint::Length(32)])
        .split(size);

    // 左カラム縦分割：地図 / タイムライン1行。
    let left_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(1)])
        .split(cols[0]);
    let map_area = left_rows[0];
    let timeline_area = left_rows[1];
    let right = cols[1];

    // 右パネル縦分割：気象 / 凡例。
    let right_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(12), Constraint::Length(5)])
        .split(right);

    // map_cols/rows を更新（境界線ぶん -2）。
    app.map_cols = map_area.width.saturating_sub(2);
    app.map_rows = map_area.height.saturating_sub(2);

    f.render_widget(map::MapWidget { app }, map_area);
    // 機能B：雨雲 OFF のときはタイムラインバーも隠す（行は空けるだけ）。
    if app.show_radar {
        f.render_widget(timeline::Timeline { app }, timeline_area);
    }
    f.render_widget(sidebar::Sidebar { app }, right_rows[0]);
    f.render_widget(legend::Legend, right_rows[1]);

    // ヘルプ行（地図領域の最下部に重ねる）。
    render_help(f, map_area);
}

fn render_help(f: &mut Frame, map_area: Rect) {
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;

    if map_area.height < 2 {
        return;
    }
    let help = Line::from(vec![Span::styled(
        " [↑↓←→/hjkl]パン [+/-]ズーム [space]再生 [,/.]コマ送り [t]雨雲 [r]更新 [q]終了 ",
        Style::default().fg(Color::Black).bg(Color::Gray),
    )]);
    // 地図領域の最下行内側に表示。
    let y = map_area.bottom().saturating_sub(1);
    let help_area = Rect::new(map_area.x + 1, y, map_area.width.saturating_sub(2), 1);
    f.render_widget(Paragraph::new(help), help_area);
}
