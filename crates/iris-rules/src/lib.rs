//! `iris-rules` — le moteur de règles.
//!
//! Deux partis pris rendent ces règles utilisables par un humain normal :
//!
//! - **elles sont lisibles** : un petit nombre de conditions explicites, aucune
//!   expression régulière imposée, aucun langage à apprendre ;
//! - **elles se simulent avant de s'appliquer.** Personne n'ose écrire une règle qui
//!   archive du courrier sans savoir ce qu'elle va toucher. La simulation à blanc sur
//!   l'historique — « cette règle aurait concerné 1 240 messages, en voici vingt » —
//!   est ce qui transforme une fonctionnalité anxiogène en outil de confiance.
//!
//! La crate est **pure** : elle décide, elle n'agit pas. L'application des actions
//! revient à l'appelant, qui seul sait écrire dans le store.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

use iris_types::{Flags, Timestamp, WorkflowState};
use serde::{Deserialize, Serialize};

/// Les faits observables d'un message, sur lesquels portent les conditions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Facts {
    pub from_addr: String,
    pub from_name: String,
    pub subject: String,
    /// Destinataires concaténés, en minuscules.
    pub recipients: String,
    pub flags: Flags,
    pub size: u64,
    pub received: Timestamp,
    /// L'utilisateur a déjà répondu à cet expéditeur.
    pub known_correspondent: bool,
    /// Nom du dossier d'origine.
    pub folder: String,
}

impl Facts {
    pub fn domain(&self) -> &str {
        match self.from_addr.rfind('@') {
            Some(i) => &self.from_addr[i + 1..],
            None => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "value")]
pub enum Condition {
    FromContains(String),
    /// Le domaine exact, ou un sous-domaine de celui-ci.
    FromDomain(String),
    SubjectContains(String),
    RecipientContains(String),
    FolderIs(String),
    HasAttachment,
    IsUnsubscribable,
    HasTracker,
    IsUnread,
    /// L'utilisateur a déjà répondu à cet expéditeur.
    KnownCorrespondent,
    /// Plus ancien que N jours, relativement à l'instant d'évaluation.
    OlderThanDays(u16),
    /// Taille supérieure à N kilooctets.
    LargerThanKb(u64),
    /// Inverse la condition qu'elle contient.
    Not(Box<Condition>),
}

impl Condition {
    /// Évalue la condition. `now` sert aux conditions temporelles.
    pub fn matches(&self, f: &Facts, now: Timestamp) -> bool {
        match self {
            Self::FromContains(s) => contains_ci(&f.from_addr, s) || contains_ci(&f.from_name, s),
            Self::FromDomain(d) => {
                let domaine = f.domain().to_lowercase();
                let attendu = d.trim().trim_start_matches('.').to_lowercase();
                // Un sous-domaine correspond, mais « notexample.com » ne doit pas
                // correspondre à « example.com ».
                domaine == attendu || domaine.ends_with(&format!(".{attendu}"))
            }
            Self::SubjectContains(s) => contains_ci(&f.subject, s),
            Self::RecipientContains(s) => contains_ci(&f.recipients, s),
            Self::FolderIs(name) => f.folder.eq_ignore_ascii_case(name),
            Self::HasAttachment => f.flags.contains(Flags::HAS_ATTACHMENT),
            Self::IsUnsubscribable => f.flags.contains(Flags::UNSUBSCRIBABLE),
            Self::HasTracker => f.flags.contains(Flags::HAS_TRACKER),
            Self::IsUnread => !f.flags.contains(Flags::SEEN),
            Self::KnownCorrespondent => f.known_correspondent,
            Self::OlderThanDays(d) => {
                let seuil = now.millis() - (*d as i64) * 86_400_000;
                f.received.millis() < seuil
            }
            Self::LargerThanKb(kb) => f.size > kb * 1024,
            Self::Not(inner) => !inner.matches(f, now),
        }
    }

