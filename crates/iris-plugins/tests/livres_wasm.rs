//! Les trois modules livrés, éprouvés tels qu'ils seront installés.
//!
//! Les tests unitaires de chaque module vérifient sa décision — qui est important, à
//! quelle heure ouvre-t-on, où va une facture. Ils ne peuvent rien dire de ce qui les
//! entoure : l'allocateur linéaire du kit, les fonctions que l'hôte importe, la lecture
//! du JSON depuis la mémoire du bac à sable, et le budget de carburant que le manifeste
//! resserre. Tout cela ne se rompt que dans le vrai WebAssembly, et se rompt en
//! silence — un module qui échoue à l'initialisation est un module absent de l'écran
//! des modules, ce qu'on remarque des semaines plus tard.
//!
//! Ces tests-ci lisent donc les binaires **déjà compilés** par
//! `packaging\build-plugins.ps1`. Sans eux, ils s'abstiennent plutôt que d'échouer :
//! compiler vers wasm depuis un test demanderait une cible installée et une minute, et
//! un test qui construit son propre monde finit par éprouver le monde qu'il construit.

use iris_plugins::{entry_points, PluginRegistry};
use std::path::PathBuf;

/// Là où `build-plugins.ps1` dépose les modules, prêts à installer.
fn depot() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("la racine du dépôt")
        .join("packaging/plugins")
}

/// Charge un module livré, ou rend `None` s'il n'a pas été compilé.
fn charge(id: &str) -> Option<PluginRegistry> {
    let dir = depot().join(id);
    if !dir.join("plugin.wasm").exists() {
        eprintln!("ignoré : {id} n'est pas compilé (packaging\\build-plugins.ps1)");
        return None;
    }
    let mut registre = PluginRegistry::new();
    registre
        .load_one(&dir)
        .unwrap_or_else(|e| panic!("{id} refuse de se charger : {e}"));
    Some(registre)
}

/// Initialise avec des réglages, puis livre un événement, et rend les actions demandées.
fn essai(id: &str, reglages: &str, evenement: &str) -> Option<Vec<String>> {
    let mut registre = charge(id)?;

    for (_, resultat) in registre.dispatch_one(id, entry_points::INIT, reglages) {
        resultat.unwrap_or_else(|e| panic!("{id} échoue à l'initialisation : {e}"));
    }

    let resultats = registre.dispatch(entry_points::ON_EVENT, evenement);
    let trace = resultats[0]
        .1
        .as_ref()
        .unwrap_or_else(|e| panic!("{id} échoue sur l'événement : {e}"));

    assert!(
        trace.denied.is_empty(),
        "{id} : permission refusée {:?}",
        trace.denied
    );
    Some(trace.actions.clone())
}

/// Une charge d'événement de la forme que l'hôte compose réellement.
fn message(de: &str, sujet: &str, pieces: &str, heure: i64, jour: i64) -> String {
    format!(
        r#"{{"event":"message-added","thread":1,"de":"{de}","sujet":"{sujet}",
           "etiquettes":["unread"],"pieces":[{pieces}],"recu":0,"heure":{heure},"jour":{jour}}}"#
    )
}

