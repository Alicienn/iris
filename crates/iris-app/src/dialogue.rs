//! The system's dialogs to choose a file or a folder, or where to save one.
//!
//! On a desktop it is `rfd`, as it always was. Android has no `rfd`: a stand-in with
//! the same calls answers there, as if the dialog were cancelled, until the system's
//! document picker is wired through [`crate::platform::Platform`].

#[cfg(not(target_os = "android"))]
pub use rfd::FileDialog;

#[cfg(target_os = "android")]
pub use autre::FileDialog;

#[cfg(target_os = "android")]
mod autre {
    use std::path::{Path, PathBuf};

    /// A dialog that is never shown: every choice comes back cancelled.
    #[derive(Debug, Default, Clone)]
    pub struct FileDialog;

    impl FileDialog {
        pub fn new() -> Self {
            Self
        }
        pub fn add_filter(self, _name: impl Into<String>, _extensions: &[impl ToString]) -> Self {
            self
        }
        pub fn set_title(self, _title: impl Into<String>) -> Self {
            self
        }
        pub fn set_directory<P: AsRef<Path>>(self, _path: P) -> Self {
            self
        }
        pub fn set_file_name(self, _name: impl Into<String>) -> Self {
            self
        }
        pub fn pick_file(self) -> Option<PathBuf> {
            None
        }
        pub fn pick_files(self) -> Option<Vec<PathBuf>> {
            None
        }
        pub fn pick_folder(self) -> Option<PathBuf> {
            None
        }
        pub fn save_file(self) -> Option<PathBuf> {
            None
        }
    }
}
