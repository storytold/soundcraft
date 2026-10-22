#!/usr/bin/env python3
"""Cross-check the SoundCraft zh-Hans catalog against reference DAW corpora.

Usage (run from the repo root):
    python3 docs/i18n/tools/cross_check.py [--corpus DIR] [--out FILE] [--detail FILE]

Reads:
  crates/ui-egui/src/i18n/zh-hans.tsv         our catalog
  docs/i18n/sources/raw/*.po, *.ts            reference translations (local,
                                              gitignored; see docs/i18n/sources/SOURCES.md)

Writes two Markdown reports: a committed summary with verdicts and counts
only, and a local detail table with the per-DAW renderings and the majority
vote, written inside the gitignored corpus directory. The reference corpora
are GPL-licensed, so their strings never enter the repository — the committed
report records terminology *facts* (how many reference projects agree with a
key), not their text. Both reports are advisory only — they do not modify the
catalog. Any catalog change still goes through docs/localization-zh-hans.md
review.
"""

from __future__ import annotations

import argparse
import collections
import re
import sys
import unicodedata
import xml.etree.ElementTree as ET
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
CATALOG = REPO / "crates/ui-egui/src/i18n/zh-hans.tsv"
DEFAULT_CORPUS = REPO / "docs/i18n/sources/raw"
DEFAULT_OUT = REPO / "docs/i18n/cross-check-report.md"
# The detail table quotes GPL-licensed corpus strings verbatim, so it lives
# inside the gitignored corpus directory and is never committed.
DEFAULT_DETAIL = DEFAULT_CORPUS / "cross-check-detail.md"


# --------------------------------------------------------------------------- #
# normalisation
# --------------------------------------------------------------------------- #

def norm_en(s: str) -> str:
    """Fold an English source string to a comparable key."""
    s = unicodedata.normalize("NFKC", s)
    s = s.replace("&", "").replace("…", "").replace("...", "")
    s = re.sub(r"[^0-9A-Za-z]+", " ", s)  # punctuation & separators -> space
    return " ".join(s.lower().split())


def norm_zh(s: str) -> str:
    """Fold a Chinese translation so votes compare the words, not spacing."""
    s = unicodedata.normalize("NFKC", s)
    s = s.replace("（", "(").replace("）", ")")
    return " ".join(s.split())


def zh_compare(s: str) -> str:
    """Further fold for voting: trailing ellipses, dots and colons are
    punctuation our coverage tests enforce independently, so they must not
    turn into terminology disagreements."""
    s = norm_zh(s)
    return s.rstrip(" .。…：:")


# --------------------------------------------------------------------------- #
# parsers
# --------------------------------------------------------------------------- #

def parse_po(path: Path) -> list[tuple[str, str]]:
    """Return (english, chinese) pairs from a gettext .po file."""
    pairs: list[tuple[str, str]] = []
    msgid: list[str] | None = None
    msgstr: list[str] | None = None

    def flush() -> None:
        nonlocal msgid, msgstr
        if msgid is not None and msgstr is not None:
            en = "".join(msgid)
            zh = "".join(msgstr)
            if en and zh and not zh.startswith(("没", "未")) or (en and zh):
                pairs.append((en, zh))
        msgid = msgstr = None

    for raw in path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = raw.strip()
        if line.startswith("msgid "):
            flush()
            msgid = [_po_unquote(line[6:])]
        elif line.startswith("msgstr "):
            msgstr = [_po_unquote(line[7:])]
        elif line.startswith('"') and msgid is not None and msgstr is None:
            msgid.append(_po_unquote(line))
        elif line.startswith('"') and msgstr is not None:
            msgstr.append(_po_unquote(line))
    flush()
    return pairs


def _po_unquote(s: str) -> str:
    s = s.strip()
    if s.startswith('"') and s.endswith('"'):
        s = s[1:-1]
    return s.encode("utf-8").decode("unicode_escape") if "\\" in s else s


