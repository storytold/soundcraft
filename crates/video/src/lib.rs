//! SoundCraft video: open a movie (MP4 / QuickTime MOV) from bytes, report its picture format, and
//! decode the frame at a time to RGBA for the Video window and the Video track's thumbnails.
//!
//! - [`Movie::open`] (bytes) / [`Movie::open_source`] (any random-access [`ByteSource`]).
//! - [`Movie::info`]: size, duration, frame rate, codec, whether it can be decoded.
//! - [`Movie::frame_at`]: the frame shown at a time, decoded forward from the previous keyframe,
//!   with a small LRU of decoded frames (sequential playback keeps decoding forward).
//! - [`Movie::thumbnail_at`] / [`Movie::thumbnails`]: low-resolution timeline pictures (keyframes).
//! - Unsupported codecs open fine (so the timeline can show the clip) but frame requests return
//!   [`VideoError::Unsupported`]. Malformed files return errors; nothing here panics.
//!
//! Picture codecs: H.264 / AVC (Baseline, Main, High; progressive 8-bit 4:2:0; CAVLC and CABAC),
//! Apple ProRes (422 Proxy/LT/Standard/HQ, 4444/XQ) and Motion JPEG (`jpeg`/`mjpa`).
//!
//! ## Origin
//!
//! [`bitstream`] and [`isobmff`] are re-exports of `craft_media::bitstream` and
//! `craft_media::isobmff` from craft-libs (`storytold/craft-libs`, the shared crate FilmCraft uses
//! too). The H.264 and ProRes code is a copy of the clean-room crates of our sibling app FilmCraft
//! (`storytold/filmcraft`, same organisation, MIT OR Apache-2.0): `filmcraft-h264` → [`h264`]
//! (without its test-only synthetic stream module), `filmcraft-prores` → [`prores`]. Only the
//! module paths were changed; fixes found here should be offered back upstream. [`Movie`], the
//! pixel conversion and the frame caches are SoundCraft's own.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
// Codec code indexes fixed-size blocks; index loops read more clearly than iterator chains there.
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

pub mod convert;
pub mod h264;
mod movie;
pub mod prores;
pub mod synth;

pub use craft_media::{bitstream, isobmff};

pub use isobmff::ByteSource;
pub use movie::{Frame, Movie, MovieInfo, SharedSource, fit_size};

/// Errors from opening or decoding a movie.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum VideoError {
    #[error("not a readable movie: {0}")]
    Container(String),
    #[error("the movie has no video track")]
    NoVideoTrack,
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("could not decode the picture: {0}")]
    Decode(String),
    #[error("no picture at that time")]
    OutOfRange,
}

pub type Result<T> = std::result::Result<T, VideoError>;

/// Open a movie and return its description (convenience for importers).
pub fn probe(bytes: &[u8]) -> Result<MovieInfo> {
    Movie::open(bytes.to_vec()).map(|m| m.info().clone())
}

#[cfg(test)]
mod tests;
