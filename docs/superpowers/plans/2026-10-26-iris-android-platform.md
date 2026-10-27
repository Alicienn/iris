# Android Platform Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Iris builds and runs on Android from the same crates, with every system service
behind one `Platform` trait, background mail, notifications with actions, intents, widgets,
and a signed APK published with each release.

**Architecture:** `run_gui` moves from the binary into the library (`iris_app::gui::run`)
so the Windows binary and a new `cdylib` crate `iris-android` start the same interface.
Everything that differs per system is a method of `iris_app::platform::Platform`, installed
once at start: Windows keeps today's code, Android implements it over JNI against a small
Kotlin layer in an `android/` Gradle project (NativeActivity, WorkManager, notifications,
CameraX, widgets). Headless entry points (`iris_app::headless`) let background work run
without the window.

**Tech Stack:** Rust 2021, Slint 1.17 `backend-android-activity-06` (NativeActivity),
`jni` 0.21, `ndk-context`, `android_logger`, `cargo-ndk`; Kotlin, Gradle (AGP 8), AndroidX
WorkManager, Core (NotificationCompat), Browser (Custom Tabs), CameraX, ZXing core.

**Spec:** `docs/superpowers/specs/2026-10-26-iris-android-design.md` (§8–§9).

## Global Constraints

- The rules of `docs/superpowers/plans/2026-10-26-iris-mobile-index.md`.
- Android: `minSdk 26`, `targetSdk 35`, ABIs `arm64-v8a` and `x86_64`; package `fr.iris.mail`
  (matches `ProjectDirs::from("fr","Iris","Iris")`); `windowSoftInputMode="adjustResize"`.
- Permissions, and only these: `INTERNET`, `ACCESS_NETWORK_STATE`, `ACCESS_WIFI_STATE`,
  `CHANGE_WIFI_MULTICAST_STATE`, `POST_NOTIFICATIONS`, `CAMERA`, `FOREGROUND_SERVICE`,
  `FOREGROUND_SERVICE_DATA_SYNC`, `SCHEDULE_EXACT_ALARM`, `RECEIVE_BOOT_COMPLETED`.
- No Google Play services; QR decoding with ZXing core (Apache-2.0).
- The Windows build behaves exactly as today at the end of Phase A: every CI job green
  with no test changed except the call sites moved behind the trait.
- No `unsafe` outside `iris-android`'s JNI entry points; each `extern "system"` function
  catches panics (`std::panic::catch_unwind`) and logs them.
- Background WorkManager period 15 minutes; Instant uses a `dataSync` foreground service and
  falls back to the period after Android's daily limit.

## Review Focus

- The phone killed by Android while a message waits in the undo delay or in `sending.json`:
  the next start (worker or activity) sends it, once (Task D1 tests a pending send picked
  up by `headless::sync_once` and not sent twice when the activity starts right after).
- A notification action (Done, Snooze, Reply) tapped while the activity is running: it goes
  through the running controller, not a second store connection (Task D2 tests both routes
  end in one `set_state`).
- Rotation, fold/unfold, dark mode switched in quick settings: the activity is not
  recreated (`configChanges`), the window resizes, `Layout` and the theme follow (Task B4).
- No network, or a captive Wi-Fi, at the worker's run: it returns `retry` without a
  notification storm (Task D1).
- Android's Back with the on-screen keyboard up: the keyboard closes first (Android's own
  behaviour), then Back reaches `system-back` (Task B4).

---

## Phase A — The trait, on Windows (no visible change)

### Task A1: the interface starts from the library

**Files:**
- Create: `crates/iris-app/src/gui.rs`
- Modify: `crates/iris-app/src/main.rs:621-1239` (the body of `run_gui` moves to `gui.rs`;
  `main.rs` keeps the CLI, `attach_parent_console`, DWM corners and the tray polling, passed
  in as hooks), `crates/iris-app/src/lib.rs` (`pub mod gui;`)

