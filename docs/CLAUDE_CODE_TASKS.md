# Claude Code 実装・検証タスク (CLAUDE_CODE_TASKS.md)

このリポジトリは設計書 + 主要コア実装まで書かれた骨格である。**まだ一度もコンパイルされていない**（生成環境に Rust ツールチェインが無かったため）。Claude Code 側で以下を順に実施し、コンパイル・テスト・動作確認まで持っていくこと。

---

## 0. 前提セットアップ

```bash
# Rust（未導入なら）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup default stable

# RISC-V ターゲット（移植性 CI 用）
rustup target add riscv32imc-unknown-none-elf
```

---

## 1. まず wm-core を単体で通す（最優先）

`wm-core` は純粋ロジックで、ここが緑なら土台は固い。

```bash
cargo build -p wm-core
cargo test  -p wm-core
```

### 想定される修正ポイント（既知のリスク）
1. **heapless のバージョン API**：`heapless::Vec::push` は `Result<(), T>` を返す。`0.8` 系前提。`Vec::new()` が const でない場合の初期化に注意。
2. **`heapless::Vec` の容量定数**：`grid.rs` の `Vec<f32, GRID_CAP>` で `GRID_CAP` は `const`。`const generic` がうまく通らない場合は型エイリアスで明示。
3. **`libm` 関数名**：`expf/sqrtf/cosf/sinf/atan2f/fabsf/tan/cos/sinh/atan` を使用。`f64` 版（`libm::sin` 等）と `f32` 版（`libm::sinf` 等）の取り違えに注意。`geo.rs` は f64、`agg`/`render` は f32。
4. **`#![no_std]` でのテスト**：テストモジュールは `std` を使う。`Cargo.toml` の dev-dependencies で `wm-core` 自身を `features=["std","alloc"]` で参照しているが、循環に見えて問題が出る場合は、テスト専用 feature で `extern crate std;` する方式に変更。
5. **`core::f32::consts::PI`** を使用（`agg/mod.rs`）。問題なければそのまま。
6. **`Grid` のインラインサイズに注意（重要）**：非 embedded では `heapless::Vec<f32, 65536>` を内包するため `Grid` は約 256KB をインラインで持つ。`App` が `Option<Grid>` を直接持つと `App` 値のムーブで大きなスタックコピーが発生しうる。コンパイルは通るが、実行時のスタック使用が気になる場合は `App.radar` を `Option<Box<Grid>>` にする、または `radar.rs` で `Grid` を `Box` に入れて返すと安全。embedded（64×64=16KB）では問題にならない。

### wm-core のテストが網羅している不変条件
- 集約：外れ値除外（28.0 が落ちる）、全一致で高信頼、古いデータの減衰、風向の循環平均、空入力。
- geo：z=0 単一タイル、緯度経度→タイル（東京 z8）、往復変換、BBox 範囲順序、ピクセル範囲。
- grid：生成・アクセス・範囲外・最近傍・最大値・容量超過拒否。
- render：全ゼロ→無描画、全降水→全点 Braille (U+28FF)、ビット配置、コードポイント、雲量閾値。
- colormap：無降水→None、各階級の色。

---

## 2. RISC-V ビルドで移植性を確認

```bash
cargo build -p wm-core --no-default-features --features embedded \
  --target riscv32imc-unknown-none-elf
```

これが通れば「wm-core に std が紛れていない」ことが保証される。**通らない場合は std 依存が混入しているので除去する**（PORTABILITY.md の禁止リスト参照）。

`grep -rn "std::" crates/wm-core/src/ | grep -v "#\[cfg(test)\]"` で混入を機械チェックできる。

---

## 3. wm-sources をビルド

```bash
cargo build -p wm-sources
cargo test  -p wm-sources
```

### 想定される修正ポイント
1. **`async-trait`**：trait に `#[async_trait]` を付与済み。`WeatherProvider` は `Send + Sync`。`Box<dyn WeatherProvider>` を使うので object-safe であること。
2. **`reqwest` の機能フラグ**：`json` feature 必須（ワークスペース Cargo.toml で指定済み）。
3. **`image` クレート**：`0.25` 系。`load_from_memory` / `GenericImageView::get_pixel` / `dimensions` を使用。`png` feature のみ有効化。`Rgba` のピクセルアクセスは `p.0` が `[u8;4]`。バージョン差でアクセス方法が変わる場合は調整。
4. **JMA forecast JSON の構造**：実レスポンスは深くネストし、`timeSeries` ごとに異なるキー集合を持つ。`providers/jma.rs` は必要部分のみ `Option` で拾う設計だが、実データで `temps`/`weatherCodes` が期待位置に無い場合がある。**実際に `https://www.jma.go.jp/bosai/forecast/data/forecast/130000.json` を取得して構造を確認し、パスを微調整すること**。
5. **OWM レスポンス**：`rain."1h"` は降水時のみ存在。`#[serde(rename = "1h")]` 済み。

