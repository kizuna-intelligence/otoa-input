#!/usr/bin/env bash
# クリーンな Linux で配布物を検証する。
#
#   bash test-e2e/run.sh [認識モデルのディレクトリ]
#
# 先に scripts/package-linux.sh で AppImage を作っておくこと。
set -euo pipefail
cd "$(dirname "$0")/.."

MODEL="${1:-${OTOA_ASR_MODEL_DIR:-}}"
[ -n "$MODEL" ] || { echo "認識モデルのディレクトリを渡す" >&2; exit 1; }
[ -e "$MODEL/tokens.txt" ] || { echo "$MODEL に tokens.txt が無い" >&2; exit 1; }
ls dist/*.AppImage > /dev/null 2>&1 || { echo "先に scripts/package-linux.sh を回す" >&2; exit 1; }

# **HuggingFace のキャッシュはファイルが blobs/ へのシンボリックリンクになっている。**
# スナップショットだけをマウントするとリンク切れになり、モデルが読めない。
# 実体を含む共通の親をマウントし、その中の相対パスを渡す。
read -r MOUNT SUBPATH <<< "$(python3 - "$MODEL" <<'PY'
import os, sys
model = os.path.realpath(sys.argv[1])
targets = [model]
for name in os.listdir(model):
    targets.append(os.path.realpath(os.path.join(model, name)))
root = os.path.commonpath(targets)
print(root, os.path.relpath(model, root))
PY
)"

echo "マウント: $MOUNT （モデル: $SUBPATH）"
docker build -q -f test-e2e/Dockerfile -t otoa-input-smoke . > /dev/null
docker run --rm \
    -e OTOA_MODEL_SUBPATH="$SUBPATH" \
    -v "$MOUNT":/models-root:ro \
    otoa-input-smoke
