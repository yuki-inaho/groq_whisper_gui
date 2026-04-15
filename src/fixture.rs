//! 手順 16: 実マイク非依存で `StreamingEncoder` を駆動するための PCM fixture helper。
//!
//! 目的: `data/test_rec.mp3` のような手元の MP3 を `ffmpeg` で raw PCM にデコードし、
//! 小さなチャンクに分割して `StreamingEncoder::write_samples` へ逐次投入することで、
//! cpal の実マイク入力を使わずに「録音中ストリーミング」相当の経路を単体テストで
//! 検証できるようにする。暗黙 fallback 禁止のため、ffmpeg の spawn / 読み取り /
//! exit code のいずれかが失敗した場合は必ず `bail!` でエラーを bubble up する。
//!
//! 固定スペック:
//! - 出力 raw: `s16le`, 48000 Hz, 1 ch (モノラル)
//! - 理由: 手順 14 で追加した `LameMp3Encoder` が 1 kHz 倍数の sample_rate のみ
//!   受け付ける制約と整合し、さらに本プロジェクトの想定キャプチャ spec とも一致する
//!   (cpal の典型 default が 48000 Hz)。
//!
//! 48000 Hz は手順 14 の `LameMp3Encoder` 制約 (整数 kHz) を満たすため、
//! 44100 Hz を使いたい場合はここではなく fixture 側を追加定義する方針とする
//! (暗黙丸め禁止)。

use anyhow::{bail, Context, Result};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::encoder::StreamingEncoder;

/// fixture helper が ffmpeg に要求する固定入力スペック。
/// `StreamingEncoder` へ流すときはこれを `AudioInputSpec` の値として使う。
pub const FIXTURE_SAMPLE_RATE_HZ: u32 = 48_000;
pub const FIXTURE_CHANNELS: u16 = 1;

