#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
ROOT_DIR=$(cd -- "$SCRIPT_DIR/.." && pwd)
ICON_DIR="$ROOT_DIR/resources/icons"
TRAY_DIR="$ICON_DIR/tray"

mkdir -p "$TRAY_DIR"
python3 "$SCRIPT_DIR/generate-wordmark.py"

for size in 16 22 24 32 48; do
  rsvg-convert -w "$size" -h "$size" \
    "$ICON_DIR/otoa-input.svg" \
    -o "$TRAY_DIR/otoa-tray-$size.png"
done

template_svg=$(mktemp)
trap 'rm -f "$template_svg"' EXIT
sed 's/currentColor/#000000/g' "$ICON_DIR/otoa-mark.svg" >"$template_svg"
for size in 16 22 32; do
  rsvg-convert -w "$size" -h "$size" \
    "$template_svg" \
    -o "$TRAY_DIR/otoa-tray-template-$size.png"
done
