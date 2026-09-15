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
pub const CURRENT_VERSION: i64 = 8;

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial schema",
        sql: SCHEMA_V1,
    },
    Migration {
        version: 2,
        name: "rules",
        sql: SCHEMA_V2,
    },
    Migration {
        version: 3,
        name: "backfill spam",
        sql: SCHEMA_V3,
    },
    Migration {
        version: 4,
        name: "take binned mail out of the queue",
        sql: SCHEMA_V4,
    },
    Migration {
        version: 5,
        name: "repair the moves that could never replay",
        sql: SCHEMA_V5,
    },
    Migration {
        version: 6,
        name: "the bin is a folder, not a state",
        sql: SCHEMA_V6,
    },
    Migration {
        version: 7,
        name: "a thread remembers which mailbox it came from",
        sql: SCHEMA_V7,
    },
    Migration {
        version: 8,
        name: "each mailbox gets a signature",
        sql: SCHEMA_V8,
    },
];

/// Une signature par compte.
///
/// Tout client de courrier en a une depuis toujours ; Iris envoyait chaque message non
/// signé, et la seule parade était de retaper quatre lignes à chaque fois. Par compte
/// et non globale, parce que c'est la raison d'avoir plusieurs comptes : on ne signe pas
/// une facture comme on écrit à sa sœur.
///
/// Vide par défaut. Personne ne veut découvrir une signature inventée par le programme
/// au bas d'un message déjà parti.
const SCHEMA_V8: &str = r#"
ALTER TABLE accounts ADD COLUMN signature TEXT NOT NULL DEFAULT '';
"#;

/// Donne à chaque fil le compte de son dernier message.
///
/// La liste dessine une pastille de couleur à gauche de chaque ligne, dont tout
/// l'intérêt est de dire de quelle boîte vient le message quand on les regarde toutes
/// ensemble. Elle était calculée à partir de l'adresse du **premier compte**, la même
/// pour toutes les lignes : cent boîtes, une seule couleur, et un repère qui affirmait
/// quelque chose de faux plutôt que de ne rien dire.
///
/// La colonne rejoint les autres colonnes dénormalisées du fil, pour la raison qui les
/// a toutes mises là : une page de liste doit se servir en une seule lecture d'index,
/// sans jointure. Le remplissage initial prend le compte du message le plus récent de
/// chaque fil — exactement ce que `refresh_thread` maintiendra ensuite.
const SCHEMA_V7: &str = r#"
ALTER TABLE threads ADD COLUMN last_account_id INTEGER NOT NULL DEFAULT 0;

UPDATE threads
SET last_account_id = COALESCE((
        SELECT m.account_id
        FROM messages m
        WHERE m.thread_id = threads.id
        ORDER BY m.received DESC, m.id DESC
        LIMIT 1
    ), 0);
"#;

/// Défait la migration 4 : la corbeille n'est pas un état, c'est un endroit.
///
/// La migration 4 rangeait dans « Terminé » tout fil qui n'existait plus que dans une
/// corbeille. L'intention était bonne — ce courrier n'a rien à faire dans la file de
/// travail — et le moyen était faux. « Terminé » veut dire « je m'en suis occupé » ;
/// huit cent cinquante messages jetés y noyaient les quelques dizaines que
/// l'utilisateur avait réellement traités, et la file « Terminé » ne voulait plus rien
/// dire.
///
/// Ces fils sont donc rendus à l'état qu'ils avaient, et c'est la **requête** qui les
/// écarte désormais des trois files. La différence se voit : ils réapparaissent quand
/// on ouvre la corbeille, et nulle part ailleurs.
///
/// Rendre l'état perdu est possible sans l'avoir enregistré parce que la migration 4
/// ne touchait que `state = 0`. Un fil que l'utilisateur aurait lui-même marqué
/// terminé **et** qui serait entièrement à la corbeille serait ramené à tort — mais la
/// nouvelle requête le cache de toute façon, donc la correction ne se voit pas.
const SCHEMA_V6: &str = r#"
UPDATE threads
SET state = 0
WHERE state = 2
  AND EXISTS (SELECT 1 FROM messages WHERE messages.thread_id = threads.id)
  AND NOT EXISTS (
        SELECT 1
        FROM messages
        JOIN folders ON folders.id = messages.folder_id
        WHERE messages.thread_id = threads.id
          AND folders.role NOT IN ('trash', 'junk')
  );
