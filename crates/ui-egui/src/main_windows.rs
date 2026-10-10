//! Edit and Mix are independent views of the same engine, not separate sessions.
//! Native hosts use an immediate viewport so both views can borrow the app on the
//! UI thread. Browsers and offscreen hosts use movable, resizable egui windows.

use crate::{MainWindow, SoundApp, edit_window, menus, mix_window, shortcuts};
use egui::{Rect, Ui, Vec2, ViewportCommand, ViewportId, pos2, vec2};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Arrangement {
    Tile,
    Horizontal,
    Vertical,
    Cascade,
}

#[derive(Debug, Default)]
pub struct WindowState {
    primary: Option<MainWindow>,
    focus: Option<MainWindow>,
    arrange: Option<Arrangement>,
    /// The workspace before tiling, in OS points on native hosts, so repeating
    /// Tile does not halve it again and UI zoom does not move it off screen.
    workspace: Option<Rect>,
    primary_rect: Option<Rect>,
    secondary_rect: Option<Rect>,
    secondary_initial_rect: Option<Rect>,
    embedded_arrangement: Option<Arrangement>,
    close_request: Option<MainWindow>,
    root_title: String,
}

pub fn secondary_id() -> ViewportId {
    ViewportId::from_hash_of("soundcraft_second_main_window")
}

impl WindowState {
    pub fn primary(&self, app: &SoundApp) -> MainWindow {
        if app.ui.both_windows { self.primary.unwrap_or(app.ui.window) } else { app.ui.window }
    }

    pub fn is_open(&self, app: &SoundApp, window: MainWindow) -> bool {
        app.ui.both_windows || window == self.primary(app)
    }

    pub fn viewport(&self, app: &SoundApp, window: MainWindow) -> ViewportId {
        if window == self.primary(app) { ViewportId::ROOT } else { secondary_id() }
    }

    pub fn inspect(&self, app: &SoundApp) -> Value {
        let primary = self.primary(app);
        let rect = |window| {
            let r = if window == primary { self.primary_rect } else { self.secondary_rect };
            r.map(|r| [r.min.x, r.min.y, r.max.x, r.max.y])
        };
        json!({
            "primary": primary, "active": app.ui.window,
            "edit": {"open": self.is_open(app, MainWindow::Edit), "rect": rect(MainWindow::Edit)},
            "mix": {"open": self.is_open(app, MainWindow::Mix), "rect": rect(MainWindow::Mix)},
        })
    }
}

pub fn activate(app: &mut SoundApp, window: MainWindow) {
    let primary = app.main_windows.primary(app);
    app.main_windows.primary = Some(primary);
    if window != primary {
        app.ui.both_windows = true;
    }
    app.ui.window = window;
    app.main_windows.focus = Some(window);
}

pub fn arrange(app: &mut SoundApp, arrangement: Arrangement) {
    app.main_windows.primary = Some(app.main_windows.primary(app));
    app.ui.both_windows = true;
    app.main_windows.arrange = Some(arrangement);
}

pub fn request_close(app: &mut SoundApp) {
    app.main_windows.close_request = Some(app.ui.window);
}

/// Closing either main view keeps the other view and the session alive. If
/// the root closes, move the remaining view into it before retiring the child.
pub fn close(app: &mut SoundApp, window: MainWindow, ctx: &egui::Context) {
    if !app.ui.both_windows {
        return;
    }
    let primary = app.main_windows.primary(app);
    if window == primary {
        app.main_windows.primary = Some(primary.other());
        if !ctx.embed_viewports()
            && let Some(rect) = app.main_windows.secondary_rect
        {
            place(ctx, ViewportId::ROOT, rect, decoration(ctx, secondary_id()));
        }
    }
    app.ui.both_windows = false;
    app.ui.window = window.other();
    app.main_windows.focus = Some(app.ui.window);
    app.main_windows.secondary_rect = None;
    ctx.request_repaint_of(ViewportId::ROOT);
}

pub fn handle_root_close(app: &mut SoundApp, ctx: &egui::Context) {
    if app.ui.both_windows && !app.quit_requested && ctx.input(|i| i.viewport().close_requested()) {
        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        close(app, app.main_windows.primary(app), ctx);
    }
}