def parse_ts(path: Path) -> list[tuple[str, str]]:
    """Return (english, chinese) pairs from a Qt .ts file."""
    pairs: list[tuple[str, str]] = []
    try:
        root = ET.parse(path).getroot()
    except ET.ParseError as exc:  # pragma: no cover - corpus is local data
        print(f"warning: {path.name}: XML parse error: {exc}", file=sys.stderr)
        return pairs
    for message in root.iter("message"):
        translation_el = message.find("translation")
        source = message.findtext("source")
        if not source or translation_el is None:
            continue
        if translation_el.get("type") in {"unfinished", "vanished"}:
            continue
        # plural forms: join all <numerusform> texts
        numerus = translation_el.findall("numerusform")
        if numerus:
            zh = " | ".join(n.text or "" for n in numerus)
        else:
            zh = translation_el.text or ""
        zh = zh.strip()
        en = source.strip()
        if en and zh:
            pairs.append((en, zh))
    return pairs


def load_corpus(corpus_dir: Path) -> dict[str, dict[str, set[str]]]:
    """daw -> folded english -> set of chinese translations."""
    daw_names = {
        "ardour": "Ardour",
        "audacity": "Audacity",
        "lmms": "LMMS",
        "mixxx": "Mixxx",
        "hydrogen": "Hydrogen",
        "musescore": "MuseScore",
        "instruments": "MuseScore",  # MuseScore's instrument-name catalog
    }
    table: dict[str, dict[str, set[str]]] = collections.defaultdict(lambda: collections.defaultdict(set))
    for path in sorted(corpus_dir.rglob("*")):
        if not path.is_file():
            continue
        if path.suffix == ".po":
            pairs = parse_po(path)
        elif path.suffix == ".ts":
            pairs = parse_ts(path)
        else:
            continue
        if not pairs:
            continue
        daw = daw_names.get(path.stem.split("_")[0].lower(), path.name)
        for en, zh in pairs:
            key = norm_en(en)
            if key:
                table[daw][key].add(norm_zh(zh))
    return table


# --------------------------------------------------------------------------- #
# catalog
# --------------------------------------------------------------------------- #

def load_catalog() -> list[tuple[str, str, str]]:
    """(context, english, chinese) — comments and blank lines skipped."""
    rows: list[tuple[str, str, str]] = []
    for line in CATALOG.read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        parts = line.split("\t")
        if len(parts) < 3:
            continue
        ctx, en, zh = parts[0], parts[1], "\t".join(parts[2:])
        rows.append((ctx, en, zh))
    return rows


# --------------------------------------------------------------------------- #
# report
# --------------------------------------------------------------------------- #

VERDICT_CONSENSUS = "consensus"      # >= half the DAWs that have the term agree with ours
VERDICT_DIFFERS = "differs"          # a clear majority uses something else
VERDICT_SPLIT = "split"              # no majority either way
VERDICT_UNIQUE = "unique"            # term only exists in our catalog


