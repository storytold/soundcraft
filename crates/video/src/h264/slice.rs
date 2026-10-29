//! NAL unit header and slice header parsing (7.3.1, 7.3.3).

use crate::bitstream::BitReader;
use crate::h264::error::{Result, ensure};
use crate::h264::params::{Pps, Sps};

/// NAL unit types used by the decoder.
pub mod nal_type {
    pub const SLICE: u8 = 1;
    pub const SLICE_DPA: u8 = 2;
    pub const SLICE_DPB: u8 = 3;
    pub const SLICE_DPC: u8 = 4;
    pub const IDR: u8 = 5;
    pub const SEI: u8 = 6;
    pub const SPS: u8 = 7;
    pub const PPS: u8 = 8;
    pub const AUD: u8 = 9;
    pub const END_SEQ: u8 = 10;
    pub const END_STREAM: u8 = 11;
    pub const FILLER: u8 = 12;
    pub const SPS_EXT: u8 = 13;
    pub const PREFIX: u8 = 14;
    pub const SUBSET_SPS: u8 = 15;
    pub const AUX_SLICE: u8 = 19;
    pub const SLICE_EXT: u8 = 20;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NalHeader {
    pub nal_ref_idc: u8,
    pub nal_unit_type: u8,
}

impl NalHeader {
    pub fn parse(b: u8) -> Result<Self> {
        ensure!(b & 0x80 == 0, "forbidden_zero_bit set");
        Ok(Self { nal_ref_idc: (b >> 5) & 3, nal_unit_type: b & 0x1f })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliceType {
    P,
    B,
    I,
    Sp,
    Si,
}

impl SliceType {
    fn from_raw(v: u32) -> Result<Self> {
        Ok(match v % 5 {
            0 => SliceType::P,
            1 => SliceType::B,
            2 => SliceType::I,
            3 => SliceType::Sp,
            _ => SliceType::Si,
        })
    }
    pub fn is_intra(self) -> bool {
        matches!(self, SliceType::I | SliceType::Si)
    }
    pub fn is_b(self) -> bool {
        self == SliceType::B
    }
    pub fn is_p(self) -> bool {
        matches!(self, SliceType::P | SliceType::Sp)
    }
}

/// One entry of ref_pic_list_modification().
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefPicListMod {
    pub idc: u32,
    /// abs_diff_pic_num_minus1 or long_term_pic_num.
    pub value: u32,
}

/// memory_management_control_operation entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mmco {
    pub op: u32,
    pub difference_of_pic_nums_minus1: u32,
    pub long_term_pic_num: u32,
    pub long_term_frame_idx: u32,
    pub max_long_term_frame_idx_plus1: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WeightEntry {
    pub luma_weight: i32,
    pub luma_offset: i32,
    pub luma_flag: bool,
    pub chroma_weight: [i32; 2],
    pub chroma_offset: [i32; 2],
    pub chroma_flag: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PredWeightTable {
    pub luma_log2_denom: u32,
    pub chroma_log2_denom: u32,
    pub l0: Vec<WeightEntry>,
    pub l1: Vec<WeightEntry>,
}

#[derive(Clone, Debug)]
pub struct SliceHeader {
    pub nal_ref_idc: u8,
    pub nal_unit_type: u8,
    pub idr: bool,
    pub first_mb_in_slice: u32,
    pub slice_type_raw: u32,
    pub slice_type: SliceType,
    pub pps_id: u32,
    pub colour_plane_id: u32,
    pub frame_num: u32,
    pub field_pic: bool,
    pub bottom_field: bool,
    pub idr_pic_id: u32,
    pub pic_order_cnt_lsb: u32,
    pub delta_pic_order_cnt_bottom: i32,
    pub delta_pic_order_cnt: [i32; 2],
    pub redundant_pic_cnt: u32,
    pub direct_spatial_mv_pred: bool,
    pub num_ref_idx_active: [u32; 2],
    pub ref_pic_list_mod: [Vec<RefPicListMod>; 2],
    pub pred_weight_table: Option<PredWeightTable>,
    pub no_output_of_prior_pics: bool,
    pub long_term_reference: bool,
    pub adaptive_ref_pic_marking: bool,
    pub mmcos: Vec<Mmco>,
    pub cabac_init_idc: u32,
    pub slice_qp_delta: i32,
    pub sp_for_switch: bool,
    pub slice_qs_delta: i32,
    pub disable_deblocking_filter_idc: u32,
    pub slice_alpha_c0_offset_div2: i32,
    pub slice_beta_offset_div2: i32,
    pub slice_group_change_cycle: u32,
    /// Bit offset of slice_data() within the RBSP.
    pub header_bits: usize,
}

impl SliceHeader {
    pub fn qp(&self, pps: &Pps) -> i32 {
        pps.pic_init_qp + self.slice_qp_delta
    }
    pub fn has_mmco5(&self) -> bool {
        self.mmcos.iter().any(|m| m.op == 5)
    }

    /// Parse the slice header. `pps_lookup` resolves pic_parameter_set_id to (PPS, SPS).
    pub fn parse<'a>(
        rbsp: &[u8],
        nal: NalHeader,
        pps_lookup: impl FnOnce(u32) -> Result<(&'a Pps, &'a Sps)>,
    ) -> Result<(SliceHeader, &'a Pps, &'a Sps)> {
        let mut r = BitReader::new(rbsp);
        let idr = nal.nal_unit_type == nal_type::IDR;
        let first_mb_in_slice = r.read_ue()?;
        let slice_type_raw = r.read_ue()?;
        ensure!(slice_type_raw <= 9, "slice_type out of range");
        let slice_type = SliceType::from_raw(slice_type_raw)?;
        let pps_id = r.read_ue()?;
        ensure!(pps_id < 256, "pic_parameter_set_id out of range");
        let (pps, sps) = pps_lookup(pps_id)?;
        let mut colour_plane_id = 0;
        if sps.separate_colour_plane {
            colour_plane_id = r.read_bits(2)?;
        }
        let frame_num = r.read_bits(sps.log2_max_frame_num)?;
        let mut field_pic = false;
        let mut bottom_field = false;
        if !sps.frame_mbs_only {
            field_pic = r.read_flag()?;
            if field_pic {
                bottom_field = r.read_flag()?;
            }
        }
        let idr_pic_id = if idr { r.read_ue()? } else { 0 };
        let mut pic_order_cnt_lsb = 0;
        let mut delta_pic_order_cnt_bottom = 0;
        let mut delta_pic_order_cnt = [0; 2];
        if sps.pic_order_cnt_type == 0 {
            pic_order_cnt_lsb = r.read_bits(sps.log2_max_poc_lsb)?;
            if pps.bottom_field_pic_order_in_frame_present && !field_pic {
                delta_pic_order_cnt_bottom = r.read_se()?;
            }
        }
        if sps.pic_order_cnt_type == 1 && !sps.delta_pic_order_always_zero {
            delta_pic_order_cnt[0] = r.read_se()?;
            if pps.bottom_field_pic_order_in_frame_present && !field_pic {
                delta_pic_order_cnt[1] = r.read_se()?;
            }
        }
        let redundant_pic_cnt = if pps.redundant_pic_cnt_present { r.read_ue()? } else { 0 };
        let direct_spatial_mv_pred = if slice_type.is_b() { r.read_flag()? } else { false };
        let mut num_ref_idx_active = [pps.num_ref_idx_l0_default_active, pps.num_ref_idx_l1_default_active];
        if !slice_type.is_intra() {
            if r.read_flag()? {
                num_ref_idx_active[0] = r.read_ue()? + 1;
                if slice_type.is_b() {
                    num_ref_idx_active[1] = r.read_ue()? + 1;
                }
            }
            let max = if field_pic { 32 } else { 16 };
            ensure!(num_ref_idx_active[0] <= max && num_ref_idx_active[1] <= max, "num_ref_idx_active out of range");
        }
        if !slice_type.is_b() {
            num_ref_idx_active[1] = 0;
        }
        if slice_type.is_intra() {
            num_ref_idx_active[0] = 0;
        }
        ensure!(nal.nal_unit_type != 20 && nal.nal_unit_type != 21, "MVC slices are not supported");
        let ref_fields = parse_reference_fields(&mut r, nal, idr, slice_type, pps, sps, num_ref_idx_active)?;
        let RefFields { ref_pic_list_mod, pred_weight_table, no_output_of_prior_pics, long_term_reference, adaptive_ref_pic_marking, mmcos } =
            ref_fields;
        let mut cabac_init_idc = 0;
        if pps.entropy_coding_mode && !slice_type.is_intra() {
            cabac_init_idc = r.read_ue()?;
            ensure!(cabac_init_idc <= 2, "cabac_init_idc out of range");
        }
        let slice_qp_delta = r.read_se()?;
        let qp = pps.pic_init_qp + slice_qp_delta;
        ensure!((0..=51).contains(&qp), "slice QP {qp} out of range");
        let mut sp_for_switch = false;
        let mut slice_qs_delta = 0;
        if matches!(slice_type, SliceType::Sp | SliceType::Si) {
            if slice_type == SliceType::Sp {
                sp_for_switch = r.read_flag()?;
            }
            slice_qs_delta = r.read_se()?;
        }
        let mut disable_deblocking_filter_idc = 0;
        let mut slice_alpha_c0_offset_div2 = 0;
        let mut slice_beta_offset_div2 = 0;
        if pps.deblocking_filter_control_present {
            disable_deblocking_filter_idc = r.read_ue()?;
            ensure!(disable_deblocking_filter_idc <= 2, "disable_deblocking_filter_idc out of range");
            if disable_deblocking_filter_idc != 1 {
                slice_alpha_c0_offset_div2 = r.read_se()?;
                slice_beta_offset_div2 = r.read_se()?;
                ensure!(
                    (-6..=6).contains(&slice_alpha_c0_offset_div2) && (-6..=6).contains(&slice_beta_offset_div2),
                    "deblocking offsets out of range"
                );
            }
        }
        let mut slice_group_change_cycle = 0;
        if pps.num_slice_groups > 1 && (3..=5).contains(&pps.slice_group_map_type) {
            // Ceil(Log2(PicSizeInMapUnits / SliceGroupChangeRate + 1)); rate is not retained, so read
            // conservatively using the picture size (FMO is unsupported for decoding anyway).
            let pic_size = sps.pic_width_in_mbs * sps.pic_height_in_map_units;
            let bits = 32 - pic_size.leading_zeros();
            slice_group_change_cycle = r.read_bits(bits)?;
        }
        let header_bits = r.position();
        Ok((
            SliceHeader {
                nal_ref_idc: nal.nal_ref_idc,
                nal_unit_type: nal.nal_unit_type,
                idr,
                first_mb_in_slice,
                slice_type_raw,
                slice_type,
                pps_id,
                colour_plane_id,
                frame_num,
                field_pic,
                bottom_field,
                idr_pic_id,
                pic_order_cnt_lsb,
                delta_pic_order_cnt_bottom,
                delta_pic_order_cnt,
                redundant_pic_cnt,
                direct_spatial_mv_pred,
                num_ref_idx_active,
                ref_pic_list_mod,
                pred_weight_table,
                no_output_of_prior_pics,
                long_term_reference,
                adaptive_ref_pic_marking,
                mmcos,
                cabac_init_idc,
                slice_qp_delta,
                sp_for_switch,
                slice_qs_delta,
                disable_deblocking_filter_idc,
                slice_alpha_c0_offset_div2,
                slice_beta_offset_div2,
                slice_group_change_cycle,
                header_bits,
            },
            pps,
            sps,
        ))
    }
}

struct RefFields {
    ref_pic_list_mod: [Vec<RefPicListMod>; 2],
    pred_weight_table: Option<PredWeightTable>,
    no_output_of_prior_pics: bool,
    long_term_reference: bool,
    adaptive_ref_pic_marking: bool,
    mmcos: Vec<Mmco>,
}

fn parse_reference_fields(
    r: &mut BitReader<'_>,
    nal: NalHeader,
    idr: bool,
    slice_type: SliceType,
    pps: &Pps,
    sps: &Sps,
    num_ref_idx_active: [u32; 2],
) -> Result<RefFields> {
    let ref_pic_list_mod = parse_ref_pic_list_mod(r, slice_type)?;
    let pred_weight_table = parse_pred_weight_table(r, slice_type, pps, sps, num_ref_idx_active)?;
    let (no_output_of_prior_pics, long_term_reference, adaptive_ref_pic_marking, mmcos) = parse_ref_pic_marking(r, nal, idr)?;
    Ok(RefFields { ref_pic_list_mod, pred_weight_table, no_output_of_prior_pics, long_term_reference, adaptive_ref_pic_marking, mmcos })
}

fn parse_ref_pic_list_mod(r: &mut BitReader<'_>, slice_type: SliceType) -> Result<[Vec<RefPicListMod>; 2]> {
    let mut lists = [Vec::new(), Vec::new()];
    for (list, mods) in lists.iter_mut().enumerate() {
        let present = if list == 0 { !slice_type.is_intra() } else { slice_type.is_b() };
        if present && r.read_flag()? {
            loop {
                let idc = r.read_ue()?;
                ensure!(idc <= 5, "modification_of_pic_nums_idc out of range");
                if idc == 3 {
                    break;
                }
                mods.push(RefPicListMod { idc, value: r.read_ue()? });
                ensure!(mods.len() <= 66, "too many ref_pic_list_modification entries");
            }
        }
    }
    Ok(lists)
}

fn parse_pred_weight_table(r: &mut BitReader<'_>, slice_type: SliceType, pps: &Pps, sps: &Sps, active: [u32; 2]) -> Result<Option<PredWeightTable>> {
    let weighted = (pps.weighted_pred && slice_type.is_p()) || (pps.weighted_bipred_idc == 1 && slice_type.is_b());
    if !weighted {
        return Ok(None);
    }
    let luma_log2_denom = r.read_ue()?;
    ensure!(luma_log2_denom <= 7, "luma_log2_weight_denom out of range");
    let chroma_log2_denom = if sps.chroma_array_type() == 0 { 0 } else { r.read_ue()? };
    ensure!(chroma_log2_denom <= 7, "chroma_log2_weight_denom out of range");
    let mut lists = [Vec::new(), Vec::new()];
    let list_count = if slice_type.is_b() { 2 } else { 1 };
    for (list, entries) in lists.iter_mut().enumerate().take(list_count) {
        for _ in 0..active[list] {
            entries.push(parse_weight_entry(r, luma_log2_denom, chroma_log2_denom, sps.chroma_array_type() != 0)?);
        }
    }
    let [l0, l1] = lists;
    Ok(Some(PredWeightTable { luma_log2_denom, chroma_log2_denom, l0, l1 }))
}

fn parse_weight_entry(r: &mut BitReader<'_>, luma_denom: u32, chroma_denom: u32, has_chroma: bool) -> Result<WeightEntry> {
    let mut entry = WeightEntry { luma_weight: 1 << luma_denom, chroma_weight: [1 << chroma_denom; 2], ..Default::default() };
    if r.read_flag()? {
        entry.luma_flag = true;
        entry.luma_weight = r.read_se()?;
        entry.luma_offset = r.read_se()?;
        ensure!((-128..=127).contains(&entry.luma_weight) && (-128..=127).contains(&entry.luma_offset), "luma weight out of range");
    }
    if has_chroma && r.read_flag()? {
        entry.chroma_flag = true;
        for index in 0..2 {
            entry.chroma_weight[index] = r.read_se()?;
            entry.chroma_offset[index] = r.read_se()?;
            ensure!(
                (-128..=127).contains(&entry.chroma_weight[index]) && (-128..=127).contains(&entry.chroma_offset[index]),
                "chroma weight out of range"
            );
        }
    }
    Ok(entry)
}

fn parse_ref_pic_marking(r: &mut BitReader<'_>, nal: NalHeader, idr: bool) -> Result<(bool, bool, bool, Vec<Mmco>)> {
    let (mut no_output, mut long_term, mut adaptive) = (false, false, false);
    let mut mmcos = Vec::new();
    if nal.nal_ref_idc == 0 {
        return Ok((no_output, long_term, adaptive, mmcos));
    }
    if idr {
        no_output = r.read_flag()?;
        long_term = r.read_flag()?;
    } else {
        adaptive = r.read_flag()?;
        if adaptive {
            parse_mmcos(r, &mut mmcos)?;
        }
    }
    Ok((no_output, long_term, adaptive, mmcos))
}

fn parse_mmcos(r: &mut BitReader<'_>, mmcos: &mut Vec<Mmco>) -> Result<()> {
    loop {
        let op = r.read_ue()?;
        ensure!(op <= 6, "memory_management_control_operation out of range");
        if op == 0 {
            return Ok(());
        }
        let mut m = Mmco { op, difference_of_pic_nums_minus1: 0, long_term_pic_num: 0, long_term_frame_idx: 0, max_long_term_frame_idx_plus1: 0 };
        if op == 1 || op == 3 {
            m.difference_of_pic_nums_minus1 = r.read_ue()?;
        }
        if op == 2 {
            m.long_term_pic_num = r.read_ue()?;
        }
        if op == 3 || op == 6 {
            m.long_term_frame_idx = r.read_ue()?;
        }
        if op == 4 {
            m.max_long_term_frame_idx_plus1 = r.read_ue()?;
        }
        mmcos.push(m);
        ensure!(mmcos.len() <= 66, "too many MMCOs");
    }
}

/// POC decoding state carried between pictures (8.2.1).
#[derive(Clone, Debug, Default)]
pub struct PocState {
    /// PicOrderCntMsb / pic_order_cnt_lsb of the previous reference picture (type 0).
    prev_ref_msb: i32,
    prev_ref_lsb: i32,
    /// FrameNumOffset / frame_num of the previous picture (types 1 and 2).
    prev_frame_num_offset: i32,
    prev_frame_num: u32,
}

/// Output of POC computation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Poc {
    pub top: i32,
    pub bottom: i32,
    msb: i32,
    frame_num_offset: i32,
}

impl Poc {
    pub fn frame(&self) -> i32 {
        self.top.min(self.bottom)
    }
}

impl PocState {
    /// Compute TopFieldOrderCnt/BottomFieldOrderCnt of a frame (8.2.1.1-8.2.1.3).
    pub fn compute(&self, sh: &SliceHeader, sps: &Sps) -> Poc {
        match sps.pic_order_cnt_type {
            0 => {
                let (prev_msb, prev_lsb) = if sh.idr { (0, 0) } else { (self.prev_ref_msb, self.prev_ref_lsb) };
                let max_lsb = 1i32 << sps.log2_max_poc_lsb;
                let lsb = sh.pic_order_cnt_lsb as i32;
                let msb = if lsb < prev_lsb && prev_lsb - lsb >= max_lsb / 2 {
                    prev_msb + max_lsb
                } else if lsb > prev_lsb && lsb - prev_lsb > max_lsb / 2 {
                    prev_msb - max_lsb
                } else {
                    prev_msb
                };
                let top = msb + lsb;
                let bottom = top + sh.delta_pic_order_cnt_bottom;
                Poc { top, bottom, msb, frame_num_offset: 0 }
            }
            1 => {
                let frame_num_offset = self.frame_num_offset(sh.idr, sh.frame_num, sps);
                let n = sps.offset_for_ref_frame.len() as i32;
                let mut abs_frame_num = if n != 0 { frame_num_offset + sh.frame_num as i32 } else { 0 };
                if sh.nal_ref_idc == 0 && abs_frame_num > 0 {
                    abs_frame_num -= 1;
                }
                let mut expected = 0i32;
                if abs_frame_num > 0 {
                    let cycle_cnt = (abs_frame_num - 1) / n;
                    let in_cycle = (abs_frame_num - 1) % n;
                    let delta_cycle: i32 = sps.offset_for_ref_frame.iter().sum();
                    expected = cycle_cnt.wrapping_mul(delta_cycle);
                    for &o in &sps.offset_for_ref_frame[..=in_cycle as usize] {
                        expected += o;
                    }
                }
                if sh.nal_ref_idc == 0 {
                    expected += sps.offset_for_non_ref_pic;
                }
                let top = expected + sh.delta_pic_order_cnt[0];
                let bottom = top + sps.offset_for_top_to_bottom_field + sh.delta_pic_order_cnt[1];
                Poc { top, bottom, msb: 0, frame_num_offset }
            }
            _ => {
                let frame_num_offset = self.frame_num_offset(sh.idr, sh.frame_num, sps);
                let temp = if sh.idr {
                    0
                } else if sh.nal_ref_idc == 0 {
                    2 * (frame_num_offset + sh.frame_num as i32) - 1
                } else {
                    2 * (frame_num_offset + sh.frame_num as i32)
                };
                Poc { top: temp, bottom: temp, msb: 0, frame_num_offset }
            }
        }
    }

