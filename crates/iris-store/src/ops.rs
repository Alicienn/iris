//! Ce qu'une opération journalisée contient.
//!
//! Ce type vit dans le magasin et non dans la couche de synchronisation, alors que
//! c'est elle qui le rejoue. La raison est un bogue qu'il rend désormais impossible.
//!
//! La charge utile était écrite d'un côté à la main, par `format!`, et relue de
//! l'autre par `serde`. Les deux formes ont divergé sur **un mot** : l'écrivain posait
//! `"to"`, le lecteur attendait `"target"`. Le résultat n'était pas une erreur bruyante
//! mais un silence — chaque opération était jugée illisible au rejeu, abandonnée, et
//! le compteur d'opérations en attente retombait à zéro. Localement le fil quittait la
//! file ; sur le serveur, **rien n'a jamais bougé**. Des mois d'archivages et de
//! suppressions perdus, sans un message d'erreur, parce que deux fichiers n'étaient
//! pas d'accord sur un nom de champ.
//!
//! Le journal appartient au magasin. Le type qui décrit ce qu'on y met aussi. Les deux
//! caisses qui l'écrivent et le lisent le partagent maintenant, et le compilateur
//! répond à leur place.

use iris_types::{AccountId, Error, Result};
use serde::{Deserialize, Serialize};

/// La charge utile d'une opération journalisée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OpPayload {
    SetFlags {
        folder: String,
        uids: Vec<u32>,
        flags: u32,
        add: bool,
    },
    Move {
        folder: String,
        uids: Vec<u32>,
        target: String,
    },
    Delete {
        folder: String,
        uids: Vec<u32>,
    },
    /// Créer un dossier sur ce compte.
    ///
    /// Passe par le journal comme tout le reste : créer un dossier sur cent boîtes est
    /// cent allers-retours réseau, et faire attendre l'utilisateur devant eux
    /// contredirait l'invariant n° 3.
    CreateFolder {
        folder: String,
    },
}

impl OpPayload {
    pub fn kind(&self) -> crate::OpKind {
        match self {
            Self::SetFlags { .. } => crate::OpKind::SetFlags,
            Self::Move { .. } => crate::OpKind::MoveMessage,
            Self::Delete { .. } => crate::OpKind::DeleteMessage,
            Self::CreateFolder { .. } => crate::OpKind::CreateFolder,
        }
    }

    /// Clé d'idempotence : deux fois la même intention ne produit qu'une entrée.
    ///
    /// Elle décrit l'**effet visé**, pas l'instant : marquer deux fois le même message
    /// comme lu est une seule opération.
    pub fn idempotency_key(&self, account: AccountId) -> String {
        match self {
            Self::SetFlags {
                folder,
                uids,
                flags,
                add,
            } => format!("{account}:flags:{folder}:{}:{flags}:{add}", join(uids)),
            Self::Move {
                folder,
                uids,
                target,
            } => format!("{account}:move:{folder}:{}:{target}", join(uids)),
            Self::Delete { folder, uids } => {
                format!("{account}:delete:{folder}:{}", join(uids))
            }
            Self::CreateFolder { folder } => format!("{account}:mkdir:{folder}"),
        }
    }

    pub fn to_json(&self) -> String {
        // Un type dont chaque variante est sérialisable ne peut pas échouer ici. Le
        // repli existe pour ne pas paniquer sur une invariante que le compilateur
        // tient déjà.
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    pub fn parse(json: &str) -> Result<Self> {
        serde_json::from_str(json)
            .map_err(|e| Error::store(format!("opération journalisée illisible : {e}")))
    }

    pub fn folder(&self) -> &str {
        match self {
            Self::SetFlags { folder, .. }
            | Self::Move { folder, .. }
            | Self::Delete { folder, .. }
            | Self::CreateFolder { folder } => folder,
        }
    }

    /// L'opération exige-t-elle que son dossier soit sélectionné d'abord ?
    ///
    /// Non pour la création : sélectionner un dossier qui n'existe pas encore échoue,
    /// et échouerait précisément sur celui qu'on vient de demander à créer.
    pub fn needs_selection(&self) -> bool {
        !matches!(self, Self::CreateFolder { .. })
    }
}

fn join(uids: &[u32]) -> String {
    let mut tries = uids.to_vec();
    // L'ordre des UID ne change pas l'effet : le normaliser évite deux entrées pour la
    // même intention.
    tries.sort_unstable();
    tries.dedup();
    tries
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deplacement() -> OpPayload {
        OpPayload::Move {
            folder: "INBOX".into(),
            uids: vec![3, 1, 2],
            target: "INBOX.Trash".into(),
        }
    }

    #[test]
    fn ce_qui_est_ecrit_se_relit() {
        // Le test qui manquait. L'écriture se faisait par « format! » dans une caisse
        // et la lecture par « serde » dans une autre ; elles ont divergé sur un mot,
        // et chaque archivage a été abandonné au rejeu pendant des mois, en silence.
        for charge in [
            deplacement(),
            OpPayload::SetFlags {
                folder: "INBOX".into(),
                uids: vec![7],
                flags: 1,
                add: true,
            },
            OpPayload::Delete {
                folder: "INBOX".into(),
                uids: vec![9],
            },
            OpPayload::CreateFolder {
                folder: "INBOX.Devis".into(),
            },
        ] {
            let json = charge.to_json();
            assert_eq!(
                OpPayload::parse(&json).unwrap(),
                charge,
                "l'aller-retour a perdu quelque chose : {json}"
            );
        }
    }

    #[test]
    fn le_champ_de_destination_s_appelle_target() {
        // Nommé explicitement dans un test parce que c'est le nom sur lequel les deux
        // côtés s'étaient trompés. Le renommer casse ce test avant de casser le rejeu.
        let json = deplacement().to_json();
        assert!(json.contains("\"target\""), "obtenu : {json}");
    }

    #[test]
    fn la_cle_ignore_l_ordre_des_uid() {
        let a = OpPayload::Move {
            folder: "INBOX".into(),
            uids: vec![1, 2, 3],
            target: "T".into(),
        };
        let b = OpPayload::Move {
            folder: "INBOX".into(),
            uids: vec![3, 2, 1],
            target: "T".into(),
        };
        assert_eq!(
            a.idempotency_key(AccountId(1)),
            b.idempotency_key(AccountId(1))
        );
    }

    #[test]
    fn deux_intentions_differentes_ont_deux_cles() {
        let vers_corbeille = deplacement();
        let vers_archive = OpPayload::Move {
            folder: "INBOX".into(),
            uids: vec![1, 2, 3],
            target: "INBOX.Archive".into(),
        };
        assert_ne!(
            vers_corbeille.idempotency_key(AccountId(1)),
            vers_archive.idempotency_key(AccountId(1))
        );
    }

    #[test]
    fn creer_un_dossier_ne_le_selectionne_pas() {
        assert!(!OpPayload::CreateFolder {
            folder: "X".into()
        }
        .needs_selection());
        assert!(deplacement().needs_selection());
    }

    #[test]
    fn une_charge_illisible_le_dit() {
        assert!(OpPayload::parse("{\"op\":\"inconnue\"}").is_err());
        assert!(OpPayload::parse("pas du json").is_err());
    }
}
