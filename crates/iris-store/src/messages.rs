//! Insertion des messages et rattachement aux fils.
//!
//! Le rattachement suit la même règle que l'algorithme de threading (`iris-thread`),
//! mais interrogée du côté base : un message rejoint le fil de n'importe quel message
//! qu'il cite, ou celui de n'importe quel message qui le cite. Le second cas est
//! celui, fréquent, où la réponse arrive avant l'original — sur une boîte
//! synchronisée en désordre, l'ignorer produirait deux fils là où il n'y en a qu'un.

use crate::model::{Contact, NewMessage, StoredMessage};
use crate::{sql_err, Store};
use iris_types::{
    AccountId, Flags, FolderId, MessageId, Result, ThreadId, Timestamp, WorkflowState,
};
use rusqlite::{params, params_from_iter, OptionalExtension, Transaction};

/// Résultat de l'insertion d'un message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inserted {
    pub message: MessageId,
    pub thread: ThreadId,
    /// Le message existait déjà : seuls ses drapeaux ont pu changer.
    pub was_known: bool,
    /// Un fil a été créé pour l'occasion.
    pub thread_created: bool,
    /// A copy of a message already here, or one back from a move: not new mail.
    pub came_back: bool,
}

/// Normalise un sujet pour la comparaison : retire les préfixes de réponse et de
/// transfert, replie les espaces, passe en minuscules.
pub fn normalize_subject(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let lower = s.to_lowercase();
        let stripped = ["re:", "re :", "rép:", "rep:", "fwd:", "fw:", "tr:", "réf:"]
            .iter()
            .find_map(|p| lower.starts_with(p).then(|| s[p.len()..].trim_start()));
        match stripped {
            Some(rest) if rest.len() < s.len() => s = rest,
            _ => break,
        }
    }
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

impl Store {
    /// Insère un message, en le rattachant au bon fil.
    ///
    /// Idempotent sur `(dossier, uid)` : resynchroniser un dossier ne crée pas de
    /// doublons, seuls les drapeaux sont rafraîchis.
    pub fn insert_message(&self, m: &NewMessage) -> Result<Inserted> {
        self.with_tx(|tx| insert_message_tx(tx, m))
    }

    /// Insère un lot dans une transaction unique.
    ///
    /// Le gain n'est pas cosmétique : sur dix mille messages, une transaction par
    /// message multiplie le temps de synchronisation par un facteur à deux chiffres.
    /// Les agrégats des fils touchés ne sont recalculés qu'une fois par fil, à la
    /// fin du lot : recalculer à chaque message d'un même fil est le principal coût
    /// évitable d'une synchronisation initiale.
    pub fn insert_messages(&self, msgs: &[NewMessage]) -> Result<Vec<Inserted>> {
        self.with_tx(|tx| {
            let mut out = Vec::with_capacity(msgs.len());
            let mut touched = std::collections::BTreeSet::new();
            for m in msgs {
                let r = insert_message_tx_deferred(tx, m)?;
                touched.insert(r.thread);
                out.push(r);
            }
            for t in touched {
                refresh_thread(tx, t)?;
            }
            Ok(out)
        })
    }

    /// Keeps the copies (`Cc`) and the reply address (`Reply-To`) of messages of a
    /// folder, by UID, as JSON lists of addresses like the recipients.
    pub fn set_message_extras(
        &self,
        folder: FolderId,
        extras: &[(u32, String, String)],
    ) -> Result<()> {
        if extras.is_empty() {
            return Ok(());
        }
        self.with_tx(|tx| {
            let mut stmt = tx
                .prepare_cached(
                    "UPDATE messages SET cc = ?1, reply_to = ?2 WHERE folder_id = ?3 AND uid = ?4",
                )
                .map_err(|e| sql_err("préparation", e))?;
            for (uid, cc, reponse) in extras {
                stmt.execute(params![cc, reponse, folder.get(), *uid as i64])
                    .map_err(|e| sql_err("copies et réponse", e))?;
            }
            Ok(())
        })
    }

