//! 雨量色凡例。truecolor で JMA 段階配色を表示。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};
use wm_core::render::colormap::PRECIP_LEGEND;

pub struct Legend;

impl Widget for Legend {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" 雨量 mm/h ");
        let inner = block.inner(area);
        block.render(area, buf);

        // 各段を「■ ラベル」で並べる。横幅に応じて折り返す。
        let mut spans: Vec<Span> = Vec::new();
        for (_, label, rgb) in PRECIP_LEGEND.iter() {
            spans.push(Span::styled(
                "■",
                Style::default().fg(Color::Rgb(rgb.r, rgb.g, rgb.b)),
            ));
            spans.push(Span::raw(format!("{} ", label)));
        }

        // 出典明示（アプリ内表示の要件）。雨雲＝気象庁ナウキャスト、地図＝地理院。
        let lines = vec![
            Line::from(spans),
            Line::from(Span::styled(
                "出典: 気象庁 / 国土地理院",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        Paragraph::new(lines).render(inner, buf);
    }
}
