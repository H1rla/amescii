//! アプリ状態。中心座標・ズーム・最新スナップショット・最新グリッド。

use wm_core::geo::GeoBBox;
use wm_core::WeatherSnapshot;
use wm_sources::basemap::{BaseLine, NameLabelJa};
use wm_sources::radar::RadarFrame;

pub struct App {
    /// 地図中心。
    pub center_lat: f64,
    pub center_lon: f64,
    pub zoom: u8,

    /// 最新の集約天気（取得できていれば）。
    pub snapshot: Option<WeatherSnapshot>,
    /// 雨雲タイムライン（時系列・昇順）。空なら未取得。
    pub frames: Vec<RadarFrame>,
    /// 現在表示中のコマ（`frames` のインデックス）。
    pub frame_idx: usize,
    /// 自動再生中か。
    pub playing: bool,
    /// 雨雲レイヤの表示 ON/OFF（機能B, `t` キー）。OFF で地図＋地名だけになる。
    pub show_radar: bool,
    /// 地図ベース層（海岸線・行政界・道路・鉄道の点列）。ズーム/中心変更時のみ再取得。
    pub basemap: Option<Vec<BaseLine>>,
    /// 拡大時の日本語地名（地理院 label 由来）。zoom>=JA_LABEL_ZOOM のときだけ取得。
    pub name_labels_ja: Option<Vec<NameLabelJa>>,

    /// ステータス行に出す短いメッセージ。
    pub status: String,
    /// 終了フラグ。
    pub should_quit: bool,

    /// マップ描画領域のセル数（描画時に更新）。BBox 計算に使う。
    pub map_cols: u16,
    pub map_rows: u16,
}

impl App {
    pub fn new(lat: f64, lon: f64, zoom: u8) -> Self {
        Self {
            center_lat: lat,
            center_lon: lon,
            zoom,
            snapshot: None,
            frames: Vec::new(),
            frame_idx: 0,
            playing: false,
            show_radar: true,
            basemap: None,
            name_labels_ja: None,
            status: String::from("起動中..."),
            should_quit: false,
            map_cols: 80,
            map_rows: 40,
        }
    }

    /// 現在表示中のフレーム（無ければ None）。
    pub fn current_frame(&self) -> Option<&RadarFrame> {
        self.frames.get(self.frame_idx)
    }

    /// 新しいタイムラインを差し込む。表示位置は「現在（最新の実況）」に合わせる。
    ///
    /// 取得直後は最新の実況コマを見せたいので、`frame_idx` を最後の実況コマへ。
    /// 予報が無ければ末尾。再生状態は維持する。
    pub fn set_frames(&mut self, frames: Vec<RadarFrame>) {
        self.frames = frames;
        if self.frames.is_empty() {
            self.frame_idx = 0;
            return;
        }
        // 最後の実況（is_forecast=false）コマ。無ければ末尾。
        self.frame_idx = self
            .frames
            .iter()
            .rposition(|f| !f.is_forecast)
            .unwrap_or(self.frames.len() - 1);
    }

    /// 再生/一時停止トグル（フレームが無ければ何もしない）。
    pub fn toggle_play(&mut self) {
        if !self.frames.is_empty() {
            self.playing = !self.playing;
        }
    }

    /// 雨雲表示の ON/OFF トグル（機能B）。OFF にしたら自動再生も止める
    /// （タイムラインを隠すので前進させる意味がない＝無駄な再描画を避ける）。
    pub fn toggle_radar(&mut self) {
        self.show_radar = !self.show_radar;
        if !self.show_radar {
            self.playing = false;
        }
    }

    /// 手動コマ送り。再生は止める（手送りと自動再生の競合を避ける）。
    pub fn step_frame(&mut self, delta: isize) {
        self.playing = false;
        if self.frames.is_empty() {
            return;
        }
        let n = self.frames.len() as isize;
        self.frame_idx = (self.frame_idx as isize + delta).rem_euclid(n) as usize;
    }

    /// 自動再生の1コマ前進（末尾で先頭へループ）。
    pub fn advance_play(&mut self) {
        if self.frames.is_empty() {
            return;
        }
        self.frame_idx = (self.frame_idx + 1) % self.frames.len();
    }

    /// 現在の中心・ズーム・マップサイズから表示 BBox を概算する。
    ///
    /// 1セル=Braille 2x4ドット。zoom と緯度からおおよその度幅を求める簡易式。
    pub fn current_bbox(&self) -> GeoBBox {
        // Web Mercator: zoom z で世界は 256*2^z ピクセル。
        // マップ領域のピクセル数 = cols*2 (横) × rows*4 (縦)。
        let world_px = 256.0_f64 * (1u64 << self.zoom) as f64;
        let view_px_x = self.map_cols as f64 * 2.0;
        let view_px_y = self.map_rows as f64 * 4.0;

        // 経度方向：360度が world_px に対応。
        let lon_span = 360.0 * view_px_x / world_px;
        // 緯度方向：メルカトルなので中心緯度のスケールを掛ける。
        let lat_rad = self.center_lat.to_radians();
        let lat_span = 360.0 * view_px_y / world_px * lat_rad.cos();

        GeoBBox::new(
            self.center_lat - lat_span / 2.0,
            self.center_lon - lon_span / 2.0,
            self.center_lat + lat_span / 2.0,
            self.center_lon + lon_span / 2.0,
        )
    }

    /// ズームに応じたパン幅（表示幅に対する割合）。
    /// 広域は一歩が大きくなりすぎ、拡大は小さくなりすぎるのを避けるため段階調整。
    pub fn pan_fraction(&self) -> f64 {
        match self.zoom {
            0..=5 => 0.10,
            6..=9 => 0.20,
            10..=13 => 0.30,
            _ => 0.40,
        }
    }

    /// パン（方向キー）。ステップは表示幅の一定割合。
    pub fn pan(&mut self, d_lat_frac: f64, d_lon_frac: f64) {
        let bbox = self.current_bbox();
        let lat_span = bbox.max_lat - bbox.min_lat;
        let lon_span = bbox.max_lon - bbox.min_lon;
        self.center_lat = (self.center_lat + d_lat_frac * lat_span).clamp(-85.0, 85.0);
        self.center_lon += d_lon_frac * lon_span;
        // 経度は -180..180 に巻き戻す。
        if self.center_lon > 180.0 {
            self.center_lon -= 360.0;
        } else if self.center_lon < -180.0 {
            self.center_lon += 360.0;
        }
    }

    pub fn zoom_in(&mut self) {
        // 地図（ベクトルタイル）は z16 まで精細化できる。雨雲は radar.rs 側で
        // RADAR_MAX_ZOOM(=10) にクランプしオーバーズーム表示する（ズーム分離）。
        if self.zoom < 16 {
            self.zoom += 1;
        }
    }

    pub fn zoom_out(&mut self) {
        if self.zoom > 3 {
            self.zoom -= 1;
        }
    }
}

/// 拡大時に日本語地名（地理院 label レイヤ）へ切り替える境界ズーム。
/// これ未満は内蔵英語都市テーブル（places.rs）。
pub const JA_LABEL_ZOOM: u8 = 11;
