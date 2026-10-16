# -*- coding: utf-8 -*-
"""Apply a VO cut plan to a SoundCraft session via soundcraft-cli.

Builds VO_source (muted full) + VO_keeps (excess cleared), markers, optional fades.
Same shape as work/build_sc_v2.py / work/strict_last_dedupe.py.
"""
from __future__ import annotations

import json
import re
import subprocess
import wave
from pathlib import Path
from typing import Any, Sequence


DEFAULT_SR = 48000
DEFAULT_FADE_SEC = 0.015


def probe_sample_rate(wav: Path) -> int:
    try:
        with wave.open(str(wav), "rb") as w:
            rate = int(w.getframerate())
            return rate if rate > 0 else DEFAULT_SR
    except (wave.Error, OSError):
        return DEFAULT_SR


def samp(sec: float, sr: int) -> int:
    return int(round(float(sec) * sr))


def add(lines: list[str], cmd: str, params: dict[str, Any]) -> None:
    payload = json.dumps(params, ensure_ascii=False, separators=(",", ":"))
    lines.append(f"{cmd} {payload}")


def write_script(path: Path, lines: Sequence[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def run_cli_script(cli: Path, script: Path, log_path: Path | None = None) -> str:
    proc = subprocess.run(
        [str(cli), "script", str(script)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    out = (proc.stdout or "") + (("\n" + proc.stderr) if proc.stderr else "")
    if log_path is not None:
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log_path.write_text(out, encoding="utf-8")
    if proc.returncode != 0:
        raise RuntimeError(
            f"soundcraft-cli script failed rc={proc.returncode}\n{out[-4000:]}"
        )
    return out


def try_control_execute(
    cli: Path, port: int, command: str, params: dict[str, Any]
) -> dict[str, Any] | None:
    """Best-effort call into a running SoundCraft --control PORT. Returns None on failure."""
    payload = json.dumps(
        {"command": command, "params": params},
        ensure_ascii=False,
        separators=(",", ":"),
    )
    proc = subprocess.run(
        [str(cli), "app", "--port", str(port), "engine.execute", payload],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=30,
    )
    if proc.returncode != 0:
        return None
    text = (proc.stdout or "").strip()
    if not text:
        return {"ok": True}
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return {"raw": text}


def probe_control_port(cli: Path, ports: Sequence[int] | None = None) -> int | None:
    candidates = list(ports or (7800, 7801, 7802, 7803, 9333, 8765))
    for p in candidates:
        proc = subprocess.run(
            [str(cli), "app", "--port", str(p), "engine.commands", "{}"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=3,
        )
        if proc.returncode == 0:
            return p
    return None


def build_pass1_lines(
    *,
    wav: Path,
    session: Path,
    keeps: Sequence[dict[str, Any]],
    remove_ranges: Sequence[dict[str, Any]],
    sr: int,
    session_mode: str = "rebuild",
) -> list[str]:
    """
    session_mode:
      - rebuild: session.new then save_as (clean 2-track layout)
      - open: session.open existing, rename/import VO tracks best-effort
    """
    lines: list[str] = []
    wav_s = str(wav).replace("\\", "/")
    sess_s = str(session).replace("\\", "/")

    if session_mode == "open":
        add(lines, "session.open", {"path": sess_s})
    else:
        add(
            lines,
            "session.new",
            {
                "name": session.stem,
                "sample_rate": sr,
                "bit_depth": 24,
                "template": "blank",
            },
        )

    add(lines, "edit.mode", {"mode": "slip"})

    if session_mode != "open":
        add(lines, "track.new", {"count": 1, "format": "mono", "name": "VO_source"})
        add(
            lines,
            "file.import_audio",
            {"path": wav_s, "track": "VO_source", "at": "0", "new_tracks": False},
        )
        add(lines, "mix.mute", {"tracks": ["VO_source"], "value": True})
        add(lines, "track.new", {"count": 1, "format": "mono", "name": "VO_keeps"})
        add(
            lines,
            "file.import_audio",
            {"path": wav_s, "track": "VO_keeps", "at": "0", "new_tracks": False},
        )
    else:
        # Open existing: re-import onto VO_keeps if present; else create tracks.
        add(lines, "track.new", {"count": 1, "format": "mono", "name": "VO_keeps"})
        add(
            lines,
            "file.import_audio",
            {"path": wav_s, "track": "VO_keeps", "at": "0", "new_tracks": False},
        )

    for i, r in enumerate(remove_ranges):
        s, e = float(r["start"]), float(r["end"])
        add(
            lines,
            "edit.select",
            {"tracks": ["VO_keeps"], "start": samp(s, sr), "end": samp(e, sr)},
        )
        add(lines, "edit.clear", {})
        add(
            lines,
            "markers.add",
            {
                "name": f"DROP_{i + 1:02d}",
                "start": samp(s, sr),
                "end": samp(e, sr),
                "kind": "selection",
            },
        )

    # Split contiguous keep boundaries into separate clips.
    ordered = sorted(keeps, key=lambda k: float(k["start"]))
    for i in range(len(ordered) - 1):
        if abs(float(ordered[i]["end"]) - float(ordered[i + 1]["start"])) < 0.02:
            b = float(ordered[i]["end"])
            add(
                lines,
                "edit.select",
                {"tracks": ["VO_keeps"], "start": samp(b, sr), "end": samp(b, sr)},
            )
            add(lines, "edit.separate", {})

    for k in ordered:
        name = str(k.get("name") or "KEEP")
        s, e = float(k["start"]), float(k["end"])
        add(
            lines,
            "markers.add",
            {
                "name": f"KEEP_{name}",
                "start": samp(s, sr),
                "end": samp(e, sr),
                "kind": "selection",
            },
        )
        add(
            lines,
            "edit.select",
            {"tracks": ["VO_keeps"], "start": samp(s, sr), "end": samp(e, sr)},
        )
        add(lines, "clip.rename", {"name": name})

    add(lines, "session.save_as", {"path": sess_s, "replace": True})
    add(lines, "session.inspect", {"detail": "full"})
    return lines


def build_pass2_fades(
    *,
    session: Path,
    keeps: Sequence[dict[str, Any]],
    clip_ids: Sequence[int],
    sr: int,
    fade_sec: float = DEFAULT_FADE_SEC,
) -> list[str]:
    fade = max(1, int(round(fade_sec * sr)))
    lines: list[str] = []
    add(lines, "session.open", {"path": str(session).replace("\\", "/")})
    if clip_ids:
        for cid in clip_ids:
            add(
                lines,
                "edit.fades_create",
                {"clip": cid, "side": "in", "length": fade, "shape": "equal power"},
            )
            add(
                lines,
                "edit.fades_create",
                {"clip": cid, "side": "out", "length": fade, "shape": "equal power"},
            )
    else:
        for k in keeps:
            s, e = float(k["start"]), float(k["end"])
            add(
                lines,
                "edit.select",
                {
                    "tracks": ["VO_keeps"],
                    "start": samp(s, sr),
                    "end": samp(s, sr) + fade,
                },
            )
            add(lines, "edit.fades_create", {"shape": "equal power"})
            add(
                lines,
                "edit.select",
                {
                    "tracks": ["VO_keeps"],
                    "start": max(samp(s, sr), samp(e, sr) - fade),
                    "end": samp(e, sr),
                },
            )
            add(lines, "edit.fades_create", {"shape": "equal power"})
    add(lines, "session.save", {"path": str(session).replace("\\", "/"), "replace": True})
    return lines


def extract_clip_ids(log: str, keep_names: Sequence[str]) -> list[int]:
    ids: list[int] = []
    for name in keep_names:
        esc = re.escape(name)
        mm = re.search(
            r'"id"\s*:\s*(\d+)[^}]{0,240}"name"\s*:\s*"' + esc + r'"', log, re.S
        )
        if not mm:
            mm = re.search(
                r'"name"\s*:\s*"' + esc + r'"[^}]{0,240}"id"\s*:\s*(\d+)', log, re.S
            )
        if mm:
            ids.append(int(mm.group(1)))
    return ids


def apply_cut_plan(
    plan: dict[str, Any],
    *,
    wav: Path,
    session: Path,
    cli: Path,
    work_dir: Path,
    session_mode: str = "rebuild",
    fade_sec: float = DEFAULT_FADE_SEC,
    control_port: int | None = None,
) -> dict[str, Any]:
    """Write CLI scripts under work_dir and run them. Returns timing/meta."""
    sr = int(plan.get("sample_rate") or probe_sample_rate(wav))
    keeps = list(plan.get("keeps") or [])
    remove = list(plan.get("remove_ranges") or [])
    if not remove and keeps:
        import sys

        reaper = Path(__file__).resolve().parent.parent / "reaper"
        if str(reaper) not in sys.path:
            sys.path.insert(0, str(reaper))
        from vo_dedupe_last import gaps_from_keeps

        dur = float(plan.get("duration") or keeps[-1]["end"])
        remove = gaps_from_keeps(keeps, dur)

    work_dir.mkdir(parents=True, exist_ok=True)
    meta: dict[str, Any] = {
        "cli": str(cli),
        "session": str(session),
        "wav": str(wav),
        "n_keeps": len(keeps),
        "n_remove": len(remove),
        "sample_rate": sr,
        "session_mode": session_mode,
        "control_port": control_port,
    }

    # Prefer headless script — GUI without --control cannot be driven.
    p1 = work_dir / "sc_vo_cut_pass1.txt"
    lines1 = build_pass1_lines(
        wav=wav,
        session=session,
        keeps=keeps,
        remove_ranges=remove,
        sr=sr,
        session_mode=session_mode,
    )
    write_script(p1, lines1)
    log1 = run_cli_script(cli, p1, work_dir / "sc_vo_cut_pass1_run.log")
    names = [str(k.get("name") or "") for k in keeps]
    clip_ids = extract_clip_ids(log1, names)
    meta["clip_ids"] = clip_ids
    meta["pass1_cmds"] = len(lines1)

    p2 = work_dir / "sc_vo_cut_pass2.txt"
    lines2 = build_pass2_fades(
        session=session, keeps=keeps, clip_ids=clip_ids, sr=sr, fade_sec=fade_sec
    )
    write_script(p2, lines2)
    run_cli_script(cli, p2, work_dir / "sc_vo_cut_pass2_run.log")
    meta["pass2_cmds"] = len(lines2)
    meta["ok"] = True
    return meta
