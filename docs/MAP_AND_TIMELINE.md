# 追補タスク：地図ベース層 + 雨雲タイムライン (MAP_AND_TIMELINE.md)

既存の weathermap に2つの機能を足す。前回同様、このファイルを読んで設計どおりに実装・検証してほしい。設計の鉄則（wm-core は no_std・無改変で RISC-V に載る／時刻は外部から u64 で渡す／データの作り方は外、描画ロジックは中）は維持すること。

---

## 機能1：地図ベース層（MapSCII方式・地理院ベクトルタイル）

### 現状の問題
`map.rs` は背景を空白で塗り、その上に雨雲 `DrawCell` を重ね、中心 `◎` を置くだけ。地図（海岸線・行政界）の絵が無いため「雨雲だけが宙に浮く」。MapSCII（https://github.com/rastapasta/mapscii）に倣い、ベクタータイルを取得して Braille で線を描く層を足す。

### データ源
地理院ベクトルタイル（experimental_bvmap, MVT/pbf, **APIキー不要**）:
```
https://cyberjapandata.gsi.go.jp/xyz/experimental_bvmap/{z}/{x}/{y}.pbf
```
- 出典明示：「国土地理院ベクトルタイル提供実験」（README とアプリ内に記載）。
- 座標系：EPSG:3857（Web Mercator）。雨雲（JMAナウキャスト）と同一座標系なので位置が揃う。
- **ズーム注意**：bvmap の z はラスターより 1 小さい（bvmap z11 ≒ ラスター z12 相当）。だが JMAナウキャストの z と「同じ XYZ スキーム」で取得すれば、同じ (z,x,y) のタイルは同じ地理範囲を指す。実装では雨雲と同じ z/x/y でタイルを引けば整合する。提供範囲は z=4..16。

### レイヤ選択（experimental_bvmap の source-layer 名とズーム対応）
取得・描画するのは以下。**ズーム帯ごとに含まれるレイヤが違う**ので、無いものは取れない（下表は実測の対応）。

| source-layer | 種別 | 用途 | z4-7 | z8-10 | z11-13 | z14-16 |
|---|---|---|---|---|---|---|
| `coastline` | line | 海岸線 | ○ | **×** | **×** | ○ |
| `waterarea` | polygon | 水域（海岸線フォールバック元） | ○ | ○ | ○ | ○ |
| `boundary` | line | 行政界（県境） | ○ | ○ | ○ | ○ |
| `road` | line | 道路 | ○ | ○ | ○ | ○ |
| `railway` | line | 鉄道 | ○ | ○ | ○ | ○ |

**確定仕様（描画するレイヤ）**：海岸線 + 行政界 + 主要道路 + 鉄道。

### 海岸線フォールバック（重要）
`coastline` は z=8,9,10 に**含まれない**（アプリのデフォルト zoom=8 はまさにこの帯）。対応は**ハイブリッド**：
1. まず `coastline` レイヤがあればそれを使う。
2. 無いズーム帯（8-10 等）では `waterarea`（polygon, 全ズームにある）の**外周リング**を線として描き、海岸線の代わりにする。waterarea は海・湖を面で持つので、その縁が実質の陸海境界になる。

実装上は「coastline の線分」も「waterarea ポリゴンの外周」も、最終的に同じ `LineString`（点列）に正規化して同じ描画関数に渡す。

### レイヤごとの色（truecolor、地図は暗めにして雨雲を引き立てる）
- 海岸線 / waterarea 輪郭：青灰 `Rgb(90,120,150)`
- 行政界：暗い灰 `Rgb(80,80,90)`（破線的に間引いてもよい）
- 道路：くすんだ茶/灰 `Rgb(110,95,80)`
- 鉄道：灰 `Rgb(100,100,105)`

雨雲が乗るセルでは雨雲を優先（§合成順序）。地図線は雨雲より暗いので、重なっても雨雲が目立つ。

---

## wm-core への追加（線分 → Braille 描画）

雨雲（Grid → DrawCell）と対をなす、**線データ → DrawCell** の純粋関数を `wm-core` に足す。これが移植性の肝：MVTデコード（std依存）は wm-sources、線を画面に投影して点を打つロジックは wm-core。

