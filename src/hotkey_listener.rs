//! IME 非依存のグローバルキー入力 (Linux evdev)。
//!
//! X11/Wayland では IME (ibus 等) が全角モード時にキーを XIM/XFilterEvent 層で
//! 横取りし、winit/egui の `Key` イベントが発火しない。そこで `/dev/input/event*`
//! をカーネル層で直接読み取り、IME より下の層で物理キー押下を捕捉する。
//!
//! 責務分離 (SOLID):
//! - 純粋ロジック (`map_key` / `modifier_kind` / `HotkeyResolver`): デバイス I/O に
//!   依存せず、モックイベント列で単体テストできる。
//! - 薄い I/O 層 (`spawn_keyboard_listener`): デバイス列挙と blocking 読み取りだけを
//!   担い、捕捉した押下を `RawHotkey` として channel へ流す。
//!
//! フォーカス判定と実アクションは呼び出し側 (app.rs) が行う。アプリにフォーカスが
//! ある時のみ発火させることで、グローバル誤爆を防ぐ (スコープ「アプリ使用中のみ」)。
//!
//! 暗黙 fallback 禁止: キーボードを 1 台も開けない場合は明示的に `Err` を返し、
//! 呼び出し側がログを残した上で egui イベント経路へ切り替える。

use anyhow::{bail, Context, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui;
use evdev::{Device, EventSummary, KeyCode};
use std::thread;

/// evdev で捕捉した物理キー押下を egui の論理キー + 修飾キーへ正規化したもの。
/// これにより app.rs 側は既存の `HotkeySet::trigger` / `is_quit_key` をそのまま
/// 再利用でき、egui イベント経路と evdev 経路でショートカット判定を共通化できる (DRY)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawHotkey {
    pub key: egui::Key,
    pub modifiers: egui::Modifiers,
}

/// evdev 読み取りスレッド群への入り口。`receiver` をドロップすると各スレッドは
/// `send` 失敗を検知して終了する (明示的なライフサイクル)。
pub struct KeyboardListener {
    pub receiver: Receiver<RawHotkey>,
    pub device_names: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
enum Modifier {
    Ctrl,
    Alt,
    Shift,
}

/// 修飾キーの押下状態を保持し、非修飾キーの「最初の押下」だけを `RawHotkey` に
/// 変換する純粋なステートマシン。デバイス I/O を持たないため単体テスト可能。
#[derive(Debug, Default)]
struct HotkeyResolver {
    ctrl: bool,
    alt: bool,
    shift: bool,
}

impl HotkeyResolver {
    /// evdev のキーイベントを 1 件処理する。
    /// `value`: 0=release, 1=press, 2=autorepeat (Linux input event の慣例)。
    ///
    /// - 修飾キー: press/release で状態を更新し、`None` を返す。
    /// - 非修飾キー: 最初の押下 (value==1) かつ対応する egui キーがある場合のみ
    ///   `RawHotkey` を返す。release / autorepeat は無視する (egui の repeat=false 相当)。
    fn on_key_event(&mut self, code: KeyCode, value: i32) -> Option<RawHotkey> {
        if let Some(modifier) = modifier_kind(code) {
            match value {
                1 => self.set_modifier(modifier, true),
                0 => self.set_modifier(modifier, false),
                _ => {}
            }
            return None;
        }

        if value != 1 {
            return None;
        }

        let key = map_key(code)?;
        Some(RawHotkey {
            key,
            modifiers: egui::Modifiers {
                alt: self.alt,
                ctrl: self.ctrl,
                shift: self.shift,
                mac_cmd: false,
                // 非 mac では command == ctrl (egui-winit の ModifiersChanged と整合)。
                command: self.ctrl,
            },
        })
    }

