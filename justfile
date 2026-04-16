# justfile — Ubuntu 向けの再利用可能コマンド定義。
#
# 手順 18: `just` が未インストールでも scripts 単独で実行できるよう、各レシピは
# `scripts/*.sh` を呼び出す薄いラッパーに統一する (DRY)。
# 前提: CWD はリポジトリ直下 (`justfile` 自体がある場所)。

set shell := ["bash", "-euo", "pipefail", "-c"]

# デフォルトは、初見ユーザーが次に打つコマンドを選べる案内を表示する。
default:
    @just help

# よく使うコマンドを目的別に表示する。
help:
    @printf '%s\n' \
      'Groq Whisper GUI - Ubuntu development commands' \
      '' \
      'First run:' \
      '  just doctor                         # required tools and fixture checks' \
      '  just verify-local                   # full local verification, no API key required' \
      '' \
      'Daily development:' \
      '  just check                          # fmt-check + clippy + unit tests' \
      '  just quality                        # check + unused dependency scan' \
      '  just fmt                            # apply rustfmt' \
      '  just run                            # launch normal GUI via cargo' \
      '  just run-release                    # launch built release binary' \
      '  just -- run --help                  # pass CLI args to cargo run' \
      '  just -- run-release --help          # pass CLI args to release binary' \
      '  just run-debug                      # launch the desktop app in debug UI mode' \
      '' \
      'Smoke tests:' \
      '  just smoke-encode                   # fixture -> 16 kHz mono 48 kbps MP3' \
      '  GROQ_API_KEY=... just verify-api    # real Groq transcription smoke' \
      '  GROQ_API_KEY=... just verify-all    # local verification + real API smoke' \
      '' \
      'Reference:'
    @printf '%s\n' '  recipe list: just --list'

# 開発前の環境確認。Rust と外部 CLI、fixture の存在だけを短時間で見る。
doctor:
    @missing=0; \
    for cmd in cargo cargo-machete ffmpeg lame ffprobe curl jq; do \
      if command -v "$cmd" >/dev/null 2>&1; then \
        printf 'ok: %s -> %s\n' "$cmd" "$(command -v "$cmd")"; \
      else \
        printf 'missing: %s\n' "$cmd" >&2; \
        missing=1; \
      fi; \
    done; \
    if [ -f data/test_rec.mp3 ]; then \
      printf 'ok: data/test_rec.mp3 exists\n'; \
    else \
      printf 'missing: data/test_rec.mp3\n' >&2; \
      missing=1; \
    fi; \
    exit "$missing"

# フォーマット適用
fmt:
    cargo fmt --all

# フォーマット確認 (CI 向け)
fmt-check:
    cargo fmt --all -- --check

# Clippy (warning を error として扱う)
clippy:
    cargo clippy --all-targets --all-features -- -D warnings

# 日常開発向けの短めチェック。外部 CLI 依存の ignored test は含めない。
check:
    just fmt-check
    just clippy
    just test

# 未使用依存の検出。新しい依存を追加した後はこのレシピで Cargo.toml の肥大化を確認する。
machete:
    cargo machete

# レビュー前の品質ゲート。fmt/clippy/test に加えて依存関係の不要物も確認する。
quality:
    just check
    just machete

# テスト実行 (ignored は含まない)
test:
    cargo test

# コンパイルだけを確認する (テストバイナリ生成まで)
test-no-run:
    cargo test --no-run

# 実機依存テストも含めた全テスト (ignored を展開)
test-ignored:
    cargo test -- --ignored

# リリースビルド (薄いラッパー)
build:
    bash scripts/build.sh

# 通常 UI モードで cargo から起動。追加 CLI 引数は `just -- run --ui-mode debug` の形で渡す。
run *args:
    cargo run -- {{args}}

# ビルド済み release バイナリを通常 UI モードで起動。追加 CLI 引数は `just -- run-release --ui-mode debug` の形で渡す。
run-release *args:
    target/release/groq-whisper-app {{args}}

# デバッグ UI モードで起動
run-debug:
    bash scripts/run-debug.sh

# ストリーミング MP3 エンコードスモーク。
# 例: `just smoke-encode data/test_rec.mp3 target/smoke/streamed-lame-16k.mp3`

