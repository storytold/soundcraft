//! Sequence and picture parameter sets (7.3.2.1, 7.3.2.2) including VUI and scaling lists.

use crate::bitstream::BitReader;
use crate::h264::error::{Error, Result, ensure, unsupported};
use crate::h264::tables::{DEFAULT_4X4_INTER, DEFAULT_4X4_INTRA, DEFAULT_8X8_INTER, DEFAULT_8X8_INTRA, ZIGZAG4, ZIGZAG8};

/// `v + k` for a coded `*_minus*` field, or the error its range check gives.
fn plus(v: u32, k: u32, err: &str) -> Result<u32> {
    v.checked_add(k).ok_or_else(|| Error::Invalid(err.to_string()))
}

/// Scaling matrices after applying fall-back rules, stored in raster order (weightScale).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalingMatrices {
    /// [Intra Y, Intra Cb, Intra Cr, Inter Y, Inter Cb, Inter Cr], raster order.
    pub m4: [[u8; 16]; 6],
    /// [Intra Y, Inter Y, Intra Cb, Inter Cb, Intra Cr, Inter Cr], raster order.
    pub m8: [[u8; 64]; 6],
}

impl ScalingMatrices {
    pub fn flat() -> Self {
        Self { m4: [[16; 16]; 6], m8: [[16; 64]; 6] }
    }
    pub fn is_flat(&self) -> bool {
        self.m4.iter().all(|m| m.iter().all(|&v| v == 16)) && self.m8.iter().all(|m| m.iter().all(|&v| v == 16))
    }
}

/// Lists as parsed, in zig-zag order, before inverse scanning.
#[derive(Clone, Debug)]
struct ZzLists {
    l4: [[u8; 16]; 6],
    l8: [[u8; 64]; 6],
}

fn zz4_to_raster(l: &[u8; 16]) -> [u8; 16] {
    let mut out = [0; 16];
    for (k, &v) in l.iter().enumerate() {
        out[ZIGZAG4[k] as usize] = v;
    }
    out
}
fn zz8_to_raster(l: &[u8; 64]) -> [u8; 64] {
    let mut out = [0; 64];
    for (k, &v) in l.iter().enumerate() {
        out[ZIGZAG8[k] as usize] = v;
    }
    out
}

impl ZzLists {
    fn to_matrices(&self) -> ScalingMatrices {
        let mut m = ScalingMatrices::flat();
        for i in 0..6 {
            m.m4[i] = zz4_to_raster(&self.l4[i]);
            m.m8[i] = zz8_to_raster(&self.l8[i]);
        }
        m
    }
    fn flat() -> Self {
        Self { l4: [[16; 16]; 6], l8: [[16; 64]; 6] }
    }
}

/// Result of parsing one scaling_list(): Some(list) or None meaning "use default".
fn parse_scaling_list<const N: usize>(r: &mut BitReader) -> Result<Option<[u8; N]>> {
    let mut list = [0u8; N];
    let mut last = 8i32;
    let mut next = 8i32;
    for (j, item) in list.iter_mut().enumerate() {
        if next != 0 {
            let delta = r.read_se()?;
            ensure!((-128..=127).contains(&delta), "delta_scale out of range");
            next = (last + delta + 256) % 256;
            if j == 0 && next == 0 {
                return Ok(None);
            }
        }
        *item = if next == 0 { last as u8 } else { next as u8 };
        last = *item as i32;
    }
    Ok(Some(list))
}

