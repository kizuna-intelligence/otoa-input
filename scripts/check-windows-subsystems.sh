#!/usr/bin/env bash
# 配布する Windows 実行ファイルの PE Subsystem を検査する。
#
# Git Bash に標準で入る od だけを使う。PE ヘッダーは DOS ヘッダーの 0x3c に
# 開始位置を持ち、その 92 byte 後に 2 byte の Subsystem がある。
set -euo pipefail

STAGE="${1:?Windows の成果物ディレクトリを渡す}"

subsystem() {
    binary="$1"
    pe_offset="$(od -An -v -tu4 -j 60 -N 4 "$binary" | tr -d ' ')"
    [ -n "$pe_offset" ] || { echo "$binary の PE ヘッダーを読めない" >&2; exit 1; }
    od -An -v -tu2 -j "$((pe_offset + 92))" -N 2 "$binary" | tr -d ' '
}

check_subsystem() {
    binary="$1"
    expected="$2"
    description="$3"
    actual="$(subsystem "$binary")"
    if [ "$actual" != "$expected" ]; then
        echo "$binary の実行形式が $description ではない（Subsystem: ${actual:-不明}）" >&2
        exit 1
    fi
}

check_subsystem "$STAGE/otoa-input.exe" 2 "Windows GUI"
check_subsystem "$STAGE/otoa-input-console.exe" 3 "Windows Console"
check_subsystem "$STAGE/otoa-asr-server.exe" 3 "Windows Console"
