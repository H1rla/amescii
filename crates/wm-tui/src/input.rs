//! キー入力 → App 状態更新。

use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// キー入力を解釈し、再取得が必要なら true を返す。
pub enum InputAction {
    None,
    /// 天気・レーダーの再取得が必要（パン/ズーム/手動更新）。
    Refetch,
    /// 現在の中心座標・ズームを設定ファイルへ保存する。
    SaveConfig,
    Quit,
}

pub fn handle_key(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        KeyCode::Char('q') => InputAction::Quit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => InputAction::Quit,

        // パン（表示幅に対する割合。ズームに応じて自動調整）。
        KeyCode::Up | KeyCode::Char('k') => {
            let f = app.pan_fraction();
            app.pan(f, 0.0);
            InputAction::Refetch
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let f = app.pan_fraction();
            app.pan(-f, 0.0);
            InputAction::Refetch
        }
        KeyCode::Left | KeyCode::Char('h') => {
            let f = app.pan_fraction();
            app.pan(0.0, -f);
            InputAction::Refetch
        }
        KeyCode::Right | KeyCode::Char('l') => {
            let f = app.pan_fraction();
            app.pan(0.0, f);
            InputAction::Refetch
        }

        // ズーム。MapSCII 互換で a=拡大 / z=縮小。+/- も後方互換で受ける。
        KeyCode::Char('a') | KeyCode::Char('+') | KeyCode::Char('=') => {
            app.zoom_in();
            InputAction::Refetch
        }
        KeyCode::Char('z') | KeyCode::Char('-') | KeyCode::Char('_') => {
            app.zoom_out();
            InputAction::Refetch
        }

        // タイムライン再生トグル（Space）。再取得は不要、次フレームで反映。
        KeyCode::Char(' ') => {
            app.toggle_play();
            InputAction::None
        }
        // コマ送り（案A）：パンと衝突しないよう `,`/`.`・`[`/`]` を使う。
        // 矢印←→と hjkl はパン専用のまま。コマ送り時は再生を止める（step_frame 内）。
        KeyCode::Char('.') | KeyCode::Char(']') => {
            app.step_frame(1);
            InputAction::None
        }
        KeyCode::Char(',') | KeyCode::Char('[') => {
            app.step_frame(-1);
            InputAction::None
        }

        // 雨雲表示トグル（機能B）。取得は継続、描画のみ切替なので再取得不要。
        KeyCode::Char('t') => {
            app.toggle_radar();
            InputAction::None
        }

        // 現在位置をデフォルト起動位置として設定ファイルへ保存。
        KeyCode::Char('s') => InputAction::SaveConfig,

        // 手動更新。
        KeyCode::Char('r') => InputAction::Refetch,

        _ => InputAction::None,
    }
}
