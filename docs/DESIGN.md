# weathermap — 設計書

ターミナル上で日本の地図をASCII（Braille点字）描画し、複数の気象APIを集約した天気と雨雲レーダーを重畳表示するアプリケーション。Rust製。将来的にESP32 (RISC-V) への移植を見据えた構成を採る。

---

## 0. 設計の最重要原則：移植性

このプロジェクトの全設計判断は**「RISC-V (ESP32-C3) 移植時に `wm-core` を一切変更しない」**という制約から導かれる。

```
┌─────────────────────────────────────────────────────────┐
│  プラットフォーム依存層 (std前提・差し替え可能)              │
│  ┌──────────────┐  ┌──────────────┐                      │
│  │  wm-sources  │  │  wm-tui      │   PC: tokio, reqwest, │
│  │  HTTP/JSON   │  │  Ratatui     │       Ratatui          │
│  │  PNG decode  │  │  crossterm   │                       │
│  └──────┬───────┘  └──────┬───────┘                      │
│         │ 抽象データ        │ 抽象描画コマンド               │
└─────────┼──────────────────┼──────────────────────────────┘
          ▼                  ▼
┌─────────────────────────────────────────────────────────┐
│  wm-core  (#![no_std], プラットフォーム非依存)              │
│  ・データモデル (WeatherSnapshot, Grid, ...)               │
│  ・集約アルゴリズム (加重平均・CV・外れ値検出)               │
│  ・Braille量子化 (グリッド → 点字セル列)                    │
│  ・色マッピング (雨量 → RGB)                                │
│  ・地理座標変換 (lat/lon ↔ tile ↔ pixel)                  │
└─────────────────────────────────────────────────────────┘
          ▲                  ▲
          │                  │ 同じwm-coreを使う
┌─────────┼──────────────────┼──────────────────────────────┐
│  wm-esp32 (将来) no_std, esp-hal                           │
│  ・Open-Meteo cloud_cover を Grid に変換 (PNG読まない)      │
│  ・embedded-graphics で液晶描画                            │
└─────────────────────────────────────────────────────────┘
```

### 鉄則
1. **`wm-core` は `#![no_std]`。** `std`・`alloc`・`std::time`・ファイルI/O・ネットワークを直接呼ばない。
2. **`wm-core` は「すでに取得・デコード済みの抽象データ」だけを受け取る。** PNGをデコードするのも、HTTPを叩くのも、現在時刻を取るのも、すべて外側の責務。
3. **時刻は `u64` (unix秒) を引数で渡す。** `wm-core` 内部で時刻を取得しない。
4. **動的確保は `feature = "alloc"` でゲートする。** デフォルトは `heapless` の固定長コンテナのみ。PC版は `alloc` feature on、ESP32版は off（または小さいheap）。
5. **浮動小数の数学関数は `libm` 経由。** `f32::sqrt()` などは `no_std` で使えないため `libm::sqrtf` を使う。

---

## 1. クレート構成

```
weathermap/
├── Cargo.toml                    # [workspace]
├── README.md
├── docs/
│   ├── DESIGN.md                 # 本書
│   ├── AGGREGATION.md            # 集約アルゴリズム詳細
│   ├── PORTABILITY.md            # RISC-V移植チェックリスト
│   └── CLAUDE_CODE_TASKS.md      # Claude Codeへの実装指示書
└── crates/
    ├── wm-core/                  # no_std コアロジック
    ├── wm-sources/               # std: API取得層
    ├── wm-tui/                   # std: Ratatui TUI
    └── wm-esp32/                 # no_std: ESP32 (スケルトンのみ)
```

### 依存方向（一方向のみ）
```
wm-tui    ──> wm-sources ──> wm-core
wm-esp32  ──────────────────> wm-core
```
`wm-core` は他のどのクレートにも依存しない。`wm-sources` は `wm-tui` を知らない。

---

## 2. wm-core 詳細

