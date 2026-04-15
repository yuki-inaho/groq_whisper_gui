use crate::encoder::{create_encoder, AudioInputSpec, EncodedAudio, EncoderSettings};
use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::sync::{
    atomic::{AtomicU32, AtomicU64, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct InputDeviceInfo {
    pub name: String,
    pub default_sample_rate: u32,
    pub channels: u16,
    pub is_default: bool,
}

impl InputDeviceInfo {
    pub fn label(&self) -> String {
        let default_prefix = if self.is_default { "[default] " } else { "" };
        format!(
            "{default_prefix}{} ({} ch / {} Hz)",
            self.name, self.channels, self.default_sample_rate
        )
    }
}

#[derive(Debug, Clone)]
pub struct AudioCaptureRequest {
    pub preferred_device_name: Option<String>,
    pub encoder_settings: EncoderSettings,
}

#[derive(Debug)]
pub struct CompletedRecording {
    pub device_name: String,
    pub input_sample_rate: u32,
    pub input_channels: u16,
    pub captured_samples: u64,
    pub duration: Duration,
    pub encoded_audio: EncodedAudio,
}

pub struct RecordingSession {
    stream: Option<cpal::Stream>,
    sender: Option<Sender<Vec<i16>>>,
    worker: Option<JoinHandle<Result<EncodedAudio>>>,
    level: Arc<AtomicU32>,
    captured_samples: Arc<AtomicU64>,
    started_at: Instant,
    device_name: String,
    input_sample_rate: u32,
    input_channels: u16,
}

impl RecordingSession {
    pub fn level(&self) -> f32 {
        self.level.load(Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub fn stop(mut self) -> Result<CompletedRecording> {
        if let Some(stream) = self.stream.take() {
            drop(stream);
        }

        if let Some(sender) = self.sender.take() {
            drop(sender);
        }

        let encoded_audio = match self.worker.take() {
            Some(handle) => handle
                .join()
                .map_err(|_| anyhow!("audio encoding worker panicked"))??,
            None => bail!("recording worker is not available"),
        };

        Ok(CompletedRecording {
            device_name: self.device_name,
            input_sample_rate: self.input_sample_rate,
            input_channels: self.input_channels,
            captured_samples: self.captured_samples.load(Ordering::Relaxed),
            duration: self.started_at.elapsed(),
            encoded_audio,
        })
    }
}

pub fn list_input_devices() -> Result<Vec<InputDeviceInfo>> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|device| device.name().ok());

    let mut devices = Vec::new();
    let input_devices = host
        .input_devices()
        .context("failed to enumerate input devices")?;

    for device in input_devices {
        let name = device
            .name()
            .unwrap_or_else(|_| "Unknown input device".to_string());

        let config = match device.default_input_config() {
            Ok(value) => value,
            Err(_) => continue,
        };

        devices.push(InputDeviceInfo {
            is_default: default_name.as_deref() == Some(name.as_str()),
            name,
            default_sample_rate: config.sample_rate().0,
            channels: config.channels(),
        });
    }

    devices.sort_by(|left, right| left.name.cmp(&right.name));
    devices.sort_by_key(|device| !device.is_default);

    Ok(devices)
}

pub fn start_recording(request: AudioCaptureRequest) -> Result<RecordingSession> {
    let host = cpal::default_host();
    let device = resolve_device(&host, request.preferred_device_name.as_deref())?;
    let device_name = device
        .name()
        .unwrap_or_else(|_| "Unknown input device".to_string());
    let config = device
        .default_input_config()
        .with_context(|| format!("failed to get default input config for {device_name}"))?;

    let stream_config: cpal::StreamConfig = config.clone().into();
    let input_spec = AudioInputSpec {
        sample_rate: stream_config.sample_rate.0,
        channels: stream_config.channels,
    };

    let (sender, receiver) = unbounded::<Vec<i16>>();
    let worker_settings = request.encoder_settings.clone();
    let worker = thread::spawn(move || worker_main(worker_settings, input_spec, receiver));

    let level = Arc::new(AtomicU32::new(0));
    let captured_samples = Arc::new(AtomicU64::new(0));

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => build_input_stream::<f32>(
            &device,
            &stream_config,
            sender.clone(),
            level.clone(),
            captured_samples.clone(),
        )?,
        cpal::SampleFormat::I16 => build_input_stream::<i16>(
            &device,
            &stream_config,
            sender.clone(),
            level.clone(),
            captured_samples.clone(),
        )?,
        cpal::SampleFormat::U16 => build_input_stream::<u16>(
            &device,
            &stream_config,
            sender.clone(),
            level.clone(),
            captured_samples.clone(),
        )?,
        sample_format => bail!("unsupported sample format: {sample_format:?}"),
    };

    stream.play().context("failed to start input stream")?;

    Ok(RecordingSession {
        stream: Some(stream),
        sender: Some(sender),
        worker: Some(worker),
        level,
        captured_samples,
        started_at: Instant::now(),
        device_name,
        input_sample_rate: input_spec.sample_rate,
        input_channels: input_spec.channels,
    })
}

fn resolve_device(host: &cpal::Host, preferred_name: Option<&str>) -> Result<cpal::Device> {
    if let Some(preferred_name) = preferred_name {
        for device in host.input_devices()? {
            if let Ok(name) = device.name() {
                if name == preferred_name {
                    return Ok(device);
                }
            }
        }

        bail!("requested input device not found: {preferred_name}");
    }

    host.default_input_device()
        .context("no default input device is available")
}

fn worker_main(
    encoder_settings: EncoderSettings,
    input_spec: AudioInputSpec,
    receiver: Receiver<Vec<i16>>,
) -> Result<EncodedAudio> {
    let mut encoder = create_encoder(&encoder_settings, input_spec)?;
    while let Ok(chunk) = receiver.recv() {
        encoder.write_samples(&chunk)?;
    }
    encoder.finish()
}

fn build_input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sender: Sender<Vec<i16>>,
    level: Arc<AtomicU32>,
    captured_samples: Arc<AtomicU64>,
) -> Result<cpal::Stream>
where
    T: cpal::Sample + cpal::SizedSample + Copy,
    i16: cpal::FromSample<T>,
{
    let err_fn = move |error| {
        eprintln!("audio input stream error: {error}");
    };

    let stream = device.build_input_stream(
        config,
        move |data: &[T], _| {
            let mut converted = Vec::with_capacity(data.len());
            let mut peak = 0.0_f32;

            for sample in data.iter().copied() {
                let value: i16 = i16::from_sample(sample);
                let amplitude =
                    ((value as i32).abs().min(i16::MAX as i32) as f32) / (i16::MAX as f32);

                if amplitude > peak {
                    peak = amplitude;
                }

                converted.push(value);
            }

            level.store((peak * 1000.0) as u32, Ordering::Relaxed);
            captured_samples.fetch_add(converted.len() as u64, Ordering::Relaxed);

            if sender.send(converted).is_err() {
                eprintln!("audio worker receiver dropped");
            }
        },
        err_fn,
        None,
    )?;

    Ok(stream)
}
