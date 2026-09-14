//! `iris-thread` — regroupement des messages en fils.
//!
//! Implémente l'algorithme de Jamie Zawinski, qui reste après vingt-cinq ans la
//! référence du domaine, avec deux ajustements dictés par notre modèle produit :
//!
//! - **le repli par sujet est borné dans le temps.** Sans borne, deux messages
//!   intitulés « Facture » à trois ans d'écart fusionneraient. La fenêtre par défaut
//!   est de trente jours.
//! - **le recollage inter-comptes est optionnel.** Il n'a de sens qu'en usage
//!   multi-boîtes ; activé sans raison, il rapproche des conversations que
//!   l'utilisateur tient volontairement séparées.
//!
//! La crate est **pure** : aucune entrée-sortie, aucune base. On lui donne un
//! ensemble de messages, elle rend des groupes.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

use iris_types::{AccountId, MessageId, RfcMessageId, Timestamp};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Ce dont l'algorithme a besoin pour placer un message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadInput {
    pub id: MessageId,
    pub account: AccountId,
    pub message_id: Option<RfcMessageId>,
    pub in_reply_to: Option<RfcMessageId>,
    /// Du plus ancien au plus récent, tel que dans l'en-tête `References`.
    pub references: Vec<RfcMessageId>,
    pub subject: String,
    pub date: Timestamp,
}

impl ThreadInput {
    pub fn new(id: MessageId, account: AccountId, date: Timestamp) -> Self {
        Self {
            id,
            account,
            message_id: None,
            in_reply_to: None,
            references: Vec::new(),
            subject: String::new(),
            date,
        }
    }

    pub fn with_id(mut self, id: &str) -> Self {
        self.message_id = RfcMessageId::parse(id);
        self
    }

    pub fn replying_to(mut self, id: &str) -> Self {
        self.in_reply_to = RfcMessageId::parse(id);
        self
    }

    pub fn referencing(mut self, ids: &[&str]) -> Self {
        self.references = ids.iter().filter_map(|s| RfcMessageId::parse(s)).collect();
        self
    }

    pub fn subject(mut self, s: &str) -> Self {
        self.subject = s.to_string();
        self
    }

    /// Tous les identifiants cités, du plus proche au plus lointain.
    fn cited(&self) -> impl Iterator<Item = &RfcMessageId> {
        self.in_reply_to.iter().chain(self.references.iter())
    }
}

/// Réglages du regroupement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadingOptions {
    /// Autoriser le repli par sujet quand aucun lien explicite n'existe.
    pub subject_fallback: bool,
    /// Fenêtre du repli par sujet, en secondes.
    pub subject_window_secs: i64,
    /// Recoller les fils qui traversent plusieurs comptes.
    pub cross_account: bool,
}

impl Default for ThreadingOptions {
    fn default() -> Self {
        Self {
            subject_fallback: true,
            subject_window_secs: 30 * 86_400,
            cross_account: false,
        }
    }
}

/// Un groupe de messages formant une conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadGroup {
    /// Messages, triés par date croissante.
    pub messages: Vec<MessageId>,
    /// Sujet normalisé du groupe.
    pub subject: String,
    /// Comptes concernés.
    pub accounts: BTreeSet<AccountId>,
    pub first: Timestamp,
    pub last: Timestamp,
}

impl ThreadGroup {
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Le fil traverse-t-il plusieurs boîtes ?
    pub fn is_cross_account(&self) -> bool {
        self.accounts.len() > 1
    }
}

