//! Headless 結合スモークテスト。
//!
//! 本番 TUI は raw mode + 代替画面 + イベントループのため CI/非対話環境では
//! 起動確認できない。この example は TUI と同じデータ経路
//! （ネットワーク → wm-sources → wm-core の集約 / Braille 量子化 / colormap）を
//! 叩いて結果を stdout に出し、東京周辺の気温・CV・雨雲重畳を検証する。
//!
//! 実行: `cargo run -p wm-tui --example smoke`

use std::time::Duration;

use wm_core::agg::compass_16_ja;
use wm_core::geo::GeoBBox;
use wm_core::render::braille::quantize;
use wm_core::render::colormap::Rgb;
use wm_core::render::{rasterize_lines, PolyLine};
use wm_core::{Grid, GridKind};
use wm_sources::basemap::{BaseLineKind, BaseMapProvider};
use wm_sources::providers::{fetch_and_aggregate, Jma, OpenMeteo, OpenWeatherMap};
use wm_sources::radar::JmaNowcast;
use wm_sources::traits::WeatherProvider;

const LAT: f64 = 35.681; // 東京駅
const LON: f64 = 139.767;
const ZOOM: u8 = 8;
const COLS: u16 = 72;
const ROWS: u16 = 24;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let http = reqwest::Client::builder()
        .user_agent("amescii-smoke/0.1 (https://github.com/H1rla/amescii)")
        .timeout(Duration::from_secs(20))
        .build()?;

    println!("== amescii 結合スモークテスト ==");
    println!("中心 {LAT:.3}N {LON:.3}E  zoom={ZOOM}  領域 {COLS}x{ROWS} セル\n");

    fetch_weather(&http).await;
    fetch_radar(&http).await;
    compose_basemap_and_rain(&http).await;
    synthetic_overlay_demo();

    println!("\n== 完了 ==");
    Ok(())
}

/// 地図ベース層（地理院ベクトルタイル）+ 実雨雲を合成して描画（map.rs 相当）。
async fn compose_basemap_and_rain(http: &reqwest::Client) {
    println!("[2.5] 地図ベース層 + 雨雲 合成（静止画）");
    let bbox = current_bbox(LAT, LON, ZOOM, COLS, ROWS);

    // 地図線。
    let cache = wm_sources::cache::TileCache::shared(64);
    let basemap = match BaseMapProvider::new(http.clone(), cache).fetch_lines(bbox, ZOOM).await {
        Ok(v) => v,
        Err(e) => {
            println!("    地図取得失敗: {e}");
            return;
        }
    };
    let mut by_kind = [0usize; 4];
    for bl in &basemap {
        by_kind[match bl.kind {
            BaseLineKind::Coastline => 0,
            BaseLineKind::Boundary => 1,
            BaseLineKind::Road => 2,
            BaseLineKind::Railway => 3,
        }] += 1;
    }
    println!(
        "    線: 海岸線{} 行政界{} 道路{} 鉄道{}",
        by_kind[0], by_kind[1], by_kind[2], by_kind[3]
    );

    // 雨雲（実況最新1コマ）。
    let gw = (COLS as u32 * 2).min(256) as u16;
    let gh = (ROWS as u32 * 4).min(256) as u16;
    let grid = {
        use wm_sources::traits::RadarProvider;
        JmaNowcast::new(http.clone(), gw, gh, wm_sources::cache::TileCache::shared(64))
            .fetch_radar(bbox, ZOOM)
            .await
            .ok()
    };

    // 合成描画：①地図線（暗色）→②雨雲（上書き）。
    let mut buf: Vec<Vec<Option<(char, Rgb)>>> =
        vec![vec![None; COLS as usize]; ROWS as usize];

    // ①地図線。
    let mut polys: Vec<PolyLine> = Vec::new();
    for kind in [
        BaseLineKind::Coastline,
        BaseLineKind::Boundary,
        BaseLineKind::Road,
        BaseLineKind::Railway,
    ] {
        let color = color_for(kind);
        for bl in basemap.iter().filter(|b| b.kind == kind) {
            polys.push(PolyLine {
                points: &bl.points,
                color,
            });
        }
    }
    let line_cells = rasterize_lines(&polys, &bbox, COLS, ROWS);
    for dc in line_cells.iter() {
        if (dc.row as usize) < buf.len() && (dc.col as usize) < buf[0].len() {
            buf[dc.row as usize][dc.col as usize] = Some((dc.braille, dc.fg));
        }
    }
    println!("    地図線セル {} ", line_cells.len());

    // ②雨雲。
    if let Some(g) = &grid {
        let rain = quantize(g, COLS, ROWS);
        for dc in rain.iter() {
            if (dc.row as usize) < buf.len() && (dc.col as usize) < buf[0].len() {
                buf[dc.row as usize][dc.col as usize] = Some((dc.braille, dc.fg));
            }
        }
        println!("    雨雲セル {} (最大 {:.1} mm/h)", rain.len(), g.max_value());
    }

    print_buf(&buf);
    println!();
}

