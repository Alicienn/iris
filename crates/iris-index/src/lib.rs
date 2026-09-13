//! `iris-index` — la recherche plein texte.
//!
//! Tantivy plutôt que FTS5 : à un million de messages, l'écart de pertinence et de
//! latence n'est plus discutable, et l'index vit hors de la base, ce qui évite de
//! faire grossir le fichier que la liste principale doit garder rapide.
//!
//! **L'unité indexée est le message, l'unité retournée est le fil.** L'utilisateur
//! cherche une conversation, pas un fragment ; la couche de recherche agrège donc les
//! correspondances par fil avant de répondre.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

use iris_types::{AccountId, Error, MessageId, Result, ThreadId, Timestamp};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, FAST, INDEXED,
    STORED, STRING,
};
use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};

/// Mémoire allouée à l'écrivain. En deçà de 15 Mo, Tantivy refuse de démarrer ;
/// au-delà de ~100 Mo, le gain sur l'indexation incrémentale devient nul alors que
/// l'empreinte au repos compte, elle, dans notre budget de 400 Mo.
const WRITER_HEAP: usize = 50_000_000;

/// Un message à indexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedMessage {
    pub message: MessageId,
    pub thread: ThreadId,
    pub account: AccountId,
    pub subject: String,
    pub from: String,
    pub recipients: String,
    pub body: String,
    pub received: Timestamp,
    pub has_attachment: bool,
}

/// Un résultat de recherche, agrégé par fil.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub thread: ThreadId,
    /// Meilleur score parmi les messages du fil.
    pub score: f32,
    /// Message ayant obtenu ce score.
    pub best_message: MessageId,
    /// Extrait du corps entourant la correspondance, balisé pour l'affichage.
    pub snippet: String,
}

#[derive(Debug, Clone, Copy)]
struct Fields {
    message: Field,
    thread: Field,
    account: Field,
    subject: Field,
    from: Field,
    recipients: Field,
    body: Field,
    received: Field,
}

/// L'index plein texte.
pub struct SearchIndex {
    index: Index,
    reader: IndexReader,
    writer: Mutex<IndexWriter>,
    fields: Fields,
    /// Documents ajoutés depuis la dernière validation.
    uncommitted: Mutex<u64>,
}

// Ni le lecteur ni l'écrivain de Tantivy n'implémentent Debug ; on expose ce qui a
// un sens pour le diagnostic plutôt que de renoncer au trait.
impl std::fmt::Debug for SearchIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchIndex")
            .field("documents", &self.document_count())
            .field("uncommitted", &self.uncommitted())
            .finish()
    }
}

fn build_schema() -> (Schema, Fields) {
    let mut b = Schema::builder();

    // Le corps est indexé avec positions : sans elles, pas de recherche de phrase
    // exacte, qui est l'usage le plus fréquent quand on cherche un mail précis.
    let texte = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer("default")
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );

    let fields = Fields {
        // Stocké et rapide : on doit pouvoir supprimer un document par son terme.
        message: b.add_i64_field("message", INDEXED | STORED | FAST),
        thread: b.add_i64_field("thread", INDEXED | STORED | FAST),
        account: b.add_i64_field("account", INDEXED | FAST),
        subject: b.add_text_field("subject", texte.clone() | STORED),
        from: b.add_text_field("from", texte.clone()),
        recipients: b.add_text_field("recipients", texte.clone()),
        // Le corps est indexé et stocké : le stocker permet de produire un extrait
        // sans relire le fichier compressé, ce qui coûterait une décompression par
        // résultat affiché.
        body: b.add_text_field("body", texte | STORED),
        received: b.add_i64_field("received", INDEXED | STORED | FAST),
    };
    let _ = b.add_text_field("_reserved", STRING);

    (b.build(), fields)
}

impl SearchIndex {
    /// Ouvre ou crée l'index sur le disque.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        std::fs::create_dir_all(path)?;
        let (schema, fields) = build_schema();

