//! One Iris per session.
//!
//! Iris keeps running in the notification area once its window is closed. Launching it
//! again from the Start menu then has to bring that window back: a second process
//! would find the database locked by the first and die without a word, which is what
//! users saw — a shortcut that did nothing.
//!
//! The first process owns a named mutex and waits on a named event. A later launch
//! finds the mutex taken, leaves what it was asked to do (a `mailto:` address) in a
//! file, sets the event and exits. The first process wakes, shows its window and reads
//! the file.
//!
//! Elsewhere than Windows every launch is the first one.

use std::path::{Path, PathBuf};

/// What `claim` found.
pub enum Claim {
    /// No other Iris is running: this one carries on, and keeps the guard alive.
    First(Primary),
    /// Another Iris is running and has been asked to show itself.
    AlreadyRunning,
}

/// Held by the running Iris for as long as it lives.
pub struct Primary {
    #[cfg(windows)]
    inner: plateforme::Handles,
}

/// Where a later launch leaves the address it was started with.
fn pending_path(cache: &Path) -> PathBuf {
    cache.join("mailto-pending.txt")
}

/// Becomes the running Iris, or hands over to the one already running.
///
/// `mailto` is what this launch was asked to compose; `wake` is false for a launch that
/// should stay out of sight (the one Windows makes at sign-in), which then just exits.
pub fn claim(cache: &Path, mailto: Option<&str>, wake: bool) -> Claim {
    #[cfg(windows)]
    {
        match plateforme::Handles::acquire() {
            Some(inner) => Claim::First(Primary { inner }),
            None => {
                if wake {
                    if let Some(adresse) = mailto {
                        let _ = std::fs::create_dir_all(cache);
                        let _ = std::fs::write(pending_path(cache), adresse);
                    }
                    plateforme::wake_running();
                }
                Claim::AlreadyRunning
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (cache, mailto, wake);
        Claim::First(Primary {})
    }
}

impl Primary {
    /// Calls `on_wake` on a thread of its own each time a later launch asks for the
    /// window. The caller moves the work onto the interface thread.
    pub fn listen(&self, on_wake: impl Fn() + Send + 'static) {
        #[cfg(windows)]
        self.inner.listen(on_wake);
        #[cfg(not(windows))]
        let _ = on_wake;
    }
}

/// The address a later launch left behind, if any. Reading it removes it.
pub fn take_pending_mailto(cache: &Path) -> Option<String> {
    let chemin = pending_path(cache);
    let texte = std::fs::read_to_string(&chemin).ok()?;
    let _ = std::fs::remove_file(&chemin);
    let texte = texte.trim();
    (!texte.is_empty()).then(|| texte.to_owned())
}

/// Puts this process's visible window in front.
///
/// Showing a window from the notification area does not raise it above the one the
/// user is working in. The launch that woke us allowed any process to take the
/// foreground (`AllowSetForegroundWindow`), so this is the moment to take it.
pub fn bring_to_front() {
    #[cfg(windows)]
    plateforme::bring_to_front();
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod plateforme {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, GetCurrentThreadId, OpenEventW, SetEvent, WaitForSingleObject,
        EVENT_MODIFY_STATE, INFINITE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, EnumThreadWindows, IsWindowVisible, SetForegroundWindow, ASFW_ANY,
    };

    // `Local\`: one Iris per signed-in user, not per machine.
    const MUTEX: &str = "Local\\Iris.SingleInstance";
    const EVENT: &str = "Local\\Iris.ShowWindow";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A handle that may cross to the listening thread. Kernel handles are not tied
    /// to the thread that opened them.
    #[derive(Clone, Copy)]
    struct Raw(HANDLE);
    unsafe impl Send for Raw {}

    pub struct Handles {
        mutex: HANDLE,
        event: Raw,
    }

    impl Handles {
        pub fn acquire() -> Option<Self> {
            // The event before the mutex: a launch that finds the mutex must also
            // find the event to set.
            let nom_evenement = wide(EVENT);
            let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, nom_evenement.as_ptr()) };
            let nom_mutex = wide(MUTEX);
            let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, nom_mutex.as_ptr()) };
            let deja = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;

            if mutex.is_null() {
                // Without a mutex there is no way to tell: carry on as the only one,
                // which is what Iris did before.
                tracing::warn!("single instance: no mutex, carrying on");
                return Some(Handles {
                    mutex,
                    event: Raw(event),
                });
            }
            if deja {
                unsafe {
                    CloseHandle(mutex);
                    if !event.is_null() {
                        CloseHandle(event);
                    }
                }
                return None;
            }
            Some(Handles {
                mutex,
                event: Raw(event),
            })
        }

        pub fn listen(&self, on_wake: impl Fn() + Send + 'static) {
            let event = self.event;
            if event.0.is_null() {
                return;
            }
            let lance = std::thread::Builder::new()
                .name("single-instance".into())
                .spawn(move || {
                    let event = event;
                    loop {
                        // Auto-reset: one wait returns once per launch.
                        if unsafe { WaitForSingleObject(event.0, INFINITE) } != WAIT_OBJECT_0 {
                            return;
                        }
                        on_wake();
                    }
                });
            if let Err(e) = lance {
                tracing::warn!("single instance: no listening thread: {e}");
            }
        }
    }

    impl Drop for Handles {
        fn drop(&mut self) {
            // The listening thread may still wait on the event: it goes with the
            // process, and the event with it.
            if !self.mutex.is_null() {
                unsafe { CloseHandle(self.mutex) };
            }
        }
    }

    pub fn wake_running() {
        // This launch was started by the user, so it may give the foreground away;
        // the running Iris could not take it on its own.
        unsafe { AllowSetForegroundWindow(ASFW_ANY) };
        let nom = wide(EVENT);
        let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, nom.as_ptr()) };
        if event.is_null() {
            tracing::warn!("single instance: the running Iris has no event to set");
            return;
        }
        unsafe {
            SetEvent(event);
            CloseHandle(event);
        }
    }

    pub fn bring_to_front() {
        // The window lives on the interface thread, the one calling us. The
        // notification area's hidden window lives there too, but is never visible.
        unsafe extern "system" fn chaque(hwnd: HWND, _: LPARAM) -> i32 {
            if unsafe { IsWindowVisible(hwnd) } != 0 {
                unsafe { SetForegroundWindow(hwnd) };
                return 0;
            }
            1
        }
        unsafe { EnumThreadWindows(GetCurrentThreadId(), Some(chaque), 0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pending_address_is_read_once() {
        let dossier = tempfile::tempdir().unwrap();
        assert_eq!(take_pending_mailto(dossier.path()), None);
        std::fs::write(pending_path(dossier.path()), "mailto:ann@example.com\n").unwrap();
        assert_eq!(
            take_pending_mailto(dossier.path()).as_deref(),
            Some("mailto:ann@example.com")
        );
        assert_eq!(take_pending_mailto(dossier.path()), None);
    }
}
