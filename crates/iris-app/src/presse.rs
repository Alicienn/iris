//! The clipboard, as the notes and spreadsheets use it.
//!
//! On a desktop it is `arboard`, as it always was. Android has no `arboard`: a stand-in
//! with the same calls answers there, its text through Android's own clipboard
//! ([`crate::platform::Platform::clipboard_set`]); images and HTML stay unavailable.
//! Text fields keep their own copy and paste either way: Slint does that itself.

#[cfg(not(target_os = "android"))]
pub use arboard::{Clipboard, Error};

#[cfg(target_os = "android")]
pub use autre::{Clipboard, Error};

#[cfg(target_os = "android")]
mod autre {
    use std::borrow::Cow;

    /// Nothing there, or nothing that can be read.
    #[derive(Debug)]
    pub struct Error;

    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the clipboard is not available here")
        }
    }

    impl std::error::Error for Error {}

    /// An image as `arboard` gives it: width, height, RGBA bytes.
    pub struct ImageData<'a> {
        pub width: usize,
        pub height: usize,
        pub bytes: Cow<'a, [u8]>,
    }

    pub struct Clipboard;

    impl Clipboard {
        pub fn new() -> Result<Self, Error> {
            Ok(Clipboard)
        }
        pub fn get_text(&mut self) -> Result<String, Error> {
            crate::platform::current().clipboard_get().ok_or(Error)
        }
        pub fn set_text<'a>(&mut self, text: impl Into<Cow<'a, str>>) -> Result<(), Error> {
            if crate::platform::current().clipboard_set(&text.into()) {
                Ok(())
            } else {
                Err(Error)
            }
        }
        pub fn get_image(&mut self) -> Result<ImageData<'static>, Error> {
            Err(Error)
        }
        pub fn get(&mut self) -> Get {
            Get
        }
    }

    pub struct Get;

    impl Get {
        pub fn html(self) -> Result<String, Error> {
            Err(Error)
        }
    }
}
