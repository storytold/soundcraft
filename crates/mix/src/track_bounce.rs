//! Bounded, source-only offline Track Bounce. Never calls full-session plugin sync.

use super::*;

const BLOCK: usize = 1024;
/// Policy cap on the conservative PCM/codec/workspace estimate; not an allocator guarantee.
pub const MAX_BOUNCE_BYTES: usize = 512 << 20;

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub volume: bool,
    pub pan: bool,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub track: TrackId,
    pub range: Range,
    pub capture_end: Samples,
    pub format: ChannelFormat,
    pub frames: usize,
    pub processing_frames: usize,
    pub memory_bytes: usize,
    pub options: Options,
}

fn add(a: usize, b: usize) -> std::result::Result<usize, String> {
    a.checked_add(b).ok_or_else(|| "export allocation overflow".into())
}

fn mul(a: usize, b: usize) -> std::result::Result<usize, String> {
    a.checked_mul(b).ok_or_else(|| "export allocation overflow".into())
}

fn builtin(ins: &soundcraft_model::Insert, instrument: bool) -> std::result::Result<(), String> {
    let info = soundcraft_dsp::plugin_info(&ins.plugin).ok_or_else(|| format!("unsupported plugin {} (built-ins only)", ins.plugin))?;
    if info.is_instrument != instrument {
        return Err("plugin is in an unsupported instrument/effect slot".into());
    }
    if ins.params.len() > info.params.len() || ins.params.iter().any(|(k, v)| !v.is_finite() || info.param(k).is_none()) {
        return Err("unknown or nonfinite plugin parameter".into());
    }
    if matches!(ins.plugin.as_str(), "pitch_shifter" | "maximizer")
        || (ins.plugin == "compressor" && ins.params.get("lookahead").is_some_and(|v| *v != 0.0))
    {
        return Err("latency-producing inserts are not supported by source-only Track Bounce yet".into());
    }
    Ok(())
}

fn audio(s: &Session, id: soundcraft_model::SourceId, session_rate: bool) -> std::result::Result<(), String> {
    let a = s.pool.get(id).ok_or_else(|| format!("audio source {} is not loaded", id.0))?;
    let b = &a.buffer;
    if !(8_000..=768_000).contains(&b.sample_rate)
        || (session_rate && b.sample_rate != s.sample_rate.hz())
        || b.channels.is_empty()
        || b.channels.len() > MAX_CHANNELS
        || b.frames() == 0
        || b.channels.iter().any(|c| c.len() != b.frames())
    {
        return Err("loaded source has an unsupported rate or channel/frame shape".into());
    }
    Ok(())
}

fn checked_midi_offset(s: &Session, t: &Track) -> std::result::Result<Samples, String> {
    let value = s.edit.values.get(&format!("midi.offset.{}", t.id.0)).copied().unwrap_or(0.0);
    let limit = s.sample_rate.samples(10.0);
    if !value.is_finite() || value < -(limit as f64) || value > limit as f64 {
        return Err("MIDI track offset must be finite and within the ten-second playback limit".into());
    }
    // The realtime scheduler uses the same sample-valued setting, truncated to whole samples.
    Ok(value as Samples)
}

/// Last audible source edge, in the same timeline coordinates as realtime note scheduling.
pub fn source_end(s: &Session, t: &Track) -> std::result::Result<Samples, String> {
    validate_metadata(s, t)?;
    let offset = if t.kind == TrackKind::Instrument { checked_midi_offset(s, t)? } else { 0 };
    let mut end = 0;
    for c in t.clips().iter().filter(|c| !c.muted) {
        let ce = c.start.checked_add(c.length).filter(|_| c.start >= 0 && c.length > 0).ok_or_else(|| "invalid source clip range".to_string())?;
        if t.kind == TrackKind::Instrument {
            if let ClipContent::Midi { sequence } = &c.content {
                let base = s.tempo.samples_to_ticks(c.start, s.sample_rate);
                let duration = s.edit.value(&format!("rtp.{}.duration", t.id.0), 100.0).clamp(1.0, 1000.0) / 100.0;
                let delay = s.edit.value(&format!("rtp.{}.delay", t.id.0), 0.0).clamp(-1e7, 1e7) as i64;
                for n in &sequence.notes {
                    let on = base.saturating_add(n.start.saturating_add(delay).max(0));
                    let off = on.saturating_add(((n.length as f64 * duration) as i64).max(1));
                    if s.tempo.tick_to_samples(on, s.sample_rate) < ce {
                        let edge = s.tempo.tick_to_samples(off, s.sample_rate).min(ce);
                        end = end.max(edge.checked_add(offset).ok_or_else(|| "MIDI source end overflow".to_string())?);
                    }
                }
            }
        } else {
            end = end.max(ce);
        }
    }
    Ok(end)
}

