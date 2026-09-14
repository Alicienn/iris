//! Téléchargement du corps d'un message.
//!
//! Les en-têtes arrivent à la synchronisation, les corps **à l'ouverture**. C'est ce
//! qui permet de synchroniser un million de messages sans télécharger des dizaines de
//! gigaoctets, et c'est aussi ce qui impose une règle : *afficher une conversation ne
//! doit jamais attendre le réseau*. L'interface montre l'aperçu — toujours présent —
//! pendant que le corps arrive, et se redessine quand il est là.
//!
//! Le contenu est adressé par son empreinte : rouvrir un message ne retélécharge
//! rien, et deux messages identiques ne sont stockés qu'une fois.

use crate::engine::SyncEngine;
use iris_blobs::BlobStore;
use iris_store::Store;
use iris_types::{BlobId, Error, MessageId, Result};
use std::sync::Arc;

/// Ce qu'un téléchargement a produit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedBody {
    pub message: MessageId,
    pub blob: BlobId,
    /// Le corps était déjà en cache : aucun octet n'a transité.
    pub from_cache: bool,
    pub bytes: usize,
}

impl SyncEngine {
    /// Télécharge le corps d'un message, ou le rend depuis le cache.
    pub async fn fetch_body(&self, message: MessageId) -> Result<FetchedBody> {
        let stocke = self
            .store()
            .message_by_id(message)?
            .ok_or_else(|| Error::store(format!("message {message} introuvable")))?;

        // Déjà téléchargé et toujours en cache : rien à faire. Le cache peut avoir
        // évincé le contenu, auquel cas on retélécharge — c'est précisément ce que
        // permet un cache borné.
        if let Some(hex) = &stocke.body_blob {
            if let Some(id) = BlobId::from_hex(hex) {
                if let Some(contenu) = self.blobs().and_then(|b| b.get(id).ok().flatten()) {
                    return Ok(FetchedBody {
                        message,
                        blob: id,
                        from_cache: true,
                        bytes: contenu.len(),
                    });
                }
            }
        }

        let blobs = self
            .blobs()
            .ok_or_else(|| Error::Config("aucun magasin de contenus configuré".into()))?;

        let compte = self
            .store()
            .account(stocke.account)?
            .ok_or_else(|| Error::store(format!("compte {} introuvable", stocke.account)))?;

        let dossier = self
            .store()
            .folders(stocke.account)?
            .into_iter()
            .find(|f| f.id == stocke.folder)
            .ok_or_else(|| Error::store("dossier du message introuvable"))?;

        // La place est réservée comme pour toute autre opération réseau : un
        // téléchargement de corps ne doit pas contourner le plafond de connexions.
        let _place = self.pool().acquire(&compte.imap_host).await?;

        let identifiants = self.credentials_for(&compte).await?;
        let point = self.endpoint_for(&compte);
        let mut conn = self.connector().connect(&point, &identifiants).await?;

        conn.select(&dossier.path).await?;
        let brut = conn.fetch_body(stocke.uid).await?;
        let _ = conn.logout().await;

        if brut.is_empty() {
            return Err(Error::Protocol {
                protocol: "IMAP",
                message: format!("corps vide pour l'UID {}", stocke.uid),
            });
        }

        let blob = blobs.put(&brut)?;
        self.store().attach_body(message, &blob.to_hex())?;

        // Le corps complet remplace l'indexation partielle faite à la
        // synchronisation : la recherche gagne le texte, pas seulement l'en-tête.
        self.reindex_with_body(&stocke, &brut)?;

        // Les pièces jointes n'existent qu'à partir d'ici : l'enveloppe ne dit pas
        // ce qu'un message contient, seul le corps complet le dit.
        self.record_attachments(&stocke, &brut);

        Ok(FetchedBody { message, blob, from_cache: false, bytes: brut.len() })
    }

    /// Télécharge les corps de tous les messages d'un fil.
    ///
    /// Un échec sur un message n'interrompt pas les autres : mieux vaut afficher
    /// trois messages sur quatre qu'un panneau vide.
    pub async fn fetch_thread_bodies(
        &self,
        thread: iris_types::ThreadId,
    ) -> Vec<(MessageId, Result<FetchedBody>)> {
        let messages = match self.store().thread_messages(thread) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(fil = %thread, erreur = %e, "lecture du fil");
                return Vec::new();
            }
        };

        let mut resultats = Vec::with_capacity(messages.len());
        for m in messages {
            resultats.push((m.id, self.fetch_body(m.id).await));
        }
        resultats
    }
}

