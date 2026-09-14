//! Composition d'un message sortant.
//!
//! Deux choses décident si une réponse restera dans son fil chez le destinataire :
//! l'en-tête `In-Reply-To` et la chaîne `References`. Les négliger — ou les tronquer
//! — casse la conversation dans le client d'en face, ce que personne ne voit de son
//! côté. C'est donc ici que se joue une bonne part de la qualité perçue du client.

use iris_types::{Address, RfcMessageId, Timestamp};

/// Longueur maximale de la chaîne `References`.
///
/// Le RFC 5322 conseille de tronquer les chaînes trop longues, mais **jamais le
/// premier élément** : c'est la racine du fil, celle qui permet de le reconstituer.
/// On garde donc la racine et les plus récents.
const MAX_REFERENCES: usize = 20;

/// Un message à envoyer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub subject: String,
    pub text_body: String,
    /// Corps HTML facultatif. Une alternative texte est toujours jointe.
    pub html_body: Option<String>,
    pub in_reply_to: Option<RfcMessageId>,
    pub references: Vec<RfcMessageId>,
    pub attachments: Vec<Attachment>,
    /// Identifiant du message, généré si absent.
    pub message_id: Option<RfcMessageId>,
    pub date: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub filename: String,
    pub mime_type: String,
    pub content: Vec<u8>,
}

impl Outgoing {
    pub fn new(from: Address, to: Vec<Address>, subject: impl Into<String>) -> Self {
        Self {
            from,
            to,
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: subject.into(),
            text_body: String::new(),
            html_body: None,
            in_reply_to: None,
            references: Vec::new(),
            attachments: Vec::new(),
            message_id: None,
            date: Timestamp::EPOCH,
        }
    }

    pub fn body(mut self, text: impl Into<String>) -> Self {
        self.text_body = text.into();
        self
    }

    /// Vérifie qu'un message est envoyable.
    pub fn validate(&self) -> Result<(), String> {
        if !self.from.looks_valid() {
            return Err(format!(
                "adresse d'expédition invalide : « {} »",
                self.from.addr
            ));
        }
        if self.to.is_empty() && self.cc.is_empty() && self.bcc.is_empty() {
            return Err("aucun destinataire".into());
        }
        for a in self.to.iter().chain(&self.cc).chain(&self.bcc) {
            if !a.looks_valid() {
                return Err(format!("destinataire invalide : « {} »", a.addr));
            }
        }
        Ok(())
    }

    /// Total des pièces jointes, en octets.
    pub fn attachments_size(&self) -> u64 {
        self.attachments
            .iter()
            .map(|a| a.content.len() as u64)
            .sum()
    }
}

/// Ce dont on a besoin du message auquel on répond.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyTarget {
    pub message_id: Option<RfcMessageId>,
    pub references: Vec<RfcMessageId>,
    pub subject: String,
    pub from: Vec<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub date: Timestamp,
    pub text_body: String,
}

/// Portée d'une réponse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyScope {
    /// À l'expéditeur seul.
    Sender,
    /// À tout le monde, moins soi-même.
    All,
}

/// Construit une réponse à un message.
///
/// `identity` est l'adresse depuis laquelle on répond : elle est retirée des
/// destinataires, sinon chaque réponse à tous se renverrait le message à soi-même.
pub fn reply(target: &ReplyTarget, identity: &Address, scope: ReplyScope) -> Outgoing {
    // `Reply-To` prime sur `From` : c'est précisément sa raison d'être.
    let destinataires = if target.reply_to.is_empty() {
        target.from.clone()
    } else {
        target.reply_to.clone()
    };

    let moi = identity.key();
    let mut to: Vec<Address> = destinataires
        .into_iter()
        .filter(|a| a.key() != moi)
        .collect();
    let mut cc = Vec::new();

    if scope == ReplyScope::All {
        let deja: std::collections::BTreeSet<String> = to.iter().map(Address::key).collect();
        for a in target.to.iter().chain(&target.cc) {
            let k = a.key();
            if k != moi && !deja.contains(&k) && !cc.iter().any(|c: &Address| c.key() == k) {
                cc.push(a.clone());
            }
        }
    }

    // Répondre à un message qu'on s'est envoyé à soi-même ne doit pas produire un
    // message sans destinataire.
    if to.is_empty() {
        if cc.is_empty() {
            to = target.from.clone();
        } else {
            to.push(cc.remove(0));
        }
    }

    Outgoing {
        from: identity.clone(),
        to,
        cc,
        bcc: Vec::new(),
        subject: reply_subject(&target.subject),
        text_body: quote(target),
        html_body: None,
        in_reply_to: target.message_id.clone(),
        references: build_references(&target.references, target.message_id.as_ref()),
        attachments: Vec::new(),
        message_id: None,
        date: Timestamp::EPOCH,
    }
}

