//! What changed here, and the merge of what changed on another device.
//!
//! A row travels with its `sync_id`, its clock and its fields; its foreign keys as the
//! parent's `sync_id`, never a local id. The merge takes the newer of two versions of
//! a row, keeps a row deleted on one side only if it was not changed after on the
//! other, and never removes an account: it tells the app, which asks.

use crate::sync::{synced, SyncedTable, SYNCED};
use crate::{sql_err, Store};
use iris_types::{Error, Result};
use rusqlite::types::Value as Sql;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};

/// Everything changed on one device since a clock.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChangeSet {
    pub from_device: String,
    /// The greatest clock in the set: the other side merges from there next time.
    pub up_to: String,
    pub rows: Vec<RowChange>,
    pub tombstones: Vec<SyncTombstone>,
    /// Shared settings: key, JSON value, clock.
    pub settings: Vec<(String, String, String)>,
}

/// One row as it travels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RowChange {
    pub table: String,
    pub sync_id: String,
    pub sync_at: String,
    pub fields: Map<String, Json>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncTombstone {
    pub table: String,
    pub sync_id: String,
    pub sync_at: String,
}

/// What a merge did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeReport {
    pub inserted: usize,
    pub updated: usize,
    pub deleted: usize,
    /// Changed on both devices since they last met: the newer was kept.
    pub conflicts: Vec<Conflict>,
    /// Accounts removed on the other device, by address. Never removed here by a
    /// merge: the app asks.
    pub accounts_removed_there: Vec<String>,
    /// Shared settings newer there: key and JSON value, for the app to apply.
    pub settings: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub table: String,
    pub sync_id: String,
    /// What the person knows it by: a task's title, a list's name.
    pub label: String,
}

/// A table's travelling columns, in order: no local id, no clock, nothing local.
fn columns(c: &Connection, table: &SyncedTable) -> Result<Vec<String>> {
    let mut s = c
        .prepare(&format!("PRAGMA table_info({})", table.name))
        .map_err(|e| sql_err("columns", e))?;
    let lignes = s
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(5)?,
            ))
        })
        .map_err(|e| sql_err("columns", e))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| sql_err("columns", e))?;
    let garde_id = table.natural_key.is_some_and(|k| k.contains(&"id"));
    Ok(lignes
        .into_iter()
        .filter(|(nom, genre, pk)| {
            let id_local =
                nom == "id" && *pk == 1 && genre.eq_ignore_ascii_case("INTEGER") && !garde_id;
            !id_local
                && nom != "sync_id"
                && nom != "sync_at"
                && !table.local_columns.contains(&nom.as_str())
        })
        .map(|(nom, _, _)| nom)
        .collect())
}

fn to_json(v: Sql) -> Json {
    match v {
        Sql::Null => Json::Null,
        Sql::Integer(i) => Json::from(i),
        Sql::Real(f) => Json::from(f),
        Sql::Text(t) => Json::from(t),
        Sql::Blob(b) => Json::from(b),
    }
}

fn to_sql(v: &Json) -> Sql {
    match v {
        Json::Null => Sql::Null,
        Json::Bool(b) => Sql::Integer(i64::from(*b)),
        Json::Number(n) => n
            .as_i64()
            .map(Sql::Integer)
            .unwrap_or_else(|| Sql::Real(n.as_f64().unwrap_or(0.0))),
        Json::String(s) => Sql::Text(s.clone()),
        Json::Array(a) => Sql::Blob(a.iter().map(|x| x.as_u64().unwrap_or(0) as u8).collect()),
        Json::Object(_) => Sql::Text(v.to_string()),
    }
}

fn sync_id_of(c: &Connection, table: &str, id: &Sql) -> Result<Option<String>> {
    if matches!(id, Sql::Null) {
        return Ok(None);
    }
    c.query_row(
        &format!("SELECT sync_id FROM {table} WHERE id = ?1"),
        [id],
        |r| r.get(0),
    )
    .optional()
    .map(Option::flatten)
    .map_err(|e| sql_err("a parent's sync id", e))
}

fn id_of(c: &Connection, table: &str, sync_id: &str) -> Result<Option<i64>> {
    c.query_row(
        &format!("SELECT id FROM {table} WHERE sync_id = ?1"),
        [sync_id],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| sql_err("a local id", e))
}

