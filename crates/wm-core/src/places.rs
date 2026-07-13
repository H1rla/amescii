//! 内蔵の主要都市テーブルと、画面内ラベルの配置算出。
//!
//! 純粋ロジック（no_std）。座標→画面セルは `render::lines::lonlat_to_cell`
//! をそのまま使う。これにより内蔵ラベルが地図線・雨雲・日本語地名と
//! **同じ投影**で並び、ズレない。画面に出すのは日本語名（`name_ja`）で、
//! 拡大時の地理院 label 由来の日本語地名と表記が揃う。
//!
//! 責務境界：wm-core は「どの名前をどのセルに置くか」(`PlaceLabel`) までを返す。
//! 文字をどう端末へ出すかは wm-tui の責務。

use crate::geo::GeoBBox;
use crate::render::lines::lonlat_to_cell;
use heapless::Vec;
use unicode_width::UnicodeWidthStr;

/// 内蔵都市の 1 件。
pub struct City {
    /// 英語名（ローマ字）。例 "Tokyo"。同定・ログ用（画面表示には使わない）。
    pub name: &'static str,
    /// 日本語名。例 "東京"。画面に出すのはこちら。
    pub name_ja: &'static str,
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
    City { name: "Tokyo", name_ja: "東京", lat: 35.69, lon: 139.69, rank: 0 },
    City { name: "Osaka", name_ja: "大阪", lat: 34.69, lon: 135.50, rank: 0 },
    City { name: "Nagoya", name_ja: "名古屋", lat: 35.18, lon: 136.91, rank: 0 },
    // rank 1：政令市・地方中枢。
    City { name: "Sapporo", name_ja: "札幌", lat: 43.06, lon: 141.35, rank: 1 },
    City { name: "Sendai", name_ja: "仙台", lat: 38.27, lon: 140.87, rank: 1 },
    City { name: "Yokohama", name_ja: "横浜", lat: 35.44, lon: 139.64, rank: 1 },
    City { name: "Kyoto", name_ja: "京都", lat: 35.01, lon: 135.77, rank: 1 },
    City { name: "Kobe", name_ja: "神戸", lat: 34.69, lon: 135.20, rank: 1 },
    City { name: "Hiroshima", name_ja: "広島", lat: 34.39, lon: 132.46, rank: 1 },
    City { name: "Fukuoka", name_ja: "福岡", lat: 33.59, lon: 130.40, rank: 1 },
    City { name: "Naha", name_ja: "那覇", lat: 26.21, lon: 127.68, rank: 1 },
    // rank 2：県庁所在地クラス。
    City { name: "Aomori", name_ja: "青森", lat: 40.82, lon: 140.74, rank: 2 },
    City { name: "Morioka", name_ja: "盛岡", lat: 39.70, lon: 141.15, rank: 2 },
    City { name: "Akita", name_ja: "秋田", lat: 39.72, lon: 140.10, rank: 2 },
    City { name: "Yamagata", name_ja: "山形", lat: 38.24, lon: 140.36, rank: 2 },
    City { name: "Fukushima", name_ja: "福島", lat: 37.75, lon: 140.47, rank: 2 },
    City { name: "Mito", name_ja: "水戸", lat: 36.37, lon: 140.47, rank: 2 },
    City { name: "Utsunomiya", name_ja: "宇都宮", lat: 36.57, lon: 139.88, rank: 2 },
    City { name: "Maebashi", name_ja: "前橋", lat: 36.39, lon: 139.06, rank: 2 },
    City { name: "Saitama", name_ja: "さいたま", lat: 35.86, lon: 139.65, rank: 2 },
    City { name: "Chiba", name_ja: "千葉", lat: 35.61, lon: 140.12, rank: 2 },
    City { name: "Niigata", name_ja: "新潟", lat: 37.90, lon: 139.02, rank: 2 },
    City { name: "Toyama", name_ja: "富山", lat: 36.70, lon: 137.21, rank: 2 },
    City { name: "Kanazawa", name_ja: "金沢", lat: 36.56, lon: 136.66, rank: 2 },
    City { name: "Fukui", name_ja: "福井", lat: 36.06, lon: 136.22, rank: 2 },
    City { name: "Kofu", name_ja: "甲府", lat: 35.66, lon: 138.57, rank: 2 },
    City { name: "Nagano", name_ja: "長野", lat: 36.65, lon: 138.18, rank: 2 },
    City { name: "Gifu", name_ja: "岐阜", lat: 35.42, lon: 136.76, rank: 2 },
    City { name: "Shizuoka", name_ja: "静岡", lat: 34.98, lon: 138.38, rank: 2 },
    City { name: "Tsu", name_ja: "津", lat: 34.73, lon: 136.51, rank: 2 },
    City { name: "Otsu", name_ja: "大津", lat: 35.00, lon: 135.87, rank: 2 },
    City { name: "Nara", name_ja: "奈良", lat: 34.69, lon: 135.80, rank: 2 },
    City { name: "Wakayama", name_ja: "和歌山", lat: 34.23, lon: 135.17, rank: 2 },
    City { name: "Tottori", name_ja: "鳥取", lat: 35.50, lon: 134.24, rank: 2 },
    City { name: "Matsue", name_ja: "松江", lat: 35.47, lon: 133.05, rank: 2 },
    City { name: "Okayama", name_ja: "岡山", lat: 34.66, lon: 133.93, rank: 2 },
    City { name: "Yamaguchi", name_ja: "山口", lat: 34.19, lon: 131.47, rank: 2 },
    City { name: "Tokushima", name_ja: "徳島", lat: 34.07, lon: 134.55, rank: 2 },
    City { name: "Takamatsu", name_ja: "高松", lat: 34.34, lon: 134.04, rank: 2 },
    City { name: "Matsuyama", name_ja: "松山", lat: 33.84, lon: 132.77, rank: 2 },
    City { name: "Kochi", name_ja: "高知", lat: 33.56, lon: 133.53, rank: 2 },
    City { name: "Saga", name_ja: "佐賀", lat: 33.25, lon: 130.30, rank: 2 },
    City { name: "Nagasaki", name_ja: "長崎", lat: 32.74, lon: 129.87, rank: 2 },
    City { name: "Kumamoto", name_ja: "熊本", lat: 32.79, lon: 130.74, rank: 2 },
    City { name: "Oita", name_ja: "大分", lat: 33.24, lon: 131.61, rank: 2 },
    City { name: "Miyazaki", name_ja: "宮崎", lat: 31.91, lon: 131.42, rank: 2 },
    City { name: "Kagoshima", name_ja: "鹿児島", lat: 31.56, lon: 130.56, rank: 2 },
];

