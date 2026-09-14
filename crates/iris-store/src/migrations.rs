//! Schéma et migrations.
//!
//! Les migrations sont une liste ordonnée et immuable : une migration publiée n'est
//! jamais modifiée, seulement suivie d'une autre. La version est portée par
//! `PRAGMA user_version`, qui vit dans l'en-tête du fichier et ne coûte donc aucune
//! table supplémentaire.

use iris_types::{Error, Result};
use rusqlite::Connection;

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Version courante du schéma.
pub const CURRENT_VERSION: i64 = 1;

pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "schéma initial",
    sql: SCHEMA_V1,
}];

/// Le schéma initial.
///
/// Trois choix méritent explication :
///
/// - **`threads` porte l'état de workflow**, pas `messages` : on traite un échange,
///   pas un message isolé.
/// - **`threads_by_state`** est l'index directeur de la liste principale. Sa forme
///   `(state, last_activity_at DESC, id DESC)` est exactement celle de la pagination
///   par curseur, ce qui permet à SQLite de servir une page sans jamais trier.
/// - **`messages.preview`** est calculé une fois à la synchronisation. Le
///   défilement ne doit jamais déclencher d'analyse MIME.
const SCHEMA_V1: &str = r#"
CREATE TABLE accounts (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    email             TEXT    NOT NULL UNIQUE,
    display_name      TEXT    NOT NULL DEFAULT '',
    imap_host         TEXT    NOT NULL,
    imap_port         INTEGER NOT NULL,
    imap_tls          INTEGER NOT NULL DEFAULT 1,
    smtp_host         TEXT    NOT NULL,
    smtp_port         INTEGER NOT NULL,
    smtp_tls          INTEGER NOT NULL DEFAULT 1,
    auth_kind         TEXT    NOT NULL DEFAULT 'password',
    group_name        TEXT,
    pinned            INTEGER NOT NULL DEFAULT 0,
    enabled           INTEGER NOT NULL DEFAULT 1,
    created_at        INTEGER NOT NULL,
    last_activity_at  INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE folders (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id      INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    path            TEXT    NOT NULL,
    role            TEXT    NOT NULL DEFAULT 'other',
    uid_validity    INTEGER NOT NULL DEFAULT 0,
    uid_next        INTEGER NOT NULL DEFAULT 0,
    highest_modseq  INTEGER NOT NULL DEFAULT 0,
    UNIQUE(account_id, path)
) STRICT;

CREATE TABLE threads (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    subject_norm      TEXT    NOT NULL DEFAULT '',
    state             INTEGER NOT NULL DEFAULT 0,
    snooze_until      INTEGER,
    snooze_restore    INTEGER,
    last_activity_at  INTEGER NOT NULL DEFAULT 0,
    message_count     INTEGER NOT NULL DEFAULT 0,
    unread_count      INTEGER NOT NULL DEFAULT 0,
    flags_union       INTEGER NOT NULL DEFAULT 0,
    -- Colonnes dénormalisées pour que la liste n'ait besoin d'aucune jointure.
    last_from_name    TEXT    NOT NULL DEFAULT '',
    last_from_addr    TEXT    NOT NULL DEFAULT '',
    last_subject      TEXT    NOT NULL DEFAULT '',
    last_preview      TEXT    NOT NULL DEFAULT ''
) STRICT;

CREATE INDEX threads_by_state
    ON threads(state, last_activity_at DESC, id DESC);
CREATE INDEX threads_snoozed
    ON threads(snooze_until) WHERE snooze_until IS NOT NULL;

CREATE TABLE messages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id      INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    folder_id       INTEGER NOT NULL REFERENCES folders(id)  ON DELETE CASCADE,
    thread_id       INTEGER NOT NULL REFERENCES threads(id),
    uid             INTEGER NOT NULL,
    rfc_message_id  TEXT,
    in_reply_to     TEXT,
    subject         TEXT    NOT NULL DEFAULT '',
    from_name       TEXT    NOT NULL DEFAULT '',
    from_addr       TEXT    NOT NULL DEFAULT '',
    recipients      TEXT    NOT NULL DEFAULT '[]',
    date            INTEGER NOT NULL DEFAULT 0,
    received        INTEGER NOT NULL DEFAULT 0,
    size            INTEGER NOT NULL DEFAULT 0,
    flags           INTEGER NOT NULL DEFAULT 0,
    preview         TEXT    NOT NULL DEFAULT '',
    body_blob       TEXT,
    unsubscribe     TEXT,
    UNIQUE(folder_id, uid)
) STRICT;

CREATE INDEX messages_by_thread  ON messages(thread_id, received);
CREATE INDEX messages_by_account ON messages(account_id, received DESC);
CREATE INDEX messages_by_rfc_id  ON messages(rfc_message_id) WHERE rfc_message_id IS NOT NULL;
-- Sans cet index, le rattachement d'une reponse arrivee avant son original balaye
-- toute la table a chaque insertion, ce qui rend la synchronisation quadratique.
CREATE INDEX messages_by_in_reply_to ON messages(in_reply_to) WHERE in_reply_to IS NOT NULL;