fn collect<T>(
    c: &Connection,
    sql: &str,
    arg: &str,
    f: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    let mut s = c.prepare(sql).map_err(|e| sql_err("changes", e))?;
    let v = s
        .query_map([arg], f)
        .map_err(|e| sql_err("changes", e))?
        .collect::<rusqlite::Result<Vec<T>>>()
        .map_err(|e| sql_err("changes", e))?;
    Ok(v)
}

impl Store {
    /// Everything changed here after `clock` ("" for everything).
    pub fn changes_since(&self, clock: &str) -> Result<ChangeSet> {
        let device = self.device_id();
        self.with_conn(|c| {
            let mut set = ChangeSet {
                from_device: device,
                up_to: clock.to_string(),
                ..ChangeSet::default()
            };
            for table in SYNCED {
                let cols = columns(c, table)?;
                let filtre = table
                    .where_local
                    .map(|w| format!(" AND {w}"))
                    .unwrap_or_default();
                let liste = if cols.is_empty() {
                    String::new()
                } else {
                    format!(", {}", cols.join(", "))
                };
                let sql = format!(
                    "SELECT sync_id, sync_at{liste} FROM {} \
                     WHERE sync_id IS NOT NULL AND sync_at > ?1{filtre} ORDER BY sync_at",
                    table.name
                );
                let n = cols.len();
                let lignes = collect(c, &sql, clock, |r| {
                    let mut valeurs = Vec::with_capacity(n);
                    for i in 0..n {
                        valeurs.push(r.get::<_, Sql>(i + 2)?);
                    }
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, valeurs))
                })?;
                for (sync_id, sync_at, valeurs) in lignes {
                    let mut fields = Map::new();
                    for (nom, v) in cols.iter().zip(valeurs) {
                        let v = match table.parents.iter().find(|(col, _)| col == nom) {
                            Some((_, ptable)) => sync_id_of(c, ptable, &v)?
                                .map(Json::from)
                                .unwrap_or(Json::Null),
                            None => to_json(v),
                        };
                        fields.insert(nom.clone(), v);
                    }
                    if sync_at > set.up_to {
                        set.up_to = sync_at.clone();
                    }
                    set.rows.push(RowChange {
                        table: table.name.to_string(),
                        sync_id,
                        sync_at,
                        fields,
                    });
                }
            }
            set.tombstones = collect(
                c,
                "SELECT tbl, sync_id, sync_at FROM sync_tombstones WHERE sync_at > ?1",
                clock,
                |r| {
                    Ok(SyncTombstone {
                        table: r.get(0)?,
                        sync_id: r.get(1)?,
                        sync_at: r.get(2)?,
                    })
                },
            )?;
            set.settings = collect(
                c,
                "SELECT key, value, sync_at FROM sync_settings WHERE sync_at > ?1",
                clock,
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            let plus_tard = set
                .tombstones
                .iter()
                .map(|t| t.sync_at.as_str())
                .chain(set.settings.iter().map(|(_, _, at)| at.as_str()))
                .max()
                .unwrap_or_default()
                .to_string();
            if plus_tard > set.up_to {
                set.up_to = plus_tard;
            }
            Ok(set)
        })
    }

    /// A shared setting changed here: it travels with the next exchange.
    pub fn set_shared_setting(&self, key: &str, json: &str) -> Result<()> {
        let at = self.clock().now();
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO sync_settings (key, value, sync_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, sync_at = excluded.sync_at",
                params![key, json, at],
            )
            .map(|_| ())
            .map_err(|e| sql_err("a shared setting", e))
        })
    }

    /// Merges what another device sent, in one transaction: all of it or nothing.
    /// `peer_last_clock` is how far this device had merged that one before: a row
    /// changed here after it, and there too, is a conflict.
    pub fn apply_changes(&self, set: &ChangeSet, peer_last_clock: &str) -> Result<MergeReport> {
        let clock = self.clock();
        clock.set_merging(true);
        let issue = self.with_tx(|tx| merge(tx, set, peer_last_clock));
        clock.set_merging(false);
        let rapport = issue?;
        clock.observe(&set.up_to);
        Ok(rapport)
    }

    /// How far this device has merged another, "" if never.
    pub fn peer_last_clock(&self, device_id: &str) -> Result<String> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT last_clock FROM sync_peers WHERE device_id = ?1",
                [device_id],
                |r| r.get(0),
            )
            .optional()
            .map(Option::unwrap_or_default)
            .map_err(|e| sql_err("a peer's clock", e))
        })
    }

    /// A device paired with this one, under the name it gives itself.
    pub fn remember_peer(&self, device_id: &str, name: &str) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO sync_peers (device_id, name, last_seen_at)
                     VALUES (?1, ?2, CAST(strftime('%s', 'now') AS INTEGER))
                 ON CONFLICT(device_id) DO UPDATE SET name = excluded.name,
                     last_seen_at = excluded.last_seen_at",
                params![device_id, name],
            )
            .map(|_| ())
            .map_err(|e| sql_err("a paired device", e))
        })
    }

    /// The paired devices: id, name, when last synced (seconds), newest first.
    pub fn peers(&self) -> Result<Vec<(String, String, i64)>> {
        self.with_conn(|c| {
            let mut s = c
                .prepare(
                    "SELECT device_id, name, last_seen_at FROM sync_peers
                     ORDER BY last_seen_at DESC",
                )
                .map_err(|e| sql_err("paired devices", e))?;
            let v = s
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(|e| sql_err("paired devices", e))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("paired devices", e))?;
            Ok(v)
        })
    }

    /// The notes both devices had after their last sync, as the app wrote it (JSON).
    pub fn peer_notes_manifest(&self, device_id: &str) -> Result<String> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT notes_manifest FROM sync_peers WHERE device_id = ?1",
                [device_id],
                |r| r.get(0),
            )
            .optional()
            .map(Option::unwrap_or_default)
            .map_err(|e| sql_err("a peer's notes", e))
        })
    }

    pub fn set_peer_notes_manifest(&self, device_id: &str, json: &str) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE sync_peers SET notes_manifest = ?2 WHERE device_id = ?1",
                params![device_id, json],
            )
            .map(|_| ())
            .map_err(|e| sql_err("a peer's notes", e))
        })
    }

    /// Unpairs a device: it is no longer synced with.
    pub fn forget_peer(&self, device_id: &str) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM sync_peers WHERE device_id = ?1", [device_id])
                .map(|_| ())
                .map_err(|e| sql_err("unpairing", e))
        })
    }

    /// Drops the tombstones older than `older_than`: the smallest clock every paired
    /// device has merged.
    pub fn prune_tombstones(&self, older_than: &str) -> Result<usize> {
        self.with_conn(|c| {
            c.execute(
                "DELETE FROM sync_tombstones WHERE sync_at < ?1",
                [older_than],
            )
            .map_err(|e| sql_err("pruning tombstones", e))
        })
    }
}