        let dir = tantivy::directory::MmapDirectory::open(path)
            .map_err(|e| Error::Index(format!("ouverture du répertoire : {e}")))?;
        let index = Index::open_or_create(dir, schema)
            .map_err(|e| Error::Index(format!("ouverture de l'index : {e}")))?;

        Self::from_index(index, fields)
    }

    /// Index en mémoire, pour les tests.
    pub fn in_memory() -> Result<Self> {
        let (schema, fields) = build_schema();
        Self::from_index(Index::create_in_ram(schema), fields)
    }

    fn from_index(index: Index, fields: Fields) -> Result<Self> {
        let writer: IndexWriter = index
            .writer(WRITER_HEAP)
            .map_err(|e| Error::Index(format!("création de l'écrivain : {e}")))?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|e| Error::Index(format!("création du lecteur : {e}")))?;

        Ok(Self { index, reader, writer: Mutex::new(writer), fields, uncommitted: Mutex::new(0) })
    }

    fn writer(&self) -> Result<std::sync::MutexGuard<'_, IndexWriter>> {
        self.writer.lock().map_err(|_| Error::Index("écrivain empoisonné".into()))
    }

    /// Ajoute ou remplace un message dans l'index.
    ///
    /// Les modifications ne sont visibles qu'après [`commit`](Self::commit) : c'est
    /// ce qui permet d'indexer un lot de synchronisation entier en une seule
    /// opération coûteuse plutôt qu'une par message.
    pub fn add(&self, m: &IndexedMessage) -> Result<()> {
        let f = self.fields;
        let writer = self.writer()?;

        // Remplacement plutôt qu'ajout : réindexer un message dont les drapeaux ont
        // changé ne doit pas le faire apparaître deux fois.
        writer.delete_term(Term::from_field_i64(f.message, m.message.get()));

        writer
            .add_document(doc!(
                f.message    => m.message.get(),
                f.thread     => m.thread.get(),
                f.account    => m.account.get(),
                f.subject    => m.subject.clone(),
                f.from       => m.from.clone(),
                f.recipients => m.recipients.clone(),
                f.body       => m.body.clone(),
                f.received   => m.received.millis(),
            ))
            .map_err(|e| Error::Index(format!("ajout du document : {e}")))?;

        *self.uncommitted.lock().map_err(|_| Error::Index("compteur empoisonné".into()))? += 1;
        Ok(())
    }

    pub fn add_batch(&self, messages: &[IndexedMessage]) -> Result<()> {
        for m in messages {
            self.add(m)?;
        }
        Ok(())
    }

    pub fn remove_message(&self, id: MessageId) -> Result<()> {
        let writer = self.writer()?;
        writer.delete_term(Term::from_field_i64(self.fields.message, id.get()));
        *self.uncommitted.lock().map_err(|_| Error::Index("compteur empoisonné".into()))? += 1;
        Ok(())
    }

    /// Retire de l'index tous les messages d'un compte supprimé.
    pub fn remove_account(&self, id: AccountId) -> Result<()> {
        let writer = self.writer()?;
        writer.delete_term(Term::from_field_i64(self.fields.account, id.get()));
        *self.uncommitted.lock().map_err(|_| Error::Index("compteur empoisonné".into()))? += 1;
        Ok(())
    }

    /// Rend visibles les modifications en attente.
    pub fn commit(&self) -> Result<u64> {
        let n = {
            let mut c = self.uncommitted.lock().map_err(|_| Error::Index("compteur".into()))?;
            std::mem::take(&mut *c)
        };
        if n == 0 {
            return Ok(0);
        }
        self.writer()?
            .commit()
            .map_err(|e| Error::Index(format!("validation : {e}")))?;
        self.reader
            .reload()
            .map_err(|e| Error::Index(format!("rechargement du lecteur : {e}")))?;
        Ok(n)
    }

    pub fn uncommitted(&self) -> u64 {
        self.uncommitted.lock().map(|c| *c).unwrap_or(0)
    }

    /// Nombre de documents visibles.
    pub fn document_count(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    /// Recherche, agrégée par fil.
    ///
    /// `limit` borne le nombre de **fils** retournés ; l'index est interrogé plus
    /// largement pour que plusieurs messages d'un même fil ne consomment pas la page.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>> {
        self.search_filtered(query, limit, &[])
    }

    /// Recherche restreinte à certains comptes. Une liste vide signifie « tous ».
    pub fn search_filtered(
        &self,
        query: &str,
        limit: usize,
        accounts: &[AccountId],
    ) -> Result<Vec<Hit>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let f = self.fields;
        let searcher = self.reader.searcher();

        // Le sujet pèse plus que le corps : un mot dans le sujet est presque toujours
        // plus significatif que le même mot noyé dans une signature.
        let mut parser =
            QueryParser::for_index(&self.index, vec![f.subject, f.body, f.from, f.recipients]);
        parser.set_field_boost(f.subject, 3.0);
        parser.set_field_boost(f.from, 2.0);

        let parsed = parser
            .parse_query(query)
            .map_err(|e| Error::Index(format!("requête invalide : {e}")))?;

        // On collecte large pour pouvoir agréger : sans cela, un fil de cinquante
        // messages monopoliserait la première page.
        let over_fetch = (limit * 8).clamp(32, 2_000);
        let top = searcher
            .search(&parsed, &TopDocs::with_limit(over_fetch).order_by_score())
            .map_err(|e| Error::Index(format!("recherche : {e}")))?;

        let filtre: Option<std::collections::BTreeSet<i64>> = if accounts.is_empty() {
            None
        } else {
            Some(accounts.iter().map(|a| a.get()).collect())
        };

        let mut par_fil: BTreeMap<i64, Hit> = BTreeMap::new();
        let mut ordre: Vec<i64> = Vec::new();

        for (score, address) in top {
            let doc: TantivyDocument = searcher
                .doc(address)
                .map_err(|e| Error::Index(format!("lecture du document : {e}")))?;

            let get_i64 = |field: Field| -> Option<i64> {
                doc.get_first(field).and_then(|v| v.as_i64())
            };
            let get_str = |field: Field| -> String {
                doc.get_first(field).and_then(|v| v.as_str()).unwrap_or("").to_string()
            };

            let (Some(thread), Some(message)) = (get_i64(f.thread), get_i64(f.message)) else {
                continue;
            };

            if let Some(filtre) = &filtre {
                // Le compte n'est pas stocké : on le retrouve par la valeur rapide.
                let account = searcher
                    .segment_reader(address.segment_ord)
                    .fast_fields()
                    .i64("account")
                    .ok()
                    .and_then(|c| c.first(address.doc_id));
                match account {
                    Some(a) if filtre.contains(&a) => {}
                    _ => continue,
                }
            }

            let entry = par_fil.entry(thread).or_insert_with(|| {
                ordre.push(thread);
                Hit {
                    thread: ThreadId(thread),
                    score,
                    best_message: MessageId(message),
                    snippet: make_snippet(&get_str(f.body), query),
                }
            });
            if score > entry.score {
                entry.score = score;
                entry.best_message = MessageId(message);
                entry.snippet = make_snippet(&get_str(f.body), query);
            }
        }

        let mut hits: Vec<Hit> = ordre
            .into_iter()
            .filter_map(|t| par_fil.remove(&t))
            .collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then(b.thread.0.cmp(&a.thread.0)));
        hits.truncate(limit);
        Ok(hits)
    }
}

