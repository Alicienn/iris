# Iris

A desktop mail client, written in Rust, designed to hold **a hundred mailboxes and a
million messages** on one machine without ever ceasing to be immediate. Beside the mail
sit a calendar and a task list, built on the same principles.

Three stances set it apart:

1. **The organising axis is work, not filing.** A conversation is *to do*, *waiting* or
   *done*. No thematic filing is imposed.
2. **Performance is a design constraint**, not an optimisation. It dictates the
   architecture — cursor pagination, event coalescing, a view model off the display
   thread — and not the other way round.
3. **Everything is a module.** The kernel does not know what a mail is. Rules,
   protocols and extensions are modules, and a third-party plugin sees the same
   permission model as an internal module.

---

## Status

The foundation is complete and exercised: the suite holds about **1,472 tests** (1,337
tests and 135 interface scenarios, measured for 3.1.0),
including those that genuinely put synchronisation, the plugin sandbox and the rendering
engine at fault, and two headless interface suites driven by real pointer and key events.

```bash
cargo test --workspace          # the full suite
cargo run -p iris-app -- doctor # checks the installation
cargo run -p iris-app           # starts the application
```

### Command line

```
iris [run]                     Start the application
iris add-account <address>     Add an account (prompts for the password)
iris import <file>             Add accounts in bulk
iris accounts                  List configured accounts
iris sync                      Synchronise once, without the interface
iris doctor                    Check the installation
iris memory                    Break down what the data layer costs
iris register                  Offer Iris to Windows as a mail client
iris unregister                Withdraw that offer
iris --tray                    Start into the notification area
iris mailto:<address>          Start and compose to that address
```

An address and a password are enough: discovery chains an embedded table, the domain's
autoconfiguration, the Mozilla database, DNS `SRV`, `MX` and probing — and **says where
the proposed configuration came from**, so that a guess never passes for a certainty.

---

## Measurements

Taken on an ordinary development machine, with
`cargo test -p iris-app --release --test budgets -- --ignored`:

| Measure | Budget | Observed |
|---|---|---|
| First list shown, 100,000 threads | ≤ 400 ms | **48 to 99 ms** |
| Rows held in memory, 100,000 threads | proportional to what is shown | **60** |
| List page served while scrolling | ≤ 10 ms | **0.7 to 0.8 ms** |
| Full-text search, 200,000 documents | ≤ 80 ms | **1 to 5 ms** |
| Triage action | imperceptible | **97 µs** |
| Cold open of every service | ≤ 2 s | **3.6 ms** |
| Insertion, 100,000 messages | — | **0.92 s** |

The ranges are real: from one run to the next, the first list varies by a factor of two
depending on what the operating system has cached. Publishing the best run would be more
flattering and less true.

A triage action cost **41 µs** in an earlier version. It now costs 97, and that is a
deliberate trade: every action is now announced on the bus — without which plugins do
not see the user's triage — and undoing it restores the snooze and the flags, not only
the state. A measure that never goes up is a measure that has stopped being confronted
with what the product must do.

The last row deserves its story: the first version took **192 seconds**. Attaching a
reply that arrived before its original queried `messages.in_reply_to` without an index,
which made synchronisation quadratic. A regression test now holds that property.

