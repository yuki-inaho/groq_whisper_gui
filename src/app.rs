use crate::audio::{
    self, AudioCaptureRequest, CompletedRecording, InputDeviceInfo, RecordingSession,
};
use crate::config::{AppConfig, GpuOffloadMode, UiMode, WHISPER_MODEL_V3, WHISPER_MODEL_V3_TURBO};
use crate::persistence::{self, LastResultRecord, StoredAppState};
use crate::transcriber::{self, TranscriptionResult};
use anyhow::anyhow;
use arboard::Clipboard;
use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui;
use std::fs;
use std::time::Duration;

pub struct VoiceDeskApp {
    config: AppConfig,
    persisted: StoredAppState,
    devices: Vec<InputDeviceInfo>,
    selected_device: Option<String>,
    recorder: Option<RecordingSession>,
    smoothed_level: f32,
    status: UiStatus,
    transcript_text: String,
    debug_lines: Vec<String>,
    show_settings: bool,
    tx: Sender<AppMessage>,
    rx: Receiver<AppMessage>,
}

#[derive(Debug, Clone)]
enum UiStatus {
    Idle,
    Recording,
    Processing(String),
    Success(String),
    Error(String),
}

#[derive(Debug)]
enum AppMessage {
    Stage(String),
    // `CompletedJob` は `CompletedRecording` + `TranscriptionResult` を保持し
    // 250 バイト程度あるため、`Stage(String)` (~24 バイト) との variant サイズ差
    // を抑える目的で `Box` で間接参照する (clippy::large_enum_variant 対策)。
    JobFinished(Result<Box<CompletedJob>, String>),
}

#[derive(Debug)]
struct CompletedJob {
    recording: CompletedRecording,
    transcript: TranscriptionResult,
}

