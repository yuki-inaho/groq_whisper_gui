use crate::audio::{
    self, AudioCaptureRequest, CompletedRecording, InputDeviceInfo, RecordingSession,
};
use crate::config::{AppConfig, GpuOffloadMode, UiMode, WHISPER_MODEL_V3, WHISPER_MODEL_V3_TURBO};
use crate::encoder::EncodedAudio;
use crate::hotkey_listener::RawHotkey;
use crate::persistence::{self, LastResultRecord, StoredAppState};
use crate::transcriber::{self, TranscriberConfig, TranscriptionResult};
use crate::usage::{self, SessionUsage};
use anyhow::anyhow;
use arboard::Clipboard;
use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
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
    session_usage: SessionUsage,
    show_exit_summary: bool,
    exit_confirmed: bool,
    hotkey_rx: Option<Receiver<RawHotkey>>,
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

/// 設定画面から変更できる項目。永続化はこの単位で行い、CLI や環境変数による
/// 一時的な上書きを `state.json` へ漏らさない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PreferenceField {
    UiMode,
    InputDevice,
    Model,
    GpuOffload,
}

#[derive(Debug)]
enum AppMessage {
    Stage(String),
    Log(String),
    // `CompletedJob` は `CompletedRecording` + `TranscriptionResult` を保持し
    // 250 バイト程度あるため、`Stage(String)` (~24 バイト) との variant サイズ差
    // を抑える目的で `Box` で間接参照する (clippy::large_enum_variant 対策)。
    JobFinished(Result<Box<CompletedJob>, String>),
}

#[derive(Debug)]
struct CompletedJob {
    recording: CompletedRecording,
    transcript: TranscriptionResult,
    model: String,
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
            session_usage: SessionUsage::default(),
            show_exit_summary: false,
            exit_confirmed: false,
            hotkey_rx: None,
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