**Interfaces:**
- Produces:
  ```rust
  pub struct GuiOptions {
      pub paths: crate::Paths,
      pub mailto: Option<crate::platform::MailtoRequest>,
      pub start_in_tray: bool,
      pub hooks: GuiHooks,
  }
  pub struct GuiHooks {                       // desktop-only extras; all may be no-ops
      pub after_show: Box<dyn FnOnce(&iris_ui::AppWindow)>,   // DWM corners
      pub tick: Option<Box<dyn FnMut(&iris_ui::AppWindow)>>,  // tray polling, every 200 ms
  }
  pub fn run(options: GuiOptions) -> iris_types::Result<()>;      // returns after the event loop
  ```
  `run` does what `run_gui` did between `memory::track_sites` and the shutdown after
  `run_event_loop_until_quit`, except the renderer selection (the caller's: winit-software
  on desktop, the Android backend there).
- [ ] Step 1: implement the move; no logic change. Test: `crates/iris-app/tests/cablage.rs`
  still finds every callback wired (the files it scans gain `gui.rs`).
- [ ] Step 2: fmt, commit, push, read CI (all jobs, including the release binary build).

### Task A2: `Platform`

**Files:**
- Create: `crates/iris-app/src/platform/mod.rs` (the trait, `install`, `current`,
  `MailtoRequest` moved here), `platform/windows.rs` (today's `windows_impl`, `open_path`,
  `open_url`, `shell_open`, notify's toast, the registry), `platform/other.rs` (today's stubs)
- Delete: `crates/iris-app/src/platform.rs` (split into the three files above)
- Modify: `crates/iris-app/src/notify.rs` (calls `current().notify`)

**Interfaces:**
- Produces:
  ```rust
  pub trait Platform: Send + Sync {
      fn open_url(&self, url: &str) -> Result<(), String>;
      fn open_path(&self, path: &std::path::Path) -> Result<(), String>;
      fn system_dark(&self) -> bool;
      fn number_style(&self) -> (char, char, bool);
      fn notify(&self, notice: &Notice) -> bool;
      fn touch(&self) -> bool { false }
      fn insets(&self) -> (f32, f32) { (0.0, 0.0) }
      fn font_override(&self) -> Option<(String, String)> { None }   // (text, display)
      fn secrets(&self, paths: &crate::Paths) -> Result<Box<dyn iris_secrets::SecretStore>, String>;
      fn dns_servers(&self) -> Vec<std::net::IpAddr> { Vec::new() } // empty: the system's config
      fn pick_file(&self, kinds: &[&str], done: Box<dyn FnOnce(Option<std::path::PathBuf>)>);
      fn pick_folder(&self, done: Box<dyn FnOnce(Option<std::path::PathBuf>)>);
      fn save_file(&self, name: &str, bytes: Vec<u8>, done: Box<dyn FnOnce(Option<std::path::PathBuf>)>);
      fn clipboard_image(&self) -> Option<image::RgbaImage>;
      fn set_clipboard_image(&self, image: &image::RgbaImage) -> bool;
      fn scan_qr(&self, done: Box<dyn FnOnce(Option<String>)>) { done(None) }
      fn has_background(&self) -> bool { false }                       // sets Chrome.has-background
      fn schedule(&self, plan: &BackgroundPlan) {}                     // background work, alarms
      fn update_channel(&self) -> UpdateChannel;
  }
  pub struct Notice { pub id: String, pub title: String, pub body: String, pub kind: NoticeKind }
  pub enum NoticeKind { Mail { thread: i64 }, Event { calendar: i64, uid: String, join: Option<String> },
                        Task { task: i64 }, Sending, Text }
  pub struct BackgroundPlan { pub every_minutes: u32, pub instant: bool, pub wifi_only: bool,
                              pub alarms: Vec<(i64 /* unix ms */, String /* alarm id */)> }
  pub enum UpdateChannel { Installer, Apk, Store }
  pub fn install(p: Box<dyn Platform>);            // once, at start; a second call panics
  pub fn current() -> &'static dyn Platform;       // `other` stub if nothing was installed
  ```
  The free functions `platform::open_url`, `open_path`, `system_dark`, `number_style`,
  `register_mailto`… keep their names as thin wrappers over `current()` so call sites in
  `shell.rs` do not move in this task.
