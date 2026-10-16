#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
SoundCraft VO cut pipeline (Windows):

  Whisper (faster-whisper) → cut plan (GigaChat / local OpenAI-compat / offline) → soundcraft-cli cuts

Reuses scripts/reaper/vo_dedupe_last.py for ASR + deterministic dedupe.
Default targets: input/nachitka.wav → sessions/nachitka/nachitka.scraft
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
REAPER = Path(__file__).resolve().parent.parent / "reaper"
if str(REAPER) not in sys.path:
    sys.path.insert(0, str(REAPER))

from vo_dedupe_last import (  # noqa: E402
    load_asr_cache,
    load_segments_file,
    probe_wav_duration,
    resolve_device,
    save_asr_cache,
    transcribe,
)

from apply_cuts import (  # noqa: E402
    apply_cut_plan,
    probe_control_port,
    probe_sample_rate,
)
from llm_cut_plan import build_cut_plan, gigachat_available  # noqa: E402

DEFAULT_WAV = ROOT / "input" / "nachitka.wav"
DEFAULT_SESSION = ROOT / "sessions" / "nachitka" / "nachitka.scraft"
DEFAULT_CLI = (
    ROOT
    / "portable"
    / "soundcraft-0.7.0-windows-x64-portable"
    / "soundcraft-cli.exe"
)
DEFAULT_WORK = ROOT / "work"
DEFAULT_ASR_CACHE_SEGS = ROOT / "work" / "asr_segments.json"


def newest_session_wav(session: Path) -> Path | None:
    """Pick the newest .wav under `<session>/Audio Files` (or next to the .scraft)."""
    roots = [
        session.parent / "Audio Files",
        session.parent / "AudioFiles",
        session.parent,
    ]
    candidates: list[Path] = []
    for root in roots:
        if not root.is_dir():
            continue
        try:
            for p in root.iterdir():
                if p.is_file() and p.suffix.lower() == ".wav":
                    candidates.append(p)
        except OSError:
            continue
    if not candidates:
        return None
    candidates.sort(key=lambda p: p.stat().st_mtime, reverse=True)
    return candidates[0]


def resolve_wav_arg(wav: Path, session: Path) -> Path:
    """If wav is missing/default and the session has Audio Files, use the newest there."""
    if wav.is_file() and wav.resolve() != DEFAULT_WAV.resolve():
        return wav
    found = newest_session_wav(session)
    if found is not None:
        return found
    return wav


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="Whisper → LLM/fallback cut plan → SoundCraft VO_keeps cuts"
    )
    p.add_argument(
        "wav",
        nargs="?",
        type=Path,
        default=DEFAULT_WAV,
        help=f"VO wav (default: {DEFAULT_WAV})",
    )
    p.add_argument(
        "--session",
        type=Path,
        default=DEFAULT_SESSION,
        help="Target .scraft session",
    )
    p.add_argument(
        "--cli",
        type=Path,
        default=DEFAULT_CLI,
        help="soundcraft-cli.exe path",
    )
    p.add_argument(
        "--work",
        type=Path,
        default=DEFAULT_WORK,
        help="Output dir for plan + CLI scripts",
    )
    p.add_argument(
        "--segments-json",
        type=Path,
        default=None,
        help="Skip Whisper; use existing ASR JSON (e.g. work/asr_segments.json)",
    )
    p.add_argument(
        "--prefer-cached-asr",
        action="store_true",
        help=f"If {DEFAULT_ASR_CACHE_SEGS.name} exists, use it instead of Whisper",
    )
    p.add_argument("--model", default="base", help="faster-whisper model")
    p.add_argument(
        "--device",
        default="auto",
        choices=("auto", "cpu", "cuda"),
        help="ASR device",
    )
    p.add_argument("--language", default="ru", help="ASR language (empty = auto)")
    p.add_argument("--no-cache", action="store_true", help="Ignore Whisper disk cache")
    p.add_argument(
        "--llm",
        default="auto",
        choices=(
            "auto",
            "fallback",
            "gigachat",
            "openai",
            "ollama",
            "lmstudio",
            "local",
        ),
        help=(
            "Cut-plan backend: auto=GigaChat if env else fallback; "
            "ollama/openai/lmstudio/local=OpenAI-compatible HTTP; "
            "fallback=deterministic keep-last"
        ),
    )
    p.add_argument(
        "--llm-base-url",
        default=None,
        help=(
            "OpenAI-compatible base URL (default: Ollama "
            "http://127.0.0.1:11434/v1 or LM Studio :1234/v1)"
        ),
    )
    p.add_argument(
        "--llm-model",
        default=None,
        help="Local LLM model id (default: qwen2.5:7b or env LOCAL_LLM_MODEL)",
    )
    p.add_argument("--method", default="prefix", choices=("prefix", "exact", "similarity"))
    p.add_argument("--merge-gap", type=float, default=0.0)
    p.add_argument(
        "--apply",
        action="store_true",
        help="Apply plan via soundcraft-cli (writes session)",
    )
    p.add_argument(
        "--dry-run",
        action="store_true",
        help="Only write cut plan JSON (default if --apply not set)",
    )
    p.add_argument(
        "--session-mode",
        default="rebuild",
        choices=("rebuild", "open"),
        help="rebuild = session.new+save_as; open = session.open existing",
    )
    p.add_argument(
        "--control-port",
        type=int,
        default=None,
        help="If set, try soundcraft-cli app --port first (needs GUI --control)",
    )
    p.add_argument(
        "--probe-control",
        action="store_true",
        help="Scan common ports for a live control channel and print result",
    )
    p.add_argument(
        "--plan-out",
        type=Path,
        default=None,
        help="Cut plan JSON path (default: work/vo_cut_plan.json)",
    )
    p.add_argument("--fade", type=float, default=0.015, help="Keep-edge fade seconds")
    return p.parse_args(argv)


