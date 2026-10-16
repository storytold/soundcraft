# -*- coding: utf-8 -*-
"""LLM cut-plan step for VO dedupe (GigaChat, local OpenAI-compatible, or fallback).

Policy (default):
  - if a phrase repeats / shares a leading prefix → KEEP THE LAST occurrence
  - drop empty chatter / filler crumbs
"""
from __future__ import annotations

import json
import os
import re
import ssl
import urllib.error
import urllib.request
from typing import Any, Sequence

# Reuse the offline keep-last rules from the REAPER VO script.
import sys
from pathlib import Path

_REAPER = Path(__file__).resolve().parent.parent / "reaper"
if str(_REAPER) not in sys.path:
    sys.path.insert(0, str(_REAPER))

from vo_dedupe_last import (  # noqa: E402
    Segment,
    decide_keeps_drops,
    gaps_from_keeps,
    is_noise_segment,
    merge_close_segments,
)

GIGACHAT_OAUTH = "https://ngw.devices.sberbank.ru:9443/api/v2/oauth"
GIGACHAT_CHAT = "https://gigachat.devices.sberbank.ru/api/v1/chat/completions"

# OpenAI-compatible defaults (Ollama / LM Studio / llama.cpp server).
DEFAULT_OLLAMA_BASE = "http://127.0.0.1:11434/v1"
DEFAULT_LMSTUDIO_BASE = "http://127.0.0.1:1234/v1"
DEFAULT_LOCAL_MODEL = "qwen2.5:7b"

OPENAI_COMPAT_BACKENDS = frozenset({"openai", "ollama", "lmstudio", "local"})

SYSTEM_PROMPT = """Ты — монтажёр закадрового голоса (VO).
Тебе дают сегменты ASR с таймкодами (start/end в секундах) и текст.

Правила:
1) Если фраза повторяется или поздний сегмент — ретейк с тем же началом → KEEP только ПОСЛЕДНИЙ.
2) Пустая болтовня, разогрев, «эм», обрывки без смысла → DROP.
3) Не сдвигай таймлайн: возвращай исходные start/end сегментов.
4) Ответ — ТОЛЬКО JSON без markdown:
{"decisions":[{"i":0,"decision":"KEEP"|"DROP","reason":"..."}],"notes":"..."}
Индексы i — номера входных сегментов (0-based). Каждый сегмент ровно один раз.
"""

JSON_RETRY_PROMPT = (
    "Предыдущий ответ был невалидным JSON. "
    "Верни ТОЛЬКО один JSON-объект, без markdown-оград и без пояснений:\n"
    '{"decisions":[{"i":0,"decision":"KEEP"|"DROP","reason":"..."}],"notes":"..."}'
)


def segments_to_models(raw: Sequence[dict[str, Any]]) -> list[Segment]:
    out: list[Segment] = []
    for i, s in enumerate(raw):
        out.append(
            Segment(
                float(s["start"]),
                float(s["end"]),
                str(s.get("text") or "").strip(),
                i,
            )
        )
    return out


def fallback_cut_plan(
    raw_segments: Sequence[dict[str, Any]],
    *,
    duration: float,
    method: str = "prefix",
    merge_gap: float = 0.0,
    min_prefix_words: int = 2,
) -> dict[str, Any]:
    """Deterministic offline plan: same keep-last / noise rules as vo_dedupe_last."""
    segs = merge_close_segments(segments_to_models(raw_segments), gap=merge_gap)
    keeps, drops = decide_keeps_drops(
        segs,
        method=method,
        min_prefix_words=min_prefix_words,
    )
    named_keeps = []
    for n, k in enumerate(keeps, 1):
        named_keeps.append(
            {
                "start": k["start"],
                "end": k["end"],
                "name": f"{n:02d}_KEEP",
                "text": k.get("text") or "",
                "reason": k.get("reason") or "keep-last / unique",
            }
        )
    remove = gaps_from_keeps(named_keeps, duration)
    return {
        "policy": "keep_last_duplicate + drop_noise",
        "llm_backend": "fallback",
        "method": method,
        "duration": duration,
        "keeps": named_keeps,
        "drops": [
            {
                "start": d["start"],
                "end": d["end"],
                "text": d.get("text") or "",
                "reason": d.get("reason") or "drop",
            }
            for d in drops
        ],
        "remove_ranges": remove,
        "notes": "deterministic prefix/exact keep-last (no API)",
    }


def _ssl_ctx() -> ssl.SSLContext:
    # GigaChat uses a corporate CA chain; allow override via env.
    if os.environ.get("GIGACHAT_VERIFY_SSL", "1").strip() in ("0", "false", "no"):
        return ssl._create_unverified_context()
    return ssl.create_default_context()


