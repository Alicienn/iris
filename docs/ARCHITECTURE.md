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

The foundation is complete and exercised: the suite holds about **1,500 tests** (1,358
tests and 142 interface scenarios, measured for 3.4.0),
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

A configuration profile (`.mobileconfig`) is the other way in: `iris-discover::mobileconfig`
reads its `com.apple.mail.managed` payloads from the XML property list. A signed profile
is a PKCS #7 envelope whose content signing tools often send in pieces (an indefinite
length, 1,000-byte octet strings) with framing between them. A small BER reader takes the
content out and puts it back together; the signature is not checked. Bytes that do not
read as an envelope are searched raw, between `<?xml` and `</plist>`. It only fills the manual screen, which the user reads
before saving. The encryption is decided by the port (993 and 465 direct TLS, any other
STARTTLS), never in the clear whatever the profile says; POP and binary profiles are
refused by name.

**Mail in 3.10.0.**

- **Folder in the reading header.** `shell::dossiers_du_fil` names the folder of the
  latest message by its role, or by the last segment of its path for a custom folder.
  It adds "+N" when other messages of the thread sit elsewhere, and says red for the
  bin and spam.
- **Search.** A text search now drops threads `Store::thread_is_binned` finds thrown
  away, by the queues' own rule: put aside, or no message outside trash and junk.
  Before, a conversation deleted from the results stayed in them. Results are sorted
  newest first unless the query holds `sort:relevance`, which `search::run` strips
  before parsing. The pills under the field (`shell::PASTILLES`) add or remove their
  words in the query and search again; which are on is read back from the query.
- **Invitations.** For each expanded message, `shell::invitation_du_message` looks for
  a `text/calendar` / `application/ics` / `.ics` attachment. It reads it from the raw
  message and has `calendar::invitation` set it against the first local calendar, by
  UID and `RECURRENCE-ID` as `import_ics` stores it. The result is one of five states
  on the message's `invite-state`: to add, in the calendar, changed since,
  cancelled-and-still-there, cancelled. The banner is on that message's card only.
  Its button runs `import_ics`, which upserts, so an invitation is never added twice.
  The conversation is then drawn again: its render cache is cleared and a full
  refresh is asked for.

**Mail in 3.11.0.**

- **Answering an invitation** (`iris-app::invite`) builds an iTIP `REPLY` (RFC 5546)
  from the invitation's own lines: UID, SEQUENCE, RECURRENCE-ID, DTSTART and
  DTEND with their parameters, and ORGANIZER. It adds one ATTENDEE, the mailbox
  answering: its line from the invitation with `PARTSTAT` replaced and `RSVP` dropped,
  or a bare one when it was not listed. The reply is folded at 75 octets and sent to
  the organiser as a `text/calendar; method=REPLY` attachment of a short message,
  from the mailbox the invitation came to, through the outbox with no undo delay.
  Accept and Maybe then `import_ics` the invitation; Decline takes it out of the
  calendar (`calendar::remove_invited`). The answer is kept by UID in
  `invite_replies` for the banner. A cancellation, an invitation one organises
  oneself, and one with no organiser are not offered the answer.
- **Sending later** keeps the draft, not a composed message, as JSON in
  `scheduled_mail` (attachments included), with its To and subject beside it for the
  list. A UI timer checks every thirty seconds: what is due is composed then (with that
  date), queued with no undo delay, and taken out of the table. A message that fails
  stays and is said. The times offered are computed by `later::options` from the local
  clock. *Scheduled* in the folder column lists what waits; *Edit* takes a draft out and
  back into the composer.
- **Sorting.** `ListQuery` carries a `Sort` (date, sender, subject, size). By date
  it pages by cursor as before; the others order by `lower(last_from_name)`,
  `lower(last_subject)` or the thread's largest message, and page by `OFFSET`, which
  costs more deep in a list but a queue is not that deep. The day titles do not show
  then, since the rows no longer run by date.
- **Undo in the calendar.** `calendar::retenir` keeps the last twenty changes (the
  event before a move or a stretch, a deleted event with its notes, a colour); Ctrl+Z
  in the calendar pops one and puts it back, a task's slot taking its task along.