/// Bound metadata scanned or copied by the offline renderer before source-end calculation.
pub fn validate_metadata(s: &Session, t: &Track) -> std::result::Result<(), String> {
    if t.mixer.inserts.len() > soundcraft_model::INSERT_SLOTS || t.automation.len() > 1024 || t.clips().len() > 10_000 {
        return Err("source has too many inserts, lanes or clips for bounded Track Bounce".into());
    }
    if !t.mixer.trim_db.is_finite()
        || !t.mixer.volume_db.is_finite()
        || t.mixer.pan.len() > MAX_CHANNELS
        || t.mixer.pan.iter().any(|x| !x.is_finite())
        || t.mixer.surround.as_ref().is_some_and(|x| ![x.x, x.y, x.z, x.divergence, x.center, x.lfe_db].iter().all(|v| v.is_finite()))
    {
        return Err("nonfinite source gain/pan metadata".into());
    }
    let mut notes = 0usize;
    let mut envelope_points = 0usize;
    for c in t.clips() {
        envelope_points = add(envelope_points, c.gain_env.len())?;
        if envelope_points > 100_000
            || !c.gain_db.is_finite()
            || c.fade_in.len < 0
            || c.fade_out.len < 0
            || c.gain_env.iter().any(|(at, value)| *at < 0 || *at > c.length || !value.is_finite())
            || c.gain_env.windows(2).any(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| a.0 >= b.0))
        {
            return Err("invalid or unbounded clip gain/fade metadata".into());
        }
        if let ClipContent::Midi { sequence } = &c.content {
            notes = add(notes, sequence.notes.len())?;
            if notes > 128 {
                return Err("Instrument Track Bounce currently supports at most 128 notes per source (bounded event queue)".into());
            }
            if !sequence.ctrls.is_empty() {
                return Err("MIDI controller events are not rendered by this Instrument bounce subset".into());
            }
        }
    }
    for track in std::iter::once(t).chain(t.mixer.vca.and_then(|v| s.tracks.iter().find(|x| x.id.0 == v && x.kind == TrackKind::Vca))) {
        if !track.mixer.volume_db.is_finite() || track.automation.len() > 1024 {
            return Err("invalid VCA/source automation metadata".into());
        }
        let mut points = 0usize;
        for lane in &track.automation {
            points = add(points, lane.points.len())?;
            if points > 100_000 {
                return Err("too many automation points for bounded Track Bounce".into());
            }
            if lane.points.iter().any(|p| p.at < 0 || !p.value.is_finite())
                || lane.points.windows(2).any(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| a.at >= b.at))
                || matches!(&lane.param, AutoParam::Plugin { param, .. } if param.len() > 128)
            {
                return Err("invalid or unbounded automation metadata".into());
            }
        }
    }
    let tempos = s.tempo.tempos();
    if tempos.len() > 1024
        || tempos.first().is_none_or(|x| x.tick != 0)
        || tempos.iter().any(|x| x.tick < 0 || !x.bpm.is_finite() || !(5.0..=1000.0).contains(&x.bpm))
        || tempos.windows(2).any(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| a.tick >= b.tick))
    {
        return Err("unsupported tempo-map metadata".into());
    }
    for key in ["velocity", "transpose", "duration", "delay"] {
        if s.edit.values.get(&format!("rtp.{}.{key}", t.id.0)).is_some_and(|value| !value.is_finite()) {
            return Err("nonfinite MIDI real-time property".into());
        }
    }
    Ok(())
}

