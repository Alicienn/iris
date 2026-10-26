# Iris — Sync between devices

**Date:** 2026-10-26
**Status:** proposed, not implemented
**Scope of this document:** how two or more copies of Iris (a PC, an Android phone, a
second PC) keep the same tasks, goals, habits, calendars, tags, rules, settings and
notes, with no server of ours. The screens are drawn in `..\Iris-design\android\sync.html`.

---

## 1. Intent

- **No server.** Nothing is hosted, paid for or run by the project. Nothing of the user's
  goes anywhere but their own devices and their own mail servers.
- **Same Wi-Fi.** Two devices sync when they are on the same local network. Away from it
  they keep working alone and catch up the next time they meet.
- **No real time promised.** A change on one device reaches the other at the next sync:
  on its own every few minutes while both run on the same network, or with one touch.
- **One gesture to start.** A QR code pairs two devices once. A new phone can be set up
  entirely from the computer, accounts and passwords included.
- **Merge, never overwrite.** Each element is merged on its own; the newest change wins.
  Nothing is ever asked, and nothing is duplicated.

### Not in scope

- Syncing over the internet, a relay, or a cloud folder. The merge described here does not
  depend on the transport, so one can be added later without changing it.
- Real-time collaboration between people.
- The mail itself: it lives on the IMAP servers, which every device already reads.

## 2. Three channels

| What | How it travels | When |
|---|---|---|
| Stars | the `\Flagged` IMAP flag (already the case) | continuously |
| Queue state: To do, Waiting, Done | IMAP keywords on the messages (§3) | continuously |
| Snoozes, tasks, goals, habits, compass, own calendars, what is attached to events, account tags, rules, unsubscribed senders, shared settings | device to device, merged record by record (§4–§6) | at each sync |
| Notes | device to device, file by file (§7) | at each sync, if chosen |
| Accounts and passwords | device to device (§8) | at pairing, then at each sync |

Calendars that come from a CalDAV server, and subscribed ones, already sync through their
server: only what Iris adds to them locally (notes, colours, video links, invitation
replies) travels device to device.

## 3. The queue state through IMAP

Today the state lives only in `threads.state` (`iris-store/src/threads.rs`), on ids that
differ on each device, and Iris writes no keyword of its own
(`iris-imap/src/client.rs:776-797` knows only the system flags and `$NotJunk`).

- **Keywords.** `$IrisWaiting` and `$IrisDone` on the newest message of the conversation;
  To do is neither. They go through the existing `op_journal`, like `\Flagged`.
- **Reading.** A message whose keywords say Done or Waiting gives its thread that state.
  A new message without the keyword brings the thread back to To do, as today.
- **Servers that refuse keywords.** `PERMANENTFLAGS` without `\*` (or without the two
  keywords) marks the mailbox; its states then travel in the device sync, keyed by
  `messages.rfc_message_id`, the only key two devices share for a message.
- **Snoozes** need a date, which a keyword cannot hold: they always travel in the device
  sync, keyed by the `rfc_message_id` of the thread's newest message.
- The keywords are readable by other clients but mean nothing to them. That is accepted,
  and said in the README's *Privacy*.

## 4. The data model

The facts this rests on (October 2026, migration 26): every synced table has an
`INTEGER PRIMARY KEY` that is a local rowid; only `tasks`, `calendar_events` and
`event_notes` have an `updated_at`; deletes are hard `DELETE`s; all SQL is in `iris-store`,
one module per kind. Rules already have a text id.

Migration 27 adds, to every synced table:

- `sync_id TEXT NOT NULL UNIQUE`: 128 random bits from `getrandom`, as hex. Backfilled for
  existing rows. Event uids made by Iris (`{millis}-{pid}@iris`) are kept as they are;
  rules keep their text id.
- `sync_at TEXT NOT NULL`: the clock of the last change (§5).

And two tables:

- `sync_tombstones(tbl TEXT, sync_id TEXT, sync_at TEXT, PRIMARY KEY (tbl, sync_id))`:
  what was deleted, so that a deleted row does not come back from the other device.
  Tombstones older than every paired device's last sync are pruned.