/// Parse scaling lists with a fall-back rule. `fallback` gives the lists used for rule B
/// (sequence-level) or None for rule A.
fn parse_scaling_lists(r: &mut BitReader, count: usize, fallback: Option<&ZzLists>) -> Result<ZzLists> {
    let mut out = ZzLists::flat();
    for i in 0..count.max(8) {
        let present = if i < count { r.read_flag()? } else { false };
        if i < 6 {
            let parsed = if present { Some(parse_scaling_list::<16>(r)?) } else { None };
            out.l4[i] = match parsed {
                Some(Some(l)) => l,
                Some(None) => {
                    if i < 3 {
                        DEFAULT_4X4_INTRA
                    } else {
                        DEFAULT_4X4_INTER
                    }
                }
                None => match (i, fallback) {
                    (0, None) => DEFAULT_4X4_INTRA,
                    (3, None) => DEFAULT_4X4_INTER,
                    (0 | 3, Some(f)) => f.l4[i],
                    _ => out.l4[i - 1],
                },
            };
        } else {
            let k = i - 6;
            if k >= 6 {
                break;
            }
            let parsed = if present { Some(parse_scaling_list::<64>(r)?) } else { None };
            out.l8[k] = match parsed {
                Some(Some(l)) => l,
                Some(None) => {
                    if k % 2 == 0 {
                        DEFAULT_8X8_INTRA
                    } else {
                        DEFAULT_8X8_INTER
                    }
                }
                None => match (k, fallback) {
                    (0, None) => DEFAULT_8X8_INTRA,
                    (1, None) => DEFAULT_8X8_INTER,
                    (0 | 1, Some(f)) => f.l8[k],
                    _ => out.l8[k - 2],
                },
            };
        }
    }
    // 8x8 chroma lists that were never signalled (count == 8) follow rule A/B chains as well.
    if count <= 8 {
        for k in 2..6 {
            out.l8[k] = out.l8[k - 2];
        }
    }
    Ok(out)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HrdParameters {
    pub cpb_cnt: u32,
    pub bit_rate_scale: u8,
    pub cpb_size_scale: u8,
    pub bit_rate_value_minus1: Vec<u32>,
    pub cpb_size_value_minus1: Vec<u32>,
    pub cbr_flag: Vec<bool>,
    pub initial_cpb_removal_delay_length: u8,
    pub cpb_removal_delay_length: u8,
    pub dpb_output_delay_length: u8,
    pub time_offset_length: u8,
}

fn parse_hrd(r: &mut BitReader) -> Result<HrdParameters> {
    let cpb_cnt = plus(r.read_ue()?, 1, "cpb_cnt_minus1 out of range")?;
    ensure!(cpb_cnt <= 32, "cpb_cnt_minus1 out of range");
    let mut h = HrdParameters { cpb_cnt, bit_rate_scale: r.read_bits(4)? as u8, cpb_size_scale: r.read_bits(4)? as u8, ..Default::default() };
    for _ in 0..cpb_cnt {
        h.bit_rate_value_minus1.push(r.read_ue()?);
        h.cpb_size_value_minus1.push(r.read_ue()?);
        h.cbr_flag.push(r.read_flag()?);
    }
    h.initial_cpb_removal_delay_length = r.read_bits(5)? as u8 + 1;
    h.cpb_removal_delay_length = r.read_bits(5)? as u8 + 1;
    h.dpb_output_delay_length = r.read_bits(5)? as u8 + 1;
    h.time_offset_length = r.read_bits(5)? as u8;
    Ok(h)
}

/// Video usability information (Annex E).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vui {
    pub aspect_ratio_idc: u8,
    pub sar: (u16, u16),
    pub overscan_appropriate: Option<bool>,
    pub video_format: u8,
    pub full_range: bool,
    pub colour_primaries: u8,
    pub transfer_characteristics: u8,
    pub matrix_coefficients: u8,
    pub chroma_loc: Option<(u32, u32)>,
    pub timing: Option<(u32, u32, bool)>,
    pub nal_hrd: Option<HrdParameters>,
    pub vcl_hrd: Option<HrdParameters>,
    pub low_delay_hrd: bool,
    pub pic_struct_present: bool,
    pub bitstream_restriction: Option<BitstreamRestriction>,
}

