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

- [x] **S27.1** `mailto:` links. Registering under `SOFTWARE\Clients\Mail`,
      `RegisteredApplications` and the `Capabilities` key so Windows offers Iris as a
      default mail client. Without it, an address clicked in a browser can never open
      Iris — the most visible gap of the three.
- [x] **S27.2** System notifications. Windows toasts require an `AppUserModelID`
      declared by a Start Menu shortcut; without one, nothing appears, or it appears
      under a generic host name. "New mail has arrived" is the whole point of a client
      that syncs in the background, so this decides whether background sync is worth
      having.
- [x] **S27.3** Start at login, as a setting rather than an installer checkbox. The
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

## E39 — One set of parts for every screen (2.0.0)

Ten kinds of buttons, five checks, five section titles, four side-column rows and seven
window frames had grown screen by screen. They are now one set, in layers, and every
screen is drawn again from an approved mockup with it.

- [x] **S39.1** Interface files in layer folders (theme, base, controls, lists, layout,
      shell, screens), imported through `@iris`.
- [x] **S39.2** The shared parts: text styles, atoms, one `Button`, `Segmented`,
      `QueueTabs`, `Pill`, `Check`, `NavItem`, list rows, frames, `Modal`, `Popover`.
- [x] **S39.3** One `Modal` for every window (settings, modules, changelog, account,
      folders, calendars, tasks, signature, source).
- [x] **S39.4** Home v3: the day in a sentence, the next three things, three ways out.
- [x] **S39.5** Tasks v2: the day and its calendar above the list, the day's progress,
      the tokens understood while typing, a panel of properties, the week's work.
- [x] **S39.6** Calendar v1: a range title, one view switch, today in red, the hour of
      now, an event's card beside it.
- [x] **S39.7** Mail v2: folders under the accounts, the queues as the list's title,
      rows of two lines, a named toolbar, a one-line reply.

## E40 — The web edition, brought home (3.x)

An interactive web edition of Iris, with sample data, served as the design reference;
the application is brought to it screen by screen.

- [x] **S40.1** Two themes, light and dark, with the reference's colours, sizes and
      radii; the appearance follows Windows by default. User themes and glass removed.
- [x] **S40.2** The rail on the left: the Iris mark for Home, Mail, Calendar and Tasks
      with their counts, search and settings at its foot; a thin strip for the window.
- [x] **S40.3** Home as in the reference: the day in a sentence, the one thing next,
      three ways in; the greeting takes the first name set in the settings.
- [x] **S40.4** Mail as in the reference (3.1.0): outlined New message, tags with their
      colour and count, the last sync at the column's foot; the queues and filters as
      pills; flat rows with Done, Snooze and Archive on hover; a toolbar with Snooze
      until when, Waiting, full-screen reading and More; each message on a framed card
      up to 900 px wide, its attachments as file cards. No summary or other AI feature.
- [x] **S40.5** Calendar as in the reference (3.2.0): outlined New event, a click on a
      free slot creates the event in place, tinted translucent events, overlaps side by
      side, the week tinted in the small month, what is left of today.
- [x] **S40.6** Goals as in the reference (3.3.0): a target or milestones by a date,
      their pace and chart, a log kept by hand, steps as tasks, time blocked weekly in
      the calendar, nudges on Today. Nothing counted automatically.
- [x] **S40.7** Task timing as in the reference (3.4.0): how long a task takes, Later
      with how often it was put off and a question at three, a free slot found in the
      day and booked in the calendar.
- [x] **S40.8** The last gaps with the reference (3.5.0): the mail list cut into days,
      a goal's log behind a switch when the page is narrow; and an account added from a
      configuration profile (`.mobileconfig`).
- [x] **S40.9** A calendar one can move things in (3.6.0): events dragged and
      stretched by quarter hours, today's tasks without an hour dragged onto the week,
      a task's slot carrying the task with it.
- [x] **S40.10** Goals changed where they stand, tasks that come back, the week in
      review (3.7.0): a goal renamed by its title and edited with the pencil, repeating
      tasks (migration 15), *This week* in Tasks.
- [x] **S40.11** Updates behind a shared address (3.7.1): the check falls back to the
      release page when the API's hourly limit turns it away.
- [x] **S40.12** Signed updates (3.8.0): a manifest signed with the project's Ed25519
      key beside each installer, read from the site instead of the API; the unsigned
      fallbacks removed. Memory remeasured after the redesign: 44 MB, window open.
- [x] **S40.13** What was seen in use (3.8.1): clicks and paint no longer reach the mail
      under the other workspaces, the event card and a dropped task placed with the
      grid's scroll counted, *Still today* removed.
