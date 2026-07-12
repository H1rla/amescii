# 追補タスク：都市/地名ラベル + 雨雲トグル (LABELS_AND_TOGGLE.md)

既存の amescii に2つ足す。前回同様このファイルを読んで実装・検証してほしい。設計の鉄則（wm-core は no_std・無改変で RISC-V に載る／時刻は外部から u64 で渡す／データの作り方は外・描画ロジックは中）は厳守。

前提：機能1（地図ベース層 `lines.rs`/`basemap.rs`）と機能2（タイムライン）は実装済み。本タスクはその上に乗せる。

---

## 機能A：都市/地名ラベル（ハイブリッド・全表記英語）

### 方針（確定）
地図上に地名ラベルを描く。**広域は内蔵テーブルの主要都市、拡大は地理院 `label` レイヤの細かい地名**、どちらも**英語（ローマ字）表記**で統一する。

- 広域（zoom 3–9）：クレート内蔵の主要都市テーブル（英語名、人口/重要度ランク付き）。軽量・オフライン・no_std。
- 拡大（zoom 10 以上）：地理院ベクトルタイル `label` レイヤの `name`（ローマ字）属性。その場所の町・地区名が出る。
- 表記が英語で一貫するので混在しない。

### 重要事実：label レイヤは英語名を持つ
`experimental_bvmap` の `label` レイヤの各フィーチャ（Point）の properties は **日本語と英語の両方**を含む。実例（金沢）:
```json
{
  "name": "Kanazawa",   // ローマ字（これを使う）
  "knj": "金沢",         // 漢字
  "kana": "かなざわ",
  "ftCode": 51301,       // 地物コード（注記種別の判別に使える）
  "dspPos": "RC",        // 表示位置の基準（配置ヒント）
  "arrng": 1, "arrngAgl": 0
}
```
→ 拡大時も `name`（ローマ字）を描けば英語で出せる。`knj` には縛られない。

`ftCode` で注記の種類（居住地名/自然地名/施設…）と規模が分かる。ズームに応じて重要な注記だけ出すフィルタに使う（例：低めのズームでは大きい居住地名のみ、拡大で小地名も）。`ftCode` の意味は提供実験リポジトリの「地物コード及び表示ズームレベル一覧」(dataspec) 準拠。実装時に実タイルを 1 枚デコードして、`label` レイヤにどんな `ftCode`/`name` が入るか確認してからフィルタ閾値を決めること。

### wm-core への追加：内蔵都市テーブル + ラベル配置

#### 新規ファイル：`crates/wm-core/src/places.rs`
```rust
//! 内蔵の主要都市テーブルと、画面内ラベルの配置算出。
//! 純粋ロジック（no_std）。座標→画面位置は geo.rs の既存変換を使う。

use crate::geo::GeoBBox;
use heapless::Vec;

/// 内蔵都市の 1 件。
pub struct City {
    pub name: &'static str,   // 英語名 "Tokyo"
    pub lat: f64,
    pub lon: f64,
    /// 重要度ランク（小さいほど重要＝低ズームでも出す）。
    /// 0=三大都市級, 1=政令市/県庁, 2=主要市 …
    pub rank: u8,
}

/// 内蔵テーブル（数十都市）。重要度ランク付き。
/// 最低限：札幌/仙台/東京/横浜/名古屋/京都/大阪/神戸/広島/福岡/那覇 等の
/// rank0–1 を確実に、rank2 で県庁所在地クラスを網羅。緯度経度は既存の
/// jma.rs area_for テーブルを出発点に拡張してよい。
pub static CITIES: &[City] = &[ /* … */ ];

/// 画面に描くべきラベル 1 件（出力中間表現）。
#[derive(Clone, Copy)]
pub struct PlaceLabel {
    pub col: u16,        // テキスト開始セル（マーカーの右）
    pub row: u16,
    pub marker_col: u16, // マーカー位置（col の 1 つ左など）
    pub name: &'static str,
}

pub const MAX_LABELS: usize = 64;
pub type LabelVec = Vec<PlaceLabel, MAX_LABELS>;

/// BBox・ズーム・画面セル数から、内蔵都市のうち画面内かつ
/// そのズームで表示すべきものをラベル配置して返す。
///
/// - ズーム閾値：rank <= zoom_to_max_rank(zoom) の都市のみ。
///   例：zoom 3–5 → rank0 のみ、6–7 → rank<=1、8–9 → rank<=2。
/// - 画面内判定：lat/lon が bbox 内か。
/// - 配置：lonlat→画面セル（geo.rs の投影を流用、radar/lines と同じ式）。
/// - 重なり回避：同じ row 帯で col が近すぎる 2 件は rank の高い方（数値小）を残す。
pub fn layout_city_labels(
    bbox: &GeoBBox,
    zoom: u8,
    cols: u16,
    rows: u16,
) -> LabelVec;

/// ズーム→表示する最大 rank。拡大ほど多く出す。
fn zoom_to_max_rank(zoom: u8) -> u8 { /* 3-5:0, 6-7:1, 8-9:2, ... */ }
```

