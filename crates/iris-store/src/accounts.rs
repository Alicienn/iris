//! Comptes et dossiers.

use crate::model::{Account, AuthKind, Folder, FolderRole, NewAccount};
use crate::{sql_err, Store};
use iris_types::{AccountId, Error, FolderId, Result, Timestamp};
use rusqlite::{params, Row};

fn account_from_row(r: &Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
        id: AccountId(r.get("id")?),
        email: r.get("email")?,
        display_name: r.get("display_name")?,
        imap_host: r.get("imap_host")?,
        imap_port: r.get::<_, i64>("imap_port")? as u16,
        imap_tls: r.get::<_, i64>("imap_tls")? != 0,
        smtp_host: r.get("smtp_host")?,
        smtp_port: r.get::<_, i64>("smtp_port")? as u16,
        smtp_tls: r.get::<_, i64>("smtp_tls")? != 0,
        auth: AuthKind::parse(&r.get::<_, String>("auth_kind")?),
        group: r.get("group_name")?,
        pinned: r.get::<_, i64>("pinned")? != 0,
        enabled: r.get::<_, i64>("enabled")? != 0,
        created_at: Timestamp::from_millis(r.get("created_at")?),
        last_activity_at: Timestamp::from_millis(r.get("last_activity_at")?),
        signature: r.get("signature")?,
    })
}

const ACCOUNT_COLUMNS: &str = "id, email, display_name, imap_host, imap_port, imap_tls, \
     smtp_host, smtp_port, smtp_tls, auth_kind, group_name, pinned, enabled, \
     created_at, last_activity_at, signature";

/// Un dossier tel que l'interface le montre : un nom, sur toutes les boîtes à la fois.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnifiedFolder {
    /// Le chemin IMAP, qui est aussi le nom : `INBOX.Devis`.
    pub path: String,
    /// Le rôle, quand les serveurs s'accordent dessus. `min` sur les rôles suffit :
    /// deux serveurs qui rangent le même nom différemment est un cas qui n'arrive pas
    /// pour les dossiers que nous créons, et pour les autres le rôle n'est qu'une
    /// icône.
    pub role: FolderRole,
    /// Sur combien de boîtes il existe. Ce qui dit à l'utilisateur si « Devis » est
    /// partout ou seulement sur trois comptes.
    pub accounts: u32,
    /// Combien de conversations il contient, tous comptes confondus.
    pub threads: u32,
}

/// Ce qu'un compte sait de ses serveurs.
///
/// Groupé plutôt que passé en huit arguments : sept d'entre eux sont des chaînes et
/// des nombres du même type, et une paire échangée entre IMAP et SMTP compile sans
/// bruit et casse la boîte à la synchronisation suivante.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountServers {
    pub email: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_tls: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_tls: bool,
}