def _gigachat_credentials() -> str | None:
    for key in (
        "GIGACHAT_CREDENTIALS",
        "GIGACHAT_AUTH_KEY",
        "GIGACHAT_AUTHORIZATION_KEY",
    ):
        val = (os.environ.get(key) or "").strip()
        if val:
            return val
    return None


def gigachat_available() -> bool:
    return _gigachat_credentials() is not None


def _oauth_token() -> str:
    creds = _gigachat_credentials()
    if not creds:
        raise RuntimeError("GIGACHAT_CREDENTIALS / GIGACHAT_AUTH_KEY not set")
    scope = (os.environ.get("GIGACHAT_SCOPE") or "GIGACHAT_API_PERS").strip()
    body = f"scope={scope}".encode("ascii")
    req = urllib.request.Request(
        GIGACHAT_OAUTH,
        data=body,
        method="POST",
        headers={
            "Content-Type": "application/x-www-form-urlencoded",
            "Accept": "application/json",
            "RqUID": os.environ.get("GIGACHAT_RQUID")
            or "00000000-0000-0000-0000-000000000001",
            "Authorization": f"Basic {creds}",
        },
    )
    with urllib.request.urlopen(req, context=_ssl_ctx(), timeout=60) as resp:
        data = json.loads(resp.read().decode("utf-8"))
    token = data.get("access_token")
    if not token:
        raise RuntimeError(f"GigaChat OAuth failed: {data!r}")
    return str(token)


def _gigachat_chat_completion(token: str, user_payload: str) -> str:
    model = (os.environ.get("GIGACHAT_MODEL") or "GigaChat").strip()
    payload = {
        "model": model,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user_payload},
        ],
        "temperature": 0.1,
    }
    req = urllib.request.Request(
        GIGACHAT_CHAT,
        data=json.dumps(payload, ensure_ascii=False).encode("utf-8"),
        method="POST",
        headers={
            "Content-Type": "application/json",
            "Accept": "application/json",
            "Authorization": f"Bearer {token}",
        },
    )
    with urllib.request.urlopen(req, context=_ssl_ctx(), timeout=120) as resp:
        data = json.loads(resp.read().decode("utf-8"))
    try:
        return str(data["choices"][0]["message"]["content"])
    except (KeyError, IndexError, TypeError) as exc:
        raise RuntimeError(f"Unexpected GigaChat response: {data!r}") from exc


def _local_base_url(backend: str) -> str:
    for key in ("LOCAL_LLM_BASE_URL", "OPENAI_BASE_URL"):
        val = (os.environ.get(key) or "").strip().rstrip("/")
        if val:
            return val
    b = backend.lower().strip()
    if b == "lmstudio":
        return DEFAULT_LMSTUDIO_BASE
    return DEFAULT_OLLAMA_BASE


def _local_model(backend: str) -> str:
    for key in ("LOCAL_LLM_MODEL", "OPENAI_MODEL"):
        val = (os.environ.get(key) or "").strip()
        if val:
            return val
    if backend.lower().strip() == "lmstudio":
        # LM Studio often uses the loaded model id; caller should set LOCAL_LLM_MODEL.
        return (os.environ.get("LOCAL_LLM_MODEL") or "local-model").strip()
    return DEFAULT_LOCAL_MODEL


def _local_api_key() -> str:
    for key in ("LOCAL_LLM_API_KEY", "OPENAI_API_KEY"):
        val = (os.environ.get(key) or "").strip()
        if val:
            return val
    return "ollama"


