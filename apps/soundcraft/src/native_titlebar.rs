//! Center main-window titles using public AppKit titlebar accessories.
//! The real window title remains available to accessibility and the Window menu.

use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{
    NSApplication, NSColor, NSFont, NSLayoutAttribute, NSLineBreakMode, NSTextAlignment, NSTextField, NSTitlebarAccessoryViewController, NSView,
    NSWindow, NSWindowTitleVisibility,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use std::{cell::RefCell, collections::HashMap};

struct Titlebar {
    view: Retained<NSView>,
    label: Retained<NSTextField>,
    label_height: f64,
}

thread_local! {
    static TITLES: RefCell<HashMap<isize, Titlebar>> = RefCell::new(HashMap::new());
}

/// Called for both root and deferred viewport input, including native resizes.
pub fn sync() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let windows = NSApplication::sharedApplication(mtm).windows();
    TITLES.with(|titles| {
        let Ok(mut titles) = titles.try_borrow_mut() else { return };
        titles.retain(|number, _| windows.iter().any(|window| window.windowNumber() == *number));
        for window in windows.iter() {
            let title = window.title();
            let text = title.to_string();
            if !text.ends_with(" — SoundCraft — Edit") && !text.ends_with(" — SoundCraft — Mix") {
                continue;
            }
            let bar = titles.entry(window.windowNumber()).or_insert_with(|| Titlebar::new(&window, mtm));
            if bar.label.stringValue() != title {
                bar.label.setStringValue(&title);
            }
            bar.layout(&window);
        }
    });
}

impl Titlebar {
    fn new(window: &NSWindow, mtm: MainThreadMarker) -> Self {
        let view = NSView::new(mtm);
        let label = NSTextField::labelWithString(&window.title(), mtm);
        label.setFont(Some(&NSFont::titleBarFontOfSize(13.0)));
        label.setTextColor(Some(&NSColor::labelColor()));
        label.setAlignment(NSTextAlignment::Center);
        label.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        label.sizeToFit();
        let label_height = label.frame().size.height;
        view.addSubview(&label);
        let accessory = NSTitlebarAccessoryViewController::new(mtm);
        accessory.setLayoutAttribute(NSLayoutAttribute::Right);
        accessory.setView(&view);
        window.addTitlebarAccessoryViewController(&accessory);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        Self { view, label, label_height }
    }

    fn layout(&self, window: &NSWindow) {
        // Reserve the same space at both edges, keeping long titles clear of
        // the system buttons while centering on the entire window.
        let width = window.frame().size.width;
        let inset = 90.0;
        let mut size = self.view.frame().size;
        size.width = (width - inset).max(0.0);
        if self.view.frame().size != size {
            self.view.setFrameSize(size);
        }
        let height = self.label_height;
        let bounds = self.view.bounds();
        let origin = self.view.convertRect_toView(bounds, None).origin;
        let label_width = (width - 2.0 * inset).max(0.0);
        let frame =
            NSRect::new(NSPoint::new((width - label_width) * 0.5 - origin.x, (bounds.size.height - height) * 0.5), NSSize::new(label_width, height));
        if self.label.frame() != frame {
            self.label.setFrame(frame);
        }
    }
}
