use anyhow::{anyhow, bail, Result};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Toggle,
    Start,
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HotkeySet {
    pub toggle: Option<HotkeyBinding>,
    pub start: Option<HotkeyBinding>,
    pub stop: Option<HotkeyBinding>,
}

impl HotkeySet {
    pub fn from_strings(
        toggle: Option<&str>,
        start: Option<&str>,
        stop: Option<&str>,
    ) -> Result<Self> {
        let toggle = match toggle {
            Some(value) if !value.trim().is_empty() => Some(value.parse()?),
            _ => None,
        };
        let start = match start {
            Some(value) if !value.trim().is_empty() => Some(value.parse()?),
            _ => None,
        };
        let stop = match stop {
            Some(value) if !value.trim().is_empty() => Some(value.parse()?),
            _ => None,
        };

        if toggle.is_some() && (start.is_some() || stop.is_some()) {
            bail!("toggle hotkey and start/stop hotkeys cannot be mixed");
        }

        if start.is_some() ^ stop.is_some() {
            bail!("start_hotkey and stop_hotkey must be provided together");
        }

        if toggle.is_none() && start.is_none() && stop.is_none() {
            return Ok(Self {
                toggle: Some("Space".parse()?),
                start: None,
                stop: None,
            });
        }

        Ok(Self {
            toggle,
            start,
            stop,
        })
    }

    pub fn trigger(&self, key: egui::Key, modifiers: egui::Modifiers) -> Option<HotkeyAction> {
        if self
            .toggle
            .as_ref()
            .is_some_and(|binding| binding.matches(key, modifiers))
        {
            return Some(HotkeyAction::Toggle);
        }

        if self
            .start
            .as_ref()
            .is_some_and(|binding| binding.matches(key, modifiers))
        {
            return Some(HotkeyAction::Start);
        }

        if self
            .stop
            .as_ref()
            .is_some_and(|binding| binding.matches(key, modifiers))
        {
            return Some(HotkeyAction::Stop);
        }

        None
    }

    pub fn description(&self) -> String {
        if let Some(toggle) = &self.toggle {
            return format!("{toggle} で開始/停止");
        }

        match (&self.start, &self.stop) {
            (Some(start), Some(stop)) => format!("開始 {start} / 停止 {stop}"),
            _ => "未設定".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: KeyCode,
}

impl HotkeyBinding {
    pub fn matches(&self, key: egui::Key, modifiers: egui::Modifiers) -> bool {
        let ctrl = modifiers.ctrl || modifiers.command;

        self.key.to_egui_key() == key
            && self.ctrl == ctrl
            && self.alt == modifiers.alt
            && self.shift == modifiers.shift
    }
}

impl Display for HotkeyBinding {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        parts.push(self.key.to_string());
        write!(f, "{}", parts.join("+"))
    }
}

impl FromStr for HotkeyBinding {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut key: Option<KeyCode> = None;

        for token in value
            .split('+')
            .map(|part| part.trim())
            .filter(|part| !part.is_empty())
        {
            let normalized = token.to_ascii_lowercase();
            match normalized.as_str() {
                "ctrl" | "control" | "cmd" | "command" => ctrl = true,
                "alt" => alt = true,
                "shift" => shift = true,
                _ => {
                    if key.is_some() {
                        bail!("multiple keys specified in hotkey: {value}");
                    }
                    key = Some(KeyCode::from_token(token)?);
                }
            }
        }

        let key = key.ok_or_else(|| anyhow!("hotkey must contain a key: {value}"))?;

