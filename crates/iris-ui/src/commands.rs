//! La palette de commandes.
//!
//! Une seule entrée pour tout ce que l'application sait faire. C'est ce qui autorise
//! une interface dépouillée : aucune fonction n'a besoin d'un bouton pour rester
//! atteignable, et l'utilisateur n'a rien à mémoriser d'autre que `Ctrl+K`.
//!
//! Le filtrage est **par sous-séquence**, pas par sous-chaîne : taper `mtr` doit
//! trouver « Marquer traité ». C'est la différence entre une palette qu'on utilise
//! et une palette qu'on referme.

use iris_viewmodel::Action;
use iris_types::WorkflowState;

/// Ce qu'une commande déclenche.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandKind {
    Thread(Action),
    SwitchTab(WorkflowState),
    SelectAccount(i64),
    UnifiedView,
    Undo,
    Search,
    AddAccount,
    Settings,
    Reload,
    Quit,
}

/// Une commande de la palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: String,
    pub label: String,
    pub shortcut: String,
    pub group: String,
    pub kind: CommandKind,
    /// La commande a besoin d'une conversation sélectionnée.
    pub needs_thread: bool,
}

impl Command {
    fn new(
        id: &str,
        label: &str,
        shortcut: &str,
        group: &str,
        kind: CommandKind,
        needs_thread: bool,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            shortcut: shortcut.into(),
            group: group.into(),
            kind,
            needs_thread,
        }
    }
}

/// Les commandes intégrées.
pub fn builtin_commands() -> Vec<Command> {
    vec![
        Command::new(
            "thread.done",
            "Marquer traité",
            "E",
            "Conversation",
            CommandKind::Thread(Action::Done),
            true,
        ),
        Command::new(
            "thread.todo",
            "Remettre à traiter",
            "U",
            "Conversation",
            CommandKind::Thread(Action::Todo),
            true,
        ),
        Command::new(
            "thread.waiting",
            "Mettre en attente",
            "W",
            "Conversation",
            CommandKind::Thread(Action::Waiting),
            true,
        ),
        Command::new(
            "thread.snooze.3h",
            "Reporter de 3 heures",
            "",
            "Conversation",
            CommandKind::Thread(Action::SnoozeHours(3)),
            true,
        ),
        Command::new(
            "thread.snooze.tomorrow",
            "Reporter à demain",
            "S",
            "Conversation",
            CommandKind::Thread(Action::SnoozeHours(24)),
            true,
        ),
        Command::new(
            "thread.snooze.week",
            "Reporter d'une semaine",
            "",
            "Conversation",
            CommandKind::Thread(Action::SnoozeHours(24 * 7)),
            true,
        ),
        Command::new(
            "thread.unsnooze",
            "Annuler le report",
            "",
            "Conversation",
            CommandKind::Thread(Action::Unsnooze),
            true,
        ),
        Command::new(
            "thread.read",
            "Marquer comme lu",
            "R",
            "Conversation",
            CommandKind::Thread(Action::MarkRead),
            true,
        ),
        Command::new(
            "thread.unread",
            "Marquer comme non lu",
            "Maj+R",
            "Conversation",
            CommandKind::Thread(Action::MarkUnread),
            true,
        ),
        Command::new(
            "thread.flag",
            "Épingler la conversation",
            "F",
            "Conversation",
            CommandKind::Thread(Action::ToggleFlag),
            true,
        ),
        Command::new("edit.undo", "Annuler", "Ctrl+Z", "Édition", CommandKind::Undo, false),
        Command::new(
            "view.todo",
            "Aller à : À traiter",
            "Ctrl+1",
            "Navigation",
            CommandKind::SwitchTab(WorkflowState::Todo),
            false,
        ),
        Command::new(
            "view.waiting",
            "Aller à : En attente",
            "Ctrl+2",
            "Navigation",
            CommandKind::SwitchTab(WorkflowState::Waiting),
            false,
        ),
        Command::new(
            "view.done",
            "Aller à : Traité",
            "Ctrl+3",
            "Navigation",
            CommandKind::SwitchTab(WorkflowState::Done),
            false,
        ),
        Command::new(
            "view.unified",
            "Vue unifiée",
            "",
            "Navigation",
            CommandKind::UnifiedView,
            false,
        ),
        Command::new("app.search", "Rechercher…", "Ctrl+F", "Application", CommandKind::Search, false),
        Command::new(
            "app.add-account",
            "Ajouter un compte…",
            "",
            "Application",
            CommandKind::AddAccount,
            false,
        ),
        Command::new("app.reload", "Synchroniser maintenant", "F5", "Application", CommandKind::Reload, false),
        Command::new("app.settings", "Réglages", "Ctrl+,", "Application", CommandKind::Settings, false),
        Command::new("app.quit", "Quitter", "Ctrl+Q", "Application", CommandKind::Quit, false),
    ]
}