#[test]
fn vip_epingle_et_remet_a_traiter() {
    let Some(actions) = essai(
        "vip",
        r#"{"people":"marie@client.fr\n@direction.example"}"#,
        &message("marie@client.fr", "Le devis", "", 10, 0),
    ) else {
        return;
    };

    assert_eq!(
        actions,
        [r#"{"action":"star"}"#, r#"{"action":"todo"}"#],
        "épingler d'abord : un message épinglé mais resté classé se retrouve, \
         un message ramené dans la file sans marque se noie"
    );
}

#[test]
fn vip_laisse_les_autres_tranquilles() {
    let Some(actions) = essai(
        "vip",
        r#"{"people":"marie@client.fr"}"#,
        &message("infolettre@boutique.com", "Nos offres", "", 10, 0),
    ) else {
        return;
    };
    assert!(actions.is_empty());
}

#[test]
fn vip_sans_liste_ne_fait_rien() {
    // Le module tel qu'il sort de l'installateur.
    let Some(actions) = essai(
        "vip",
        r#"{"people":""}"#,
        &message("marie@client.fr", "Le devis", "", 10, 0),
    ) else {
        return;
    };
    assert!(actions.is_empty());
}

#[test]
fn office_hours_reporte_le_soir() {
    let reglages = r#"{"enabled":"true","start":"8","end":"18","weekend":"true"}"#;
    let Some(actions) = essai(
        "office-hours",
        reglages,
        // Vingt-trois heures un lundi : une heure jusqu'à minuit, puis huit.
        &message("qui@que.ce", "Tard", "", 23, 0),
    ) else {
        return;
    };
    assert_eq!(actions, [r#"{"action":"snooze","hours":9}"#]);
}

#[test]
fn office_hours_ne_touche_pas_aux_heures_ouvrees() {
    let reglages = r#"{"enabled":"true","start":"8","end":"18","weekend":"true"}"#;
    let Some(actions) = essai(
        "office-hours",
        reglages,
        &message("qui@que.ce", "À l'heure", "", 10, 0),
    ) else {
        return;
    };
    assert!(actions.is_empty());
}

#[test]
fn office_hours_eteint_ne_reporte_rien() {
    // Le module tel qu'il sort de l'installateur : ses horaires sont plausibles, son
    // interrupteur est fermé.
    let Some(actions) = essai(
        "office-hours",
        r#"{"enabled":"false","start":"8","end":"18","weekend":"true"}"#,
        &message("qui@que.ce", "Tard", "", 23, 0),
    ) else {
        return;
    };
    assert!(actions.is_empty());
}

#[test]
fn filer_range_sur_le_nom_d_une_piece_jointe() {
    let Some(actions) = essai(
        "filer",
        r#"{"rules":"facture -> Comptabilité"}"#,
        &message("compta@x.fr", "Bonjour", r#""facture-2026-03.pdf""#, 10, 0),
    ) else {
        return;
    };
    assert_eq!(actions, [r#"{"action":"move","folder":"Comptabilité"}"#]);
}

#[test]
fn filer_sans_regle_ne_range_rien() {
    let Some(actions) = essai(
        "filer",
        r#"{"rules":""}"#,
        &message("compta@x.fr", "Votre facture", r#""facture.pdf""#, 10, 0),
    ) else {
        return;
    };
    assert!(actions.is_empty());
}

#[test]
fn un_module_survit_a_une_longue_journee() {
    // Deux mille messages, sur la même instance : ce que voit une boîte occupée en une
    // journée, et ce qu'aucun test unitaire ne peut voir.
    //
    // L'instance d'un module vit désormais d'un appel au suivant — sans quoi il perdrait
    // ses réglages à chaque message. Le prix est que son tas, qui ne se libère jamais,
    // s'épuiserait après quelques centaines de charges. La fonction `iris_reset` du kit
    // est ce qui l'en empêche, et voici la seule chose qui le prouve : sans elle, ce
    // test échoue vers le trois centième message, et le module se désactive.
    let Some(mut registre) = charge("filer") else {
        return;
    };

    for (_, r) in registre.dispatch_one("filer", entry_points::INIT, r#"{"rules":"facture -> C"}"#)
    {
        r.expect("initialisation");
    }

    for i in 0..2_000 {
        let charge = message("a@b.c", &format!("message numéro {i}"), r#""p.pdf""#, 10, 0);
        let resultats = registre.dispatch(entry_points::ON_EVENT, &charge);
        resultats[0]
            .1
            .as_ref()
            .unwrap_or_else(|e| panic!("filer lâche au message {i} : {e}"));
    }

    // Et il trie toujours, deux mille messages plus tard : le rappel de mémoire n'a pas
    // emporté les règles lues à l'initialisation.
    let charge = message("a@b.c", "Votre facture", "", 10, 0);
    let trace = registre.dispatch(entry_points::ON_EVENT, &charge);
    assert_eq!(
        trace[0].1.as_ref().unwrap().actions,
        [r#"{"action":"move","folder":"C"}"#]
    );
}

#[test]
fn les_budgets_declares_suffisent_a_une_charge_longue() {
    // Le carburant de chaque manifeste est resserré exprès. La vérification est qu'il
    // reste réaliste : un budget trop court ne se voit pas en test unitaire, il se voit
    // le jour où quelqu'un reçoit un sujet de quatre mille caractères et où le module
    // se désactive tout seul après trois échecs.
    let bourrage = "x".repeat(4_000);
    let pieces = (0..20)
        .map(|i| format!(r#""piece-{i}.pdf""#))
        .collect::<Vec<_>>()
        .join(",");

    let regles: String = (0..30)
        .map(|i| format!("motif{i} -> Dossier{i}\\n"))
        .collect();

    for (id, reglages) in [
        ("vip", r#"{"people":"a@b.c\nd@e.f\n@g.h"}"#.to_string()),
        (
            "office-hours",
            r#"{"enabled":"true","start":"8","end":"18"}"#.to_string(),
        ),
        ("filer", format!(r#"{{"rules":"{regles}"}}"#)),
    ] {
        // Rien ne doit correspondre : c'est le cas le plus cher, celui où toutes les
        // règles sont essayées jusqu'à la dernière.
        essai(id, &reglages, &message("z@z.z", &bourrage, &pieces, 10, 0));
    }
}