# ストリーミング MP3 エンコードスモーク
smoke-encode input="data/test_rec.mp3" output="target/smoke/streamed-lame-16k.mp3":
    bash scripts/smoke-encode.sh "{{input}}" "{{output}}"

# Groq API スモーク。
# 例: `just smoke-api data/test_rec.mp3`
# GROQ_API_KEY 未設定時は smoke-api.sh が exit 2 で失敗する (ALLOW_API_SKIP=1 を
# 環境変数で与えた場合だけ exit 0 で skip される)。

# Groq API スモーク
smoke-api input="data/test_rec.mp3":
    bash scripts/smoke-api.sh "{{input}}"

# API キーを使わないローカル完結の一連の検証。
# 実行内容:
# - cargo test --no-run
# - cargo fmt --all -- --check
# - cargo clippy --all-targets --all-features -- -D warnings
# - cargo test
# - cargo test -- --ignored
# - scripts/smoke-encode.sh
# - GROQ_API_KEY 未設定時の smoke-api exit=2 確認
# - ALLOW_API_SKIP=1 時の smoke-api 明示 skip 確認

# API キー不要のローカル総合検証
verify-local input="data/test_rec.mp3" output="target/smoke/streamed-lame-16k.mp3":
    # 1. テストバイナリ生成まで通るか確認する。ここでコンパイルエラーを先に検出する。
    @echo "[verify-local] 1/8 cargo test --no-run"
    cargo test --no-run
    # 2. rustfmt の未適用差分を検出する。自動修正したい場合は `just fmt` を使う。
    @echo "[verify-local] 2/8 cargo fmt --check"
    cargo fmt --all -- --check
    # 3. clippy warning を error として扱い、レビュー前の静的品質を担保する。
    @echo "[verify-local] 3/8 cargo clippy -D warnings"
    cargo clippy --all-targets --all-features -- -D warnings
    # 4. 通常 unit test を実行する。外部コマンド依存の ignored test はここでは含めない。
    @echo "[verify-local] 4/8 cargo test"
    cargo test
    # 5. lame / ffmpeg / ffprobe / fixture に依存する ignored test も明示実行する。
    @echo "[verify-local] 5/8 cargo test -- --ignored"
    cargo test -- --ignored
    # 6. GUI と実マイクを使わず、fixture 音声から streaming MP3 を生成する。
    @echo "[verify-local] 6/8 smoke-encode"
    bash scripts/smoke-encode.sh "{{input}}" "{{output}}"
    # 7. API キー未設定時が成功扱いにならず、exit=2 で明示失敗することを固定する。
    @echo "[verify-local] 7/8 smoke-api missing-key must exit 2"
    env -u GROQ_API_KEY bash -c 'set +e; bash scripts/smoke-api.sh "{{input}}"; code=$?; echo "expected missing-key exit=2, actual exit=${code}"; test "${code}" -eq 2'
    # 8. API スモークを意図的に省略する場合だけ ALLOW_API_SKIP=1 で明示 skip できることを確認する。
    @echo "[verify-local] 8/8 smoke-api explicit skip"
    env -u GROQ_API_KEY ALLOW_API_SKIP=1 bash scripts/smoke-api.sh "{{input}}"

# Groq API まで含めたオンライン検証。GROQ_API_KEY 未設定なら失敗する。
verify-api input="data/test_rec.mp3":
    # 実 API に multipart 送信し、JSON の .text が空でないことを確認する。
    # API キーの値は echo しない。未設定時は scripts/smoke-api.sh が exit=2 で失敗する。
    @echo "[verify-api] smoke-api with GROQ_API_KEY"
    bash scripts/smoke-api.sh "{{input}}"

# API キーが設定済みの作業環境で使う全部入り検証。
# GROQ_API_KEY 未設定時は verify-api で失敗し、成功扱いにしない。
verify-all input="data/test_rec.mp3" output="target/smoke/streamed-lame-16k.mp3":
    # まず API キー不要のローカル検証を完了させる。
    @echo "[verify-all] local verification"
    just verify-local "{{input}}" "{{output}}"
    # 次に Groq API まで含めたオンライン検証を実行する。
    @echo "[verify-all] online API verification"
    just verify-api "{{input}}"
