#!/usr/bin/env bash
# Linux 向けの AppImage を作る。
#
# **1 つのファイルをダブルクリックすれば起動する**状態にするためのもの。
# 実行ファイルは 1 つに統合してあり、クライアントが同梱の ASR サーバーを
# 必要なときだけ自分で立ち上げる。
#
# 認識モデルは同梱しない。数百 MB あるので、AppImage の隣か
# ~/.local/share/otoa-input/models/reazonspeech-k2-v2 に置いてもらう。
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
[ "$(uname -s)" = Linux ] || { echo "Linux で実行する" >&2; exit 1; }

# appimagetool は第三者の配布物なので、リポジトリへ入れず取得して使う。
TOOL="${OTOA_APPIMAGETOOL:-$ROOT/tools/appimagetool}"
if [ ! -x "$TOOL" ]; then
    echo "==> appimagetool を取得"
    mkdir -p "$(dirname "$TOOL")"
    curl -fsSL -o "$TOOL" \
        https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x "$TOOL"
fi

APPDIR=/tmp/otoa-input.AppDir
echo "==> ビルド"
cargo build --release

echo "==> AppDir を作り直す"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/icons/hicolor/256x256/apps"
cp target/release/otoa-input "$APPDIR/usr/bin/"
cp resources/icons/otoa-input-192.png "$APPDIR/usr/share/icons/hicolor/256x256/apps/otoa-input.png"
cp resources/icons/otoa-input-192.png "$APPDIR/otoa-input.png"

# 第三者ライセンスの表示を同梱する。AppImage の中と、隣に置く分の両方。
bash scripts/generate-licenses.sh
mkdir -p "$APPDIR/usr/share/doc/otoa-input"
cp LICENSE NOTICE THIRD-PARTY-LICENSES.md "$APPDIR/usr/share/doc/otoa-input/"

cat > "$APPDIR/otoa-input.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Otoa Input
Comment=話した内容をカーソル位置へ貼り付ける音声入力
Exec=otoa-input
Icon=otoa-input
Categories=Utility;AudioVideo;
Terminal=false
DESKTOP
cp "$APPDIR/otoa-input.desktop" "$APPDIR/usr/share/applications/"

# AppRun は実行ファイルを呼ぶだけ。認識モデルは AppImage の外に置くので、
# **AppImage の場所**を渡して、そこからも探せるようにする。
cat > "$APPDIR/AppRun" <<'APPRUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
# AppImage 本体の隣に models/ があればそこを使う。
if [ -n "$APPIMAGE" ]; then
    OTOA_APPIMAGE_DIR="$(dirname "$APPIMAGE")"
    export OTOA_APPIMAGE_DIR
fi
exec "$HERE/usr/bin/otoa-input" "$@"
APPRUN
chmod +x "$APPDIR/AppRun"

echo "==> AppImage"
mkdir -p "$ROOT/dist"
OUT="$ROOT/dist/otoa-input-$VERSION-linux-x86_64.AppImage"
rm -f "$OUT"
ARCH=x86_64 "$TOOL" "$APPDIR" "$OUT" 2>&1 | tail -3

echo "==> 検査"
"$OUT" --help > /dev/null
echo "    --help が動いた"
( cd "$ROOT/dist" && sha256sum "$(basename "$OUT")" > "$(basename "$OUT").sha256" )
echo
echo "できた: $OUT"
du -h "$OUT" | cut -f1 | sed 's/^/  大きさ: /'
