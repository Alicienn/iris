# Iris — Design specification

**Date:** 2026-09-13
**Status:** approved
**Scope of this document:** the complete target architecture, and the precise content of
the first implementation batch.

> **This is the original record, kept as it was decided.** Several decisions have since
> been revised by measurement or by the product growing. See
> [What changed since](#what-changed-since) at the end, and
> [ARCHITECTURE.md](../../ARCHITECTURE.md) for the current state.

---

## 1. Intent

Iris is a native desktop mail client designed for use at scale: up to **~100 mailboxes
and ~1 million messages** on one machine, without the interface ever ceasing to be
immediate.

Three stances found the product:

1. **The organising axis is work, not filing.** A conversation is *to do*, *waiting* or
   *done*. No thematic filing is imposed.
2. **Performance is a design constraint, not an optimisation.** It dictates the
   architecture (see §4), not the other way round.
3. **Everything is a module.** The kernel does not know what a mail is. Themes, rules,
   views and protocols are replaceable modules, and a third-party plugin sees the same
   API as the internal modules.

### Non-goals (v1)

Explicitly out of scope, to prevent drift:

- No AI features.
- No calendar, contacts or tasks.
- No PGP/S-MIME encryption.
- No full WYSIWYG editor.
- No online plugin store.
- No proprietary multi-device sync.

---

## 2. Technical decisions

| Area | Decision | Reason |
|---|---|---|
| Language | Rust | One language from the foundation to the UI. |
| UI | **Slint** (GPU backend) | Official wgpu interop, low RAM, design hot reload, 100 % Rust. |
| HTML body rendering | **Blitz** (Rust HTML/CSS engine, Vello/wgpu) | No webview, rendering sharing Slint's GPU device. |
| Metadata | **SQLite** in WAL mode | Relational queries, transactions, proven reliability. |
| Search | **Tantivy** | Relevance and speed out of FTS5's reach at 1 M documents. |
| Bodies and attachments | **zstd** files, bounded LRU cache | Keeps large objects out of the database, bounds disk space. |
| v1 protocols | IMAP + SMTP, Google/Microsoft OAuth2 | Covers every case, a single implementation to optimise. |
| Extensibility | Compiled crates (core) + **WASM plugins** on wasmtime | Zero overhead on hot paths, sandbox for third parties. |
| Secrets | OS keychain, encrypted file fallback | No secret in clear on disk. |
| v1 platform | Windows, then macOS | Immediate development environment. |

### Accepted risks

- **Blitz is young.** Its CSS support is partial. Mitigation: bodies are sanitised and
  normalised before rendering; integration goes through an `HtmlRenderer` trait, so the
  engine can be replaced without touching the rest.
- **The Slint × Blitz integration** relies on sharing a wgpu texture. It is validated by
  a dedicated prototype before any commitment (see §10, batch 0).
- **Slint provides almost no widgets.** The shell, the virtualised list and the palette
  must be built. The plan accounts for it.

---

## 3. Architecture

Five layers, plus a cross-cutting kernel. No diagonal dependency: a layer only knows the
contract of the one before it.

```
                         ┌──────────────────────────────────┐
  Presentation           │ iris-ui · iris-htmlview · theme  │  never any I/O
                         ├──────────────────────────────────┤
  View model             │ iris-viewmodel                   │  testable without UI
                         ├──────────────────────────────────┤
  Domain                 │ workflow · thread · rules ·      │  pure, no I/O
                         │ search · mime                    │
                         ├──────────────────────────────────┤
  Data                   │ store (SQLite) · index (Tantivy) │  local truth
                         │ blobs (zstd + LRU)               │
                         ├──────────────────────────────────┤
  Network                │ sync · imap · smtp · discover ·  │  tokio
                         │ secrets                          │
                         └──────────────────────────────────┘
  Cross-cutting: iris-kernel (registry, bus, capabilities) · iris-types (contract)
                 iris-plugin-host (wasmtime) · iris-plugin-api (WIT)
```

### Crates

| Crate | Responsibility | Depends on |
|---|---|---|
| `iris-types` | Identifiers, states, errors, shared structures | — |
| `iris-kernel` | Module registry, lifecycle, event bus, capabilities | types |
| `iris-store` | SQLite schema, migrations, cursor queries, operation journal | types |
| `iris-index` | Tantivy index, incremental writes, queries | types |
| `iris-blobs` | Compressed storage of bodies and attachments, LRU cache | types |
| `iris-mime` | MIME parsing, HTML sanitising, tracker detection | types |
| `iris-thread` | Threading (JWZ), cross-account option | types |
| `iris-workflow` | State machine, snooze, follow-ups, automations | types, store |
| `iris-rules` | Rules engine, dry run | types, store |
| `iris-search` | Query language, store + index planning | types, store, index |
| `iris-discover` | ISPDB, autoconfig, SRV, MX, port probing | types |
| `iris-secrets` | OS keychain, encrypted fallback | types |
| `iris-imap` / `iris-smtp` | Protocols, connection pool | types, secrets |
| `iris-sync` | Scheduler, IDLE/polling, reconciliation, replay | everything above |
| `iris-viewmodel` | Observable state, selectors, row window | domain, store |
| `iris-theme` | Token loading, hot reload | types |
| `iris-htmlview` | Blitz → wgpu texture adapter | types, mime |
| `iris-ui` | Slint shell, components, palette | viewmodel, theme, htmlview |
| `iris-plugin-api` | Versioned WIT contract | — |
| `iris-plugin-host` | wasmtime, permissions, quotas | kernel, plugin-api |
| `iris-app` | Assembly, configuration, entry point | everything |

### The kernel

`iris-kernel` does not know what a mail is. It exposes:

- a **module registry**: each module declares a name, a version, the capabilities it
  requires and a lifecycle (`init`, `start`, `stop`);
- a **typed event bus**, asynchronous, with multi-subscriber broadcast and time-based
  coalescing;
- **capabilities** handed out explicitly: a module can only reach what it declared it
  wants. This is what makes the plugin permission model identical to that of internal
  modules.

---

## 4. The four performance invariants

They take precedence over every other consideration and are checked by tests.

1. **Zero I/O on the UI thread.** No SQL query, no MIME parsing, no network call during a
   frame. The UI only consumes pre-computed diffs.
2. **Nothing is loaded whole.** The list only holds a window of rows around what is
   visible, paginated **by cursor** (keyset) and never by `OFFSET`. Memory follows what
   is shown, not what is stored.
3. **Every local action is instant, then reconciled.** Immediate local write, entry in
   the idempotent operation journal, replay to the server. Never a network wait in front
   of the user.
4. **Events are batched.** Diffs are coalesced in 16 ms windows: syncing 100 accounts
   does not wake the interface a thousand times a second.

### Budgets

| Metric | Target |
|---|---|
| Memory, 100 accounts / 1 M messages, idle | ≤ 400 MB |
| Memory at cold start | ≤ 150 MB |
| Time to the first list shown | ≤ 400 ms |
| UI frame budget | ≤ 1.5 ms CPU |
| Scrolling | a steady 120 fps, no dropped frame |
| Search over 1 M messages | ≤ 80 ms to the first results |
| Idle CPU, 100 accounts synced | ≤ 1 % |

---

## 5. Data model

Main entities: `account`, `folder`, `message`, `thread`, `thread_state`, `blob_ref`,
`op_journal`, `rule`, `snooze`, `contact_seen`.

Structuring points:

- **The workflow state belongs to the thread**, not to the message. One does not deal
  with an isolated message, one deals with an exchange.
- `contact_seen` remembers the correspondents the user has already replied to. It is the
  system's only "memory", and it serves the rules.
- `op_journal` holds every local action not yet confirmed by the server, with an
  idempotency key.
- Indexes are designed for cursor pagination: `(state, last_activity_at DESC,
  thread_id)` is the leading index of the main list.

### State machine

```
                 reply sent
    To do ────────────────────▶ Waiting
        ▲  │                        │
        │  │ manual action          │ delay passed without an answer
        │  ▼                        │
        │ Done ◀────────────────────┘  (follow-up)
        │    │
        └────┘ new message received
```

`Snoozed` is **orthogonal**: a snoozed thread keeps its state and reappears when due.
Each automatic transition can be switched off individually in the settings.

---

## 6. Synchronisation

- **Priority scheduling**: active account > pinned > recently active > the rest.
- **Bounded pool** of about 16 live connections, assigned in IDLE to priority accounts,
  with rotation. Respects provider quotas.
- **Adaptive polling** for the others: the interval follows the account's real activity,
  from 1 to 60 minutes.
- **Incremental sync** through CONDSTORE/QRESYNC when the server announces them; fallback
  on `UIDVALIDITY` + UID ranges.
- **Cache**: all headers are synced; bodies are downloaded on open, then kept compressed
  under a bounded LRU; attachments are never downloaded automatically.
- **Offline**: the operation journal is replayed on reconnection, idempotently, with
  deterministic conflict resolution (the server wins on flags, local wins on the
  workflow state, which is its own).

---

## 7. Interface and design system

**Structure**: three columns. Sidebar (accounts: unified, pinned, groups, account
search) · conversation list topped by the state tabs (To do / Waiting / Done) ·
conversation with a reply area built in at the bottom of the thread.

**Default visual direction**: deep monochrome glass. Neutral dark background, genuinely
translucent panels, 1 px luminous edges, **no accent colour**: hierarchy relies on
luminosity and transparency. A fine grain is applied to the glass to remove gradient
banding. Functional signals (error, urgency) go through shape, weight and icon rather
than hue, except for the error red.

**Shipped themes**: `mono` (default), `ice` (desaturated cold accent), `sand`
(desaturated warm accent).

**Design tokens**: colours, radii, spacing, typography, density, animation durations and
glass parameters (blur, opacity, saturation, grain) live in TOML files, hot-reloaded
without a restart. A theme is a file, not code.

**Icons**: a single, careful set, shipped with the application. Not replaceable in v1.
**Fonts**: freely chosen among the system's or by file, with weight, size, tracking and
density settings.

**Glass has a cost.** It is applied to fixed panels (sidebar, list) and to floating
surfaces (palette, popovers, menus). It is never applied to an element that moves during
a scroll.

---

## 8. Extensibility

The core is made of compiled crates, for performance. On top, `iris-plugin-host` runs
WebAssembly plugins through wasmtime, described by a versioned WIT contract.

Capabilities offered to plugins in v1:

1. **Subscribe to events and act on mail** — label, change state, move, notify.
2. **Add commands to the palette** and keyboard shortcuts.
3. **Provide views and panels** rendered by the application from a declarative
   description (no direct GPU access).
4. **Access the network and a storage space of their own**, under explicit permission
   granted by the user at install time.

Each plugin is limited in CPU time and memory. A failing plugin is disabled, never the
application.

**Updates**: the core is updated as one block, signed and verified, by deltas. Themes and
plugins install, update and disable live, independently of the application's version.

---

## 9. Security and privacy

- Remote images blocked by default, loaded through a local proxy that masks the IP
  address and strips identifying headers.
- Tracking pixels detected and reported; the sender is marked as a tracker in the
  interface.
- HTML is sanitised before rendering: no script, no form, no unauthorised external
  resource.
- One-click unsubscribe conforming to RFC 8058, preferred over the HTTP link when the
  `List-Unsubscribe-Post` header is present.
- No secret in clear: OS keychain, fallback on a file encrypted by an Argon2-derived key.
- No telemetry.

---

## 10. First batch: kernel plus a vertical slice

**Goal**: the complete architecture is built *and proven end to end*, not only drawn.

Content:

- The complete kernel: module registry, event bus, capabilities, configuration.
- The data layers (SQLite, Tantivy, blobs) with their migrations and tests.
- The domain: MIME parsing, threading, workflow state machine.
- The network: automatic discovery, secrets, IMAP reading, SMTP sending.
- The sync scheduler with a bounded pool and the operation journal.
- The view model and its cursor-paginated row window.
- The theme engine with the three presets and hot reload.
- The plugin host with the WIT contract and an example plugin.
- The Slint interface: three-column shell, virtualised list, command palette, inline
  reply, sending with a 10 s undo.
- Blitz integration for rendering message bodies.

**Batch 0, prerequisite**: three throwaway prototypes, each lifting one risk — (a) a
Blitz texture composed into a Slint scene, (b) a list of 1 M entries scrolling at
120 fps, (c) 50 simultaneous IMAP connections under a bounded pool.

**Done when**: adding a real account with only an address and a password, seeing its
messages arrive, reading a correctly rendered thread, replying to it, and seeing the
conversation move to *Waiting* — all within the performance budgets of §4.

---

## What changed since

Decisions of this document that the shipped application no longer follows, and why.
Details in [ARCHITECTURE.md](../../ARCHITECTURE.md).

- **Calendar and tasks are in** (0.3.0 and 0.5.0), against the v1 non-goal. They came
  from use, not from the plan, and each has a pure domain crate (`iris-calendar`,
  `iris-tasks`) on the same model as the mail's.
- **No GPU for the interface.** Slint's software renderer replaced the GPU backend:
  38.5 MB against 209.7 MB for the same window, the difference being what the graphics
  driver reserves.
- **No shared wgpu texture for Blitz.** Message bodies are laid out once and painted on
  the CPU, tile by tile, at the width shown. The texture sharing of §2 and prototype (a)
  of §10 were abandoned: a GPU device of our own was the largest memory cost.
- **The plugin crates are `iris-plugins`** (host, WIT contract in `wit/iris.wit`) and
  **`iris-plugin-sdk`** (guest side), not `iris-plugin-host` / `iris-plugin-api`.
  Plugins have no network access at all (§8, capability 4): a plugin that needs a new
  host capability is a feature request, not a plugin.
- **Updates** come as a full installer from GitHub releases, checked by size and SHA-256,
  not as signed deltas (§8).
- **Undo send** is 5 seconds by default and a setting (0 to 30), not a fixed 10 seconds.
- **Remote images** are fetched directly over HTTPS for a message the reader has
  unblocked, not through a local proxy (§9).
- **Calendar, tasks and Home read the database on the display thread**, a stated
  departure from invariant 1 of §4.
- **Iris opens on Home, not on the queue** (1.0.0): the day at a glance, mail, tasks and
  events together, one click from each. The queue stays the heart of the mail; the
  start page can be switched off.
- **The interface is one set of shared components, in layers** (2.0.0), and every screen
  is drawn first as a mockup restricted to what the software renderer can draw. The
  folders share the accounts' column, and the queues are the list's title rather than
  tabs above it.
- **Themes are no longer modules** (3.0.0). Two themes ship, light and dark, compiled
  in; the appearance follows Windows by default. User theme files and the glass
  material are gone: one look, designed as a whole, in two lights.
- **Workspaces are chosen from a rail on the left** (3.0.0), not from tabs in the title
  bar; the Iris mark at its top leads Home.
- **The queues are pills above the list again** (3.1.0), no longer its title, as the
  web edition that became the reference design has them.
- **Tasks have goals** (3.3.0): something to reach by a day, with its pace, beyond the
  spec's lists of things to do. They are counted by hand only.
- **Apple's apps are the model** (4.0.0): the rail gave way to a side column per place
  with a switcher at its top, the queues moved into a toolbar shared by the list and the
  reader, and Home became widgets. The window's buttons are the Mac's lights or
  Windows', as the user chooses.
- **The journal's idempotency key holds only while its operation waits** (4.5.3): the
  same intention twice in a row is one operation, but once acknowledged, or once
  another intention came after it, the same key is a new step. Kept for ever, it made
  "read, unread, read" leave the server unread. Each account replays its own queue.
- **Undo reaches the server** (4.6.0): an undone archive or delete withdraws its moves
  from the journal, or moves the messages back by their `Message-ID` once the server
  has carried them out. Automatic transitions (a reply arriving, a snooze or a
  follow-up due) are not undoable: undo is for what the user did.
