use std::process::Command;

fn groq_whisper_bin() -> &'static str {
    env!("CARGO_BIN_EXE_groq-whisper-app")
}

#[test]
fn short_help_points_to_long_help() {
    let output = Command::new(groq_whisper_bin()).arg("-h").output().unwrap();

    assert!(output.status.success(), "short help failed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Desktop push-to-transcribe client for Groq Whisper."));
    assert!(stdout.contains("Run `groq-whisper-app --help`"));
    assert!(stdout.contains("--mp3-encoder <MP3_ENCODER>"));
    assert!(!stdout.contains("Configuration sources:"));
    assert!(!stdout.contains("Troubleshooting:"));
}

#[test]
fn long_help_describes_runtime_behavior_and_examples() {
    let output = Command::new(groq_whisper_bin())
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success(), "long help failed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("groq-whisper-app [OPTIONS]"));
    assert!(stdout.contains("GROQ_API_KEY or --api-key is required"));
    assert!(stdout.contains("Configuration sources:"));
    assert!(stdout.contains("Default persisted hotkey is Space"));
    assert!(stdout.contains("There is no implicit fallback between ffmpeg and lame"));
    assert!(stdout.contains("--word-timestamps"));
    assert!(stdout.contains("Troubleshooting:"));
    assert!(stdout.contains("ffmpeg `Unknown encoder 'libmp3lame'`"));
}
