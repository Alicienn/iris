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
    /// Les fils cochés, pour agir sur plusieurs d'un coup.
    ///
    /// Séparé du fil courant, et pas confondu avec lui : la ligne courante décide de
    /// ce que montre le volet de lecture, les fils cochés décident de ce que subira
    /// la prochaine action. Les confondre voudrait dire qu'on ne peut pas lire un
    /// message sans le retirer d'une sélection en cours, ce qui est précisément ce
    /// qu'on veut faire avant de valider un lot.
    marked: std::collections::BTreeSet<ThreadId>,
    /// D'où part une extension au clavier ou à la souris.
    anchor: Option<ThreadId>,
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

    // --- La sélection multiple ---

    /// Les fils cochés, dans l'ordre.
    pub fn marked(&self) -> &std::collections::BTreeSet<ThreadId> {
        &self.marked
    }

    pub fn marked_count(&self) -> usize {
        self.marked.len()
    }

    pub fn is_marked(&self, thread: ThreadId) -> bool {
        self.marked.contains(&thread)
    }

    /// Coche ou décoche un fil, et en fait le point d'ancrage.
    pub fn toggle_mark(&mut self, thread: ThreadId) {
        if !self.marked.remove(&thread) {
            self.marked.insert(thread);
        }
        self.anchor = Some(thread);
    }

    /// Coche tout ce qui va de l'ancre jusqu'ici.
    ///
    /// Ajoute sans jamais retirer. Une extension qui décocherait au passage ferait
    /// perdre, d'un clic mal placé, une sélection construite en plusieurs gestes — et
    /// c'est le geste qu'on fait juste avant de supprimer cinquante messages.
    ///
    /// Sans ancre, se comporte comme un simple cochage : c'est ce qu'un `Maj+clic`
    /// dans une liste vierge peut vouloir dire de plus utile.
    pub fn extend_mark_to(&mut self, thread: ThreadId, rows: &[ThreadId]) {
        let Some(ancre) = self.anchor.or(self.thread) else {
            self.toggle_mark(thread);
            return;
        };

        let (Some(a), Some(b)) = (
            rows.iter().position(|t| *t == ancre),
            rows.iter().position(|t| *t == thread),
        ) else {
            // L'ancre a quitté la liste — triée, filtrée — et il n'y a plus
            // d'intervalle à décrire. Cocher la ligne visée reste juste.
            self.marked.insert(thread);
            return;
        };

        for fil in &rows[a.min(b)..=a.max(b)] {
            self.marked.insert(*fil);
        }
        self.anchor = Some(thread);
    }

    /// Coche tout ce qui est chargé.
    pub fn mark_all(&mut self, rows: &[ThreadId]) {
        self.marked.extend(rows.iter().copied());
    }

    /// Retourne `true` s'il y avait quelque chose à décocher.
    pub fn clear_marks(&mut self) -> bool {
        self.anchor = None;
        let avait = !self.marked.is_empty();
        self.marked.clear();
        avait
    }

    /// Retire de la sélection ce qui n'existe plus.
    ///
    /// Appelé quand la liste change : un fil traité disparaît, et le garder coché
    /// ferait porter l'action suivante sur des lignes que personne ne voit.
    pub fn retain_marks(&mut self, exists: impl Fn(ThreadId) -> bool) {
        self.marked.retain(|t| exists(*t));
    }

    /// Ce sur quoi la prochaine action doit porter.
    ///
    /// Les fils cochés s'il y en a, sinon la ligne courante. Une seule règle, partagée
    /// par le clavier, la barre d'outils et le menu contextuel : sans elle, chacun
    /// répondrait différemment à « archiver » selon l'endroit d'où on le demande.
    pub fn targets(&self) -> Vec<ThreadId> {
        if !self.marked.is_empty() {
            return self.marked.iter().copied().collect();
        }
        self.thread.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fils(ids: &[i64]) -> Vec<ThreadId> {
        ids.iter().map(|i| ThreadId(*i)).collect()
    }

    #[test]
    fn cocher_est_independant_de_la_ligne_courante() {
        // On doit pouvoir lire un message sans le retirer du lot qu'on prépare.
        let mut s = Selection::default();
        s.set(Some(ThreadId(1)));
        s.toggle_mark(ThreadId(2));
        s.set(Some(ThreadId(3)));

        assert_eq!(s.thread(), Some(ThreadId(3)));
        assert_eq!(s.marked_count(), 1);
        assert!(s.is_marked(ThreadId(2)));
    }

    #[test]
    fn cocher_deux_fois_decoche() {
        let mut s = Selection::default();
        s.toggle_mark(ThreadId(1));
        s.toggle_mark(ThreadId(1));
        assert_eq!(s.marked_count(), 0);
    }

    #[test]
    fn l_extension_prend_tout_l_intervalle() {
        let mut s = Selection::default();
        let liste = fils(&[1, 2, 3, 4, 5]);
        s.toggle_mark(ThreadId(2));
        s.extend_mark_to(ThreadId(4), &liste);

        assert_eq!(s.marked_count(), 3);
        for id in [2, 3, 4] {
            assert!(s.is_marked(ThreadId(id)), "{id} devrait être coché");
        }
    }

    #[test]
    fn l_extension_marche_vers_le_haut() {
        let mut s = Selection::default();
        let liste = fils(&[1, 2, 3, 4, 5]);
        s.toggle_mark(ThreadId(4));
        s.extend_mark_to(ThreadId(2), &liste);
        assert_eq!(s.marked_count(), 3);
    }

    #[test]
    fn l_extension_n_efface_jamais_ce_qui_etait_coche() {
        // Un Maj+clic mal placé ne doit pas détruire une sélection construite en
        // plusieurs gestes, juste avant une suppression de cinquante messages.
        let mut s = Selection::default();
        let liste = fils(&[1, 2, 3, 4, 5]);
        s.toggle_mark(ThreadId(1));
        s.toggle_mark(ThreadId(3));
        s.extend_mark_to(ThreadId(5), &liste);

        assert!(s.is_marked(ThreadId(1)), "le premier coché survit");
        assert_eq!(s.marked_count(), 4);
    }

    #[test]
    fn une_ancre_disparue_ne_fait_pas_perdre_le_clic() {
        let mut s = Selection::default();
        s.toggle_mark(ThreadId(99));
        s.extend_mark_to(ThreadId(2), &fils(&[1, 2, 3]));
        assert!(s.is_marked(ThreadId(2)));
    }

    #[test]
    fn la_cible_est_le_lot_quand_il_y_en_a_un() {
        // Une seule règle pour le clavier, la barre d'outils et le menu contextuel :
        // sans elle, « archiver » ne veut pas la même chose selon d'où on le demande.
        let mut s = Selection::default();
        s.set(Some(ThreadId(1)));
        assert_eq!(s.targets(), vec![ThreadId(1)]);

        s.toggle_mark(ThreadId(7));
        s.toggle_mark(ThreadId(8));
        assert_eq!(s.targets(), vec![ThreadId(7), ThreadId(8)]);
    }

    #[test]
    fn ce_qui_quitte_la_liste_est_decoche() {
        // Un fil traité disparaît ; le garder coché ferait porter l'action suivante
        // sur des lignes que personne ne voit.
        let mut s = Selection::default();
        s.toggle_mark(ThreadId(1));
        s.toggle_mark(ThreadId(2));
        s.retain_marks(|t| t == ThreadId(2));
        assert_eq!(s.targets(), vec![ThreadId(2)]);
    }

    #[test]
    fn tout_cocher_puis_tout_decocher() {
        let mut s = Selection::default();
        s.mark_all(&fils(&[1, 2, 3]));
        assert_eq!(s.marked_count(), 3);
        assert!(s.clear_marks());
        assert!(!s.clear_marks(), "rien à décocher la seconde fois");
    }

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
        assert_eq!(
            s.last_index(),
            42,
            "la position survit au changement de fil"
        );
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
