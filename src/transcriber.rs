use crate::config::ResponseFormat;
use crate::encoder::EncodedAudio;
use anyhow::{anyhow, Context, Result};
use reqwest::blocking::multipart::{Form, Part};
use serde_json::Value;
use std::fs::File;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct TranscriberConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub response_format: ResponseFormat,
    pub temperature: f32,
    pub word_timestamps: bool,
    pub segment_timestamps: bool,
    pub request_timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    pub text: String,
    pub request_id: Option<String>,
    pub raw_response: Option<String>,
    pub elapsed: Duration,
}

pub fn transcribe_file(
    config: &TranscriberConfig,
    audio: &EncodedAudio,
) -> Result<TranscriptionResult> {
    let started_at = Instant::now();
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(config.request_timeout_secs))
        .build()
        .context("failed to create HTTP client")?;

    let file = File::open(&audio.path)
        .with_context(|| format!("failed to open audio file: {}", audio.path.display()))?;
    let part = Part::reader(file)
        .file_name(audio.file_name.clone())
        .mime_str(audio.mime())?;

    let mut form = Form::new()
        .text("model", config.model.clone())
        .text(
            "response_format",
            config.response_format.api_value().to_string(),
        )
        .text("temperature", config.temperature.to_string())
        .part("file", part);

    if let Some(language) = &config.language {
        form = form.text("language", language.clone());
    }

    if let Some(prompt) = &config.prompt {
        form = form.text("prompt", prompt.clone());
    }

    if config.word_timestamps {
        form = form.text("timestamp_granularities[]", "word");
    }

    if config.segment_timestamps {
        form = form.text("timestamp_granularities[]", "segment");
    }

    let url = format!(
        "{}/audio/transcriptions",
        config.base_url.trim_end_matches('/')
    );

    let response = client
        .post(url)
        .bearer_auth(&config.api_key)
        .multipart(form)
        .send()
        .context("failed to call Groq transcription endpoint")?
        .error_for_status()
        .context("Groq transcription request returned an error")?;

    let body = response.text().context("failed to read response body")?;
    parse_transcription_response(config.response_format, body, started_at.elapsed())
}

fn parse_transcription_response(
    response_format: ResponseFormat,
    body: String,
    elapsed: Duration,
) -> Result<TranscriptionResult> {
    if matches!(response_format, ResponseFormat::Text) {
        return Ok(TranscriptionResult {
            text: body,
            request_id: None,
            raw_response: None,
            elapsed,
        });
    }

    let value: Value = serde_json::from_str(&body)
        .with_context(|| format!("failed to parse JSON response: {body}"))?;

    let text = value
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Groq response did not include a text field"))?
        .to_string();

    let request_id = value
        .get("x_groq")
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    let raw_response = serde_json::to_string_pretty(&value).ok();

    Ok(TranscriptionResult {
        text,
        request_id,
        raw_response,
        elapsed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_response() {
        let response = parse_transcription_response(
            ResponseFormat::Json,
            r#"{"text":"hello","x_groq":{"id":"req_123"}}"#.to_string(),
            Duration::from_millis(25),
        )
        .unwrap();

        assert_eq!(response.text, "hello");
        assert_eq!(response.request_id.as_deref(), Some("req_123"));
    }
}
