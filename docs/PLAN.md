# Iris — Implementation plan

Reference: [design specification](superpowers/specs/2026-09-13-iris-design.md).

Order imposed by the dependencies: the contract, then the kernel, then the data, then
the domain, then the network, then the presentation. A story is done when it compiles,
its tests pass, and it is committed.

**Test convention**: each crate keeps its unit tests in `src/` and its integration
tests in `tests/`. No test depends on the network; protocols are tested against
in-memory simulated servers.

---

## E0 — Foundations

- [x] **S0.1** Git repository, `.gitignore`, specification, plan.
- [x] **S0.2** Cargo workspace, build profiles, shared lints.
- [x] **S0.3** Check access to the package registry and to the heavy dependencies.

## E1 — `iris-types`: the shared contract

- [x] **S1.1** Typed identifiers (`AccountId`, `MessageId`, `ThreadId`, `FolderId`, `BlobId`).
- [x] **S1.2** Address, headers, message envelope, flags.
- [x] **S1.3** Workflow states and legal transitions.
- [x] **S1.4** Common error type and conversions.

## E2 — `iris-kernel`: modular kernel

- [x] **S2.1** Typed, multi-subscriber, asynchronous event bus.
- [x] **S2.2** Time-based coalescing of diffs (16 ms window).
- [x] **S2.3** Module registry and lifecycle (`init` / `start` / `stop`).
- [x] **S2.4** Capabilities: declaration, grant, refusal.
- [x] **S2.5** Typed, reloadable configuration.

## E3 — `iris-store`: metadata

- [x] **S3.1** SQLite opened in WAL mode, performance settings, versioned migrations.
- [x] **S3.2** Schema: accounts, folders, messages, threads, states, known correspondents.
- [x] **S3.3** Cursor (keyset) pagination on the main list.
- [x] **S3.4** Idempotent operation journal: write, read, acknowledge.
- [x] **S3.5** Synthetic data set and measurement on 1 M messages.

## E4 — `iris-blobs`: bodies and attachments

- [x] **S4.1** zstd-compressed write and read, addressed by identifier.
- [x] **S4.2** Size-bounded LRU cache, eviction, statistics.

## E5 — `iris-index`: full-text search

- [x] **S5.1** Tantivy schema, incremental writer, commit.
- [x] **S5.2** Queries, pagination, match highlighting.

## E6 — `iris-mime`: message parsing

- [x] **S6.1** MIME parsing: envelope, parts, attachments, encodings.
- [x] **S6.2** HTML sanitising: scripts, forms and external resources removed.
- [x] **S6.3** Detection of tracking pixels and known trackers.
- [x] **S6.4** Unsubscribe extraction (RFC 2369 and RFC 8058).

## E7 — `iris-thread`: grouping into threads

- [x] **S7.1** JWZ algorithm on `Message-ID`, `In-Reply-To`, `References`.
- [x] **S7.2** Fallback on normalised subject within a time window.
- [x] **S7.3** Cross-account joining, opt-in.

## E8 — `iris-workflow`: state machine

- [x] **S8.1** Manual transitions with an undo stack.
- [x] **S8.2** Automatic transitions, each one switchable.
- [x] **S8.3** Snooze and follow-up when due.

## E9 — `iris-rules`: rules engine

- [x] **S9.1** Rule model: conditions, actions, order, stop.
- [x] **S9.2** Evaluation against a message.
- [x] **S9.3** Dry run over the history, with a sample.

## E10 — `iris-search`: query language

- [x] **S10.1** Lexing and parsing (`from:`, `has:`, `older_than:`, `state:`, free text).
- [x] **S10.2** Planning: what goes to the store, what goes to the index.
- [x] **S10.3** Pinned searches as persistent views.

## E11 — `iris-discover`: automatic configuration

- [x] **S11.1** Mozilla ISPDB and the domain's autoconfig.
- [x] **S11.2** SRV records, then MX, then probing of the usual ports.
- [x] **S11.3** Guided manual fallback and validation of the configuration.

## E12 — `iris-secrets`