/// Normalise un sujet : retire les préfixes de réponse et de transfert, replie les
/// espaces, passe en minuscules.
pub fn normalize_subject(subject: &str) -> String {
    const PREFIXES: &[&str] = &[
        "re:", "re :", "ré:", "ré :", "rép:", "rep:", "fwd:", "fw:", "tr:", "réf:", "aw:",
    ];

    let mut s = subject.trim();
    loop {
        let lower = s.to_lowercase();
        let trouve = PREFIXES.iter().find_map(|p| {
            lower.starts_with(p).then(|| {
                // Les préfixes contiennent des caractères accentués : on avance dans
                // la chaîne d'origine du même nombre d'octets que dans la version en
                // minuscules, ce qui est exact tant que la casse ne change pas la
                // longueur — vrai pour tous les préfixes de cette liste.
                let coupe = p.len().min(s.len());
                s[coupe..].trim_start()
            })
        });
        match trouve {
            Some(reste) if reste.len() < s.len() => s = reste,
            _ => break,
        }
    }
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Regroupe un ensemble de messages en fils.
///
/// Les groupes sont rendus triés par date du dernier message, du plus récent au plus
/// ancien — l'ordre de la liste principale.
pub fn group(messages: &[ThreadInput], options: ThreadingOptions) -> Vec<ThreadGroup> {
    if messages.is_empty() {
        return Vec::new();
    }

    let mut uf = UnionFind::new(messages.len());

    // Table des identifiants connus, pour retrouver l'indice d'un message cité.
    let mut par_identifiant: HashMap<&str, usize> = HashMap::new();
    for (i, m) in messages.iter().enumerate() {
        if let Some(id) = &m.message_id {
            // En cas de doublon d'identifiant — cela arrive, notamment sur les
            // messages envoyés à soi-même —, le premier gagne.
            par_identifiant.entry(id.as_str()).or_insert(i);
        }
    }

    // 1. Liens explicites : un message rejoint tout message qu'il cite.
    for (i, m) in messages.iter().enumerate() {
        for cite in m.cited() {
            if let Some(&j) = par_identifiant.get(cite.as_str()) {
                uf.union(i, j);
            }
        }
    }

    // 2. Liens implicites par références partagées : deux réponses à un même message
    //    absent de notre base appartiennent au même fil, même si nous n'avons jamais
    //    vu leur parent commun. Sans cette étape, une conversation dont on n'a que
    //    les réponses se disperserait.
    let mut par_reference: HashMap<&str, usize> = HashMap::new();
    for (i, m) in messages.iter().enumerate() {
        for cite in m.cited() {
            match par_reference.get(cite.as_str()) {
                Some(&j) => uf.union(i, j),
                None => {
                    par_reference.insert(cite.as_str(), i);
                }
            }
        }
    }

    // 3. Repli par sujet, borné dans le temps.
    if options.subject_fallback {
        let mut par_sujet: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, m) in messages.iter().enumerate() {
            let sujet = normalize_subject(&m.subject);
            if sujet.is_empty() {
                continue;
            }
            par_sujet.entry(sujet).or_default().push(i);
        }

        for indices in par_sujet.values() {
            let mut tries: Vec<usize> = indices.clone();
            tries.sort_by_key(|&i| messages[i].date.millis());
            for paire in tries.windows(2) {
                let (a, b) = (paire[0], paire[1]);
                let ecart = messages[b].date.distance_secs(messages[a].date);
                if ecart <= options.subject_window_secs {
                    uf.union(a, b);
                }
            }
        }
    }

    // 4. Cloisonnement par compte, si le recollage inter-comptes n'est pas demandé.
    let mut racines: BTreeMap<(usize, Option<AccountId>), Vec<usize>> = BTreeMap::new();
    for (i, m) in messages.iter().enumerate() {
        let cle = if options.cross_account {
            None
        } else {
            Some(m.account)
        };
        racines.entry((uf.find(i), cle)).or_default().push(i);
    }

    let mut groupes: Vec<ThreadGroup> = racines
        .into_values()
        .map(|mut indices| {
            indices.sort_by_key(|&i| (messages[i].date.millis(), messages[i].id.get()));
            let first = messages[indices[0]].date;
            let last = messages[*indices.last().unwrap()].date;

            // Le sujet du groupe est celui du message le plus ancien qui en a un :
            // c'est celui qui a ouvert la conversation.
            let subject = indices
                .iter()
                .map(|&i| normalize_subject(&messages[i].subject))
                .find(|s| !s.is_empty())
                .unwrap_or_default();

            ThreadGroup {
                accounts: indices.iter().map(|&i| messages[i].account).collect(),
                messages: indices.iter().map(|&i| messages[i].id).collect(),
                subject,
                first,
                last,
            }
        })
        .collect();

    groupes.sort_by_key(|g| std::cmp::Reverse((g.last.millis(), g.messages[0].get())));
    groupes
}

