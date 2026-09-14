//! La liste principale : lecture paginée par curseur et transitions d'état.

use crate::model::{ListQuery, ThreadRow};
use crate::{sql_err, Store};
use iris_types::{
    AccountId, Address, Flags, MessageId, Result, Snooze, ThreadId, Timestamp, WorkflowState,
};
use rusqlite::{params, params_from_iter, types::Value as SqlValue, Row};
use std::collections::BTreeMap;

/// The `Flags::SPAM` bit, as SQL sees it.
///
/// Written here rather than imported because it is interpolated into query text; the
/// test below pins it to the constant so the two cannot drift apart.
const SPAM_BIT: u32 = 1 << 9;

const THREAD_COLUMNS: &str = "id, state, last_activity_at, last_from_name, last_from_addr, \
     last_subject, last_preview, message_count, unread_count, flags_union, snooze_until";

fn row_from_sql(r: &Row<'_>) -> rusqlite::Result<ThreadRow> {
    let name: String = r.get(3)?;
    let addr: String = r.get(4)?;
    // Le nom d'affichage est calculé ici plutôt qu'à chaque frame : la liste ne doit
    // faire aucun travail au défilement.
    let from_display = if name.trim().is_empty() {
        Address::new(addr).display().to_string()
    } else {
        name
    };

    Ok(ThreadRow {
        id: ThreadId(r.get(0)?),
        state: WorkflowState::from_i64(r.get(1)?).unwrap_or(WorkflowState::Todo),
        last_activity: Timestamp::from_millis(r.get(2)?),
        from_display,
        subject: r.get(5)?,
        preview: r.get(6)?,
        message_count: r.get::<_, i64>(7)? as u32,
        unread_count: r.get::<_, i64>(8)? as u32,
        flags_union: Flags(r.get::<_, i64>(9)? as u32),
        snoozed_until: r.get::<_, Option<i64>>(10)?.map(Timestamp::from_millis),
    })
}

