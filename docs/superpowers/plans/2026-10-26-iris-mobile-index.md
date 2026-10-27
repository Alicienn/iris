# Iris on PC and Android, with sync — the plans and their order

Four plans carry the two specs. Each plan's phases are shippable on their own, each with its
changelog line and version; nothing waits for the whole.

| Plan | Spec | What it delivers |
|---|---|---|
| [Adaptive interface](2026-10-26-iris-adaptive-ui.md) | [android](../specs/2026-10-26-iris-android-design.md) §1–§7 | `Layout` (width classes, touch, insets), the touch scale, Modal / Popover / menus as pages and sheets, the tab bar and rail, stacked pages with Back, swipe and long press, `apercu_mobile`. All testable on Windows in a narrow window. |
| [Screens](2026-10-26-iris-android-screens.md) | android §2–§6, the mockups | Every place in compact and medium: Home and search, Mail, Calendar, Tasks, goals and habits, Notes, Settings and first run. |
| [Android platform](2026-10-26-iris-android-platform.md) | android §8–§9 | The `Platform` trait over today's Windows code, `iris-android`, the Gradle project, the services over JNI, background mail, notifications, intents, widgets, the APK in CI and in releases. |
| [Sync](2026-10-26-iris-sync.md) | [sync](../specs/2026-10-26-iris-sync-design.md) | The queue state as IMAP keywords, the merge in `iris-store`, `iris-peer` (pairing, channel, discovery), notes, Settings › Devices. |

## Order

```
Adaptive A ─► Adaptive B ─► Screens A … F ─────────────┐
     │                                                  ├─► Android release (Platform F)
Platform A ─► Platform B ─► Platform C ─► D ─► E ───────┘
Sync A (keywords) ─► Sync B (merge) ─► Sync C (pairing) ─► Sync D (notes) ─► Sync E (Android screens, after Screens F)
```

- **Adaptive A** (the `Layout` global and the touch scale) comes first: every other
  interface task reads it.
- **Platform A** (the trait over Windows' code, `run_gui` moved into the library) changes
  nothing users see; it can run in parallel with Adaptive A, on other files.
- **Sync A** touches only `iris-imap`, `iris-sync`, `iris-workflow` and `iris-store`; it can
  run at any time, and helps two PCs before any phone exists.
- The **first APK** (Platform F) needs Screens A–F and Platform B–E. Until then, the Android
  build is a CI artifact for testing, not a release.

## Releases (proposed, the numbers follow the rule in `CLAUDE.md`)

| Version | Contents |
|---|---|
| 6.2.0 | Sync A: To do, Waiting and Done follow conversations to Iris on another computer. |
| 6.3.0 | Adaptive A–C + Screens A–B: Iris fits a narrow window; Home and Mail in one column. |
| 6.4.0 | Screens C–F: Calendar, Tasks, Notes, Settings in one column. |
| 6.5.0 | Sync B–C + the desktop part of Sync E: pair two computers on the same Wi-Fi. |
| 6.6.0 | Platform F: Iris for Android (a new feature, nothing broken on Windows: MINOR). |
| 6.7.0 | Sync D and the Android part of Sync E: notes sync, pairing from the phone. |

## Rules every plan inherits

- **Never run tests, clippy or builds locally.** `cargo fmt --all`, commit, push, read CI
  (`gh run list -R Alicienn/iris`, `gh run view <id> --log-failed`). "Run the test" in a plan
  means exactly that. The only local runs allowed are the `apercu_*` captures, and only when
  a step says so.
- CI runs clippy with `-D warnings`: alias complex types, ≤ 7 arguments, no immediately
  invoked closures.
- Every user-facing change: `CHANGELOG.md` (English, one short line, New / Improved / Fixed /
  Removed) and the version in `[workspace.package]` plus the `iris-*` entries of
  `Cargo.lock`. The documents of the table in `CLAUDE.md` in the same commit (README,
  ARCHITECTURE, PLAN, COMPARISON, PLUGINS, packaging/README, the spec's *What changed since*).
  Unreleased work is never described as shipped.
- English only in the repository; test data uses `example.com`.
- Interface: shared components only; every modal, panel, menu **and sheet** joins
  `modal-open`; text fields from `controls/field.slint`; closing hands the keyboard back to
  `shortcuts.focus()`; a new interaction bug gets a scenario in `crates/iris-ui/tests/saisie.rs`
  first, driven by real pointer and key events.
- Stage files by path (`git add <path>`), never `-A` or `-u`: other sessions share the repo.
- Screens are compared with the mockups in `..\Iris-design\android\` (and their captures in
  `shots\`) before they are shown to anyone; captures of the port go to the same folder,
  never into the repository.
