//! `iris-store` — la source de vérité locale.
//!
//! SQLite en mode WAL porte les métadonnées : comptes, dossiers, messages, fils,
//! états de workflow et journal d'opérations. Les corps et les pièces jointes vivent
//! ailleurs (`iris-blobs`), le plein texte aussi (`iris-index`).
//!
//! **Le store est synchrone et bloquant.** C'est délibéré : SQLite l'est de toute
//! façon, et une fausse façade asynchrone ne ferait que masquer le coût réel. Les
//! appelants l'exécutent hors du thread d'affichage, ce que garantit la couche
//! vue-modèle.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod account_tags;
mod accounts;
mod attachments;
mod backup;
mod caldav;
mod calendar;
mod goals;
mod habits;
mod journal;
mod messages;
mod migrations;
mod model;
mod ops;
mod rules;
mod scheduled;
mod tasks;
mod threads;

pub use account_tags::AccountTag;
pub use accounts::{AccountServers, UnifiedFolder};
pub use attachments::StoredAttachment;
pub use backup::{backup_info, BackupInfo};
pub use caldav::{CalendarAccount, ObjectState, OutgoingObject, Tombstone};
pub use calendar::{NewEvent, StoredCalendar, StoredEvent};
pub use goals::{Goal, GoalEntry, GoalKind, Milestone, NewGoal};
pub use habits::{Habit, HabitCheck, NewHabit};
pub use migrations::CURRENT_VERSION;
pub use model::{
    Account, AuthKind, Contact, Filters, Folder, FolderRole, ListCursor, ListQuery, NewAccount,
    NewMessage, OpKind, PendingOp, Scope, Sort, StoredMessage, ThreadRow,
};
pub use ops::OpPayload;
pub use rules::StoredRule;
pub use scheduled::{Alias, ScheduledMail};
pub use tasks::{NewTask, StoredTask, TaskList};
pub use threads::MailStats;

use iris_types::{Error, Result};
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

/// A folder's path without the `INBOX.` or `INBOX/` some hosts put in front of the
/// user's folders: the name it is shown, and matched across mailboxes, under.
pub fn display_path(path: &str) -> &str {
    let prefixe = path.get(..6);
    match prefixe {
        Some(p) if p.eq_ignore_ascii_case("INBOX.") || p.eq_ignore_ascii_case("INBOX/") => {
            &path[6..]
        }
        _ => path,
    }
}

/// Whether two paths name the same folder as shown: `Devis` at one host,
/// `INBOX.Devis` at another.
pub fn same_folder(a: &str, b: &str) -> bool {
    a == b || display_path(a) == display_path(b)
}

/// Poignée sur la base locale.
#[derive(Debug)]
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Ouvre ou crée la base au chemin indiqué, et applique les migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path.as_ref())
            .map_err(|e| Error::store(format!("ouverture : {e}")))?;
        Self::configure(&conn)?;
        migrations::migrate(&conn)?;
        // A repair and not a migration: copies stored apart by an older version can
        // also arrive later from a copy of the base, and finding none costs nothing.
        match messages::join_copies_of_one_message(&conn) {
            Ok(0) => {}
            Ok(n) => tracing::info!(threads = n, "copies of one message joined"),
            Err(e) => tracing::warn!(error = %e, "joining copies of one message"),
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Joins the threads that hold copies of one message. Done when the base opens.
    pub fn join_copies_of_one_message(&self) -> Result<usize> {
        self.with_conn(messages::join_copies_of_one_message)
    }

    /// Base en mémoire, pour les tests.
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|e| Error::store(format!("ouverture en mémoire : {e}")))?;
        Self::configure(&conn)?;
        migrations::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Réglages appliqués à chaque connexion.
    ///
    /// - `journal_mode = WAL` : les lectures ne bloquent pas l'écriture. Sans cela,
    ///   une synchronisation en cours figerait l'affichage de la liste.
    /// - `synchronous = NORMAL` : sûr en WAL, et évite un `fsync` par transaction —
    ///   décisif quand la synchronisation écrit par lots de milliers de messages.
    /// - `foreign_keys = ON` : les suppressions en cascade doivent réellement avoir
    ///   lieu, sinon supprimer un compte laisserait ses messages orphelins.
    /// - `busy_timeout` : plutôt attendre que retourner « base occupée » à l'interface.
    /// - `cache_size` à 8 Mo, et les tris temporaires **sur disque**. Le cache valait
    ///   32 Mo et les temporaires restaient en mémoire : après un grand parcours — une
    ///   recherche, une resynchronisation — SQLite gardait ces pages pour toujours, et
    ///   une application qui passe ses journées dans la zone de notification les payait
    ///   toute la journée. Les requêtes de la liste lisent une page de soixante lignes
    ///   par un index ; huit mégaoctets les servent entièrement, et le système de
    ///   fichiers garde de toute façon le reste en cache pour nous.
    fn configure(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA temp_store = FILE;
             PRAGMA cache_size = -8192;",
        )
        .map_err(|e| Error::store(format!("configuration : {e}")))
    }

    /// Exécute une fermeture avec la connexion.
    pub(crate) fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| Error::store("connexion empoisonnée"))?;
        f(&guard)
    }

    /// Exécute une fermeture dans une transaction, annulée en cas d'erreur.
    pub(crate) fn with_tx<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| Error::store("connexion empoisonnée"))?;
        let tx = guard
            .transaction()
            .map_err(|e| Error::store(format!("ouverture de transaction : {e}")))?;
        let out = f(&tx)?;
        tx.commit()
            .map_err(|e| Error::store(format!("validation : {e}")))?;
        Ok(out)
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.with_conn(|c| {
            c.query_row("PRAGMA user_version", [], |r| r.get(0))
                .map_err(|e| Error::store(e.to_string()))
        })
    }

    /// Compacte la base et met à jour les statistiques du planificateur.
    ///
    /// À appeler rarement : sur une base d'un million de messages, l'opération est
    /// longue et réécrit tout le fichier.
    pub fn maintenance(&self) -> Result<()> {
        self.with_conn(|c| {
            c.execute_batch("ANALYZE; PRAGMA optimize;")
                .map_err(|e| Error::store(format!("maintenance : {e}")))
        })
    }
}

