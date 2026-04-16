use crate::config::{GpuOffloadMode, ResponseFormat, UiMode};
use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const APP_DIR_NAME: &str = "groq-whisper-app";
const LEGACY_APP_DIR_NAME: &str = "groq-whisper-desktop";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastResultRecord {
    pub text_preview: String,
    pub chars: usize,
    pub model: String,
    pub request_id: Option<String>,
    pub copied_to_clipboard: bool,
    pub occurred_at_local: String,
    pub audio_path: Option<PathBuf>,
}

impl LastResultRecord {
    pub fn now(
        text_preview: String,
        chars: usize,
        model: String,
        request_id: Option<String>,
        copied_to_clipboard: bool,
        audio_path: Option<PathBuf>,
    ) -> Self {
        let occurred_at_local: DateTime<Local> = Local::now();
        Self {
            text_preview,
            chars,
            model,
            request_id,
            copied_to_clipboard,
            occurred_at_local: occurred_at_local.to_rfc3339(),
            audio_path,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAppState {
    pub ui_mode: UiMode,
    pub show_settings: bool,
    pub input_device: Option<String>,
    pub hotkey_toggle: Option<String>,
    pub hotkey_start: Option<String>,
    pub hotkey_stop: Option<String>,
    pub model: Option<String>,
    pub gpu_offload: Option<GpuOffloadMode>,
    pub response_format: Option<ResponseFormat>,
    pub last_result: Option<LastResultRecord>,
}

impl Default for StoredAppState {
    fn default() -> Self {
        Self {
            ui_mode: UiMode::Compact,
            show_settings: false,
            input_device: None,
            hotkey_toggle: Some("Space".to_string()),
            hotkey_start: None,
            hotkey_stop: None,
            model: None,
            gpu_offload: None,
            response_format: Some(ResponseFormat::Json),
            last_result: None,
        }
    }
}

pub fn load_state() -> Result<StoredAppState> {
    let path = load_state_path()?;
    if !path.exists() {
        return Ok(StoredAppState::default());
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read state file: {}", path.display()))?;
    let state: StoredAppState = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse state file: {}", path.display()))?;
    Ok(state)
}

pub fn save_state(state: &StoredAppState) -> Result<()> {
    let path = state_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory: {}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(state)?;
    fs::write(&path, json).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub fn state_path() -> Result<PathBuf> {
    let project_dirs = ProjectDirs::from("com", "sandbox", APP_DIR_NAME)
        .context("failed to determine application data directory")?;
    Ok(project_dirs.config_dir().join("state.json"))
}

fn load_state_path() -> Result<PathBuf> {
    let new_path = state_path()?;
    if new_path.exists() {
        return Ok(new_path);
    }

    let legacy_dirs = ProjectDirs::from("com", "sandbox", LEGACY_APP_DIR_NAME)
        .context("failed to determine legacy application data directory")?;
    let legacy_path = legacy_dirs.config_dir().join("state.json");
    if legacy_path.exists() {
        return Ok(legacy_path);
    }

    Ok(new_path)
}
