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

use iris_imap::ImapConnection;
use iris_store::{PendingOp, Store};
use iris_types::{Flags, Result, Timestamp};

/// La charge utile d'une opération journalisée.
///
/// Définie dans le magasin, avec le journal qu'elle décrit. Elle vivait ici, et
/// `iris-workflow` — qui ne peut pas dépendre de cette caisse — écrivait la sienne à
/// la main avec `format!`. Les deux ont divergé sur un nom de champ, et chaque
/// archivage a été abandonné au rejeu, en silence, pendant des mois. Un seul type
/// partagé par les deux côtés rend ce bogue impossible à réécrire.
pub use iris_store::OpPayload;

/// Résultat du rejeu d'un lot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReplayReport {
    pub applied: usize,
    pub failed: usize,
    /// Opérations abandonnées parce qu'elles ne pourront jamais réussir.
    pub dropped: usize,
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
            if let Err(e) = conn.select(charge.folder()).await {
                store.fail_op(op.id, &e.to_string(), now)?;
                rapport.failed += 1;
                break;
            }
            dossier_courant = Some(charge.folder().to_string());
        }

        match apply(conn, &charge).await {
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
            Err(e) => {
                // Erreur définitive : le dossier cible n'existe plus, le message a
                // disparu. Réessayer indéfiniment bloquerait tout le compte.
                tracing::warn!(op = %op.id, error = %e, "opération abandonnée");
                store.complete_op(op.id)?;
                rapport.dropped += 1;
            }
        }
    }

    Ok(rapport)
}

async fn apply(conn: &mut dyn ImapConnection, charge: &OpPayload) -> Result<()> {
    match charge {
        OpPayload::SetFlags {
            uids, flags, add, ..
        } => conn.store_flags(uids, Flags(*flags), *add).await,
        OpPayload::Move { uids, target, .. } => conn.move_messages(uids, target).await,
        OpPayload::Delete { uids, .. } => {
            // Marquer supprimé plutôt que purger : la corbeille du serveur est le
            // seul filet de sécurité de l'utilisateur.
            conn.store_flags(uids, Flags::DELETED, true).await
        }
        OpPayload::CreateFolder { folder } => conn.create_folder(folder).await,
        OpPayload::RenameFolder { folder, target } => conn.rename_folder(folder, target).await,
        OpPayload::DeleteFolder { folder } => conn.delete_folder(folder).await,
    }
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
    async fn une_operation_acquittee_n_est_pas_reintroduite_par_sa_clef() {
        // C'est la protection inverse : apres succes, la clef reste connue le temps
        // qu'un rejeu tardif ne renvoie pas ce qui est deja parti.
        let f = fixture();
        let charge = OpPayload::SetFlags {
            folder: "INBOX".into(),
            uids: vec![1],
            flags: Flags::SEEN.0,
            add: true,
        };
        enqueue(&f.store, f.account, &charge, t(0)).unwrap();
        assert_eq!(f.rejouer().await.applied, 1);

        enqueue(&f.store, f.account, &charge, t(50)).unwrap();
        assert_eq!(
            f.store.pending_op_count().unwrap(),
            0,
            "la clef deja acquittee ne doit pas reintroduire l'operation"
        );
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
    async fn une_suppression_marque_sans_purger() {
        // La corbeille du serveur est le seul filet de sécurité de l'utilisateur.
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

        f.rejouer().await;
        assert_eq!(
            f.server.message_count("INBOX"),
            3,
            "le message existe toujours"
        );

        let mut c = f.conn().await;
        c.select("INBOX").await.unwrap();
        let messages = c.fetch_envelopes(UidRange::ALL).await.unwrap();
        assert!(messages[0].flags.contains(Flags::DELETED));
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
