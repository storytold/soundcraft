//! A stopped play start that does not replace the edit or timeline selection.

use super::*;
use crate::cmd;
use serde_json::json;

pub fn specs() -> Vec<CommandSpec> {
    let mut cue = cmd!(noundo "transport.set_play_start", "Set Play Start", [], None, "{at: position} — move the stopped cue without changing selections", always, |e, p| {
        let at = position_param(e, "transport.set_play_start", p, "at")?
            .ok_or_else(|| bad("transport.set_play_start", "`at` required"))?
            .clamp(0, soundcraft_time::MAX_POSITION);
        if e.session().edit.playhead != at {
            e.session_mut().edit.playhead = at;
        }
        Ok(json!({"at": at}))
    });
    // Continuous editor gestures must not grow the replay journal.
    cue.journal = false;
    vec![cue]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cue_preserves_selections_document_and_history_and_rejects_bad_input() {
        let mut e = crate::demo::demo_engine();
        e.mark_clean();
        let before = e.session().clone();
        let undo = e.undo_history().len();
        let journal = e.journal.len();
        e.execute("transport.set_play_start", &json!({"at": 123_456})).unwrap();
        assert_eq!(e.session().edit.playhead, 123_456);
        let mut expected = before;
        expected.edit.playhead = 123_456;
        assert_eq!(e.session(), &expected);
        assert!(!e.is_dirty());
        assert_eq!(e.undo_history().len(), undo);
        assert_eq!(e.journal.len(), journal);
        for p in [json!({}), json!({"at": "bad"}), json!({"at": null})] {
            assert!(e.execute("transport.set_play_start", &p).is_err(), "{p}");
            assert_eq!(e.session(), &expected);
        }
        e.execute("transport.set_play_start", &json!({"at": 1e100})).unwrap();
        assert_eq!(e.session().edit.playhead, soundcraft_time::MAX_POSITION);
        e.execute("transport.set_play_start", &json!({"at": -1e100})).unwrap();
        assert_eq!(e.session().edit.playhead, 0);
    }

    #[test]
    fn explicit_play_start_is_one_request_without_changing_selection() {
        let mut e = crate::demo::demo_engine();
        let selection = e.session().edit.selection;
        e.execute("transport.play", &json!({"from": 123_456})).unwrap();
        assert_eq!(e.transport_requests, vec![crate::TransportRequest::PlayFrom(123_456)]);
        assert_eq!(e.session().edit.selection, selection);
    }
}
