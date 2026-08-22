#!/bin/bash
# 配布物がクリーンな環境で動くかを確かめる。
#
# 認識モデルは /models に読み取り専用でマウントされている前提。
set -u
FAIL=0
ok()   { echo "  OK   $1"; }
ng()   { echo "  NG   $1"; FAIL=1; }

APP=/home/tester/otoa-input.AppImage
chmod +x "$APP"
# FUSE が使えないコンテナでも動くように展開して使う。
"$APP" --appimage-extract > /dev/null 2>&1 && APP=/home/tester/squashfs-root/AppRun

echo "== 1. 使い方が表示できる =="
"$APP" --help > /dev/null 2>&1 && ok "--help" || ng "--help"

echo "== 2. 設定ファイルが無い状態で同梱サーバーへ繋がる =="
mkdir -p /home/tester/models
ln -sfn "/models-root/${OTOA_MODEL_SUBPATH:?}" /home/tester/models/reazonspeech-k2-v2
[ -f /home/tester/models/reazonspeech-k2-v2/tokens.txt ] \
    || { echo "  NG   モデルが読めない（マウントの指定を確認）"; exit 1; }
rm -rf /home/tester/.config
out=$("$APP" --check-connection 2>&1 | tail -2)
echo "$out" | grep -q "^OK:" && ok "設定なしで接続" || { ng "設定なしで接続"; echo "$out" | sed 's/^/       /'; }

echo "== 3. 他の配布の設定ファイルを読まない =="
# 製品版など、別の配布が ~/.config/otoa-input へ残した設定を模す。
# **この版は自分のディレクトリしか見ないので、影響を受けてはいけない。**
mkdir -p /home/tester/.config/otoa-input
cat > /home/tester/.config/otoa-input/settings.json <<'JSON'
{"asr_endpoint":"gateway","gateway_url":"wss://example.invalid/ws/asr"}
JSON
out=$("$APP" --check-connection 2>&1 | tail -2)
if echo "$out" | grep -q "example.invalid"; then
    ng "他の配布の設定を読んでいる"
    echo "$out" | sed 's/^/       /'
elif echo "$out" | grep -q "^OK:"; then
    ok "他の配布の設定を読まない"
else
    ng "他の配布の設定は読んでいないが接続に失敗した"
    echo "$out" | sed 's/^/       /'
fi
rm -rf /home/tester/.config

echo "== 4. 貼り付けができる（Xvfb 上）=="
Xvfb :99 -screen 0 1280x720x24 > /dev/null 2>&1 &
sleep 2
DISPLAY=:99 "$APP" --paste-test "SMOKE_TEST" 2>&1 | grep -q "^OK:" \
    && ok "貼り付け" || ng "貼り付け"

echo "== 5. 画面が出る（GUI が起動して落ちない）=="
# **--help だけ通っても意味がない。** GUI は別のライブラリを要求するので、
# 実際に起動して落ちないことを確かめる。実際にこれで
# libxkbcommon-x11 / libayatana-appindicator3 の不足を見逃していた。
rm -rf /home/tester/.config
( DISPLAY=:99 "$APP" > /home/tester/gui.log 2>&1 & )
sleep 12
if pgrep -f otoa-input > /dev/null; then
    ok "GUI 起動"
    pkill -f otoa-input
else
    ng "GUI 起動"
    grep -iE "cannot open|could not be loaded|panicked" /home/tester/gui.log | head -3 | sed 's/^/       /'
fi

echo
[ $FAIL -eq 0 ] && echo "すべて通過" || echo "失敗あり"
exit $FAIL
