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

# **変数名の直後に全角文字を置かない。** macOS の bash は 3.2 で、非 ASCII
# バイトを識別子の一部として読む。`$VERSION（` と書くと `VERSION（` という
# 名前だと解釈し、set -u で「unbound variable」になって梱包だけが落ちる。
# 続ける必要があるときは `${VERSION}` と括る。
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

# **名前にバージョンを入れない。** 入れると毎回リンクが変わり、
# releases/latest/download/<名前> で「いつでも最新」を指せなくなる。
# どの版かは中の はじめに.txt と --version で分かる。
NAME="otoa-input-$OS-$ARCH"
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

# 第三者ライセンスの表示。**配布のたびに作り直す。** 依存が変われば内容も
# 変わるので、手で管理すると必ずずれる。
bash scripts/generate-licenses.sh
cp README.md LICENSE NOTICE THIRD-PARTY-LICENSES.md "$STAGE/"

# **OS ごとに書き分ける。** 起動コマンドも、貼り付けに要るものも、出る警告も
# OS で違う。1 つの文面を使い回すと、Windows の利用者に「./otoa-input」と
# 「xdotool を入れろ」を読ませることになる（0.1.4 で実際にそうなっていた）。
case "$OS" in
    windows)
        LAUNCH='otoa-input.exe'
        CONT='^'
        PLATFORM_NOTE='署名していないため、初回起動時に Windows の SmartScreen が
「WindowsによってPCが保護されました」と警告します。
［詳細情報］→［実行］で起動できます。'
        ;;
    macos)
        LAUNCH='./otoa-input'
        CONT='\'
        PLATFORM_NOTE='システム設定 →「プライバシーとセキュリティ」→「アクセシビリティ」と
「マイク」で otoa-input を許可してください。許可しないと、認識はできるのに
貼り付けだけが失敗します。'
        ;;
    *)
        LAUNCH='./otoa-input'
        CONT='\'
        PLATFORM_NOTE='貼り付けに xdotool（Wayland では wtype）が要ります。
入っていないと、認識はできるのに貼り付けだけが失敗します。'
        ;;
esac

# **最初に読まれる文なので、実際のふるまいとずれていると効く。**
# 0.3.0 でモデルは初回起動時に自分で落ちてくるようになった。手で取る手順を
# 1 番目に置いたままだと、要らない 587MB の取得を利用者にさせることになる。
cat > "$STAGE/はじめに.txt" <<EOF
Otoa Input $VERSION ($OS/$ARCH)

1. 起動する

   $LAUNCH

   認識モデル（数百 MB）は同梱していませんが、**初回起動時に自動で
   落ちてきます。** 進み具合は入力バーに出ます。落とし終えるまで認識は
   始まりません。回線によっては数分かかります。途中で終了しても、次の
   起動で残りだけを取り直します。

   ASR サーバーは必要なときに自分で立ち上がります。別で起動する必要は
   ありません。別の機械でサーバーだけ動かしたい場合は otoa-asr-server
   （または otoa-input --serve）を使ってください。

$PLATFORM_NOTE

2. 自分でモデルを置きたい場合（回線が細い、複数台へ配る、など）

   精度を優先するなら ReazonSpeech k2-v2（587MB、既定）:

   pip install -U "huggingface_hub[cli]"
   hf download reazon-research/reazonspeech-k2-v2 $CONT
       --local-dir models/reazonspeech-k2-v2

   メモリを抑えたいなら kodama（309MB）:

   hf download ayousanz/kodama-ja-streaming-small $CONT
       --include tokenizer.json --include "onnx/*" $CONT
       --local-dir kodama-download

   取得した onnx/ の 5 ファイルと tokenizer.json を
   models/kodama-ja-streaming-small/ に置き、設定画面の「認識エンジン」で
   kodama を選びます。.onnx.data を忘れると読み込みに失敗します。

この配布物の版: ${VERSION}（$LAUNCH --version でも確かめられます）
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
        # 中身は zip-windows.ps1 側に書いてある。**ここへ直に PowerShell を
        # 書かない。** bash と cmd と PowerShell の引用符が三重にかかり、
        # 直すたびに壊れる。
        "$POWERSHELL" -NoProfile -ExecutionPolicy Bypass \
            -File "$(cygpath -w "$ROOT/scripts/zip-windows.ps1")" \
            -SourceDir "$NAME" -Destination "$(cygpath -w "$OUT/$NAME.zip")"
    fi
    ARCHIVE="$NAME.zip"
else
    rm -f "$NAME.tar.gz"; tar czf "$NAME.tar.gz" "$NAME"; ARCHIVE="$NAME.tar.gz"
fi
# **書庫の中の名前を検査する。** 化けたまま配ると、利用者には
# 「уБпуБШуВБуБл.txt」のような名前で届く（0.1.4 で実際に起きた）。
#
# **unzip -l の表示では判定しない。** Info-ZIP は名前を端末の文字コードへ
# 直して出すので、正しく入っていても化けて見える（この検査自体が誤って
# 落ちた）。書庫の生バイトに UTF-8 の「はじめに」が居ることを見る。
# PowerShell で作った場合は、UTF-8 フラグまで zip-windows.ps1 側で
# 読み直して確かめている。
if [ "$OS" = windows ]; then
    if ! grep -qa "$(printf '\xe3\x81\xaf\xe3\x81\x98\xe3\x82\x81\xe3\x81\xab')" "$ARCHIVE"; then
        echo "zip の中の日本語ファイル名が UTF-8 で入っていない" >&2
        exit 1
    fi
fi

( command -v sha256sum >/dev/null && sha256sum "$ARCHIVE" || shasum -a 256 "$ARCHIVE" ) > "$ARCHIVE.sha256"

echo
echo "できた: $OUT/$ARCHIVE"
du -h "$OUT/$ARCHIVE" | cut -f1 | sed 's/^/  大きさ: /'