/// Traduit une erreur rusqlite en erreur du domaine, en conservant le contexte.
pub(crate) fn sql_err(context: &str, e: rusqlite::Error) -> Error {
    Error::store(format!("{context} : {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_one_name_whatever_the_host_puts_in_front() {
        assert_eq!(display_path("INBOX.Devis"), "Devis");
        assert_eq!(display_path("inbox/Devis"), "Devis");
        assert_eq!(display_path("Devis"), "Devis");
        assert_eq!(display_path("INBOX"), "INBOX");
        assert_eq!(display_path("Réunions"), "Réunions");
        assert!(same_folder("Devis", "INBOX.Devis"));
        assert!(!same_folder("Devis", "INBOX.Offres"));
    }

    #[test]
    fn une_base_neuve_est_migree() {
        let s = Store::in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), CURRENT_VERSION);
    }

    #[test]
    fn les_cles_etrangeres_sont_actives() {
        let s = Store::in_memory().unwrap();
        let on: i64 = s
            .with_conn(|c| {
                c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))
                    .map_err(|e| Error::store(e.to_string()))
            })
            .unwrap();
        assert_eq!(
            on, 1,
            "sans cela, supprimer un compte laisserait des orphelins"
        );
    }

    #[test]
    fn une_transaction_qui_echoue_n_ecrit_rien() {
        let s = Store::in_memory().unwrap();
        let r: Result<()> = s.with_tx(|tx| {
            tx.execute("INSERT INTO settings(key, value) VALUES('a', '1')", [])
                .map_err(|e| Error::store(e.to_string()))?;
            Err(Error::other("échec volontaire"))
        });
        assert!(r.is_err());

        let n: i64 = s
            .with_conn(|c| {
                c.query_row("SELECT count(*) FROM settings", [], |r| r.get(0))
                    .map_err(|e| Error::store(e.to_string()))
            })
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn la_base_sur_disque_survit_a_une_reouverture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("iris.db");

        {
            let s = Store::open(&path).unwrap();
            s.with_conn(|c| {
                c.execute(
                    "INSERT INTO settings(key, value) VALUES('theme', 'mono')",
                    [],
                )
                .map_err(|e| Error::store(e.to_string()))
            })
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let v: String = s
            .with_conn(|c| {
                c.query_row("SELECT value FROM settings WHERE key='theme'", [], |r| {
                    r.get(0)
                })
                .map_err(|e| Error::store(e.to_string()))
            })
            .unwrap();
        assert_eq!(v, "mono");
    }

    #[test]
    fn la_maintenance_ne_casse_rien() {
        let s = Store::in_memory().unwrap();
        s.maintenance().unwrap();
        assert_eq!(s.schema_version().unwrap(), CURRENT_VERSION);
    }
}