### 新規ファイル：`crates/wm-core/src/render/lines.rs`

```rust
//! 線分（緯度経度の点列）→ Braille セルへのラスタライズ。
//! 雨雲の braille.rs と同じく、画面を 2x4 ドットの Braille グリッドとして扱う。
//! Bresenham でドット単位に線を引き、点灯ドットを Braille セルへ畳む。

use heapless::Vec;
use super::{DrawCell, Rgb};
use crate::geo::GeoBBox;

/// 1本の線（点列）。座標は緯度経度。
pub struct PolyLine<'a> {
    pub points: &'a [(f64, f64)], // (lat, lon)
    pub color: Rgb,
}

/// 線描画の最大セル数（雨雲と別枠）。
pub const MAX_LINE_CELLS: usize = 12_000;
pub type LineCellVec = Vec<DrawCell, MAX_LINE_CELLS>;

/// 複数の線を、指定 BBox・セル数の画面へラスタライズする。
///
/// - `bbox`: 画面が覆う地理範囲（App::current_bbox と同じもの）。
/// - `cols/rows`: マップ領域の文字数。1セル=2x4ドット。
/// - 出力は点灯セルのみ。同一セルに複数線が来たら後勝ち（呼び出し側で
///   描画順を制御：海岸線→行政界→道路→鉄道 の順に呼べば鉄道が上）。
pub fn rasterize_lines(
    lines: &[PolyLine],
    bbox: &GeoBBox,
    cols: u16,
    rows: u16,
) -> LineCellVec;
```

### アルゴリズム
1. ドットキャンバスを用意（幅 `cols*2`、高さ `rows*4` ドット）。各ドットに「点灯フラグ + 色」を持たせる。サイズが大きいので `heapless` の固定長配列、または各セルの 8bit パターン + 代表色を持つ `cols*rows` の配列で表現（後者が省メモリ）。
2. 各 PolyLine の各セグメント (p0→p1) について：
   - 緯度経度 → ドット座標へ投影。投影は **Web Mercator で線形化**：`geo.rs` の `lonlat_to_pixel` で両端点をグローバルピクセルにし、BBox の左上ピクセルを原点に、BBox のピクセル幅で正規化 → `[0,1)` → `*dots` でドット座標。雨雲 radar.rs と同じ投影式なので位置が一致する。
   - 2 ドット点を **Bresenham** で結び、通過ドットを点灯。色はその線の色。
3. ドットキャンバス → DrawCell：セルごとに 8 ドットのビットパターンを作り（braille.rs の `dot_bit` と同じ規格）、点灯があれば `U+2800+pattern` の文字と代表色で DrawCell を生成。

> braille.rs に既にある `dot_bit` / `braille_char` を再利用できるよう、それらを `pub(crate)` か `pub` に上げて lines.rs から使う。重複実装しないこと。

### geo.rs に必要なら追加
`lonlat_to_pixel` は既にある（雨雲で使用）。線描画でも同じものを使うので**追加不要**のはず。BBox→ドット正規化のヘルパーが欲しければ `geo.rs` に純粋関数で足してよい（no_std 維持）。

### render/mod.rs
```rust
pub mod lines;          // 追加
pub use lines::{rasterize_lines, PolyLine, LineCellVec};
```

### テスト（wm-core、std で）
- 既知 BBox・1本の水平線 → 期待される行のセルが点灯し、他は消灯。
- 対角線 → Bresenham で連続したドット列になる（穴が空かない）。
- BBox 外の点を含む線 → 画面内クリップされる（パニックしない）。
- 雨雲 braille と同じ投影なので、同じ緯度経度の点が雨雲セルと同じ (col,row) に来ることを確認するテストを1本入れると「地図と雨雲がズレない」ことの回帰防止になる。

---

## wm-sources への追加（MVTデコード）

### 新規ファイル：`crates/wm-sources/src/basemap.rs`

地理院ベクトルタイルを取得し、必要レイヤの線・ポリゴン外周を緯度経度の点列に変換する。

