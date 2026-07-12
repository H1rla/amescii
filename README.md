# amescii

ターミナル上で日本地図を Braille 点字で描画し、複数の気象 API を**信頼度つきで集約**した天気と、**JMA 雨雲レーダーの truecolor 重畳**を表示する Rust 製アプリ。将来的に ESP32 (RISC-V) へ無改変コアで移植できる構成。

```
┌─ amescii ────────────────────────┬──────────────────┐
│  ⠠⠤⡀  ⢀⡠⠊      Braille 地図        │ 位置 35.68N 139.69E│
│ ⡜  ⠘⢆⡰⠁   + truecolor 雨雲         │ 気温 23.4°C ±0.6  │
│ ⠧⣀⡠⠊⠢⡀                            │ 風  2.8m/s 北西    │
│                                    │ 一致 CV 2.6%      │
│ [↑↓←→]パン [+/-]ズーム [r]更新 [q]  │ 信頼 87% 使用3/除外0│
└─────────────────────────────────────┴──────────────────┘
```

## 特徴

- **複数ソース集約**：気象庁 (JMA) / Open-Meteo / OpenWeatherMap の同一指標を、静的信頼度 × 新鮮度で加重平均。z-score で外れ値を自動除外し、変動係数 (CV) で「サイト間の予報の食い違い」を定量化。
- **truecolor 雨雲レーダー**：JMA 高解像度降水ナウキャストの PNG タイルをピクセル解析し、JMA の雨量配色をそのまま端末に再現。
- **Braille レンダリング**：1文字 = 2×4 ドットで地図と降水分布を高密度表示。
- **移植性ファースト**：コアロジック (`wm-core`) は `#![no_std]`。RISC-V (ESP32-C3) へ無改変で載せられる。

## 構成

```
crates/
├── wm-core/     no_std コア（集約・量子化・色・座標）。移植時に無改変。
├── wm-sources/  std: API 取得（JMA/Open-Meteo/OWM）+ ナウキャスト PNG → Grid。
├── wm-tui/      std: Ratatui TUI。バイナリ名 amescii。
└── wm-esp32/    将来の ESP32 移植スケルトン（通常ビルド対象外）。
```

依存方向は一方向：`wm-tui → wm-sources → wm-core`。詳細は [docs/DESIGN.md](docs/DESIGN.md)。

## ビルドと実行

```bash
cargo run -p wm-tui --release
# または
cargo build --release && ./target/release/amescii
```

リリースビルドはサイズ最適化（`opt-level="z"` + LTO + `codegen-units=1` + strip）済み。
**最適化前 5.39 MB → 後 2.26 MB（約 -60%）**。端末復帰の安全のため `panic="abort"` は使わない
（panic hook で raw mode / 代替画面を確実に戻すため）。

タイル（地図ベクトル・雨雲）はセッション中メモリにキャッシュされ、一度見た範囲へ戻る
操作やタイムラインのコマ往復で再取得しない。雨雲はメモリのみ（古いフレームを掴まないよう
ディスクには残さない）。

## 設定

`~/.config/amescii/config.toml`（無ければ東京・デフォルト値で起動）:

```toml
[startup]
lat = 35.681
lon = 139.767
zoom = 8           # 3..=16（地図は z16 まで精細化。雨雲は z10 上限でオーバーズーム）

[sources]
owm_api_key = ""   # OpenWeatherMap のみキー必要。空なら JMA + Open-Meteo の2ソース。

[refresh]
weather_secs = 600 # 天気更新間隔
radar_secs = 300   # 雨雲更新間隔（ナウキャストは5分更新）
```

起動位置は設定ファイル、その後はキー操作（パン／ズーム）でインタラクティブに移動。

## 操作

| キー | 動作 |
|---|---|
| `↑↓←→` / `hjkl` | パン（ズームに応じて移動幅が自動調整） |
| `a` / `z`（`+`/`-` も可） | ズーム |
| `space` | 雨雲タイムライン 再生 / 一時停止 |
| `.` / `]` | 1 コマ進める（手動・再生停止） |
| `,` / `[` | 1 コマ戻す（手動・再生停止） |
| `t` | 雨雲レイヤ 表示 / 非表示（OFF で地図＋地名のみ） |
| `r` | 手動更新 |
| `q` / `Ctrl-C` | 終了 |

ズームは `a`/`z`（`+`/`-` も可）で **z3〜z16**。**地図ベース層は z16 まで精細化**（道路・街区が細かく
なる）。**雨雲は z10 が配信上限**のため、z11 以上では z10 タイルをオーバーズーム（引き伸ばし）
して粗く表示する（`t` で消せる）。

地名ラベルはズームで切り替わる：

- **広域（z3〜z10）**：内蔵の主要都市テーブル（県庁所在地＋三大都市）を英語表記で、
  ズームに応じて重要度順に表示（拡大すると表示都市が絞り込まれる）。
- **拡大（z11〜z16）**：国土地理院 `label` レイヤの**日本語地名**（駅・町丁目・施設名）を
  表示。1画面あたり上限を設けて間引く。広域=英語／拡大=日本語で用途が分かれる。

## ドキュメント

- [docs/DESIGN.md](docs/DESIGN.md) — 全体設計
- [docs/AGGREGATION.md](docs/AGGREGATION.md) — 集約アルゴリズム詳細
- [docs/PORTABILITY.md](docs/PORTABILITY.md) — RISC-V 移植チェックリスト
- [docs/CLAUDE_CODE_TASKS.md](docs/CLAUDE_CODE_TASKS.md) — 実装・検証タスク

## データ出典

- 気象データ © 気象庁 (Japan Meteorological Agency)
- 雨雲レーダー：気象庁 高解像度降水ナウキャスト
- 地図ベース層：国土地理院ベクトルタイル（experimental_bvmap）「国土地理院ベクトルタイル提供実験」
- Open-Meteo (CC BY 4.0)
- OpenWeatherMap

## ライセンス

MIT