/// Préfixe le sujet, sans empiler les « Re: ».
pub fn reply_subject(subject: &str) -> String {
    let s = subject.trim();
    let minuscules = s.to_lowercase();
    if minuscules.starts_with("re:") || minuscules.starts_with("re :") {
        s.to_string()
    } else {
        format!("Re: {s}")
    }
}

/// Construit la chaîne `References` de la réponse.
pub fn build_references(
    parent_references: &[RfcMessageId],
    parent_id: Option<&RfcMessageId>,
) -> Vec<RfcMessageId> {
    let mut chaine: Vec<RfcMessageId> = parent_references.to_vec();
    if let Some(id) = parent_id {
        // Un identifiant déjà présent ne doit pas être répété : certains clients
        // bouclent sur les doublons.
        if !chaine.contains(id) {
            chaine.push(id.clone());
        }
    }

    if chaine.len() > MAX_REFERENCES {
        // On conserve la racine, puis les plus récents. Perdre la racine coupe le
        // fil chez tous ceux qui l'utilisent pour le reconstituer.
        let racine = chaine[0].clone();
        let queue: Vec<RfcMessageId> = chaine[chaine.len() - (MAX_REFERENCES - 1)..].to_vec();
        chaine = std::iter::once(racine).chain(queue).collect();
    }
    chaine
}