    /// Description en français, pour l'éditeur de règles.
    pub fn describe(&self) -> String {
        match self {
            Self::FromContains(s) => format!("l'expéditeur contient « {s} »"),
            Self::FromDomain(d) => format!("le domaine est « {d} »"),
            Self::SubjectContains(s) => format!("le sujet contient « {s} »"),
            Self::RecipientContains(s) => format!("un destinataire contient « {s} »"),
            Self::FolderIs(n) => format!("le dossier est « {n} »"),
            Self::HasAttachment => "le message a une pièce jointe".into(),
            Self::IsUnsubscribable => "le message propose un désabonnement".into(),
            Self::HasTracker => "le message contient un traqueur".into(),
            Self::IsUnread => "le message n'est pas lu".into(),
            Self::KnownCorrespondent => "vous avez déjà répondu à cet expéditeur".into(),
            Self::OlderThanDays(d) => format!("le message a plus de {d} jours"),
            Self::LargerThanKb(kb) => format!("le message dépasse {kb} ko"),
            Self::Not(inner) => format!("non ({})", inner.describe()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "value")]
pub enum Action {
    SetState(WorkflowState),
    MarkSeen,
    Flag,
    SnoozeDays(u16),
    MoveTo(String),
    Notify,
}

impl Action {
    pub fn describe(&self) -> String {
        match self {
            Self::SetState(s) => match s {
                WorkflowState::Todo => "mettre à traiter".into(),
                WorkflowState::Waiting => "mettre en attente".into(),
                WorkflowState::Done => "marquer traité".into(),
            },
            Self::MarkSeen => "marquer comme lu".into(),
            Self::Flag => "épingler".into(),
            Self::SnoozeDays(d) => format!("reporter de {d} jours"),
            Self::MoveTo(f) => format!("déplacer vers « {f} »"),
            Self::Notify => "notifier".into(),
        }
    }
}

/// Comment combiner les conditions d'une règle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Match {
    /// Toutes les conditions doivent être vraies.
    All,
    /// Au moins une condition doit être vraie.
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub name: String,
    #[serde(default = "vrai")]
    pub enabled: bool,
    pub match_mode: Match,
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
    /// Arrêter l'évaluation des règles suivantes si celle-ci s'applique.
    #[serde(default)]
    pub stop_on_match: bool,
}

fn vrai() -> bool {
    true
}

impl Rule {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            enabled: true,
            match_mode: Match::All,
            conditions: Vec::new(),
            actions: Vec::new(),
            stop_on_match: false,
        }
    }

    pub fn when(mut self, c: Condition) -> Self {
        self.conditions.push(c);
        self
    }

    pub fn then(mut self, a: Action) -> Self {
        self.actions.push(a);
        self
    }

    pub fn matching_any(mut self) -> Self {
        self.match_mode = Match::Any;
        self
    }

    pub fn stopping(mut self) -> Self {
        self.stop_on_match = true;
        self
    }

    /// La règle s'applique-t-elle à ce message ?
    ///
    /// Une règle **sans condition ne s'applique jamais**. C'est délibéré : une règle
    /// vide qui s'appliquerait à tout est le moyen le plus rapide de détruire une
    /// boîte, et l'utilisateur qui vient de créer une règle n'a encore rien écrit.
    pub fn matches(&self, f: &Facts, now: Timestamp) -> bool {
        if !self.enabled || self.conditions.is_empty() {
            return false;
        }
        match self.match_mode {
            Match::All => self.conditions.iter().all(|c| c.matches(f, now)),
            Match::Any => self.conditions.iter().any(|c| c.matches(f, now)),
        }
    }

    pub fn describe(&self) -> String {
        let liaison = match self.match_mode {
            Match::All => " et ",
            Match::Any => " ou ",
        };
        let conditions = self
            .conditions
            .iter()
            .map(Condition::describe)
            .collect::<Vec<_>>()
            .join(liaison);
        let actions =
            self.actions.iter().map(Action::describe).collect::<Vec<_>>().join(", puis ");
        format!("Si {conditions}, alors {actions}.")
    }
}

