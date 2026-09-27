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
     last_subject, last_preview, message_count, unread_count, flags_union, snooze_until, \
     last_account_id";

/// Ajoute à la requête ce que la portée demandée impose.
///
/// Partagé par la liste et par les compteurs, et c'est tout l'intérêt : deux clauses
/// écrites séparément finissent par diverger, et l'onglet annonce alors un nombre que
/// la liste en dessous ne montre pas. C'est exactement ce qui s'était produit entre la
/// barre latérale — qui comptait les indésirables — et les onglets, qui ne les
/// comptaient pas : 191 d'un côté, 188 de l'autre, sur le même écran.
fn push_scope(sql: &mut String, args: &mut Vec<SqlValue>, q: &ListQuery) {
    use crate::model::Scope;

    // Le compte. Croisé avec le dossier quand les deux sont posés : le même `EXISTS`
    // porte les deux conditions, donc c'est bien « ce compte-là dans ce dossier-là » et
    // non « ce compte quelque part, ce dossier ailleurs ».
    let compte_dans = |args: &mut Vec<SqlValue>| -> String {
        if q.accounts.is_empty() {
            return String::new();
        }
        let places = std::iter::repeat_n("?", q.accounts.len())
            .collect::<Vec<_>>()
            .join(",");
        args.extend(q.accounts.iter().map(|a| SqlValue::Integer(a.get())));
        format!(" AND m.account_id IN ({places})")
    };

    match &q.scope {
        Scope::Queue => {
            // Une file de travail ignore ce qui est mis de côté. Un fil compte tant
            // qu'il lui reste **un** message ailleurs que dans une corbeille ou des
            // indésirables : un échange dont un message a été jeté et dont la réponse
            // est dans la boîte de réception est du travail vivant.
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM messages m
                              JOIN folders f ON f.id = m.folder_id
                              WHERE m.thread_id = threads.id
                                AND f.role NOT IN ('trash', 'junk'))",
            );
            // Et ce que le serveur a jugé indésirable, où qu'il l'ait rangé. Onze fils
            // sur cette boîte portent le verdict sans être dans le dossier : le
            // verdict voyage avec le message, pas avec l'endroit.
            sql.push_str(&format!(" AND (flags_union & {SPAM_BIT}) = 0"));

            if !q.accounts.is_empty() {
                let places = std::iter::repeat_n("?", q.accounts.len())
                    .collect::<Vec<_>>()
                    .join(",");
                sql.push_str(&format!(
                    " AND EXISTS (SELECT 1 FROM thread_accounts ta
                                  WHERE ta.thread_id = threads.id
                                    AND ta.account_id IN ({places}))"
                ));
                args.extend(q.accounts.iter().map(|a| SqlValue::Integer(a.get())));
            }
        }

        Scope::Role(role) => {
            let filtre_compte = compte_dans(args);
            // Les indésirables sont le seul rôle qui déborde de son dossier : le
            // verdict du serveur est écrit dans le message, et un message tagué mais
            // laissé en boîte de réception appartient quand même à ce dossier-là.
            let aussi_marques = if *role == crate::model::FolderRole::Junk {
                format!(" OR (threads.flags_union & {SPAM_BIT}) != 0")
            } else {
                String::new()
            };

            // Le rôle vient d'une énumération fermée, jamais d'une saisie : il est
            // interpolé sans risque, et le paramétrer obligerait à réordonner les
            // arguments autour du filtre de comptes.
            sql.push_str(&format!(
                " AND (EXISTS (SELECT 1 FROM messages m
                               JOIN folders f ON f.id = m.folder_id
                               WHERE m.thread_id = threads.id
                                 AND f.role = '{}'{filtre_compte}){aussi_marques})",
                role.as_str()
            ));
        }

        Scope::Path(chemin) => {
            let filtre_compte = compte_dans(args);
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM messages m
                              JOIN folders f ON f.id = m.folder_id
                              WHERE m.thread_id = threads.id
                                AND f.path = ?{filtre_compte})"
            ));
            // Le chemin est un paramètre lié, lui, parce qu'il vient de l'utilisateur.
            // Il est inséré avant les identifiants de comptes ajoutés ci-dessus.
            let position = args.len() - q.accounts.len();
            args.insert(position, SqlValue::Text(chemin.clone()));
        }
    }

    // Les filtres rapides, dans la même clause partagée que la portée — et pour la même
    // raison. Un filtre appliqué à la liste mais pas aux compteurs ferait annoncer « 42 »
    // au-dessus de sept lignes, ce qui est exactement la divergence que cette fonction
    // existe pour empêcher.
    //
    // Les drapeaux sont dénormalisés sur le fil : `unread_count` et l'union des drapeaux
    // de ses messages. Un fil compte donc comme portant une pièce jointe dès qu'un de
    // ses messages en porte une, ce qui est ce qu'on cherche — on filtre des
    // conversations, pas des messages.
    if q.filters.unread {
        sql.push_str(" AND unread_count > 0");
    }
    if q.filters.attachments {
        sql.push_str(&format!(
            " AND (flags_union & {}) != 0",
            Flags::HAS_ATTACHMENT.0
        ));
    }
    if q.filters.starred {
        sql.push_str(&format!(" AND (flags_union & {}) != 0", Flags::FLAGGED.0));
    }
}

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
        account: AccountId(r.get(11)?),
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
            let mut sql = format!("SELECT {THREAD_COLUMNS} FROM threads WHERE state = ?");
            let mut args: Vec<SqlValue> = vec![SqlValue::Integer(q.state.as_i64())];

            if let Some(now) = q.hide_snoozed_until {
                sql.push_str(" AND (snooze_until IS NULL OR snooze_until <= ?)");
                args.push(SqlValue::Integer(now.millis()));
            }

            push_scope(&mut sql, &mut args, q);

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

    /// Le nombre de messages non lus, hors indésirables et hors corbeille.
    ///
    /// C'est le chiffre de la zone de notification, et le seul qu'on y cherche. Les
    /// indésirables en sont exclus : une icône qui annonce « 162 non lus » alors que
    /// ce sont 162 courriels que le serveur a écartés apprend à ne plus la regarder,
    /// et une pastille qu'on n'a plus envie de regarder ne sert plus à rien.
    pub fn unread_count(&self) -> Result<u32> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT count(*) FROM messages m
                 JOIN folders f ON f.id = m.folder_id
                 WHERE (m.flags & ?1) = 0
                   AND (m.flags & ?2) = 0
                   AND f.role NOT IN ('trash', 'junk')",
                params![
                    iris_types::Flags::SEEN.0 as i64,
                    iris_types::Flags::SPAM.0 as i64
                ],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as u32)
            .map_err(|e| sql_err("comptage des non-lus", e))
        })
    }

    /// Qui a écrit en dernier, et à quel sujet.
    ///
    /// Ce que dit une notification d'arrivée. Le plus récent des non-lus, hors
    /// indésirables et hors corbeille — prévenir de l'arrivée de ce qu'on a filtré
    /// annulerait le filtre — et hors messages envoyés, qui reviennent du serveur
    /// après un envoi et ne sont pas une arrivée.
    ///
    /// `now` borne la recherche à la dernière heure : au premier démarrage sur une
    /// boîte de dix ans, le plus ancien non-lu n'est pas une nouvelle.
    pub fn latest_unread(&self, now: Timestamp) -> Result<Option<(String, String)>> {
        const UNE_HEURE: i64 = 3_600_000;
        let depuis = now.millis() - UNE_HEURE;

        self.with_conn(|c| {
            let resultat = c.query_row(
                "SELECT m.from_name, m.from_addr, m.subject
                 FROM messages m
                 JOIN folders f ON f.id = m.folder_id
                 WHERE (m.flags & ?1) = 0
                   AND (m.flags & ?2) = 0
                   AND f.role NOT IN ('trash', 'junk', 'sent', 'drafts')
                   AND m.received >= ?3
                 ORDER BY m.received DESC
                 LIMIT 1",
                params![
                    iris_types::Flags::SEEN.0 as i64,
                    iris_types::Flags::SPAM.0 as i64,
                    depuis
                ],
                |r| {
                    let nom: String = r.get(0)?;
                    let adresse: String = r.get(1)?;
                    let sujet: String = r.get(2)?;
                    // Le nom quand il y en a un, l'adresse sinon : une bulle qui
                    // annonce « (sans nom) » n'apprend rien.
                    Ok((if nom.trim().is_empty() { adresse } else { nom }, sujet))
                },
            );

            match resultat {
                Ok(v) => Ok(Some(v)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(sql_err("dernier message non lu", e)),
            }
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
            // Les mêmes exclusions que l'onglet « À faire », parce que c'est le même
            // nombre. La barre latérale annonçait 191 pendant que l'onglet annonçait
            // 188 : elle comptait les indésirables et la corbeille, lui non.
            //
            // Et les non lus comme lui, pour la même raison : les deux répondent à
            // « qu'est-ce qui m'attend », l'un par compte et l'autre par file. Changer
            // l'un sans l'autre remettrait deux nombres différents sur le même écran,
            // ce qui est précisément la faute que le paragraphe ci-dessus décrit.
            let mut stmt = c
                .prepare_cached(&format!(
                    "SELECT ta.account_id, count(*)
                     FROM thread_accounts ta
                     JOIN threads t ON t.id = ta.thread_id
                     WHERE t.state = ?1
                       AND t.unread_count > 0
                       AND (t.snooze_until IS NULL OR t.snooze_until <= ?2)
                       AND (t.flags_union & {SPAM_BIT}) = 0
                       AND EXISTS (SELECT 1 FROM messages m
                                   JOIN folders f ON f.id = m.folder_id
                                   WHERE m.thread_id = t.id
                                     AND f.role NOT IN ('trash', 'junk'))
                     GROUP BY ta.account_id"
                ))
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

    /// Les trois compteurs d'onglets, éventuellement restreints à des comptes.
    ///
    /// Ils passent par **la même** clause de portée que la liste. Un compteur écrit
    /// séparément finit par annoncer un nombre que la liste en dessous ne montre pas,
    /// et une pastille qui compte des lignes que la liste refuse est une pastille qui
    /// ment. C'est arrivé : 191 dans la barre latérale, 188 dans l'onglet, sur le même
    /// écran, parce que l'une comptait les indésirables et l'autre non.
    pub fn state_counts(
        &self,
        accounts: &[AccountId],
        now: Option<Timestamp>,
        filters: crate::model::Filters,
    ) -> Result<[u32; 3]> {
        self.with_conn(|c| {
            let mut counts = [0u32; 3];
            // Les **non lus**, pas le total.
            //
            // Le nombre à côté d'un onglet répond à « qu'est-ce qui m'attend », et un
            // total ne répond pas à cette question : « Done 865 » compte du courrier
            // dont on s'est occupé, et « To do 188 » compte des fils qu'on a déjà lus
            // et laissés là. Un compteur qui ne bouge pas quand on travaille cesse
            // d'être lu, et c'est le prochain qui compte qu'on ne verra pas.
            //
            // Un fil est non lu s'il contient au moins un message non lu, ce que
            // `unread_count` porte déjà, dénormalisé. Le zéro est donc dit par
            // l'absence de ligne, comme avant : rien à faire de plus.
            let mut sql =
                String::from("SELECT state, count(*) FROM threads WHERE unread_count > 0");
            let mut args: Vec<SqlValue> = Vec::new();

            if let Some(now) = now {
                sql.push_str(" AND (snooze_until IS NULL OR snooze_until <= ?)");
                args.push(SqlValue::Integer(now.millis()));
            }

            // Les filtres rapides voyagent avec la portée, sans quoi les onglets
            // annonceraient un nombre que la liste filtrée en dessous ne montre pas —
            // la divergence même que cette fonction partagée existe pour empêcher.
            let portee = ListQuery {
                accounts: accounts.to_vec(),
                filters,
                ..ListQuery::new(WorkflowState::Todo, 0)
            };
            push_scope(&mut sql, &mut args, &portee);
            sql.push_str(" GROUP BY state");

            let mut stmt = c
                .prepare_cached(&sql)
                .map_err(|e| sql_err("préparation", e))?;
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
    /// Combien de conversations le serveur a jugées indésirables.
    ///
    /// Le dossier « Spam » de l'arborescence, et plus un onglet : le courrier mis de
    /// côté n'est pas une étape du travail, c'est un endroit. Le décompte croise le
    /// rôle du dossier et le verdict porté par le message, parce que le serveur ne
    /// range pas toujours ce qu'il a marqué.
    pub fn spam_count(&self) -> Result<u32> {
        self.with_conn(|c| {
            let n: i64 = c
                .prepare_cached(&format!(
                    "SELECT count(*) FROM threads t
                     WHERE (t.flags_union & {SPAM_BIT}) != 0
                        OR EXISTS (SELECT 1 FROM messages m
                                   JOIN folders f ON f.id = m.folder_id
                                   WHERE m.thread_id = t.id AND f.role = 'junk')"
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
    use crate::model::Filters;
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
    fn un_filtre_reduit_la_liste_et_les_compteurs_ensemble() {
        // C'est toute la raison pour laquelle ils partagent une clause. Un filtre
        // appliqué à la liste mais pas aux compteurs ferait annoncer « 3 » au-dessus
        // d'une seule ligne — la divergence que `push_scope` existe pour empêcher, et
        // qui s'est déjà produite une fois entre la barre latérale et les onglets.
        let f = fixture();
        f.thread_at(1000, "A");
        f.thread_at(2000, "B");
        let lu = f.thread_at(3000, "C");

        // Un des trois est lu.
        f.store
            .apply_flag_changes(f.folder, &[(3, Flags::SEEN)])
            .unwrap();
        let _ = lu;

        let non_lus = Filters {
            unread: true,
            ..Default::default()
        };
        let page = f
            .store
            .list_threads(&ListQuery::new(WorkflowState::Todo, 10).filtered(non_lus))
            .unwrap();
        assert_eq!(page.len(), 2);

        let compteurs = f.store.state_counts(&[], None, non_lus).unwrap();
        assert_eq!(
            compteurs[0] as usize,
            page.len(),
            "le compteur et la liste disent le même nombre"
        );
    }

    #[test]
    fn les_filtres_se_cumulent_par_un_et() {
        // « Non lus avec une pièce jointe » est la question qu'on se pose ; jamais
        // « non lus ou avec une pièce jointe ».
        let f = fixture();
        f.thread_at(1000, "Ni l'un ni l'autre");
        f.thread_at(2000, "Avec pièce jointe");
        f.store
            .apply_flag_changes(f.folder, &[(2, Flags::HAS_ATTACHMENT)])
            .unwrap();

        let deux = Filters {
            unread: true,
            attachments: true,
            starred: false,
        };
        let page = f
            .store
            .list_threads(&ListQuery::new(WorkflowState::Todo, 10).filtered(deux))
            .unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].from_display, "Avec pièce jointe");
    }

    #[test]
    fn sans_filtre_rien_n_est_retire() {
        let f = fixture();
        f.thread_at(1000, "A");
        f.thread_at(2000, "B");

        assert!(Filters::default().is_empty());
        let page = f
            .store
            .list_threads(&ListQuery::new(WorkflowState::Todo, 10))
            .unwrap();
        assert_eq!(page.len(), 2);
    }

    #[test]
    fn les_compteurs_d_onglet_comptent_les_non_lus() {
        // Un nombre à côté d'un onglet répond à « qu'est-ce qui m'attend ». Le total ne
        // répond pas à cette question : « Done 865 » compte du courrier dont on s'est
        // occupé, et un compteur qui ne bouge pas quand on travaille cesse d'être lu.
        let f = fixture();
        let lu = f.thread_at(1000, "Lu");
        f.thread_at(2000, "Non lu");

        assert_eq!(
            f.store.state_counts(&[], None, Filters::default()).unwrap()[0],
            2,
            "les deux fils arrivent non lus"
        );

        // Le même fil, lu.
        f.store
            .apply_flag_changes(f.folder, &[(1, Flags::SEEN)])
            .unwrap();
        assert_eq!(
            f.store.state_counts(&[], None, Filters::default()).unwrap()[0],
            1
        );

        // Et la barre latérale dit le même nombre, sans quoi deux compteurs
        // contradictoires se retrouvent sur le même écran.
        let par_compte = f
            .store
            .todo_counts_by_account(Timestamp::from_millis(9999))
            .unwrap();
        assert_eq!(par_compte.get(&f.account).copied(), Some(1));
        let _ = lu;
    }

    #[test]
    fn chaque_ligne_porte_son_propre_compte() {
        // La pastille de couleur de la liste vient de là. L'interface la calculait à
        // partir de l'adresse du **premier** compte pour toutes les lignes : cent
        // boîtes, une seule couleur, et un repère qui affirmait quelque chose de faux
        // au lieu de ne rien dire. Le fil doit donc porter son compte, et ce test est
        // ce qui l'y oblige.
        let f = fixture();
        let autre = f
            .store
            .create_account(
                &NewAccount::new("b@y.fr", "i", "s"),
                Timestamp::from_millis(0),
            )
            .unwrap();
        let autre_dossier = f
            .store
            .upsert_folder(autre, "INBOX", FolderRole::Inbox)
            .unwrap();

        f.thread_at(1000, "Depuis A");
        f.store
            .insert_message(&NewMessage {
                account: autre,
                folder: autre_dossier,
                uid: 900,
                rfc_message_id: Some("autre@x".into()),
                in_reply_to: None,
                references: vec![],
                subject: "Depuis B".into(),
                from_name: "Depuis B".into(),
                from_addr: "exp@example.com".into(),
                recipients_json: "[]".into(),
                date: Timestamp::from_millis(2000),
                received: Timestamp::from_millis(2000),
                size: 10,
                flags: Flags::NONE,
                preview: "aperçu".into(),
            })
            .unwrap();

        let page = f
            .store
            .list_threads(&ListQuery::new(WorkflowState::Todo, 10))
            .unwrap();
        let comptes: Vec<_> = page
            .iter()
            .map(|r| (r.from_display.as_str(), r.account))
            .collect();
        assert_eq!(comptes, [("Depuis B", autre), ("Depuis A", f.account)]);
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

        assert_eq!(
            f.store.state_counts(&[], None, Filters::default()).unwrap(),
            [2, 0, 0]
        );
        assert_eq!(
            f.store.set_thread_state(a, WorkflowState::Done).unwrap(),
            Some(WorkflowState::Todo)
        );
        assert_eq!(
            f.store.state_counts(&[], None, Filters::default()).unwrap(),
            [1, 0, 1]
        );
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

        assert_eq!(
            f.store.state_counts(&[], None, Filters::default()).unwrap()[0],
            2
        );
        assert_eq!(
            f.store
                .state_counts(&[], Some(Timestamp::from_millis(1)), Filters::default())
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