- [x] **S12.1** OS keychain.
- [x] **S12.2** Encrypted fallback (Argon2 + AEAD).

## E13 — Protocols

- [x] **S13.1** IMAP client: connection, capabilities, select, header fetch.
- [x] **S13.2** IDLE, CONDSTORE, QRESYNC with a `UIDVALIDITY` fallback.
- [x] **S13.3** Bounded connection pool, per-server quotas.
- [x] **S13.4** SMTP: sending, authentication, error handling.
- [x] **S13.5** Google and Microsoft OAuth2, token refresh.

## E14 — `iris-sync`: orchestration

- [x] **S14.1** Scheduler by account priority.
- [x] **S14.2** IDLE assignment and adaptive polling.
- [x] **S14.3** Incremental reconciliation and divergence detection.
- [x] **S14.4** Operation journal replay and conflict resolution.

## E15 — `iris-viewmodel`

- [x] **S15.1** Row window, prefetch, targeted invalidation.
- [x] **S15.2** State selectors and diffs for the interface.
- [x] **S15.3** User actions: immediate local application, journalling.

## E16 — `iris-theme`

- [x] **S16.1** Token schema and TOML loading.
- [x] **S16.2** The three shipped themes: `mono`, `ice`, `sand`.
- [x] **S16.3** Hot reload by watching files.

## E17 — Plugins

- [x] **S17.1** Versioned WIT contract.
- [x] **S17.2** wasmtime host, permissions, CPU and memory quotas.
- [x] **S17.3** Example plugin and end-to-end tests.

## E18 — `iris-ui`: Slint interface

- [x] **S18.1** Three-column shell, theme tokens anchored.
- [x] **S18.2** Sidebar: unified, pinned, groups, account search.
- [x] **S18.3** Virtualised list fed by the row window.
- [x] **S18.4** Conversation view and collapsed threads.
- [x] **S18.5** Command palette.
- [x] **S18.6** Inline reply and sending with a 10 s undo.
- [x] **S18.7** Glass material: blur, edges, grain.

## E19 — `iris-htmlview`: body rendering

- [x] **S19.1** `HtmlRenderer` trait and a rich-text fallback implementation.
- [x] **S19.2** Blitz engine, off-screen rendering into an image.

## E20 — `iris-app`

- [x] **S20.1** Module assembly, configuration, entry points.
- [x] **S20.2** Adding an account end to end.
- [x] **S20.3** Performance measurements against the specification's budgets.

---

## E21 — Keeping the specification's promises

The foundation is complete, but three chains stop before their last link: bodies are
never downloaded, nothing is indexed, and replying sends nothing. Until they are
closed, reading, search and the workflow only work in theory.

- [x] **S21.1** Indexing during sync, then re-indexing when the body arrives.
- [x] **S21.2** Body downloaded on open, cached and attached.
- [x] **S21.3** Sending a reply: composition, send queue, move to Waiting.
- [x] **S21.4** Waking due snoozes and follow-ups, in the background loop.
- [x] **S21.5** Search from the interface, wired to the planner.

## E22 — The missing screens

- [x] **S22.1** Manual account configuration, when discovery fails.
- [x] **S22.2** Settings: theme, density, workflow automations.
- [x] **S22.3** Suspended accounts: reported and resumed.

## E23 — What is built but nobody reaches

An audit of the application layer reveals four complete, tested chains that are never
called from the interface. These are not design gaps: the code exists and works, but
nothing invokes it. An unreachable feature costs the same as a missing one, and lies
about what the application can do on top.

Two of them touch requirements set from the start: Google support, and "everything is a
module".

- [x] **S23.1** Plugins run: loaded at startup, subscribed to the bus, their commands in
      the palette.
- [x] **S23.2** Google and Microsoft OAuth from the add-account screen.
- [x] **S23.3** Attachments: recorded on download, listed, savable.
- [x] **S23.4** Per-account counters in the sidebar.
- [x] **S23.5** Sync status, visible while it happens.
- [x] **S23.6** The comments that became wrong once the screens existed.

## E24 — Software, not a prototype

