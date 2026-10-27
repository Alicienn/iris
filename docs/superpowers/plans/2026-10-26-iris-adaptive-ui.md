# Adaptive Interface Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The existing Slint interface lays itself out by window width (compact < 600 px,
medium 600–840, expanded ≥ 840) and sizes itself for touch, with windows as pages and
sheets, a tab bar and rail, stacked pages behind Back, and swipe and long press, all
testable on Windows in a narrow window.

**Architecture:** A new global `Layout` (`ui/theme/layout.slint`) holds the window's width
class, the touch flag, the safe-area insets and the text scale; `AppWindow` feeds the width,
the platform feeds the rest. `Type` and the row tokens read it. The frames (`Modal`,
`Popover`, `MenuSurface`) choose page, sheet or card from it. The shell draws a tab bar
(compact) or a rail (medium) and stacks pages; `nav.rs` gains a `page` in `Place` so Back
pops a page before it changes place.

**Tech Stack:** Rust 2021, Slint 1.17 (software renderer), `i-slint-backend-testing`.

**Spec:** `docs/superpowers/specs/2026-10-26-iris-android-design.md` (§1–§7); mockups in
`..\Iris-design\android\` (`index.html`, `adaptive.html`).

## Global Constraints

- The rules of `docs/superpowers/plans/2026-10-26-iris-mobile-index.md` (no local tests;
  fmt, commit, push, read CI; changelog and version; documents in the same commit).
- Thresholds: compact `< 600px`, medium `< 840px`, expanded `≥ 840px` of the window's width.
- Touch sizes: buttons 44 px, rows ≥ 48 px, mail rows 76 px in compact, checks 26 px in a
  48 px target, tab bar 66 px, sheet radius 22 px, sheet handle 40×5 px.
- Swipe threshold 96 px; long press 500 ms; a drag only starts after a long press on touch.
- Text scale 0.85 to 1.30, default 1.0.
- At expanded width with `touch-input: false` nothing changes on screen: every existing
  `saisie.rs` and `interface.rs` scenario passes unmodified at 1280×800.
- `AppWindow.min-width` goes from 720 px to 320 px, `min-height` from 480 px to 320 px (a
  phone in landscape).

## Review Focus

- Resizing a PC window across 840 px and 600 px while a modal is open or a conversation is
  read: the modal stays open and keeps its text, the conversation stays opened (Task A1
  tests a resize from 1280 to 390 and back with the composer open).
- A sheet opened at compact width and the window then widened: it becomes a card in place,
  still in `modal-open`, Escape still closes it (Task B1).
- Back on a pushed page while a sheet is open: the sheet closes first, the page stays
  (Task C2 tests Back with the snooze sheet open on a conversation).
- A swipe that ends under the threshold, or turns into a vertical scroll: nothing happens,
  the row returns (Task D1 tests 80 px and a 30° drag).
- Text scale 1.30 on a 360 px wide window: titles clip with an ellipsis, nothing overlaps
  the tab bar (Task A2 asserts no element of the mail list crosses `TabBar.y`).

---

## Phase A — Layout and scale (no visible change at expanded width)

### Task A1: the `Layout` global

**Files:**
- Create: `crates/iris-ui/ui/theme/layout.slint`
- Modify: `crates/iris-ui/ui/app.slint:381-406` (min-width, feed `Layout`, export it)
- Modify: `crates/iris-ui/src/lib.rs` (re-export `Layout`, `WidthClass` from the generated code)
- Test: `crates/iris-ui/tests/saisie.rs` (new helper and scenarios, added to the list in `main` at :2908)

**Interfaces:**
- Produces (Slint):
  ```slint
  export enum WidthClass { compact, medium, expanded }
  export global Layout {
      in-out property <length> window-width: 1280px;
      in-out property <length> window-height: 800px;
      out property <WidthClass> width-class: window-width < 600px ? WidthClass.compact
          : window-width < 840px ? WidthClass.medium : WidthClass.expanded;
      out property <bool> compact: width-class == WidthClass.compact;
      in-out property <bool> touch-input: false;   // set by the platform
      in-out property <length> inset-top: 0px;     // status bar, cut-out
      in-out property <length> inset-bottom: 0px;  // gesture bar
      in-out property <float> text-scale: 1.0;
      out property <length> target: touch-input ? 48px : 28px; // minimum hit height
  }
  ```
- Produces (Rust test helper): `fn fenetre_de(w: f32, h: f32) -> AppWindow` in saisie.rs.
- [ ] Step 1: tests — `la_largeur_choisit_la_classe` (1280 → expanded, 700 → medium,
  390 → compact, read with `f.global::<Layout>().get_width_class()`);
  `redimensionner_garde_le_composer_ouvert` (open compose at 1280, resize to 390×844 with
  `window().set_size`, `get_compose_open()` still true and its text unchanged, back to 1280).
- [ ] Step 2: implement: `AppWindow` sets `Layout.window-width/height` in `init` and in
  `changed width` / `changed height`; `min-width: 320px`. fmt, commit, push, read CI.

### Task A2: the touch scale and the text scale

**Files:**
- Modify: `crates/iris-ui/ui/theme/type.slint:38+` (`Type.size` reads `Layout`)
- Modify: `crates/iris-ui/ui/theme/tokens.slint:79-80` (row height and padding read `Layout`)
- Modify: `crates/iris-ui/ui/lists/nav.slint:14`, `lists/rows.slint:95`, `controls/button.slint`
  (heights `max(…, Layout.target)`)
- Modify: `crates/iris-app/src/settings.rs:71-165` (`text_scale: f32`, default 1.0, clamped
  0.85–1.30 in `sanitize`), `crates/iris-app/src/shell.rs:3501` (`appliquer_apparence`
  sets `Layout.text-scale`)
- Test: `crates/iris-ui/tests/interface.rs`, `crates/iris-app/src/settings.rs` tests

**Interfaces:**
- Consumes: `Layout` (A1).
- Produces: `Type.size(s)` = the desktop or touch value of the spec's §4 table ×
  `Layout.text-scale`; `Tokens.row-height` becomes `max(56px × density, touch-input ? 76px : 0)`
  for mail rows only through a new `Tokens.mail-row-height`.
- [ ] Step 1: tests — touch off: every size equals today's (a table test over `TextStyle`);
  touch on: body 15, caption 13, title 32; text-scale 1.3 multiplies; settings clamp 0.5 →
  0.85 and 2.0 → 1.30; the Review Focus check: at 360×780, touch on, scale 1.3, the last
  visible list row's bottom ≤ the tab bar's top (asserted after Task B3 adds the tab bar —
  write it now, `#[ignore]`d with the reason, and un-ignore in B3).
