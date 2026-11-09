//! CLI 起動契約の BDD 形式結合テスト。
//!
//! 実際のバイナリを子プロセスとして実行し、GUI を開く前に終了する入力だけを
//! 対象にする。環境変数と永続設定は各コマンドで隔離し、開発者の `state.json` や
//! `GROQ_*` が結果を左右しないようにする。

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

fn groq_whisper_bin() -> &'static str {
    env!("CARGO_BIN_EXE_groq-whisper-app")
}

fn isolated_config_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must be after the Unix epoch")
        .as_nanos();
    let sequence = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "groq-whisper-cli-bdd-{}-{nonce}-{sequence}",
        std::process::id()
    ))
}

fn isolated_command_with_config_dir() -> (Command, PathBuf) {
    let config_dir = isolated_config_dir();
    let mut command = Command::new(groq_whisper_bin());
    command
        .env_clear()
        .env("HOME", &config_dir)
        .env("XDG_CONFIG_HOME", &config_dir)
        .env("TERM", "dumb")
        .env("NO_COLOR", "1");
    (command, config_dir)
}

fn isolated_command() -> Command {
    isolated_command_with_config_dir().0
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn given_conflicting_hotkey_modes_when_app_starts_then_startup_fails_before_gui() {
    // Given: toggle モードと start/stop モードを同時に指定した CLI。
    let mut command = isolated_command();
    command.args([
        "--toggle-hotkey",
        "Space",
        "--start-hotkey",
        "Ctrl+S",
        "--stop-hotkey",
        "Ctrl+E",
    ]);

    // When: 実バイナリを起動する。
    let output = command.output().expect("run groq-whisper-app");

    // Then: GUI を開かず、設定の矛盾を修正できるエラーで終了する。
    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("toggle hotkey and start/stop hotkeys cannot be mixed"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn given_only_start_hotkey_when_app_starts_then_stop_hotkey_is_required() {
    // Given: start-hotkey だけを指定し、対になる stop-hotkey を省略した CLI。
    let mut command = isolated_command();
    command.args(["--start-hotkey", "Ctrl+S"]);

    // When: 実バイナリを起動する。
    let output = command.output().expect("run groq-whisper-app");

    // Then: 片方だけの設定を受理せず、不足している契約を明示する。
    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("start_hotkey and stop_hotkey must be provided together"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn given_unsupported_hotkey_when_app_starts_then_rejected_key_is_reported() {
    // Given: サポート外の F13 を録音トグルに指定した CLI。
    let mut command = isolated_command();
    command.args(["--toggle-hotkey", "F13"]);

    // When: 実バイナリを起動する。
    let output = command.output().expect("run groq-whisper-app");

    // Then: 暗黙の既定値へ戻らず、不正なキー名を報告する。
    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("unsupported key: F13"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn given_unknown_encoder_backend_when_cli_is_parsed_then_clap_returns_usage_error() {
    // Given: 宣言されていない MP3 バックエンド。
    let mut command = isolated_command();
    command.args(["--mp3-encoder", "unknown"]);

    // When: 実バイナリを起動する。
    let output = command.output().expect("run groq-whisper-app");

    // Then: clap の用法エラーとして終了し、有効値を案内する。
    assert_eq!(output.status.code(), Some(2));
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("invalid value 'unknown'"),
        "unexpected stderr: {stderr}"
    );
    assert!(stderr.contains("possible values: ffmpeg, lame"));
}

#[test]
fn given_quit_key_as_hotkey_when_app_starts_then_conflict_is_reported() {
    // Given: 終了キーとして予約されている Q を録音トグルに指定した CLI。
    let mut command = isolated_command();
    command.args(["--toggle-hotkey", "Q"]);

    // When: 実バイナリを起動する。
    let output = command.output().expect("run groq-whisper-app");

    // Then: 実行時に無反応になる前に、予約キーであることを起動時エラーで知らせる。
    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("reserved for application quit"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn given_corrupt_state_file_when_app_starts_then_fallback_is_logged() {
    // Given: 破損した state.json と、GUI 起動前に終了する不正ホットキー。
    let (mut command, config_dir) = isolated_command_with_config_dir();
    let state_dir = config_dir.join("groq-whisper-app");
    fs::create_dir_all(&state_dir).expect("create isolated state dir");
    let state_path = state_dir.join("state.json");
    fs::write(&state_path, "{ this is not valid json").expect("write corrupt state");
    command.args(["--toggle-hotkey", "F13"]);

    // When: 実バイナリを起動する。
    let output = command.output().expect("run groq-whisper-app");

    // Then: 既定値への fallback は行うが、無言にはせずパスと理由を診断できる。
    assert_eq!(output.status.code(), Some(1));
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("永続設定を読み込めませんでした"),
        "unexpected stderr: {stderr}"
    );
    assert!(stderr.contains("state.json"), "unexpected stderr: {stderr}");
    assert!(
        stderr.contains("unsupported key: F13"),
        "unexpected stderr: {stderr}"
    );
    let _ = fs::remove_dir_all(config_dir);
}

#[test]
fn given_secret_environment_values_when_help_is_rendered_then_values_are_not_exposed() {
    // Given: 環境変数に API キーと業務プロンプトを設定したプロセス。
    let api_key = "gsk_bdd_secret_that_must_not_appear";
    let prompt = "confidential_bdd_prompt_that_must_not_appear";
    let mut command = isolated_command();
    command
        .env("GROQ_API_KEY", api_key)
        .env("GROQ_WHISPER_PROMPT", prompt)
        .arg("--help");

    // When: 長形式ヘルプを要求する。
    let output = command.output().expect("render --help");

    // Then: 変数名は案内するが、秘密値そのものは出力しない。
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("GROQ_API_KEY"));
    assert!(stdout.contains("GROQ_WHISPER_PROMPT"));
    assert!(!stdout.contains(api_key));
    assert!(!stdout.contains(prompt));
}
