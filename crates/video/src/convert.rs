//! Pixel conversion: Y'CbCr → RGBA (BT.601 / BT.709, limited or full range) and RGBA downscaling.

/// Y'CbCr → R'G'B' matrix coefficients in 1/65536 units: (Cr→R, Cb→G, Cr→G, Cb→B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Matrix {
    cr_r: i32,
    cb_g: i32,
    cr_g: i32,
    cb_b: i32,
    full_range: bool,
}

impl Matrix {
    /// BT.601 (SD) or BT.709 (HD) coefficients.
    pub fn new(bt709: bool, full_range: bool) -> Matrix {
        // Kr/Kb: 601 = 0.299/0.114, 709 = 0.2126/0.0722.
        let (kr, kb) = if bt709 { (0.2126f64, 0.0722f64) } else { (0.299, 0.114) };
        let kg = 1.0 - kr - kb;
        let s = 65536.0;
        Matrix {
            cr_r: (2.0 * (1.0 - kr) * s).round() as i32,
            cb_b: (2.0 * (1.0 - kb) * s).round() as i32,
            cb_g: (2.0 * (1.0 - kb) * kb / kg * s).round() as i32,
            cr_g: (2.0 * (1.0 - kr) * kr / kg * s).round() as i32,
            full_range,
        }
    }

    /// Choose by ITU-T H.273 matrix code (1 = 709, 5/6 = 601), else by picture height.
    pub fn for_code(matrix: u8, height: u32, full_range: bool) -> Matrix {
        let bt709 = match matrix {
            1 => true,
            5 | 6 => false,
            _ => height >= 600,
        };
        Matrix::new(bt709, full_range)
    }

    /// Convert one 8-bit sample triple.
    #[inline]
    pub fn rgb(&self, y: u8, cb: u8, cr: u8) -> [u8; 3] {
        let (y, cb, cr) = (i32::from(y), i32::from(cb) - 128, i32::from(cr) - 128);
        let (y, cb, cr) = if self.full_range {
            (y << 16, cb, cr)
        } else {
            // Expand 16..235 / 16..240 to full range.
            ((y - 16) * 76309, (cb * 74_606) >> 16, (cr * 74_606) >> 16)
        };
        let y = y + (1 << 15); // round to nearest
        let r = (y + self.cr_r * cr) >> 16;
        let g = (y - self.cb_g * cb - self.cr_g * cr) >> 16;
        let b = (y + self.cb_b * cb) >> 16;
        [r.clamp(0, 255) as u8, g.clamp(0, 255) as u8, b.clamp(0, 255) as u8]
    }
}

/// The planes (and their geometry) of an 8-bit planar Y'CbCr picture for [`planar_to_rgba`].
#[derive(Clone, Copy)]
pub struct Planes<'a> {
    pub width: usize,
    pub height: usize,
    pub y: &'a [u8],
    pub y_stride: usize,
    pub u: &'a [u8],
    pub v: &'a [u8],
    pub uv_stride: usize,
    pub cx: usize,
    pub cy: usize,
}

/// Planar 8-bit Y'CbCr with chroma subsampled by `cx` horizontally and `cy` vertically
/// (1 or 2 each) → packed RGBA. Missing samples (short planes) read as black / neutral.
pub fn planar_to_rgba(p: &Planes<'_>, m: &Matrix) -> Vec<u8> {
    let Planes { width, height, y, y_stride, u, v, uv_stride, cx, cy } = *p;
    let (cx, cy) = (cx.max(1), cy.max(1));
    let mut out = vec![0u8; width.saturating_mul(height).saturating_mul(4)];
    for (row, line) in out.chunks_exact_mut(width.max(1) * 4).enumerate().take(height) {
        let yl = y.get(row * y_stride..).unwrap_or(&[]);
        let crow = (row / cy) * uv_stride;
        let ul = u.get(crow..).unwrap_or(&[]);
        let vl = v.get(crow..).unwrap_or(&[]);
        for (col, px) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let yy = yl.get(col).copied().unwrap_or(16);
            let cb = ul.get(col / cx).copied().unwrap_or(128);
            let cr = vl.get(col / cx).copied().unwrap_or(128);
            let [r, g, b] = m.rgb(yy, cb, cr);
            px.copy_from_slice(&[r, g, b, 255]);
        }
    }
    out
}

/// Area-average downscale (or nearest upscale) of packed RGBA.
pub fn scale_rgba(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    let (dw, dh) = (dw.max(1), dh.max(1));
    let mut out = vec![0u8; dw * dh * 4];
    if sw == 0 || sh == 0 || src.len() < sw * sh * 4 {
        return out;
    }
    for dy in 0..dh {
        let y0 = dy * sh / dh;
        let y1 = ((dy + 1) * sh / dh).max(y0 + 1).min(sh);
        for dx in 0..dw {
            let x0 = dx * sw / dw;
            let x1 = ((dx + 1) * sw / dw).max(x0 + 1).min(sw);
            // Sample at most a 4x4 grid inside the source box (keeps big reductions cheap).
            let (sx, sy) = ((x1 - x0).div_ceil(4).max(1), (y1 - y0).div_ceil(4).max(1));
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            let mut y = y0;
            while y < y1 {
                let mut x = x0;
                while x < x1 {
                    if let Some(p) = src.get((y * sw + x) * 4..(y * sw + x) * 4 + 4) {
                        for (a, v) in acc.iter_mut().zip(p) {
                            *a += u32::from(*v);
                        }
                        n += 1;
                    }
                    x += sx;
                }
                y += sy;
            }
            if let Some(o) = out.get_mut((dy * dw + dx) * 4..(dy * dw + dx) * 4 + 4) {
                for (o, a) in o.iter_mut().zip(acc) {
                    *o = (a / n.max(1)) as u8;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limited_range_black_white_and_grey() {
        for bt709 in [false, true] {
            let m = Matrix::new(bt709, false);
            assert_eq!(m.rgb(16, 128, 128), [0, 0, 0]);
            assert_eq!(m.rgb(235, 128, 128), [255, 255, 255]);
            let g = m.rgb(126, 128, 128);
            assert!(g[0] == g[1] && g[1] == g[2]);
        }
        let f = Matrix::new(true, true);
        assert_eq!(f.rgb(0, 128, 128), [0, 0, 0]);
        assert_eq!(f.rgb(255, 128, 128), [255, 255, 255]);
    }

    #[test]
    fn red_is_red() {
        // BT.601 limited-range red: Y 81, Cb 90, Cr 240.
        let [r, g, b] = Matrix::new(false, false).rgb(81, 90, 240);
        assert!(r > 250 && g < 5 && b < 5, "{r} {g} {b}");
    }

    #[test]
    fn scaling_averages_and_survives_bad_input() {
        let src = [255u8, 255, 255, 255, 0, 0, 0, 255];
        let out = scale_rgba(&src, 2, 1, 1, 1);
        assert_eq!(out, vec![127, 127, 127, 255]);
        assert_eq!(scale_rgba(&[], 10, 10, 2, 2), vec![0; 16]);
        assert_eq!(scale_rgba(&src, 2, 1, 4, 2).len(), 32);
    }

    #[test]
    fn planar_conversion_tolerates_short_planes() {
        let m = Matrix::new(true, false);
        let out = planar_to_rgba(&Planes { width: 4, height: 4, y: &[235; 3], y_stride: 4, u: &[], v: &[], uv_stride: 2, cx: 2, cy: 2 }, &m);
        assert_eq!(out.len(), 64);
        assert_eq!(&out[..4], &[255, 255, 255, 255]);
    }
}
