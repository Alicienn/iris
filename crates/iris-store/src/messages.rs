//! Insertion des messages et rattachement aux fils.
//!
//! Le rattachement suit la même règle que l'algorithme de threading (`iris-thread`),
//! mais interrogée du côté base : un message rejoint le fil de n'importe quel message
//! qu'il cite, ou celui de n'importe quel message qui le cite. Le second cas est
//! celui, fréquent, où la réponse arrive avant l'original — sur une boîte
//! synchronisée en désordre, l'ignorer produirait deux fils là où il n'y en a qu'un.

use crate::model::{NewMessage, StoredMessage};
use crate::{sql_err, Store};
use iris_types::{
    AccountId, Flags, FolderId, MessageId, Result, ThreadId, Timestamp, WorkflowState,
};
use rusqlite::{params, params_from_iter, Transaction};

/// Résultat de l'insertion d'un message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inserted {
    pub message: MessageId,
    pub thread: ThreadId,
    /// Le message existait déjà : seuls ses drapeaux ont pu changer.
    pub was_known: bool,
    /// Un fil a été créé pour l'occasion.
    pub thread_created: bool,
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
    pub fn thread_messages(&self, thread: ThreadId) -> Result<Vec<StoredMessage>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, account_id, folder_id, thread_id, uid, rfc_message_id, subject,
                            from_name, from_addr, date, received, size, flags, preview, body_blob
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

    /// Supprime les messages d'un dossier dont l'UID n'est plus présent côté serveur.
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
                            from_name, from_addr, date, received, size, flags, preview, body_blob
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
                            from_name, from_addr, date, received, size, flags, preview, body_blob
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

                // Ne rien écrire quand rien ne change : sur une resynchronisation
                // complète, la quasi-totalité des drapeaux sont identiques.
                if actuels == flags.0 as i64 {
                    continue;
                }

                let mut stmt = tx
                    .prepare_cached(
                        "UPDATE messages SET flags = ?1 WHERE folder_id = ?2 AND uid = ?3",
                    )
                    .map_err(|e| sql_err("préparation", e))?;
                stmt.execute(params![flags.0 as i64, folder.get(), *uid as i64])
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
        let mut stmt = tx
            .prepare_cached("UPDATE messages SET flags = ?1 WHERE id = ?2")
            .map_err(|e| sql_err("preparation", e))?;
        stmt.execute(params![m.flags.0 as i64, id])
            .map_err(|e| sql_err("rafraichissement des drapeaux", e))?;
        return Ok(Inserted {
            message: MessageId(id),
            thread: ThreadId(thread),
            was_known: true,
            thread_created: false,
        });
    }

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

    Ok(Inserted {
        message,
        thread,
        was_known: false,
        thread_created,
    })
}

/// Trouve le fil auquel rattacher un message, ou en cree un.
fn resolve_thread(tx: &Transaction<'_>, m: &NewMessage) -> Result<(ThreadId, bool)> {
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
    let subject_norm = normalize_subject(&m.subject);
    let mut stmt = tx
        .prepare_cached(
            "INSERT INTO threads (subject_norm, state, last_activity_at) VALUES (?1, ?2, ?3)",
        )
        .map_err(|e| sql_err("preparation", e))?;
    stmt.execute(params![
        subject_norm,
        WorkflowState::Todo.as_i64(),
        m.received.millis()
    ])
    .map_err(|e| sql_err("creation du fil", e))?;
    Ok((ThreadId(tx.last_insert_rowid()), true))
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
    let mut last: Option<(String, String, String, String)> = None;

    {
        let mut stmt = tx
            .prepare_cached(
                "SELECT flags, received, from_name, from_addr, subject, preview
                 FROM messages WHERE thread_id = ?1 ORDER BY received DESC, id DESC",
            )
            .map_err(|e| sql_err("preparation", e))?;
        let mut rows = stmt
            .query(params![thread.get()])
            .map_err(|e| sql_err("agregats du fil", e))?;

        while let Some(r) = rows.next().map_err(|e| sql_err("agregats du fil", e))? {
            let flags: i64 = r.get(0).map_err(|e| sql_err("agregats du fil", e))?;
            let received: i64 = r.get(1).map_err(|e| sql_err("agregats du fil", e))?;
            count += 1;
            union |= flags;
            if flags & (Flags::SEEN.0 as i64) == 0 {
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
                                last_subject = ?7, last_preview = ?8
             WHERE id = ?9",
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