        // IME 非依存ショートカット: evdev でキーボードを直接読めるなら有効化する。
        // 読めない場合 (権限不足等) は明示的にログを残し、egui イベント経路へ切り替える
        // (暗黙 fallback 禁止)。
        match crate::hotkey_listener::spawn_keyboard_listener() {
            Ok(listener) => {
                app.append_log(format!(
                    "IME 非依存ショートカット(evdev)を有効化。監視デバイス: {}",
                    listener.device_names.join(", ")
                ));
                app.hotkey_rx = Some(listener.receiver);
            }
            Err(error) => {
                app.append_log(format!(
                    "evdev を有効化できませんでした（{error}）。半角入力時のみ通常ショートカットが効きます。"
                ));
            }
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
                AppMessage::Log(message) => {
                    self.append_log(message);
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
        let recording_duration = job.recording.duration;
        let model = job.model.clone();
        self.transcript_text = job.transcript.text.clone();

        let copied_to_clipboard = if self.config.copy_to_clipboard {
            self.copy_text_to_clipboard(&job.transcript.text)
        } else {
            false
        };

        let audio_path = if self.config.keep_audio {
            Some(job.recording.encoded_audio.path.clone())
        } else {
            None
        };

        let char_count = job.transcript.text.chars().count();
        let preview: String = job.transcript.text.chars().take(120).collect();
        self.persisted.last_result = Some(LastResultRecord::now(
            preview,
            char_count,
            model.clone(),
            request_id.clone(),
            copied_to_clipboard,
            audio_path,
        ));
        self.session_usage
            .record_transcription(&model, recording_duration);

        if let Err(error) = self.persist_last_result() {
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
        self.append_log(format!(
            "今回の推定利用: {} request / 実音声 {} / 課金対象 {} / 推定 {}",
            self.session_usage.total_requests(),
            usage::format_seconds(self.session_usage.total_actual_seconds()),
            usage::format_seconds(self.session_usage.total_billable_seconds()),
            usage::format_usd(self.session_usage.estimated_total_cost_usd()),
        ));

        if let Some(raw_response) = job.transcript.raw_response {
            self.append_log("Groq 応答 JSON をデバッグログに保持しました。".to_string());
            self.append_log(raw_response);
        }
    }

    /// 与えられたテキストをクリップボードへ書き込む。成功可否を返し、失敗時は
    /// 理由をデバッグログへ残す。自動コピー(転写完了時)と手動コピー(ボタン)で共用する。
    fn copy_text_to_clipboard(&mut self, text: &str) -> bool {
        match Clipboard::new() {
            Ok(mut clipboard) => match clipboard.set_text(text.to_owned()) {
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

    /// 転写結果だけを保存する。CLI / 環境変数による実行時上書きは保存しない。
    fn persist_last_result(&mut self) -> anyhow::Result<()> {
        self.persisted.show_settings = self.show_settings;
        persistence::save_state(&self.persisted)
    }

    /// 設定画面でユーザーが実際に変更した項目だけを保存する。
    fn persist_preferences(&mut self, changed: &BTreeSet<PreferenceField>) -> anyhow::Result<()> {
        apply_preference_changes(
            &mut self.persisted,
            &self.config,
            self.selected_device.as_deref(),
            changed,
        );
        self.persisted.show_settings = self.show_settings;
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
        let keep_audio = self.config.keep_audio;

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
            finish_transcription_job(
                recording,
                transcriber_config,
                keep_audio,
                transcriber::transcribe_file,
                &tx,
            );
        });
    }

    fn toggle_recording(&mut self) {
        if self.recorder.is_some() {
            self.stop_recording();
        } else {
            self.start_recording();
        }
    }

    fn process_hotkeys(&mut self, ctx: &egui::Context) {
        // evdev が有効なときはそれを唯一のショートカット源とし、egui イベント経路は
        // 使わない (半角モードで二重発火しないようにするため)。evdev が無効なときだけ
        // egui イベント経路へ明示的に切り替える (暗黙 fallback 禁止)。
        if self.hotkey_rx.is_some() {
            self.process_global_hotkeys(ctx);
        } else {
            self.handle_shortcuts(ctx);
        }
    }

    /// evdev 経路。自ウィンドウにフォーカスがある時のみ発火させ、グローバル誤爆を防ぐ。
    /// フォーカス有無に関わらずキューは drain し、非フォーカス時の押下は破棄する。
    fn process_global_hotkeys(&mut self, ctx: &egui::Context) {
        let pending: Vec<RawHotkey> = match &self.hotkey_rx {
            Some(rx) => rx.try_iter().collect(),
            None => return,
        };

        if !ctx.input(|input| input.focused) {
            return;
        }

        for hotkey in pending {
            if self.dispatch_shortcut(ctx, hotkey.key, hotkey.modifiers) {
                break;
            }
        }
    }

    /// egui イベント経路 (evdev が使えない環境向けの明示的 fallback)。
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
                if self.dispatch_shortcut(ctx, key, modifiers) {
                    break;
                }
            }
        }
    }

    /// 論理キー + 修飾キーをショートカット動作へ振り分ける。egui 経路と evdev 経路で
    /// 共通利用する (DRY)。同フレームの後続キーを見ない場合に true を返す。
    fn dispatch_shortcut(
        &mut self,
        ctx: &egui::Context,
        key: egui::Key,
        modifiers: egui::Modifiers,
    ) -> bool {
        if is_quit_key(key) {
            self.request_exit(ctx);
            return true;
        }

        match self.config.hotkeys.trigger(key, modifiers) {
            Some(crate::hotkey::HotkeyAction::Toggle) => {
                self.toggle_recording();
                true
            }
            Some(crate::hotkey::HotkeyAction::Start) => {
                if self.recorder.is_none() {
                    self.start_recording();
                }
                true
            }
            Some(crate::hotkey::HotkeyAction::Stop) => {
                if self.recorder.is_some() {
                    self.stop_recording();
                }
                true
            }
            None => false,
        }
    }

    fn request_exit(&mut self, ctx: &egui::Context) {
        if self.show_exit_summary {
            self.confirm_exit(ctx);
        } else {
            self.show_exit_summary = true;
        }
    }