/// Prépare le corps cité de la réponse.
fn quote(target: &ReplyTarget) -> String {
    let auteur = target
        .from
        .first()
        .map(|a| a.display().to_string())
        .unwrap_or_else(|| "l'expéditeur".into());

    let cite: String = target
        .text_body
        .lines()
        .map(|l| {
            if l.is_empty() {
                ">".to_string()
            } else {
                format!("> {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n");

    format!(
        "\r\n\r\nLe {}, {auteur} a écrit :\r\n{cite}\r\n",
        format_date(target.date)
    )
}

/// Date lisible, en heure locale approximative (UTC).
fn format_date(t: Timestamp) -> String {
    // Format volontairement minimal : la ligne de citation n'a pas à être un
    // horodatage exact, et dépendre d'une bibliothèque de calendrier pour cela
    // serait disproportionné.
    let secondes = t.seconds();
    let jours = secondes.div_euclid(86_400);
    let reste = secondes.rem_euclid(86_400);
    let (h, m) = (reste / 3600, (reste % 3600) / 60);

    // 1970-01-01 + jours, calculé en jours civils.
    let (annee, mois, jour) = civil_from_days(jours);
    format!("{jour:02}/{mois:02}/{annee} à {h:02}:{m:02}")
}

/// Conversion jours depuis l'époque → date civile (algorithme de Howard Hinnant).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Construit un transfert.
pub fn forward(target: &ReplyTarget, identity: &Address, to: Vec<Address>) -> Outgoing {
    let entete = format!(
        "\r\n\r\n---------- Message transféré ----------\r\nDe : {}\r\nDate : {}\r\nSujet : {}\r\nÀ : {}\r\n\r\n{}",
        target.from.iter().map(Address::to_string).collect::<Vec<_>>().join(", "),
        format_date(target.date),
        target.subject,
        target.to.iter().map(Address::to_string).collect::<Vec<_>>().join(", "),
        target.text_body
    );

    Outgoing {
        from: identity.clone(),
        to,
        cc: Vec::new(),
        bcc: Vec::new(),
        subject: forward_subject(&target.subject),
        text_body: entete,
        html_body: None,
        // Un transfert ouvre une nouvelle conversation : le rattacher au fil
        // d'origine mêlerait deux échanges qui n'ont pas les mêmes participants.
        in_reply_to: None,
        references: Vec::new(),
        attachments: Vec::new(),
        message_id: None,
        date: Timestamp::EPOCH,
    }
}

pub fn forward_subject(subject: &str) -> String {
    let s = subject.trim();
    let minuscules = s.to_lowercase();
    if minuscules.starts_with("tr:") || minuscules.starts_with("fwd:") {
        s.to_string()
    } else {
        format!("Tr: {s}")
    }
}

/// Engendre un `Message-ID` unique.
pub fn generate_message_id(domain: &str, seed: u64) -> RfcMessageId {
    let domaine = if domain.is_empty() {
        "iris.local"
    } else {
        domain
    };
    let horodatage = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    RfcMessageId(format!("{horodatage:x}.{seed:x}@{domaine}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cible() -> ReplyTarget {
        ReplyTarget {
            message_id: RfcMessageId::parse("parent@example.com"),
            references: vec![RfcMessageId("racine@example.com".into())],
            subject: "Devis refonte".into(),
            from: vec![Address::named("Marie", "marie@example.com")],
            to: vec![
                Address::new("moi@example.com"),
                Address::new("luc@example.com"),
            ],
            cc: vec![Address::new("compta@example.com")],
            reply_to: vec![],
            date: Timestamp::from_millis(1_700_000_000_000),
            text_body: "Bonjour,\nVoici le devis.".into(),
        }
    }

    fn moi() -> Address {
        Address::named("Moi", "moi@example.com")
    }

    #[test]
    fn une_reponse_simple_va_a_l_expediteur() {
        let r = reply(&cible(), &moi(), ReplyScope::Sender);
        assert_eq!(r.to.len(), 1);
        assert_eq!(r.to[0].addr, "marie@example.com");
        assert!(r.cc.is_empty());
    }

    #[test]
    fn une_reponse_a_tous_inclut_les_autres_sans_soi_meme() {
        let r = reply(&cible(), &moi(), ReplyScope::All);
        let destinataires: Vec<_> = r.to.iter().chain(&r.cc).map(|a| a.addr.as_str()).collect();

        assert!(destinataires.contains(&"marie@example.com"));
        assert!(destinataires.contains(&"luc@example.com"));
        assert!(destinataires.contains(&"compta@example.com"));
        assert!(
            !destinataires.contains(&"moi@example.com"),
            "se répondre à soi-même à chaque fois est le défaut le plus agaçant"
        );
    }

    #[test]
    fn reply_to_prime_sur_from() {
        let mut c = cible();
        c.reply_to = vec![Address::new("contact@example.com")];
        let r = reply(&c, &moi(), ReplyScope::Sender);
        assert_eq!(r.to[0].addr, "contact@example.com");
    }

    #[test]
    fn repondre_a_soi_meme_ne_produit_pas_un_message_sans_destinataire() {
        let mut c = cible();
        c.from = vec![moi()];
        c.to = vec![];
        c.cc = vec![];
        let r = reply(&c, &moi(), ReplyScope::All);
        assert!(!r.to.is_empty());
    }

    #[test]
    fn le_sujet_n_empile_pas_les_prefixes() {
        assert_eq!(reply_subject("Devis"), "Re: Devis");
        assert_eq!(reply_subject("Re: Devis"), "Re: Devis");
        assert_eq!(reply_subject("RE: Devis"), "RE: Devis");
        assert_eq!(reply_subject("  Devis  "), "Re: Devis");
    }

    #[test]
    fn la_chaine_de_references_est_completee() {
        let r = reply(&cible(), &moi(), ReplyScope::Sender);
        let chaine: Vec<_> = r.references.iter().map(|x| x.as_str()).collect();
        assert_eq!(chaine, ["racine@example.com", "parent@example.com"]);
        assert_eq!(r.in_reply_to.unwrap().as_str(), "parent@example.com");
    }

    #[test]
    fn un_identifiant_deja_present_n_est_pas_repete() {
        let refs = vec![RfcMessageId("a@x".into()), RfcMessageId("b@x".into())];
        let chaine = build_references(&refs, Some(&RfcMessageId("b@x".into())));
        assert_eq!(chaine.len(), 2);
    }

    #[test]
    fn une_chaine_trop_longue_conserve_sa_racine() {
        // Perdre la racine coupe le fil chez tous les clients qui s'en servent.
        let refs: Vec<RfcMessageId> = (0..50).map(|i| RfcMessageId(format!("m{i}@x"))).collect();
        let chaine = build_references(&refs, Some(&RfcMessageId("dernier@x".into())));

        assert_eq!(chaine.len(), MAX_REFERENCES);
        assert_eq!(chaine[0].as_str(), "m0@x", "la racine doit survivre");
        assert_eq!(chaine.last().unwrap().as_str(), "dernier@x");
    }

    #[test]
    fn le_corps_cite_le_message_d_origine() {
        let r = reply(&cible(), &moi(), ReplyScope::Sender);
        assert!(r.text_body.contains("Marie a écrit"));
        assert!(r.text_body.contains("> Bonjour,"));
        assert!(r.text_body.contains("> Voici le devis."));
    }

    #[test]
    fn un_transfert_ouvre_une_nouvelle_conversation() {
        // Le rattacher au fil d'origine mêlerait deux échanges aux participants
        // différents.
        let f = forward(&cible(), &moi(), vec![Address::new("tiers@example.com")]);
        assert!(f.in_reply_to.is_none());
        assert!(f.references.is_empty());
        assert_eq!(f.subject, "Tr: Devis refonte");
        assert!(f.text_body.contains("Message transféré"));
    }

    #[test]
    fn le_sujet_de_transfert_n_empile_pas_non_plus() {
        assert_eq!(forward_subject("Tr: Devis"), "Tr: Devis");
        assert_eq!(forward_subject("Fwd: Devis"), "Fwd: Devis");
    }

    #[test]
    fn la_validation_refuse_un_message_sans_destinataire() {
        let m = Outgoing::new(moi(), vec![], "Sujet");
        assert!(m.validate().unwrap_err().contains("aucun destinataire"));
    }

    #[test]
    fn la_validation_refuse_une_adresse_malformee() {
        let m = Outgoing::new(moi(), vec![Address::new("pas-une-adresse")], "Sujet");
        assert!(m.validate().unwrap_err().contains("destinataire invalide"));

        let m = Outgoing::new(Address::new("cassé"), vec![Address::new("a@b.fr")], "S");
        assert!(m.validate().unwrap_err().contains("expédition"));
    }

    #[test]
    fn un_message_correct_passe_la_validation() {
        let m = Outgoing::new(moi(), vec![Address::new("a@b.fr")], "Sujet").body("Bonjour");
        assert!(m.validate().is_ok());
    }

    #[test]
    fn les_identifiants_engendres_sont_distincts() {
        let a = generate_message_id("example.com", 1);
        let b = generate_message_id("example.com", 2);
        assert_ne!(a, b);
        assert!(a.as_str().ends_with("@example.com"));
    }

    #[test]
    fn un_domaine_vide_retombe_sur_un_domaine_par_defaut() {
        assert!(generate_message_id("", 1).as_str().ends_with("@iris.local"));
    }

    #[test]
    fn la_date_de_citation_est_correcte() {
        // 1 700 000 000 s = 14 novembre 2023, 22 h 13 UTC.
        assert_eq!(
            format_date(Timestamp::from_millis(1_700_000_000_000)),
            "14/11/2023 à 22:13"
        );
        assert_eq!(format_date(Timestamp::EPOCH), "01/01/1970 à 00:00");
    }

    #[test]
    fn la_taille_des_pieces_jointes_se_calcule() {
        let mut m = Outgoing::new(moi(), vec![Address::new("a@b.fr")], "S");
        m.attachments.push(Attachment {
            filename: "a.pdf".into(),
            mime_type: "application/pdf".into(),
            content: vec![0; 1024],
        });
        assert_eq!(m.attachments_size(), 1024);
    }
}