    fn set_modifier(&mut self, modifier: Modifier, pressed: bool) {
        match modifier {
            Modifier::Ctrl => self.ctrl = pressed,
            Modifier::Alt => self.alt = pressed,
            Modifier::Shift => self.shift = pressed,
        }
    }
}

fn modifier_kind(code: KeyCode) -> Option<Modifier> {
    match code {
        KeyCode::KEY_LEFTCTRL | KeyCode::KEY_RIGHTCTRL => Some(Modifier::Ctrl),
        KeyCode::KEY_LEFTALT | KeyCode::KEY_RIGHTALT => Some(Modifier::Alt),
        KeyCode::KEY_LEFTSHIFT | KeyCode::KEY_RIGHTSHIFT => Some(Modifier::Shift),
        _ => None,
    }
}

/// evdev の物理キーコードを egui の論理キーへ写像する。対応範囲は
/// `crate::hotkey::KeyCode` が表現できるショートカット用キー (Space/Enter/Escape/
/// Tab/Backspace/A-Z/F1-F12) に限定する。それ以外は `None`。
fn map_key(code: KeyCode) -> Option<egui::Key> {
    use egui::Key;
    Some(match code {
        KeyCode::KEY_SPACE => Key::Space,
        KeyCode::KEY_ENTER => Key::Enter,
        KeyCode::KEY_ESC => Key::Escape,
        KeyCode::KEY_TAB => Key::Tab,
        KeyCode::KEY_BACKSPACE => Key::Backspace,
        KeyCode::KEY_A => Key::A,
        KeyCode::KEY_B => Key::B,
        KeyCode::KEY_C => Key::C,
        KeyCode::KEY_D => Key::D,
        KeyCode::KEY_E => Key::E,
        KeyCode::KEY_F => Key::F,
        KeyCode::KEY_G => Key::G,
        KeyCode::KEY_H => Key::H,
        KeyCode::KEY_I => Key::I,
        KeyCode::KEY_J => Key::J,
        KeyCode::KEY_K => Key::K,
        KeyCode::KEY_L => Key::L,
        KeyCode::KEY_M => Key::M,
        KeyCode::KEY_N => Key::N,
        KeyCode::KEY_O => Key::O,
        KeyCode::KEY_P => Key::P,
        KeyCode::KEY_Q => Key::Q,
        KeyCode::KEY_R => Key::R,
        KeyCode::KEY_S => Key::S,
        KeyCode::KEY_T => Key::T,
        KeyCode::KEY_U => Key::U,
        KeyCode::KEY_V => Key::V,
        KeyCode::KEY_W => Key::W,
        KeyCode::KEY_X => Key::X,
        KeyCode::KEY_Y => Key::Y,
        KeyCode::KEY_Z => Key::Z,
        KeyCode::KEY_F1 => Key::F1,
        KeyCode::KEY_F2 => Key::F2,
        KeyCode::KEY_F3 => Key::F3,
        KeyCode::KEY_F4 => Key::F4,
        KeyCode::KEY_F5 => Key::F5,
        KeyCode::KEY_F6 => Key::F6,
        KeyCode::KEY_F7 => Key::F7,
        KeyCode::KEY_F8 => Key::F8,
        KeyCode::KEY_F9 => Key::F9,
        KeyCode::KEY_F10 => Key::F10,
        KeyCode::KEY_F11 => Key::F11,
        KeyCode::KEY_F12 => Key::F12,
        _ => return None,
    })
}

fn is_keyboard(device: &Device) -> bool {
    device
        .supported_keys()
        .is_some_and(|keys| keys.contains(KeyCode::KEY_SPACE) && keys.contains(KeyCode::KEY_A))
}

/// `/dev/input` を走査してキーボードを開き、デバイス毎に blocking 読み取りスレッドを
/// 起動する。1 台も開けない場合 (権限不足 / デバイス無し) は `Err` を返す。
pub fn spawn_keyboard_listener() -> Result<KeyboardListener> {
    let keyboards: Vec<(std::path::PathBuf, Device)> =
        evdev::enumerate().filter(|(_, device)| is_keyboard(device)).collect();

    if keyboards.is_empty() {
        bail!(
            "readable keyboard not found under /dev/input \
             (add your user to the 'input' group: sudo usermod -aG input $USER, then re-login)"
        );
    }

    let (tx, rx) = unbounded();
    let mut device_names = Vec::with_capacity(keyboards.len());

    for (path, device) in keyboards {
        let label = device
            .name()
            .map(str::to_owned)
            .unwrap_or_else(|| path.display().to_string());
        device_names.push(label);

        let tx = tx.clone();
        let thread_name = format!("evdev-{}", path.display());
        thread::Builder::new()
            .name(thread_name)
            .spawn(move || run_device_loop(device, tx))
            .with_context(|| format!("failed to spawn evdev reader thread for {}", path.display()))?;
    }

    Ok(KeyboardListener {
        receiver: rx,
        device_names,
    })
}

/// 1 デバイスの blocking 読み取りループ。`fetch_events` 失敗 (デバイス切断等) や
/// `send` 失敗 (受信側ドロップ = アプリ終了) で終了する。
fn run_device_loop(mut device: Device, tx: Sender<RawHotkey>) {
    let mut resolver = HotkeyResolver::default();
    loop {
        let events = match device.fetch_events() {
            Ok(events) => events,
            Err(_) => return,
        };
        for event in events {
            if let EventSummary::Key(_, code, value) = event.destructure() {
                if let Some(hotkey) = resolver.on_key_event(code, value) {
                    if tx.send(hotkey).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Key;

    #[test]
    fn space_press_emits_space_without_modifiers() {
        let mut resolver = HotkeyResolver::default();
        let hotkey = resolver.on_key_event(KeyCode::KEY_SPACE, 1).expect("space press");
        assert_eq!(hotkey.key, Key::Space);
        assert_eq!(hotkey.modifiers, egui::Modifiers::default());
    }

    #[test]
    fn release_and_autorepeat_are_ignored() {
        let mut resolver = HotkeyResolver::default();
        assert_eq!(resolver.on_key_event(KeyCode::KEY_SPACE, 0), None); // release
        assert_eq!(resolver.on_key_event(KeyCode::KEY_SPACE, 2), None); // autorepeat
    }

    #[test]
    fn escape_maps_to_escape() {
        let mut resolver = HotkeyResolver::default();
        let hotkey = resolver.on_key_event(KeyCode::KEY_ESC, 1).expect("esc press");
        assert_eq!(hotkey.key, Key::Escape);
    }

    #[test]
    fn ctrl_is_tracked_and_released() {
        let mut resolver = HotkeyResolver::default();
        // Ctrl 押下は単体ではアクションを生まない。
        assert_eq!(resolver.on_key_event(KeyCode::KEY_LEFTCTRL, 1), None);
        let with_ctrl = resolver.on_key_event(KeyCode::KEY_S, 1).expect("ctrl+s");
        assert_eq!(with_ctrl.key, Key::S);
        assert!(with_ctrl.modifiers.ctrl);
        assert!(with_ctrl.modifiers.command);
        // Ctrl 解放後は修飾なしに戻る。
        assert_eq!(resolver.on_key_event(KeyCode::KEY_LEFTCTRL, 0), None);
        let without_ctrl = resolver.on_key_event(KeyCode::KEY_S, 1).expect("s");
        assert!(!without_ctrl.modifiers.ctrl);
    }

    #[test]
    fn unmapped_key_returns_none() {
        let mut resolver = HotkeyResolver::default();
        assert_eq!(resolver.on_key_event(KeyCode::KEY_CAPSLOCK, 1), None);
    }

    #[test]
    fn modifier_only_press_does_not_emit() {
        let mut resolver = HotkeyResolver::default();
        assert_eq!(resolver.on_key_event(KeyCode::KEY_LEFTSHIFT, 1), None);
        assert_eq!(resolver.on_key_event(KeyCode::KEY_RIGHTALT, 1), None);
    }
}
