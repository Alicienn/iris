//! Le rejeu du journal d'opérations.
//!
//! Invariant n° 3 : toute action locale est instantanée, puis réconciliée. Le rejeu
//! est la seconde moitié, et il obéit à trois règles.
//!
//! **Idempotence.** Une opération peut être rejouée après une coupure survenue entre
//! son exécution et sa confirmation. Poser un drapeau déjà posé ou déplacer un
//! message déjà déplacé doit réussir silencieusement.
//!
//! **Ordre par compte.** Déplacer puis supprimer n'a pas le même effet que l'inverse.
//! Les opérations d'un même compte sont donc rejouées dans l'ordre, et un échec
//! bloque la file de ce compte — mais d'aucun autre.
//!
//! **Arbitrage.** Quand le local et le distant divergent, le serveur l'emporte sur
//! les drapeaux — il est la référence, d'autres clients y écrivent — et le local
//! l'emporte sur l'état de workflow, qui n'existe que chez nous.

use iris_imap::{ImapConnection, UidRange};
use iris_store::{PendingOp, Store};
use iris_types::{Error, Flags, Result, Timestamp};

/// La charge utile d'une opération journalisée.
///
/// Définie dans le magasin, avec le journal qu'elle décrit. Elle vivait ici, et
/// `iris-workflow` — qui ne peut pas dépendre de cette caisse — écrivait la sienne à
/// la main avec `format!`. Les deux ont divergé sur un nom de champ, et chaque
/// archivage a été abandonné au rejeu, en silence, pendant des mois. Un seul type
/// partagé par les deux côtés rend ce bogue impossible à réécrire.
pub use iris_store::OpPayload;

/// Résultat du rejeu d'un lot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplayReport {
    pub applied: usize,
    pub failed: usize,
    /// Opérations abandonnées parce qu'elles ne pourront jamais réussir.
    pub dropped: usize,
    /// Why the server refused the operations given up after their last try: said to
    /// the user, who saw them done here.
    pub refused: Vec<String>,
}

/// How many times an operation the server refuses is tried before it is given up:
/// with the journal's back-off, about twenty minutes. A refusal can pass (a full
/// mailbox emptied, Office 365's "not connected" between two of its servers), and
/// giving up at the first one lost up to a hundred archivings and deletions at once.
pub const MAX_ATTEMPTS: u32 = 10;

/// The server says the folder is not there: no retry can help.
fn says_gone(e: &Error) -> bool {
    let dit = e.to_string().to_ascii_uppercase();
    dit.contains("NONEXISTENT") || dit.contains("TRYCREATE")
}