Memory is measured separately, on the real window (see [Rendering](#rendering)).

---

## Architecture

Five layers, plus a cross-cutting kernel. No diagonal dependency: a layer only knows the
contract of the one before it.

```
  Presentation   iris-ui · iris-htmlview · iris-theme        never any I/O
  View model     iris-viewmodel                              testable without UI
  Domain         iris-workflow · iris-thread · iris-rules    pure, no I/O
                 iris-search · iris-mime
                 iris-calendar · iris-tasks
  Data           iris-store (SQLite) · iris-index (Tantivy)  local truth
                 iris-blobs (zstd + LRU)
  Network        iris-sync · iris-imap · iris-smtp           tokio
                 iris-discover · iris-secrets · iris-oauth
  Cross-cutting  iris-kernel · iris-types
                 iris-plugins (host) · iris-plugin-sdk (guest)
  Assembly       iris-app                                    binary, wiring
```

`iris-app` is the composition root: the `iris` binary, the shell that wires the window to
the services, and the pieces that belong to no layer — updates, tray, single instance,
calendar and tasks wiring. The three bundled modules (`iris-plugin-vip`,
`iris-plugin-office-hours`, `iris-plugin-filer`) are WebAssembly crates built on the SDK;
see [PLUGINS.md](PLUGINS.md).

### The four invariants

They take precedence over every other consideration, and each is held by a **structural
property**, not by discipline:

1. **Zero I/O on the display thread.** The view model lives in its own thread and only
   communicates by messages: the code that queries the database does not run where the
   frames are drawn.
2. **Nothing is loaded whole.** The list only holds a prefix, extended by cursor. Memory
   follows what is shown, never what is stored.
3. **Every local action is instant, then reconciled.** Immediate write, idempotent
   operation journal, replay to the server. No hourglass for a user's gesture.
4. **Events are batched.** Coalescing in 16 ms windows, and a batch touching more than
   256 threads degrades into a full refresh — the cost on the interface side stays
   bounded however intense the synchronisation.

### Known departures

Stated here so they are decided rather than discovered:

- **Calendar, tasks and Home read SQLite on the display thread.** Their state lives on
  the UI thread and their queries are small and bounded (a visible period, a list, a
  handful of counts), but they break invariant 1 in letter. Moving them behind the view
  model is the fix if a large calendar ever shows up in a frame time.
- **The mouse's back and forward buttons are read from winit**, through Slint's
  `unstable-winit-030` accessor: inside the interface, whatever row or button is under
  the pointer takes the press first. The accessor is tied to Slint's minor version.
- `iris-ui` and `iris-workflow` depend on `iris-store` directly.

---

## Interface

Four workspaces share one window, switched from the rail on the left (`shell/rail.slint`,
3.0.0) or with `Ctrl`+`0` to `Ctrl`+`3`: **Home** (behind the Iris mark at the top of the
rail), **Mail**, **Calendar**, **Tasks**, each with its count (the mail's To do, the tasks
due or late). The palette and the settings sit at the rail's foot. To the right of the
rail a thin strip moves the window and holds back, forward and the window's buttons;
the status bar stays at the bottom. `app.slint` holds a `workspace` property (3 is Home)
and renders the matching view over the mail, after the rail (`rail-width`);
`workspace.rs` fans the change out to every Rust follower, because Slint keeps only one
handler per callback.

### Components in layers (2.0.0)

The interface files sit in layer folders under `crates/iris-ui/ui/`, imported through the
`@iris` library path (`build.rs` maps it to that folder), so a file's imports read the
same wherever it lives. A layer only imports the ones above it in this list:

| Folder | Holds |
|---|---|
| `theme/` | `Tokens` (colours, sizes, radii, fonts), `Type` (the named text styles and tones) |
| `base/` | icons, surfaces (`Glass`, `Floating`, `Backdrop`, flat since 3.0.0), spinner, and the atoms: `Label`, `Dot`, `Kbd`, `Hairline`, `Avatar` |
| `controls/` | `Button` (primary, secondary, ghost, danger; two sizes; its key), `Link`, `Segmented`, `QueueTabs`, `Pill`, `Check`, `Toggle`, `PriorityTag`, the text fields |
| `lists/` | `NavItem`, `SectionTitle` (the side columns), `SectionHeader`, `ListRow`, `TimeRow`, `NowLine`, `PropertyRow` |
| `layout/` | `Rail`, `PageHeader`, `Plate`, `DetailPanel`, `Toolbar`, `EmptyState`, `Modal` + `ModalFooter`, `Popover`, menus |
| `shell/` | the rail, the title strip, toast |
| `screens/` | one file per screen, and its panels |

`types.slint` (the data crossing from Rust) and `app.slint` (the window) stay at the root.
`crates/iris-ui/tests/architecture.rs` holds the rules: a layer imports only the ones
before it, screens write no colour of their own (black shadows aside), and no field is
the style's `LineEdit` or `TextEdit`.
Every window over the others is a `Modal`: one veil, one card, one close button, one
`Escape`. A card beside what it is about (an event in the week) is a `Popover`. Each
control draws its hover and pressed states once; screens do not. Icons are stroked with
round caps and joins.

The screens were redrawn in 2.0.0 from HTML mockups kept outside the repository, each
port captured with its `apercu_*` example and compared with its mockup side by side.

**Home** (`home.rs`, `screens/home.slint`, 1.0.0, redrawn in 1.1.0, 2.0.0 and 3.0.0) is
read from the base when it shows, after each sync and each minute while it stays. It
holds the date, a greeting with the first name from the settings, a sentence built from
the To do count, the tasks due (and late) and the events left, then **Next**: the one
thing coming, the event under way or next, or the next task with an hour, whichever
starts first (`home::day`, `upcoming`); what is late is not "next". Three links lead to
Mail, Tasks and Calendar with their counts, and a line at the foot names `Ctrl`+`K`. It
opens first unless `home_at_startup` is off.

**Tasks** puts above the list, on Today, a date block and the day's calendar
(`calendar::upcoming`), and in the header the day's progress; all of it travels as one
`TaskOverviewData`. The rail ends on the tasks done this week, a bar a day, counted from
`done_at`. The add line parses what is typed at each keystroke (`iris_tasks::parse`) and
shows what it understood as tokens before `Enter`.

**Calendar** opens an event clicked in the grid in a `Popover` beside it: the grid
reports where the event is (`event-anchored`) before it opens it. Opened from Home or a
task, the same card comes up in the middle; the anchor is forgotten when the card closes.

**Back and forward** (`nav.rs`, 1.0.0). A place is the workspace plus what each one last
showed: the mailbox or tag, folder and tab of the mail, the tasks' view, the calendar's
view. The interface reports every choice (`navigated(kind, value)`), and each new place
is a step; Back and Forward replay a place through the window's own callbacks, with the
recording off. The mouse's buttons, two arrows beside the window buttons, `Alt`+`←` and
`Alt`+`→`, 100 steps kept.

Mail is three columns (236, 380 and the rest, redrawn in 3.1.0 after the web design):
accounts with the folders under them (one column since 2.0.0, with an outlined **New
message** at its top and, at its foot, when the mail last synced), the work queue, and
the conversation with its toolbar and a built-in reply. Above the list sit the search
box, the queues as pills (`QueueTabs`) and the filter chips, over a hairline. Rows run
edge to edge with a hairline under each, two lines (56 px at normal density, 45
compact, 73 comfortable with the excerpt on a third line); the chosen one takes the
accent's soft ground. Each sender gets a round mark with their initials and a stable
tint, the mailbox a dot, unread mail an accent dot in the margin. On hover a row shows
Done, Snooze and Archive; they have no touch area of their own — the row's reads where
it was hit (`action-at`), as it does for the tick box — and they go through the same
`menu-*` callbacks as the right-click menu. The accounts group under their tags by
default: a tag's title (its colour, the To do of its mailboxes) folds its accounts (the
folded tags are a setting) and a click on it filters the queue to its mailboxes, through
the same `FilterAccounts` request as one mailbox. Tags keep the order they are dragged
into.

The conversation runs up to 900 px wide, centred beyond: the subject, the mailbox it
came to, then each message on a framed card (face, name, address, date; the body under
the name; the attachments of the message read as file cards, as many to a row as fit).
The toolbar's Snooze and More open `ReaderMenu` (`reader-menu` in `app.slint`, part of
`modal-open`); snoozing "weekend" means Saturday 8:00 (`heures_de_report`). Read full
screen (`reading-focus`) folds the accounts and the list to nothing without destroying
them, so the list keeps its scroll; `Escape` brings them back.

