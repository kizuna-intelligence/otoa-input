#!/usr/bin/env bash
# 配布用のアーカイブを作る。
#
# 利用者にビルドさせないためのものなので、**実行に必要なものを全部入れる。**
# 入れ忘れると、起動はするのに貼り付けだけ動かない、といった形で表に出る。
#
#   otoa-input          本体。これ 1 つで動く。必要なら ASR サーバーも
#                       自分で立ち上げる（--serve でサーバーだけにもできる）
#   otoa-asr-server     ASR サーバー単体。別の機械でサーバーだけ動かす用
#
# ONNX Runtime は静的リンク、Silero VAD モデルはバイナリへ埋め込んであるので、
# 共有ライブラリも resources/ も要らない。**バイナリ 2 つだけで動く。**
#
# 認識モデル(ReazonSpeech k2-v2)は同梱しない。数百 MB あり、README の手順で
# 各自が取得する。
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
OUT="${OUT_DIR:-$ROOT/dist}"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
case "$(uname -s)" in
    Linux)  OS=linux;   LIB_EXT=so    ;;
    Darwin) OS=macos;   LIB_EXT=dylib ;;
    MINGW*|MSYS*|CYGWIN*) OS=windows; LIB_EXT=dll ;;
    *) echo "対応していない OS: $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
    x86_64|amd64) ARCH=x86_64 ;;
    arm64|aarch64) ARCH=arm64 ;;
    *) echo "対応していない CPU: $(uname -m)" >&2; exit 1 ;;
esac

NAME="otoa-input-$VERSION-$OS-$ARCH"
STAGE="$OUT/$NAME"

echo "==> ビルド ($OS/$ARCH)"
cargo build --release

echo "==> 収集 -> $STAGE"
rm -rf "$STAGE"
mkdir -p "$STAGE"

for binary in otoa-input otoa-asr-server; do
    src="target/release/$binary"
    [ "$OS" = windows ] && src="$src.exe"
    [ -f "$src" ] || { echo "実行ファイルが無い: $src" >&2; exit 1; }
    cp "$src" "$STAGE/"
done

cp README.md LICENSE NOTICE "$STAGE/"

cat > "$STAGE/はじめに.txt" <<EOF
Otoa Input $VERSION ($OS/$ARCH)

1. 認識モデルを取得する（同梱していない）

   pip install -U "huggingface_hub[cli]"
   hf download reazon-research/reazonspeech-k2-v2 \\
       --local-dir models/reazonspeech-k2-v2

2. otoa-input を起動する

   ./otoa-input

   ASR サーバーは必要なときに自分で立ち上がります。別で起動する必要は
   ありません。別の機械でサーバーだけ動かしたい場合は otoa-asr-server
   （または otoa-input --serve）を使ってください。

Linux では貼り付けに xdotool（Wayland では wtype）が要ります。
入っていないと、認識はできるのに貼り付けだけが失敗します。

詳細は README.md、オプションは --help を見てください。
EOF

echo "==> 検査"

# ONNX を動的に参照していないことを確かめる。参照していると、
# 共有ライブラリを同梱しない配布物は利用者の環境で起動できない。
# target/ に .so が残っていても、バイナリが参照していなければ問題ない。
case "$OS" in
    linux) list_links() { ldd "$1" 2>/dev/null; } ;;
    macos) list_links() { otool -L "$1" 2>/dev/null; } ;;
    *)     list_links() { :; } ;;   # Windows は同梱 DLL を持たない前提
esac
for binary in otoa-input otoa-asr-server; do
    target="$STAGE/$binary"
    [ "$OS" = windows ] && target="$target.exe"
    if list_links "$target" | grep -qiE "onnx|sherpa"; then
        echo "$binary が ONNX を動的に参照している。静的リンクが効いていない:" >&2
        list_links "$target" | grep -iE "onnx|sherpa" >&2
        exit 1
    fi
done
echo "    ONNX への動的リンクなし"

"$STAGE/otoa-asr-server" --help > /dev/null
"$STAGE/otoa-input" --help > /dev/null
echo "    --help が両方とも動いた"

echo "==> 圧縮"
cd "$OUT"
if [ "$OS" = windows ]; then
    rm -f "$NAME.zip"
    # Git Bash には zip が無いことがある。その場合は PowerShell に任せる。
    if command -v zip >/dev/null; then
        zip -qr "$NAME.zip" "$NAME"
    else
        # Git Bash の PATH に powershell が無いことがあるので実体を探す。
        POWERSHELL="$(command -v powershell.exe || command -v pwsh.exe || true)"
        [ -n "$POWERSHELL" ] || POWERSHELL="/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
        [ -x "$POWERSHELL" ] || { echo "zip も PowerShell も使えない" >&2; exit 1; }
        "$POWERSHELL" -NoProfile -Command \
            "Compress-Archive -Path '$NAME' -DestinationPath '$NAME.zip' -Force"
    fi
    ARCHIVE="$NAME.zip"
else
    rm -f "$NAME.tar.gz"; tar czf "$NAME.tar.gz" "$NAME"; ARCHIVE="$NAME.tar.gz"
fi
( command -v sha256sum >/dev/null && sha256sum "$ARCHIVE" || shasum -a 256 "$ARCHIVE" ) > "$ARCHIVE.sha256"

echo
echo "できた: $OUT/$ARCHIVE"
du -h "$OUT/$ARCHIVE" | cut -f1 | sed 's/^/  大きさ: /'