impl VoiceDeskApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: AppConfig,
        persisted: StoredAppState,
    ) -> Self {
        let ui_font = crate::ui::configure(&cc.egui_ctx);
        let devices = audio::list_input_devices().unwrap_or_default();
        let selected_device = resolve_initial_device(&config, &persisted, &devices);
        let (tx, rx) = unbounded();

        let mut app = Self {
            config,
            persisted,
            devices,
            selected_device,
            recorder: None,
            smoothed_level: 0.0,
            status: UiStatus::Idle,
            transcript_text: String::new(),
            debug_lines: Vec::new(),
            show_settings: false,
            tx,
            rx,
        };

        app.show_settings = app.config.show_settings_on_launch;
        app.append_log("UI テーマ: light".to_string());
        match ui_font {
            Some(path) => app.append_log(format!("UI フォント: {}", path.display())),
            None => app.append_log(
                "日本語 UI フォントを自動検出できませんでした。システムの既定フォントを使います。"
                    .to_string(),
            ),
        }
        app.append_log(format!(
            "ショートカット: {}",
            app.config.hotkeys.description()
        ));

        if let Some(device) = &app.selected_device {
            app.append_log(format!("入力デバイス: {device}"));
        } else {
            app.append_log("利用可能な入力デバイスが見つかりませんでした。".to_string());
        }

        if app.config.api_key.is_none() {
            app.append_log(
                "GROQ_API_KEY が未設定です。録音はできますが、転写 API 呼び出しは失敗します。"
                    .to_string(),
            );
        }

        if let Some(last_result) = &app.persisted.last_result {
            app.append_log(format!("前回転写: {}", last_result.occurred_at_local));
        }

        app
    }

    fn process_messages(&mut self) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                AppMessage::Stage(stage) => {
                    self.status = UiStatus::Processing(stage.clone());
                    self.append_log(stage);
                }
                AppMessage::JobFinished(result) => match result {
                    Ok(job) => self.handle_completed_job(*job),
                    Err(error) => {
                        self.status = UiStatus::Error(error.clone());
                        self.append_log(format!("エラー: {error}"));
                    }
                },
            }
        }
    }

    fn handle_completed_job(&mut self, job: CompletedJob) {
        let request_id = job.transcript.request_id.clone();
        let elapsed_ms = job.transcript.elapsed.as_millis();
        self.transcript_text = job.transcript.text.clone();

        let copied_to_clipboard = if self.config.copy_to_clipboard {
            match Clipboard::new() {
                Ok(mut clipboard) => match clipboard.set_text(job.transcript.text.clone()) {
                    Ok(_) => true,
                    Err(error) => {
                        self.append_log(format!("クリップボード更新失敗: {error}"));
                        false
                    }
                },
                Err(error) => {
                    self.append_log(format!("クリップボード初期化失敗: {error}"));
                    false
                }
            }
        } else {
            false
        };

        let audio_path = if self.config.keep_audio {
            Some(job.recording.encoded_audio.path.clone())
        } else {
            None
        };

        if !self.config.keep_audio {
            let _ = fs::remove_file(&job.recording.encoded_audio.path);
        }

        let char_count = job.transcript.text.chars().count();
        let preview: String = job.transcript.text.chars().take(120).collect();
        self.persisted.last_result = Some(LastResultRecord::now(
            preview,
            char_count,
            self.config.model.clone(),
            request_id.clone(),
            copied_to_clipboard,
            audio_path,
        ));

        if let Err(error) = self.persist_preferences() {
            self.append_log(format!("設定保存失敗: {error}"));
        }

        let status_message = match request_id.as_deref() {
            Some(request_id) => format!(
                "文字起こし完了。{} 文字。{} ms。request_id={request_id}",
                char_count, elapsed_ms
            ),
            None => format!("文字起こし完了。{} 文字。{} ms。", char_count, elapsed_ms),
        };

        self.status = UiStatus::Success(status_message.clone());
        self.append_log(status_message);

        if let Some(raw_response) = job.transcript.raw_response {
            self.append_log("Groq 応答 JSON をデバッグログに保持しました。".to_string());
            self.append_log(raw_response);
        }
    }

    fn refresh_devices(&mut self) {
        match audio::list_input_devices() {
            Ok(devices) => {
                self.devices = devices;

                let current_is_valid = self.selected_device.as_ref().is_some_and(|selected| {
                    self.devices.iter().any(|device| &device.name == selected)
                });

                if !current_is_valid {
                    self.selected_device = self
                        .devices
                        .iter()
                        .find(|device| device.is_default)
                        .or_else(|| self.devices.first())
                        .map(|device| device.name.clone());
                }

                self.append_log("入力デバイス一覧を更新しました。".to_string());
            }
            Err(error) => {
                self.status = UiStatus::Error(error.to_string());
                self.append_log(format!("入力デバイス更新失敗: {error}"));
            }
        }
    }

    fn persist_preferences(&mut self) -> anyhow::Result<()> {
        self.persisted.ui_mode = self.config.ui_mode;
        self.persisted.show_settings = self.show_settings;
        self.persisted.input_device = self.selected_device.clone();
        self.persisted.response_format = Some(self.config.response_format);
        self.persisted.hotkey_toggle = self.config.hotkeys.toggle.as_ref().map(ToString::to_string);
        self.persisted.hotkey_start = self.config.hotkeys.start.as_ref().map(ToString::to_string);
        self.persisted.hotkey_stop = self.config.hotkeys.stop.as_ref().map(ToString::to_string);
        self.persisted.model = Some(self.config.model.clone());
        self.persisted.gpu_offload = Some(self.config.gpu_offload);

        persistence::save_state(&self.persisted)
    }

    fn append_log(&mut self, message: String) {
        let line = format!(
            "{} | {}",
            chrono::Local::now().format("%H:%M:%S"),
            message.replace('\n', " ")
        );
        self.debug_lines.push(line);
        if self.debug_lines.len() > 200 {
            let overflow = self.debug_lines.len() - 200;
            self.debug_lines.drain(0..overflow);
        }
    }

    fn is_processing(&self) -> bool {
        matches!(self.status, UiStatus::Processing(_))
    }

    fn start_recording(&mut self) {
        if self.recorder.is_some() || self.is_processing() {
            return;
        }

        let request = AudioCaptureRequest {
            preferred_device_name: self.selected_device.clone(),
            encoder_settings: self.config.encoder_settings(),
        };

        match audio::start_recording(request) {
            Ok(session) => {
                self.append_log("録音を開始しました。".to_string());
                self.recorder = Some(session);
                self.status = UiStatus::Recording;
            }
            Err(error) => {
                self.status = UiStatus::Error(error.to_string());
                self.append_log(format!("録音開始失敗: {error}"));
            }
        }
    }

    fn stop_recording(&mut self) {
        if self.is_processing() {
            return;
        }

        let Some(session) = self.recorder.take() else {
            return;
        };

        let tx = self.tx.clone();
        let transcriber_config = self.config.transcriber_config();

        self.status = UiStatus::Processing("録音停止。音声ファイルを確定しています。".to_string());

        // `RecordingSession` holds a `cpal::Stream`, which is `!Send` on ALSA/
        // JACK backends. Finalize the recording on the current (UI) thread so
        // only the `Send`-safe `CompletedRecording` crosses the thread boundary.
        let _ = tx.send(AppMessage::Stage(
            "録音停止。音声ファイルを確定しています。".to_string(),
        ));
        let recording = match session.stop() {
            Ok(recording) => recording,
            Err(error) => {
                let _ = tx.send(AppMessage::JobFinished(Err(error.to_string())));
                return;
            }
        };

        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<Box<CompletedJob>> {
                let file_size_kb = recording.encoded_audio.byte_len as f64 / 1024.0;
                let _ = tx.send(AppMessage::Stage(format!(
                    "Groq API へ送信します: {} / {:.1} KiB / {:.2} 秒 (device: {}, sr: {} Hz, ch: {}, samples: {})",
                    recording.encoded_audio.file_name,
                    file_size_kb,
                    recording.duration.as_secs_f32(),
                    recording.device_name,
                    recording.input_sample_rate,
                    recording.input_channels,
                    recording.captured_samples,
                )));

                let api = transcriber_config
                    .ok_or_else(|| anyhow!("GROQ_API_KEY が設定されていません。"))?;

                let transcript = transcriber::transcribe_file(&api, &recording.encoded_audio)?;
                Ok(Box::new(CompletedJob {
                    recording,
                    transcript,
                }))
            })()
            .map_err(|error| error.to_string());

            let _ = tx.send(AppMessage::JobFinished(result));
        });
    }

    fn toggle_recording(&mut self) {
        if self.recorder.is_some() {
            self.stop_recording();
        } else {
            self.start_recording();
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|input| input.events.clone());
        for event in events {
            if let egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } = event
            {
                if is_quit_key(key) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    break;
                }

                match self.config.hotkeys.trigger(key, modifiers) {
                    Some(crate::hotkey::HotkeyAction::Toggle) => {
                        self.toggle_recording();
                        break;
                    }
                    Some(crate::hotkey::HotkeyAction::Start) => {
                        if self.recorder.is_none() {
                            self.start_recording();
                        }
                        break;
                    }
                    Some(crate::hotkey::HotkeyAction::Stop) => {
                        if self.recorder.is_some() {
                            self.stop_recording();
                        }
                        break;
                    }
                    None => {}
                }
            }
        }
    }

    fn status_text(&self) -> String {
        match &self.status {
            UiStatus::Idle => "待機中".to_string(),
            UiStatus::Recording => {
                let elapsed = self
                    .recorder
                    .as_ref()
                    .map(|recorder| recorder.elapsed())
                    .unwrap_or_else(|| Duration::from_secs(0));
                format!("録音中 {}", format_duration(elapsed))
            }
            UiStatus::Processing(message) => compact_status_message(message),
            UiStatus::Success(message) => compact_status_message(message),
            UiStatus::Error(message) => format!("エラー: {}", compact_status_message(message)),
        }
    }

    fn draw_main_panel(&mut self, ctx: &egui::Context) {
        let is_recording = self.recorder.is_some();
        let status_text = self.status_text();
        let selected_device = display_device_name(self.selected_device.as_deref());

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("音声文字起こし");
            ui.label(
                "録音を停止すると Groq Whisper に送信し、結果をクリップボードへコピーします。",
            );
            ui.add_space(4.0);

            ui.add(
                egui::Label::new(format!(
                    "録音: {} / 終了: Esc または Q",
                    self.config.hotkeys.description()
                ))
                .wrap(true),
            );

            ui.horizontal(|ui| {
                let record_button_label = if is_recording {
                    "録音停止"
                } else {
                    "録音開始"
                };
                if ui
                    .add_enabled(
                        !self.is_processing() || is_recording,
                        egui::Button::new(record_button_label),
                    )
                    .clicked()
                {
                    self.toggle_recording();
                }

                if ui.button("設定").clicked() {
                    self.show_settings = true;
                }
            });

            ui.separator();
            draw_status_indicator(ui, self.smoothed_level, &status_text);
            ui.label(format!("マイク: {selected_device}"));

            if let Some(last_result) = &self.persisted.last_result {
                ui.separator();
                ui.label(format!("最終更新: {}", last_result.occurred_at_local));
                ui.label(format!(
                    "クリップボード更新: {}",
                    if last_result.copied_to_clipboard {
                        "成功"
                    } else {
                        "未更新または失敗"
                    }
                ));
                ui.label(format!("前回文字数: {}", last_result.chars));
                if let Some(request_id) = &last_result.request_id {
                    ui.label(format!("前回 request_id: {request_id}"));
                }
            }

            if matches!(self.config.ui_mode, UiMode::Debug) {
                ui.separator();
                ui.label("転写結果");
                let mut transcript = self.transcript_text.clone();
                ui.add_enabled(
                    false,
                    egui::TextEdit::multiline(&mut transcript)
                        .desired_rows(12)
                        .desired_width(f32::INFINITY),
                );

                ui.separator();
                ui.label("デバッグログ");
                let mut logs = self.debug_lines.join("\n");
                ui.add_enabled(
                    false,
                    egui::TextEdit::multiline(&mut logs)
                        .desired_rows(10)
                        .desired_width(f32::INFINITY),
                );
            }
        });
    }

    fn draw_settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }

        let mut open = self.show_settings;
        let mut changed = false;
        let device_entries = self.devices.clone();

        egui::Window::new("設定")
            .open(&mut open)
            .resizable(true)
            .show(ctx, |ui| {
                if ui.button("入力デバイス一覧を更新").clicked() {
                    self.refresh_devices();
                }

                egui::ComboBox::from_label("入力デバイス")
                    .selected_text(
                        self.selected_device
                            .clone()
                            .unwrap_or_else(|| "既定入力デバイス".to_string()),
                    )
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_value(
                                &mut self.selected_device,
                                None,
                                "既定入力デバイスを使用",
                            )
                            .changed()
                        {
                            changed = true;
                        }

                        for device in &device_entries {
                            if ui
                                .selectable_value(
                                    &mut self.selected_device,
                                    Some(device.name.clone()),
                                    device.label(),
                                )
                                .changed()
                            {
                                changed = true;
                            }
                        }
                    });

                ui.separator();
                ui.label("UI モード");
                if ui
                    .selectable_value(&mut self.config.ui_mode, UiMode::Compact, "Compact")
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .selectable_value(&mut self.config.ui_mode, UiMode::Debug, "Debug")
                    .changed()
                {
                    changed = true;
                }

                ui.separator();
                egui::ComboBox::from_label("Whisper モデル")
                    .selected_text(whisper_model_label(&self.config.model))
                    .show_ui(ui, |ui| {
                        for (model, label) in whisper_model_choices() {
                            if ui
                                .selectable_value(&mut self.config.model, model.to_string(), label)
                                .changed()
                            {
                                changed = true;
                            }
                        }
                    });

                egui::ComboBox::from_label("GPU オフロード")
                    .selected_text(gpu_offload_label(self.config.gpu_offload))
                    .show_ui(ui, |ui| {
                        for mode in gpu_offload_choices() {
                            if ui
                                .selectable_value(
                                    &mut self.config.gpu_offload,
                                    mode,
                                    gpu_offload_label(mode),
                                )
                                .changed()
                            {
                                changed = true;
                            }
                        }
                    });
                ui.add(egui::Label::new(self.config.gpu_offload.note()).wrap(true));

                ui.label(format!(
                    "エンコード形式: {} / {} kbps / {} Hz / {} ch",
                    self.config.encoder_format.label(),
                    self.config.bitrate_kbps,
                    self.config.output_sample_rate,
                    self.config.output_channels
                ));
                ui.label(format!(
                    "応答形式: {}",
                    self.config.response_format.api_value()
                ));
                ui.label(format!(
                    "クリップボード自動反映: {}",
                    if self.config.copy_to_clipboard {
                        "有効"
                    } else {
                        "無効"
                    }
                ));
            });

        self.show_settings = open;
        if changed {
            if let Err(error) = self.persist_preferences() {
                self.append_log(format!("設定保存失敗: {error}"));
            }
        }
    }
}

