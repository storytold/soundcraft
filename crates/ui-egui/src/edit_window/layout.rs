use super::{Rect, Samples, Session, Track, pos2};

pub const HEADER_W: f32 = 136.0;

pub const COLUMN_W: f32 = 112.0;

/// Insert, send and instrument columns: Pro Tools' narrow stacks of slots.
const PLUGIN_COLUMN_W: f32 = 75.0;

/// The track colour strip at the left of each track header.
pub(super) const STRIP_W: f32 = 14.0;

/// The row under the rulers that holds the column titles, when any column is shown.
pub(super) const COLUMN_TITLES_H: f32 = 16.0;

/// An optional Edit-window column: view id, menu label, title and width.
pub(super) type Column = (&'static str, &'static str, &'static str, f32);

pub(super) const COLUMNS: [Column; 7] = [
    ("instruments", "Instruments", "INSTRUMENT", PLUGIN_COLUMN_W),
    ("inserts_ae", "Inserts A-E", "INSERTS A-E", PLUGIN_COLUMN_W),
    ("inserts_fj", "Inserts F-J", "INSERTS F-J", PLUGIN_COLUMN_W),
    ("sends_ae", "Sends A-E", "SENDS A-E", PLUGIN_COLUMN_W),
    ("sends_fj", "Sends F-J", "SENDS F-J", PLUGIN_COLUMN_W),
    ("io", "I/O", "I/O", 94.0),
    ("comments", "Comments", "COMMENTS", COLUMN_W),
];

/// Optional Edit-window columns (View › Edit Window Views) that are switched on, left to right.
pub fn edit_columns(s: &Session) -> Vec<Column> {
    let all = s.edit.flag("edit_view.all");
    COLUMNS.into_iter().filter(|c| all || s.edit.flag(&format!("edit_view.{}", c.0))).collect()
}

pub fn header_width(cols: &[Column]) -> f32 {
    HEADER_W + cols.iter().map(|c| c.3).sum::<f32>()
}

pub const RULER_H: f32 = 17.0;

/// Geometry of the last frame, for hit testing and agents (`ui.inspect`).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct EditLayout {
    pub timeline: [f32; 4],
    pub rows: Vec<(u64, [f32; 4])>,
    pub scroll_y: f32,
    pub content_h: f32,
    /// While Zoom Toggle is on: the vertical scroll to go back to.
    pub(super) zoom_back: Option<f32>,
    /// Follow-hold: the view was moved by something other than the playhead
    /// follow (a manual pan, scrollbar, universe jump, or programmatic
    /// scroll) during playback, so timeline auto-scroll stays paused until
    /// the playhead is scrolled back into view (or the transport
    /// stops/starts, or a scrolling mode is re-selected). Lets the timeline
    /// be inspected anywhere while playing.
    pub follow_hold: bool,
    /// Session `zoom.scroll` seen by the last overlay frame, for telling our
    /// own follow jumps apart from outside view moves.
    pub last_scroll: Samples,
    /// Follow target issued by the last overlay frame (`None` when none).
    pub last_follow_to: Option<Samples>,
    #[serde(skip)]
    pub scroll_carry: f64,
    /// The current swipe's movement per axis until either passes 6 points; it scrolls only along the larger.
    #[serde(skip)]
    pub(super) swipe: egui::Vec2,
}

impl EditLayout {
    pub fn scroll_by(&mut self, dy: f32, view_h: f32) {
        self.scroll_y = (self.scroll_y + dy).clamp(0.0, (self.content_h - view_h + 40.0).max(0.0));
    }

    /// Clears the vertical scroll position Zoom Toggle would restore, so Cancel Zoom Toggle keeps the current scroll.
    pub(crate) fn forget_zoom_back(&mut self) {
        self.zoom_back = None;
    }
}

pub fn rect([x0, y0, x1, y1]: [f32; 4]) -> Rect {
    Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
}

pub(super) fn lane_rect(track: &Track, row: Rect, tl: Rect) -> Rect {
    Rect::from_min_max(pos2(tl.min.x, row.min.y), pos2(tl.max.x, row.min.y + track.height.points()))
}

pub fn x_of(s: &Session, tl: Rect, at: Samples) -> f32 {
    tl.min.x + ((at - s.edit.zoom.scroll) as f64 / s.edit.zoom.samples_per_px.max(0.01)) as f32
}

pub fn sample_at(s: &Session, tl: Rect, x: f32) -> Samples {
    s.edit.zoom.scroll + soundcraft_time::to_samples(f64::from(x - tl.min.x) * s.edit.zoom.samples_per_px)
}

pub const LANE_H: f32 = 46.0;

pub fn track_height(t: &Track) -> f32 {
    let alt = if t.view == "playlists" { t.playlists.len().saturating_sub(1) as f32 * LANE_H } else { 0.0 };
    t.height.points() + alt
}
