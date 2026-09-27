# Packaging

## Building the installer

```
powershell -File packaging\build-installer.ps1
```

The result is `packaging\output\iris-setup-<version>.exe`, around 16 MB. The script runs
the three steps below in order and passes the version from `Cargo.toml` to Inno Setup,
so it is written in one place. The release workflow runs the same script when a
`vX.Y.Z` tag is pushed.

```
cargo build --release
powershell -File packaging\build-plugins.ps1
iscc /DAppVersion=x.y.z packaging\iris.iss
```

The middle step compiles the three bundled modules to WebAssembly and lays them out
under `packaging\plugins\<id>\` exactly as the registry reads them — one directory per
module, manifest and binary side by side. The installer copies that tree verbatim, so
the layout is written down in one place. It needs the wasm target:

```
rustup target add wasm32-unknown-unknown
```

Skipping the step does not break the compile; it produces an installer whose "install
the bundled modules" box installs nothing.

[Inno Setup 6](https://jrsoftware.org/isdl.php) provides `iscc`, or
`winget install JRSoftware.InnoSetup`. Where it lands depends on how it was installed —
`%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe` for a per-user install,
`C:\Program Files (x86)\Inno Setup 6\ISCC.exe` for a machine-wide one. Add it to `PATH`
or call it by its full path.

## Why there is an installer at all

Three things Windows grants through a Start Menu shortcut and the registry, and
withholds from a loose executable.

**Notifications.** A Windows toast is shown only for an identity the system knows —
an `AppUserModelID` — and the only way to declare one is a Start Menu shortcut
carrying it. Without the shortcut, `notify::show` returns false and nothing appears.
The common workaround is to borrow PowerShell's identifier so that *something* shows
up; a notification that lies about who sent it is worse than none, so Iris does not.

**`mailto:` links.** Registering means writing to `Software\Clients\Mail`,
`Software\RegisteredApplications` and a `Capabilities` key. Any two of the three
achieve nothing. The application does this itself — `iris register` — and the
installer merely calls it, because two copies of the same registry layout eventually
disagree and the one that matters is the one the code knows.

**Start at login.** One value under `Run`, passing `--tray`.

None of it needs administrator rights. `PrivilegesRequired=lowest`: everything goes
into the user's own profile, no elevation prompt appears, and no other account on the
machine is touched.

## What the uninstaller removes, and what it keeps

Removed: the program, the shortcuts, the registry entries, the cache under
`%LOCALAPPDATA%\Iris\Iris\cache`, which is entirely rebuildable, and the three module
files it placed under `%APPDATA%\Iris\Iris\data\plugins`.

Kept: everything under `%APPDATA%\Iris\Iris` that the installer did not put there — the
database, the settings, the vault, and each module's `settings.toml`, which the
application writes and which survives a reinstall. That is the user's mail, not the
installer's. An uninstall that takes ten years of correspondence with it is not
forgiven.

The modules go into the profile rather than next to the program on purpose: a module's
settings are written beside its manifest, and a module under `Program Files` would be
a module whose settings cannot be changed on a machine where the user cannot write
there. The application reads exactly one modules directory, and that is it.

The doubled name is not a typo. Iris locates its data through the `directories` crate,
which on Windows composes `%APPDATA%\<organisation>\<application>\data` — and both are
"Iris". A copy dropped under `%APPDATA%\Iris\plugins`, which is what anyone would write,
is never read: the Modules screen simply stays empty, and nothing says why.

## What it does not solve

**SmartScreen.** An unsigned installer downloaded from the web gets "Windows protected
your PC" until enough people have run it, or until it is signed with a code-signing
certificate. Nothing in the script changes that; only a certificate does.

**Portability.** Iris keeps its data in `%APPDATA%\Iris\Iris`, so a copy on a USB stick
still leaves traces on the machine. A portable mode that puts the database beside the
binary is separate, small, and not done.