Two reservations raised at the end of E23 still hold, and one of them is embarrassing:
Slint was chosen over Makepad **partly for its accessibility**, and no element of the
interface declares a role. The argument was right; not using it makes it hollow.

The two go together: Slint's headless test harness finds elements by their
accessibility label. Writing interface tests therefore forces the interface to be
accessible, and accessibility stops being an intention.

- [x] **S24.1** Accessibility roles and labels on everything clickable.
- [x] **S24.2** Headless interface tests, driving the real window.
- [x] **S24.3** Continuous integration: build, tests, format and lint.

## E25 — Foundations: the engines nobody was calling

An audit of what the application actually invokes found three crates entirely
unwired, and two of them duplicating logic that was live elsewhere. A feature that
cannot be reached costs as much as one that does not exist, and lies about what the
application can do on top.

- [x] **S25.1** One state machine: undo, snooze and follow-ups in a single engine.
- [x] **S25.2** Rules: stored, run on arrival, and simulated before they are trusted.
- [x] **S25.3** Subject-based regrouping, opt-in and bounded.

## E26 — English, and the screens for the modules

- [x] **S26.1** Everything the user reads, in English: interface, palette, status bar,
      command line, doctor report, log lines, error messages, dates.
- [x] **S26.2** A modules screen: rules read as sentences, plugins with their powers.
- [x] **S26.3** Compose: a new message, not only a reply.
- [x] **S26.4** More themes, and a picker that shows them.

## E27 — What only an installed copy can do

Three capabilities Windows grants through the registry and a Start Menu shortcut, and
withholds from a loose executable. They are grouped because they share a prerequisite:
an installer. Until one exists, none of them can be built and tested honestly.

The ordering below is by how often the absence is felt, not by effort.

- [ ] **S27.1** `mailto:` links. Registering under `SOFTWARE\Clients\Mail`,
      `RegisteredApplications` and the `Capabilities` key so Windows offers Iris as a
      default mail client. Without it, an address clicked in a browser can never open
      Iris — the most visible gap of the three.
- [ ] **S27.2** System notifications. Windows toasts require an `AppUserModelID`
      declared by a Start Menu shortcut; without one, nothing appears, or it appears
      under a generic host name. "New mail has arrived" is the whole point of a client
      that syncs in the background, so this decides whether background sync is worth
      having.
- [ ] **S27.3** Start at login, as a setting rather than an installer checkbox. The
      installer registers the possibility; **the switch lives in Settings**, next to
      the other automations, because starting itself is something the application does
      on its own and every one of those is switchable in one place.

Prerequisite, not a story of its own: an installer (Inno Setup produces a single
`.exe`; WiX produces an `.msi` for managed deployment). Two things it will not fix —
SmartScreen, which only a signing certificate silences, and portability: Iris keeps
its data in `%APPDATA%\Iris`, so a copy on a USB stick still leaves traces. A portable
mode that keeps the database beside the binary is a separate, small piece of work.

## E28 — What sixteen screenshots showed

An epic written from a user's own list rather than from a plan. Half of it turned out
to be one cause with many faces, which is the usual shape of a bug report: people
describe symptoms, and the symptoms are further apart than the fault.

- [x] **S28.1** An opaque ground for anything that floats. Every `surface` token is a
      white wash at three to nine percent — right on our own backdrop, invisible over
      the application. The context menu, the tooltips and the settings panel were all
      the same mistake, and there are now `panel` and `panel-high`, mixed from each
      theme's own two colours so they stay right in both directions.
- [x] **S28.2** Icons instead of characters. "◆▣◉⏱" exist in Unicode and in almost no
      interface font: a starred thread showed a lozenge because that is the one Inter
      carries, and an attachment showed an empty box. Also the last two "✕" glyphs and
      a gear that looks like a gear.
- [x] **S28.3** The grain tiles. `image-fit: preserve` centres what it cannot fill, so
      a 64-pixel texture drew one square in the middle of every empty panel — the grey
      patch that hovered above "Nothing to do".
- [x] **S28.4** A Delete that deletes, and errors that reach the user. Moving mail that
      is already in the bin produces nothing to send, so the action reported
      "unchanged" and the button appeared dead; and a failed action was a log line in a
      build with no console.
