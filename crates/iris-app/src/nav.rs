//! Back and forward, between the places visited.
//!
//! A place is where the window is: which workspace, and inside each one what was
//! chosen last — the mailbox or tag, the folder and the tab of the mail, the view of
//! the tasks, the calendar's month, week or day. Every choice made in the interface
//! records the place it leads to; Back returns to the one before, Forward undoes a
//! Back, and a new choice after a Back forgets what was ahead, as a browser does.
//!
//! Going back replays the choices through the window's own callbacks, so a place is
//! restored exactly as if it had been clicked. Those replays are not recorded.

use iris_ui::AppWindow;
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};

/// How many steps back are kept.
const PROFONDEUR: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// 0 mail, 1 calendar, 2 tasks, 3 Home, 4 Notes.
    pub workspace: i32,
    /// The mailbox shown, 0 for all of them.
    pub account: i32,
    /// The tag whose mailboxes are shown, -1 for none.
    pub tag: i32,
    /// The folder, empty for the work queues.
    pub folder: String,
    /// To do, Waiting, Done.
    pub tab: i32,
    /// The view of the tasks: "today", "list:3", "habits", "goal:3"…
    pub tasks: String,
    /// The calendar: 0 month, 1 week, 2 day.
    pub calendar: i32,
    /// The note open, by its path in its space.
    pub note: String,
    /// What is open over the place in a narrow window, where each opening is a page
    /// of its own (`thread:4`, `compose`, `task:9`, `settings`…); empty for the place
    /// itself. Back closes it before it leaves the place.
    pub page: String,
}

impl Place {
    pub fn start(workspace: i32, calendar: i32) -> Self {
        Self {
            workspace,
            account: 0,
            tag: -1,
            folder: String::new(),
            tab: 0,
            tasks: "today".into(),
            calendar,
            note: String::new(),
            page: String::new(),
        }
    }

    /// The place a choice leads to, from this one. `None` for a choice the history
    /// does not know.
    pub fn after(&self, kind: &str, value: &str) -> Option<Place> {
        let mut p = self.clone();
        let nombre = || value.parse::<i32>().ok();
        if kind == "page" {
            p.page = value.to_string();
            return Some(p);
        }
        // Any other choice is made from the place itself: what was open over it closes.
        p.page.clear();
        match kind {
            "workspace" => p.workspace = nombre()?,
            "account" => {
                p.workspace = 0;
                p.account = nombre()?;
                p.tag = -1;
            }
            "unified" => {
                p.workspace = 0;
                p.account = 0;
                p.tag = -1;
            }
            "tag" => {
                p.workspace = 0;
                p.tag = nombre()?;
            }
            "folder" => {
                p.workspace = 0;
                p.folder = value.to_string();
            }
            "tab" => {
                p.workspace = 0;
                p.tab = nombre()?;
                p.folder.clear();
            }
            "tasks" => {
                p.workspace = 2;
                p.tasks = value.to_string();
            }
            "calendar-mode" => {
                p.workspace = 1;
                p.calendar = nombre()?;
            }
            "note" => {
                p.workspace = 4;
                p.note = value.to_string();
            }
            _ => return None,
        }
        Some(p)
    }
}

/// Where the window has been, and where Back left from.
#[derive(Debug)]
pub struct History {
    pub current: Place,
    back: Vec<Place>,
    forward: Vec<Place>,
}

impl History {
    pub fn new(start: Place) -> Self {
        Self {
            current: start,
            back: Vec::new(),
            forward: Vec::new(),
        }
    }

    /// A new place was reached by a choice. The same place again is not a step.
    pub fn visit(&mut self, place: Place) -> bool {
        if place == self.current {
            return false;
        }
        self.back.push(std::mem::replace(&mut self.current, place));
        if self.back.len() > PROFONDEUR {
            self.back.remove(0);
        }
        self.forward.clear();
        true
    }

    /// One step back: the place to go to.
    pub fn back(&mut self) -> Option<Place> {
        let p = self.back.pop()?;
        self.forward
            .push(std::mem::replace(&mut self.current, p.clone()));
        Some(p)
    }