- [ ] Step 2: implement; fmt, commit, push, read CI.

### Task A3: the platform feeds `Layout`

**Files:**
- Modify: `crates/iris-app/src/shell.rs` (`build`: set `touch-input`, insets from
  `platform::Platform::current().insets()` — the trait comes from the Android platform plan,
  Task A2; until it lands, set `touch-input` from `IRIS_TOUCH=1` and insets to 0)
- Modify: `crates/iris-app/examples/` (new `apercu_mobile.rs`, see Phase E)

**Interfaces:**
- Consumes: `Layout`; `Platform::insets() -> (f32, f32)` and `Platform::touch() -> bool`
  when available.
- [ ] Step 1: test in `crates/iris-app/tests/cablage.rs`: with `IRIS_TOUCH=1`,
  `get_touch_input()` is true after `shell::build`.
- [ ] Step 2: implement; fmt, commit, push, read CI.

## Phase B — Windows over the others

### Task B1: `Modal` as a page or a sheet

**Files:**
- Modify: `crates/iris-ui/ui/layout/frames.slint:160-253` (`Modal`, `ModalFooter`)
- Create: `crates/iris-ui/ui/layout/sheet.slint` (`SheetSurface`: scrim, card from the
  bottom, handle, pull-down to dismiss)
- Test: `crates/iris-ui/tests/saisie.rs`