/// 地図レイヤ種別 → 色（map.rs の color_for と同じ）。
fn color_for(kind: BaseLineKind) -> Rgb {
    match kind {
        BaseLineKind::Coastline => Rgb::new(90, 120, 150),
        BaseLineKind::Boundary => Rgb::new(80, 80, 90),
        BaseLineKind::Road => Rgb::new(110, 95, 80),
        BaseLineKind::Railway => Rgb::new(100, 100, 105),
    }
}

/// 実 API から集約天気を取得して表示（サイドバー相当）。
async fn fetch_weather(http: &reqwest::Client) {
    println!("[1] 集約天気（JMA + Open-Meteo, OWM はキーがあれば）");

    let mut providers: Vec<Box<dyn WeatherProvider>> = Vec::new();
    let area = Jma::area_for(LAT, LON);
    println!("    JMA area_code = {area}");
    providers.push(Box::new(Jma::new(http.clone(), area)));
    providers.push(Box::new(OpenMeteo::new(http.clone())));
    if let Ok(key) = std::env::var("OWM_API_KEY") {
        if !key.is_empty() {
            providers.push(Box::new(OpenWeatherMap::new(http.clone(), key)));
        }
    }

    match fetch_and_aggregate(&providers, LAT, LON).await {
        Ok(snap) => {
            let t = &snap.temp_c;
            println!(
                "    気温 {:.1}°C  ±{:.2}  CV {:.1}%  信頼 {:.0}%  使用{}/除外{}",
                t.value,
                (t.cv * t.value).abs(),
                t.cv * 100.0,
                t.confidence * 100.0,
                t.n_used,
                t.n_excluded
            );
            print_metric("湿度", &snap.humidity_pct, "%");
            let dir = compass_16_ja(snap.wind_dir_deg.value);
            if snap.wind_ms.value.is_finite() {
                println!(
                    "    風   {:.1} m/s {}  (集中度 {:.2})",
                    snap.wind_ms.value, dir, snap.wind_dir_deg.confidence
                );
            } else {
                println!("    風   --");
            }
            print_metric("降水", &snap.precip_mmh, "mm/h");
            println!("    天気 {}", snap.condition.label_ja());
        }
        Err(e) => println!("    取得失敗: {e}"),
    }
    println!();
}

fn print_metric(label: &str, a: &wm_core::model::AggregatedValue, unit: &str) {
    if a.value.is_finite() {
        println!(
            "    {label} {:.1}{unit}  CV {:.1}%  使用{}",
            a.value,
            a.cv * 100.0,
            a.n_used
        );
    } else {
        println!("    {label} --");
    }
}

