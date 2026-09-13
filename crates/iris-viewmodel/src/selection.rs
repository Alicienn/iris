//! La sélection.

use iris_types::ThreadId;

/// Un déplacement demandé au clavier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Movement {
    Next,
    Previous,
    PageDown,
    PageUp,
    First,
    Last,
}

impl Movement {
    /// Interprète une touche selon les conventions habituelles.
    ///
    /// Les touches vim sont reconnues en plus des flèches : le triage au clavier
    /// s'adresse d'abord à ceux qui gardent les mains sur les lettres.
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "Down" | "j" => Some(Self::Next),
            "Up" | "k" => Some(Self::Previous),
            "PageDown" => Some(Self::PageDown),
            "PageUp" => Some(Self::PageUp),
            "Home" | "g" => Some(Self::First),
            "End" | "G" => Some(Self::Last),
            _ => None,
        }
    }
}

/// La ligne courante.
///
/// Conserve la dernière position connue en plus du fil : quand le fil sélectionné
/// quitte la liste — parce qu'il vient d'être traité — c'est cette position qui
/// permet de reprendre au même endroit au lieu de tout désélectionner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    thread: Option<ThreadId>,
    last_index: usize,
}

impl Selection {
    pub fn thread(&self) -> Option<ThreadId> {
        self.thread
    }

    pub fn is_empty(&self) -> bool {
        self.thread.is_none()
    }

    pub fn last_index(&self) -> usize {
        self.last_index
    }

    /// Change la sélection. Retourne `true` si elle a bougé.
    pub fn set(&mut self, thread: Option<ThreadId>) -> bool {
        if self.thread == thread {
            return false;
        }
        self.thread = thread;
        true
    }

    /// Mémorise la position de la ligne sélectionnée.
    pub fn remember_index(&mut self, index: usize) {
        self.last_index = index;
    }

    pub fn clear(&mut self) -> bool {
        self.set(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn une_selection_neuve_est_vide() {
        let s = Selection::default();
        assert!(s.is_empty());
        assert_eq!(s.last_index(), 0);
    }

    #[test]
    fn changer_de_fil_est_signale() {
        let mut s = Selection::default();
        assert!(s.set(Some(ThreadId(1))));
        assert!(!s.set(Some(ThreadId(1))), "même fil, aucun changement");
        assert!(s.set(Some(ThreadId(2))));
        assert!(s.clear());
        assert!(!s.clear());
    }

    #[test]
    fn la_position_est_memorisee_independamment_du_fil() {
        let mut s = Selection::default();
        s.set(Some(ThreadId(7)));
        s.remember_index(42);

        s.set(Some(ThreadId(8)));
        assert_eq!(s.last_index(), 42, "la position survit au changement de fil");
    }

    #[test]
    fn les_touches_vim_sont_reconnues() {
        // Le triage au clavier s'adresse d'abord à ceux qui gardent les mains sur
        // les lettres.
        assert_eq!(Movement::from_key("j"), Some(Movement::Next));
        assert_eq!(Movement::from_key("k"), Some(Movement::Previous));
        assert_eq!(Movement::from_key("g"), Some(Movement::First));
        assert_eq!(Movement::from_key("G"), Some(Movement::Last));
    }

    #[test]
    fn les_fleches_le_sont_aussi() {
        assert_eq!(Movement::from_key("Down"), Some(Movement::Next));
        assert_eq!(Movement::from_key("Up"), Some(Movement::Previous));
        assert_eq!(Movement::from_key("PageDown"), Some(Movement::PageDown));
        assert_eq!(Movement::from_key("End"), Some(Movement::Last));
    }

    #[test]
    fn une_touche_inconnue_ne_deplace_rien() {
        assert_eq!(Movement::from_key("F13"), None);
        assert_eq!(Movement::from_key(""), None);
    }
}