- `sync_peers(device_id TEXT PRIMARY KEY, name TEXT, secret_ref TEXT, last_seen_at TEXT,
  last_clock TEXT)`: the paired devices and how far each has been merged.

**The 80 write sites are not changed.** SQLite triggers, `AFTER INSERT / UPDATE / DELETE`
on each synced table, set `sync_at` and write tombstones. They call `iris_clock()`, a
function registered on the connection by `iris-store` (rusqlite `create_scalar_function`),
so the clock is the application's, not SQL's. A write that comes from a merge sets a
connection flag that tells the triggers to keep the incoming clock.

**References between rows** stay integer foreign keys locally. In the payload they are
written as the `sync_id` of the row they point to (a task's `list_id`, `parent_id` and
`goal_id`; a check's habit; an event's calendar) and turned back into local ids on arrival.
Rows whose parent has not arrived yet wait in the same transaction until it does.

**Settings** are split. Shared: first name, Home's widgets, undo-send delay, density,
accent colour, the mail options. Device-local, never sent: theme, text size, window
buttons, notification and battery choices, folded tags and folders (they hold local ids).
Shared settings are kept per key with their own clock in a `sync_settings` table; the
TOML file stays the source the app reads.

## 5. The clock

A hybrid logical clock: `wall_ms` (Unix milliseconds), a `counter`, and the `device_id`,
written as one sortable string. Each device takes the maximum of its wall clock and the
greatest clock it has seen, so a phone set five minutes late cannot lose all its changes.
Equal clocks are broken by device id, so both devices always reach the same result.

## 6. The merge

For each record, on its `sync_id`:

| Here | There | Result |
|---|---|---|
| absent | present | inserted |
| present | absent | sent to the other side |
| both present | | the one with the greater `sync_at` |
| present | tombstone | deleted if the tombstone is newer, else kept and sent |

