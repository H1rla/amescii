//! 雨雲レーダー取得：JMA 高解像度降水ナウキャスト PNG タイル → `wm-core::Grid`。
//!
//! 仕組み（雲レンダーの中核）:
//! 1. targetTimes_N1/N2.json から basetime/validtime を取得（実況＋予報）。
//!    ※現在時刻から推測してはいけない。404 が CDN にキャッシュされ有害。
//! 2. 表示 BBox + zoom から必要タイル範囲 (x,y) を算出（wm-core::geo）。
//! 3. 各 PNG タイル(256x256, 透明=無降水)を取得・デコード。
//! 4. JMA 配色をピクセルから逆引きして雨量レベル(mm/h)へ量子化。
//! 5. 1枚の Grid(PrecipMmH) に合成して返す。
//!
//! タイムライン（複数コマ）は `fetch_radar_timeline`：実況(N1)＋予報(N2)の時刻を
//! 集めて昇順整列・間引きし、各時刻ぶんの Grid を `RadarFrame` 列にして返す。

use crate::cache::{fetch_cached, SharedCache};
use crate::error::{Result, SourceError};
use crate::traits::RadarProvider;
use async_trait::async_trait;
use image::GenericImageView;
use serde::Deserialize;
use wm_core::geo::{bbox_to_tile_range, lonlat_to_pixel};
use wm_core::{GeoBBox, Grid, GridKind};

const TILE_SIZE: u32 = 256;
const NOWCAST_BASE: &str = "https://www.jma.go.jp/bosai/jmatile/data/nowc";
/// 実況（過去〜現在）。
const TARGET_TIMES_N1: &str = "https://www.jma.go.jp/bosai/jmatile/data/nowc/targetTimes_N1.json";
/// 予報（現在〜1時間先）。
const TARGET_TIMES_N2: &str = "https://www.jma.go.jp/bosai/jmatile/data/nowc/targetTimes_N2.json";

/// タイムラインに含める実況コマ数（直近から）。
const NOWCAST_FRAMES: usize = 6;
/// タイムラインに含める予報コマ数（間引き後、最大1時間先）。
const FORECAST_FRAMES: usize = 7;

/// 雨雲タイルの取得ズーム上限。JMA ナウキャストは z10 までしか配信されないため、
/// app_zoom がこれを超えても z10 タイルを取得し、現在 bbox に対応する部分を
/// グリッドへ引き伸ばして（オーバーズーム）表示する。地図ズームとは独立。
pub const RADAR_MAX_ZOOM: u8 = 10;

/// 1 フレーム：時刻 + その時刻の雨量グリッド。
///
/// `grid` は `Box` でヒープに置く。非 embedded の `Grid` は約 256KB インラインなので、
/// 13 コマを値で `Vec` に持つとスタックに 3MB 載りかねない（前回の既知事項）。
pub struct RadarFrame {
    /// validtime を unix 秒へ変換した表示用時刻（UTC 基準の絶対秒）。
    pub valid_unix: u64,
    /// 予報フレームか（UI で実況と色分けする）。
    pub is_forecast: bool,
    pub grid: Box<Grid>,
}

pub struct JmaNowcast {
    client: reqwest::Client,
    /// 出力グリッドの解像度（セル数）。PC では 256 まで可。
    grid_w: u16,
    grid_h: u16,
    /// 雨雲タイルのメモリキャッシュ（**メモリのみ・ディスク永続化しない**）。
    /// URL に basetime/validtime を含むので、同一フレームの再取得だけ防ぐ
    /// （古いフレームを掴むことはない）。タイムライン往復で再取得が消える。
    cache: SharedCache,
}

impl JmaNowcast {
    pub fn new(client: reqwest::Client, grid_w: u16, grid_h: u16, cache: SharedCache) -> Self {
        Self {
            client,
            grid_w,
            grid_h,
            cache,
        }
    }