The three side columns share `lists/nav.slint`: one selection (the active surface and a
bold label, as on the web; the pill inside the edge went in 3.1.0), one hover, one
section title in sentence case. Rows are 30 px everywhere. Panels docked to the
window's edge are square, with a hairline between them; only floating cards are rounded.

### Keyboard, focus and clicks

These rules come from bugs users hit, and each has a scenario in
`crates/iris-ui/tests/saisie.rs`, driven by real pointer and key events:

- **One `FocusScope` for global shortcuts, placed beneath everything.** A Slint
  `FocusScope` that does not have the focus consumes the click that gives it focus; on
  top, it swallowed the first click on every field.
- **An open window owns the keyboard.** `modal-open` in `app.slint` is the OR of every
  overlay — compose, settings, modules, palette, menus, calendar and task panels. While
  it is true, only `Escape` (and `Ctrl`+`K`) get through; when it turns false, focus goes
  back to the shortcuts.
- Home, calendar and tasks have their own keys (Home has none) and accept them, so a
  mail key like `E` never acts on a thread that is not on screen. `Ctrl`+`Z` in Tasks
  brings back the last task deleted, not a mail action.
- Text fields are `TextField` / `TextArea` from `controls/field.slint`: focus shows on
  the border, never the background, and AltGr characters are accepted. A field inside a
  frame that shows the focus itself (the task add line, the reply) is `bare`.

