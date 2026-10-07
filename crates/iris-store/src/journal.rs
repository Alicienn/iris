//! Le journal d'opérations.
//!
//! Invariant n° 3 de la spécification : toute action locale est instantanée, puis
//! réconciliée. Le journal est ce qui rend la seconde moitié possible. Il porte trois
//! propriétés :
//!
//! - **idempotence** : chaque opération a une clé stable ; l'enregistrer deux fois
//!   ne produit qu'une entrée, ce qui permet de rejouer sans crainte après une
//!   coupure survenue entre l'action et sa confirmation ;
//! - **repli exponentiel** : un serveur en panne n'est pas martelé ;
//! - **ordre préservé par compte** : déplacer puis supprimer un message ne doit pas
//!   s'exécuter dans l'autre sens.

use crate::model::{OpKind, PendingOp};
use crate::{sql_err, Store};
use iris_types::{AccountId, OpId, Result, Timestamp};
use rusqlite::{params, OptionalExtension};

/// Délai avant nouvelle tentative, en secondes, selon le nombre d'échecs.
///
/// Croissance exponentielle plafonnée à cinq minutes : au-delà, l'utilisateur a
/// besoin d'un signal, pas d'une attente plus longue.
pub fn backoff_secs(attempts: u32) -> i64 {
    const MAX: i64 = 300;
    match attempts {
        0 => 0,
        n => (2i64.saturating_pow(n.min(10))).min(MAX),
    }
}