    /// 実況＋予報をまとめて時系列フレーム列（昇順）を取得する。
    ///
    /// - N1/N2 の targetTimes を取得 → validtime を unix へ → 昇順整列・間引き
    ///   （直近実況数コマ + 予報を 10 分間隔程度に）→ 各時刻でタイル取得・Grid 化。
    /// - 合計コマ数は `max_frames` 以内に収める。
    pub async fn fetch_radar_timeline(
        &self,
        bbox: GeoBBox,
        zoom: u8,
        max_frames: usize,
    ) -> Result<Vec<RadarFrame>> {
        // N1（実況）は必須。N2（予報）は取れなければ実況のみで続行。
        let n1: Vec<TargetTime> = self
            .client
            .get(TARGET_TIMES_N1)
            .send()
            .await?
            .json()
            .await?;
        let n2: Vec<TargetTime> = match self.client.get(TARGET_TIMES_N2).send().await {
            Ok(r) => r.json().await.unwrap_or_default(),
            Err(_) => Vec::new(),
        };

        let specs = select_frame_specs(&n1, &n2, max_frames);
        if specs.is_empty() {
            return Err(SourceError::NoTargetTime);
        }

        let mut frames: Vec<RadarFrame> = Vec::with_capacity(specs.len());
        for s in specs {
            // grid は Box<Grid>。Grid は約256KB インラインなので、値渡しや await
            // 跨ぎでスタックに載せると debug ビルドで多重コピーになりオーバーフローする
            // （tokio worker のスタックを溢れさせる）。常に Box 越しに扱う。
            let grid = self
                .fetch_frame_grid(bbox, zoom, &s.basetime, &s.validtime)
                .await?;
            frames.push(RadarFrame {
                valid_unix: s.valid_unix,
                is_forecast: s.is_forecast,
                grid,
            });
        }
        Ok(frames)
    }

    /// 指定 basetime/validtime の 1 コマぶんをタイル取得して Grid 化する。
    ///
    /// `fetch_radar`（最新実況1コマ）と `fetch_radar_timeline`（各コマ）の共通本体。
    /// 戻り値は `Box<Grid>`：256KB の `Grid` を値で持ち回らず、生成直後にヒープへ
    /// 載せ、以降は Box 越しに書き込む（スタックオーバーフロー対策）。
    async fn fetch_frame_grid(
        &self,
        bbox: GeoBBox,
        zoom: u8,
        basetime: &str,
        validtime: &str,
    ) -> Result<Box<Grid>> {
        // 雨雲は z10 を上限にクランプ（オーバーズーム）。タイル取得・タイル内
        // ピクセル→グローバルピクセルはこの取得ズーム(z)で一貫して計算する。
        // 一方、グリッドが覆う地理範囲は引数 bbox（＝app の現在 bbox）のまま。
        // 両者は「z でのグローバルピクセル座標」を介してつながり、現在 bbox に
        // 入る z10 ピクセルだけがグリッドへ引き伸ばされて入る。
        let zoom = zoom.min(RADAR_MAX_ZOOM);
        let (nw, se) = bbox_to_tile_range(&bbox, zoom);

        // 出力グリッドを 0 初期化し、即ヒープへ（Box）。以降 await を跨ぐローカルは
        // Box<Grid>（ポインタ）だけになり、256KB 値がスタック/future に載らない。
        let mut grid: Box<Grid> =
            Box::new(Grid::new_zeroed(self.grid_w, self.grid_h, GridKind::PrecipMmH, bbox)
                .ok_or(SourceError::GridTooLarge)?);

        // グリッドが覆うピクセル範囲（zoom basis）。BBox の四隅をグローバルピクセルへ。
        let (px_min, py_min) =
            lonlat_to_pixel(bbox.max_lat, bbox.min_lon, zoom, TILE_SIZE as f64); // 北西
        let (px_max, py_max) =
            lonlat_to_pixel(bbox.min_lat, bbox.max_lon, zoom, TILE_SIZE as f64); // 南東
        let span_x = (px_max - px_min).max(1.0);
        let span_y = (py_max - py_min).max(1.0);

        for ty in nw.y..=se.y {
            for tx in nw.x..=se.x {
                let url = format!(
                    "{base}/{bt}/none/{vt}/surf/hrpns/{z}/{x}/{y}.png",
                    base = NOWCAST_BASE,
                    bt = basetime,
                    vt = validtime,
                    z = zoom,
                    x = tx,
                    y = ty,
                );

                // キャッシュ優先で取得（同一フレームの再取得を防ぐ）。タイル欠損はスキップ。
                let bytes = match fetch_cached(&self.client, &self.cache, &url).await {
                    Some(b) => b,
                    None => continue,
                };

                // 透明 PNG（無降水域）も正常にデコードできる。
                let img = match image::load_from_memory(&bytes) {
                    Ok(i) => i,
                    Err(_) => continue,
                };

                // このタイルの左上に対応するグローバルピクセル原点。
                let tile_origin_px = tx as f64 * TILE_SIZE as f64;
                let tile_origin_py = ty as f64 * TILE_SIZE as f64;

                let (iw, ih) = img.dimensions();
                for iy in 0..ih {
                    for ix in 0..iw {
                        let p = img.get_pixel(ix, iy);
                        let [r, g, b, a] = p.0;
                        if a == 0 {
                            continue; // 透明＝降水なし
                        }
                        let mmh = jma_color_to_precip(r, g, b);
                        if mmh <= 0.0 {
                            continue;
                        }
                        // このピクセルのグローバル座標 → グリッドセルへ写像。
                        let gpx = tile_origin_px + ix as f64;
                        let gpy = tile_origin_py + iy as f64;
                        if gpx < px_min || gpx > px_max || gpy < py_min || gpy > py_max {
                            continue; // BBox 範囲外は無視
                        }
                        let u = (gpx - px_min) / span_x; // 0..1
                        let v = (gpy - py_min) / span_y; // 0..1
                        let cx = ((u * self.grid_w as f64) as u16).min(self.grid_w - 1);
                        let cy = ((v * self.grid_h as f64) as u16).min(self.grid_h - 1);
                        // 既存値より大きければ更新（セル内最大降水を代表値に）。
                        if let Some(cur) = grid.get(cx, cy) {
                            if mmh > cur {
                                grid.set(cx, cy, mmh);
                            }
                        }
                    }
                }
            }
        }

        Ok(grid)
    }
}

