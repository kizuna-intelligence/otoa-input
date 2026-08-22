#!/usr/bin/env bash
# 配布物へ同梱する第三者ライセンス表示を作る。
#
# **配布のたびに作り直す。** 依存が変わると内容が変わるので、
# 手で管理すると必ずずれる。build-release.sh から呼ばれる。
set -euo pipefail
cd "$(dirname "$0")/.."

command -v cargo-about > /dev/null \
    || { echo "cargo-about が要る: cargo install --locked --features cli cargo-about" >&2; exit 1; }

cargo about generate about.hbs -o THIRD-PARTY-LICENSES.md
echo "生成: THIRD-PARTY-LICENSES.md ($(wc -l < THIRD-PARTY-LICENSES.md) 行)"
