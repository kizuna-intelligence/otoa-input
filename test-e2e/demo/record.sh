#!/bin/bash
# デモを録画する。
set -u
export DISPLAY=:99
export XDG_RUNTIME_DIR=/tmp/rt && mkdir -p "$XDG_RUNTIME_DIR"

Xvfb :99 -screen 0 1000x620x24 > /dev/null 2>&1 &
sleep 2
openbox > /dev/null 2>&1 &
sleep 1

# 仮想マイク。null sink の monitor を既定の入力にする。
pulseaudio --start --exit-idle-time=-1 --log-level=0 > /dev/null 2>&1
sleep 2
pactl load-module module-null-sink sink_name=vspk sink_properties=device.description=vspk > /dev/null
pactl set-default-sink vspk
pactl set-default-source vspk.monitor
sleep 1

# 貼り付け先のエディタ
mousepad > /dev/null 2>&1 &
sleep 3
xdotool search --name "Mousepad" windowmove 20 60 windowsize 960 480 windowactivate 2>/dev/null
sleep 1

# 本体（同梱サーバーごと自分で立ち上がる）
chmod +x /home/demo/otoa-input.AppImage
/home/demo/otoa-input.AppImage --appimage-extract > /dev/null 2>&1
APP=/home/demo/squashfs-root/AppRun
nohup "$APP" > /home/demo/app.log 2>&1 &

echo "モデル読み込みを待つ"
for i in $(seq 1 120); do
  (echo > /dev/tcp/127.0.0.1/8770) 2>/dev/null && break
  sleep 1
done
sleep 5
xdotool search --name "Mousepad" windowactivate 2>/dev/null
sleep 1

echo "録画開始"
ffmpeg -y -loglevel error -f x11grab -framerate 15 -video_size 1000x620 -i :99 \
       -t 22 /home/demo/out.mp4 &
FF=$!
sleep 2

# 仮想マイクへ流す（2 回）
paplay --device=vspk /home/demo/demo_speech.wav
sleep 5
paplay --device=vspk /home/demo/demo_speech.wav
sleep 6

wait $FF
# 最後の画面も撮る。貼り付いたかを目で確かめるため。
import_ok=0
command -v import >/dev/null && import -window root /home/demo/final.png 2>/dev/null && import_ok=1
ffmpeg -y -loglevel error -f x11grab -video_size 1000x620 -i :99 -frames:v 1 /home/demo/final.png 2>/dev/null
echo "録画終了: $(ls -la /home/demo/out.mp4 | awk '{print $5}') バイト"
echo "--- アプリのログ（末尾）---"
grep -aE "emit start|emit done|overlay view changed|ERROR" /home/demo/app.log | sed 's/\x1b\[[0-9;]*m//g' | tail -8
