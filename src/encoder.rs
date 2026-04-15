use crate::config::{EncoderFormat, GpuOffloadMode, Mp3EncoderBackend};
use anyhow::{anyhow, bail, Context, Result};
use chrono::Local;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

#[derive(Debug, Clone)]
pub struct EncoderSettings {
    pub format: EncoderFormat,
    pub mp3_encoder: Mp3EncoderBackend,
    pub ffmpeg_path: String,
    pub ffmpeg_extra_args: Vec<String>,
    pub lame_path: String,
    pub gpu_offload: GpuOffloadMode,
    pub temp_dir: PathBuf,
    pub bitrate_kbps: u32,
    pub output_sample_rate: u32,
    pub output_channels: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct AudioInputSpec {
    pub sample_rate: u32,
    pub channels: u16,
}

#[derive(Debug, Clone)]
pub struct EncodedAudio {
    pub path: PathBuf,
    pub format: EncoderFormat,
    pub file_name: String,
    pub byte_len: u64,
}

impl EncodedAudio {
    /// `format` から一貫して導出される MIME type。
    /// `EncoderFormat::mime()` を薄くラップしており、`mime` 文字列を
    /// 別フィールドとして保持する冗長性 (DRY 違反) を避ける目的。
    pub fn mime(&self) -> &'static str {
        self.format.mime()
    }
}

pub trait StreamingEncoder: Send {
    fn write_samples(&mut self, samples: &[i16]) -> Result<()>;
    fn finish(self: Box<Self>) -> Result<EncodedAudio>;
}

pub fn create_encoder(
    settings: &EncoderSettings,
    input_spec: AudioInputSpec,
) -> Result<Box<dyn StreamingEncoder>> {
    fs::create_dir_all(&settings.temp_dir).with_context(|| {
        format!(
            "failed to create temp audio directory: {}",
            settings.temp_dir.display()
        )
    })?;

    match settings.format {
        EncoderFormat::Mp3 => {
            let output_path = next_output_path(&settings.temp_dir, settings.format.extension());
            match settings.mp3_encoder {
                Mp3EncoderBackend::Ffmpeg => Ok(Box::new(FfmpegMp3Encoder::new(
                    settings,
                    input_spec,
                    output_path,
                )?)),
                Mp3EncoderBackend::Lame => Ok(Box::new(LameMp3Encoder::new(
                    settings,
                    input_spec,
                    output_path,
                )?)),
            }
        }
        EncoderFormat::Wav => {
            let output_path = next_output_path(&settings.temp_dir, settings.format.extension());
            Ok(Box::new(WavEncoder::new(
                settings,
                input_spec,
                output_path,
            )?))
        }
    }
}

/// i16 サンプル列をリトルエンディアン生バイト列に詰め直す。
/// ffmpeg / lame の raw PCM stdin 書き込みで共通に利用する DRY ヘルパー。
fn i16_samples_to_le_bytes(samples: &[i16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn lame_sample_rate_arg(sample_rate_hz: u32) -> String {
    let khz_integer = sample_rate_hz / 1000;
    let khz_fraction = sample_rate_hz % 1000;

    if khz_fraction == 0 {
        return khz_integer.to_string();
    }

    let mut fraction = format!("{khz_fraction:03}");
    while fraction.ends_with('0') {
        fraction.pop();
    }
    format!("{khz_integer}.{fraction}")
}

fn next_output_path(temp_dir: &Path, extension: &str) -> PathBuf {
    let stamp = Local::now().format("%Y%m%d_%H%M%S_%3f");
    temp_dir.join(format!("capture_{stamp}.{extension}"))
}

struct FfmpegMp3Encoder {
    output_path: PathBuf,
    child: Child,
    stdin: Option<ChildStdin>,
}

impl FfmpegMp3Encoder {
    fn new(
        settings: &EncoderSettings,
        input_spec: AudioInputSpec,
        output_path: PathBuf,
    ) -> Result<Self> {
        let mut command = Command::new(&settings.ffmpeg_path);
        command
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-f")
            .arg("s16le")
            .arg("-ar")
            .arg(input_spec.sample_rate.to_string())
            .arg("-ac")
            .arg(input_spec.channels.to_string())
            .arg("-i")
            .arg("pipe:0")
            .arg("-vn")
            .arg("-ac")
            .arg(settings.output_channels.to_string())
            .arg("-ar")
            .arg(settings.output_sample_rate.to_string())
            .arg("-codec:a")
            .arg("libmp3lame")
            .arg("-b:a")
            .arg(format!("{}k", settings.bitrate_kbps))
            .arg("-write_xing")
            .arg("0");

        // 注記:
        // 音声専用 MP3 パイプラインに対して GPU オフロードで実効的な速度改善を
        // 得られるケースは限定的です。そのためここでは安全なデフォルトのみを採用し、
        // ベンダー依存の調整は --ffmpeg-extra-arg で明示的に渡す拡張点として残します
        // (現在 `settings.gpu_offload` はここでは参照せず、UI / ログ表示でのみ使用)。
        let _ = settings.gpu_offload;

        for arg in &settings.ffmpeg_extra_args {
            command.arg(arg);
        }

        command
            .arg("-y")
            .arg(&output_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to launch ffmpeg. Ensure '{}' is installed and available in PATH",
                settings.ffmpeg_path
            )
        })?;

        let stdin = child.stdin.take().context("failed to open ffmpeg stdin")?;

        Ok(Self {
            output_path,
            child,
            stdin: Some(stdin),
        })
    }
}

