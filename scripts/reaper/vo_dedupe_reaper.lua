-- VO Dedupe (ReaScript)
-- Track 1 = full source (untouched). Track 2 = VO_keeps at original times + short fades.
-- Calls scripts/reaper/vo_dedupe_last.py (faster-whisper). Windows-friendly paths.
-- Default rule: shared leading-word prefix (≥2) or exact duplicate → keep last only.
-- See --method prefix in the Python CLI. No fuzzy bag similarity.
--
-- ExtState section "ZVUKACH_VO_Dedupe":
--   python            = path to python.exe (default: python)
--   model             = whisper model (default: base)
--   device            = auto|cpu|cuda (default: auto)
--   script_dir        = folder with vo_dedupe_last.py (optional; auto-detected)
--   fade_ms           = fade in/out milliseconds (default: 15)
--   method            = prefix (default) | exact | similarity (legacy)
--   min_prefix_words  = min shared leading words (default: 2)
--   similarity        = legacy near-dup threshold if method=similarity (default: 0.72)

local EXT = "ZVUKACH_VO_Dedupe"
local TRACK_KEEPS_NAME = "VO_keeps"
local FADE_DEFAULT_MS = 15

----------------------------------------------------------------
-- helpers
----------------------------------------------------------------

local function msg(s)
  reaper.ShowConsoleMsg(tostring(s) .. "\n")
end

local function get_ext(key, default)
  local v = reaper.GetExtState(EXT, key)
  if not v or v == "" then return default end
  return v
end

local function set_ext(key, value)
  reaper.SetExtState(EXT, key, tostring(value), true)
end

local function trim(s)
  return (tostring(s):gsub("^%s+", ""):gsub("%s+$", ""))
end

local function path_join(a, b)
  a = a:gsub("[/\\]+$", "")
  b = b:gsub("^[/\\]+", "")
  return a .. "\\" .. b
end

local function file_exists(path)
  local f = io.open(path, "rb")
  if f then f:close() return true end
  return false
end

local function quote_win(path)
  -- Wrap for cmd.exe; escape embedded quotes.
  return '"' .. tostring(path):gsub('"', '\\"') .. '"'
end

local function script_dir_guess()
  local info = debug.getinfo(1, "S")
  local src = info and info.source or ""
  if src:sub(1, 1) == "@" then
    local p = src:sub(2)
    local dir = p:match("^(.*)[/\\][^/\\]+$")
    if dir and file_exists(path_join(dir, "vo_dedupe_last.py")) then
      return dir
    end
  end
  -- Fallback: common workspace layout next to this chat's project
  local fallback = [[C:\Users\mir24\Documents\CODING\ZVUKACH\scripts\reaper]]
  if file_exists(path_join(fallback, "vo_dedupe_last.py")) then
    return fallback
  end
  return nil
end

local function read_file(path)
  local f = io.open(path, "rb")
  if not f then return nil end
  local data = f:read("*a")
  f:close()
  return data
end

-- Minimal JSON array extractor for keeps: looks for "keeps":[ ... ]
-- Enough for our CLI output (no nested arrays inside keep objects).
local function parse_keeps_json(json_text)
  local keeps = {}
  local block = json_text:match('"keeps"%s*:%s*%[')
  if not block then return keeps end
  local start_i = json_text:find('"keeps"%s*:%s*%[')
  if not start_i then return keeps end
  local i = json_text:find("%[", start_i)
  if not i then return keeps end
  -- walk objects { ... }
  local pos = i + 1
  local n = #json_text
  while pos <= n do
    local ch = json_text:sub(pos, pos)
    if ch == "]" then break end
    if ch == "{" then
      local depth = 1
      local j = pos + 1
      while j <= n and depth > 0 do
        local c = json_text:sub(j, j)
        if c == "{" then depth = depth + 1
        elseif c == "}" then depth = depth - 1
        elseif c == '"' then
          -- skip string
          j = j + 1
          while j <= n do
            local cc = json_text:sub(j, j)
            if cc == "\\" then j = j + 2
            elseif cc == '"' then break
            else j = j + 1 end
          end
        end
        j = j + 1
      end
      local obj = json_text:sub(pos, j - 1)
      local s = obj:match('"start"%s*:%s*([%-%d%.]+)')
      local e = obj:match('"end"%s*:%s*([%-%d%.]+)')
      local name = obj:match('"name"%s*:%s*"([^"]*)"')
      if s and e then
        keeps[#keeps + 1] = {
          start = tonumber(s),
          ["end"] = tonumber(e),
          name = name or "",
        }
      end
      pos = j
    else
      pos = pos + 1
    end
  end
  table.sort(keeps, function(a, b) return a.start < b.start end)
  return keeps
