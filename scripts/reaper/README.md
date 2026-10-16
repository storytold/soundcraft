# VO Dedupe — автономный workflow для REAPER

Локальная расшифровка (faster-whisper) → **оставить только последний** вариант фразы при общем начале / точном повторе → KEEP на треке 2. Трек 1 — полный исходник; трек 2 (`VO_keeps`) — KEEP на исходных позициях с короткими фейдами (~15 мс). Cursor/агент не нужны.

## Правило (по умолчанию)

После ASR сравниваются **нормализованные** тексты сегментов:

1. **Точный дубль** — одинаковый текст → KEEP только последнее вхождение.
2. **Общий префикс** (≥2 ведущих слова, напр. «ответы пациентов») — даже если продолжение разное → KEEP только **последний** вариант, ранние = DROP.

Fuzzy «мешок слов» / similarity **не** используется по умолчанию.

Пример:

- A @1:07: «Ответы пациентов сразу…» → B @1:24: «Ответы пациентов сразу… врач видит…» → DROP A, KEEP B.
- A: «Привет, это дубль» → B: «привет это дубль» → DROP A, KEEP B.
- Совершенно разные фразы (разное начало) → обе KEEP.

Опционально: `--method exact` (только точные повторы) или `--method similarity` / ExtState `method=similarity`.

## Файлы

| Файл | Назначение |
|------|------------|
| `vo_dedupe_last.py` | CLI: ASR, кэш, prefix-dedupe, JSON/SRT, опциональный render WAV |
| `vo_dedupe_reaper.lua` | ReaScript: вызывает Python и режет клипы на `VO_keeps` |
| `run_dedupe.bat` | Быстрый прогон `input\*.wav` без REAPER |
| `requirements.txt` | `faster-whisper` |

## Требования

- Windows, Python 3.10+ в PATH (или путь в переменной / ExtState)
- `pip install -r requirements.txt` (из этой папки)
- Для GPU: CUDA + свежий `ctranslate2` (иначе `--device cpu`)
- REAPER (для Lua); без REAPER достаточно CLI / `.bat`
- ffmpeg — только если render не-PCM WAV (обычно не нужен для обычных WAV)

## Установка Python-части

```bat
cd C:\Users\mir24\Documents\CODING\ZVUKACH\scripts\reaper
python -m pip install -r requirements.txt
python vo_dedupe_last.py --self-test
```