- [ ] Step 1: tests in `platform/mod.rs`: `current()` before `install` is the stub; a fake
  platform installed in a test receives `open_url("https://example.com")`.
- [ ] Step 2: implement; Windows main installs `WindowsPlatform` before `gui::run`. fmt,
  commit, push, read CI.

### Task A3: the call sites that cannot exist on Android

**Files:**
- Modify: `crates/iris-app/Cargo.toml` (`rfd`, `arboard`, `directories`, `tray-icon`,
  `tauri-winrt-notification`, `windows-sys` and slint's `backend-winit` / `renderer-femtovg` /
  `unstable-winit-030` under `[target.'cfg(not(target_os = "android"))'.dependencies]`;
  `crates/iris-ui/Cargo.toml` the same for slint's winit features)
- Modify: `crates/iris-app/src/backup.rs`, `notes/mod.rs`, `notes/sheet.rs`, `shell.rs` (every
  `rfd::` dialog → `current().pick_file / pick_folder / save_file`); `notes/{mod,cellules,saisie,sheet}.rs`
  (every `arboard` image → `current().clipboard_image / set_clipboard_image`);
  `services.rs:138` (`open_secrets` → `current().secrets(paths)`, the Windows/other impl
  being today's keyring-then-vault logic); `crates/iris-discover/src/io.rs` (the resolver
  takes `servers: &[IpAddr]`, empty = today's system config); `shell.rs:3501`
  (`appliquer_apparence` applies `font_override`); `notes/bin.rs` (Recycle Bin on Windows,
  `<space>/.trash/` elsewhere)
- Test: existing tests; `crates/iris-discover` test for `servers` non-empty building a
  resolver with exactly those.

**Interfaces:**
- Consumes: `Platform` (A2).
- [ ] Step 1: implement; `cargo check --target aarch64-linux-android -p iris-app` is the
  proof, run in CI by Task B3 (not locally). fmt, commit, push, read CI.

## Phase B — The Android build

### Task B1: `iris-android`

**Files:**
- Create: `crates/iris-android/Cargo.toml`, `crates/iris-android/src/lib.rs`,
  `crates/iris-android/src/platform.rs` (`AndroidPlatform`, filled in Phases C–E),
  `crates/iris-android/src/jni.rs`
- Modify: root `Cargo.toml` (member; workspace dependency `iris-android` not needed)

**Interfaces:**
- Produces:
  ```rust
  // lib.rs — crate-type = ["cdylib"]; slint features: compat-1-2, renderer-software, std,
  // backend-android-activity-06
  #[no_mangle]
  fn android_main(app: slint::android::AndroidApp);  // logcat logging (android_logger, tag "Iris"),
      // slint::android::init(app.clone()), Paths::under(app.internal_data_path()),
      // platform::install(Box::new(AndroidPlatform::new(app))), iris_app::gui::run(options)
  // jni.rs
  pub fn with_env<T>(f: impl FnOnce(&mut jni::JNIEnv, &jni::objects::JObject) -> T) -> T; // the activity, via ndk_context
  pub fn call_bridge(method: &str, sig: &str, args: &[jni::objects::JValue]) -> jni::errors::Result<jni::objects::JValueOwned>;
      // static methods of the Kotlin object fr.iris.mail.IrisBridge
  ```
- [ ] Step 1: implement with `AndroidPlatform` returning defaults (no services yet:
  `open_url` logs, `secrets` an `EncryptedVault` keyed by a fixed test key behind
  `debug_assertions` only — replaced in C2). fmt, commit, push.

### Task B2: the Gradle project

**Files:**
- Create: `android/settings.gradle.kts`, `android/build.gradle.kts`, `android/gradle.properties`,
  `android/gradle/wrapper/*`, `android/app/build.gradle.kts`,
  `android/app/src/main/AndroidManifest.xml`, `android/app/src/main/kotlin/fr/iris/mail/IrisActivity.kt`,
  `android/app/src/main/kotlin/fr/iris/mail/IrisBridge.kt`, `android/app/src/main/res/` (launcher
  icon from `docs/assets/logo.svg`, `values/strings.xml`), `android/README.md`
- Modify: `.gitignore` (`android/.gradle`, `android/app/build`, `android/app/src/main/jniLibs`)

**Interfaces:**
- Produces: `IrisActivity : android.app.NativeActivity` (`meta-data android.app.lib_name =
  iris_android`, `configChanges="orientation|screenSize|screenLayout|smallestScreenSize|keyboardHidden|uiMode|density"`);
  `IrisBridge` (a Kotlin `object` with `@JvmStatic` methods called from Rust, and `external
  fun` entry points implemented in Rust). A Gradle task `cargoBuild` runs
  `cargo ndk -t arm64-v8a -t x86_64 -o app/src/main/jniLibs build --release -p iris-android`
  before `merge*JniLibFolders`.
- [ ] Step 1: write the project; `android/README.md` says how to build
  (`./gradlew assembleDebug`) and install (`adb install`). Commit, push.

### Task B3: Android in CI

**Files:**
- Modify: `.github/workflows/ci.yml` (job `android` on `ubuntu-latest`: rust targets
  `aarch64-linux-android x86_64-linux-android`, `cargo install cargo-ndk --locked`,
  `actions/setup-java@v4` 17, `android-actions/setup-android@v3` with NDK 27,
  `cargo clippy --target aarch64-linux-android -p iris-android -p iris-app --no-default-features -- -D warnings`,
  `./gradlew assembleDebug`, upload artifact `iris-debug-apk`; Swatinem cache keyed per target)
- Modify: `packaging/README.md` (an *Android* section: what CI builds, where the artifact is)

- [ ] Step 1: write the job; push; read CI until `android` is green (fix what it reports:
  features that pull winit, crates with C code missing a cross toolchain — rusqlite bundled
  and zstd build with the NDK's clang through cargo-ndk). The `forme` and `tests` jobs stay
  on Windows and unchanged.

### Task B4: the window on a phone

**Files:**
- Modify: `IrisActivity.kt` (window insets → `IrisBridge.insetsChanged(top, bottom)` in px /
  density; `onBackPressedDispatcher` callback → `IrisBridge.back()`, `finish()` when it
  returns false; `onConfigurationChanged` → `IrisBridge.configChanged(dark)`),
  `crates/iris-android/src/lib.rs` (the matching `extern "system"` functions, each posting to
  the event loop with `slint::invoke_from_event_loop`), `crates/iris-android/src/platform.rs`
  (`touch() → true`, `insets()`, `font_override() → Some(("sans-serif","sans-serif"))`,
  `system_dark()`, `number_style()` from `java.text.DecimalFormatSymbols`)
- Test: Kotlin unit test `IrisActivityTest` for the inset conversion; Rust test of the
  `system-back` return value is in the adaptive plan (C2).

**Interfaces:**
- Consumes: `AppWindow.invoke_system_back() -> bool` (adaptive plan C2), `Layout.inset-top/bottom`,
  `Layout.touch-input` (adaptive plan A1).
- [ ] Step 1: implement; install the CI artifact on a phone or the emulator and check the
  Review Focus line on rotation and Back (the one manual step of this phase; note the result
  in the PR). fmt, commit, push, read CI.

## Phase C — Services over JNI

### Task C1: secrets without a master password

**Files:**
- Create: `android/app/src/main/kotlin/fr/iris/mail/KeyVault.kt`
- Modify: `crates/iris-android/src/platform.rs` (`secrets`)

**Interfaces:**
- Produces: `KeyVault.vaultKey(context): String` — 32 random bytes made once, stored in
  `SharedPreferences` encrypted with an AES-256-GCM key of the `AndroidKeyStore`
  (alias `iris-vault`), returned as hex. `secrets()` = `EncryptedVault::open(paths.vault(),
  &Secret::new(vault_key))`.
- [ ] Step 1: Kotlin instrumented test (emulator in CI is not required; JVM test with a fake
  keystore) that two calls return the same key and a fresh install a new one.
- [ ] Step 2: implement; remove B1's test key. fmt, commit, push, read CI.

### Task C2: TLS, DNS, links and files

**Files:**
- Modify: `android/app/build.gradle.kts` (the `rustls-platform-verifier` Android component
  from its crate's `maven/` folder), `crates/iris-android/src/lib.rs` (call
  `rustls_platform_verifier::android::init_hosted(env, context)` before `gui::run`)
- Modify: `IrisBridge.kt` (`dnsServers(): Array<String>` from `ConnectivityManager`,
  `openUrl(url)` with `CustomTabsIntent` for http(s) and `ACTION_VIEW` otherwise,
  `openFile(path, mime)` through a `FileProvider` (`res/xml/file_paths.xml`: `cache/attachments`,
  `files/`), `pickFile(kinds, requestId)`, `pickFolder`, `saveFile(name, requestId)` via
  `ActivityResultContracts` copying into `cache/picked/`, then `IrisBridge.picked(requestId, path?)`)
- Modify: `crates/iris-android/src/platform.rs` (the matching methods; pending callbacks kept
  in a `Mutex<HashMap<u32, Box<dyn FnOnce(Option<PathBuf>) + Send>>>`)
- Modify: `crates/iris-oauth/src/http.rs:221` (`open_browser` takes an opener:
  `pub fn open_browser_with(url: &str, open: &dyn Fn(&str) -> Result<(), String>)`;
  `crates/iris-app/src/oauth.rs` passes `platform::current().open_url`)

- [ ] Step 1: Rust test in `iris-oauth` that `open_browser_with` hands the authorization
  URL to the opener unchanged; Kotlin JVM test that `dnsServers` formats IPv4 and IPv6.
- [ ] Step 2: implement; fmt, commit, push, read CI.

### Task C3: the clipboard's images

**Files:** Modify `IrisBridge.kt` (`clipboardImage(): ByteArray?` PNG, `setClipboardImage(png)`
via a `content://` URI of `FileProvider`), `crates/iris-android/src/platform.rs`.
- [ ] Step 1: implement with a JVM test of the PNG round trip; fmt, commit, push, read CI.

## Phase D — Mail in the background, notifications

### Task D1: headless sync

**Files:**
- Create: `crates/iris-app/src/headless.rs`
- Modify: `crates/iris-app/src/main.rs` (the CLI `sync` command calls `headless::sync_once`),
  `crates/iris-android/src/lib.rs` (`Java_fr_iris_mail_IrisBridge_syncOnce`),
  create `android/app/src/main/kotlin/fr/iris/mail/MailWorker.kt`
- Test: `crates/iris-app/tests/envoi.rs` (pending send picked up once)

**Interfaces:**
- Produces:
  ```rust
  pub struct SyncReport { pub new_threads: Vec<(i64, String, String)> /* thread, from, subject */,
                          pub sent: usize, pub failed: Option<String> }
  pub fn sync_once(paths: &crate::Paths, budget: std::time::Duration) -> iris_types::Result<SyncReport>;
      // opens Services, sends what is due (sending.json, scheduled_mail), syncs every account's
      // inbox within the budget, closes; refuses to run (Ok, empty) when the GUI holds the store
  pub fn gui_running() -> bool;   // a static flag set by gui::run
  ```
  `MailWorker : CoroutineWorker` returns `Result.retry()` on `failed`, `success()` otherwise,
  and posts a notification per new To do thread through `Platform::notify`'s Kotlin side.
  When `gui_running()`, the worker only asks the running window to sync (`AppWindow::invoke_sync_all`, the callback wired at `shell.rs:353`, posted with `slint::invoke_from_event_loop`).
- [ ] Step 1: tests — a pending send in `sending.json` is sent by `sync_once` and the file
  emptied; a second call sends nothing; with `gui_running()` true, `sync_once` returns an
  empty report without opening the store.
- [ ] Step 2: implement; `AndroidPlatform::schedule` enqueues a unique periodic work
  (`ExistingPeriodicWorkPolicy.UPDATE`, network constraint, unmetered when `wifi_only`).
  fmt, commit, push, read CI.

### Task D2: notifications and their actions

**Files:**
- Create: `android/app/src/main/kotlin/fr/iris/mail/Notices.kt`, `ActionReceiver.kt`
- Modify: `crates/iris-app/src/headless.rs` (`act`), `crates/iris-android/src/platform.rs` (`notify`),
  `crates/iris-android/src/lib.rs` (`Java_fr_iris_mail_IrisBridge_onAction`)
- Test: `crates/iris-app/tests/` new `actions.rs`

**Interfaces:**
- Produces: channels `mail`, `reminders`, `sending`, `instant`; actions per `NoticeKind` —
  Mail: `done`, `snooze` (tomorrow 08:00), `reply` (RemoteInput); Event: `join`, `snooze5`;
  Task: `done`, `later` (tomorrow). `pub fn act(paths: &Paths, notice_id: &str, action: &str, text: Option<&str>) -> iris_types::Result<()>`
  — through the running controller when `gui_running()`, else a short-lived `Services`.
  A reply goes into the same send queue as the composer, with its undo delay shown as a
  `Sending` notification carrying *Undo*.
- [ ] Step 1: tests — `act("mail:7","done",None)` sets thread 7 Done once on either route;
  `reply` with text enqueues one message to the thread's sender and marks it Waiting when
  `automation.reply_marks_waiting`.
- [ ] Step 2: implement; fmt, commit, push, read CI.

### Task D3: Instant, alarms, boot

**Files:**
- Create: `android/app/src/main/kotlin/fr/iris/mail/InstantService.kt`, `AlarmReceiver.kt`, `BootReceiver.kt`
- Modify: `crates/iris-app/src/settings.rs` (`background_instant: bool`, `background_wifi_only: bool`,
  device-local), `crates/iris-app/src/gui.rs` (`Platform::schedule` called at start and when
  settings, scheduled sends or reminders change)

**Interfaces:**
- Produces: `InstantService` (foreground, type `dataSync`, notification on channel `instant`:
  "Instant mail is on · N accounts connected") keeps the process and the controller's IDLE
  alive; on `onTimeout` (Android 15's limit) it stops and the periodic work continues, and
  the setting page shows "Instant paused until tomorrow". Alarms: `setExactAndAllowWhileIdle`
  for each `BackgroundPlan.alarms` entry → `headless::sync_once`. `BootReceiver` re-schedules.
- [ ] Step 1: Kotlin JVM tests of the plan → work/alarm mapping; fmt, commit, push, read CI.

## Phase E — Fitting in

### Task E1: open with Iris, share to Iris

**Files:**
- Modify: `AndroidManifest.xml` (intent filters: `VIEW` `mailto:`; `VIEW` mime
  `text/calendar`, `message/rfc822`, `application/x-apple-aspen-config`, `text/markdown`, and
  `.sheet` by path pattern; `SEND` `text/plain`, `image/*`), `IrisActivity.kt` (`onCreate`/
  `onNewIntent` → copy to `cache/incoming/` → `IrisBridge.incoming(kind, path, text)`),
  `crates/iris-android/src/lib.rs`, `crates/iris-app/src/gui.rs`
- Test: `crates/iris-app/src/gui.rs` test of `Incoming` routing

**Interfaces:**
- Produces: `pub enum Incoming { Mailto(MailtoRequest), Calendar(PathBuf), Message(PathBuf), Profile(PathBuf), NoteFile(PathBuf), Shared { text: String, image: Option<PathBuf> } }`
  and `pub fn receive(window: &iris_ui::AppWindow, incoming: Incoming)` — mailto opens the
  composer (today's path), a calendar file opens the *Add to calendar* page (Screens F3), a
  profile the account setup with it, a note file imports into the current space, `Shared`
  opens the *Send to Iris* sheet (Screens F3).
- [ ] Step 1: tests per variant reaching the right window flag; implement; fmt, commit, push, read CI.

### Task E2: scanning a QR code

**Files:** Create `android/app/src/main/kotlin/fr/iris/mail/ScanActivity.kt` (CameraX preview,
ZXing `QRCodeReader` on frames, the frame corners of `sync.html` Y3, ×), modify
`IrisBridge.kt` (`scanQr(requestId)` → result `IrisBridge.scanned(requestId, text?)`),
`crates/iris-android/src/platform.rs` (`scan_qr`).
- [ ] Step 1: JVM test decoding a QR image fixture of `iris-pair:1?…` (example values);
  implement; fmt, commit, push, read CI.

### Task E3: widgets

**Files:**
- Create: `crates/iris-app/src/widgets.rs`; `android/app/src/main/kotlin/fr/iris/mail/widgets/{TodayWidget,InboxWidget,HabitsWidget}.kt`,
  `android/app/src/main/res/layout/widget_*.xml`, `res/xml/widget_*_info.xml`
- Modify: `crates/iris-app/src/home.rs` (after each refresh of Home's data, `widgets::write`)

**Interfaces:**
- Produces: `pub struct WidgetData { pub date: String, pub next: Option<WidgetEvent>, pub tasks: Vec<WidgetTask>, pub todo: u32, pub unread: u32, pub waiting: u32, pub habits: Vec<WidgetHabit> }`
  (each item with its id and label, at most 5 tasks and 6 habits) and
  `pub fn write(paths: &Paths, data: &WidgetData) -> std::io::Result<()>` writing
  `files/widgets.json` atomically (`.partial` then rename), then `IrisBridge.widgetsChanged()`.
  Widgets read the file; a tap opens `IrisActivity` with an `iris://open?page=<Place.page>`
  intent; ticking a task or a habit goes to `ActionReceiver` (`act`).
- [ ] Step 1: tests — the JSON of a sample `WidgetData` (field names fixed for Kotlin);
  atomic write; implement; fmt, commit, push, read CI.

## Phase F — The release

### Task F1: a signed APK in every release, and the update check

**Files:**
- Modify: `.github/workflows/release.yml` (job `apk` on ubuntu after the version check:
  build `assembleRelease`, sign with `apksigner` from secrets `ANDROID_KEYSTORE_B64`,
  `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`; add `apk`, `apk_size`, `apk_sha256` to
  `latest.json` before it is signed; upload `iris-X.Y.Z.apk` to the same release)
- Modify: `crates/iris-app/src/update.rs:50,158` (`Manifest` gains `apk: Option<String>`,
  `apk_size`, `apk_sha256`; with `UpdateChannel::Apk`, an update opens the download page
  instead of `launch_installer`; `Store` never checks)
- Modify: `packaging/README.md` (*Android*: signing key, what the job does), `CLAUDE.md`
  (*Publishing a release*: the APK goes with the installer)
- Test: `update.rs` tests for a manifest with and without `apk`, and the existing ignored
  signature test extended with the new fields.

- [ ] Step 1: tests; implement; fmt, commit, push, read CI.

### Task F2: release 6.6.0 — Iris for Android

- [ ] CHANGELOG `## 6.6.0` — New: "Iris for Android: your mail, calendar, tasks and notes on
  your phone." / "Notifications you can act on: Done, Snooze, Reply." / "Widgets for today,
  your inbox and your habits." / "Open invitations, profiles and mail links with Iris, and
  share pages to it."
- [ ] README (*Install*: Android; *Current limits*: "Windows only" becomes Windows and
  Android, plus Android's limits — Instant's daily limit, notes in Iris's own folder;
  *Privacy*: background work and widgets stay on the phone), ARCHITECTURE (layer diagram:
  `iris-android` and the Kotlin layer; *Running in the background* for Android; *Known
  departures*), COMPARISON (mobile), PLAN (the epic's stories ticked), the Android spec's
  *What changed since* if anything moved. fmt, commit, push, read CI, check the Release run
  and that the APK is attached.
