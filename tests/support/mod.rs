//! 結合テスト共通の一時ディレクトリと音声 fixture。
//!
//! 実マイク・外部 CLI・実 API を使わず、各シナリオが同じ手順で再現できるようにする。

use groq_whisper_app::config::{EncoderFormat, GpuOffloadMode, Mp3EncoderBackend};
use groq_whisper_app::encoder::{create_encoder, AudioInputSpec, EncodedAudio, EncoderSettings};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct TestDir(PathBuf);

impl TestDir {
    pub fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "groq-whisper-bdd-{label}-{}-{nonce}",
            std::process::id()
        ));
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn encoder_settings(temp_dir: &Path, format: EncoderFormat) -> EncoderSettings {
    EncoderSettings {
        format,
        mp3_encoder: Mp3EncoderBackend::Lame,
        ffmpeg_path: "ffmpeg".to_string(),
        ffmpeg_extra_args: Vec::new(),
        lame_path: "/usr/bin/lame".to_string(),
        gpu_offload: GpuOffloadMode::Off,
        temp_dir: temp_dir.to_path_buf(),
        bitrate_kbps: 48,
        output_sample_rate: 16_000,
        output_channels: 1,
    }
}

/// 16 kHz / mono PCM を 20 ms チャンクで実際の WAV エンコーダへ流す。
pub fn create_wav(temp_dir: &Path, samples: &[i16]) -> EncodedAudio {
    let settings = encoder_settings(temp_dir, EncoderFormat::Wav);
    let input_spec = AudioInputSpec {
        sample_rate: 16_000,
        channels: 1,
    };
    let mut encoder = create_encoder(&settings, input_spec).expect("create WAV encoder");
    for chunk in samples.chunks(160) {
        encoder.write_samples(chunk).expect("write WAV samples");
    }
    encoder.finish().expect("finalize test WAV")
}