/// Le verdict de l'évaluation pour un message.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Verdict {
    /// Actions à appliquer, dans l'ordre, sans doublon.
    pub actions: Vec<Action>,
    /// Identifiants des règles ayant agi.
    pub matched: Vec<String>,
    /// Une règle a interrompu l'évaluation.
    pub stopped: bool,
}

impl Verdict {
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}

/// Évalue une liste de règles, dans l'ordre, sur un message.
pub fn evaluate(rules: &[Rule], f: &Facts, now: Timestamp) -> Verdict {
    let mut verdict = Verdict::default();

    for rule in rules {
        if !rule.matches(f, now) {
            continue;
        }
        verdict.matched.push(rule.id.clone());
        for action in &rule.actions {
            // Une même action demandée par deux règles ne doit pas être appliquée
            // deux fois — reporter deux fois de trois jours en ferait six.
            if !verdict.actions.contains(action) {
                verdict.actions.push(action.clone());
            }
        }
        if rule.stop_on_match {
            verdict.stopped = true;
            break;
        }
    }

    verdict
}

/// Un exemple retenu pour la simulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleHit {
    pub index: usize,
    pub from: String,
    pub subject: String,
    pub actions: Vec<Action>,
}

/// Résultat d'une simulation à blanc.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Simulation {
    /// Nombre de messages examinés.
    pub examined: usize,
    /// Nombre de messages qu'au moins une règle aurait touchés.
    pub affected: usize,
    /// Nombre de messages touchés, par règle.
    pub per_rule: Vec<(String, usize)>,
    /// Échantillon de messages concernés, pour montrer à l'utilisateur.
    pub sample: Vec<SampleHit>,
}

impl Simulation {
    /// Résumé en une phrase, tel qu'affiché avant application.
    pub fn summary(&self) -> String {
        match self.affected {
            0 => format!("Aucun des {} messages examinés ne serait touché.", self.examined),
            1 => format!("1 message sur {} serait touché.", self.examined),
            n => format!("{n} messages sur {} seraient touchés.", self.examined),
        }
    }
}