**Interfaces:**
- Produces:
  ```slint
  export enum ModalKind { form, question }   // form → page in compact; question → sheet
  // Modal gains: in property <ModalKind> kind: ModalKind.form;
  //              in property <string> action-label;  callback action();  // page top bar
  export component SheetSurface inherits Rectangle {
      in property <length> sheet-height;    // preferred; clamped to parent.height - 48px - Layout.inset-top
      in property <bool> pullable: true;    // pull down 80px to dismiss
      callback dismissed();
      @children
  }
  ```
  In compact, a `form` Modal fills the window: radius 0, a 56 px top bar (×, title,
  `action-label` button when set), its footer hidden when `action-label` is set; a
  `question` Modal is a `SheetSurface`. Medium and expanded: today's card.
- [ ] Step 1: tests — at 390×844: the account panel's card is at x 0, width 390; the
  delete-folder question sits at the bottom (`y + height == 844`); a tap on the scrim and a
  pull of 100 px close it, 60 px does not; Escape closes both; `modal-open` true while open;
  the first field has the focus on opening (rule 3); widening to 1280 with the sheet open
  turns it into a centred card, still open (Review Focus).
- [ ] Step 2: implement; mark every Modal in `screens/` with its kind (questions:
  `CalendarDeletePanel`, `EmptyFolderPanel`, `TaskListDeletePanel`, `GoalDeletePanel`,
  `HabitDeletePanel`, `NotesNamePanel`; everything else `form`). fmt, commit, push, read CI.

### Task B2: `Popover` and `MenuSurface` as sheets

**Files:**
- Modify: `crates/iris-ui/ui/layout/frames.slint:275-312` (`Popover`),
  `crates/iris-ui/ui/layout/menu.slint:9-138` (`MenuEntry` height `max(30px, Layout.target)`,
  `MenuSurface`)
- Test: `crates/iris-ui/tests/saisie.rs`