/// Native integrations keep logic ticking when every window is minimized or
/// occluded. Window commands must still close views and bring them back then.
pub fn process_requests(app: &mut SoundApp, ctx: &egui::Context) {
    if let Some(window) = app.main_windows.close_request.take() {
        close(app, window, ctx);
    }
    if ctx.embed_viewports() {
        return;
    }
    let (root_rect, child_rect, child_exists) = ctx.input(|i| {
        (
            i.raw.viewports.get(&ViewportId::ROOT).and_then(|v| v.outer_rect),
            i.raw.viewports.get(&secondary_id()).and_then(|v| v.outer_rect),
            i.raw.viewports.contains_key(&secondary_id()),
        )
    });
    app.main_windows.primary_rect = root_rect;
    app.main_windows.secondary_rect = child_rect;
    if let Some(window) = app.main_windows.focus {
        let mut id = app.main_windows.viewport(app, window);
        // Wake the parent first if the second window has not been created yet.
        if id == secondary_id() && !child_exists {
            id = ViewportId::ROOT;
        }
        ctx.send_viewport_cmd_to(id, ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd_to(id, ViewportCommand::Focus);
    }
}

/// Tile Horizontal stacks rows; Tile Vertical places columns. The automatic
/// variant chooses the split with the larger usable dimension. All rectangles
/// stay inside the original workspace, including negative monitor coordinates.
pub fn arranged_rects(area: Rect, arrangement: Arrangement) -> (Rect, Rect) {
    let horizontal = match arrangement {
        Arrangement::Horizontal => true,
        Arrangement::Vertical => false,
        _ => area.height() > area.width(),
    };
    if arrangement == Arrangement::Cascade {
        let inset = 36.0_f32.min(area.width() * 0.1).min(area.height() * 0.1);
        let size = (area.size() - vec2(inset, inset)).max(vec2(1.0, 1.0));
        return (Rect::from_min_size(area.min, size), Rect::from_min_size(area.min + vec2(inset, inset), size));
    }
    if horizontal {
        let middle = area.center().y;
        (Rect::from_min_max(area.min, pos2(area.max.x, middle)), Rect::from_min_max(pos2(area.min.x, middle), area.max))
    } else {
        let middle = area.center().x;
        (Rect::from_min_max(area.min, pos2(middle, area.max.y)), Rect::from_min_max(pos2(middle, area.min.y), area.max))
    }
}

fn decoration(ctx: &egui::Context, id: ViewportId) -> Vec2 {
    ctx.input(|i| i.raw.viewports.get(&id).and_then(|v| Some((v.outer_rect?.size() - v.inner_rect?.size()).max(Vec2::ZERO))))
        .unwrap_or(vec2(0.0, 28.0))
}

fn place(ctx: &egui::Context, id: ViewportId, rect: Rect, chrome: Vec2) {
    ctx.send_viewport_cmd_to(id, ViewportCommand::Fullscreen(false));
    ctx.send_viewport_cmd_to(id, ViewportCommand::Maximized(false));
    ctx.send_viewport_cmd_to(id, ViewportCommand::MinInnerSize(vec2(320.0, 220.0)));
    ctx.send_viewport_cmd_to(id, ViewportCommand::OuterPosition(rect.min));
    ctx.send_viewport_cmd_to(id, ViewportCommand::InnerSize((rect.size() - chrome).max(vec2(1.0, 1.0))));
}

fn scaled_rect(rect: Rect, scale: f32) -> Rect {
    Rect::from_min_max((rect.min.to_vec2() * scale).to_pos2(), (rect.max.to_vec2() * scale).to_pos2())
}

pub fn title(app: &SoundApp, window: MainWindow) -> String {
    format!("{}{} — SoundCraft — {}", app.engine.session().name, if app.engine.is_dirty() { " *" } else { "" }, window.label())
}

fn surface(app: &mut SoundApp, ui: &mut Ui, window: MainWindow) {
    // Independent panel/scroll IDs also matter when both views are embedded in
    // one browser canvas: scrolling a track list must not scroll the other one.
    ui.push_id(window.label(), |ui| match window {
        MainWindow::Edit => edit_window::show(app, ui),
        MainWindow::Mix => mix_window::show(app, ui),
    });
}

fn received_input(ctx: &egui::Context) -> bool {
    ctx.input(|i| {
        i.raw.focused
            && i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::WindowFocused(true) | egui::Event::Key { pressed: true, .. } | egui::Event::PointerButton { pressed: true, .. }
                )
            })
    })
}

pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    if !app.ui.both_windows {
        app.main_windows.primary = Some(app.ui.window);
    }
    let primary = app.main_windows.primary(app);
    app.main_windows.primary = Some(primary);
    let embedded = ctx.embed_viewports();
    let area = if embedded { ui.max_rect() } else { ctx.input(|i| i.viewport().outer_rect).unwrap_or(ui.max_rect()) };
    if !app.ui.both_windows || app.main_windows.workspace.is_none() {
        app.main_windows.workspace = Some(if embedded { area } else { scaled_rect(area, ctx.zoom_factor()) });
    }
    if !embedded && app.main_windows.focus.is_none() && received_input(&ctx) {
        app.ui.window = primary;
    }
    // Embedded windows share an input stream; dispatch shortcuts exactly once.
    shortcuts::handle(app, &ctx);
    if !app.native_menu_bar {
        menus::menu_bar(app, ui);
    }
    if let Some(window) = app.main_windows.close_request.take() {
        close(app, window, &ctx);
    }
    let primary = app.main_windows.primary(app);
    if let Some(arrangement) = app.main_windows.arrange.take() {
        let workspace = app.main_windows.workspace.map_or(area, |rect| if embedded { rect } else { scaled_rect(rect, ctx.zoom_factor().recip()) });
        if embedded {
            app.main_windows.embedded_arrangement = Some(arrangement);
        } else {
            let (first, second) = arranged_rects(workspace, arrangement);
            place(&ctx, ViewportId::ROOT, first, decoration(&ctx, ViewportId::ROOT));
            place(&ctx, secondary_id(), second, decoration(&ctx, secondary_id()));
            app.main_windows.secondary_initial_rect = Some(scaled_rect(second, ctx.zoom_factor()));
        }
    }
    if embedded && app.ui.both_windows {
        embedded_windows(app, ui, primary);
        app.floating_ui(&ctx);
        return;
    }
    app.main_windows.primary_rect = Some(area);
    let root_title = title(app, primary);
    if root_title != app.main_windows.root_title {
        ctx.send_viewport_cmd(ViewportCommand::Title(root_title.clone()));
        app.main_windows.root_title = root_title;
    }
    surface(app, ui, primary);
    if app.ui.window == primary || !app.ui.both_windows {
        app.floating_ui(&ctx);
    }
    if app.ui.both_windows {
        let secondary = primary.other();
        let mut builder = egui::ViewportBuilder::default()
            .with_title(title(app, secondary))
            .with_inner_size([1100.0, 780.0])
            .with_min_inner_size([320.0, 220.0])
            .with_drag_and_drop(true);
        if let Some(rect) = app.main_windows.secondary_initial_rect {
            let rect = scaled_rect(rect, ctx.zoom_factor().recip());
            builder = builder.with_position(rect.min).with_inner_size((rect.size() - decoration(&ctx, secondary_id())).max(vec2(1.0, 1.0)));
        }
        ctx.show_viewport_immediate(secondary_id(), builder, |ui, _class| {
            let child_ctx = ui.ctx().clone();
            let revision = app.engine.revision;
            if child_ctx.input(|i| i.viewport().close_requested()) {
                close(app, secondary, &child_ctx);
                return;
            }
            app.main_windows.secondary_rect = child_ctx.input(|i| i.viewport().outer_rect);
            if app.main_windows.focus.is_none() && received_input(&child_ctx) {
                app.ui.window = secondary;
            }
            shortcuts::handle(app, &child_ctx);
            if let Some(window) = app.main_windows.close_request.take() {
                close(app, window, &child_ctx);
            }
            if !app.native_menu_bar {
                menus::menu_bar(app, ui);
            }
            surface(app, ui, secondary);
            if app.ui.window == secondary {
                app.floating_ui(&child_ctx);
            }
            // Child edits need the root logic pass to sync audio, undo/status,
            // and the native menu. No transport or automation runs twice.
            if app.engine.revision != revision || app.is_playing() || child_ctx.input(|i| !i.events.is_empty()) {
                child_ctx.request_repaint_of(ViewportId::ROOT);
            }
        });
    }
    if let Some(window) = app.main_windows.focus.take() {
        ctx.send_viewport_cmd_to(app.main_windows.viewport(app, window), ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd_to(app.main_windows.viewport(app, window), ViewportCommand::Focus);
    }
}

fn embedded_windows(app: &mut SoundApp, ui: &mut Ui, primary: MainWindow) {
    let ctx = ui.ctx().clone();
    let area = ui.available_rect_before_wrap();
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(crate::theme::Tokens::current().window_bg)).show(ui, |_| {});
    let requested = app.main_windows.embedded_arrangement.take();
    let (first, second) = arranged_rects(area.shrink(4.0), requested.unwrap_or(Arrangement::Cascade));
    for (window, rect) in [(primary, first), (primary.other(), second)] {
        if !app.ui.both_windows {
            break;
        }
        let mut open = true;
        let mut pane = egui::Window::new(title(app, window))
            .id(egui::Id::new(("main_window", window.label())))
            .open(&mut open)
            .collapsible(false)
            .min_size([320.0, 220.0])
            .default_pos(rect.min)
            .default_size(rect.size());
        if requested.is_some() {
            // Area constrains positions using the previous frame's window
            // size. Do not push a newly tiled pane left based on its old width.
            pane = pane.fixed_rect(rect).constrain(false);
        }
        let response = pane.show(&ctx, |ui| {
            if ui.rect_contains_pointer(ui.max_rect()) && ui.input(|i| i.pointer.any_pressed()) {
                app.ui.window = window;
            }
            surface(app, ui, window);
            ui.take_available_space();
        });
        if let Some(response) = response {
            if window == primary {
                app.main_windows.primary_rect = Some(response.response.rect);
            } else {
                app.main_windows.secondary_rect = Some(response.response.rect);
            }
        }
        if !open {
            close(app, window, &ctx);
        }
    }
    if let Some(window) = app.main_windows.focus {
        ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, egui::Id::new(("main_window", window.label()))));
    }
    app.main_windows.focus = None;
}
