#!/usr/bin/env python3
"""Turn Zen Maru Gothic Black's Otoa Input glyphs into a two-colour SVG."""

from __future__ import annotations

import argparse
import html
from pathlib import Path

from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.ttLib import TTFont


def make_wordmark(font_path: Path, output_path: Path, font_size: float = 128.0) -> None:
    font = TTFont(str(font_path))
    glyph_set = font.getGlyphSet()
    cmap = font.getBestCmap()
    units_per_em = font["head"].unitsPerEm
    scale = font_size / units_per_em

    text = "Otoa Input"
    x = 0.0
    paths: list[tuple[str, str, float]] = []
    min_x = float("inf")
    min_y = float("inf")
    max_x = float("-inf")
    max_y = float("-inf")

    for character in text:
        glyph_name = cmap.get(ord(character))
        if glyph_name is None:
            raise ValueError(f"font has no glyph for {character!r}")
        glyph = glyph_set[glyph_name]
        pen = SVGPathPen(glyph_set)
        glyph.draw(pen)
        if pen.getCommands():
            paths.append((character, pen.getCommands(), x))

            bounds_pen = BoundsPen(glyph_set)
            glyph.draw(bounds_pen)
            if bounds_pen.bounds is not None:
                x_min, y_min, x_max, y_max = bounds_pen.bounds
                min_x = min(min_x, x + x_min)
                min_y = min(min_y, y_min)
                max_x = max(max_x, x + x_max)
                max_y = max(max_y, y_max)

        advance, _ = font["hmtx"].metrics[glyph_name]
        x += advance

    if not paths or min_x == float("inf"):
        raise ValueError("wordmark has no drawable glyphs")

    width = (max_x - min_x) * scale
    height = (max_y - min_y) * scale
    x_offset = -min_x
    baseline = -min_y

    path_elements: list[str] = []
    for character, commands, glyph_x in paths:
        fill = "#0d1f3c" if character in "Otoa " else "#1c5cbb"
        path_elements.append(
            f'<path fill="{fill}" d="{html.escape(commands, quote=True)}" '
            f'transform="translate({(glyph_x + x_offset) * scale:.4f} '
            f'{baseline * scale:.4f}) scale({scale:.8f} {-scale:.8f})"/>'
        )

    svg = "\n".join(
        [
            '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 '
            f'{width:.4f} {height:.4f}" role="img" aria-label="Otoa Input">',
            *path_elements,
            "</svg>",
            "",
        ]
    )
    output_path.write_text(svg, encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--font",
        type=Path,
        default=Path("/home/yusuke/.local/share/fonts/ZenMaruGothic-Black.ttf"),
    )
    parser.add_argument(
        "--output", type=Path, default=Path("resources/icons/otoa-wordmark.svg")
    )
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    make_wordmark(args.font, args.output)


if __name__ == "__main__":
    main()