    fn confirm_exit(&mut self, ctx: &egui::Context) {
        self.exit_confirmed = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if close_requested && !self.exit_confirmed {
            if self.show_exit_summary {
                self.confirm_exit(ctx);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.show_exit_summary = true;
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

                if ui
                    .add_enabled(
                        !self.transcript_text.is_empty(),
                        egui::Button::new("クリップボードへコピー"),
                    )
                    .on_hover_text("直前の文字起こし結果をクリップボードへコピーします。")
                    .clicked()
                {
                    let text = self.transcript_text.clone();
                    if self.copy_text_to_clipboard(&text) {
                        self.status = UiStatus::Success(
                            "直前の文字起こしをクリップボードへコピーしました。".to_string(),
                        );
                        self.append_log(
                            "直前の文字起こしをクリップボードへコピーしました。".to_string(),
                        );
                    }
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
        let mut changed = BTreeSet::new();
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
                            changed.insert(PreferenceField::InputDevice);
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
                                changed.insert(PreferenceField::InputDevice);
                            }
                        }
                    });

                ui.separator();
                ui.label("UI モード");
                if ui
                    .selectable_value(&mut self.config.ui_mode, UiMode::Compact, "Compact")
                    .changed()
                {
                    changed.insert(PreferenceField::UiMode);
                }
                if ui
                    .selectable_value(&mut self.config.ui_mode, UiMode::Debug, "Debug")
                    .changed()
                {
                    changed.insert(PreferenceField::UiMode);
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
                                changed.insert(PreferenceField::Model);
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
                                changed.insert(PreferenceField::GpuOffload);
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
        if !changed.is_empty() {
            if let Err(error) = self.persist_preferences(&changed) {
                self.append_log(format!("設定保存失敗: {error}"));
            }
        }
    }

    fn draw_exit_summary_window(&mut self, ctx: &egui::Context) {
        if !self.show_exit_summary {
            return;
        }

        let mut open = true;
        egui::Window::new("終了確認")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("このまま終了しますか？").strong());
                ui.add_space(6.0);
                ui.add(egui::Label::new("この起動中に成功した Groq Whisper API 呼び出しの推定利用量です。Groq の音声文字起こしはトークンではなく音声時間ベースで課金されるため、料金は録音時間からの推定です。").wrap(true));
                ui.add_space(6.0);

                if self.session_usage.is_empty() {
                    ui.label("今回の起動中に成功したAPI呼び出しはありません。");
                } else {
                    ui.label(format!(
                        "APIリクエスト数: {}",
                        self.session_usage.total_requests()
                    ));
                    ui.label(format!(
                        "実録音時間: {}",
                        usage::format_seconds(self.session_usage.total_actual_seconds())
                    ));
                    ui.label(format!(
                        "課金対象時間: {}",
                        usage::format_seconds(self.session_usage.total_billable_seconds())
                    ));
                    ui.label(format!(
                        "推定合計: {}",
                        usage::format_usd(self.session_usage.estimated_total_cost_usd())
                    ));

                    ui.separator();
                    for (model, model_usage) in self.session_usage.model_usages() {
                        let cost = model_usage
                            .estimated_cost_usd(model)
                            .map(usage::format_usd)
                            .unwrap_or_else(|| "単価不明".to_string());
                        ui.label(format!(
                            "{}: {} request / 実音声 {} / 課金対象 {} / 推定 {}",
                            whisper_model_label(model),
                            model_usage.requests,
                            usage::format_seconds(model_usage.actual_seconds),
                            usage::format_seconds(model_usage.billable_seconds),
                            cost
                        ));
                    }

                    if self.session_usage.has_unknown_price() {
                        ui.add(egui::Label::new("単価不明のモデルは推定合計に含めていません。").wrap(true));
                    }
                }