impl eframe::App for VoiceDeskApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_messages();
        self.handle_shortcuts(ctx);

        let raw_level = self
            .recorder
            .as_ref()
            .map(|recorder| recorder.level())
            .unwrap_or(0.0);
        self.smoothed_level = (self.smoothed_level * 0.75) + (raw_level * 0.25);

        if self.recorder.is_none() && !self.is_processing() {
            self.smoothed_level *= 0.90;
        }

        self.draw_main_panel(ctx);
        self.draw_settings_window(ctx);
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

fn resolve_initial_device(
    config: &AppConfig,
    persisted: &StoredAppState,
    devices: &[InputDeviceInfo],
) -> Option<String> {
    let preferred: Option<&str> = config
        .input_device
        .as_deref()
        .or(persisted.input_device.as_deref());

    if let Some(device_name) = preferred {
        if devices.iter().any(|device| device.name == device_name) {
            return Some(device_name.to_owned());
        }
    }

    devices
        .iter()
        .find(|device| device.is_default)
        .or_else(|| devices.first())
        .map(|device| device.name.clone())
}

fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    let centis = duration.subsec_millis() / 10;
    format!("{minutes:02}:{seconds:02}.{centis:02}")
}

fn display_device_name(device_name: Option<&str>) -> String {
    match device_name {
        Some(name) if name.eq_ignore_ascii_case("default") => "既定のマイク".to_string(),
        Some(name) if !name.trim().is_empty() => name.to_string(),
        _ => "既定のマイク".to_string(),
    }
}