/// 画面に描くべきラベル 1 件（出力中間表現）。
#[derive(Clone, Copy)]
pub struct PlaceLabel {
    /// テキスト開始セル（マーカーの右）。
    pub col: u16,
    pub row: u16,
    /// マーカー位置（`col` の左側）。
    pub marker_col: u16,
    /// 画面に出す表示名（日本語＝`City::name_ja`）。全角は端末で 2 セル幅。
    pub name: &'static str,
}

pub const MAX_LABELS: usize = 64;
pub type LabelVec = Vec<PlaceLabel, MAX_LABELS>;

/// マーカーとテキストの間隔（"· Tokyo" の "· " ぶん）。
const MARKER_GAP: u16 = 2;

/// ズーム→表示する最大 rank。拡大ほど多く出す。
fn zoom_to_max_rank(zoom: u8) -> u8 {
    match zoom {
        0..=3 => 0, // 広域：三大都市のみ
        4..=5 => 1, // 政令市・地方中枢まで（11都市）
        _ => 2,     // zoom>=6：全県庁所在地
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
            // 幅は実際に描く日本語名で測る（全角は 2 セル）。
            if overlaps(&out, marker_col, row, label_width(c.name_ja)) {
                continue;
            }
            let col = marker_col.saturating_add(MARKER_GAP);
            if out
                .push(PlaceLabel {
                    col,
                    row,
                    marker_col,
                    name: c.name_ja,
                })
                .is_err()
            {
                return out; // 容量到達
            }
        }
    }
    out
}