"#;

/// Réécrit les opérations de déplacement que le rejeu n'a jamais su lire.
///
/// Le producteur écrivait `"to"`, le consommateur attendait `"target"`. Chaque
/// archivage et chaque suppression était donc jugé illisible, abandonné, et marqué
/// terminé sans avoir rien fait. Le fil quittait la file localement ; sur le serveur,
/// le message n'a jamais bougé.
///
/// La correction du code empêche d'en écrire de nouveaux. Celle-ci répare ceux qui
/// existent : le nom du champ est corrigé, et l'opération est remise en attente,
/// parce qu'aucune n'a jamais réussi — elles ont été abandonnées, pas exécutées.
/// L'utilisateur a demandé ces déplacements ; il n'y a pas de raison de les perdre.
///
/// Un UID qui n'existe plus fait échouer le rejeu d'une erreur définitive, que la
/// boucle traite déjà en abandonnant l'opération. Le pire cas est donc de revenir à
/// l'état actuel, ce qui est le bon pire cas.
///
/// La condition porte sur `'"to":'` plutôt que sur le type d'opération : elle décrit
/// exactement les lignes cassées, et une ligne saine que l'on toucherait par
/// approximation serait une régression introduite par une réparation.
const SCHEMA_V5: &str = r#"
UPDATE op_journal
SET payload = replace(payload, '"to":', '"target":'),
    done = 0,
    attempts = 0,
    next_attempt_at = 0,
    last_error = NULL
WHERE payload LIKE '%"op":"move"%' AND payload LIKE '%"to":%';
"#;

/// Takes out of the work queue every thread that only exists in the bin.
///
/// A mailbox triaged in another client — webmail, a phone — has most of its mail in
/// Trash already, and the first sync pulled all of it in as work to do. On the real
/// mailbox this was 485 messages of the 740: two thirds of the queue was rubbish
/// somebody had already thrown away.
///
/// Worse, Delete could not clear them. Moving a message that is already at its
/// destination produces nothing to send, the action reported "unchanged", and the
/// button appeared broken. That is fixed in the workflow; this is the backlog it
/// leaves behind, and pressing Delete four hundred times is not a fix.
///
/// Only threads whose messages are *all* in a bin or a junk folder. A conversation
/// with one message deleted and a reply still in the inbox is live work, and the one
/// deleted message says nothing about the other.
///
/// 2 is `WorkflowState::Done`, checked against the constant by a test below.
const SCHEMA_V4: &str = r#"
UPDATE threads
SET state = 2
WHERE state = 0
  AND EXISTS (SELECT 1 FROM messages WHERE messages.thread_id = threads.id)
  AND NOT EXISTS (
        SELECT 1
        FROM messages
        JOIN folders ON folders.id = messages.folder_id
        WHERE messages.thread_id = threads.id
          AND folders.role NOT IN ('trash', 'junk')
  );
"#;

/// Marks mail that was already in the database when spam detection arrived.
///
/// A flag only set on arrival would have left every message received before the
/// feature looking like ordinary mail for ever — which is exactly what a user sees
/// as "the spam filter does not work".
///
/// Only the subject marker is available here: the headers are not stored, and
/// re-fetching every message to read them would take hours. That is the weaker of
/// the two signals, but it is the one that put `***Potentiel-SPAM***` on screen, and
/// anything it misses gets caught on the next sync.
///
/// 512 is `Flags::SPAM`, checked against the constant by a test in this module.
const SCHEMA_V3: &str = r#"
UPDATE messages
SET flags = flags | 512
WHERE lower(subject) LIKE '%***spam***%'
   OR lower(subject) LIKE '%***potentiel-spam***%'
   OR lower(subject) LIKE '%***potential-spam***%'
   OR lower(subject) LIKE '%[spam]%'
   OR lower(subject) LIKE '%[spam?]%'
   OR lower(subject) LIKE '%{spam}%';