    fn frame_num_offset(&self, idr: bool, frame_num: u32, sps: &Sps) -> i32 {
        if idr {
            0
        } else if self.prev_frame_num > frame_num {
            self.prev_frame_num_offset + sps.max_frame_num() as i32
        } else {
            self.prev_frame_num_offset
        }
    }

    /// Update after a picture has been decoded. `poc` is the value returned by [`PocState::compute`]
    /// (before any MMCO5 adjustment).
    pub fn update(&mut self, sh: &SliceHeader, poc: &Poc) {
        let mmco5 = sh.has_mmco5();
        if sh.nal_ref_idc != 0 {
            if mmco5 {
                self.prev_ref_msb = 0;
                self.prev_ref_lsb = poc.top - poc.frame();
            } else {
                self.prev_ref_msb = poc.msb;
                self.prev_ref_lsb = sh.pic_order_cnt_lsb as i32;
            }
        }
        if mmco5 {
            self.prev_frame_num_offset = 0;
            self.prev_frame_num = 0;
        } else {
            self.prev_frame_num_offset = poc.frame_num_offset;
            self.prev_frame_num = sh.frame_num;
        }
    }

    /// Account for a "non-existing" frame inferred by the frame_num gap process (8.2.5.2).
    pub fn update_gap_frame(&mut self, frame_num: u32, sps: &Sps) {
        let off = self.frame_num_offset(false, frame_num, sps);
        self.prev_frame_num_offset = off;
        self.prev_frame_num = frame_num;
    }
}
