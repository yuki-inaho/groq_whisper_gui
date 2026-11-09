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
#   0  既存ファイルを再利用、または生成成功
#   1  必須コマンド不足、生成失敗、空ファイル

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

OUTPUT="${1:-data/test_rec.mp3}"

if [[ -s "${OUTPUT}" ]]; then
    printf 'fixture already exists: %s\n' "${OUTPUT}"
    exit 0
fi

require_command() {
    local name="$1"
    if ! command -v "${name}" >/dev/null 2>&1; then
        printf 'error: required command not found: %s\n' "${name}" >&2
        exit 1
    fi
}

require_command ffmpeg
require_command lame

mkdir -p "$(dirname "${OUTPUT}")"
rm -f "${OUTPUT}"

# sine を raw PCM へ変換し、lame で 16 kHz / mono / 48 kbps MP3 にする。
# ffmpeg の libmp3lame 対応状況に依存しないよう、MP3 化は lame が担当する。
ffmpeg -hide_banner -loglevel error \
    -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=3" \
    -vn -ac 1 -ar 48000 -f s16le pipe:1 \
  | lame --quiet -r -s 48 --signed --little-endian \
        --resample 16 -m m -b 48 - "${OUTPUT}"

if [[ ! -s "${OUTPUT}" ]]; then
    printf 'error: generated fixture is empty: %s\n' "${OUTPUT}" >&2
    exit 1
fi

printf 'wrote fixture: %s\n' "${OUTPUT}"
