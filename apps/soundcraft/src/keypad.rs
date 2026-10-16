//! Tells the numeric keypad's Enter from Return, which egui reports as the same key.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, StartCause, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::WindowId;

/// Passes every event on to `app`, first noting in `keypad` whether the last Enter pressed was the keypad's.
pub struct KeypadEnter<A> {
    app: A,
    keypad: Arc<AtomicBool>,
}

impl<A> KeypadEnter<A> {
    pub fn new(app: A, keypad: Arc<AtomicBool>) -> Self {
        Self { app, keypad }
    }
}

/// The keypad's Enter is `Some(true)`, Return `Some(false)`, any other key `None`.
fn keypad_enter(key: PhysicalKey) -> Option<bool> {
    match key {
        PhysicalKey::Code(KeyCode::NumpadEnter) => Some(true),
        PhysicalKey::Code(KeyCode::Enter) => Some(false),
        _ => None,
    }
}

impl<T: 'static, A: ApplicationHandler<T>> ApplicationHandler<T> for KeypadEnter<A> {
    fn window_event(&mut self, event_loop: &ActiveEventLoop, window: WindowId, event: WindowEvent) {
        if let WindowEvent::KeyboardInput { event: key, .. } = &event
            && key.state.is_pressed()
            && let Some(keypad) = keypad_enter(key.physical_key)
        {
            self.keypad.store(keypad, Ordering::Relaxed);
        }
        self.app.window_event(event_loop, window, event);
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.app.resumed(event_loop);
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        self.app.new_events(event_loop, cause);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: T) {
        self.app.user_event(event_loop, event);
    }

    fn device_event(&mut self, event_loop: &ActiveEventLoop, device: DeviceId, event: DeviceEvent) {
        self.app.device_event(event_loop, device, event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.app.about_to_wait(event_loop);
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        self.app.suspended(event_loop);
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        self.app.exiting(event_loop);
    }

    fn memory_warning(&mut self, event_loop: &ActiveEventLoop) {
        self.app.memory_warning(event_loop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_enter_keys_set_the_flag() {
        assert_eq!(keypad_enter(PhysicalKey::Code(KeyCode::NumpadEnter)), Some(true));
        assert_eq!(keypad_enter(PhysicalKey::Code(KeyCode::Enter)), Some(false));
        assert_eq!(keypad_enter(PhysicalKey::Code(KeyCode::Numpad1)), None);
        assert_eq!(keypad_enter(PhysicalKey::Code(KeyCode::Space)), None);
    }
}
