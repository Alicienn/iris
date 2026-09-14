# Packaging

## Building the installer

```
cargo build --release
iscc packaging\iris.iss
```

The result is `packaging\output\iris-setup-0.1.0.exe`.

[Inno Setup 6](https://jrsoftware.org/isdl.php) provides `iscc`. Add it to `PATH`, or
call it by its full path — the default is
`C:\Program Files (x86)\Inno Setup 6\ISCC.exe`.

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

Removed: the program, the shortcuts, the registry entries, and the cache under
`%LOCALAPPDATA%\Iris`, which is entirely rebuildable.

Kept: everything under `%APPDATA%\Iris` — the database, the settings, the vault. That
is the user's mail, not the installer's. An uninstall that takes ten years of
correspondence with it is not forgiven.

## What it does not solve

**SmartScreen.** An unsigned installer downloaded from the web gets "Windows protected
your PC" until enough people have run it, or until it is signed with a code-signing
certificate. Nothing in the script changes that; only a certificate does.

**Portability.** Iris keeps its data in `%APPDATA%\Iris`, so a copy on a USB stick
still leaves traces on the machine. A portable mode that puts the database beside the
binary is separate, small, and not done.