```rust
//! 地理院ベクトルタイル(MVT/pbf)を取得し、海岸線/行政界/道路/鉄道を
//! 緯度経度の点列へデコードする。wm-core の rasterize_lines に渡す材料を作る。

use wm_core::geo::GeoBBox;

/// 1本の線（緯度経度）+ 種別。
pub struct BaseLine {
    pub points: Vec<(f64, f64)>,   // (lat, lon)
    pub kind: BaseLineKind,
}

#[derive(Clone, Copy)]
pub enum BaseLineKind { Coastline, Boundary, Road, Railway }

pub struct BaseMapProvider { client: reqwest::Client }

impl BaseMapProvider {
    /// BBox を覆うタイルを取得・デコードし、線群を返す。
    /// coastline が無いズーム帯では waterarea ポリゴン外周を Coastline として返す。
    pub async fn fetch_lines(&self, bbox: GeoBBox, zoom: u8) -> Result<Vec<BaseLine>>;
}
```

### MVTデコードの実装方針（crate 選定はあなたに任せる）
**自前 protobuf 実装はしないこと。** MVT のジオメトリエンコーディング（zigzag + コマンド整数列）は罠が多く、本題（Braille 描画）から逸れる。以下のいずれか、ビルドが通り依存が軽い方を選ぶ：
- `geozero` の MVT 機能（`features = ["with-mvt"]`）でレイヤ・フィーチャを反復し、ジオメトリを取り出す。
- または MVT 特化の軽量 crate（例：`mvt` 系）。
- どうしても薄くしたいなら `prost` + vector-tile-spec の .proto から生成し、ジオメトリのコマンドデコード（MoveTo/LineTo/ClosePath, zigzag）だけ自前。ただし優先度は低い。

選んだ crate と理由を実装時にコメントで残すこと。

### デコード手順
1. `bbox_to_tile_range`（geo.rs, 既存）で必要タイル (x,y) 範囲を出す。
2. 各タイル `.../experimental_bvmap/{z}/{x}/{y}.pbf` を取得。404/空はスキップ。
3. MVT をデコードし、対象 source-layer（`coastline`/`boundary`/`road`/`railway`、無ければ `waterarea`）のフィーチャを取得。
4. 各フィーチャのジオメトリ（タイルローカル整数座標、`extent` で正規化、通常 4096）を **タイル位置と合わせて緯度経度へ逆投影**。
   - タイルローカル `(lx, ly) ∈ [0, extent)` → タイル内正規化 `(lx/extent, ly/extent)` → グローバルピクセル → 緯度経度。geo.rs に逆変換 `pixel_to_lonlat` が無ければ足す（`tile_nw_corner` + オフセットでも可）。no_std 維持。
5. `waterarea`（polygon）はリング（外周）を取り出して閉じた点列にし、`Coastline` 扱いで返す。
6. 取得過多を防ぐため、ズーム別にレイヤを間引く（道路・鉄道は低ズームでは省く等）。負荷と可読性のバランスはあなたの判断で。

### lib.rs
```rust
pub mod basemap;   // 追加
```

---

## wm-tui への反映（地図描画 + 合成順序）

### app.rs
`App` に地図線のキャッシュを追加：
```rust
pub basemap: Option<Vec<wm_sources::basemap::BaseLine>>,
```
パン/ズームで BBox が変わったら再取得（雨雲と同じタイミング）。地図は雨雲ほど頻繁に変わらないので、**ズーム/中心が変わったときだけ**取得すれば十分（毎フレーム取得しない）。

### map.rs の合成順序（重要）
現状の「背景→雨雲→マーカー」を次に変える：
```
1. 地図ベース層を rasterize_lines で描画
     順番：海岸線 → 行政界 → 道路 → 鉄道（後のものが上）
     各 DrawCell を buf に設定（暗めの色）
2. 雨雲レイヤ（タイムラインの現在コマ）を quantize で描画
     雨雲セルは地図セルを上書き（雨を優先）
3. 中心マーカー ◎
4. タイムライン UI（§機能2）
```
実装：`BaseLine` を `wm_core::render::lines::PolyLine` に詰め替えて（kind→color のマッピングはここで）、`rasterize_lines(&lines, &bbox, cols, rows)` を呼び、返った DrawCell を buf へ。続いて雨雲の DrawCell を上書き。

