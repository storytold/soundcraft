use super::{ChannelFormat, MAX_CHANNELS, PanLaw, Speaker, SpeakerPos, SurroundPan, SurroundParams, db_to_gain, gains};

/// Per-block routing gains: `m[src][dst]`.
type Matrix = [[f32; MAX_CHANNELS]; MAX_CHANNELS];

/// How a source is panned: the stereo pan pair (mono uses `[0]`) and the optional surround pan.
pub(crate) struct PanSpec<'a> {
    pub(crate) stereo: [f32; 2],
    pub(crate) surround: Option<&'a SurroundPan>,
}

pub(crate) fn identity_matrix(n: usize) -> Matrix {
    let mut m = [[0.0; MAX_CHANNELS]; MAX_CHANNELS];
    for (i, row) in m.iter_mut().enumerate().take(n) {
        if let Some(g) = row.get_mut(i) {
            *g = 1.0;
        }
    }
    m
}

/// ITU-R BS.775 stereo fold-down gains (to L, to R) for one speaker: C and surrounds at -3 dB,
/// the centre surround at -6 dB per side, LFE dropped.
pub fn itu_stereo_fold(sp: Speaker) -> (f32, f32) {
    use Speaker::*;
    let h = std::f32::consts::FRAC_1_SQRT_2;
    match sp {
        L | Lc | Lw => (1.0, 0.0),
        R | Rc | Rw => (0.0, 1.0),
        C => (h, h),
        Lfe => (0.0, 0.0),
        Cs => (0.5, 0.5),
        Ls | Lss | Lrs | Ltf | Ltm | Ltr => (h, 0.0),
        Rs | Rss | Rrs | Rtf | Rtm | Rtr => (0.0, h),
    }
}

/// Speaker directions of a format (for the DSP panner). Returns the count written.
fn layout_of(fmt: ChannelFormat, out: &mut [SpeakerPos; MAX_CHANNELS]) -> usize {
    let sp = fmt.speakers();
    for (o, s) in out.iter_mut().zip(sp.iter()) {
        *o = SpeakerPos { az: s.azimuth(), el: s.elevation(), lfe: s.is_lfe() };
    }
    sp.len().min(MAX_CHANNELS)
}

fn surround_params(sp: &SurroundPan, dx: f32) -> SurroundParams {
    SurroundParams { x: (sp.x + dx).clamp(-1.0, 1.0), y: sp.y, z: sp.z, divergence: sp.divergence, center: sp.center / 100.0 }
}

/// One source → destination routing: channel counts and formats, plus how the source is panned.
struct Link<'a> {
    src_fmt: ChannelFormat,
    ns: usize,
    dst_fmt: ChannelFormat,
    nd: usize,
    pan: &'a PanSpec<'a>,
    law: PanLaw,
}

fn add_gain(m: &mut Matrix, s: usize, d: usize, g: f32) {
    if let Some(x) = m.get_mut(s).and_then(|r| r.get_mut(d)) {
        *x += g;
    }
}

/// Routing gains from a source (`ns` channels of `src_fmt`) into a destination (`nd` channels of
/// `dst_fmt`). See the crate docs for the rules.
pub(crate) fn build_matrix(src_fmt: ChannelFormat, ns: usize, dst_fmt: ChannelFormat, nd: usize, pan: &PanSpec<'_>, law: PanLaw) -> Matrix {
    let mut m = [[0.0f32; MAX_CHANNELS]; MAX_CHANNELS];
    let (ns, nd) = (ns.min(MAX_CHANNELS), nd.min(MAX_CHANNELS));
    if ns == 0 || nd == 0 {
        return m;
    }
    if nd == 1 {
        // Mono destination: the stereo routing summed at -6 dB.
        let st = build_matrix(src_fmt, ns, ChannelFormat::Stereo, 2, pan, law);
        for (row, srow) in m.iter_mut().zip(st.iter()).take(ns) {
            row[0] = 0.5 * (srow[0] + srow[1]);
        }
        return m;
    }
    let link = Link { src_fmt, ns, dst_fmt, nd, pan, law };
    let src_amb = src_fmt.is_ambisonic() && ns >= 4;
    if nd == 2 && !dst_fmt.is_ambisonic() {
        return stereo_dest(&link, src_amb);
    }
    if ns > 2 && src_fmt == dst_fmt && ns == nd {
        return identity_matrix(ns);
    }
    if dst_fmt.is_ambisonic() {
        return ambisonic_dest(&link, src_amb);
    }
    layout_dest(&link, src_amb)
}

