//! Les raccourcis clavier.
//!
//! Les raccourcis sont des **données**, pas du code d'interface : c'est ce qui
//! permettra de les reconfigurer, et c'est ce qui les rend testables sans fenêtre.
//!
//! Le jeu par défaut suit les conventions de Gmail et de mutt, que la plupart des
//! gens qui trient au clavier connaissent déjà. Inventer les siennes n'apporte rien
//! et coûte un apprentissage.

use crate::commands::CommandKind;
use iris_types::WorkflowState;
use iris_viewmodel::{Action, Movement};

/// Ce qu'une touche déclenche.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    Move(Movement),
    Command(CommandKind),
    /// Ouvre la palette.
    OpenPalette,
    /// Aucune correspondance.
    Ignored,
}

/// Table des raccourcis.
#[derive(Debug, Clone)]
pub struct Keymap {
    bindings: Vec<(String, KeyOutcome)>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::standard()
    }
}

impl Keymap {
    /// Le jeu par défaut.
    pub fn standard() -> Self {
        let mut bindings: Vec<(String, KeyOutcome)> = Vec::new();

        let mut lier = |touche: &str, effet: KeyOutcome| {
            bindings.push((touche.to_string(), effet));
        };

        // Navigation, en flèches et en touches vim.
        for (touche, mouvement) in [
            ("j", Movement::Next),
            ("k", Movement::Previous),
            ("\u{f701}", Movement::Next),
            ("\u{f700}", Movement::Previous),
            ("g", Movement::First),
            ("G", Movement::Last),
        ] {
            lier(touche, KeyOutcome::Move(mouvement));
        }

        // Workflow. Les lettres reprennent celles de Gmail : « e » pour archiver y
        // signifie exactement ce que « traité » signifie ici.
        lier("e", KeyOutcome::Command(CommandKind::Thread(Action::Done)));
        lier("u", KeyOutcome::Command(CommandKind::Thread(Action::Todo)));
        lier(
            "w",
            KeyOutcome::Command(CommandKind::Thread(Action::Waiting)),
        );
        lier(
            "s",
            KeyOutcome::Command(CommandKind::Thread(Action::SnoozeHours(24))),
        );
        lier(
            "r",
            KeyOutcome::Command(CommandKind::Thread(Action::MarkRead)),
        );
        lier(
            "R",
            KeyOutcome::Command(CommandKind::Thread(Action::MarkUnread)),
        );
        lier(
            "f",
            KeyOutcome::Command(CommandKind::Thread(Action::ToggleFlag)),
        );

        // Onglets.
        lier(
            "1",
            KeyOutcome::Command(CommandKind::SwitchTab(WorkflowState::Todo)),
        );
        lier(
            "2",
            KeyOutcome::Command(CommandKind::SwitchTab(WorkflowState::Waiting)),
        );
        lier(
            "3",
            KeyOutcome::Command(CommandKind::SwitchTab(WorkflowState::Done)),
        );

        // Divers.
        lier("z", KeyOutcome::Command(CommandKind::Undo));
        lier("/", KeyOutcome::Command(CommandKind::Search));
        lier("?", KeyOutcome::OpenPalette);

        Self { bindings }
    }