### Rendering

- **The window is drawn by the CPU.** Measured with the `mesure` example, same window,
  same newsletter: **209.7 MB** with OpenGL (femtovg), **38.5 MB** with Slint's software
  renderer, for a picture the eye cannot tell apart. The difference is what the graphics
  driver reserves for a GL context, and it shows in no counter of the application. A
  mail client redraws little, and the software renderer repaints only what changed.
  `SLINT_BACKEND=winit-femtovg` restores the old path, for comparison only.
- **Message bodies** are rich text by default, and go to a **full HTML engine** (Blitz)
  only when the layout demands it — newsletters in nested tables, pixel-exact layouts.
  The choice is measured, not guessed. Blitz lays the document out **once**, at the
  width actually shown and the real display scale, then paints it **on the CPU, in
  512-point tiles**, straight into the interface's buffer. Only tiles near the viewport
  are painted; those scrolling away are released. There is no GPU device of our own and
  never one image the height of the message: a 51,000-pixel newsletter costs two tiles,
  which `cargo test -p iris-app --test memoire` enforces.
- Past 4 MB of HTML, 20,000 tags or 400,000 pixels of height, the body falls back to rich
  text, which cannot fail.
- **Memory is given back in the tray.** When the window closes to the notification area,
  body tiles and painter caches are dropped (the layout is kept), then the working set
  is returned to Windows. Tiles repaint on demand when the window comes back.

### Light and dark

Since 3.0.0 Iris has one look in two lights: `iris-theme` ships `light` and `dark`
(TOML files compiled in: colours, radii, type sizes, row height, durations) and nothing
else. User themes, their folder and the watcher that reloaded them are gone, and so is
the glass: surfaces are flat, a hairline where two meet, a shadow only under what floats.
The user's `appearance` is System (the default), Light or Dark. System reads Windows'
`AppsUseLightTheme` at start and every three seconds after, and switches when it
changes; applying a theme also sets the style's `Palette.color-scheme`, so its own
controls (check boxes, scroll bars) follow. The font is Segoe UI Variable, Text for
reading and Display for titles.

---

## Calendar

`iris-calendar` is pure and knows nothing of the database or the window. All times are
UTC milliseconds; time zones matter only when reading and displaying.

- `ics` reads iCalendar (RFC 5545) — a subscription, or an invitation attached to a mail.
  A malformed event is dropped without taking the others with it.
- `recur` expands recurrence rules in the event's own zone, so 10:00 stays 10:00 across a
  daylight-saving change; exceptions and modified occurrences included.
- `layout` builds the month grid and the week/day columns, where simultaneous events
  share the width.
- `link` recognises subscription links (`webcal://` becomes `https://`).

The application side (`crates/iris-app/src/calendar.rs`):

- **Subscriptions** are read at once, then every 30 minutes, with `ETag` /
  `If-Modified-Since` and a 20 MB cap. A refresh replaces the calendar's events as a
  whole; subscribed calendars are read-only.
- **Invitations**: opening an `.ics` attachment imports it into the first local calendar,
  matched on `UID` + `RECURRENCE-ID`. Opening the organiser's update or cancellation
  modifies the existing event — that is what "kept up to date" means. There is no import
  during sync.
- **Reminders**: a 30-second timer scans the next two days and shows a Windows
  notification. A reminder more than two minutes late (the PC was asleep) is skipped.