impl StreamingEncoder for FfmpegMp3Encoder {
    fn write_samples(&mut self, samples: &[i16]) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .context("ffmpeg stdin is already closed")?;
        stdin.write_all(&i16_samples_to_le_bytes(samples))?;
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<EncodedAudio> {
        if let Some(mut stdin) = self.stdin.take() {
            stdin.flush().ok();
            drop(stdin);
        }

        let output = self.child.wait_with_output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // 手順 15: libmp3lame が組み込まれていない ffmpeg ビルドを使うと
            // `Unknown encoder 'libmp3lame'` で失敗する。この失敗は暗黙 fallback
            // させず、明示的に ffmpeg バックエンドの libmp3lame 非対応として
            // bubble up する (--mp3-encoder lame への切替はユーザーの明示選択で行う)。
            if stderr.contains("Unknown encoder 'libmp3lame'") {
                return Err(anyhow!(
                    "ffmpeg does not support libmp3lame (--mp3-encoder ffmpeg) on this system: {}. \
                     Re-run with --mp3-encoder lame to use the stand-alone lame binary instead.",
                    stderr.trim()
                ));
            }
            bail!("ffmpeg failed during mp3 encoding: {}", stderr);
        }

        let metadata = fs::metadata(&self.output_path)
            .with_context(|| format!("failed to inspect {}", self.output_path.display()))?;

        Ok(EncodedAudio {
            path: self.output_path.clone(),
            format: EncoderFormat::Mp3,
            file_name: self
                .output_path
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_else(|| "capture.mp3".to_string()),
            byte_len: metadata.len(),
        })
    }
}

/// `lame` コマンドへ raw PCM を stdin pipe し、16 kHz / mono / 48 kbps MP3 を
/// 生成するストリーミングエンコーダ。暗黙 fallback 禁止: ffmpeg backend とは
/// 完全に独立し、ユーザーが `--mp3-encoder lame` を明示選択したときのみ使われる。
struct LameMp3Encoder {
    output_path: PathBuf,
    child: Child,
    stdin: Option<ChildStdin>,
}

impl LameMp3Encoder {
    fn new(
        settings: &EncoderSettings,
        input_spec: AudioInputSpec,
        output_path: PathBuf,
    ) -> Result<Self> {
        if input_spec.sample_rate == 0 {
            bail!("LameMp3Encoder requires a non-zero input sample rate");
        }

        let mode_arg: &str = match input_spec.channels {
            1 => "m", // mono
            2 => "m", // stereo を mono にダウンミックス (-a と併用)
            other => bail!(
                "LameMp3Encoder supports only 1 or 2 channel input, got {other} channels. \
                 Re-run with --mp3-encoder ffmpeg or configure the input device to mono/stereo."
            ),
        };

        let input_khz = lame_sample_rate_arg(input_spec.sample_rate);
        let output_khz = lame_sample_rate_arg(settings.output_sample_rate);

        let mut command = Command::new(&settings.lame_path);
        command
            .arg("--quiet")
            .arg("-r")
            .arg("-s")
            .arg(input_khz)
            .arg("--signed")
            .arg("--little-endian")
            .arg("--resample")
            .arg(output_khz)
            .arg("-m")
            .arg(mode_arg);

        if input_spec.channels == 2 {
            // stereo 入力を mono に集約するオプション (lame 3.100: -a)
            command.arg("-a");
        }

        command
            .arg("-b")
            .arg(settings.bitrate_kbps.to_string())
            .arg("-")
            .arg(&output_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to launch lame. Ensure '{}' is installed and executable. \
                 Override the path with --lame-path or GROQ_LAME_PATH.",
                settings.lame_path
            )
        })?;

        let stdin = child.stdin.take().context("failed to open lame stdin")?;

        Ok(Self {
            output_path,
            child,
            stdin: Some(stdin),
        })
    }
}

