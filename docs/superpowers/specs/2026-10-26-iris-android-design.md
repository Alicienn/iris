# Iris — One interface for PC and Android

**Date:** 2026-10-26
**Status:** proposed, not implemented
**Scope of this document:** how the existing Slint interface adapts to any window width
and to touch, and how Iris builds, runs and fits in on Android. iOS is not planned; the
choices that make it easier later are noted. The screens are drawn, light and dark, in
`..\Iris-design\android\` (open `index.html`; one page per area, captures in `shots\`).
Sync between devices is its own spec: `2026-10-26-iris-sync-design.md`.

---

## 1. Intent

- **One application.** The same crates, the same `.slint` screens, the same components
  and tokens run on Windows and on Android. There is no second interface to keep in step.
- **The width decides the layout, the input decides the sizes.** Two independent axes:
  - *Width class*, from the window's width only: **compact** below 600 px (a phone, or a
    narrow PC window), **medium** from 600 to 840 px (an open foldable, a tablet in
    portrait), **expanded** from 840 px (a tablet in landscape, every PC window today).
  - *Touch*: on when the device's main input is a finger (Android). It raises targets to
    48 px, rows and type to the touch scale (§4), and turns hover affordances into
    always-visible controls or gestures.
- **The desktop does not change** at expanded width with a mouse, except where a fix
  benefits both.

## 2. Navigation

- **The five places** (Home, Mail, Calendar, Tasks, Notes) are a tab bar at the bottom in
  compact, a navigation rail at the left in medium, and the side column's place switcher in
  expanded, as today. Badges are the same (`Chrome.mail-badge`, `Chrome.tasks-badge`).
- **Pages stack** in compact: a list, then what is opened from it (a conversation, a task,
  an event, a habit, a goal, a note), then what is opened from that (the composer, an
  editor). Each push is a `Place` in the history (`nav.rs`), so **Back pops a page**
  before it goes back to the previous place.
- **Android's Back** gesture is `nav-back`. At the start of the history it leaves Iris, as
  every Android app does. A back arrow is always drawn at the top left of a pushed page:
  iOS has no system Back, and that arrow is what will make the port possible.
- **Settings** is a page (pushed) in compact and medium, the full-width page of today in
  expanded. **Search and commands** (the palette) is a page in compact. Both are reached
  from Home's header and from each place's magnifier.

## 3. Windows over the others

| Component | Expanded | Compact |
|---|---|---|
| `Modal` used as an editor or a form (composer, event editor, new habit, account…) | centred card | full page with a top bar (× or ←, title, main action) |
| `Modal` used as a short question (delete, empty a folder) | centred card | bottom sheet |
| `Popover` (event detail, slot picker, Later) | card beside its anchor | bottom sheet, half height, can be pulled up |
| `MenuSurface` (context menus, sort, snooze) | at the pointer | bottom sheet, except menus that open from a button in a top bar (the reader's ⋯), which stay anchored |

The component decides from `Layout.width-class`; screens only say which kind they are.
Every sheet has a handle, closes with a tap on the scrim, a pull down, Escape or Back, and
stays in `modal-open`. The keyboard rules of `CLAUDE.md` hold unchanged: a window opened for
typing puts the cursor in its first field (and so raises the on-screen keyboard), and
closing one hands the keyboard back to `shortcuts.focus()`.

## 4. Touch scale

On touch, `Type.size` and the row tokens use a second table, multiplied by the user's text
size (*Settings › Appearance › Text size*, 85 % to 130 %, following Android's font scale by
default):

| Style | Desktop | Touch |
|---|---|---|
| display | 56 | 34 |
| title | 34 | 32 |
| heading | 22 | 22 |
| body-large | 15 | 16 |
| body, section | 13 | 15, 14 |
| caption | 12 | 13 |
| micro | 11 | 11.5 |

Mail rows are 76 px in compact (three lines), list rows and nav items at least 48 px,
checks 26 px inside a 48 px target, buttons 44 px, the tab bar 66 px.

## 5. Gestures, and their visible twin

- **Swipe a row**: a conversation right for Done, left for Snooze; a task right to tick,
  left for Later. The ground takes the action's colour past the threshold (96 px); an Undo
  follows. The two mail actions can be changed in *Settings › Mail*.
- **Long press** (500 ms) opens what right-click opens today, as a sheet.
- **Drag** (move an event, carry a task to a day, reorder tags, move a note) starts after a
  long press, so a scroll never moves anything by mistake.
- Every gesture has a visible twin in a menu or a button, for accessibility and because
  gestures differ from one system to the next.

## 6. The on-screen keyboard

The window is resized by the keyboard (`adjustResize`), so what sits at the bottom of a page
(the quick-add field, the reply field, the notes' format bar, the address suggestions) sits
on the keyboard. Shortcuts stay for a physical keyboard.

## 7. Safe areas

`Layout.inset-top` and `Layout.inset-bottom` come from the platform (Android's window
insets: status bar, gesture bar, cut-out). Pages pad by them; the tab bar and docks extend
under the gesture bar with their own background. iOS will fill the same two values.

## 8. Fonts

Segoe UI Variable cannot be distributed on Android. On Android, `Tokens.font-family` and
`Tokens.font-display` are the system's sans-serif (Roboto, or the maker's). The theme files
keep Segoe for Windows; the platform overrides the two names when it applies the theme. The
mockups use Segoe only to be compared with the desktop.

## 9. Android specifics

- **Build.** A new crate `iris-android` (a `cdylib`) holds `android_main` and the Android
  side of the platform trait; Slint runs on its `android-activity` backend (NativeActivity).
  An `android/` Gradle project (Kotlin) packages it, with the pieces that need the Java API:
  notifications and their actions, background work, the camera, widgets, share and open
  intents, the keystore.
- **Platform trait.** Everything that differs between systems goes behind
  `iris_app::platform::Platform`: open a link or a file, pick a file or a folder, the
  clipboard's images, notifications, the secrets' key, DNS servers, system dark mode and
  number style, insets, background scheduling, QR scanning, sharing. Windows keeps today's
  code behind it; Android implements it over JNI; iOS will implement it in Swift.
- **Secrets.** Android has no `keyring` backend. Iris uses its `EncryptedVault`, whose key
  is random and kept wrapped by the Android Keystore: no master password to type.
- **Sign-in with Google or Microsoft** keeps the desktop flow: a loopback redirect on
  `127.0.0.1`, the page opened in a Custom Tab. Both providers accept loopback for the
  client types Iris already asks users to create.
- **TLS.** `rustls-platform-verifier` needs its Android component and an init with the
  JNI context at start.
- **DNS** for account discovery uses the servers Android reports (`ConnectivityManager`),
  not `/etc/resolv.conf`.
- **Plugins.** wasmtime with Cranelift works on Android (JIT is allowed). iOS will need its
  interpreter, Pulley; nothing else changes.
- **Mail in the background** (no push server): IMAP IDLE while Iris is visible; WorkManager
  every 15 minutes otherwise, running the headless sync the CLI already has (`iris sync`);
  *Instant* keeps a foreground service with its required quiet notification; Android 15
  allows such a data-sync service about six hours a day, after which Iris falls back to the
  15-minute work and *Settings › Notifications & sync* says so. Scheduled sends and
  reminders wake Iris with exact alarms.
- **Notifications** carry actions: Done, Snooze, Reply (inline) for mail; Join and Snooze
  for events; Done and Later for tasks.
- **Widgets** (Today, To do count, Habits) are native `RemoteViews`, fed by a small JSON file
  Iris writes when what they show changes; they are the only part not drawn by Slint.
- **Share to Iris** (a task, a note, a message, an event) and **Open with Iris** (`.ics`,
  `.eml`, `mailto:`, `.mobileconfig`, `.md`, `.sheet`) are intents.
- **Notes** live in Iris's own folder on the device (app-specific storage: a real path, which
  `iris-vault` needs and a Storage Access Framework tree is not). They travel between
  devices with the device sync (sync spec §7). Opening any folder of the phone as a space is
  left for later.
- **Updates.** From a store, the store updates. The GitHub release also carries a signed
  `iris-X.Y.Z.apk`, listed in the same signed `latest.json`; Iris checks it and opens the
  download. Same version number and changelog as Windows.
- **Not on Android:** the tray, start at login, window buttons, single-instance mutex,
  `Zone.Identifier` marks, the Recycle Bin (notes go to a `.trash` folder of the space).

## 10. What stays desktop only, for now

Notes' tabs become an *Open notes* stack in compact; *Open beside* and the notes' side panel
exist from medium width. Drag and drop to and from other apps is not supported on Android.