- [x] **S28.5** The bin is not work to do. 485 of 740 messages were in Trash and every
      one of them was in the queue. Schema 4 clears the backlog, ingest stops it coming
      back, and a thread with one message deleted and a reply in the inbox stays.
- [x] **S28.6** Remote images, end to end. No image had ever rendered — Blitz fetches
      through a `NetProvider` and none was installed, so the button under "N images
      blocked" had nowhere to go. There is one now: `data:` with no network, HTTPS only
      for a message the reader has unblocked, once, no redirects, capped at 8 MB.
- [x] **S28.7** An account is reachable from its own row. Right-click for sync,
      password, servers, pin, disable, remove — changing a password used to require the
      mailbox to fail first so the warning marker would appear. Editing rewrites the
      account in place, because deleting and recreating would cost the user their
      mailbox history over a hostname typo.
- [x] **S28.8** Counts shorten to 1.2k and 1.24M with the exact figure on hover, the
      collapse chevron stays in its column, the density chips match the queue tabs, and
      "After N days" is a row rather than a box the height of the panel.

## E29 — Folders, batches, and the system

- [x] **S29.1** A log on disk. A release build has no console and aborts on panic: a
      crash left one event-viewer line saying `0xc0000409` at an offset in a stripped
      binary. Everything tracing produces now goes to a rotated file, a panic hook
      writes the message and backtrace straight to it, and the release profile keeps
      its symbol table so the backtrace has names in it.
- [x] **S29.2** HTML signatures show their logo. The logo is not a web address — it is
      a part of the message, referenced by `cid:`. Nothing could fetch it because there
      was nothing to fetch. The parser keeps those parts and they go back into the body
      as data: URLs before rendering.
- [x] **S29.3** Bulk selection. A marked set beside the current thread, never folded
      into it: the current row decides what the reading pane shows, the marked set
      decides what the next action hits. Extending only ever adds.
- [x] **S29.4** Folders as one set across every mailbox. A folder is a name, not a
      place; creating one creates it everywhere, through the operation journal; picking
      an account *and* a folder crosses the two.
- [x] **S29.5** A tray icon. Left click restores, right click offers open or quit,
      hover gives the unread count. It is what makes closing the window reversible, and
      therefore what makes background sync honest.
- [x] **S29.6** E27, all three. `mailto:` needs three registry entries agreeing, not
      one. Notifications are real toasts under Iris's own AppUserModelID, which means
      an installed copy — the workaround is to borrow PowerShell's identity, and a
      notification that lies about its sender is worse than none. Start at login writes
      one value under `Run` and passes `--tray`.
- [x] **S29.7** An installer. Inno Setup, no elevation, everything in the user's
      profile. The Start Menu shortcut carries the AppUserModelID, which is the one
      line that makes notifications exist. Uninstalling removes the cache and keeps the
      mail.
- [x] **S29.8** A module browser and per-module settings. Install from a folder with no
      network at all, or from an HTTPS catalogue whose address is a setting with no
      default. Permissions above the install button, never behind it.
- [x] **S29.9** The build directory had reached 188 GB and filled the disk, which is
      what took the application down. Line tables only in dev, no debug info for
      dependencies, and free space in `iris doctor`.

## E30 — Releases that reach people (0.2.0)

- [x] **S30.1** A changelog written for users, compiled into the binary, shown from the
      status bar. A test ties its first section to the `Cargo.toml` version.
- [x] **S30.2** Updates from GitHub releases: checked at launch and every six hours,
      verified by size and SHA-256, installed silently in one click.
- [x] **S30.3** Publishing is a push: a green CI on `main` with a new version builds the
      installer, tags it and releases it with its changelog section.

## E31 — A window five times lighter (0.3.0)

- [x] **S31.1** Slint's software renderer instead of OpenGL: 38.5 MB against 209.7 MB
      for the same window.
- [x] **S31.2** Message bodies laid out once by Blitz and painted on the CPU in tiles, at
      the width shown. No GPU device of our own, never one image the height of the
      message.