Проверка на файле из `input\`:

```bat
run_dedupe.bat
```

или явно:

```bat
python vo_dedupe_last.py ..\..\input\nachitka.wav --model base --device auto --render
```

Результат рядом с WAV: `*.vo_dedupe.json`, опционально `*_keeps.wav`.

### Кэш ASR

По умолчанию: `<папка_wav>\.cache\<имя>.<hash16>.asr.json` (+ `.meta.json`). Повторный запуск с тем же файлом и моделью не гоняет Whisper заново. Сброс: удалить `.cache` или `--no-cache`.

### Полезные флаги CLI

| Флаг | Смысл |
|------|--------|
| `--method prefix` | **По умолчанию:** ведущий префикс (≥2 слова) + точные дубли → оставить последнее |
| `--method exact` | Только точные повторы |
| `--method similarity` | Опционально: старый fuzzy-кластер «оставить последний» |
| `--min-prefix-words 2` | Мин. общих ведущих слов (default 2) |
| `--min-prefix-chars 12` | Для incomplete→full, если слов чуть меньше порога |
| `--model base` | Модель Whisper (`tiny`/`base`/`small`/…) |
| `--device auto\|cpu\|cuda` | Устройство |
| `--merge-gap 0` | Склеивать соседние ASR-сегменты ближе этого зазора (сек); 0 = не клеить |
| `--similarity 0.72` | Только для `--method similarity` |
| `--fade 0.015` | Фейды при `--render` |
| `--render [path]` | WAV только с KEEP на местах + тишина в дырах |
| `--srt out.srt` | Субтитры keep+drop |
| `--segments-json file.json` | Без ASR — только дедуп по готовым сегментам |

## REAPER: первый запуск (3–5 шагов)

1. **Actions → Show action list → New action → Load ReaScript…**  
   Указать `scripts\reaper\vo_dedupe_reaper.lua`. При желании назначить хоткей.
2. Импортировать VO на **трек 1** (один media item; путь к файлу на диске должен резолвиться — не только встроенный glue без файла).
3. Выделить этот item (или навести мышь) → запустить action **VO Dedupe**.
4. Дождаться окна «Готово»: появится трек **`VO_keeps`** с нарезанными KEEP и фейдами; трек 1 не меняется. JSON пишется рядом с исходным WAV.
5. При ошибке Python: в REAPER → ExtState задать путь к `python.exe`.

### Повторный прогон после этого фикса

1. Убедиться, что Lua подхватил обновлённый файл (тот же путь `scripts\reaper\vo_dedupe_reaper.lua`).
2. Если раньше в ExtState стояло `method=exact` — скрипт **один раз** мигрирует на `prefix` сам; либо вручную:  
   `reaper.SetExtState("ZVUKACH_VO_Dedupe","method","prefix",true)`  
   и `min_prefix_words=2`.
3. Выделить исходный item на треке 1 → снова **VO Dedupe**.  
   JSON перезапишется, `VO_keeps` очистится и нарежется заново. ASR при неизменном файле — из кэша.

### Настройки (ExtState)

Секция `ZVUKACH_VO_Dedupe`:

| Ключ | По умолчанию | Описание |
|------|--------------|----------|
| `python` | `python` | Полный путь к `python.exe` при необходимости |
| `model` | `base` | Модель Whisper |
| `device` | `auto` | `auto` / `cpu` / `cuda` |
| `script_dir` | авто | Папка с `vo_dedupe_last.py` |
| `fade_ms` | `15` | Fade in/out на KEEP-клипах |
| `method` | `prefix` | `prefix` (по умолчанию) / `exact` / `similarity` |
| `min_prefix_words` | `2` | Мин. общих ведущих слов |
| `similarity` | `0.72` | Только если `method=similarity` |

Переменная окружения для `.bat`: `ZVUKACH_PYTHON=C:\Path\to\python.exe`.

## Логика дедупа (prefix)

1. ASR (VAD) → сегменты текста.  
2. Близкие по времени сегменты склеиваются (`--merge-gap`; по умолчанию выкл.).  
3. Нормализация текста (нижний регистр, ё→е, без пунктуации); таймкоды исходные.  
4. Если позже есть сегмент с тем же текстом **или** тем же ведущим префиксом (≥ N слов) → ранний DROP.  
5. Уникальные фразы (другое начало) не трогаются.  
6. На таймлайне позиции **не сдвигаются**. Lua склеивает соседние KEEP (±20 мс), чтобы не было дыр/двойных фейдов на стыках.

## Troubleshooting

| Симптом | Что сделать |
|---------|-------------|
| `No module named faster_whisper` | `pip install -r requirements.txt` тем же Python, что видит REAPER |
| CUDA / cublas | Скрипт падает на `cpu` при `auto`; явно: `--device cpu` |
| Lua: source path not found | Render/glue item в WAV на диск |
| Старое exact-only поведение | ExtState `method=prefix`, `min_prefix_words=2` |
| 0 KEEP | Проверить ASR / язык `--language ru` |
| Долгий первый прогон | Нормально: качается модель; дальше кэш |
| Дыры на треке 2 | Обновлённый Lua coalesce + file→timeline map; перезапустить Action |

## Известные ограничения

- Сравнивается ведущий **префикс слов** и точный текст, не «похожесть» мешком токенов.
- Дубли внутри **одного** длинного ASR-сегмента скрипт не распилит.
- Render через stdlib `wave` — для PCM WAV.
- Сессии SoundCraft (`sessions/nachitka`) этот скрипт **не трогает**.

## SoundCraft (тот же ASR → cuts в `.scraft`)

См. [`scripts/soundcraft/README.md`](../soundcraft/README.md): Whisper + keep-last (или GigaChat) → `soundcraft-cli` режет `VO_keeps` в сессии nachitka. Хоткей: ярлык на `scripts\soundcraft\run_vo_cut.bat`.

## Пример без REAPER (только JSON из кэша ASR проекта)

```bat
python vo_dedupe_last.py --segments-json ..\..\work\asr_segments.json -o ..\..\work\vo_dedupe_from_cache.json
```
