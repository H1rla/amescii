//! 地理院ベクトルタイル(MVT/pbf)を取得し、海岸線/行政界/道路/鉄道を
//! 緯度経度の点列へデコードする。wm-core の `rasterize_lines` に渡す材料を作る。
//!
//! 出典：「国土地理院ベクトルタイル提供実験」(experimental_bvmap)。APIキー不要。
//! 座標系は EPSG:3857（Web Mercator）で JMA ナウキャストと同一。同じ (z,x,y) の
//! タイルは同じ地理範囲を指すので、雨雲と同じ zoom で引けば位置が揃う。
//!
//! MVT のデコードは geozero（prost 生成の MVT 構造体を生で公開）に任せ、
//! ジオメトリのコマンド列（MoveTo/LineTo/ClosePath + zigzag）だけ MVT 仕様
//! どおりに自前展開する。自前 protobuf 実装はしない（罠が多く本題から逸れる）。

use crate::cache::{fetch_cached_with_disk, SharedCache};
use crate::error::Result;
use geozero::mvt::tile::{Feature, Layer};
use geozero::mvt::{Message, Tile};
use wm_core::geo::{bbox_to_tile_range, tile_frac_to_lonlat, GeoBBox};

const BVMAP_BASE: &str = "https://cyberjapandata.gsi.go.jp/xyz/experimental_bvmap";
/// 道路・鉄道を描画する最小ズーム。これ未満（広域）はフィーチャが密すぎるので省く。
const ROADS_MIN_ZOOM: u8 = 8;

/// 1本の線（緯度経度 (lat, lon)）+ 種別。
pub struct BaseLine {
    pub points: Vec<(f64, f64)>,
    pub kind: BaseLineKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BaseLineKind {
    Coastline,
    Boundary,
    Road,
    Railway,
}

pub struct BaseMapProvider {
    client: reqwest::Client,
    /// 地図ベクトルタイルのメモリキャッシュ（共有）。fetch_lines と
    /// fetch_labels_ja は同じ (z,x,y).pbf URL を引くので、共有すると相互に効く。
    cache: SharedCache,
}

impl BaseMapProvider {
    pub fn new(client: reqwest::Client, cache: SharedCache) -> Self {
        Self { client, cache }
    }