- **Event notes** (0.6.0) live in their own table, not on the event, because a
  subscribed event is replaced at every refresh. The key is
  `(calendar, UID, occurrence start)`, so a note on Monday's meeting is not next
  Monday's. Notes follow a local event moved to another calendar and go with it when it
  is deleted; an empty note is deleted.
- **Colours** (0.6.0): an eight-colour palette, written to the existing
  `calendars.color` column.
- **Tasks of an event** (1.0.0): added from the event's panel, due when the occurrence
  starts (the day alone for an all-day event), keyed like the notes by UID and
  occurrence but **not** by calendar, so hiding, unsubscribing or deleting the calendar
  leaves them in Tasks.
- **The view** opens on the week the first time, then on the one last chosen
  (`calendar_view` in the settings).

## Tasks

`iris-tasks` is pure as well:

- `quick` reads a one-line entry, in English or French — `tomorrow 9am Call Marie
  #Work !!`, `vendredi 14h30 Dentiste` — into title, due date, time, list and priority. It
  is conservative on purpose: a bare number is never a date, English day abbreviations
  are not read (`mon` is also a French word), and anything not understood stays in the
  title.
- `due` says when a task is due, in words, and which section it belongs to.

The application side (`crates/iris-app/src/tasks.rs`) builds the views — **Today**,
**Upcoming**, **All tasks**, **From mail**, and the user's lists. One predicate says
whether a task belongs to a view, for the open tasks and the completed ones alike, so a
task checked from Today stays there struck through until the completed are cleared. A
task made from a conversation keeps the thread's identifier *and* a "Sender: Subject"
line, so it outlives the thread; there is at most one task per thread. Reminders use the
same 30-second timer pattern as the calendar, with the due-reminder state stored in the
database so a reminder fires once.

A delete keeps the task and its subtasks in memory; `Ctrl`+`Z` puts them back under
their own identifiers (`Store::restore_tasks`), unless their list has gone since.

## Sending, and undoing a send

The outbox (`crates/iris-smtp/src/outbox.rs`) holds each message for a delay **before**
sending it, never after: recalling a sent message is impossible, and pretending otherwise
would lie. The delay is a setting (`undo_send_seconds` in `iris.toml`, 5 s by default,
0 to 30), and each message keeps the delay in force when it was queued.

Sending closes the compose window at once and clears the saved draft. A notice at the
bottom of the window counts down; **Undo** cancels the queued message and reopens the
compose window from what was captured at Send time — recipients, subject, body,
attachments, sender. A second send replaces the notice, and the first message simply
goes out. `crates/iris-app/tests/envoi.rs` covers both paths on a headless window.

---

## Data

`iris-store` is SQLite in WAL mode, synchronous on purpose — SQLite is anyway, and an
async façade would only hide the real cost. Bodies and attachments live in `iris-blobs`,
full text in `iris-index`.

The schema version is `PRAGMA user_version`. Each migration runs in its own
transaction, a published migration is never edited, and a database written by a newer
Iris is refused rather than misread.

| # | Migration |
|---|---|
| 1 | Initial schema: accounts, folders, threads, messages, references, attachments, correspondents, operation journal, settings |
| 2 | Rules, and which message each rule has already acted on |
| 3 | Spam flag backfilled on existing mail |
| 4 | Binned mail taken out of the queue (reverted by 6) |
| 5 | Journalled moves that could never replay, repaired |
| 6 | The bin is a folder, not a state |
| 7 | A thread remembers which mailbox it came from |
| 8 | A signature per mailbox |
| 9 | Calendars and events, with a "Personal" calendar (0.3.0) |
| 10 | Task lists and tasks, with "My tasks" (0.5.0) |
| 11 | Threads put aside from Done, and event notes (0.6.0) |
| 12 | Tags on one's own mailboxes, and their links (0.7.0) |
| 13 | Tags in an order of one's own; tasks tied to an event's UID and occurrence (1.0.0) |

A new table or column always arrives as a new migration.

---

## Running in the background

