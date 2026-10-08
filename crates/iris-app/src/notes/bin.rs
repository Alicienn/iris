//! Deleted notes go to the Windows Recycle Bin, where Explorer can bring them back.

#[cfg(windows)]
use iris_types::Error;
use iris_types::Result;
use std::path::Path;

/// The Recycle Bin of the system.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemBin;

impl iris_vault::RecycleBin for SystemBin {
    fn throw(&self, path: &Path) -> Result<()> {
        jeter(path)
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn jeter(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE,
        SHFILEOPSTRUCTW,
    };
    // A list of paths, each ended by a zero, the list by another.
    let mut chemin: Vec<u16> = path.as_os_str().encode_wide().collect();
    chemin.push(0);
    chemin.push(0);
    let mut operation = SHFILEOPSTRUCTW {
        hwnd: std::ptr::null_mut(),
        wFunc: FO_DELETE as _,
        pFrom: chemin.as_ptr(),
        pTo: std::ptr::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as _,
        fAnyOperationsAborted: 0,
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: std::ptr::null(),
    };
    // SAFETY: `operation` points to `chemin`, double-zero-terminated and alive for the
    // whole call; no window, no name mappings asked for.
    let code = unsafe { SHFileOperationW(&mut operation) };
    if code != 0 || operation.fAnyOperationsAborted != 0 {
        return Err(Error::other(format!(
            "it could not go to the Recycle Bin (code {code})"
        )));
    }
    Ok(())
}

#[cfg(not(windows))]
fn jeter(path: &Path) -> Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Shows a file in Explorer, selected.
pub fn reveal(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer.exe")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn()
            .map(|_| ())
            .map_err(|e| Error::other(format!("Explorer: {e}")))
    }
    #[cfg(not(windows))]
    {
        crate::platform::open_path(path.parent().unwrap_or(path))
    }
}