    /// BBox を覆うタイルを取得・デコードし、線群を返す。
    /// `coastline` が無いズーム帯（z=8..10 等）では `waterarea` ポリゴン外周を
    /// `Coastline` として返す（陸海境界の代わり）。
    pub async fn fetch_lines(&self, bbox: GeoBBox, zoom: u8) -> Result<Vec<BaseLine>> {
        let (nw, se) = bbox_to_tile_range(&bbox, zoom);
        let want_roads = zoom >= ROADS_MIN_ZOOM;

        // タイル取得（ネットワーク）は並列化する。デコード・合成は取得後に逐次。
        let mut futs = Vec::new();
        for ty in nw.y..=se.y {
            for tx in nw.x..=se.x {
                let url = format!("{BVMAP_BASE}/{zoom}/{tx}/{ty}.pbf");
                let client = self.client.clone();
                let cache = self.cache.clone();
                futs.push(async move {
                    // メモリ→ディスク→HTTP。地図タイルはほぼ不変なのでディスクへ永続化。
                    // 404/空タイルはスキップ（提供範囲外や海上など）。
                    let bytes = fetch_cached_with_disk(&client, &cache, &url).await;
                    (tx, ty, bytes)
                });
            }
        }
        let results = futures::future::join_all(futs).await;

        let mut out: Vec<BaseLine> = Vec::new();
        for (tx, ty, bytes) in results {
            let bytes = match bytes {
                Some(b) if !b.is_empty() => b,
                _ => continue,
            };
            let tile = match Tile::decode(&bytes[..]) {
                Ok(t) => t,
                Err(_) => continue, // 壊れたタイルはスキップ
            };
            decode_tile(&tile, zoom, tx, ty, want_roads, &mut out);
        }

        Ok(out)
    }
}

// ─────────────────── 拡大時の日本語地名（label レイヤ） ───────────────────
//
// 実タイル確認結果（z=11/13/15, 東京駅。experimental_bvmap, 2026-06 時点）:
//   label レイヤは全ズーム帯に存在。1タイルあたり件数は z11=85, z13=30, z15=36。
//   テキスト系プロパティの充足率：
//     knj(漢字)   = 85/85, 30/30, 36/36  ← 100% 充足。最も信頼できる。
//     annoChar    = 76/85, 10/30,  0/36  ← 簡潔な表示注記。高ズームで消える。
//     kana        = 67/85,  9/30, 14/36  ← 部分的。
//   ftCode は全件あるが種別判別の有用性は不明（前回調査で一律 100 のことも）。
// → 表示テキストは **annoChar 優先・無ければ knj** にフォールバックする。
//   これで z11–16 全帯で日本語地名が必ず出る（"東京駅"/"皇居"/"丸の内二丁目" 等）。
//   ※ローマ字(name)は当レイヤに存在しない（前回タスクで確認済み）。広域は内蔵英語都市。

/// 拡大時の地名ラベル（日本語）。
pub struct NameLabelJa {
    pub lat: f64,
    pub lon: f64,
    /// 表示テキスト（annoChar 優先、無ければ knj）。日本語注記文字列。
    pub text: String,
}

impl BaseMapProvider {
    /// `label` レイヤをデコードし、日本語注記つきの点を返す。
    ///
    /// 取得タイルは地図線と同じ z/x/y（現在 app_zoom、最大 16）。高ズーム専用
    /// （呼び出しは zoom>=11 のときのみ）。空テキスト・取得不能はスキップ。
    pub async fn fetch_labels_ja(&self, bbox: GeoBBox, zoom: u8) -> Result<Vec<NameLabelJa>> {
        let (nw, se) = bbox_to_tile_range(&bbox, zoom);

        // タイル取得は並列化。デコードは取得後に逐次（fetch_lines と同構造）。
        let mut futs = Vec::new();
        for ty in nw.y..=se.y {
            for tx in nw.x..=se.x {
                let url = format!("{BVMAP_BASE}/{zoom}/{tx}/{ty}.pbf");
                let client = self.client.clone();
                let cache = self.cache.clone();
                futs.push(async move {
                    // fetch_lines と同一 URL・同一ディスクキャッシュを共有（相互ヒット）。
                    let bytes = fetch_cached_with_disk(&client, &cache, &url).await;
                    (tx, ty, bytes)
                });
            }
        }
        let results = futures::future::join_all(futs).await;

        let mut out: Vec<NameLabelJa> = Vec::new();
        for (tx, ty, bytes) in results {
            let bytes = match bytes {
                Some(b) if !b.is_empty() => b,
                _ => continue,
            };
            let tile = match Tile::decode(&bytes[..]) {
                Ok(t) => t,
                Err(_) => continue,
            };
            decode_labels_ja(&tile, zoom, tx, ty, &mut out);
        }
        Ok(out)
    }
}

/// 1タイルの `label` レイヤから日本語注記つきの点を取り出す（純粋・テスト可）。
fn decode_labels_ja(tile: &Tile, z: u8, tx: u32, ty: u32, out: &mut Vec<NameLabelJa>) {
    for layer in &tile.layers {
        if layer.name != "label" {
            continue;
        }
        let extent = layer.extent.unwrap_or(4096) as f64;
        for f in &layer.features {
            // 表示テキスト：annoChar 優先、無ければ knj（漢字・全件充足）。
            let text = match prop_str(layer, f, "annoChar")
                .or_else(|| prop_str(layer, f, "knj"))
            {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => continue,
            };
            // 注記は Point。最初のパスの最初の点を代表座標にする。
            let (lx, ly) = match decode_geom(&f.geometry)
                .first()
                .and_then(|p| p.first())
                .copied()
            {
                Some(p) => p,
                None => continue,
            };
            let xf = tx as f64 + lx as f64 / extent;
            let yf = ty as f64 + ly as f64 / extent;
            let (lat, lon) = tile_frac_to_lonlat(z, xf, yf);
            out.push(NameLabelJa { lat, lon, text });
        }
    }
}

/// フィーチャの properties から key の文字列値を引く。
///
/// MVT の `tags` は [key_index, value_index, ...] のフラット列。layer.keys /
/// layer.values を索く。
fn prop_str<'a>(layer: &'a Layer, f: &Feature, key: &str) -> Option<&'a str> {
    let ki = layer.keys.iter().position(|s| s == key)?;
    for pair in f.tags.chunks_exact(2) {
        if pair[0] as usize == ki {
            return layer.values.get(pair[1] as usize)?.string_value.as_deref();
        }
    }
    None
}