- [x] **S40.14** An event's menu (3.9.0): right-click to open, edit, delete, or give it
      a colour of its own that outlives a subscription's refresh (migration 16).
- [x] **S40.15** Sync that never hangs (3.9.1): a time limit on reaching a server and
      on each account's pass, and the accounts synced side by side.
- [x] **S40.16** Mail that says where it is (3.10.0): the folder in the reading header,
      invitations as a banner over their message, search pills and results by date, a
      deleted conversation gone from the results.
- [x] **S40.17** Accessibility of what 3.x added (3.10.1): named combo boxes in the task
      details, the pace chart and the folder said in words, how an event moves told,
      and *To plan* usable without a mouse (its clock button and default action book
      the first free time).
- [x] **S40.18** What the other clients have (3.11.0): answering an invitation (iTIP
      REPLY), sending later with a Scheduled list, sorting the list, Ctrl+Z in the
      calendar (migration 17).
- [x] **S40.19** Accounts (3.12.0): browser sign-in with an OAuth client set in
      Settings (Google's secret sent), aliases to send as (migration 18), a profile's
      account chosen among several, app passwords accepted for Gmail and Outlook.
- [x] **S40.20** Fixes (3.12.1): table rows in a body wrap (an unwrapped layout table
      made a card over 32,767 px wide and the software renderer aborted on its rounded
      corner), the invitation's answer buttons laid out in a row of their own, the
      window opened maximised.
- [x] **S40.21** Cozier and quicker (3.14.0): words selected and copied in both kinds
      of body, inbox zero with its streak, a task's length asked after a quick add,
      Tonight in Later, `?` for the keys, every folder named over "Inbox +2", tasks as
      cards in a centred column with floating, shorter details (no Goal field), larger
      radii and one-time entrance animations.
- [x] **S40.22** Several drafts, video calls, Google (3.14.0): messages minimised to
      bars at the foot of the window and kept on disk, reopened rising and growing at
      once; video call links on events (migration 19, found in invitations) with Join
      on the card and Home; Continue with Google; recipients over Send and Reply all;
      a tag's "!"; a manual sync said in the status bar; the palette filled and
      filtered (its query never reached Rust); an event stretched live with the week
      laid out again; the goal page compacted, its title kept on leaving the field;
      IPv4 first, 8 s an address, for Gmail on networks with dead IPv6; Slint 1.18;
      visual fixes (today's disc, key caps, the folded column, Home's line, the
      changelog's wrapping).
- [x] **S40.23** Motion and order (3.15.0): the tick drawn before it is said (300 ms,
      tasks, steps, milestones), the title struck through as it goes, a new task
      sliding in with a halo, the star's bounce, the queue tabs' sliding plate, the
      progress bars filling, the now point's single pulse, a dropped event's bounce;
      three shadow heights; our own thin scroll bar; tasks reordered by dragging
      (`set_task_positions`, the order given after the hour within a day); Send and
      Reply all laid out again.

## E41 — After Apple's own apps (4.0.0)

An HTML mockup in the style of Apple's Mail, Calendar and Reminders, with a Windows
variant, brought into the application.

- [x] **S41.1** The window chrome: each place's side column with the four-place
      switcher, back and forward at its top, settings and the palette at its foot; a
      52 px toolbar over the content; the Mac's lights or Windows' buttons, chosen in
      the settings. The rail and the title strip removed.
- [x] **S41.2** Apple's palette in both themes, segmented controls on a grey well,
      round blue checks, blue selections with white text in every side column.
- [x] **S41.3** Mail: one toolbar for the queues, the actions and search; rows with a
      blue plate when selected and an inset hairline; messages without frames.
- [x] **S41.4** Calendar: today on a red disc, the time in a red capsule, events pale
      with a coloured edge. Tasks: tiles for the four views, the title in the list's
      colour with its count, a white page.
- [x] **S41.5** Home as widgets: up next, a goal, today's tasks, mail to answer, the week.
- [x] **S41.6** A pass against the mockup, screen by screen and in both themes: one
      section title row, Mac push buttons, white fields, our own check box, pop-up
      button and stepper in place of the style's, grouped settings with switches,
      scroll bars that fade, a `group` colour for inset plates.
- [x] **S41.7** Closer still (4.1.0): rounded window corners on Windows 11, a
      message's recipients in its header (`StoredMessage::recipients_json`), calendars
      and lists as the mockup draws them, the account filter behind a magnifier, the
      week card and the day strip removed from Tasks; starring no longer re-renders
      the open message (its signature ignores the read, star and answered flags).
- [x] **S41.8** Home, Calendar and Tasks laid flush with the window as the mail is
      (4.1.1): the pixel's inset kept from the old frame showed as a pale border.
- [x] **S41.10** The column's foot (settings, search) in the status bar's stretch under
      it, on the window's edge (4.2.0); the header's line across the pane; events
      rounded all round (their bar inset); subjects regular; no search on Home.
- [x] **S41.9** Home tells the truth (4.2.0): To answer counts every To do
      conversation, every mailbox is listed (`home-accounts`, unfolded) with its
      failure in red, faces by sender, tomorrow's first event when today is done.
- [x] **S41.11** Settings as the mockup's page (4.3.0): beside the side column, groups
      of rows each led by a coloured tile, switches and pop-ups at the right; an accent
      colour among eight, over the theme, light and dark.
- [x] **S41.12** A new mark (4.4.0), chosen among six proposals: "Trio", three arcs
      (mail blue, calendar red, tasks orange) round a pupil, on a white plate; in
      `iris.ico` (16 to 256 px: the executable, the installer, the notification
      area), the README and the side column's badge.
- [x] **S41.13** Mail that tells the truth (4.4.1): Google accounts sign in (the IMAP
      greeting is read before `AUTHENTICATE XOAUTH2`), a message binned on another
      device leaves Inbox at the next pass, not the next deletion scan, and a sync
      failure is written to the log.
- [x] **S41.14** Each message leaves through its own mailbox (4.5.0):
      `sending::AccountMailer` picks the account from the `From` (its address or one
      of its aliases) and signs in to its SMTP server with its password or, for Google
      and Microsoft, `XOAUTH2`. Before, everything went through the first enabled
      mailbox. Sent messages are filed in their mailbox's Sent folder (they never
      were); Gmail's is left to Gmail, which files one itself.
- [x] **S41.15** A first Gmail sync that does not freeze the window (4.5.1): the
      view model folds the diffs waiting into one and answers with one snapshot; the
      copies of one message (inbox and All Mail) share a thread, are counted and read
      once, and threads split that way before are joined when the base opens.
- [x] **S41.16** A folder of one mailbox read by its threads' own messages (4.5.2):
      `+m.account_id` keeps SQLite off `messages_by_account` in the scope's correlated
      subquery. Measured on a 21,000-message Gmail: one page 3.1 s → 1 ms, the tab
      counts over two minutes → 39 ms.
- [x] **S41.17** The critical findings of an audit of sending, receiving, folders and
      message state, fixed (4.5.3). Sending: a failure is said and the message put
      back (it was announced as sent), a scheduled message leaves the list only once
      gone, quitting waits for queued mail, Undo decided under the outbox's lock, the
      sender kept as an address, a reply after one's own message goes to the
      correspondent. Journal: repeated intentions reach the server, a dropped
      connection is retried, each account replays its own queue, a vanished folder
      no longer blocks it. Folders: deletion empties what the server holds first,
      roles guessed only at the top and never over a `SPECIAL-USE` one, the menu acts
      on the exact path. A body is not fetched across a `UIDVALIDITY` change.
- [x] **S41.18** The high findings of the same audit, fixed (4.6.0). Accounts: IMAP
      STARTTLS, logins other than the address (profiles, autoconfig, a *User name*
      field) and a separate sending password, *Sign in again* for Google and
      Microsoft, Google Workspace recognised, duplicates and edits that no longer
      touch another account. Receiving: flags re-read without CONDSTORE and merged
      with ours, accounts that recover on their own, disabled ones out of the
      schedule, bounded body downloads. Managing: undo reaches the server, copies in
      Sent and Gmail labels left alone, thread state kept across moves
      (`thread_ghosts`), replies reopen done threads, emptying deletes for good, no
      duplicate without MOVE. Folders: modified UTF-7, the server's delimiter and
      prefix, Gmail's Starred and Important left out, unread counted once. Sending:
      Reply-To and Cc, invitation answers as a calendar part.
- [x] **S41.19** The medium findings of the same audit, fixed (4.7.0). Previews and
      attachment marks at arrival from the start of the text, spam headers read, an
      unreadable batch fetched one by one, first syncs newest first, polling every
      15 minutes at most. System certificates for IMAP, DNS records outside the
      domain refused, Google identity checked, old tokens renewed. Flags after a move
      by `Message-ID`, UID operations dropped after a rebuild, rescues to the inbox,
      snoozes, stars, checks and bulk errors put right; folders matched by their shown
      name and subscribed. Replies from the alias written to, drafts with Bcc and
      alias, Reply-To-safe recipient lists, lighter scheduled sends.
- [x] **S41.20** The low findings of the same audit, fixed (4.7.1). Quote lines in
      English and local time, attachments weighed together, Bcc kept in the Sent copy,
      a removed account gone from search, credentials never printed, a folder named
      `dovecot` shown.