/// Filtre et classe les commandes selon la saisie.
///
/// Les commandes inapplicables — celles qui exigent une conversation quand rien n'est
/// sélectionné — sont retirées plutôt que grisées : une palette n'a pas la place
/// d'expliquer pourquoi une entrée ne répond pas.
pub fn filter<'a>(
    commands: &'a [Command],
    query: &str,
    has_thread: bool,
) -> Vec<&'a Command> {
    let requete = query.trim().to_lowercase();

    let mut resultats: Vec<(&Command, i32)> = commands
        .iter()
        .filter(|c| has_thread || !c.needs_thread)
        .filter_map(|c| score(&c.label.to_lowercase(), &requete).map(|s| (c, s)))
        .collect();

    // Meilleur score d'abord, puis ordre alphabétique pour que la liste ne bouge pas
    // de façon imprévisible entre deux frappes.
    resultats.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.label.cmp(&b.0.label)));
    resultats.into_iter().map(|(c, _)| c).collect()
}

/// Score d'une correspondance par sous-séquence. `None` si le libellé ne correspond
/// pas du tout.
fn score(label: &str, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }

    let etiquette: Vec<char> = label.chars().collect();
    let recherche: Vec<char> = query.chars().collect();

    let mut score = 0;
    let mut position = 0;
    let mut precedent_consecutif = false;

    for c in recherche {
        // On avance dans le libellé jusqu'au prochain caractère correspondant.
        let trouve = etiquette[position..].iter().position(|e| *e == c)?;
        let absolu = position + trouve;

        score += if trouve == 0 && precedent_consecutif {
            // Caractères consécutifs : forte prime, c'est le signe d'un préfixe.
            10
        } else if absolu == 0 || etiquette[absolu - 1] == ' ' {
            // Début de mot : prime moyenne, c'est ce que cherchent les initiales.
            6
        } else {
            1
        };

        precedent_consecutif = trouve == 0;
        position = absolu + 1;
    }

    // À correspondance égale, le libellé le plus court est le plus probable.
    Some(score - (etiquette.len() as i32) / 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commandes() -> Vec<Command> {
        builtin_commands()
    }

    #[test]
    fn une_requete_vide_montre_tout() {
        let c = commandes();
        assert_eq!(filter(&c, "", true).len(), c.len());
    }

    #[test]
    fn sans_conversation_les_commandes_de_conversation_disparaissent() {
        // Les griser obligerait à expliquer pourquoi, et une palette n'en a pas la
        // place.
        let c = commandes();
        let visibles = filter(&c, "", false);
        assert!(visibles.iter().all(|x| !x.needs_thread));
        assert!(visibles.len() < c.len());
    }

    #[test]
    fn le_filtrage_est_par_sous_sequence() {
        // C'est la différence entre une palette qu'on utilise et une qu'on referme.
        let c = commandes();
        let r = filter(&c, "mtr", true);
        assert_eq!(r.first().map(|x| x.id.as_str()), Some("thread.done"));
    }

    #[test]
    fn les_initiales_sont_privilegiees() {
        let c = commandes();
        let r = filter(&c, "ma", true);
        assert!(r.first().unwrap().label.to_lowercase().starts_with("ma"));
    }

    #[test]
    fn une_recherche_sans_correspondance_ne_rend_rien() {
        let c = commandes();
        assert!(filter(&c, "zzzzz", true).is_empty());
    }

    #[test]
    fn le_filtrage_ignore_la_casse_et_les_espaces() {
        let c = commandes();
        assert_eq!(filter(&c, "  TRAITÉ ", true).len(), filter(&c, "traité", true).len());
    }

    #[test]
    fn l_ordre_est_stable_entre_deux_frappes() {
        // Une liste qui se réordonne de façon imprévisible fait cliquer à côté.
        let c = commandes();
        let a = filter(&c, "re", true);
        let b = filter(&c, "re", true);
        assert_eq!(
            a.iter().map(|x| &x.id).collect::<Vec<_>>(),
            b.iter().map(|x| &x.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn chaque_commande_a_un_identifiant_unique() {
        let c = commandes();
        let identifiants: std::collections::BTreeSet<_> = c.iter().map(|x| &x.id).collect();
        assert_eq!(identifiants.len(), c.len());
    }

    #[test]
    fn les_commandes_sont_groupees() {
        let c = commandes();
        let groupes: std::collections::BTreeSet<_> = c.iter().map(|x| x.group.as_str()).collect();
        assert!(groupes.contains("Conversation"));
        assert!(groupes.contains("Navigation"));
        assert!(groupes.contains("Application"));
    }

    #[test]
    fn les_reports_courants_sont_proposes() {
        let c = commandes();
        let reports: Vec<_> = c.iter().filter(|x| x.id.starts_with("thread.snooze")).collect();
        assert!(reports.len() >= 3, "trois horizons au moins : heures, demain, semaine");
    }

    #[test]
    fn le_score_recompense_les_caracteres_consecutifs() {
        let consecutif = score("marquer", "mar").unwrap();
        let disperse = score("marquer", "mqe").unwrap();
        assert!(consecutif > disperse);
    }

    #[test]
    fn le_score_est_absent_quand_les_lettres_manquent() {
        assert!(score("marquer traité", "xyz").is_none());
        // L'ordre compte : les lettres doivent apparaître dans le bon sens.
        assert!(score("abc", "cba").is_none());
    }
}
