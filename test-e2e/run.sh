#!/usr/bin/env bash
# クリーンな Linux で配布物を検証する。
#
#   bash test-e2e/run.sh <ReazonSpeechモデル> <kodamaモデル>
#
# HuggingFace キャッシュ内のシンボリックリンクも解決して、それぞれ必要な
# 共通親だけをコンテナへ読み取り専用でマウントする。スクリーンショットの
# 取り出し先は呼び出し時のディレクトリ。OTOA_E2E_ARTIFACT_DIR で変更できる。
# 先に scripts/package-linux.sh で AppImage を作っておくこと。
set -euo pipefail
CALLER_DIR=$PWD
cd "${BASH_SOURCE[0]%/*}/.."

REAZON_MODEL="${1:-${OTOA_REAZON_MODEL_DIR:-}}"
KODAMA_MODEL="${2:-${OTOA_KODAMA_MODEL_DIR:-}}"
[ -n "$REAZON_MODEL" ] || { echo "ReazonSpeech モデルのディレクトリを渡す" >&2; exit 1; }
[ -n "$KODAMA_MODEL" ] || { echo "kodama モデルのディレクトリを渡す" >&2; exit 1; }
ARTIFACT_DIR="${OTOA_E2E_ARTIFACT_DIR:-$CALLER_DIR}"

docker build -q -f test-e2e/Dockerfile -t otoa-input-smoke . > /dev/null

# パス解決も Docker 内で行う。/host を一時的に chroot のルートにすることで、
# 絶対・相対どちらのシンボリックリンクでもホストと同じ解釈になる。
resolve_model_mount() {
    docker run --rm -i --user root --entrypoint python3 \
        -v /:/host:ro otoa-input-smoke - "$1" "$2" <<'PY'
import os
import sys

path, marker = sys.argv[1:]
if not os.path.isabs(path):
    raise SystemExit(f"モデルパスは絶対パスで指定する: {path}")
os.chroot("/host")
os.chdir("/")
model = os.path.realpath(path)
if not os.path.isfile(os.path.join(model, marker)):
    raise SystemExit(f"{path} に {marker} が無い")

targets = [model]
for directory, subdirs, files in os.walk(model):
    for name in subdirs + files:
        targets.append(os.path.realpath(os.path.join(directory, name)))
root = os.path.commonpath(targets)
print(f"{root}\t{os.path.relpath(model, root)}")
PY
}

# **コマンド置換の中の失敗は $? に出ない。** 受け取った値が空なら止める。
# これを見ないと、相対パスを渡したときに「絶対パスで指定する」と出したまま
# 先へ進み、docker の意味の分からないエラーになる。
REAZON_RESOLVED=$(resolve_model_mount "$REAZON_MODEL" tokens.txt) \
    || { echo "ReazonSpeech モデルのパスを解決できない" >&2; exit 1; }
KODAMA_RESOLVED=$(resolve_model_mount "$KODAMA_MODEL" tokenizer.json) \
    || { echo "kodama モデルのパスを解決できない" >&2; exit 1; }
IFS=$'\t' read -r REAZON_MOUNT REAZON_SUBPATH <<< "$REAZON_RESOLVED"
IFS=$'\t' read -r KODAMA_MOUNT KODAMA_SUBPATH <<< "$KODAMA_RESOLVED"
[ -n "$REAZON_MOUNT" ] && [ -n "$REAZON_SUBPATH" ] \
    || { echo "ReazonSpeech モデルのパスを解決できない" >&2; exit 1; }
[ -n "$KODAMA_MOUNT" ] && [ -n "$KODAMA_SUBPATH" ] \
    || { echo "kodama モデルのパスを解決できない" >&2; exit 1; }

echo "ReazonSpeech マウント: $REAZON_MOUNT （モデル: $REAZON_SUBPATH）"
echo "kodama マウント:       $KODAMA_MOUNT （モデル: $KODAMA_SUBPATH）"

CONTAINER_ID=$(docker create \
    -e REAZON_MODEL_SUBPATH="$REAZON_SUBPATH" \
    -e KODAMA_MODEL_SUBPATH="$KODAMA_SUBPATH" \
    -v "$REAZON_MOUNT":/reazon-root:ro \
    -v "$KODAMA_MOUNT":/kodama-root:ro \
    otoa-input-smoke)
trap 'docker rm -f "$CONTAINER_ID" > /dev/null 2>&1 || true' EXIT

set +e
docker start -a "$CONTAINER_ID"
STATUS=$?
set -e

# 必須画像。追加の GUI 画像は、操作できた場合の確認材料として取り出す。
docker cp "$CONTAINER_ID":/home/tester/missing-model.png \
    "$ARTIFACT_DIR/missing-model.png"
docker cp "$CONTAINER_ID":/home/tester/settings-screen.png \
    "$ARTIFACT_DIR/settings-screen.png" 2>/dev/null || true
docker cp "$CONTAINER_ID":/home/tester/settings-dropdown.png \
    "$ARTIFACT_DIR/settings-dropdown.png" 2>/dev/null || true
docker cp "$CONTAINER_ID":/home/tester/tray-menu.png \
    "$ARTIFACT_DIR/tray-menu.png" 2>/dev/null || true

docker rm "$CONTAINER_ID" > /dev/null
trap - EXIT
exit "$STATUS"
