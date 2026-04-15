#!/usr/bin/env bash
# 手順 17: Groq Whisper API スモークテスト。
#
# Ubuntu 前提。実マイクを使わずに、任意の音声ファイルを
# `/audio/transcriptions` へ multipart で送信し、レスポンス JSON の `.text` を
# 標準出力へ出す。
#
# 使い方:
#   scripts/smoke-api.sh [input_audio]
#
# 引数:
#   input_audio  送信する音声ファイルパス (デフォルト: data/test_rec.mp3)
#
# 環境変数:
#   GROQ_API_KEY         (必須) Groq API キー。値は本スクリプトに hard code しない。
#   GROQ_BASE_URL        (任意) API base URL。デフォルト https://api.groq.com/openai/v1
#   GROQ_WHISPER_MODEL   (任意) Whisper モデル名。デフォルト whisper-large-v3-turbo
#   GROQ_WHISPER_LANGUAGE (任意) 言語コード。デフォルト ja
#   ALLOW_API_SKIP       (任意) 1 を指定すると GROQ_API_KEY 未設定時に skip(exit 0) を許可する。
#                         通常はエラー扱い (exit 2) にする。skip は「API スモーク未実施」
#                         扱いであり、完了条件「.text が返る」を満たすものではない。
#
# 終了コード:
#   0  成功 (API が 200 を返し .text が 1 文字以上) またはALLOW_API_SKIP=1 による明示 skip
#   2  GROQ_API_KEY が未設定 (ALLOW_API_SKIP !=1)
#   3  API から返った .text が空
#   その他 0 以外  curl や jq の失敗 (set -euo pipefail により bubble up)

set -euo pipefail

# 引数解析 — DRY のため repo root 起点の相対パスを既定にする。
INPUT_AUDIO="${1:-data/test_rec.mp3}"

# repo root を安定的に解決する (script が異なる CWD から呼ばれても動くように)。
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

if [[ ! -f "${INPUT_AUDIO}" ]]; then
    printf 'error: input audio not found: %s\n' "${INPUT_AUDIO}" >&2
    exit 4
fi

# GROQ_API_KEY の扱い。未設定時は明示 skip でない限り exit 2。
# `set -u` 下で未設定環境変数を参照するために `${VAR:-}` 形式を使う。
if [[ -z "${GROQ_API_KEY:-}" ]]; then
    if [[ "${ALLOW_API_SKIP:-}" == "1" ]]; then
        printf 'skip: GROQ_API_KEY not set (ALLOW_API_SKIP=1)\n' >&2
        exit 0
    fi
    printf 'error: GROQ_API_KEY is not set\n' >&2
    exit 2
fi

BASE_URL="${GROQ_BASE_URL:-https://api.groq.com/openai/v1}"
MODEL="${GROQ_WHISPER_MODEL:-whisper-large-v3-turbo}"
LANGUAGE="${GROQ_WHISPER_LANGUAGE:-ja}"

# MIME type は拡張子から決める。mp3/wav 以外は application/octet-stream で送る。
case "${INPUT_AUDIO##*.}" in
    mp3|MP3) MIME="audio/mpeg" ;;
    wav|WAV) MIME="audio/wav" ;;
    *)       MIME="application/octet-stream" ;;
esac

# curl multipart で送信。--fail-with-body を使って HTTP エラー時も本文を得る。
# 応答 JSON は一時ファイル経由で jq に渡す (巨大レスポンス対策 + HTTP エラー切り分け)。
RESPONSE_FILE="$(mktemp --tmpdir groq-smoke-api.XXXXXX.json)"
trap 'rm -f "${RESPONSE_FILE}"' EXIT

# API キーの値は echo しない (ログ・作業書への漏洩防止)。
curl -sS --fail-with-body --max-time 120 \
    "${BASE_URL}/audio/transcriptions" \
    -H "Authorization: Bearer ${GROQ_API_KEY}" \
    -F "file=@${INPUT_AUDIO};type=${MIME}" \
    -F "model=${MODEL}" \
    -F "language=${LANGUAGE}" \
    -F "response_format=json" \
    -o "${RESPONSE_FILE}"

# `.text` を抽出する。存在しない場合は null が返るため空文字列扱いにする。
TEXT="$(jq -r '.text // ""' "${RESPONSE_FILE}")"

if [[ -z "${TEXT}" ]]; then
    printf 'error: groq returned empty .text\n' >&2
    # 参考のため raw レスポンスを stderr に出す (API キーは含まない)。
    jq . "${RESPONSE_FILE}" >&2 || true
    exit 3
fi

printf '%s\n' "${TEXT}"
