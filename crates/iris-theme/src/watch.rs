//! Rechargement à chaud des thèmes.
//!
//! Surveille le répertoire des thèmes de l'utilisateur et recharge le registre à
//! chaque modification. Deux précautions valent d'être expliquées :
//!
//! - **les événements sont regroupés** : un éditeur de texte produit trois à cinq
//!   événements pour un seul enregistrement (écriture, troncature, renommage du
//!   fichier temporaire). Recharger à chaque événement ferait clignoter l'interface ;
//! - **un thème invalide ne remplace rien** : on garde le précédent et on
//!   avertit. Une accolade oubliée pendant qu'on retouche une couleur ne doit pas
//!   laisser l'application sans thème.

use crate::ThemeRegistry;
use iris_kernel::{Event, EventBus};
use iris_types::{Error, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Fenêtre de regroupement des événements de fichier.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Surveille le répertoire des thèmes.
///
/// Le fil d'exécution s'arrête quand la structure est détruite.
#[derive(Debug)]
pub struct ThemeWatcher {
    _watcher: RecommendedWatcher,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl ThemeWatcher {
    /// Démarre la surveillance. Sans répertoire utilisateur, il n'y a rien à
    /// surveiller et la fonction retourne `None`.
    pub fn start(registry: Arc<ThemeRegistry>, bus: EventBus) -> Result<Option<Self>> {
        let Some(dir) = registry.user_dir().map(|d| d.to_path_buf()) else {
            return Ok(None);
        };
        std::fs::create_dir_all(&dir)?;

        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            // Une erreur de surveillance ne doit pas faire tomber l'application :
            // au pire, le rechargement à chaud cesse de fonctionner.
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        })
        .map_err(|e| Error::Config(format!("surveillance des thèmes : {e}")))?;

        watcher
            .watch(&dir, RecursiveMode::NonRecursive)
            .map_err(|e| Error::Config(format!("surveillance de {} : {e}", dir.display())))?;

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);

        std::thread::Builder::new()
            .name("iris-theme-watch".into())
            .spawn(move || {
                Self::run(rx, registry, bus, stop_thread);
            })
            .map_err(|e| Error::Config(format!("fil de surveillance : {e}")))?;

        Ok(Some(Self {
            _watcher: watcher,
            stop,
        }))
    }

    fn run(
        rx: mpsc::Receiver<notify::Event>,
        registry: Arc<ThemeRegistry>,
        bus: EventBus,
        stop: Arc<std::sync::atomic::AtomicBool>,
    ) {
        use std::sync::atomic::Ordering;

        while !stop.load(Ordering::Relaxed) {
            // Attente du premier événement, sans consommer de processeur.
            let Ok(_) = rx.recv_timeout(Duration::from_millis(500)) else {
                continue;
            };

            // Regroupement : on avale tout ce qui arrive dans la fenêtre.
            let echeance = Instant::now() + DEBOUNCE;
            while let Some(reste) = echeance.checked_duration_since(Instant::now()) {
                if rx.recv_timeout(reste).is_err() {
                    break;
                }
            }

            match registry.reload_user_themes() {
                Ok(charges) if !charges.is_empty() => {
                    let actif = registry.active().name.clone();
                    tracing::info!(?charges, "thèmes rechargés");
                    bus.publish(Event::ThemeReloaded {
                        name: Arc::from(actif.as_str()),
                    });
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "rechargement des thèmes"),
            }
        }
    }
}

impl Drop for ThemeWatcher {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_kernel::EventKind;

    #[test]
    fn sans_repertoire_utilisateur_il_n_y_a_rien_a_surveiller() {
        let registry = Arc::new(ThemeRegistry::builtin().unwrap());
        let w = ThemeWatcher::start(registry, EventBus::new()).unwrap();
        assert!(w.is_none());
    }

    #[test]
    fn une_modification_recharge_le_theme_et_publie() {
        let dir = tempfile::tempdir().unwrap();
        let fichier = dir.path().join("nuit.toml");
        std::fs::write(&fichier, "name = \"nuit\"\n[density]\nrow_height = 50.0\n").unwrap();

        let registry = Arc::new(ThemeRegistry::with_user_dir(dir.path()).unwrap());
        let bus = EventBus::new();
        let mut abonne = bus.subscribe_kind(EventKind::Presentation);

        let _watcher = ThemeWatcher::start(Arc::clone(&registry), bus)
            .unwrap()
            .unwrap();
        assert_eq!(registry.get("nuit").unwrap().density.row_height, 50.0);

        std::fs::write(&fichier, "name = \"nuit\"\n[density]\nrow_height = 30.0\n").unwrap();

        // La surveillance de fichiers dépend du système : on laisse largement le
        // temps à l'événement d'arriver plutôt que de supposer un délai.
        // L'annonce part *après* le rechargement, depuis le fil de surveillance : on
        // attend les deux, sans quoi le test lit le registre à jour et le bus encore
        // vide dans l'intervalle — ce qui arrivait sur une machine chargée.
        let echeance = Instant::now() + Duration::from_secs(10);
        let mut annonces = 0;
        while Instant::now() < echeance {
            annonces += abonne.drain().len();
            if registry.get("nuit").unwrap().density.row_height == 30.0 && annonces > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        assert_eq!(
            registry.get("nuit").unwrap().density.row_height,
            30.0,
            "la modification doit être prise en compte"
        );
        assert!(annonces > 0, "un rechargement doit être annoncé sur le bus");
    }

    #[test]
    fn un_theme_devenu_invalide_laisse_le_precedent_en_place() {
        let dir = tempfile::tempdir().unwrap();
        let fichier = dir.path().join("nuit.toml");
        std::fs::write(&fichier, "name = \"nuit\"\n[density]\nrow_height = 50.0\n").unwrap();

        let registry = Arc::new(ThemeRegistry::with_user_dir(dir.path()).unwrap());
        std::fs::write(&fichier, "name = = cassé").unwrap();
        registry.reload_user_themes().unwrap();

        assert_eq!(
            registry.get("nuit").unwrap().density.row_height,
            50.0,
            "une accolade oubliée ne doit pas laisser l'application sans thème"
        );
    }
}