- **By record, not by field**, in this first version: simple, and it matches how Iris
  writes (each save writes the row). A task renamed here and ticked there keeps the newer
  of the two; the sync screen says so once ("1 task changed on both. The newest change was
  kept: …"). Merging by field can come later without changing the format.
- **Things that only add up merge on their own:** habit checks (one record per habit and
  day), goal entries and milestones, area scores. Each is its own record, so two devices
  never fight over them.
- **Seeded rows** (the six compass areas, the *Personal* calendar, the vision row) get
  fixed `sync_id`s in the migration, the same on every device, so they meet rather than
  double.
- **Accounts** are merged on their address (`accounts.email` is unique), not on an id.
- **Account tags** are merged on their name, ignoring case, as the table already enforces.

The merge runs in one transaction per sync. If it fails, nothing changes on that side and
the next sync starts again from the same point.

## 7. Notes

A space is a folder of files, so notes are merged file by file, not row by row:

1. Each side sends a manifest: relative path, size, modification time, BLAKE3 hash
   (`blake3` is already in the workspace).
2. A file present on one side only is copied. A file changed on one side only is copied.
3. A file changed on both: the newer one wins, and the other is kept as a version in the
   note's history (`.iris`, as the version history already does), so nothing is lost.
4. A file deleted on one side and unchanged on the other is deleted; deleted on one side
   and changed on the other, it is kept.

Deletions need a memory of what was there: each space keeps the manifest of its last sync
with each peer in its `.iris` folder. Spaces are matched by a `space_id` written in their
`.iris` folder at pairing.

Notes are optional (they can weigh tens of megabytes) and on by default.

## 8. Accounts and passwords

- Account settings (servers, user name, aliases, signature, tags, colour) merge as records.
- Passwords and OAuth refresh tokens are read from the sender's secret store (`iris-secrets`)
  and written into the receiver's, under the same `account/kind` keys. They travel only
  inside the encrypted channel and never sit in a file.
- Removing an account on one device does **not** remove it on the other: it is the one
  deletion that asks, on the other device, the next time ("Marie removed archives@… on
  PC-Marie. Remove it here too?"). Losing a mailbox by accident costs too much.

## 9. Pairing

1. The computer opens a pairing window (Settings › Devices › Pair a device) and starts
   listening on a random TCP port of the local network.
2. It shows a QR code holding, as a URI:
   `iris-pair:1?h=192.168.1.20&p=51834&d=<device id>&n=PC-Marie&k=<32 random bytes, base64url>`.
   The code is good for five minutes and works once.
3. The phone scans it, connects, and both sides prove they hold `k` (challenge and answer,
   §10). Both then show six digits derived from the session key; the user checks they
   match. This guards against a QR code photographed by someone else on the same network.
4. They exchange a long-term pairing secret of 32 random bytes, kept in each device's secret
   store under `device/<peer id>`, and the peer goes into `sync_peers`.
5. The first sync runs at once, with the choices made on the confirmation screen.

**Setting up a new phone** is the same flow from the first-run screen ("Set up from my
computer"): the phone has nothing yet, so the first sync fills it.

## 10. The channel

- **Discovery.** After pairing, each device announces itself on the local network with mDNS
  (`_iris-sync._tcp`, its device id only, never the user's name or address). A pure-Rust
  crate (`mdns-sd`) is enough. On Android this needs a multicast lock while Iris is open.
- **Security.** TCP, then each side sends 32 random bytes; the session key is
  `BLAKE3-keyed(secret, "iris-sync 1" ‖ nonce_a ‖ nonce_b)`. Every frame is sealed with
  XChaCha20Poly1305 (already in `iris-secrets`) and a counter in its nonce. A frame that
  does not open ends the connection. An unpaired device gets nothing but a closed socket.
- **Exchange.** Each side sends the last clock it merged from the other (`last_clock`);
  each answers with its records and tombstones changed since, by table, as JSON compressed
  with zstd (both already in the workspace). Then the note manifests, then the files.
- **Listening.** Only while Iris is open and on a local network: on Android, on Wi-Fi only.
  Windows shows its firewall question the first time; Settings › Devices says why before it
  appears.

## 11. When it syncs

- **On its own** every ten minutes while both devices run Iris on the same network, and when
  one of them comes back to it. Settings › Devices switches this off.
- **By hand**: *Sync now* in Settings › Devices, and on Home when the other device is on
  the network and the last sync is more than a day old.
- The time of the last sync with each device is always shown.

## 12. Where it goes in the code

- `iris-store`: migration 27, triggers, `iris_clock()`, reading changes since a clock,
  applying a batch in one transaction. Tested with two stores in memory.
- A new crate **`iris-peer`**: the clock, the payload format, the merge, the pairing URI,
  the handshake and framing, the note manifests. No knowledge of the interface; no
  `unsafe`; tested on its own (two peers over a local socket).
- `iris-app`: Settings › Devices, the pairing window, the sync summary, the Home nudge; the
  camera and QR reading go through the platform trait (on Android: CameraX with a free QR
  decoder such as ZXing; on Windows, pairing is always started from the computer, which
  shows the code and needs no camera).
- `iris-imap` / `iris-sync`: the two keywords (§3).

## 13. Order of work

1. Queue state as IMAP keywords: useful at once, even between two PCs.
2. Migration 27, triggers and clock; `iris-peer` with the merge, tested on two stores.
3. Pairing and the channel between two PCs on one network (no Android needed to test).
4. Notes.
5. The Android screens, once the port has its shell.

Each step goes to users on its own, with its changelog line.

## 14. Open questions

- Merge by field for tasks, if record-level merges surprise people in practice.
- Drafts and scheduled sends: sent with the rest, or kept on the device that wrote them?
  Proposed: scheduled sends sync (so that one device sends them, the one that scheduled,
  and the other shows them), drafts do not.
- A later transport for devices that never share a network (a folder synced by
  Syncthing or a cloud, or a dedicated IMAP folder), carrying the same payload.