def _openai_chat_completion(
    user_payload: str,
    *,
    backend: str,
    base_url: str | None = None,
    model: str | None = None,
    timeout: float = 300.0,
    messages: Sequence[dict[str, str]] | None = None,
    json_mode: bool = True,
) -> str:
    """POST /v1/chat/completions (Ollama, LM Studio, llama.cpp, etc.)."""
    root = (base_url or _local_base_url(backend)).rstrip("/")
    mdl = (model or _local_model(backend)).strip()
    url = f"{root}/chat/completions"
    msgs = list(messages) if messages is not None else [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": user_payload},
    ]
    payload: dict[str, Any] = {
        "model": mdl,
        "messages": msgs,
        "temperature": 0.1,
    }
    # Ollama OpenAI-compat maps response_format json_object → format:"json".
    if json_mode:
        payload["response_format"] = {"type": "json_object"}
        # Native Ollama /v1 also accepts format; harmless if ignored elsewhere.
        if backend.lower().strip() in ("ollama", "local"):
            payload["format"] = "json"
    req = urllib.request.Request(
        url,
        data=json.dumps(payload, ensure_ascii=False).encode("utf-8"),
        method="POST",
        headers={
            "Content-Type": "application/json",
            "Accept": "application/json",
            "Authorization": f"Bearer {_local_api_key()}",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            data = json.loads(resp.read().decode("utf-8"))
    except urllib.error.URLError as exc:
        raise RuntimeError(
            f"Local LLM unreachable at {url} ({exc}). "
            "Start Ollama (`ollama serve`) or LM Studio server, "
            "or set LOCAL_LLM_BASE_URL."
        ) from exc
    try:
        return str(data["choices"][0]["message"]["content"])
    except (KeyError, IndexError, TypeError) as exc:
        raise RuntimeError(f"Unexpected local LLM response: {data!r}") from exc


def _light_json_repair(text: str) -> str:
    """Safe-ish fixes: trailing commas before } or ]."""
    return re.sub(r",(\s*[}\]])", r"\1", text)


def _strip_markdown_fences(text: str) -> str:
    text = (text or "").strip()
    if "```" not in text:
        return text
    fenced = re.search(r"```(?:json)?\s*\r?\n?(.*?)```", text, re.S | re.I)
    if fenced:
        return fenced.group(1).strip()
    text = re.sub(r"^```(?:json)?\s*", "", text, flags=re.I)
    text = re.sub(r"\s*```\s*$", "", text)
    return text.strip()


def _outermost_object(text: str) -> str | None:
    start = text.find("{")
    end = text.rfind("}")
    if start < 0 or end <= start:
        return None
    return text[start : end + 1]


def _extract_json(text: str) -> dict[str, Any]:
    text = _strip_markdown_fences(text)
    candidates: list[str] = []
    if text:
        candidates.append(text)
    outer = _outermost_object(text)
    if outer and outer not in candidates:
        candidates.append(outer)

    last_err: json.JSONDecodeError | None = None
    for cand in candidates:
        for variant in (cand, _light_json_repair(cand)):
            try:
                obj = json.loads(variant)
            except json.JSONDecodeError as exc:
                last_err = exc
                continue
            if isinstance(obj, dict):
                return obj
            last_err = json.JSONDecodeError(
                "JSON root must be an object", variant, 0
            )
    if last_err is not None:
        raise last_err
    raise json.JSONDecodeError("Expecting value", text or "", 0)

def _segments_user_payload(segs: Sequence[Segment]) -> str:
    listing = []
    for i, s in enumerate(segs):
        listing.append(
            {
                "i": i,
                "start": round(s.start, 3),
                "end": round(s.end, 3),
                "text": s.text,
            }
        )
    return (
        "Сегменты:\n"
        + json.dumps(listing, ensure_ascii=False, indent=2)
        + "\n\nВерни JSON decisions для всех i."
    )


def _apply_llm_decisions(
    segs: Sequence[Segment],
    decisions: Sequence[dict[str, Any]],
    *,
    duration: float,
    notes: str,
    llm_backend: str,
) -> dict[str, Any]:
    by_i: dict[int, dict[str, Any]] = {}
    for d in decisions:
        try:
            i = int(d["i"])
        except (KeyError, TypeError, ValueError):
            continue
        by_i[i] = d

    keeps: list[dict[str, Any]] = []
    drops: list[dict[str, Any]] = []
    for i, seg in enumerate(segs):
        d = by_i.get(i)
        # Missing → fallback to noise check only; else KEEP.
        if d is None:
            decision = "DROP" if is_noise_segment(seg) else "KEEP"
            reason = (
                "missing LLM decision; noise"
                if decision == "DROP"
                else "missing LLM decision; keep"
            )
        else:
            decision = str(d.get("decision") or "KEEP").upper().strip()
            if decision not in ("KEEP", "DROP"):
                decision = "KEEP"
            reason = str(d.get("reason") or "llm")
        piece = {
            "start": round(seg.start, 3),
            "end": round(seg.end, 3),
            "text": seg.text,
            "reason": reason,
        }
        if decision == "DROP":
            drops.append(piece)
        else:
            keeps.append(piece)

    named = []
    for n, k in enumerate(sorted(keeps, key=lambda x: x["start"]), 1):
        named.append(
            {
                "start": k["start"],
                "end": k["end"],
                "name": f"{n:02d}_KEEP",
                "text": k.get("text") or "",
                "reason": k.get("reason") or "llm KEEP",
            }
        )
    return {
        "policy": "keep_last_duplicate + drop_noise",
        "llm_backend": llm_backend,
        "duration": duration,
        "keeps": named,
        "drops": sorted(drops, key=lambda x: x["start"]),
        "remove_ranges": gaps_from_keeps(named, duration),
        "notes": notes,
    }


def gigachat_cut_plan(
    raw_segments: Sequence[dict[str, Any]],
    *,
    duration: float,
    merge_gap: float = 0.0,
) -> dict[str, Any]:
    segs = merge_close_segments(segments_to_models(raw_segments), gap=merge_gap)
    user = _segments_user_payload(segs)
    token = _oauth_token()
    content = _gigachat_chat_completion(token, user)
    parsed = _extract_json(content)
    decisions = parsed.get("decisions") or []
    notes = str(parsed.get("notes") or "")
    return _apply_llm_decisions(
        segs, decisions, duration=duration, notes=notes, llm_backend="gigachat"
    )


def openai_cut_plan(
    raw_segments: Sequence[dict[str, Any]],
    *,
    duration: float,
    merge_gap: float = 0.0,
    backend: str = "ollama",
    base_url: str | None = None,
    model: str | None = None,
    method: str = "prefix",
    allow_fallback: bool = True,
) -> dict[str, Any]:
    """Local (or remote) OpenAI-compatible chat completions for cut decisions.

    On JSON parse failure: one stricter retry, then deterministic fallback
    (unless allow_fallback=False).
    """
    segs = merge_close_segments(segments_to_models(raw_segments), gap=merge_gap)
    user = _segments_user_payload(segs)
    label = backend.lower().strip() or "openai"
    used_url = (base_url or _local_base_url(label)).rstrip("/")
    used_model = (model or _local_model(label)).strip()

    def _finish(parsed: dict[str, Any]) -> dict[str, Any]:
        decisions = parsed.get("decisions") or []
        notes = str(parsed.get("notes") or "")
        plan = _apply_llm_decisions(
            segs, decisions, duration=duration, notes=notes, llm_backend=label
        )
        plan["local_llm"] = {"base_url": used_url, "model": used_model}
        return plan

    def _fallback(exc: BaseException, *, after_retry: bool) -> dict[str, Any]:
        where = "after JSON retry" if after_retry else "on JSON parse"
        msg = f"{label} LLM JSON failed {where} ({exc}); used fallback"
        print(f"warning: {msg}", file=sys.stderr)
        plan = fallback_cut_plan(
            raw_segments, duration=duration, method=method, merge_gap=merge_gap
        )
        plan["notes"] = msg
        plan["llm_backend"] = f"fallback_after_{label}_json_error"
        plan["local_llm"] = {"base_url": used_url, "model": used_model}
        return plan

    content = _openai_chat_completion(
        user, backend=label, base_url=base_url, model=model
    )
    try:
        return _finish(_extract_json(content))
    except json.JSONDecodeError as first_exc:
        retry_messages = [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user},
            {"role": "assistant", "content": content},
            {"role": "user", "content": JSON_RETRY_PROMPT},
        ]
        try:
            content2 = _openai_chat_completion(
                user,
                backend=label,
                base_url=base_url,
                model=model,
                messages=retry_messages,
            )
            return _finish(_extract_json(content2))
        except (json.JSONDecodeError, RuntimeError) as retry_exc:
            if not allow_fallback:
                raise retry_exc from first_exc
            return _fallback(retry_exc, after_retry=True)


def build_cut_plan(
    raw_segments: Sequence[dict[str, Any]],
    *,
    duration: float,
    backend: str = "auto",
    method: str = "prefix",
    merge_gap: float = 0.0,
    openai_base_url: str | None = None,
    openai_model: str | None = None,
) -> dict[str, Any]:
    """
    backend:
      - auto: GigaChat if credentials present, else fallback
      - fallback / offline / none: deterministic keep-last
      - gigachat: require GigaChat API (raises on failure)
      - openai / ollama / lmstudio / local: OpenAI-compatible HTTP
        (default Ollama http://127.0.0.1:11434/v1);
        bad JSON → one retry → deterministic fallback (no crash)
    """
    b = (backend or "auto").lower().strip()
    if b in ("fallback", "offline", "none"):
        return fallback_cut_plan(
            raw_segments, duration=duration, method=method, merge_gap=merge_gap
        )
    if b == "gigachat":
        return gigachat_cut_plan(
            raw_segments, duration=duration, merge_gap=merge_gap
        )
    if b in OPENAI_COMPAT_BACKENDS:
        # JSON failures: retry + fallback inside openai_cut_plan (no crash).
        # Unreachable server still raises RuntimeError.
        return openai_cut_plan(
            raw_segments,
            duration=duration,
            merge_gap=merge_gap,
            backend=b,
            base_url=openai_base_url,
            model=openai_model,
            method=method,
            allow_fallback=True,
        )
    # auto
    if gigachat_available():
        try:
            return gigachat_cut_plan(
                raw_segments, duration=duration, merge_gap=merge_gap
            )
        except (
            urllib.error.URLError,
            TimeoutError,
            OSError,
            RuntimeError,
            json.JSONDecodeError,
        ) as exc:
            plan = fallback_cut_plan(
                raw_segments, duration=duration, method=method, merge_gap=merge_gap
            )
            plan["notes"] = f"GigaChat failed ({exc}); used fallback"
            plan["llm_backend"] = "fallback_after_gigachat_error"
            return plan
    return fallback_cut_plan(
        raw_segments, duration=duration, method=method, merge_gap=merge_gap
    )