#[async_trait]
impl RadarProvider for JmaNowcast {
    /// 最新実況1コマだけを取得する（タイムライン化前との互換用）。
    async fn fetch_radar(&self, bbox: GeoBBox, zoom: u8) -> Result<Grid> {
        // N1 の先頭（最新）を使う。降順前提に依存しないよう、validtime 最大を選ぶ。
        let times: Vec<TargetTime> = self
            .client
            .get(TARGET_TIMES_N1)
            .send()
            .await?
            .json()
            .await?;
        let latest = times
            .iter()
            .max_by(|a, b| a.validtime.cmp(&b.validtime))
            .ok_or(SourceError::NoTargetTime)?;
        // trait は Grid を要求。Box から取り出す（256KB の移動は1回のみ）。
        let grid = self
            .fetch_frame_grid(bbox, zoom, &latest.basetime, &latest.validtime)
            .await?;
        Ok(*grid)
    }
}

#[derive(Deserialize, Clone)]
struct TargetTime {
    basetime: String,
    validtime: String,
}

/// 取得対象の 1 コマ仕様（取得前の時刻情報）。
#[derive(Clone, Debug, PartialEq)]
struct FrameSpec {
    basetime: String,
    validtime: String,
    valid_unix: u64,
    is_forecast: bool,
}

