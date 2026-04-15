#!/usr/bin/env bash
# 手順 18: `cargo run -- --ui-mode debug` の薄いラッパー。
#
# Ubuntu 前提。デバッグ UI モードでアプリを起動する。追加引数はそのまま
# アプリ側に転送する (例: `scripts/run-debug.sh --show-settings`)。
# リリースビルドではなく debug ビルドを使うため、起動が速いがアプリのロジックは
# 未最適化である点に注意。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

exec cargo run -- --ui-mode debug "$@"
