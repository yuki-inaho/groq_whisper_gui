use crate::encoder::EncoderSettings;
use crate::hotkey::HotkeySet;
use crate::persistence::StoredAppState;
use crate::transcriber::TranscriberConfig;
use anyhow::Result;
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const DEFAULT_BASE_URL: &str = "https://api.groq.com/openai/v1";
pub const WHISPER_MODEL_V3_TURBO: &str = "whisper-large-v3-turbo";
pub const WHISPER_MODEL_V3: &str = "whisper-large-v3";
pub const DEFAULT_MODEL: &str = WHISPER_MODEL_V3_TURBO;
pub const DEFAULT_LANGUAGE: &str = "ja";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
pub enum UiMode {
    Compact,
    Debug,
}

impl UiMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Debug => "Debug",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
pub enum EncoderFormat {
    Mp3,
    Wav,
}

/// MP3 エンコーダバックエンドの明示選択。
///
/// `ffmpeg` は `libmp3lame` を内蔵した ffmpeg を必要とし、
/// `lame` は `lame` CLI コマンドへ raw PCM を pipe する。
/// 既定は `lame` (暗黙 fallback 禁止: 手順 13/15 参照)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
pub enum Mp3EncoderBackend {
    Ffmpeg,
    Lame,
}

impl Mp3EncoderBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::Lame => "lame",
        }
    }
}

impl EncoderFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
pub enum GpuOffloadMode {
    Off,
    Auto,
    Cuda,
    Qsv,
    Amf,
    Vaapi,
}

impl GpuOffloadMode {
    pub fn note(self) -> &'static str {
        match self {
            Self::Off => "CPU のみを使用します。",
            Self::Auto => "拡張用の自動モードです。音声専用 MP3 パイプラインでは効果が限定的です。",
            Self::Cuda => "将来拡張・カスタム FFmpeg 引数向けの CUDA モードです。",
            Self::Qsv => "将来拡張・カスタム FFmpeg 引数向けの Intel QSV モードです。",
            Self::Amf => "将来拡張・カスタム FFmpeg 引数向けの AMD AMF モードです。",
            Self::Vaapi => "将来拡張・カスタム FFmpeg 引数向けの VAAPI モードです。",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
pub enum ResponseFormat {
    Json,
    Text,
    VerboseJson,
}

impl ResponseFormat {
    pub fn api_value(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Text => "text",
            Self::VerboseJson => "verbose_json",
        }
    }
}

#[derive(Debug, Clone, Parser)]
#[command(
    author,
    version,
    about = "Standalone desktop transcriber for Groq Whisper API"
)]
pub struct CliArgs {
    // clap は env 値を help に表示できるため、秘密値は変数名だけを出す。
    #[arg(long, env = "GROQ_API_KEY", hide_env_values = true)]
    pub api_key: Option<String>,

    #[arg(long, env = "GROQ_BASE_URL")]
    pub base_url: Option<String>,

    #[arg(long, env = "GROQ_WHISPER_MODEL")]
    pub model: Option<String>,

    #[arg(long, env = "GROQ_WHISPER_LANGUAGE", default_value = DEFAULT_LANGUAGE)]
    pub language: String,

    // プロンプトにも業務文脈や個人情報が入りうるので API key と同じ扱いにする。
    #[arg(long, env = "GROQ_WHISPER_PROMPT", hide_env_values = true)]
    pub prompt: Option<String>,

    #[arg(long, value_enum, env = "GROQ_UI_MODE")]
    pub ui_mode: Option<UiMode>,

    #[arg(long, env = "GROQ_INPUT_DEVICE")]
    pub input_device: Option<String>,

    #[arg(long, value_enum, env = "GROQ_RESPONSE_FORMAT")]
    pub response_format: Option<ResponseFormat>,

    #[arg(long, value_enum, env = "GROQ_ENCODER_FORMAT")]
    pub encoder_format: Option<EncoderFormat>,