### 地図取得の非同期
地図取得もネットワークなので、§機能2 と合わせて取得は別 task + チャネルにするのが理想（現状の同期取得のままだと固まる）。最低限、地図取得はズーム/パン時のみに限定して頻度を下げる。

---

## 機能2：雨雲タイムライン（space 再生 + コマ送り）

### 現状
`radar.rs` の `JmaNowcast` は targetTimes_N1（実況）の**最新1コマ**だけ取得。これを**実況＋予報の複数コマ**にし、再生・コマ送りできるようにする。

### データ源（2系統）
- 実況：`https://www.jma.go.jp/bosai/jmatile/data/nowc/targetTimes_N1.json`（過去〜現在、降順）
- 予報：`https://www.jma.go.jp/bosai/jmatile/data/nowc/targetTimes_N2.json`（現在〜1時間先、5分間隔）

両方ともタイル URL は同形式：
```
https://www.jma.go.jp/bosai/jmatile/data/nowc/{basetime}/none/{validtime}/surf/hrpns/{z}/{x}/{y}.png
```
**鉄則（厳守）**：時刻は必ず targetTimes から取得。現在時刻から推測しない（404 が CDN にキャッシュされ有害）。これは既存実装のコメントにもある。

### コマ数（軽さ優先・確定仕様）
合計 **13 コマ程度**に抑える：
- 実況：直近の数コマ（例：過去 20〜30 分ぶん＝5〜6 コマ）
- 予報：1 時間先まで（5 分間隔＝最大 12 コマだが間引いて 6〜7 コマ）
- 時間昇順（過去→未来）に整列して 1 本のフレーム列にする。
- N1 は降順なので反転して使う。N2 は昇順。

各フレーム = 1 時刻ぶんの Grid（PrecipMmH）。13 個の Grid を保持。

> **メモリ注意（前回の既知事項）**：非 embedded の Grid は heapless で約 256KB インライン。13 個を `Vec<Grid>` でヒープに持てば問題ないが、`Vec<Box<Grid>>` か、グリッド解像度を抑える（表示セル数ぶんで十分、256 上限は不要）こと。13×256KB=3.3MB をスタックに置かないよう、必ずヒープ（Vec/Box）に。

### radar.rs の変更
```rust
/// 1 フレーム：時刻 + その時刻の雨量グリッド。
pub struct RadarFrame {
    pub valid_unix: u64,   // validtime を unix 秒に変換（表示用）
    pub is_forecast: bool, // 予報フレームか（UI で色分け）
    pub grid: Box<Grid>,   // ヒープに置く
}

/// 実況+予報をまとめて時系列フレーム列を取得する。
pub async fn fetch_radar_timeline(
    &self,
    bbox: GeoBBox,
    zoom: u8,
    max_frames: usize,   // 13 程度
) -> Result<Vec<RadarFrame>>;
```
- N1 と N2 を取得 → 時刻を集めて昇順整列 → 間引いて max_frames 以内 → 各時刻でタイル取得・PNG デコード・色逆引き（既存 `jma_color_to_precip` 再利用）→ Grid 化。
- 既存の単一フレーム `fetch_radar` は内部的にこれの「最新実況1コマ」版として残してよい。

### app.rs（再生状態）
```rust
pub frames: Vec<RadarFrame>,   // 時系列（昇順）
pub frame_idx: usize,          // 現在表示中のコマ
pub playing: bool,             // 再生中か
last_advance: Instant,         // 自動再生の前進タイミング（main 側でも可）
```
- 表示する雨雲は `frames[frame_idx].grid`。
- 再生中は一定間隔（例 500ms）で `frame_idx` を +1、末尾までいったら先頭へループ。

### input.rs（キー追加）
```
Space      → playing をトグル（再生/一時停止）
. または → → 1 コマ進める（手動、playing は止める）
, または ← → 1 コマ戻す
```
> 既存の ← → はパンに割り当て済み。**競合する**ので方針を決める：
> - 案A（推奨）：`,`/`.`（および `[`/`]`）をコマ送りに、パンは方向キーと hjkl のままにする。Space は再生トグル。
> - 案B：再生モードと地図操作モードを分け、Space でモード切替。
>
> ユーザー要望は「space で再生トグル、矢印などでコマ送り」。方向キーをコマ送りにするとパンと衝突するため、**パンを hjkl 専用にし、方向キー←→をコマ送り、↑↓をパンに残す**、もしくは案A で `,`/`.` をコマ送りにするのが衝突が少ない。実装時にどちらか選び、ヘルプ行に明記すること。コマ送り時は playing=false にする。

