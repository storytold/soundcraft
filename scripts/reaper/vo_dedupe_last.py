#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
VO dedupe (standalone): local ASR → keep-last on shared leading words / exacts → KEEP survivors.

Default rule (prefix): if two phrases share a leading word prefix (≥2 words, e.g.
«ответы пациентов») — or are exact normalized duplicates — keep only the
chronologically last variant; drop earlier ones. No fuzzy bag-of-words similarity.

Opt-in: ``--method exact`` (identical phrases only) or ``--method similarity`` (legacy fuzzy).

Two-track scheme (same idea as SoundCraft VO_source / VO_keeps):
  - source stays intact on track 1
  - keeps sit at original timeline positions with short fades; gaps are silence / cut away

Minimal deps: faster-whisper (+ its stack). Optional render uses stdlib ``wave`` for PCM WAV,
or ffmpeg if present for other formats.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import struct
import subprocess
import sys
import time
import traceback
import wave
from dataclasses import dataclass
from difflib import SequenceMatcher
from pathlib import Path
from typing import Any, Sequence


DEFAULT_MODEL = "base"
DEFAULT_FADE_SEC = 0.015
DEFAULT_SIM = 0.72
DEFAULT_MIN_DUR = 0.35
# ≥2 leading words («ответы пациентов») is enough to treat as a retake restart.
DEFAULT_MIN_PREFIX_WORDS = 2
DEFAULT_MIN_PREFIX_CHARS = 12
DEFAULT_METHOD = "prefix"
# Default 0: Whisper VAD segments are already phrase-sized; gluing adjacent
# zero-gap lines would merge *different* phrases.
DEFAULT_MERGE_GAP = 0.0
CACHE_SCHEMA = 1


# ---------------------------------------------------------------------------
# Text / dedupe
# ---------------------------------------------------------------------------


def normalize_text(text: str) -> str:
    t = (text or "").lower().replace("ё", "е")
    t = re.sub(r"[^\w\s]+", " ", t, flags=re.UNICODE)
    t = re.sub(r"\s+", " ", t).strip()
    return t


def tokens(text: str) -> list[str]:
    return [tok for tok in normalize_text(text).split() if tok]


def token_set(text: str) -> set[str]:
    return set(tokens(text))


def common_prefix_word_count(a: Sequence[str], b: Sequence[str]) -> int:
    n = 0
    for x, y in zip(a, b):
        if x != y:
            break
        n += 1
    return n


def similarity(a: str, b: str) -> float:
    """Blend SequenceMatcher ratio with Jaccard on tokens (legacy clustering only)."""
    na, nb = normalize_text(a), normalize_text(b)
    if not na or not nb:
        return 0.0
    seq = SequenceMatcher(None, na, nb).ratio()
    ta, tb = token_set(na), token_set(nb)
    if not ta or not tb:
        jacc = 0.0
    else:
        inter = len(ta & tb)
        union = len(ta | tb)
        jacc = inter / union if union else 0.0
    # Containment helps when one take is a truncated retake of the other.
    shorter, longer = (ta, tb) if len(ta) <= len(tb) else (tb, ta)
    contain = (len(shorter & longer) / len(shorter)) if shorter else 0.0
    return max(seq, 0.55 * seq + 0.45 * jacc, 0.5 * seq + 0.5 * contain)


@dataclass
class Segment:
    start: float
    end: float
    text: str
    index: int = 0

    @property
    def duration(self) -> float:
        return max(0.0, self.end - self.start)

    @property
    def norm(self) -> str:
        return normalize_text(self.text)


def merge_close_segments(
    segs: Sequence[Segment], gap: float = DEFAULT_MERGE_GAP
) -> list[Segment]:
    """Glue adjacent ASR crumbs into phrase-sized units when the gap is small."""
    if not segs:
        return []
    ordered = sorted(segs, key=lambda s: s.start)
    out: list[Segment] = [
        Segment(ordered[0].start, ordered[0].end, ordered[0].text, 0)
    ]
    for s in ordered[1:]:
        prev = out[-1]
        # gap<=0 disables merging (adjacent VAD lines often touch at exactly 0).
        if gap > 0 and (s.start - prev.end) <= gap:
            joined = (prev.text + " " + s.text).strip()
            out[-1] = Segment(prev.start, max(prev.end, s.end), joined, prev.index)
        else:
            out.append(Segment(s.start, s.end, s.text, len(out)))
    for i, s in enumerate(out):
        s.index = i
    return out


def is_noise_segment(seg: Segment, min_duration: float = DEFAULT_MIN_DUR) -> bool:
    if not seg.norm or seg.duration < 0.08:
        return True
    ntok = len(tokens(seg.norm))
    # Filler crumbs («эм», «а») — not phrase takes.
    if ntok <= 2 and seg.duration < min_duration:
        return True
    return False


def substantial_prefix_match(
    earlier_toks: Sequence[str],
    later_toks: Sequence[str],
    *,
    min_words: int,
    min_chars: int,
) -> tuple[bool, int, str]:
    """
    Return (match, common_words, kind) when later restarts / supersedes earlier.

    kinds:
      - exact: same normalized token sequence
      - prefix: earlier tokens are a full prefix of later
      - shared: shared leading words long enough (false start that diverged)

    Shared-then-diverge requires ``min_words`` (avoids 2 long-token false hits).
    Char threshold only helps the incomplete→full prefix case when word count
    is slightly under the word floor.
    """
    if not earlier_toks or not later_toks:
        return False, 0, ""
    if list(earlier_toks) == list(later_toks):
        return True, len(earlier_toks), "exact"

    cpl = common_prefix_word_count(earlier_toks, later_toks)
    if cpl <= 0:
        return False, 0, ""

    shared = " ".join(earlier_toks[:cpl])
    shared_chars = len(shared)

    # Classic incomplete → full: earlier is entirely a prefix of later.
    if cpl == len(earlier_toks) and len(earlier_toks) < len(later_toks):
        if cpl >= min_words or (
            cpl >= max(2, min_words - 1) and shared_chars >= min_chars
        ):
            return True, cpl, "prefix"

    # Same start, then diverge (aborted wrong continuation).
    if cpl >= min_words and cpl < max(len(earlier_toks), len(later_toks)):
        return True, cpl, "shared"

    return False, cpl, ""


