//! The interface's layers, held by a test rather than by memory.
//!
//! The files live in layer folders (`theme`, `base`, `controls`, `lists`, `layout`,
//! `shell`, `screens`). A layer imports only the layers before it; a screen takes its
//! colours from the theme and its fields from the controls.

use std::fs;
use std::path::{Path, PathBuf};

const COUCHES: [&str; 7] = [
    "theme", "base", "controls", "lists", "layout", "shell", "screens",
];

fn ui() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("ui")
}

/// Every `.slint` file of a layer, with its text.
fn fichiers(couche: &str) -> Vec<(PathBuf, String)> {
    let dossier = ui().join(couche);
    let mut v: Vec<_> = fs::read_dir(&dossier)
        .unwrap_or_else(|e| panic!("{}: {e}", dossier.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "slint"))
        .map(|p| {
            let texte = fs::read_to_string(&p).unwrap();
            (p, texte)
        })
        .collect();
    v.sort();
    v
}

/// The layers a file imports, read from its `"@iris/<layer>/…"` paths.
fn importees(texte: &str) -> Vec<String> {
    texte
        .match_indices("\"@iris/")
        .filter_map(|(i, _)| {
            let reste = &texte[i + 7..];
            let fin = reste.find(['/', '"'])?;
            reste[fin..]
                .starts_with('/')
                .then(|| reste[..fin].to_string())
        })
        .collect()
}

#[test]
fn a_layer_imports_only_the_layers_before_it() {
    let mut fautes = Vec::new();
    for (rang, couche) in COUCHES.iter().enumerate() {
        for (chemin, texte) in fichiers(couche) {
            for importee in importees(&texte) {
                match COUCHES.iter().position(|c| *c == importee) {
                    Some(r) if r <= rang => {}
                    _ => fautes.push(format!(
                        "{} imports {importee}",
                        chemin.strip_prefix(ui()).unwrap().display()
                    )),
                }
            }
        }
    }
    assert!(
        fautes.is_empty(),
        "imports against the layers:\n{}",
        fautes.join("\n")
    );
}

#[test]
fn screens_take_their_colours_from_the_theme() {
    let mut fautes = Vec::new();
    for (chemin, texte) in fichiers("screens") {
        for (n, ligne) in texte.lines().enumerate() {
            let code = ligne.split("//").next().unwrap_or("");
            for (i, _) in code.match_indices('#') {
                let hex: String = code[i + 1..]
                    .chars()
                    .take_while(|c| c.is_ascii_hexdigit())
                    .collect();
                // A shadow is black at some opacity, in every theme.
                let ombre = hex.len() == 8 && hex.starts_with("000000");
                if hex.len() >= 6 && !ombre {
                    fautes.push(format!(
                        "{}:{}: {}",
                        chemin.file_name().unwrap().to_string_lossy(),
                        n + 1,
                        ligne.trim()
                    ));
                }
            }
        }
    }
    assert!(
        fautes.is_empty(),
        "colours written in a screen:\n{}",
        fautes.join("\n")
    );
}

#[test]
fn fields_are_iris_own() {
    // The style's fields change their background with focus; ours show it on the border.
    let mut fautes = Vec::new();
    for couche in COUCHES {
        for (chemin, texte) in fichiers(couche) {
            for mot in ["LineEdit", "TextEdit"] {
                if texte
                    .lines()
                    .any(|l| l.split("//").next().unwrap_or("").contains(mot))
                {
                    fautes.push(format!("{} uses {mot}", chemin.display()));
                }
            }
        }
    }
    assert!(fautes.is_empty(), "{}", fautes.join("\n"));
}
