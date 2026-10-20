# i18n knowledge base

Reference material for translating SoundCraft's UI accurately. This directory is
**documentation and tooling only** — the shipped catalog stays
`crates/ui-egui/src/i18n/zh-hans.tsv`, and the shipped terminology decisions stay
in [`../localization-zh-hans.md`](../localization-zh-hans.md).

## Why this exists

The zh-Hans catalog was written from the meaning of the English source strings,
without consulting any proprietary translation memory. That is the right
clean-room posture, but it also means some wording was chosen blind. This
knowledge base grounds future wording decisions in what the open-source DAW
ecosystem actually does, so a term like *automation latch* or *pre-fader* is
translated the way working Chinese-speaking musicians expect, not by literal
composition.

## Layout

| Path | Role | Committed |
|---|---|---|
| `sources/SOURCES.md` | index of reference corpora: origin, version, license, extraction route | ✅ |
| `sources/raw/` | the downloaded `.po`/`.ts` files themselves (reference data, GPL-licensed) | ❌ gitignored |
| `tools/cross_check.py` | compares our catalog against the corpora, writes the report below | ✅ |
| `cross-check-report.md` | generated per-term comparison; advisory | ✅ |
| `glossary.md` | curated term-by-term conclusions: what other DAWs do, what we chose, and the resolved decisions | ✅ |

## Method

1. **Corpus** — Chinese UI translations of six open-source DAWs (Ardour,
   Audacity, LMMS, Mixxx, Hydrogen, MuseScore) are downloaded into
   `sources/raw/` and never committed. `sources/SOURCES.md` records where each
   file came from; re-run the curl/unzip lines there to refresh.
2. **Cross-check** — `python3 docs/i18n/tools/cross_check.py` folds every
   English key (punctuation, case, trailing `…`/colons are normalised away) and
   collects the Chinese renderings each DAW uses for the same key, then votes.
   Nothing in the catalog is modified.
3. **Curation** — the raw vote table is full of false positives (same English
   word in a different sense, DAW-specific bugs, older corpora). The human/agent
   pass distils the report into `glossary.md`, marking each row *keep* (our
   wording is right and deliberate), *consider* (majority disagrees for a real
   reason), or *false positive* (the match was coincidental). A *consider* row is
   applied to the catalog and the terminology table in the same pass, then
   recorded as a resolved decision in §3 of the glossary.

## Clean-room rules, restated for translations

- No proprietary translation memory: no Pro Tools, Cubase, Ableton, Logic
  language packs or manuals are consulted, extracted or copied. Feature-name
  observation of the incumbent stays within the usual bounds.
- The reference corpora are GPL-licensed projects' translation data. They live
  outside version control; `glossary.md` records terminology *facts* (which
  term which project uses) in our own words, and the shipped catalog keeps its
  original, MIT/Apache-licensed translations. A terminology fact is not a
  copied string; wholesale reuse of a corpus would be, and is not done.
- SoundCraft, AudioSuite, CLAP, VST3, MIDI, dB, kHz and file extensions keep
  their spelling.

## Refreshing

```
# see sources/SOURCES.md for the exact commands per project
python3 docs/i18n/tools/cross_check.py          # regenerate the report
```

When the English catalog grows, re-run the cross-check before translating new
strings: a term the ecosystem already agrees on should not be reinvented, and a
term it disagrees on should be decided consciously and recorded in
`glossary.md` plus the terminology table of `docs/localization-zh-hans.md`.
