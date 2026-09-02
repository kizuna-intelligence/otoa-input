#!/usr/bin/env bash
# dist/ のアーカイブを GitHub Release として公開する。
#
# ビルドはしない。各 OS で build-release.sh を回し、成果物を dist/ に集めてから
# 実行する。3 つ揃う前に公開すると、利用者が自分の OS の版を見つけられない。
#
#   OTOA_REPO=<owner/name> scripts/publish-release.sh [--draft] [--tag vX.Y.Z]
#
# 既定では下書きとして作る。中身を確認してから公開する想定である。
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
TAG="v$VERSION"
DRAFT=--draft

while [ $# -gt 0 ]; do
    case "$1" in
        --tag) TAG="$2"; shift 2 ;;
        --draft) DRAFT=--draft; shift ;;
        --publish) DRAFT=""; shift ;;
        *) echo "不明な引数: $1" >&2; exit 1 ;;
    esac
done

command -v gh >/dev/null || { echo "gh コマンドが要る: https://cli.github.com/" >&2; exit 1; }
gh auth status >/dev/null 2>&1 || { echo "gh auth login を先に実行する" >&2; exit 1; }

REPO="${OTOA_REPO:-$(git remote get-url origin 2>/dev/null | sed -E 's#.*github\.com[:/]([^/]+/[^/.]+)(\.git)?#\1#')}"
[ -n "$REPO" ] || { echo "リポジトリが分からない。OTOA_REPO=owner/name を渡す" >&2; exit 1; }

shopt -s nullglob
ASSETS=(dist/*.tar.gz dist/*.zip dist/*.dmg dist/*.AppImage dist/*.sha256)
shopt -u nullglob
[ ${#ASSETS[@]} -gt 0 ] || { echo "dist/ にアーカイブが無い。先に build-release.sh を回す" >&2; exit 1; }

# 3 つの OS が揃っているか。揃わない公開は利用者から見て壊れている。
missing=()
for os in linux macos windows; do
    ls dist/*-"$os"-* >/dev/null 2>&1 || missing+=("$os")
done
if [ ${#missing[@]} -gt 0 ]; then
    echo "!! 次の OS の成果物が無い: ${missing[*]}"
    echo "   その OS では 'ビルドせずに使える' が成立しない。"
    [ -n "$DRAFT" ] || { echo "   --publish では続行しない。--draft にするか成果物を揃える" >&2; exit 1; }
    echo "   下書きなので続行する。"
fi

echo "==> 公開先: $REPO / タグ: $TAG"
printf '    %s\n' "${ASSETS[@]}"

NOTES="$(cat <<EOF
話した内容をカーソル位置へ貼り付ける音声入力です。
クライアントと認識サーバーの両方が入っており、**外部サービスなしで動きます。**

## 使い方

1. 自分の OS のファイルを取る。**macOS は \`.dmg\` を使う**
   （\`.tar.gz\` は署名が無く Gatekeeper に弾かれる）
2. 起動する。**ASR サーバーは自分で立ち上がるので、起動するのは 1 つだけ**
3. 認識モデル（数百 MB）は**初回起動時に自動で落ちてきます。**
   進み具合は入力バーに出ます。落とし終えるまで認識は始まりません

配布物の名前に版番号は入れていません。次のリンクは常に最新版を指します。

- Linux: \`otoa-input-linux-x86_64.AppImage\`
- macOS (Apple Silicon): \`otoa-input-macos-arm64.dmg\`
- Windows: \`otoa-input-windows-x86_64.zip\`

手元のものがどの版かは \`--version\` で分かります。Windows では
\`otoa-input-console.exe --version\` を使います。

Linux では貼り付けに \`xdotool\`（Wayland では \`wtype\`）が必要です。
入っていないと、認識はできるのに貼り付けだけが失敗します。

## 動作要件

CPU は 1 コアで足ります。**効いてくるのはメモリで、認識サーバーが約 1.3 GB 使います。**
詳しくは README の「どれくらいの機械で動くか」を見てください。
EOF
)"

if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
    echo "==> 既存のリリースへ差し替え"
    gh release upload "$TAG" "${ASSETS[@]}" --repo "$REPO" --clobber
else
    echo "==> 新規リリースを作成"
    gh release create "$TAG" "${ASSETS[@]}" \
        --repo "$REPO" --title "Otoa Input $VERSION" --notes "$NOTES" $DRAFT
fi

gh release view "$TAG" --repo "$REPO" --json url,isDraft,assets \
    --jq '"URL: \(.url)\n下書き: \(.isDraft)\n添付: \(.assets | length) 件"'
