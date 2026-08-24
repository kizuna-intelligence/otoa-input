#!/bin/bash
# デモを録画する。アプリ本体はホストからマウントした ELF を使う。
set -euo pipefail

export DISPLAY=:99
export XDG_RUNTIME_DIR=/tmp/rt
export XDG_CONFIG_HOME=/tmp/cfg
unset XDG_DATA_HOME
export RUST_LOG="info,otoa_input_app=debug,otoa_input_platform=debug"
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME/otoa-input-oss"

APP=/home/demo/otoa-input
APP_PID=
FF_PID=
XVFB_PID=
OPENBOX_PID=
COMPOSITOR_PID=
RAISER_PID=

cleanup() {
  for pid in "$RAISER_PID" "$FF_PID" "$APP_PID" "$COMPOSITOR_PID" "$OPENBOX_PID" "$XVFB_PID"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
    fi
  done
}
trap cleanup EXIT INT TERM

Xvfb :99 -screen 0 1000x620x24 > /home/demo/xvfb.log 2>&1 &
XVFB_PID=$!
sleep 2
xsetroot -solid '#263247'

openbox > /home/demo/openbox.log 2>&1 &
OPENBOX_PID=$!
sleep 1

# 透過表示を実際の環境に近づける。失敗した場合はアプリが不透過へ戻る。
if [[ "${DEMO_COMPOSITOR:-0}" == 1 ]]; then
  xcompmgr -n > /home/demo/xcompmgr.log 2>&1 &
  COMPOSITOR_PID=$!
  sleep 1
fi

# 仮想マイク。ALSA の default を PulseAudio の monitor へ向ける。
printf '%s\n' 'pcm.!default pulse' 'ctl.!default pulse' > "$HOME/.asoundrc"
pulseaudio --start --exit-idle-time=-1 --log-level=0 > /home/demo/pulseaudio.log 2>&1
export PULSE_SERVER="unix:$XDG_RUNTIME_DIR/pulse/native"
sleep 2
pactl load-module module-null-sink sink_name=vspk sink_properties=device.description=vspk > /dev/null
pactl set-default-sink vspk
pactl set-default-source vspk.monitor

# 合成音声。認識結果を確認しやすいよう、日本語音声を 16 kHz mono にする。
DEMO_FIRST_TEXT=${DEMO_FIRST_TEXT:-音声入力のデモです}
DEMO_SECOND_TEXT=${DEMO_SECOND_TEXT:-話した内容がそのままカーソル位置に貼り付けられます}
DEMO_SECOND_TTS=${DEMO_SECOND_TTS:-remove-no}
if command -v open_jtalk >/dev/null 2>&1; then
  OPEN_JTALK_DIC=/var/lib/mecab/dic/open-jtalk/naist-jdic
  OPEN_JTALK_VOICE=/usr/share/hts-voice/nitech-jp-atr503-m001/nitech_jp_atr503_m001.htsvoice
  OPEN_JTALK_RATE=${OPEN_JTALK_RATE:-1.0}
  printf '%s\n' "$DEMO_FIRST_TEXT" \
    | open_jtalk -r "$OPEN_JTALK_RATE" -x "$OPEN_JTALK_DIC" -m "$OPEN_JTALK_VOICE" -ow /home/demo/speech-1-jtalk.wav
  if [[ "$DEMO_SECOND_TTS" == remove-no ]]; then
    # 「カーソルの位置」を合成し、助詞「の」の音声区間だけを除いて
    # 「カーソル位置」として認識される読み上げにする。
    printf '%s\n' '話した内容がそのままカーソルの位置に貼り付けられます' \
      | open_jtalk -r "$OPEN_JTALK_RATE" -x "$OPEN_JTALK_DIC" -m "$OPEN_JTALK_VOICE" -ow /home/demo/speech-2-full-jtalk.wav
    no_start=$(awk -v rate="$OPEN_JTALK_RATE" 'BEGIN { printf "%.3f", 2.315 / rate }')
    no_end=$(awk -v rate="$OPEN_JTALK_RATE" 'BEGIN { printf "%.3f", 2.435 / rate }')
    sox /home/demo/speech-2-full-jtalk.wav /home/demo/speech-2-before.wav trim 0 "$no_start"
    sox /home/demo/speech-2-full-jtalk.wav /home/demo/speech-2-after.wav trim "$no_end"
    sox /home/demo/speech-2-before.wav /home/demo/speech-2-after.wav /home/demo/speech-2-jtalk.wav
  else
    printf '%s\n' "$DEMO_SECOND_TEXT" \
      | open_jtalk -r "$OPEN_JTALK_RATE" -x "$OPEN_JTALK_DIC" -m "$OPEN_JTALK_VOICE" -ow /home/demo/speech-2-jtalk.wav
  fi
  SPEECH_1=/home/demo/speech-1-jtalk.wav
  SPEECH_2=/home/demo/speech-2-jtalk.wav
