//! The window is drawn by the processor, and that choice is made where the window is.
//!
//! The call selecting Slint's software renderer lived for a while in the `iris memory`
//! command instead of the start of the interface: the ordinary window ran on OpenGL,
//! 161 MB against 55 MB measured on the same build. Nothing in the running
//! application shows which renderer it got, so the source is what is checked.

#[test]
fn the_interface_selects_the_software_renderer_before_building_its_window() {
    let racine = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let main = std::fs::read_to_string(racine.join("src/main.rs")).expect("main.rs");

    let debut = main.find("fn run_gui(").expect("run_gui");
    let corps = &main[debut..];
    let choix = corps
        .find("select_software_renderer();")
        .expect("run_gui must select the software renderer");
    let fenetre = corps
        .find("shell::build(")
        .expect("run_gui builds the window");
    assert!(
        choix < fenetre,
        "the renderer is chosen before the first window, or Slint has already picked OpenGL"
    );
}
