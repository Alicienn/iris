//! Les pièces jointes recensées.
//!
//! Seules les **métadonnées** vivent ici : nom, type, taille, rang. Les octets sont
//! déjà dans le message brut, lui-même dans le magasin de contenus ; les recopier
//! doublerait la place occupée par toutes les pièces jointes de la boîte, pour une
//! information qu'on sait reconstituer en une analyse.
//!
//! Le rang est ce qui relie une ligne à son contenu. Il vient de l'ordre des parties
//! dans le message, qui ne change pas : le message est immuable une fois reçu.

use crate::{sql_err, Store};
use iris_types::{AttachmentMeta, MessageId, Result};

/// Une pièce jointe, telle qu'elle est rangée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAttachment {
    pub meta: AttachmentMeta,
    /// Rang dans le message, pour retrouver les octets.
    pub index: usize,
}

impl Store {
    /// Recense les pièces jointes d'un message.
    ///
    /// Idempotent : rejouer l'analyse du même message ne crée pas de doublons. Le
    /// corps peut être retéléchargé, et il le sera.
    pub fn record_attachments(
        &self,
        message: MessageId,
        attachments: &[AttachmentMeta],
    ) -> Result<usize> {
        self.with_conn(|c| {
            c.prepare_cached("DELETE FROM attachments WHERE message_id = ?1")
                .map_err(|e| sql_err("préparation", e))?
                .execute([message.get()])
                .map_err(|e| sql_err("effacement des pièces jointes", e))?;

            let mut insertion = c
                .prepare_cached(
                    "INSERT INTO attachments
                         (message_id, filename, mime_type, size, blob, inline)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(|e| sql_err("préparation", e))?;

            for piece in attachments {
                insertion
                    .execute(rusqlite::params![
                        message.get(),
                        piece.filename,
                        piece.mime_type,
                        piece.size as i64,
                        piece.blob.map(|b| b.to_hex()),
                        piece.inline as i64,
                    ])
                    .map_err(|e| sql_err("recensement d'une pièce jointe", e))?;
            }

            Ok(attachments.len())
        })
    }

    /// Les pièces jointes d'un message, dans l'ordre du message.
    pub fn attachments(&self, message: MessageId) -> Result<Vec<StoredAttachment>> {
        self.with_conn(|c| {
            let mut requete = c
                .prepare_cached(
                    "SELECT filename, mime_type, size, blob, inline
                     FROM attachments WHERE message_id = ?1 ORDER BY id",
                )
                .map_err(|e| sql_err("préparation", e))?;

            let lignes = requete
                .query_map([message.get()], |row| {
                    Ok(AttachmentMeta {
                        filename: row.get(0)?,
                        mime_type: row.get(1)?,
                        size: row.get::<_, i64>(2)? as u64,
                        blob: row
                            .get::<_, Option<String>>(3)?
                            .and_then(|h| iris_types::BlobId::from_hex(&h)),
                        inline: row.get::<_, i64>(4)? != 0,
                    })
                })
                .map_err(|e| sql_err("lecture des pièces jointes", e))?;

            let mut sortie = Vec::new();
            for (index, ligne) in lignes.enumerate() {
                let meta = ligne.map_err(|e| sql_err("lecture d'une pièce jointe", e))?;
                sortie.push(StoredAttachment { meta, index });
            }
            Ok(sortie)
        })
    }

    /// Les seules qui intéressent l'utilisateur : celles qui ne sont pas incrustées
    /// dans le corps.
    ///
    /// Une image référencée par `cid:` est une pièce jointe pour MIME, pas pour
    /// quelqu'un qui lit son courrier : la lister ferait passer une signature à
    /// logo pour un document reçu.
    pub fn visible_attachments(&self, message: MessageId) -> Result<Vec<StoredAttachment>> {
        Ok(self.attachments(message)?.into_iter().filter(|a| !a.meta.inline).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FolderRole, NewAccount, NewMessage};
    use iris_types::{Flags, Timestamp};

    fn fixture() -> (Store, MessageId) {
        let store = Store::in_memory().unwrap();
        let compte = store
            .create_account(&NewAccount::new("a@x.fr", "i", "s"), Timestamp::EPOCH)
            .unwrap();
        let dossier = store.upsert_folder(compte, "INBOX", FolderRole::Inbox).unwrap();
        let insere = store
            .insert_message(&NewMessage {
                account: compte,
                folder: dossier,
                uid: 1,
                rfc_message_id: Some("m1@x".into()),
                in_reply_to: None,
                references: vec![],
                subject: "Devis".into(),
                from_name: "Marie".into(),
                from_addr: "marie@x.fr".into(),
                recipients_json: "[]".into(),
                date: Timestamp::EPOCH,
                received: Timestamp::EPOCH,
                size: 10,
                flags: Flags::NONE,
                preview: String::new(),
            })
            .unwrap();
        (store, insere.message)
    }

    fn piece(nom: &str, inline: bool) -> AttachmentMeta {
        AttachmentMeta {
            filename: nom.into(),
            mime_type: "application/pdf".into(),
            size: 1024,
            blob: None,
            inline,
        }
    }

    #[test]
    fn les_pieces_sont_recensees_dans_l_ordre() {
        // Le rang relie une ligne à ses octets : le perdre rendrait les fichiers
        // inaccessibles.
        let (store, message) = fixture();
        store
            .record_attachments(message, &[piece("a.pdf", false), piece("b.pdf", false)])
            .unwrap();

        let lues = store.attachments(message).unwrap();
        assert_eq!(lues.len(), 2);
        assert_eq!(lues[0].meta.filename, "a.pdf");
        assert_eq!(lues[0].index, 0);
        assert_eq!(lues[1].index, 1);
    }

    #[test]
    fn rejouer_l_analyse_ne_cree_pas_de_doublons() {
        // Le corps peut être retéléchargé, et il le sera.
        let (store, message) = fixture();
        let pieces = [piece("a.pdf", false)];

        store.record_attachments(message, &pieces).unwrap();
        store.record_attachments(message, &pieces).unwrap();

        assert_eq!(store.attachments(message).unwrap().len(), 1);
    }

    #[test]
    fn les_parties_incrustees_ne_sont_pas_listees() {
        // Une image de signature n'est pas un document reçu.
        let (store, message) = fixture();
        store
            .record_attachments(message, &[piece("logo.png", true), piece("devis.pdf", false)])
            .unwrap();

        let visibles = store.visible_attachments(message).unwrap();
        assert_eq!(visibles.len(), 1);
        assert_eq!(visibles[0].meta.filename, "devis.pdf");
        assert_eq!(visibles[0].index, 1, "le rang reste celui du message");
    }

    #[test]
    fn un_message_sans_piece_rend_une_liste_vide() {
        let (store, message) = fixture();
        assert!(store.attachments(message).unwrap().is_empty());
    }

    #[test]
    fn recenser_zero_piece_efface_les_precedentes() {
        let (store, message) = fixture();
        store.record_attachments(message, &[piece("a.pdf", false)]).unwrap();
        store.record_attachments(message, &[]).unwrap();

        assert!(store.attachments(message).unwrap().is_empty());
    }

    #[test]
    fn la_taille_et_le_type_sont_conserves() {
        let (store, message) = fixture();
        let mut p = piece("devis.pdf", false);
        p.size = 3_500_000;
        p.mime_type = "application/pdf".into();
        store.record_attachments(message, &[p]).unwrap();

        let lue = &store.attachments(message).unwrap()[0].meta;
        assert_eq!(lue.size, 3_500_000);
        assert_eq!(lue.mime_type, "application/pdf");
    }

    #[test]
    fn supprimer_le_message_emporte_ses_pieces() {
        let (store, message) = fixture();
        store.record_attachments(message, &[piece("a.pdf", false)]).unwrap();

        let compte = store.accounts().unwrap()[0].id;
        let dossier = store.folders(compte).unwrap()[0].id;
        store.delete_messages_by_uid(dossier, &[1]).unwrap();

        assert!(store.attachments(message).unwrap().is_empty());
    }
}