/// Extrait du corps entourant le premier terme trouvé.
///
/// Volontairement simple et sans dépendance : une fenêtre autour de la première
/// occurrence. Un surlignage complet exigerait de rejouer l'analyse lexicale, pour un
/// gain nul dans une liste où l'extrait tient sur une ligne.
fn make_snippet(body: &str, query: &str) -> String {
    const WINDOW: usize = 160;

    let terme = query
        .split_whitespace()
        .find(|t| !t.contains(':') && t.len() > 2)
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();

    let corps: Vec<char> = body.chars().collect();
    if corps.is_empty() {
        return String::new();
    }

    let debut = if terme.is_empty() {
        0
    } else {
        body.to_lowercase()
            .find(&terme)
            // Position en caractères, et non en octets : couper au milieu d'un
            // caractère accentué produirait un extrait invalide.
            .map(|byte_pos| body[..byte_pos].chars().count())
            .map(|pos| pos.saturating_sub(WINDOW / 3))
            .unwrap_or(0)
    };

    let fin = (debut + WINDOW).min(corps.len());
    let mut extrait: String = corps[debut..fin].iter().collect();

    if debut > 0 {
        extrait.insert(0, '…');
    }
    if fin < corps.len() {
        extrait.push('…');
    }
    extrait.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(message: i64, thread: i64, subject: &str, body: &str) -> IndexedMessage {
        IndexedMessage {
            message: MessageId(message),
            thread: ThreadId(thread),
            account: AccountId(1),
            subject: subject.into(),
            from: "Marie Vasseur <marie@example.com>".into(),
            recipients: "moi@example.com".into(),
            body: body.into(),
            received: Timestamp::from_millis(1000 * message),
            has_attachment: false,
        }
    }

    fn index_with(messages: &[IndexedMessage]) -> SearchIndex {
        let idx = SearchIndex::in_memory().unwrap();
        idx.add_batch(messages).unwrap();
        idx.commit().unwrap();
        idx
    }

    #[test]
    fn rien_n_est_visible_avant_validation() {
        let idx = SearchIndex::in_memory().unwrap();
        idx.add(&msg(1, 1, "Devis", "le contenu du devis")).unwrap();
        assert_eq!(idx.document_count(), 0);
        assert_eq!(idx.uncommitted(), 1);

        assert_eq!(idx.commit().unwrap(), 1);
        assert_eq!(idx.document_count(), 1);
        assert_eq!(idx.uncommitted(), 0);
    }

    #[test]
    fn valider_sans_rien_faire_est_gratuit() {
        let idx = SearchIndex::in_memory().unwrap();
        assert_eq!(idx.commit().unwrap(), 0);
    }

    #[test]
    fn recherche_dans_le_corps_et_le_sujet() {
        let idx = index_with(&[
            msg(1, 1, "Devis refonte", "Voici le montant proposé pour la prestation."),
            msg(2, 2, "Facture", "Le paiement a bien été reçu, merci."),
        ]);

        let hits = idx.search("paiement", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].thread, ThreadId(2));

        let hits = idx.search("devis", 10).unwrap();
        assert_eq!(hits[0].thread, ThreadId(1));
    }

    #[test]
    fn une_requete_vide_ne_retourne_rien() {
        let idx = index_with(&[msg(1, 1, "Devis", "corps")]);
        assert!(idx.search("", 10).unwrap().is_empty());
        assert!(idx.search("   ", 10).unwrap().is_empty());
    }

    #[test]
    fn les_resultats_sont_agreges_par_fil() {
        // Cinquante messages d'un même fil ne doivent pas remplir la page.
        let messages: Vec<_> =
            (1..=50).map(|i| msg(i, 1, "Devis refonte", "prestation proposée")).collect();
        let idx = index_with(&messages);

        let hits = idx.search("prestation", 10).unwrap();
        assert_eq!(hits.len(), 1, "un fil, un résultat");
        assert_eq!(hits[0].thread, ThreadId(1));
    }

    #[test]
    fn le_sujet_pese_plus_que_le_corps() {
        let idx = index_with(&[
            msg(1, 1, "Sujet neutre", "le mot rare apparaît ici dans le corps du message"),
            msg(2, 2, "rare", "un corps sans rapport"),
        ]);
        let hits = idx.search("rare", 10).unwrap();
        assert_eq!(hits[0].thread, ThreadId(2), "la correspondance dans le sujet prime");
    }

    #[test]
    fn reindexer_un_message_ne_le_duplique_pas() {
        let idx = SearchIndex::in_memory().unwrap();
        idx.add(&msg(1, 1, "Devis", "version initiale")).unwrap();
        idx.commit().unwrap();
        idx.add(&msg(1, 1, "Devis", "version corrigée")).unwrap();
        idx.commit().unwrap();

        assert_eq!(idx.document_count(), 1);
        assert!(idx.search("initiale", 10).unwrap().is_empty());
        assert_eq!(idx.search("corrigée", 10).unwrap().len(), 1);
    }

    #[test]
    fn supprimer_un_message_le_retire_des_resultats() {
        let idx = index_with(&[msg(1, 1, "Devis", "contenu unique")]);
        idx.remove_message(MessageId(1)).unwrap();
        idx.commit().unwrap();
        assert!(idx.search("unique", 10).unwrap().is_empty());
        assert_eq!(idx.document_count(), 0);
    }

    #[test]
    fn supprimer_un_compte_retire_tous_ses_messages() {
        let idx = SearchIndex::in_memory().unwrap();
        let mut autre = msg(2, 2, "Autre", "contenu partagé");
        autre.account = AccountId(2);
        idx.add(&msg(1, 1, "Premier", "contenu partagé")).unwrap();
        idx.add(&autre).unwrap();
        idx.commit().unwrap();
        assert_eq!(idx.search("partagé", 10).unwrap().len(), 2);

        idx.remove_account(AccountId(2)).unwrap();
        idx.commit().unwrap();
        let restant = idx.search("partagé", 10).unwrap();
        assert_eq!(restant.len(), 1);
        assert_eq!(restant[0].thread, ThreadId(1));
    }

    #[test]
    fn le_filtre_par_compte_restreint_les_resultats() {
        let idx = SearchIndex::in_memory().unwrap();
        let mut autre = msg(2, 2, "Autre", "contenu partagé");
        autre.account = AccountId(2);
        idx.add(&msg(1, 1, "Premier", "contenu partagé")).unwrap();
        idx.add(&autre).unwrap();
        idx.commit().unwrap();

        let hits = idx.search_filtered("partagé", 10, &[AccountId(2)]).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].thread, ThreadId(2));
    }

    #[test]
    fn une_requete_syntaxiquement_invalide_est_signalee() {
        let idx = index_with(&[msg(1, 1, "Devis", "corps")]);
        let e = idx.search("champ_inconnu:valeur", 10).unwrap_err();
        assert!(e.to_string().contains("requête invalide"));
    }

    #[test]
    fn l_extrait_entoure_le_terme_trouve() {
        let corps = "a".repeat(300) + " trouvaille " + &"b".repeat(300);
        let extrait = make_snippet(&corps, "trouvaille");
        assert!(extrait.contains("trouvaille"));
        assert!(extrait.starts_with('…'), "le début est tronqué");
        assert!(extrait.chars().count() <= 170);
    }

    #[test]
    fn l_extrait_ne_coupe_pas_au_milieu_d_un_caractere_accentue() {
        // Découper sur des positions en octets produirait ici une chaîne invalide.
        let corps = "é".repeat(400) + " cible";
        let extrait = make_snippet(&corps, "cible");
        assert!(extrait.contains("cible"));
    }

    #[test]
    fn un_corps_vide_donne_un_extrait_vide() {
        assert_eq!(make_snippet("", "quoi"), "");
    }

    #[test]
    fn l_index_persiste_sur_le_disque() {
        let dir = tempfile::tempdir().unwrap();
        {
            let idx = SearchIndex::open(dir.path()).unwrap();
            idx.add(&msg(1, 1, "Devis", "contenu persistant")).unwrap();
            idx.commit().unwrap();
        }
        let idx = SearchIndex::open(dir.path()).unwrap();
        assert_eq!(idx.document_count(), 1);
        assert_eq!(idx.search("persistant", 10).unwrap().len(), 1);
    }
}
