//! 雨雲タイムラインバー：マップ下部の1行。
//!
//! フレーム列を `━` で表し、現在位置を `●`、実況/予報を色で分ける。
//! 現在コマの時刻（`valid_unix` をローカル `HH:MM` に整形）と「実況/予報」
//! ラベル、再生(`▶`)/停止(`⏸`)を右側に出す。

use crate::app::App;
use chrono::{Local, TimeZone, Utc};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

pub struct Timeline<'a> {
    pub app: &'a App,
}

/// 実況コマの色（白〜灰）。
const OBS_COLOR: Color = Color::Gray;
/// 予報コマの色（水色系）。
const FORECAST_COLOR: Color = Color::Cyan;

impl<'a> Widget for Timeline<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let frames = &self.app.frames;
        if frames.is_empty() {
            let line = Line::from(Span::styled(
                " 雨雲タイムライン: 取得待ち...",
                Style::default().fg(Color::DarkGray),
            ));
            Paragraph::new(line).render(area, buf);
            return;
        }

        let idx = self.app.frame_idx.min(frames.len() - 1);
        let mut spans: Vec<Span> = Vec::new();

        // 過去マーカー。
        spans.push(Span::styled("過去 ◀", Style::default().fg(Color::DarkGray)));
        spans.push(Span::raw(" "));

        // フレーム列。現在位置は ●、それ以外は ━。色は実況/予報で分ける。
        for (i, f) in frames.iter().enumerate() {
            let base = if f.is_forecast {
                FORECAST_COLOR
            } else {
                OBS_COLOR
            };
            if i == idx {
                spans.push(Span::styled(
                    "●",
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled("━", Style::default().fg(base)));
            }
        }

        // 未来マーカー。
        spans.push(Span::raw(" "));
        spans.push(Span::styled("▶ 未来", Style::default().fg(Color::DarkGray)));

        // 現在コマの時刻・ラベル・再生状態。
        let cur = &frames[idx];
        let (label, label_color) = if cur.is_forecast {
            ("予報", FORECAST_COLOR)
        } else {
            ("実況", Color::White)
        };
        let play = if self.app.playing { "▶再生" } else { "⏸停止" };

        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("{} ", hhmm_local(cur.valid_unix)),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(label, Style::default().fg(label_color)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(play, Style::default().fg(Color::Yellow)));

        Paragraph::new(Line::from(spans)).render(area, buf);
    }
}

/// unix 秒（UTC 基準）→ ローカルタイムゾーンの `HH:MM`。
fn hhmm_local(unix: u64) -> String {
    match Utc.timestamp_opt(unix as i64, 0).single() {
        Some(dt) => dt.with_timezone(&Local).format("%H:%M").to_string(),
        None => "--:--".to_string(),
    }
}
