mod app;
mod audio;
mod config;
mod encoder;
#[cfg(test)]
mod fixture;
mod hotkey;
mod persistence;
mod transcriber;
mod ui;
mod usage;

use anyhow::Result;
use clap::Parser;
use config::{AppConfig, CliArgs, UiMode};
use eframe::egui;
use persistence::StoredAppState;

fn build_native_options(config: &AppConfig) -> eframe::NativeOptions {
    let (width, height) = match config.ui_mode {
        UiMode::Compact => (473.0, 375.0),
        UiMode::Debug => (924.0, 825.0),
    };

    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Groq Whisper 文字起こし")
            .with_inner_size(egui::vec2(width, height))
            .with_min_inner_size(egui::vec2(430.0, 350.0)),
        ..Default::default()
    }
}

fn main() -> Result<()> {
    let cli = CliArgs::parse();
    let stored = persistence::load_state().unwrap_or_else(|_| StoredAppState::default());
    let config = AppConfig::from_sources(cli, &stored)?;

    // 起動時に明示的な選択値をログへ出力する (暗黙 fallback 禁止 / 監査トレーサビリティ)。
    eprintln!(
        "[groq-whisper-app] ui_mode={}, mp3_encoder={}, lame_path={}",
        config.ui_mode.label(),
        config.mp3_encoder.label(),
        config.lame_path,
    );

    let native_options = build_native_options(&config);
    eframe::run_native(
        "Groq Whisper 文字起こし",
        native_options,
        Box::new(move |cc| {
            Box::new(app::VoiceDeskApp::new(cc, config.clone(), stored.clone()))
                as Box<dyn eframe::App>
        }),
    )
    .map_err(|error| anyhow::anyhow!("eframe error: {error}"))
}