/// Union-find avec compression de chemin.
///
/// Le regroupement est par nature une relation d'équivalence : le construire par
/// unions successives évite d'avoir à reconstruire l'arbre de conversation, dont nous
/// n'avons pas besoin — l'interface affiche un fil à plat.
#[derive(Debug)]
struct UnionFind {
    parent: Vec<usize>,
    rang: Vec<u8>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rang: vec![0; n],
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        match self.rang[ra].cmp(&self.rang[rb]) {
            std::cmp::Ordering::Less => self.parent[ra] = rb,
            std::cmp::Ordering::Greater => self.parent[rb] = ra,
            std::cmp::Ordering::Equal => {
                self.parent[rb] = ra;
                self.rang[ra] += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(n: i64, jour: i64) -> ThreadInput {
        ThreadInput::new(
            MessageId(n),
            AccountId(1),
            Timestamp::from_millis(jour * 86_400_000),
        )
        .with_id(&format!("m{n}@x"))
        .subject("Devis refonte")
    }

    fn ids(g: &ThreadGroup) -> Vec<i64> {
        g.messages.iter().map(|m| m.get()).collect()
    }

    #[test]
    fn un_ensemble_vide_donne_aucun_groupe() {
        assert!(group(&[], ThreadingOptions::default()).is_empty());
    }

    #[test]
    fn une_reponse_rejoint_son_parent() {
        let messages = vec![msg(1, 1), msg(2, 2).replying_to("m1@x")];
        let g = group(&messages, ThreadingOptions::default());
        assert_eq!(g.len(), 1);
        assert_eq!(ids(&g[0]), [1, 2]);
    }

    #[test]
    fn une_chaine_de_references_recolle_tout_le_fil() {
        let messages = vec![
            msg(1, 1),
            msg(2, 2).referencing(&["m1@x"]),
            msg(3, 3).referencing(&["m1@x", "m2@x"]),
        ];
        let g = group(&messages, ThreadingOptions::default());
        assert_eq!(g.len(), 1);
        assert_eq!(ids(&g[0]), [1, 2, 3]);
    }

    #[test]
    fn deux_reponses_a_un_parent_absent_restent_ensemble() {
        // Le parent n'a jamais été synchronisé. Sans le rapprochement par référence
        // partagée, on obtiendrait deux fils pour une seule conversation.
        let messages = vec![
            msg(2, 2).replying_to("disparu@x").subject("Sujet A"),
            msg(3, 3).replying_to("disparu@x").subject("Sujet B"),
        ];
        let g = group(&messages, ThreadingOptions::default());
        assert_eq!(g.len(), 1);
        assert_eq!(ids(&g[0]), [2, 3]);
    }

    #[test]
    fn le_repli_par_sujet_regroupe_les_messages_proches() {
        let messages = vec![
            msg(1, 1).subject("Facture mars"),
            msg(2, 3).subject("Re: Facture mars"),
        ];
        let g = group(&messages, ThreadingOptions::default());
        assert_eq!(g.len(), 1);
    }

    #[test]
    fn le_repli_par_sujet_ne_traverse_pas_les_annees() {
        // Deux « Facture » à trois ans d'écart ne sont pas la même conversation.
        let messages = vec![
            msg(1, 0).subject("Facture"),
            msg(2, 1000).subject("Facture"),
        ];
        let g = group(&messages, ThreadingOptions::default());
        assert_eq!(g.len(), 2);
    }

    #[test]
    fn le_repli_par_sujet_est_desactivable() {
        let options = ThreadingOptions {
            subject_fallback: false,
            ..Default::default()
        };
        let messages = vec![
            msg(1, 1).subject("Facture mars"),
            msg(2, 2).subject("Re: Facture mars"),
        ];
        assert_eq!(group(&messages, options).len(), 2);
    }

    #[test]
    fn un_sujet_vide_ne_regroupe_rien() {
        // Sans cette garde, tous les messages sans sujet fusionneraient.
        let messages = vec![msg(1, 1).subject(""), msg(2, 1).subject("")];
        assert_eq!(group(&messages, ThreadingOptions::default()).len(), 2);
    }

    #[test]
    fn le_lien_explicite_l_emporte_sur_la_fenetre_de_sujet() {
        // Une vraie réponse, même très tardive, reste dans son fil.
        let messages = vec![msg(1, 0), msg(2, 1000).replying_to("m1@x")];
        let g = group(&messages, ThreadingOptions::default());
        assert_eq!(g.len(), 1);
    }

    #[test]
    fn les_comptes_sont_cloisonnes_par_defaut() {
        let mut autre = msg(2, 2).replying_to("m1@x");
        autre.account = AccountId(2);
        let messages = vec![msg(1, 1), autre];

        let g = group(&messages, ThreadingOptions::default());
        assert_eq!(g.len(), 2, "sans recollage inter-comptes, deux fils");
    }

    #[test]
    fn le_recollage_inter_comptes_reunit_le_fil() {
        let mut autre = msg(2, 2).replying_to("m1@x");
        autre.account = AccountId(2);
        let messages = vec![msg(1, 1), autre];

        let options = ThreadingOptions {
            cross_account: true,
            ..Default::default()
        };
        let g = group(&messages, options);
        assert_eq!(g.len(), 1);
        assert!(g[0].is_cross_account());
        assert_eq!(g[0].accounts.len(), 2);
    }

    #[test]
    fn les_groupes_sortent_du_plus_recent_au_plus_ancien() {
        let messages = vec![
            msg(1, 1).subject("Ancien").with_id("a@x"),
            msg(2, 50).subject("Récent").with_id("b@x"),
            msg(3, 25).subject("Milieu").with_id("c@x"),
        ];
        let g = group(&messages, ThreadingOptions::default());
        let ordre: Vec<_> = g.iter().map(|x| x.messages[0].get()).collect();
        assert_eq!(ordre, [2, 3, 1]);
    }

    #[test]
    fn le_sujet_du_groupe_est_celui_du_message_d_origine() {
        let messages = vec![
            msg(1, 1).subject("Devis refonte"),
            msg(2, 2).replying_to("m1@x").subject("Re: Devis refonte"),
        ];
        let g = group(&messages, ThreadingOptions::default());
        assert_eq!(g[0].subject, "devis refonte");
    }

    #[test]
    fn un_identifiant_duplique_ne_fait_pas_paniquer() {
        let messages = vec![
            msg(1, 1).with_id("meme@x"),
            msg(2, 2).with_id("meme@x").subject("Autre chose"),
        ];
        let g = group(&messages, ThreadingOptions::default());
        assert!(!g.is_empty());
    }

    #[test]
    fn normalisation_des_sujets() {
        assert_eq!(normalize_subject("Re: Devis"), "devis");
        assert_eq!(
            normalize_subject("RE: FWD: Devis  refonte "),
            "devis refonte"
        );
        assert_eq!(normalize_subject("TR: Devis"), "devis");
        assert_eq!(normalize_subject("AW: Devis"), "devis");
        assert_eq!(normalize_subject("   "), "");
        // « Réponse » n'est pas un préfixe de réponse.
        assert_eq!(normalize_subject("Réponse attendue"), "réponse attendue");
    }

    #[test]
    fn un_grand_fil_reste_un_seul_groupe() {
        let mut messages = vec![msg(1, 1)];
        for n in 2..=200 {
            messages.push(msg(n, n).replying_to(&format!("m{}@x", n - 1)));
        }
        let g = group(&messages, ThreadingOptions::default());
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].len(), 200);
    }
}
