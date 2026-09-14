//! Le plugin d'exemple, éprouvé de bout en bout.
//!
//! Ce test vaut plus qu'une démonstration : il vérifie que le contrat annoncé est
//! celui qui fonctionne réellement. Un exemple qui ne serait pas exécuté finirait par
//! diverger de l'hôte, et le premier auteur de plugin le découvrirait à ses dépens.

use iris_plugins::{entry_points, Manifest, Plugin, PluginRegistry, MANIFEST_FILE};
use std::path::PathBuf;

fn repertoire_exemple() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/marquer-infolettres")
}

/// Compile l'exemple et l'installe dans un répertoire temporaire, comme le ferait
/// une véritable installation.
fn installer() -> (tempfile::TempDir, PathBuf) {
    let source = repertoire_exemple();
    let wat = std::fs::read_to_string(source.join("plugin.wat")).expect("source du plugin");
    let wasm = wat::parse_str(&wat).expect("le plugin d'exemple doit compiler");

    let dir = tempfile::tempdir().unwrap();
    let cible = dir.path().join("marquer-infolettres");
    std::fs::create_dir_all(&cible).unwrap();
    std::fs::copy(source.join(MANIFEST_FILE), cible.join(MANIFEST_FILE)).unwrap();
    std::fs::write(cible.join("plugin.wasm"), wasm).unwrap();

    (dir, cible)
}

#[test]
fn le_manifeste_de_l_exemple_est_valide() {
    let source = std::fs::read_to_string(repertoire_exemple().join(MANIFEST_FILE)).unwrap();
    let manifeste = Manifest::from_toml(&source).expect("manifeste valide");

    assert_eq!(manifeste.id, "marquer-infolettres");
    assert!(manifeste.permissions.write_mail);
    assert!(
        manifeste.permissions.network.is_empty(),
        "l'exemple ne doit demander aucun accès réseau"
    );
}

#[test]
fn l_exemple_ne_demande_que_ce_dont_il_a_besoin() {
    // Chaque permission superflue est une raison d'hésiter au moment d'installer.
    let source = std::fs::read_to_string(repertoire_exemple().join(MANIFEST_FILE)).unwrap();
    let manifeste = Manifest::from_toml(&source).unwrap();
    let demandees = manifeste.permissions.describe();

    assert_eq!(demandees.len(), 2, "lire et modifier, rien de plus : {demandees:?}");
}

#[test]
fn l_exemple_marque_une_infolettre() {
    let (_dir, chemin) = installer();
    let mut registre = PluginRegistry::new();
    registre.load_one(&chemin).expect("chargement de l'exemple");

    // Un message qui propose un désabonnement : le plugin doit agir.
    let evenement = r#"{"thread":42,"subject":"Nos offres","headers":{"list-unsubscribe":"<https://x.fr>"}}"#;
    let resultats = registre.dispatch(entry_points::ON_EVENT, evenement);

    assert_eq!(resultats.len(), 1);
    let trace = resultats[0].1.as_ref().expect("appel réussi");
    assert_eq!(trace.actions, [r#"{"action":"done"}"#]);
    assert!(trace.denied.is_empty(), "toutes les permissions nécessaires sont accordées");
    assert_eq!(trace.logs.len(), 1);
}

#[test]
fn l_exemple_laisse_les_autres_messages_tranquilles() {
    let (_dir, chemin) = installer();
    let mut registre = PluginRegistry::new();
    registre.load_one(&chemin).unwrap();

    let evenement = r#"{"thread":7,"subject":"Devis refonte","headers":{"from":"marie@x.fr"}}"#;
    let resultats = registre.dispatch(entry_points::ON_EVENT, evenement);

    let trace = resultats[0].1.as_ref().unwrap();
    assert!(trace.actions.is_empty(), "un message ordinaire ne doit pas être touché");
    assert!(trace.logs.is_empty());
}

#[test]
fn l_exemple_tient_dans_son_budget_reduit() {
    // Le manifeste resserre volontairement le carburant : la vérification garantit
    // que ce réglage reste réaliste à mesure que le plugin évolue.
    let (_dir, chemin) = installer();
    let mut registre = PluginRegistry::new();
    registre.load_one(&chemin).unwrap();

    // Une charge longue, pour éprouver la recherche de sous-chaîne.
    let bourrage = "x".repeat(4_000);
    let evenement = format!(r#"{{"subject":"{bourrage}","h":"list-unsubscribe"}}"#);

    let resultats = registre.dispatch(entry_points::ON_EVENT, &evenement);
    assert!(resultats[0].1.is_ok(), "le budget doit suffire : {:?}", resultats[0].1);
}

#[test]
fn l_exemple_expose_les_points_d_entree_attendus() {
    let source = repertoire_exemple();
    let wat = std::fs::read_to_string(source.join("plugin.wat")).unwrap();
    let wasm = wat::parse_str(&wat).unwrap();

    let manifeste =
        Manifest::from_toml(&std::fs::read_to_string(source.join(MANIFEST_FILE)).unwrap())
            .unwrap();
    let mut plugin = Plugin::load(manifeste, &wasm).unwrap();

    assert!(plugin.call(entry_points::INIT, "{}").is_ok());
    assert!(plugin.call(entry_points::ON_EVENT, "{}").is_ok());
}

#[test]
fn le_contrat_wit_accompagne_l_hote() {
    // Le fichier de contrat est la référence lisible ; son absence ferait de
    // l'implémentation la seule documentation, ce qui est le meilleur moyen de la
    // voir diverger.
    let wit = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wit/iris.wit");
    let contenu = std::fs::read_to_string(&wit).expect("le contrat doit exister");

    for fonction in ["log:", "act:", "add-command:", "notify:", "on-event:"] {
        assert!(contenu.contains(fonction), "« {fonction} » manque au contrat");
    }
    assert!(contenu.contains("@1.0.0"), "le contrat doit être versionné");
}