fn compact_status_message(message: &str) -> String {
    if message.contains("GROQ_API_KEY") {
        return "API キーが未設定です".to_string();
    }

    if message.contains("Groq API") || message.contains("送信") {
        return "文字起こし中".to_string();
    }

    if message.contains("録音停止") || message.contains("確定") {
        return "音声を保存中".to_string();
    }

    if message.contains("LameMp3Encoder") || message.contains("lame failed") {
        return "MP3 変換に失敗しました".to_string();
    }

    const MAX_CHARS: usize = 42;
    let char_count = message.chars().count();
    if char_count <= MAX_CHARS {
        return message.to_string();
    }

    let mut shortened: String = message.chars().take(MAX_CHARS).collect();
    shortened.push('…');
    shortened
}

fn draw_status_indicator(ui: &mut egui::Ui, level: f32, status_text: &str) {
    ui.horizontal(|ui| {
        let size = egui::vec2(18.0, 18.0);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), 7.0, level_color(level));
        ui.label(":");
        ui.add(egui::Label::new(egui::RichText::new(status_text).strong()).wrap(true));
    });
}

fn level_color(level: f32) -> egui::Color32 {
    let level = level.clamp(0.0, 1.0);

    if level < 0.03 {
        return egui::Color32::from_rgb(170, 190, 196);
    }

    if level < 0.35 {
        return egui::Color32::from_rgb(76, 180, 120);
    }

    if level < 0.70 {
        return egui::Color32::from_rgb(238, 183, 54);
    }

    egui::Color32::from_rgb(231, 96, 54)
}