    #[arg(
        long,
        value_enum,
        env = "GROQ_MP3_ENCODER",
        default_value_t = Mp3EncoderBackend::Lame,
    )]
    pub mp3_encoder: Mp3EncoderBackend,

    #[arg(long, env = "GROQ_LAME_PATH", default_value = "/usr/bin/lame")]
    pub lame_path: String,

    #[arg(long, value_enum, env = "GROQ_GPU_OFFLOAD")]
    pub gpu_offload: Option<GpuOffloadMode>,

    #[arg(long, env = "GROQ_TOGGLE_HOTKEY")]
    pub toggle_hotkey: Option<String>,

    #[arg(long, env = "GROQ_START_HOTKEY")]
    pub start_hotkey: Option<String>,

    #[arg(long, env = "GROQ_STOP_HOTKEY")]
    pub stop_hotkey: Option<String>,

    #[arg(long, env = "GROQ_FFMPEG_PATH")]
    pub ffmpeg_path: Option<String>,

    #[arg(long)]
    pub ffmpeg_extra_arg: Vec<String>,

    #[arg(long, env = "GROQ_TEMP_DIR")]
    pub temp_dir: Option<PathBuf>,

    #[arg(long, default_value_t = 48, env = "GROQ_BITRATE_KBPS")]
    pub bitrate_kbps: u32,

    #[arg(long, default_value_t = 16000, env = "GROQ_OUTPUT_SAMPLE_RATE")]
    pub output_sample_rate: u32,

    #[arg(long, default_value_t = 1, env = "GROQ_OUTPUT_CHANNELS")]
    pub output_channels: u16,

    #[arg(long, default_value_t = 120, env = "GROQ_REQUEST_TIMEOUT_SECS")]
    pub request_timeout_secs: u64,

    #[arg(long, default_value_t = 0.0, env = "GROQ_TEMPERATURE")]
    pub temperature: f32,

    #[arg(long, default_value_t = false, env = "GROQ_SHOW_SETTINGS")]
    pub show_settings: bool,

    #[arg(long, default_value_t = false, env = "GROQ_KEEP_AUDIO")]
    pub keep_audio: bool,

    #[arg(long, default_value_t = false, env = "GROQ_DISABLE_CLIPBOARD")]
    pub disable_clipboard: bool,

    #[arg(long, default_value_t = false, env = "GROQ_WORD_TIMESTAMPS")]
    pub word_timestamps: bool,

    #[arg(long, default_value_t = false, env = "GROQ_SEGMENT_TIMESTAMPS")]
    pub segment_timestamps: bool,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub ui_mode: UiMode,
    pub input_device: Option<String>,
    pub response_format: ResponseFormat,
    pub encoder_format: EncoderFormat,
    pub mp3_encoder: Mp3EncoderBackend,
    pub gpu_offload: GpuOffloadMode,
    pub ffmpeg_path: String,
    pub ffmpeg_extra_args: Vec<String>,
    pub lame_path: String,
    pub temp_dir: PathBuf,
    pub bitrate_kbps: u32,
    pub output_sample_rate: u32,
    pub output_channels: u16,
    pub request_timeout_secs: u64,
    pub temperature: f32,
    pub show_settings_on_launch: bool,
    pub keep_audio: bool,
    pub copy_to_clipboard: bool,
    pub word_timestamps: bool,
    pub segment_timestamps: bool,
    pub hotkeys: HotkeySet,
}

impl AppConfig {
    pub fn from_sources(cli: CliArgs, stored: &StoredAppState) -> Result<Self> {
        let ui_mode = cli.ui_mode.unwrap_or(stored.ui_mode);
        let input_device = cli
            .input_device
            .clone()
            .or_else(|| stored.input_device.clone());

        let cli_has_hotkey_override =
            cli.toggle_hotkey.is_some() || cli.start_hotkey.is_some() || cli.stop_hotkey.is_some();

        let (toggle_hotkey, start_hotkey, stop_hotkey) = if cli_has_hotkey_override {
            (
                cli.toggle_hotkey.as_deref(),
                cli.start_hotkey.as_deref(),
                cli.stop_hotkey.as_deref(),
            )
        } else {
            (
                stored.hotkey_toggle.as_deref(),
                stored.hotkey_start.as_deref(),
                stored.hotkey_stop.as_deref(),
            )
        };

        let hotkeys = HotkeySet::from_strings(toggle_hotkey, start_hotkey, stop_hotkey)?;

        let mut response_format = cli
            .response_format
            .or(stored.response_format)
            .unwrap_or(ResponseFormat::Json);

        if cli.word_timestamps || cli.segment_timestamps {
            response_format = ResponseFormat::VerboseJson;
        }

        let temp_dir = cli
            .temp_dir
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("groq-whisper-desktop"));

