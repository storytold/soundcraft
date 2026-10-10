//! Multilingual names survive text entry in the real rename dialogs and session serialization.

use egui::{Event, Key, Modifiers};
use soundcraft_ui_egui::{Services, SoundApp};

fn frame(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>) {
    ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    })
    .textures_delta
    .clear();
}

fn enter_name(ctx: &egui::Context, app: &mut SoundApp, name: &str) {
    for _ in 0..3 {
        frame(ctx, app, vec![]);
    }
    frame(ctx, app, vec![Event::Text(name.into())]);
    frame(ctx, app, vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    assert!(app.dialogs.open.is_none());
}

#[test]
fn multilingual_track_and_clip_names_can_be_entered_and_saved() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let track = app.engine.session().track_by_name("Kick").unwrap();
    let track_id = track.id;
    let clip_id = track.clips().first().unwrap().id;
    let ctx = egui::Context::default();

    for name in [
        "ร้องนำ - น้ำเสียง ๑",
        "录音轨道 / 錄音軌道",
        "ボーカル 録音",
        "보컬 녹음",
        "تسجيل الصوت",
        "הקלטת שירה",
        "स्वर रिकॉर्डिंग",
        "কণ্ঠ রেকর্ডিং",
        "અવાજ રેકોર્ડિંગ",
        "ਆਵਾਜ਼ ਰਿਕਾਰਡਿੰਗ",
        "ସ୍ୱର ରେକର୍ଡିଂ",
        "குரல் பதிவு",
        "గాత్రం రికార్డింగ్",
        "ಧ್ವನಿ ರೆಕಾರ್ಡಿಂಗ್",
        "ശബ്ദം റെക്കോർഡിംഗ്",
        "ᱥᱟᱱᱛᱟᱲᱤ",
        "ꯃꯤꯇꯩ ꯂꯣꯟ",
        "හඬ පටිගත කිරීම",
        "བོད་ཡིག",
    ] {
        app.dialogs.open_rename_track(track_id, "");
        enter_name(&ctx, &mut app, name);
        assert_eq!(app.engine.session().track(track_id).unwrap().name, name);

        let clip_name = format!("{name} - take 2.wav");
        app.dialogs.open_rename_clip(clip_id, "");
        enter_name(&ctx, &mut app, &clip_name);
        assert_eq!(app.engine.session().find_clip(clip_id).unwrap().1.name, clip_name);

        let saved = app.engine.session().to_json().unwrap();
        let restored = soundcraft_model::Session::from_json(&saved).unwrap();
        assert_eq!(restored.track(track_id).unwrap().name, name);
        assert_eq!(restored.find_clip(clip_id).unwrap().1.name, clip_name);
    }
}
