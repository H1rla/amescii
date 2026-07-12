//! # wm-esp32（スケルトン）
//!
//! ESP32-C3 (RISC-V, riscv32imc-unknown-none-elf) への移植用の骨格。
//! **目的は「wm-core を 1 行も変えずに載せられる」ことの提示。**
//!
//! 本ファイルは実機 HAL を呼ばない疑似コードに近いスケルトン。実装時に
//! esp-hal / esp-wifi / embedded-graphics を有効化して肉付けする。
//! docs/PORTABILITY.md の手順に従うこと。
//!
//! データフロー（PC版と同じ wm-core を通る）:
//! ```text
//! WiFi → Open-Meteo /v1/forecast?hourly=cloud_cover,...
//!      → serde-json-core で最小パース（ヒープ無し）
//!      → wm-core::Grid { CloudPct, 64x64 } を構築
//!      → wm-core::render::braille::quantize(&grid, cols, rows)
//!      → embedded-graphics で液晶へ DrawCell を描画
//! 数値天気も wm-core::aggregate で集約（PNG 不要なのでそのまま動く）。
//! ```

#![no_std]
#![no_main]

// 実機では panic ハンドラとエントリポイントマクロを使う:
// use esp_backtrace as _;
// use esp_hal::{prelude::*, ...};

// wm-core は no_std。embedded feature で小さいグリッド。
use wm_core::geo::GeoBBox;
use wm_core::grid::{Grid, GridKind};
use wm_core::render::braille::quantize;

/// 実機では #[entry]。ここではシグネチャだけ示す。
// #[entry]
fn _main_skeleton() -> ! {
    // 1. HAL 初期化（クロック・WiFi・SPI 液晶）。実装時に記述。
    // 2. WiFi 接続。
    // 3. Open-Meteo から cloud_cover を取得し Grid を作る。
    let bbox = GeoBBox::japan();
    let mut grid = Grid::new_zeroed(64, 64, GridKind::CloudPct, bbox)
        .expect("64x64 fits in embedded GRID_CAP");

    // ダミー：実機では HTTP レスポンスから cloud_cover を書き込む。
    let _ = grid.set(10, 10, 80.0);

    // 4. wm-core で量子化（PC とまったく同じ呼び出し）。
    let _cells = quantize(&grid, 32, 16);

    // 5. embedded-graphics で _cells を液晶に描画（実装時）。

    loop {
        // 6. 定期的に再取得・再描画。
        // delay.delay_ms(300_000u32); など。
        cortex_loop();
    }
}

#[inline(never)]
fn cortex_loop() {
    // プレースホルダ。実機では割り込み待ち or ディレイ。
}

// スケルトンを通常ホストでビルドした場合のダミー main（no_main を満たさないため
// 実際にはターゲットビルドのみを想定）。ホストでは何もしない。
#[cfg(not(target_os = "none"))]
fn main() {}
