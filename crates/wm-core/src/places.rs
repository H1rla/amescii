//! 内蔵の主要都市テーブルと、画面内ラベルの配置算出。
//!
//! 純粋ロジック（no_std）。座標→画面セルは `render::lines::lonlat_to_cell`
//! をそのまま使う。これにより内蔵ラベルが地図線・雨雲・日本語地名と
//! **同じ投影**で並び、ズレない。表記は英語（ローマ字）で統一する。
//!
//! 責務境界：wm-core は「どの名前をどのセルに置くか」(`PlaceLabel`) までを返す。
//! 文字をどう端末へ出すかは wm-tui の責務。

use crate::geo::GeoBBox;
use crate::render::lines::lonlat_to_cell;
use heapless::Vec;

/// 内蔵都市の 1 件。
pub struct City {
    /// 英語名（ローマ字）。例 "Tokyo"。
    pub name: &'static str,
    pub lat: f64,
    pub lon: f64,
    /// 重要度ランク（小さいほど重要＝低ズームでも出す）。
    /// 0=三大都市級, 1=政令市/地方中枢, 2=県庁所在地クラス。
    pub rank: u8,
}

/// 内蔵テーブル（県庁所在地＋三大都市を網羅）。重要度ランク付き。
///
/// 緯度経度は各市役所/中心部のおおよその値。広域表示で「どこに何市があるか」が
/// 分かれば十分なので精密でなくてよい（投影は雨雲と一致させてある）。
pub static CITIES: &[City] = &[
    // rank 0：三大都市。
    City { name: "Tokyo", lat: 35.69, lon: 139.69, rank: 0 },
    City { name: "Osaka", lat: 34.69, lon: 135.50, rank: 0 },
    City { name: "Nagoya", lat: 35.18, lon: 136.91, rank: 0 },
    // rank 1：政令市・地方中枢。
    City { name: "Sapporo", lat: 43.06, lon: 141.35, rank: 1 },
    City { name: "Sendai", lat: 38.27, lon: 140.87, rank: 1 },
    City { name: "Yokohama", lat: 35.44, lon: 139.64, rank: 1 },
    City { name: "Kyoto", lat: 35.01, lon: 135.77, rank: 1 },
    City { name: "Kobe", lat: 34.69, lon: 135.20, rank: 1 },
    City { name: "Hiroshima", lat: 34.39, lon: 132.46, rank: 1 },
    City { name: "Fukuoka", lat: 33.59, lon: 130.40, rank: 1 },
    City { name: "Naha", lat: 26.21, lon: 127.68, rank: 1 },
    // rank 2：県庁所在地クラス。
    City { name: "Aomori", lat: 40.82, lon: 140.74, rank: 2 },
    City { name: "Morioka", lat: 39.70, lon: 141.15, rank: 2 },
    City { name: "Akita", lat: 39.72, lon: 140.10, rank: 2 },
    City { name: "Yamagata", lat: 38.24, lon: 140.36, rank: 2 },
    City { name: "Fukushima", lat: 37.75, lon: 140.47, rank: 2 },
    City { name: "Mito", lat: 36.37, lon: 140.47, rank: 2 },
    City { name: "Utsunomiya", lat: 36.57, lon: 139.88, rank: 2 },
    City { name: "Maebashi", lat: 36.39, lon: 139.06, rank: 2 },
    City { name: "Saitama", lat: 35.86, lon: 139.65, rank: 2 },
    City { name: "Chiba", lat: 35.61, lon: 140.12, rank: 2 },
    City { name: "Niigata", lat: 37.90, lon: 139.02, rank: 2 },
    City { name: "Toyama", lat: 36.70, lon: 137.21, rank: 2 },
    City { name: "Kanazawa", lat: 36.56, lon: 136.66, rank: 2 },
    City { name: "Fukui", lat: 36.06, lon: 136.22, rank: 2 },
    City { name: "Kofu", lat: 35.66, lon: 138.57, rank: 2 },
    City { name: "Nagano", lat: 36.65, lon: 138.18, rank: 2 },
    City { name: "Gifu", lat: 35.42, lon: 136.76, rank: 2 },
    City { name: "Shizuoka", lat: 34.98, lon: 138.38, rank: 2 },
    City { name: "Tsu", lat: 34.73, lon: 136.51, rank: 2 },
    City { name: "Otsu", lat: 35.00, lon: 135.87, rank: 2 },
    City { name: "Nara", lat: 34.69, lon: 135.80, rank: 2 },
    City { name: "Wakayama", lat: 34.23, lon: 135.17, rank: 2 },
    City { name: "Tottori", lat: 35.50, lon: 134.24, rank: 2 },
    City { name: "Matsue", lat: 35.47, lon: 133.05, rank: 2 },
    City { name: "Okayama", lat: 34.66, lon: 133.93, rank: 2 },
    City { name: "Yamaguchi", lat: 34.19, lon: 131.47, rank: 2 },
    City { name: "Tokushima", lat: 34.07, lon: 134.55, rank: 2 },
    City { name: "Takamatsu", lat: 34.34, lon: 134.04, rank: 2 },
    City { name: "Matsuyama", lat: 33.84, lon: 132.77, rank: 2 },
    City { name: "Kochi", lat: 33.56, lon: 133.53, rank: 2 },
    City { name: "Saga", lat: 33.25, lon: 130.30, rank: 2 },
    City { name: "Nagasaki", lat: 32.74, lon: 129.87, rank: 2 },
    City { name: "Kumamoto", lat: 32.79, lon: 130.74, rank: 2 },
    City { name: "Oita", lat: 33.24, lon: 131.61, rank: 2 },
    City { name: "Miyazaki", lat: 31.91, lon: 131.42, rank: 2 },
    City { name: "Kagoshima", lat: 31.56, lon: 130.56, rank: 2 },
];