/// `fixture_path` の音声ファイルを ffmpeg で raw PCM (s16le / 48 kHz / mono) に
/// デコードし、`chunk_frames` サンプル単位で `encoder.write_samples` に投入する。
///
/// 戻り値はストリーミングに流した**チャンクの総数**。呼び出し側 (テストやスモーク)
/// はこの値が 2 以上であることを確認することで「一度の write ではなくチャンク
/// 分割して流し込んだ」ことを検証できる (= 単なる一括 encode との差分を保証)。
///
/// 制約:
/// - `chunk_frames > 0` であること。0 の場合は無限ループになるため `bail!`。
/// - ffmpeg が終了コード 0 で終わること。非 0 の場合は `bail!`。
/// - デコード結果が空 (0 バイト) の場合は `bail!` (fixture として使い物にならない)。
/// - 合計チャンク数が 2 未満 (= 1 回の write_samples で終わるような短い fixture)
///   の場合は `bail!`。ストリーミングしていると言えないため。
///
/// `encoder` のライフサイクル (`finish()` の呼び出し) は呼び出し側責務。
/// 本関数は `write_samples` 呼び出しだけを行い、`finish` は呼ばない。
pub fn stream_fixture_through_encoder(
    fixture_path: &Path,
    encoder: &mut dyn StreamingEncoder,
    chunk_frames: usize,
) -> Result<u32> {
    if chunk_frames == 0 {
        bail!("chunk_frames must be greater than 0");
    }
    if !fixture_path.exists() {
        bail!(
            "fixture audio not found: {}. Place a readable MP3/WAV at this path.",
            fixture_path.display()
        );
    }

    // ffmpeg を明示引数で起動し、stdout に raw PCM (s16le / 48 kHz / mono) を流す。
    // loglevel=error + hide_banner で余計な出力を抑え、stderr は失敗時の診断用に
    // キャプチャする。
    let mut child = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(fixture_path)
        .arg("-vn")
        .arg("-f")
        .arg("s16le")
        .arg("-ar")
        .arg(FIXTURE_SAMPLE_RATE_HZ.to_string())
        .arg("-ac")
        .arg(FIXTURE_CHANNELS.to_string())
        .arg("pipe:1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "failed to spawn ffmpeg to decode fixture {}",
                fixture_path.display()
            )
        })?;

    let mut stdout = child
        .stdout
        .take()
        .context("failed to capture ffmpeg stdout for fixture decode")?;

    // chunk_frames サンプル (= chunk_frames * 2 バイト) 単位で stream する。
    let chunk_bytes = chunk_frames
        .checked_mul(2)
        .context("chunk_frames overflow")?;
    let mut raw = vec![0_u8; chunk_bytes];
    let mut total_bytes: usize = 0;
    let mut chunk_count: u32 = 0;

    loop {
        // read_exact ではなく read_to_end ではなく、各チャンクをタイトに読む。
        // EOF までに中途半端な残り (チャンク未満) が出た場合は最後のチャンクとして
        // 部分サイズで encoder に流す必要があるので、手動で read ループを回す。
        let mut filled = 0_usize;
        while filled < chunk_bytes {
            match stdout.read(&mut raw[filled..]) {
                Ok(0) => break, // EOF
                Ok(n) => filled += n,
                Err(err) => {
                    // stderr を拾って診断に加える。
                    let _ = child.kill();
                    let stderr = child
                        .wait_with_output()
                        .map(|out| String::from_utf8_lossy(&out.stderr).into_owned())
                        .unwrap_or_default();
                    bail!(
                        "failed to read ffmpeg stdout while streaming fixture: {err}. stderr: {}",
                        stderr.trim()
                    );
                }
            }
        }

        if filled == 0 {
            break;
        }

        // filled が 2 バイト境界でない場合は raw PCM として不整合なので明示エラー
        // (暗黙切り捨て禁止)。
        if !filled.is_multiple_of(2) {
            let _ = child.kill();
            bail!(
                "ffmpeg returned {} bytes which is not a multiple of 2 (s16le expects 16-bit samples)",
                filled
            );
        }

        let samples_in_chunk = filled / 2;
        let mut samples = Vec::with_capacity(samples_in_chunk);
        for pair in raw[..filled].chunks_exact(2) {
            samples.push(i16::from_le_bytes([pair[0], pair[1]]));
        }

        encoder
            .write_samples(&samples)
            .context("encoder.write_samples failed while streaming fixture")?;

        total_bytes += filled;
        chunk_count += 1;

        if filled < chunk_bytes {
            // 最後の不完全チャンクを書き込んだ。これ以上の読み込みは無い。
            break;
        }
    }

    // ffmpeg の exit code を確実に確認する (暗黙 fallback 禁止)。
    let status = child
        .wait()
        .context("failed to wait for ffmpeg to exit after fixture streaming")?;
    if !status.success() {
        // stderr を拾ってメッセージに含める (child は wait 済みなので stderr は
        // output() ではなく read で取り直す方式には戻せないが、ここまでで既に
        // stdout は drop 済みのため stderr は別途 take する)。
        bail!(
            "ffmpeg exited with non-zero status {:?} while decoding fixture {}",
            status.code(),
            fixture_path.display()
        );
    }

    if total_bytes == 0 {
        bail!(
            "ffmpeg produced no PCM data for fixture {} — the file may be empty or unreadable",
            fixture_path.display()
        );
    }
    if chunk_count < 2 {
        bail!(
            "fixture produced only {} chunk (needs >= 2 to validate streaming behavior). \
             Use a smaller chunk_frames or a longer fixture audio.",
            chunk_count
        );
    }

    Ok(chunk_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EncoderFormat, GpuOffloadMode, Mp3EncoderBackend};
    use crate::encoder::{create_encoder, AudioInputSpec, EncoderSettings};
    use std::fs;
    use std::path::PathBuf;

    /// 手順 16 の受入テスト: `data/test_rec.mp3` を raw PCM にデコードし、
    /// 20 ms (= 960 frames @ 48 kHz mono) チャンクで `LameMp3Encoder` に流し、
    /// 次の 2 点を確認する。
    ///
    /// - チャンク数が 2 以上であること
    /// - 出力 MP3 のバイト長が 0 より大きいこと
    ///
    /// `#[ignore]` 理由: 実機の `ffmpeg` / `/usr/bin/lame` / `data/test_rec.mp3`
    /// に依存するため、CI のデフォルト `cargo test` からは除外し、手順 22 と同じ
    /// タイミングで `cargo test -- --ignored` 経由で実行する。
    #[test]
    #[ignore]
    fn fixture_audio_streams_in_chunks() {
        // data/test_rec.mp3 は repo 直下からの相対パス。cargo test の CWD は
        // crate root なので PathBuf::from("data/test_rec.mp3") で到達可能。
        let fixture = PathBuf::from("data/test_rec.mp3");
        assert!(
            fixture.exists(),
            "fixture must exist at {}. Skipping via #[ignore] if absent is not sufficient; \
             this test is wired to require the file.",
            fixture.display()
        );

        let temp_dir = std::env::temp_dir().join("groq-whisper-tests-fixture");
        fs::create_dir_all(&temp_dir).expect("create temp dir");

        let settings = EncoderSettings {
            format: EncoderFormat::Mp3,
            mp3_encoder: Mp3EncoderBackend::Lame,
            ffmpeg_path: "ffmpeg".to_string(),
            ffmpeg_extra_args: Vec::new(),
            lame_path: "/usr/bin/lame".to_string(),
            gpu_offload: GpuOffloadMode::Off,
            temp_dir: temp_dir.clone(),
            bitrate_kbps: 48,
            output_sample_rate: 16_000,
            output_channels: 1,
        };

        let input_spec = AudioInputSpec {
            sample_rate: FIXTURE_SAMPLE_RATE_HZ,
            channels: FIXTURE_CHANNELS,
        };

        let mut encoder =
            create_encoder(&settings, input_spec).expect("create LameMp3Encoder for fixture");

        // 20 ms @ 48 kHz mono = 960 frames
        let chunk_frames = (FIXTURE_SAMPLE_RATE_HZ as usize) * 20 / 1000;
        assert_eq!(chunk_frames, 960);

        let chunks = stream_fixture_through_encoder(&fixture, encoder.as_mut(), chunk_frames)
            .expect("stream fixture into encoder");

        assert!(
            chunks >= 2,
            "expected at least 2 chunks to validate streaming behavior, got {chunks}"
        );

        let encoded = encoder
            .finish()
            .expect("finish encoder after fixture stream");
        assert!(
            encoded.byte_len > 0,
            "encoded mp3 must be non-empty, got {} bytes",
            encoded.byte_len
        );
        assert!(
            encoded.path.exists(),
            "encoded file must exist at {}",
            encoded.path.display()
        );
    }
}
