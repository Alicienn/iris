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
use rusqlite::params;

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
    /// Retourne l'identifiant existant si la clé d'idempotence est déjà connue.
    pub fn enqueue_op(
        &self,
        account: AccountId,
        kind: OpKind,
        payload: &str,
        idempotency_key: &str,
        now: Timestamp,
    ) -> Result<OpId> {
        self.with_conn(|c| {
            c.execute(
                "INSERT OR IGNORE INTO op_journal
                   (account_id, kind, payload, idempotency_key, created_at, next_attempt_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                params![
                    account.get(),
                    kind.as_str(),
                    payload,
                    idempotency_key,
                    now.millis()
                ],
            )
            .map_err(|e| sql_err("enregistrement de l'opération", e))?;

            c.query_row(
                "SELECT id FROM op_journal WHERE idempotency_key = ?1",
                params![idempotency_key],
                |r| r.get::<_, i64>(0),
            )
            .map(OpId)
            .map_err(|e| sql_err("relecture de l'opération", e))
        })
    }

    /// Les opérations prêtes à être rejouées, dans l'ordre d'enregistrement.
    pub fn pending_ops(&self, now: Timestamp, limit: u32) -> Result<Vec<PendingOp>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached(
                    "SELECT id, account_id, kind, payload, idempotency_key, created_at,
                            attempts, next_attempt_at, last_error
                     FROM op_journal
                     WHERE done = 0 AND next_attempt_at <= ?1
                     ORDER BY id ASC LIMIT ?2",
                )
                .map_err(|e| sql_err("préparation", e))?;
            let rows = stmt
                .query_map(params![now.millis(), limit as i64], |r| {
                    Ok(PendingOp {
                        id: OpId(r.get(0)?),
                        account: AccountId(r.get(1)?),
                        kind: OpKind::parse(&r.get::<_, String>(2)?).unwrap_or(OpKind::SetFlags),
                        payload: r.get(3)?,
                        idempotency_key: r.get(4)?,
                        created_at: Timestamp::from_millis(r.get(5)?),
                        attempts: r.get::<_, i64>(6)? as u32,
                        next_attempt_at: Timestamp::from_millis(r.get(7)?),
                        last_error: r.get(8)?,
                    })
                })
                .map_err(|e| sql_err("opérations en attente", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("opérations en attente", e))
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
                .query_row("SELECT attempts FROM op_journal WHERE id = ?1", params![id.get()], |r| {
                    r.get(0)
                })
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
            c.query_row("SELECT count(*) FROM op_journal WHERE done = 0", [], |r| {
                r.get::<_, i64>(0)
            })
            .map(|n| n as u64)
            .map_err(|e| sql_err("comptage", e))
        })
    }

    /// Purge les opérations réconciliées plus anciennes que la date donnée.
    ///
    /// Les clés d'idempotence sont conservées un temps après succès : sans cela, un
    /// rejeu tardif provoqué par une reprise du réseau pourrait renvoyer un message
    /// déjà parti.
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
            .create_account(&NewAccount::new("a@x.fr", "i", "s"), Timestamp::from_millis(0))
            .unwrap();
        (s, a)
    }

    fn t(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    #[test]
    fn une_operation_enregistree_est_en_attente() {
        let (s, a) = setup();
        let id = s.enqueue_op(a, OpKind::SetFlags, "{}", "clef-1", t(0)).unwrap();

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
        let un = s.enqueue_op(a, OpKind::SendMessage, "{\"a\":1}", "clef", t(0)).unwrap();
        let deux = s.enqueue_op(a, OpKind::SendMessage, "{\"a\":2}", "clef", t(5)).unwrap();
        assert_eq!(un, deux);
        assert_eq!(s.pending_op_count().unwrap(), 1);
        // La charge d'origine est conservée : la première intention fait foi.
        assert_eq!(s.pending_ops(t(10), 10).unwrap()[0].payload, "{\"a\":1}");
    }

    #[test]
    fn l_ordre_d_enregistrement_est_preserve() {
        let (s, a) = setup();
        for i in 0..5 {
            s.enqueue_op(a, OpKind::MoveMessage, "{}", &format!("k{i}"), t(i)).unwrap();
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
        assert_eq!(reprise[0].last_error.as_deref(), Some("serveur injoignable"));
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
        let vieille = s.enqueue_op(a, OpKind::SetFlags, "{}", "vieille", t(1_000)).unwrap();
        let recente = s.enqueue_op(a, OpKind::SetFlags, "{}", "récente", t(9_000)).unwrap();
        s.complete_op(vieille).unwrap();
        s.complete_op(recente).unwrap();

        assert_eq!(s.purge_completed_ops(t(5_000)).unwrap(), 1);
        // La clef récente subsiste : un rejeu tardif ne doit pas renvoyer le message.
        let reste = s.enqueue_op(a, OpKind::SetFlags, "{}", "récente", t(10_000)).unwrap();
        assert_eq!(reste, recente);
    }

    #[test]
    fn la_purge_ne_touche_pas_aux_operations_en_attente() {
        let (s, a) = setup();
        s.enqueue_op(a, OpKind::SetFlags, "{}", "k", t(1_000)).unwrap();
        assert_eq!(s.purge_completed_ops(t(999_999)).unwrap(), 0);
        assert_eq!(s.pending_op_count().unwrap(), 1);
    }

    #[test]
    fn la_limite_de_lot_est_respectee() {
        let (s, a) = setup();
        for i in 0..20 {
            s.enqueue_op(a, OpKind::SetFlags, "{}", &format!("k{i}"), t(0)).unwrap();
        }
        assert_eq!(s.pending_ops(t(0), 5).unwrap().len(), 5);
    }
}
