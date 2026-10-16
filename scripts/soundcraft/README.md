# VO Cut — SoundCraft (Whisper → план → cuts)

Минимальный пример автоматизации: транскрипция VO → решение «что вырезать» → нарезка в сессии SoundCraft.

## Поток

1. **Whisper** (`faster-whisper`, тот же код что в `scripts/reaper/vo_dedupe_last.py`) → сегменты с таймкодами. Уже локально.
2. **План cuts** (JSON):
   - **offline / fallback** — правило «если фраза повторяется → KEEP **последний**», плюс отброс пустой болтовни;
   - **GigaChat** — если заданы env-креды (`GIGACHAT_CREDENTIALS` или `GIGACHAT_AUTH_KEY`);
   - **локальный LLM** — OpenAI-compatible HTTP (`--llm ollama` / `openai` / `lmstudio` / `local`): Ollama, LM Studio, llama.cpp server.
3. **SoundCraft** — `soundcraft-cli script`: треки `VO_source` (muted) + `VO_keeps` (лишнее `edit.clear`), маркеры, фейды ~15 ms.

По умолчанию цели: `input/nachitka.wav` → `sessions/nachitka/nachitka.scraft` (portable CLI в `portable/…`).

## Быстрый старт (nachitka)

```bat
cd C:\Users\mir24\Documents\CODING\ZVUKACH\scripts\reaper
python -m pip install -r requirements.txt

cd ..\soundcraft
REM только план (без правок сессии):
run_vo_cut.bat plan

REM план + нарезка сессии:
run_vo_cut.bat
```

Или явно:

```bat
python vo_cut_pipeline.py --prefer-cached-asr --llm fallback --dry-run
python vo_cut_pipeline.py --prefer-cached-asr --llm auto --apply
```

`--prefer-cached-asr` берёт `work/asr_segments.json`, если есть (быстрый демо без повторного Whisper).

Артефакты: `work/vo_cut_plan.json`, `work/sc_vo_cut_pass1.txt`, логи `*_run.log`. Перед `--apply` (режим `rebuild`) сессия копируется в `sessions/nachitka/Backups/`.

## Локальный LLM (рекомендуется: Ollama)

Whisper уже на машине. Для умного плана cuts без облака — Ollama + небольшая модель с нормальным русским JSON.