else
  DEMO_SPEECH_SPEED=${DEMO_SPEECH_SPEED:-180}
  espeak-ng -v ja -s "$DEMO_SPEECH_SPEED" -w /home/demo/speech-1-espeak.wav "$DEMO_FIRST_TEXT"
  espeak-ng -v ja -s "$DEMO_SPEECH_SPEED" -w /home/demo/speech-2-espeak.wav "$DEMO_SECOND_TEXT"
  SPEECH_1=/home/demo/speech-1-espeak.wav
  SPEECH_2=/home/demo/speech-2-espeak.wav
fi
sox "$SPEECH_1" -r 16000 -c 1 -b 16 -e signed-integer /home/demo/speech-1.wav
sox "$SPEECH_2" -r 16000 -c 1 -b 16 -e signed-integer /home/demo/speech-2.wav
printf '%s\n' '{"asr_engine":"reazonspeech"}' > "$XDG_CONFIG_HOME/otoa-input-oss/settings.json"

# 貼り付け先のエディタ。
mousepad > /home/demo/mousepad.log 2>&1 &
sleep 3
MOUSEPAD=$(xdotool search --name 'Mousepad' | tail -n 1)
xdotool windowmove "$MOUSEPAD" 20 60
xdotool windowsize "$MOUSEPAD" 960 480
xdotool windowactivate --sync "$MOUSEPAD"
sleep 1

# 本体（同梱サーバーごと自分で立ち上がる）。
test -x "$APP"
"$APP" > /home/demo/app.log 2>&1 &
APP_PID=$!

echo "モデル読み込みを待つ"
ready=0
for _ in $(seq 1 180); do
  if (echo > /dev/tcp/127.0.0.1/8770) 2>/dev/null; then
    ready=1
    break
  fi
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "アプリが起動途中で終了しました" >&2
    tail -80 /home/demo/app.log >&2 || true
    exit 1
  fi
  sleep 1
done
if [[ "$ready" != 1 ]]; then
  echo "モデル読み込みがタイムアウトしました" >&2
  tail -80 /home/demo/app.log >&2 || true
  exit 1
fi
sleep 4
xdotool windowactivate --sync "$MOUSEPAD"
xdotool mousemove 0 0
# Mousepad を入力先として前面にしたあと、AlwaysOnTop のバーを重ねる。
OVERLAY_WINDOW=$(xdotool search --name '^Otoa Input$' | head -n 1 || true)
if [[ -n "$OVERLAY_WINDOW" ]]; then
  xdotool windowraise "$OVERLAY_WINDOW"
fi
sleep 1

echo "録画開始"
ffmpeg -y -loglevel error -f x11grab -framerate 15 -draw_mouse 0 \
       -video_size 1000x620 -i :99 -t 25 /home/demo/out.mp4 &
FF_PID=$!
# Mousepad のアクティブ状態を保ったまま、状態変更後もバーを前面に置く。
(
  while kill -0 "$FF_PID" 2>/dev/null; do
    overlay_window=$(xdotool search --name '^Otoa Input$' 2>/dev/null | head -n 1 || true)
    if [[ -n "$overlay_window" ]]; then
      xdotool windowraise "$overlay_window" 2>/dev/null || true
    fi
    sleep 0.1
  done
) &
RAISER_PID=$!
sleep 1

paplay --device=vspk /home/demo/speech-1.wav
sleep 5
xclip -o -selection clipboard > /home/demo/first.txt || true
paplay --device=vspk /home/demo/speech-2.wav
sleep 8

wait "$FF_PID"
FF_PID=

# 最後の画面と、Mousepad に実際に入った全文を保存する。
ffmpeg -y -loglevel error -f x11grab -video_size 1000x620 -i :99 -frames:v 1 /home/demo/final.png
xdotool windowactivate --sync "$MOUSEPAD"
xdotool key --clearmodifiers ctrl+a
xdotool key --clearmodifiers ctrl+c
sleep 1
xclip -o -selection clipboard > /home/demo/mousepad.txt

echo "録画終了: $(stat -c '%s' /home/demo/out.mp4) バイト"
echo "--- Mousepad に貼り付いた全文 ---"
cat /home/demo/mousepad.txt
echo "--- アプリのログ（末尾）---"
grep -aE 'emit start|emit done|overlay view changed|ERROR' /home/demo/app.log \
  | sed 's/\x1b\[[0-9;]*m//g' | tail -20 || true