- **One Iris per session.** A named mutex (`Local\Iris.SingleInstance`) and event make a
  second launch hand over to the first: it leaves any `mailto:` for it, wakes it, and
  exits. Before this, a second process found the database locked and died silently, and
  the shortcut seemed to do nothing. `Local\` means one per signed-in user, not per
  machine.
- **The tray.** Closing the window keeps Iris running in the notification area; the only
  way out is Quit. The icon is polled on the main thread every 200 ms rather than through
  callbacks, because a callback from another thread may not touch the window. The unread
  count in its tooltip is rewritten only when it changes.
- **Start at login** writes one value under `Run`, passing `--tray`, and is switched in
  Settings.

## Updates and releases

- **The changelog is compiled in.** `CHANGELOG.md` is included in the binary and parsed
  by `crates/iris-app/src/changelog.rs`; the same file feeds the Changelog window, the
  GitHub release notes and the repository. A test fails if its first section is not the
  `Cargo.toml` version, or if the format breaks.
- **The updater is part of the core, not a module**: the plugin sandbox has no network,
  no file system and no way to start a process, and an updater needs all three. Iris
  asks GitHub for the latest release 8 seconds after launch and then every 6 hours —
  it can sit in the tray for weeks, so "at launch" would mean "never". The installer is
  downloaded only from this repository's release URLs, checked against the size and
  SHA-256 GitHub publishes, and run silently; Inno Setup reopens Iris afterwards. A
  failed automatic check is shown only in Settings — being offline is not news.
- **CI** (`.github/workflows/ci.yml`): format and clippy with warnings denied, the test
  suite on Windows and macOS without the HTML engine (no GPU there), and a Windows
  release build kept as an artifact.
- **Release** (`.github/workflows/release.yml`) runs after every green CI on `main`. If
  the `Cargo.toml` version has no release yet, it reuses the CI binary, builds the
  installer, tags `vX.Y.Z` and publishes it with that version's changelog section.
  The same version twice publishes nothing. See [packaging/README.md](../packaging/README.md).

---

## Extensions

Plugins are WebAssembly, run in a sandbox. The question "can a plugin do harm?" has three
answers, each verified by a test that genuinely puts a plugin at fault:

- **fuel** bounds execution time — an infinite loop stops within microseconds, and the
  plugin is disabled;
- **memory** is capped — a greedy plugin is refused, not the machine;
- **capabilities** bound what it may ask for, and the keychain is never granted: a plugin
  able to read passwords is no longer a plugin.

A failing plugin is disabled, never fatal. The contract lives in
[`wit/iris.wit`](../crates/iris-plugins/wit/iris.wit); a complete example, run by the
tests, is in
[`examples/marquer-infolettres`](../crates/iris-plugins/examples/marquer-infolettres).

Plugins run in their own thread: a plugin that loops burns its fuel, not a frame. What
they receive is deliberately poor — sender, subject, labels, never the body nor the
account identifier — and what they ask for comes back as intents, applied through the
same path as the keyboard. The commands they declare appear in the palette without a
restart.

Google and Microsoft accounts go through the system browser, never an embedded window:
one must see one's provider's address bar to know who is being given the password. The
client identifier is not in the binary — a secret handed to everyone is not one — it is
set in `iris.toml`, and its absence is stated plainly rather than disguised as an
authentication failure.

---

## Privacy

- Remote images blocked by default; merely loading them tells the sender when the
  message was read and from which IP address.
- Tracking pixels told apart from legitimate images by their dimensions, style or domain
  — confusing the two would make the warning useless.
- HTML sanitised before rendering: no script, no form, no external resource.
- Secrets in the system keychain, with a fallback to an Argon2id + XChaCha20-Poly1305
  vault. The type that carries them never displays its content, not even in debug
  output.
- Network traffic beyond the mail servers: the calendars the user subscribes to, and
  GitHub for the latest release, a request that carries nothing about mail or accounts.
- No telemetry.

---

## Documentation

- [Design specification](superpowers/specs/2026-09-13-iris-design.md) — the original
  decisions and their reasons, and what changed since.
- [Implementation plan](PLAN.md) — epics and stories, with their status.
- [Modules](PLUGINS.md) — the bundled plugins, and what running them took.
- [Comparison](COMPARISON.md) — Iris beside the clients people arrive from.
- [Packaging](../packaging/README.md) — the installer, and what it keeps.
- [Changelog](../CHANGELOG.md) — what users got, version by version.

## Licence

MIT or Apache-2.0, at your option.