impl Store {
    /// Crée un compte. L'adresse est unique : une seconde tentative est refusée
    /// plutôt que de créer un doublon silencieux.
    pub fn create_account(&self, new: &NewAccount, now: Timestamp) -> Result<AccountId> {
        self.with_conn(|c| {
            let email = new.email.trim().to_lowercase();
            if email.is_empty() {
                return Err(Error::Config("adresse vide".into()));
            }
            c.execute(
                "INSERT INTO accounts
                   (email, display_name, imap_host, imap_port, imap_tls,
                    smtp_host, smtp_port, smtp_tls, auth_kind, group_name, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![
                    email,
                    new.display_name,
                    new.imap_host,
                    new.imap_port as i64,
                    new.imap_tls as i64,
                    new.smtp_host,
                    new.smtp_port as i64,
                    new.smtp_tls as i64,
                    new.auth.as_str(),
                    new.group,
                    now.millis(),
                ],
            )
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(f, _)
                    if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    Error::Config(format!("le compte « {email} » existe déjà"))
                }
                other => sql_err("création du compte", other),
            })?;
            Ok(AccountId(c.last_insert_rowid()))
        })
    }

    pub fn account(&self, id: AccountId) -> Result<Option<Account>> {
        self.with_conn(|c| {
            let sql = format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = ?1");
            match c.query_row(&sql, params![id.get()], account_from_row) {
                Ok(a) => Ok(Some(a)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(sql_err("lecture du compte", e)),
            }
        })
    }

    pub fn account_by_email(&self, email: &str) -> Result<Option<Account>> {
        let email = email.trim().to_lowercase();
        self.with_conn(|c| {
            let sql = format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE email = ?1");
            match c.query_row(&sql, params![email], account_from_row) {
                Ok(a) => Ok(Some(a)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(sql_err("lecture du compte", e)),
            }
        })
    }

    /// Tous les comptes, épinglés d'abord, puis par adresse.
    ///
    /// Cet ordre est celui de la barre latérale : avec cent boîtes, l'ordre de
    /// création n'a aucun sens pour l'utilisateur.
    pub fn accounts(&self) -> Result<Vec<Account>> {
        self.with_conn(|c| {
            let sql =
                format!("SELECT {ACCOUNT_COLUMNS} FROM accounts ORDER BY pinned DESC, email ASC");
            let mut stmt = c.prepare(&sql).map_err(|e| sql_err("préparation", e))?;
            let rows = stmt
                .query_map([], account_from_row)
                .map_err(|e| sql_err("liste des comptes", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("liste des comptes", e))
        })
    }

    pub fn set_account_pinned(&self, id: AccountId, pinned: bool) -> Result<()> {
        self.update_account_field(id, "pinned", pinned as i64)
    }

    pub fn set_account_enabled(&self, id: AccountId, enabled: bool) -> Result<()> {
        self.update_account_field(id, "enabled", enabled as i64)
    }

    /// Réécrit l'adresse et les serveurs d'un compte existant.
    ///
    /// Rewriting rather than deleting and recreating: an account carries its folders,
    /// its messages and its workflow states, and a provider changing its server names
    /// must not cost the user their mailbox history. The identifier survives, so
    /// everything that points at it survives too.
    pub fn update_account_servers(&self, id: AccountId, servers: &AccountServers) -> Result<()> {
        self.with_conn(|c| {
            let n = c
                .execute(
                    "UPDATE accounts SET email = ?1, imap_host = ?2, imap_port = ?3, \
                     imap_tls = ?4, smtp_host = ?5, smtp_port = ?6, smtp_tls = ?7 WHERE id = ?8",
                    params![
                        servers.email,
                        servers.imap_host,
                        servers.imap_port as i64,
                        servers.imap_tls as i64,
                        servers.smtp_host,
                        servers.smtp_port as i64,
                        servers.smtp_tls as i64,
                        id.get()
                    ],
                )
                .map_err(|e| sql_err("mise à jour des serveurs", e))?;
            if n == 0 {
                return Err(Error::store(format!("compte {id} introuvable")));
            }
            Ok(())
        })
    }

    /// Les dossiers, vus comme un seul jeu plutôt que comme cent.
    ///
    /// Un dossier « Devis » sur douze boîtes est **un** dossier. C'est la seule vue
    /// qui tienne au-delà de quelques comptes : une arborescence qui répète la même
    /// dizaine de noms pour chaque boîte est une arborescence que personne ne déplie.
    ///
    /// Le décompte est celui des fils, pas des messages : c'est ce que la liste
    /// affiche, et compter les messages annoncerait quarante là où douze lignes
    /// apparaîtront.
    pub fn unified_folders(&self) -> Result<Vec<UnifiedFolder>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached(
                    "SELECT f.path,
                            min(f.role),
                            count(DISTINCT f.account_id),
                            count(DISTINCT m.thread_id)
                     FROM folders f
                     LEFT JOIN messages m ON m.folder_id = f.id
                     GROUP BY f.path
                     ORDER BY f.path",
                )
                .map_err(|e| sql_err("préparation", e))?;

            let rows = stmt
                .query_map([], |r| {
                    Ok(UnifiedFolder {
                        path: r.get(0)?,
                        role: FolderRole::parse(&r.get::<_, String>(1)?),
                        accounts: r.get::<_, i64>(2)? as u32,
                        threads: r.get::<_, i64>(3)? as u32,
                    })
                })
                .map_err(|e| sql_err("liste des dossiers", e))?;

            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("liste des dossiers", e))
        })
    }

    /// Les comptes qui ont ce dossier.
    ///
    /// Le miroir de la fonction ci-dessous : renommer et supprimer s'adressent à ceux
    /// qui l'ont, créer à ceux qui ne l'ont pas.
    pub fn accounts_with_folder(&self, path: &str) -> Result<Vec<AccountId>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached(
                    "SELECT DISTINCT account_id FROM folders WHERE path = ?1 ORDER BY account_id",
                )
                .map_err(|e| sql_err("préparation", e))?;

            let rows = stmt
                .query_map(params![path], |r| Ok(AccountId(r.get(0)?)))
                .map_err(|e| sql_err("comptes ayant le dossier", e))?;

            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("comptes ayant le dossier", e))
        })
    }

    /// Les comptes qui n'ont pas encore ce dossier.
    ///
    /// Ce sont ceux à qui il faut le demander. Créer un dossier veut dire le créer
    /// partout ; le demander à un serveur qui l'a déjà obtient un refus, et un refus
    /// qu'on a provoqué soi-même ressemble à une panne dans le journal.
    pub fn accounts_without_folder(&self, path: &str) -> Result<Vec<AccountId>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached(
                    "SELECT a.id FROM accounts a
                     WHERE a.enabled = 1
                       AND NOT EXISTS (SELECT 1 FROM folders f
                                       WHERE f.account_id = a.id AND f.path = ?1)
                     ORDER BY a.id",
                )
                .map_err(|e| sql_err("préparation", e))?;

            let rows = stmt
                .query_map(params![path], |r| Ok(AccountId(r.get(0)?)))
                .map_err(|e| sql_err("comptes sans le dossier", e))?;

            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("comptes sans le dossier", e))
        })
    }

    pub fn touch_account(&self, id: AccountId, now: Timestamp) -> Result<()> {
        self.update_account_field(id, "last_activity_at", now.millis())
    }

    /// Enregistre la signature d'un compte.
    ///
    /// Sans normalisation : ce que l'utilisateur a écrit est ce qui part. Retirer les
    /// blancs de fin ferait disparaître une ligne vide voulue, et une signature est du
    /// texte que quelqu'un a mis en forme exprès.
    pub fn set_account_signature(&self, id: AccountId, signature: &str) -> Result<()> {
        self.with_conn(|c| {
            let n = c
                .execute(
                    "UPDATE accounts SET signature = ?1 WHERE id = ?2",
                    params![signature, id.get()],
                )
                .map_err(|e| sql_err("mise à jour de la signature", e))?;
            if n == 0 {
                return Err(Error::store(format!("compte {id} introuvable")));
            }
            Ok(())
        })
    }

    fn update_account_field(&self, id: AccountId, column: &'static str, value: i64) -> Result<()> {
        self.with_conn(|c| {
            let n = c
                .execute(
                    &format!("UPDATE accounts SET {column} = ?1 WHERE id = ?2"),
                    params![value, id.get()],
                )
                .map_err(|e| sql_err("mise à jour du compte", e))?;
            if n == 0 {
                return Err(Error::store(format!("compte {id} introuvable")));
            }
            Ok(())
        })
    }

    /// Supprime un compte et, en cascade, ses dossiers et ses messages.
    pub fn delete_account(&self, id: AccountId) -> Result<bool> {
        self.with_conn(|c| {
            let n = c
                .execute("DELETE FROM accounts WHERE id = ?1", params![id.get()])
                .map_err(|e| sql_err("suppression du compte", e))?;
            Ok(n > 0)
        })
    }

    // --- Dossiers ---

    /// Enregistre un dossier, ou renvoie celui qui existe déjà.
    pub fn upsert_folder(
        &self,
        account: AccountId,
        path: &str,
        role: FolderRole,
    ) -> Result<FolderId> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO folders (account_id, path, role) VALUES (?1, ?2, ?3)
                 ON CONFLICT(account_id, path) DO UPDATE SET role = excluded.role",
                params![account.get(), path, role.as_str()],
            )
            .map_err(|e| sql_err("enregistrement du dossier", e))?;

            c.query_row(
                "SELECT id FROM folders WHERE account_id = ?1 AND path = ?2",
                params![account.get(), path],
                |r| r.get::<_, i64>(0),
            )
            .map(FolderId)
            .map_err(|e| sql_err("relecture du dossier", e))
        })
    }

    /// Retire un dossier de la copie locale.
    ///
    /// Ce que l'application faisait en supprimant un dossier, c'était mettre deux
    /// opérations dans le journal — déplacer le courrier vers la boîte de réception,
    /// puis supprimer le dossier — et **rien** localement. Le serveur obéissait ; la
    /// colonne des dossiers, qui lit la copie locale, gardait la ligne pour toujours.
    /// Ensuite chaque synchronisation essayait de sélectionner une boîte qui n'existait
    /// plus, et le journal se remplissait de « Mailbox doesn't exist ».
    ///
    /// Les messages partent avec, par cascade. Ce n'est pas une perte : la
    /// **première** opération de la file les a déplacés vers la boîte de réception du
    /// serveur, et la prochaine synchronisation les redescendra avec les UID que le
    /// serveur leur aura donnés là-bas. Les réaffecter ici en gardant leurs anciens UID
    /// serait pire — la contrainte d'unicité porte sur `(dossier, uid)`, et deux
    /// messages venant de deux dossiers finiraient par se marcher dessus.
    ///
    /// Si l'opération serveur échoue, le dossier réapparaît à la prochaine liste. C'est
    /// le bon comportement : la copie locale est un cache, et le serveur a le dernier
    /// mot dans les deux sens.
    pub fn forget_folder(&self, account: AccountId, path: &str) -> Result<bool> {
        self.with_conn(|c| {
            c.execute(
                "DELETE FROM folders WHERE account_id = ?1 AND path = ?2",
                params![account.get(), path],
            )
            .map(|n| n > 0)
            .map_err(|e| sql_err("suppression du dossier", e))
        })
    }

    pub fn folders(&self, account: AccountId) -> Result<Vec<Folder>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, account_id, path, role, uid_validity, uid_next, highest_modseq
                     FROM folders WHERE account_id = ?1 ORDER BY path",
                )
                .map_err(|e| sql_err("préparation", e))?;
            let rows = stmt
                .query_map(params![account.get()], |r| {
                    Ok(Folder {
                        id: FolderId(r.get(0)?),
                        account: AccountId(r.get(1)?),
                        path: r.get(2)?,
                        role: FolderRole::parse(&r.get::<_, String>(3)?),
                        uid_validity: r.get::<_, i64>(4)? as u32,
                        uid_next: r.get::<_, i64>(5)? as u32,
                        highest_modseq: r.get::<_, i64>(6)? as u64,
                    })
                })
                .map_err(|e| sql_err("liste des dossiers", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("liste des dossiers", e))
        })
    }

    /// Met à jour l'état de synchronisation d'un dossier.
    ///
    /// Retourne `true` si le `UIDVALIDITY` a changé : dans ce cas, tous les UID
    /// connus de ce dossier sont caducs et l'appelant doit resynchroniser
    /// intégralement. C'est le seul moyen sûr de détecter une boîte reconstruite
    /// côté serveur.
    pub fn update_folder_sync_state(
        &self,
        folder: FolderId,
        uid_validity: u32,
        uid_next: u32,
        highest_modseq: u64,
    ) -> Result<bool> {
        self.with_tx(|tx| {
            let previous: i64 = tx
                .query_row(
                    "SELECT uid_validity FROM folders WHERE id = ?1",
                    params![folder.get()],
                    |r| r.get(0),
                )
                .map_err(|e| sql_err("lecture de UIDVALIDITY", e))?;

            tx.execute(
                "UPDATE folders SET uid_validity = ?1, uid_next = ?2, highest_modseq = ?3
                 WHERE id = ?4",
                params![
                    uid_validity as i64,
                    uid_next as i64,
                    highest_modseq as i64,
                    folder.get()
                ],
            )
            .map_err(|e| sql_err("mise à jour du dossier", e))?;

            Ok(previous != 0 && previous != uid_validity as i64)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewAccount;

    fn store() -> Store {
        Store::in_memory().unwrap()
    }

    fn now() -> Timestamp {
        Timestamp::from_millis(1_700_000_000_000)
    }

    #[test]
    fn reecrire_les_serveurs_conserve_l_identifiant() {
        // Un hébergeur qui renomme ses serveurs ne doit pas coûter à l'utilisateur
        // l'historique de sa boîte. Supprimer puis recréer le compte serait plus
        // court à écrire et emporterait ses dossiers, ses messages et leurs états.
        let s = store();
        let id = s
            .create_account(&NewAccount::new("marie@x.fr", "imap.x.fr", "smtp.x.fr"), now())
            .unwrap();

        s.update_account_servers(
            id,
            &AccountServers {
                email: "marie@y.fr".into(),
                imap_host: "mail.y.fr".into(),
                imap_port: 143,
                imap_tls: false,
                smtp_host: "mail.y.fr".into(),
                smtp_port: 587,
                smtp_tls: true,
            },
        )
        .unwrap();

        let relu = s.account(id).unwrap().unwrap();
        assert_eq!(relu.id, id, "l'identifiant survit");
        assert_eq!(relu.email, "marie@y.fr");
        assert_eq!(relu.imap_host, "mail.y.fr");
        assert_eq!(relu.imap_port, 143);
        assert!(!relu.imap_tls);
        assert_eq!(relu.smtp_port, 587);
        assert!(relu.smtp_tls);
    }

    #[test]
    fn reecrire_un_compte_disparu_le_dit() {
        // Écrire zéro ligne et rendre « c'est fait » laisserait l'écran des réglages
        // annoncer une sauvegarde qui n'a rien sauvegardé.
        let s = store();
        let erreur = s.update_account_servers(
            AccountId(4242),
            &AccountServers {
                email: "personne@x.fr".into(),
                imap_host: "i".into(),
                imap_port: 993,
                imap_tls: true,
                smtp_host: "s".into(),
                smtp_port: 465,
                smtp_tls: true,
            },
        );
        assert!(erreur.is_err());
    }

    #[test]
    fn creer_puis_relire_un_compte() {
        let s = store();
        let id = s
            .create_account(
                &NewAccount::new("Moi@Example.COM", "imap.example.com", "smtp.example.com"),
                now(),
            )
            .unwrap();

        let a = s.account(id).unwrap().unwrap();
        assert_eq!(a.email, "moi@example.com", "l'adresse est normalisée");
        assert_eq!(a.imap_port, 993);
        assert!(a.enabled);
        assert!(!a.pinned);
    }

    #[test]
    fn un_compte_absent_donne_none() {
        let s = store();
        assert!(s.account(AccountId(42)).unwrap().is_none());
        assert!(s.account_by_email("inconnu@example.com").unwrap().is_none());
    }

    #[test]
    fn la_meme_adresse_ne_peut_pas_etre_ajoutee_deux_fois() {
        let s = store();
        let a = NewAccount::new("moi@example.com", "i", "s");
        s.create_account(&a, now()).unwrap();
        let e = s.create_account(&a, now()).unwrap_err();
        assert!(e.to_string().contains("existe déjà"));
    }

    #[test]
    fn la_recherche_par_adresse_ignore_la_casse() {
        let s = store();
        s.create_account(&NewAccount::new("moi@example.com", "i", "s"), now())
            .unwrap();
        assert!(s.account_by_email("  MOI@Example.com ").unwrap().is_some());
    }

    #[test]
    fn les_comptes_epingles_arrivent_en_tete() {
        let s = store();
        for e in ["c@x.fr", "a@x.fr", "b@x.fr"] {
            s.create_account(&NewAccount::new(e, "i", "s"), now())
                .unwrap();
        }
        let b = s.account_by_email("b@x.fr").unwrap().unwrap();
        s.set_account_pinned(b.id, true).unwrap();

        let ordre: Vec<_> = s.accounts().unwrap().into_iter().map(|a| a.email).collect();
        assert_eq!(ordre, ["b@x.fr", "a@x.fr", "c@x.fr"]);
    }

    #[test]
    fn modifier_un_compte_inexistant_est_une_erreur() {
        let s = store();
        assert!(s.set_account_pinned(AccountId(999), true).is_err());
    }

    #[test]
    fn supprimer_un_compte_emporte_ses_dossiers() {
        let s = store();
        let id = s
            .create_account(&NewAccount::new("a@x.fr", "i", "s"), now())
            .unwrap();
        s.upsert_folder(id, "INBOX", FolderRole::Inbox).unwrap();
        assert_eq!(s.folders(id).unwrap().len(), 1);

        assert!(s.delete_account(id).unwrap());
        assert!(s.folders(id).unwrap().is_empty());
        assert!(
            !s.delete_account(id).unwrap(),
            "seconde suppression sans effet"
        );
    }

    #[test]
    fn enregistrer_deux_fois_le_meme_dossier_ne_le_duplique_pas() {
        let s = store();
        let a = s
            .create_account(&NewAccount::new("a@x.fr", "i", "s"), now())
            .unwrap();
        let f1 = s.upsert_folder(a, "INBOX", FolderRole::Other).unwrap();
        let f2 = s.upsert_folder(a, "INBOX", FolderRole::Inbox).unwrap();
        assert_eq!(f1, f2);

        let dossiers = s.folders(a).unwrap();
        assert_eq!(dossiers.len(), 1);
        // Le rôle est bien corrigé au passage.
        assert_eq!(dossiers[0].role, FolderRole::Inbox);
    }

    #[test]
    fn un_changement_d_uidvalidity_est_signale() {
        let s = store();
        let a = s
            .create_account(&NewAccount::new("a@x.fr", "i", "s"), now())
            .unwrap();
        let f = s.upsert_folder(a, "INBOX", FolderRole::Inbox).unwrap();

        // Première synchronisation : rien à signaler, il n'y avait pas de valeur.
        assert!(!s.update_folder_sync_state(f, 100, 10, 1).unwrap());
        // Même valeur : toujours rien.
        assert!(!s.update_folder_sync_state(f, 100, 20, 5).unwrap());
        // Le serveur a reconstruit la boîte : tous les UID connus sont caducs.
        assert!(s.update_folder_sync_state(f, 101, 1, 1).unwrap());

        let f0 = &s.folders(a).unwrap()[0];
        assert_eq!(f0.uid_validity, 101);
        assert_eq!(f0.highest_modseq, 1);
    }

    #[test]
    fn une_adresse_vide_est_refusee() {
        let s = store();
        let e = s
            .create_account(&NewAccount::new("   ", "i", "s"), now())
            .unwrap_err();
        assert!(e.to_string().contains("vide"));
    }
}
