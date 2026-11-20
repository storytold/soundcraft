use soundcraft_model::SourceAudio;
use std::convert::Infallible;
use std::path::Path;
use std::sync::Arc;

pub const STREAM_BLOCK: usize = 1 << 15;
pub const STREAM_CACHE_BLOCKS: usize = 32;
pub const OVERVIEW_FRAMES: usize = 256;

#[derive(Debug)]
pub struct StreamedAudio(Infallible);

impl StreamedAudio {
    pub fn frames(&self) -> u64 {
        match self.0 {}
    }

    pub fn channels(&self) -> usize {
        match self.0 {}
    }

    pub fn path(&self) -> &Path {
        match self.0 {}
    }

    pub fn overview(&self) -> &[Vec<(f32, f32)>] {
        match self.0 {}
    }
}

pub fn streamed(_: &SourceAudio) -> Option<Arc<StreamedAudio>> {
    None
}

pub fn source_frames(audio: &SourceAudio) -> usize {
    audio.frames()
}

pub fn whole(audio: &Arc<SourceAudio>) -> Result<Arc<SourceAudio>, String> {
    Ok(Arc::clone(audio))
}

pub fn source_span(audio: &SourceAudio, start: usize, len: usize) -> Result<Vec<Vec<f32>>, String> {
    Ok(audio.buffer.channels.iter().map(|c| c.iter().skip(start).take(len).copied().collect()).collect())
}

pub fn cue_streams(_: &soundcraft_model::Session, _: soundcraft_time::Samples) {}

pub fn open_streamed(_: &Path, _: u32, _: u64) -> Result<Option<Arc<SourceAudio>>, String> {
    Ok(None)
}

pub(crate) fn render_streamed(_: &SourceAudio, _: impl FnOnce(&mut dyn FnMut(usize, i64) -> f32), _: i64) -> bool {
    false
}
