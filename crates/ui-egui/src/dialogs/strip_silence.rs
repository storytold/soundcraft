use super::{Arc, ClipId, Dialog, SoundApp, StripNames, Tokens, TrackId, Value, json, vec2};

/// What Strip keeps on each track: the Strip Silence rectangles.
pub type StripRects = Arc<Vec<(TrackId, Vec<soundcraft_time::Range>)>>;

/// The session revision and the Strip Silence values (as bits) that rectangles were found for.
type StripKey = (u64, [u64; 4]);

/// The Strip Silence rectangles last found, and the scan running for newer values.
#[derive(Default)]
pub(super) struct StripRectangles {
    shown: Option<(StripKey, StripRects)>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) running: Option<StripScanning>,
}

/// A Strip Silence scan on a worker thread: what it was started for, where its result arrives, and
/// the flag that stops it.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct StripScanning {
    key: StripKey,
    rx: std::sync::mpsc::Receiver<StripRects>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

/// A scan whose rectangles nobody will draw stops before its next window of audio.
#[cfg(not(target_arch = "wasm32"))]
impl Drop for StripScanning {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The ranges Strip keeps on each selected track, found by the scan of `edit.strip_silence_preview`
/// with the values of the open Strip Silence window (or of the one Rename Selected Clips goes back
/// to). The Edit window draws them as the Strip Silence rectangles. Empty when neither window is
/// open. A scan runs again only when the session or a value changes. Off the web it runs on a worker
/// thread, one at a time, and the rectangles last found stay until it ends. On the web it runs on the
/// frame thread once no pointer button is down, so a slider drag scans once, on release. A scan for
/// older values, or one that runs on after the window closes, stops before its next window of audio.
pub fn strip_silence_rectangles(app: &mut SoundApp, ctx: &egui::Context) -> StripRects {
    let values = match &app.dialogs.open {
        Some(Dialog::StripSilence { threshold, min_ms, pre_ms, post_ms }) => [*threshold, *min_ms, *pre_ms, *post_ms],
        Some(Dialog::StripSilenceRename { strip, .. }) => *strip,
        _ => {
            app.dialogs.strip_rectangles = StripRectangles::default();
            return Arc::default();
        }
    };
    let key: StripKey = (app.engine.revision, values.map(f64::to_bits));
    let r = &mut app.dialogs.strip_rectangles;
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(job) = &r.running {
        match job.rx.try_recv() {
            Ok(rects) => {
                r.shown = Some((job.key, rects));
                r.running = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => r.running = None,
            // Values changed since it started: stop it, and the next scan starts when it has ended.
            Err(std::sync::mpsc::TryRecvError::Empty) if job.key != key => job.stop.store(true, std::sync::atomic::Ordering::Relaxed),
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }
    let shown = r.shown.as_ref().map(|(k, rects)| (*k == key, Arc::clone(rects)));
    #[cfg(not(target_arch = "wasm32"))]
    let busy = r.running.is_some();
    // The browser's scan blocks the frame, so it waits for the release that ends a slider drag.
    #[cfg(target_arch = "wasm32")]
    let busy = ctx.input(|i| i.pointer.any_down());
    if let Some((true, rects)) = shown {
        return rects;
    }
    if busy {
        return shown.map(|(_, rects)| rects).unwrap_or_default();
    }
    let [threshold, min_ms, pre_ms, post_ms] = values;
    let params = json!({"threshold_db": threshold, "min_length_ms": min_ms, "pad_before_ms": pre_ms, "pad_after_ms": post_ms});
    let Ok(scan) = soundcraft_engine::cmd::strip_silence_scan(&app.engine, &params) else {
        app.dialogs.strip_rectangles.shown = Some((key, Arc::default()));
        return Arc::default();
    };
    let scan = Arc::new(scan);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        let (ctx, job) = (ctx.clone(), Arc::clone(&scan));
        let work = move || {
            let found = job.run();
            // A stopped scan sends nothing: its sender drops, and the rectangles last found stay.
            if !job.stop.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = tx.send(Arc::new(found.unwrap_or_default()));
            }
            ctx.request_repaint();
        };
        if std::thread::Builder::new().name("strip-silence-scan".into()).spawn(work).is_ok() {
            app.dialogs.strip_rectangles.running = Some(StripScanning { key, rx, stop: Arc::clone(&scan.stop) });
            return shown.map(|(_, rects)| rects).unwrap_or_default();
        }
    }
    // The browser has no threads, and without a thread to spare the scan runs on this one.
    let rects: StripRects = Arc::new(scan.run().unwrap_or_default());
    app.dialogs.strip_rectangles.shown = Some((key, Arc::clone(&rects)));
    rects
}

/// The command a Strip Silence button runs: `mode` is strip, extract or separate, and `values` holds
/// the threshold in dB, the minimum strip duration and the clip start and end pads in milliseconds.
/// `names` names the clips it leaves when its name is not blank.
pub(super) fn strip_silence_action(mode: &str, values: [f64; 4], names: &StripNames) -> (String, Value) {
    let [threshold, min_ms, pre_ms, post_ms] = values;
    let mut params = json!({"threshold_db": threshold, "min_length_ms": min_ms, "pad_before_ms": pre_ms, "pad_after_ms": post_ms, "mode": mode});
    if !names.name.trim().is_empty() {
        params["name"] = json!(names.name);
        params["number"] = json!(names.number);
        params["zeros"] = json!(names.zeros);
        params["suffix"] = json!(names.suffix);
    }
    ("edit.strip_silence".into(), params)
}

/// The command Auto Rename's OK runs: the scheme for `clips`, or no scheme when the name is blank.
pub(super) fn auto_rename_action(clips: &[ClipId], names: &StripNames) -> (String, Value) {
    let mut params = json!({"clips": clips});
    if !names.name.trim().is_empty() {
        params["name"] = json!(names.name);
        params["number"] = json!(names.number);
        params["zeros"] = json!(names.zeros);
        params["suffix"] = json!(names.suffix);
    }
    ("clip.auto_rename".into(), params)
}

/// The labels of the number and zeros fields in Strip Silence's Rename Selected Clips.
pub(super) const STRIP_NAME_LABELS: [&str; 2] = ["Number", "Zeros"];

/// The labels of the number and zeros fields in Auto Rename's Rename Selected Clips.
pub(super) const AUTO_NAME_LABELS: [&str; 2] = ["Starting Number", "Number of Places"];

/// The Name, number, zeros and Suffix fields of Rename Selected Clips, with the first name they
/// give. `labels` names the number and zeros fields.
pub(super) fn strip_name_fields(ui: &mut egui::Ui, names: &mut StripNames, labels: [&str; 2]) {
    let [number, zeros] = labels;
    egui::Grid::new("strip_names").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        let name = ui.label("Name");
        ui.text_edit_singleline(&mut names.name).labelled_by(name.id);
        ui.end_row();
        let number = ui.label(number);
        ui.add(egui::DragValue::new(&mut names.number).range(0..=StripNames::MAX_NUMBER)).labelled_by(number.id);
        ui.end_row();
        let zeros = ui.label(zeros);
        ui.add(egui::DragValue::new(&mut names.zeros).range(0..=StripNames::MAX_ZEROS)).labelled_by(zeros.id);
        ui.end_row();
        let suffix = ui.label("Suffix");
        ui.text_edit_singleline(&mut names.suffix).labelled_by(suffix.id);
        ui.end_row();
    });
    let example = if names.name.trim().is_empty() {
        "Without a name, the clips keep their names.".to_string()
    } else {
        format!("First clip: {}", names.clip_name(0))
    };
    ui.label(egui::RichText::new(example).small().color(Tokens::current().text_dim));
}
