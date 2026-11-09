//! ローカル mock Groq サーバを使った文字起こし HTTP 契約の BDD 形式結合テスト。
//!
//! 実 API キー・実マイク・外部エンコーダを必要としない。WAV は実装と同じ
//! `StreamingEncoder` で生成し、multipart 送信から応答解析までを一通り通す。

mod support;

use groq_whisper_app::config::ResponseFormat;
use groq_whisper_app::encoder::EncodedAudio;
use groq_whisper_app::transcriber::{transcribe_file, TranscriberConfig};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use support::{create_wav, TestDir};

fn create_test_wav(temp_dir: &Path) -> EncodedAudio {
    let samples: Vec<i16> = (0..1_600).map(|index| (index % 100) as i16).collect();
    let mut audio = create_wav(temp_dir, &samples);
    audio.file_name = "bdd-input.wav".to_string();
    audio
}

fn transcriber_config(base_url: String, response_format: ResponseFormat) -> TranscriberConfig {
    TranscriberConfig {
        api_key: "test-api-key".to_string(),
        base_url,
        model: "whisper-large-v3-turbo".to_string(),
        language: Some("ja".to_string()),
        prompt: Some("meeting notes".to_string()),
        response_format,
        temperature: 0.0,
        word_timestamps: false,
        segment_timestamps: false,
        request_timeout_secs: 5,
    }
}

/// 1 接続だけを受け付け、リクエストを記録して固定応答を返す。
/// reqwest は音声 reader の長さを事前に知らないため chunked transfer encoding を
/// 使い得るので、Content-Length と chunked の双方を処理する。
fn spawn_mock_groq(
    status: &'static str,
    content_type: &'static str,
    body: &'static str,
) -> (String, JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local mock server");
    let address = listener.local_addr().expect("read mock server address");

    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept transcription request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set mock server read timeout");
        let request = read_http_request(&mut stream);

        let response_head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream
            .write_all(response_head.as_bytes())
            .expect("write mock response head");
        stream
            .write_all(body.as_bytes())
            .expect("write mock response body");
        stream.flush().expect("flush mock response");
        request
    });

    (format!("http://{address}"), handle)
}

fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 8_192];
    let header_end = loop {
        if let Some(end) = find_header_end(&request) {
            break end;
        }
        let read = stream.read(&mut buffer).expect("read HTTP request headers");
        assert!(read > 0, "connection closed before HTTP headers completed");
        request.extend_from_slice(&buffer[..read]);
    };

    let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
    let content_length = headers.lines().find_map(|line| {
        line.strip_prefix("content-length:")
            .and_then(|value| value.trim().parse::<usize>().ok())
    });
    let is_chunked = headers
        .lines()
        .any(|line| line.trim() == "transfer-encoding: chunked");

    if let Some(content_length) = content_length {
        let expected_total = header_end + content_length;
        while request.len() < expected_total {
            let read = stream.read(&mut buffer).expect("read HTTP request body");
            assert!(read > 0, "connection closed before Content-Length body");
            request.extend_from_slice(&buffer[..read]);
        }
    } else if is_chunked {
        while !chunked_body_is_complete(&request[header_end..]) {
            let read = stream.read(&mut buffer).expect("read chunked request body");
            assert!(read > 0, "connection closed before final chunk");
            request.extend_from_slice(&buffer[..read]);
        }
    }

    request
}