/// N1（実況）・N2（予報）の targetTimes から取得するコマを選ぶ純粋関数。
///
/// 順序の前提（N1=降順/N2=昇順 等）に依存せず、validtime を unix へ変換して
/// 昇順整列する。実況は直近 `NOWCAST_FRAMES` コマ、予報は 1 つ飛ばしに間引いて
/// `FORECAST_FRAMES` コマまで（実況の最新時刻より後のものだけ）。最後に
/// validtime で重複排除し、`max_frames` を超える分は古い方から落とす。
fn select_frame_specs(n1: &[TargetTime], n2: &[TargetTime], max_frames: usize) -> Vec<FrameSpec> {
    let to_spec = |t: &TargetTime, is_forecast: bool| -> Option<FrameSpec> {
        Some(FrameSpec {
            basetime: t.basetime.clone(),
            validtime: t.validtime.clone(),
            valid_unix: validtime_to_unix(&t.validtime)?,
            is_forecast,
        })
    };

    // 実況：昇順整列し、直近 NOWCAST_FRAMES コマ。
    let mut obs: Vec<FrameSpec> = n1.iter().filter_map(|t| to_spec(t, false)).collect();
    obs.sort_by_key(|s| s.valid_unix);
    if obs.len() > NOWCAST_FRAMES {
        obs.drain(0..obs.len() - NOWCAST_FRAMES);
    }
    let last_obs = obs.last().map(|s| s.valid_unix).unwrap_or(0);

    // 予報：昇順整列し、1つ飛ばしに間引いて FORECAST_FRAMES まで。
    // 実況の最新より後（未来）のものだけ採用し、実況との重複を避ける。
    let mut fc_all: Vec<FrameSpec> = n2.iter().filter_map(|t| to_spec(t, true)).collect();
    fc_all.sort_by_key(|s| s.valid_unix);
    let fc: Vec<FrameSpec> = fc_all
        .into_iter()
        .filter(|s| s.valid_unix > last_obs)
        .step_by(2)
        .take(FORECAST_FRAMES)
        .collect();

    // 結合 → validtime で重複排除（実況を優先）→ 昇順。
    let mut out: Vec<FrameSpec> = Vec::with_capacity(obs.len() + fc.len());
    out.extend(obs);
    for f in fc {
        if !out.iter().any(|s| s.validtime == f.validtime) {
            out.push(f);
        }
    }
    out.sort_by_key(|s| s.valid_unix);

    // max_frames を超えたら古い方（先頭）から落とす。
    if out.len() > max_frames {
        out.drain(0..out.len() - max_frames);
    }
    out
}

/// JMA validtime 文字列 "YYYYMMDDHHMMSS"(UTC) → unix 秒。
///
/// 暦日→経過日数は Howard Hinnant の `days_from_civil`（うるう年・世紀補正込み）。
/// 依存追加を避けるため自前計算（純粋・テスト可）。
fn validtime_to_unix(s: &str) -> Option<u64> {
    if s.len() < 14 || !s.bytes().take(14).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let num = |a: usize, b: usize| -> i64 { s[a..b].parse::<i64>().unwrap_or(0) };
    let (y, mo, d) = (num(0, 4), num(4, 6), num(6, 8));
    let (h, mi, se) = (num(8, 10), num(10, 12), num(12, 14));
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let days = days_from_civil(y, mo, d);
    let secs = days * 86_400 + h * 3_600 + mi * 60 + se;
    if secs < 0 {
        None
    } else {
        Some(secs as u64)
    }
}

/// 暦日 (y, m, d) → 1970-01-01 からの経過日数（負も可）。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    // m を 3..=14（3月始まり）へ寄せると2月末のうるう調整が末尾に来て扱いやすい。
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 }; // 3月=0 .. 2月=11
    let doy = (153 * mp + 2) / 5 + d - 1; // 年初(3月)からの日数 [0,365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // era 内通日 [0,146096]
    era * 146_097 + doe - 719_468 // 1970-01-01 を 0 にする補正
}

