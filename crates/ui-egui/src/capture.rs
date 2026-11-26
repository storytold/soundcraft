//! One-shot completion slots for `ui.screenshot` captures.
//!
//! The app keeps request ids, paths, deadlines, settling and budgets in its request record
//! (`control::PendingShot`). A slot only correlates one capture with one record, checks the image against
//! a per-image byte bound, and wakes the owning viewport.
//!
//! [`CaptureSlot::new`] sends nothing: send its callback with [`egui::ViewportCommand::Screenshot`]
//! when the capture is due. The callback holds only a weak reference to the slot, so dropping the
//! slot releases its image and makes a late result inert. Every retry needs a
//! fresh slot and callback.
//!
//! The slot holds a strong [`egui::Context`]: keep it in the app's request record, never in
//! `Context`-owned data, which would form a cycle.

use std::mem::size_of;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use egui::{Color32, ColorImage, Context, ScreenshotCallback, ViewportId};

/// Why a completed screenshot was rejected and released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureError {
    /// The checked pixel count, or its byte count, overflowed `usize`.
    DimensionsOverflow { width: usize, height: usize },
    /// The pixel buffer does not match the dimensions, or the image has no area.
    MalformedImage { width: usize, height: usize, pixels: usize },
    /// The image is larger than the slot's per-image byte bound.
    ImageTooLarge { bytes: usize, max_bytes: usize },
}

impl core::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DimensionsOverflow { width, height } => {
                write!(f, "screenshot dimensions {width}×{height} overflow the checked pixel or byte count")
            }
            Self::MalformedImage { width, height, pixels } => write!(f, "screenshot has {pixels} pixels for its {width}×{height} dimensions"),
            Self::ImageTooLarge { bytes, max_bytes } => write!(f, "screenshot uses {bytes} bytes, over the {max_bytes}-byte per-image limit"),
        }
    }
}

impl std::error::Error for CaptureError {}

enum SlotState {
    Pending,
    Ready(Result<Arc<ColorImage>, CaptureError>),
    Taken,
    Cancelled,
}

struct Shared {
    state: Mutex<SlotState>,
    /// The live request owns the strong context; the callback only upgrades it briefly to wake.
    context: Context,
    viewport: ViewportId,
    max_image_bytes: usize,
}

/// The live request side of a capture. Dropping it cancels the completion and releases any
/// retained image.
#[must_use = "the slot must be kept alive to receive and take the screenshot"]
pub(crate) struct CaptureSlot {
    shared: Arc<Shared>,
}

impl CaptureSlot {
    /// A completion slot for a capture of `viewport` and its one-shot callback. `max_image_bytes`
    /// bounds this one image; the app reserves its aggregate budget itself.
    pub(crate) fn new(context: &Context, viewport: ViewportId, max_image_bytes: usize) -> (Self, ScreenshotCallback) {
        let shared = Arc::new(Shared { state: Mutex::new(SlotState::Pending), context: context.clone(), viewport, max_image_bytes });
        let weak = Arc::downgrade(&shared);
        let callback = ScreenshotCallback::new(move |image| complete(&weak, image));
        (Self { shared }, callback)
    }

    /// The completed result, at most once; `None` while pending and after an
    /// earlier take. The request record knows which.
    pub(crate) fn take(&self) -> Option<Result<Arc<ColorImage>, CaptureError>> {
        let mut state = lock(&self.shared.state);
        if !matches!(&*state, SlotState::Ready(_)) {
            return None;
        }
        match core::mem::replace(&mut *state, SlotState::Taken) {
            SlotState::Ready(result) => Some(result),
            _ => None,
        }
    }
}

impl Drop for CaptureSlot {
    fn drop(&mut self) {
        close(&self.shared);
    }
}

impl core::fmt::Debug for CaptureSlot {
    /// The viewport only: logging never holds or dumps screenshot contents.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CaptureSlot").field("viewport", &self.shared.viewport).finish_non_exhaustive()
    }
}

/// Locks the slot state, recovering from poisoning. Release the guard before calling into the
/// context: a repaint observer may take or cancel re-entrantly.
fn lock(state: &Mutex<SlotState>) -> MutexGuard<'_, SlotState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Marks the slot closed; the retained image drops after the mutex is released.
fn close(shared: &Shared) {
    let previous = {
        let mut state = lock(&shared.state);
        core::mem::replace(&mut *state, SlotState::Cancelled)
    };
    drop(previous);
}