end

----------------------------------------------------------------
-- item / track
----------------------------------------------------------------

local function get_target_item()
  local item = reaper.GetSelectedMediaItem(0, 0)
  if item then return item end
  -- item under mouse (requires SWS for BR_*; fall back to cursor track first item)
  if reaper.BR_ItemAtMouseCursor then
    local it = reaper.BR_ItemAtMouseCursor()
    if it then return it end
  end
  local x, y = reaper.GetMousePosition()
  if reaper.GetItemFromPoint then
    local it = reaper.GetItemFromPoint(x, y, true)
    if it then return it end
  end
  return nil
end

local function item_source_path(item)
  local take = reaper.GetActiveTake(item)
  if not take then return nil, "no active take" end
  local src = reaper.GetMediaItemTake_Source(take)
  if not src then return nil, "no take source" end
  -- resolve section/reversed to root file
  local root = src
  for _ = 1, 8 do
    local parent = reaper.GetMediaSourceParent(root)
    if not parent then break end
    root = parent
  end
  local path = reaper.GetMediaSourceFileName(root, "")
  if type(path) == "string" and path ~= "" then
    return path, nil
  end
  -- REAPER API sometimes returns retval, filename
  local ok, filename = reaper.GetMediaSourceFileName(root, "")
  if type(ok) == "string" and ok ~= "" then return ok, nil end
  if type(filename) == "string" and filename ~= "" then return filename, nil end
  return nil, "cannot resolve source filename (glue/export the item first?)"
end

local function ensure_keeps_track(source_track)
  local src_idx = reaper.GetMediaTrackInfo_Value(source_track, "IP_TRACKNUMBER") - 1
  -- Prefer next track if named VO_keeps, else find by name, else insert after source.
  local cnt = reaper.CountTracks(0)
  for i = 0, cnt - 1 do
    local tr = reaper.GetTrack(0, i)
    local _, name = reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", "", false)
    if name == TRACK_KEEPS_NAME then
      return tr
    end
  end
  reaper.InsertTrackAtIndex(src_idx + 1, true)
  local tr = reaper.GetTrack(0, src_idx + 1)
  reaper.GetSetMediaTrackInfo_String(tr, "P_NAME", TRACK_KEEPS_NAME, true)
  return tr
end

local function clear_track_items(track)
  for i = reaper.CountTrackMediaItems(track) - 1, 0, -1 do
    local it = reaper.GetTrackMediaItem(track, i)
    reaper.DeleteTrackMediaItem(track, it)
  end
end

local function copy_item_to_track(item, dest_track)
  -- Duplicate via API: create new item with same source take stretch markers etc.
  local take = reaper.GetActiveTake(item)
  local src = reaper.GetMediaItemTake_Source(take)
  local pos = reaper.GetMediaItemInfo_Value(item, "D_POSITION")
  local len = reaper.GetMediaItemInfo_Value(item, "D_LENGTH")
  local soffs = reaper.GetMediaItemTakeInfo_Value(take, "D_STARTOFFS")
  local rate = reaper.GetMediaItemTakeInfo_Value(take, "D_PLAYRATE")
  local new_item = reaper.AddMediaItemToTrack(dest_track)
  reaper.SetMediaItemInfo_Value(new_item, "D_POSITION", pos)
  reaper.SetMediaItemInfo_Value(new_item, "D_LENGTH", len)
  local new_take = reaper.AddTakeToMediaItem(new_item)
  reaper.SetMediaItemTake_Source(new_take, src)
  reaper.SetMediaItemTakeInfo_Value(new_take, "D_STARTOFFS", soffs)
  reaper.SetMediaItemTakeInfo_Value(new_take, "D_PLAYRATE", rate)
  local _, take_name = reaper.GetSetMediaItemTakeInfo_String(take, "P_NAME", "", false)
  if take_name and take_name ~= "" then
    reaper.GetSetMediaItemTakeInfo_String(new_take, "P_NAME", take_name, true)
  end
  return new_item
end

local function apply_fade(item, fade_sec)
  if fade_sec <= 0 then return end
  local len = reaper.GetMediaItemInfo_Value(item, "D_LENGTH")
  local f = math.min(fade_sec, len * 0.45)
  reaper.SetMediaItemInfo_Value(item, "D_FADEINLEN", f)
  reaper.SetMediaItemInfo_Value(item, "D_FADEOUTLEN", f)
  -- equal-power-ish curve (1 = scurve-ish in REAPER; 0 = linear)
  reaper.SetMediaItemInfo_Value(item, "C_FADEINSHAPE", 1)
  reaper.SetMediaItemInfo_Value(item, "C_FADEOUTSHAPE", 1)