/// JMA 高解像度降水ナウキャストの配色 → 雨量(mm/h) 逆引き。
///
/// JMA タイルは決まった段階配色。各 RGB を最も近い階級の代表雨量へ写す。
/// 完全一致でなくとも、配色テーブルとの最小距離で階級を決める。
fn jma_color_to_precip(r: u8, g: u8, b: u8) -> f32 {
    // (R,G,B, 代表mm/h)。JMA の降水強度配色（公開タイルの実測色に基づく代表値）。
    const TABLE: [(u8, u8, u8, f32); 9] = [
        (0xF2, 0xF2, 0xFF, 0.5),   // ごく弱い  (0.1-1)
        (0xB2, 0xD2, 0xFF, 2.0),   // 弱い      (1-5)
        (0x66, 0xB3, 0xFF, 7.0),   // 並        (5-10)
        (0x33, 0x99, 0xFF, 15.0),  // やや強い  青系
        (0xFA, 0xF5, 0x00, 25.0),  // 強い      黄 (20-30)
        (0xFF, 0x99, 0x00, 40.0),  // 激しい    橙 (30-50)
        (0xFF, 0x40, 0x00, 65.0),  // 非常に激しい 赤 (50-80)
        (0xB2, 0x00, 0x4C, 100.0), // 猛烈      赤紫 (80+)
        (0xB4, 0x00, 0xB4, 100.0), // 紫系の猛烈
    ];

    let mut best = 0.0f32;
    let mut best_d = i32::MAX;
    for (tr, tg, tb, mmh) in TABLE.iter() {
        let dr = r as i32 - *tr as i32;
        let dg = g as i32 - *tg as i32;
        let db = b as i32 - *tb as i32;
        let d = dr * dr + dg * dg + db * db;
        if d < best_d {
            best_d = d;
            best = *mmh;
        }
    }
    // あまりに遠い色（地図の境界線など想定外）は降水なし扱い。
    if best_d > 12_000 {
        0.0
    } else {
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tt(validtime: &str) -> TargetTime {
        TargetTime {
            basetime: validtime.to_string(),
            validtime: validtime.to_string(),
        }
    }

    #[test]
    fn maps_known_colors() {
        let v = jma_color_to_precip(0xFA, 0xF5, 0x00);
        assert!(v >= 20.0, "yellow → {}", v);
        let v2 = jma_color_to_precip(0xF2, 0xF2, 0xFF);
        assert!(v2 > 0.0 && v2 < 5.0, "pale → {}", v2);
    }

    #[test]
    fn rejects_far_colors() {
        let v = jma_color_to_precip(0x00, 0x80, 0x00);
        assert_eq!(v, 0.0);
    }

    #[test]
    fn validtime_parses_known_epoch() {
        // 1970-01-01T00:00:00Z → 0。
        assert_eq!(validtime_to_unix("19700101000000"), Some(0));
        // 2000-01-01T00:00:00Z → 946684800（既知値）。
        assert_eq!(validtime_to_unix("20000101000000"), Some(946_684_800));
        // 2026-06-28T12:30:00Z → 既知値（`date -u -d ... +%s` で確認）。
        assert_eq!(validtime_to_unix("20260628123000"), Some(1_782_649_800));
        // 不正入力。
        assert_eq!(validtime_to_unix("2026"), None);
        assert_eq!(validtime_to_unix("2026XX28123000"), None);
        assert_eq!(validtime_to_unix("20261328000000"), None); // 13月
    }

    #[test]
    fn validtime_is_monotonic() {
        // 5分後は 300 秒後。
        let a = validtime_to_unix("20260628120000").unwrap();
        let b = validtime_to_unix("20260628120500").unwrap();
        assert_eq!(b - a, 300);
    }

    #[test]
    fn select_orders_and_caps() {
        // N1 を降順、N2 を昇順で渡しても、結果は昇順・max_frames 以内・
        // 実況→予報の並びになることを確認（順序前提に非依存）。
        let n1: Vec<TargetTime> = [
            "20260628120000",
            "20260628115500",
            "20260628115000",
            "20260628114500",
            "20260628114000",
            "20260628113500",
            "20260628113000",
            "20260628112500",
        ]
        .iter()
        .map(|s| tt(s))
        .collect();
        let n2: Vec<TargetTime> = [
            "20260628120500",
            "20260628121000",
            "20260628121500",
            "20260628122000",
            "20260628122500",
            "20260628123000",
        ]
        .iter()
        .map(|s| tt(s))
        .collect();

        let specs = select_frame_specs(&n1, &n2, 13);
        assert!(!specs.is_empty());
        assert!(specs.len() <= 13);
        // 昇順。
        for w in specs.windows(2) {
            assert!(w[0].valid_unix <= w[1].valid_unix, "not ascending");
        }
        // 実況は最大 NOWCAST_FRAMES コマ。
        let obs = specs.iter().filter(|s| !s.is_forecast).count();
        assert!(obs <= NOWCAST_FRAMES, "obs={obs}");
        // 予報は実況の最後より後。
        let last_obs = specs
            .iter()
            .filter(|s| !s.is_forecast)
            .map(|s| s.valid_unix)
            .max()
            .unwrap();
        for f in specs.iter().filter(|s| s.is_forecast) {
            assert!(f.valid_unix > last_obs, "forecast not after obs");
        }
    }

    #[test]
    fn select_survives_no_forecast() {
        let n1: Vec<TargetTime> = ["20260628120000", "20260628115500"]
            .iter()
            .map(|s| tt(s))
            .collect();
        let specs = select_frame_specs(&n1, &[], 13);
        assert_eq!(specs.len(), 2);
        assert!(specs.iter().all(|s| !s.is_forecast));
    }
}