**Interfaces:**
- Produces: `Popover` and `MenuSurface` gain `in property <bool> anchored-in-compact: false`;
  in compact and not anchored they draw as a `SheetSurface` (`Popover`: half the window,
  pullable to full; `MenuSurface`: its entries' height). The reader's ⋯ menu
  (`ReaderMenu`, conversation.slint) sets `anchored-in-compact: true`.
- [ ] Step 1: tests — at 390×844: the thread context menu (long press arrives in D2; open
  it here with `f.set_context_menu_open(true)`) is at the bottom, full width; the reader ⋯
  menu stays under its button; the event detail popover is a sheet of half height; all
  close on Escape and on the scrim.
- [ ] Step 2: implement; fmt, commit, push, read CI.

### Task B3: the tab bar and the rail

**Files:**
- Create: `crates/iris-ui/ui/shell/tabbar.slint` (`TabBar`), `crates/iris-ui/ui/shell/navrail.slint` (`NavRail`)
- Modify: `crates/iris-ui/ui/shell/chrome.slint:324-443` (`SidebarHead` hides its places
  unless expanded; `WindowButtons` hidden when `Layout.touch-input`), `crates/iris-ui/ui/app.slint`
  (place the bar at the bottom in compact, the rail at the left in medium; StatusBar hidden
  in compact, its sync text moves to the mail subtitle in Screens B1)
- Test: `crates/iris-ui/tests/saisie.rs`

**Interfaces:**
- Produces:
  ```slint
  export component TabBar inherits Rectangle {   // 66px + Layout.inset-bottom
      in property <int> workspace;  in property <int> mail-badge;  in property <bool> tasks-badge;
      callback picked(int);         // 3 Home, 0 Mail, 1 Calendar, 2 Tasks, 4 Notes, in that order
  }
  export component NavRail inherits Rectangle {  // 80px wide; the place's main action on top
      in property <int> workspace;  in property <int> mail-badge;  in property <bool> tasks-badge;
      in property <string> action-icon;  callback action();  callback picked(int);  callback settings();
  }
  ```
  `picked` sets `Chrome.workspace`, calls `workspace-changed` and then `shortcuts.focus()`
  (rule 4), exactly as `PlaceTab` does today.
- [ ] Step 1: tests — at 390: the tab bar is visible, `SidebarHead`'s places are not; a
  click on "Calendar" sets workspace 1 and the next typed key reaches the shortcuts; at 700:
  the rail is visible, the bar is not; at 1280: neither; with `touch-input` the window
  buttons are gone; un-ignore A2's overlap test.
- [ ] Step 2: implement; fmt, commit, push, read CI.

## Phase C — Stacked pages and Back

### Task C1: a page in `Place`

**Files:**
- Modify: `crates/iris-app/src/nav.rs:19-99` (`Place.page`, kind `"page"`), `crates/iris-ui/ui/app.slint:942`
  (`navigated("page", value)`)
- Test: `crates/iris-app/src/nav.rs` tests

**Interfaces:**
- Produces: `Place { …, page: String }` — `""` for the place's root, else one of
  `"thread:<id>"`, `"compose"`, `"event:<id>"`, `"task:<id>"`, `"habit:<id>"`, `"goal:<id>"`,
  `"note:<path>"`, `"settings"`, `"settings:<group>"`, `"search"`. `Place::after("page", v)`
  returns the place with `page = v`; any other kind resets `page` to `""`. `app.slint`
  gains `in-out property <string> page` and calls `navigated("page", …)` when a screen pushes.
- [ ] Step 1: tests — `after("page","thread:4")` keeps the workspace and sets the page;
  `after("folder","Sent")` clears the page; history back from a page returns the root.
- [ ] Step 2: implement; fmt, commit, push, read CI.

### Task C2: Back, in order

**Files:**
- Modify: `crates/iris-ui/ui/app.slint:1579-2014` (Escape and `nav-back` order),
  `crates/iris-app/src/nav.rs` (`wire_navigation`: apply `page` on back and forward)
- Test: `crates/iris-ui/tests/saisie.rs`

**Interfaces:**
- Produces: one order for Escape, Android Back and `Alt`+`←` in compact: (1) close the top
  sheet or menu, (2) close the top page-modal, (3) pop `page`, (4) history back. In medium
  and expanded, Escape keeps today's behaviour; `Alt`+`←` is (4).
  `callback system-back() -> bool` on `AppWindow` returns false when nothing was left to do
  (the platform then lets Android leave the app).
- [ ] Step 1: tests — at 390: open a thread (page `thread:1`), open the snooze sheet,
  `invoke_system_back()` closes the sheet only; again pops to the list; again goes back in
  history; on a fresh window it returns false.
- [ ] Step 2: implement; fmt, commit, push, read CI.

## Phase D — Touch gestures

### Task D1: `SwipeRow`

**Files:**
- Create: `crates/iris-ui/ui/lists/swipe.slint`
- Test: `crates/iris-ui/tests/saisie.rs` (with `porter(f, de, a)` from :1126)

**Interfaces:**
- Produces:
  ```slint
  export component SwipeRow inherits Rectangle {
      in property <bool> enabled: Layout.touch-input;
      in property <string> start-label; in property <image> start-icon; in property <color> start-color;
      in property <string> end-label;   in property <image> end-icon;   in property <color> end-color;
      callback swiped-start();   // dragged right past 96px and released
      callback swiped-end();     // dragged left past 96px and released
      @children
  }
  ```
  Horizontal only when the drag's angle is under 20° from horizontal; otherwise the parent
  scrolls. Below the threshold the row springs back (`Tokens.quick`).
- [ ] Step 1: tests — drag 120 px right fires `swiped-start` once; 80 px fires nothing; a
  drag of 120 px at 30° fires nothing; disabled when `touch-input` is false.
- [ ] Step 2: implement; fmt, commit, push, read CI.

### Task D2: long press, and drag after a long press

**Files:**
- Create: `crates/iris-ui/ui/base/press.slint` (`LongPress` behaviour over a `TouchArea`)
- Modify: `crates/iris-ui/ui/lists/rows.slint:62-95`, `lists/nav.slint:70-99` (their
  `context-menu(x, y)` also fires on long press when `touch-input`)
- Test: `crates/iris-ui/tests/saisie.rs` (`cocher_le_temps()` at :46 drives the clock)

**Interfaces:**
- Produces: `export component LongPress inherits TouchArea { callback long-pressed(length, length); }`
  — fires after 500 ms without moving more than 8 px; a press that moves first is a scroll.
  The drags of `calendar.slint:696-760`, `calendar.slint:1050-1128`, `tasks.slint:258-350`,
  `tags.slint:60-80`, `notes.slint:590-616` start only after `long-pressed` when `touch-input`.
- [ ] Step 1: tests — hold 600 ms on a thread row opens the context menu; hold 300 ms does
  nothing; a 20 px move during the hold cancels; with touch on, a quick drag on an event does
  not move it, a long press then drag does.
- [ ] Step 2: implement; fmt, commit, push, read CI.

### Task D3: hover affordances on touch

**Files:**
- Modify the hover-only controls listed by line: `list.slint:130,257`, `conversation.slint:685`,
  `calendar.slint:242,251,259,1130`, `sidebar.slint:237`, `modules.slint:151`, `tasks.slint:627`,
  `notes.slint:2437`, `sheet.slint:149`, `calendar-panels.slint:65`
- Test: `crates/iris-ui/tests/interface.rs`

**Interfaces:**
- Produces: each `opacity: area.has-hover ? 1 : 0` becomes
  `opacity: area.has-hover || Layout.touch-input ? 1 : 0`, except the thread row's quick
  actions (`list.slint:257`), which are replaced by the swipe on touch (Screens B1).
- [ ] Step 1: test — with touch on, the attachment actions and the task's Later arrow are
  visible without hover.
- [ ] Step 2: implement; fmt, commit, push, read CI.

## Phase E — Seeing it

### Task E1: `apercu_mobile`

**Files:**
- Create: `crates/iris-app/examples/apercu_mobile.rs` (the pattern of
  `examples/apercu_accueil.rs:15-160`: temp `Paths::under`, sample data, `capture(f, path)`
  every 700 ms, `IRIS_THEME=dark`)
- Modify: `CLAUDE.md` (the command, under the previews)

**Interfaces:**
- Produces: `cargo run -p iris-app --no-default-features --example apercu_mobile -- <dir> [width] [height]`
  (default 412×892, `touch-input` on) writing `home.png`, `mail.png`, `conversation.png`,
  `compose.png`, `calendar-agenda.png`, `tasks-today.png`, `habits.png`, `notes.png`,
  `settings.png` and a `sheet-snooze.png`. Each screen plan adds the captures it needs.
- [ ] Step 1: write it; this is the one step that runs locally, and only when the owner
  asks: `$env:SLINT_BACKEND="winit-software"` then the command into
  `..\Iris-design\android\port\`; compare side by side with `..\Iris-design\android\shots\`.
- [ ] Step 2: fmt, commit (the example and CLAUDE.md), push, read CI.

### Task E2: release 6.3.0 (with Screens A–B)

- [ ] CHANGELOG `## 6.3.0` — Improved: "Iris fits a narrow window: one column, a tab bar,
  sheets instead of floating windows." / New: "Text size in Settings › Appearance."
- [ ] README (*Getting around*: the tab bar and Back in a narrow window; *Appearance*: text
  size), ARCHITECTURE (*Interface*: `Layout`, width classes, the frames' kinds, `Place.page`;
  the components table gains `SheetSurface`, `TabBar`, `NavRail`, `SwipeRow`, `LongPress`),
  PLAN (an epic "One interface for PC and Android" with these stories ticked), the Android
  spec untouched. fmt, commit, push, read CI, check the Release run.