impl Store {
    /// Enregistre une opération à réconcilier.
    ///
    /// La même intention enregistrée deux fois de suite n'est qu'une opération : la
    /// dernière en attente du compte est retournée. Mais une intention qui revient
    /// après une autre (lu, non lu, puis lu) est une nouvelle étape : la clé ne
    /// décrit que l'effet visé, et la garder pour toujours faisait ignorer en silence
    /// le troisième geste, que le serveur ne recevait jamais.
    pub fn enqueue_op(
        &self,
        account: AccountId,
        kind: OpKind,
        payload: &str,
        idempotency_key: &str,
        now: Timestamp,
    ) -> Result<OpId> {
        self.with_tx(|tx| {
            let derniere: Option<(i64, String)> = tx
                .query_row(
                    "SELECT id, idempotency_key FROM op_journal
                     WHERE account_id = ?1 AND done = 0
                     ORDER BY id DESC LIMIT 1",
                    params![account.get()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(|e| sql_err("dernière opération", e))?;
            if let Some((id, clef)) = derniere {
                if clef == idempotency_key {
                    return Ok(OpId(id));
                }
            }

            // Toute autre ligne qui porte la clé (acquittée, ou suivie d'une autre
            // intention) la cède : la colonne est unique.
            tx.execute(
                "UPDATE op_journal SET idempotency_key = idempotency_key || '#' || id
                 WHERE idempotency_key = ?1",
                params![idempotency_key],
            )
            .map_err(|e| sql_err("libération de la clé", e))?;

            // The folder's validity as its UIDs were read: replay drops the operation
            // if the server has rebuilt the folder since, whatever a sync has stored.
            let validite: i64 = match crate::OpPayload::parse(payload) {
                Ok(charge) if charge.uses_uids() => tx
                    .query_row(
                        "SELECT uid_validity FROM folders WHERE account_id = ?1 AND path = ?2",
                        params![account.get(), charge.folder()],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| sql_err("validité du dossier", e))?
                    .unwrap_or(0),
                _ => 0,
            };

            tx.execute(
                "INSERT INTO op_journal
                   (account_id, kind, payload, idempotency_key, created_at, next_attempt_at,
                    uid_validity)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
                params![
                    account.get(),
                    kind.as_str(),
                    payload,
                    idempotency_key,
                    now.millis(),
                    validite
                ],
            )
            .map_err(|e| sql_err("enregistrement de l'opération", e))?;
            Ok(OpId(tx.last_insert_rowid()))
        })
    }

    /// Les opérations prêtes à être rejouées, tous comptes confondus, dans l'ordre
    /// d'enregistrement.
    pub fn pending_ops(&self, now: Timestamp, limit: u32) -> Result<Vec<PendingOp>> {
        self.pending_ops_where(None, now, limit)
    }

    /// Les opérations d'un compte prêtes à être rejouées.
    ///
    /// Filtrer après coup les cent plus anciennes de tous les comptes laissait un
    /// compte retiré, éteint ou refusé occuper la fenêtre : les actions des autres
    /// n'atteignaient plus jamais leur serveur.
    pub fn pending_ops_for(
        &self,
        account: AccountId,
        now: Timestamp,
        limit: u32,
    ) -> Result<Vec<PendingOp>> {
        self.pending_ops_where(Some(account), now, limit)
    }

    fn pending_ops_where(
        &self,
        account: Option<AccountId>,
        now: Timestamp,
        limit: u32,
    ) -> Result<Vec<PendingOp>> {
        // In order, per account, up to the first one still waiting out its back-off:
        // the ones after it may be about the same messages. Taking every one whose time
        // had come let "read" then "unread" end read on the server, the failed
        // "unread" replayed after the "read" that followed it.
        let toutes = self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached(
                    "SELECT id, account_id, kind, payload, idempotency_key, created_at,
                            attempts, next_attempt_at, last_error, uid_validity
                     FROM op_journal
                     WHERE done != 1
                       AND (?3 IS NULL OR account_id = ?3)
                     ORDER BY id ASC LIMIT ?2",
                )
                .map_err(|e| sql_err("préparation", e))?;
            let rows = stmt
                .query_map(
                    params![now.millis(), limit as i64 * 4, account.map(|a| a.get())],
                    |r| {
                        Ok(PendingOp {
                            id: OpId(r.get(0)?),
                            account: AccountId(r.get(1)?),
                            kind: OpKind::parse(&r.get::<_, String>(2)?)
                                .unwrap_or(OpKind::SetFlags),
                            payload: r.get(3)?,
                            idempotency_key: r.get(4)?,
                            created_at: Timestamp::from_millis(r.get(5)?),
                            attempts: r.get::<_, i64>(6)? as u32,
                            next_attempt_at: Timestamp::from_millis(r.get(7)?),
                            last_error: r.get(8)?,
                            uid_validity: r.get::<_, i64>(9)? as u32,
                        })
                    },
                )
                .map_err(|e| sql_err("opérations en attente", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("opérations en attente", e))
        })?;

        let mut bloques = std::collections::HashSet::new();
        let mut pretes = Vec::new();
        for op in toutes {
            if bloques.contains(&op.account) {
                continue;
            }
            if op.next_attempt_at > now {
                bloques.insert(op.account);
                continue;
            }
            pretes.push(op);
            if pretes.len() >= limit as usize {
                break;
            }
        }
        Ok(pretes)
    }

    /// Where the latest move journalled for this message sent it, if one did: the
    /// target folder of a `Move` from `folder` naming `uid`, waiting or carried out.
    pub fn moved_to(&self, account: AccountId, folder: &str, uid: u32) -> Result<Option<String>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached(
                    "SELECT payload FROM op_journal
                     WHERE account_id = ?1 AND kind IN (?2, ?3)
                     ORDER BY id DESC LIMIT 200",
                )
                .map_err(|e| sql_err("préparation", e))?;
            let lignes = stmt
                .query_map(
                    params![
                        account.get(),
                        OpKind::MoveMessage.as_str(),
                        OpKind::DeleteMessage.as_str()
                    ],
                    |r| r.get::<_, String>(0),
                )
                .map_err(|e| sql_err("déplacements", e))?;
            // Moves back by `Message-ID` (an undo once the server had moved it), newer
            // than what is looked at: the message is back where it was, under a UID
            // no one knows yet, and is found by its `Message-ID` there.
            let mut retours: Vec<(String, String)> = Vec::new();
            for ligne in lignes {
                let charge = ligne.map_err(|e| sql_err("déplacements", e))?;
                match crate::OpPayload::parse(&charge) {
                    Ok(crate::OpPayload::Move {
                        folder: source,
                        uids,
                        target,
                    }) if source == folder && uids.contains(&uid) => {
                        let revenu = retours
                            .iter()
                            .any(|(de, vers)| *de == target && vers == folder);
                        return Ok(Some(if revenu { folder.to_string() } else { target }));
                    }
                    Ok(crate::OpPayload::MoveByMessageId {
                        folder: de,
                        target: vers,
                        ..
                    }) => retours.push((de, vers)),
                    _ => {}
                }
            }
            Ok(None)
        })
    }

    /// Takes an operation for replay, just before it is sent: `false` if it was
    /// withdrawn meanwhile. Once taken (`done = 2`) it can no longer be withdrawn, and
    /// undo reverses it on the server instead. Withdrawn while replay held it, it was
    /// still carried out there and undone only here.
    ///
    /// One left taken by a replay that never finished (Iris closed mid-pass) is handed
    /// over again: only one pass runs per account.
    pub fn claim_op(&self, id: OpId) -> Result<bool> {
        self.with_conn(|c| {
            let n = c
                .execute(
                    "UPDATE op_journal SET done = 2 WHERE id = ?1 AND done != 1",
                    params![id.get()],
                )
                .map_err(|e| sql_err("prise de l'opération", e))?;
            Ok(n > 0)
        })
    }

    /// Takes back an operation the server has not been told of yet. Says whether it
    /// was still waiting: once replayed, it can only be reversed by another.
    pub fn withdraw_op(&self, id: OpId) -> Result<bool> {
        self.with_conn(|c| {
            let n = c
                .execute(
                    "DELETE FROM op_journal WHERE id = ?1 AND done = 0",
                    params![id.get()],
                )
                .map_err(|e| sql_err("retrait de l'opération", e))?;
            Ok(n > 0)
        })
    }

    /// Marque une opération comme réconciliée.
    pub fn complete_op(&self, id: OpId) -> Result<bool> {
        self.with_conn(|c| {
            let n = c
                .execute(
                    "UPDATE op_journal SET done = 1, last_error = NULL WHERE id = ?1",
                    params![id.get()],
                )
                .map_err(|e| sql_err("acquittement", e))?;
            Ok(n > 0)
        })
    }

    /// Enregistre un échec et programme la prochaine tentative.
    pub fn fail_op(&self, id: OpId, error: &str, now: Timestamp) -> Result<u32> {
        self.with_tx(|tx| {
            let attempts: i64 = tx
                .query_row(
                    "SELECT attempts FROM op_journal WHERE id = ?1",
                    params![id.get()],
                    |r| r.get(0),
                )
                .map_err(|e| sql_err("lecture des tentatives", e))?;
            let attempts = attempts + 1;
            let next = now.millis() + backoff_secs(attempts as u32) * 1000;

            tx.execute(
                "UPDATE op_journal SET attempts = ?1, next_attempt_at = ?2, last_error = ?3
                 WHERE id = ?4",
                params![attempts, next, error, id.get()],
            )
            .map_err(|e| sql_err("enregistrement de l'échec", e))?;

            Ok(attempts as u32)
        })
    }

    /// Nombre d'opérations non réconciliées.
    pub fn pending_op_count(&self) -> Result<u64> {
        self.with_conn(|c| {
            c.query_row("SELECT count(*) FROM op_journal WHERE done != 1", [], |r| {
                r.get::<_, i64>(0)
            })
            .map(|n| n as u64)
            .map_err(|e| sql_err("comptage", e))
        })
    }

    /// Forgets the states left by messages that went before `before` and never came
    /// back (deleted for good, most of them).
    pub fn purge_thread_ghosts(&self, before: Timestamp) -> Result<usize> {
        self.with_conn(|c| {
            c.execute(
                "DELETE FROM thread_ghosts WHERE left_at < ?1",
                params![before.millis()],
            )
            .map_err(|e| sql_err("purge des fantômes", e))
        })
    }

    /// Purge les opérations réconciliées plus anciennes que la date donnée.
    ///
    /// Elles ne servent plus qu'au diagnostic : sans purge, le journal grossit sans
    /// fin.
    pub fn purge_completed_ops(&self, before: Timestamp) -> Result<usize> {
        self.with_conn(|c| {
            c.execute(
                "DELETE FROM op_journal WHERE done = 1 AND created_at < ?1",
                params![before.millis()],
            )
            .map_err(|e| sql_err("purge du journal", e))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewAccount;

    fn setup() -> (Store, AccountId) {
        let s = Store::in_memory().unwrap();
        let a = s
            .create_account(
                &NewAccount::new("a@x.fr", "i", "s"),
                Timestamp::from_millis(0),
            )
            .unwrap();
        (s, a)
    }

    fn t(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    #[test]
    fn une_operation_enregistree_est_en_attente() {
        let (s, a) = setup();
        let id = s
            .enqueue_op(a, OpKind::SetFlags, "{}", "clef-1", t(0))
            .unwrap();

        let ops = s.pending_ops(t(0), 10).unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].id, id);
        assert_eq!(ops[0].attempts, 0);
        assert_eq!(s.pending_op_count().unwrap(), 1);
    }

    #[test]
    fn la_meme_clef_ne_produit_qu_une_entree() {
        // Propriété centrale : après une coupure entre l'action et sa confirmation,
        // rejouer ne doit pas dupliquer l'opération.
        let (s, a) = setup();
        let un = s
            .enqueue_op(a, OpKind::SendMessage, "{\"a\":1}", "clef", t(0))
            .unwrap();
        let deux = s
            .enqueue_op(a, OpKind::SendMessage, "{\"a\":2}", "clef", t(5))
            .unwrap();
        assert_eq!(un, deux);
        assert_eq!(s.pending_op_count().unwrap(), 1);
        // La charge d'origine est conservée : la première intention fait foi.
        assert_eq!(s.pending_ops(t(10), 10).unwrap()[0].payload, "{\"a\":1}");
    }

    #[test]
    fn l_ordre_d_enregistrement_est_preserve() {
        let (s, a) = setup();
        for i in 0..5 {
            s.enqueue_op(a, OpKind::MoveMessage, "{}", &format!("k{i}"), t(i))
                .unwrap();
        }
        let ops = s.pending_ops(t(100), 10).unwrap();
        let clefs: Vec<_> = ops.iter().map(|o| o.idempotency_key.as_str()).collect();
        assert_eq!(clefs, ["k0", "k1", "k2", "k3", "k4"]);
    }

    #[test]
    fn acquitter_retire_de_la_file() {
        let (s, a) = setup();
        let id = s.enqueue_op(a, OpKind::SetFlags, "{}", "k", t(0)).unwrap();
        assert!(s.complete_op(id).unwrap());
        assert!(s.pending_ops(t(0), 10).unwrap().is_empty());
        assert_eq!(s.pending_op_count().unwrap(), 0);
    }

    #[test]
    fn un_echec_repousse_la_prochaine_tentative() {
        let (s, a) = setup();
        let id = s.enqueue_op(a, OpKind::SetFlags, "{}", "k", t(0)).unwrap();

        assert_eq!(s.fail_op(id, "serveur injoignable", t(0)).unwrap(), 1);
        // Deux secondes de repli après le premier échec.
        assert!(s.pending_ops(t(1_000), 10).unwrap().is_empty());
        let reprise = s.pending_ops(t(2_000), 10).unwrap();
        assert_eq!(reprise.len(), 1);
        assert_eq!(
            reprise[0].last_error.as_deref(),
            Some("serveur injoignable")
        );
    }

    #[test]
    fn le_repli_est_exponentiel_puis_plafonne() {
        assert_eq!(backoff_secs(0), 0);
        assert_eq!(backoff_secs(1), 2);
        assert_eq!(backoff_secs(2), 4);
        assert_eq!(backoff_secs(8), 256);
        // Plafond : un serveur en panne ne doit pas repousser la reprise à l'infini.
        assert_eq!(backoff_secs(9), 300);
        assert_eq!(backoff_secs(1000), 300);
    }

    #[test]
    fn les_tentatives_s_accumulent() {
        let (s, a) = setup();
        let id = s.enqueue_op(a, OpKind::SetFlags, "{}", "k", t(0)).unwrap();
        for attendu in 1..=3 {
            assert_eq!(s.fail_op(id, "erreur", t(0)).unwrap(), attendu);
        }
        assert_eq!(s.pending_ops(t(1_000_000), 10).unwrap()[0].attempts, 3);
    }

    #[test]
    fn la_purge_epargne_les_operations_recentes() {
        let (s, a) = setup();
        let vieille = s
            .enqueue_op(a, OpKind::SetFlags, "{}", "vieille", t(1_000))
            .unwrap();
        let recente = s
            .enqueue_op(a, OpKind::SetFlags, "{}", "récente", t(9_000))
            .unwrap();
        s.complete_op(vieille).unwrap();
        s.complete_op(recente).unwrap();

        assert_eq!(s.purge_completed_ops(t(5_000)).unwrap(), 1);
        assert_eq!(s.purge_completed_ops(t(5_000)).unwrap(), 0);
    }

    #[test]
    fn une_intention_acquittee_peut_etre_refaite() {
        // Lu, non lu, puis lu : le troisième geste doit atteindre le serveur. La clé
        // gardée après succès le faisait ignorer en silence.
        let (s, a) = setup();
        let lu = s.enqueue_op(a, OpKind::SetFlags, "{}", "lu", t(0)).unwrap();
        s.complete_op(lu).unwrap();
        let non_lu = s
            .enqueue_op(a, OpKind::SetFlags, "{}", "non-lu", t(1))
            .unwrap();
        s.complete_op(non_lu).unwrap();

        let encore = s.enqueue_op(a, OpKind::SetFlags, "{}", "lu", t(2)).unwrap();
        assert_ne!(encore, lu);
        assert_eq!(s.pending_op_count().unwrap(), 1);
    }

    #[test]
    fn une_intention_qui_revient_apres_une_autre_est_une_nouvelle_etape() {
        // Toutes en attente : lu, non lu, lu. Fusionner le troisième avec le premier
        // laisserait le serveur sur « non lu » alors que l'écran dit « lu ».
        let (s, a) = setup();
        s.enqueue_op(a, OpKind::SetFlags, "{}", "lu", t(0)).unwrap();
        s.enqueue_op(a, OpKind::SetFlags, "{}", "non-lu", t(1))
            .unwrap();
        s.enqueue_op(a, OpKind::SetFlags, "{}", "lu", t(2)).unwrap();

        let ops = s.pending_ops(t(10), 10).unwrap();
        assert_eq!(ops.len(), 3);
        assert_eq!(ops.last().unwrap().idempotency_key, "lu");
    }

    #[test]
    fn an_operation_waiting_to_be_tried_again_holds_back_those_after_it() {
        // "Unread" failed and waits; "read" after it must not reach the server first.
        let (s, a) = setup();
        let non_lu = s
            .enqueue_op(a, OpKind::SetFlags, "{}", "non-lu", t(0))
            .unwrap();
        s.enqueue_op(a, OpKind::SetFlags, "{}", "lu", t(1)).unwrap();
        s.fail_op(non_lu, "serveur occupé", t(0)).unwrap();

        assert!(s.pending_ops_for(a, t(1_000), 10).unwrap().is_empty());
        let reprise = s.pending_ops_for(a, t(5_000), 10).unwrap();
        assert_eq!(reprise.len(), 2);
        assert_eq!(reprise[0].idempotency_key, "non-lu");
    }

    #[test]
    fn la_file_d_un_compte_ignore_celle_des_autres() {
        // Cent opérations d'un compte bloqué ne doivent pas cacher celles d'un autre.
        let (s, a) = setup();
        let b = s
            .create_account(&NewAccount::new("b@x.fr", "i", "s"), t(0))
            .unwrap();
        for i in 0..5 {
            s.enqueue_op(a, OpKind::SetFlags, "{}", &format!("a{i}"), t(0))
                .unwrap();
        }
        s.enqueue_op(b, OpKind::SetFlags, "{}", "b0", t(0)).unwrap();

        let de_b = s.pending_ops_for(b, t(0), 3).unwrap();
        assert_eq!(de_b.len(), 1);
        assert_eq!(de_b[0].idempotency_key, "b0");
    }

    #[test]
    fn retirer_un_compte_vide_son_journal() {
        let (s, a) = setup();
        s.enqueue_op(a, OpKind::SetFlags, "{}", "k", t(0)).unwrap();
        s.delete_account(a).unwrap();
        assert_eq!(s.pending_op_count().unwrap(), 0);
    }

    #[test]
    fn la_purge_ne_touche_pas_aux_operations_en_attente() {
        let (s, a) = setup();
        s.enqueue_op(a, OpKind::SetFlags, "{}", "k", t(1_000))
            .unwrap();
        assert_eq!(s.purge_completed_ops(t(999_999)).unwrap(), 0);
        assert_eq!(s.pending_op_count().unwrap(), 1);
    }

    #[test]
    fn la_limite_de_lot_est_respectee() {
        let (s, a) = setup();
        for i in 0..20 {
            s.enqueue_op(a, OpKind::SetFlags, "{}", &format!("k{i}"), t(0))
                .unwrap();
        }
        assert_eq!(s.pending_ops(t(0), 5).unwrap().len(), 5);
    }
}