/// 実 JMA ナウキャストから雨雲グリッドを取得し、量子化して地図表示。
async fn fetch_radar(http: &reqwest::Client) {
    println!("[2] 雨雲レーダー（JMA 高解像度降水ナウキャスト）");

    let bbox = current_bbox(LAT, LON, ZOOM, COLS, ROWS);
    println!(
        "    bbox {:.3}..{:.3}N {:.3}..{:.3}E",
        bbox.min_lat, bbox.max_lat, bbox.min_lon, bbox.max_lon
    );

    let gw = (COLS as u32 * 2).min(256) as u16;
    let gh = (ROWS as u32 * 4).min(256) as u16;
    let nowcast = JmaNowcast::new(http.clone(), gw, gh, wm_sources::cache::TileCache::shared(64));

    use wm_sources::traits::RadarProvider;
    match nowcast.fetch_radar(bbox, ZOOM).await {
        Ok(grid) => {
            let max = grid.max_value();
            println!("    グリッド {gw}x{gh}  最大降水 {max:.2} mm/h");
            if max <= 0.0 {
                println!("    → 現在この領域に降水なし（透過＝正常。雨雲は描画されない）");
            } else {
                println!("    → 降水あり。truecolor 雨雲を重畳描画:");
            }
            render_grid(&grid, COLS, ROWS);
        }
        Err(e) => println!("    取得失敗: {e}"),
    }
    println!();
}

/// 降水の有無に依らず Braille + truecolor 重畳の描画経路を見せる合成デモ。
fn synthetic_overlay_demo() {
    println!("[3] 合成雨域デモ（描画パイプライン検証・ネットワーク非依存）");
    let bbox = GeoBBox::new(35.0, 139.0, 36.0, 140.0);
    let gw = (COLS * 2).min(256);
    let gh = (ROWS * 4).min(256);
    let mut grid = Grid::new_zeroed(gw, gh, GridKind::PrecipMmH, bbox).expect("grid alloc");

    // 中央に強度勾配のある円形の雨域を置く（中心ほど強い雨＝赤紫、外縁ほど弱い＝水色）。
    let (cx, cy) = (gw as f32 * 0.5, gh as f32 * 0.45);
    let radius = (gh as f32) * 0.40;
    for y in 0..gh {
        for x in 0..gw {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < radius {
                // 中心 80mm/h → 外縁 0.2mm/h の線形勾配。
                let mmh = 80.0 * (1.0 - dist / radius);
                grid.set(x, y, mmh);
            }
        }
    }
    println!("    最大降水 {:.1} mm/h（合成）:", grid.max_value());
    render_grid(&grid, COLS, ROWS);
}

/// `Grid` を量子化し、ANSI 24bit truecolor で端末に出力する（map.rs 相当）。
fn render_grid(grid: &Grid, cols: u16, rows: u16) {
    let cells = quantize(grid, cols, rows);
    let mut buf: Vec<Vec<Option<(char, Rgb)>>> = vec![vec![None; cols as usize]; rows as usize];
    for dc in cells.iter() {
        if (dc.row as usize) < buf.len() && (dc.col as usize) < buf[0].len() {
            buf[dc.row as usize][dc.col as usize] = Some((dc.braille, dc.fg));
        }
    }
    print_buf(&buf);
    println!("    描画セル数 {}", cells.len());
}

/// 文字バッファを ANSI 24bit truecolor で枠付き出力。
fn print_buf(buf: &[Vec<Option<(char, Rgb)>>]) {
    let cols = buf.first().map(|r| r.len()).unwrap_or(0);
    let border: String = std::iter::repeat('─').take(cols).collect();
    println!("    ┌{border}┐");
    for row in buf {
        let mut line = String::from("    │");
        for cell in row {
            match cell {
                Some((ch, c)) => {
                    line.push_str(&format!("\x1b[38;2;{};{};{}m{ch}\x1b[0m", c.r, c.g, c.b));
                }
                None => line.push('·'),
            }
        }
        line.push('│');
        println!("{line}");
    }
    println!("    └{border}┘");
}

/// `App::current_bbox` と同じ式（example から binary 内 App を参照できないため複製）。
fn current_bbox(lat: f64, lon: f64, zoom: u8, cols: u16, rows: u16) -> GeoBBox {
    let world_px = 256.0_f64 * (1u64 << zoom) as f64;
    let view_px_x = cols as f64 * 2.0;
    let view_px_y = rows as f64 * 4.0;
    let lon_span = 360.0 * view_px_x / world_px;
    let lat_rad = lat.to_radians();
    let lat_span = 360.0 * view_px_y / world_px * lat_rad.cos();
    GeoBBox::new(
        lat - lat_span / 2.0,
        lon - lon_span / 2.0,
        lat + lat_span / 2.0,
        lon + lon_span / 2.0,
    )
}
