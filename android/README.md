# Iris for Android

The APK around the Rust library `libiris_android.so` (crate `crates/iris-android`).
Android's own `NativeActivity` loads it; the window, its keys and its touches are
Slint's, and the rest is the same Iris as on Windows. Design: the spec
`docs/superpowers/specs/2026-10-26-iris-android-design.md`.

## Building

CI does it (job `android` in `.github/workflows/ci.yml`) and the Release workflow
attaches the APK to each release beside the Windows installer. CI builds for phones
only (arm64); the emulator also needs `x86_64`, as below. By hand, with the Android
SDK, NDK 27 and Java 17:

```
rustup target add aarch64-linux-android x86_64-linux-android
cargo install cargo-ndk --locked
cargo ndk -t arm64-v8a -t x86_64 -o android/app/src/main/jniLibs build --release -p iris-android
IRIS_VERSION=6.3.0 gradle -p android assembleRelease
adb install android/app/build/outputs/apk/release/app-release.apk
```

## Signing

A release is signed with the project's key when CI has these secrets:
`ANDROID_KEYSTORE_B64` (the keystore, base64), `ANDROID_KEYSTORE_PASSWORD`,
`ANDROID_KEY_ALIAS` and, if it differs, `ANDROID_KEY_PASSWORD`. Without them the APK is
signed with the build machine's debug key: it installs, but every new one has another
key, so Android asks to uninstall the old one first. Make the keystore once, keep it
outside the repository with the update key, and never change it: copies installed with
one key only accept updates signed with it.

```
keytool -genkeypair -v -keystore iris-release.jks -alias iris -keyalg RSA -keysize 4096 -validity 36500
```

## What is not there yet

The platform's services that need Android's Java API (notifications and their actions,
background mail, the document picker, the clipboard, the camera, widgets) come with
their Kotlin sources under `app/src/main/kotlin`, following the plan
`docs/superpowers/plans/2026-10-26-iris-android-platform.md`.