/// Rejoue les opérations en attente d'un compte.
///
/// S'arrête au premier échec passager : les suivantes portent peut-être sur le même
/// message, et les exécuter dans le désordre produirait un état incohérent.
pub async fn replay_account(
    conn: &mut dyn ImapConnection,
    store: &Store,
    ops: &[PendingOp],
    now: Timestamp,
) -> Result<ReplayReport> {
    let mut rapport = ReplayReport::default();
    let mut dossier_courant: Option<String> = None;
    // The folder selected was rebuilt by the server since the local copy last saw it.
    let mut uid_perimes = false;
    // Its validity as the server gives it now.
    let mut validite_serveur = 0u32;

    for op in ops {
        let charge = match OpPayload::parse(&op.payload) {
            Ok(c) => c,
            Err(e) => {
                // Une charge illisible ne deviendra jamais lisible : la garder
                // bloquerait la file du compte pour toujours.
                tracing::error!(op = %op.id, error = %e, "opération abandonnée");
                store.complete_op(op.id)?;
                rapport.dropped += 1;
                continue;
            }
        };

        // On ne sélectionne le dossier que lorsqu'il change : une sélection IMAP
        // coûte un aller-retour complet.
        if charge.needs_selection() && dossier_courant.as_deref() != Some(charge.folder()) {
            match conn.select(charge.folder()).await {
                Ok(selection) => {
                    dossier_courant = Some(charge.folder().to_string());
                    let connue = store
                        .folders(op.account)?
                        .into_iter()
                        .find(|f| f.path == charge.folder())
                        .map(|f| f.uid_validity)
                        .unwrap_or(0);
                    uid_perimes = connue != 0
                        && selection.uid_validity != 0
                        && connue != selection.uid_validity;
                    validite_serveur = selection.uid_validity;
                }
                Err(e) if e.is_transient() => {
                    store.fail_op(op.id, &e.to_string(), now)?;
                    rapport.failed += 1;
                    break;
                }
                Err(e) if says_gone(&e) => {
                    // Le dossier n'existe plus (supprimé ailleurs, renommé) : le
                    // retenter à chaque passe bloquait toute la file du compte, et
                    // plus aucune action n'atteignait le serveur.
                    tracing::warn!(op = %op.id, error = %e, "dossier introuvable, opération abandonnée");
                    store.complete_op(op.id)?;
                    rapport.dropped += 1;
                    dossier_courant = None;
                    continue;
                }
                // Any other refusal may pass: tried again later, and given up only
                // after `MAX_ATTEMPTS`, said.
                Err(e) => {
                    refuse(store, op, &e, now, &mut rapport)?;
                    dossier_courant = None;
                    continue;
                }
            }
        }

        // Its UIDs were given by a folder the server has rebuilt since: they would
        // land on other messages (a flag, a move, a deletion on mail nobody chose).
        // Dropped; the next sync reads the folder again. The validity written with the
        // operation decides; the local copy's only for operations written before it
        // was, since a sync may already have stored the new one.
        let reconstruit = if op.uid_validity != 0 {
            validite_serveur != 0 && op.uid_validity != validite_serveur
        } else {
            uid_perimes
        };
        if reconstruit && charge.uses_uids() {
            tracing::warn!(op = %op.id, folder = %charge.folder(), "folder rebuilt since: operation dropped");
            store.complete_op(op.id)?;
            rapport.dropped += 1;
            continue;
        }

        // Withdrawn by an undo since the batch was read: not to be sent.
        if !store.claim_op(op.id)? {
            continue;
        }
        let resultat = apply(conn, &charge).await;
        if !charge.needs_selection() {
            // Supprimer un dossier en sélectionne d'autres : ne plus rien supposer.
            dossier_courant = None;
        }
        match resultat {
            Ok(()) => {
                store.complete_op(op.id)?;
                rapport.applied += 1;
            }
            Err(e) if e.is_transient() => {
                store.fail_op(op.id, &e.to_string(), now)?;
                rapport.failed += 1;
                // La suite porte peut-être sur les mêmes messages.
                break;
            }
            Err(e) if says_gone(&e) => {
                // Erreur définitive : le dossier cible n'existe plus. Réessayer
                // indéfiniment bloquerait tout le compte.
                tracing::warn!(op = %op.id, error = %e, "opération abandonnée");
                store.complete_op(op.id)?;
                rapport.dropped += 1;
            }
            // Refused for another reason (`BAD` with no code, `[OVERQUOTA]`, a
            // read-only folder…): all were given up at once, silently, although they
            // showed as done here.
            Err(e) => refuse(store, op, &e, now, &mut rapport)?,
        }
    }

    Ok(rapport)
}

/// An operation the server refused: tried again later, given up after
/// `MAX_ATTEMPTS` with its reason kept for the user.
fn refuse(
    store: &Store,
    op: &PendingOp,
    e: &Error,
    now: Timestamp,
    rapport: &mut ReplayReport,
) -> Result<()> {
    let essais = store.fail_op(op.id, &e.to_string(), now)?;
    if essais >= MAX_ATTEMPTS {
        tracing::warn!(op = %op.id, error = %e, attempts = essais, "operation given up");
        store.complete_op(op.id)?;
        rapport.dropped += 1;
        rapport.refused.push(e.to_string());
    } else {
        tracing::info!(op = %op.id, error = %e, attempts = essais, "operation refused, tried again later");
        rapport.failed += 1;
    }
    Ok(())
}