/// 1タイルから対象レイヤの線・ポリゴン外周を緯度経度の点列へ。
fn decode_tile(tile: &Tile, z: u8, tx: u32, ty: u32, want_roads: bool, out: &mut Vec<BaseLine>) {
    // このタイルに coastline レイヤがあるか（無ければ waterarea で代替）。
    let has_coastline = tile.layers.iter().any(|l| l.name == "coastline");

    for layer in &tile.layers {
        let kind = match layer.name.as_str() {
            "coastline" => BaseLineKind::Coastline,
            "boundary" => BaseLineKind::Boundary,
            "road" if want_roads => BaseLineKind::Road,
            "railway" if want_roads => BaseLineKind::Railway,
            // 海岸線フォールバック：waterarea（polygon, 全ズームにある）の外周を線扱い。
            "waterarea" if !has_coastline => BaseLineKind::Coastline,
            _ => continue,
        };

        let extent = layer.extent.unwrap_or(4096) as f64;
        for f in &layer.features {
            for path in decode_geom(&f.geometry) {
                if path.len() < 2 {
                    continue; // 単独点は線にならない
                }
                let mut pts: Vec<(f64, f64)> = Vec::with_capacity(path.len());
                for (lx, ly) in path {
                    // タイルローカル整数 → 分数タイル座標 → 緯度経度。
                    let xf = tx as f64 + lx as f64 / extent;
                    let yf = ty as f64 + ly as f64 / extent;
                    pts.push(tile_frac_to_lonlat(z, xf, yf));
                }
                out.push(BaseLine { points: pts, kind });
            }
        }
    }
}

/// MVT ジオメトリのコマンド列 → タイルローカル整数座標のパス群。
///
/// コマンド = (id & 0x7) | (count << 3)。id: 1=MoveTo, 2=LineTo, 7=ClosePath。
/// MoveTo/LineTo の後に count*2 個の zigzag 差分が続く。MoveTo は新しいパスを開始、
/// ClosePath は現在リングを始点に戻して閉じる。
fn decode_geom(geom: &[u32]) -> Vec<Vec<(i32, i32)>> {
    let mut paths: Vec<Vec<(i32, i32)>> = Vec::new();
    let mut cur: Vec<(i32, i32)> = Vec::new();
    let (mut x, mut y) = (0i32, 0i32);
    let mut i = 0usize;

    while i < geom.len() {
        let cmd = geom[i] & 0x7;
        let count = (geom[i] >> 3) as usize;
        i += 1;
        match cmd {
            1 => {
                // MoveTo：通常は1点。出るたびに新しいパスを開始。
                for _ in 0..count {
                    if i + 1 >= geom.len() {
                        break;
                    }
                    x += zigzag(geom[i]);
                    y += zigzag(geom[i + 1]);
                    i += 2;
                    if !cur.is_empty() {
                        paths.push(std::mem::take(&mut cur));
                    }
                    cur.push((x, y));
                }
            }
            2 => {
                // LineTo
                for _ in 0..count {
                    if i + 1 >= geom.len() {
                        break;
                    }
                    x += zigzag(geom[i]);
                    y += zigzag(geom[i + 1]);
                    i += 2;
                    cur.push((x, y));
                }
            }
            7 => {
                // ClosePath：始点に戻して閉じる（count はパラメータを持たない）。
                if let Some(&first) = cur.first() {
                    cur.push(first);
                }
            }
            _ => break, // 未知コマンド：安全側で打ち切り
        }
    }
    if !cur.is_empty() {
        paths.push(cur);
    }
    paths
}

