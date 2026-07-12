//! サイドバー：集約天気の数値と、ソース間比較バー（乖離の可視化）。

use crate::app::App;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};
use wm_core::agg::compass_16_ja;
use wm_core::model::AggregatedValue;

pub struct Sidebar<'a> {
    pub app: &'a App,
}

impl<'a> Widget for Sidebar<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(" 気象 ");
        let inner = block.inner(area);
        block.render(area, buf);

        let mut lines: Vec<Line> = Vec::new();

        // 位置。
        lines.push(Line::from(format!(
            "位置 {:.3}N {:.3}E z{}",
            self.app.center_lat, self.app.center_lon, self.app.zoom
        )));
        lines.push(Line::from(""));

        if let Some(snap) = &self.app.snapshot {
            // 各指標を「値 ±幅 信頼度」で表示。
            lines.push(metric_line("気温", &snap.temp_c, "°C", 1));
            lines.push(metric_line("湿度", &snap.humidity_pct, "%", 0));

            // 風：速度 + 風向（方位）。
            let dir = compass_16_ja(snap.wind_dir_deg.value);
            let wind_val = if snap.wind_ms.value.is_finite() {
                format!("{:.1}", snap.wind_ms.value)
            } else {
                "--".into()
            };
            lines.push(Line::from(format!("風   {} m/s {}", wind_val, dir)));

            lines.push(metric_line("降水", &snap.precip_mmh, "mm/h", 1));
            lines.push(Line::from(""));
            lines.push(Line::from(format!("天気 {}", snap.condition.label_ja())));
            lines.push(Line::from(""));

            // ソース一致度（気温の CV を代表値として表示）。
            let cv_pct = snap.temp_c.cv * 100.0;
            let conf_pct = snap.temp_c.confidence * 100.0;
            lines.push(Line::from(vec![
                Span::raw("一致 CV "),
                Span::styled(
                    format!("{:.1}%", cv_pct),
                    Style::default().fg(cv_color(snap.temp_c.cv)),
                ),
            ]));
            lines.push(Line::from(format!(
                "信頼 {:.0}%  使用{}/除外{}",
                conf_pct, snap.temp_c.n_used, snap.temp_c.n_excluded
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "天気データ取得待ち...",
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            &self.app.status,
            Style::default().fg(Color::DarkGray),
        )));

        Paragraph::new(lines).render(inner, buf);
    }
}

/// 「ラベル 値±幅 (信頼バー)」の1行。
fn metric_line(label: &str, a: &AggregatedValue, unit: &str, prec: usize) -> Line<'static> {
    if !a.value.is_finite() {
        return Line::from(format!("{:<4} --", label));
    }
    // ±幅は cv*value を目安に。
    let spread = (a.cv * a.value).abs();
    let val = format!("{:.*}", prec, a.value);
    let spr = format!("{:.*}", prec, spread);
    Line::from(vec![
        Span::raw(format!("{:<4} ", label)),
        Span::styled(
            format!("{}{}", val, unit),
            Style::default().fg(Color::White),
        ),
        Span::styled(
            format!("  ±{}", spr),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

/// CV に応じた色（一致＝緑、乖離＝赤）。
fn cv_color(cv: f32) -> Color {
    if cv < 0.05 {
        Color::Green
    } else if cv < 0.12 {
        Color::Yellow
    } else {
        Color::Red
    }
}
