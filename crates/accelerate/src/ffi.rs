//! The only unsafe boundary: checked slices passed to Apple Accelerate/vDSP.

use std::{ffi::c_void, ptr::NonNull};

#[link(name = "Accelerate", kind = "framework")]
unsafe extern "C" {
    fn vDSP_maxmgv(input: *const f32, stride: isize, output: *mut f32, count: usize);
    fn vDSP_DFT_zop_CreateSetup(previous: *mut c_void, length: usize, direction: i32) -> *mut c_void;
    fn vDSP_DFT_Execute(setup: *const c_void, input_real: *const f32, input_imag: *const f32, output_real: *mut f32, output_imag: *mut f32);
    fn vDSP_DFT_DestroySetup(setup: *mut c_void);
}

pub struct DftPlan(NonNull<c_void>);

impl DftPlan {
    pub fn new(length: usize) -> Option<Self> {
        // SAFETY: a null previous setup asks vDSP to create independent immutable plan data. The
        // caller validates the supported power-of-two length; null reports allocation/rejection.
        NonNull::new(unsafe { vDSP_DFT_zop_CreateSetup(std::ptr::null_mut(), length, 1) }).map(Self)
    }

    pub fn forward(&self, real: &[f32], imag: &[f32], output_real: &mut [f32], output_imag: &mut [f32]) {
        debug_assert_eq!(real.len(), imag.len());
        debug_assert_eq!(real.len(), output_real.len());
        debug_assert_eq!(real.len(), output_imag.len());
        // SAFETY: construction produced a valid forward plan for exactly this slice length. The
        // public safe wrapper verifies all lengths and owns disjoint output buffers. Per-thread
        // plan storage prevents concurrent execution of this setup as required by vDSP.
        unsafe { vDSP_DFT_Execute(self.0.as_ptr(), real.as_ptr(), imag.as_ptr(), output_real.as_mut_ptr(), output_imag.as_mut_ptr()) };
    }
}

impl Drop for DftPlan {
    fn drop(&mut self) {
        // SAFETY: this is the unique owner of a setup returned by CreateSetup; thread-local
        // storage ensures no execution can still borrow it while it is being dropped.
        unsafe { vDSP_DFT_DestroySetup(self.0.as_ptr()) };
    }
}

pub fn peak(values: &[f32]) -> f32 {
    let mut out = 0.0;
    // SAFETY: `values` covers `len` contiguous elements and `out` is valid for one result.
    unsafe { vDSP_maxmgv(values.as_ptr(), 1, &raw mut out, values.len()) };
    out
}