/// MVT の zigzag デコード（u32 → i32）。
#[inline]
fn zigzag(n: u32) -> i32 {
    ((n >> 1) as i32) ^ (-((n & 1) as i32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use geozero::mvt::tile::Value;

    #[test]
    fn zigzag_roundtrip() {
        // MVT 仕様の代表値。
        assert_eq!(zigzag(0), 0);
        assert_eq!(zigzag(1), -1);
        assert_eq!(zigzag(2), 1);
        assert_eq!(zigzag(3), -2);
        assert_eq!(zigzag(10), 5);
    }

    /// i32 → MVT zigzag エンコード（テスト用、`zigzag` の逆）。
    fn zz(n: i32) -> u32 {
        ((n << 1) ^ (n >> 31)) as u32
    }
    fn val_str(s: &str) -> Value {
        Value {
            string_value: Some(s.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn decode_labels_ja_uses_annochar_then_knj_and_skips_empty() {
        // label レイヤを合成（keys: annoChar, knj）：
        //  feat0: annoChar="東京駅", knj="東京駅"   → "東京駅"（annoChar 優先）
        //  feat1: knj="日本歯科大" のみ（annoChar 欠）→ "日本歯科大"（knj フォールバック）
        //  feat2: どちらも空                          → スキップ
        let z = 13u8;
        let (tx, ty) = (7276u32, 3225u32);
        let pt = vec![9u32, zz(2048), zz(2048)]; // MoveTo 1点（タイル中心）
        let layer = Layer {
            name: "label".to_string(),
            extent: Some(4096),
            keys: vec!["annoChar".to_string(), "knj".to_string()],
            values: vec![
                val_str("東京駅"),       // 0
                val_str("日本歯科大"),   // 1
                val_str(""),             // 2 (空)
            ],
            features: vec![
                Feature { tags: vec![0, 0, 1, 0], geometry: pt.clone(), ..Default::default() },
                Feature { tags: vec![1, 1], geometry: pt.clone(), ..Default::default() },
                Feature { tags: vec![0, 2], geometry: pt, ..Default::default() },
            ],
            ..Default::default()
        };
        let tile = Tile { layers: vec![layer] };

        let mut out = Vec::new();
        decode_labels_ja(&tile, z, tx, ty, &mut out);

        assert_eq!(out.len(), 2, "空テキストは除外され2件");
        assert_eq!(out[0].text, "東京駅");
        assert_eq!(out[1].text, "日本歯科大");
        // 座標はタイル中心と一致。
        let (elat, elon) = tile_frac_to_lonlat(z, tx as f64 + 0.5, ty as f64 + 0.5);
        assert!((out[0].lat - elat).abs() < 1e-9);
        assert!((out[0].lon - elon).abs() < 1e-9);
    }

    #[test]
    fn decode_linestring() {
        // MoveTo(5,5) → LineTo(+2,0)(0,+3)。
        // MoveTo count1 = 9, params zigzag(5)=10,10
        // LineTo count2 = 18, params (2→4,0→0),(0→0,3→6)
        let geom = [9u32, 10, 10, 18, 4, 0, 0, 6];
        let paths = decode_geom(&geom);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], vec![(5, 5), (7, 5), (7, 8)]);
    }

    #[test]
    fn decode_polygon_ring_is_closed() {
        // MoveTo(0,0) → LineTo(+4,0)(0,+4) → ClosePath。閉じた4点リング。
        let geom = [9u32, 0, 0, 18, 8, 0, 0, 8, 15];
        let paths = decode_geom(&geom);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], vec![(0, 0), (4, 0), (4, 4), (0, 0)]);
    }

    #[test]
    fn decode_multi_ring() {
        // 2リング：MoveTo..ClosePath を2回。
        let geom = [
            9, 0, 0, 18, 8, 0, 0, 8, 15, // ring1: (0,0)(4,0)(4,4)+close, カーソルは(4,4)
            9, 2, 2, 18, 4, 0, 0, 4, 15, // ring2: MoveTo(+1,+1)→(5,5) から開始（相対累積）
        ];
        let paths = decode_geom(&geom);
        // ClosePath 後の MoveTo はカーソルを継続（相対）。リングは2本。
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], vec![(0, 0), (4, 0), (4, 4), (0, 0)]);
        // ring2 開始点: 直前カーソルは (4,4)。MoveTo(+1,+1)→(5,5)。
        assert_eq!(paths[1][0], (5, 5));
    }
}
