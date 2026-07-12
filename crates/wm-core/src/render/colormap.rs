//! 色マッピング：雨量・雲量 → RGB。truecolor 前提。
//!
//! 雨量配色は気象庁・高解像度降水ナウキャストの段階配色に準拠。

/// truecolor RGB。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// 雨量(mm/h) → RGB。JMA 段階配色準拠。
///
/// 0.1mm/h 未満は「降水なし」として `None`（＝描画しない）。
pub fn precip_to_rgb(mmh: f32) -> Option<Rgb> {
    // JMA 高解像度降水ナウキャスト相当の閾値と色。
    if mmh < 0.1 {
        None
    } else if mmh < 1.0 {
        Some(Rgb::new(0xC0, 0xD8, 0xFF)) // 薄水色
    } else if mmh < 5.0 {
        Some(Rgb::new(0x60, 0xA8, 0xFF)) // 水色
    } else if mmh < 10.0 {
        Some(Rgb::new(0x21, 0x8C, 0xFF)) // 青
    } else if mmh < 20.0 {
        Some(Rgb::new(0x00, 0xC8, 0x00)) // 緑
    } else if mmh < 30.0 {
        Some(Rgb::new(0xFA, 0xF5, 0x00)) // 黄
    } else if mmh < 50.0 {
        Some(Rgb::new(0xFF, 0x99, 0x00)) // 橙
    } else if mmh < 80.0 {
        Some(Rgb::new(0xFF, 0x28, 0x28)) // 赤
    } else {
        Some(Rgb::new(0xB4, 0x00, 0xB4)) // 紫（猛烈な雨）
    }
}

/// 雲量(%) → RGB。白〜灰のグレースケール系ランプ。
///
/// 10% 未満は「ほぼ快晴」として `None`（背景＝地図を見せる）。
pub fn cloud_to_rgb(pct: f32) -> Option<Rgb> {
    if pct < 10.0 {
        None
    } else if pct < 30.0 {
        Some(Rgb::new(0xD8, 0xDC, 0xE0)) // 薄い雲
    } else if pct < 60.0 {
        Some(Rgb::new(0xAC, 0xB2, 0xB8)) // 中程度
    } else if pct < 85.0 {
        Some(Rgb::new(0x84, 0x8A, 0x90)) // 厚い
    } else {
        Some(Rgb::new(0x5C, 0x62, 0x68)) // どんより
    }
}

/// 凡例の段階（雨量）。UI の色凡例描画に使う。
/// (下限mm/h, ラベル, 色)。
pub const PRECIP_LEGEND: [(f32, &str, Rgb); 8] = [
    (0.1, "0.1", Rgb::new(0xC0, 0xD8, 0xFF)),
    (1.0, "1", Rgb::new(0x60, 0xA8, 0xFF)),
    (5.0, "5", Rgb::new(0x21, 0x8C, 0xFF)),
    (10.0, "10", Rgb::new(0x00, 0xC8, 0x00)),
    (20.0, "20", Rgb::new(0xFA, 0xF5, 0x00)),
    (30.0, "30", Rgb::new(0xFF, 0x99, 0x00)),
    (50.0, "50", Rgb::new(0xFF, 0x28, 0x28)),
    (80.0, "80+", Rgb::new(0xB4, 0x00, 0xB4)),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_rain_is_none() {
        assert_eq!(precip_to_rgb(0.0), None);
        assert_eq!(precip_to_rgb(0.05), None);
    }

    #[test]
    fn light_rain_light_blue() {
        assert_eq!(precip_to_rgb(0.5), Some(Rgb::new(0xC0, 0xD8, 0xFF)));
    }

    #[test]
    fn heavy_rain_red_then_purple() {
        assert_eq!(precip_to_rgb(60.0), Some(Rgb::new(0xFF, 0x28, 0x28)));
        assert_eq!(precip_to_rgb(120.0), Some(Rgb::new(0xB4, 0x00, 0xB4)));
    }

    #[test]
    fn clear_sky_none() {
        assert_eq!(cloud_to_rgb(5.0), None);
    }

    #[test]
    fn overcast_dark() {
        assert_eq!(cloud_to_rgb(95.0), Some(Rgb::new(0x5C, 0x62, 0x68)));
    }
}
