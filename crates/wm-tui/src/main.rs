//! weathermap エントリポイント。
//!
//! 端末セットアップ → 初回取得 → イベントループ（入力＋取得結果＋再生）→ 後始末。
//!
//! 取得（天気・雨雲タイムライン・地図）はすべてバックグラウンド `tokio::spawn`
//! に出し、結果は `mpsc` で受ける。入力は `spawn_blocking` の専用スレッドで
//! 読んでチャネルへ送る。これによりイベントループは取得・入力どちらにも
//! ブロックされず、取得中も再生（frame_idx 前進）と再描画が動き続ける。

mod app;
mod config;
mod input;
mod ui;

use anyhow::Result;
use app::App;
use config::Config;
use crossterm::{
    event::{self, Event, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use input::{handle_key, InputAction};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io::stdout;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use app::JA_LABEL_ZOOM;
use wm_sources::basemap::{BaseLine, BaseMapProvider, NameLabelJa};
use wm_sources::cache::{SharedCache, TileCache};
use wm_sources::providers::{fetch_and_aggregate, Jma, OpenMeteo, OpenWeatherMap};
use wm_sources::radar::{JmaNowcast, RadarFrame};
use wm_sources::traits::WeatherProvider;
use wm_core::WeatherSnapshot;

/// タイムラインに載せる最大コマ数（実況＋予報、軽さ優先）。
const MAX_FRAMES: usize = 13;
/// 自動再生の前進間隔。
const PLAY_INTERVAL: Duration = Duration::from_millis(500);
/// アイドル時の再描画/再生ティック。
const TICK: Duration = Duration::from_millis(100);
/// タイルキャッシュ上限（エントリ数）。地図ベクトルタイルと雨雲 PNG で別枠。
const MAP_CACHE_CAP: usize = 256;
const RADAR_CACHE_CAP: usize = 256;
/// ズーム/パン連打時のデバウンス。最後の操作からこの時間が経つまで取得しない。
const REFETCH_DEBOUNCE: Duration = Duration::from_millis(200);

/// バックグラウンド取得タスク → イベントループへ返す結果。
/// エラーはステータス表示用に文字列化して渡す（端末を壊さない）。
enum Msg {
    Weather(std::result::Result<WeatherSnapshot, String>),
    Radar(std::result::Result<Vec<RadarFrame>, String>),
    Basemap(std::result::Result<Vec<BaseLine>, String>),
    LabelsJa(std::result::Result<Vec<NameLabelJa>, String>),
}

fn main() -> Result<()> {
    // tokio worker のスタックを拡張する。wm-core の戻り値型は heapless インライン
    // （Grid≈256KB、CellVec/LineCellVec≈192KB）で、生成・量子化時にこれらが値で
    // スタックに載る。既定 2MB だと debug ビルドの多重コピーで worker が溢れるため、
    // 余裕を持って 16MB にする（移植性に関わる wm-core の型は変更しない方針のため）。
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(16 * 1024 * 1024)
        .build()?;
    rt.block_on(async_main())
}

async fn async_main() -> Result<()> {
    let cfg = Config::load()?;

    // panic hook：パニック時に raw mode 解除 + 代替スクリーン離脱で端末を復帰させる。
    // 取得タスクや描画が panic しても端末が壊れたまま残らないようにする。
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        original_hook(info);
    }));

    // 端末セットアップ。
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    let res = run(&mut terminal, cfg).await;

    // 後始末（通常終了パス。panic 時は hook が同等処理を行う）。
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    res
}

