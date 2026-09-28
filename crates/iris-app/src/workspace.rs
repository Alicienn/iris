//! Changing workspace — mail, calendar, tasks — told to everyone who cares.
//!
//! The window has one `workspace-changed` callback, and Slint keeps one handler per
//! callback: the calendar and the tasks each installing theirs would leave only the
//! last. Each follows the change here instead.

use iris_ui::AppWindow;
use std::cell::RefCell;

type Suiveur = Box<dyn Fn(i32)>;

thread_local! {
    static SUIVEURS: RefCell<Vec<Suiveur>> = RefCell::new(Vec::new());
}

/// Calls `handler` with the new workspace every time it changes.
pub fn follow(f: &AppWindow, handler: impl Fn(i32) + 'static) {
    SUIVEURS.with(|s| s.borrow_mut().push(Box::new(handler)));
    f.on_workspace_changed(|w| {
        SUIVEURS.with(|s| {
            for h in s.borrow().iter() {
                h(w);
            }
        })
    });
}
