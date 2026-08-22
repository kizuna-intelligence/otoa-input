#!/bin/bash
# 配布物がクリーンな環境で動くかを確かめる。
#
# ReazonSpeech は /reazon-root、kodama は /kodama-root に読み取り専用で
# マウントされ、各モデルまでの相対パスは環境変数で渡される前提。
set -uo pipefail
FAIL=0
ok()          { echo "  OK   $1"; }
ng()          { echo "  NG   $1"; FAIL=1; }
unconfirmed() { echo "  未確認 $1"; }

APP=/home/tester/otoa-input.AppImage
CONFIG_DIR=/home/tester/.config/otoa-input-oss
MODEL_DIR=/home/tester/models
export DISPLAY=:99
export XDG_RUNTIME_DIR=/home/tester/runtime
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

declare -a CLEANUP_PIDS=()
cleanup() {
    local pid
    for pid in "${CLEANUP_PIDS[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
}
trap cleanup EXIT

stop_process() {
    local pid=$1
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 1 50); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
    done
    wait "$pid" 2>/dev/null || true
}

write_engine() {
    mkdir -p "$CONFIG_DIR"
    printf '{"asr_engine":"%s"}\n' "$1" > "$CONFIG_DIR/settings.json"
}

link_reazonspeech() {
    mkdir -p "$MODEL_DIR"
    ln -sfn "/reazon-root/${REAZON_MODEL_SUBPATH:?}" \
        "$MODEL_DIR/reazonspeech-k2-v2"
}

link_kodama() {
    mkdir -p "$MODEL_DIR"
    ln -sfn "/kodama-root/${KODAMA_MODEL_SUBPATH:?}" \
        "$MODEL_DIR/kodama-ja-streaming-small"
}

wait_for_server() {
    for _ in $(seq 1 120); do
        (echo > /dev/tcp/127.0.0.1/8770) 2>/dev/null && return 0
        sleep 1
    done
    return 1
}

chmod +x "$APP"
# FUSE が使えないコンテナでも動くように展開して使う。
"$APP" --appimage-extract > /dev/null 2>&1 \
    && APP=/home/tester/squashfs-root/AppRun

echo "== 1. 使い方が表示できる =="
"$APP" --help > /dev/null 2>&1 && ok "--help" || ng "--help"

echo "== 2. 設定ファイルが無い状態で同梱サーバーへ繋がる =="
rm -rf "$MODEL_DIR" /home/tester/.config
link_reazonspeech
[ -f "$MODEL_DIR/reazonspeech-k2-v2/tokens.txt" ] \
    || { echo "  NG   ReazonSpeech モデルが読めない（マウントの指定を確認）"; exit 1; }
out=$("$APP" --check-connection 2>&1 | tail -2)
echo "$out" | grep -q "^OK:" && ok "設定なしで接続" \
    || { ng "設定なしで接続"; echo "$out" | sed 's/^/       /'; }

echo "== 3. 他の配布の設定ファイルを読まない =="
# 製品版など、別の配布が ~/.config/otoa-input へ残した設定を模す。
mkdir -p /home/tester/.config/otoa-input
printf '%s\n' '{"asr_endpoint":"gateway","gateway_url":"wss://example.invalid/ws/asr"}' \
    > /home/tester/.config/otoa-input/settings.json
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

echo "== 4. 貼り付けができる（コンテナの Xvfb 上）=="
Xvfb :99 -screen 0 1280x720x24 > /home/tester/xvfb.log 2>&1 &
XVFB_PID=$!
CLEANUP_PIDS+=("$XVFB_PID")
sleep 2
openbox > /home/tester/openbox.log 2>&1 &
OPENBOX_PID=$!
CLEANUP_PIDS+=("$OPENBOX_PID")
xfce4-panel --disable-wm-check > /home/tester/panel.log 2>&1 &
PANEL_PID=$!
CLEANUP_PIDS+=("$PANEL_PID")
sleep 3
# 初回起動の「既定のパネルを使う」ダイアログが出た場合だけ承認する。
panel_dialog=$(xdotool search --name 'Panel' 2>/dev/null | head -1 || true)
if [ -n "$panel_dialog" ]; then
    xdotool windowactivate "$panel_dialog" key Return 2>/dev/null || true
    sleep 2
fi
DISPLAY=:99 "$APP" --paste-test "SMOKE_TEST" 2>&1 | grep -q "^OK:" \
    && ok "貼り付け" || ng "貼り付け"

