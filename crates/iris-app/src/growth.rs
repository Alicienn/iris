//! Growth: the goals, that moved here from Tasks.
//!
//! Growth's column lists the goals; a goal's page, or all of them, is the tasks' own,
//! drawn beside it: choosing a goal here hands it to the tasks (`goal:3`), which fill
//! it as before. The habits live in Tasks (`crate::habits`).

use iris_ui::AppWindow;
use slint::ComponentHandle;

/// Opens Growth on `place` ("goals", "goal:3"), as one step back.
pub fn open(f: &AppWindow, place: &str) {
    crate::nav::note(f, "growth", place);
    f.invoke_growth_place_chosen(place.into());
    if f.get_workspace() != WORKSPACE {
        f.set_workspace(WORKSPACE);
        f.invoke_workspace_changed(WORKSPACE);
    }
}

/// Growth's place in the window: after Home (3) and Notes (4).
pub const WORKSPACE: i32 = 5;

pub fn wire_growth(f: &AppWindow) {
    // A place of Growth is a goal or all of them, which the tasks draw.
    let faible = f.as_weak();
    f.on_growth_place_chosen(move |cle| {
        if let Some(f) = faible.upgrade() {
            f.invoke_task_place_chosen(cle);
        }
    });
}