impl Store {
    pub fn thread_row(&self, id: ThreadId) -> Result<Option<ThreadRow>> {
        self.with_conn(|c| {
            let sql = format!("SELECT {THREAD_COLUMNS} FROM threads WHERE id = ?1");
            match c.query_row(&sql, [id.get()], row_from_sql) {
                Ok(t) => Ok(Some(t)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(sql_err("lecture du fil", e)),
            }
        })
    }

    /// Une page de la liste principale.
    ///
    /// La pagination est **par curseur** : la clause de reprise porte sur
    /// `(last_activity_at, id)`, exactement l'ordre de l'index directeur. SQLite
    /// atteint donc la page par une descente d'index, sans jamais parcourir ni trier
    /// les lignes précédentes — la millionième page coûte autant que la première.
    /// Avec `OFFSET`, elle coûterait un million de lignes lues.
    pub fn list_threads(&self, q: &ListQuery) -> Result<Vec<ThreadRow>> {
        self.with_conn(|c| {
            // A spam list crosses the three states: whether the server threw a
            // message out has nothing to do with whether it was answered.
            let mut sql = match q.spam {
                crate::model::SpamFilter::Only => format!(
                    "SELECT {THREAD_COLUMNS} FROM threads WHERE (flags_union & {SPAM_BIT}) != 0"
                ),
                crate::model::SpamFilter::Exclude => format!(
                    "SELECT {THREAD_COLUMNS} FROM threads \
                     WHERE state = ? AND (flags_union & {SPAM_BIT}) = 0"
                ),
            };
            let mut args: Vec<SqlValue> = match q.spam {
                crate::model::SpamFilter::Only => Vec::new(),
                crate::model::SpamFilter::Exclude => vec![SqlValue::Integer(q.state.as_i64())],
            };

            if let Some(now) = q.hide_snoozed_until {
                sql.push_str(" AND (snooze_until IS NULL OR snooze_until <= ?)");
                args.push(SqlValue::Integer(now.millis()));
            }

            if !q.accounts.is_empty() {
                let placeholders = std::iter::repeat_n("?", q.accounts.len())
                    .collect::<Vec<_>>()
                    .join(",");
                sql.push_str(&format!(
                    " AND EXISTS (SELECT 1 FROM thread_accounts ta
                                  WHERE ta.thread_id = threads.id
                                    AND ta.account_id IN ({placeholders}))"
                ));
                args.extend(q.accounts.iter().map(|a| SqlValue::Integer(a.get())));
            }

            if let Some(cur) = q.after {
                // Comparaison de n-uplets plutôt que « a < ? OR (a = ? AND b < ?) » :
                // la forme disjonctive empêche SQLite d'exploiter l'index directeur et
                // le fait retomber sur un balayage, dont le coût croît avec la
                // profondeur de la page. Le n-uplet, lui, se traduit en une simple
                // descente d'index.
                sql.push_str(" AND (last_activity_at, id) < (?, ?)");
                args.push(SqlValue::Integer(cur.last_activity.millis()));
                args.push(SqlValue::Integer(cur.id.get()));
            }

            sql.push_str(" ORDER BY last_activity_at DESC, id DESC LIMIT ?");
            args.push(SqlValue::Integer(q.limit as i64));

            let mut stmt = c
                .prepare_cached(&sql)
                .map_err(|e| sql_err("préparation", e))?;
            let rows = stmt
                .query_map(params_from_iter(args), row_from_sql)
                .map_err(|e| sql_err("liste des fils", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("liste des fils", e))
        })
    }

    /// Nombre de fils par état, pour les compteurs des onglets.
    /// Fils à traiter, par compte.
    ///
    /// Ce que la barre latérale affiche à droite de chaque boîte. La file de travail
    /// est le seul décompte qui vaille : le nombre total de messages d'un compte ne
    /// dit rien de ce qu'il reste à faire, et un « 12 483 » permanent n'apprend rien.
    ///
    /// Les fils reportés en sont exclus : ils ont été mis de côté exprès, et les
    /// compter les remettrait sous les yeux par la petite porte.
    pub fn todo_counts_by_account(&self, now: Timestamp) -> Result<BTreeMap<AccountId, u32>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached(
                    "SELECT ta.account_id, count(*)
                     FROM thread_accounts ta
                     JOIN threads t ON t.id = ta.thread_id
                     WHERE t.state = ?1 AND (t.snooze_until IS NULL OR t.snooze_until <= ?2)
                     GROUP BY ta.account_id",
                )
                .map_err(|e| sql_err("préparation", e))?;

            let rows = stmt
                .query_map(params![WorkflowState::Todo.as_i64(), now.millis()], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                })
                .map_err(|e| sql_err("compteurs par compte", e))?;

