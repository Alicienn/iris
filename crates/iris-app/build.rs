//! Stamps the Windows executable with its identity.
//!
//! Without this, an installed Iris shows a blank page icon in Explorer, "iris.exe" in
//! the task manager, and no version at all in the file properties. None of that stops
//! the application working, and all of it makes an installed copy look like something
//! that escaped from a build directory.
//!
//! The step is skipped anywhere but Windows, and a failure to run the resource
//! compiler is a warning rather than an error: an icon is not worth refusing to build
//! over.

fn main() {
    println!("cargo:rerun-if-changed=assets/iris.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/iris.ico");
        resource.set("ProductName", "Iris");
        resource.set("FileDescription", "Iris — a mail client");
        resource.set("CompanyName", "Iris");
        resource.set("LegalCopyright", "Iris contributors");
        resource.set("OriginalFilename", "iris.exe");

        if let Err(e) = resource.compile() {
            println!("cargo:warning=could not stamp the executable: {e}");
        }
    }
}
