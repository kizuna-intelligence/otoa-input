#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
ROOT_DIR=$(cd -- "$SCRIPT_DIR/.." && pwd)
ICON_DIR="$ROOT_DIR/resources/icons"
TRAY_DIR="$ICON_DIR/tray"

mkdir -p "$TRAY_DIR"
python3 "$SCRIPT_DIR/generate-wordmark.py" \
  --output "$ICON_DIR/otoa-wordmark.svg"
python3 "$SCRIPT_DIR/generate-wordmark.py" \
  --part otoa \
  --output "$ICON_DIR/otoa-wordmark-otoa.svg"
python3 "$SCRIPT_DIR/generate-wordmark.py" \
  --part input \
  --output "$ICON_DIR/otoa-wordmark-input.svg"

normal_svg=$(mktemp)
attention_svg=$(mktemp)
stopped_svg=$(mktemp)
trap 'rm -f "$template_svg" "$normal_svg" "$attention_svg" "$stopped_svg"' EXIT

# 通常はマーク本体だけ、要対応時だけ右下の琥珀の点を足す。
sed '/<circle cx="716" cy="716"/d' "$ICON_DIR/otoa-input.svg" >"$normal_svg"
cp "$ICON_DIR/otoa-input.svg" "$attention_svg"
sed 's/currentColor/#9fb3d6/g' "$ICON_DIR/otoa-mark.svg" >"$stopped_svg"

for size in 16 22 24 32 48; do
  for variant in normal attention stopped; do
    case "$variant" in
      normal) source_svg="$normal_svg"; output_name="otoa-tray-$size.png" ;;
      attention) source_svg="$attention_svg"; output_name="otoa-tray-attention-$size.png" ;;
      stopped) source_svg="$stopped_svg"; output_name="otoa-tray-stopped-$size.png" ;;
    esac
    rsvg-convert -w "$size" -h "$size" \
      "$source_svg" \
      -o "$TRAY_DIR/$output_name"
  done
done

# tray-icon は RGBA の生データを要求するため、PNG と同じ画像を依存なしで埋め込める
# ように保存する。通常版のファイル名だけは P0 から維持する。
for size in 16 22 32; do
  for variant in normal attention stopped; do
    png="$TRAY_DIR/otoa-tray-$size.png"
    if [ "$variant" != normal ]; then
      png="$TRAY_DIR/otoa-tray-${variant}-$size.png"
    fi
    raw_name="otoa-tray-$size.rgba"
    if [ "$variant" != normal ]; then
      raw_name="otoa-tray-${variant}-$size.rgba"
    fi
    convert "$png" -depth 8 "rgba:$TRAY_DIR/$raw_name"
  done
done

template_svg=$(mktemp)
trap 'rm -f "$template_svg"' EXIT
sed 's/currentColor/#000000/g' "$ICON_DIR/otoa-mark.svg" >"$template_svg"
for size in 16 22 32; do
  rsvg-convert -w "$size" -h "$size" \
    "$template_svg" \
    -o "$TRAY_DIR/otoa-tray-template-$size.png"
  convert "$TRAY_DIR/otoa-tray-template-$size.png" -depth 8 \
    "rgba:$TRAY_DIR/otoa-tray-template-$size.rgba"
done