**Accounts in 3.12.0.**

- **OAuth clients.** The OAuth clients live in the settings file
  (`OAuthSettings`: Google's client ID and secret, Microsoft's application ID). They
  are typed in *Settings › Sign in with Google or Microsoft* and take effect for the
  next sign-in or refresh (`services.oauth`). Google's desktop clients ask their
  secret back even with PKCE, so `iris_oauth::exchange_code` and `refresh` send
  `client_secret` when there is one. Adding an account whose provider signs in through
  the browser, with no client set but a password typed, goes the password way (an
  app password) instead of failing.
- **Aliases.** They sit in `account_aliases`. The composer's senders
  (`shell::EXPEDITEURS`) are each enabled mailbox followed by its aliases, read again
  when one is added or removed. Sending as an alias composes as the mailbox (its
  server, its signature) and then replaces `From`. A message sent later keeps its alias
  in its JSON.

The mail list's day titles (Today, Yesterday, This week, Earlier) are computed in
`iris_ui::format::day_headers` and ride on the first row of each day, not as items of
their own: the list keeps one item per conversation. They are given only when the rows
run newest first (never over search results). Each row carries how many titles stand
above it, so the right-click that goes through an open menu's veil still finds its row
by arithmetic.

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
5. **No server holds back another.** Since 3.9.1 the accounts due (`tick`) and *Sync
   all* run side by side, `concurrency` at a time (4), with the IMAP pool still capping
   connections per host. Reaching a server and signing in must take less than
   `connect_timeout` (30 s), and an account's whole pass less than `account_timeout`
   (10 min). A pass brings at most 5,000 messages per folder and keeps them, so
   giving up loses only time. Dropping the pass drops its connection and frees its
   place in the pool. The failure is recorded like any other (the red ! on the
   account), and the account is tried again at its next turn. Before, the accounts went
   one after the other with no time limit, and a server that took the connection and
   then said nothing stopped every account after it and every later pass.
   Since 3.14.0 the TCP connection tries a server's IPv4 addresses first, then its IPv6
   ones, 8 s each: on a network whose IPv6 goes nowhere, Gmail's IPv6 addresses each
   held the whole attempt for Windows' twenty seconds and used up `connect_timeout`.

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

Four workspaces share one window, switched with `Ctrl`+`0` to `Ctrl`+`3` or from the
switcher at the top of each place's side column: **Home**, **Mail**, **Calendar**,
**Tasks** (4.0.0, after Apple's own apps; the rail and title strip of 3.x are gone).
The window chrome lives in `shell/chrome.slint`:

- `SidebarFrame` is a place's side column: `SidebarHead` (the window's lights on a Mac
  setting, else the Iris badge; back and forward; the four-place switcher), the
  place's own rows, then `SidebarFoot` (settings, the palette).
- `WindowToolbar` is the 52 px bar over a place's content: it moves the window
  (double click maximises), holds the place's actions, and ends on Windows' three
  buttons unless the Mac's lights are chosen (*Settings › Window buttons*,
  `mac_window_buttons`, by system by default). With no side column (a conversation
  read full screen) it takes the lights at its left end.
- The window is frameless; on Windows 11 its corners are rounded by the system
  (`DwmSetWindowAttribute`, corner preference *round*, set in `main.rs` once the
  window exists), which also gives it its shadow and keeps it square when maximised.