1. Установить [Ollama](https://ollama.com) для Windows.
2. Скачать модель (один раз, несколько GB):

```bat
ollama pull qwen2.5:7b
REM альтернатива полегче (~3–4 GB): ollama pull qwen2.5:3b
```

3. Убедиться, что сервер слушает `11434` (обычно сам после установки), затем:

```bat
cd scripts\soundcraft
run_vo_cut.bat ollama
REM или сразу в сессию:
run_vo_cut.bat ollama-apply

REM то же явно:
python vo_cut_pipeline.py --prefer-cached-asr --llm ollama --dry-run
python vo_cut_pipeline.py --prefer-cached-asr --llm ollama --apply --llm-model qwen2.5:7b
```

LM Studio: запустить Local Server (OpenAI), затем:

```bat
python vo_cut_pipeline.py --prefer-cached-asr --llm lmstudio --llm-model <id_из_LM_Studio> --dry-run
```

| Env / флаг | Смысл |
|------------|--------|
| `--llm ollama\|openai\|lmstudio\|local` | OpenAI-compatible `/v1/chat/completions` |
| `--llm-base-url` / `LOCAL_LLM_BASE_URL` / `OPENAI_BASE_URL` | по умолчанию Ollama `http://127.0.0.1:11434/v1`, LM Studio `:1234/v1` |
| `--llm-model` / `LOCAL_LLM_MODEL` / `OPENAI_MODEL` | по умолчанию `qwen2.5:7b` |
| `LOCAL_LLM_API_KEY` / `OPENAI_API_KEY` | опционально (Ollama принимает любой Bearer) |

### Модели и VRAM (ориентир)

| Модель | Когда брать | VRAM / RAM (примерно) |
|--------|-------------|------------------------|
| `qwen2.5:7b` | лучший баланс для русского JSON keep/drop | ~6–8 GB VRAM (GPU) или ~8–12 GB RAM (CPU, медленнее) |
| `qwen2.5:3b` | слабее железо, короткие VO | ~3–4 GB VRAM / ~6 GB RAM |

`--llm auto` **не** поднимает Ollama сам — только GigaChat (если есть креды) или fallback. Локальный LLM только по явному `--llm ollama` (и синонимам).

## Из SoundCraft (меню / хоткей)

После сборки из исходников (`cargo run --release -p soundcraft` или ваш локальный билд):

| | |
|--|--|
| Команда | `edit.vo_cut` |
| Меню | **Edit › VO Cut Pipeline...** |
| Хоткей | **Ctrl+Alt+Shift+V** (Windows; на macOS Cmd+Alt+Shift+V) |

Из меню / по умолчанию: **apply** (не dry-run), **свежий Whisper** (не старый `work/asr_segments.json`), `llm=ollama`, WAV = новейший файл в `Audio Files` сессии. В статус-баре снизу появится итог. После apply: **File › Open** ту же сессию снова (GUI держит старую копию в памяти). Dry-run только явно: `dry_run=true`.

Путь к скрипту: рядом с репо (`scripts/soundcraft/vo_cut_pipeline.py`), либо `SOUNDCRAFT_VO_CUT_SCRIPT`, либо param `script`. Python: `SOUNDCRAFT_PYTHON` / `ZVUKACH_PYTHON` / `PYTHON` / `python` в PATH.

Нужен **пересбор** SoundCraft из этого репо (`cargo run -p soundcraft`). Установленный `C:\Program Files\SoundCraft\` пункт **не** покажет — там старый бинарник. Пайплайн пишет в `.scraft` через portable CLI — если GUI держит ту же сессию открытой, **переоткройте** её после `--apply`.

Дальнейшая кастомизация (идеи): progress-диалог в UI, выбор wav/session из диалога, in-process план без Python, хоткей под «ollama-apply» одним нажатием.

### Внешний .bat (по-прежнему ок)

1. Создать ярлык на `scripts\soundcraft\run_vo_cut.bat`.
2. В свойствах ярлыка назначить **быстрый вызов** (или PowerToys Keyboard Manager / AutoHotkey → запуск `.bat`).
3. Для живого GUI с control channel: запускать SoundCraft как  
   `soundcraft.exe --control 7801`  
   затем `python vo_cut_pipeline.py --probe-control` / `--control-port 7801`.  
   Сейчас пайплайн **режет через CLI** в файл сессии; если GUI уже открыл тот же `.scraft`, после прогона **переоткройте** сессию.

## GigaChat

| Env | Смысл |
|-----|--------|
| `GIGACHAT_CREDENTIALS` или `GIGACHAT_AUTH_KEY` | Base64 ключ Authorization (Basic) |
| `GIGACHAT_SCOPE` | по умолчанию `GIGACHAT_API_PERS` |
| `GIGACHAT_MODEL` | по умолчанию `GigaChat` |
| `GIGACHAT_VERIFY_SSL` | `0` если корпоративный CA мешает |

Без кредов `--llm auto` = offline fallback. Принудительно: `--llm fallback` или `--llm gigachat`.

## CLI-флаги

| Флаг | Смысл |
|------|--------|
| `--dry-run` / `run_vo_cut.bat plan` | только JSON-план |
| `--apply` | записать сессию через portable `soundcraft-cli` |
| `--llm auto\|fallback\|gigachat\|ollama\|openai\|lmstudio\|local` | бэкенд плана |
| `--llm-base-url` / `--llm-model` | URL и модель для локального OpenAI-compat |
| `--segments-json PATH` | без Whisper |
| `--session-mode rebuild\|open` | `rebuild` (по умолчанию) = чистая 2-дорожечная сессия |
| `--probe-control` | поиск открытого `--control` порта |

## Offline vs ключ vs локальный LLM

| Шаг | Offline | Нужен ключ | Локальный LLM |
|-----|---------|------------|---------------|
| Whisper / кэш ASR | да | нет | — |
| План keep-last | да (`fallback`) | нет | — |
| План через GigaChat | — | `GIGACHAT_*` | — |
| План через Ollama/LM Studio | — | нет | Ollama/сервер + модель |
| Cuts в SoundCraft | portable CLI | нет | — |

**Когда хватает `fallback`:** явные ретейки с тем же началом фразы, сегменты ASR уже разбиты нормально.

**Когда помогает LLM (GigaChat или Ollama):** «почти те же» перефразы, мусор/разогрев между дублями, спорные короткие обрывки — модель читает текст и решает KEEP/DROP по смыслу. Fallback не распилит дубль *внутри* одного длинного ASR-сегмента.

## Ограничения

- `edit.vo_cut` зависит от установленного Python и наличия `vo_cut_pipeline.py` (или env-пути); WASM/web — unsupported.
- GUI без `--control` не принимает `soundcraft-cli app`; cuts идут в файл `.scraft` (после apply переоткройте сессию).
- Дубли внутри одного длинного ASR-сегмента fallback не распилит (как в REAPER-скрипте).
- GigaChat / локальный LLM могут ошибиться в индексах — при сбое `auto` (только GigaChat) откатывается на fallback; `--llm ollama` при недоступном сервере падает с ошибкой.