            let mut sortie = BTreeMap::new();
            for row in rows {
                let (compte, n) = row.map_err(|e| sql_err("compteurs par compte", e))?;
                sortie.insert(AccountId(compte), n as u32);
            }
            Ok(sortie)
        })
    }

    pub fn state_counts(&self, now: Option<Timestamp>) -> Result<[u32; 3]> {
        self.with_conn(|c| {
            let mut counts = [0u32; 3];
            // The counts must agree with the lists, so spam is left out of them too.
            // A badge that counts rows the list refuses to show is a badge that lies.
            let (sql, hide) = match now {
                Some(_) => (
                    concat!(
                        "SELECT state, count(*) FROM threads
                         WHERE (flags_union & ",
                        stringify!(512),
                        ") = 0
                           AND (snooze_until IS NULL OR snooze_until <= ?) GROUP BY state"
                    ),
                    true,
                ),
                None => (
                    concat!(
                        "SELECT state, count(*) FROM threads
                         WHERE (flags_union & ",
                        stringify!(512),
                        ") = 0 GROUP BY state"
                    ),
                    false,
                ),
            };
            let mut stmt = c
                .prepare_cached(sql)
                .map_err(|e| sql_err("préparation", e))?;
            let args: Vec<SqlValue> = if hide {
                vec![SqlValue::Integer(now.unwrap().millis())]
            } else {
                vec![]
            };
            let rows = stmt
                .query_map(params_from_iter(args), |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                })
                .map_err(|e| sql_err("compteurs", e))?;
            for row in rows {
                let (state, n) = row.map_err(|e| sql_err("compteurs", e))?;
                if let Some(s) = WorkflowState::from_i64(state) {
                    counts[s.as_i64() as usize] = n as u32;
                }
            }
            Ok(counts)
        })
    }

    /// Change l'état d'un fil. Retourne l'état précédent.
    /// Moves a message into another thread, and refreshes both.
    ///
    /// Used by cross-account regrouping. Both aggregates are recomputed because both
    /// changed: the thread that lost a message may now be empty, and the one that
    /// gained it has a new last activity, a new count, and possibly a new account in
    /// its list.
    /// How many conversations the server judged unwanted.
    pub fn spam_count(&self) -> Result<u32> {
        self.with_conn(|c| {
            let n: i64 = c
                .prepare_cached(&format!(
                    "SELECT count(*) FROM threads WHERE (flags_union & {SPAM_BIT}) != 0"
                ))
                .map_err(|e| sql_err("préparation", e))?
                .query_row([], |r| r.get(0))
                .map_err(|e| sql_err("comptage du spam", e))?;
            Ok(n as u32)
        })
    }

    pub fn move_message_to_thread(&self, message: MessageId, target: ThreadId) -> Result<bool> {
        self.with_tx(|tx| {
            let previous: Option<i64> = tx
                .prepare_cached("SELECT thread_id FROM messages WHERE id = ?1")
                .map_err(|e| sql_err("préparation", e))?
                .query_row([message.get()], |r| r.get(0))
                .ok();

            let Some(previous) = previous else {
                return Ok(false);
            };
            if previous == target.get() {
                return Ok(false);
            }

            tx.prepare_cached("UPDATE messages SET thread_id = ?2 WHERE id = ?1")
                .map_err(|e| sql_err("préparation", e))?
                .execute(params![message.get(), target.get()])
                .map_err(|e| sql_err("déplacement du message", e))?;

            // The account list is what lets a unified view filter by mailbox; a
            // thread that gained a message from another account must say so.
            let account: i64 = tx
                .prepare_cached("SELECT account_id FROM messages WHERE id = ?1")
                .map_err(|e| sql_err("préparation", e))?
                .query_row([message.get()], |r| r.get(0))
                .map_err(|e| sql_err("lecture du compte", e))?;

            tx.prepare_cached(
                "INSERT OR IGNORE INTO thread_accounts (thread_id, account_id) VALUES (?1, ?2)",
            )
            .map_err(|e| sql_err("préparation", e))?
            .execute(params![target.get(), account])
            .map_err(|e| sql_err("rattachement du compte", e))?;

            crate::messages::refresh_thread(tx, ThreadId(previous))?;
            crate::messages::refresh_thread(tx, target)?;
            Ok(true)
        })
    }

    /// Removes threads that no longer hold any message.
    ///
    /// A thread emptied by regrouping would otherwise show as a blank row.
    pub fn prune_empty_threads(&self) -> Result<usize> {
        self.with_conn(|c| {
            let removed = c
                .prepare_cached(
                    "DELETE FROM threads
                     WHERE id NOT IN (SELECT DISTINCT thread_id FROM messages)",
                )
                .map_err(|e| sql_err("préparation", e))?
                .execute([])
                .map_err(|e| sql_err("purge des fils vides", e))?;
            Ok(removed)
        })
    }

    pub fn set_thread_state(
        &self,
        thread: ThreadId,
        state: WorkflowState,
    ) -> Result<Option<WorkflowState>> {
        self.with_conn(|c| {
            let previous: Option<i64> = c
                .query_row(
                    "SELECT state FROM threads WHERE id = ?1",
                    [thread.get()],
                    |r| r.get(0),
                )
                .ok();
            let Some(previous) = previous else {
                return Ok(None);
            };

            c.execute(
                "UPDATE threads SET state = ?1 WHERE id = ?2",
                rusqlite::params![state.as_i64(), thread.get()],
            )
            .map_err(|e| sql_err("changement d'état", e))?;

            Ok(WorkflowState::from_i64(previous))
        })
    }

    /// Reporte un fil. L'état est conservé et restauré à l'échéance.
    pub fn snooze_thread(&self, thread: ThreadId, snooze: Snooze) -> Result<bool> {
        self.with_conn(|c| {
            let n = c
                .execute(
                    "UPDATE threads SET snooze_until = ?1, snooze_restore = ?2 WHERE id = ?3",
                    rusqlite::params![
                        snooze.until.millis(),
                        snooze.restore_to.as_i64(),
                        thread.get()
                    ],
                )
                .map_err(|e| sql_err("report du fil", e))?;
            Ok(n > 0)
        })
    }

    pub fn clear_snooze(&self, thread: ThreadId) -> Result<bool> {
        self.with_conn(|c| {
            let n = c
                .execute(
                    "UPDATE threads SET snooze_until = NULL, snooze_restore = NULL WHERE id = ?1",
                    [thread.get()],
                )
                .map_err(|e| sql_err("annulation du report", e))?;
            Ok(n > 0)
        })
    }

    /// Les fils dont le report est arrivé à échéance, avec l'état à restaurer.
    pub fn due_snoozes(&self, now: Timestamp) -> Result<Vec<(ThreadId, WorkflowState)>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached(
                    "SELECT id, snooze_restore FROM threads
                     WHERE snooze_until IS NOT NULL AND snooze_until <= ?1
                     ORDER BY snooze_until",
                )
                .map_err(|e| sql_err("préparation", e))?;
            let rows = stmt
                .query_map([now.millis()], |r| {
                    Ok((
                        ThreadId(r.get::<_, i64>(0)?),
                        WorkflowState::from_i64(r.get::<_, Option<i64>>(1)?.unwrap_or(0))
                            .unwrap_or(WorkflowState::Todo),
                    ))
                })
                .map_err(|e| sql_err("reports échus", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("reports échus", e))
        })
    }

    /// Fils en attente depuis plus longtemps que le délai de relance.
    pub fn threads_needing_follow_up(
        &self,
        now: Timestamp,
        after_days: u16,
        limit: u32,
    ) -> Result<Vec<ThreadId>> {
        let cutoff = now.millis() - (after_days as i64) * 86_400_000;
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached(
                    "SELECT id FROM threads
                     WHERE state = ?1 AND last_activity_at <= ?2 AND snooze_until IS NULL
                     ORDER BY last_activity_at LIMIT ?3",
                )
                .map_err(|e| sql_err("préparation", e))?;
            let rows = stmt
                .query_map(
                    rusqlite::params![WorkflowState::Waiting.as_i64(), cutoff, limit as i64],
                    |r| r.get::<_, i64>(0).map(ThreadId),
                )
                .map_err(|e| sql_err("relances", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("relances", e))
        })
    }

    /// Comptes participant à un fil.
    pub fn thread_accounts(&self, thread: ThreadId) -> Result<Vec<AccountId>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached(
                    "SELECT account_id FROM thread_accounts WHERE thread_id = ?1 ORDER BY account_id",
                )
                .map_err(|e| sql_err("préparation", e))?;
            let rows = stmt
                .query_map([thread.get()], |r| r.get::<_, i64>(0).map(AccountId))
                .map_err(|e| sql_err("comptes du fil", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| sql_err("comptes du fil", e))
        })
    }

    /// Parcourt une liste page après page, en suivant le curseur.
    ///
    /// Utilitaire de test et de traitement par lots ; l'interface, elle, ne charge
    /// jamais qu'une fenêtre.
    pub fn list_all_threads(&self, mut q: ListQuery) -> Result<Vec<ThreadRow>> {
        let mut all = Vec::new();
        loop {
            let page = self.list_threads(&q)?;
            let done = page.len() < q.limit as usize;
            if let Some(last) = page.last() {
                q = q.clone().after(last.cursor());
            }
            all.extend(page);
            if done {
                return Ok(all);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FolderRole, NewAccount, NewMessage};
    use iris_types::FolderId;

    struct Fixture {
        store: Store,
        account: AccountId,
        folder: FolderId,
        uid: std::cell::Cell<u32>,
    }

    fn fixture() -> Fixture {
        let store = Store::in_memory().unwrap();
        let account = store
            .create_account(
                &NewAccount::new("a@x.fr", "i", "s"),
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
            uid: std::cell::Cell::new(1),
        }
    }

    impl Fixture {
        /// Crée un fil distinct avec une activité donnée.
        fn thread_at(&self, millis: i64, from: &str) -> ThreadId {
            let uid = self.uid.get();
            self.uid.set(uid + 1);
            self.store
                .insert_message(&NewMessage {
                    account: self.account,
                    folder: self.folder,
                    uid,
                    rfc_message_id: Some(format!("m{uid}@x")),
                    in_reply_to: None,
                    references: vec![],
                    subject: format!("Sujet {uid}"),
                    from_name: from.to_string(),
                    from_addr: "exp@example.com".into(),
                    recipients_json: "[]".into(),
                    date: Timestamp::from_millis(millis),
                    received: Timestamp::from_millis(millis),
                    size: 10,
                    flags: Flags::NONE,
                    preview: "aperçu".into(),
                })
                .unwrap()
                .thread
        }
    }

    #[test]
    fn la_liste_est_triee_du_plus_recent_au_plus_ancien() {
        let f = fixture();
        f.thread_at(1000, "A");
        f.thread_at(3000, "C");
        f.thread_at(2000, "B");

        let page = f
            .store
            .list_threads(&ListQuery::new(WorkflowState::Todo, 10))
            .unwrap();
        let noms: Vec<_> = page.iter().map(|r| r.from_display.as_str()).collect();
        assert_eq!(noms, ["C", "B", "A"]);
    }

    #[test]
    fn la_pagination_par_curseur_ne_saute_ni_ne_repete_aucune_ligne() {
        let f = fixture();
        for i in 0..25 {
            f.thread_at(1000 + i, &format!("n{i}"));
        }

        let mut vus = Vec::new();
        let mut q = ListQuery::new(WorkflowState::Todo, 7);
        loop {
            let page = f.store.list_threads(&q).unwrap();
            if page.is_empty() {
                break;
            }
            q = q.after(page.last().unwrap().cursor());
            vus.extend(page.iter().map(|r| r.id));
        }

        assert_eq!(vus.len(), 25);
        let uniques: std::collections::BTreeSet<_> = vus.iter().collect();
        assert_eq!(uniques.len(), 25, "aucune ligne répétée");
    }

    #[test]
    fn deux_fils_de_meme_horodatage_restent_departages() {
        // Sans l'identifiant dans le curseur, une page pourrait boucler indéfiniment
        // ou sauter des lignes à horodatage identique.
        let f = fixture();
        for i in 0..6 {
            f.thread_at(5000, &format!("n{i}"));
        }
        let tous = f
            .store
            .list_all_threads(ListQuery::new(WorkflowState::Todo, 2))
            .unwrap();
        assert_eq!(tous.len(), 6);
    }

    #[test]
    fn le_filtre_par_compte_restreint_la_liste() {
        let f = fixture();
        let autre = f
            .store
            .create_account(
                &NewAccount::new("b@x.fr", "i", "s"),
                Timestamp::from_millis(0),
            )
            .unwrap();
        let autre_dossier = f
            .store
            .upsert_folder(autre, "INBOX", FolderRole::Inbox)
            .unwrap();

        f.thread_at(1000, "compte-a");
        f.store
            .insert_message(&NewMessage {
                account: autre,
                folder: autre_dossier,
                uid: 1,
                rfc_message_id: Some("z@x".into()),
                in_reply_to: None,
                references: vec![],
                subject: "Autre".into(),
                from_name: "compte-b".into(),
                from_addr: "z@example.com".into(),
                recipients_json: "[]".into(),
                date: Timestamp::from_millis(2000),
                received: Timestamp::from_millis(2000),
                size: 1,
                flags: Flags::NONE,
                preview: String::new(),
            })
            .unwrap();

        let tous = f
            .store
            .list_threads(&ListQuery::new(WorkflowState::Todo, 10))
            .unwrap();
        assert_eq!(tous.len(), 2);

        let filtre = f
            .store
            .list_threads(&ListQuery::new(WorkflowState::Todo, 10).for_accounts(vec![autre]))
            .unwrap();
        assert_eq!(filtre.len(), 1);
        assert_eq!(filtre[0].from_display, "compte-b");
    }

    #[test]
    fn un_fil_reporte_disparait_de_la_liste_puis_revient() {
        let f = fixture();
        let t = f.thread_at(1000, "Marie");
        f.store
            .snooze_thread(
                t,
                Snooze {
                    until: Timestamp::from_millis(5000),
                    restore_to: WorkflowState::Todo,
                },
            )
            .unwrap();

        let avant =
            ListQuery::new(WorkflowState::Todo, 10).hiding_snoozed(Timestamp::from_millis(4000));
        assert!(f.store.list_threads(&avant).unwrap().is_empty());

        let apres =
            ListQuery::new(WorkflowState::Todo, 10).hiding_snoozed(Timestamp::from_millis(6000));
        assert_eq!(f.store.list_threads(&apres).unwrap().len(), 1);
    }

    #[test]
    fn les_reports_echus_sont_recenses_avec_leur_etat_a_restaurer() {
        let f = fixture();
        let t = f.thread_at(1000, "Marie");
        f.store
            .snooze_thread(
                t,
                Snooze {
                    until: Timestamp::from_millis(5000),
                    restore_to: WorkflowState::Waiting,
                },
            )
            .unwrap();

        assert!(f
            .store
            .due_snoozes(Timestamp::from_millis(4999))
            .unwrap()
            .is_empty());
        let echus = f.store.due_snoozes(Timestamp::from_millis(5000)).unwrap();
        assert_eq!(echus, [(t, WorkflowState::Waiting)]);

        f.store.clear_snooze(t).unwrap();
        assert!(f
            .store
            .due_snoozes(Timestamp::from_millis(9999))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn les_compteurs_suivent_les_changements_d_etat() {
        let f = fixture();
        let a = f.thread_at(1000, "A");
        f.thread_at(2000, "B");

        assert_eq!(f.store.state_counts(None).unwrap(), [2, 0, 0]);
        assert_eq!(
            f.store.set_thread_state(a, WorkflowState::Done).unwrap(),
            Some(WorkflowState::Todo)
        );
        assert_eq!(f.store.state_counts(None).unwrap(), [1, 0, 1]);
    }

    #[test]
    fn les_compteurs_peuvent_exclure_les_fils_reportes() {
        let f = fixture();
        let a = f.thread_at(1000, "A");
        f.thread_at(2000, "B");
        f.store
            .snooze_thread(
                a,
                Snooze {
                    until: Timestamp::from_millis(9000),
                    restore_to: WorkflowState::Todo,
                },
            )
            .unwrap();

        assert_eq!(f.store.state_counts(None).unwrap()[0], 2);
        assert_eq!(
            f.store
                .state_counts(Some(Timestamp::from_millis(1)))
                .unwrap()[0],
            1
        );
    }

    #[test]
    fn changer_l_etat_d_un_fil_inexistant_ne_fait_rien() {
        let f = fixture();
        assert!(f
            .store
            .set_thread_state(ThreadId(999), WorkflowState::Done)
            .unwrap()
            .is_none());
        assert!(!f
            .store
            .snooze_thread(
                ThreadId(999),
                Snooze {
                    until: Timestamp::EPOCH,
                    restore_to: WorkflowState::Todo
                }
            )
            .unwrap());
    }

    #[test]
    fn la_relance_ne_ramasse_que_les_fils_en_attente_assez_anciens() {
        let f = fixture();
        let vieux = f.thread_at(1_000_000, "vieux");
        let recent = f.thread_at(500_000_000, "récent");
        for t in [vieux, recent] {
            f.store.set_thread_state(t, WorkflowState::Waiting).unwrap();
        }
        let a_traiter = f.thread_at(1_000, "à traiter");

        let now = Timestamp::from_millis(500_000_000);
        let dus = f.store.threads_needing_follow_up(now, 3, 10).unwrap();
        assert_eq!(dus, [vieux]);
        assert!(
            !dus.contains(&a_traiter),
            "un fil à traiter n'est pas relancé"
        );
    }

    #[test]
    fn un_fil_reporte_n_est_pas_relance() {
        let f = fixture();
        let t = f.thread_at(1_000, "vieux");
        f.store.set_thread_state(t, WorkflowState::Waiting).unwrap();
        f.store
            .snooze_thread(
                t,
                Snooze {
                    until: Timestamp::from_millis(999_000_000),
                    restore_to: WorkflowState::Waiting,
                },
            )
            .unwrap();

        let dus = f
            .store
            .threads_needing_follow_up(Timestamp::from_millis(500_000_000), 3, 10)
            .unwrap();
        assert!(dus.is_empty(), "le report l'emporte sur la relance");
    }

    #[test]
    fn le_nom_affiche_retombe_sur_l_adresse() {
        let f = fixture();
        let t = f.thread_at(1000, "");
        let row = f.store.thread_row(t).unwrap().unwrap();
        assert_eq!(row.from_display, "exp");
    }

    #[test]
    fn les_comptes_participants_sont_enregistres() {
        let f = fixture();
        let t = f.thread_at(1000, "A");
        assert_eq!(f.store.thread_accounts(t).unwrap(), [f.account]);
    }

    #[test]
    fn les_fils_a_traiter_sont_comptes_par_compte() {
        // C'est ce que la barre latérale affiche à droite de chaque boîte.
        let f = fixture();
        let second = f
            .store
            .create_account(&NewAccount::new("b@x.fr", "i", "s"), Timestamp::EPOCH)
            .unwrap();
        let dossier_b = f
            .store
            .upsert_folder(second, "INBOX", FolderRole::Inbox)
            .unwrap();

        f.thread_at(1000, "Marie");
        f.thread_at(2000, "Luc");
        f.store
            .insert_message(&NewMessage {
                account: second,
                folder: dossier_b,
                uid: 900,
                rfc_message_id: Some("b1@x".into()),
                in_reply_to: None,
                references: vec![],
                subject: "Chez b".into(),
                from_name: "Luc".into(),
                from_addr: "luc@x.fr".into(),
                recipients_json: "[]".into(),
                date: Timestamp::from_millis(1),
                received: Timestamp::from_millis(1),
                size: 10,
                flags: Flags::NONE,
                preview: String::new(),
            })
            .unwrap();

        let comptes = f
            .store
            .todo_counts_by_account(Timestamp::from_millis(10_000))
            .unwrap();
        assert_eq!(comptes.get(&f.account), Some(&2));
        assert_eq!(comptes.get(&second), Some(&1));
    }

    #[test]
    fn un_fil_traite_ne_compte_plus() {
        // Le total des messages ne dirait rien de ce qu'il reste à faire.
        let f = fixture();
        let fil = f.thread_at(1000, "Marie");
        f.store.set_thread_state(fil, WorkflowState::Done).unwrap();

        let comptes = f
            .store
            .todo_counts_by_account(Timestamp::from_millis(10_000))
            .unwrap();
        assert_eq!(comptes.get(&f.account), None);
    }

    #[test]
    fn un_fil_reporte_ne_compte_pas_avant_son_echeance() {
        // Il a été mis de côté exprès : le compter le remettrait sous les yeux par
        // la petite porte.
        let f = fixture();
        let fil = f.thread_at(1000, "Marie");
        f.store
            .snooze_thread(
                fil,
                Snooze {
                    until: Timestamp::from_millis(50_000),
                    restore_to: WorkflowState::Todo,
                },
            )
            .unwrap();

        let avant = f
            .store
            .todo_counts_by_account(Timestamp::from_millis(10_000))
            .unwrap();
        assert_eq!(avant.get(&f.account), None);

        let apres = f
            .store
            .todo_counts_by_account(Timestamp::from_millis(60_000))
            .unwrap();
        assert_eq!(apres.get(&f.account), Some(&1));
    }

    #[test]
    fn un_compte_sans_fil_n_apparait_pas() {
        let f = fixture();
        let comptes = f
            .store
            .todo_counts_by_account(Timestamp::from_millis(10_000))
            .unwrap();
        assert!(comptes.is_empty());
    }
}