fn label(fields: &Map<String, Json>) -> String {
    ["title", "name", "summary", "email", "address", "text"]
        .iter()
        .find_map(|k| fields.get(*k).and_then(Json::as_str))
        .unwrap_or_default()
        .to_string()
}

/// The local row a remote one is: by `sync_id`, else by its natural key (made on both
/// devices before they met). Returns its `sync_id` and its `sync_at`.
fn local_row(
    tx: &Connection,
    table: &SyncedTable,
    row: &RowChange,
    valeurs: &[(String, Sql)],
) -> Result<Option<(String, String)>> {
    let par_id = tx
        .query_row(
            &format!(
                "SELECT sync_id, sync_at FROM {} WHERE sync_id = ?1",
                table.name
            ),
            [&row.sync_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| sql_err("a local row", e))?;
    let Some(cles) = table.natural_key else {
        return Ok(par_id);
    };
    if par_id.is_some() {
        return Ok(par_id);
    }
    let sans_casse = matches!(table.name, "account_tags" | "accounts");
    let mut conditions = Vec::new();
    let mut args = Vec::new();
    for k in cles {
        let Some((_, v)) = valeurs.iter().find(|(n, _)| n == k) else {
            return Ok(None);
        };
        conditions.push(format!(
            "{k} = ?{}{}",
            args.len() + 1,
            if sans_casse { " COLLATE NOCASE" } else { "" }
        ));
        args.push(v.clone());
    }
    tx.query_row(
        &format!(
            "SELECT coalesce(sync_id, ''), sync_at FROM {} WHERE {}",
            table.name,
            conditions.join(" AND ")
        ),
        rusqlite::params_from_iter(args.iter()),
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map_err(|e| sql_err("a local row by its key", e))
}

fn merge(tx: &Connection, set: &ChangeSet, peer_last_clock: &str) -> Result<MergeReport> {
    let mut rapport = MergeReport::default();
    for table in SYNCED {
        let cols = columns(tx, table)?;
        let mut en_attente: Vec<&RowChange> =
            set.rows.iter().filter(|r| r.table == table.name).collect();
        // A task whose parent task comes later in the batch waits for it.
        while !en_attente.is_empty() {
            let avant = en_attente.len();
            let mut reste = Vec::new();
            for row in en_attente {
                match resolve(tx, table, &cols, row)? {
                    Some(valeurs) => {
                        merge_row(tx, table, row, valeurs, peer_last_clock, &mut rapport)?
                    }
                    None => reste.push(row),
                }
            }
            if reste.len() == avant {
                return Err(Error::store(format!(
                    "sync: {avant} rows of {} whose parent never came",
                    table.name
                )));
            }
            en_attente = reste;
        }
    }

    for t in &set.tombstones {
        let Some(table) = synced(&t.table) else {
            continue;
        };
        let local: Option<String> = tx
            .query_row(
                &format!("SELECT sync_at FROM {} WHERE sync_id = ?1", table.name),
                [&t.sync_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| sql_err("a row deleted there", e))?;
        if let Some(local_at) = local {
            if local_at > t.sync_at {
                // Changed here after it was deleted there: it stays.
                continue;
            }
            if table.name == "accounts" {
                let email: String = tx
                    .query_row(
                        "SELECT email FROM accounts WHERE sync_id = ?1",
                        [&t.sync_id],
                        |r| r.get(0),
                    )
                    .map_err(|e| sql_err("an account deleted there", e))?;
                rapport.accounts_removed_there.push(email);
                continue;
            }
            tx.execute(
                &format!("DELETE FROM {} WHERE sync_id = ?1", table.name),
                [&t.sync_id],
            )
            .map_err(|e| sql_err("deleting what was deleted there", e))?;
            rapport.deleted += 1;
        }
        // Kept here too: for a third device, and so that an older copy of the row
        // does not bring it back.
        tx.execute(
            "INSERT INTO sync_tombstones (tbl, sync_id, sync_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(tbl, sync_id) DO UPDATE SET sync_at = max(sync_at, excluded.sync_at)",
            params![t.table, t.sync_id, t.sync_at],
        )
        .map_err(|e| sql_err("a tombstone", e))?;
    }

    for (key, value, at) in &set.settings {
        let ici: Option<String> = tx
            .query_row(
                "SELECT sync_at FROM sync_settings WHERE key = ?1",
                [key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| sql_err("a shared setting", e))?;
        if ici.as_deref().is_some_and(|i| i >= at.as_str()) {
            continue;
        }
        tx.execute(
            "INSERT INTO sync_settings (key, value, sync_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, sync_at = excluded.sync_at",
            params![key, value, at],
        )
        .map_err(|e| sql_err("a shared setting", e))?;
        rapport.settings.push((key.clone(), value.clone()));
    }

    tx.execute(
        "INSERT INTO sync_peers (device_id, last_clock, last_seen_at)
             VALUES (?1, ?2, CAST(strftime('%s', 'now') AS INTEGER))
         ON CONFLICT(device_id) DO UPDATE SET
             last_clock = max(last_clock, excluded.last_clock),
             last_seen_at = excluded.last_seen_at",
        params![set.from_device, set.up_to],
    )
    .map_err(|e| sql_err("the peer's clock", e))?;
    Ok(rapport)
}

/// The row's values as this base writes them, parents turned back into local ids.
/// `None` while a parent is not here yet.
fn resolve(
    tx: &Connection,
    table: &SyncedTable,
    cols: &[String],
    row: &RowChange,
) -> Result<Option<Vec<(String, Sql)>>> {
    let mut valeurs = Vec::new();
    for nom in cols {
        let Some(v) = row.fields.get(nom) else {
            continue;
        };
        let v = match table.parents.iter().find(|(c, _)| c == nom) {
            Some((_, ptable)) => match v.as_str() {
                Some(psid) => match id_of(tx, ptable, psid)? {
                    Some(id) => Sql::Integer(id),
                    None => return Ok(None),
                },
                None => Sql::Null,
            },
            None => to_sql(v),
        };
        valeurs.push((nom.clone(), v));
    }
    Ok(Some(valeurs))
}

fn merge_row(
    tx: &Connection,
    table: &SyncedTable,
    row: &RowChange,
    valeurs: Vec<(String, Sql)>,
    peer_last_clock: &str,
    rapport: &mut MergeReport,
) -> Result<()> {
    let tombe: Option<String> = tx
        .query_row(
            "SELECT sync_at FROM sync_tombstones WHERE tbl = ?1 AND sync_id = ?2",
            [table.name, &row.sync_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| sql_err("a tombstone", e))?;
    if tombe.as_deref().is_some_and(|t| t >= row.sync_at.as_str()) {
        // Deleted here after it was last changed there.
        return Ok(());
    }

    let Some((local_sid, local_at)) = local_row(tx, table, row, &valeurs)? else {
        let mut noms: Vec<&str> = valeurs.iter().map(|(n, _)| n.as_str()).collect();
        noms.extend(["sync_id", "sync_at"]);
        let marques: Vec<String> = (1..=noms.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table.name,
            noms.join(", "),
            marques.join(", ")
        );
        let mut args: Vec<Sql> = valeurs.into_iter().map(|(_, v)| v).collect();
        args.push(Sql::Text(row.sync_id.clone()));
        args.push(Sql::Text(row.sync_at.clone()));
        tx.execute(&sql, rusqlite::params_from_iter(args.iter()))
            .map_err(|e| sql_err(&format!("adding a row of {}", table.name), e))?;
        rapport.inserted += 1;
        return Ok(());
    };

    // Met by its natural key under another id: both devices keep the smaller one.
    let sid = if local_sid.is_empty() || local_sid == row.sync_id {
        row.sync_id.clone()
    } else {
        local_sid.clone().min(row.sync_id.clone())
    };
    let change_ici = !peer_last_clock.is_empty() && local_at.as_str() > peer_last_clock;
    let conflit = || Conflict {
        table: table.name.into(),
        sync_id: row.sync_id.clone(),
        label: label(&row.fields),
    };
    // The row is found by its id here, or by its natural key when it has none yet.
    let n = valeurs.len();
    let (ou, cle): (String, Sql) = if local_sid.is_empty() {
        let cles = table.natural_key.unwrap_or_default();
        let mut conditions = Vec::new();
        for k in cles {
            let i = valeurs.iter().position(|(nom, _)| nom == k).unwrap_or(0);
            conditions.push(format!("{k} = ?{}", i + 1));
        }
        (conditions.join(" AND "), Sql::Null)
    } else {
        (
            format!("sync_id = ?{}", n + 3),
            Sql::Text(local_sid.clone()),
        )
    };

    if row.sync_at > local_at {
        let mut sets: Vec<String> = valeurs
            .iter()
            .enumerate()
            .map(|(i, (nom, _))| format!("{nom} = ?{}", i + 1))
            .collect();
        sets.push(format!("sync_id = ?{}", n + 1));
        sets.push(format!("sync_at = ?{}", n + 2));
        let sql = format!("UPDATE {} SET {} WHERE {ou}", table.name, sets.join(", "));
        let mut args: Vec<Sql> = valeurs.into_iter().map(|(_, v)| v).collect();
        args.push(Sql::Text(sid));
        args.push(Sql::Text(row.sync_at.clone()));
        if !local_sid.is_empty() {
            args.push(cle);
        }
        tx.execute(&sql, rusqlite::params_from_iter(args.iter()))
            .map_err(|e| sql_err("updating from the other device", e))?;
        rapport.updated += 1;
        if change_ici {
            rapport.conflicts.push(conflit());
        }
    } else {
        if sid != local_sid {
            tx.execute(
                &format!("UPDATE {} SET sync_id = ?1 WHERE sync_id IS ?2", table.name),
                params![sid, local_sid],
            )
            .map_err(|e| sql_err("taking the shared id", e))?;
        }
        if row.sync_at != local_at && change_ici {
            rapport.conflicts.push(conflit());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sql(s: &Store, q: &str) {
        s.with_conn(|c| c.execute_batch(q).map_err(|e| sql_err(q, e)))
            .unwrap();
    }

    fn un(s: &Store, q: &str) -> Option<String> {
        s.with_conn(|c| {
            c.query_row(q, [], |r| r.get::<_, String>(0))
                .optional()
                .map_err(|e| sql_err(q, e))
        })
        .unwrap()
    }

    fn nombre(s: &Store, q: &str) -> i64 {
        s.with_conn(|c| c.query_row(q, [], |r| r.get(0)).map_err(|e| sql_err(q, e)))
            .unwrap()
    }

    /// A sends B everything B has not merged from it.
    fn envoyer(a: &Store, b: &Store) -> MergeReport {
        let depuis = b.peer_last_clock(&a.device_id()).unwrap();
        let set = a.changes_since(&depuis).unwrap();
        b.apply_changes(&set, &depuis).unwrap()
    }

    fn compte(email: &str) -> String {
        format!(
            "INSERT INTO accounts (email, imap_host, imap_port, smtp_host, smtp_port, created_at)
             VALUES ('{email}', 'imap.example.com', 993, 'smtp.example.com', 465, 0);"
        )
    }

    #[test]
    fn a_task_made_on_one_device_appears_on_the_other_with_its_list() {
        let (a, b) = (Store::in_memory().unwrap(), Store::in_memory().unwrap());
        sql(
            &a,
            "INSERT INTO task_lists (name, color, created_at) VALUES ('Courses', '#000', 0);
             INSERT INTO tasks (list_id, title, created_at, updated_at, thread_id)
                 SELECT id, 'Buy bread', 0, 0, 77 FROM task_lists WHERE name = 'Courses';",
        );
        let set = a.changes_since("").unwrap();
        let tache = set.rows.iter().find(|r| r.table == "tasks").unwrap();
        assert!(tache.fields.get("id").is_none(), "no local id travels");
        assert!(tache.fields.get("thread_id").is_none(), "nor a mail thread");
        assert_eq!(
            tache.fields["list_id"].as_str().map(str::len),
            Some(32),
            "the list as its sync id"
        );
        let r = envoyer(&a, &b);
        assert!(r.inserted >= 2, "{r:?}");
        assert_eq!(
            un(
                &b,
                "SELECT l.name FROM tasks t JOIN task_lists l ON l.id = t.list_id
                 WHERE t.title = 'Buy bread'"
            )
            .as_deref(),
            Some("Courses")
        );
        assert_eq!(
            nombre(&b, "SELECT count(*) FROM sync_tombstones"),
            0,
            "a merge writes no tombstone of its own"
        );
        // The seeded rows met instead of doubling.
        assert_eq!(nombre(&b, "SELECT count(*) FROM areas"), 6);
        assert_eq!(nombre(&b, "SELECT count(*) FROM task_lists"), 2);
    }

    #[test]
    fn merging_twice_changes_nothing() {
        let (a, b) = (Store::in_memory().unwrap(), Store::in_memory().unwrap());
        sql(
            &a,
            "INSERT INTO goals (title, due_day, color, created_at) VALUES ('Run', '2026-12-31', '#000', 0);",
        );
        let set = a.changes_since("").unwrap();
        b.apply_changes(&set, "").unwrap();
        let avant = b.changes_since("").unwrap();
        let r = b.apply_changes(&set, "").unwrap();
        assert_eq!((r.inserted, r.updated), (0, 0));
        assert_eq!(b.changes_since("").unwrap().rows, avant.rows);
    }

    #[test]
    fn edited_on_both_the_newer_wins_and_is_named() {
        let (a, b) = (Store::in_memory().unwrap(), Store::in_memory().unwrap());
        sql(
            &a,
            "INSERT INTO task_lists (name, color, created_at) VALUES ('Work', '#000', 0);",
        );
        envoyer(&a, &b);
        envoyer(&b, &a);
        sql(
            &a,
            "UPDATE task_lists SET name = 'Work (A)' WHERE name = 'Work';",
        );
        b.clock().observe(&a.changes_since("").unwrap().up_to);
        sql(
            &b,
            "UPDATE task_lists SET name = 'Work (B)' WHERE name = 'Work';",
        );

        let r = envoyer(&b, &a);
        assert_eq!(r.conflicts.len(), 1, "{r:?}");
        assert_eq!(r.conflicts[0].label, "Work (B)");
        assert!(un(&a, "SELECT name FROM task_lists WHERE name = 'Work (B)'").is_some());
        let r = envoyer(&a, &b);
        assert!(r.conflicts.is_empty() || r.updated == 0);
        assert!(un(&b, "SELECT name FROM task_lists WHERE name = 'Work (B)'").is_some());
    }

    #[test]
    fn a_delete_wins_only_over_older_edits() {
        let (a, b) = (Store::in_memory().unwrap(), Store::in_memory().unwrap());
        sql(
            &a,
            "INSERT INTO cycles (name, start_day, created_at) VALUES ('Autumn', '2026-09-07', 0);
             INSERT INTO cycles (name, start_day, created_at) VALUES ('Winter', '2026-12-07', 0);",
        );
        envoyer(&a, &b);
        // Autumn: edited on B, then deleted on A — gone.
        sql(&b, "UPDATE cycles SET weeks = 10 WHERE name = 'Autumn';");
        a.clock().observe(&b.changes_since("").unwrap().up_to);
        sql(&a, "DELETE FROM cycles WHERE name = 'Autumn';");
        // Winter: deleted on A, then edited on B — stays.
        sql(&a, "DELETE FROM cycles WHERE name = 'Winter';");
        b.clock().observe(&a.changes_since("").unwrap().up_to);
        sql(&b, "UPDATE cycles SET weeks = 8 WHERE name = 'Winter';");

        let r = envoyer(&a, &b);
        assert_eq!(r.deleted, 1, "{r:?}");
        assert!(un(&b, "SELECT name FROM cycles WHERE name = 'Autumn'").is_none());
        assert!(un(&b, "SELECT name FROM cycles WHERE name = 'Winter'").is_some());
        envoyer(&b, &a);
        assert!(
            un(&a, "SELECT name FROM cycles WHERE name = 'Winter'").is_some(),
            "edited after its deletion, it comes back"
        );
        assert!(un(&a, "SELECT name FROM cycles WHERE name = 'Autumn'").is_none());
    }

    #[test]
    fn an_account_removed_there_stays_here_and_is_asked_about() {
        let (a, b) = (Store::in_memory().unwrap(), Store::in_memory().unwrap());
        sql(&a, &compte("marie@example.com"));
        envoyer(&a, &b);
        sql(
            &a,
            "DELETE FROM accounts WHERE email = 'marie@example.com';",
        );
        let r = envoyer(&a, &b);
        assert_eq!(r.accounts_removed_there, ["marie@example.com"]);
        assert_eq!(nombre(&b, "SELECT count(*) FROM accounts"), 1);
    }

    #[test]
    fn the_same_address_on_both_is_one_account() {
        let (a, b) = (Store::in_memory().unwrap(), Store::in_memory().unwrap());
        sql(&a, &compte("marie@example.com"));
        sql(&b, &compte("Marie@Example.com"));
        envoyer(&a, &b);
        envoyer(&b, &a);
        assert_eq!(nombre(&a, "SELECT count(*) FROM accounts"), 1);
        assert_eq!(nombre(&b, "SELECT count(*) FROM accounts"), 1);
        assert_eq!(
            un(&a, "SELECT sync_id FROM accounts"),
            un(&b, "SELECT sync_id FROM accounts"),
            "both keep one id"
        );
    }

    #[test]
    fn checks_of_one_habit_on_two_days_both_survive() {
        let (a, b) = (Store::in_memory().unwrap(), Store::in_memory().unwrap());
        sql(
            &a,
            "INSERT INTO habits (title, color, created_at) VALUES ('Read', '#000', 0);",
        );
        envoyer(&a, &b);
        sql(
            &a,
            "INSERT INTO habit_checks (habit_id, day) VALUES (1, '2026-10-26');",
        );
        sql(
            &b,
            "INSERT INTO habit_checks (habit_id, day)
                 SELECT id, '2026-10-27' FROM habits WHERE title = 'Read';",
        );
        envoyer(&a, &b);
        envoyer(&b, &a);
        assert_eq!(nombre(&a, "SELECT count(*) FROM habit_checks"), 2);
        assert_eq!(nombre(&b, "SELECT count(*) FROM habit_checks"), 2);
    }

    #[test]
    fn a_batch_that_fails_leaves_nothing_behind() {
        let (a, b) = (Store::in_memory().unwrap(), Store::in_memory().unwrap());
        sql(
            &a,
            "INSERT INTO task_lists (name, color, created_at) VALUES ('Courses', '#000', 0);
             INSERT INTO tasks (list_id, title, created_at, updated_at)
                 SELECT id, 'Buy bread', 0, 0 FROM task_lists WHERE name = 'Courses';",
        );
        let mut set = a.changes_since("").unwrap();
        // The list never arrives: the task cannot be placed.
        set.rows.retain(|r| r.table != "task_lists");
        set.rows.push(RowChange {
            table: "goals".into(),
            sync_id: "f".repeat(32),
            sync_at: set.up_to.clone(),
            fields: serde_json::from_str(
                r##"{"title":"Run","due_day":"2026-12-31","color":"#000","created_at":0}"##,
            )
            .unwrap(),
        });
        assert!(b.apply_changes(&set, "").is_err());
        assert_eq!(nombre(&b, "SELECT count(*) FROM goals"), 0, "nothing kept");
        assert!(!b.clock().merging(), "the merge is over");
    }

    #[test]
    fn both_ways_the_two_bases_end_the_same() {
        let (a, b) = (Store::in_memory().unwrap(), Store::in_memory().unwrap());
        sql(
            &a,
            "INSERT INTO goals (title, due_day, color, created_at) VALUES ('Run', '2026-12-31', '#000', 0);
             INSERT INTO goal_milestones (goal_id, title) VALUES (1, '10 km');",
        );
        sql(
            &b,
            "INSERT INTO unsubscribed (address, at) VALUES ('news@example.com', 1);
             UPDATE vision SET text = 'Calm' WHERE id = 1;",
        );
        envoyer(&a, &b);
        envoyer(&b, &a);
        envoyer(&a, &b);
        let rangs = |s: &Store| {
            let mut v: Vec<(String, String, String)> = s
                .changes_since("")
                .unwrap()
                .rows
                .into_iter()
                .map(|r| {
                    (
                        r.table,
                        r.sync_id,
                        serde_json::to_string(&r.fields).unwrap(),
                    )
                })
                .collect();
            v.sort();
            v
        };
        assert_eq!(rangs(&a), rangs(&b));
    }

    #[test]
    fn tombstones_every_peer_has_merged_are_pruned() {
        let a = Store::in_memory().unwrap();
        sql(
            &a,
            "INSERT INTO cycles (name, start_day, created_at) VALUES ('Old', '2026-01-05', 0);
             DELETE FROM cycles WHERE name = 'Old';",
        );
        let premier = un(&a, "SELECT sync_at FROM sync_tombstones").unwrap();
        sql(
            &a,
            "INSERT INTO cycles (name, start_day, created_at) VALUES ('New', '2026-02-02', 0);
             DELETE FROM cycles WHERE name = 'New';",
        );
        let second = un(&a, "SELECT max(sync_at) FROM sync_tombstones").unwrap();
        assert!(second > premier);
        assert_eq!(a.prune_tombstones(&second).unwrap(), 1);
        assert_eq!(nombre(&a, "SELECT count(*) FROM sync_tombstones"), 1);
    }

    #[test]
    fn a_shared_setting_travels_and_the_newer_one_stays() {
        let (a, b) = (Store::in_memory().unwrap(), Store::in_memory().unwrap());
        a.set_shared_setting("accent", "\"#ff9500\"").unwrap();
        let r = envoyer(&a, &b);
        assert_eq!(
            r.settings,
            [("accent".to_string(), "\"#ff9500\"".to_string())]
        );
        assert!(envoyer(&a, &b).settings.is_empty(), "only once");
    }
}