-- The list reads the thread, not the message, so the union needs the bit too.
-- Only ever setting it, never clearing, means this cannot disturb any other flag:
-- SQLite has no bitwise-or aggregate, and rebuilding the union with arithmetic
-- would quietly corrupt every thread it touched.
UPDATE threads
SET flags_union = flags_union | 512
WHERE EXISTS (
    SELECT 1 FROM messages
    WHERE messages.thread_id = threads.id AND (messages.flags & 512) != 0
);
"#;

/// Rules, and where they have already been applied.
///
/// The conditions and actions are stored as JSON rather than in columns. They are a
/// small tree with a shape that will grow — a new condition kind is a new variant, not
/// a new table — and nothing queries inside them: rules are read in bulk, evaluated in
/// memory, and there are tens of them, not millions.
///
/// `applied_to` is what stops a rule acting twice on the same message. Without it, a
/// rule that snoozes would push its own target forward on every synchronisation, and
/// the message would never come back.
const SCHEMA_V2: &str = r#"
CREATE TABLE rules (
    id          TEXT    PRIMARY KEY,
    name        TEXT    NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    position    INTEGER NOT NULL,
    definition  TEXT    NOT NULL
) STRICT;

CREATE INDEX rules_by_position ON rules(position);

CREATE TABLE rule_applications (
    rule_id    TEXT    NOT NULL,
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    applied_at INTEGER NOT NULL,
    PRIMARY KEY (rule_id, message_id)
) STRICT, WITHOUT ROWID;
"#;

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

    #[test]
    fn the_spam_bit_in_sql_matches_the_constant() {
        // The migration interpolates 512 into query text; if the flag ever moves,
        // this catches it before the wrong bit is set on someone's mailbox.
        assert_eq!(iris_types::Flags::SPAM.0, 512);
    }

    #[test]
    fn the_backfill_marks_mail_that_arrived_before_the_feature() {
        // A flag only set on arrival leaves everything already downloaded looking
        // like ordinary mail for ever, which reads as "the spam filter is broken".
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();

        conn.execute_batch(
            "INSERT INTO accounts (id, email, imap_host, imap_port, smtp_host, smtp_port, created_at)
             VALUES (1, 'a@x.fr', 'i', 993, 's', 465, 0);
             INSERT INTO folders (id, account_id, path, role) VALUES (1, 1, 'INBOX', 'inbox');
             INSERT INTO threads (id, subject_norm, state, last_activity_at, flags_union)
             VALUES (1, 'a', 0, 0, 0), (2, 'b', 0, 0, 0);
             INSERT INTO messages
                (id, account_id, folder_id, thread_id, uid, subject, from_name, from_addr,
                 recipients, date, received, size, flags, preview)
             VALUES
                (1, 1, 1, 1, 1, '***Potentiel-SPAM*** Virement', 'x', 'x@y.fr', '[]', 0, 0, 1, 0, ''),
                (2, 1, 1, 2, 2, 'Quote for the cylinders', 'x', 'x@y.fr', '[]', 0, 0, 1, 0, '');",
        )
        .unwrap();

        conn.execute_batch(SCHEMA_V3).unwrap();

        let spam: i64 = conn
            .query_row("SELECT flags FROM messages WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        let ordinary: i64 = conn
            .query_row("SELECT flags FROM messages WHERE id = 2", [], |r| r.get(0))
            .unwrap();

        assert_eq!(spam & 512, 512, "the tagged subject must be marked");
        assert_eq!(ordinary & 512, 0, "ordinary mail must be left alone");

        // And the thread, because the list reads the thread rather than the message.
        let thread: i64 = conn
            .query_row("SELECT flags_union FROM threads WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(thread & 512, 512);
    }

    #[test]
    fn les_deplacements_casses_sont_repares_et_remis_en_attente() {
        // Ce sont des actions que l'utilisateur a demandées et qui n'ont jamais eu
        // lieu. Corriger le code sans les réparer laisserait le courrier là où il
        // n'aurait jamais dû rester.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        for sql in [SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4] {
            conn.execute_batch(sql).unwrap();
        }

        conn.execute_batch(
            r#"INSERT INTO op_journal
                 (id, account_id, kind, payload, idempotency_key, created_at, done, attempts)
               VALUES
                 (1, 1, 'move_message',
                  '{"op":"move","folder":"INBOX","uids":[7],"to":"INBOX.Archive"}',
                  'k1', 0, 1, 3),
                 (2, 1, 'set_flags',
                  '{"op":"set_flags","folder":"INBOX","uids":[8],"flags":4,"add":true}',
                  'k2', 0, 1, 0);"#,
        )
        .unwrap();

        conn.execute_batch(SCHEMA_V5).unwrap();

        let (charge, fini, essais): (String, i64, i64) = conn
            .query_row(
                "SELECT payload, done, attempts FROM op_journal WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();

        assert!(charge.contains(r#""target":"INBOX.Archive""#), "obtenu : {charge}");
        assert!(!charge.contains(r#""to":"#), "l'ancien nom doit disparaître");
        assert_eq!(fini, 0, "l'opération doit être rejouée");
        assert_eq!(essais, 0, "et repartir d'un compteur neuf");
    }

    #[test]
    fn les_operations_saines_ne_sont_pas_touchees() {
        // Une ligne saine réveillée par une réparation serait une régression
        // introduite par la réparation elle-même.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        for sql in [SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4] {
            conn.execute_batch(sql).unwrap();
        }

        conn.execute_batch(
            r#"INSERT INTO op_journal
                 (id, account_id, kind, payload, idempotency_key, created_at, done)
               VALUES
                 (1, 1, 'set_flags',
                  '{"op":"set_flags","folder":"INBOX","uids":[8],"flags":4,"add":true}',
                  'k1', 0, 1),
                 (2, 1, 'move_message',
                  '{"op":"move","folder":"INBOX","uids":[9],"target":"INBOX.Archive"}',
                  'k2', 0, 1);"#,
        )
        .unwrap();

        conn.execute_batch(SCHEMA_V5).unwrap();

        let reveilles: i64 = conn
            .query_row("SELECT count(*) FROM op_journal WHERE done = 0", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(reveilles, 0, "aucune ligne saine ne doit être remise en file");
    }

    #[test]
    fn deux_est_bien_l_etat_termine() {
        // La migration écrit un nombre en dur : si l'encodage change, elle doit
        // échouer ici plutôt que de ranger silencieusement les fils dans la mauvaise
        // file de tous les utilisateurs.
        assert_eq!(iris_types::WorkflowState::Done.as_i64(), 2);
        assert_eq!(iris_types::WorkflowState::Todo.as_i64(), 0);
    }

    #[test]
    fn un_fil_entierement_a_la_corbeille_quitte_la_file() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();
        conn.execute_batch(SCHEMA_V3).unwrap();

        conn.execute_batch(
            "INSERT INTO accounts (id, email, imap_host, imap_port, smtp_host, smtp_port, created_at)
             VALUES (1, 'a@x.fr', 'i', 993, 's', 465, 0);
             INSERT INTO folders (id, account_id, path, role) VALUES (1, 1, 'INBOX', 'inbox');
             INSERT INTO folders (id, account_id, path, role) VALUES (2, 1, 'Trash', 'trash');
             INSERT INTO threads (id, subject_norm, state, last_activity_at, flags_union)
             VALUES (1, 'jete', 0, 0, 0), (2, 'vivant', 0, 0, 0), (3, 'mixte', 0, 0, 0);
             INSERT INTO messages
                (id, account_id, folder_id, thread_id, uid, subject, from_name, from_addr,
                 recipients, date, received, size, flags, preview)
             VALUES (1, 1, 2, 1, 1, 'jete', 'x', 'x@y.fr', '[]', 0, 0, 1, 0, ''),
                    (2, 1, 1, 2, 2, 'vivant', 'x', 'x@y.fr', '[]', 0, 0, 1, 0, ''),
                    (3, 1, 2, 3, 3, 'mixte a', 'x', 'x@y.fr', '[]', 0, 0, 1, 0, ''),
                    (4, 1, 1, 3, 4, 'mixte b', 'x', 'x@y.fr', '[]', 0, 0, 1, 0, '');",
        )
        .unwrap();

        conn.execute_batch(SCHEMA_V4).unwrap();

        let etat = |id: i64| -> i64 {
            conn.query_row("SELECT state FROM threads WHERE id = ?1", [id], |r| r.get(0))
                .unwrap()
        };

        assert_eq!(etat(1), 2, "un fil entièrement jeté sort de la file");
        assert_eq!(etat(2), 0, "un fil de la boîte de réception reste à traiter");
        assert_eq!(
            etat(3),
            0,
            "un message jeté ne dit rien de la réponse restée dans la boîte"
        );
    }

    #[test]
    fn un_fil_deja_classe_n_est_pas_deplace() {
        // Seule la file « à traiter » est concernée. Un fil rangé en attente par
        // l'utilisateur est une décision qui lui appartient.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();
        conn.execute_batch(SCHEMA_V3).unwrap();

        conn.execute_batch(
            "INSERT INTO accounts (id, email, imap_host, imap_port, smtp_host, smtp_port, created_at)
             VALUES (1, 'a@x.fr', 'i', 993, 's', 465, 0);
             INSERT INTO folders (id, account_id, path, role) VALUES (2, 1, 'Trash', 'trash');
             INSERT INTO threads (id, subject_norm, state, last_activity_at, flags_union)
             VALUES (1, 'attente', 1, 0, 0);
             INSERT INTO messages
                (id, account_id, folder_id, thread_id, uid, subject, from_name, from_addr,
                 recipients, date, received, size, flags, preview)
             VALUES (1, 1, 2, 1, 1, 'attente', 'x', 'x@y.fr', '[]', 0, 0, 1, 0, '');",
        )
        .unwrap();

        conn.execute_batch(SCHEMA_V4).unwrap();

        let etat: i64 = conn
            .query_row("SELECT state FROM threads WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(etat, 1);
    }

    #[test]
    fn the_backfill_leaves_other_flags_untouched() {
        // SQLite has no bitwise-or aggregate; rebuilding the union with arithmetic
        // would quietly corrupt every thread it touched.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();

        conn.execute_batch(
            "INSERT INTO accounts (id, email, imap_host, imap_port, smtp_host, smtp_port, created_at)
             VALUES (1, 'a@x.fr', 'i', 993, 's', 465, 0);
             INSERT INTO folders (id, account_id, path, role) VALUES (1, 1, 'INBOX', 'inbox');
             INSERT INTO threads (id, subject_norm, state, last_activity_at, flags_union)
             VALUES (1, 'a', 0, 0, 5);
             INSERT INTO messages
                (id, account_id, folder_id, thread_id, uid, subject, from_name, from_addr,
                 recipients, date, received, size, flags, preview)
             VALUES (1, 1, 1, 1, 1, '[SPAM] hello', 'x', 'x@y.fr', '[]', 0, 0, 1, 5, '');",
        )
        .unwrap();

        conn.execute_batch(SCHEMA_V3).unwrap();

        let flags: i64 = conn
            .query_row("SELECT flags FROM messages WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(flags, 5 | 512, "seen and flagged must survive");

        let union: i64 = conn
            .query_row("SELECT flags_union FROM threads WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(union, 5 | 512);
    }
}