/// Routing into a stereo destination.
fn stereo_dest(l: &Link<'_>, src_amb: bool) -> Matrix {
    let mut m = [[0.0f32; MAX_CHANNELS]; MAX_CHANNELS];
    let src_sp = l.src_fmt.speakers();
    match l.ns {
        1 => {
            let (gl, gr) = gains(l.pan.stereo[0], l.law);
            m[0][0] = gl;
            m[0][1] = gr;
        }
        2 if !src_amb => {
            let (a_l, a_r) = gains(l.pan.stereo[0], PanLaw::Zero);
            let (b_l, b_r) = gains(l.pan.stereo[1], PanLaw::Zero);
            m[0] = [0.0; MAX_CHANNELS];
            m[0][0] = a_l;
            m[0][1] = a_r;
            m[1][0] = b_l;
            m[1][1] = b_r;
        }
        _ if src_amb => {
            for (d, az) in [(0usize, -30.0f32), (1, 30.0)] {
                let g = soundcraft_dsp::pan::ambisonic_decode(az, 0.0, 2);
                for (c, gc) in g.iter().enumerate() {
                    add_gain(&mut m, c, d, *gc);
                }
            }
        }
        ns if src_sp.len() == ns => {
            for (s, sp) in src_sp.iter().enumerate() {
                let (left, right) = itu_stereo_fold(*sp);
                add_gain(&mut m, s, 0, left);
                add_gain(&mut m, s, 1, right);
            }
        }
        ns => {
            // Unknown layout: first two channels to L/R, the rest to the centre.
            let h = std::f32::consts::FRAC_1_SQRT_2;
            m[0][0] = 1.0;
            m[1][1] = 1.0;
            for row in m.iter_mut().take(ns).skip(2) {
                row[0] = h;
                row[1] = h;
            }
        }
    }
    m
}

/// `±1` offset spreading a stereo source's two channels left and right of the surround pan.
fn spread_dx(ns: usize, s: usize) -> f32 {
    if ns != 2 {
        0.0
    } else if s == 0 {
        -1.0
    } else {
        1.0
    }
}

/// Routing into an ambisonic destination.
fn ambisonic_dest(l: &Link<'_>, src_amb: bool) -> Matrix {
    let mut m = [[0.0f32; MAX_CHANNELS]; MAX_CHANNELS];
    let (ns, nd) = (l.ns, l.nd);
    if src_amb || (l.src_fmt.is_ambisonic() && ns > 2) {
        return identity_matrix(ns.min(nd));
    }
    let src_sp = l.src_fmt.speakers();
    let mut g = [0.0f32; MAX_CHANNELS];
    let mut enc = |m: &mut Matrix, s: usize, az: f32, el: f32, spread: f32| {
        soundcraft_dsp::pan::ambisonic_encode(az, el, spread, &mut g);
        for (d, gd) in g.iter().enumerate().take(nd) {
            add_gain(m, s, d, *gd);
        }
    };
    match (ns, l.pan.surround) {
        (1 | 2, Some(sp)) => {
            for s in 0..ns {
                let p = surround_params(sp, spread_dx(ns, s));
                enc(&mut m, s, soundcraft_dsp::pan::puck_azimuth(&[], p.x, p.y), p.z * 90.0, p.divergence);
            }
        }
        (1, None) => enc(&mut m, 0, l.pan.stereo[0].clamp(-1.0, 1.0) * 30.0, 0.0, 0.0),
        (2, None) => {
            enc(&mut m, 0, -30.0, 0.0, 0.0);
            enc(&mut m, 1, 30.0, 0.0, 0.0);
        }
        _ => {
            for (s, sp) in src_sp.iter().enumerate().take(ns) {
                if !sp.is_lfe() {
                    enc(&mut m, s, sp.azimuth(), sp.elevation(), 0.0);
                }
            }
        }
    }
    m
}

/// A surround-panned mono or stereo source into a speaker layout, plus its LFE send.
fn surround_into_layout(m: &mut Matrix, l: &Link<'_>, layout: &[SpeakerPos], sp: &SurroundPan) {
    let (ns, nd) = (l.ns, l.nd);
    let dst_sp = l.dst_fmt.speakers();
    let mut g = [0.0f32; MAX_CHANNELS];
    for s in 0..ns {
        soundcraft_dsp::pan::surround_gains(layout, &surround_params(sp, spread_dx(ns, s)), &mut g);
        for (d, gd) in g.iter().enumerate().take(nd) {
            add_gain(m, s, d, *gd);
        }
        if let Some(lfe) = dst_sp.iter().position(|x| *x == Speaker::Lfe).filter(|i| *i < nd)
            && sp.lfe_db > -143.9
        {
            add_gain(m, s, lfe, db_to_gain(sp.lfe_db.min(12.0)) / ns as f32);
        }
    }
}