/// Pure metadata preflight. No plugin creation, source copy, render or filesystem action.
pub fn plan(s: &Session, id: TrackId, range: Range, capture_end: Samples, options: Options) -> std::result::Result<Plan, String> {
    let t = s.track(id).ok_or_else(|| "unknown track".to_string())?;
    if !matches!(t.kind, TrackKind::Audio | TrackKind::Instrument) {
        return Err("Track Bounce currently supports Audio and built-in Instrument tracks; Aux/Master/MIDI/Folder/VCA are unsupported".into());
    }
    if t.inactive || t.mixer.record_arm || t.mixer.input_monitor {
        return Err("inactive, record-armed or input-monitored sources cannot be bounced offline".into());
    }
    if !(8_000..=768_000).contains(&s.sample_rate.hz()) {
        return Err("unsupported session sample rate".into());
    }
    validate_metadata(s, t)?;
    let strip_ch = strip_channels(t, 2);
    if t.channels() > MAX_CHANNELS || range.start < 0 || range.end <= range.start || capture_end <= range.start || capture_end > range.end {
        return Err("unsupported track width or empty/negative range".into());
    }
    if !options.pan && t.kind == TrackKind::Instrument && t.channels() == 1 {
        return Err("mono Instrument Pan-off requires an explicit stereo-to-mono policy; use Pan-on or a stereo Instrument".into());
    }
    if t.kind == TrackKind::Instrument {
        checked_midi_offset(s, t)?;
    }
    let format = if options.pan {
        match t.mixer.output {
            Route::Main => {
                if s.outputs.first().is_some_and(|o| o.format.channels() > MAX_CHANNELS) {
                    return Err("main output exceeds the 16-channel renderer".into());
                }
                s.main_format()
            }
            Route::Bus(b) => s.bus(b).map(|b| b.format).ok_or_else(|| "output bus is missing".to_string())?,
            _ => return Err("Pan-on requires a single Main or internal Bus output; hardware/None outputs are unsupported".into()),
        }
    } else {
        t.format
    };
    if format.channels() > MAX_CHANNELS {
        return Err("output width exceeds the 16-channel renderer".into());
    }
    let frames = usize::try_from(range.end.checked_sub(range.start).ok_or_else(|| "range overflow".to_string())?)
        .map_err(|_| "range exceeds platform size".to_string())?;
    let width = strip_ch.max(format.channels()).max(2);
    // A fresh instrument must play earlier notes to establish voices at selection start.
    // A negative playback offset can put those note-ons before session zero.
    // Preroll is discarded block-by-block; bound its CPU span as well as delivered PCM.
    let processing_frames = if t.kind == TrackKind::Instrument {
        let start = checked_midi_offset(s, t)?.min(0);
        usize::try_from(range.end.checked_sub(start).ok_or_else(|| "instrument preroll overflow".to_string())?)
            .map_err(|_| "instrument preroll exceeds platform size".to_string())?
    } else {
        frames
    };
    if processing_frames > MAX_RENDER_SAMPLES / width {
        return Err("range exceeds the offline render limit".into());
    }
    let mut plugins = 0usize;
    for ins in t.mixer.inserts.iter().flatten().filter(|i| i.active && !i.bypass) {
        builtin(ins, false)?;
        plugins += 1;
    }
    if t.automation.iter().any(|l| matches!(&l.param, AutoParam::Plugin { param, .. } if param == "lookahead") && !l.points.is_empty()) {
        return Err("automated insert lookahead is unsupported".into());
    }
    if t.kind == TrackKind::Instrument {
        let ins = t.instrument.as_ref().filter(|i| i.active && !i.bypass).ok_or_else(|| "an active built-in instrument is required".to_string())?;
        builtin(ins, true)?;
        if ins.plugin == "sampler" {
            audio(s, ins.sample.ok_or_else(|| "Sampler has no loaded sample".to_string())?, false)?;
        }
        plugins += 1;
    }
    // MaxDelay currently reserves 2.01 seconds/channel; the other registered processors use
    // less. Four seconds/channel + fixed overhead covers preparation (including the initial
    // 48 kHz preparation), parameter state and channel-strip sub-processors conservatively.
    // This is a policy estimate of current built-ins, not a formal bound on allocator overhead.
    let per_plugin = add(mul(mul(s.sample_rate.hz().max(48_000) as usize, strip_ch)?, 16)?, 1 << 20)?;
    let mut workspace = mul(plugins, per_plugin)?;
    for c in t.clips() {
        if c.length <= 0 || c.start < 0 || c.start.checked_add(c.length).is_none() || !c.stretch.is_finite() || c.stretch <= 0.0 {
            return Err("invalid clip range/stretch".into());
        }
        match (&c.content, t.kind) {
            (ClipContent::Audio { source, offset }, TrackKind::Audio) if !c.muted => {
                audio(s, *source, true)?;
                let frames = s.pool.get(*source).map_or(0, |a| a.buffer.frames());
                let last = *offset as f64 + (c.length - 1) as f64 / c.stretch;
                if *offset < 0 || !last.is_finite() || last.floor() >= frames as f64 {
                    return Err("audio clip exceeds loaded source coverage".into());
                }
            }
            (ClipContent::Audio { .. }, TrackKind::Audio) => {}
            (ClipContent::Midi { sequence }, TrackKind::Instrument) => {
                if c.stretch != 1.0 {
                    return Err("MIDI clip stretch is unsupported by this Instrument bounce subset".into());
                }
                if sequence.notes.iter().any(|n| {
                    n.pitch > 127
                        || n.velocity == 0
                        || n.velocity > 127
                        || n.channel > 15
                        || n.release_velocity > 127
                        || n.start < 0
                        || n.length <= 0
                        || n.start.checked_add(n.length).is_none()
                }) {
                    return Err("invalid MIDI note range/value".into());
                }
            }
            _ => return Err("track contains unsupported clip content".into()),
        }
        let prefix = format!("clip_fx.{}.", c.id.0);
        let mut fx_count = 0usize;
        let mut fx_bytes = 0usize;
        for (key, value) in s.edit.values.range(prefix.clone()..).take_while(|(k, _)| k.starts_with(&prefix)) {
            fx_count += 1;
            fx_bytes = add(fx_bytes, key.len())?;
            if fx_count > 64 || fx_bytes > 8192 {
                return Err("Clip Effects metadata exceeds the bounded export limit".into());
            }
            if !value.is_finite() || (key.ends_with("comp.lookahead") && *value != 0.0) {
                return Err("nonfinite Clip Effects or clip compressor lookahead is unsupported".into());
            }
        }
        if fx_count > 0 {
            workspace = add(workspace, add(mul(strip_ch, add(s.sample_rate.hz() as usize / 25, 1 << 17)?)?, fx_bytes)?)?;
        }
    }
    // Every note may enqueue both edges in one block. The registered instruments currently
    // have a fixed 512-event queue; keep the whole source below that capacity conservatively.
    // PCM + worst-case integer/float codec output; extra block buffers, strip state, and
    // lazy per-clip effect buffers. No latency extension is allocated in this first subset.
    let memory_bytes = add(add(mul(mul(processing_frames, format.channels())?, 12)?, workspace)?, mul(width, BLOCK * 32)?)?;
    if memory_bytes > MAX_BOUNCE_BYTES {
        return Err("PCM, codec and plugin workspaces exceed the 512 MiB Track Bounce allocation budget".into());
    }
    Ok(Plan { track: id, range, capture_end, format, frames, processing_frames, memory_bytes, options })
}