/// Purge les contenus dont plus aucun message ne se réclame.
///
/// Le cache s'évince tout seul par ancienneté, mais un message supprimé laisse son
/// contenu orphelin : personne ne le lira plus, et il occupe la place de contenus
/// encore utiles.
pub fn purge_orphan_bodies(store: &Store, blobs: &BlobStore) -> Result<usize> {
    let references = store.referenced_blobs()?;
    let connus: std::collections::BTreeSet<BlobId> =
        references.iter().filter_map(|h| BlobId::from_hex(h)).collect();

    let mut supprimes = 0;
    for id in blobs.ids()? {
        if !connus.contains(&id) && blobs.remove(id)? {
            supprimes += 1;
        }
    }
    Ok(supprimes)
}

/// Rend un magasin de contenus partagé.
pub type SharedBlobs = Arc<BlobStore>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineConfig, StaticCredentials, SyncEngine};
    use iris_imap::fake::FakeServer;
    use iris_imap::Connector;
    use iris_kernel::EventBus;
    use iris_store::NewAccount;
    use iris_types::{Flags, Timestamp};

    /// Un message MIME portant une pièce jointe et une image incrustée.
    fn message_avec_pieces(sujet: &str) -> Vec<u8> {
        format!(
            "Subject: {sujet}\r\nFrom: Marie <marie@example.com>\r\n\
             Message-ID: <{sujet}@x>\r\n\
             MIME-Version: 1.0\r\n\
             Content-Type: multipart/mixed; boundary=\"SEP\"\r\n\r\n\
             --SEP\r\nContent-Type: text/plain\r\n\r\nVoici le devis.\r\n\
             --SEP\r\nContent-Type: application/pdf\r\n\
             Content-Disposition: attachment; filename=\"devis.pdf\"\r\n\r\n\
             %PDF-faux\r\n\
             --SEP\r\nContent-Type: image/png\r\n\
             Content-ID: <logo>\r\n\
             Content-Disposition: inline; filename=\"logo.png\"\r\n\r\n\
             PNG-faux\r\n\
             --SEP--\r\n"
        )
        .into_bytes()
    }

    fn message(sujet: &str, corps: &str) -> Vec<u8> {
        format!(
            "Subject: {sujet}\r\nFrom: Marie <marie@example.com>\r\n\
             Message-ID: <{sujet}@x>\r\n\r\n{corps}\r\n"
        )
        .into_bytes()
    }

    struct Fixture {
        engine: SyncEngine,
        store: Arc<Store>,
        blobs: Arc<BlobStore>,
        index: Arc<iris_index::SearchIndex>,
        server: Arc<FakeServer>,
        _dir: tempfile::TempDir,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::in_memory().unwrap());
        let blobs = Arc::new(BlobStore::open(dir.path().join("blobs"), 1 << 20).unwrap());
        let index = Arc::new(iris_index::SearchIndex::in_memory().unwrap());

        store
            .create_account(
                &NewAccount::new("moi@example.com", "imap.x.fr", "smtp.x.fr"),
                Timestamp::EPOCH,
            )
            .unwrap();

        let server = Arc::new(FakeServer::default());
        let engine = SyncEngine::new(
            Arc::clone(&store),
            Arc::clone(&server) as Arc<dyn Connector>,
            Arc::new(StaticCredentials::new("motdepasse")),
            EventBus::new(),
            EngineConfig::default(),
        )
        .with_blobs(Arc::clone(&blobs))
        .with_index(Arc::clone(&index));

        Fixture { engine, store, blobs, index, server, _dir: dir }
    }

    impl Fixture {
        async fn synchroniser(&self) {
            self.engine.load_accounts(Timestamp::EPOCH).await.unwrap();
            self.engine.tick(Timestamp::EPOCH).await;
        }

        fn premier_message(&self) -> MessageId {
            self.store.thread_messages(iris_types::ThreadId(1)).unwrap()[0].id
        }
    }

    #[tokio::test]
    async fn un_corps_est_telecharge_et_mis_en_cache() {
        let f = fixture();
        f.server.deliver("INBOX", &message("devis", "Le contenu complet du message."), Flags::NONE);
        f.synchroniser().await;

        let id = f.premier_message();
        let rapport = f.engine.fetch_body(id).await.unwrap();

        assert!(!rapport.from_cache);
        assert!(rapport.bytes > 0);
        let contenu = f.blobs.get(rapport.blob).unwrap().unwrap();
        assert!(String::from_utf8_lossy(&contenu).contains("Le contenu complet"));
    }

    #[tokio::test]
    async fn le_corps_est_rattache_au_message() {
        let f = fixture();
        f.server.deliver("INBOX", &message("devis", "Contenu."), Flags::NONE);
        f.synchroniser().await;

        let id = f.premier_message();
        f.engine.fetch_body(id).await.unwrap();

        let stocke = f.store.message_by_id(id).unwrap().unwrap();
        assert!(stocke.body_blob.is_some());
    }

    #[tokio::test]
    async fn un_second_appel_sert_le_cache_sans_reseau() {
        let f = fixture();
        f.server.deliver("INBOX", &message("devis", "Contenu."), Flags::NONE);
        f.synchroniser().await;

        let id = f.premier_message();
        f.engine.fetch_body(id).await.unwrap();
        let connexions = f.server.connection_count();

        let second = f.engine.fetch_body(id).await.unwrap();
        assert!(second.from_cache);
        assert_eq!(f.server.connection_count(), connexions, "aucune connexion nouvelle");
    }

    #[tokio::test]
    async fn un_contenu_evince_est_retelecharge() {
        // C'est précisément ce que permet un cache borné : oublier sans perdre.
        let f = fixture();
        f.server.deliver("INBOX", &message("devis", "Contenu."), Flags::NONE);
        f.synchroniser().await;

        let id = f.premier_message();
        let premier = f.engine.fetch_body(id).await.unwrap();
        f.blobs.remove(premier.blob).unwrap();

        let second = f.engine.fetch_body(id).await.unwrap();
        assert!(!second.from_cache);
        assert_eq!(second.blob, premier.blob, "même contenu, même empreinte");
    }

    #[tokio::test]
    async fn le_corps_enrichit_l_index() {
        // À la synchronisation, seuls les en-têtes sont indexés ; le corps ajoute le
        // texte, et c'est lui qu'on cherche le plus souvent.
        let f = fixture();
        f.server.deliver(
            "INBOX",
            &message("devis", "La formule retenue est celle du forfait annuel."),
            Flags::NONE,
        );
        f.synchroniser().await;
        f.index.commit().unwrap();

        assert!(f.index.search("forfait", 10).unwrap().is_empty(), "le corps n'est pas encore là");

        f.engine.fetch_body(f.premier_message()).await.unwrap();
        f.index.commit().unwrap();

        assert_eq!(f.index.search("forfait", 10).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn un_message_inconnu_est_signale() {
        let f = fixture();
        let e = f.engine.fetch_body(MessageId(999)).await.unwrap_err();
        assert!(e.to_string().contains("introuvable"));
    }

    #[tokio::test]
    async fn un_serveur_injoignable_est_une_erreur_temporaire() {
        let f = fixture();
        f.server.deliver("INBOX", &message("devis", "Contenu."), Flags::NONE);
        f.synchroniser().await;
        let id = f.premier_message();

        f.server.refuse_connections(true);
        let e = f.engine.fetch_body(id).await.unwrap_err();
        assert!(e.is_transient(), "l'interface doit pouvoir réessayer");
    }

    #[tokio::test]
    async fn les_corps_d_un_fil_sont_tous_telecharges() {
        let f = fixture();
        let racine = message("devis", "Premier message.");
        f.server.deliver("INBOX", &racine, Flags::NONE);
        f.server.deliver(
            "INBOX",
            format!(
                "Subject: Re: devis\r\nFrom: Luc <luc@x.fr>\r\nMessage-ID: <r@x>\r\n\
                 In-Reply-To: <devis@x>\r\n\r\nRéponse.\r\n"
            )
            .as_bytes(),
            Flags::NONE,
        );
        f.synchroniser().await;

        let resultats = f.engine.fetch_thread_bodies(iris_types::ThreadId(1)).await;
        assert_eq!(resultats.len(), 2);
        assert!(resultats.iter().all(|(_, r)| r.is_ok()));
    }

    #[tokio::test]
    async fn un_echec_isole_n_interrompt_pas_le_fil() {
        let f = fixture();
        f.server.deliver("INBOX", &message("a", "Un."), Flags::NONE);
        f.server.deliver(
            "INBOX",
            b"Subject: Re: a\r\nMessage-ID: <b@x>\r\nIn-Reply-To: <a@x>\r\n\r\nDeux.\r\n",
            Flags::NONE,
        );
        f.synchroniser().await;

        // Le second téléchargement échouera.
        f.server.fail_next("serveur occupé");
        let resultats = f.engine.fetch_thread_bodies(iris_types::ThreadId(1)).await;

        assert_eq!(resultats.len(), 2);
        assert!(resultats.iter().any(|(_, r)| r.is_ok()), "au moins un doit passer");
    }

    #[tokio::test]
    async fn la_purge_retire_les_contenus_orphelins() {
        let f = fixture();
        f.server.deliver("INBOX", &message("devis", "Contenu."), Flags::NONE);
        f.synchroniser().await;
        f.engine.fetch_body(f.premier_message()).await.unwrap();

        // Un contenu qui n'appartient à aucun message.
        f.blobs.put(b"orphelin").unwrap();
        assert_eq!(f.blobs.stats().unwrap().count, 2);

        assert_eq!(purge_orphan_bodies(&f.store, &f.blobs).unwrap(), 1);
        assert_eq!(f.blobs.stats().unwrap().count, 1);
    }

    #[tokio::test]
    async fn la_purge_epargne_les_contenus_utiles() {
        let f = fixture();
        f.server.deliver("INBOX", &message("devis", "Contenu."), Flags::NONE);
        f.synchroniser().await;
        f.engine.fetch_body(f.premier_message()).await.unwrap();

        assert_eq!(purge_orphan_bodies(&f.store, &f.blobs).unwrap(), 0);
        assert_eq!(f.blobs.stats().unwrap().count, 1);
    }

    #[tokio::test]
    async fn le_corps_revele_les_pieces_jointes() {
        // L'enveloppe ne les connaît pas : seul le corps complet dit ce qu'un
        // message contient.
        let f = fixture();
        f.server.deliver("INBOX", &message_avec_pieces("Devis"), Flags::NONE);
        f.synchroniser().await;

        let message = f.premier_message();
        assert!(f.store.attachments(message).unwrap().is_empty(), "rien avant le corps");

        f.engine.fetch_body(message).await.unwrap();

        let pieces = f.store.attachments(message).unwrap();
        assert_eq!(pieces.len(), 2, "le PDF et l'image incrustée");
        assert_eq!(pieces[0].meta.filename, "devis.pdf");
    }

    #[tokio::test]
    async fn seules_les_pieces_utiles_sont_listees() {
        let f = fixture();
        f.server.deliver("INBOX", &message_avec_pieces("Devis"), Flags::NONE);
        f.synchroniser().await;

        let message = f.premier_message();
        f.engine.fetch_body(message).await.unwrap();

        let visibles = f.store.visible_attachments(message).unwrap();
        assert_eq!(visibles.len(), 1);
        assert_eq!(visibles[0].meta.filename, "devis.pdf");
    }

    #[tokio::test]
    async fn les_octets_se_ressortent_du_message_brut() {
        // Ils ne sont pas dupliqués dans la base : le message les contient déjà.
        let f = fixture();
        f.server.deliver("INBOX", &message_avec_pieces("Devis"), Flags::NONE);
        f.synchroniser().await;

        let message = f.premier_message();
        let corps = f.engine.fetch_body(message).await.unwrap();
        let brut = f.blobs.get(corps.blob).unwrap().unwrap();

        let piece = f.store.visible_attachments(message).unwrap().remove(0);
        let octets = iris_mime::attachment_bytes(&brut, piece.index).unwrap();
        assert!(
            String::from_utf8_lossy(&octets).contains("%PDF-faux"),
            "le rang doit désigner le bon fichier"
        );
    }

    #[tokio::test]
    async fn retelecharger_un_corps_ne_duplique_pas_les_pieces() {
        let f = fixture();
        f.server.deliver("INBOX", &message_avec_pieces("Devis"), Flags::NONE);
        f.synchroniser().await;

        let message = f.premier_message();
        f.engine.fetch_body(message).await.unwrap();
        f.engine.fetch_body(message).await.unwrap();

        assert_eq!(f.store.attachments(message).unwrap().len(), 2);
    }

    #[tokio::test]
    async fn un_message_sans_piece_n_en_invente_pas() {
        let f = fixture();
        f.server.deliver("INBOX", &message("Bonjour", "Rien de particulier."), Flags::NONE);
        f.synchroniser().await;

        let message = f.premier_message();
        f.engine.fetch_body(message).await.unwrap();
        assert!(f.store.attachments(message).unwrap().is_empty());
    }
}