### ui/mod.rs・新規 ui/timeline.rs（タイムライン表示）
マップ下部に 1 行のタイムラインバーを足す：
```
過去 ◀ ━━━●━━━━━━━━ ▶ 未来   12:30 (実況)
        └ 現在位置（frame_idx）
```
- フレーム列を `━` で表し、現在位置を `●`、実況/予報の境界を色で分ける（実況＝白〜灰、予報＝水色系）。
- 現在コマの時刻（`valid_unix` をローカル時刻に整形）と「実況/予報」ラベルを出す。
- 再生中は `▶`、停止中は `⏸` を表示。

レイアウトは既存 `ui::draw` の地図領域の下に 1〜2 行確保して差し込む（凡例やヘルプ行と干渉しないよう Layout を調整）。

### 時刻整形
`valid_unix`（u64）をローカル時刻 `HH:MM` にするのは wm-tui 側（std）。`chrono` を足すか、`time` crate を使う。wm-core には持ち込まない。

---

## 取得の非同期化（この際やる・前回 §6/§7 の宿題）
地図・雨雲タイムラインで取得量が増えるため、同期取得のままだと UI が長く固まる。**取得をバックグラウンド task に出す**：
- `tokio::spawn` で取得（天気・雨雲タイムライン・地図）を回し、`tokio::sync::mpsc` で結果（Snapshot / Vec<RadarFrame> / Vec<BaseLine>）を送る。
- イベントループは `rx.try_recv()` でノンブロッキングに受けて App を更新、毎フレーム再描画。
- 取得中も再生（frame_idx の前進）と UI は動き続ける。
- 併せて **panic hook**（前回 §7）も入れる：panic 時に raw mode 解除 + LeaveAlternateScreen して端末を復帰。

---

## 受け入れ基準（目視 + テスト）
- [ ] `cargo test -p wm-core`：lines.rs のテスト含め緑。地図線と雨雲が同じ投影で一致するテストが通る。
- [ ] RISC-V embedded ビルドが通る（lines.rs が no_std を壊していない）。
- [ ] `cargo test -p wm-sources`：basemap デコードのテスト（縮約タイル or モック）。
- [ ] `cargo run -p wm-tui`：起動すると**海岸線・県境・道路・鉄道が暗色で描かれ、その上に雨雲が truecolor で乗る**（雨雲が宙に浮かない）。
- [ ] z=8 でも海岸線が出る（waterarea フォールバックが効いている）。
- [ ] Space で雨雲が再生（過去→現在→1時間先がパラパラ動く）、もう一度 Space で停止。
- [ ] コマ送りキーで 1 コマずつ前後し、タイムラインの現在位置と時刻表示が追従する。
- [ ] 実況/予報の境界がタイムラインで色分けされる。
- [ ] 取得中も UI が固まらない（非同期化）。panic しても端末が壊れない（hook）。

## 実装順序の提案
1. wm-core `lines.rs`（純粋・テスト先行）→ 雨雲と投影一致を確認。
2. wm-sources `basemap.rs`（MVT crate 選定 → 1 タイルをデコードして点列が出るところまで）。
3. wm-tui で地図を描画（合成順序）→ まず静止画で「地図＋雨雲」を成立させる。
4. radar.rs タイムライン化 → app/input/timeline で再生・コマ送り。
5. 取得の非同期化 + panic hook。
6. 受け入れ基準を順に確認。

各段階でコンパイルを通してから次へ。最大の不確実点は (a) MVT crate のジオメトリ取得 API と (b) bvmap のズーム/タイル整合（雨雲と同じ z/x/y で地理範囲が一致するか）。(b) は実タイルを 1 枚取って、既知地点（東京駅など）が画面の正しい位置に来るか早めに確認すること。
