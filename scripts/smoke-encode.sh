#!/usr/bin/env bash
# 手順 18: ストリーミング MP3 エンコードのスモークテスト。
#
# Ubuntu 前提。`data/test_rec.mp3` (または任意の音声ファイル) を `ffmpeg` で
# raw PCM (s16le / 48 kHz / mono) にデコードし、`lame` に stdin pipe で
# 流し込んで 16 kHz / mono / 48 kbps MP3 を生成する。GUI も cpal マイクも
# 使わず、Ubuntu CLI コマンドだけで Phase 2 のエンコード経路を再現する。
#
# 使い方:
#   scripts/smoke-encode.sh [input_audio] [output_mp3]
#
# 引数:
#   input_audio  入力音声ファイル (default: data/test_rec.mp3)
#   output_mp3   出力 MP3 ファイル (default: target/smoke/streamed-lame-16k.mp3)
#
# 終了コード:
#   0  成功 (出力 MP3 が 0 バイトより大きく、ffprobe メタデータも期待値一致)
#   1  入力ファイル不在、外部コマンド不在、エンコード失敗、または検証失敗
#   set -euo pipefail 下で `ffmpeg | lame` パイプが途中で落ちれば bubble up

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

INPUT="${1:-data/test_rec.mp3}"
OUTPUT="${2:-target/smoke/streamed-lame-16k.mp3}"

if [[ ! -f "${INPUT}" ]]; then
    printf 'error: input audio not found: %s\n' "${INPUT}" >&2
    exit 1
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

# 出力ディレクトリを用意し、既存ファイルは上書きのため削除しておく。
OUTPUT_DIR="$(dirname "${OUTPUT}")"
mkdir -p "${OUTPUT_DIR}"
rm -f "${OUTPUT}"

# ffmpeg は -hide_banner -loglevel error でノイズを抑え、-re でリアルタイムに
# 近い速度で読むことでストリーミング経路相当の負荷を再現する (ただしテストの
# 所要時間が長引くため、smoke で許容範囲なら -re を外す判断もあり)。
# lame のオプション:
#   -r                 raw PCM 入力
#   -s 48              入力 sample rate = 48 kHz
#   --signed           署名付き PCM
#   --little-endian    endianness
#   --resample 16      出力 sample rate = 16 kHz
#   -m m               出力モノラル
#   -b 48              出力ビットレート 48 kbps
#   -                  stdin
#   "${OUTPUT}"        出力ファイル
ffmpeg -hide_banner -loglevel error -re \
    -i "${INPUT}" \
    -f s16le -ar 48000 -ac 1 pipe:1 \
  | lame --quiet -r -s 48 --signed --little-endian \
        --resample 16 -m m -b 48 \
        - "${OUTPUT}"

if [[ ! -s "${OUTPUT}" ]]; then
    printf 'error: lame produced empty output: %s\n' "${OUTPUT}" >&2
    exit 1
fi

printf 'wrote %s\n' "${OUTPUT}"

# 完了条件は「生成できた」だけではなく 16 kHz / mono / 48 kbps MP3 であること。
# ここで強制検証し、想定外の codec や sample rate を成功扱いにしない。
require_command ffprobe
require_command jq

probe_json="$(ffprobe -v error -select_streams a:0 \
    -show_entries stream=codec_name,sample_rate,channels,bit_rate \
    -show_entries format=duration,size \
    -of json "${OUTPUT}")"
printf '%s\n' "${probe_json}"

codec_name="$(jq -r '.streams[0].codec_name // ""' <<<"${probe_json}")"
sample_rate="$(jq -r '.streams[0].sample_rate // ""' <<<"${probe_json}")"
channels="$(jq -r '.streams[0].channels // ""' <<<"${probe_json}")"
bit_rate="$(jq -r '.streams[0].bit_rate // "0"' <<<"${probe_json}")"
duration="$(jq -r '.format.duration // ""' <<<"${probe_json}")"
size="$(jq -r '.format.size // "0"' <<<"${probe_json}")"

if [[ "${codec_name}" != "mp3" ]]; then
    printf 'error: expected codec_name=mp3, got %s\n' "${codec_name}" >&2
    exit 1
fi

if [[ "${sample_rate}" != "16000" ]]; then
    printf 'error: expected sample_rate=16000, got %s\n' "${sample_rate}" >&2
    exit 1
fi

if [[ "${channels}" != "1" ]]; then
    printf 'error: expected channels=1, got %s\n' "${channels}" >&2
    exit 1
fi

if ! [[ "${bit_rate}" =~ ^[0-9]+$ ]]; then
    printf 'error: bit_rate is not numeric: %s\n' "${bit_rate}" >&2
    exit 1
fi

if (( bit_rate < 44000 || bit_rate > 52000 )); then
    printf 'error: expected bit_rate around 48000, got %s\n' "${bit_rate}" >&2
    exit 1
fi

if ! [[ "${size}" =~ ^[0-9]+$ ]] || (( size <= 0 )); then
    printf 'error: expected positive output size, got %s\n' "${size}" >&2
    exit 1
fi

printf 'verified mp3 metadata: codec=%s sample_rate=%s channels=%s bit_rate=%s duration=%s size=%s\n' \
    "${codec_name}" "${sample_rate}" "${channels}" "${bit_rate}" "${duration}" "${size}"
