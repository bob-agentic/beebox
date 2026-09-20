#!/usr/bin/env python3
"""Subset MesloLGS NF to what a terminal renders, then compress to WOFF2.

The full family is ~10 MB of TTF, most of it icon sets nothing here draws.
Subsetting plus WOFF2 gets that to a few hundred KB.

Note xterm.js draws box-drawing and many Powerline separators itself, as
vectors, in its CustomGlyphs module — but we keep those ranges anyway: the
canvas renderer falls back to the font for glyphs it has no vector for, and a
missing one shows as tofu, which is exactly the failure we saw on a phone.

Run from ui/:  python3 scripts/build-font.py
"""

import subprocess
import sys
from pathlib import Path

# Ranges a terminal actually needs.
UNICODES = ",".join([
    "U+0000-00FF",    # Latin-1
    "U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC",
    "U+0300-036F",    # combining marks
    "U+2000-206F",    # general punctuation, incl. the  ›  the UI draws
    "U+2070-209F",    # super/subscripts
    "U+20A0-20BF",    # currency
    "U+2100-214F",    # letterlike, incl. ℹ
    "U+2190-21FF",    # arrows
    "U+2200-22FF",    # maths operators, incl. ✓-adjacent
    "U+2300-23FF",    # misc technical, incl. ⏺ ⎇
    "U+2500-257F",    # box drawing   ╭──╮
    "U+2580-259F",    # block elements
    "U+25A0-25FF",    # geometric shapes
    "U+2600-26FF",    # misc symbols
    "U+2700-27BF",    # dingbats, incl. ✓
    "U+2B00-2BFF",    # arrows and symbols
    "U+E0A0-E0A3",    # Powerline
    "U+E0B0-E0D7",    # Powerline extra separators
    "U+F000-F2FF",    # Font Awesome / devicons (Starship uses these)
])

# Kept so wide glyphs stay wide; dropped ligatures save GSUB weight.
FLAGS = [
    f"--unicodes={UNICODES}",
    "--layout-features=",
    "--flavor=woff2",
    "--desubroutinize",
    "--no-hinting",
]


def main() -> int:
    src_dir = Path.home() / "Library" / "Fonts"
    out_dir = Path("public/fonts")
    out_dir.mkdir(parents=True, exist_ok=True)

    faces = {
        "Regular": "MesloLGS NF Regular.ttf",
        "Bold": "MesloLGS NF Bold.ttf",
        "Italic": "MesloLGS NF Italic.ttf",
        "BoldItalic": "MesloLGS NF Bold Italic.ttf",
    }

    total = 0
    for name, filename in faces.items():
        src = src_dir / filename
        if not src.exists():
            print(f"skip {name}: {src} not found", file=sys.stderr)
            continue

        out = out_dir / f"MesloLGS-NF-{name}.woff2"
        subprocess.run(
            [sys.executable, "-m", "fontTools.subset", str(src), f"--output-file={out}", *FLAGS],
            check=True,
        )
        size = out.stat().st_size
        total += size
        print(f"{name:11} {src.stat().st_size // 1024:>5} KB -> {size // 1024:>4} KB")

    # Remove the TTFs we copied in while experimenting.
    for ttf in out_dir.glob("*.ttf"):
        ttf.unlink()

    print(f"{'total':11} {'':>5}    {total // 1024:>4} KB")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
