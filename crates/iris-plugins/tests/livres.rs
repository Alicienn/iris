//! Les manifestes des modules livrés avec Iris sont-ils lisibles ?
//!
//! Un manifeste refusé ne fait rien tomber : le registre écarte le module, écrit une
//! ligne dans le journal, et l'application démarre sans lui. C'est le bon
//! comportement — et c'est exactement pourquoi il faut ce test : la seule chose qu'on
//! observerait sans lui est un module absent de l'écran des modules, plusieurs
//! semaines après l'avoir livré.
//!
//! Le piège précis est le champ `kind` d'un réglage. L'hôte connaît `text`, `number` et
//! `toggle` ; écrire `integer` ou `boolean`, ce que tout le monde écrit d'abord, rend
//! le manifeste entier illisible.

use iris_plugins::Manifest;
use std::path::{Path, PathBuf};

/// Les modules livrés, par le nom de leur caisse.
const LIVRES: [&str; 3] = [
    "iris-plugin-vip",
    "iris-plugin-office-hours",
    "iris-plugin-filer",
];

fn crates_dir() -> PathBuf {
    // `CARGO_MANIFEST_DIR` est `crates/iris-plugins`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf()
}

#[test]
fn les_manifestes_livres_se_lisent() {
    for caisse in LIVRES {
        let chemin = crates_dir().join(caisse).join("plugin.toml");
        let source = std::fs::read_to_string(&chemin)
            .unwrap_or_else(|e| panic!("{}: {e}", chemin.display()));

        let manifeste = Manifest::from_toml(&source)
            .unwrap_or_else(|e| panic!("{}: {e}", chemin.display()));

        assert!(!manifeste.id.is_empty(), "{caisse}: no id");
        assert!(!manifeste.name.is_empty(), "{caisse}: no name");
        assert!(
            !manifeste.description.is_empty(),
            "{caisse}: a module with no description is a module nobody can decide to install"
        );

        for reglage in &manifeste.settings {
            assert!(
                reglage.is_valid_key(),
                "{caisse}: unusable setting key {:?}",
                reglage.key
            );
            assert!(
                !reglage.hint.is_empty(),
                "{caisse}: setting {:?} has no hint — the format of a rule list is not \
                 guessable from its label",
                reglage.key
            );
        }
    }
}

#[test]
fn aucun_module_livre_ne_demande_le_reseau() {
    // Les trois trient du courrier à partir de ce que l'hôte leur passe. Aucun n'a
    // d'hôte à joindre, et une permission réseau qui apparaîtrait ici serait soit une
    // erreur, soit un changement qui mérite d'être remarqué.
    for caisse in LIVRES {
        let source =
            std::fs::read_to_string(crates_dir().join(caisse).join("plugin.toml")).unwrap();
        let manifeste = Manifest::from_toml(&source).unwrap();
        assert!(
            manifeste.permissions.network.is_empty(),
            "{caisse} asks for the network"
        );
    }
}
