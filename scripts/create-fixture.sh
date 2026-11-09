#!/usr/bin/env bash
# 決定的な短い音声 fixture を生成する。
#
# 目的: Git 管理外の data/test_rec.mp3 をクリーン clone でも再現できるようにする。
# 音声は文字起こし品質評価用ではなく、エンコード・API 送信経路のスモーク用。
# 実 API の文字起こし確認には、実際の発話を含む音声を指定すること。
#
# 使い方:
#   scripts/create-fixture.sh [output_mp3]
#
# 終了コード:
#   0  既存の読み取り可能な音声を再利用、または生成成功
#   1  必須コマンド不足、生成失敗、検証失敗

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

OUTPUT="${1:-data/test_rec.mp3}"

require_command() {
    local name="$1"
    if ! command -v "${name}" >/dev/null 2>&1; then
        printf 'error: required command not found: %s\n' "${name}" >&2
        exit 1
    fi
}

require_command ffmpeg
require_command lame
require_command ffprobe

audio_stream_info() {
    ffprobe -v error -select_streams a:0 \
        -show_entries stream=codec_name,sample_rate,channels \
        -of csv=p=0 "$1" 2>/dev/null || true
}

if [[ -s "${OUTPUT}" ]]; then
    if [[ -n "$(audio_stream_info "${OUTPUT}")" ]]; then
        printf 'fixture already exists: %s\n' "${OUTPUT}"
        exit 0
    fi
    printf 'discarding unreadable fixture: %s\n' "${OUTPUT}" >&2
    rm -f "${OUTPUT}"
fi

mkdir -p "$(dirname "${OUTPUT}")"

# 同一ディレクトリの一時ファイルへ書き、検証してから atomic rename する。
# 途中失敗時に部分 MP3 を fixture として再利用しない。
TEMP_OUTPUT="$(dirname "${OUTPUT}")/.$(basename "${OUTPUT}").tmp.$$"
trap 'rm -f "${TEMP_OUTPUT}"' EXIT

# sine を raw PCM へ変換し、lame で 16 kHz / mono / 48 kbps MP3 にする。
# ffmpeg の libmp3lame 対応状況に依存しないよう、MP3 化は lame が担当する。
ffmpeg -hide_banner -loglevel error \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=3" \
    -vn -ac 1 -ar 48000 -f s16le pipe:1 \
  | lame --quiet -r -s 48 --signed --little-endian \
        --resample 16 -m m -b 48 - "${TEMP_OUTPUT}"

if [[ ! -s "${TEMP_OUTPUT}" ]]; then
    printf 'error: generated fixture is empty: %s\n' "${TEMP_OUTPUT}" >&2
    exit 1
fi

generated_info="$(audio_stream_info "${TEMP_OUTPUT}")"
if [[ "${generated_info}" != "mp3,16000,1" ]]; then
    printf 'error: generated fixture has unexpected stream info: %s\n' "${generated_info}" >&2
    exit 1
fi

mv -f "${TEMP_OUTPUT}" "${OUTPUT}"
trap - EXIT

printf 'wrote fixture: %s\n' "${OUTPUT}"