### 2.1 モジュール構成
```
wm-core/src/
├── lib.rs            # #![no_std], feature gates, re-export
├── model.rs          # WeatherSnapshot, Measurement, SourceId, WeatherCode
├── grid.rs           # Grid<T>: 雨量/雲量の2次元格子（抽象データ）
├── geo.rs            # 座標変換: lat/lon ↔ Web Mercator tile ↔ pixel
├── agg/
│   ├── mod.rs        # 集約エントリポイント aggregate()
│   ├── weight.rs     # 静的重み + 新鮮度減衰
│   └── outlier.rs    # z-score外れ値検出, CV計算
└── render/
    ├── mod.rs        # 描画コマンド型 DrawCell の定義
    ├── braille.rs    # Grid → Braille点字セル列への量子化
    └── colormap.rs   # 雨量/雲量 → Rgb の色マッピング
```

### 2.2 主要型

```rust
// model.rs
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SourceId { Jma, OpenMeteo, OpenWeatherMap }

#[derive(Clone, Copy, Debug)]
pub struct Measurement {
    pub source: SourceId,
    pub value: f32,
    pub observed_at: u64,   // unix秒。wm-core外部から渡す
}

/// 1指標の集約結果
#[derive(Clone, Copy, Debug)]
pub struct Aggregated {
    pub value: f32,         // 加重平均
    pub cv: f32,            // 変動係数 σ/μ
    pub confidence: f32,    // 0.0..=1.0
    pub n_used: u8,         // 集約に使われたソース数
    pub n_excluded: u8,     // 外れ値として除外された数
}

#[derive(Clone, Copy, Debug)]
pub struct WeatherSnapshot {
    pub temp_c: Aggregated,
    pub humidity_pct: Aggregated,
    pub wind_ms: Aggregated,
    pub wind_dir_deg: Aggregated,
    pub precip_mmh: Aggregated,
    pub condition: WeatherCode,   // 多数決
    pub generated_at: u64,
}
```

### 2.3 Grid：移植性の要

雨雲レーダーも雲量も「2次元の数値格子」に正規化してから `wm-core` に渡す。これが移植性の肝。

```rust
// grid.rs — alloc不要、固定容量
pub const GRID_MAX_W: usize = 256;
pub const GRID_MAX_H: usize = 256;

pub struct Grid {
    pub width: u16,
    pub height: u16,
    /// 行優先。値の意味は GridKind で決まる
    data: heapless::Vec<f32, { GRID_MAX_W * GRID_MAX_H }>,
    pub kind: GridKind,
    /// この格子が覆う地理範囲（色や座標対応に使う）
    pub bbox: GeoBBox,
}

#[derive(Clone, Copy)]
pub enum GridKind {
    PrecipMmH,    // 雨量 mm/h（JMAナウキャストPNG由来 or 数値）
    CloudPct,     // 雲量 %（Open-Meteo cloud_cover由来）
}
```

> **注**: `GRID_MAX_W * GRID_MAX_H = 65536` 要素 × 4byte = 256KB。ESP32-C3 のRAMは約400KBなので、ESP32では実際にはもっと小さいグリッド（例: 64×64）を使う。型としては最大値で確保せず、ESP32向けには別の小さい const generic 版を用意するか、feature で `GRID_MAX_*` を切り替える。→ PORTABILITY.md 参照。

### 2.4 描画コマンド：UI非依存

`wm-core` は「画面にどう描くか」を知らない。代わりに**描画コマンドの列**を出力し、各プラットフォームがそれを解釈する。

```rust
// render/mod.rs
#[derive(Clone, Copy)]
pub struct Rgb { pub r: u8, pub g: u8, pub b: u8 }

/// 1つのBrailleセル（端末の1文字ぶん = 2x4ドット）
#[derive(Clone, Copy)]
pub struct DrawCell {
    pub col: u16,
    pub row: u16,
    pub braille: char,    // U+2800..=U+28FF
    pub fg: Rgb,          // truecolor前景
}
```

PC側（wm-tui）は `DrawCell` を crossterm の `SetForegroundColor` + 文字出力に変換。ESP32側は同じ `braille`/`fg` を `embedded-graphics` のピクセル描画に変換。**`wm-core` は両者を区別しない。**

### 2.5 Braille量子化アルゴリズム

```
入力: Grid (雨量 or 雲量), 表示領域のセル数 (cols × rows)
出力: heapless::Vec<DrawCell>

1セル = 横2 × 縦4 ドット = 8ドット。各ドットは Grid 上の対応サンプル点。
各ドットについて「描画する閾値を超えるか」を判定し、対応するビットを立てる。
Braille文字 = U+2800 + ビットパターン。
前景色 = セル内の代表値（最大 or 平均）を colormap で RGB 化。
```

