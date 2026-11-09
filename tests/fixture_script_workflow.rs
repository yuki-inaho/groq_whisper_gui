//! fixture 生成スクリプトの失敗時契約を検証する headless 結合テスト。

mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use support::TestDir;

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write fake command");
    let mut permissions = fs::metadata(path)
        .expect("fake command metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("make fake command executable");
}

fn fixture_script() -> String {
    format!("{}/scripts/create-fixture.sh", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn given_pipeline_failure_when_fixture_is_generated_then_no_partial_file_remains() {
    // Given: 部分 PCM を出して失敗する ffmpeg と、非空の一時 MP3 を書く lame。
    let temp_dir = TestDir::new("fixture-partial");
    let bin_dir = temp_dir.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create fake bin dir");

    write_executable(
        &bin_dir.join("ffmpeg"),
        "#!/usr/bin/env bash\nprintf 'partial pcm'\nexit 1\n",
    );
    write_executable(
        &bin_dir.join("lame"),
        "#!/usr/bin/env bash\nfor arg in \"$@\"; do last=\"$arg\"; done\nprintf 'partial mp3' > \"$last\"\ncat >/dev/null || true\nexit 0\n",
    );
    write_executable(&bin_dir.join("ffprobe"), "#!/usr/bin/env bash\nexit 1\n");

    let output = temp_dir.path().join("fixture.mp3");
    let path = format!("{}:/usr/bin:/bin", bin_dir.display());

    // When: fixture 生成を実行する。
    let status = Command::new("bash")
        .arg(fixture_script())
        .arg(&output)
        .env("PATH", path)
        .status()
        .expect("run create-fixture.sh");

    // Then: 失敗を返し、部分 MP3 も一時ファイルも残さない。
    assert!(!status.success(), "pipeline failure must fail the script");
    assert!(!output.exists(), "partial fixture must not be published");

    let leftovers: Vec<_> = fs::read_dir(temp_dir.path())
        .expect("read temp dir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp."))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temporary outputs must be removed: {leftovers:?}"
    );
}