async fn run<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>, cfg: Config) -> Result<()> {
    let mut app = App::new(cfg.startup.lat, cfg.startup.lon, cfg.startup.zoom);

    let http = reqwest::Client::builder()
        .user_agent("weathermap/0.1 (https://github.com/H1rla/weathermap)")
        .timeout(Duration::from_secs(15))
        .build()?;
    let owm_key = cfg.sources.owm_api_key.clone();

    // タイルキャッシュ（セッション中共有・メモリのみ）。
    // 地図(bvmap .pbf)は fetch_lines/fetch_labels_ja が同一 URL を引くので共有。
    // 雨雲(JMA PNG)は別枠＝ディスク永続化しない（古いフレームを掴まない）。
    let map_cache: SharedCache = TileCache::shared(MAP_CACHE_CAP);
    let radar_cache: SharedCache = TileCache::shared(RADAR_CACHE_CAP);

    // 取得結果チャネル。
    let (tx, mut rx) = mpsc::channel::<Msg>(16);

    // 入力読み取りは専用ブロッキングスレッドへ。キーイベントだけ転送する。
    // event::read() はブロックするので、ここを spawn_blocking に隔離して
    // イベントループ側（select!）が入力に固まらないようにする。
    let (in_tx, mut in_rx) = mpsc::channel::<KeyEvent>(32);
    tokio::task::spawn_blocking(move || loop {
        match event::read() {
            Ok(Event::Key(k)) => {
                if in_tx.blocking_send(k).is_err() {
                    break; // 受け手が消えた＝終了
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    });

    // 初回：一度描画して map_cols/rows を実領域に合わせてから取得を投げる
    // （BBox・グリッド解像度が実画面サイズに依存するため）。初回はデバウンスしない。
    terminal.draw(|f| ui::draw(f, &mut app))?;
    trigger_refetch(&app, &tx, &http, &owm_key, &map_cache, &radar_cache);

    let weather_interval = Duration::from_secs(cfg.refresh.weather_secs.max(60));
    let radar_interval = Duration::from_secs(cfg.refresh.radar_secs.max(60));
    let mut last_weather = Instant::now();
    let mut last_radar = Instant::now();
    let mut last_advance = Instant::now();
    // デバウンス：パン/ズーム連打を最後の1回に集約する。
    let mut pending_refetch = false;
    let mut last_input = Instant::now();

    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;

        // 入力・取得結果・再生ティックのいずれかで起床する。どれにもブロックしない。
        tokio::select! {
            Some(key) = in_rx.recv() => {
                match handle_key(&mut app, key) {
                    InputAction::Quit => break,
                    InputAction::Refetch => {
                        // 広域へ戻ったら日本語地名は捨てる（描画されないが残骸を残さない）。
                        if app.zoom < JA_LABEL_ZOOM {
                            app.name_labels_ja = None;
                        }
                        // 即取得せずデバウンス：連打が落ち着いてから1回だけ取得する
                        // （中間状態のタイルを取りに行かない）。
                        pending_refetch = true;
                        last_input = Instant::now();
                    }
                    InputAction::None => {}
                }
            }
            Some(msg) = rx.recv() => apply_msg(&mut app, msg),
            _ = tokio::time::sleep(TICK) => {}
        }

        if app.should_quit {
            break;
        }

        // デバウンス発火：最後の操作から一定時間が経ったら取得を1回投げる。
        if pending_refetch && last_input.elapsed() >= REFETCH_DEBOUNCE {
            trigger_refetch(&app, &tx, &http, &owm_key, &map_cache, &radar_cache);
            last_weather = Instant::now();
            last_radar = Instant::now();
            pending_refetch = false;
        }

        // 自動再生：一定間隔で 1 コマ前進（末尾→先頭ループ）。
        // 雨雲 OFF 中は前進させない（タイムラインを隠しているため）。
        if app.show_radar && app.playing && last_advance.elapsed() >= PLAY_INTERVAL {
            app.advance_play();
            last_advance = Instant::now();
        }

        // 定期更新（天気・雨雲）。地図は中心/ズーム変更時のみなのでここでは出さない。
        if last_weather.elapsed() >= weather_interval {
            spawn_weather(tx.clone(), http.clone(), app.center_lat, app.center_lon, owm_key.clone());
            last_weather = Instant::now();
        }
        if last_radar.elapsed() >= radar_interval {
            let (bbox, gw, gh) = radar_params(&app);
            spawn_radar(tx.clone(), http.clone(), radar_cache.clone(), bbox, app.zoom, gw, gh);
            last_radar = Instant::now();
        }
    }

    Ok(())
}

/// 取得結果を App へ反映する。
fn apply_msg(app: &mut App, msg: Msg) {
    match msg {
        Msg::Weather(Ok(snap)) => {
            app.snapshot = Some(snap);
            app.status = "天気更新済み".into();
        }
        Msg::Weather(Err(e)) => app.status = format!("天気取得失敗: {e}"),
        Msg::Radar(Ok(frames)) => {
            let n = frames.len();
            app.set_frames(frames);
            app.status = format!("雨雲更新済み（{n}コマ）");
        }
        Msg::Radar(Err(e)) => app.status = format!("雨雲取得失敗: {e}"),
        Msg::Basemap(Ok(lines)) => {
            app.basemap = Some(lines);
            app.status = "地図更新済み".into();
        }
        Msg::Basemap(Err(e)) => app.status = format!("地図取得失敗: {e}"),
        Msg::LabelsJa(Ok(labels)) => {
            app.name_labels_ja = Some(labels);
        }
        Msg::LabelsJa(Err(e)) => app.status = format!("地名取得失敗: {e}"),
    }
}

/// 中心/ズーム/サイズ変更時の全取得（天気・地図・雨雲・拡大時日本語地名）を投げる。
fn trigger_refetch(
    app: &App,
    tx: &mpsc::Sender<Msg>,
    http: &reqwest::Client,
    owm_key: &str,
    map_cache: &SharedCache,
    radar_cache: &SharedCache,
) {
    let bbox = app.current_bbox();
    let (_, gw, gh) = radar_params(app);
    spawn_weather(tx.clone(), http.clone(), app.center_lat, app.center_lon, owm_key.to_string());
    spawn_basemap(tx.clone(), http.clone(), map_cache.clone(), bbox, app.zoom);
    spawn_radar(tx.clone(), http.clone(), radar_cache.clone(), bbox, app.zoom, gw, gh);
    // 拡大時のみ日本語地名を取得（広域は内蔵英語都市で足りる＋負荷削減）。
    // 地図と同じ map_cache を共有＝同一 (z,x,y).pbf がヒットする。
    if app.zoom >= JA_LABEL_ZOOM {
        spawn_labels_ja(tx.clone(), http.clone(), map_cache.clone(), bbox, app.zoom);
    }
}

/// 雨雲取得に必要な BBox とグリッド解像度（マップ領域に概ね合わせ、上限 256）。
fn radar_params(app: &App) -> (wm_core::geo::GeoBBox, u16, u16) {
    let bbox = app.current_bbox();
    let gw = (app.map_cols as u32 * 2).clamp(1, 256) as u16;
    let gh = (app.map_rows as u32 * 4).clamp(1, 256) as u16;
    (bbox, gw, gh)
}

fn spawn_weather(
    tx: mpsc::Sender<Msg>,
    http: reqwest::Client,
    lat: f64,
    lon: f64,
    owm_key: String,
) {
    tokio::spawn(async move {
        let mut providers: Vec<Box<dyn WeatherProvider>> = Vec::new();
        let area = Jma::area_for(lat, lon);
        providers.push(Box::new(Jma::new(http.clone(), area)));
        providers.push(Box::new(OpenMeteo::new(http.clone())));
        if !owm_key.is_empty() {
            providers.push(Box::new(OpenWeatherMap::new(http.clone(), owm_key)));
        }
        let r = fetch_and_aggregate(&providers, lat, lon)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(Msg::Weather(r)).await;
    });
}

fn spawn_radar(
    tx: mpsc::Sender<Msg>,
    http: reqwest::Client,
    cache: SharedCache,
    bbox: wm_core::geo::GeoBBox,
    zoom: u8,
    gw: u16,
    gh: u16,
) {
    tokio::spawn(async move {
        let nowcast = JmaNowcast::new(http, gw, gh, cache);
        let r = nowcast
            .fetch_radar_timeline(bbox, zoom, MAX_FRAMES)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(Msg::Radar(r)).await;
    });
}

fn spawn_basemap(
    tx: mpsc::Sender<Msg>,
    http: reqwest::Client,
    cache: SharedCache,
    bbox: wm_core::geo::GeoBBox,
    zoom: u8,
) {
    tokio::spawn(async move {
        let provider = BaseMapProvider::new(http, cache);
        let r = provider
            .fetch_lines(bbox, zoom)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(Msg::Basemap(r)).await;
    });
}

/// 拡大時の日本語地名（地理院 label）を取得（zoom>=JA_LABEL_ZOOM のときのみ呼ぶ）。
fn spawn_labels_ja(
    tx: mpsc::Sender<Msg>,
    http: reqwest::Client,
    cache: SharedCache,
    bbox: wm_core::geo::GeoBBox,
    zoom: u8,
) {
    tokio::spawn(async move {
        let provider = BaseMapProvider::new(http, cache);
        let r = provider
            .fetch_labels_ja(bbox, zoom)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(Msg::LabelsJa(r)).await;
    });
}