> 投影は radar.rs / lines.rs と同一式（geo.rs の `lonlat_to_pixel` → BBox 正規化 → セル）。**同じ投影を使うこと**——でないとラベルと地図・雨雲がズレる。lines.rs で BBox→セル変換のヘルパを作っているはずなので、それを `pub(crate)` 化して places.rs からも使うと重複が無い。

#### lib.rs / render との関係
- `places.rs` は `crate::geo` だけに依存。`lib.rs` に `pub mod places;`。
- ラベルの「文字をどう画面に出すか」は wm-tui の責務（後述）。wm-core は「どの名前をどのセルに置くか」(`PlaceLabel`) までを返す。

### wm-sources への追加：label レイヤのデコード（拡大時の地名）

`basemap.rs` に、`label` レイヤをデコードして英語地名の点を返す機能を足す。

```rust
/// 拡大時の地名ラベル 1 件（緯度経度 + 英語名）。
pub struct NameLabel {
    pub lat: f64,
    pub lon: f64,
    pub name: String,    // properties.name（ローマ字）
    pub ft_code: u32,    // 種別フィルタ用
}

impl BaseMapProvider {
    /// label レイヤをデコードし、英語名つきの地名点を返す。
    /// name が空 or ASCII でないものは除外（ローマ字のみ採用）。
    /// 取得タイルは地図線と同じ z/x/y。
    pub async fn fetch_labels(&self, bbox: GeoBBox, zoom: u8) -> Result<Vec<NameLabel>>;
}
```
- 既存の MVT デコード基盤（`fetch_lines` で使っている crate）をそのまま使い、対象 source-layer を `label` にしてフィーチャの properties から `name`/`ftCode` を読む。
- `name` が無い／非 ASCII のフィーチャはスキップ（英語表記に統一するため）。
- 拡大時のみ呼ぶ（zoom >= 10 等）。広域では呼ばない（内蔵テーブルで足りる＋負荷削減）。

### wm-tui への反映：ラベル描画

#### app.rs
```rust
/// 拡大時の地名（地理院 label 由来）。zoom>=閾値のときだけ取得。
pub name_labels: Option<Vec<wm_sources::basemap::NameLabel>>,
pub show_radar: bool,   // 機能B
```
- ズーム/中心が変わったら、zoom>=10 なら `fetch_labels` を（地図取得と同じ非同期経路で）取得、未満なら `name_labels=None`。

#### map.rs：描画順序（更新）
```
1. 地図ベース層 lines（海岸線→行政界→道路→鉄道）
2. 雨雲（show_radar==true のときのみ）quantize で上書き
3. 地名ラベル
     - zoom < 10：wm_core::places::layout_city_labels の PlaceLabel
     - zoom >= 10：fetch_labels の NameLabel を画面セルへ投影して描画
       （投影は wm-core と同式。NameLabel は String なので幅ぶんセルに書く）
     ラベルは雨雲の「後」に描く＝地名が雨雲の上に出る（場所を見失わないため）。
     ただしマーカー点は控えめ、文字色は白に薄い影/背景で可読性確保。
4. 中心マーカー ◎
5. タイムライン（show_radar==true のときのみ）
```
- ラベル文字は 1 セル幅前提（英語）。テキストが画面右端を超えるならクリップ。
- 重なり：wm-core 側（内蔵）は layout で間引き済み。label 由来（拡大時）は数が多いので、wm-tui 側で「同じ row の近接ラベルは先勝ち/重要 ftCode 優先」で簡易間引きする。
- マーカーは `·`（U+00B7）か小さめの記号。`· Tokyo` のように出す。

