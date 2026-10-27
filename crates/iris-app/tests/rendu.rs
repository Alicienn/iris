//! The window is drawn by the processor, and that choice is made where the window is.
//!
//! The call selecting Slint's software renderer lived for a while in the `iris memory`
//! command instead of the start of the interface: the ordinary window ran on OpenGL,
//! 161 MB against 55 MB measured on the same build. Nothing in the running
//! application shows which renderer it got, so the source is what is checked.
//!
//! Since 6.3.0 the window starts in `gui::run`, shared with Android: Windows hands it a
//! `prepare` that selects the renderer, and `run` calls it before the first window.

#[test]
fn the_interface_selects_the_software_renderer_before_building_its_window() {
    let racine = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let main = std::fs::read_to_string(racine.join("src/main.rs")).expect("main.rs");
    let gui = std::fs::read_to_string(racine.join("src/gui.rs")).expect("gui.rs");

    // Windows' `prepare`: the renderer first.
    let appel = main
        .find("gui::run(")
        .expect("main starts the window through gui::run");
    assert!(
        main[appel..].contains("select_software_renderer();"),
        "the closure handed to gui::run must select the software renderer"
    );

    // `run`: `prepare` before the first window.
    let debut = gui.find("pub fn run(").expect("gui::run");
    let corps = &gui[debut..];
    let prepare = corps.find("prepare()").expect("run calls prepare");
    let fenetre = corps.find("shell::build(").expect("run builds the window");
    assert!(
        prepare < fenetre,
        "the renderer is chosen before the first window, or Slint has already picked OpenGL"
    );
}