impl Default for Vui {
    fn default() -> Self {
        Self {
            aspect_ratio_idc: 0,
            sar: (0, 0),
            overscan_appropriate: None,
            video_format: 5,
            full_range: false,
            colour_primaries: 2,
            transfer_characteristics: 2,
            matrix_coefficients: 2,
            chroma_loc: None,
            timing: None,
            nal_hrd: None,
            vcl_hrd: None,
            low_delay_hrd: false,
            pic_struct_present: false,
            bitstream_restriction: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitstreamRestriction {
    pub motion_vectors_over_pic_boundaries: bool,
    pub max_bytes_per_pic_denom: u32,
    pub max_bits_per_mb_denom: u32,
    pub log2_max_mv_length_horizontal: u32,
    pub log2_max_mv_length_vertical: u32,
    pub max_num_reorder_frames: u32,
    pub max_dec_frame_buffering: u32,
}

/// Table E-1 sample aspect ratios.
const SAR_TABLE: [(u16, u16); 17] = [
    (0, 0),
    (1, 1),
    (12, 11),
    (10, 11),
    (16, 11),
    (40, 33),
    (24, 11),
    (20, 11),
    (32, 11),
    (80, 33),
    (18, 11),
    (15, 11),
    (64, 33),
    (160, 99),
    (4, 3),
    (3, 2),
    (2, 1),
];

fn parse_vui(r: &mut BitReader) -> Result<Vui> {
    let mut v = Vui::default();
    if r.read_flag()? {
        v.aspect_ratio_idc = r.read_bits(8)? as u8;
        if v.aspect_ratio_idc == 255 {
            v.sar = (r.read_bits(16)? as u16, r.read_bits(16)? as u16);
        } else if (v.aspect_ratio_idc as usize) < SAR_TABLE.len() {
            v.sar = SAR_TABLE[v.aspect_ratio_idc as usize];
        }
    }
    if r.read_flag()? {
        v.overscan_appropriate = Some(r.read_flag()?);
    }
    if r.read_flag()? {
        v.video_format = r.read_bits(3)? as u8;
        v.full_range = r.read_flag()?;
        if r.read_flag()? {
            v.colour_primaries = r.read_bits(8)? as u8;
            v.transfer_characteristics = r.read_bits(8)? as u8;
            v.matrix_coefficients = r.read_bits(8)? as u8;
        }
    }
    if r.read_flag()? {
        v.chroma_loc = Some((r.read_ue()?, r.read_ue()?));
    }
    if r.read_flag()? {
        let n = r.read_bits(32)?;
        let t = r.read_bits(32)?;
        let fixed = r.read_flag()?;
        v.timing = Some((n, t, fixed));
    }
    if r.read_flag()? {
        v.nal_hrd = Some(parse_hrd(r)?);
    }
    if r.read_flag()? {
        v.vcl_hrd = Some(parse_hrd(r)?);
    }
    if v.nal_hrd.is_some() || v.vcl_hrd.is_some() {
        v.low_delay_hrd = r.read_flag()?;
    }
    v.pic_struct_present = r.read_flag()?;
    // Some streams truncate the VUI after this point; treat EOF as "absent".
    if r.bits_left() > 0 && r.read_flag()? {
        let br = (|| -> Result<BitstreamRestriction> {
            Ok(BitstreamRestriction {
                motion_vectors_over_pic_boundaries: r.read_flag()?,
                max_bytes_per_pic_denom: r.read_ue()?,
                max_bits_per_mb_denom: r.read_ue()?,
                log2_max_mv_length_horizontal: r.read_ue()?,
                log2_max_mv_length_vertical: r.read_ue()?,
                max_num_reorder_frames: r.read_ue()?,
                max_dec_frame_buffering: r.read_ue()?,
            })
        })();
        v.bitstream_restriction = br.ok();
    }
    Ok(v)
}

/// Sequence parameter set.
#[derive(Clone, Debug)]
pub struct Sps {
    pub profile_idc: u8,
    pub constraint_flags: u8,
    pub level_idc: u8,
    pub id: u32,
    pub chroma_format_idc: u32,
    pub separate_colour_plane: bool,
    pub bit_depth_luma: u32,
    pub bit_depth_chroma: u32,
    pub qpprime_y_zero_transform_bypass: bool,
    pub seq_scaling_matrix_present: bool,
    scaling_lists: ZzLists,
    pub scaling: ScalingMatrices,
    pub log2_max_frame_num: u32,
    pub pic_order_cnt_type: u32,
    pub log2_max_poc_lsb: u32,
    pub delta_pic_order_always_zero: bool,
    pub offset_for_non_ref_pic: i32,
    pub offset_for_top_to_bottom_field: i32,
    pub offset_for_ref_frame: Vec<i32>,
    pub max_num_ref_frames: u32,
    pub gaps_in_frame_num_allowed: bool,
    pub pic_width_in_mbs: u32,
    pub pic_height_in_map_units: u32,
    pub frame_mbs_only: bool,
    pub mb_adaptive_frame_field: bool,
    pub direct_8x8_inference: bool,
    /// (left, right, top, bottom) in crop units as coded.
    pub frame_crop: Option<(u32, u32, u32, u32)>,
    pub vui: Option<Vui>,
}

impl Sps {
    pub fn parse(rbsp: &[u8]) -> Result<Sps> {
        let mut r = BitReader::new(rbsp);
        let profile_idc = r.read_bits(8)? as u8;
        let constraint_flags = r.read_bits(8)? as u8;
        let level_idc = r.read_bits(8)? as u8;
        let id = r.read_ue()?;
        ensure!(id < 32, "seq_parameter_set_id {id} out of range");
        let mut chroma_format_idc = 1;
        let mut separate_colour_plane = false;
        let mut bit_depth_luma = 8;
        let mut bit_depth_chroma = 8;
        let mut bypass = false;
        let mut seq_scaling_matrix_present = false;
        let mut scaling_lists = ZzLists::flat();
        if matches!(profile_idc, 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135) {
            chroma_format_idc = r.read_ue()?;
            ensure!(chroma_format_idc <= 3, "chroma_format_idc out of range");
            if chroma_format_idc == 3 {
                separate_colour_plane = r.read_flag()?;
            }
            bit_depth_luma = plus(r.read_ue()?, 8, "bit depth out of range")?;
            bit_depth_chroma = plus(r.read_ue()?, 8, "bit depth out of range")?;
            ensure!(bit_depth_luma <= 14 && bit_depth_chroma <= 14, "bit depth out of range");
            bypass = r.read_flag()?;
            seq_scaling_matrix_present = r.read_flag()?;
            if seq_scaling_matrix_present {
                let count = if chroma_format_idc != 3 { 8 } else { 12 };
                scaling_lists = parse_scaling_lists(&mut r, count, None)?;
            }
        }
        let log2_max_frame_num = plus(r.read_ue()?, 4, "log2_max_frame_num out of range")?;
        ensure!(log2_max_frame_num <= 16, "log2_max_frame_num out of range");
        let pic_order_cnt_type = r.read_ue()?;
        ensure!(pic_order_cnt_type <= 2, "pic_order_cnt_type out of range");
        let mut log2_max_poc_lsb = 0;
        let mut delta_pic_order_always_zero = false;
        let mut offset_for_non_ref_pic = 0;
        let mut offset_for_top_to_bottom_field = 0;
        let mut offset_for_ref_frame = Vec::new();
        if pic_order_cnt_type == 0 {
            log2_max_poc_lsb = plus(r.read_ue()?, 4, "log2_max_pic_order_cnt_lsb out of range")?;
            ensure!(log2_max_poc_lsb <= 16, "log2_max_pic_order_cnt_lsb out of range");
        } else if pic_order_cnt_type == 1 {
            delta_pic_order_always_zero = r.read_flag()?;
            offset_for_non_ref_pic = r.read_se()?;
            offset_for_top_to_bottom_field = r.read_se()?;
            let n = r.read_ue()?;
            ensure!(n <= 255, "num_ref_frames_in_pic_order_cnt_cycle out of range");
            for _ in 0..n {
                offset_for_ref_frame.push(r.read_se()?);
            }
        }
        let max_num_ref_frames = r.read_ue()?;
        ensure!(max_num_ref_frames <= 16, "max_num_ref_frames out of range");
        let gaps_in_frame_num_allowed = r.read_flag()?;
        let pic_width_in_mbs = plus(r.read_ue()?, 1, "picture too large")?;
        let pic_height_in_map_units = plus(r.read_ue()?, 1, "picture too large")?;
        ensure!(pic_width_in_mbs <= 1024 && pic_height_in_map_units <= 1024, "picture too large");
        let frame_mbs_only = r.read_flag()?;
        let mb_adaptive_frame_field = if !frame_mbs_only { r.read_flag()? } else { false };
        let direct_8x8_inference = r.read_flag()?;
        let frame_crop = if r.read_flag()? { Some((r.read_ue()?, r.read_ue()?, r.read_ue()?, r.read_ue()?)) } else { None };
        let vui = if r.read_flag()? { Some(parse_vui(&mut r)?) } else { None };
        let scaling = if seq_scaling_matrix_present { scaling_lists.to_matrices() } else { ScalingMatrices::flat() };
        let sps = Sps {
            profile_idc,
            constraint_flags,
            level_idc,
            id,
            chroma_format_idc,
            separate_colour_plane,
            bit_depth_luma,
            bit_depth_chroma,
            qpprime_y_zero_transform_bypass: bypass,
            seq_scaling_matrix_present,
            scaling_lists,
            scaling,
            log2_max_frame_num,
            pic_order_cnt_type,
            log2_max_poc_lsb,
            delta_pic_order_always_zero,
            offset_for_non_ref_pic,
            offset_for_top_to_bottom_field,
            offset_for_ref_frame,
            max_num_ref_frames,
            gaps_in_frame_num_allowed,
            pic_width_in_mbs,
            pic_height_in_map_units,
            frame_mbs_only,
            mb_adaptive_frame_field,
            direct_8x8_inference,
            frame_crop,
            vui,
        };
        if let Some((l, rr, t, b)) = sps.frame_crop {
            let (cx, cy) = sps.crop_unit();
            let fits = |a: u32, b: u32, unit: u32, size: u32| a.checked_add(b).and_then(|v| v.checked_mul(unit)).is_some_and(|v| v < size);
            ensure!(fits(l, rr, cx, sps.width()) && fits(t, b, cy, sps.height()), "frame cropping exceeds picture size");
        }
        Ok(sps)
    }

    /// ChromaArrayType.
    pub fn chroma_array_type(&self) -> u32 {
        if self.separate_colour_plane { 0 } else { self.chroma_format_idc }
    }
    pub fn frame_height_in_mbs(&self) -> u32 {
        (2 - self.frame_mbs_only as u32) * self.pic_height_in_map_units
    }
    /// Decoded (uncropped) luma width.
    pub fn width(&self) -> u32 {
        self.pic_width_in_mbs * 16
    }
    pub fn height(&self) -> u32 {
        self.frame_height_in_mbs() * 16
    }
    pub fn max_frame_num(&self) -> u32 {
        1 << self.log2_max_frame_num
    }
    /// (CropUnitX, CropUnitY).
    pub fn crop_unit(&self) -> (u32, u32) {
        let (sub_w, sub_h) = match self.chroma_array_type() {
            1 => (2, 2),
            2 => (2, 1),
            _ => (1, 1),
        };
        if self.chroma_array_type() == 0 { (1, 2 - self.frame_mbs_only as u32) } else { (sub_w, sub_h * (2 - self.frame_mbs_only as u32)) }
    }
    /// Cropping rectangle in luma samples: (x, y, width, height).
    pub fn crop_rect(&self) -> (u32, u32, u32, u32) {
        match self.frame_crop {
            None => (0, 0, self.width(), self.height()),
            Some((l, r, t, b)) => {
                let (cx, cy) = self.crop_unit();
                let cut = |a: u32, b: u32, unit: u32| a.saturating_add(b).saturating_mul(unit);
                (l.saturating_mul(cx), t.saturating_mul(cy), self.width().saturating_sub(cut(l, r, cx)), self.height().saturating_sub(cut(t, b, cy)))
            }
        }
    }

    /// MaxDpbFrames from Table A-1 (and the VUI max_dec_frame_buffering when present).
    pub fn max_dpb_frames(&self) -> usize {
        let max_dpb_mbs: u32 = match self.level_idc {
            9 => 396,
            10 => 396,
            11 => {
                if self.constraint_flags & 0x10 != 0 && !matches!(self.profile_idc, 100 | 110 | 122 | 244 | 44) {
                    396
                } else {
                    900
                }
            }
            12 | 13 | 20 => 2376,
            21 => 4752,
            22 | 30 => 8100,
            31 => 18000,
            32 => 20480,
            40 | 41 => 32768,
            42 => 34816,
            50 => 110400,
            51 | 52 => 184320,
            60..=62 => 696320,
            _ => 184320,
        };
        let frame_mbs = self.pic_width_in_mbs * self.frame_height_in_mbs();
        let mut n = (max_dpb_mbs / frame_mbs.max(1)).clamp(1, 16) as usize;
        if let Some(br) = self.vui.as_ref().and_then(|v| v.bitstream_restriction.as_ref()) {
            n = (br.max_dec_frame_buffering as usize).min(16);
        }
        n.max(self.max_num_ref_frames as usize).max(1)
    }

    /// Number of frames that may precede a frame in decoding order and follow it in output order.
    pub fn max_num_reorder_frames(&self) -> usize {
        if let Some(br) = self.vui.as_ref().and_then(|v| v.bitstream_restriction.as_ref()) {
            return br.max_num_reorder_frames as usize;
        }
        // Profiles that cannot reorder (no B slices) with constraint_set3 (intra) etc.
        if matches!(self.profile_idc, 44 | 86 | 100 | 110 | 122 | 244) && self.constraint_flags & 0x10 != 0 {
            return 0;
        }
        self.max_dpb_frames()
    }

    pub fn check_supported(&self) -> Result<()> {
        if !self.frame_mbs_only {
            return unsupported("interlaced (field / MBAFF) coding");
        }
        if self.chroma_array_type() != 1 {
            return unsupported(format!("chroma_format_idc {} (only 4:2:0 is implemented)", self.chroma_format_idc));
        }
        if self.bit_depth_luma != 8 || self.bit_depth_chroma != 8 {
            return unsupported(format!("bit depth luma {} chroma {} (only 8-bit is implemented)", self.bit_depth_luma, self.bit_depth_chroma));
        }
        if self.qpprime_y_zero_transform_bypass {
            return unsupported("qpprime_y_zero_transform_bypass (lossless)");
        }
        Ok(())
    }
}

/// Picture parameter set.
#[derive(Clone, Debug)]
pub struct Pps {
    pub id: u32,
    pub sps_id: u32,
    pub entropy_coding_mode: bool,
    pub bottom_field_pic_order_in_frame_present: bool,
    pub num_slice_groups: u32,
    pub slice_group_map_type: u32,
    pub num_ref_idx_l0_default_active: u32,
    pub num_ref_idx_l1_default_active: u32,
    pub weighted_pred: bool,
    pub weighted_bipred_idc: u32,
    pub pic_init_qp: i32,
    pub pic_init_qs: i32,
    pub chroma_qp_index_offset: i32,
    pub deblocking_filter_control_present: bool,
    pub constrained_intra_pred: bool,
    pub redundant_pic_cnt_present: bool,
    pub transform_8x8_mode: bool,
    pub pic_scaling_matrix_present: bool,
    pub second_chroma_qp_index_offset: i32,
    /// Effective scaling matrices (after PPS/SPS fall-back), raster order.
    pub scaling: ScalingMatrices,
}

impl Pps {
    /// Parse a PPS. Needs the SPS table because the tail depends on chroma_format_idc.
    pub fn parse(rbsp: &[u8], spss: &[Option<Sps>]) -> Result<Pps> {
        let mut r = BitReader::new(rbsp);
        let id = r.read_ue()?;
        ensure!(id < 256, "pic_parameter_set_id out of range");
        let sps_id = r.read_ue()?;
        ensure!(sps_id < 32, "seq_parameter_set_id out of range");
        let sps = spss
            .get(sps_id as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| crate::h264::Error::MissingParameterSet(format!("SPS {sps_id} for PPS {id}")))?;
        let entropy_coding_mode = r.read_flag()?;
        let bottom_field_pic_order_in_frame_present = r.read_flag()?;
        let num_slice_groups = plus(r.read_ue()?, 1, "num_slice_groups out of range")?;
        ensure!(num_slice_groups <= 8, "num_slice_groups out of range");
        let mut slice_group_map_type = 0;
        if num_slice_groups > 1 {
            // Parsed for completeness; FMO decoding is unsupported.
            slice_group_map_type = r.read_ue()?;
            match slice_group_map_type {
                0 => {
                    for _ in 0..num_slice_groups {
                        r.read_ue()?;
                    }
                }
                2 => {
                    for _ in 0..num_slice_groups - 1 {
                        r.read_ue()?;
                        r.read_ue()?;
                    }
                }
                3..=5 => {
                    r.read_flag()?;
                    r.read_ue()?;
                }
                6 => {
                    let n = plus(r.read_ue()?, 1, "pic_size_in_map_units_minus1 out of range")?;
                    let bits = 32 - (num_slice_groups - 1).leading_zeros();
                    for _ in 0..n {
                        r.read_bits(bits)?;
                    }
                }
                _ => {}
            }
        }
        let num_ref_idx_l0_default_active = plus(r.read_ue()?, 1, "num_ref_idx_default_active out of range")?;
        let num_ref_idx_l1_default_active = plus(r.read_ue()?, 1, "num_ref_idx_default_active out of range")?;
        ensure!(num_ref_idx_l0_default_active <= 32 && num_ref_idx_l1_default_active <= 32, "num_ref_idx_default_active out of range");
        let weighted_pred = r.read_flag()?;
        let weighted_bipred_idc = r.read_bits(2)?;
        let pic_init_qp = r.read_se()?.checked_add(26).ok_or_else(|| Error::Invalid("pic_init_qp_minus26 out of range".into()))?;
        let pic_init_qs = r.read_se()?.checked_add(26).ok_or_else(|| Error::Invalid("pic_init_qs_minus26 out of range".into()))?;
        let chroma_qp_index_offset = r.read_se()?;
        ensure!((-12..=12).contains(&chroma_qp_index_offset), "chroma_qp_index_offset out of range");
        let deblocking_filter_control_present = r.read_flag()?;
        let constrained_intra_pred = r.read_flag()?;
        let redundant_pic_cnt_present = r.read_flag()?;
        let mut transform_8x8_mode = false;
        let mut pic_scaling_matrix_present = false;
        let mut second_chroma_qp_index_offset = chroma_qp_index_offset;
        let mut scaling = sps.scaling.clone();
        if r.more_rbsp_data() {
            transform_8x8_mode = r.read_flag()?;
            pic_scaling_matrix_present = r.read_flag()?;
            if pic_scaling_matrix_present {
                let count = 6 + if sps.chroma_format_idc != 3 { 2 } else { 6 } * transform_8x8_mode as usize;
                // Fall-back rule B uses the sequence-level lists (or Flat when the SPS has none, but then
                // rule A applies per 7.4.2.2).
                let lists = if sps.seq_scaling_matrix_present {
                    parse_scaling_lists(&mut r, count, Some(&sps.scaling_lists))?
                } else {
                    parse_scaling_lists(&mut r, count, None)?
                };
                scaling = lists.to_matrices();
            }
            second_chroma_qp_index_offset = r.read_se()?;
            ensure!((-12..=12).contains(&second_chroma_qp_index_offset), "second_chroma_qp_index_offset out of range");
        }
        Ok(Pps {
            id,
            sps_id,
            entropy_coding_mode,
            bottom_field_pic_order_in_frame_present,
            num_slice_groups,
            slice_group_map_type,
            num_ref_idx_l0_default_active,
            num_ref_idx_l1_default_active,
            weighted_pred,
            weighted_bipred_idc,
            pic_init_qp,
            pic_init_qs,
            chroma_qp_index_offset,
            deblocking_filter_control_present,
            constrained_intra_pred,
            redundant_pic_cnt_present,
            transform_8x8_mode,
            pic_scaling_matrix_present,
            second_chroma_qp_index_offset,
            scaling,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitstream::BitWriter;

    #[test]
    fn scaling_list_default_signal() {
        // delta_scale = -8 on the first entry => nextScale 0 => use default.
        let mut w = BitWriter::new();
        w.write_se(-8);
        w.rbsp_trailing();
        let b = w.finish();
        let mut r = BitReader::new(&b);
        assert_eq!(parse_scaling_list::<16>(&mut r).unwrap(), None);
    }

    #[test]
    fn scaling_list_repeat_last() {
        // 8 -> +2 = 10, then +0 = 10, then -10 => 0 => repeat 10 for the rest.
        let mut w = BitWriter::new();
        w.write_se(2);
        w.write_se(0);
        w.write_se(-10);
        w.rbsp_trailing();
        let b = w.finish();
        let mut r = BitReader::new(&b);
        let l = parse_scaling_list::<16>(&mut r).unwrap().unwrap();
        assert_eq!(l, [10; 16]);
    }

    #[test]
    fn parse_minimal_baseline_sps() {
        // profile 66, level 30, sps_id 0, log2_max_frame_num_minus4 0, poc type 2, 1 ref, 11x9 MBs.
        let mut w = BitWriter::new();
        w.write_bits(66, 8);
        w.write_bits(0xC0, 8);
        w.write_bits(30, 8);
        w.write_ue(0);
        w.write_ue(0);
        w.write_ue(2);
        w.write_ue(1);
        w.write_bit(false);
        w.write_ue(10);
        w.write_ue(8);
        w.write_bit(true); // frame_mbs_only
        w.write_bit(true); // direct_8x8
        w.write_bit(false); // cropping
        w.write_bit(false); // vui
        w.rbsp_trailing();
        let sps = Sps::parse(&w.finish()).unwrap();
        assert_eq!((sps.width(), sps.height()), (176, 144));
        assert_eq!(sps.pic_order_cnt_type, 2);
        assert_eq!(sps.crop_rect(), (0, 0, 176, 144));
    }

    /// Coded SPS fields; the default is the minimal baseline SPS above.
    #[derive(Clone, Copy)]
    struct SpsFields {
        high_profile: bool,
        bit_depth_luma_minus8: u32,
        log2_max_frame_num_minus4: u32,
        poc_type: u32,
        log2_max_poc_lsb_minus4: u32,
        width_minus1: u32,
        height_minus1: u32,
        crop: Option<(u32, u32, u32, u32)>,
        cpb_cnt_minus1: Option<u32>,
    }

    impl Default for SpsFields {
        fn default() -> Self {
            Self {
                high_profile: false,
                bit_depth_luma_minus8: 0,
                log2_max_frame_num_minus4: 0,
                poc_type: 2,
                log2_max_poc_lsb_minus4: 0,
                width_minus1: 10,
                height_minus1: 8,
                crop: None,
                cpb_cnt_minus1: None,
            }
        }
    }

    fn sps(f: SpsFields) -> Result<Sps> {
        let mut w = BitWriter::new();
        w.write_bits(if f.high_profile { 100 } else { 66 }, 8);
        w.write_bits(0, 8);
        w.write_bits(30, 8);
        w.write_ue(0);
        if f.high_profile {
            w.write_ue(1); // chroma_format_idc
            w.write_ue(f.bit_depth_luma_minus8);
            w.write_ue(0); // bit_depth_chroma_minus8
            w.write_bit(false); // qpprime_y_zero_transform_bypass
            w.write_bit(false); // seq_scaling_matrix_present
        }
        w.write_ue(f.log2_max_frame_num_minus4);
        w.write_ue(f.poc_type);
        if f.poc_type == 0 {
            w.write_ue(f.log2_max_poc_lsb_minus4);
        }
        w.write_ue(1);
        w.write_bit(false);
        w.write_ue(f.width_minus1);
        w.write_ue(f.height_minus1);
        w.write_bit(true); // frame_mbs_only
        w.write_bit(true); // direct_8x8
        w.write_bit(f.crop.is_some());
        if let Some((l, r, t, b)) = f.crop {
            for v in [l, r, t, b] {
                w.write_ue(v);
            }
        }
        w.write_bit(f.cpb_cnt_minus1.is_some()); // vui
        if let Some(n) = f.cpb_cnt_minus1 {
            for _ in 0..5 {
                w.write_bit(false); // aspect ratio, overscan, video signal, chroma loc, timing
            }
            w.write_bit(true); // nal_hrd
            w.write_ue(n);
            w.write_bits(0, 8);
            w.write_ue(0);
            w.write_ue(0);
            w.write_bit(false);
            w.write_bits(0, 20);
            w.write_bit(false); // vcl_hrd
            w.write_bit(false); // low_delay_hrd
            w.write_bit(false); // pic_struct_present
            w.write_bit(false); // bitstream_restriction
        }
        w.rbsp_trailing();
        Sps::parse(&w.finish())
    }

    fn invalid_sps(f: SpsFields) -> String {
        match sps(f) {
            Err(crate::h264::Error::Invalid(m)) => m,
            other => panic!("expected an invalid SPS, got {other:?}"),
        }
    }

    #[test]
    fn sps_fields_of_the_test_writer_parse() {
        let s = sps(SpsFields { high_profile: true, poc_type: 0, crop: Some((0, 2, 0, 2)), cpb_cnt_minus1: Some(0), ..Default::default() }).unwrap();
        assert_eq!((s.bit_depth_luma, s.log2_max_poc_lsb, s.width(), s.height()), (8, 4, 176, 144));
        assert_eq!(s.crop_rect(), (0, 0, 172, 140));
        assert_eq!(s.vui.and_then(|v| v.nal_hrd).map(|h| h.cpb_cnt), Some(1));
    }

    #[test]
    fn huge_bit_depth_is_an_error() {
        assert_eq!(invalid_sps(SpsFields { high_profile: true, bit_depth_luma_minus8: u32::MAX, ..Default::default() }), "bit depth out of range");
    }

    #[test]
    fn huge_log2_max_frame_num_is_an_error() {
        assert_eq!(invalid_sps(SpsFields { log2_max_frame_num_minus4: u32::MAX, ..Default::default() }), "log2_max_frame_num out of range");
    }

    #[test]
    fn huge_log2_max_poc_lsb_is_an_error() {
        let f = SpsFields { poc_type: 0, log2_max_poc_lsb_minus4: u32::MAX, ..Default::default() };
        assert_eq!(invalid_sps(f), "log2_max_pic_order_cnt_lsb out of range");
    }

    #[test]
    fn huge_picture_size_is_an_error() {
        assert_eq!(invalid_sps(SpsFields { width_minus1: u32::MAX, ..Default::default() }), "picture too large");
        assert_eq!(invalid_sps(SpsFields { height_minus1: u32::MAX, ..Default::default() }), "picture too large");
    }

    #[test]
    fn crop_offsets_that_overflow_are_an_error() {
        let msg = "frame cropping exceeds picture size";
        // The sum of two offsets overflows, then the scaling by the crop unit does.
        assert_eq!(invalid_sps(SpsFields { crop: Some((u32::MAX, 1, 0, 0)), ..Default::default() }), msg);
        assert_eq!(invalid_sps(SpsFields { crop: Some((0, 0, 1 << 31, 0)), ..Default::default() }), msg);
    }

    #[test]
    fn huge_cpb_count_is_an_error() {
        assert_eq!(invalid_sps(SpsFields { cpb_cnt_minus1: Some(u32::MAX), ..Default::default() }), "cpb_cnt_minus1 out of range");
    }

    /// A PPS for the SPS above; `slice_groups` is (num_slice_groups_minus1, pic_size_in_map_units_minus1)
    /// coded with slice_group_map_type 6.
    fn pps(slice_groups: Option<(u32, u32)>, num_ref_idx_l0_minus1: u32, pic_init_qp_minus26: i32) -> Result<Pps> {
        let mut w = BitWriter::new();
        w.write_ue(0);
        w.write_ue(0);
        w.write_bit(false);
        w.write_bit(false);
        match slice_groups {
            Some((groups, units)) => {
                w.write_ue(groups);
                w.write_ue(6);
                w.write_ue(units);
            }
            None => w.write_ue(0),
        }
        w.write_ue(num_ref_idx_l0_minus1);
        w.write_ue(0);
        w.write_bit(false);
        w.write_bits(0, 2);
        w.write_se(pic_init_qp_minus26);
        w.write_se(0);
        w.write_se(0);
        w.write_bit(false);
        w.write_bit(false);
        w.write_bit(false);
        w.rbsp_trailing();
        Pps::parse(&w.finish(), &[Some(sps(SpsFields::default()).unwrap())])
    }

    fn invalid_pps(p: Result<Pps>) -> String {
        match p {
            Err(crate::h264::Error::Invalid(m)) => m,
            other => panic!("expected an invalid PPS, got {other:?}"),
        }
    }

    #[test]
    fn pps_fields_of_the_test_writer_parse() {
        let p = pps(None, 2, -4).unwrap();
        assert_eq!((p.num_slice_groups, p.num_ref_idx_l0_default_active, p.pic_init_qp), (1, 3, 22));
    }

    #[test]
    fn huge_pps_counts_are_an_error() {
        assert_eq!(invalid_pps(pps(Some((u32::MAX, 0)), 0, 0)), "num_slice_groups out of range");
        assert_eq!(invalid_pps(pps(Some((1, u32::MAX)), 0, 0)), "pic_size_in_map_units_minus1 out of range");
        assert_eq!(invalid_pps(pps(None, u32::MAX, 0)), "num_ref_idx_default_active out of range");
    }

    #[test]
    fn huge_pic_init_qp_is_an_error() {
        assert_eq!(invalid_pps(pps(None, 0, i32::MAX)), "pic_init_qp_minus26 out of range");
    }
}