/// 画面に描くべきラベル 1 件（出力中間表現）。
#[derive(Clone, Copy)]
pub struct PlaceLabel {
    /// テキスト開始セル（マーカーの右）。
    pub col: u16,
    pub row: u16,
    /// マーカー位置（`col` の左側）。
    pub marker_col: u16,
    pub name: &'static str,
}

pub const MAX_LABELS: usize = 64;
pub type LabelVec = Vec<PlaceLabel, MAX_LABELS>;

/// マーカーとテキストの間隔（"· Tokyo" の "· " ぶん）。
const MARKER_GAP: u16 = 2;

/// ズーム→表示する最大 rank。拡大ほど多く出す。
fn zoom_to_max_rank(zoom: u8) -> u8 {
    match zoom {
        0..=5 => 0, // 広域：三大都市のみ
        6..=7 => 1, // 政令市・地方中枢まで
        _ => 2,     // zoom>=8：県庁所在地クラスまで
    }
}

/// BBox・ズーム・画面セル数から、内蔵都市のうち画面内かつそのズームで表示すべき
/// ものをラベル配置して返す。
///
/// - ズーム閾値：`rank <= zoom_to_max_rank(zoom)` の都市のみ。
/// - 画面内判定：lat/lon が bbox 内か（投影後 [0,1] 内か）。
/// - 配置：`project_norm`（lines/radar と同式）で正規化→セル。
/// - 重なり回避：rank 昇順に貪欲配置し、同じ行で水平に重なる後続（低ランク）は捨てる。
pub fn layout_city_labels(bbox: &GeoBBox, zoom: u8, cols: u16, rows: u16) -> LabelVec {
    let mut out: LabelVec = Vec::new();
    if cols == 0 || rows == 0 {
        return out;
    }
    let max_rank = zoom_to_max_rank(zoom);

    // rank 昇順（重要→些末）に走査し、先に置いた重要ラベルを優先する。
    for want in 0..=max_rank {
        for c in CITIES.iter().filter(|c| c.rank == want) {
            // 画面セルへ投影（lines/radar/JP地名と同一の lonlat_to_cell）。
            // 画面外（範囲外）は None で除外＝投影一致を1関数に集約。
            let (marker_col, row) = match lonlat_to_cell(bbox, c.lat, c.lon, cols, rows) {
                Some(cr) => cr,
                None => continue,
            };
            // 重なり回避：同じ行で水平範囲が被る既配置があれば捨てる。
            if overlaps(&out, marker_col, row, label_width(c.name)) {
                continue;
            }
            let col = marker_col.saturating_add(MARKER_GAP);
            if out
                .push(PlaceLabel {
                    col,
                    row,
                    marker_col,
                    name: c.name,
                })
                .is_err()
            {
                return out; // 容量到達
            }
        }
    }
    out
}

/// ラベルの水平占有セル数（"· Name" のマーカー＋空白＋名前）。
#[inline]
fn label_width(name: &str) -> u16 {
    MARKER_GAP + name.len() as u16
}