fn planar(ch: usize, frames: usize) -> std::result::Result<Vec<Vec<f32>>, String> {
    let mut out = Vec::new();
    out.try_reserve_exact(ch).map_err(|_| "cannot allocate channel list".to_string())?;
    for _ in 0..ch {
        let mut c = Vec::new();
        c.try_reserve_exact(frames).map_err(|_| "cannot allocate rendered channel".to_string())?;
        c.resize(frames, 0.0);
        out.push(c);
    }
    Ok(out)
}

/// Render only the selected strip. Other tracks and their plugin slots are never instantiated,
/// even if they are inactive. Session/VCA metadata is read immutably, not cloned into a mixer.
pub fn render(s: &Session, p: &Plan) -> std::result::Result<Vec<Vec<f32>>, String> {
    // Plans are public values: revalidate before any plugin/workspace/PCM allocation.
    let checked = plan(s, p.track, p.range, p.capture_end, p.options)?;
    if checked.format != p.format || checked.frames != p.frames || checked.processing_frames != p.processing_frames {
        return Err("Track Bounce plan changed".into());
    }
    let t = s.track(p.track).ok_or_else(|| "source disappeared".to_string())?;
    let ch = strip_channels(t, 2);
    let mut strip = Strip::new();
    strip.skip_volume = !p.options.volume;
    strip.buf = planar(ch, BLOCK)?;
    strip.scratch = vec![0.0; BLOCK];
    for ins in &t.mixer.inserts {
        strip.plugins.push(match ins.as_ref().filter(|i| i.active && !i.bypass) {
            Some(ins) => Some(slot(ins, s.sample_rate.as_f64() as f32, ch)?),
            None => None,
        });
    }
    if let Some(ins) = t.instrument.as_ref().filter(|_| t.kind == TrackKind::Instrument) {
        strip.instrument = Some(slot(ins, s.sample_rate.as_f64() as f32, ch)?);
        sync_sample(&mut strip, Some(ins), s);
    }
    let mut out = planar(p.format.channels(), p.frames)?;
    let empty_busses = HashMap::new();
    let live = LiveInput { input: &[], monitor_only: false, recording: false };
    let mut routed = planar(p.format.channels(), BLOCK)?;
    let processing_start = if t.kind == TrackKind::Instrument { checked_midi_offset(s, t)?.min(0) } else { p.range.start };
    let mut done = 0usize;
    while done < p.processing_frames {
        let mut n = (p.processing_frames - done).min(BLOCK);
        let pos = processing_start.checked_add(done as i64).ok_or_else(|| "render position overflow".to_string())?;
        if pos < p.capture_end {
            n = n.min((p.capture_end - pos) as usize);
        }
        if pos == p.capture_end {
            if let Some(inst) = strip.instrument.as_mut() {
                for pitch in strip.held.drain(..) {
                    inst.plugin.note_off(0, pitch);
                }
            }
            strip.skip_source = true;
        }
        if !strip.skip_source {
            release_clip_boundary(s, t, &mut strip, pos);
        }
        process_strip(s, t, &mut strip, &empty_busses, &live, pos, n, false);
        // Realtime mixing silences a failed plugin's block. File delivery must still fail,
        // rather than publish that contained silence as a successful source render.
        if strip.failures.count != 0 {
            return Err("renderer returned nonfinite plugin audio; no file delivered".into());
        }
        if strip.latency != 0 {
            return Err("insert latency is unsupported by source-only Track Bounce".into());
        }
        if p.options.pan {
            for c in &mut routed {
                c.fill(0.0);
            }
            let pan = PanSpec { stereo: [pan_at(t, 0, pos), pan_at(t, 1, pos)], surround: t.mixer.surround.as_ref() };
            let matrix = build_matrix(strip_format(t, ch), ch, p.format, routed.len(), &pan, PanLaw::Minus3);
            apply_matrix(&strip.buf, n, &mut routed, &matrix, 1.0);
        }
        let source = if p.options.pan { &routed } else { &strip.buf };
        let block_end = pos.checked_add(n as i64).ok_or_else(|| "render position overflow".to_string())?;
        let keep_start = pos.max(p.range.start);
        if block_end > keep_start {
            let dst_start = usize::try_from(keep_start - p.range.start).map_err(|_| "invalid output offset".to_string())?;
            let src_start = usize::try_from(keep_start - pos).map_err(|_| "invalid strip offset".to_string())?;
            let count = n - src_start;
            for (dst, src) in out.iter_mut().zip(source.iter()) {
                let d = dst.get_mut(dst_start..dst_start + count).ok_or_else(|| "incomplete output channel".to_string())?;
                let a = src.get(src_start..src_start + count).ok_or_else(|| "incomplete strip channel".to_string())?;
                for (d, a) in d.iter_mut().zip(a) {
                    if !a.is_finite() {
                        return Err("renderer returned nonfinite audio; no file delivered".into());
                    }
                    *d = *a;
                }
            }
        }
        done += n;
    }
    Ok(out)
}

