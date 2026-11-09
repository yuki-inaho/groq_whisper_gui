//! 実マイクと外部エンコーダを使わない音声パイプラインの BDD 形式結合テスト。

mod support;

use groq_whisper_app::config::EncoderFormat;
use std::fs;
use support::{create_wav, encoder_settings, TestDir};

#[test]
fn given_pcm_chunks_when_streamed_to_wav_then_file_preserves_every_sample() {
    // Given: 16 kHz / mono の決定的な PCM 1,600 サンプル。
    let temp_dir = TestDir::new("wav-roundtrip");
    let expected_samples: Vec<i16> = (0..1_600)
        .map(|index| ((index % 200) as i16) - 100)
        .collect();

    // When: 20 ms チャンクで WAV エンコーダへ流し、finish する。
    let audio = create_wav(temp_dir.path(), &expected_samples);

    // Then: 形式・サイズ・全サンプルが入力と一致する。
    assert_eq!(audio.format, EncoderFormat::Wav);
    assert_eq!(audio.mime(), "audio/wav");
    assert!(audio.path.exists());
    assert_eq!(audio.byte_len, fs::metadata(&audio.path).unwrap().len());

    let mut reader = hound::WavReader::open(&audio.path).expect("read generated WAV");
    assert_eq!(reader.spec().sample_rate, 16_000);
    assert_eq!(reader.spec().channels, 1);
    assert_eq!(reader.spec().bits_per_sample, 16);
    let actual_samples: Vec<i16> = reader
        .samples::<i16>()
        .collect::<Result<_, _>>()
        .expect("decode generated WAV samples");
    assert_eq!(actual_samples, expected_samples);
}

#[test]
fn given_three_channel_input_when_lame_is_selected_then_error_names_the_constraint() {
    // Given: lame バックエンドがサポートしない 3 チャンネル入力。
    let temp_dir = TestDir::new("unsupported-channels");
    let settings = encoder_settings(temp_dir.path(), EncoderFormat::Mp3);
    let input_spec = groq_whisper_app::encoder::AudioInputSpec {
        sample_rate: 48_000,
        channels: 3,
    };

    // When: エンコーダを生成する。
    let error = match groq_whisper_app::encoder::create_encoder(&settings, input_spec) {
        Ok(_) => panic!("3-channel lame input must be rejected"),
        Err(error) => error,
    };

    // Then: ffmpeg への暗黙 fallback はせず、制約と明示的な代替案を返す。
    let message = error.to_string();
    assert!(message.contains("only 1 or 2 channel input"));
    assert!(message.contains("--mp3-encoder ffmpeg"));
}
