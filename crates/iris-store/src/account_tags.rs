//! Les tags des adresses : des étiquettes posées sur ses propres boîtes.

use crate::Store;
use iris_types::{AccountId, Error, Result, Timestamp};
use rusqlite::{params, OptionalExtension};
use std::collections::BTreeMap;

/// Un tag, et combien de boîtes le portent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountTag {
    pub id: i64,
    pub name: String,
    /// `#rrggbb`.
    pub color: String,
    pub accounts: u32,
}

fn err(quoi: &str) -> impl Fn(rusqlite::Error) -> Error + '_ {
    move |e| Error::store(format!("{quoi} : {e}"))
}

/// Un nom de tag utilisable : pas vide, pas interminable.
fn nom_valide(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Config("A tag needs a name.".into()));
    }
    if name.chars().count() > 40 {
        return Err(Error::Config("That name is too long for a tag.".into()));
    }
    Ok(name)
}

impl Store {
    /// Tous les tags, dans l'ordre où l'utilisateur les a rangés.
    pub fn account_tags(&self) -> Result<Vec<AccountTag>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT t.id, t.name, t.color, COUNT(l.account_id) FROM account_tags t \
                     LEFT JOIN account_tag_links l ON l.tag_id = t.id \
                     GROUP BY t.id ORDER BY t.position, t.name COLLATE NOCASE",
                )
                .map_err(err("lecture des tags"))?;
            let lignes = stmt
                .query_map([], |r| {
                    Ok(AccountTag {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        color: r.get(2)?,
                        accounts: r.get(3)?,
                    })
                })
                .map_err(err("lecture des tags"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("lecture des tags"));
            lignes
        })
    }

    /// Crée un tag. Un nom déjà pris — à la casse près — est refusé en le disant.
    pub fn create_account_tag(&self, name: &str, color: &str, now: Timestamp) -> Result<i64> {
        let name = nom_valide(name)?;
        self.with_conn(|c| {
            let existe: Option<i64> = c
                .query_row(
                    "SELECT id FROM account_tags WHERE name = ?1 COLLATE NOCASE",
                    [name],
                    |r| r.get(0),
                )
                .optional()
                .map_err(err("recherche d'un tag"))?;
            if existe.is_some() {
                return Err(Error::Config(format!("There is already a tag “{name}”.")));
            }
            // A new tag goes last: the order is the user's, and a new one has no place
            // in it yet.
            c.execute(
                "INSERT INTO account_tags (name, color, position, created_at) VALUES \
                 (?1, ?2, (SELECT COALESCE(MAX(position), 0) + 1 FROM account_tags), ?3)",
                params![name, color, now.millis()],
            )
            .map_err(err("création d'un tag"))?;
            Ok(c.last_insert_rowid())
        })
    }

    /// Moves a tag to `index` in the order (0 is first); the others close up behind it.
    pub fn move_account_tag(&self, id: i64, index: usize) -> Result<()> {
        self.with_tx(|tx| {
            let mut ordre: Vec<i64> = {
                let mut stmt = tx
                    .prepare("SELECT id FROM account_tags ORDER BY position, name COLLATE NOCASE")
                    .map_err(err("ordre des tags"))?;
                let ids = stmt
                    .query_map([], |r| r.get(0))
                    .map_err(err("ordre des tags"))?
                    .collect::<rusqlite::Result<Vec<i64>>>()
                    .map_err(err("ordre des tags"))?;
                ids
            };
            let Some(depuis) = ordre.iter().position(|t| *t == id) else {
                return Ok(());
            };
            let tag = ordre.remove(depuis);
            ordre.insert(index.min(ordre.len()), tag);
            for (i, t) in ordre.iter().enumerate() {
                tx.execute(
                    "UPDATE account_tags SET position = ?2 WHERE id = ?1",
                    params![t, i as i64 + 1],
                )
                .map_err(err("ordre des tags"))?;
            }
            Ok(())
        })
    }

    /// Renomme un tag, sous les mêmes conditions qu'à la création.
    pub fn rename_account_tag(&self, id: i64, name: &str) -> Result<()> {
        let name = nom_valide(name)?;
        self.with_conn(|c| {
            let autre: Option<i64> = c
                .query_row(
                    "SELECT id FROM account_tags WHERE name = ?1 COLLATE NOCASE AND id != ?2",
                    params![name, id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(err("recherche d'un tag"))?;
            if autre.is_some() {
                return Err(Error::Config(format!("There is already a tag “{name}”.")));
            }
            c.execute(
                "UPDATE account_tags SET name = ?2 WHERE id = ?1",
                params![id, name],
            )
            .map(|_| ())
            .map_err(err("renommage d'un tag"))
        })
    }

    pub fn set_account_tag_color(&self, id: i64, color: &str) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE account_tags SET color = ?2 WHERE id = ?1",
                params![id, color],
            )
            .map(|_| ())
            .map_err(err("couleur d'un tag"))
        })
    }

    /// Supprime un tag. Les boîtes restent ; elles perdent l'étiquette.
    pub fn delete_account_tag(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM account_tags WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(err("suppression d'un tag"))
        })
    }

    /// Pose ou retire un tag sur une boîte.
    pub fn set_account_tagged(&self, account: AccountId, tag: i64, on: bool) -> Result<()> {
        self.with_conn(|c| {
            if on {
                c.execute(
                    "INSERT OR IGNORE INTO account_tag_links (account_id, tag_id) VALUES (?1, ?2)",
                    params![account.get(), tag],
                )
            } else {
                c.execute(
                    "DELETE FROM account_tag_links WHERE account_id = ?1 AND tag_id = ?2",
                    params![account.get(), tag],
                )
            }
            .map(|_| ())
            .map_err(err("tag d'une boîte"))
        })
    }

    /// Les tags de chaque boîte qui en porte.
    pub fn account_tag_links(&self) -> Result<BTreeMap<AccountId, Vec<i64>>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare("SELECT account_id, tag_id FROM account_tag_links")
                .map_err(err("liens des tags"))?;
            let mut liens: BTreeMap<AccountId, Vec<i64>> = BTreeMap::new();
            let lignes = stmt
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
                .map_err(err("liens des tags"))?;
            for ligne in lignes {
                let (compte, tag) = ligne.map_err(err("liens des tags"))?;
                liens.entry(AccountId(compte)).or_default().push(tag);
            }
            Ok(liens)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NewAccount;

    fn t() -> Timestamp {
        Timestamp::from_millis(1)
    }

    #[test]
    fn a_tag_name_is_unique_whatever_the_case() {
        let s = Store::in_memory().unwrap();
        s.create_account_tag("Clients", "#4fb286", t()).unwrap();
        let e = s
            .create_account_tag("  clients ", "#000000", t())
            .unwrap_err();
        assert!(e.to_string().contains("already"), "{e}");
        assert!(s.create_account_tag("   ", "#000000", t()).is_err());
        let perso = s.create_account_tag("Perso", "#e0795b", t()).unwrap();
        assert!(s.rename_account_tag(perso, "CLIENTS").is_err());
        s.rename_account_tag(perso, "Famille").unwrap();
        let noms: Vec<String> = s
            .account_tags()
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(noms, ["Clients", "Famille"]);
    }

    #[test]
    fn tags_keep_the_order_they_are_dragged_into() {
        let s = Store::in_memory().unwrap();
        let a = s.create_account_tag("Alpha", "#4fb286", t()).unwrap();
        let b = s.create_account_tag("Beta", "#4fb286", t()).unwrap();
        let c = s.create_account_tag("Gamma", "#4fb286", t()).unwrap();
        let ordre = |s: &Store| -> Vec<i64> {
            s.account_tags()
                .unwrap()
                .into_iter()
                .map(|t| t.id)
                .collect()
        };
        assert_eq!(ordre(&s), [a, b, c], "a new tag goes last");
        s.move_account_tag(c, 0).unwrap();
        assert_eq!(ordre(&s), [c, a, b]);
        s.move_account_tag(c, 9).unwrap();
        assert_eq!(ordre(&s), [a, b, c], "past the end: last");
        s.move_account_tag(a, 1).unwrap();
        assert_eq!(ordre(&s), [b, a, c]);
    }

    #[test]
    fn tags_are_set_counted_and_leave_with_their_account() {
        let s = Store::in_memory().unwrap();
        let a = s
            .create_account(
                &NewAccount::new("a@example.com", "imap.example.com", "smtp.example.com"),
                t(),
            )
            .unwrap();
        let b = s
            .create_account(
                &NewAccount::new("b@example.com", "imap.example.com", "smtp.example.com"),
                t(),
            )
            .unwrap();
        let clients = s.create_account_tag("Clients", "#4fb286", t()).unwrap();
        s.set_account_tagged(a, clients, true).unwrap();
        s.set_account_tagged(a, clients, true).unwrap();
        s.set_account_tagged(b, clients, true).unwrap();
        assert_eq!(s.account_tags().unwrap()[0].accounts, 2);

        s.set_account_tagged(b, clients, false).unwrap();
        assert_eq!(s.account_tag_links().unwrap().get(&a), Some(&vec![clients]));
        assert!(!s.account_tag_links().unwrap().contains_key(&b));

        s.delete_account_tag(clients).unwrap();
        assert!(
            s.account_tag_links().unwrap().is_empty(),
            "the accounts stay, untagged"
        );
    }
}