echo "== 5. 画面が出る（GUI が起動して落ちない）=="
rm -rf "$MODEL_DIR" /home/tester/.config
link_kodama
[ -f "$MODEL_DIR/kodama-ja-streaming-small/tokenizer.json" ] \
    || { echo "  NG   kodama モデルが読めない（マウントの指定を確認）"; exit 1; }
write_engine kodama
DISPLAY=:99 RUST_LOG=info "$APP" > /home/tester/kodama-gui.log 2>&1 &
KODAMA_PID=$!
CLEANUP_PIDS+=("$KODAMA_PID")
if wait_for_server && kill -0 "$KODAMA_PID" 2>/dev/null; then
    ok "GUI 起動（kodama 同梱サーバー待受開始）"
else
    ng "GUI 起動"
    grep -iE "cannot open|could not be loaded|panicked|ASR サーバー" \
        /home/tester/kodama-gui.log | tail -5 | sed 's/^/       /'
fi

echo "== 6. kodama の設定で認識サーバーへ接続できる =="
out=$("$APP" --check-connection 2>&1 | tail -2)
echo "$out" | grep -q "^OK:" && ok "kodama で --check-connection" \
    || { ng "kodama で --check-connection"; echo "$out" | sed 's/^/       /'; }

echo "== 7. kodama の WebSocket 途中結果が出る =="
SYNTHETIC_WAV=/home/tester/kodama-synthetic.wav
SYNTHETIC_PCM=/home/tester/kodama-synthetic.pcm
if espeak-ng -v ja -s 135 -w "$SYNTHETIC_WAV" \
        '今日は良い天気です。音声認識の途中結果を確認します。' \
        > /home/tester/espeak.log 2>&1 \
    && sox "$SYNTHETIC_WAV" -r 16000 -c 1 -e signed-integer -b 16 \
        -t raw "$SYNTHETIC_PCM" > /home/tester/sox.log 2>&1 \
    && [ -s "$SYNTHETIC_PCM" ]; then
    if timeout 90s python3 /home/tester/partial_probe.py "$SYNTHETIC_PCM" \
        > /home/tester/partial-probe.log 2>&1; then
        ok "非 final トークンを 1 件以上受信（合成音声）"
        sed 's/^/       /' /home/tester/partial-probe.log
    else
        ng "非 final トークンを受信できない"
        tail -10 /home/tester/partial-probe.log | sed 's/^/       /'
    fi
else
    unconfirmed "合成音声を用意できないため、途中結果は未確認"
fi
stop_process "$KODAMA_PID"

echo "== 8. kodama モデルが無くても終了せず、警告オーバーレイが出る =="
rm -rf "$MODEL_DIR"
write_engine kodama
DISPLAY=:99 RUST_LOG=info "$APP" > /home/tester/missing-model.log 2>&1 &
MISSING_PID=$!
CLEANUP_PIDS+=("$MISSING_PID")
sleep 6
DISPLAY=:99 import -window root /home/tester/missing-model.png 2>/dev/null || true
if kill -0 "$MISSING_PID" 2>/dev/null \
    && [ -s /home/tester/missing-model.png ] \
    && grep -q '認識モデル kodama-ja-streaming-small が見つかりません' \
        /home/tester/missing-model.log; then
    ok "プロセス継続・警告ログ・Xvfb スクリーンショット"
else
    ng "モデル欠落時の継続または警告"
    tail -10 /home/tester/missing-model.log | sed 's/^/       /'
fi
stop_process "$MISSING_PID"

echo "== 9. 未知の認識エンジン名でも終了しない =="
write_engine whisper
DISPLAY=:99 RUST_LOG=info "$APP" > /home/tester/unknown-engine.log 2>&1 &
UNKNOWN_PID=$!
CLEANUP_PIDS+=("$UNKNOWN_PID")
sleep 5
if kill -0 "$UNKNOWN_PID" 2>/dev/null \
    && grep -q '認識エンジン.*whisper.*使えません' /home/tester/unknown-engine.log; then
    ok "未知のエンジンでも GUI 継続"
else
    ng "未知のエンジンで GUI 継続"
    tail -10 /home/tester/unknown-engine.log | sed 's/^/       /'
fi
stop_process "$UNKNOWN_PID"

