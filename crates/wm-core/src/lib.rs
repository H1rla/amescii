//! # wm-core
//!
//! ameSCII のプラットフォーム非依存コア。`#![no_std]`。
//!
//! このクレートは「すでに取得・デコード済みの抽象データ」だけを受け取り、
//! 集約・量子化・色変換・座標変換を行う純粋ロジックの集合である。
//! HTTP・PNGデコード・現在時刻取得・画面描画は **一切持たない**。
//! それらはすべて呼び出し側（wm-sources / wm-tui / wm-esp32）の責務。
//!
//! この規約により、RISC-V (ESP32-C3) 移植時に本クレートを無改変で再利用できる。
//! 詳細は docs/PORTABILITY.md を参照。

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod agg;
pub mod geo;
pub mod grid;
pub mod model;
pub mod places;
pub mod render;

// 主要型の re-export
pub use agg::{aggregate, AggParams, Aggregated};
pub use geo::{GeoBBox, TileCoord};
pub use grid::{Grid, GridKind};
pub use model::{Measurement, SourceId, WeatherCode, WeatherSnapshot};
pub use render::{colormap, lonlat_to_cell, rasterize_lines, DrawCell, LineCellVec, PolyLine, Rgb};