async fn apply(conn: &mut dyn ImapConnection, charge: &OpPayload) -> Result<()> {
    match charge {
        OpPayload::SetFlags {
            uids, flags, add, ..
        } => conn.store_flags(uids, Flags(*flags), *add).await,
        OpPayload::Move { uids, target, .. } => conn.move_messages(uids, target).await,
        OpPayload::SetFlagsByMessageId {
            message_ids,
            flags,
            add,
            ..
        } => {
            let mut uids = Vec::new();
            for id in message_ids {
                uids.extend(conn.find_message_id(id).await?);
            }
            conn.store_flags(&uids, Flags(*flags), *add).await
        }
        OpPayload::MoveByMessageId {
            message_ids,
            target,
            ..
        } => {
            let mut uids = Vec::new();
            for id in message_ids {
                uids.extend(conn.find_message_id(id).await?);
            }
            // Not there any more (moved again, deleted elsewhere): nothing to undo.
            conn.move_messages(&uids, target).await
        }
        OpPayload::Delete { uids, .. } => {
            // Emptying the bin or the junk folder: deleted for good, which is what was
            // asked. Marked deleted only, the messages stayed on the server, nothing
            // was freed, and the next pass, finding the local copy empty, brought
            // them all back.
            conn.store_flags(uids, Flags::DELETED, true).await?;
            conn.expunge(uids).await
        }
        OpPayload::CreateFolder { folder } => conn.create_folder(folder).await,
        OpPayload::RenameFolder { folder, target } => conn.rename_folder(folder, target).await,
        OpPayload::DeleteFolder {
            folder,
            rescue: None,
        } => conn.delete_folder(folder).await,
        OpPayload::DeleteFolder {
            folder,
            rescue: Some(refuge),
        } => delete_emptied_folder(conn, folder, refuge).await,
    }
}

/// Supprime un dossier après avoir déplacé vers `refuge` tout ce que le **serveur** y
/// tient, et seulement s'il est vide ensuite.
///
/// `DELETE` détruit le contenu. Déplacer ce que la copie locale connaissait laissait
/// détruire le reste : ce qu'un filtre y avait livré depuis la dernière
/// synchronisation, ce qu'une première synchronisation n'avait pas encore lu, ce qu'un
/// déplacement refusé (quota) n'avait pas emporté.
async fn delete_emptied_folder(
    conn: &mut dyn ImapConnection,
    folder: &str,
    refuge: &str,
) -> Result<()> {
    let etat = conn.select(folder).await?;
    if etat.exists > 0 {
        let uids = conn.existing_uids(UidRange::ALL).await?;
        for lot in uids.chunks(500) {
            conn.move_messages(lot, refuge).await?;
        }
        // Ce qui y serait encore serait détruit avec lui : le dossier est gardé.
        let reste = conn.select(folder).await?;
        if reste.exists > 0 {
            return Err(Error::Protocol {
                protocol: "IMAP",
                message: format!(
                    "« {folder} » still holds {} message(s) after moving its mail: kept",
                    reste.exists
                ),
            });
        }
    }
    // Un dossier qu'on tient ouvert se supprime mal : on en sort d'abord.
    conn.select(refuge).await?;
    conn.delete_folder(folder).await
}