    pub fn forward(&mut self) -> Option<Place> {
        let p = self.forward.pop()?;
        self.back
            .push(std::mem::replace(&mut self.current, p.clone()));
        Some(p)
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

thread_local! {
    static HISTORIQUE: RefCell<History> = RefCell::new(History::new(Place::start(0, 1)));
    /// True while a place is being restored: what it triggers is not a new step.
    static REJOUE: Cell<bool> = const { Cell::new(false) };
}

fn boutons(f: &AppWindow) {
    HISTORIQUE.with(|h| {
        let h = h.borrow();
        f.set_can_go_back(h.can_go_back());
        f.set_can_go_forward(h.can_go_forward());
    });
}

/// Records a choice made in the window, unless it is a replay.
pub fn note(f: &AppWindow, kind: &str, value: &str) {
    if REJOUE.with(Cell::get) {
        return;
    }
    HISTORIQUE.with(|h| {
        let mut h = h.borrow_mut();
        if let Some(p) = h.current.after(kind, value) {
            h.visit(p);
        }
    });
    boutons(f);
}

/// Puts the window in `to`, coming from `from`, through its own callbacks.
fn aller(f: &AppWindow, from: &Place, to: &Place) {
    REJOUE.with(|r| r.set(true));
    if to.tag != from.tag || to.account != from.account {
        if to.tag >= 0 {
            f.invoke_tag_selected(to.tag);
        } else if to.account == 0 {
            f.invoke_unified_selected();
        } else {
            f.invoke_account_selected(to.account);
        }
    }
    if to.tab != from.tab {
        f.invoke_tab_selected(to.tab);
    }
    if to.folder != from.folder {
        if to.folder.is_empty() {
            f.invoke_folder_cleared();
        } else {
            f.invoke_folder_selected(to.folder.as_str().into());
        }
    }
    if to.tasks != from.tasks {
        f.invoke_task_place_chosen(to.tasks.as_str().into());
    }
    if to.calendar != from.calendar {
        f.invoke_calendar_mode_chosen(to.calendar);
    }
    if to.note != from.note && !to.note.is_empty() {
        f.invoke_notes_quick_chosen(to.note.as_str().into());
    }
    if to.page != from.page {
        f.set_page(to.page.as_str().into());
    }
    if to.workspace != from.workspace {
        f.set_workspace(to.workspace);
        f.invoke_workspace_changed(to.workspace);
    }
    REJOUE.with(|r| r.set(false));
}

pub fn back(f: &AppWindow) {
    let pas = HISTORIQUE.with(|h| {
        let mut h = h.borrow_mut();
        let depuis = h.current.clone();
        h.back().map(|vers| (depuis, vers))
    });
    if let Some((depuis, vers)) = pas {
        aller(f, &depuis, &vers);
    }
    boutons(f);
}

pub fn forward(f: &AppWindow) {
    let pas = HISTORIQUE.with(|h| {
        let mut h = h.borrow_mut();
        let depuis = h.current.clone();
        h.forward().map(|vers| (depuis, vers))
    });
    if let Some((depuis, vers)) = pas {
        aller(f, &depuis, &vers);
    }
    boutons(f);
}

/// The system's Back (Android's gesture, a phone's button): what is open on top closes
/// first, then the page over the place, then the history goes back a step. False when
/// there was nothing left to do, and the system may leave Iris.
pub fn system_back(f: &AppWindow) -> bool {
    if f.get_modal_open() {
        // Every window over the others closes on Escape: the same key, the same way.
        let echap = slint::SharedString::from(slint::platform::Key::Escape);
        f.window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: echap.clone(),
            });
        f.window()
            .dispatch_event(slint::platform::WindowEvent::KeyReleased { text: echap });
        return true;
    }
    if HISTORIQUE.with(|h| h.borrow().can_go_back()) {
        back(f);
        return true;
    }
    if !f.get_page().is_empty() {
        f.set_page("".into());
        return true;
    }
    false
}