/// 同じ行で水平範囲 [marker_col, marker_col+width) が既配置と被るか。
fn overlaps(placed: &[PlaceLabel], marker_col: u16, row: u16, width: u16) -> bool {
    let a0 = marker_col;
    let a1 = marker_col.saturating_add(width);
    placed.iter().any(|p| {
        if p.row != row {
            return false;
        }
        let b0 = p.marker_col;
        let b1 = p.marker_col.saturating_add(label_width(p.name));
        a0 < b1 && b0 < a1 // 区間重なり判定
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Grid, GridKind};
    use crate::render::braille::quantize;
    use crate::render::lines::project_norm; // 投影一致テスト用

    fn find<'a>(labels: &'a LabelVec, name: &str) -> Option<&'a PlaceLabel> {
        labels.iter().find(|l| l.name == name)
    }

    #[test]
    fn tokyo_appears_zoom8() {
        // 東京中心の BBox、zoom8。"Tokyo" が画面内に出る。
        let bbox = GeoBBox::new(35.0, 139.0, 36.4, 140.4);
        let labels = layout_city_labels(&bbox, 8, 60, 30);
        let tokyo = find(&labels, "Tokyo").expect("Tokyo should appear");
        assert!(tokyo.marker_col < 60 && tokyo.row < 30);
        // 県庁クラス（Chiba/Saitama）も zoom8 では出る。
        assert!(find(&labels, "Chiba").is_some() || find(&labels, "Saitama").is_some());
    }

    #[test]
    fn zoom3_only_rank0() {
        // 日本全域・zoom3：rank0（三大都市）のみ。県庁クラスは出ない。
        let bbox = GeoBBox::japan();
        let labels = layout_city_labels(&bbox, 3, 120, 50);
        assert!(find(&labels, "Tokyo").is_some());
        assert!(find(&labels, "Osaka").is_some());
        // rank2 の金沢は zoom3 では出ない。
        assert!(find(&labels, "Kanazawa").is_none());
        // 全件 rank0。
        for l in labels.iter() {
            let c = CITIES.iter().find(|c| c.name == l.name).unwrap();
            assert_eq!(c.rank, 0, "{} should be rank0", l.name);
        }
    }

    #[test]
    fn offscreen_city_excluded() {
        // 東京周辺の狭い BBox に大阪は入らない。
        let bbox = GeoBBox::new(35.4, 139.4, 35.9, 139.9);
        let labels = layout_city_labels(&bbox, 9, 80, 40);
        assert!(find(&labels, "Osaka").is_none());
    }

    #[test]
    fn higher_rank_wins_on_overlap() {
        // 近接する2都市が同じセル帯に来る極小画面では、rank の高い方が残る。
        // 東京(rank0)と横浜(rank1)を含む BBox を 6x3 の極小セルに描くと衝突する。
        let bbox = GeoBBox::new(35.3, 139.5, 35.8, 139.8);
        let labels = layout_city_labels(&bbox, 8, 6, 3);
        // 衝突時、Tokyo(rank0) が優先。Yokohama が両方残ることはない。
        if find(&labels, "Tokyo").is_some() && find(&labels, "Yokohama").is_some() {
            // 別行に分かれて両立した場合のみ許容（同行衝突なら片方）。
            let t = find(&labels, "Tokyo").unwrap();
            let y = find(&labels, "Yokohama").unwrap();
            assert_ne!(t.row, y.row, "同行で衝突しているのに両方残っている");
        }
    }

    #[test]
    fn projection_matches_rain() {
        // 投影一致回帰：内蔵ラベルのマーカーセルが、同じ緯度経度に置いた
        // 雨雲ドットの quantize セルと一致する（lines の投影一致テストに準ずる）。
        let bbox = GeoBBox::new(35.0, 139.0, 36.4, 140.4);
        let cols = 60u16;
        let rows = 30u16;
        let labels = layout_city_labels(&bbox, 8, cols, rows);
        let tokyo = find(&labels, "Tokyo").unwrap();

        // 東京の (lat,lon) を雨雲グリッド（cols*2 × rows*4 ドット）に1点だけ立てる。
        let (u, v) = project_norm(&bbox, 35.69, 139.69);
        let dotx = (u * cols as f64 * 2.0) as u16;
        let doty = (v * rows as f64 * 4.0) as u16;
        let mut grid =
            Grid::new_zeroed(cols * 2, rows * 4, GridKind::PrecipMmH, bbox).unwrap();
        grid.set(dotx, doty, 50.0);
        let rain = quantize(&grid, cols, rows);
        assert_eq!(rain.len(), 1);
        // ラベルのマーカーセル == 雨雲セル。
        assert_eq!((tokyo.marker_col, tokyo.row), (rain[0].col, rain[0].row));
    }
}
