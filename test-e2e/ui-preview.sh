#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "使い方: $0 [--compositor] [--out DIR] <state...>" >&2
    echo "state: splash connecting listening finalizing committed error login settings:<pane>" >&2
}

compositor=0
out_dir="$PWD/target/ui-preview-docker"
states=()
while [ "$#" -gt 0 ]; do
    case "$1" in
        --compositor)
            compositor=1
            ;;
        --out)
            shift
            [ "$#" -gt 0 ] || { usage; exit 2; }
            out_dir=$1
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --*)
            echo "未知のオプション: $1" >&2
            usage
            exit 2
            ;;
        *)
            states+=("$1")
            ;;
    esac
    shift
done

[ "${#states[@]}" -gt 0 ] || { usage; exit 2; }
[ -x target/release/otoa-input ] || {
    echo 'target/release/otoa-input がありません。先に cargo build --release -p otoa-input-app を実行してください。' >&2
    exit 1
}

mkdir -p "$out_dir"
out_dir=$(cd "$out_dir" && pwd)
# コンテナ内の tester が、ホストからマウントした検証成果物を書けるようにする。
chmod 0777 "$out_dir"

docker build -q -f test-e2e/Dockerfile.ui -t otoa-input-ui .
docker run --rm \
    --user tester \
    -e UI_COMPOSITOR="$compositor" \
    -v "$PWD/target/release/otoa-input:/opt/otoa-input:ro" \
    -v "$PWD/test-e2e/ui-preview-inner.sh:/home/tester/ui-preview-inner.sh:ro" \
    -v "$out_dir:/out" \
    --entrypoint dbus-run-session \
    otoa-input-ui -- bash /home/tester/ui-preview-inner.sh "${states[@]}"