    /// Table vide, pour un utilisateur qui préfère tout définir lui-même.
    pub fn empty() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }

    /// Ajoute ou remplace un raccourci.
    pub fn bind(&mut self, key: &str, outcome: KeyOutcome) {
        match self.bindings.iter_mut().find(|(k, _)| k == key) {
            Some(entree) => entree.1 = outcome,
            None => self.bindings.push((key.to_string(), outcome)),
        }
    }

    pub fn unbind(&mut self, key: &str) -> bool {
        let avant = self.bindings.len();
        self.bindings.retain(|(k, _)| k != key);
        self.bindings.len() != avant
    }

    /// Traduit une touche.
    pub fn resolve(&self, key: &str) -> KeyOutcome {
        // La correspondance est **sensible à la casse** : « r » et « R » sont deux
        // actions différentes, et les confondre marquerait comme non lu ce que
        // l'utilisateur voulait marquer comme lu.
        self.bindings
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, o)| o.clone())
            .unwrap_or(KeyOutcome::Ignored)
    }

    /// Touche associée à une action, pour l'afficher dans l'aide.
    pub fn shortcut_for(&self, outcome: &KeyOutcome) -> Option<&str> {
        self.bindings
            .iter()
            .find(|(_, o)| o == outcome)
            .map(|(k, _)| k.as_str())
    }

    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_touches_de_navigation_sont_liees() {
        let k = Keymap::standard();
        assert_eq!(k.resolve("j"), KeyOutcome::Move(Movement::Next));
        assert_eq!(k.resolve("k"), KeyOutcome::Move(Movement::Previous));
    }

    #[test]
    fn les_touches_de_workflow_suivent_les_conventions_connues() {
        let k = Keymap::standard();
        assert_eq!(
            k.resolve("e"),
            KeyOutcome::Command(CommandKind::Thread(Action::Done))
        );
        assert_eq!(
            k.resolve("s"),
            KeyOutcome::Command(CommandKind::Thread(Action::SnoozeHours(24)))
        );
    }

    #[test]
    fn la_casse_distingue_deux_actions_opposees() {
        // Les confondre marquerait comme non lu ce que l'utilisateur voulait marquer
        // comme lu.
        let k = Keymap::standard();
        assert_eq!(
            k.resolve("r"),
            KeyOutcome::Command(CommandKind::Thread(Action::MarkRead))
        );
        assert_eq!(
            k.resolve("R"),
            KeyOutcome::Command(CommandKind::Thread(Action::MarkUnread))
        );
    }

    #[test]
    fn une_touche_inconnue_est_ignoree() {
        let k = Keymap::standard();
        assert_eq!(k.resolve("²"), KeyOutcome::Ignored);
        assert_eq!(k.resolve(""), KeyOutcome::Ignored);
    }

    #[test]
    fn un_raccourci_peut_etre_remplace() {
        let mut k = Keymap::standard();
        k.bind("e", KeyOutcome::Command(CommandKind::Undo));
        assert_eq!(k.resolve("e"), KeyOutcome::Command(CommandKind::Undo));
    }

    #[test]
    fn remplacer_ne_duplique_pas_l_entree() {
        let mut k = Keymap::standard();
        let avant = k.len();
        k.bind("e", KeyOutcome::Command(CommandKind::Undo));
        assert_eq!(k.len(), avant);
    }

    #[test]
    fn un_raccourci_peut_etre_retire() {
        let mut k = Keymap::standard();
        assert!(k.unbind("e"));
        assert_eq!(k.resolve("e"), KeyOutcome::Ignored);
        assert!(!k.unbind("e"), "retirer deux fois ne fait rien");
    }

    #[test]
    fn une_table_vide_n_interprete_rien() {
        let k = Keymap::empty();
        assert!(k.is_empty());
        assert_eq!(k.resolve("j"), KeyOutcome::Ignored);
    }

    #[test]
    fn le_raccourci_d_une_action_est_retrouvable_pour_l_aide() {
        let k = Keymap::standard();
        let effet = KeyOutcome::Command(CommandKind::Thread(Action::Done));
        assert_eq!(k.shortcut_for(&effet), Some("e"));
    }

    #[test]
    fn aucune_touche_n_est_liee_deux_fois() {
        let k = Keymap::standard();
        let mut vues = std::collections::BTreeSet::new();
        for (touche, _) in &k.bindings {
            assert!(
                vues.insert(touche.clone()),
                "touche « {touche} » liée deux fois"
            );
        }
    }

    #[test]
    fn les_onglets_sont_accessibles_au_clavier() {
        let k = Keymap::standard();
        for (touche, etat) in [
            ("1", WorkflowState::Todo),
            ("2", WorkflowState::Waiting),
            ("3", WorkflowState::Done),
        ] {
            assert_eq!(
                k.resolve(touche),
                KeyOutcome::Command(CommandKind::SwitchTab(etat))
            );
        }
    }
}