    /// Whether another row holds the same message (same `Message-ID`): a Gmail label,
    /// All Mail, or the copy a move left on its way. Such a copy is not an arrival.
    pub fn has_other_copy(&self, id: MessageId) -> Result<bool> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT EXISTS (SELECT 1 FROM messages o, messages m
                                WHERE m.id = ?1 AND m.rfc_message_id IS NOT NULL
                                  AND o.rfc_message_id = m.rfc_message_id AND o.id != m.id)",
                params![id.get()],
                |r| r.get::<_, bool>(0),
            )
            .map_err(|e| sql_err("copies du message", e))
        })
    }

    /// The thread a message of this `Message-ID` is in now, on this account: where a
    /// moved message came back under a new thread once its old copy was dropped.
    pub fn thread_of_message_id(
        &self,
        account: AccountId,
        message_id: &str,
    ) -> Result<Option<ThreadId>> {
        let id = message_id
            .trim()
            .trim_start_matches('<')
            .trim_end_matches('>');
        self.with_conn(|c| {
            c.query_row(
                "SELECT thread_id FROM messages
                 WHERE account_id = ?1 AND rfc_message_id = ?2
                 ORDER BY id DESC LIMIT 1",
                params![account.get(), id],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map(|t| t.map(ThreadId))
            .map_err(|e| sql_err("fil du message", e))
        })
    }

    /// A message's copies and reply address, as kept: `(cc, reply_to)` JSON lists.
    pub fn message_extras(&self, id: MessageId) -> Result<(String, String)> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT cc, reply_to FROM messages WHERE id = ?1",
                params![id.get()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| sql_err("copies et réponse", e))
        })
    }

    /// What a message's whole body says, once downloaded: its preview, and whether it
    /// has an attachment, a tracker, a way to unsubscribe. The headers alone gave
    /// guesses (every HTML newsletter looked as if it carried a file).
    pub fn set_body_facts(&self, id: MessageId, preview: &str, derived: Flags) -> Result<()> {
        const DU_CORPS: u32 =
            Flags::HAS_ATTACHMENT.0 | Flags::HAS_TRACKER.0 | Flags::UNSUBSCRIBABLE.0;
        self.with_tx(|tx| {
            let ligne: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT thread_id, flags FROM messages WHERE id = ?1",
                    params![id.get()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok();
            let Some((fil, drapeaux)) = ligne else {
                return Ok(());
            };
            let nouveaux = (drapeaux as u32 & !DU_CORPS) | (derived.0 & DU_CORPS);
            tx.execute(
                "UPDATE messages SET flags = ?1,
                        preview = CASE WHEN ?2 = '' THEN preview ELSE ?2 END
                 WHERE id = ?3",
                params![nouveaux as i64, preview, id.get()],
            )
            .map_err(|e| sql_err("ce que dit le corps", e))?;
            refresh_thread(tx, ThreadId(fil))
        })
    }

    /// Change les drapeaux d'un message et met le fil à jour.
    pub fn set_message_flags(&self, id: MessageId, flags: Flags) -> Result<Option<ThreadId>> {
        self.with_tx(|tx| {
            let thread: Option<i64> = tx
                .query_row(
                    "SELECT thread_id FROM messages WHERE id = ?1",
                    params![id.get()],
                    |r| r.get(0),
                )
                .ok();
            let Some(thread) = thread else {
                return Ok(None);
            };

            tx.execute(
                "UPDATE messages SET flags = ?1 WHERE id = ?2",
                params![flags.0 as i64, id.get()],
            )
            .map_err(|e| sql_err("mise à jour des drapeaux", e))?;

            refresh_thread(tx, ThreadId(thread))?;
            Ok(Some(ThreadId(thread)))
        })
    }

    /// Associe un corps téléchargé à un message.
    pub fn attach_body(&self, id: MessageId, blob_hex: &str) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE messages SET body_blob = ?1 WHERE id = ?2",
                params![blob_hex, id.get()],
            )
            .map_err(|e| sql_err("association du corps", e))?;
            Ok(())
        })
    }

    /// Les messages d'un fil, du plus ancien au plus récent.
    /// The most recently received messages, newest first.
    ///
    /// Used by the rules dry run, which needs a representative slice of history
    /// rather than all of it: the point is to tell the user what a rule would touch,
    /// and a few thousand recent messages answer that without reading a million rows.
    pub fn latest_messages(&self, limit: usize) -> Result<Vec<StoredMessage>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached(
                    "SELECT id, account_id, folder_id, thread_id, uid, rfc_message_id, subject,
                            from_name, from_addr, date, received, size, flags, preview, body_blob,
                            recipients
                     FROM messages ORDER BY received DESC, id DESC LIMIT ?1",
                )
                .map_err(|e| sql_err("préparation", e))?;

            let rows = stmt
                .query_map(params![limit as i64], stored_message_from_row)
                .map_err(|e| sql_err("lecture des messages récents", e))?;

            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("lecture d'un message", e))
        })
    }

    pub fn thread_messages(&self, thread: ThreadId) -> Result<Vec<StoredMessage>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, account_id, folder_id, thread_id, uid, rfc_message_id, subject,
                            from_name, from_addr, date, received, size, flags, preview, body_blob,
                            recipients
                     FROM messages WHERE thread_id = ?1 ORDER BY received ASC, id ASC",
                )
                .map_err(|e| sql_err("préparation", e))?;
            let rows = stmt
                .query_map(params![thread.get()], stored_message_from_row)
                .map_err(|e| sql_err("messages du fil", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("messages du fil", e))
        })
    }

    /// A thread as it is read: one copy of each message.
    ///
    /// `thread_messages` gives every copy, which is what moving or marking needs (each
    /// folder is told). Reading needs each message once: on Gmail every message is in
    /// the inbox and in All Mail. The copy kept is one whose body is here, else the
    /// first stored.
    pub fn conversation(&self, thread: ThreadId) -> Result<Vec<StoredMessage>> {
        Ok(one_copy_each(self.thread_messages(thread)?))
    }

    /// Supprime les messages d'un dossier dont l'UID n'est plus présent côté serveur.
    /// The local ids of a folder's messages by their UIDs, for what must follow them
    /// out (the search index).
    pub fn message_ids_by_uid(&self, folder: FolderId, uids: &[u32]) -> Result<Vec<MessageId>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached("SELECT id FROM messages WHERE folder_id = ?1 AND uid = ?2")
                .map_err(|e| sql_err("préparation", e))?;
            let mut ids = Vec::with_capacity(uids.len());
            for uid in uids {
                if let Ok(id) = stmt.query_row(params![folder.get(), *uid as i64], |r| r.get(0)) {
                    ids.push(MessageId(id));
                }
            }
            Ok(ids)
        })
    }

    pub fn delete_messages_by_uid(&self, folder: FolderId, uids: &[u32]) -> Result<usize> {
        if uids.is_empty() {
            return Ok(0);
        }
        self.with_tx(|tx| {
            let threads = threads_of_uids(tx, folder, uids)?;
            let placeholders = std::iter::repeat_n("?", uids.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql =
                format!("DELETE FROM messages WHERE folder_id = ? AND uid IN ({placeholders})");
            let mut args: Vec<i64> = Vec::with_capacity(uids.len() + 1);
            args.push(folder.get());
            args.extend(uids.iter().map(|u| *u as i64));

            let n = tx
                .execute(&sql, params_from_iter(args))
                .map_err(|e| sql_err("suppression de messages", e))?;

            for t in threads {
                refresh_thread(tx, t)?;
            }
            Ok(n)
        })
    }

    /// Un message désigné par son identifiant local.
    pub fn message_by_id(&self, id: MessageId) -> Result<Option<StoredMessage>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached(
                    "SELECT id, account_id, folder_id, thread_id, uid, rfc_message_id, subject,
                            from_name, from_addr, date, received, size, flags, preview, body_blob,
                            recipients
                     FROM messages WHERE id = ?1",
                )
                .map_err(|e| sql_err("préparation", e))?;
            match stmt.query_row(params![id.get()], stored_message_from_row) {
                Ok(m) => Ok(Some(m)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(sql_err("lecture du message", e)),
            }
        })
    }

    /// Messages d'un dossier dont le corps n'a pas encore été téléchargé.
    ///
    /// Sert à l'indexation : ce sont exactement ceux dont l'entrée d'index ne
    /// contient encore que les en-têtes.
    pub fn folder_messages_without_body(
        &self,
        folder: FolderId,
        limit: u32,
    ) -> Result<Vec<StoredMessage>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached(
                    "SELECT id, account_id, folder_id, thread_id, uid, rfc_message_id, subject,
                            from_name, from_addr, date, received, size, flags, preview, body_blob,
                            recipients
                     FROM messages WHERE folder_id = ?1 AND body_blob IS NULL
                     ORDER BY received DESC LIMIT ?2",
                )
                .map_err(|e| sql_err("préparation", e))?;
            let rows = stmt
                .query_map(params![folder.get(), limit as i64], stored_message_from_row)
                .map_err(|e| sql_err("messages sans corps", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("messages sans corps", e))
        })
    }

    /// Empreintes des contenus encore référencés par un message.
    ///
    /// Sert à repérer les contenus orphelins : un message supprimé laisse son corps
    /// derrière lui, et celui-ci occupe la place de contenus encore utiles.
    pub fn referenced_blobs(&self) -> Result<Vec<String>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached(
                    "SELECT DISTINCT body_blob FROM messages WHERE body_blob IS NOT NULL
                     UNION
                     SELECT DISTINCT blob FROM attachments WHERE blob IS NOT NULL",
                )
                .map_err(|e| sql_err("préparation", e))?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| sql_err("contenus référencés", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("contenus référencés", e))
        })
    }

    /// UID connus localement pour un dossier, triés.
    ///
    /// Sert à détecter les suppressions faites ailleurs : on compare cette liste à
    /// celle que le serveur rapporte.
    pub fn folder_uids(&self, folder: FolderId) -> Result<Vec<u32>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached("SELECT uid FROM messages WHERE folder_id = ?1 ORDER BY uid")
                .map_err(|e| sql_err("préparation", e))?;
            let rows = stmt
                .query_map(params![folder.get()], |r| Ok(r.get::<_, i64>(0)? as u32))
                .map_err(|e| sql_err("UID du dossier", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("UID du dossier", e))
        })
    }

    /// Plus grand UID connu localement pour un dossier.
    pub fn max_uid(&self, folder: FolderId) -> Result<u32> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT coalesce(max(uid), 0) FROM messages WHERE folder_id = ?1",
                params![folder.get()],
                |r| Ok(r.get::<_, i64>(0)? as u32),
            )
            .map_err(|e| sql_err("UID maximal", e))
        })
    }

    /// The lowest UID stored for a folder, 0 when it holds none: where a first sync,
    /// which goes from the newest down, carries on.
    pub fn min_uid(&self, folder: FolderId) -> Result<u32> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT coalesce(min(uid), 0) FROM messages WHERE folder_id = ?1",
                params![folder.get()],
                |r| Ok(r.get::<_, i64>(0)? as u32),
            )
            .map_err(|e| sql_err("UID minimal", e))
        })
    }

    /// Identifiant local d'un message désigné par son UID.
    pub fn message_by_uid(&self, folder: FolderId, uid: u32) -> Result<Option<MessageId>> {
        self.with_conn(|c| {
            match c.query_row(
                "SELECT id FROM messages WHERE folder_id = ?1 AND uid = ?2",
                params![folder.get(), uid as i64],
                |r| r.get::<_, i64>(0),
            ) {
                Ok(id) => Ok(Some(MessageId(id))),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(sql_err("recherche par UID", e)),
            }
        })
    }

    /// Applique un lot de changements de drapeaux venus du serveur.
    ///
    /// En une transaction, et avec un seul rafraîchissement par fil touché : un
    /// serveur peut annoncer des milliers de changements après une absence.
    pub fn apply_flag_changes(&self, folder: FolderId, changes: &[(u32, Flags)]) -> Result<usize> {
        if changes.is_empty() {
            return Ok(0);
        }
        self.with_tx(|tx| {
            let mut touches = std::collections::BTreeSet::new();
            let mut appliques = 0;
            let indesirables: bool = tx
                .query_row(
                    "SELECT role = 'junk' FROM folders WHERE id = ?1",
                    params![folder.get()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| sql_err("rôle du dossier", e))?
                .unwrap_or(false);

            for (uid, flags) in changes {
                let existant: Option<(i64, i64)> = {
                    let mut stmt = tx
                        .prepare_cached(
                            "SELECT thread_id, flags FROM messages WHERE folder_id = ?1 AND uid = ?2",
                        )
                        .map_err(|e| sql_err("préparation", e))?;
                    stmt.query_row(params![folder.get(), *uid as i64], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .ok()
                };
                let Some((thread, actuels)) = existant else { continue };

                // The server's say on what it knows (read, answered, starred…), ours
                // kept on what only we work out (spam, attachment, tracker,
                // unsubscribe). Overwriting the whole field dropped those at every flag
                // change, our own mark-read included: a spam message came back into
                // the queue once read.
                let mut fusion = Flags(
                    (flags.0 & Flags::PROTOCOL.0) | (actuels as u32 & !Flags::PROTOCOL.0),
                );
                // Taken out of the junk elsewhere (a phone sets `$NotJunk`): no longer
                // spam here either, outside the junk folder.
                if fusion.contains(Flags::NOT_JUNK) && !indesirables {
                    fusion = fusion.without(Flags::SPAM);
                }

                // Ne rien écrire quand rien ne change : sur une resynchronisation
                // complète, la quasi-totalité des drapeaux sont identiques.
                if actuels == fusion.0 as i64 {
                    continue;
                }

                let mut stmt = tx
                    .prepare_cached(
                        "UPDATE messages SET flags = ?1 WHERE folder_id = ?2 AND uid = ?3",
                    )
                    .map_err(|e| sql_err("préparation", e))?;
                stmt.execute(params![fusion.0 as i64, folder.get(), *uid as i64])
                    .map_err(|e| sql_err("mise à jour des drapeaux", e))?;

                touches.insert(ThreadId(thread));
                appliques += 1;
            }

            for t in touches {
                refresh_thread(tx, t)?;
            }
            Ok(appliques)
        })
    }

    /// Les UID des messages non lus d'un dossier.
    ///
    /// Seulement les non lus : marquer lu ce qui l'est déjà enverrait au serveur des
    /// ordres qui ne changent rien, par lots de cinquante, pour une boîte de mille
    /// messages dont trois sont neufs.
    pub fn folder_unread_uids(&self, folder: FolderId) -> Result<Vec<u32>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached(&format!(
                    "SELECT uid FROM messages
                     WHERE folder_id = ?1 AND (flags & {}) = 0
                     ORDER BY uid",
                    Flags::SEEN.0
                ))
                .map_err(|e| sql_err("préparation", e))?;
            let rows = stmt
                .query_map(params![folder.get()], |r| {
                    r.get::<_, i64>(0).map(|u| u as u32)
                })
                .map_err(|e| sql_err("uids non lus", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("uids non lus", e))
        })
    }

    /// Marque lu tout ce que contient un dossier, localement.
    ///
    /// Localement d'abord : la pastille doit s'éteindre au clic, pas à la
    /// synchronisation suivante. L'ordre part vers le serveur par le journal, comme
    /// toute autre action.
    pub fn mark_folder_read(&self, folder: FolderId) -> Result<usize> {
        self.with_tx(|tx| {
            let threads: Vec<ThreadId> = {
                let mut stmt = tx
                    .prepare_cached(&format!(
                        "SELECT DISTINCT thread_id FROM messages
                         WHERE folder_id = ?1 AND (flags & {}) = 0",
                        Flags::SEEN.0
                    ))
                    .map_err(|e| sql_err("préparation", e))?;
                let rows = stmt
                    .query_map(params![folder.get()], |r| r.get::<_, i64>(0).map(ThreadId))
                    .map_err(|e| sql_err("fils du dossier", e))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| sql_err("fils du dossier", e))?
            };

            // The other copies of those messages too, in the same mailbox (Gmail's All
            // Mail and labels, where reading one copy reads them all): left unread,
            // their threads stayed unread until the next sync.
            tx.execute(
                &format!(
                    "UPDATE messages SET flags = flags | {seen}
                     WHERE (flags & {seen}) = 0
                       AND rfc_message_id IN (
                           SELECT rfc_message_id FROM messages
                           WHERE folder_id = ?1 AND (flags & {seen}) = 0
                             AND rfc_message_id IS NOT NULL)
                       AND account_id = (SELECT account_id FROM folders WHERE id = ?1)
                       AND folder_id != ?1",
                    seen = Flags::SEEN.0
                ),
                params![folder.get()],
            )
            .map_err(|e| sql_err("marquage des copies", e))?;

            let n = tx
                .execute(
                    &format!(
                        "UPDATE messages SET flags = flags | {}
                         WHERE folder_id = ?1 AND (flags & {}) = 0",
                        Flags::SEEN.0,
                        Flags::SEEN.0
                    ),
                    params![folder.get()],
                )
                .map_err(|e| sql_err("marquage du dossier", e))?;

            // Les agrégats du fil portent le compte de non-lus : sans ce recalcul, la
            // liste continuerait d'afficher en gras des messages qui ne le sont plus.
            for t in threads {
                refresh_thread(tx, t)?;
            }
            Ok(n)
        })
    }

    /// Supprime tous les messages d'un dossier.
    ///
    /// Utilisé quand le serveur a changé son `UIDVALIDITY` : les UID connus ne
    /// désignent plus rien, et les conserver produirait des doublons.
    pub fn clear_folder(&self, folder: FolderId) -> Result<usize> {
        self.with_tx(|tx| {
            let threads: Vec<ThreadId> = {
                let mut stmt = tx
                    .prepare_cached("SELECT DISTINCT thread_id FROM messages WHERE folder_id = ?1")
                    .map_err(|e| sql_err("préparation", e))?;
                let rows = stmt
                    .query_map(params![folder.get()], |r| r.get::<_, i64>(0).map(ThreadId))
                    .map_err(|e| sql_err("fils du dossier", e))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| sql_err("fils du dossier", e))?
            };

            let n = tx
                .execute(
                    "DELETE FROM messages WHERE folder_id = ?1",
                    params![folder.get()],
                )
                .map_err(|e| sql_err("vidage du dossier", e))?;

            for t in threads {
                refresh_thread(tx, t)?;
            }
            Ok(n)
        })
    }

    /// Les correspondants qui ressemblent à ce qu'on est en train de taper.
    ///
    /// Classés par ce qu'on a reçu d'eux, puis par récence. Un correspondant qui écrit
    /// souvent est celui qu'on vise le plus probablement ; à volume égal, le plus
    /// récent l'emporte, parce qu'une adresse abandonnée il y a trois ans ne doit pas
    /// rester en tête d'une liste pour toujours.
    ///
    /// La recherche porte sur l'adresse **et** sur le nom : on cherche « marie » aussi
    /// souvent qu'on cherche « @client.fr », et n'accepter que l'un des deux reviendrait
    /// à demander de se souvenir de ce dont on ne se souvient justement pas.
    pub fn contacts_like(&self, fragment: &str, limit: u32) -> Result<Vec<Contact>> {
        let fragment = fragment.trim().to_lowercase();
        if fragment.is_empty() {
            return Ok(Vec::new());
        }

        self.with_conn(|c| {
            let motif = format!("%{}%", fragment.replace('%', "\\%").replace('_', "\\_"));
            let mut stmt = c
                .prepare_cached(
                    "SELECT addr_key, display, received_count
                     FROM contacts_seen
                     WHERE addr_key LIKE ?1 ESCAPE '\\'
                        OR lower(display) LIKE ?1 ESCAPE '\\'
                     ORDER BY received_count DESC, last_seen_at DESC
                     LIMIT ?2",
                )
                .map_err(|e| sql_err("préparation", e))?;

            let rows = stmt
                .query_map(params![motif, limit as i64], |r| {
                    Ok(Contact {
                        address: r.get(0)?,
                        display: r.get(1)?,
                        seen: r.get::<_, i64>(2)? as u32,
                    })
                })
                .map_err(|e| sql_err("correspondants", e))?;

            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("correspondants", e))
        })
    }

    /// Nombre de messages, tous comptes confondus. Utile aux mesures.
    pub fn message_count(&self) -> Result<u64> {
        self.with_conn(|c| {
            c.query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))
                .map(|n| n as u64)
                .map_err(|e| sql_err("comptage", e))
        })
    }
}