                ui.add_space(6.0);
                ui.add(egui::Label::new("実際の請求額はクレジット、割引、価格変更、失敗リクエストの扱いにより異なる可能性があります。最終確認は Groq Console の Usage / Billing で行ってください。").wrap(true));
                ui.add_space(6.0);
                ui.add(egui::Label::new("このダイアログを開いた状態で Q または Esc をもう一度押しても終了できます。").wrap(true));
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    if ui.button("アプリに戻る").clicked() {
                        self.show_exit_summary = false;
                    }

                    if ui.button("終了する（Q / Esc）").clicked() {
                        self.confirm_exit(ctx);
                    }
                });
            });

        if !open {
            self.show_exit_summary = false;
        }
    }
}

impl eframe::App for VoiceDeskApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_close_request(ctx);
        self.process_messages();
        self.process_hotkeys(ctx);

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
        self.draw_exit_summary_window(ctx);
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

/// 転写ジョブのワーカー本体。GUI から独立してテストできるよう、HTTP 呼び出しを
/// 関数引数として受け取る。`keep_audio=false` のときは成功・失敗に関わらず
/// 一時音声ファイルを削除し、削除自体の失敗だけをログへ残す。
fn finish_transcription_job<F>(
    recording: CompletedRecording,
    transcriber_config: Option<TranscriberConfig>,
    keep_audio: bool,
    transcribe: F,
    tx: &Sender<AppMessage>,
) where
    F: FnOnce(&TranscriberConfig, &EncodedAudio) -> anyhow::Result<TranscriptionResult>,
{
    let cleanup_path = recording.encoded_audio.path.clone();
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

        let api =
            transcriber_config.ok_or_else(|| anyhow!("GROQ_API_KEY が設定されていません。"))?;
        let model = api.model.clone();

        let transcript = transcribe(&api, &recording.encoded_audio)?;
        Ok(Box::new(CompletedJob {
            recording,
            transcript,
            model,
        }))
    })()
    .map_err(|error| error.to_string());

    if !keep_audio {
        if let Err(error) = cleanup_audio_file(&cleanup_path) {
            let _ = tx.send(AppMessage::Log(format!(
                "一時音声ファイルを削除できませんでした: {} ({error})",
                cleanup_path.display()
            )));
        }
    }

    let _ = tx.send(AppMessage::JobFinished(result));
}