fn find_header_end(request: &[u8]) -> Option<usize> {
    request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

fn chunked_body_is_complete(body: &[u8]) -> bool {
    body == b"0\r\n\r\n" || body.ends_with(b"\r\n0\r\n\r\n")
}

fn multipart_field(name: &str, value: &str) -> String {
    format!("name=\"{name}\"\r\n\r\n{value}\r\n")
}

#[test]
fn given_wav_and_verbose_json_when_transcribed_then_request_and_result_match_contract() {
    // Given: 実エンコーダで作った WAV と、verbose_json を返すローカル Groq mock。
    let temp_dir = TestDir::new("success");
    let audio = create_test_wav(temp_dir.path());
    let (base_url, server) = spawn_mock_groq(
        "200 OK",
        "application/json",
        r#"{"text":"こんにちは","x_groq":{"id":"req_bdd_success"}}"#,
    );
    let mut config = transcriber_config(base_url, ResponseFormat::VerboseJson);
    config.word_timestamps = true;
    config.segment_timestamps = true;

    // When: multipart で文字起こし API を呼び出す。
    let result = transcribe_file(&config, &audio).expect("transcribe through mock server");
    let request_bytes = server.join().expect("join mock server");
    let request = String::from_utf8_lossy(&request_bytes);

    // Then: 必須フィールドと timestamp 指定が送信され、本文と request_id を復元する。
    assert!(request.starts_with("POST /audio/transcriptions HTTP/1.1"));
    assert!(request
        .to_ascii_lowercase()
        .contains("authorization: bearer test-api-key"));
    assert!(request.contains(&multipart_field("model", "whisper-large-v3-turbo")));
    assert!(request.contains(&multipart_field("language", "ja")));
    assert!(request.contains(&multipart_field("prompt", "meeting notes")));
    assert!(request.contains(&multipart_field("response_format", "verbose_json")));
    assert!(request.contains(&multipart_field("temperature", "0")));
    assert!(request.contains(&multipart_field("timestamp_granularities[]", "word")));
    assert!(request.contains(&multipart_field("timestamp_granularities[]", "segment")));
    assert!(request.contains("filename=\"bdd-input.wav\""));
    assert!(request.to_ascii_lowercase().contains("audio/wav"));

    assert_eq!(result.text, "こんにちは");
    assert_eq!(result.request_id.as_deref(), Some("req_bdd_success"));
    assert!(result
        .raw_response
        .as_deref()
        .is_some_and(|body| body.contains("req_bdd_success")));
}

#[test]
fn given_plain_text_response_when_text_format_is_requested_then_body_is_used_verbatim() {
    // Given: response_format=text に対応する text/plain 応答。
    let temp_dir = TestDir::new("text");
    let audio = create_test_wav(temp_dir.path());
    let (base_url, server) =
        spawn_mock_groq("200 OK", "text/plain", "プレーンテキストの文字起こし");
    let config = transcriber_config(base_url, ResponseFormat::Text);

    // When: 文字起こし API を呼び出す。
    let result = transcribe_file(&config, &audio).expect("read text response");
    let _request = server.join().expect("join mock server");

    // Then: JSON 解析や前後の trim を挟まず、本文をそのまま転写結果にする。
    assert_eq!(result.text, "プレーンテキストの文字起こし");
    assert_eq!(result.request_id, None);
    assert_eq!(result.raw_response, None);
}

#[test]
fn given_api_error_when_transcription_fails_then_status_and_response_body_are_diagnosable() {
    // Given: モデル指定を拒否する HTTP 400 と構造化エラー本文。
    let temp_dir = TestDir::new("api-error");
    let audio = create_test_wav(temp_dir.path());
    let (base_url, server) = spawn_mock_groq(
        "400 Bad Request",
        "application/json",
        r#"{"error":{"message":"unsupported model","code":"invalid_model"}}"#,
    );
    let config = transcriber_config(base_url, ResponseFormat::Json);

    // When: 文字起こし API を呼び出す。
    let error = transcribe_file(&config, &audio).expect_err("HTTP 400 must fail");
    let _request = server.join().expect("join mock server");

    // Then: ステータスだけでなく、利用者が修正可能な API エラー本文も保持する。
    let message = error.to_string();
    assert!(message.contains("400 Bad Request"), "{message}");
    assert!(message.contains("unsupported model"), "{message}");
    assert!(message.contains("invalid_model"), "{message}");
}

#[test]
fn given_invalid_json_success_response_when_parsed_then_missing_contract_is_an_error() {
    // Given: HTTP 200 だが JSON 契約を満たさない応答本文。
    let temp_dir = TestDir::new("invalid-json");
    let audio = create_test_wav(temp_dir.path());
    let (base_url, server) = spawn_mock_groq("200 OK", "application/json", "not-json");
    let config = transcriber_config(base_url, ResponseFormat::Json);

    // When: 文字起こし API を呼び出す。
    let error = transcribe_file(&config, &audio).expect_err("invalid JSON must fail");
    let _request = server.join().expect("join mock server");

    // Then: 空文字列で成功扱いにせず、応答解析エラーとして返す。
    assert!(error.to_string().contains("failed to parse JSON response"));
}