def later_supersedes_earlier(
    earlier: Segment,
    later: Segment,
    *,
    min_words: int = DEFAULT_MIN_PREFIX_WORDS,
    min_chars: int = DEFAULT_MIN_PREFIX_CHARS,
) -> tuple[bool, str]:
    """True if ``later`` makes ``earlier`` an obsolete false start / duplicate.

    Default ``--method prefix``: exact duplicates or shared leading words.
    """
    if later.start < earlier.start - 1e-9:
        return False, ""
    if earlier.norm == later.norm and earlier.norm:
        return True, "exact duplicate; keep later"
    ea, la = tokens(earlier.norm), tokens(later.norm)
    ok, cpl, kind = substantial_prefix_match(
        ea, la, min_words=min_words, min_chars=min_chars
    )
    if not ok:
        return False, ""
    if kind == "exact":
        return True, "exact duplicate; keep later"
    if kind == "prefix":
        return (
            True,
            f"earlier is prefix of later ({cpl} words); false start",
        )
    return (
        True,
        f"shared start of {cpl} words with later restart; false start",
    )


def dedupe_exact_repeats(
    segs: Sequence[Segment],
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """
    Default: drop earlier segments whose normalized text exactly matches a later one.
    Keep the chronologically last occurrence of each unique phrase. Touch nothing else.
    """
    ordered = sorted(segs, key=lambda s: (s.start, s.end, s.index))
    by_norm: dict[str, list[int]] = {}
    for i, seg in enumerate(ordered):
        if not seg.norm:
            continue
        by_norm.setdefault(seg.norm, []).append(i)

    drop_reason: dict[int, str] = {}
    drop_by: dict[int, int] = {}
    for idxs in by_norm.values():
        if len(idxs) < 2:
            continue
        keep_i = idxs[-1]
        for i in idxs[:-1]:
            drop_reason[i] = (
                f"exact duplicate; keep@{ordered[keep_i].start:.3f}"
                f"-{ordered[keep_i].end:.3f}"
            )
            drop_by[i] = keep_i

    keeps: list[dict[str, Any]] = []
    drops: list[dict[str, Any]] = []
    keep_n = 0
    for i, seg in enumerate(ordered):
        if i in drop_reason:
            ref = drop_by.get(i)
            cid = f"drop_{i:03d}"
            if ref is not None:
                cid = f"r{i:03d}_by_{ref:03d}"
            drops.append(_piece(seg, "DROP", cid, drop_reason[i]))
            continue
        keep_n += 1
        cid = f"k{keep_n:03d}"
        keeps.append(_piece(seg, "KEEP", cid, "unique / last occurrence"))

    keeps.sort(key=lambda x: x["start"])
    drops.sort(key=lambda x: x["start"])
    return keeps, drops


def dedupe_prefix_restarts(
    segs: Sequence[Segment],
    *,
    min_prefix_words: int = DEFAULT_MIN_PREFIX_WORDS,
    min_prefix_chars: int = DEFAULT_MIN_PREFIX_CHARS,
    min_duration: float = DEFAULT_MIN_DUR,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """
    Drop earlier false starts / exact retakes when a later segment shares a
    substantial text prefix (or is an exact normalized duplicate).

    Chained restarts (A abort → B abort → C full) drop A and B; only C keeps.
    """
    ordered = sorted(segs, key=lambda s: (s.start, s.end, s.index))
    n = len(ordered)
    drop_reason: dict[int, str] = {}
    drop_by: dict[int, int] = {}

    for i, seg in enumerate(ordered):
        if is_noise_segment(seg, min_duration):
            drop_reason[i] = "noise/empty"
            continue

    for i in range(n):
        if i in drop_reason and drop_reason[i] == "noise/empty":
            continue
        for j in range(i + 1, n):
            if j in drop_reason and drop_reason[j] == "noise/empty":
                continue
            earlier, later = ordered[i], ordered[j]
            if is_noise_segment(earlier, min_duration):
                break
            if is_noise_segment(later, min_duration):
                continue
            ok, reason = later_supersedes_earlier(
                earlier,
                later,
                min_words=min_prefix_words,
                min_chars=min_prefix_chars,
            )
            if ok:
                # Keep scanning so the chronologically last restart wins.
                drop_reason[i] = reason
                drop_by[i] = j

    # Exact-equal groups: keep only the chronologically last (covers cases where
    # pairwise loop already marked earlier ones, and cleans equal chains).
    by_norm: dict[str, list[int]] = {}
    for i, seg in enumerate(ordered):
        if not seg.norm or is_noise_segment(seg, min_duration):
            continue
        by_norm.setdefault(seg.norm, []).append(i)
    for idxs in by_norm.values():
        if len(idxs) < 2:
            continue
        keep_i = idxs[-1]
        for i in idxs[:-1]:
            drop_reason[i] = (
                f"exact duplicate; keep@{ordered[keep_i].start:.3f}"
                f"-{ordered[keep_i].end:.3f}"
            )
            drop_by[i] = keep_i

    keeps: list[dict[str, Any]] = []
    drops: list[dict[str, Any]] = []
    keep_n = 0
    for i, seg in enumerate(ordered):
        if i in drop_reason:
            ref = drop_by.get(i)
            cid = f"drop_{i:03d}"
            if ref is not None:
                cid = f"r{i:03d}_by_{ref:03d}"
            drops.append(_piece(seg, "DROP", cid, drop_reason[i]))
            continue
        keep_n += 1
        cid = f"k{keep_n:03d}"
        keeps.append(
            _piece(seg, "KEEP", cid, "no later restart / unique phrase")
        )

    keeps.sort(key=lambda x: x["start"])
    drops.sort(key=lambda x: x["start"])
    return keeps, drops


def cluster_keep_last(
    segs: Sequence[Segment],
    *,
    similarity_threshold: float = DEFAULT_SIM,
    min_duration: float = DEFAULT_MIN_DUR,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """
    Legacy: greedy similarity clustering; KEEP = last substantial member.
    Opt-in via ``--method similarity``.
    """
    ordered = sorted(segs, key=lambda s: (s.start, s.end))
    clusters: list[dict[str, Any]] = []

    for seg in ordered:
        if is_noise_segment(seg, min_duration):
            clusters.append(
                {
                    "id": f"noise_{seg.index}",
                    "members": [seg],
                    "rep": seg.norm,
                    "always_drop": True,
                }
            )
            continue
        placed = False
        for cl in clusters:
            if cl.get("always_drop"):
                continue
            if similarity(seg.text, cl["rep"]) >= similarity_threshold:
                cl["members"].append(seg)
                if len(seg.norm) > len(cl["rep"]):
                    cl["rep"] = seg.norm
                placed = True
                break
        if not placed:
            clusters.append(
                {
                    "id": f"c{len(clusters):03d}",
                    "members": [seg],
                    "rep": seg.norm,
                    "always_drop": False,
                }
            )

    keeps: list[dict[str, Any]] = []
    drops: list[dict[str, Any]] = []

    for cl in clusters:
        members: list[Segment] = list(cl["members"])
        members.sort(key=lambda s: s.start)
        if cl.get("always_drop"):
            for m in members:
                drops.append(_piece(m, "DROP", cl["id"], "noise/empty"))
            continue
        substantial = [m for m in members if m.duration >= min_duration]
        chosen_list = substantial if substantial else members
        chosen = chosen_list[-1]
        for m in members:
            if m is chosen or (
                abs(m.start - chosen.start) < 1e-6
                and abs(m.end - chosen.end) < 1e-6
                and m.text == chosen.text
            ):
                continue
            drops.append(
                _piece(
                    m,
                    "DROP",
                    cl["id"],
                    f"earlier/tiny duplicate; keep@{chosen.start:.3f}-{chosen.end:.3f}",
                )
            )
        keeps.append(
            _piece(
                chosen,
                "KEEP",
                cl["id"],
                f"LAST of cluster ({len(members)} takes)",
            )
        )

    keeps.sort(key=lambda x: x["start"])
    drops.sort(key=lambda x: x["start"])
    return keeps, drops


def decide_keeps_drops(
    segs: Sequence[Segment],
    *,
    method: str = DEFAULT_METHOD,
    min_prefix_words: int = DEFAULT_MIN_PREFIX_WORDS,
    min_prefix_chars: int = DEFAULT_MIN_PREFIX_CHARS,
    similarity_threshold: float = DEFAULT_SIM,
    min_duration: float = DEFAULT_MIN_DUR,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    m = (method or DEFAULT_METHOD).lower().strip()
    if m in ("similarity", "cluster", "legacy"):
        return cluster_keep_last(
            segs,
            similarity_threshold=similarity_threshold,
            min_duration=min_duration,
        )
    if m in ("exact",):
        return dedupe_exact_repeats(segs)
    # Default: prefix / restart / anything else → leading-word keep-last.
    return dedupe_prefix_restarts(
        segs,
        min_prefix_words=min_prefix_words,
        min_prefix_chars=min_prefix_chars,
        min_duration=min_duration,
    )


def _piece(seg: Segment, decision: str, cluster: str, reason: str) -> dict[str, Any]:
    return {
        "start": round(seg.start, 3),
        "end": round(seg.end, 3),
        "text": seg.text,
        "norm": seg.norm,
        "cluster": cluster,
        "decision": decision,
        "reason": reason,
        "duration": round(seg.duration, 3),
    }


def gaps_from_keeps(
    keeps: Sequence[dict[str, Any]], total_duration: float
) -> list[dict[str, float]]:
    """Silent / DROP ranges covering [0, total) minus keep spans."""
    gaps: list[dict[str, float]] = []
    t = 0.0
    for k in sorted(keeps, key=lambda x: x["start"]):
        s, e = float(k["start"]), float(k["end"])
        if s > t + 0.01:
            gaps.append({"start": round(t, 3), "end": round(s, 3)})
        t = max(t, e)
    if total_duration > t + 0.01:
        gaps.append({"start": round(t, 3), "end": round(total_duration, 3)})
    return gaps


# ---------------------------------------------------------------------------
# Cache / ASR
# ---------------------------------------------------------------------------


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest().lower()


def cache_paths(wav: Path, cache_dir: Path | None) -> tuple[Path, Path]:
    digest_prefix = sha256_file(wav)[:16]
    if cache_dir is not None:
        cache_dir.mkdir(parents=True, exist_ok=True)
        base = cache_dir / f"{wav.stem}.{digest_prefix}"
    else:
        # Next to input: hidden-ish .cache sibling folder preferred if writable.
        sibling = wav.parent / ".cache"
        try:
            sibling.mkdir(exist_ok=True)
            base = sibling / f"{wav.stem}.{digest_prefix}"
        except OSError:
            base = wav.with_suffix("") 
            base = Path(str(base) + f".{digest_prefix}")
    return Path(str(base) + ".asr.json"), Path(str(base) + ".meta.json")


def load_asr_cache(
    wav: Path, cache_dir: Path | None, model: str
) -> dict[str, Any] | None:
    asr_path, meta_path = cache_paths(wav, cache_dir)
    if not asr_path.is_file() or not meta_path.is_file():
        return None
    try:
        meta = json.loads(meta_path.read_text(encoding="utf-8-sig"))
        asr = json.loads(asr_path.read_text(encoding="utf-8-sig"))
    except (OSError, json.JSONDecodeError):
        return None
    digest = sha256_file(wav)
    if str(meta.get("sha256", "")).lower() != digest:
        return None
    if meta.get("schema") != CACHE_SCHEMA:
        return None
    if str(meta.get("model", "")) != model:
        return None
    asr["_cache_hit"] = True
    asr["_cache_asr_path"] = str(asr_path)
    return asr


def save_asr_cache(
    wav: Path,
    cache_dir: Path | None,
    model: str,
    device: str,
    asr: dict[str, Any],
) -> tuple[Path, Path]:
    asr_path, meta_path = cache_paths(wav, cache_dir)
    digest = sha256_file(wav)
    payload = {
        "language": asr.get("language"),
        "duration": asr.get("duration"),
        "segments": asr.get("segments", []),
    }
    asr_path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    meta = {
        "schema": CACHE_SCHEMA,
        "sha256": digest,
        "model": model,
        "device": device,
        "source": str(wav.resolve()),
        "asr": str(asr_path.resolve()),
    }
    meta_path.write_text(
        json.dumps(meta, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    return asr_path, meta_path


def _cublas_loadable() -> bool:
    """Windows often reports a CUDA device while cuBLAS DLLs are missing from PATH."""
    if sys.platform != "win32":
        return True
    try:
        import ctypes

        for name in ("cublas64_12.dll", "cublas64_11.dll"):
            try:
                ctypes.WinDLL(name)
                return True
            except OSError:
                continue
    except Exception:
        return False
    return False


def cuda_usable() -> bool:
    """True when ctranslate2 sees a GPU *and* cuBLAS can actually load."""
    try:
        import ctranslate2  # type: ignore

        if ctranslate2.get_cuda_device_count() <= 0:
            return False
    except Exception:
        return False
    return _cublas_loadable()


def resolve_device(device: str) -> str:
    d = (device or "auto").lower().strip()
    if d == "cpu":
        return "cpu"
    if d == "cuda":
        if not cuda_usable():
            print(
                "warning: --device cuda requested but CUDA/cuBLAS is not usable; "
                "falling back to cpu",
                file=sys.stderr,
            )
            return "cpu"
        return "cuda"
    # auto
    if cuda_usable():
        return "cuda"
    return "cpu"


def _transcribe_once(
    wav: Path,
    *,
    model_name: str,
    device: str,
    language: str | None,
    vad_filter: bool,
) -> dict[str, Any]:
    from faster_whisper import WhisperModel

    compute = "float16" if device == "cuda" else "int8"
    model = WhisperModel(model_name, device=device, compute_type=compute)
    kwargs: dict[str, Any] = {
        "vad_filter": vad_filter,
        "word_timestamps": False,
    }
    if language:
        kwargs["language"] = language
    segments_iter, info = model.transcribe(str(wav), **kwargs)
    segs = []
    for seg in segments_iter:
        text = (seg.text or "").strip()
        if not text:
            continue
        segs.append(
            {
                "start": round(float(seg.start), 3),
                "end": round(float(seg.end), 3),
                "text": text,
            }
        )
    duration = float(getattr(info, "duration", 0.0) or 0.0)
    if duration <= 0 and segs:
        duration = float(segs[-1]["end"])
    return {
        "language": getattr(info, "language", language),
        "duration": duration,
        "segments": segs,
        "_cache_hit": False,
        "_device_used": device,
    }


def _is_cuda_runtime_error(exc: BaseException) -> bool:
    msg = str(exc).lower()
    needles = (
        "cublas",
        "cuda",
        "cudnn",
        "nvrtc",
        "cublas64",
        "cannot be loaded",
        "library cublas",
    )
    return any(n in msg for n in needles)


def transcribe(
    wav: Path,
    *,
    model_name: str,
    device: str,
    language: str | None,
    vad_filter: bool,
) -> dict[str, Any]:
    try:
        return _transcribe_once(
            wav,
            model_name=model_name,
            device=device,
            language=language,
            vad_filter=vad_filter,
        )
    except Exception as exc:
        if device != "cuda" or not _is_cuda_runtime_error(exc):
            raise
        print(
            f"warning: CUDA transcription failed ({exc}); retrying on cpu",
            file=sys.stderr,
        )
        return _transcribe_once(
            wav,
            model_name=model_name,
            device="cpu",
            language=language,
            vad_filter=vad_filter,
        )


# ---------------------------------------------------------------------------
# Audio render (keeps @ original positions)
# ---------------------------------------------------------------------------


def probe_wav_duration(path: Path) -> float | None:
    try:
        with wave.open(str(path), "rb") as w:
            rate = w.getframerate()
            n = w.getnframes()
            if rate <= 0:
                return None
            return n / float(rate)
    except (wave.Error, OSError):
        return None


def _fade_gain(i: int, n: int, side: str) -> float:
    if n <= 1:
        return 1.0
    # Equal-power-ish: sin curve
    t = i / (n - 1)
    if side == "in":
        return math.sin(0.5 * math.pi * t)
    return math.sin(0.5 * math.pi * (1.0 - t))


def render_keeps_wav(
    src: Path,
    dst: Path,
    keeps: Sequence[dict[str, Any]],
    *,
    fade_sec: float = DEFAULT_FADE_SEC,
) -> None:
    """Write PCM WAV: silence everywhere except keep regions (with short fades)."""
    with wave.open(str(src), "rb") as w:
        nch = w.getnchannels()
        sw = w.getsampwidth()
        rate = w.getframerate()
        nframes = w.getnframes()
        raw = w.readframes(nframes)

    if sw not in (2, 3, 4):
        raise SystemExit(
            f"Unsupported sample width {sw * 8}-bit; use 16/24/32-bit PCM WAV "
            "or pass --no-render and edit in Reaper."
        )

    total_samples = nframes
    fade_n = max(1, int(round(fade_sec * rate)))
    # Work in float32 interleaved
    fmt = {2: "h", 3: None, 4: "i"}[sw]
    max_amp = float(1 << (8 * sw - 1))

    if sw == 3:
        samples = _unpack_24(raw, nch * total_samples)
    else:
        count = nch * total_samples
        samples = list(struct.unpack("<" + fmt * count, raw))

    out = [0.0] * len(samples)
    for k in keeps:
        s0 = int(round(float(k["start"]) * rate))
        s1 = int(round(float(k["end"]) * rate))
        s0 = max(0, min(total_samples, s0))
        s1 = max(0, min(total_samples, s1))
        if s1 <= s0:
            continue
        length = s1 - s0
        fin = min(fade_n, length // 2 if length > 1 else 1)
        fout = fin
        for i in range(length):
            g = 1.0
            if i < fin:
                g = _fade_gain(i, fin, "in")
            elif i >= length - fout:
                g = _fade_gain(i - (length - fout), fout, "out")
            for c in range(nch):
                idx = (s0 + i) * nch + c
                out[idx] = samples[idx] * g

    if sw == 3:
        packed = _pack_24(out)
    else:
        clipped = []
        lim = int(max_amp) - 1
        for v in out:
            iv = int(round(v))
            if iv > lim:
                iv = lim
            elif iv < -lim - 1:
                iv = -lim - 1
            clipped.append(iv)
        packed = struct.pack("<" + fmt * len(clipped), *clipped)

    dst.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(dst), "wb") as w:
        w.setnchannels(nch)
        w.setsampwidth(sw)
        w.setframerate(rate)
        w.writeframes(packed)


def _unpack_24(raw: bytes, n: int) -> list[float]:
    out: list[float] = []
    for i in range(n):
        b0 = raw[i * 3]
        b1 = raw[i * 3 + 1]
        b2 = raw[i * 3 + 2]
        val = b0 | (b1 << 8) | (b2 << 16)
        if val & 0x800000:
            val -= 1 << 24
        out.append(float(val))
    return out


def _pack_24(samples: Sequence[float]) -> bytes:
    buf = bytearray()
    lim = (1 << 23) - 1
    for v in samples:
        iv = int(round(v))
        if iv > lim:
            iv = lim
        elif iv < -lim - 1:
            iv = -lim - 1
        if iv < 0:
            iv += 1 << 24
        buf.append(iv & 0xFF)
        buf.append((iv >> 8) & 0xFF)
        buf.append((iv >> 16) & 0xFF)
    return bytes(buf)


def try_ffmpeg_to_wav(src: Path, tmp_wav: Path) -> bool:
    try:
        r = subprocess.run(
            [
                "ffmpeg",
                "-y",
                "-i",
                str(src),
                "-acodec",
                "pcm_s16le",
                str(tmp_wav),
            ],
            capture_output=True,
            text=True,
        )
        return r.returncode == 0 and tmp_wav.is_file()
    except OSError:
        return False


# ---------------------------------------------------------------------------
# SRT / report
# ---------------------------------------------------------------------------


def sec_to_srt(t: float) -> str:
    if t < 0:
        t = 0.0
    h = int(t // 3600)
    m = int((t % 3600) // 60)
    s = int(t % 60)
    ms = int(round((t - math.floor(t)) * 1000))
    if ms >= 1000:
        ms = 0
        s += 1
    return f"{h:02d}:{m:02d}:{s:02d},{ms:03d}"


def write_srt(path: Path, pieces: Sequence[dict[str, Any]]) -> None:
    lines: list[str] = []
    for i, p in enumerate(pieces, 1):
        lines.append(str(i))
        lines.append(f"{sec_to_srt(p['start'])} --> {sec_to_srt(p['end'])}")
        tag = p.get("decision", "")
        text = p.get("text", "")
        lines.append(f"[{tag}] {text}".strip() if tag else text)
        lines.append("")
    path.write_text("\n".join(lines), encoding="utf-8")


def build_report(
    wav: Path,
    asr: dict[str, Any],
    keeps: list[dict[str, Any]],
    drops: list[dict[str, Any]],
    *,
    model: str,
    device: str,
    method: str,
    min_prefix_words: int,
    min_prefix_chars: int,
    similarity_threshold: float,
    fade_sec: float,
    timing: dict[str, Any],
) -> dict[str, Any]:
    duration = float(asr.get("duration") or 0.0)
    m = (method or DEFAULT_METHOD).lower().strip()
    named_keeps = []
    for i, k in enumerate(keeps, 1):
        suffix = "KEEP" if m not in ("similarity", "cluster", "legacy") else "LAST"
        named_keeps.append(
            {
                **k,
                "name": f"{i:02d}_{k['cluster']}_{suffix}",
            }
        )
    if m in ("similarity", "cluster", "legacy"):
        method_id = "similarity"
        method_desc = (
            "legacy near-duplicate phrase clustering (normalize + similarity); "
            "KEEP last take"
        )
        rule_desc = (
            "For each similarity cluster keep ONLY the chronologically last "
            "segment; earlier = DROP"
        )
    elif m in ("exact",):
        method_id = "exact"
        method_desc = (
            "exact-repeat dedupe (normalize); DROP earlier identical phrases only"
        )
        rule_desc = (
            "If normalized text of an earlier segment exactly matches a later "
            "one, DROP the earlier; keep the last occurrence. Unique phrases "
            "are untouched."
        )
    else:
        method_id = "prefix"
        method_desc = (
            "prefix-restart dedupe (normalize; leading-word / exact prefix); "
            "DROP earlier false starts; KEEP last"
        )
        rule_desc = (
            "If a later segment shares a leading word prefix (≥ min_prefix_words) "
            "with an earlier one, or texts are equal, DROP the earlier; chained "
            "restarts drop all but the last. No fuzzy bag similarity."
        )
    return {
        "input": str(wav.resolve()),
        "sample_rate_hint": None,
        "duration": duration,
        "method": method_desc,
        "rule": rule_desc,
        "method_id": method_id,
        "model": model,
        "device": device,
        "min_prefix_words": min_prefix_words,
        "min_prefix_chars": min_prefix_chars,
        "similarity_threshold": similarity_threshold,
        "fade_sec": fade_sec,
        "asr_cache_hit": bool(asr.get("_cache_hit")),
        "language": asr.get("language"),
        "keeps": named_keeps,
        "drops": drops,
        "drop_gaps": gaps_from_keeps(named_keeps, duration),
        "timing": timing,
    }


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description=(
            "Local VO dedupe: ASR → keep last of shared leading-word / exact "
            "phrase repeats → KEEP survivors."
        )
    )
    p.add_argument(
        "input",
        nargs="?",
        help="Path to WAV (or other audio readable by faster-whisper)",
    )
    p.add_argument(
        "-o",
        "--output",
        type=Path,
        help="JSON report path (default: <input>.vo_dedupe.json)",
    )
    p.add_argument("--srt", type=Path, help="Optional SRT of keep+drop segments")
    p.add_argument(
        "--srt-keeps",
        type=Path,
        help="Optional SRT with KEEP segments only",
    )
    p.add_argument("--model", default=DEFAULT_MODEL, help=f"Whisper model (default {DEFAULT_MODEL})")
    p.add_argument(
        "--device",
        default="auto",
        choices=("auto", "cpu", "cuda"),
        help="Inference device",
    )
    p.add_argument("--language", default="ru", help="ASR language ('' = auto-detect)")
    p.add_argument("--no-vad", action="store_true", help="Disable VAD filter")
    p.add_argument(
        "--cache-dir",
        type=Path,
        default=None,
        help="ASR cache directory (default: <input_dir>/.cache/)",
    )
    p.add_argument("--no-cache", action="store_true", help="Ignore/write no ASR cache")
    p.add_argument(
        "--method",
        default=DEFAULT_METHOD,
        choices=("exact", "prefix", "similarity", "cluster", "legacy"),
        help=(
            "Dedupe method: prefix (default, leading-word + exact keep-last), "
            "exact (identical only), or similarity/legacy (fuzzy cluster)"
        ),
    )
    p.add_argument(
        "--min-prefix-words",
        type=int,
        default=DEFAULT_MIN_PREFIX_WORDS,
        help=(
            f"Min shared leading words for --method prefix "
            f"(default {DEFAULT_MIN_PREFIX_WORDS})"
        ),
    )
    p.add_argument(
        "--min-prefix-chars",
        type=int,
        default=DEFAULT_MIN_PREFIX_CHARS,
        help=(
            f"Alt. min shared leading chars for incomplete→full prefix "
            f"(default {DEFAULT_MIN_PREFIX_CHARS})"
        ),
    )
    p.add_argument(
        "--similarity",
        type=float,
        default=DEFAULT_SIM,
        help=(
            f"Legacy near-duplicate threshold 0..1 for --method similarity "
            f"(default {DEFAULT_SIM})"
        ),
    )
    p.add_argument(
        "--min-duration",
        type=float,
        default=DEFAULT_MIN_DUR,
        help=(
            f"Min seconds for substantial / non-noise takes "
            f"(default {DEFAULT_MIN_DUR})"
        ),
    )
    p.add_argument(
        "--merge-gap",
        type=float,
        default=DEFAULT_MERGE_GAP,
        help=f"Merge adjacent ASR segments closer than this; 0=off (default {DEFAULT_MERGE_GAP})",
    )
    p.add_argument(
        "--fade",
        type=float,
        default=DEFAULT_FADE_SEC,
        help=f"Fade in/out seconds for rendered keeps (default {DEFAULT_FADE_SEC})",
    )
    p.add_argument(
        "--render",
        type=Path,
        nargs="?",
        const=True,
        default=None,
        help="Render keeps WAV at original positions (optional path; default beside input)",
    )
    p.add_argument(
        "--segments-json",
        type=Path,
        help="Skip ASR: load segments JSON {segments:[{start,end,text},...]} or a bare list",
    )
    p.add_argument(
        "--self-test",
        action="store_true",
        help="Run built-in prefix/exact fixtures and exit",
    )
    return p.parse_args(argv)


def self_test() -> int:
    """Tiny fixtures — no ASR / no GPU."""
    # Two identical phrases → first DROP, second KEEP.
    exact = [
        Segment(0.0, 1.0, "Привет, это первая дубль фраза", 0),
        Segment(2.0, 3.0, "привет это первая дубль фраза", 1),
    ]
    keeps, drops = dedupe_exact_repeats(exact)
    assert len(keeps) == 1 and len(drops) == 1, (keeps, drops)
    assert abs(keeps[0]["start"] - 2.0) < 1e-6, keeps[0]
    assert abs(drops[0]["start"] - 0.0) < 1e-6, drops[0]

    # Different phrases → both KEEP.
    different = [
        Segment(0.0, 1.0, "Первая фраза", 0),
        Segment(2.0, 3.0, "Вторая фраза совсем другая", 1),
    ]
    keeps, drops = dedupe_exact_repeats(different)
    assert len(keeps) == 2 and len(drops) == 0, (keeps, drops)

    # Same start, different ending → NOT a drop under exact; YES under default prefix.
    prefix_case = [
        Segment(0.0, 2.5, "Он пришел в этот мир чтобы пера....", 0),
        Segment(3.0, 6.0, "Он пришел в этот мир чтобы показать всем класс", 1),
    ]
    keeps, drops = dedupe_exact_repeats(prefix_case)
    assert len(keeps) == 2 and len(drops) == 0, (keeps, drops)

    # Default decide path = prefix (leading words + exact).
    dk, dd = decide_keeps_drops(exact)
    assert len(dk) == 1 and len(dd) == 1, (dk, dd)
    dk2, dd2 = decide_keeps_drops(prefix_case)
    assert len(dk2) == 1 and len(dd2) == 1, (dk2, dd2)
    assert abs(dk2[0]["start"] - 3.0) < 1e-6, dk2[0]
    assert abs(dd2[0]["start"] - 0.0) < 1e-6, dd2[0]

    # Screenshot case: two takes starting «ответы пациентов» — keep last only.
    otvety = [
        Segment(
            67.0,
            72.0,
            "Ответы пациентов сразу поступают на медицинскую платформу.",
            0,
        ),
        Segment(
            84.0,
            91.0,
            "Ответы пациентов сразу поступают на медицинскую платформу врач видит.",
            1,
        ),
    ]
    ok_k, ok_d = decide_keeps_drops(otvety)
    assert len(ok_k) == 1 and len(ok_d) == 1, (ok_k, ok_d)
    assert abs(ok_k[0]["start"] - 84.0) < 1e-6, ok_k[0]
    assert abs(ok_d[0]["start"] - 67.0) < 1e-6, ok_d[0]
    # Exact-only would wrongly keep both (texts differ).
    ek, ed = decide_keeps_drops(otvety, method="exact")
    assert len(ek) == 2 and len(ed) == 0, (ek, ed)

    # Triple exact → keep only last.
    triple = [
        Segment(0.0, 1.0, "Привет, это первая дубль фраза", 0),
        Segment(2.0, 3.0, "привет это первая дубль фраза", 1),
        Segment(4.0, 5.0, "Привет, это первая дубль фраза!", 2),
        Segment(6.0, 7.0, "Совсем другая фраза тут", 3),
    ]
    keeps, drops = dedupe_exact_repeats(triple)
    assert len(keeps) == 2, keeps
    assert abs(keeps[0]["start"] - 4.0) < 1e-6, keeps[0]
    assert abs(keeps[1]["start"] - 6.0) < 1e-6, keeps[1]
    assert sum(1 for d in drops if d["start"] in (0.0, 2.0)) == 2

    # Short filler crumb → noise DROP; real phrase stays.
    stammer = [
        Segment(0.0, 0.25, "эм", 0),
        Segment(1.0, 3.0, "Нормальная фраза без рестарта", 1),
    ]
    keeps, drops = decide_keeps_drops(stammer)
    assert len(keeps) == 1 and abs(keeps[0]["start"] - 1.0) < 1e-6, keeps
    assert len(drops) == 1 and abs(drops[0]["start"] - 0.0) < 1e-6, drops

    # merge test
    crumbs = [
        Segment(0.0, 0.4, "один", 0),
        Segment(0.5, 0.9, "два", 1),
        Segment(2.0, 2.5, "три", 2),
    ]
    merged = merge_close_segments(crumbs, gap=0.2)
    assert len(merged) == 2
    assert "один" in merged[0].text and "два" in merged[0].text

    # Opt-in exact still callable.
    pk, pd = decide_keeps_drops(prefix_case, method="exact")
    assert len(pk) == 2 and len(pd) == 0, (pk, pd)

    # Opt-in similarity still callable.
    legacy = [
        Segment(0.0, 1.2, "Привет, это первая дубль фраза", 0),
        Segment(2.0, 3.1, "Привет это первая дубль фраза", 1),
        Segment(6.0, 7.4, "привет, это первая дубль фраза!", 2),
    ]
    lk, ld = decide_keeps_drops(legacy, method="similarity", similarity_threshold=0.7)
    assert len(lk) == 1 and abs(lk[0]["start"] - 6.0) < 1e-6, lk

    assert DEFAULT_METHOD == "prefix"
    assert DEFAULT_MIN_PREFIX_WORDS == 2

    print("self-test OK")
    print(
        json.dumps(
            {
                "exact_pair_keeps": dedupe_exact_repeats(exact)[0],
                "exact_pair_drops": dedupe_exact_repeats(exact)[1],
                "otvety_default_keeps": ok_k,
                "otvety_default_drops": ok_d,
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    return 0


def load_segments_file(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8-sig"))
    if isinstance(data, list):
        segs = data
        duration = max((float(s.get("end", 0)) for s in segs), default=0.0)
        return {"language": None, "duration": duration, "segments": segs, "_cache_hit": False}
    segs = data.get("segments") or data.get("islands") or []
    duration = float(data.get("duration") or 0.0)
    if duration <= 0 and segs:
        duration = max(float(s.get("end", 0)) for s in segs)
    return {
        "language": data.get("language"),
        "duration": duration,
        "segments": segs,
        "_cache_hit": False,
    }


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    if args.self_test:
        return self_test()

    if not args.input and not args.segments_json:
        print("error: provide input wav or --segments-json / --self-test", file=sys.stderr)
        return 2

    timing: dict[str, Any] = {}
    wall0 = time.perf_counter()

    wav: Path | None = Path(args.input) if args.input else None
    if wav is not None and not wav.is_file():
        print(f"error: file not found: {wav}", file=sys.stderr)
        return 2

    device = resolve_device(args.device)
    language = args.language if args.language else None

    # --- ASR / segments ---
    t = time.perf_counter()
    if args.segments_json:
        asr = load_segments_file(args.segments_json)
        asr_mode = "segments_json"
    else:
        assert wav is not None
        asr = None
        if not args.no_cache:
            asr = load_asr_cache(wav, args.cache_dir, args.model)
        if asr is None:
            asr_mode = "transcribe"
            asr = transcribe(
                wav,
                model_name=args.model,
                device=device,
                language=language,
                vad_filter=not args.no_vad,
            )
            # Prefer real WAV duration when available
            dur = probe_wav_duration(wav)
            if dur:
                asr["duration"] = dur
            if not args.no_cache:
                save_asr_cache(wav, args.cache_dir, args.model, device, asr)
        else:
            asr_mode = "cache_hit"
            dur = probe_wav_duration(wav)
            if dur:
                asr["duration"] = dur
    timing["asr"] = {
        "seconds": round(time.perf_counter() - t, 3),
        "mode": asr_mode,
        "n_raw_segments": len(asr.get("segments") or []),
    }

    raw_segs = []
    for i, s in enumerate(asr.get("segments") or []):
        raw_segs.append(
            Segment(
                float(s["start"]),
                float(s["end"]),
                str(s.get("text") or "").strip(),
                i,
            )
        )

    t = time.perf_counter()
    merged = merge_close_segments(raw_segs, gap=args.merge_gap)
    keeps, drops = decide_keeps_drops(
        merged,
        method=args.method,
        min_prefix_words=args.min_prefix_words,
        min_prefix_chars=args.min_prefix_chars,
        similarity_threshold=args.similarity,
        min_duration=args.min_duration,
    )
    timing["dedupe"] = {
        "seconds": round(time.perf_counter() - t, 3),
        "method": args.method,
        "n_merged": len(merged),
        "n_keeps": len(keeps),
        "n_drops": len(drops),
    }
    # Alias for older readers of the JSON.
    timing["cluster"] = timing["dedupe"]

    input_path = wav if wav is not None else Path(args.segments_json)
    out_json = args.output
    if out_json is None:
        out_json = input_path.with_suffix(input_path.suffix + ".vo_dedupe.json")
        if wav is not None:
            out_json = wav.with_suffix(".vo_dedupe.json")

    report = build_report(
        input_path if wav is None else wav,
        asr,
        keeps,
        drops,
        model=args.model,
        device=device,
        method=args.method,
        min_prefix_words=args.min_prefix_words,
        min_prefix_chars=args.min_prefix_chars,
        similarity_threshold=args.similarity,
        fade_sec=args.fade,
        timing=timing,
    )

    # --- optional render ---
    if args.render is not None:
        if wav is None:
            print("error: --render requires an audio input file", file=sys.stderr)
            return 2
        render_path: Path
        if args.render is True:
            render_path = wav.with_name(wav.stem + "_keeps.wav")
        else:
            render_path = Path(args.render)
        t = time.perf_counter()
        src_for_render = wav
        tmp: Path | None = None
        try:
            try:
                render_keeps_wav(
                    src_for_render, render_path, report["keeps"], fade_sec=args.fade
                )
            except (wave.Error, SystemExit) as exc:
                # Try ffmpeg convert once
                tmp = render_path.with_suffix(".tmp16.wav")
                if not try_ffmpeg_to_wav(wav, tmp):
                    raise SystemExit(str(exc)) from exc
                render_keeps_wav(tmp, render_path, report["keeps"], fade_sec=args.fade)
        finally:
            if tmp is not None and tmp.is_file():
                try:
                    tmp.unlink()
                except OSError:
                    pass
        timing["render"] = {
            "seconds": round(time.perf_counter() - t, 3),
            "path": str(render_path.resolve()),
        }
        report["render"] = str(render_path.resolve())
        report["timing"] = timing

    if args.srt:
        write_srt(args.srt, list(report["keeps"]) + list(report["drops"]))
        report["srt"] = str(args.srt.resolve())
    if args.srt_keeps:
        write_srt(args.srt_keeps, report["keeps"])
        report["srt_keeps"] = str(args.srt_keeps.resolve())

    timing["total_seconds"] = round(time.perf_counter() - wall0, 3)
    report["timing"] = timing

    out_json = Path(out_json)
    out_json.parent.mkdir(parents=True, exist_ok=True)
    out_json.write_text(
        json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8"
    )

    print(f"OK  keeps={len(keeps)} drops={len(drops)}  json={out_json}")
    print(
        f"    asr={asr_mode} ({timing['asr']['seconds']}s)  "
        f"dedupe={timing['dedupe']['seconds']}s method={args.method}  "
        f"total={timing['total_seconds']}s"
    )
    for k in report["keeps"]:
        print(
            f"  KEEP {k['start']:8.3f}-{k['end']:8.3f}  {k['name']}  {(k.get('text') or '')[:72]}"
        )
    return 0


def _guess_error_sidecar() -> Path | None:
    """Best-effort path for a failure log Reaper Lua can show (next to -o or input)."""
    argv = sys.argv[1:]
    out: Path | None = None
    inp: Path | None = None
    i = 0
    while i < len(argv):
        a = argv[i]
        if a in ("-o", "--output") and i + 1 < len(argv):
            out = Path(argv[i + 1])
            i += 2
            continue
        if not a.startswith("-") and inp is None:
            inp = Path(a)
        i += 1
    if out is not None:
        return out.with_suffix(out.suffix + ".err.txt")
    if inp is not None:
        return inp.with_suffix(inp.suffix + ".vo_dedupe.err.txt")
    return None


def _report_failure(exc: BaseException) -> int:
    tb = traceback.format_exc()
    # One clear line on stdout (REAPER ExecProcess often captures only stdout).
    print(f"ERROR: {type(exc).__name__}: {exc}", flush=True)
    print(tb, file=sys.stderr, flush=True)
    side = _guess_error_sidecar()
    if side is not None:
        try:
            side.parent.mkdir(parents=True, exist_ok=True)
            side.write_text(
                f"ERROR: {type(exc).__name__}: {exc}\n\n{tb}",
                encoding="utf-8",
            )
            print(f"error log: {side}", flush=True)
        except OSError:
            pass
    return 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        print("interrupted", file=sys.stderr)
        raise SystemExit(130)
    except SystemExit:
        raise
    except Exception as exc:
        raise SystemExit(_report_failure(exc)) from None