impl StreamingEncoder for LameMp3Encoder {
    fn write_samples(&mut self, samples: &[i16]) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .context("lame stdin is already closed")?;
        stdin.write_all(&i16_samples_to_le_bytes(samples))?;
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<EncodedAudio> {
        if let Some(mut stdin) = self.stdin.take() {
            stdin.flush().ok();
            drop(stdin);
        }

        let output = self.child.wait_with_output()?;
        if !output.status.success() {
            bail!(
                "lame failed during mp3 encoding: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let metadata = fs::metadata(&self.output_path)
            .with_context(|| format!("failed to inspect {}", self.output_path.display()))?;

        Ok(EncodedAudio {
            path: self.output_path.clone(),
            format: EncoderFormat::Mp3,
            file_name: self
                .output_path
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_else(|| "capture.mp3".to_string()),
            byte_len: metadata.len(),
        })
    }
}

struct WavEncoder {
    output_path: PathBuf,
    writer: hound::WavWriter<BufWriter<File>>,
}

impl WavEncoder {
    fn new(
        _settings: &EncoderSettings,
        input_spec: AudioInputSpec,
        output_path: PathBuf,
    ) -> Result<Self> {
        let spec = hound::WavSpec {
            channels: input_spec.channels,
            sample_rate: input_spec.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let writer = hound::WavWriter::create(&output_path, spec)
            .with_context(|| format!("failed to create {}", output_path.display()))?;

        Ok(Self {
            output_path,
            writer,
        })
    }
}

impl StreamingEncoder for WavEncoder {
    fn write_samples(&mut self, samples: &[i16]) -> Result<()> {
        for sample in samples {
            self.writer.write_sample(*sample)?;
        }
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<EncodedAudio> {
        let this = *self;
        this.writer.finalize()?;

        let metadata = fs::metadata(&this.output_path)
            .with_context(|| format!("failed to inspect {}", this.output_path.display()))?;

        Ok(EncodedAudio {
            path: this.output_path.clone(),
            format: EncoderFormat::Wav,
            file_name: this
                .output_path
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_else(|| "capture.wav".to_string()),
            byte_len: metadata.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn creates_unique_mp3_path_in_temp_dir() {
        let dir = std::env::temp_dir().join("groq-whisper-tests");
        let path = next_output_path(&dir, "mp3");
        assert_eq!(path.extension().unwrap().to_string_lossy(), "mp3");
    }

    #[test]
    fn lame_sample_rate_arg_keeps_common_device_rates() {
        assert_eq!(lame_sample_rate_arg(16_000), "16");
        assert_eq!(lame_sample_rate_arg(44_100), "44.1");
        assert_eq!(lame_sample_rate_arg(22_050), "22.05");
        assert_eq!(lame_sample_rate_arg(11_025), "11.025");
        assert_eq!(lame_sample_rate_arg(48_000), "48");
    }

    /// 16 kHz / mono / 48 kbps MP3 を生成する想定の raw PCM を、
    /// 実機の `/usr/bin/lame` に pipe して `ffprobe` で検証する。
    ///
    /// `#[ignore]` 理由: 実行時に `/usr/bin/lame` と `/usr/local/bin/ffprobe` を
    /// 必要とするため、普通の `cargo test` では skip し、手順 22
    /// (smoke-encode) と同じタイミングで `cargo test -- --ignored` から
    /// 実行する。CI 環境に lame/ffprobe が無い場合は明示的 fail させる。
    #[test]
    #[ignore]
    fn streaming_lame_encoder_writes_valid_mp3() {
        let temp_dir = std::env::temp_dir().join("groq-whisper-tests-lame");
        fs::create_dir_all(&temp_dir).expect("create temp dir");

        let output_path = temp_dir.join("streaming_lame_encoder_writes_valid_mp3.mp3");
        if output_path.exists() {
            fs::remove_file(&output_path).expect("clean previous output");
        }

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

        // 48 kHz mono を 1 秒分、440 Hz サイン波で生成する (2^15 - 1 の 25% 振幅)。
        let sample_rate = 48_000_u32;
        let input_spec = AudioInputSpec {
            sample_rate,
            channels: 1,
        };
        let mut encoder: Box<dyn StreamingEncoder> = Box::new(
            LameMp3Encoder::new(&settings, input_spec, output_path.clone())
                .expect("spawn lame. Ensure /usr/bin/lame exists."),
        );

        let total_samples = sample_rate as usize; // 1 秒分
        let amplitude = (i16::MAX as f32 * 0.25) as i16;
        let chunk = 960_usize; // 20 ms @ 48 kHz mono
        let mut phase: f32 = 0.0;
        let phase_step = std::f32::consts::TAU * 440.0 / sample_rate as f32;

        let mut written = 0_usize;
        while written < total_samples {
            let take = chunk.min(total_samples - written);
            let mut buf = Vec::with_capacity(take);
            for _ in 0..take {
                buf.push((phase.sin() * amplitude as f32) as i16);
                phase += phase_step;
            }
            encoder.write_samples(&buf).expect("write chunk");
            written += take;
        }

        let encoded = encoder.finish().expect("finish lame encoder");
        assert!(encoded.byte_len > 0, "encoded mp3 must be non-empty");
        assert_eq!(encoded.format, EncoderFormat::Mp3);
        assert!(output_path.exists(), "output file must exist");

        // ffprobe で MP3 メタデータを検証
        let probe = Command::new("/usr/local/bin/ffprobe")
            .arg("-v")
            .arg("error")
            .arg("-of")
            .arg("json")
            .arg("-show_entries")
            .arg("stream=codec_name,sample_rate,channels,bit_rate")
            .arg(&output_path)
            .output()
            .expect("ffprobe must be executable");
        assert!(
            probe.status.success(),
            "ffprobe failed: {}",
            String::from_utf8_lossy(&probe.stderr)
        );
        let stdout = String::from_utf8(probe.stdout).expect("ffprobe stdout utf-8");
        assert!(
            stdout.contains("\"codec_name\": \"mp3\""),
            "expected codec_name=mp3, got: {stdout}"
        );
        assert!(
            stdout.contains("\"sample_rate\": \"16000\""),
            "expected sample_rate=16000, got: {stdout}"
        );
        assert!(
            stdout.contains("\"channels\": 1"),
            "expected channels=1, got: {stdout}"
        );
        // lame は平均 bit_rate を返す。48 kbps 近傍 (46000..=52000) を許容。
        let has_bitrate =
            (46_000..=52_000).any(|target| stdout.contains(&format!("\"bit_rate\": \"{target}\"")));
        assert!(has_bitrate, "expected bit_rate near 48000, got: {stdout}");
    }

    /// 手順 15: 本環境の `/usr/local/bin/ffmpeg` が `libmp3lame` 非対応である
    /// ことを想定し、既定引数で `FfmpegMp3Encoder` を走らせたときに
    /// `anyhow!("ffmpeg does not support libmp3lame ...")` 形式の明示的エラーが
    /// 返ることを確認する。暗黙 fallback (lame へ自動切替) していないことも
    /// エラー内容で保証する。
    ///
    /// `#[ignore]` 理由: `--enable-libmp3lame` 付き ffmpeg を使っている環境では
    /// 成功してしまうので、フレーキー回避のため ignored。手順 22 のタイミングで
    /// `cargo test -- --ignored` 経由で実行する (本環境は non-libmp3lame なので pass)。
    #[test]
    #[ignore]
    fn ffmpeg_backend_reports_libmp3lame_failure_explicitly() {
        let temp_dir = std::env::temp_dir().join("groq-whisper-tests-ffmpeg-lame");
        fs::create_dir_all(&temp_dir).expect("create temp dir");
        let output_path = temp_dir.join("ffmpeg_backend_reports_libmp3lame_failure.mp3");
        if output_path.exists() {
            fs::remove_file(&output_path).expect("clean previous output");
        }

        let settings = EncoderSettings {
            format: EncoderFormat::Mp3,
            mp3_encoder: Mp3EncoderBackend::Ffmpeg,
            ffmpeg_path: "/usr/local/bin/ffmpeg".to_string(),
            ffmpeg_extra_args: Vec::new(),
            lame_path: "/usr/bin/lame".to_string(),
            gpu_offload: GpuOffloadMode::Off,
            temp_dir,
            bitrate_kbps: 48,
            output_sample_rate: 16_000,
            output_channels: 1,
        };

        let input_spec = AudioInputSpec {
            sample_rate: 48_000,
            channels: 1,
        };

        let mut encoder: Box<dyn StreamingEncoder> = Box::new(
            FfmpegMp3Encoder::new(&settings, input_spec, output_path.clone())
                .expect("spawn ffmpeg. Ensure /usr/local/bin/ffmpeg exists."),
        );

        let silent: Vec<i16> = vec![0; 1_000];
        let _ = encoder.write_samples(&silent);

        let err = encoder
            .finish()
            .expect_err("ffmpeg without libmp3lame must fail explicitly");
        let message = format!("{err:#}");
        assert!(
            message.contains("libmp3lame"),
            "expected error to mention libmp3lame, got: {message}"
        );
        assert!(
            message.contains("--mp3-encoder lame"),
            "expected error to suggest --mp3-encoder lame, got: {message}"
        );
    }

    /// 手順 15: 不正な codec 名で `FfmpegMp3Encoder` を走らせ、stderr の中身が
    /// 具体的なエラーとして伝播することを確認する (`libmp3lame` 以外の失敗経路)。
    ///
    /// `ffmpeg_extra_args` の後段に `-codec:a nonexistent_codec` を渡すことで
    /// ffmpeg 側の codec 指定を強制的に上書きし、`Unknown encoder` を意図的に
    /// 発生させる (暗黙 fallback が起きていないことを確認するための negative test)。
    ///
    /// `#[ignore]` 理由: 実機の `/usr/local/bin/ffmpeg` を spawn するため、
    /// CI で ffmpeg が無い場合のフレーキーを避ける。手順 22 のタイミングで
    /// `cargo test -- --ignored` 経由で実行する。
    #[test]
    #[ignore]
    fn ffmpeg_backend_does_not_implicitly_fallback() {
        let temp_dir = std::env::temp_dir().join("groq-whisper-tests-ffmpeg");
        fs::create_dir_all(&temp_dir).expect("create temp dir");
        let output_path = temp_dir.join("ffmpeg_backend_does_not_implicitly_fallback.mp3");
        if output_path.exists() {
            fs::remove_file(&output_path).expect("clean previous output");
        }

        let settings = EncoderSettings {
            format: EncoderFormat::Mp3,
            mp3_encoder: Mp3EncoderBackend::Ffmpeg,
            ffmpeg_path: "/usr/local/bin/ffmpeg".to_string(),
            // 最後の -codec:a で上書きして Unknown encoder を誘発する
            ffmpeg_extra_args: vec!["-codec:a".to_string(), "nonexistent_codec".to_string()],
            lame_path: "/usr/bin/lame".to_string(),
            gpu_offload: GpuOffloadMode::Off,
            temp_dir,
            bitrate_kbps: 48,
            output_sample_rate: 16_000,
            output_channels: 1,
        };

        let input_spec = AudioInputSpec {
            sample_rate: 48_000,
            channels: 1,
        };

        let mut encoder: Box<dyn StreamingEncoder> = Box::new(
            FfmpegMp3Encoder::new(&settings, input_spec, output_path.clone())
                .expect("spawn ffmpeg. Ensure /usr/local/bin/ffmpeg exists."),
        );

        // 短い無音バッファを書き込む (1 チャンクで十分)
        let silent: Vec<i16> = vec![0; 48_000];
        // write_samples 自体は stdin write なので成功する可能性がある。
        // ffmpeg 側の失敗は finish() で検出される。
        let _ = encoder.write_samples(&silent);

        let result = encoder.finish();
        let err = result.expect_err("ffmpeg with nonexistent_codec must fail explicitly");
        let message = format!("{err:#}");
        // 暗黙 fallback していれば別 backend が成功して Ok を返すはず。
        // 失敗エラーには codec 名または ffmpeg のエラー文字列が含まれること。
        assert!(
            message.contains("nonexistent_codec") || message.contains("Unknown encoder"),
            "expected ffmpeg failure to mention the bad codec, got: {message}"
        );
        // 出力ファイルは作られない (または空)
        assert!(
            !output_path.exists()
                || fs::metadata(&output_path)
                    .map(|m| m.len() == 0)
                    .unwrap_or(true),
            "ffmpeg failure must not leave a populated mp3"
        );
    }
}