- The parts talk to the window through the `Chrome` global. Slint does not let a
  `.slint` component handle a global's callback, so a button calls `Chrome.ask(what,
  arg)`, which bumps a request counter; `app.slint` aliases that counter and acts in a
  `changed` handler. Dragging the window goes the same way (`drag-start-id`,
  `drag-id`).

`app.slint` holds a `workspace` property (3 is Home), aliased to `Chrome.workspace`, and
renders the matching view over the mail; `workspace.rs` fans the change out to every
Rust follower, because Slint keeps only one handler per callback. The status bar stays
at the bottom.

### Components in layers (2.0.0)

The interface files sit in layer folders under `crates/iris-ui/ui/`, imported through the
`@iris` library path (`build.rs` maps it to that folder), so a file's imports read the
same wherever it lives. A layer only imports the ones above it in this list:

| Folder | Holds |
|---|---|
| `theme/` | `Tokens` (colours, sizes, radii, fonts), `Type` (the named text styles and tones) |
| `base/` | icons, surfaces (`Glass`, `Floating`, `Backdrop`, flat since 3.0.0), spinner, and the atoms: `Label`, `Dot`, `Kbd`, `Hairline`, `Avatar` |
| `controls/` | `Button` (primary, secondary, ghost, danger; two sizes; its key), `Link`, `Segmented`, `QueueTabs`, `Pill`, `Check`, `Toggle`, `PriorityTag`, the text fields, and `pickers.slint`: `CheckBox`, `ComboBox` (a pop-up button) and `SpinBox` with the standard widgets' API, so no screen uses the style's Fluent ones (4.0.0) |
| `lists/` | `NavItem`, `SectionTitle` and `SectionRow` (the side columns, one title row for all), `SectionHeader`, `ListRow`, `TimeRow`, `NowLine`, `PropertyRow` |
| `layout/` | `Rail`, `PageHeader`, `Plate`, `DetailPanel`, `Toolbar`, `EmptyState`, `Modal` + `ModalFooter`, `Popover`, menus |
| `shell/` | the window chrome (`Chrome`, `SidebarFrame`, `WindowToolbar`), toast |
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

**Home** (`home.rs`, `screens/home.slint`, 1.0.0, redrawn in 1.1.0, 2.0.0, 3.0.0 and
4.0.0) is read from the base when it shows, after each sync and each minute while it
stays. It holds the date, a greeting with the first name from the settings, a sentence
built from the To do count, the tasks due (and late) and the events left, then widgets
(4.0.0): **Up next**, the one thing coming, the event under way or next, or the next
task with an hour, whichever starts first (`home::day`, `upcoming`; what is late is not
"next"), and the two events after it; a **Goal**, the first not reached
(`goals::for_home`); **Today**, the first four tasks due or late; **To answer**, the
first three conversations of To do (`list_threads`, initials on the mailbox's tint) and
how many are in Waiting; **This week**, the tasks done a bar a day (`tasks::week`) and
the inbox zero streak. Its side column lists the places with their counts and the
mailboxes. It opens first unless `home_at_startup` is off.

**Goals** (3.3.0, `iris-store::goals`, `iris-tasks::goals`, `iris-app::goals`). A goal is
counted (`goal_entries`, logged by hand with *Log one*) or made of milestones
(`goal_milestones`, ticked in order), due on a day. Its pace is the straight line from
the day it was set to its day: on track within half a step of it, behind beyond,
`pace()` saying by how many and at what rate per week. Rust draws the shapes as path
commands (the ring of the side column on a 16-unit grid, the pace chart on 520 × 120),
so Slint only strokes them. Tasks point to a goal (`tasks.goal_id`, set to NULL when the
goal goes); adding on a goal's page adds a step toward it. *Make time for it* writes one
weekly event (`FREQ=WEEKLY;BYDAY=…;UNTIL=` the goal's day) in the first local calendar.
Today lists up to three goals behind or due within the week. The Tasks view key is
`goal:<id>` or `goals`, so Back returns to them. Nothing is counted automatically.
Since 3.7.0 a goal is changed where it stands. A click on its title swaps the text for
a field, which is focused with the old title selected. The field sits in a `FocusScope`
that takes `Escape`, and closing it hands the keys back (`goal-rename-closed`). The
pencil opens the new-goal window with `editing` set: its fields are filled from the
goal, and `goals::creer(…, Some(id))` rewrites the goal with `update_goal`. The kind and
colour stay, because the log or the milestones belong to it.

**Repeating tasks and the week** (3.7.0). `tasks.repeat` (migration 15) holds `daily`,
`weekdays`, `weekly`, `monthly` or `yearly`. `tasks::toggle_done` is the only way a
task is ticked, from Tasks, Home and an event's panel alike. Ticking a repeating task
inserts its next one on `iris_tasks::repeat::next_day`: one step from its due day,
stepped again until after today, with a month from the 31st landing on the month's
last day. It does not insert when an open task with the same title, list, rule and day
already exists, so untick and tick again makes no second copy. `Vue::Week` (key `week`)
is a view without tasks of its own. It lays out its own sections (late, put off, next
week, done since Monday) and fills the nudges with up to three goals not yet reached.

**When to do a task** (3.4.0). `tasks.estimate` is its length in minutes
(`iris_tasks::goals::parse_duration` reads `45m`, `1h20`). *Find a slot* gathers the
day's timed events (`calendar::upcoming`) and timed tasks, and `iris_tasks::slots::
free_slots` offers the starts where the length fits between 8:00 and 20:00, from now
on. The slot chosen is an event in the first local calendar with the fixed UID
`task-<id>@iris` (so booking again moves it), tied to the task through
`event_uid`/`event_start = 0` so the event lists it; *Remove* deletes it and keeps the
task's hour. *Later* (`Later::day`) sets the day and counts `tasks.postponed`; at three
the details ask whether to split, book or drop the task; a booked slot goes with the
old day.

**Tasks** (the list redrawn in 3.4.1 after the web edition, then in 4.0.0 after
Reminders: a large title in the list's colour, rows as a ledger) puts in the header the
day's progress; it travels as one `TaskOverviewData`. Since 4.1.0 neither the day's
events nor the week's bars show there: Home has them (`tasks::week` counts the tasks
done from `done_at`). The add line parses what is typed at each keystroke
(`iris_tasks::parse`) and shows what it understood as tokens before `Enter`.

**Calendar** opens an event clicked in the grid in a `Popover` beside it: the grid
reports where the event is (`event-anchored`) before it opens it. Opened from Home or a
task, the same card comes up in the middle; the anchor is forgotten when the card closes.
Since 3.2.0 (redrawn after the web edition, the hour 48 px high) a click on a free slot
of the week opens a draft there: a bare field, focused at once, inside a `FocusScope`
that takes `Escape`; `Enter` sends `calendar-quick-event(day, minute, title)` and
`calendar::evenement_rapide` inserts an hour-long event in the first local calendar.
The draft lives in the grid, not in `modal-open`: while it has the focus the keys are
the field's, and closing it hands them back (`draft-closed`). Events are their
calendar's colour at 16 % with a 45 % outline, past ones at 55 % opacity
(`TimedEventData.past`); the small month tints the days shown (`MonthCellData.in-week`).
The column's *Still today* was removed in 3.8.1: the week in front of it already says
it.

A right-click on an event (3.9.0), in the week, the month or the all-day row, sends
`calendar-event-menu(key, x, y)`. Rust makes it the chosen event, as a click does
(`calendar::choisir` fills the card's data without opening it), and opens
`EventContextMenu`. *Edit* and *Delete* then go through the card's own `event-edit` and
`event-delete`, and *Open* opens the card beside the pointer. The colour is
`event_colors (calendar_id, uid, color)` (migration 16): kept beside the events, as the
notes are, because a subscription's events are replaced at each refresh.
`calendar::teinte` prefers it to the calendar's colour everywhere an event is drawn:
the grid, the month, Home's and Tasks' day lines (`upcoming`), the card.

The mail's columns are hidden (`visible: workspace == 0`) while another workspace is
shown. The calendar, the tasks and Home are laid over them, and Slint passes a click on
a part without a `TouchArea` to what lies under it; the software renderer also painted
bits of the mail through them in the regions it redrew. Inside the week's scrolled
grid, `absolute-position` is not to be trusted: what needs a place on screen (the
event card's anchor, a task dropped on the week) counts it from the `ScrollView`'s own
position plus its `viewport-x`/`viewport-y`.

Since 3.6.0 an event can be dragged. `TimedEventData.movable` is true for a local
calendar's event that neither repeats nor is an exception to a repeat. A press becomes a
drag past four pixels, measured in window coordinates (they stay put while the grid
moves). The event stays in place, faded, and a copy drawn after every other event
(Slint's `z` takes only literals) shows where it would land, by quarter hours. On release
the grid sends `calendar-event-moved(key, days, minutes)` or
`calendar-event-resized(key, minutes)`, and the click the release makes is swallowed.
`calendar::deplacer` shifts the event on the local clock, so summer time does not move
its hour, and keeps at least a quarter hour. When the event is a task's slot
(`task-{id}@iris`), `tasks::slot_moved` gives the task the new day and hour. The side
column's *To plan* list (`tasks::to_plan`, `PlanTaskData`) holds today's and late tasks
with no hour, six at most. A task is carried from there as the task list carries tasks:
a ghost under the pointer and a `drop` counter that the grid watches. The grid answers
with the day and quarter under the pointer, and `tasks::book` reserves the slot as
*Find a slot* does.

**Back and forward** (`nav.rs`, 1.0.0). A place is the workspace plus what each one last
showed: the mailbox or tag, folder and tab of the mail, the tasks' view, the calendar's
view. The interface reports every choice (`navigated(kind, value)`), and each new place
is a step; Back and Forward replay a place through the window's own callbacks, with the
recording off. The mouse's buttons, two arrows at the top of the side column, `Alt`+`←` and
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
- **Selecting a message's words (3.14.0).** Rich-text blocks are read-only
  `TextInput`s (`SelectableText`), which select and copy on their own; the keys they
  ignore bubble to a `FocusScope` around the body, which hands the keyboard back to the
  shortcuts and sends the key again a turn later (`key-redispatch`), so `E` still acts
  after a click in the words. Bodies painted by Blitz select in the document itself:
  a press and a drag on a tile go to `TiledDocument::select_from` / `select_to` in
  document pixels (a drag past its tile goes on into the next), Blitz paints the
  highlight, and the tiles on show are painted again. `Ctrl`+`C` at the shortcuts asks
  the document for its words and puts them on the clipboard through a hidden text
  field (`copy-text`), the only way to it Slint offers. It used to be read as `C`,
  *New message*.
- `?` opens the list of keys (`screens/help.slint`), a modal like the others.
- **The palette (`Ctrl`+`K`)** asks Rust for its list when it opens and at each key
  (`palette-query-changed`, filtered by `commands::filter`). The callback was declared
  and wired in Rust but never called, so until 3.14.0 the palette opened empty and
  stayed so.
- Slint is 1.18 since 3.14.0 (1.17 before): its software renderer no longer panics on
  very long lines of text, and skips what opaque elements cover.

### Rendering

- **The window is drawn by the CPU.** Measured with the `mesure` example, same window,
  same newsletter: **209.7 MB** with OpenGL (femtovg), **38.5 MB** with Slint's software
  renderer, for a picture the eye cannot tell apart. Remeasured after the 3.x redesign
  (the rail, message cards, goals, the draggable calendar): **44.0 MB** with the window
  open and the newsletter shown, 9.6 MB at start, 2.0 MB in the notification area. The difference is what the graphics
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

Since 3.14.0 the radii are 6, 9 and 12 px (5, 7, 9 before), and what opens eases in once:
modals, popovers and menus fade and move a few pixels, the task details slide in. Each
runs on an `entree` property set in `init`, so nothing animates afterwards and the
software renderer stays idle. Tasks are cards on the window's ground in a column of at
most 840 px; their details are a floating card rather than a docked strip.

3.15.0 adds the small motions, each played once: a tick is drawn before the box says
it was ticked (`Check.animated`, 300 ms, so the list is not redrawn under it), a new
task (`fresh`) and a dropped event (`landed`) are flagged by Rust for one drawing and
animate from `init`, the queue tabs' plate slides, the star bounces, the now point
pulses. Shadows come in three heights (`shadow-card`, `shadow-float`,
`shadow-window` in the tokens). Scroll areas use `controls/scroll.slint`, a Flickable
with a thin thumb of our own; list views keep the style's.

### Inbox zero

When the *To do* queue is empty, with nothing searched or filtered and a mailbox to
empty, `apply_snapshot` counts the day (`inbox_zero_day` and `inbox_zero_streak` in the
settings, through `settings::update`) and the list shows a medal and the days in a row
instead of an empty sentence.

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
- **Video calls** (3.14.0): a link set by hand lives in `event_links` (migration 19), by
  calendar and UID like the colours, so a subscribed event keeps it; without one,
  `visio::find` reads the first Meet, Teams, Zoom or Webex link in the event's place or
  description, where invitations put theirs. The service is told by the link's host.
  The card and Home offer *Join*, which hands the `https://` link to the system
  (`platform::open_url`, nothing else accepted).