/// Routing into a speaker layout other than stereo or ambisonic.
fn layout_dest(l: &Link<'_>, src_amb: bool) -> Matrix {
    let mut m = [[0.0f32; MAX_CHANNELS]; MAX_CHANNELS];
    let (ns, nd) = (l.ns, l.nd);
    let src_sp = l.src_fmt.speakers();
    let mut layout = [SpeakerPos::default(); MAX_CHANNELS];
    let nl = layout_of(l.dst_fmt, &mut layout);
    let layout = layout.get(..nl).unwrap_or(&[]);
    let dst_sp = l.dst_fmt.speakers();
    if dst_sp.len() != nd {
        // No layout to pan on: channel for channel.
        return identity_matrix(ns.min(nd));
    }
    let find = |sp: Speaker| dst_sp.iter().position(|x| *x == sp).filter(|i| *i < nd);
    let (li, ri) = (find(Speaker::L), find(Speaker::R));
    let mut g = [0.0f32; MAX_CHANNELS];
    match (ns, l.pan.surround) {
        (1 | 2, Some(sp)) if !src_amb => surround_into_layout(&mut m, l, layout, sp),
        (1, None) => {
            let (gl, gr) = gains(l.pan.stereo[0], l.law);
            if let (Some(left), Some(right)) = (li, ri) {
                add_gain(&mut m, 0, left, gl);
                add_gain(&mut m, 0, right, gr);
            }
        }
        (2, None) if !src_amb => {
            let (a_l, a_r) = gains(l.pan.stereo[0], PanLaw::Zero);
            let (b_l, b_r) = gains(l.pan.stereo[1], PanLaw::Zero);
            if let (Some(left), Some(right)) = (li, ri) {
                add_gain(&mut m, 0, left, a_l);
                add_gain(&mut m, 0, right, a_r);
                add_gain(&mut m, 1, left, b_l);
                add_gain(&mut m, 1, right, b_r);
            }
        }
        _ if src_amb => {
            let n_spk = dst_sp.iter().filter(|s| !s.is_lfe()).count();
            for (d, sp) in dst_sp.iter().enumerate().take(nd) {
                if sp.is_lfe() {
                    continue;
                }
                let dec = soundcraft_dsp::pan::ambisonic_decode(sp.azimuth(), sp.elevation(), n_spk);
                for (c, gc) in dec.iter().enumerate() {
                    add_gain(&mut m, c, d, *gc);
                }
            }
        }
        _ if src_sp.len() == ns => {
            for (s, sp) in src_sp.iter().enumerate() {
                if let Some(d) = find(*sp) {
                    add_gain(&mut m, s, d, 1.0);
                } else if !sp.is_lfe() {
                    // (An LFE with no LFE in the destination is dropped.)
                    soundcraft_dsp::pan::direction_gains(layout, sp.azimuth(), sp.elevation(), &mut g);
                    for (d, gd) in g.iter().enumerate().take(nd) {
                        add_gain(&mut m, s, d, *gd);
                    }
                }
            }
        }
        _ => return identity_matrix(ns.min(nd)),
    }
    m
}

/// `dst[d] += Σ_s src[s] × gain × m[s][d]` for every destination channel (no allocation).
pub(crate) fn apply_matrix(src: &[Vec<f32>], frames: usize, dst: &mut [Vec<f32>], m: &Matrix, gain: f32) {
    apply_matrix_span(src, 0..frames, dst, m, gain);
}

/// [`apply_matrix`] over the frames in `span` only.
pub(crate) fn apply_matrix_span(src: &[Vec<f32>], span: std::ops::Range<usize>, dst: &mut [Vec<f32>], m: &Matrix, gain: f32) {
    let ns = src.len().min(MAX_CHANNELS);
    for (d, out) in dst.iter_mut().enumerate().take(MAX_CHANNELS) {
        let mut idx = [(0usize, 0.0f32); MAX_CHANNELS];
        let mut k = 0;
        for (s, row) in m.iter().enumerate().take(ns) {
            let g = row.get(d).copied().unwrap_or(0.0);
            if g != 0.0
                && let Some(slot) = idx.get_mut(k)
            {
                *slot = (s, g);
                k += 1;
            }
        }
        let taps = idx.get(..k).unwrap_or(&[]);
        if taps.is_empty() {
            continue;
        }
        let end = span.end.min(out.len());
        let Some(dst) = out.get_mut(span.start..end) else { continue };
        let lane = |s: usize| src.get(s).and_then(|c| c.get(span.start..end));
        match *taps {
            [(s0, g0)] => {
                if let Some(x0) = lane(s0) {
                    for (o, a) in dst.iter_mut().zip(x0) {
                        let mut acc = 0.0f32;
                        acc += a * gain * g0;
                        *o += acc;
                    }
                    continue;
                }
            }
            [(s0, g0), (s1, g1)] => {
                if let (Some(x0), Some(x1)) = (lane(s0), lane(s1)) {
                    for ((o, a), b) in dst.iter_mut().zip(x0).zip(x1) {
                        let mut acc = 0.0f32;
                        acc += a * gain * g0;
                        acc += b * gain * g1;
                        *o += acc;
                    }
                    continue;
                }
            }
            _ => {}
        }
        for (i, o) in out.iter_mut().enumerate().take(span.end).skip(span.start) {
            let mut acc = 0.0f32;
            for &(s, g) in taps {
                acc += src.get(s).and_then(|c| c.get(i)).copied().unwrap_or(0.0) * gain * g;
            }
            *o += acc;
        }
    }
}