/// Simule l'application des règles sur un échantillon d'historique, sans rien
/// modifier.
///
/// `sample_size` borne le nombre d'exemples conservés : sur un million de messages,
/// on veut le compte exact et vingt exemples, pas un million de lignes.
pub fn simulate(
    rules: &[Rule],
    messages: &[Facts],
    now: Timestamp,
    sample_size: usize,
) -> Simulation {
    let mut sim = Simulation { examined: messages.len(), ..Default::default() };
    let mut compteurs: Vec<(String, usize)> =
        rules.iter().map(|r| (r.id.clone(), 0)).collect();

    for (index, facts) in messages.iter().enumerate() {
        let verdict = evaluate(rules, facts, now);
        if verdict.is_empty() && verdict.matched.is_empty() {
            continue;
        }

        sim.affected += 1;
        for id in &verdict.matched {
            if let Some(c) = compteurs.iter_mut().find(|(rid, _)| rid == id) {
                c.1 += 1;
            }
        }

        if sim.sample.len() < sample_size {
            sim.sample.push(SampleHit {
                index,
                from: facts.from_addr.clone(),
                subject: facts.subject.clone(),
                actions: verdict.actions.clone(),
            });
        }
    }

    sim.per_rule = compteurs;
    sim
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            from_addr: "newsletter@boutique.example.com".into(),
            from_name: "Boutique".into(),
            subject: "Nos offres du mois".into(),
            recipients: "moi@example.com".into(),
            flags: Flags::UNSUBSCRIBABLE,
            size: 40 * 1024,
            received: Timestamp::from_millis(1_000_000_000_000),
            known_correspondent: false,
            folder: "INBOX".into(),
        }
    }

    fn now() -> Timestamp {
        Timestamp::from_millis(1_000_000_000_000)
    }

    #[test]
    fn une_condition_de_domaine_accepte_les_sous_domaines() {
        let f = facts();
        assert!(Condition::FromDomain("example.com".into()).matches(&f, now()));
        assert!(Condition::FromDomain("boutique.example.com".into()).matches(&f, now()));
    }

    #[test]
    fn un_domaine_voisin_ne_correspond_pas() {
        // « notexample.com » ne doit pas satisfaire « example.com ».
        let mut f = facts();
        f.from_addr = "x@notexample.com".into();
        assert!(!Condition::FromDomain("example.com".into()).matches(&f, now()));
    }

    #[test]
    fn les_comparaisons_textuelles_ignorent_la_casse() {
        let f = facts();
        assert!(Condition::SubjectContains("OFFRES".into()).matches(&f, now()));
        assert!(Condition::FromContains("BOUTIQUE".into()).matches(&f, now()));
    }

    #[test]
    fn une_recherche_vide_ne_correspond_a_rien() {
        // Sinon une condition à moitié saisie s'appliquerait à toute la boîte.
        let f = facts();
        assert!(!Condition::SubjectContains(String::new()).matches(&f, now()));
    }

    #[test]
    fn les_conditions_sur_drapeaux() {
        let mut f = facts();
        assert!(Condition::IsUnsubscribable.matches(&f, now()));
        assert!(Condition::IsUnread.matches(&f, now()));
        assert!(!Condition::HasAttachment.matches(&f, now()));

        f.flags = Flags::SEEN | Flags::HAS_ATTACHMENT;
        assert!(!Condition::IsUnread.matches(&f, now()));
        assert!(Condition::HasAttachment.matches(&f, now()));
    }

    #[test]
    fn la_condition_temporelle_est_relative_a_l_instant_donne() {
        let f = facts();
        let dans_dix_jours = Timestamp::from_millis(now().millis() + 10 * 86_400_000);
        assert!(!Condition::OlderThanDays(30).matches(&f, dans_dix_jours));
        assert!(Condition::OlderThanDays(5).matches(&f, dans_dix_jours));
    }

    #[test]
    fn la_negation_inverse_la_condition() {
        let f = facts();
        let c = Condition::Not(Box::new(Condition::HasAttachment));
        assert!(c.matches(&f, now()));
        assert!(c.describe().starts_with("non ("));
    }

    #[test]
    fn le_mode_toutes_exige_chaque_condition() {
        let f = facts();
        let r = Rule::new("r", "Test")
            .when(Condition::IsUnsubscribable)
            .when(Condition::HasAttachment);
        assert!(!r.matches(&f, now()));
    }

    #[test]
    fn le_mode_une_suffit() {
        let f = facts();
        let r = Rule::new("r", "Test")
            .matching_any()
            .when(Condition::IsUnsubscribable)
            .when(Condition::HasAttachment);
        assert!(r.matches(&f, now()));
    }

    #[test]
    fn une_regle_sans_condition_ne_s_applique_jamais() {
        // Protection délibérée : une règle vide qui toucherait tout est le moyen le
        // plus rapide de détruire une boîte.
        let f = facts();
        let r = Rule::new("vide", "Vide").then(Action::SetState(WorkflowState::Done));
        assert!(!r.matches(&f, now()));
        assert!(evaluate(&[r], &f, now()).is_empty());
    }

    #[test]
    fn une_regle_desactivee_ne_s_applique_pas() {
        let f = facts();
        let mut r = Rule::new("r", "Test").when(Condition::IsUnsubscribable);
        r.enabled = false;
        assert!(!r.matches(&f, now()));
    }

    #[test]
    fn les_regles_s_evaluent_dans_l_ordre() {
        let f = facts();
        let regles = vec![
            Rule::new("a", "A").when(Condition::IsUnsubscribable).then(Action::MarkSeen),
            Rule::new("b", "B")
                .when(Condition::FromDomain("example.com".into()))
                .then(Action::SetState(WorkflowState::Done)),
        ];
        let v = evaluate(&regles, &f, now());
        assert_eq!(v.matched, ["a", "b"]);
        assert_eq!(v.actions, [Action::MarkSeen, Action::SetState(WorkflowState::Done)]);
    }

    #[test]
    fn une_regle_bloquante_interrompt_la_suite() {
        let f = facts();
        let regles = vec![
            Rule::new("a", "A")
                .when(Condition::IsUnsubscribable)
                .then(Action::MarkSeen)
                .stopping(),
            Rule::new("b", "B")
                .when(Condition::FromDomain("example.com".into()))
                .then(Action::Notify),
        ];
        let v = evaluate(&regles, &f, now());
        assert_eq!(v.matched, ["a"]);
        assert!(v.stopped);
        assert!(!v.actions.contains(&Action::Notify));
    }

    #[test]
    fn une_action_demandee_deux_fois_n_est_appliquee_qu_une_fois() {
        // Deux reports de trois jours ne doivent pas en faire six.
        let f = facts();
        let regles = vec![
            Rule::new("a", "A").when(Condition::IsUnsubscribable).then(Action::SnoozeDays(3)),
            Rule::new("b", "B").when(Condition::IsUnread).then(Action::SnoozeDays(3)),
        ];
        let v = evaluate(&regles, &f, now());
        assert_eq!(v.actions, [Action::SnoozeDays(3)]);
        assert_eq!(v.matched.len(), 2);
    }

    #[test]
    fn la_simulation_compte_sans_rien_modifier() {
        let regles =
            vec![Rule::new("news", "Infolettres")
                .when(Condition::IsUnsubscribable)
                .then(Action::SetState(WorkflowState::Done))];

        let mut messages = Vec::new();
        for i in 0..100 {
            let mut f = facts();
            if i % 4 != 0 {
                f.flags = Flags::NONE;
            }
            f.subject = format!("Message {i}");
            messages.push(f);
        }

        let sim = simulate(&regles, &messages, now(), 5);
        assert_eq!(sim.examined, 100);
        assert_eq!(sim.affected, 25);
        assert_eq!(sim.per_rule, [("news".to_string(), 25)]);
        assert_eq!(sim.sample.len(), 5, "l'échantillon est borné");
        assert!(sim.summary().contains("25 messages sur 100"));
    }

    #[test]
    fn une_simulation_sans_correspondance_le_dit_clairement() {
        let regles = vec![Rule::new("r", "R").when(Condition::HasAttachment)];
        let sim = simulate(&regles, &[facts()], now(), 10);
        assert_eq!(sim.affected, 0);
        assert!(sim.summary().starts_with("Aucun"));
    }

    #[test]
    fn une_simulation_sur_un_seul_message_s_accorde_au_singulier() {
        let regles = vec![Rule::new("r", "R").when(Condition::IsUnsubscribable)];
        let sim = simulate(&regles, &[facts()], now(), 10);
        assert_eq!(sim.summary(), "1 message sur 1 serait touché.");
    }

    #[test]
    fn la_description_d_une_regle_est_lisible() {
        let r = Rule::new("r", "R")
            .when(Condition::FromDomain("boutique.fr".into()))
            .when(Condition::IsUnsubscribable)
            .then(Action::SetState(WorkflowState::Done));
        let d = r.describe();
        assert!(d.starts_with("Si le domaine est « boutique.fr » et le message propose"));
        assert!(d.ends_with("alors marquer traité."));
    }

    #[test]
    fn une_regle_survit_a_un_aller_retour_json() {
        let r = Rule::new("r", "Infolettres")
            .when(Condition::FromDomain("boutique.fr".into()))
            .when(Condition::Not(Box::new(Condition::HasAttachment)))
            .then(Action::SnoozeDays(7))
            .stopping();
        let json = serde_json::to_string(&r).unwrap();
        let relu: Rule = serde_json::from_str(&json).unwrap();
        assert_eq!(r, relu);
    }
}
