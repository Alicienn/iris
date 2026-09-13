//! `iris-search` — le langage de requête et sa planification.
//!
//! Une recherche s'écrit comme on parle : `de:marie devis -facture has:pj plus_vieux:30j`.
//! Le langage n'a que trois formes — un filtre `clé:valeur`, un mot libre, et un `-`
//! qui nie ce qui suit — et tout ce qui ne ressemble pas à un filtre connu redevient
//! du texte libre. Une faute de frappe cherche donc quelque chose au lieu de produire
//! une erreur.
//!
//! La planification sépare ce que le **store** sait faire vite (états, comptes,
//! drapeaux, dates) de ce qui exige l'**index** (le texte). Sans cette séparation,
//! chercher « tous les non-lus » passerait par un moteur plein texte qui n'a rien à
//! y faire.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

use iris_types::{Flags, Timestamp, WorkflowState};
use serde::{Deserialize, Serialize};

/// Un critère structuré, exécutable par le store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Filter {
    From(String),
    To(String),
    Subject(String),
    Account(String),
    Folder(String),
    State(WorkflowState),
    HasAttachment,
    HasTracker,
    Unsubscribable,
    Unread,
    Snoozed,
    /// Reçu il y a plus de N jours.
    OlderThanDays(u32),
    /// Reçu il y a moins de N jours.
    NewerThanDays(u32),
    LargerThanKb(u64),
}

impl Filter {
    /// Ce filtre porte-t-il sur du texte ? Ceux-là partent à l'index.
    pub fn is_textual(&self) -> bool {
        matches!(self, Self::From(_) | Self::To(_) | Self::Subject(_))
    }

    pub fn describe(&self) -> String {
        match self {
            Self::From(v) => format!("de « {v} »"),
            Self::To(v) => format!("à « {v} »"),
            Self::Subject(v) => format!("sujet « {v} »"),
            Self::Account(v) => format!("compte « {v} »"),
            Self::Folder(v) => format!("dossier « {v} »"),
            Self::State(s) => match s {
                WorkflowState::Todo => "à traiter".into(),
                WorkflowState::Waiting => "en attente".into(),
                WorkflowState::Done => "traité".into(),
            },
            Self::HasAttachment => "avec pièce jointe".into(),
            Self::HasTracker => "avec traqueur".into(),
            Self::Unsubscribable => "désabonnable".into(),
            Self::Unread => "non lu".into(),
            Self::Snoozed => "reporté".into(),
            Self::OlderThanDays(d) => format!("plus vieux que {d} jours"),
            Self::NewerThanDays(d) => format!("plus récent que {d} jours"),
            Self::LargerThanKb(k) => format!("plus gros que {k} ko"),
        }
    }
}

/// Un terme de la requête, éventuellement nié.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Term {
    pub negated: bool,
    pub kind: TermKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TermKind {
    Filter(Filter),
    /// Mot ou phrase à chercher dans le texte.
    Text(String),
}

/// Une requête analysée.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Query {
    pub terms: Vec<Term>,
    /// La requête telle que saisie, conservée pour l'affichage et l'épinglage.
    pub raw: String,
}

impl Query {
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    pub fn filters(&self) -> impl Iterator<Item = (&Filter, bool)> {
        self.terms.iter().filter_map(|t| match &t.kind {
            TermKind::Filter(f) => Some((f, t.negated)),
            TermKind::Text(_) => None,
        })
    }

    pub fn texts(&self) -> impl Iterator<Item = (&str, bool)> {
        self.terms.iter().filter_map(|t| match &t.kind {
            TermKind::Text(s) => Some((s.as_str(), t.negated)),
            TermKind::Filter(_) => None,
        })
    }

