//! SoundCraft's optional provider bridge keeps the native .scraftz project and all source audio.
use soundcraft_engine::{Engine, io};
use soundcraft_model::Session;
use soundcraft_ui_egui::{
    SoundApp,
    file_providers::{Capture, Command},
};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Default)]
pub struct Runtime {
    checkpoints: HashMap<(u64, u64), Arc<Session>>,
}

impl Runtime {
    pub fn tick(&mut self, app: &mut SoundApp) {
        let host = app.services.file_providers.clone();
        for command in host.take_commands() {
            match command {
                Command::Save { request, doc, destination, name, save_as } => {
                    if doc != soundcraft_ui_egui::provider_ui::document_id(app) {
                        host.failed(&request, "The document was replaced before capture");
                        continue;
                    }
                    app.capture_plugin_states();
                    let revision = app.engine.revision;
                    let snapshot = app.engine.session_arc();
                    let capture_bytes = if destination == "device" {
                        io::session_bundle(&snapshot).map(|(bytes, _)| bytes)
                    } else {
                        io::session_provider_bundle(&snapshot)
                    };
                    let result = capture_bytes.map_err(|e| e.to_string()).and_then(|bytes| {
                        if destination == "device" {
                            let native_name = std::path::Path::new(&name).with_extension("scraftz").to_string_lossy().into_owned();
                            super::web::download(&native_name, &bytes)?;
                            host.finish_device(&request);
                        } else {
                            host.capture(&request, Capture { doc, revision, name, save_as }, bytes)?;
                            self.checkpoints.insert((doc, revision), snapshot);
                        }
                        Ok(())
                    });
                    if let Err(error) = result {
                        host.failed(&request, &error);
                        app.ui.status = error;
                    }
                }
                Command::Result { capture, resource, status } => {
                    let snapshot = self.checkpoints.remove(&(capture.doc, capture.revision));
                    if status == "saved" && capture.doc == soundcraft_ui_egui::provider_ui::document_id(app) {
                        host.bind(&capture, resource);
                        if let Some(snapshot) = snapshot {
                            app.engine.acknowledge_external_save(snapshot);
                        }
                    }
                }
                Command::Open { request, resource: _, name, bytes } => {
                    let result = if app.engine.is_dirty() || host.pending_for(soundcraft_ui_egui::provider_ui::document_id(app)) {
                        Err("Save the current session, or close it explicitly, before opening a provider session".into())
                    } else {
                        let mut decoded = Engine::default();
                        io::open_provider_session_bytes(&mut decoded, &name, &bytes).map_err(|e| e.to_string()).map(|_| {
                            soundcraft_ui_egui::provider_ui::install_session(app, decoded.session().clone());
                        })
                    };
                    match result {
                        Ok(()) => host.opened(
                            &request,
                            Some(Capture {
                                doc: soundcraft_ui_egui::provider_ui::document_id(app),
                                revision: app.engine.revision,
                                name,
                                save_as: false,
                            }),
                            "",
                        ),
                        Err(error) => {
                            host.opened(&request, None, &error);
                            app.ui.status = error;
                        }
                    }
                }
            }
        }
        soundcraft_ui_egui::provider_ui::publish(app);
        crate::file_providers::publish(&host);
    }
}