        Ok(Self {
            ctrl,
            alt,
            shift,
            key,
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum KeyCode {
    Space,
    Enter,
    Escape,
    Tab,
    Backspace,
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

impl KeyCode {
    fn from_token(value: &str) -> Result<Self> {
        match value.trim().to_ascii_uppercase().as_str() {
            "SPACE" => Ok(Self::Space),
            "ENTER" | "RETURN" => Ok(Self::Enter),
            "ESC" | "ESCAPE" => Ok(Self::Escape),
            "TAB" => Ok(Self::Tab),
            "BACKSPACE" => Ok(Self::Backspace),
            "A" => Ok(Self::A),
            "B" => Ok(Self::B),
            "C" => Ok(Self::C),
            "D" => Ok(Self::D),
            "E" => Ok(Self::E),
            "F" => Ok(Self::F),
            "G" => Ok(Self::G),
            "H" => Ok(Self::H),
            "I" => Ok(Self::I),
            "J" => Ok(Self::J),
            "K" => Ok(Self::K),
            "L" => Ok(Self::L),
            "M" => Ok(Self::M),
            "N" => Ok(Self::N),
            "O" => Ok(Self::O),
            "P" => Ok(Self::P),
            "Q" => Ok(Self::Q),
            "R" => Ok(Self::R),
            "S" => Ok(Self::S),
            "T" => Ok(Self::T),
            "U" => Ok(Self::U),
            "V" => Ok(Self::V),
            "W" => Ok(Self::W),
            "X" => Ok(Self::X),
            "Y" => Ok(Self::Y),
            "Z" => Ok(Self::Z),
            "F1" => Ok(Self::F1),
            "F2" => Ok(Self::F2),
            "F3" => Ok(Self::F3),
            "F4" => Ok(Self::F4),
            "F5" => Ok(Self::F5),
            "F6" => Ok(Self::F6),
            "F7" => Ok(Self::F7),
            "F8" => Ok(Self::F8),
            "F9" => Ok(Self::F9),
            "F10" => Ok(Self::F10),
            "F11" => Ok(Self::F11),
            "F12" => Ok(Self::F12),
            _ => bail!("unsupported key: {value}"),
        }
    }

    fn to_egui_key(self) -> egui::Key {
        match self {
            Self::Space => egui::Key::Space,
            Self::Enter => egui::Key::Enter,
            Self::Escape => egui::Key::Escape,
            Self::Tab => egui::Key::Tab,
            Self::Backspace => egui::Key::Backspace,
            Self::A => egui::Key::A,
            Self::B => egui::Key::B,
            Self::C => egui::Key::C,
            Self::D => egui::Key::D,
            Self::E => egui::Key::E,
            Self::F => egui::Key::F,
            Self::G => egui::Key::G,
            Self::H => egui::Key::H,
            Self::I => egui::Key::I,
            Self::J => egui::Key::J,
            Self::K => egui::Key::K,
            Self::L => egui::Key::L,
            Self::M => egui::Key::M,
            Self::N => egui::Key::N,
            Self::O => egui::Key::O,
            Self::P => egui::Key::P,
            Self::Q => egui::Key::Q,
            Self::R => egui::Key::R,
            Self::S => egui::Key::S,
            Self::T => egui::Key::T,
            Self::U => egui::Key::U,
            Self::V => egui::Key::V,
            Self::W => egui::Key::W,
            Self::X => egui::Key::X,
            Self::Y => egui::Key::Y,
            Self::Z => egui::Key::Z,
            Self::F1 => egui::Key::F1,
            Self::F2 => egui::Key::F2,
            Self::F3 => egui::Key::F3,
            Self::F4 => egui::Key::F4,
            Self::F5 => egui::Key::F5,
            Self::F6 => egui::Key::F6,
            Self::F7 => egui::Key::F7,
            Self::F8 => egui::Key::F8,
            Self::F9 => egui::Key::F9,
            Self::F10 => egui::Key::F10,
            Self::F11 => egui::Key::F11,
            Self::F12 => egui::Key::F12,
        }
    }
}

impl Display for KeyCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Space => "Space",
            Self::Enter => "Enter",
            Self::Escape => "Escape",
            Self::Tab => "Tab",
            Self::Backspace => "Backspace",
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::E => "E",
            Self::F => "F",
            Self::G => "G",
            Self::H => "H",
            Self::I => "I",
            Self::J => "J",
            Self::K => "K",
            Self::L => "L",
            Self::M => "M",
            Self::N => "N",
            Self::O => "O",
            Self::P => "P",
            Self::Q => "Q",
            Self::R => "R",
            Self::S => "S",
            Self::T => "T",
            Self::U => "U",
            Self::V => "V",
            Self::W => "W",
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F6 => "F6",
            Self::F7 => "F7",
            Self::F8 => "F8",
            Self::F9 => "F9",
            Self::F10 => "F10",
            Self::F11 => "F11",
            Self::F12 => "F12",
        };
        write!(f, "{label}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ctrl_s() {
        let binding: HotkeyBinding = "Ctrl+S".parse().unwrap();
        assert!(binding.ctrl);
        assert_eq!(binding.key, KeyCode::S);
    }

    #[test]
    fn default_hotkey_is_space_toggle() {
        let set = HotkeySet::from_strings(None, None, None).unwrap();
        assert_eq!(set.description(), "Space で開始/停止");
    }

    #[test]
    fn rejects_mixed_toggle_and_separate_shortcuts() {
        let error = HotkeySet::from_strings(Some("Space"), Some("Ctrl+S"), Some("Ctrl+E"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("cannot be mixed"));
    }
}