fn whisper_model_choices() -> [(&'static str, &'static str); 2] {
    [
        (WHISPER_MODEL_V3_TURBO, "Whisper v3 Turbo (高速)"),
        (WHISPER_MODEL_V3, "Whisper v3 (高精度)"),
    ]
}

fn whisper_model_label(model: &str) -> String {
    whisper_model_choices()
        .iter()
        .find(|(value, _)| *value == model)
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| format!("カスタム: {model}"))
}

fn gpu_offload_choices() -> [GpuOffloadMode; 6] {
    [
        GpuOffloadMode::Off,
        GpuOffloadMode::Auto,
        GpuOffloadMode::Cuda,
        GpuOffloadMode::Qsv,
        GpuOffloadMode::Amf,
        GpuOffloadMode::Vaapi,
    ]
}

fn gpu_offload_label(mode: GpuOffloadMode) -> &'static str {
    match mode {
        GpuOffloadMode::Off => "off (CPU のみ)",
        GpuOffloadMode::Auto => "auto",
        GpuOffloadMode::Cuda => "cuda (NVIDIA)",
        GpuOffloadMode::Qsv => "qsv (Intel)",
        GpuOffloadMode::Amf => "amf (AMD)",
        GpuOffloadMode::Vaapi => "vaapi (Linux)",
    }
}

fn is_quit_key(key: egui::Key) -> bool {
    matches!(key, egui::Key::Escape | egui::Key::Q)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_keys_are_escape_and_q() {
        assert!(is_quit_key(egui::Key::Escape));
        assert!(is_quit_key(egui::Key::Q));
        assert!(!is_quit_key(egui::Key::Space));
    }

    #[test]
    fn display_device_name_hides_default_internal_name() {
        assert_eq!(display_device_name(Some("default")), "既定のマイク");
        assert_eq!(display_device_name(None), "既定のマイク");
    }

    #[test]
    fn whisper_model_label_names_supported_models() {
        assert_eq!(
            whisper_model_label(WHISPER_MODEL_V3_TURBO),
            "Whisper v3 Turbo (高速)"
        );
        assert_eq!(whisper_model_label(WHISPER_MODEL_V3), "Whisper v3 (高精度)");
    }
}