/// Lit une ligne de message. Les colonnes sont attendues dans l'ordre du `SELECT`
/// partagé par les deux lectures.
fn stored_message_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<StoredMessage> {
    Ok(StoredMessage {
        id: MessageId(r.get(0)?),
        account: AccountId(r.get(1)?),
        folder: FolderId(r.get(2)?),
        thread: ThreadId(r.get(3)?),
        uid: r.get::<_, i64>(4)? as u32,
        rfc_message_id: r.get(5)?,
        subject: r.get(6)?,
        from_name: r.get(7)?,
        from_addr: r.get(8)?,
        date: Timestamp::from_millis(r.get(9)?),
        received: Timestamp::from_millis(r.get(10)?),
        size: r.get::<_, i64>(11)? as u64,
        flags: Flags(r.get::<_, i64>(12)? as u32),
        preview: r.get(13)?,
        body_blob: r.get(14)?,
        recipients_json: r.get(15)?,
    })
}

fn threads_of_uids(tx: &Transaction<'_>, folder: FolderId, uids: &[u32]) -> Result<Vec<ThreadId>> {
    let placeholders = std::iter::repeat_n("?", uids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT DISTINCT thread_id FROM messages WHERE folder_id = ? AND uid IN ({placeholders})"
    );
    let mut args: Vec<i64> = Vec::with_capacity(uids.len() + 1);
    args.push(folder.get());
    args.extend(uids.iter().map(|u| *u as i64));

    let mut stmt = tx.prepare(&sql).map_err(|e| sql_err("préparation", e))?;
    let rows = stmt
        .query_map(params_from_iter(args), |r| r.get::<_, i64>(0).map(ThreadId))
        .map_err(|e| sql_err("fils concernés", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| sql_err("fils concernés", e))
}

fn insert_message_tx(tx: &Transaction<'_>, m: &NewMessage) -> Result<Inserted> {
    let r = insert_message_tx_deferred(tx, m)?;
    refresh_thread(tx, r.thread)?;
    Ok(r)
}

/// Insere sans recalculer les agregats du fil : l'appelant s'en charge, une fois par
/// fil touche.
///
/// Toutes les instructions du chemin chaud passent par `prepare_cached`. Ce n'est pas
/// un detail : sans cache, une synchronisation d'un million de messages recompile
/// huit requetes par message, et ce cout de compilation depasse largement celui de
/// l'ecriture elle-meme.
fn insert_message_tx_deferred(tx: &Transaction<'_>, m: &NewMessage) -> Result<Inserted> {
    // Deja connu ? On se contente de rafraichir les drapeaux.
    let known: Option<(i64, i64)> = {
        let mut stmt = tx
            .prepare_cached("SELECT id, thread_id FROM messages WHERE folder_id = ?1 AND uid = ?2")
            .map_err(|e| sql_err("preparation", e))?;
        stmt.query_row(params![m.folder.get(), m.uid as i64], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .ok()
    };

    if let Some((id, thread)) = known {
        // The server's flags, and ours kept: read again, a message lost what only its
        // body had told (an attachment, a tracker), and a junk mark.
        let mut stmt = tx
            .prepare_cached("UPDATE messages SET flags = (?1 & ?3) | (flags & ~?3) WHERE id = ?2")
            .map_err(|e| sql_err("preparation", e))?;
        stmt.execute(params![m.flags.0 as i64, id, Flags::PROTOCOL.0 as i64])
            .map_err(|e| sql_err("rafraichissement des drapeaux", e))?;
        return Ok(Inserted {
            message: MessageId(id),
            thread: ThreadId(thread),
            was_known: true,
            thread_created: false,
            came_back: false,
        });
    }

    // A copy of a message already here (Gmail's All Mail), or one coming back after a
    // move: not new mail, whatever row it gets.
    let came_back = match &m.rfc_message_id {
        Some(id) => {
            let mut stmt = tx
                .prepare_cached(
                    "SELECT EXISTS (SELECT 1 FROM messages WHERE rfc_message_id = ?1)
                         OR EXISTS (SELECT 1 FROM thread_ghosts WHERE rfc_message_id = ?1)",
                )
                .map_err(|e| sql_err("preparation", e))?;
            stmt.query_row(params![id], |r| r.get::<_, bool>(0))
                .map_err(|e| sql_err("copies du message", e))?
        }
        None => false,
    };

    let (thread, thread_created) = resolve_thread(tx, m)?;

    {
        let mut stmt = tx
            .prepare_cached(
                "INSERT INTO messages
                   (account_id, folder_id, thread_id, uid, rfc_message_id, in_reply_to, subject,
                    from_name, from_addr, recipients, date, received, size, flags, preview)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            )
            .map_err(|e| sql_err("preparation", e))?;
        stmt.execute(params![
            m.account.get(),
            m.folder.get(),
            thread.get(),
            m.uid as i64,
            m.rfc_message_id,
            m.in_reply_to,
            m.subject,
            m.from_name,
            m.from_addr,
            m.recipients_json,
            m.date.millis(),
            m.received.millis(),
            m.size as i64,
            m.flags.0 as i64,
            m.preview,
        ])
        .map_err(|e| sql_err("insertion du message", e))?;
    }

    let message = MessageId(tx.last_insert_rowid());

    if !m.references.is_empty() {
        let mut stmt = tx
            .prepare_cached(
                "INSERT OR IGNORE INTO message_refs (message_id, position, ref_id)
                 VALUES (?1, ?2, ?3)",
            )
            .map_err(|e| sql_err("preparation", e))?;
        for (i, r) in m.references.iter().enumerate() {
            stmt.execute(params![message.get(), i as i64, r])
                .map_err(|e| sql_err("insertion des references", e))?;
        }
    }

    {
        let mut stmt = tx
            .prepare_cached(
                "INSERT OR IGNORE INTO thread_accounts (thread_id, account_id) VALUES (?1, ?2)",
            )
            .map_err(|e| sql_err("preparation", e))?;
        stmt.execute(params![thread.get(), m.account.get()])
            .map_err(|e| sql_err("rattachement du compte au fil", e))?;
    }

    // On retient qui écrit.
    //
    // La table existait depuis la première migration et personne ne l'avait jamais
    // remplie : le champ « À » de l'éditeur n'a donc jamais rien proposé, et taper une
    // adresse de mémoire est le plus sûr moyen de l'écrire de travers — un client de
    // courrier qui ne connaît pas ses correspondants fait retaper cent fois par mois
    // ce qu'il a reçu cent fois.
    //
    // Ici et non ailleurs : c'est le seul endroit par lequel passe tout message
    // entrant, une fois, et l'écriture tient dans la transaction qui insère déjà.
    if !m.from_addr.trim().is_empty() {
        let mut stmt = tx
            .prepare_cached(
                "INSERT INTO contacts_seen (addr_key, display, received_count, last_seen_at)
                 VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(addr_key) DO UPDATE SET
                     received_count = received_count + 1,
                     last_seen_at   = max(last_seen_at, excluded.last_seen_at),
                     -- Un nom vide ne doit pas effacer celui qu'on avait : la moitié
                     -- des messages n'en portent pas, et le dernier arrivé n'est pas
                     -- le mieux renseigné.
                     display = CASE WHEN excluded.display <> '' THEN excluded.display
                                    ELSE display END",
            )
            .map_err(|e| sql_err("preparation", e))?;
        stmt.execute(params![
            m.from_addr.trim().to_lowercase(),
            m.from_name.trim(),
            m.received.millis(),
        ])
        .map_err(|e| sql_err("memoire des correspondants", e))?;
    }

    Ok(Inserted {
        message,
        thread,
        was_known: false,
        thread_created,
        came_back,
    })
}

/// Trouve le fil auquel rattacher un message, ou en cree un.
fn resolve_thread(tx: &Transaction<'_>, m: &NewMessage) -> Result<(ThreadId, bool)> {
    // 0. Another copy of this very message. Gmail shows each message in two folders
    //    at least (the inbox and All Mail), and a copy in each mailbox it was sent to
    //    is the same message too: one conversation, not two rows saying the same.
    //    The same sender too: two different messages given one identifier (a mailer
    //    that reuses them) were made one, and one of them vanished from the reader.
    if let Some(mine) = &m.rfc_message_id {
        let mut stmt = tx
            .prepare_cached(
                "SELECT thread_id FROM messages
                 WHERE rfc_message_id = ?1 AND lower(from_addr) = lower(?2) LIMIT 1",
            )
            .map_err(|e| sql_err("preparation", e))?;
        if let Ok(t) = stmt.query_row(params![mine, m.from_addr], |r| r.get::<_, i64>(0)) {
            return Ok((ThreadId(t), false));
        }
    }

    // 1. Un message que nous citons est-il deja connu ?
    if m.in_reply_to.is_some() || !m.references.is_empty() {
        let mut stmt = tx
            .prepare_cached("SELECT thread_id FROM messages WHERE rfc_message_id = ?1 LIMIT 1")
            .map_err(|e| sql_err("preparation", e))?;
        for id in m.in_reply_to.iter().chain(m.references.iter()) {
            if let Ok(t) = stmt.query_row(params![id], |r| r.get::<_, i64>(0)) {
                return Ok((ThreadId(t), false));
            }
        }

        // 1b. A known message citing what we cite: two answers to a message not here
        //     (older than what was synced, deleted, in a folder not read) made two
        //     threads, and nothing joined them. A first sync, which goes from the
        //     newest down, made that the common case.
        let mut par_reference = tx
            .prepare_cached(
                "SELECT m.thread_id FROM message_refs r
                 JOIN messages m ON m.id = r.message_id
                 WHERE r.ref_id = ?1 LIMIT 1",
            )
            .map_err(|e| sql_err("preparation", e))?;
        let mut par_reponse = tx
            .prepare_cached("SELECT thread_id FROM messages WHERE in_reply_to = ?1 LIMIT 1")
            .map_err(|e| sql_err("preparation", e))?;
        for id in m.in_reply_to.iter().chain(m.references.iter()) {
            if let Ok(t) = par_reference.query_row(params![id], |r| r.get::<_, i64>(0)) {
                return Ok((ThreadId(t), false));
            }
            if let Ok(t) = par_reponse.query_row(params![id], |r| r.get::<_, i64>(0)) {
                return Ok((ThreadId(t), false));
            }
        }
    }

    // 2. Un message deja connu nous cite-t-il ? Cas de la reponse arrivee avant
    //    l'original, banal sur une boite synchronisee en desordre.
    if let Some(mine) = &m.rfc_message_id {
        let par_reference = {
            let mut stmt = tx
                .prepare_cached(
                    "SELECT m.thread_id FROM message_refs r
                     JOIN messages m ON m.id = r.message_id
                     WHERE r.ref_id = ?1 LIMIT 1",
                )
                .map_err(|e| sql_err("preparation", e))?;
            stmt.query_row(params![mine], |r| r.get::<_, i64>(0)).ok()
        };
        let trouve = match par_reference {
            Some(t) => Some(t),
            None => {
                let mut stmt = tx
                    .prepare_cached("SELECT thread_id FROM messages WHERE in_reply_to = ?1 LIMIT 1")
                    .map_err(|e| sql_err("preparation", e))?;
                stmt.query_row(params![mine], |r| r.get::<_, i64>(0)).ok()
            }
        };
        if let Some(t) = trouve {
            return Ok((ThreadId(t), false));
        }
    }

    // 3. Nouveau fil.
    //
    // Toujours « à traiter », quel que soit le dossier d'arrivée. Une version
    // précédente créait « terminé » ce qui arrivait dans une corbeille, pour tenir le
    // courrier jeté hors de la file — l'intention était juste, l'endroit ne l'était
    // pas. L'état dit ce que l'utilisateur a décidé du fil ; le dossier dit où le
    // message se trouve. Confondre les deux mettait la corbeille dans « Terminé », et
    // un fil sorti de la corbeille en gardait un état que personne n'avait choisi.
    //
    // C'est la requête de liste qui écarte les dossiers mis de côté, et elle seule.
    //
    // Sauf pour un message qui revient : déplacé, son dossier renommé ou reconstruit,
    // il a quitté la copie locale avec son fil, et son fil retrouve l'état qu'il
    // avait (`thread_ghosts`, rempli par un déclencheur). Sans cela, archiver ou
    // ranger un fil ailleurs que chez Gmail le recréait « à traiter ».
    let subject_norm = normalize_subject(&m.subject);
    type Fantome = (i64, Option<i64>, Option<i64>, Option<i64>);
    let fantome: Option<Fantome> = match &m.rfc_message_id {
        Some(id) => {
            let mut stmt = tx
                .prepare_cached(
                    "SELECT state, snooze_until, snooze_restore, put_aside_at
                     FROM thread_ghosts WHERE rfc_message_id = ?1",
                )
                .map_err(|e| sql_err("preparation", e))?;
            stmt.query_row(params![id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .ok()
        }
        None => None,
    };
    let (etat, report, retour, mis_de_cote) =
        fantome.unwrap_or((WorkflowState::Todo.as_i64(), None, None, None));

    let mut stmt = tx
        .prepare_cached(
            "INSERT INTO threads
               (subject_norm, state, last_activity_at, snooze_until, snooze_restore,
                put_aside_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .map_err(|e| sql_err("preparation", e))?;
    stmt.execute(params![
        subject_norm,
        etat,
        m.received.millis(),
        report,
        retour,
        mis_de_cote
    ])
    .map_err(|e| sql_err("creation du fil", e))?;
    let fil = ThreadId(tx.last_insert_rowid());
    if let (Some(_), Some(id)) = (fantome, &m.rfc_message_id) {
        tx.execute(
            "DELETE FROM thread_ghosts WHERE rfc_message_id = ?1",
            params![id],
        )
        .map_err(|e| sql_err("fantôme du fil", e))?;
    }
    Ok((fil, true))
}

/// One copy of each message, in the order given: one whose body is here, else the first.
fn one_copy_each(messages: Vec<StoredMessage>) -> Vec<StoredMessage> {
    let mut sortie: Vec<StoredMessage> = Vec::with_capacity(messages.len());
    let mut rang: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for m in messages {
        let Some(cle) = m.rfc_message_id.clone() else {
            sortie.push(m);
            continue;
        };
        match rang.get(&cle) {
            Some(&i) => {
                // The sent message rather than its draft, which shares its identifier;
                // then the copy whose body is here.
                let brouillon = |x: &StoredMessage| x.flags.contains(Flags::DRAFT);
                let mieux = (brouillon(&sortie[i]) && !brouillon(&m))
                    || (brouillon(&sortie[i]) == brouillon(&m)
                        && sortie[i].body_blob.is_none()
                        && m.body_blob.is_some());
                if mieux {
                    sortie[i] = m;
                }
            }
            None => {
                rang.insert(cle, sortie.len());
                sortie.push(m);
            }
        }
    }
    sortie
}

/// Joins the threads that hold copies of one message, oldest thread first. Returns how
/// many threads were absorbed.
///
/// Until 4.5.1 a copy of a message already stored opened a thread of its own, so a
/// Gmail mailbox listed nearly everything twice (its inbox and All Mail). Run when the
/// base opens: with nothing to join it costs one grouped read of an index.
pub(crate) fn join_copies_of_one_message(conn: &rusqlite::Connection) -> Result<usize> {
    let partages: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT rfc_message_id FROM messages
                 WHERE rfc_message_id IS NOT NULL
                 GROUP BY rfc_message_id
                 HAVING count(DISTINCT thread_id) > 1",
            )
            .map_err(|e| sql_err("preparation", e))?;
        let lignes = stmt
            .query_map([], |r| r.get(0))
            .map_err(|e| sql_err("copies d'un message", e))?;
        lignes
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| sql_err("copies d'un message", e))?
    };
    if partages.is_empty() {
        return Ok(0);
    }

    let tx = conn
        .unchecked_transaction()
        .map_err(|e| sql_err("transaction", e))?;
    let mut absorbes = 0;
    let mut touches = std::collections::BTreeSet::new();
    for id in &partages {
        // Read again for each message: an earlier one may already have joined these.
        let fils: Vec<i64> = {
            let mut stmt = tx
                .prepare_cached(
                    "SELECT DISTINCT thread_id FROM messages WHERE rfc_message_id = ?1
                     ORDER BY thread_id",
                )
                .map_err(|e| sql_err("preparation", e))?;
            let lignes = stmt
                .query_map([id], |r| r.get(0))
                .map_err(|e| sql_err("copies d'un message", e))?;
            lignes
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| sql_err("copies d'un message", e))?
        };
        let Some((&cible, autres)) = fils.split_first() else {
            continue;
        };
        for &autre in autres {
            tx.execute(
                "UPDATE messages SET thread_id = ?1 WHERE thread_id = ?2",
                params![cible, autre],
            )
            .map_err(|e| sql_err("rattachement des copies", e))?;
            tx.execute(
                "INSERT OR IGNORE INTO thread_accounts (thread_id, account_id)
                 SELECT ?1, account_id FROM thread_accounts WHERE thread_id = ?2",
                params![cible, autre],
            )
            .map_err(|e| sql_err("rattachement des copies", e))?;
            tx.execute(
                "UPDATE tasks SET thread_id = ?1 WHERE thread_id = ?2",
                params![cible, autre],
            )
            .map_err(|e| sql_err("rattachement des copies", e))?;
            tx.execute("DELETE FROM threads WHERE id = ?1", params![autre])
                .map_err(|e| sql_err("rattachement des copies", e))?;
            absorbes += 1;
        }
        touches.insert(cible);
    }
    for fil in touches {
        refresh_thread(&tx, ThreadId(fil))?;
    }
    tx.commit().map_err(|e| sql_err("transaction", e))?;
    Ok(absorbes)
}

/// Recalcule les colonnes agregees d'un fil.
///
/// Ces colonnes sont denormalisees pour que la liste principale n'ait besoin d'aucune
/// jointure : c'est ce qui rend une page de liste servable en une seule lecture
/// d'index. Le prix est ce recalcul, borne au nombre de messages du fil.
///
/// Un seul balayage suffit pour tout calculer. L'union des drapeaux se fait par OU
/// logique et non par somme : deux messages portant le meme drapeau ne doivent pas en
/// produire un troisieme.
pub(crate) fn refresh_thread(tx: &Transaction<'_>, thread: ThreadId) -> Result<()> {
    let mut count = 0i64;
    let mut unread = 0i64;
    let mut last_activity = 0i64;
    let mut union = 0i64;
    // Le compte accompagne les quatre autres champs du dernier message : la pastille
    // de couleur de la liste en vient, et c'est la seule chose qui dise de quelle
    // boîte un message arrive quand on les regarde toutes ensemble.
    let mut last: Option<(String, String, String, String, i64)> = None;

    {
        let mut stmt = tx
            .prepare_cached(
                "SELECT flags, received, from_name, from_addr, subject, preview, account_id,
                        rfc_message_id
                 FROM messages WHERE thread_id = ?1 ORDER BY received DESC, id DESC",
            )
            .map_err(|e| sql_err("preparation", e))?;
        let mut rows = stmt
            .query(params![thread.get()])
            .map_err(|e| sql_err("agregats du fil", e))?;

        // Messages are counted once, however many folders hold a copy (Gmail's inbox
        // and All Mail): a single message read as "2 messages, 2 unread" otherwise.
        let mut vus = std::collections::HashSet::new();
        let mut non_lus = std::collections::HashSet::new();
        let mut sans_identifiant = 0i64;
        while let Some(r) = rows.next().map_err(|e| sql_err("agregats du fil", e))? {
            let flags: i64 = r.get(0).map_err(|e| sql_err("agregats du fil", e))?;
            let received: i64 = r.get(1).map_err(|e| sql_err("agregats du fil", e))?;
            let identifiant: Option<String> =
                r.get(7).map_err(|e| sql_err("agregats du fil", e))?;
            let cle = identifiant.unwrap_or_else(|| {
                sans_identifiant += 1;
                format!("\u{0}{sans_identifiant}")
            });
            if vus.insert(cle.clone()) {
                count += 1;
            }
            union |= flags;
            // Marked deleted by another client and not purged yet: on its way out, not
            // mail waiting to be read.
            let efface = flags & (Flags::DELETED.0 as i64) != 0;
            if flags & (Flags::SEEN.0 as i64) == 0 && !efface && non_lus.insert(cle) {
                unread += 1;
            }
            if received > last_activity {
                last_activity = received;
            }
            if last.is_none() {
                // Premiere ligne du tri decroissant : c'est le dernier message.
                last = Some((
                    r.get(2).map_err(|e| sql_err("agregats du fil", e))?,
                    r.get(3).map_err(|e| sql_err("agregats du fil", e))?,
                    r.get(4).map_err(|e| sql_err("agregats du fil", e))?,
                    r.get(5).map_err(|e| sql_err("agregats du fil", e))?,
                    r.get(6).map_err(|e| sql_err("agregats du fil", e))?,
                ));
            }
        }
    }

    // Un fil vide de ses messages n'a plus lieu d'etre.
    let Some(last) = last else {
        let mut stmt = tx
            .prepare_cached("DELETE FROM threads WHERE id = ?1")
            .map_err(|e| sql_err("preparation", e))?;
        stmt.execute(params![thread.get()])
            .map_err(|e| sql_err("suppression du fil vide", e))?;
        return Ok(());
    };

    let mut stmt = tx
        .prepare_cached(
            "UPDATE threads SET message_count = ?1, unread_count = ?2, last_activity_at = ?3,
                                flags_union = ?4, last_from_name = ?5, last_from_addr = ?6,
                                last_subject = ?7, last_preview = ?8, last_account_id = ?9,
                                put_aside_at = CASE WHEN put_aside_at IS NOT NULL
                                    AND ?3 > put_aside_at THEN NULL ELSE put_aside_at END
             WHERE id = ?10",
        )
        .map_err(|e| sql_err("preparation", e))?;
    stmt.execute(params![
        count,
        unread,
        last_activity,
        union,
        last.0,
        last.1,
        last.2,
        last.3,
        last.4,
        thread.get()
    ])
    .map_err(|e| sql_err("mise a jour du fil", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FolderRole, NewAccount};

    struct Fixture {
        store: Store,
        account: AccountId,
        folder: FolderId,
        next_uid: std::cell::Cell<u32>,
    }

    fn fixture() -> Fixture {
        let store = Store::in_memory().unwrap();
        let account = store
            .create_account(
                &NewAccount::new("moi@example.com", "i", "s"),
                Timestamp::from_millis(0),
            )
            .unwrap();
        let folder = store
            .upsert_folder(account, "INBOX", FolderRole::Inbox)
            .unwrap();
        Fixture {
            store,
            account,
            folder,
            next_uid: std::cell::Cell::new(1),
        }
    }

    impl Fixture {
        fn msg(&self, id: &str, received: i64) -> NewMessage {
            let uid = self.next_uid.get();
            self.next_uid.set(uid + 1);
            NewMessage {
                account: self.account,
                folder: self.folder,
                uid,
                rfc_message_id: Some(id.to_string()),
                in_reply_to: None,
                references: vec![],
                subject: "Devis refonte".into(),
                from_name: "Marie".into(),
                from_addr: "marie@example.com".into(),
                recipients_json: "[]".into(),
                date: Timestamp::from_millis(received),
                received: Timestamp::from_millis(received),
                size: 100,
                flags: Flags::NONE,
                preview: format!("aperçu {id}"),
            }
        }
    }

    #[test]
    fn a_message_moved_elsewhere_keeps_its_thread_state() {
        // Archived on a server other than Gmail: the inbox copy goes, the archive copy
        // comes in a later pass with nothing left to join. Its thread came back To do.
        let f = fixture();
        let archives = f
            .store
            .upsert_folder(f.account, "Archive", FolderRole::Archive)
            .unwrap();
        let avant = f.store.insert_message(&f.msg("un@x", 1000)).unwrap();
        f.store
            .set_thread_state(avant.thread, WorkflowState::Done)
            .unwrap();
        f.store.delete_messages_by_uid(f.folder, &[1]).unwrap();
        assert!(f.store.thread_row(avant.thread).unwrap().is_none());

        let mut revenu = f.msg("un@x", 1000);
        revenu.folder = archives;
        let apres = f.store.insert_message(&revenu).unwrap();
        assert!(apres.thread_created);
        let ligne = f.store.thread_row(apres.thread).unwrap().unwrap();
        assert_eq!(ligne.state, WorkflowState::Done);

        // Taken once: a later new message with no thread is To do as ever.
        f.store
            .delete_messages_by_uid(archives, &[revenu.uid])
            .unwrap();
        f.store
            .purge_thread_ghosts(Timestamp::from_millis(i64::MAX))
            .unwrap();
        let neuf = f.store.insert_message(&f.msg("un@x", 2000)).unwrap();
        let ligne = f.store.thread_row(neuf.thread).unwrap().unwrap();
        assert_eq!(ligne.state, WorkflowState::Todo);
    }

    #[test]
    fn a_copy_in_another_folder_joins_the_same_thread() {
        // Gmail: the inbox and All Mail hold the same message.
        let f = fixture();
        let tout = f
            .store
            .upsert_folder(f.account, "[Gmail]/All Mail", FolderRole::Archive)
            .unwrap();
        let premier = f.store.insert_message(&f.msg("un@x", 1000)).unwrap();
        let mut copie = f.msg("un@x", 1000);
        copie.folder = tout;
        let second = f.store.insert_message(&copie).unwrap();

        assert_eq!(second.thread, premier.thread);
        assert!(!second.thread_created);

        // Read once, counted once; both copies kept for what moves them.
        let fil = premier.thread;
        assert_eq!(f.store.thread_messages(fil).unwrap().len(), 2);
        assert_eq!(f.store.conversation(fil).unwrap().len(), 1);
        let ligne = f.store.thread_row(fil).unwrap().unwrap();
        assert_eq!(ligne.message_count, 1);
        assert_eq!(ligne.unread_count, 1);
    }

    #[test]
    fn copies_stored_apart_are_joined() {
        let f = fixture();
        let tout = f
            .store
            .upsert_folder(f.account, "[Gmail]/All Mail", FolderRole::Archive)
            .unwrap();
        let premier = f.store.insert_message(&f.msg("un@x", 1000)).unwrap();
        let mut copie = f.msg("autre@x", 2000);
        copie.folder = tout;
        let second = f.store.insert_message(&copie).unwrap();
        // As an older version stored it: the copy in a thread of its own.
        f.store
            .with_conn(|c| {
                c.execute(
                    "UPDATE messages SET rfc_message_id = 'un@x' WHERE id = ?1",
                    [second.message.get()],
                )
                .map_err(|e| sql_err("test", e))
            })
            .unwrap();
        assert_ne!(premier.thread, second.thread);

        assert_eq!(f.store.join_copies_of_one_message().unwrap(), 1);
        let messages = f.store.thread_messages(premier.thread).unwrap();
        assert_eq!(messages.len(), 2);
        assert!(f.store.thread_row(second.thread).unwrap().is_none());
        assert_eq!(
            f.store
                .thread_row(premier.thread)
                .unwrap()
                .unwrap()
                .message_count,
            1,
            "two copies of one message"
        );
        assert_eq!(
            f.store.join_copies_of_one_message().unwrap(),
            0,
            "once only"
        );
    }

    #[test]
    fn les_correspondants_s_apprennent_du_courrier_recu() {
        // La table existait depuis la première migration et personne ne l'avait jamais
        // remplie : le champ « À » de l'éditeur ne proposait donc rien, et il fallait
        // retaper de mémoire des adresses reçues cent fois.
        let f = fixture();
        f.store.insert_message(&f.msg("un@x", 1000)).unwrap();
        f.store.insert_message(&f.msg("deux@x", 2000)).unwrap();

        let trouves = f.store.contacts_like("mar", 10).unwrap();
        assert_eq!(trouves.len(), 1, "une seule adresse, vue deux fois");
        assert_eq!(trouves[0].address, "marie@example.com");
        assert_eq!(trouves[0].display, "Marie");
        assert_eq!(trouves[0].seen, 2);
        assert_eq!(trouves[0].to_header(), "Marie <marie@example.com>");
    }

    #[test]
    fn on_cherche_un_correspondant_par_son_nom_autant_que_par_son_adresse() {
        // On se souvient de « Marie » bien plus souvent que de « m.durand@… », et
        // n'accepter que l'adresse reviendrait à demander ce dont on ne se souvient
        // justement pas.
        let f = fixture();
        let mut m = f.msg("x@x", 1000);
        m.from_name = "Marie Durand".into();
        m.from_addr = "m.durand@client.fr".into();
        f.store.insert_message(&m).unwrap();

        assert_eq!(f.store.contacts_like("durand", 10).unwrap().len(), 1);
        assert_eq!(f.store.contacts_like("client.fr", 10).unwrap().len(), 1);
        assert_eq!(f.store.contacts_like("inconnu", 10).unwrap().len(), 0);
        assert!(
            f.store.contacts_like("  ", 10).unwrap().is_empty(),
            "un champ vide ne propose pas tout le carnet"
        );
    }

    #[test]
    fn le_plus_frequent_vient_en_premier() {
        let f = fixture();
        for i in 0..3 {
            let mut m = f.msg(&format!("a{i}@x"), 1000 + i);
            m.from_addr = "souvent@x.fr".into();
            m.from_name = "Souvent".into();
            f.store.insert_message(&m).unwrap();
        }
        let mut rare = f.msg("b@x", 9000);
        rare.from_addr = "rare@x.fr".into();
        rare.from_name = "Rare".into();
        f.store.insert_message(&rare).unwrap();

        let trouves = f.store.contacts_like("x.fr", 10).unwrap();
        assert_eq!(trouves[0].address, "souvent@x.fr");
        assert_eq!(
            trouves[1].address, "rare@x.fr",
            "plus récent, mais vu une fois"
        );
    }

    #[test]
    fn un_nom_vide_n_efface_pas_celui_qu_on_avait() {
        // La moitié des messages n'en portent pas, et le dernier arrivé n'est pas le
        // mieux renseigné.
        let f = fixture();
        f.store.insert_message(&f.msg("un@x", 1000)).unwrap();

        let mut anonyme = f.msg("deux@x", 2000);
        anonyme.from_name = String::new();
        f.store.insert_message(&anonyme).unwrap();

        assert_eq!(
            f.store.contacts_like("marie", 10).unwrap()[0].display,
            "Marie"
        );
    }

    #[test]
    fn un_message_arrive_a_la_corbeille_garde_son_etat_et_quitte_la_file() {
        // Une version précédente le créait « terminé » pour le tenir hors de la file.
        // L'intention était juste, l'endroit ne l'était pas : « Terminé » veut dire
        // « je m'en suis occupé », et huit cent cinquante messages jetés y noyaient
        // les quelques dizaines réellement traités. L'état dit ce que l'utilisateur a
        // décidé ; le dossier dit où le message est. C'est la requête qui écarte.
        let f = fixture();
        let corbeille = f
            .store
            .upsert_folder(f.account, "Trash", FolderRole::Trash)
            .unwrap();

        let mut m = f.msg("jete@x", 1000);
        m.folder = corbeille;
        let r = f.store.insert_message(&m).unwrap();

        assert_eq!(
            f.store.thread_row(r.thread).unwrap().unwrap().state,
            WorkflowState::Todo,
            "l'état n'est pas décidé par le dossier"
        );

        for etat in [
            WorkflowState::Todo,
            WorkflowState::Waiting,
            WorkflowState::Done,
        ] {
            assert!(
                f.store
                    .list_threads(&crate::model::ListQuery::new(etat, 10))
                    .unwrap()
                    .is_empty(),
                "et il n'apparaît dans aucune des trois files"
            );
        }

        let dans_la_corbeille = f
            .store
            .list_threads(
                &crate::model::ListQuery::new(WorkflowState::Todo, 10).in_role(FolderRole::Trash),
            )
            .unwrap();
        assert_eq!(
            dans_la_corbeille.len(),
            1,
            "mais il est là où on est allé le chercher"
        );
    }

    #[test]
    fn un_message_de_la_boite_de_reception_reste_a_traiter() {
        let f = fixture();
        let r = f.store.insert_message(&f.msg("vivant@x", 1000)).unwrap();

        assert_eq!(
            f.store.thread_row(r.thread).unwrap().unwrap().state,
            WorkflowState::Todo
        );
    }

    #[test]
    fn une_reponse_dans_la_boite_ramene_le_fil() {
        // Le fil est jugé sur l'ensemble de ses messages, jamais sur le premier
        // arrivé : un échange dont un message est jeté et la réponse toujours là est
        // du travail vivant.
        let f = fixture();
        let corbeille = f
            .store
            .upsert_folder(f.account, "Trash", FolderRole::Trash)
            .unwrap();

        let mut premier = f.msg("a@x", 1000);
        premier.folder = corbeille;
        let fil = f.store.insert_message(&premier).unwrap().thread;

        let mut reponse = f.msg("b@x", 2000);
        reponse.in_reply_to = Some("a@x".into());
        f.store.insert_message(&reponse).unwrap();

        // Le fil existe toujours et porte les deux messages : c'est le classement qui
        // appartient désormais à l'utilisateur, pas au dossier d'origine.
        assert_eq!(f.store.thread_messages(fil).unwrap().len(), 2);
    }

    #[test]
    fn un_message_isole_cree_son_fil() {
        let f = fixture();
        let r = f.store.insert_message(&f.msg("a@x", 1000)).unwrap();
        assert!(r.thread_created);
        assert!(!r.was_known);
    }

    #[test]
    fn une_reponse_rejoint_le_fil_de_l_original() {
        let f = fixture();
        let a = f.store.insert_message(&f.msg("a@x", 1000)).unwrap();

        let mut reponse = f.msg("b@x", 2000);
        reponse.in_reply_to = Some("a@x".into());
        let b = f.store.insert_message(&reponse).unwrap();

        assert_eq!(a.thread, b.thread);
        assert!(!b.thread_created);
    }

    #[test]
    fn une_reponse_arrivee_avant_l_original_recolle_le_fil() {
        // Cas fréquent en synchronisation désordonnée : sans ce rattrapage, on
        // obtiendrait deux fils pour une seule conversation.
        let f = fixture();
        let mut reponse = f.msg("b@x", 2000);
        reponse.in_reply_to = Some("a@x".into());
        reponse.references = vec!["a@x".into()];
        let b = f.store.insert_message(&reponse).unwrap();
        assert!(b.thread_created);

        let a = f.store.insert_message(&f.msg("a@x", 1000)).unwrap();
        assert_eq!(
            a.thread, b.thread,
            "l'original doit rejoindre le fil existant"
        );
        assert!(!a.thread_created);
    }

    #[test]
    fn two_answers_to_a_message_not_here_are_one_thread() {
        let f = fixture();
        let mut une = f.msg("b@x", 2000);
        une.in_reply_to = Some("absent@x".into());
        une.references = vec!["absent@x".into()];
        let mut autre = f.msg("c@x", 3000);
        autre.in_reply_to = Some("absent@x".into());
        autre.references = vec!["absent@x".into()];

        let b = f.store.insert_message(&une).unwrap();
        let c = f.store.insert_message(&autre).unwrap();
        assert_eq!(b.thread, c.thread);
        assert!(!c.thread_created);
    }

    #[test]
    fn reinserer_le_meme_uid_ne_cree_pas_de_doublon() {
        let f = fixture();
        let m = f.msg("a@x", 1000);
        let first = f.store.insert_message(&m).unwrap();

        let mut relu = m.clone();
        relu.flags = Flags::SEEN;
        let second = f.store.insert_message(&relu).unwrap();

        assert_eq!(first.message, second.message);
        assert!(second.was_known);
        assert_eq!(f.store.message_count().unwrap(), 1);
        // Les drapeaux ont bien été rafraîchis au passage.
        assert_eq!(
            f.store.thread_messages(first.thread).unwrap()[0].flags,
            Flags::SEEN
        );
    }

    #[test]
    fn les_agregats_du_fil_suivent_les_messages() {
        let f = fixture();
        let a = f.store.insert_message(&f.msg("a@x", 1000)).unwrap();

        let mut second = f.msg("b@x", 3000);
        second.in_reply_to = Some("a@x".into());
        second.from_name = "Luc".into();
        second.flags = Flags::SEEN | Flags::HAS_ATTACHMENT;
        f.store.insert_message(&second).unwrap();

        let row = f.store.thread_row(a.thread).unwrap().unwrap();
        assert_eq!(row.message_count, 2);
        assert_eq!(row.unread_count, 1, "seul le premier est non lu");
        assert_eq!(row.last_activity, Timestamp::from_millis(3000));
        assert_eq!(
            row.from_display, "Luc",
            "la liste montre le dernier expéditeur"
        );
        assert!(row.flags_union.contains(Flags::HAS_ATTACHMENT));
    }

    #[test]
    fn l_union_des_drapeaux_n_est_pas_une_somme() {
        // Deux messages « vus » ne doivent pas produire le drapeau 2, qui est
        // « répondu ». C'est l'erreur classique du sum() sur un champ de bits.
        let f = fixture();
        let mut a = f.msg("a@x", 1000);
        a.flags = Flags::SEEN;
        let ins = f.store.insert_message(&a).unwrap();

        let mut b = f.msg("b@x", 2000);
        b.in_reply_to = Some("a@x".into());
        b.flags = Flags::SEEN;
        f.store.insert_message(&b).unwrap();

        let row = f.store.thread_row(ins.thread).unwrap().unwrap();
        assert_eq!(row.flags_union, Flags::SEEN);
        assert!(!row.flags_union.contains(Flags::ANSWERED));
    }

    #[test]
    fn supprimer_le_dernier_message_supprime_le_fil() {
        let f = fixture();
        let r = f.store.insert_message(&f.msg("a@x", 1000)).unwrap();
        assert_eq!(f.store.delete_messages_by_uid(f.folder, &[1]).unwrap(), 1);
        assert!(f.store.thread_row(r.thread).unwrap().is_none());
    }

    #[test]
    fn supprimer_un_message_sur_deux_conserve_le_fil() {
        let f = fixture();
        let a = f.store.insert_message(&f.msg("a@x", 1000)).unwrap();
        let mut b = f.msg("b@x", 2000);
        b.in_reply_to = Some("a@x".into());
        f.store.insert_message(&b).unwrap();

        f.store.delete_messages_by_uid(f.folder, &[2]).unwrap();
        let row = f.store.thread_row(a.thread).unwrap().unwrap();
        assert_eq!(row.message_count, 1);
        assert_eq!(row.last_activity, Timestamp::from_millis(1000));
    }

    #[test]
    fn changer_les_drapeaux_met_le_fil_a_jour() {
        let f = fixture();
        let r = f.store.insert_message(&f.msg("a@x", 1000)).unwrap();
        let id = f.store.thread_messages(r.thread).unwrap()[0].id;

        assert_eq!(
            f.store.thread_row(r.thread).unwrap().unwrap().unread_count,
            1
        );
        f.store.set_message_flags(id, Flags::SEEN).unwrap();
        assert_eq!(
            f.store.thread_row(r.thread).unwrap().unwrap().unread_count,
            0
        );
    }

    #[test]
    fn les_drapeaux_d_un_message_inconnu_ne_font_rien() {
        let f = fixture();
        assert!(f
            .store
            .set_message_flags(MessageId(999), Flags::SEEN)
            .unwrap()
            .is_none());
    }

    #[test]
    fn un_message_se_relit_par_son_identifiant() {
        let f = fixture();
        let r = f.store.insert_message(&f.msg("a@x", 1000)).unwrap();

        let relu = f.store.message_by_id(r.message).unwrap().unwrap();
        assert_eq!(relu.id, r.message);
        assert_eq!(relu.subject, "Devis refonte");
        assert!(f.store.message_by_id(MessageId(999)).unwrap().is_none());
    }

    #[test]
    fn les_messages_sans_corps_sont_listes() {
        let f = fixture();
        let a = f.store.insert_message(&f.msg("a@x", 1000)).unwrap();
        f.store.insert_message(&f.msg("b@x", 2000)).unwrap();
        assert_eq!(
            f.store
                .folder_messages_without_body(f.folder, 10)
                .unwrap()
                .len(),
            2
        );

        f.store
            .attach_body(a.message, "00112233445566778899aabbccddeeff")
            .unwrap();
        let restants = f.store.folder_messages_without_body(f.folder, 10).unwrap();
        assert_eq!(restants.len(), 1);
        assert_ne!(restants[0].id, a.message);
    }

    #[test]
    fn les_contenus_references_sont_recenses() {
        let f = fixture();
        let r = f.store.insert_message(&f.msg("a@x", 1000)).unwrap();
        assert!(f.store.referenced_blobs().unwrap().is_empty());

        f.store
            .attach_body(r.message, "00112233445566778899aabbccddeeff")
            .unwrap();
        assert_eq!(
            f.store.referenced_blobs().unwrap(),
            ["00112233445566778899aabbccddeeff"]
        );
    }

    #[test]
    fn normalisation_des_sujets() {
        assert_eq!(normalize_subject("Re: Devis"), "devis");
        assert_eq!(
            normalize_subject("RE: Fwd:  Devis   refonte "),
            "devis refonte"
        );
        assert_eq!(normalize_subject("TR: Rép: Devis"), "devis");
        assert_eq!(normalize_subject("Devis"), "devis");
        assert_eq!(normalize_subject(""), "");
        // Un sujet qui commence par « Réponse » n'est pas un préfixe de réponse.
        assert_eq!(normalize_subject("Réponse attendue"), "réponse attendue");
    }

    #[test]
    fn un_lot_s_insere_en_une_transaction() {
        let f = fixture();
        let lot: Vec<_> = (0..50)
            .map(|i| f.msg(&format!("m{i}@x"), 1000 + i))
            .collect();
        let out = f.store.insert_messages(&lot).unwrap();
        assert_eq!(out.len(), 50);
        assert_eq!(f.store.message_count().unwrap(), 50);
    }
}