Brailleドットのビット配置（Unicode規格）:
```
ドット番号 → ビット
(0,0)=0x01  (1,0)=0x08
(0,1)=0x02  (1,1)=0x10
(0,2)=0x04  (1,2)=0x20
(0,3)=0x40  (1,3)=0x80
```

### 2.6 色マッピング（truecolor雨量色）

JMAの雨量レーダー配色を再現する。雨量(mm/h)→RGBのルックアップ。

```rust
// colormap.rs — JMA高解像度降水ナウキャスト準拠の段階配色
// 0.1未満: 透明（描画しない）
// 0.1-1 : 薄水色  (242,242,255)
// 1-5   : 水色    (160,210,255)
// 5-10  : 青      ( 33,140,255)
// 10-20 : 黄緑→緑 (  0,200,  0) 付近
// 20-30 : 黄      (250,245,  0)
// 30-50 : 橙      (255,153,  0)
// 50-80 : 赤      (255,  0,  0)
// 80+   : 紫      (180,  0,180)
```

雲量(%)用には別のグレースケール/白系ランプを用意。

---

## 3. wm-sources 詳細（std層）

### 3.1 責務
- 各気象APIへのHTTPリクエスト（`reqwest` + `tokio`）
- JSONパース（`serde`）
- JMAナウキャストPNGタイルのデコード（`image` crate）とGrid変換
- 取得した生データを `wm-core` の `Measurement` / `Grid` に変換して返す

### 3.2 プロバイダ trait

```rust
#[async_trait]
pub trait WeatherProvider {
    fn id(&self) -> SourceId;
    async fn fetch_point(&self, lat: f64, lon: f64) -> Result<Vec<Measurement>>;
}

pub trait RadarProvider {
    async fn fetch_radar(&self, bbox: GeoBBox, zoom: u8) -> Result<Grid>;
}
```

### 3.3 各プロバイダ

| ファイル | API | 認証 | 役割 |
|---|---|---|---|
| `providers/jma.rs` | 気象庁 forecast JSON + nowcast PNG tile | 不要 | 基準値 + 雨雲レーダー |
| `providers/open_meteo.rs` | Open-Meteo forecast (model=jma_seamless) | 不要 | 第2予報源 + cloud_cover |
| `providers/owm.rs` | OpenWeatherMap Current Weather | APIキー | 第3予報源（欧州モデル） |

### 3.4 JMAナウキャスト雨雲タイル取得（雲レンダーの中核）

```
ベースURL（高解像度降水ナウキャスト）:
https://www.jma.go.jp/bosai/jmatile/data/nowc/{basetime}/none/{validtime}/surf/hrpns/{z}/{x}/{y}.png

手順:
1. https://www.jma.go.jp/bosai/jmatile/data/nowc/targetTimes_N1.json
   を取得し、最新の basetime / validtime を得る
2. 表示中の bbox + zoom から必要なタイル範囲 (x,y) を計算（geo.rs使用）
3. 各PNGタイル(256x256)を取得・デコード
4. JMAの配色をピクセルから逆引き → 雨量レベルへ量子化 → Grid に書き込む
5. Grid を wm-core のBraille量子化へ渡す
```

PNGの各ピクセル色は既知のJMA配色テーブルと照合して雨量レベルを復元する（色→値の逆マップ）。

### 3.5 雲データのハイブリッド戦略
- **第1選択（レーダー）**: JMAナウキャストPNG → `PrecipMmH` Grid。降水の実況。
- **フォールバック（雲量）**: JMA取得失敗時、または将来のESP32では Open-Meteo `cloud_cover` → `CloudPct` Grid。
- 両者とも最終的に同じ `Grid` 型 → 同じ `wm-core::render` ロジックを通る。

---

## 4. wm-tui 詳細（std層・Ratatui）