- [x] **S31.3** Memory given back while Iris waits in the tray.
- [x] **S31.4** One Iris per session: a second launch hands over to the first.
- [x] **S31.5** One click reaches its field, and no shortcut reaches the mail behind an
      open window — each rule with a scenario in `saisie.rs`.

## E32 — A calendar beside the mail (0.3.0, 0.4.0)

- [x] **S32.1** `iris-calendar`: iCalendar reading, recurrence in the event's own zone,
      month and week layout, subscription links.
- [x] **S32.2** Local calendar, events that repeat and remind (schema 9).
- [x] **S32.3** Subscriptions by link, re-read every half hour.
- [x] **S32.4** An invitation opened from a message goes into the calendar, and its
      updates and cancellations modify the same event.
- [x] **S32.5** A right-click menu on calendars; dates picked in a small month.

## E33 — Tasks (0.5.0)

- [x] **S33.1** `iris-tasks`: one-line entry in English or French, due dates in words.
- [x] **S33.2** Lists, steps, notes, due dates and reminders (schema 10).
- [x] **S33.3** Today: tasks due or late, today's events, conversations still to do.
- [x] **S33.4** A conversation becomes a task that opens the message again.

## E34 — Undo a send, and notes on events (0.6.0)

- [x] **S34.1** Sending closes the window at once; a notice offers the undo and brings the
      message back as it was. The delay is a setting, 5 s by default.
- [x] **S34.2** Personal notes on any event, subscribed calendars included, kept apart
      from the event so a refresh cannot erase them (schema 11).
- [x] **S34.3** A colour per calendar.
- [x] **S34.4** Conversations in Done can be deleted.

## E35 — Tags for the mailboxes, calendars of one's own (0.7.0)

- [x] **S35.1** Tags on one's own mailboxes: made, renamed, recoloured in their window,
      ticked from an account's right-click menu with a search that can create one
      (schema 12). The accounts column groups by tag.
- [x] **S35.2** Local calendars besides Personal, each event put in the one chosen.

## E36 — Drafts, and tasks one can carry (0.8.0)

- [x] **S36.1** The software renderer selected where the window opens, not only for the
      memory command: 55 MB instead of 161 MB with OpenGL.
- [x] **S36.2** Save draft to the account's Drafts folder, kept locally if the server
      refuses; closing a started message asks.
- [x] **S36.3** Tasks dragged onto a list or Today; the add line at the bottom; priority
      as a tag; checked tasks stay struck through where they were checked.

## E37 — Home, and a way back (1.0.0)

- [x] **S37.1** A Home screen behind the name in the title bar: greeting, a quote, four
      figures, the tasks coming up and the week's events, each leading to its place.
      Opens first unless switched off in Settings.
- [x] **S37.2** Back and forward between the places visited: the mouse's own buttons,
      two arrows beside the window buttons, Alt+Left and Alt+Right.
- [x] **S37.3** Tags fold, show their mailboxes' mail on a click, and keep the order
      they are dragged into (schema 13). Grouping by tag is on by default.
- [x] **S37.4** Tasks added from an event, due when it starts, kept if its calendar goes.
- [x] **S37.5** One look for the three side columns; square corners where a panel meets
      the window, no straight line through a rounded one; one-line fields that keep
      one line's height; compact rows of two lines that fit.
- [x] **S37.6** A task deleted from its panel comes back with Ctrl+Z; the calendar opens
      on the week, or on the view last chosen.

## E38 — Designed first, then built (1.1.0)

Screens are now drawn as a mockup before they are written in Slint, restricted to what
the software renderer can draw, and the port is compared with the mockup side by side.
The mockups are a working tool and stay out of the repository.

- [x] **S38.1** Home redrawn: a dial of the day (the 24 hours round the Iris mark,
      today's events as arcs, the present), one sentence on what is waiting, and mail,
      today and tasks on one plate. A conversation opens from Home.
- [x] **S38.2** `ui/kit.slint`: buttons, links, column headers, checks, avatars and
      priority tags drawn once, with their states.
- [x] **S38.3** Icons stroked with round caps and joins.
