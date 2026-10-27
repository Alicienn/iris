# Screens in One Column — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every place of Iris works in compact width (a phone, a narrow PC window) and
medium width (a foldable) as drawn in the mockups, with the desktop layout untouched at
expanded width.

**Architecture:** Each screen keeps its component and its Rust driver; it reads
`Layout.width-class` and `Layout.touch-input` (adaptive plan A1) to place its parts. In
compact, a place's root is a list and what opens from it is a page (`Place.page`, adaptive
plan C1); windows become pages or sheets by the frames themselves (adaptive plan B1–B2).
New pieces are shared components first (`controls/`, `lists/`, `layout/`), never drawn
inside a screen.

**Tech Stack:** Rust 2021, Slint 1.17, `i-slint-backend-testing`.

**Spec:** `docs/superpowers/specs/2026-10-26-iris-android-design.md`; the mockups in
`..\Iris-design\android\` — each task names the frames it ports (M1 = Mail's first frame,
C3 = Calendar's third, …, in the order of each page; the ids are in the HTML comments).

## Global Constraints

- The rules of `docs/superpowers/plans/2026-10-26-iris-mobile-index.md`.
- Depends on the adaptive plan, Phases A–D. Every task's tests run at `fenetre_de(390.0, 844.0)`
  with `touch-input` on, and one scenario per task at 1280×800 proves the desktop is unchanged.
- Sizes from the spec's §4 and the mockups: mail row 76 px, task row ≥ 56 px, the floating
  action button 56 px high (radius 18), docks 64 px, top bar 56 px, large title 32 px.
- Each task ends by adding its captures to `examples/apercu_mobile.rs` (adaptive plan E1);
  captured only when the owner asks, into `..\Iris-design\android\port\`, and compared with
  the mockup's capture in `shots\` before the task is called done.
- Hover-only controls and right-click menus have their touch twin (adaptive D2–D3).

## Review Focus

- A conversation opened from a notification or a widget (a deep link to `thread:<id>`)
  while another page is on top: the stack becomes list → that conversation, so Back lands
  in the list, not on the old page (Task B3 tests `navigated("page","thread:9")` from a
  settings page).
- The on-screen keyboard over a docked field (quick add, reply, format bar, address
  suggestions): the field and its suggestions stay visible above it, the list above
  shrinks (Tasks B4, D2, E2 test a window height cut by 300 px with the field focused).
- Rotating a phone with a page open (390×844 → 844×390, medium is not reached, landscape
  compact): the page stays, nothing is cut off at 390 px high (Task B3).
- An account, a list or a note name of 80 characters: rows, titles and chips ellipsize on
  one line (every row test uses one long name).
- Swipe on the last row of a list with the floating button over it: the swipe still works
  and the button does not take the touch (Task B2).

---

## Phase A — Home and search

### Task A1: Home in one column (H1, H2)

**Files:** Modify `crates/iris-ui/ui/screens/home.slint:237-747` (columns: 1 in compact, 2 in
medium, today's 3–4 in expanded; content width `min(980px, visible-width - 32px)` in compact;
a header with search and settings `IconButton`s when the side column is hidden; Goal and
Habits side by side as the two small widgets); `crates/iris-app/src/home.rs` (unchanged data).
Create `controls/button.slint` `Fab` (`icon`, `label`, `collapsed: bool`; `callback clicked()`).
- [ ] Step 1: tests — at 390: widgets in one column in the order of the settings; the
  search button opens the palette page (A2), the settings button opens Settings; at 700:
  two columns; at 1280: unchanged.
- [ ] Step 2: implement; capture `home.png`, `home-scrolled.png`; fmt, commit, push, read CI.

### Task A2: search and commands as a page (H3, H4)

**Files:** Modify the palette in `crates/iris-ui/ui/app.slint` (`palette-open` → in compact a
page: top bar with ← and a focused `TextField`, groups below, the keyboard's search key
accepts); `crates/iris-app/src/shell.rs` (palette results unchanged).
- [ ] Step 1: tests — at 390: opening puts the cursor in the field (rule 3); typing
  "marie" lists Mail, Tasks, Events, Notes groups; a task's check in the results ticks it;
  Back closes the page and hands the keyboard back.
- [ ] Step 2: implement; capture `search.png`; fmt, commit, push, read CI.

## Phase B — Mail

### Task B1: the list in one column (M1, M12, M14)

**Files:** Modify `crates/iris-ui/ui/app.slint:2062-2310` (in compact: `SidebarFrame`,
`MailToolbar` and the divider hidden; the list column full width unless `page` starts with
`thread:`), `crates/iris-ui/ui/screens/list.slint` (`ThreadListView`: a compact header with
the place's name and ▾, the subtitle "N to do · synced …", the queue as `Segmented`, the
filters as scrolling `Pill`s, rows of `Tokens.mail-row-height` on three lines, the `Fab`
collapsing while the list scrolls), `crates/iris-ui/ui/shell/chrome.slint` (StatusBar's sync
text exposed as `Chrome.sync-text`).
- [ ] Step 1: tests — at 390: the queue's segments switch To do / Waiting / Done (same
  callbacks as the desktop tabs); a tap on a row pushes `thread:<id>`; the Fab opens the
  composer; an empty To do shows inbox zero with its streak; Waiting rows say since when.
- [ ] Step 2: implement; capture `mail.png`, `mail-waiting.png`, `inbox-zero.png`; fmt,
  commit, push, read CI.

### Task B2: swipe, long press, selection (M2, M4, M5)

**Files:** Modify `crates/iris-ui/ui/screens/list.slint:159-257` (rows inside `SwipeRow`: start
= Done, green; end = Snooze, orange, opening the snooze sheet; `LongPress` → the thread
context menu, shown as a sheet with the four queue actions as large tiles on top), `BulkBar`
(in compact, the top bar "N selected" with Done, Snooze, Archive, Delete, ⋯; avatars become
`Check`s), `crates/iris-app/src/settings.rs` (`swipe_start`, `swipe_end`: `"done" | "snooze" |
"archive" | "waiting"`, shared settings).
- [ ] Step 1: tests — a 120 px right swipe marks the thread Done and shows the Undo toast;
  Undo restores it; a left swipe opens the snooze sheet; a long press opens the menu with
  the row outlined; "Select" enters selection, taps add rows, × leaves; the Review Focus
  swipe on the last row under the Fab.
- [ ] Step 2: implement; capture `mail-swipe.png`, `mail-menu.png`, `mail-select.png`;
  fmt, commit, push, read CI.

### Task B3: the mailboxes sheet and the conversation page (M3, M6, M7, M8)

**Files:** Create `crates/iris-ui/ui/screens/mailboxes.slint` (`MailboxesSheet`: a pullable
`SheetSurface` holding `Sidebar` and `FolderTree` as they are, the sync line at the foot);
modify `crates/iris-ui/ui/screens/conversation.slint` (compact: a top bar ← Snooze Waiting
To task Archive ⋯; the column width = the window minus 36 px; earlier messages folded to one
line; attachments as two-up tiles; a dock with the reply field and a primary Done button),
`crates/iris-app/src/shell.rs:1830-1890` (`largeur_de_lecture` reports the compact width so
Blitz paints at it).
- [ ] Step 1: tests — the title's ▾ opens the sheet; picking a tag or a folder closes it and
  hands the keyboard back; in the page, Done marks the thread and pops to the list; the
  reply field opens the composer filled in; ⋯ is anchored; the invitation banner's three
  buttons answer; the Review Focus deep link and rotation lines.
- [ ] Step 2: implement; capture `mailboxes.png`, `conversation.png`, `invitation.png`,
  `reader-more.png`; fmt, commit, push, read CI.

### Task B4: the composer, send later, search (M9, M10, M11, M13)

**Files:** Modify `crates/iris-ui/ui/screens/compose.slint:631-673` (`ComposePanel` in compact
= a page: top bar × "New message"/"Reply" 📎 🕒 Send; From, To with chips and suggestions
under the field, Cc/Bcc on demand, Subject, body, quotes; minimising disabled), the reader's
snooze and the send-later menus (sheets with the time at the right), `list.slint`'s `SearchBar`
(compact: a page with the field focused, pills under it, the query echoed, results by day).
- [ ] Step 1: tests — opening puts the cursor in To (rule 3) and with To filled, in the
  body; a suggestion tapped is still there when the press ends (rule 1), with the keyboard
  height cut of the Review Focus; Send with the undo toast; send-later lists the four times;
  search pills write `is:unread` into the query.
- [ ] Step 2: implement; capture `compose.png`, `reply-later.png`, `snooze.png`,
  `mail-search.png`; fmt, commit, push, read CI.

### Task B5: medium width, and release 6.3.0 (F1)

**Files:** Modify `app.slint` (medium: `NavRail` with New message, list 300 px, conversation
beside it, Done in the conversation's top bar).
- [ ] Step 1: test at 700×820 — list and conversation both visible; picking a thread keeps
  `page` empty. Capture `fold-mail.png`.
- [ ] Step 2: the adaptive plan's Task E2 (release 6.3.0) with these screens.

## Phase C — Calendar

### Task C1: Agenda, the week strip, Day (C1, C2)

**Files:** Modify `crates/iris-ui/ui/screens/calendar.slint` (a fourth mode `agenda`, the
default in compact; a `WeekStrip` component in `lists/` — seven days, today in red, the chosen
day filled, a dot under days with events; the agenda as `TimeRow`s by day with the now line
and the Join button on the next video event; Day keeps the timeline with "To plan" above it),
`crates/iris-app/src/calendar.rs` and `nav.rs` (`Place.calendar` 3 = agenda; `calendar_view`
setting keeps desktop and compact choices apart: `calendar_view_compact: i32`, device-local).
- [ ] Step 1: tests — at 390 the calendar opens on Agenda; the strip's day scrolls the agenda
  to it; Day shows the now line; "To plan" then a tap on a free half hour books the task
  (the two-tap twin of the drag); a long press then drag moves an event by quarter hours.
- [ ] Step 2: implement; capture `calendar-agenda.png`, `calendar-day.png`; fmt, commit,
  push, read CI.

### Task C2: 3 days, Month, medium week (C3, C4, F2)

**Files:** Modify `calendar.slint:391-760` (the week view takes `in property <int> days-shown`:
3 in compact, 7 otherwise; a horizontal swipe moves by `days-shown`; Month in compact: two
events a day then "+N", the chosen day's list under the grid).
- [ ] Step 1: tests — 3 columns at 390, 7 at 700; swipe left advances three days; Month's
  day tap fills the list below. Capture `calendar-3days.png`, `calendar-month.png`,
  `fold-week.png`; fmt, commit, push, read CI.

### Task C3: the event sheet, the editor, quick add, calendars (C5, C6, C7, C8)

**Files:** Modify `crates/iris-ui/ui/screens/calendar-panels.slint` (`EventDetailPanel`: Join
first, then repeat, reminder, calendar, guests, tasks for it, notes; `EventEditorPanel` as a
form page with `TimeStrip`), create `controls/pickers.slint` `TimeStrip` (hours 6 wide, then
:00 :15 :30 :45; `in-out property <int> minutes`), a compact quick-add field over the
keyboard for a tapped slot, the calendars side column's content in a `CalendarsSheet`
opened from ⋯ (`screens/calendar-panels.slint`).
- [ ] Step 1: tests — event sheet pulls to full height; the editor's title has the cursor;
  `TimeStrip` 10 then :30 sets 630 minutes; a tapped slot draws the draft and the field
  takes the keyboard, Enter adds the event; calendars toggle visibility.
- [ ] Step 2: implement; capture `event.png`, `event-new.png`, `slot-add.png`,
  `calendars.png`; fmt, commit, push, read CI.

## Phase D — Tasks, goals and habits

### Task D1: the overview and a list (T1, T2, T7)

**Files:** Modify `crates/iris-ui/ui/screens/tasks.slint:1218-1847` (compact root = the side
column's content as a page: four tiles, Habits and This week rows, Goals with rings, Lists;
opening one pushes `page = "list"` and shows today's list view in one column with its header,
nudge, the day's events line, groups and the dock), `crates/iris-app/src/tasks.rs`.
- [ ] Step 1: tests — at 390 the Tasks tab shows the overview; Today pushes the list, Back
  returns; rows are ≥ 56 px with the check's 48 px target; a right swipe ticks, a left swipe
  opens Later; the dock's field is above the tab bar.
- [ ] Step 2: implement; capture `tasks.png`, `tasks-today.png`, `tasks-upcoming.png`;
  fmt, commit, push, read CI.

### Task D2: quick add, duration, the task page, Later, slot (T3, T4, T5, T6, T8)

**Files:** Modify `tasks.slint` (the add field over the keyboard with its tokens above it;
the duration bubble as `Pill`s; `DetailPanel` in compact = a page — add `in property <bool>
page-in-compact: true` to `DetailPanel` in `layout/frames.slint:77`; `LaterMenu` and
`SlotPicker` as sheets with the "put off three times" question first).
- [ ] Step 1: tests — typing "tomorrow 9am Call Marie #Work !!!" shows three tokens and Add
  creates the task; the bubble's "30 min" sets 30; the task page's priority segments; Later
  lists its six choices; the slot sheet blocks 13:30; the keyboard cut of the Review Focus.
- [ ] Step 2: implement; capture `task-add.png`, `task-duration.png`, `task.png`,
  `task-later.png`, `task-slot.png`; fmt, commit, push, read CI.

### Task D3: habits, a habit, a goal (G1–G5)

**Files:** Modify `crates/iris-ui/ui/screens/habits.slint` (`HabitRow` in compact on two lines:
icon, name, sub, streak, then the seven discs of 32 px under it; `HabitPanel` a page with −/+,
three figures, the heat map, the goal, the reminder; `NewHabitPanel` a form page),
`crates/iris-ui/ui/screens/goals.slint` (`GoalPage` in one column: ring card with Log one and
Make time, the chart, the steps; Log one as a sheet over the keyboard).
- [ ] Step 1: tests — a disc tap ticks that day; today's disc is first in the week; − and +
  change today's amount; Log one with a word adds an entry and a done task; at 1280 the
  habit page is the side panel of today.
- [ ] Step 2: implement; capture `habits.png`, `habit.png`, `habit-new.png`, `goal.png`,
  `goal-log.png`; fmt, commit, push, read CI.

## Phase E — Notes

### Task E1: the tree, reading first, open notes (N1, N3, N8)

**Files:** Modify `crates/iris-ui/ui/screens/notes.slint:176-2301` (compact root = the space's
tree page with Pinned and Folders, the space's name with ▾; a note opens as a page in reading
mode with a pencil `Fab` and "Revise N"; the tabs become an *Open notes* sheet from a counter
in the top bar), `crates/iris-app/src/notes/mod.rs` (`notes_tabs` drives the sheet).
- [ ] Step 1: tests — a tree tap pushes `note:<path>` in reading mode; the pencil switches to
  writing with the cursor at the end; Back from writing saves and returns to reading, then to
  the tree; the counter shows the open notes, × closes one.
- [ ] Step 2: implement; capture `notes.png`, `note-read.png`, `notes-open.png`; fmt,
  commit, push, read CI.

### Task E2: writing on a phone (N2, N4)

**Files:** Create `crates/iris-ui/ui/controls/formatbar.slint` (`FormatBar`: B, I, H, list,
checkbox, Σ, link, `/`, ⌄; `callback act(string)` with the same names as the shortcuts'
actions in `notes/saisie.rs`); modify `notes.slint` (the bar sits on the keyboard while a
block is edited; the `/` list in compact is a two-column sheet above the keyboard, filtered
by what is typed).
- [ ] Step 1: tests — B on a selection wraps it in `**`; `/` then "for" leaves Formula only;
  ⌄ hides the keyboard (focus to the page) and the bar; the keyboard cut of the Review Focus.
- [ ] Step 2: implement; capture `note-write.png`, `note-slash.png`; fmt, commit, push, read CI.

### Task E3: the spreadsheet, the graph, revising (N5, N6, N7)

**Files:** Modify `crates/iris-ui/ui/screens/sheet.slint` (compact: the formula bar at the top,
the sheets' tabs and the selection's sum under the grid, the toolbar folded into a key row
on the keyboard, the rest in ⋯), `notes.slint` (`NotesGraphPanel` and `NotesReviewPanel` as
pages; the graph's node card at the bottom with Open; review's four buttons in a row with
their delays).
- [ ] Step 1: tests — tapping B2 while editing "=SUM(" inserts `B2`; the sum line adds the
  selection; Good schedules the card as the desktop does.
- [ ] Step 2: implement; capture `sheet.png`, `graph.png`, `revise.png`; fmt, commit, push,
  read CI.

## Phase F — Settings, first run, Android pages

### Task F1: Settings as pages (S1–S6)

**Files:** Modify `crates/iris-ui/ui/screens/settings.slint:274-541` (compact root = the groups
as a list with coloured tiles; each group a pushed page `settings:<group>`; Appearance gains
the theme previews, accent swatches, density segments and the text-size slider; a new
*Notifications & sync* group shown when `Chrome.has-background` — set by the platform when
`Platform::schedule` does something), `crates/iris-ui/ui/screens/account.slint` (an account as
a page), `crates/iris-app/src/settings.rs` (text size already from adaptive A2).
- [ ] Step 1: tests — root → Appearance → back; the text size slider sets 1.15 and the
  scale follows; *Notifications & sync* hidden on desktop; the account page shows its
  servers and "Remove from this phone"/"Remove" by platform.
- [ ] Step 2: implement; capture `settings.png`, `settings-appearance.png`,
  `settings-account.png`, `settings-sync.png`; fmt, commit, push, read CI.

### Task F2: first run (O1–O4)

**Files:** Create `crates/iris-ui/ui/screens/welcome.slint` (`WelcomePage`: the mark, the line,
three points, "Add your first account", "Set up from my computer" — shown when the sync plan's
Phase C exists, else hidden —, "Restore a backup"; `NotifyChoicePage` with Every 15 minutes /
Instant and two toggles, only when `Chrome.has-background`); modify `account.slint`
(`AccountSetupPanel` compact page: address focused, password, Continue, Google and Microsoft,
Import a profile, Configure manually; the found-servers page with the progress of the first
sync and the tag chips).
- [ ] Step 1: tests — with no account, Iris opens on `WelcomePage`; Add opens the setup with
  the cursor in Address; after the account, the notification page appears on a platform
  with background work and not on Windows.
- [ ] Step 2: implement; capture `welcome.png`, `account-add.png`, `account-found.png`,
  `notify-choice.png`; fmt, commit, push, read CI.

### Task F3: Add to calendar, Send to Iris (system A4, A5)

**Files:** Create `crates/iris-ui/ui/screens/incoming.slint` (`AddToCalendarPage`: the event
card, calendar and reminder rows, the conflict line, Add; `SendToIrisSheet`: what was shared,
then New task, Add to a note, New message, New event), wired by `gui::receive` (platform plan
E1); the same pages serve a `.ics` opened on Windows.
- [ ] Step 1: tests — an `.ics` fixture (example.com organiser) fills the card; Add inserts
  one event and a second open says it is already in the calendar; Send to Iris → New task
  creates a task whose note holds the link.
- [ ] Step 2: implement; capture `add-calendar.png`, `share.png`; fmt, commit, push, read CI.

### Task F4: release 6.4.0

- [ ] CHANGELOG `## 6.4.0` — Improved: "Calendar, Tasks, Notes and Settings fit a narrow
  window too." / New: "An Agenda view in the calendar." / New: "Open an invitation file to
  add it to your calendar." (and any other user-visible line these phases brought).
- [ ] README (Calendar: Agenda; *Getting around*), ARCHITECTURE (*Interface*: compact roots
  and pages per place; the new components `Fab`, `WeekStrip`, `TimeStrip`, `FormatBar`,
  `MailboxesSheet`, `CalendarsSheet`, `WelcomePage`, incoming pages), PLAN (stories ticked).
  fmt, commit, push, read CI, check the Release run.