-- Chaîne References, utilisée par le regroupement en fils.
CREATE TABLE message_refs (
    message_id  INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    ref_id      TEXT    NOT NULL,
    PRIMARY KEY(message_id, position)
) STRICT;

CREATE INDEX message_refs_by_ref ON message_refs(ref_id);

CREATE TABLE attachments (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id  INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    filename    TEXT    NOT NULL,
    mime_type   TEXT    NOT NULL,
    size        INTEGER NOT NULL,
    blob        TEXT,
    inline      INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE INDEX attachments_by_message ON attachments(message_id);

-- Quels comptes participent à un fil. Permet de filtrer une liste unifiée par
-- compte sans jointure sur messages.
CREATE TABLE thread_accounts (
    thread_id   INTEGER NOT NULL REFERENCES threads(id)   ON DELETE CASCADE,
    account_id  INTEGER NOT NULL REFERENCES accounts(id)  ON DELETE CASCADE,
    PRIMARY KEY(thread_id, account_id)
) STRICT;

CREATE INDEX thread_accounts_by_account ON thread_accounts(account_id);

-- La seule mémoire du système : à qui l'utilisateur a déjà écrit.
CREATE TABLE contacts_seen (
    addr_key        TEXT    PRIMARY KEY,
    display         TEXT    NOT NULL DEFAULT '',
    received_count  INTEGER NOT NULL DEFAULT 0,
    replied_count   INTEGER NOT NULL DEFAULT 0,
    last_seen_at    INTEGER NOT NULL DEFAULT 0
) STRICT;

-- Journal d'opérations : toute action locale en attente de réconciliation.
CREATE TABLE op_journal (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id       INTEGER NOT NULL,
    kind             TEXT    NOT NULL,
    payload          TEXT    NOT NULL,
    idempotency_key  TEXT    NOT NULL UNIQUE,
    created_at       INTEGER NOT NULL,
    attempts         INTEGER NOT NULL DEFAULT 0,
    next_attempt_at  INTEGER NOT NULL DEFAULT 0,
    last_error       TEXT,
    done             INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE INDEX op_journal_pending
    ON op_journal(done, next_attempt_at, id);

CREATE TABLE settings (
    key    TEXT PRIMARY KEY,
    value  TEXT NOT NULL
) STRICT;
"#;

/// Applique les migrations manquantes, dans une transaction par migration.
///
/// Retourne la version atteinte.
pub fn migrate(conn: &Connection) -> Result<i64> {
    let mut version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| Error::store(format!("lecture de la version : {e}")))?;

    if version > CURRENT_VERSION {
        return Err(Error::store(format!(
            "base écrite par une version plus récente d'Iris (schéma {version}, \
             cette version en connaît {CURRENT_VERSION})"
        )));
    }

    let pending: Vec<&Migration> = MIGRATIONS.iter().filter(|m| m.version > version).collect();
    for m in pending {
        tracing::info!(version = m.version, name = m.name, "migration");
        conn.execute_batch(&format!(
            "BEGIN; {} PRAGMA user_version = {}; COMMIT;",
            m.sql, m.version
        ))
        .map_err(|e| Error::store(format!("migration {} « {} » : {e}", m.version, m.name)))?;
        version = m.version;
    }

    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn une_base_neuve_atteint_la_version_courante() {
        let c = conn();
        assert_eq!(migrate(&c).unwrap(), CURRENT_VERSION);
    }

    #[test]
    fn migrer_deux_fois_ne_change_rien() {
        let c = conn();
        migrate(&c).unwrap();
        assert_eq!(migrate(&c).unwrap(), CURRENT_VERSION);
    }

    #[test]
    fn une_base_trop_recente_est_refusee_au_lieu_d_etre_corrompue() {
        let c = conn();
        c.execute_batch("PRAGMA user_version = 9999").unwrap();
        let e = migrate(&c).unwrap_err();
        assert!(e.to_string().contains("plus récente"));
    }

    #[test]
    fn les_versions_de_migration_sont_ordonnees_et_uniques() {
        let mut vus = std::collections::BTreeSet::new();
        let mut precedent = 0;
        for m in MIGRATIONS {
            assert!(m.version > precedent, "migrations non ordonnées");
            assert!(vus.insert(m.version), "version dupliquée");
            precedent = m.version;
        }
        assert_eq!(precedent, CURRENT_VERSION);
    }

    #[test]
    fn l_index_directeur_de_la_liste_existe() {
        let c = conn();
        migrate(&c).unwrap();
        let n: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='index' AND name='threads_by_state'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
}