### 注意：JMA ナウキャストの作法（重要）
`radar.rs` は **必ず targetTimes_N1.json を先に取得**してから最新 basetime/validtime でタイル URL を組む。現在時刻から推測してはならない（404 が CDN にキャッシュされ、自他に害）。この順序を崩さないこと。

---

## 4. wm-tui をビルドして実行

```bash
cargo build -p wm-tui
cargo run   -p wm-tui
```

### 想定される修正ポイント
1. **ratatui `0.28` の API**：
   - `Frame::area()`（旧 `size()`）を使用。バージョンによっては `f.size()`。エラーが出たら合わせる。
   - `Buffer` のインデックスは `buf[(x, y)]`（`0.28` で `Index<(u16,u16)>` 実装）。古い版では `buf.get_mut(x, y)`。**ここはバージョン差が出やすい**。
   - `Style`/`Color::Rgb` はそのまま使える。
2. **crossterm `0.28`**：`event::poll` / `event::read` / `KeyCode` / `KeyModifiers` を使用。
3. **truecolor**：`Color::Rgb(r,g,b)` を使用。端末が truecolor 非対応だと色が落ちる。`COLORTERM=truecolor` を確認。フォールバック（ANSI256 近似）は未実装なので、必要なら追加。
4. **マップ領域サイズ**：`ui::draw` で `app.map_cols/rows` を実領域から更新し、次回 BBox 計算に使う。初回は仮値で取得 → 描画後に正しいサイズになる。
5. **panic 時の端末復帰**：現状 `run()` 正常終了でのみ raw mode を戻す。**panic hook を入れて確実に `LeaveAlternateScreen` するよう改善するのが望ましい**（タスク 7）。

---

## 5. 結合動作の確認

実行して以下を目視確認：
- [ ] 起動して東京周辺が中心に表示される。
- [ ] サイドバーに気温などの数値と CV・信頼度が出る（少なくとも JMA + Open-Meteo の2ソース）。
- [ ] 雨が降っている地域があれば truecolor の雨雲が地図に重なる（無降水時は何も出ない＝正常）。
- [ ] 矢印キーでパンすると再取得され表示が動く。
- [ ] `+`/`-` でズームが変わる（3..=10）。
- [ ] `q` で正常終了し、端末が元に戻る。

雨雲が出ているか確認したいときは、雨天時に試すか、`radar.rs` に既知の雨域座標を一時ハードコードしてテストする。

---

## 6. 仕上げ（推奨タスク）

- [ ] **並列取得**：`fetch_and_aggregate` はプロバイダを直列取得している。`tokio::join!` か `FuturesUnordered` で並列化すると起動が速くなる。
- [ ] **非同期取得とUIの分離**：現状ループ内 `await` で取得中は UI が止まる。`tokio::sync::mpsc` で取得タスクを spawn し、結果をチャネルで受けて UI をノンブロッキングにする。
- [ ] **panic hook**：端末復帰を保証。
- [ ] **JMA area.json**：`Jma::area_for` は主要8都市の最近傍簡易版。正式には `https://www.jma.go.jp/bosai/common/const/area.json` と府県予報区の対応表を使い、任意地点で正しい区域を引く。
- [ ] **雲量フォールバック**：JMA ナウキャスト取得失敗時に Open-Meteo `cloud_cover` で `CloudPct` グリッドを作る経路（`radar.rs` 冒頭コメント参照）を実装すると堅牢。
- [ ] **OSM ベースタイル**：地図の海岸線・県境を出したい場合、地理院/OSM タイルを取得して Braille エッジ抽出する層を `map.rs` の背景に追加（重いので任意）。

---

## 7. ディレクトリ早見

```
weathermap/
├── Cargo.toml                      workspace
├── README.md
├── docs/
│   ├── DESIGN.md                   全体設計（最初に読む）
│   ├── AGGREGATION.md              集約アルゴリズム
│   ├── PORTABILITY.md              RISC-V 移植
│   └── CLAUDE_CODE_TASKS.md        本書
└── crates/
    ├── wm-core/   src/{lib,model,grid,geo}.rs, agg/{mod,weight,outlier}.rs, render/{mod,braille,colormap}.rs
    ├── wm-sources/src/{lib,error,traits,radar}.rs, providers/{mod,jma,open_meteo,owm}.rs
    ├── wm-tui/    src/{main,app,config,input}.rs, ui/{mod,map,sidebar,legend}.rs
    └── wm-esp32/  src/main.rs（スケルトン・通常ビルド対象外）
```

---

## 8. コンパイルを通す順序のまとめ

1. `cargo test -p wm-core`（純粋ロジック、ここを最初に緑へ）
2. `cargo build -p wm-core --no-default-features --features embedded --target riscv32imc-unknown-none-elf`（移植性ガード）
3. `cargo test -p wm-sources`（API 変換、JMA JSON 構造は実データで確認）
4. `cargo run -p wm-tui`（結合、ratatui 0.28 の Buffer/area API 差に注意）

各段階でエラーを潰してから次へ進むこと。最大の不確実性は **(a) ratatui 0.28 の Buffer インデックス API** と **(b) JMA forecast JSON の実構造** の2点。