#### 可読性
地図線・雨雲の上に文字を重ねると下のドットが消える。ラベルは要所のみ・短く。背景を 1 段暗くする（セルの bg を黒寄りにして文字を白で）と読みやすい。やりすぎると地図が隠れるのでラベル数を絞ることで対処。

### wm-core テスト
- `layout_city_labels`：東京を含む BBox・zoom8 で "Tokyo" が画面内の妥当なセルに来る。zoom3 では rank0 のみ（中小都市が出ない）。
- 画面外の都市は出ない。近接 2 都市で重なる場合に高ランクが残る。
- 投影一致：同じ緯度経度が lines/radar と同じ (col,row) に来る回帰テスト（既存の投影一致テストに準じる）。

---

## 機能B：雨雲表示トグル（t キー）

### 仕様（確定）
`t` キーで雨雲レイヤの表示/非表示を切り替え。OFF のときは**地図と地名だけ**になる（タイムラインバーも隠す）。パン・ズーム・ラベルは ON/OFF どちらでも有効。

### input.rs
```
t  → app.show_radar をトグル
```
ヘルプ行に `[t]雨雲` を追加。既存キー（space=再生, ,/.=コマ送り, 矢印/hjkl=パン, +/-=ズーム, r=更新, q=終了）は不変。

### app.rs
```rust
pub show_radar: bool,   // 既定 true
```
- OFF の間も `frames` は保持してよい（再 ON で即表示）。OFF 中は `advance_play`（自動再生の前進）を止めると無駄が無い。

### map.rs / ui/mod.rs
- 雨雲描画（quantize 部）を `if app.show_radar { … }` で囲う。
- タイムラインバー（ui/timeline.rs）も `show_radar` が false なら描画しない。空いた行は地図領域に回す or 単に空ける（レイアウトを崩さない範囲で）。
- 雨雲 OFF 時、サイドバーの天気数値は出したままでよい（天気とレーダーは別物）。降水量の数値表示も維持。

### 取得との関係
- 雨雲 OFF にしても天気・地図・ラベルの取得は通常どおり。雨雲 OFF 中に新規取得を止めるかは任意（止めると通信節約、続けると再 ON 時に最新）。既定は「OFF 中はタイムラインの自動再生だけ止める、取得は継続」で十分。

---

## 受け入れ基準
- [ ] `cargo test -p wm-core`：places.rs のテスト（配置・ズーム閾値・投影一致）含め緑。
- [ ] RISC-V embedded ビルド緑（places.rs が no_std を壊していない）。
- [ ] `cargo test -p wm-sources`：fetch_labels のデコード（縮約 label タイル or モック）。
- [ ] `cargo run -p wm-tui`：
  - [ ] 広域（zoom 5–8）で主要都市名が英語で出る（東京/大阪等）。ズームを下げると数が減り、上げると増える。
  - [ ] zoom 10 以上に拡大すると、その場所の細かい地名（地理院 label の `name`、英語）が出る。**どこまで拡大してもその地点の地名が出る**。
  - [ ] ラベルが地図・雨雲の上に読める形で出る（潰れすぎない／重なりすぎない）。
  - [ ] `t` で雨雲が消え、地図＋地名だけになる。もう一度 `t` で戻る。雨雲 OFF 中もパン/ズーム/ラベルが動く。
- [ ] 英語表記で一貫（広域も拡大も）。

## 実装順序の提案
1. wm-core `places.rs`（内蔵テーブル＋配置、テスト先行、投影一致確認）。
2. wm-tui で内蔵ラベルを描画（zoom<10）→ まず広域で都市名が出る状態に。
3. 機能B（t トグル）を先に入れてしまう（軽い・独立）。雨雲 ON/OFF を確認。
4. wm-sources `fetch_labels`（label レイヤを 1 タイルデコード → `name`/`ftCode` が取れるか確認 → ftCode フィルタ閾値決定）。
5. wm-tui で拡大時ラベル（zoom>=10）を描画。広域=内蔵／拡大=label の切替を確認。
6. 受け入れ基準を順に確認。

各段階でコンパイルを通してから次へ。最大の不確実点は `label` レイヤの実属性（`name` の有無・`ftCode` の分布・1 タイルあたりのラベル数）。実タイルを 1 枚デコードして中身を見てからフィルタとデコード処理を確定すること。広域↔拡大の切替ズーム（10 を境にするか）は実際の見え方で微調整してよい。