// Shared realtime scheduling does not visit a clip after its half-open range ends. An edge
// exactly at the start of this block therefore needs an explicit note-off for an offline tail.
fn release_clip_boundary(s: &Session, t: &Track, strip: &mut Strip, pos: Samples) {
    let Some(inst) = strip.instrument.as_mut() else { return };
    // schedule_notes subtracts this sample offset before inspecting clip/note coordinates.
    let pos = pos.saturating_sub(midi_offset(s, t));
    let rtp = |key: &str, default: f64| s.edit.value(&format!("rtp.{}.{key}", t.id.0), default);
    let transpose = rtp("transpose", 0.0).clamp(-127.0, 127.0) as i64;
    let duration = rtp("duration", 100.0).clamp(1.0, 1000.0) / 100.0;
    let delay = rtp("delay", 0.0).clamp(-1e7, 1e7) as i64;
    for c in t.clips().iter().filter(|c| !c.muted && c.end() == pos) {
        if let ClipContent::Midi { sequence } = &c.content {
            let base = s.tempo.samples_to_ticks(c.start, s.sample_rate);
            for note in &sequence.notes {
                let on = base.saturating_add(note.start.saturating_add(delay).max(0));
                let off = on.saturating_add(((note.length as f64 * duration) as i64).max(1));
                if s.tempo.tick_to_samples(on, s.sample_rate) >= c.end() || s.tempo.tick_to_samples(off, s.sample_rate).min(c.end()) != pos {
                    continue;
                }
                let pitch = (i64::from(note.pitch) + transpose).clamp(0, 127) as u8;
                if strip.held.contains(&pitch) {
                    inst.plugin.note_off(0, pitch);
                    strip.held.retain(|p| *p != pitch);
                }
            }
        }
    }
}

fn slot(ins: &soundcraft_model::Insert, sr: f32, ch: usize) -> std::result::Result<PluginSlot, String> {
    // Built-in factory only. No hosted fallback and no opaque licensed state load.
    let mut plugin = soundcraft_dsp::create(&ins.plugin).ok_or_else(|| "built-in plugin unavailable".to_string())?;
    plugin.prepare(sr, BLOCK, ch);
    Ok(PluginSlot { id: ins.plugin.clone(), ch, plugin, applied: Vec::new(), failed: false })
}