fn cleanup_audio_file(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn apply_preference_changes(
    persisted: &mut StoredAppState,
    config: &AppConfig,
    selected_device: Option<&str>,
    changed: &BTreeSet<PreferenceField>,
) {
    for field in changed {
        match field {
            PreferenceField::UiMode => persisted.ui_mode = config.ui_mode,
            PreferenceField::InputDevice => {
                persisted.input_device = selected_device.map(str::to_owned);
            }
            PreferenceField::Model => persisted.model = Some(config.model.clone()),
            PreferenceField::GpuOffload => persisted.gpu_offload = Some(config.gpu_offload),
        }
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
    use crate::config::{CliArgs, EncoderFormat};
    use clap::Parser;

    fn test_transcriber_config() -> TranscriberConfig {
        TranscriberConfig {
            api_key: "test-key".to_string(),
            base_url: "http://127.0.0.1:1".to_string(),
            model: WHISPER_MODEL_V3_TURBO.to_string(),
            language: Some("ja".to_string()),
            prompt: None,
            response_format: crate::config::ResponseFormat::Json,
            temperature: 0.0,
            word_timestamps: false,
            segment_timestamps: false,
            request_timeout_secs: 1,
        }
    }

    fn sample_recording(path: &Path) -> CompletedRecording {
        fs::write(path, b"mock audio").expect("write mock audio");
        CompletedRecording {
            device_name: "Mock Mic".to_string(),
            input_sample_rate: 16_000,
            input_channels: 1,
            captured_samples: 1_600,
            duration: Duration::from_millis(100),
            encoded_audio: EncodedAudio {
                path: path.to_path_buf(),
                format: EncoderFormat::Mp3,
                file_name: "capture.mp3".to_string(),
                byte_len: 10,
            },
        }
    }

    fn temp_audio_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "groq-whisper-app-test-{label}-{}-{}.mp3",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ))
    }

    #[test]
    fn preference_persistence_only_applies_explicit_ui_changes() {
        let stored = StoredAppState {
            ui_mode: UiMode::Compact,
            input_device: Some("Built-in Mic".to_string()),
            hotkey_toggle: Some("Space".to_string()),
            model: Some(WHISPER_MODEL_V3.to_string()),
            gpu_offload: Some(GpuOffloadMode::Off),
            ..StoredAppState::default()
        };
        let cli = CliArgs::parse_from([
            "voice-app",
            "--ui-mode",
            "debug",
            "--input-device",
            "USB Mic",
            "--model",
            WHISPER_MODEL_V3_TURBO,
            "--gpu-offload",
            "cuda",
            "--start-hotkey",
            "Ctrl+S",
            "--stop-hotkey",
            "Ctrl+E",
        ]);
        let config = AppConfig::from_sources(cli, &stored).unwrap();
        let mut persisted = stored.clone();

        apply_preference_changes(
            &mut persisted,
            &config,
            Some("USB Mic"),
            &BTreeSet::from([PreferenceField::UiMode, PreferenceField::Model]),
        );

        // 明示的に変更した 2 項目だけが反映される。
        assert_eq!(persisted.ui_mode, UiMode::Debug);
        assert_eq!(persisted.model.as_deref(), Some(WHISPER_MODEL_V3_TURBO));
        // CLI 由来の他項目は保存されない。
        assert_eq!(persisted.input_device.as_deref(), Some("Built-in Mic"));
        assert_eq!(persisted.gpu_offload, Some(GpuOffloadMode::Off));
        assert_eq!(persisted.hotkey_toggle.as_deref(), Some("Space"));
        assert!(persisted.hotkey_start.is_none());
        assert!(persisted.hotkey_stop.is_none());
    }

    #[test]
    fn failed_transcription_removes_temporary_audio_when_not_kept() {
        let path = temp_audio_path("cleanup-failure");
        let recording = sample_recording(&path);
        let (tx, rx) = unbounded();

        finish_transcription_job(
            recording,
            Some(test_transcriber_config()),
            false,
            |_, _| Err(anyhow!("mock transcription failure")),
            &tx,
        );

        assert!(!path.exists(), "failed job must remove temporary audio");
        let messages: Vec<AppMessage> = rx.try_iter().collect();
        assert!(matches!(messages.first(), Some(AppMessage::Stage(_))));
        assert!(matches!(
            messages.last(),
            Some(AppMessage::JobFinished(Err(_)))
        ));
    }

    #[test]
    fn failed_transcription_keeps_temporary_audio_when_requested() {
        let path = temp_audio_path("keep-failure");
        let recording = sample_recording(&path);
        let (tx, _rx) = unbounded();

        finish_transcription_job(
            recording,
            Some(test_transcriber_config()),
            true,
            |_, _| Err(anyhow!("mock transcription failure")),
            &tx,
        );

        assert!(path.exists(), "keep_audio=true must retain the file");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn successful_transcription_removes_temporary_audio_and_returns_job() {
        let path = temp_audio_path("cleanup-success");
        let recording = sample_recording(&path);
        let (tx, rx) = unbounded();

        finish_transcription_job(
            recording,
            Some(test_transcriber_config()),
            false,
            |_, _| {
                Ok(TranscriptionResult {
                    text: "テスト".to_string(),
                    request_id: Some("req_test".to_string()),
                    raw_response: None,
                    elapsed: Duration::from_millis(5),
                })
            },
            &tx,
        );

        assert!(!path.exists(), "successful job must remove temporary audio");
        let messages: Vec<AppMessage> = rx.try_iter().collect();
        let completed = messages
            .iter()
            .find_map(|message| match message {
                AppMessage::JobFinished(Ok(job)) => Some(job),
                _ => None,
            })
            .expect("successful job must be reported");
        assert_eq!(completed.transcript.text, "テスト");
        assert_eq!(completed.transcript.request_id.as_deref(), Some("req_test"));
    }

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