def build_report(catalog: list[tuple[str, str, str]], corpus: dict[str, dict[str, set[str]]]) -> tuple[str, str]:
    """Return (committed summary, local detail). The summary never quotes the
    GPL-licensed corpora; the detail table does and must stay uncommitted."""
    daws = sorted(corpus)
    lines: list[str] = []
    detail: list[str] = []
    stats = collections.Counter()
    buckets: dict[str, list[str]] = collections.defaultdict(list)
    detail_buckets: dict[str, list[str]] = collections.defaultdict(list)

    lines.append("# zh-Hans cross-check report\n")
    lines.append(f"Generated by `docs/i18n/tools/cross_check.py` against the local corpus in "
                 f"`docs/i18n/sources/raw/` (gitignored; index in `sources/SOURCES.md`). "
                 f"DAWs compared: {', '.join(daws)}.\n")
    lines.append("Advisory only — nothing here changes the catalog automatically. "
                 "Wording changes go through review and land in "
                 "`crates/ui-egui/src/i18n/zh-hans.tsv` + the terminology table in "
                 "`docs/localization-zh-hans.md`.\n")
    lines.append("The reference corpora are GPL-licensed, so this committed report records only "
                 "terminology *facts* — how many reference projects agree with each key — and never "
                 "their strings. The per-key vote table with every DAW's wording is regenerated "
                 "locally at `docs/i18n/sources/raw/cross-check-detail.md` (gitignored).\n")
    detail.append("# zh-Hans cross-check detail (local, do not commit)\n")
    detail.append("Per-DAW renderings quoted verbatim from the GPL-licensed reference corpora in "
                  "`docs/i18n/sources/raw/`. This file is gitignored; only "
                  "`docs/i18n/cross-check-report.md` (verdicts and counts) is committed.\n")

    seen: set[str] = set()  # dedupe repeated English keys (same word in several contexts)
    for ctx, en, zh in catalog:
        key = norm_en(en)
        if not key or key in seen or len(key) < 3:
            continue
        seen.add(key)
        votes: dict[str, set[str]] = {}
        for daw in daws:
            hits = corpus[daw].get(key)
            if hits:
                votes[daw] = hits
        if not votes:
            stats["no-data"] += 1
            continue
        total = len(votes)
        ours = zh_compare(zh)
        agree = sum(1 for v in votes.values() if ours in {zh_compare(t) for t in v})
        # best-scoring alternative translation
        alt_counts: collections.Counter[str] = collections.Counter()
        for v in votes.values():
            for t in v:
                if zh_compare(t) != ours:
                    alt_counts[norm_zh(t)] += 1
        alt, alt_n = (alt_counts.most_common(1)[0] if alt_counts else ("", 0))
        if agree == 0:
            verdict = VERDICT_DIFFERS if alt_n * 2 > total else VERDICT_SPLIT
        elif agree * 2 > total:
            verdict = VERDICT_CONSENSUS
        else:
            verdict = VERDICT_SPLIT if alt_n >= agree else VERDICT_CONSENSUS
        stats[verdict] += 1
        per_daw = " · ".join(
            f"{daw}: {'/'.join(sorted(v))}" for daw, v in votes.items()
        )
        en_cell = en.replace("|", "\\|")
        zh_cell = zh.replace("|", "\\|")
        buckets[verdict].append(f"| {en_cell} | {zh_cell} | {agree}/{total} | {verdict} |")
        detail_buckets[verdict].append(f"| {en_cell} | {zh_cell} | {per_daw} | {agree}/{total} | {verdict} |")

    lines.append("\n## Summary\n")
    lines.append("| verdict | count |")
    lines.append("|---|---:|")
    for verdict in (VERDICT_CONSENSUS, VERDICT_DIFFERS, VERDICT_SPLIT, "no-data"):
        lines.append(f"| {verdict} | {stats[verdict]} |")
    lines.append(f"\nCatalog keys compared: {len(seen)} (deduplicated, length ≥ 3).\n")

    titles = {
        VERDICT_DIFFERS: "## Majority differs from our wording (review first)",
        VERDICT_SPLIT: "## No clear majority (informational)",
        VERDICT_CONSENSUS: "## Our wording matches the majority (no action)",
    }
    for verdict in (VERDICT_DIFFERS, VERDICT_SPLIT, VERDICT_CONSENSUS):
        lines.append(f"\n{titles[verdict]}\n")
        lines.append("| English | ours | agree | verdict |")
        lines.append("|---|---|---|---|")
        rows = buckets[verdict]
        lines.extend(rows if rows else ["| — | — | — | — |"])
        detail.append(f"\n{titles[verdict]}\n")
        detail.append("| English | ours | reference votes | agree | verdict |")
        detail.append("|---|---|---|---|---|")
        detail_rows = detail_buckets[verdict]
        detail.extend(detail_rows if detail_rows else ["| — | — | — | — | — |"])

    return "\n".join(lines) + "\n", "\n".join(detail) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", type=Path, default=DEFAULT_CORPUS)
    ap.add_argument("--out", type=Path, default=DEFAULT_OUT)
    ap.add_argument("--detail", type=Path, default=DEFAULT_DETAIL)
    args = ap.parse_args()

    catalog = load_catalog()
    corpus = load_corpus(args.corpus)
    if not corpus:
        print("corpus is empty — download sources first (see docs/i18n/sources/SOURCES.md)",
              file=sys.stderr)
        return 1
    report, detail = build_report(catalog, corpus)
    args.out.write_text(report, encoding="utf-8")
    n = sum(1 for _ in args.out.open())
    print(f"wrote {args.out} ({n} lines)")
    args.detail.write_text(detail, encoding="utf-8")
    print(f"wrote {args.detail} (local, gitignored)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
