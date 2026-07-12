//! キー入力 → App 状態更新。

use crate::app::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// キー入力を解釈し、再取得が必要なら true を返す。
pub enum InputAction {
    None,
    /// 天気・レーダーの再取得が必要（パン/ズーム/手動更新）。
    Refetch,
    Quit,
}

pub fn handle_key(app: &mut App, key: KeyEvent) -> InputAction {
    match key.code {
        KeyCode::Char('q') => InputAction::Quit,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => InputAction::Quit,

        // パン（表示幅の 20% ずつ）。
        KeyCode::Up | KeyCode::Char('k') => {
            app.pan(0.2, 0.0);
            InputAction::Refetch
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.pan(-0.2, 0.0);
            InputAction::Refetch
        }
        KeyCode::Left | KeyCode::Char('h') => {
            app.pan(0.0, -0.2);
            InputAction::Refetch
        }
        KeyCode::Right | KeyCode::Char('l') => {
            app.pan(0.0, 0.2);
            InputAction::Refetch
        }

        // ズーム。
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.zoom_in();
            InputAction::Refetch
        }
        KeyCode::Char('-') | KeyCode::Char('_') => {
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

        // 手動更新。
        KeyCode::Char('r') => InputAction::Refetch,

        _ => InputAction::None,
    }
}