- **Stretching live** (3.14.0): while an event is stretched, each quarter hour lays the
  week out again with its new end (`etirer_en_direct`) and changes the grid's rows in
  place, so the events it reaches share the column as it grows. A new model would
  rebuild the rows and drop the drag under the pointer. A move still draws a copy.

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

Since 3.15.0 a list's order within a day is the hour first (timed tasks by time), then
the order given by hand: a task dropped on another goes above or below it (and takes
its day), and that day's tasks without an hour are renumbered as shown
(`Store::set_task_positions`). Priority no longer reorders; its tag says it.

After a quick add (3.14.0) the new task's length is asked in a bubble over the add bar
that takes nothing from the keyboard: while it shows, `Enter` sends the field to
`task-add-answered`, where nothing skips, a length (`goals::parse_duration`) is kept,
and anything else is the next task. `Later` has *Tonight* (today, 23:59).

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

Minimising (3.14.0) takes the message out of the window: `ranger` keeps it as a bar at
the foot of the window (`REDUITS`, written to `minimised-drafts.json` beside
`draft.json`, attachments kept only while Iris runs) and empties the window, so New
message writes another. A bar clicked puts its message back, opened minimised at the
bar's place and grown a turn later: the window's corners are worked out from its two
fixed shapes, not from the size being animated, so it rises and grows at once.
What the window held when a bar is opened goes down to a bar of its own first.

