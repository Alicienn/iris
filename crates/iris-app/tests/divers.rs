//! Three small checks that once each had a test binary of its own, every one a full
//! link of the application: the window's callbacks are wired, the journal starts after
//! another logger, the software renderer is chosen before the first window.
//!
//! One binary now. The journal's test installs process-wide loggers (a `log` logger,
//! the `tracing` subscriber, a panic hook); nothing else here logs or installs one, so
//! it may run beside the others.

mod cablage {
    //! Chaque rappel déclaré par la fenêtre est-il relié à quelque chose ?
    //!
    //! Un rappel Slint sans gestionnaire ne fait rien, ne dit rien et ne casse rien : le
    //! bouton est là, il s'allume au survol, il s'enfonce au clic, et il n'arrive
    //! strictement rien. C'est la panne la plus difficile à voir de tout ce projet, parce
    //! qu'elle ressemble exactement à une fonction qui marche.
    //!
    //! Elle est arrivée au moins deux fois : le bouton « Show » du bandeau d'images
    //! bloquées, puis « Reply all », déclaré dans l'interface, transmis depuis la vue de
    //! conversation jusqu'à la fenêtre, et relié à rien. Les deux ont été signalés par
    //! l'utilisateur, des semaines plus tard.
    //!
    //! Le test lit les deux fichiers et compare. Ce n'est pas de l'analyse statique, c'est
    //! une lecture de texte — mais elle attrape exactement ce cas, elle coûte une
    //! milliseconde, et elle n'a besoin d'aucune fenêtre.

    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    fn racine() -> PathBuf {
        // `CARGO_MANIFEST_DIR` est `crates/iris-app`.
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("la racine du dépôt")
            .to_path_buf()
    }

    /// Les rappels que `AppWindow` déclare, dans l'ordre où on les lit.
    ///
    /// Seulement ceux du composant exporté : les rappels internes d'un composant sont
    /// reliés dans le même fichier `.slint`, et les chercher en Rust n'aurait pas de sens.
    fn rappels_de_la_fenetre(source: &str) -> BTreeSet<String> {
        let debut = source
            .find("export component AppWindow")
            .expect("AppWindow doit exister");

        source[debut..]
            .lines()
            .filter_map(|ligne| {
                let ligne = ligne.trim();
                let reste = ligne.strip_prefix("callback ")?;
                let nom: String = reste
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect();
                (!nom.is_empty()).then_some(nom)
            })
            .collect()
    }

    #[test]
    fn aucun_rappel_de_la_fenetre_ne_reste_sans_gestionnaire() {
        let racine = racine();
        let slint =
            std::fs::read_to_string(racine.join("crates/iris-ui/ui/app.slint")).expect("app.slint");

        // Tout le Rust qui branche la fenêtre. Les gestionnaires sont répartis entre le
        // module d'interface et le démarrage, et un rappel branché dans l'un ou l'autre
        // est branché.
        let rust: String = [
            "crates/iris-app/src/shell.rs",
            "crates/iris-app/src/main.rs",
            "crates/iris-app/src/gui.rs",
            "crates/iris-app/src/calendar.rs",
            "crates/iris-app/src/tasks.rs",
            "crates/iris-app/src/habits.rs",
            "crates/iris-app/src/tags.rs",
            "crates/iris-app/src/workspace.rs",
            "crates/iris-app/src/home.rs",
            "crates/iris-app/src/partout.rs",
            "crates/iris-app/src/nav.rs",
            "crates/iris-app/src/backup.rs",
            "crates/iris-app/src/caldav.rs",
            "crates/iris-app/src/devices.rs",
            "crates/iris-app/src/permissions.rs",
            "crates/iris-app/src/notes/mod.rs",
            "crates/iris-app/src/notes/saisie.rs",
        ]
        .iter()
        .map(|p| std::fs::read_to_string(racine.join(p)).unwrap_or_else(|e| panic!("{p}: {e}")))
        .collect();

        let mut orphelins: Vec<String> = Vec::new();

        for rappel in rappels_de_la_fenetre(&slint) {
            // Slint transforme les tirets en soulignés pour l'API Rust.
            let generateur = format!("on_{}", rappel.replace('-', "_"));
            // Posé directement — `on_x(` — ou par une macro qui reçoit le nom — `on_x,`.
            if rust.contains(&format!("{generateur}(")) || rust.contains(&format!("{generateur},"))
            {
                continue;
            }
            // Un rappel peut aussi être traité entièrement dans le `.slint` — ouvrir un
            // panneau, fermer un menu — auquel cas il y est affecté quelque part.
            if slint.contains(&format!("{rappel} =>")) || slint.contains(&format!("{rappel}=>")) {
                continue;
            }
            orphelins.push(rappel);
        }

        assert!(
            orphelins.is_empty(),
            "ces rappels sont déclarés et ne mènent nulle part — le bouton existe, \
             le clic ne fait rien : {orphelins:?}"
        );
    }

    #[test]
    fn le_test_reconnait_bien_un_rappel() {
        // Sans quoi il pourrait passer en ne trouvant jamais rien à vérifier, ce qui est
        // la façon dont ce genre de test s'éteint sans qu'on le remarque.
        let slint = std::fs::read_to_string(racine().join("crates/iris-ui/ui/app.slint")).unwrap();
        let rappels = rappels_de_la_fenetre(&slint);

        assert!(
            rappels.len() > 50,
            "la fenêtre en déclare une centaine ; en trouver {} veut dire que la \
             lecture ne marche plus",
            rappels.len()
        );
        // « reply-all », celui qui manquait, a laissé place à « reply-requested » (6.0.0).
        assert!(rappels.contains("reply-requested"), "celui qui répond");
    }
}

mod journal {
    //! The journal starts even when another logger is already there.
    //!
    //! On Android, `iris-android` installs Android's logger before Iris's journal; the
    //! journal's `init` then panicked ("Unable to install global subscriber:
    //! SetLoggerError"), and Iris closed at launch on every phone (6.4.0). The only test
    //! of this binary that installs process-wide loggers: keep it so.

    struct Muet;

    impl log::Log for Muet {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, _: &log::Record) {}
        fn flush(&self) {}
    }

    #[test]
    fn the_journal_starts_after_another_logger() {
        log::set_logger(&Muet).expect("the first logger");
        let dossier = tempfile::tempdir().unwrap();
        let chemins = iris_app::paths::Paths::under(dossier.path());
        // Panicked before the fix.
        iris_app::logging::install(&chemins);
        tracing::warn!("the journal writes");
    }
}

mod rendu {
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
            "the renderer is chosen before the first window, or Slint has already picked \
             OpenGL"
        );
    }
}