        Ok(Self {
            api_key: cli.api_key.clone(),
            base_url: cli
                .base_url
                .clone()
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            model: cli
                .model
                .clone()
                .or_else(|| stored.model.clone())
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            language: Some(cli.language.clone()),
            prompt: cli.prompt.clone(),
            ui_mode,
            input_device,
            response_format,
            encoder_format: cli.encoder_format.unwrap_or(EncoderFormat::Mp3),
            mp3_encoder: cli.mp3_encoder,
            gpu_offload: cli
                .gpu_offload
                .or(stored.gpu_offload)
                .unwrap_or(GpuOffloadMode::Off),
            ffmpeg_path: cli
                .ffmpeg_path
                .clone()
                .unwrap_or_else(|| "ffmpeg".to_string()),
            ffmpeg_extra_args: cli.ffmpeg_extra_arg.clone(),
            lame_path: cli.lame_path.clone(),
            temp_dir,
            bitrate_kbps: cli.bitrate_kbps,
            output_sample_rate: cli.output_sample_rate,
            output_channels: cli.output_channels,
            request_timeout_secs: cli.request_timeout_secs,
            temperature: cli.temperature,
            show_settings_on_launch: cli.show_settings || stored.show_settings,
            keep_audio: cli.keep_audio,
            copy_to_clipboard: !cli.disable_clipboard,
            word_timestamps: cli.word_timestamps,
            segment_timestamps: cli.segment_timestamps,
            hotkeys,
        })
    }

    pub fn encoder_settings(&self) -> EncoderSettings {
        EncoderSettings {
            format: self.encoder_format,
            mp3_encoder: self.mp3_encoder,
            ffmpeg_path: self.ffmpeg_path.clone(),
            ffmpeg_extra_args: self.ffmpeg_extra_args.clone(),
            lame_path: self.lame_path.clone(),
            gpu_offload: self.gpu_offload,
            temp_dir: self.temp_dir.clone(),
            bitrate_kbps: self.bitrate_kbps,
            output_sample_rate: self.output_sample_rate,
            output_channels: self.output_channels,
        }
    }

    pub fn transcriber_config(&self) -> Option<TranscriberConfig> {
        self.api_key.clone().map(|api_key| TranscriberConfig {
            api_key,
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            language: self.language.clone(),
            prompt: self.prompt.clone(),
            response_format: self.response_format,
            temperature: self.temperature,
            word_timestamps: self.word_timestamps,
            segment_timestamps: self.segment_timestamps,
            request_timeout_secs: self.request_timeout_secs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[test]
    fn timestamp_flags_force_verbose_json() {
        let cli = CliArgs::parse_from(["voice-app", "--word-timestamps"]);
        let config = AppConfig::from_sources(cli, &StoredAppState::default()).unwrap();
        assert_eq!(config.response_format, ResponseFormat::VerboseJson);
    }

    #[test]
    fn language_defaults_to_japanese() {
        let cli = CliArgs::parse_from(["voice-app"]);
        assert_eq!(cli.language, DEFAULT_LANGUAGE);

        let config = AppConfig::from_sources(cli, &StoredAppState::default()).unwrap();
        assert_eq!(config.language.as_deref(), Some(DEFAULT_LANGUAGE));
    }

    #[test]
    fn language_can_be_overridden_by_arg() {
        let cli = CliArgs::parse_from(["voice-app", "--language", "en"]);
        assert_eq!(cli.language, "en");

        let config = AppConfig::from_sources(cli, &StoredAppState::default()).unwrap();
        assert_eq!(config.language.as_deref(), Some("en"));
    }

    #[test]
    fn uses_persisted_device_when_cli_is_missing() {
        let cli = CliArgs::parse_from(["voice-app"]);
        let stored = StoredAppState {
            input_device: Some("Persisted Mic".to_string()),
            ..StoredAppState::default()
        };

        let config = AppConfig::from_sources(cli, &stored).unwrap();
        assert_eq!(config.input_device.as_deref(), Some("Persisted Mic"));
    }

    #[test]
    fn uses_persisted_model_and_gpu_when_cli_is_missing() {
        let cli = CliArgs::parse_from(["voice-app"]);
        let stored = StoredAppState {
            model: Some(WHISPER_MODEL_V3.to_string()),
            gpu_offload: Some(GpuOffloadMode::Auto),
            ..StoredAppState::default()
        };

        let config = AppConfig::from_sources(cli, &stored).unwrap();
        assert_eq!(config.model, WHISPER_MODEL_V3);
        assert_eq!(config.gpu_offload, GpuOffloadMode::Auto);
    }

    #[test]
    fn cli_model_and_gpu_override_persisted_settings() {
        let cli = CliArgs::parse_from([
            "voice-app",
            "--model",
            WHISPER_MODEL_V3_TURBO,
            "--gpu-offload",
            "cuda",
        ]);
        let stored = StoredAppState {
            model: Some(WHISPER_MODEL_V3.to_string()),
            gpu_offload: Some(GpuOffloadMode::Auto),
            ..StoredAppState::default()
        };

        let config = AppConfig::from_sources(cli, &stored).unwrap();
        assert_eq!(config.model, WHISPER_MODEL_V3_TURBO);
        assert_eq!(config.gpu_offload, GpuOffloadMode::Cuda);
    }

    #[test]
    fn cli_parses_mp3_encoder_backend() {
        // default は lame (ffmpeg libmp3lame 非対応環境があるため)
        let cli = CliArgs::parse_from(["voice-app"]);
        assert_eq!(cli.mp3_encoder, Mp3EncoderBackend::Lame);
        assert_eq!(cli.lame_path, "/usr/bin/lame");

        // --mp3-encoder ffmpeg の明示指定
        let cli = CliArgs::parse_from(["voice-app", "--mp3-encoder", "ffmpeg"]);
        assert_eq!(cli.mp3_encoder, Mp3EncoderBackend::Ffmpeg);

        // --mp3-encoder lame の明示指定
        let cli = CliArgs::parse_from(["voice-app", "--mp3-encoder", "lame"]);
        assert_eq!(cli.mp3_encoder, Mp3EncoderBackend::Lame);

        // 不正値は parse error
        let result = CliArgs::try_parse_from(["voice-app", "--mp3-encoder", "unknown"]);
        assert!(
            result.is_err(),
            "expected parse error for --mp3-encoder unknown"
        );

        // AppConfig への伝播
        let cli = CliArgs::parse_from([
            "voice-app",
            "--mp3-encoder",
            "ffmpeg",
            "--lame-path",
            "/tmp/lame",
        ]);
        let config = AppConfig::from_sources(cli, &StoredAppState::default()).unwrap();
        assert_eq!(config.mp3_encoder, Mp3EncoderBackend::Ffmpeg);
        assert_eq!(config.lame_path, "/tmp/lame");
    }

    #[test]
    fn help_does_not_expose_secret_env_values() {
        std::env::set_var("GROQ_API_KEY", "gsk_test_secret_that_must_not_appear");
        std::env::set_var("GROQ_WHISPER_PROMPT", "prompt_secret_that_must_not_appear");

        let mut help = Vec::new();
        CliArgs::command()
            .write_long_help(&mut help)
            .expect("render help");
        let help = String::from_utf8(help).expect("help is utf-8");

        std::env::remove_var("GROQ_API_KEY");
        std::env::remove_var("GROQ_WHISPER_PROMPT");

        assert!(
            !help.contains("gsk_test_secret_that_must_not_appear"),
            "help output must not include the API key env value"
        );
        assert!(
            !help.contains("prompt_secret_that_must_not_appear"),
            "help output must not include prompt env value"
        );
        assert!(
            help.contains("GROQ_API_KEY"),
            "help should still document the supported env variable name"
        );
    }

    #[test]
    fn cli_hotkey_override_ignores_persisted_toggle() {
        let cli = CliArgs::parse_from([
            "voice-app",
            "--start-hotkey",
            "Ctrl+S",
            "--stop-hotkey",
            "Ctrl+E",
        ]);

        let stored = StoredAppState::default();
        let config = AppConfig::from_sources(cli, &stored).unwrap();

        assert!(config.hotkeys.toggle.is_none());
        assert!(config.hotkeys.start.is_some());
        assert!(config.hotkeys.stop.is_some());
    }
}