/// ラベルの水平占有セル数（"· 名前" のマーカー＋空白＋名前）。
///
/// バイト数ではなく端末表示幅で測る（"東京" は 6 バイトだが 4 セル）。
#[inline]
fn label_width(name: &str) -> u16 {
    MARKER_GAP + UnicodeWidthStr::width(name) as u16
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

    /// `PlaceLabel.name` は表示名＝日本語（`City::name_ja`）なので日本語で引く。
    fn find<'a>(labels: &'a LabelVec, name_ja: &str) -> Option<&'a PlaceLabel> {
        labels.iter().find(|l| l.name == name_ja)
    }

    #[test]
    fn tokyo_appears_zoom8() {
        // 東京中心の BBox、zoom8。"東京" が画面内に出る。
        let bbox = GeoBBox::new(35.0, 139.0, 36.4, 140.4);
        let labels = layout_city_labels(&bbox, 8, 60, 30);
        let tokyo = find(&labels, "東京").expect("東京 should appear");
        assert!(tokyo.marker_col < 60 && tokyo.row < 30);
        // 県庁クラス（千葉/さいたま）も zoom8 では出る。
        assert!(find(&labels, "千葉").is_some() || find(&labels, "さいたま").is_some());
    }

    #[test]
    fn zoom3_only_rank0() {
        // 日本全域・zoom3：rank0（三大都市）のみ。県庁クラスは出ない。
        let bbox = GeoBBox::japan();
        let labels = layout_city_labels(&bbox, 3, 120, 50);
        assert!(find(&labels, "東京").is_some());
        assert!(find(&labels, "大阪").is_some());
        // rank2 の金沢は zoom3 では出ない。
        assert!(find(&labels, "金沢").is_none());
        // 全件 rank0。
        for l in labels.iter() {
            let c = CITIES.iter().find(|c| c.name_ja == l.name).unwrap();
            assert_eq!(c.rank, 0, "{} should be rank0", l.name);
        }
    }

    #[test]
    fn zoom4_shows_rank1_but_not_rank2() {
        // 新閾値：zoom4 は rank1（政令市・地方中枢）まで。県庁クラスは出さない。
        let bbox = GeoBBox::japan();
        let labels = layout_city_labels(&bbox, 4, 120, 50);
        assert!(find(&labels, "札幌").is_some(), "rank1 の札幌は zoom4 で出る");
        assert!(find(&labels, "金沢").is_none(), "rank2 の金沢は zoom4 では出ない");
        for l in labels.iter() {
            let c = CITIES.iter().find(|c| c.name_ja == l.name).unwrap();
            assert!(c.rank <= 1, "{} should be rank<=1", l.name);
        }
    }

    #[test]
    fn zoom6_shows_rank2() {
        // 新閾値：zoom6 から県庁所在地クラス（rank2）が出る。
        let bbox = GeoBBox::new(36.0, 136.0, 37.2, 137.4); // 金沢周辺
        let labels = layout_city_labels(&bbox, 6, 80, 40);
        assert!(find(&labels, "金沢").is_some());
    }

    #[test]
    fn offscreen_city_excluded() {
        // 東京周辺の狭い BBox に大阪は入らない。
        let bbox = GeoBBox::new(35.4, 139.4, 35.9, 139.9);
        let labels = layout_city_labels(&bbox, 9, 80, 40);
        assert!(find(&labels, "大阪").is_none());
    }

    #[test]
    fn label_width_counts_display_cells() {
        // 全角は 1 文字 2 セル（バイト数ではない）。"· 東京" = 2 + 4 = 6 セル。
        assert_eq!(label_width("東京"), 6);
        assert_eq!(label_width("さいたま"), 10);
    }

    #[test]
    fn higher_rank_wins_on_overlap() {
        // 近接する2都市が同じセル帯に来る極小画面では、rank の高い方が残る。
        // 東京(rank0)と横浜(rank1)を含む BBox を 6x3 の極小セルに描くと衝突する。
        let bbox = GeoBBox::new(35.3, 139.5, 35.8, 139.8);
        let labels = layout_city_labels(&bbox, 8, 6, 3);
        // 衝突時、東京(rank0) が優先。横浜が両方残ることはない。
        if find(&labels, "東京").is_some() && find(&labels, "横浜").is_some() {
            // 別行に分かれて両立した場合のみ許容（同行衝突なら片方）。
            let t = find(&labels, "東京").unwrap();
            let y = find(&labels, "横浜").unwrap();
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
        let tokyo = find(&labels, "東京").unwrap();

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
