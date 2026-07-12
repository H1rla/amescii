# 移植性チェックリスト (PORTABILITY.md)

`wm-core` を ESP32-C3 (RISC-V, `riscv32imc-unknown-none-elf`) へ無改変で載せるための設計規約と移植手順。

---

## 1. なぜESP32-C3か

| 観点 | 理由 |
|---|---|
| ISA | RISC-V (RV32IMC)。あなたのRISC-V研究と地続き。 |
| Rust | `riscv32imc-unknown-none-elf` の Tier 2 サポート。`esp-hal` が成熟。 |
| 無線 | WiFi内蔵で単体でAPI取得可能。 |
| 価格/入手 | DevKitM-1 が安価。M5StickC系なら液晶・電池一体で持ち運び向き。 |

代替: M5StickC Plus2（ESP32 Xtensaだが液晶＋電池一体）。ISA親和性を優先するならC3、即持ち運びを優先するならM5Stick。

---

## 2. wm-core が踏んではいけない地雷（禁止リスト）

これらが `wm-core` に紛れ込むと移植が壊れる。CIの `no_std` ビルドで機械的に検出する。

| 禁止 | 理由 | 代替 |
|---|---|---|
| `use std::*` | no_std環境に無い | `core::*` |
| `Vec`, `String`, `Box`（alloc無し時） | ヒープ前提 | `heapless::Vec`, `heapless::String` |
| `f32::sqrt/sin/cos/exp/atan2` | これらは std のメソッド | `libm::sqrtf` 等 |
| `std::time::*`, `Instant::now()` | OS時刻に依存 | `now: u64` を引数で渡す |
| `println!`, `eprintln!` | 標準出力に依存 | `defmt`（ESP側）/ caller側でログ |
| `std::fs`, `std::net` | OS資源 | caller（wm-sources）の責務 |
| `thread`, `Mutex`（std版） | OSスレッド | 不要な設計にする / `critical-section` |
| panicでの巨大フォーマット | コードサイズ増 | `panic-halt` + 最小メッセージ |

---

## 3. feature flag 設計

```toml
# wm-core/Cargo.toml
[features]
default = ["alloc"]
# PC版（wm-tui/wm-sources）: alloc on
alloc = []
# ESP32版: alloc off（または小heap）, 小さいグリッド
embedded = []
# テスト時のみ std を許可（純粋関数のテスト用）
std = []

[dependencies]
heapless = "0.8"
libm = "0.2"
serde = { version = "1", default-features = false, features = ["derive"], optional = true }
```

### グリッドサイズの切り替え
```rust
// grid.rs
#[cfg(not(feature = "embedded"))]
pub const GRID_MAX_W: usize = 256;
#[cfg(not(feature = "embedded"))]
pub const GRID_MAX_H: usize = 256;

#[cfg(feature = "embedded")]
pub const GRID_MAX_W: usize = 64;   // ESP32: RAM節約
#[cfg(feature = "embedded")]
pub const GRID_MAX_H: usize = 64;
```

PC: 256×256×4B = 256KB は問題なし。
ESP32-C3 (~400KB RAM): 64×64×4B = 16KB に抑える。

---

## 4. CIで移植性を保証する

`wm-core` を**実際にRISC-Vターゲットでビルド**するCIジョブを置く。これが緑なら移植性は保たれている。

```bash
# .github/workflows などで
rustup target add riscv32imc-unknown-none-elf
cargo build -p wm-core \
  --no-default-features --features embedded \
  --target riscv32imc-unknown-none-elf
```

`std` を1つでも踏むとこのビルドが赤くなる → 移植性の自動ガード。

さらに `#![no_std]` を強制:
```rust
// wm-core/src/lib.rs
#![cfg_attr(not(feature = "std"), no_std)]
```

---

## 5. データ源の差し替え（PC ↔ ESP32）

`wm-core` は `Grid` を受け取るだけ。`Grid` の作り方だけが変わる。

```
PC (wm-sources):
   JMAナウキャストPNG (256x256タイル複数)
      → image crateでデコード
      → JMA配色を逆引きして雨量レベル
      → Grid { kind: PrecipMmH, 256x256 }

ESP32 (wm-esp32):
   Open-Meteo /v1/forecast?hourly=cloud_cover&...
      → serde-json-core で最小パース（ヒープ無し）
      → 緯度経度グリッド点の cloud_cover を読む
      → Grid { kind: CloudPct, 64x64 }

両者とも → wm-core::render::braille::quantize(&grid, ...) → Vec<DrawCell>
                                          ↑ 同じ関数
```

PNGデコードは重い（メモリ・CPU）のでESP32ではやらない。数値の `cloud_cover` なら軽量。**雨量(PrecipMmH)と雲量(CloudPct)で色マップは変わるが、Braille量子化ロジックは共通。**

---

## 6. 描画バックエンドの差し替え

`wm-core::render::DrawCell { col, row, braille, fg: Rgb }` を各々が解釈:

```
PC (wm-tui):
   DrawCell → ratatui::buffer::Cell
              .set_char(braille)
              .set_fg(Color::Rgb(fg.r, fg.g, fg.b))

ESP32 (wm-esp32):
   DrawCell → embedded-graphics
              Brailleパターンを2x4ピクセルとして描く
              or Brailleフォントグリフを Text で描画
              色は液晶のRGB565へ変換
```

`wm-core` はどちらも知らない。`DrawCell` という共通中間表現だけが境界。

---

## 7. 移植手順（将来、実際にやるとき）

```
1. ハード入手: ESP32-C3 DevKitM-1（+ SSD1306 or 内蔵液晶ボード）
2. ツールチェイン:
     rustup target add riscv32imc-unknown-none-elf
     cargo install espflash
3. wm-esp32 を esp-hal テンプレートから作成
     依存: esp-hal, esp-wifi, embedded-graphics, serde-json-core
     wm-core を path 依存で追加（--features embedded, no alloc）
4. WiFi接続 → Open-Meteo に GET → cloud_cover を Grid 化
5. wm-core::aggregate で数値集約（PNGなし、数値のみなのでそのまま動く）
6. wm-core::render で DrawCell 生成
7. embedded-graphics で液晶に描画
8. espflash で書き込み
※ この間 wm-core は一切変更しない。変更が必要になったら設計違反。
```

---

## 8. 移植性の最終チェック（受け入れ基準）

- [ ] `cargo build -p wm-core --no-default-features --features embedded --target riscv32imc-unknown-none-elf` が通る
- [ ] `wm-core` の `grep -r "std::" src/` が空（テストモジュール除く）
- [ ] `wm-core` 内の数学が全て `libm::` 経由
- [ ] 時刻は全て引数 `now: u64`、内部取得ゼロ
- [ ] 動的確保が `#[cfg(feature = "alloc")]` でゲートされている
- [ ] `wm-core` の単体テストが std 環境で全て緑（純粋関数の検証）