/// Checks and stores a completed image, then wakes the owning viewport, with the slot lock
/// released. A cancelled, taken or dropped slot stores nothing and does not wake.
fn complete(weak: &Weak<Shared>, image: Arc<ColorImage>) {
    let Some(shared) = weak.upgrade() else { return };
    let result = checked_image(image, shared.max_image_bytes);
    let wake = {
        let mut state = lock(&shared.state);
        if matches!(&*state, SlotState::Pending) {
            *state = SlotState::Ready(result);
            true
        } else {
            false
        }
    };
    if wake {
        shared.context.request_repaint_of(shared.viewport);
    }
}

/// `image` when it has area, its buffer matches its size, and its byte count (checked) is within
/// `max_image_bytes`.
fn checked_image(image: Arc<ColorImage>, max_image_bytes: usize) -> Result<Arc<ColorImage>, CaptureError> {
    let [width, height] = image.size;
    if width == 0 || height == 0 {
        return Err(CaptureError::MalformedImage { width, height, pixels: image.pixels.len() });
    }
    let pixels = width.checked_mul(height).ok_or(CaptureError::DimensionsOverflow { width, height })?;
    let bytes = pixels.checked_mul(size_of::<Color32>()).ok_or(CaptureError::DimensionsOverflow { width, height })?;
    if image.pixels.len() != pixels {
        return Err(CaptureError::MalformedImage { width, height, pixels: image.pixels.len() });
    }
    if bytes > max_image_bytes {
        return Err(CaptureError::ImageTooLarge { bytes, max_bytes: max_image_bytes });
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(size: [usize; 2]) -> Arc<ColorImage> {
        Arc::new(ColorImage::filled(size, Color32::RED))
    }

    fn observe_repaints(ctx: &Context) -> Arc<Mutex<Vec<ViewportId>>> {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = requests.clone();
        ctx.set_repaint_observer(move |info| observed.lock().unwrap().push(info.viewport_id));
        requests
    }

    #[test]
    fn a_completion_wakes_its_viewport_and_is_taken_once() {
        let ctx = Context::default();
        let requests = observe_repaints(&ctx);
        let viewport = ViewportId::from_hash_of("window");
        let (slot, callback) = CaptureSlot::new(&ctx, viewport, 1024);
        assert!(slot.take().is_none(), "pending");
        let shot = image([2, 1]);
        callback.complete(shot.clone());
        assert!(Arc::ptr_eq(&slot.take().unwrap().unwrap(), &shot));
        assert!(slot.take().is_none(), "taken once");
        assert_eq!(*requests.lock().unwrap(), [viewport]);
    }

    #[test]
    fn a_dropped_slot_releases_a_late_image_without_waking() {
        let ctx = Context::default();
        let requests = observe_repaints(&ctx);
        let (slot, callback) = CaptureSlot::new(&ctx, ViewportId::ROOT, 1024);
        drop(slot);
        let late = image([2, 2]);
        let weak = Arc::downgrade(&late);
        callback.complete(late);
        assert!(weak.upgrade().is_none());
        assert!(requests.lock().unwrap().is_empty());
    }

    #[test]
    fn oversized_malformed_and_overflowing_images_are_errors() {
        let ctx = Context::default();
        let (slot, callback) = CaptureSlot::new(&ctx, ViewportId::ROOT, 16);
        callback.complete(image([3, 3]));
        assert!(matches!(slot.take(), Some(Err(CaptureError::ImageTooLarge { bytes: 36, max_bytes: 16 }))));
        let (slot, callback) = CaptureSlot::new(&ctx, ViewportId::ROOT, 1024);
        callback.complete(Arc::new(ColorImage { size: [2, 2], source_size: egui::Vec2::ZERO, pixels: vec![Color32::RED; 3] }));
        assert!(matches!(slot.take(), Some(Err(CaptureError::MalformedImage { width: 2, height: 2, pixels: 3 }))));
        let (slot, callback) = CaptureSlot::new(&ctx, ViewportId::ROOT, 1024);
        callback.complete(Arc::new(ColorImage { size: [usize::MAX, 2], source_size: egui::Vec2::ZERO, pixels: Vec::new() }));
        assert!(matches!(slot.take(), Some(Err(CaptureError::DimensionsOverflow { .. }))));
    }
}
