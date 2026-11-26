use super::*;
use std::cell::Cell;
use std::rc::Rc;

/// The app binary's exit hook runs from `on_exit`, the one exit path macOS has (its quit ends the
/// process before `run_native` returns), and only once.
#[test]
fn leaving_runs_the_exit_hook_once() {
    let runs = Rc::new(Cell::new(0));
    let counted = runs.clone();
    let mut services = Services { at_exit: Some(Box::new(move || counted.set(counted.get() + 1))), ..Default::default() };
    services.run_exit_hook();
    assert_eq!(runs.get(), 1);
    services.run_exit_hook();
    assert_eq!(runs.get(), 1, "a second exit must not run the hook again");
    assert!(services.at_exit.is_none());
}

/// Stable tests and the web build leave without a hook installed.
#[test]
fn no_hook_means_a_no_op() {
    Services::default().run_exit_hook();
}