### 4.1 レイアウト
```
┌─ weathermap ────────────────────────┬──────────────────────┐
│                                     │ 位置 35.68N 139.69E   │
│         Braille地図 + 雨雲           │ 気温  23.4C ±0.6      │
│         (truecolor)                 │ 湿度  68%             │
│                                     │ 風   2.8m/s NW        │
│                                     ├──────────────────────┤
│                                     │ ソース比較 CV:2.6%    │
│                                     │ JMA  ███ 23.1C        │
│                                     │ O-M  ███ 23.6C        │
│                                     │ OWM  ███ 23.5C        │
│  [↑↓←→]パン [+/-]ズーム [r]更新      ├──────────────────────┤
│                                     │ 雨量 凡例 (truecolor) │
└─────────────────────────────────────┴──────────────────────┘
```

### 4.2 主要モジュール
```
wm-tui/src/
├── main.rs          # tokio::main, 端末セットアップ, イベントループ
├── app.rs           # App状態: 中心座標, zoom, 最新Snapshot, 最新Grid
├── config.rs        # 設定読み込み（起動位置, APIキー, 更新間隔）
├── input.rs         # キー入力 → App状態更新（パン/ズーム/更新）
└── ui/
    ├── mod.rs       # 全体レイアウト（ratatui Layout）
    ├── map.rs       # wm-core::render の DrawCell を Ratatui Buffer へ
    ├── sidebar.rs   # 天気数値 + ソース比較バー
    └── legend.rs    # 雨量色凡例
```

### 4.3 truecolor描画の要点
- 端末がtruecolor対応か確認（`COLORTERM=truecolor`）。非対応時はANSI256へフォールバック。
- `wm-core` が返す `DrawCell { braille, fg }` を `ratatui::buffer::Cell` に設定。`fg` は `Color::Rgb(r,g,b)`。
- 地図ベース層（OSMタイル輪郭 or 単色背景）の上に雨雲セルを重畳。

### 4.4 設定ファイル例（起動位置は設定、操作はインタラクティブ）
```toml
# ~/.config/weathermap/config.toml
[startup]
lat = 35.681
lon = 139.767
zoom = 8

[sources]
owm_api_key = "..."        # OpenWeatherMap のみキー要

[refresh]
weather_secs = 600         # 10分
radar_secs = 300           # 5分（ナウキャストは5分更新）
```

---

## 5. wm-esp32（将来・本リポジトリではスケルトンのみ）

実装はしないが、`wm-core` がそのまま載ることを示すスケルトンと方針コメントだけ置く。

- ターゲット: `riscv32imc-unknown-none-elf`（ESP32-C3）
- HAL: `esp-hal`、WiFi: `esp-wifi`
- データ源: Open-Meteo `cloud_cover`（PNG読まない、JSON最小パース `serde-json-core`）
- 描画: `embedded-graphics` で SSD1306/TFT に `wm-core` の `DrawCell` を変換
- **`wm-core` のコードは1行も変更しない。** feature flag で `alloc` off, 小さいGRID_MAX。

詳細は PORTABILITY.md。

---

## 6. 実装フェーズ順序

| Phase | 内容 | 成果 |
|---|---|---|
| P0 | workspace + wm-core 型定義 | コンパイル通る骨格 |
| P1 | wm-core 集約ロジック + 単体テスト | アルゴリズム検証 |
| P2 | wm-core Braille量子化 + colormap + テスト | 描画ロジック検証 |
| P3 | wm-sources JMA forecast JSON のみ | 数値が取れる |
| P4 | wm-tui サイドバーのみ（地図なし） | 天気が見える |
| P5 | Open-Meteo + OWM 追加 → 集約表示 | CV表示が動く |
| P6 | JMAナウキャストPNG → Grid → Braille地図 | 雨雲レーダー表示 |
| P7 | パン/ズーム/更新のインタラクション | 完成 |
| P8 | (将来) wm-esp32 移植 | 持ち運び |

---

## 7. テスト戦略

- **wm-core は std テストでカバー**（`#[cfg(test)]` 内は std 可）。集約・Braille・色マップは純粋関数なので決定的にテスト可能。
- 集約: 既知の入力ベクトル（例: 23.1/23.4/28.0）→ 期待CV・外れ値除外を assert。
- Braille: 既知グリッド → 期待Braille文字列を assert。
- geo: 既知 lat/lon → 既知タイル座標（OSM wiki の検証値）を assert。
