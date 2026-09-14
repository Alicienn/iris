//! Le journal, sur disque.
//!
//! Une application sans fenêtre de console n'a **aucun** moyen de dire ce qui lui est
//! arrivé. Le build de publication est compilé en `windows_subsystem = "windows"` et
//! en `panic = "abort"` : quand quelque chose se casse, le processus disparaît, et
//! tout ce qu'il reste est une ligne de l'observateur d'événements de Windows qui dit
//! `0xc0000409` et un décalage dans un binaire dont les symboles ont été retirés.
//! C'est-à-dire rien.
//!
//! Ce module écrit donc trois choses dans un fichier :
//!
//! 1. tout ce que `tracing` produit, à partir du niveau demandé ;
//! 2. **le message de panique et sa pile**, par un crochet installé avant tout le
//!    reste — c'est le seul enregistrement qui existera d'un arrêt brutal ;
//! 3. l'en-tête d'ouverture : version, date, chemins. Un journal qui ne dit pas de
//!    quelle version il parle ne sert à rien deux versions plus tard.
//!
//! Le fichier est **tourné à l'ouverture** et non par taille : une session est l'unité
//! qui a un sens quand on cherche « pourquoi ça a planté tout à l'heure ». Les cinq
//! dernières sessions sont conservées, ce qui couvre le temps qu'il faut à quelqu'un
//! pour venir le demander, sans occuper le disque indéfiniment.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Combien de sessions passées restent sur le disque.
const SESSIONS_GARDEES: usize = 5;

/// Le journal de la session en cours.
pub fn current(paths: &crate::paths::Paths) -> PathBuf {
    paths.cache.join("logs").join("iris.log")
}

/// Prépare le répertoire, décale les journaux précédents, ouvre celui-ci.
fn open(paths: &crate::paths::Paths) -> Option<std::fs::File> {
    let dossier = paths.cache.join("logs");
    std::fs::create_dir_all(&dossier).ok()?;

    let actuel = dossier.join("iris.log");
    rotate(&dossier, &actuel);

    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&actuel)
        .ok()
}

/// `iris.log` devient `iris.1.log`, et ainsi de suite.
///
/// Renommer plutôt que numéroter à l'écriture : le fichier que quelqu'un ouvrira
/// s'appelle toujours `iris.log`, et c'est le seul nom qu'on ait à lui donner.
fn rotate(dossier: &Path, actuel: &Path) {
    if !actuel.exists() {
        return;
    }

    let nom = |n: usize| dossier.join(format!("iris.{n}.log"));
    let _ = std::fs::remove_file(nom(SESSIONS_GARDEES));
    for n in (1..SESSIONS_GARDEES).rev() {
        let _ = std::fs::rename(nom(n), nom(n + 1));
    }
    let _ = std::fs::rename(actuel, nom(1));
}

/// Une destination qui écrit dans le fichier **et** sur la sortie d'erreur.
///
/// Les deux, parce que les deux ont un lecteur différent : la console sert pendant le
/// développement, le fichier sert le jour où l'utilisateur dit « ça a planté ». En
/// publication la console n'existe pas et l'écriture y est simplement perdue, ce qui
/// ne coûte rien.
struct Deux {
    fichier: Mutex<std::fs::File>,
}

impl Write for &Deux {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        if let Ok(mut f) = self.fichier.lock() {
            let _ = f.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Ok(mut f) = self.fichier.lock() {
            let _ = f.flush();
        }
        Ok(())
    }
}

/// Installe le journal et le crochet de panique.
///
/// Appelé le plus tôt possible : une panique pendant l'ouverture des services est
/// exactement le genre de chose qu'on veut voir écrite, et un crochet installé après
/// coup ne l'aurait pas vue.
pub fn install(paths: &crate::paths::Paths) {
    let filtre = std::env::var("IRIS_LOG").unwrap_or_else(|_| "warn,iris=info".into());

    match open(paths) {
        Some(fichier) => {
            let cible: &'static Deux = Box::leak(Box::new(Deux {
                fichier: Mutex::new(fichier),
            }));

            tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::new(&filtre))
                .with_target(false)
                .with_ansi(false)
                .with_writer(move || cible)
                .init();

            install_panic_hook(current(paths));
            entete(paths);
        }
        // Un disque plein ou un répertoire en lecture seule ne doit pas empêcher
        // l'application de démarrer : on perd le journal, pas le courrier.
        None => {
            tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::new(&filtre))
                .with_target(false)
                .init();
            tracing::warn!("no log file: the log directory could not be opened");
        }
    }
}

/// Ce qu'il faut savoir avant de lire la suite.
fn entete(paths: &crate::paths::Paths) {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        data = %paths.data.display(),
        cache = %paths.cache.display(),
        "Iris starting"
    );
}

/// Écrit la panique dans le journal avant que le processus ne s'en aille.
///
/// Écrit **directement dans le fichier**, sans passer par `tracing` : une panique peut
/// très bien survenir dans le souscripteur lui-même, ou pendant la fermeture, une fois
/// qu'il n'écoute plus. Le crochet précédent est appelé ensuite, pour ne rien retirer
/// à ce que fait déjà l'environnement d'exécution.
fn install_panic_hook(chemin: PathBuf) {
    let precedent = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        let message = info.payload().downcast_ref::<&str>().map_or_else(
            || {
                info.payload()
                    .downcast_ref::<String>()
                    .cloned()
                    .unwrap_or_else(|| "(payload not a string)".into())
            },
            |s| (*s).to_string(),
        );

        let lieu = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "(unknown location)".into());

        let pile = std::backtrace::Backtrace::force_capture();
        let fil = std::thread::current();
        let nom = fil.name().unwrap_or("(unnamed)").to_string();

        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&chemin)
        {
            let _ = writeln!(
                f,
                "\n=== PANIC ===\nthread : {nom}\nat     : {lieu}\nmessage: {message}\n\
                 backtrace:\n{pile}\n=== END PANIC ===\n"
            );
            let _ = f.flush();
        }

        precedent(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_rotation_decale_les_sessions() {
        let racine = tempfile::tempdir().unwrap();
        let dossier = racine.path();
        let actuel = dossier.join("iris.log");

        std::fs::write(&actuel, "session A").unwrap();
        rotate(dossier, &actuel);
        std::fs::write(&actuel, "session B").unwrap();
        rotate(dossier, &actuel);

        assert_eq!(
            std::fs::read_to_string(dossier.join("iris.1.log")).unwrap(),
            "session B",
            "la session précédente porte le numéro 1"
        );
        assert_eq!(
            std::fs::read_to_string(dossier.join("iris.2.log")).unwrap(),
            "session A"
        );
    }

    #[test]
    fn la_rotation_ne_garde_pas_tout() {
        // Un journal qui ne s'efface jamais finit par être le plus gros fichier de
        // l'application, et personne ne lit la trentième session en arrière.
        let racine = tempfile::tempdir().unwrap();
        let dossier = racine.path();
        let actuel = dossier.join("iris.log");

        for i in 0..12 {
            std::fs::write(&actuel, format!("session {i}")).unwrap();
            rotate(dossier, &actuel);
        }

        assert!(dossier.join(format!("iris.{SESSIONS_GARDEES}.log")).exists());
        assert!(!dossier
            .join(format!("iris.{}.log", SESSIONS_GARDEES + 1))
            .exists());
    }

    #[test]
    fn sans_journal_precedent_la_rotation_ne_fait_rien() {
        let racine = tempfile::tempdir().unwrap();
        rotate(racine.path(), &racine.path().join("iris.log"));
        assert!(!racine.path().join("iris.1.log").exists());
    }
}