/// Starts the history where the window opens, and wires the buttons, Alt+arrows and
/// every choice the window reports.
pub fn wire_navigation(f: &AppWindow, start: Place) {
    HISTORIQUE.with(|h| *h.borrow_mut() = History::new(start));
    boutons(f);
    {
        let faible = f.as_weak();
        f.on_system_back(move || {
            let Some(f) = faible.upgrade() else { return };
            // Nothing left to go back to: the system takes Back (Android shows the
            // app before).
            if !system_back(&f) {
                crate::platform::current().leave();
            }
        });
    }
    {
        let faible = f.as_weak();
        crate::workspace::follow(f, move |w| {
            if let Some(f) = faible.upgrade() {
                note(&f, "workspace", &w.to_string());
            }
        });
    }
    {
        let faible = f.as_weak();
        f.on_navigated(move |kind, value| {
            if let Some(f) = faible.upgrade() {
                note(&f, &kind, &value);
            }
        });
    }
    {
        let faible = f.as_weak();
        f.on_nav_back(move || {
            if let Some(f) = faible.upgrade() {
                back(&f);
            }
        });
    }
    {
        let faible = f.as_weak();
        f.on_nav_forward(move || {
            if let Some(f) = faible.upgrade() {
                forward(&f);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_and_forward_walk_the_places_visited() {
        let mut h = History::new(Place::start(3, 1));
        let mail = h.current.after("workspace", "0").unwrap();
        h.visit(mail.clone());
        let compte = h.current.after("account", "7").unwrap();
        h.visit(compte.clone());
        let taches = h.current.after("tasks", "list:2").unwrap();
        h.visit(taches.clone());
        assert_eq!(taches.workspace, 2);
        assert_eq!(taches.account, 7, "the mail keeps its mailbox while away");

        assert_eq!(h.back(), Some(compte.clone()));
        assert_eq!(h.back(), Some(mail.clone()));
        assert!(h.can_go_forward());
        assert_eq!(h.forward(), Some(compte.clone()));

        // A new choice after going back forgets what was ahead.
        let agenda = h.current.after("workspace", "1").unwrap();
        h.visit(agenda);
        assert!(!h.can_go_forward());
        assert_eq!(h.back(), Some(compte));
    }

    #[test]
    fn the_same_place_twice_is_one_step() {
        let mut h = History::new(Place::start(0, 1));
        let p = h.current.after("tab", "1").unwrap();
        assert!(h.visit(p.clone()));
        assert!(!h.visit(p));
        assert_eq!(h.back().map(|p| p.tab), Some(0));
        assert!(h.back().is_none());
    }

    #[test]
    fn a_tag_replaces_the_mailbox_and_a_mailbox_the_tag() {
        let p = Place::start(0, 1);
        let t = p.after("tag", "4").unwrap();
        assert_eq!((t.tag, t.account), (4, 0));
        let c = t.after("account", "9").unwrap();
        assert_eq!((c.tag, c.account), (-1, 9));
        assert!(p.after("nonsense", "1").is_none());
        assert!(p.after("tab", "x").is_none());
    }

    #[test]
    fn the_habits_are_a_place_of_the_tasks() {
        let mut h = History::new(Place::start(0, 1));
        let habitudes = h.current.after("tasks", "habits").unwrap();
        assert_eq!(
            (habitudes.workspace, habitudes.tasks.as_str()),
            (2, "habits")
        );
        h.visit(habitudes);
        let liste = h.current.after("tasks", "list:2").unwrap();
        h.visit(liste);
        assert_eq!(h.back().map(|p| p.tasks), Some("habits".to_string()));
    }

    #[test]
    fn a_page_is_a_step_and_any_other_choice_closes_it() {
        let mut h = History::new(Place::start(0, 1));
        let fil = h.current.after("page", "thread:4").unwrap();
        assert_eq!((fil.workspace, fil.page.as_str()), (0, "thread:4"));
        h.visit(fil.clone());
        let ailleurs = h.current.after("folder", "Sent").unwrap();
        assert!(
            ailleurs.page.is_empty(),
            "choosing a folder closes the page"
        );
        h.visit(ailleurs);
        assert_eq!(h.back(), Some(fil));
        assert_eq!(h.back().map(|p| p.page), Some(String::new()));
    }

    #[test]
    fn a_goal_is_a_place_of_the_tasks() {
        let p = Place::start(3, 1).after("tasks", "goal:3").unwrap();
        assert_eq!((p.workspace, p.tasks.as_str()), (2, "goal:3"));
        assert!(p.after("growth", "goals").is_none());
    }
}