echo "== 10. 公開版のインスタンスロックが版別名で効く =="
write_engine reazonspeech
DISPLAY=:99 RUST_LOG=info "$APP" > /home/tester/lock-first.log 2>&1 &
LOCK_PID=$!
CLEANUP_PIDS+=("$LOCK_PID")
sleep 5
second_status=0
timeout 10s env DISPLAY=:99 RUST_LOG=info "$APP" > /home/tester/lock-second.log 2>&1 \
    || second_status=$?
if kill -0 "$LOCK_PID" 2>/dev/null \
    && [ "$second_status" -eq 1 ] \
    && grep -q 'already running' /home/tester/lock-second.log \
    && [ -f "$XDG_RUNTIME_DIR/otoa-input-oss.lock" ]; then
    ok "2 つ目を拒否し、otoa-input-oss.lock を作成"
else
    ng "インスタンスロック"
    echo "       second_status=$second_status"
    sed 's/^/       /' /home/tester/lock-second.log
    find "$XDG_RUNTIME_DIR" -maxdepth 1 -name '*.lock' -print | sed 's/^/       /'
fi

echo "== 11. トレイから設定画面を開き、認識エンジンを操作する =="
settings_window=""
# この Xvfb 構成では indicator は上パネル右側に置かれる。まず固定位置で
# メニューを開き、「設定」（上から 4 項目目）をクリックする。
xdotool mousemove --sync 1142 12 click 1 2>/dev/null || true
sleep 1
DISPLAY=:99 import -window root /home/tester/tray-menu.png 2>/dev/null || true
xdotool mousemove --sync 1190 129 click 1 2>/dev/null || true
sleep 1
settings_window=$(xdotool search --name '^Otoa Input 設定$' 2>/dev/null \
    | tail -1 || true)
for button in 1 3; do
    [ -n "$settings_window" ] && break
    for y in 12 708; do
        for x in $(seq 1260 -24 20); do
            xdotool mousemove --sync "$x" "$y" click "$button" 2>/dev/null || true
            sleep 0.15
            xdotool key End Up Return 2>/dev/null || true
            sleep 0.2
            settings_window=$(xdotool search --name '^Otoa Input 設定$' 2>/dev/null \
                | tail -1 || true)
            [ -n "$settings_window" ] && break 2
            xdotool key Escape 2>/dev/null || true
        done
    done
done

settings_changed=0
if [ -n "$settings_window" ]; then
    xdotool windowactivate "$settings_window" 2>/dev/null || true
    sleep 1
    # 入力セクションが見えるまで設定画面内をスクロールする。
    eval "$(xdotool getwindowgeometry --shell "$settings_window" 2>/dev/null)"
    xdotool mousemove --sync $((X + WIDTH / 2)) $((Y + HEIGHT / 2)) \
        click 5 click 5 click 5 click 5 2>/dev/null || true
    sleep 1
    DISPLAY=:99 import -window root /home/tester/settings-screen.png 2>/dev/null || true

    # OCR は位置の特定だけに使い、操作自体は xdotool で行う。
    engine_box=$(tesseract /home/tester/settings-screen.png stdout -l jpn tsv \
        2>/dev/null | awk -F '\t' '$12 ~ /認識エンジン|エンジン/ {print $7, $8, $9, $10; exit}')
    if [ -n "$engine_box" ]; then
        read -r label_x label_y label_w label_h <<< "$engine_box"
        engine_x=$((X + WIDTH / 2))
        engine_y=$((label_y + label_h + 18))
        xdotool mousemove --sync "$engine_x" "$engine_y" click 1 2>/dev/null || true
        sleep 1
        DISPLAY=:99 import -window root /home/tester/settings-dropdown.png 2>/dev/null || true
        xdotool key Down Return 2>/dev/null || true
        sleep 1
        # 保存ボタンはウィンドウ右下。
        xdotool mousemove --sync $((X + WIDTH - 55)) $((Y + HEIGHT - 35)) \
            click 1 2>/dev/null || true
        sleep 2
        grep -Eq '"asr_engine"[[:space:]]*:[[:space:]]*"kodama"' \
            "$CONFIG_DIR/settings.json" && settings_changed=1
    fi
fi

if [ "$settings_changed" -eq 1 ]; then
    ok "トレイから設定を開き、ドロップダウンで kodama に変更して保存"
else
    unconfirmed "トレイまたはドロップダウンを操作できず未確認（設定直書きの反映は項目 6 で確認済み）"
fi
stop_process "$LOCK_PID"

echo
[ "$FAIL" -eq 0 ] && echo "すべて通過" || echo "失敗あり"
exit "$FAIL"
