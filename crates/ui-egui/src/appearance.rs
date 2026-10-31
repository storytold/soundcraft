//! The desktop's dark/light setting for the System theme on Linux, where eframe/winit reports no
//! system theme to egui.
//!
//! The XDG settings portal (`org.freedesktop.appearance` / `color-scheme`) is read once on a worker
//! thread, which then follows its `SettingChanged` signal and wakes the UI only when the value
//! changes. Nothing is polled. Elsewhere egui's own report (`Context::system_theme`) is enough.

/// egui's own report when it has one, otherwise the desktop's.
pub fn resolve(reported: Option<egui::Theme>, desktop: impl FnOnce() -> Option<egui::Theme>) -> Option<egui::Theme> {
    reported.or_else(desktop)
}

/// The desktop's appearance, or `None` when it is unknown or this platform has no portal.
/// Unit tests never ask the real desktop, so they do not depend on the machine's setting.
pub fn desktop_theme(ctx: &egui::Context) -> Option<egui::Theme> {
    #[cfg(all(target_os = "linux", not(test)))]
    {
        linux::current(ctx)
    }
    #[cfg(any(not(target_os = "linux"), test))]
    {
        let _ = ctx;
        None
    }
}

#[cfg(target_os = "linux")]
#[cfg_attr(test, allow(dead_code))]
mod linux {
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::{Arc, OnceLock};

    const PORTAL_NAMESPACE: &str = "org.freedesktop.appearance";
    const PORTAL_KEY: &str = "color-scheme";

    /// The last `color-scheme` seen: 0 = unknown or no preference, 1 = dark, 2 = light.
    static STATE: OnceLock<Arc<AtomicU8>> = OnceLock::new();

    pub fn current(ctx: &egui::Context) -> Option<egui::Theme> {
        let state = STATE.get_or_init(|| {
            let value = Arc::new(AtomicU8::new(0));
            let (worker_value, worker_ctx) = (Arc::clone(&value), ctx.clone());
            // If the thread cannot start, the value stays 0 and System keeps following egui only.
            let _ = std::thread::Builder::new().name("appearance-portal".into()).spawn(move || watch(&worker_value, &worker_ctx));
            value
        });
        decode(u32::from(state.load(Ordering::Relaxed)))
    }

    fn portal_proxy() -> Option<zbus::blocking::Proxy<'static>> {
        let conn = zbus::blocking::connection::Builder::session().ok()?.method_timeout(std::time::Duration::from_secs(2)).build().ok()?;
        zbus::blocking::Proxy::new(&conn, "org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop", "org.freedesktop.portal.Settings").ok()
    }

    fn watch(value: &AtomicU8, ctx: &egui::Context) {
        let Some(proxy) = portal_proxy() else { return };
        // Subscribe before Read so a change during startup is queued rather than lost.
        let Ok(signals) = proxy.receive_signal("SettingChanged") else { return };
        let initial: Option<zbus::zvariant::OwnedValue> = proxy.call("Read", &(PORTAL_NAMESPACE, PORTAL_KEY)).ok();
        publish(value, ctx, initial.and_then(portal_code).and_then(decode));
        // Blocks until the portal sends a signal: no polling.
        for message in signals {
            let Ok((namespace, key, changed)) = message.body().deserialize::<(String, String, zbus::zvariant::OwnedValue)>() else {
                continue;
            };
            if namespace == PORTAL_NAMESPACE && key == PORTAL_KEY {
                publish(value, ctx, portal_code(changed).and_then(decode));
            }
        }
    }

    fn publish(value: &AtomicU8, ctx: &egui::Context, theme: Option<egui::Theme>) {
        if value.swap(code_of(theme), Ordering::Relaxed) != code_of(theme) {
            ctx.request_repaint();
        }
    }

    /// The `u32` inside a portal value. `Settings.Read` answers a variant, and some portals nest
    /// another variant inside it.
    fn portal_code(value: zbus::zvariant::OwnedValue) -> Option<u32> {
        let mut value: zbus::zvariant::Value<'_> = value.into();
        for _ in 0..4 {
            match value {
                zbus::zvariant::Value::Value(inner) => value = *inner,
                other => return u32::try_from(other).ok(),
            }
        }
        None
    }

    /// The portal's `color-scheme`: 0 = no preference, 1 = prefer dark, 2 = prefer light.
    pub(super) fn decode(code: u32) -> Option<egui::Theme> {
        match code {
            1 => Some(egui::Theme::Dark),
            2 => Some(egui::Theme::Light),
            _ => None,
        }
    }

    fn code_of(theme: Option<egui::Theme>) -> u8 {
        match theme {
            Some(egui::Theme::Dark) => 1,
            Some(egui::Theme::Light) => 2,
            None => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_setting_is_used_only_when_egui_reports_nothing() {
        assert_eq!(resolve(None, || Some(egui::Theme::Light)), Some(egui::Theme::Light));
        assert_eq!(resolve(None, || None), None);
        assert_eq!(resolve(Some(egui::Theme::Dark), || Some(egui::Theme::Light)), Some(egui::Theme::Dark));
        let asked = std::cell::Cell::new(false);
        let theme = resolve(Some(egui::Theme::Light), || {
            asked.set(true);
            None
        });
        assert_eq!(theme, Some(egui::Theme::Light));
        assert!(!asked.get(), "must not ask the desktop");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn portal_color_scheme_codes() {
        assert_eq!(linux::decode(1), Some(egui::Theme::Dark));
        assert_eq!(linux::decode(2), Some(egui::Theme::Light));
        assert_eq!(linux::decode(0), None);
        assert_eq!(linux::decode(7), None);
    }
}