def resolve_asr(args: argparse.Namespace) -> tuple[dict[str, Any], str]:
    """Return (asr_dict, mode_label)."""
    if args.segments_json is not None:
        return load_segments_file(args.segments_json), "segments_json"

    if args.prefer_cached_asr and DEFAULT_ASR_CACHE_SEGS.is_file():
        return load_segments_file(DEFAULT_ASR_CACHE_SEGS), "work_asr_segments"

    wav: Path = args.wav
    if not wav.is_file():
        # Last resort: project ASR cache without wav
        if DEFAULT_ASR_CACHE_SEGS.is_file():
            return load_segments_file(DEFAULT_ASR_CACHE_SEGS), "work_asr_segments_no_wav"
        raise FileNotFoundError(f"WAV not found: {wav}")

    if not args.no_cache:
        cached = load_asr_cache(wav, None, args.model)
        if cached is not None:
            dur = probe_wav_duration(wav)
            if dur:
                cached["duration"] = dur
            return cached, "whisper_cache"

    device = resolve_device(args.device)
    lang = args.language.strip() or None
    asr = transcribe(
        wav,
        model_name=args.model,
        device=device,
        language=lang,
        vad_filter=True,
    )
    dur = probe_wav_duration(wav)
    if dur:
        asr["duration"] = dur
    if not args.no_cache:
        save_asr_cache(wav, None, args.model, device, asr)
    return asr, "transcribe"


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    work: Path = args.work
    work.mkdir(parents=True, exist_ok=True)
    plan_out = args.plan_out or (work / "vo_cut_plan.json")
    args.wav = resolve_wav_arg(args.wav, args.session)
    print(f"WAV: {args.wav}")

    if args.probe_control:
        if not args.cli.is_file():
            print(f"error: CLI not found: {args.cli}", file=sys.stderr)
            return 2
        port = probe_control_port(args.cli)
        if port is None:
            print("control channel: not found (GUI needs --control PORT)")
        else:
            print(f"control channel: 127.0.0.1:{port}")
        if args.control_port is None and port is not None:
            args.control_port = port

    t0 = time.perf_counter()
    timing: dict[str, Any] = {}

    print(
        f"ASR... (llm={args.llm}, gigachat_creds="
        f"{'yes' if gigachat_available() else 'no'})"
    )
    try:
        asr, asr_mode = resolve_asr(args)
    except FileNotFoundError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    except Exception as exc:
        print(f"error: ASR failed: {exc}", file=sys.stderr)
        return 1

    segs = asr.get("segments") or []
    duration = float(asr.get("duration") or 0.0)
    if duration <= 0 and segs:
        duration = float(segs[-1].get("end") or 0.0)
    timing["asr"] = {
        "mode": asr_mode,
        "n_segments": len(segs),
        "duration": duration,
        "seconds": round(time.perf_counter() - t0, 3),
    }
    print(f"  mode={asr_mode} segments={len(segs)} duration={duration:.3f}s")

    # Mirror segments into work for reuse
    asr_mirror = work / "vo_cut_asr_segments.json"
    asr_mirror.write_text(
        json.dumps(
            {"language": asr.get("language"), "duration": duration, "segments": segs},
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )

    t1 = time.perf_counter()
    plan = build_cut_plan(
        segs,
        duration=duration,
        backend=args.llm,
        method=args.method,
        merge_gap=args.merge_gap,
        openai_base_url=args.llm_base_url,
        openai_model=args.llm_model,
    )
    if args.wav.is_file():
        plan["sample_rate"] = probe_sample_rate(args.wav)
    else:
        plan["sample_rate"] = 48000
    plan["input"] = str(args.wav)
    plan["session"] = str(args.session)
    plan["asr_mode"] = asr_mode
    timing["plan"] = {
        "backend": plan.get("llm_backend"),
        "n_keeps": len(plan.get("keeps") or []),
        "n_drops": len(plan.get("drops") or []),
        "n_remove": len(plan.get("remove_ranges") or []),
        "seconds": round(time.perf_counter() - t1, 3),
    }
    plan_out.write_text(
        json.dumps(plan, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    print(
        f"Plan -> {plan_out}  backend={plan.get('llm_backend')} "
        f"keeps={timing['plan']['n_keeps']} drops={timing['plan']['n_drops']} "
        f"remove_ranges={timing['plan']['n_remove']}"
    )

    do_apply = args.apply and not args.dry_run
    if not do_apply:
        timing["total_s"] = round(time.perf_counter() - t0, 3)
        (work / "vo_cut_timing.json").write_text(
            json.dumps(timing, ensure_ascii=False, indent=2), encoding="utf-8"
        )
        print("Dry-run only (pass --apply to cut the SoundCraft session).")
        return 0

    if not args.cli.is_file():
        print(f"error: soundcraft-cli not found: {args.cli}", file=sys.stderr)
        return 2
    if not args.wav.is_file():
        print(f"error: WAV required to apply cuts: {args.wav}", file=sys.stderr)
        return 2

    control = args.control_port
    if control is None and args.probe_control:
        control = probe_control_port(args.cli)

    args.session.parent.mkdir(parents=True, exist_ok=True)
    # Backup existing session before rewrite
    if args.session.is_file() and args.session_mode == "rebuild":
        bak_dir = args.session.parent / "Backups"
        bak_dir.mkdir(parents=True, exist_ok=True)
        stamp = time.strftime("%Y%m%d_%H%M%S")
        bak = bak_dir / f"{args.session.stem}_pre_vo_cut_{stamp}.scraft"
        try:
            bak.write_bytes(args.session.read_bytes())
            print(f"Backup -> {bak}")
        except OSError as exc:
            print(f"warning: could not backup session: {exc}", file=sys.stderr)

    t2 = time.perf_counter()
    print(f"Applying via {args.cli.name} (session_mode={args.session_mode})...")
    try:
        meta = apply_cut_plan(
            plan,
            wav=args.wav,
            session=args.session,
            cli=args.cli,
            work_dir=work,
            session_mode=args.session_mode,
            fade_sec=args.fade,
            control_port=control,
        )
    except Exception as exc:
        print(f"error: apply failed: {exc}", file=sys.stderr)
        return 1

    timing["apply"] = {
        "seconds": round(time.perf_counter() - t2, 3),
        **{k: meta.get(k) for k in ("n_keeps", "n_remove", "clip_ids", "pass1_cmds", "pass2_cmds")},
    }
    timing["total_s"] = round(time.perf_counter() - t0, 3)
    (work / "vo_cut_timing.json").write_text(
        json.dumps(timing, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    print(f"Done. Session: {args.session}")
    print("Open in GUI (reload if already open):")
    gui = args.cli.with_name("soundcraft.exe")
    print(f'  "{gui}" "{args.session}"')
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