end

local function coalesce_regions(regions, gap_tol)
  -- Merge overlapping / touching keep spans so carve does not leave micro-holes
  -- or double-fade notches between adjacent ASR phrases.
  gap_tol = gap_tol or 0.02
  if #regions == 0 then return regions end
  table.sort(regions, function(a, b) return a.start < b.start end)
  local out = {
    {
      start = regions[1].start,
      ["end"] = regions[1]["end"],
      name = regions[1].name,
    },
  }
  for i = 2, #regions do
    local r = regions[i]
    local prev = out[#out]
    if r.start <= prev["end"] + gap_tol then
      if r["end"] > prev["end"] then
        prev["end"] = r["end"]
      end
      -- Keep first name; append marker if useful
      if (not prev.name or prev.name == "") and r.name and r.name ~= "" then
        prev.name = r.name
      end
    else
      out[#out + 1] = {
        start = r.start,
        ["end"] = r["end"],
        name = r.name,
      }
    end
  end
  return out
end

local function carve_keeps(full_item, keeps, fade_sec)
  -- Split full_item into keep regions only; delete gaps.
  -- ASR times are file-relative; map through take start offset + playrate + item pos.
  local item_pos = reaper.GetMediaItemInfo_Value(full_item, "D_POSITION")
  local item_len = reaper.GetMediaItemInfo_Value(full_item, "D_LENGTH")
  local item_end = item_pos + item_len
  local track = reaper.GetMediaItem_Track(full_item)
  local take = reaper.GetActiveTake(full_item)
  local soffs = 0.0
  local rate = 1.0
  if take then
    soffs = reaper.GetMediaItemTakeInfo_Value(take, "D_STARTOFFS") or 0.0
    rate = reaper.GetMediaItemTakeInfo_Value(take, "D_PLAYRATE") or 1.0
    if not rate or math.abs(rate) < 1e-9 then rate = 1.0 end
  end

  local function file_to_timeline(t)
    return item_pos + (t - soffs) / rate
  end

  -- File-relative keeps → timeline, clamped to item
  local regions = {}
  for _, k in ipairs(keeps) do
    local a = file_to_timeline(k.start)
    local b = file_to_timeline(k["end"])
    if b < a then a, b = b, a end
    a = math.max(item_pos, a)
    b = math.min(item_end, b)
    if b - a > 0.005 then
      regions[#regions + 1] = { start = a, ["end"] = b, name = k.name }
    end
  end
  regions = coalesce_regions(regions, 0.02)
  if #regions == 0 then
    reaper.DeleteTrackMediaItem(track, full_item)
    return 0
  end

  -- Collect all cut points
  local cuts = {}
  local function add_cut(t)
    if t > item_pos + 1e-6 and t < item_end - 1e-6 then
      cuts[#cuts + 1] = t
    end
  end
  for _, r in ipairs(regions) do
    add_cut(r.start)
    add_cut(r["end"])
  end
  table.sort(cuts)
  -- unique
  local uniq = {}
  for _, t in ipairs(cuts) do
    if #uniq == 0 or math.abs(uniq[#uniq] - t) > 1e-5 then
      uniq[#uniq + 1] = t
    end
  end

  for _, t in ipairs(uniq) do
    -- Find which item contains t (re-scan after each split)
    local cnt = reaper.CountTrackMediaItems(track)
    for i = 0, cnt - 1 do
      local it = reaper.GetTrackMediaItem(track, i)
      local p = reaper.GetMediaItemInfo_Value(it, "D_POSITION")
      local l = reaper.GetMediaItemInfo_Value(it, "D_LENGTH")
      if t > p + 1e-6 and t < p + l - 1e-6 then
        reaper.SplitMediaItem(it, t)
        break
      end
    end
  end

  -- Delete pieces that are not inside any keep region (midpoint test).
  -- Use half-open [start, end) so a boundary midpoint cannot fall into a
  -- microscopic gap between coalesced-adjacent originals.
  local to_delete = {}
  local kept = {}
  local cnt = reaper.CountTrackMediaItems(track)
  for i = 0, cnt - 1 do
    local it = reaper.GetTrackMediaItem(track, i)
    local p = reaper.GetMediaItemInfo_Value(it, "D_POSITION")
    local l = reaper.GetMediaItemInfo_Value(it, "D_LENGTH")
    local mid = p + l * 0.5
    local ok = false
    local match_name = nil
    for _, r in ipairs(regions) do
      if mid >= r.start - 1e-4 and mid < r["end"] + 1e-4 then
        ok = true
        match_name = r.name
        break
      end
    end
    if ok then
      kept[#kept + 1] = { item = it, name = match_name }
    else
      to_delete[#to_delete + 1] = it
    end
  end
  for _, it in ipairs(to_delete) do
    reaper.DeleteTrackMediaItem(track, it)
  end
  for _, k in ipairs(kept) do
    apply_fade(k.item, fade_sec)
    if k.name and k.name ~= "" then
      local kt = reaper.GetActiveTake(k.item)
      if kt then
        reaper.GetSetMediaItemTakeInfo_String(kt, "P_NAME", k.name, true)
      end
    end
  end
  return #kept
end

----------------------------------------------------------------
-- run python
----------------------------------------------------------------

local function tail_text(path, max_chars)
  local data = read_file(path)
  if not data or data == "" then return nil end
  max_chars = max_chars or 1200
  if #data > max_chars then
    return data:sub(#data - max_chars + 1)
  end
  return data
end

local function run_python(py, script_py, wav_path, out_json, model, device, method, min_prefix_words, similarity)
  local err_side = out_json .. ".err.txt"
  -- Remove stale sidecar so we don't show an old failure after a new run.
  if file_exists(err_side) then
    os.remove(err_side)
  end

  -- Merge stderr into stdout: ExecProcess on Windows typically captures stdout only,
  -- and faster-whisper/ctranslate2 exceptions land on stderr.
  local extra = ""
  if method == "similarity" or method == "cluster" or method == "legacy" then
    extra = string.format(" --similarity %s", tostring(similarity))
  elseif method == "prefix" or method == "restart" then
    extra = string.format(" --min-prefix-words %s", tostring(min_prefix_words))
  end
  -- Always pass min-prefix-words for prefix (default method); exact needs none.
  local inner = string.format(
    "%s %s %s -o %s --model %s --device %s --method %s%s --language ru 2>&1",
    quote_win(py),
    quote_win(script_py),
    quote_win(wav_path),
    quote_win(out_json),
    quote_win(model),
    quote_win(device),
    quote_win(method),
    extra
  )
  local cmd = 'cmd.exe /C "' .. inner:gsub('"', '""') .. '"'
  msg("Running: " .. cmd)
  -- Hide window; wait for completion
  local ret = reaper.ExecProcess(cmd, 0)  -- 0 = wait indefinitely on some builds
  -- ExecProcess returns: exitcode\toutput  OR  negative on failure (older API)
  if not ret then
    -- Fallback: os.execute via cmd
    local ok = os.execute(cmd)
    if ok == 0 or ok == true then
      return true, "os.execute ok"
    end
    local side = tail_text(err_side, 1200)
    if side then
      return false, "os.execute failed\n" .. side
    end
    return false, "ExecProcess unavailable and os.execute failed"
  end
  local ret_s = tostring(ret)
  local code = tonumber(ret_s:match("^(%-?%d+)")) or -1
  local captured = ret_s:match("^%-?%d+\t(.*)$") or ret_s
  if code ~= 0 then
    local side = tail_text(err_side, 1200)
    local detail = side or captured
    -- Prefer the ERROR: line if present (also written to stdout by the script).
    local err_line = detail:match("ERROR:[^\r\n]+")
    local body = err_line or detail
    if #body > 900 then body = body:sub(1, 900) .. "…" end
    return false, "python exit " .. tostring(code) .. "\n" .. body
  end
  return true, ret
end

----------------------------------------------------------------
-- main
----------------------------------------------------------------

local function main()
  reaper.ClearConsole()
  msg("=== VO Dedupe (prefix keep-last) ===")

  local item = get_target_item()
  if not item then
    reaper.MB("Select a media item on the source track (or hover it).", "VO Dedupe", 0)
    return
  end

  local src_track = reaper.GetMediaItem_Track(item)
  local wav, err = item_source_path(item)
  if not wav then
    reaper.MB("Source path: " .. tostring(err), "VO Dedupe", 0)
    return
  end
  if not file_exists(wav) then
    reaper.MB("File not found:\n" .. wav, "VO Dedupe", 0)
    return
  end
  msg("Source: " .. wav)

  local script_dir = get_ext("script_dir", "")
  if script_dir == "" then
    script_dir = script_dir_guess() or ""
  end
  local script_py = path_join(script_dir, "vo_dedupe_last.py")
  if not file_exists(script_py) then
    local rv, path = reaper.GetUserFileNameForRead(
      script_dir ~= "" and path_join(script_dir, "vo_dedupe_last.py") or "vo_dedupe_last.py",
      "Find vo_dedupe_last.py",
      "py"
    )
    if not rv or not path or path == "" then
      reaper.MB("vo_dedupe_last.py not found. Set ExtState script_dir.", "VO Dedupe", 0)
      return
    end
    script_py = path
    script_dir = path:match("^(.*)[/\\][^/\\]+$") or script_dir
    set_ext("script_dir", script_dir)
  else
    set_ext("script_dir", script_dir)
  end

  local py = get_ext("python", "python")
  local model = get_ext("model", "base")
  local device = get_ext("device", "auto")
  local method = trim(get_ext("method", "prefix"))
  if method == "" then method = "prefix" end
  -- Migrate stale ExtState from the old exact-only / 4-word defaults.
  local migrated = get_ext("migrated_prefix_default", "")
  if migrated ~= "1" then
    if method == "exact" then
      method = "prefix"
      set_ext("method", "prefix")
      msg("ExtState method was exact → migrated to prefix (new default).")
    end
    local old_mpw = tonumber(get_ext("min_prefix_words", ""))
    if old_mpw and old_mpw > 2 then
      set_ext("min_prefix_words", "2")
      msg("ExtState min_prefix_words " .. tostring(old_mpw) .. " → 2 (new default).")
    end
    set_ext("migrated_prefix_default", "1")
  end
  local min_prefix_words = tonumber(get_ext("min_prefix_words", "2")) or 2
  if min_prefix_words < 2 then min_prefix_words = 2 end
  local similarity = tonumber(get_ext("similarity", "0.72")) or 0.72
  local fade_ms = tonumber(get_ext("fade_ms", tostring(FADE_DEFAULT_MS))) or FADE_DEFAULT_MS
  local fade_sec = fade_ms / 1000.0

  local out_json = wav:gsub("%.[^%.\\/]+$", "") .. ".vo_dedupe.json"
  -- if no extension match, append
  if out_json == wav then
    out_json = wav .. ".vo_dedupe.json"
  end

  reaper.Undo_BeginBlock()
  reaper.PreventUIRefresh(1)

  local ok, detail = run_python(
    py, script_py, wav, out_json, model, device, method, min_prefix_words, similarity
  )
  if not ok then
    reaper.PreventUIRefresh(-1)
    reaper.Undo_EndBlock("VO Dedupe (failed)", -1)
    reaper.MB("Python failed:\n" .. tostring(detail), "VO Dedupe", 0)
    return
  end

  local json_text = read_file(out_json)
  if not json_text then
    reaper.PreventUIRefresh(-1)
    reaper.Undo_EndBlock("VO Dedupe (no json)", -1)
    reaper.MB("JSON not written:\n" .. out_json, "VO Dedupe", 0)
    return
  end

  local keeps = parse_keeps_json(json_text)
  msg(string.format("Parsed %d KEEP regions from %s", #keeps, out_json))
  if #keeps == 0 then
    reaper.PreventUIRefresh(-1)
    reaper.Undo_EndBlock("VO Dedupe (0 keeps)", -1)
    reaper.MB("0 KEEP regions — check ASR / language.", "VO Dedupe", 0)
    return
  end

  local keeps_tr = ensure_keeps_track(src_track)
  clear_track_items(keeps_tr)
  local copy = copy_item_to_track(item, keeps_tr)
  local n = carve_keeps(copy, keeps, fade_sec)

  -- Mute source track? Leave intact and unmuted — user decides. Optional: mute source.
  -- reaper.SetMediaTrackInfo_Value(src_track, "B_MUTE", 1)

  reaper.PreventUIRefresh(-1)
  reaper.UpdateArrange()
  reaper.Undo_EndBlock(
    string.format("VO Dedupe (%d keeps)", n),
    -1
  )
  msg(string.format("Done: %d keep clips on track «%s». Source track untouched.", n, TRACK_KEEPS_NAME))
  reaper.MB(
    string.format(
      "Готово.\nKEEP клипов: %d\nТрек: %s\nJSON: %s\nИсходный трек не изменён.",
      n, TRACK_KEEPS_NAME, out_json
    ),
    "VO Dedupe",
    0
  )
end

main()