The reply box's Send and Reply all say, when pointed at, whom they write to: worked
out by `SendService::compose_reply` as sending would, when another conversation shows.

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
| 14 | Goals, their log and milestones; a task's goal, estimate and times put off (3.3.0) |
| 15 | A task's `repeat` rule (3.7.0) |
| 16 | `event_colors`: an event's own colour, by calendar and UID (3.9.0) |
| 17 | `scheduled_mail` (drafts to send later) and `invite_replies` (answers given, by UID) (3.11.0) |
| 18 | `account_aliases`: other addresses a mailbox sends as (3.12.0) |
| 19 | `event_links`: an event's video call link set by hand, by calendar and UID (3.14.0) |

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
  it can sit in the tray for weeks, so "at launch" would mean "never". Since 3.8.0 it
  reads no API. Each release carries `latest.json` (version, tag, installer name, size,
  SHA-256) and `latest.json.sig`, an Ed25519 signature made by the release workflow with
  the project's key (the `UPDATE_SIGNING_KEY` secret). Iris fetches both from
  `releases/latest/download/`, which the site serves outside the API's sixty calls an
  hour per address; a school or an office puts everyone behind one address. Iris checks
  the signature against the public key compiled in (`update::UPDATE_PUBLIC_KEY`), and
  then that the manifest holds together (the tag and installer named for its version, a
  real digest). The installer is downloaded only from this repository's release URLs and
  must match the manifest's size and digest; it runs silently and Inno Setup reopens
  Iris afterwards. A manifest that does not verify is an error, never a reason to try a
  weaker way. A latest release without a manifest can only be older than any build that
  reads them, so it offers nothing. A failed automatic check is shown only in Settings;
  being offline is not news.
- **CI** (`.github/workflows/ci.yml`): format and clippy with warnings denied, the test
  suite on Windows without the HTML engine (no GPU there), and a Windows release build
  kept as an artifact. The same tests run on macOS in `macos.yml`, a separate workflow
  so that Release, which follows CI, does not wait for the slowest runner: the
  installer is Windows-only. The test jobs build unit and integration tests
  only (`--lib --tests`, no examples, no doc-tests); a push that only
  touches `docs/`, a README or `CLAUDE.md` runs nothing, and a newer push to a pull
  request cancels the older run.
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
