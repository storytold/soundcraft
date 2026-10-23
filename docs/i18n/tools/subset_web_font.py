#!/usr/bin/env python3
"""Regenerate `assets/fonts/NotoSansSC-SoundCraftSubset.ttf`.

Usage (run from the repo root):
    python3 docs/i18n/tools/subset_web_font.py NotoSansSC-VariableFont_wght.ttf
    python3 docs/i18n/tools/subset_web_font.py --check

The web build has no operating-system fonts, so it bundles a small subset of
Noto Sans SC (SIL OFL-1.1, see `assets/fonts/OFL.txt` and `ATTRIBUTION.md`),
pinned at weight 400. The full variable font is a noto-cjk release:
https://github.com/notofonts/noto-cjk (Sans/Variable/OTF or the "Subset"
OTC — any file whose family is Noto Sans SC with a wght axis).

The codepoint set is a superset of the UI glyph set:

  - printable ASCII (U+0020-007E) and Latin-1 (U+00A0-00FF);
  - the GB 2312 level-1 common Han characters (3,755, rows 16-55), so the
    catalog can grow new everyday words without touching this script;
  - every BMP, non-control character used in `crates/ui-egui/src/i18n/*.tsv`
    (sources, translations and comments — emoji outside the BMP come from
    egui's bundled emoji fonts, see `fonts.rs`);
  - `EXTRA_SYMBOLS` below: the punctuation, arrows and symbols the UI draws
    outside the catalogs (meters, transport, musical notation).

`--check` compares that set against the shipped subset and exits non-zero on
any difference; the catalog-side guarantee (every catalog glyph present) is
enforced by the `bundled_web_font_*` tests in `crates/ui-egui/src/fonts.rs`.

Requires fontTools (`pip install fonttools`). Steps: pin weight 400 with
`fontTools.varLib.instancer`, then subset with `fontTools.subset`, keeping the
name table (the OFL copyright notice rides along) and all layout features.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
CATALOG_DIR = REPO / "crates/ui-egui/src/i18n"
SHIPPED = REPO / "assets/fonts/NotoSansSC-SoundCraftSubset.ttf"

# Punctuation, arrows and symbols the UI draws outside the catalogs. Comment
# each addition; the `--check` run against the shipped font catches drift.
EXTRA_SYMBOLS = (
    "ΔΣπ"        # Greek letters used in math/technical readouts
    "–‘’′″›※"    # dashes, quotes, primes and reference marks
    "←↑→↓↔⬆"     # arrows (transport, nudge, navigation)
    "∈√∞≈≤≥"     # math symbols
    "⌘"          # command-key hint glyphs
    "─★"         # box drawing and rating star
    "♩♯"         # musical notation
    "〈〉《》「」『』【】〔〕"  # CJK brackets
    "！￥"        # fullwidth exclamation mark, yen sign
)


def collect_codepoints() -> set[int]:
    """The full codepoint set the subset must carry."""
    points = set(range(0x20, 0x7F)) | set(range(0xA0, 0x100))
    # GB 2312 level-1 Han: rows 16-55, decoded straight from the encoding so
    # no external table is needed (row 55 stops at column 89).
    for row in range(16, 56):
        for col in range(1, 95):
            try:
                points.add(ord(bytes([0xA0 + row, 0xA0 + col]).decode("gb2312")))
            except UnicodeDecodeError:
                continue
    for path in sorted(CATALOG_DIR.glob("*.tsv")):
        for ch in path.read_text(encoding="utf-8"):
            cp = ord(ch)
            if cp <= 0xFFFF and not ch.isspace() and not unicodedata_category_cc(ch):
                points.add(cp)
    points |= {ord(ch) for ch in EXTRA_SYMBOLS}
    return points


def unicodedata_category_cc(ch: str) -> bool:
    import unicodedata

    return unicodedata.category(ch) == "Cc"


def shipped_codepoints() -> set[int]:
    from fontTools.ttLib import TTFont

    return set(TTFont(SHIPPED).getBestCmap())


def check(points: set[int]) -> int:
    shipped = shipped_codepoints()
    missing = sorted(points - shipped)
    stale = sorted(shipped - points)
    for label, diff in (("missing from the shipped subset", missing), ("no longer needed", stale)):
        if diff:
            print(f"{label}: {len(diff)}")
            for cp in diff[:40]:
                print(f"  U+{cp:04X} {chr(cp)}")
    if missing:
        print("regenerate: python3 docs/i18n/tools/subset_web_font.py <NotoSansSC variable font>")
        return 1
    if stale:
        print("the recipe and the shipped subset drifted — regenerate or update EXTRA_SYMBOLS")
        return 1
    print(f"ok: the shipped subset matches the recipe exactly ({len(points)} codepoints)")
    return 0


def regenerate(font: Path, points: set[int], out: Path) -> int:
    with tempfile.TemporaryDirectory() as tmp:
        instanced = Path(tmp) / "NotoSansSC-400.ttf"
        subprocess.run(
            [sys.executable, "-m", "fontTools.varLib.instancer", str(font), "wght=400", "-o", str(instanced)],
            check=True,
        )
        unicodes = ",".join(f"U+{cp:04X}" for cp in sorted(points))
        subprocess.run(
            [
                sys.executable, "-m", "fontTools.subset", str(instanced),
                f"--output-file={out}",
                f"--unicodes={unicodes}",
                "--layout-features=*",
                "--glyph-names",
                "--name-IDs=*",
                "--name-legacy",
                "--name-languages=*",
                "--notdef-glyph",
                "--notdef-outline",
                "--recommended-glyphs",
            ],
            check=True,
        )
    size = out.stat().st_size
    print(f"wrote {out} ({size} bytes, {len(points)} codepoints)")
    if size > 2 * 1024 * 1024:
        print("warning: over the 2 MiB web budget enforced by the fonts.rs tests")
        return 1
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("font", type=Path, nargs="?", help="Noto Sans SC variable font (a noto-cjk release)")
    ap.add_argument("--out", type=Path, default=SHIPPED)
    ap.add_argument("--check", action="store_true", help="compare the recipe against the shipped subset and exit")
    args = ap.parse_args()

    points = collect_codepoints()
    if args.check:
        return check(points)
    if args.font is None:
        ap.error("a Noto Sans SC variable font is required (or use --check)")
    if not args.font.is_file():
        print(f"error: {args.font} is not a file", file=sys.stderr)
        return 1
    return regenerate(args.font, points, args.out)


if __name__ == "__main__":
    sys.exit(main())
