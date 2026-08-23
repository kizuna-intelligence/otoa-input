#!/usr/bin/env bash
set -euo pipefail

export DISPLAY=:99
export XDG_RUNTIME_DIR=/tmp/otoa-ui-runtime
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

mkdir -p /out
: > /out/summary.txt

declare -a CLEANUP_PIDS=()

stop_pid() {
    local pid=$1
    if [ -z "$pid" ] || ! kill -0 "$pid" 2>/dev/null; then
        return 0
    fi
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 1 50); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
    done
    wait "$pid" 2>/dev/null || true
}

cleanup() {
    local pid
    for pid in "${CLEANUP_PIDS[@]}"; do
        stop_pid "$pid"
    done
}
trap cleanup EXIT

BACKGROUND_PID=''

paint_background() {
    xsetroot -solid '#6b7a90'
    if [ -n "$BACKGROUND_PID" ]; then
        stop_pid "$BACKGROUND_PID"
    fi
    # xcompmgr と Xvfb の組み合わせでは root の色が 8-bit 化されるため、
    # 24-bit の背景ウィンドウを先に置き、プレビュー窓をその上に載せる。
    xmessage -geometry 1280x800+0+0 -borderwidth 0 \
        -bg '#6b7a90' -buttons '' -timeout 86400 '' \
        > /tmp/otoa-ui-background.log 2>&1 &
    BACKGROUND_PID=$!
    CLEANUP_PIDS+=("$BACKGROUND_PID")
    sleep 0.2
}

Xvfb :99 -screen 0 1280x800x24 > /tmp/xvfb.log 2>&1 &
XVFB_PID=$!
CLEANUP_PIDS+=("$XVFB_PID")
sleep 1
xsetroot -solid '#6b7a90'

COMPOSITOR=0
if [ "${UI_COMPOSITOR:-}" = "1" ]; then
    xcompmgr > /tmp/xcompmgr.log 2>&1 &
    COMPOSITOR=1
    COMPOSITOR_PID=$!
    CLEANUP_PIDS+=("$COMPOSITOR_PID")
    sleep 1
fi

find_window() {
    local kind=$1
    local pattern
    if [ "$kind" = "overlay" ]; then
        pattern='^Otoa Input$'
    else
        pattern='Otoa Input の設定'
    fi
    for _ in $(seq 1 50); do
        local window_id
        window_id=$(xdotool search --onlyvisible --name "$pattern" 2>/dev/null | head -n 1 || true)
        if [ -n "$window_id" ]; then
            printf '%s\n' "$window_id"
            return 0
        fi
        if [ "$kind" = "settings" ]; then
            while read -r window_id; do
                if [ "$(xdotool getwindowname "$window_id" 2>/dev/null || true)" = 'Otoa Input の設定' ]; then
                    printf '%s\n' "$window_id"
                    return 0
                fi
            done < <(xdotool search --onlyvisible --name '.*' 2>/dev/null || true)
        fi
        sleep 0.1
    done
    return 1
}

window_geometry() {
    local window_id=$1
    local geometry position
    geometry=$(xdotool getwindowgeometry "$window_id" | awk '/Geometry:/ {print $2}')
    position=$(xdotool getwindowgeometry "$window_id" | awk '/Position:/ {print $2}')
    printf '%s %s\n' "$geometry" "$position"
}

crop_window() {
    local window_id=$1
    local output=$2
    local geometry position width height x y crop_x crop_y crop_width crop_height
    read -r geometry position < <(window_geometry "$window_id")
    width=${geometry%x*}
    height=${geometry#*x}
    x=${position%,*}
    y=${position#*,}
    crop_x=$((x - 40))
    crop_y=$((y - 40))
    [ "$crop_x" -lt 0 ] && crop_x=0
    [ "$crop_y" -lt 0 ] && crop_y=0
    crop_width=$((width + 80))
    crop_height=$((height + 80))
    import -window root -crop "${crop_width}x${crop_height}+${crop_x}+${crop_y}" "$output"
    printf '%sx%s+%s+%s\n' "$width" "$height" "$x" "$y"
}

cpu_average() {
    local pid=$1
    local top_file=$2
    top -b -n 3 -d 2 -p "$pid" > "$top_file" 2>&1 || true
    mapfile -t samples < <(awk -v pid="$pid" '$1 == pid {print $9}' "$top_file")
    local count=${#samples[@]}
    if [ "$count" -ge 2 ]; then
        awk -v first="${samples[$((count - 2))]}" -v second="${samples[$((count - 1))]}" \
            'BEGIN { printf "%.1f", (first + second) / 2 }'
    elif [ "$count" -eq 1 ]; then
        printf '%.1f' "${samples[0]}"
    else
        printf 'unknown'
    fi
}

transparent_from_log() {
    local log_file=$1
    local normalized
    normalized=$(sed $'s/\033\\[[0-9;]*m//g' "$log_file")
    if grep -Eq 'transparent[[:space:]]*=[[:space:]]*true' <<< "$normalized"; then
        printf 'true'
    elif grep -Eq 'transparent[[:space:]]*=[[:space:]]*false' <<< "$normalized"; then
        printf 'false'
    else
        printf 'unknown'
    fi
}

run_state() {
    local state=$1
    local safe_state=${state//:/-}
    local kind=overlay
    local argument="--preview-overlay=$state"
    if [[ "$state" == settings:* ]]; then
        kind=settings
        argument="--preview-settings=${state#settings:}"
    fi

    rm -f "/out/$safe_state.png" "/out/$safe_state.log" "/out/$safe_state.top"
    rm -rf /tmp/cfg /tmp/data
    mkdir -p /tmp/cfg /tmp/data
    paint_background

    XDG_CONFIG_HOME=/tmp/cfg XDG_DATA_HOME=/tmp/data RUST_LOG=info \
        /opt/otoa-input "$argument" > "/out/$safe_state.log" 2>&1 &
    local app_pid=$!
    sleep 2.5

    local window_id
    if ! window_id=$(find_window "$kind"); then
        echo "window not found: $state" >&2
        stop_pid "$app_pid"
        return 1
    fi

    xdotool mousemove 0 0
    local geometry
    geometry=$(crop_window "$window_id" "/out/$safe_state.png")
    local cpu
    cpu=$(cpu_average "$app_pid" "/out/$safe_state.top")
    local transparent
    transparent=$(transparent_from_log "/out/$safe_state.log")
    printf '%s %s %s compositor=%s transparent=%s\n' \
        "$state" "$geometry" "cpu=${cpu}%" "$COMPOSITOR" "$transparent" \
        >> /out/summary.txt
    stop_pid "$app_pid"
}

if [ "$#" -eq 0 ]; then
    echo 'usage: ui-preview-inner.sh <state...>' >&2
    exit 2
fi

for state in "$@"; do
    case "$state" in
        splash|connecting|listening|finalizing|committed|error|login|\
        settings:general|settings:mic|settings:asr|settings:advanced|\
        settings:account|settings:about)
            run_state "$state"
            ;;
        *)
            echo "unknown state: $state" >&2
            exit 2
            ;;
    esac
done