    /// Description en français, pour confirmer à l'utilisateur ce qui a été compris.
    pub fn describe(&self) -> String {
        if self.terms.is_empty() {
            return "recherche vide".into();
        }
        self.terms
            .iter()
            .map(|t| {
                let base = match &t.kind {
                    TermKind::Filter(f) => f.describe(),
                    TermKind::Text(s) => format!("texte « {s} »"),
                };
                if t.negated {
                    format!("sans {base}")
                } else {
                    base
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Analyse une requête écrite par l'utilisateur.
///
/// N'échoue jamais : ce qui n'est pas reconnu devient du texte libre. Une barre de
/// recherche qui refuse de chercher parce qu'un caractère la dérange est une barre de
/// recherche cassée.
pub fn parse(input: &str) -> Query {
    let mut terms = Vec::new();

    for brut in tokenize(input) {
        let (negated, reste) = match brut.strip_prefix('-') {
            Some(r) if !r.is_empty() => (true, r.to_string()),
            _ => (false, brut),
        };

        let kind = match reste.split_once(':') {
            Some((cle, valeur)) if !valeur.is_empty() => match parse_filter(cle, valeur) {
                Some(f) => TermKind::Filter(f),
                // Clé inconnue : on cherche la chaîne entière comme du texte.
                None => TermKind::Text(reste.clone()),
            },
            _ => TermKind::Text(reste.clone()),
        };

        terms.push(Term { negated, kind });
    }

    Query { terms, raw: input.trim().to_string() }
}

/// Découpe la saisie en jetons, en respectant les guillemets.
fn tokenize(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut courant = String::new();
    let mut dans_guillemets = false;

    for c in input.chars() {
        match c {
            '"' => dans_guillemets = !dans_guillemets,
            c if c.is_whitespace() && !dans_guillemets => {
                if !courant.is_empty() {
                    out.push(std::mem::take(&mut courant));
                }
            }
            c => courant.push(c),
        }
    }
    if !courant.is_empty() {
        out.push(courant);
    }
    out
}

/// Reconnaît une clé de filtre. Les formes française et anglaise sont acceptées :
/// l'utilisateur ne doit pas avoir à deviner la langue de l'auteur.
fn parse_filter(cle: &str, valeur: &str) -> Option<Filter> {
    let v = valeur.trim();
    match cle.to_lowercase().as_str() {
        "de" | "from" => Some(Filter::From(v.to_string())),
        "a" | "à" | "to" => Some(Filter::To(v.to_string())),
        "sujet" | "subject" => Some(Filter::Subject(v.to_string())),
        "compte" | "account" => Some(Filter::Account(v.to_string())),
        "dossier" | "folder" | "in" => Some(Filter::Folder(v.to_string())),
        "etat" | "état" | "state" | "is" => parse_state(v),
        "a_pj" | "has" => parse_has(v),
        "plus_vieux" | "older_than" => parse_days(v).map(Filter::OlderThanDays),
        "plus_recent" | "newer_than" => parse_days(v).map(Filter::NewerThanDays),
        "taille" | "larger_than" => parse_size_kb(v).map(Filter::LargerThanKb),
        _ => None,
    }
}

fn parse_state(v: &str) -> Option<Filter> {
    match v.to_lowercase().as_str() {
        "a_traiter" | "todo" => Some(Filter::State(WorkflowState::Todo)),
        "en_attente" | "waiting" => Some(Filter::State(WorkflowState::Waiting)),
        "traite" | "traité" | "done" => Some(Filter::State(WorkflowState::Done)),
        "non_lu" | "unread" => Some(Filter::Unread),
        "reporte" | "reporté" | "snoozed" => Some(Filter::Snoozed),
        _ => None,
    }
}

fn parse_has(v: &str) -> Option<Filter> {
    match v.to_lowercase().as_str() {
        "pj" | "attachment" | "piece_jointe" => Some(Filter::HasAttachment),
        "traqueur" | "tracker" => Some(Filter::HasTracker),
        "desabonnement" | "désabonnement" | "unsubscribe" => Some(Filter::Unsubscribable),
        _ => None,
    }
}

/// Accepte « 30 », « 30j », « 30d », « 4s » (semaines), « 6m », « 2a ».
fn parse_days(v: &str) -> Option<u32> {
    let v = v.trim().to_lowercase();
    let (nombre, facteur) = match v.chars().last()? {
        'j' | 'd' => (&v[..v.len() - 1], 1),
        's' | 'w' => (&v[..v.len() - 1], 7),
        'm' => (&v[..v.len() - 1], 30),
        'a' | 'y' => (&v[..v.len() - 1], 365),
        _ => (v.as_str(), 1),
    };
    nombre.trim().parse::<u32>().ok().map(|n| n * facteur)
}

/// Accepte « 500 » (ko), « 500ko », « 2mo », « 1go ».
fn parse_size_kb(v: &str) -> Option<u64> {
    let v = v.trim().to_lowercase().replace(' ', "");
    for (suffixe, facteur) in [("go", 1024 * 1024), ("gb", 1024 * 1024), ("mo", 1024), ("mb", 1024), ("ko", 1), ("kb", 1)] {
        if let Some(n) = v.strip_suffix(suffixe) {
            return n.parse::<u64>().ok().map(|x| x * facteur);
        }
    }
    v.parse().ok()
}

/// Le plan d'exécution : qui fait quoi.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Critères exécutés par le store, en SQL.
    pub store_filters: Vec<(Filter, bool)>,
    /// Requête à transmettre à l'index plein texte. Vide s'il n'y a pas de texte.
    pub index_query: String,
    /// La recherche peut-elle être servie sans toucher à l'index ?
    pub store_only: bool,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.store_filters.is_empty() && self.index_query.is_empty()
    }
}

/// Répartit une requête entre le store et l'index.
pub fn plan(query: &Query) -> Plan {
    let mut store_filters = Vec::new();
    let mut morceaux = Vec::new();

    for term in &query.terms {
        match &term.kind {
            // Les filtres textuels partent à l'index, où ils profitent de l'analyse
            // lexicale ; les autres restent au store, où ils sont indexés en SQL.
            TermKind::Filter(f) if f.is_textual() => {
                let champ = match f {
                    Filter::From(v) => format!("from:{}", escape(v)),
                    Filter::To(v) => format!("recipients:{}", escape(v)),
                    Filter::Subject(v) => format!("subject:{}", escape(v)),
                    _ => unreachable!("is_textual restreint aux trois cas ci-dessus"),
                };
                morceaux.push(if term.negated { format!("-{champ}") } else { champ });
            }
            TermKind::Filter(f) => store_filters.push((f.clone(), term.negated)),
            TermKind::Text(t) => {
                let t = escape(t);
                morceaux.push(if term.negated { format!("-{t}") } else { t });
            }
        }
    }

    let index_query = morceaux.join(" ");
    Plan {
        store_only: index_query.is_empty(),
        store_filters,
        index_query,
    }
}

/// Protège les caractères que le moteur d'index interpréterait comme de la syntaxe.
fn escape(s: &str) -> String {
    let besoin_guillemets = s.contains(char::is_whitespace);
    let nettoye: String = s
        .chars()
        .filter(|c| !matches!(c, '"' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '~' | '\\'))
        .collect();
    if besoin_guillemets {
        format!("\"{nettoye}\"")
    } else {
        nettoye
    }
}

/// Une recherche épinglée, présentée comme un dossier vivant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSearch {
    pub id: String,
    pub name: String,
    pub query: String,
    /// Position dans la barre latérale.
    pub position: u32,
}

impl SavedSearch {
    pub fn new(id: impl Into<String>, name: impl Into<String>, query: impl Into<String>) -> Self {
        Self { id: id.into(), name: name.into(), query: query.into(), position: 0 }
    }

    pub fn parsed(&self) -> Query {
        parse(&self.query)
    }
}

/// Évalue les filtres non textuels sur un message donné.
///
/// Utilisé pour vérifier un message isolé — après réception, par exemple — sans
/// repasser par le store.
pub fn matches_filters(
    filters: &[(Filter, bool)],
    flags: Flags,
    state: WorkflowState,
    received: Timestamp,
    size: u64,
    account: &str,
    folder: &str,
    snoozed: bool,
    now: Timestamp,
) -> bool {
    filters.iter().all(|(f, negated)| {
        let brut = match f {
            Filter::State(s) => state == *s,
            Filter::HasAttachment => flags.contains(Flags::HAS_ATTACHMENT),
            Filter::HasTracker => flags.contains(Flags::HAS_TRACKER),
            Filter::Unsubscribable => flags.contains(Flags::UNSUBSCRIBABLE),
            Filter::Unread => !flags.contains(Flags::SEEN),
            Filter::Snoozed => snoozed,
            Filter::Account(a) => account.eq_ignore_ascii_case(a),
            Filter::Folder(d) => folder.eq_ignore_ascii_case(d),
            Filter::OlderThanDays(d) => {
                received.millis() < now.millis() - (*d as i64) * 86_400_000
            }
            Filter::NewerThanDays(d) => {
                received.millis() >= now.millis() - (*d as i64) * 86_400_000
            }
            Filter::LargerThanKb(k) => size > k * 1024,
            // Les filtres textuels ne sont pas évaluables ici : ils appartiennent à
            // l'index. On ne les laisse pas rejeter le message par défaut.
            Filter::From(_) | Filter::To(_) | Filter::Subject(_) => true,
        };
        brut != *negated
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn une_requete_vide_ne_contient_rien() {
        let q = parse("   ");
        assert!(q.is_empty());
        assert_eq!(q.describe(), "recherche vide");
        assert!(plan(&q).is_empty());
    }

    #[test]
    fn les_mots_libres_deviennent_du_texte() {
        let q = parse("devis refonte");
        let textes: Vec<_> = q.texts().map(|(t, _)| t).collect();
        assert_eq!(textes, ["devis", "refonte"]);
    }

    #[test]
    fn les_filtres_sont_reconnus_en_francais_et_en_anglais() {
        for entree in ["de:marie", "from:marie"] {
            let q = parse(entree);
            assert_eq!(
                q.filters().next().unwrap().0,
                &Filter::From("marie".into()),
                "échec sur « {entree} »"
            );
        }
    }

    #[test]
    fn une_cle_inconnue_redevient_du_texte() {
        // Une faute de frappe doit chercher, pas échouer.
        let q = parse("expediteur:marie");
        let textes: Vec<_> = q.texts().map(|(t, _)| t).collect();
        assert_eq!(textes, ["expediteur:marie"]);
    }

    #[test]
    fn le_tiret_nie_le_terme_suivant() {
        let q = parse("devis -facture -de:spam@x.fr");
        let negations: Vec<_> = q.terms.iter().map(|t| t.negated).collect();
        assert_eq!(negations, [false, true, true]);
    }

    #[test]
    fn un_tiret_seul_reste_du_texte() {
        let q = parse("-");
        assert_eq!(q.terms.len(), 1);
        assert!(!q.terms[0].negated);
    }

    #[test]
    fn les_guillemets_preservent_les_phrases() {
        let q = parse(r#""devis de refonte" marie"#);
        let textes: Vec<_> = q.texts().map(|(t, _)| t).collect();
        assert_eq!(textes, ["devis de refonte", "marie"]);
    }

    #[test]
    fn un_guillemet_non_ferme_ne_perd_pas_le_texte() {
        let q = parse(r#""devis de refonte"#);
        let textes: Vec<_> = q.texts().map(|(t, _)| t).collect();
        assert_eq!(textes, ["devis de refonte"]);
    }

    #[test]
    fn les_etats_sont_reconnus() {
        assert_eq!(
            parse("etat:a_traiter").filters().next().unwrap().0,
            &Filter::State(WorkflowState::Todo)
        );
        assert_eq!(parse("is:unread").filters().next().unwrap().0, &Filter::Unread);
        assert_eq!(parse("état:reporté").filters().next().unwrap().0, &Filter::Snoozed);
    }

    #[test]
    fn les_durees_acceptent_plusieurs_unites() {
        assert_eq!(parse_days("30"), Some(30));
        assert_eq!(parse_days("30j"), Some(30));
        assert_eq!(parse_days("4s"), Some(28));
        assert_eq!(parse_days("6m"), Some(180));
        assert_eq!(parse_days("2a"), Some(730));
        assert_eq!(parse_days("bientôt"), None);
    }

    #[test]
    fn les_tailles_acceptent_plusieurs_unites() {
        assert_eq!(parse_size_kb("500"), Some(500));
        assert_eq!(parse_size_kb("500ko"), Some(500));
        assert_eq!(parse_size_kb("2mo"), Some(2048));
        assert_eq!(parse_size_kb("1go"), Some(1024 * 1024));
        assert_eq!(parse_size_kb("gros"), None);
    }

    #[test]
    fn une_duree_invalide_redevient_du_texte() {
        let q = parse("plus_vieux:bientôt");
        assert_eq!(q.texts().count(), 1);
        assert_eq!(q.filters().count(), 0);
    }

    #[test]
    fn le_plan_separe_le_structurel_du_textuel() {
        let q = parse("devis etat:a_traiter has:pj de:marie");
        let p = plan(&q);

        assert_eq!(p.store_filters.len(), 2, "l'état et la pièce jointe vont au store");
        assert!(p.index_query.contains("devis"));
        assert!(p.index_query.contains("from:marie"));
        assert!(!p.store_only);
    }

    #[test]
    fn une_recherche_purement_structurelle_evite_l_index() {
        // « tous les non-lus » n'a rien à faire dans un moteur plein texte.
        let q = parse("is:unread etat:a_traiter");
        let p = plan(&q);
        assert!(p.store_only);
        assert!(p.index_query.is_empty());
        assert_eq!(p.store_filters.len(), 2);
    }

    #[test]
    fn la_negation_est_transmise_a_l_index() {
        let p = plan(&parse("devis -facture"));
        assert!(p.index_query.contains("-facture"));
    }

    #[test]
    fn les_caracteres_de_syntaxe_sont_neutralises() {
        // Sans cela, une parenthèse saisie par mégarde ferait échouer la requête.
        let p = plan(&parse("compta (2024)"));
        assert!(!p.index_query.contains('('));
        assert!(p.index_query.contains("compta"));
    }

    #[test]
    fn une_phrase_est_transmise_entre_guillemets() {
        let p = plan(&parse(r#""devis de refonte""#));
        assert_eq!(p.index_query, "\"devis de refonte\"");
    }

    #[test]
    fn la_description_reformule_la_requete() {
        let q = parse("de:marie -has:pj devis");
        let d = q.describe();
        assert!(d.contains("de « marie »"));
        assert!(d.contains("sans avec pièce jointe"));
        assert!(d.contains("texte « devis »"));
    }

    #[test]
    fn les_filtres_structurels_s_evaluent_sur_un_message() {
        let now = Timestamp::from_millis(1_000_000_000_000);
        let vieux = Timestamp::from_millis(now.millis() - 100 * 86_400_000);

        let filtres = plan(&parse("is:unread plus_vieux:30j")).store_filters;
        assert!(matches_filters(
            &filtres,
            Flags::NONE,
            WorkflowState::Todo,
            vieux,
            0,
            "a@x.fr",
            "INBOX",
            false,
            now
        ));
        // Lu : le filtre « non lu » rejette.
        assert!(!matches_filters(
            &filtres,
            Flags::SEEN,
            WorkflowState::Todo,
            vieux,
            0,
            "a@x.fr",
            "INBOX",
            false,
            now
        ));
    }

    #[test]
    fn la_negation_inverse_l_evaluation() {
        let now = Timestamp::from_millis(1_000_000_000_000);
        let filtres = plan(&parse("-has:pj")).store_filters;
        assert!(matches_filters(
            &filtres,
            Flags::NONE,
            WorkflowState::Todo,
            now,
            0,
            "a",
            "INBOX",
            false,
            now
        ));
        assert!(!matches_filters(
            &filtres,
            Flags::HAS_ATTACHMENT,
            WorkflowState::Todo,
            now,
            0,
            "a",
            "INBOX",
            false,
            now
        ));
    }

    #[test]
    fn un_filtre_textuel_ne_rejette_pas_un_message_localement() {
        // Il appartient à l'index ; l'évaluer ici reviendrait à tout rejeter.
        let now = Timestamp::from_millis(0);
        let filtres = vec![(Filter::From("marie".into()), false)];
        assert!(matches_filters(
            &filtres,
            Flags::NONE,
            WorkflowState::Todo,
            now,
            0,
            "a",
            "INBOX",
            false,
            now
        ));
    }

    #[test]
    fn une_recherche_epinglee_se_reanalyse() {
        let s = SavedSearch::new("factures", "Factures en attente", "sujet:facture etat:en_attente");
        let q = s.parsed();
        assert_eq!(q.filters().count(), 2);
    }

    #[test]
    fn une_requete_survit_a_un_aller_retour_json() {
        let q = parse("de:marie -has:pj \"devis de refonte\"");
        let json = serde_json::to_string(&q).unwrap();
        assert_eq!(serde_json::from_str::<Query>(&json).unwrap(), q);
    }
}