/// Journalise une opération pour rejeu ultérieur.
pub fn enqueue(
    store: &Store,
    account: iris_types::AccountId,
    payload: &OpPayload,
    now: Timestamp,
) -> Result<iris_types::OpId> {
    store.enqueue_op(
        account,
        payload.kind(),
        &payload.to_json(),
        &payload.idempotency_key(account),
        now,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_imap::fake::FakeServer;
    use iris_imap::{Connector, Credentials, Endpoint, FolderKind, UidRange};
    use iris_store::OpKind;
    use iris_store::{FolderRole, NewAccount};
    use iris_types::AccountId;

    fn t(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    struct Fixture {
        store: Store,
        server: FakeServer,
        account: AccountId,
    }

    fn fixture() -> Fixture {
        let store = Store::in_memory().unwrap();
        let account = store
            .create_account(&NewAccount::new("a@x.fr", "imap.x.fr", "s"), t(0))
            .unwrap();
        store
            .upsert_folder(account, "INBOX", FolderRole::Inbox)
            .unwrap();

        let server = FakeServer::default();
        server.add_folder("Archive", FolderKind::Archive);
        for i in 0..3 {
            server.deliver(
                "INBOX",
                format!("Subject: m{i}\r\nMessage-ID: <m{i}@x>\r\n\r\nCorps.\r\n").as_bytes(),
                Flags::NONE,
            );
        }
        Fixture {
            store,
            server,
            account,
        }
    }

    impl Fixture {
        async fn conn(&self) -> Box<dyn ImapConnection> {
            self.server
                .connect(
                    &Endpoint::tls("imap.x.fr", 993),
                    &Credentials::Password {
                        user: "a@x.fr".into(),
                        password: "p".into(),
                    },
                )
                .await
                .unwrap()
        }

        async fn rejouer(&self) -> ReplayReport {
            let ops = self.store.pending_ops(t(1_000_000), 100).unwrap();
            let mut c = self.conn().await;
            replay_account(c.as_mut(), &self.store, &ops, t(1_000_000))
                .await
                .unwrap()
        }
    }

    #[tokio::test]
    async fn une_pose_de_drapeau_est_appliquee_puis_acquittee() {
        let f = fixture();
        let charge = OpPayload::SetFlags {
            folder: "INBOX".into(),
            uids: vec![1],
            flags: Flags::SEEN.0,
            add: true,
        };
        enqueue(&f.store, f.account, &charge, t(0)).unwrap();

        let r = f.rejouer().await;
        assert_eq!(r.applied, 1);
        assert_eq!(f.store.pending_op_count().unwrap(), 0);

        let mut c = f.conn().await;
        c.select("INBOX").await.unwrap();
        let messages = c.fetch_envelopes(UidRange::ALL).await.unwrap();
        assert!(messages[0].flags.contains(Flags::SEEN));
    }

    #[tokio::test]
    async fn uids_of_a_folder_rebuilt_since_touch_nothing_even_after_a_sync() {
        let f = fixture();
        let inbox = |s: &Store| {
            s.folders(f.account)
                .unwrap()
                .into_iter()
                .find(|d| d.path == "INBOX")
                .unwrap()
                .id
        };
        f.store
            .update_folder_sync_state(inbox(&f.store), 1, 0, 0)
            .unwrap();
        let charge = OpPayload::Move {
            folder: "INBOX".into(),
            uids: vec![1],
            target: "Archive".into(),
        };
        enqueue(&f.store, f.account, &charge, t(0)).unwrap();

        // The server rebuilds the folder, and a sync stores the new validity before
        // the move is replayed: the two agree, the operation's own does not.
        f.server.bump_uid_validity("INBOX");
        f.store
            .update_folder_sync_state(inbox(&f.store), 2, 0, 0)
            .unwrap();

        let r = f.rejouer().await;
        assert_eq!(r.dropped, 1);
        assert_eq!(f.server.message_count("Archive"), 0);
    }

    #[tokio::test]
    async fn a_refusal_is_tried_again_then_given_up_and_said() {
        let f = fixture();
        let charge = OpPayload::Move {
            folder: "INBOX".into(),
            uids: vec![1],
            target: "Archive".into(),
        };
        enqueue(&f.store, f.account, &charge, t(0)).unwrap();

        // Office 365's "User is authenticated but not connected": it passes.
        f.server
            .refuse_next("BAD User is authenticated but not connected", 1);
        let r = f.rejouer().await;
        assert_eq!((r.dropped, r.failed), (0, 1));
        assert_eq!(f.store.pending_op_count().unwrap(), 1);

        // A refusal that lasts is given up in the end, and its reason kept.
        let mut dernier = ReplayReport::default();
        for essai in 1..=MAX_ATTEMPTS {
            // Past the back-off each time (it reaches five minutes at most).
            let quand = t(1_000_000 * essai as i64 + 10_000_000);
            f.server.refuse_next("NO [OVERQUOTA] Mailbox is full", 1);
            let ops = f.store.pending_ops(quand, 100).unwrap();
            if ops.is_empty() {
                break;
            }
            let mut c = f.conn().await;
            dernier = replay_account(c.as_mut(), &f.store, &ops, quand)
                .await
                .unwrap();
        }
        assert_eq!(dernier.dropped, 1);
        assert!(dernier.refused[0].contains("OVERQUOTA"));
        assert_eq!(f.store.pending_op_count().unwrap(), 0);
    }

    #[tokio::test]
    async fn un_deplacement_est_applique() {
        let f = fixture();
        let charge = OpPayload::Move {
            folder: "INBOX".into(),
            uids: vec![1, 2],
            target: "Archive".into(),
        };
        enqueue(&f.store, f.account, &charge, t(0)).unwrap();

        assert_eq!(f.rejouer().await.applied, 1);
        assert_eq!(f.server.message_count("Archive"), 2);
        assert_eq!(f.server.message_count("INBOX"), 1);
    }

    #[tokio::test]
    async fn la_meme_intention_ne_produit_qu_une_operation() {
        // Marquer deux fois le même message comme lu est une seule opération.
        let f = fixture();
        let charge = OpPayload::SetFlags {
            folder: "INBOX".into(),
            uids: vec![1],
            flags: Flags::SEEN.0,
            add: true,
        };
        enqueue(&f.store, f.account, &charge, t(0)).unwrap();
        enqueue(&f.store, f.account, &charge, t(10)).unwrap();

        assert_eq!(f.store.pending_op_count().unwrap(), 1);
    }

    #[tokio::test]
    async fn l_ordre_des_uid_ne_cree_pas_de_doublon() {
        let f = fixture();
        let a = OpPayload::SetFlags {
            folder: "INBOX".into(),
            uids: vec![1, 2, 3],
            flags: Flags::SEEN.0,
            add: true,
        };
        let b = OpPayload::SetFlags {
            folder: "INBOX".into(),
            uids: vec![3, 1, 2],
            flags: Flags::SEEN.0,
            add: true,
        };
        assert_eq!(a.idempotency_key(f.account), b.idempotency_key(f.account));
    }

    #[tokio::test]
    async fn rejouer_une_operation_deja_appliquee_reussit_silencieusement() {
        // Cas reel : la coupure est survenue entre l'execution cote serveur et la
        // confirmation cote client. L'operation est donc encore en attente, alors
        // que son effet est deja la.
        let f = fixture();
        let charge = OpPayload::SetFlags {
            folder: "INBOX".into(),
            uids: vec![1],
            flags: Flags::SEEN.0,
            add: true,
        };
        enqueue(&f.store, f.account, &charge, t(0)).unwrap();
        f.server.set_flags_remotely("INBOX", 1, Flags::SEEN);

        let r = f.rejouer().await;
        assert_eq!(r.applied, 1);
        assert_eq!(r.failed, 0);
        assert_eq!(f.store.pending_op_count().unwrap(), 0);
    }

    #[tokio::test]
    async fn lu_non_lu_puis_lu_finit_lu_sur_le_serveur() {
        // La clef gardée après succès faisait ignorer le troisième geste : l'écran
        // disait « lu », le serveur gardait « non lu ».
        let f = fixture();
        let lu = OpPayload::SetFlags {
            folder: "INBOX".into(),
            uids: vec![1],
            flags: Flags::SEEN.0,
            add: true,
        };
        let non_lu = OpPayload::SetFlags {
            folder: "INBOX".into(),
            uids: vec![1],
            flags: Flags::SEEN.0,
            add: false,
        };
        enqueue(&f.store, f.account, &lu, t(0)).unwrap();
        assert_eq!(f.rejouer().await.applied, 1);
        enqueue(&f.store, f.account, &non_lu, t(10)).unwrap();
        assert_eq!(f.rejouer().await.applied, 1);
        enqueue(&f.store, f.account, &lu, t(20)).unwrap();
        assert_eq!(f.rejouer().await.applied, 1);

        let mut c = f.conn().await;
        c.select("INBOX").await.unwrap();
        let messages = c.fetch_envelopes(UidRange::ALL).await.unwrap();
        assert!(messages[0].flags.contains(Flags::SEEN));
    }

    #[tokio::test]
    async fn les_operations_sont_rejouees_dans_l_ordre() {
        // Déplacer puis supprimer n'est pas la même chose que l'inverse.
        let f = fixture();
        enqueue(
            &f.store,
            f.account,
            &OpPayload::Move {
                folder: "INBOX".into(),
                uids: vec![1],
                target: "Archive".into(),
            },
            t(0),
        )
        .unwrap();
        enqueue(
            &f.store,
            f.account,
            &OpPayload::SetFlags {
                folder: "Archive".into(),
                uids: vec![1],
                flags: Flags::SEEN.0,
                add: true,
            },
            t(1),
        )
        .unwrap();

        let r = f.rejouer().await;
        assert_eq!(r.applied, 2);
        assert_eq!(f.server.message_count("Archive"), 1);
    }

    #[tokio::test]
    async fn un_echec_passager_arrete_la_file_du_compte() {
        // La suite porte peut-être sur les mêmes messages : les exécuter dans le
        // désordre produirait un état incohérent.
        let f = fixture();
        for uid in 1..=3 {
            enqueue(
                &f.store,
                f.account,
                &OpPayload::SetFlags {
                    folder: "INBOX".into(),
                    uids: vec![uid],
                    flags: Flags::SEEN.0,
                    add: true,
                },
                t(uid as i64),
            )
            .unwrap();
        }

        f.server.refuse_connections(false);
        let ops = f.store.pending_ops(t(1_000_000), 100).unwrap();
        let mut c = f.conn().await;
        // La première commande échoue.
        f.server.fail_next("serveur occupé");
        let r = replay_account(c.as_mut(), &f.store, &ops, t(1_000_000))
            .await
            .unwrap();

        assert_eq!(r.applied, 0);
        assert_eq!(r.failed, 1);
        assert_eq!(
            f.store.pending_op_count().unwrap(),
            3,
            "rien n'est acquitté"
        );
    }

    #[tokio::test]
    async fn une_operation_definitivement_impossible_est_abandonnee() {
        // Le dossier cible n'existe plus : réessayer indéfiniment bloquerait tout le
        // compte.
        let f = fixture();
        enqueue(
            &f.store,
            f.account,
            &OpPayload::Move {
                folder: "INBOX".into(),
                uids: vec![1],
                target: "Dossier disparu".into(),
            },
            t(0),
        )
        .unwrap();

        let r = f.rejouer().await;
        assert_eq!(r.dropped, 1);
        assert_eq!(f.store.pending_op_count().unwrap(), 0);
    }

    #[tokio::test]
    async fn un_dossier_disparu_ne_bloque_pas_la_file() {
        // Le dossier a été supprimé ou renommé ailleurs : l'opération ne réussira
        // jamais, et la retenter à chaque passe gelait toutes les suivantes.
        let f = fixture();
        enqueue(
            &f.store,
            f.account,
            &OpPayload::SetFlags {
                folder: "Devis".into(),
                uids: vec![1],
                flags: Flags::SEEN.0,
                add: true,
            },
            t(0),
        )
        .unwrap();
        enqueue(
            &f.store,
            f.account,
            &OpPayload::SetFlags {
                folder: "INBOX".into(),
                uids: vec![1],
                flags: Flags::SEEN.0,
                add: true,
            },
            t(1),
        )
        .unwrap();

        let r = f.rejouer().await;
        assert_eq!(r.dropped, 1);
        assert_eq!(r.applied, 1, "l'action suivante doit atteindre le serveur");
        assert_eq!(f.store.pending_op_count().unwrap(), 0);
    }

    #[tokio::test]
    async fn supprimer_un_dossier_sauve_ce_que_la_copie_locale_ignore() {
        // Deux messages livrés dans « Devis » depuis la dernière synchronisation : la
        // copie locale n'en sait rien. Ils étaient détruits avec le dossier.
        let f = fixture();
        f.server.add_folder("Devis", FolderKind::Other);
        for i in 0..2 {
            f.server.deliver(
                "Devis",
                format!("Subject: d{i}\r\nMessage-ID: <d{i}@x>\r\n\r\nDevis.\r\n").as_bytes(),
                Flags::NONE,
            );
        }
        enqueue(
            &f.store,
            f.account,
            &OpPayload::DeleteFolder {
                folder: "Devis".into(),
                rescue: Some("INBOX".into()),
            },
            t(0),
        )
        .unwrap();

        assert_eq!(f.rejouer().await.applied, 1);
        assert_eq!(f.server.message_count("INBOX"), 5, "rien n'est perdu");
        let mut c = f.conn().await;
        let restants: Vec<_> = c
            .list_folders()
            .await
            .unwrap()
            .into_iter()
            .map(|d| d.path)
            .collect();
        assert!(!restants.contains(&"Devis".to_string()));
    }

    #[tokio::test]
    async fn une_charge_illisible_est_abandonnee_et_ne_bloque_rien() {
        let f = fixture();
        f.store
            .enqueue_op(
                f.account,
                OpKind::SetFlags,
                "pas du json",
                "clef-cassee",
                t(0),
            )
            .unwrap();
        enqueue(
            &f.store,
            f.account,
            &OpPayload::SetFlags {
                folder: "INBOX".into(),
                uids: vec![1],
                flags: Flags::SEEN.0,
                add: true,
            },
            t(1),
        )
        .unwrap();

        let r = f.rejouer().await;
        assert_eq!(r.dropped, 1);
        assert_eq!(
            r.applied, 1,
            "l'opération valide doit passer malgré la précédente"
        );
    }

    #[tokio::test]
    async fn emptying_the_bin_deletes_for_good() {
        // Only used to empty the bin and the junk folder. Marked deleted and left
        // there, the messages came back at the next pass.
        let f = fixture();
        enqueue(
            &f.store,
            f.account,
            &OpPayload::Delete {
                folder: "INBOX".into(),
                uids: vec![1],
            },
            t(0),
        )
        .unwrap();

        assert_eq!(f.rejouer().await.applied, 1);
        assert_eq!(f.server.message_count("INBOX"), 2, "gone from the server");
    }

    #[test]
    fn une_charge_survit_a_un_aller_retour_json() {
        let charge = OpPayload::Move {
            folder: "INBOX".into(),
            uids: vec![1, 2],
            target: "Archive".into(),
        };
        assert_eq!(OpPayload::parse(&charge.to_json()).unwrap(), charge);
    }

    #[test]
    fn les_natures_correspondent_aux_charges() {
        assert_eq!(
            OpPayload::Delete {
                folder: "x".into(),
                uids: vec![]
            }
            .kind(),
            OpKind::DeleteMessage
        );
        assert_eq!(
            OpPayload::Move {
                folder: "x".into(),
                uids: vec![],
                target: "y".into()
            }
            .kind(),
            OpKind::MoveMessage
        );
    }

    #[test]
    fn des_intentions_differentes_ont_des_clefs_differentes() {
        let a = AccountId(1);
        let poser = OpPayload::SetFlags {
            folder: "INBOX".into(),
            uids: vec![1],
            flags: Flags::SEEN.0,
            add: true,
        };
        let retirer = OpPayload::SetFlags {
            folder: "INBOX".into(),
            uids: vec![1],
            flags: Flags::SEEN.0,
            add: false,
        };
        assert_ne!(poser.idempotency_key(a), retirer.idempotency_key(a));

        // Deux comptes distincts ne partagent jamais une clef.
        assert_ne!(
            poser.idempotency_key(AccountId(1)),
            poser.idempotency_key(AccountId(2))
        );
    }
}
