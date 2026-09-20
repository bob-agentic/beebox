#!/usr/bin/env python3
"""Draws the BeeBox app icon: the "double honeycomb" mark (④).

macOS icons are not full-bleed: the artwork sits in a rounded square that fills
about 80% of the canvas, with transparent margin around it. Filling the whole
1024px canvas makes the icon read noticeably larger than every neighbour in the
Dock, so the squircle is inset.

The mark is a hexagon (honeycomb cell) with an outer stroke and a faint inner
fill, and a terminal prompt `>_` centred inside it — the same artwork shown in
design/icons.html, variant ④.

Rendered with Pillow at 4x supersampling, then downsampled, so every edge is
smooth without the hand-rolled coverage sampling the first version needed.

    python3 icons/make-icon.py   ->   icons/icon.png (1024px, transparent bg)
"""

import math
from pathlib import Path

from PIL import Image, ImageDraw

S = 1024                 # final canvas
SS = 4                   # supersampling factor
C = S * SS               # working canvas

INSET = 100 * SS         # transparent margin (Apple's ~80% grid)
RADIUS = 185 * SS        # squircle corner radius

# Palette — matches the UI tokens.
BG_TOP = (32, 48, 58)        # #20303a
BG_BOTTOM = (11, 15, 19)     # #0b0f13
FG = (74, 222, 128)          # --accent  #4ade80
INNER_FILL = (74, 222, 128)  # same green, drawn at low alpha


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def hexagon(cx, cy, r):
    """Pointy-top hexagon vertices, matching the SVG mark."""
    pts = []
    for i in range(6):
        a = math.radians(60 * i - 90)
        pts.append((cx + r * math.cos(a), cy + r * math.sin(a)))
    return pts


def main() -> int:
    # --- rounded-square background with a vertical gradient ---
    # Build the gradient as a full-canvas image, then punch it through a
    # rounded-rectangle mask so only the squircle shows.
    grad = Image.new("RGB", (C, C), BG_BOTTOM)
    gpx = grad.load()
    x0, y0, x1, y1 = INSET, INSET, C - INSET, C - INSET
    for y in range(C):
        t = (y - y0) / (y1 - y0) if y1 > y0 else 0.0
        row = lerp(BG_TOP, BG_BOTTOM, min(max(t, 0.0), 1.0))
        for x in range(C):
            gpx[x, y] = row

    mask = Image.new("L", (C, C), 0)
    ImageDraw.Draw(mask).rounded_rectangle([x0, y0, x1, y1], radius=RADIUS, fill=255)

    canvas = Image.new("RGBA", (C, C), (0, 0, 0, 0))
    canvas.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(canvas)

    # --- the mark: double hexagon + centred prompt ---
    cx = cy = C / 2
    # Scale the 128-unit design coordinates up to the working canvas. The design
    # draws on a 128 box; the artwork area here is (C - 2*INSET) wide.
    art = (C - 2 * INSET)
    u = art / 128.0                      # one design unit, in working px
    origin = INSET                       # design (0,0) maps here

    def D(v):                            # design length -> working px
        return v * u

    def P(dx, dy):                       # design point -> working px
        return (origin + dx * u, origin + dy * u)

    hcx, hcy = P(64, 64)

    # outer hexagon, stroked (design: r=48, stroke-width=4)
    outer = hexagon(hcx, hcy, D(48))
    ow = round(D(4))
    d.line(outer + [outer[0]], fill=FG, width=ow, joint="curve")
    # PIL's polyline leaves a hairline seam where the stroke closes on itself
    # (the top vertex): overdraw each vertex with a dot so the ring reads as one
    # continuous outline.
    for vx, vy in outer:
        r = ow / 2
        d.ellipse([vx - r, vy - r, vx + r, vy + r], fill=FG)

    # inner hexagon, faint fill (design: r=30, opacity .16)
    inner = hexagon(hcx, hcy, D(30))
    fillimg = Image.new("RGBA", (C, C), (0, 0, 0, 0))
    ImageDraw.Draw(fillimg).polygon(inner, fill=(*INNER_FILL, round(255 * 0.16)))
    canvas = Image.alpha_composite(canvas, fillimg)
    d = ImageDraw.Draw(canvas)

    # centred `>_` prompt (design: scale .9 of the base glyph, centred on 64,64)
    scale = 0.9
    ch = 12 * scale
    cw = 15 * scale
    gap = 7 * scale
    ul = 20 * scale
    total = cw + gap + ul
    gx0 = 64 - total / 2
    tip = gx0 + cw
    uy = 64 + ch * 0.72
    w = round(D(6))
    # chevron
    d.line([P(gx0, 64 - ch), P(tip, 64), P(gx0, 64 + ch)],
           fill=FG, width=w, joint="curve")
    # round the chevron ends and vertex a touch by overdrawing dots
    for pt in (P(gx0, 64 - ch), P(gx0, 64 + ch), P(tip, 64)):
        r = w / 2
        d.ellipse([pt[0] - r, pt[1] - r, pt[0] + r, pt[1] + r], fill=FG)
    # underscore
    ux0, uxy = P(tip + gap, uy)
    ux1, _ = P(tip + gap + ul, uy)
    d.line([(ux0, uxy), (ux1, uxy)], fill=FG, width=w)
    for pt in ((ux0, uxy), (ux1, uxy)):
        r = w / 2
        d.ellipse([pt[0] - r, pt[1] - r, pt[0] + r, pt[1] + r], fill=FG)

    # --- downsample to final size ---
    final = canvas.resize((S, S), Image.LANCZOS)
    out = Path(__file__).with_name("icon.png")
    final.save(out)
    print(f"{out.name}: {out.stat().st_size // 1024} KB, honeycomb mark, inset {INSET // SS}px")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
