#!/usr/bin/env bash
# 手順 18: `cargo build --release` の薄いラッパー。
#
# Ubuntu 前提。repo root に CWD を固定してから cargo build を起動するだけの
# 最小スクリプト。追加で引数を渡した場合はそのまま `cargo build --release` に
# 転送する (例: `scripts/build.sh --verbose`)。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

exec cargo build --release "$@"
