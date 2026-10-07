//! La synchronisation d'un dossier.
//!
//! Trois chemins, choisis d'après ce que le serveur annonce et ce que nous savons
//! déjà :
//!
//! - **complet** : première visite, ou `UIDVALIDITY` changé. Tous les en-têtes sont
//!   relus, par tranches ;
//! - **incrémental** : avec `CONDSTORE`, on demande les nouveaux UID et les drapeaux
//!   modifiés depuis le dernier numéro connu. C'est ce qui rend une synchronisation
//!   périodique quasi gratuite ;
//! - **de rattrapage** : sans `CONDSTORE`, on relit les nouveaux UID et on relève
//!   périodiquement les drapeaux de l'ensemble, faute de mieux.
//!
//! Le cas le plus dangereux est le changement de `UIDVALIDITY` : le serveur a
//! reconstruit la boîte, et **tous les UID connus désignent désormais autre chose**.
//! Les conserver produirait des messages mélangés — un contenu attribué au mauvais
//! expéditeur. On efface donc le dossier localement avant de le relire.

use iris_imap::{ImapConnection, UidRange};
use iris_store::{Folder, NewMessage, Store};
use iris_types::{AccountId, Error, Flags, Result, Timestamp};

/// Réglages de la synchronisation d'un dossier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderSyncOptions {
    /// Nombre d'UID demandés par commande.
    pub chunk_size: u32,
    /// Relever les suppressions faites ailleurs. Coûteux : une recherche sur tout
    /// le dossier. On ne le fait pas à chaque cycle.
    pub detect_deletions: bool,
    /// Nombre maximal de messages ramenés en une passe.
    ///
    /// Sur une boîte de cent mille messages, la première synchronisation doit
    /// pouvoir s'interrompre et reprendre : l'utilisateur veut voir sa liste se
    /// remplir, pas attendre un quart d'heure devant un écran vide.
    pub max_per_pass: usize,
    /// Leave the flags here as they are: changes made here still wait to reach the
    /// server (its journal could not be replayed). Read back, the server's flags
    /// undid them at every pass until they got through.
    pub keep_local_flags: bool,
}

impl Default for FolderSyncOptions {
    fn default() -> Self {
        Self {
            chunk_size: 500,
            detect_deletions: false,
            max_per_pass: 5_000,
            keep_local_flags: false,
        }
    }
}

/// Ce qu'une synchronisation a produit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderReport {
    pub added: usize,
    /// Threads a new message has just joined: arrived in the inbox, not a copy of a
    /// message already here. What may reopen them.
    pub arrivals: Vec<iris_types::ThreadId>,
    pub flags_updated: usize,
    pub deleted: usize,
    /// The messages this pass added and removed, for the search index: re-reading the
    /// folder's 5,000 newest at every arrival indexed some twice and others never,
    /// and what was deleted stayed searchable.
    pub new_messages: Vec<iris_types::MessageId>,
    pub removed_messages: Vec<iris_types::MessageId>,
    /// Of the new rows, the new mail: not a copy of a message already here (Gmail's
    /// All Mail), nor one back from a move. What the rules look at, once.
    pub fresh_messages: Vec<iris_types::MessageId>,
    /// New mail in the inbox, outside a first sync: what a notification tells of.
    /// Every new row was counted, so Gmail's mail counted twice, a sent or archived
    /// copy counted, and a first sync announced thousands.
    pub inbox_arrivals: usize,
    /// Threads whose mail came back into the inbox from elsewhere (moved there on a
    /// phone, Gmail's Inbox label put back): to do again. They kept the Done state
    /// their move had left, and stayed hidden.
    pub back_in_inbox: Vec<iris_types::ThreadId>,
    /// Le dossier a été relu intégralement.
    pub full_resync: bool,
    /// Le serveur avait reconstruit la boîte.
    pub uid_validity_changed: bool,
    /// Il reste des messages à ramener : il faut repasser.
    pub more_available: bool,
}

impl FolderReport {
    pub fn changed(&self) -> bool {
        self.added > 0 || self.flags_updated > 0 || self.deleted > 0
    }
}

/// Up to how many messages a folder without `CONDSTORE` has all its flags read again
/// at every pass. A larger one has them read on the deletion scan only: about forty
/// bytes a message, so this is a few hundred kilobytes at most.
const FLAG_RESYNC_EVERY_PASS: usize = 5_000;

/// Synchronise un dossier.
pub async fn sync_folder(
    conn: &mut dyn ImapConnection,
    store: &Store,
    account: AccountId,
    folder: &Folder,
    options: FolderSyncOptions,
) -> Result<FolderReport> {
    let mut rapport = FolderReport::default();

    let etat = conn.select(&folder.path).await?;
    let capacites = conn.capabilities();

    // Le serveur a-t-il reconstruit la boîte ?
    //
    // Les deux côtés doivent être connus. Le zéro local était déjà écarté — c'est la
    // première visite, on ne sait rien — mais pas le zéro **du serveur**, et celui-là
    // ne veut pas dire « boîte reconstruite » : il veut dire que la réponse au SELECT
    // n'en portait pas, ou qu'on ne l'a pas lue.
    //
    // Le journal en garde la trace : quatre dossiers passés à `apres=0` dans la même
    // seconde, chacun suivi d'un effacement local et du re-téléchargement complet de
    // son contenu. Quatre boîtes ne sont pas reconstruites en même temps ; c'était une
    // connexion coupée. Le prix d'une erreur ici est tout le contenu local d'un
    // dossier, donc le doute profite au cache.
    let validite_changee = folder.uid_validity != 0
        && etat.uid_validity != 0
        && folder.uid_validity != etat.uid_validity;

    if etat.uid_validity == 0 {
        tracing::warn!(
            folder = %folder.path,
            "le serveur n'a pas donné d'UIDVALIDITY : rien n'est effacé"
        );
    }
    if validite_changee {
        tracing::warn!(
            folder = %folder.path,
            avant = folder.uid_validity,
            apres = etat.uid_validity,
            "UIDVALIDITY modifié : effacement local avant relecture"
        );
        store.clear_folder(folder.id)?;
        rapport.uid_validity_changed = true;
    }

    // Le `UIDVALIDITY` est enregistre des maintenant, avant meme d'avoir ramene quoi
    // que ce soit. Une passe interrompue doit pouvoir reprendre a l'UID ou elle s'est
    // arretee ; sans cette ecriture, elle se croirait a sa premiere visite et
    // repartirait indefiniment de zero.
    //
    // Jamais avec un zéro, en revanche, et c'est la seconde moitié du même défaut.
    // Écraser une valeur connue par l'absence de valeur fait croire à la passe suivante
    // qu'elle n'est jamais venue : `premiere_visite` devient vrai, la synchronisation
    // repart complète, et tout le dossier redescend. C'est le `added=88` qui suivait
    // chaque `apres=0` dans le journal.
    if etat.uid_validity != 0 && folder.uid_validity != etat.uid_validity {
        // A rebuilt folder starts its first sync again: its `UIDNEXT` is forgotten
        // until that sync is done, which is how the next passes know to go on down.
        store.update_folder_sync_state(
            folder.id,
            etat.uid_validity,
            if validite_changee { 0 } else { folder.uid_next },
            if validite_changee {
                0
            } else {
                folder.highest_modseq
            },
        )?;
    }

    let premiere_visite = folder.uid_validity == 0;
    let complet = validite_changee || premiere_visite || folder.highest_modseq == 0;
    rapport.full_resync = complet;

    // --- Drapeaux modifiés ailleurs ---
    let lire_drapeaux = !options.keep_local_flags;
    if lire_drapeaux
        && !complet
        && capacites.condstore
        && etat.highest_modseq > folder.highest_modseq
    {
        let changements = conn.flags_changed_since(folder.highest_modseq).await?;
        rapport.flags_updated = store.apply_flag_changes(folder.id, &changements)?;
    }

    // --- Nouveaux messages ---
    // Le point de reprise est toujours le plus grand UID deja stocke : apres un
    // effacement pour cause de `UIDVALIDITY`, il vaut zero, et la passe repart donc
    // naturellement du debut.
    let depuis = store.max_uid(folder.id)?;
    let intervalle = if depuis == 0 {
        UidRange::ALL
    } else {
        UidRange::since(depuis)
    };

    // A first sync not finished yet (the folder's `UIDNEXT` is only kept once one is):
    // the newest mail first, then down. Oldest first, a large mailbox showed mail from
    // years ago for its first passes, today's last.
    let premiere_synchro = folder.uid_next == 0 || validite_changee;
    let tranches = if premiere_synchro {
        let mut t = Vec::new();
        if depuis > 0 {
            t.extend(plan_chunks(intervalle, etat.uid_next, options.chunk_size));
        }
        let bas = store.min_uid(folder.id)?;
        let plafond = if bas > 0 {
            bas - 1
        } else {
            etat.uid_next.saturating_sub(1)
        };
        if plafond >= 1 {
            let mut vers_le_bas = UidRange::new(1, plafond).chunks(options.chunk_size);
            vers_le_bas.reverse();
            t.extend(vers_le_bas);
        }
        t
    } else {
        plan_chunks(intervalle, etat.uid_next, options.chunk_size)
    };

    let mut ramenes = 0usize;
    for tranche in tranches {
        if ramenes >= options.max_per_pass {
            rapport.more_available = true;
            break;
        }

        let bruts = match conn.fetch_envelopes(tranche).await {
            Ok(b) => b,
            Err(e) if e.is_transient() => return Err(e),
            // One answer the library cannot read failed the whole batch, the next
            // pass started from the same place and failed the same way: nothing later
            // in the folder ever arrived. The batch is asked for again one message at
            // a time, and only the unreadable one is set aside.
            // A batch the server refused half-way (Gmail's "Some messages could not be
            // FETCHed", Office 365's limits) is the same case: what it did send is not
            // all there is, and resuming above it left the rest out for good.
            Err(e) => {
                tracing::warn!(folder = %folder.path, error = %e, "batch unreadable, one at a time");
                let mut un_par_un = Vec::new();
                for uid in conn.existing_uids(tranche).await? {
                    match conn.fetch_envelopes(UidRange::new(uid, uid)).await {
                        // Nothing for it: expunged since the search.
                        Ok(b) => un_par_un.extend(b),
                        Err(e) if e.is_transient() => return Err(e),
                        // Kept as a placeholder, not left out: left out, it was gone
                        // without a trace, since the next pass starts above it. Its
                        // body may still be read when it is opened.
                        Err(_) => {
                            tracing::warn!(uid, folder = %folder.path, "message unreadable, kept as a placeholder");
                            un_par_un.push(placeholder(uid));
                        }
                    }
                }
                un_par_un
            }
        };
        if bruts.is_empty() {
            continue;
        }

        let mut lot = Vec::with_capacity(bruts.len());
        let mut extras = Vec::new();
        for brut in &bruts {
            match to_new_message(account, folder, brut) {
                Ok((m, copies, reponse)) => {
                    if copies != "[]" || reponse != "[]" {
                        extras.push((m.uid, copies, reponse));
                    }
                    lot.push(m);
                }
                // Un message illisible ne doit pas interrompre la synchronisation
                // des dix mille autres.
                Err(e) => tracing::warn!(uid = brut.uid, error = %e, "message ignoré"),
            }
        }

        let inseres = store.insert_messages(&lot)?;
        store.set_message_extras(folder.id, &extras)?;
        ramenes += lot.len();
        rapport.added += inseres.iter().filter(|i| !i.was_known).count();
        rapport
            .new_messages
            .extend(inseres.iter().filter(|i| !i.was_known).map(|i| i.message));
        let frais: Vec<_> = inseres
            .iter()
            .filter(|i| !i.was_known && !i.came_back)
            .map(|i| i.message)
            .collect();
        if !premiere_synchro && folder.role == iris_store::FolderRole::Inbox {
            rapport.inbox_arrivals += frais.len();
            for i in inseres.iter().filter(|i| !i.was_known && i.came_back) {
                if !rapport.back_in_inbox.contains(&i.thread) {
                    rapport.back_in_inbox.push(i.thread);
                }
            }
        }
        rapport.fresh_messages.extend(frais);

        // Not on a first visit: everything there is old news, not an arrival. The
        // inbox only: mail filed into a folder of one's own is often mail Iris moved
        // there, whose old copy may already be gone, and that is not an answer.
        if !premiere_synchro && folder.role == iris_store::FolderRole::Inbox {
            for i in inseres.iter().filter(|i| !i.was_known && !i.thread_created) {
                if !store.has_other_copy(i.message)? && !rapport.arrivals.contains(&i.thread) {
                    rapport.arrivals.push(i.thread);
                }
            }
        }
    }

    // --- Drapeaux modifiés ailleurs, sans CONDSTORE ---
    //
    // A server that keeps no `MODSEQ` (Exchange, Courier, many hosts) cannot say what
    // changed, and nothing asked: a message read on the phone stayed unread here for
    // ever. Its flags are read again, all of them, each pass for a folder of a
    // reasonable size and on the deletion scan for a large one.
    let deja_connu = !(validite_changee || premiere_visite);
    // A first sync that has just finished reads every flag once: changed elsewhere
    // while its passes went on, a flag on mail already fetched was otherwise never
    // seen (the change counter's baseline is only taken at the end).
    let premiere_finie = premiere_synchro && !rapport.more_available && !premiere_visite;
    if lire_drapeaux && premiere_finie && capacites.condstore && etat.highest_modseq > 0 {
        let connus = store.max_uid(folder.id)?;
        if connus > 0 {
            let tous = conn.fetch_flags(UidRange::new(1, connus)).await?;
            rapport.flags_updated += store.apply_flag_changes(folder.id, &tous)?;
        }
    }
    if lire_drapeaux && deja_connu && !(capacites.condstore && etat.highest_modseq > 0) {
        let connus = store.max_uid(folder.id)?;
        let taille = store.folder_uids(folder.id)?.len();
        if connus > 0 && (taille <= FLAG_RESYNC_EVERY_PASS || options.detect_deletions) {
            let tous = conn.fetch_flags(UidRange::new(1, connus)).await?;
            rapport.flags_updated += store.apply_flag_changes(folder.id, &tous)?;
        }
    }

    // --- Suppressions faites ailleurs ---
    //
    // Looked for on the scheduled scan, and on any pass where the count gives it away:
    // once the new messages are in, the server holding fewer than the local copy means
    // some went — moved to the bin from a phone, most often. Waiting for the scan alone
    // left them in Inbox here for hours, since a quiet account is visited once an hour
    // and scanned one visit in ten.
    //
    // Not on a first visit or after a rebuilt mailbox: there is nothing local to drop.
    // A folder whose server keeps no `MODSEQ` is no reason to skip it, though: that
    // only stops flag changes from being asked for, and it kept such folders' deletions
    // from ever being seen.
    let locaux = if deja_connu {
        store.folder_uids(folder.id)?
    } else {
        Vec::new()
    };
    let en_trop = !rapport.more_available && locaux.len() as u64 > etat.exists as u64;
    // Never on a selection that gave no `UIDVALIDITY`: a real one always does, and one
    // without it was an answer cut short, whose message count of zero emptied the
    // folder here.
    if deja_connu && etat.uid_validity != 0 && (options.detect_deletions || en_trop) {
        let distants = conn.existing_uids(UidRange::ALL).await?;
        // A search that finds nothing in a folder the server says holds mail
        // contradicts it: believed, it deleted every local copy.
        if distants.is_empty() && etat.exists > 0 {
            tracing::warn!(
                folder = %folder.path,
                exists = etat.exists,
                "the server found no message in a folder it says is not empty: nothing deleted"
            );
        } else {
            let disparus = missing_uids(&locaux, &distants);
            if !disparus.is_empty() {
                rapport.removed_messages = store.message_ids_by_uid(folder.id, &disparus)?;
                rapport.deleted = store.delete_messages_by_uid(folder.id, &disparus)?;
            }
        }
    }

    // L'état de synchronisation n'est enregistré qu'à la fin, et seulement si la
    // passe est complète : l'enregistrer trop tôt ferait manquer définitivement les
    // messages restants après une interruption.
    //
    // La validité qu'on réécrit est celle qu'on avait quand le serveur n'en donne pas.
    // Le reste de la passe s'est déroulé normalement — les UID sont les mêmes qu'avant,
    // puisque rien ne dit le contraire — et il n'y a aucune raison de perdre en chemin
    // la seule chose qui permettra la prochaine fois de reprendre où l'on s'arrête.
    if !rapport.more_available {
        let validite = if etat.uid_validity != 0 {
            etat.uid_validity
        } else {
            folder.uid_validity
        };
        // The change counter only moves on once flags were read up to it: moved on
        // without, the changes in between were never asked for again.
        let modseq = if lire_drapeaux {
            etat.highest_modseq
        } else {
            folder.highest_modseq
        };
        store.update_folder_sync_state(folder.id, validite, etat.uid_next, modseq)?;
    }

    Ok(rapport)
}

/// Découpe l'intervalle à demander, en le bornant à ce que le dossier contient.
fn plan_chunks(range: UidRange, uid_next: u32, chunk_size: u32) -> Vec<UidRange> {
    // `uid_next` donne la borne supérieure réelle : sans elle, l'intervalle est
    // ouvert et ne peut pas être découpé, ce qui ramènerait tout d'un coup.
    let plafond = uid_next.saturating_sub(1);
    if plafond == 0 || range.from > plafond {
        return Vec::new();
    }
    UidRange::new(range.from, range.to.min(plafond)).chunks(chunk_size)
}

/// UID connus localement mais absents du serveur.
///
/// Les deux listes sont triées : une fusion suffit, là où une comparaison naïve
/// coûterait le produit des tailles.
/// A message the server would not send in any form, kept so that it can be seen and
/// opened later rather than skipped for good.
fn placeholder(uid: u32) -> iris_imap::RawMessage {
    let maintenant = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    iris_imap::RawMessage {
        uid,
        flags: Flags::NONE,
        internal_date: Timestamp::from_millis(maintenant),
        size: 0,
        content: b"Subject: (a message Iris could not read; open it to try again)\r\n\r\n".to_vec(),
    }
}

fn missing_uids(locaux: &[u32], distants: &[u32]) -> Vec<u32> {
    let mut out = Vec::new();
    let mut j = 0;
    for &local in locaux {
        while j < distants.len() && distants[j] < local {
            j += 1;
        }
        if j >= distants.len() || distants[j] != local {
            out.push(local);
        }
    }
    out
}

/// Has the server already judged this message unwanted?
///
/// Iris does not classify spam itself — see `iris_mime::spam` for why. It reads the
/// verdict from wherever the provider left it.
fn is_spam(folder: &Folder, raw: &[u8], subject: &str) -> bool {
    // Filed in the junk folder: there is nothing left to decide.
    if folder.role == iris_store::FolderRole::Junk {
        return true;
    }
    if iris_mime::subject_is_tagged(subject) {
        return true;
    }

    // Only the header block is examined. Scanning a whole message for these strings
    // would match any mail that happens to quote one — including, reliably, mail
    // about spam filtering.
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .or_else(|| raw.windows(2).position(|w| w == b"\n\n"))
        .unwrap_or(raw.len().min(16 * 1024));

    let headers = String::from_utf8_lossy(&raw[..head_end]);
    iris_mime::headers_say_spam(&headers)
}

/// Whether a message's top-level type can hold an attachment: `multipart/mixed`, or a
/// single part that is itself a file. Text and `multipart/alternative` cannot.
fn may_carry_files(raw: &[u8]) -> bool {
    let fin = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(raw.len().min(16 * 1024));
    let entetes = String::from_utf8_lossy(&raw[..fin]).to_ascii_lowercase();
    // The header, unfolded enough to read its first value.
    let Some(debut) = entetes.lines().position(|l| l.starts_with("content-type:")) else {
        return false; // No type: text/plain.
    };
    let ligne = entetes.lines().nth(debut).unwrap_or_default();
    let valeur = ligne["content-type:".len()..].trim();
    !(valeur.starts_with("text/")
        || valeur.starts_with("multipart/alternative")
        || valeur.starts_with("multipart/related")
        || valeur.starts_with("multipart/report"))
}

/// Traduit un message brut du serveur en message à insérer, avec ses copies (`Cc`)
/// et son adresse de réponse (`Reply-To`) en listes JSON.
fn to_new_message(
    account: AccountId,
    folder: &Folder,
    brut: &iris_imap::RawMessage,
) -> Result<(NewMessage, String, String)> {
    let analyse = iris_mime::parse(&brut.content)
        .map_err(|e| Error::parse(format!("UID {} : {e}", brut.uid)))?;

    let destinataires = serde_json::to_string(&analyse.to).unwrap_or_else(|_| "[]".into());
    let copies = serde_json::to_string(&analyse.cc).unwrap_or_else(|_| "[]".into());
    let reponse = serde_json::to_string(&analyse.reply_to).unwrap_or_else(|_| "[]".into());
    let expediteur = analyse.from.first();

    // Les drapeaux du serveur et ceux déduits du contenu se combinent : le serveur
    // sait ce qui est lu, nous savons ce qui contient une pièce jointe.
    let mut flags = brut.flags.with(analyse.derived_flags);
    // Only the start of the text is here: a message whose structure cannot carry a
    // file (text, or an HTML newsletter's alternative) is not said to have one. The
    // whole body settles it once downloaded (`set_body_facts`).
    if !may_carry_files(&brut.content) {
        flags = flags.without(iris_types::Flags::HAS_ATTACHMENT);
    }

    // Whether this is spam is the server's judgement, read back rather than
    // recomputed: from the folder it filed the message in, from the headers its
    // filter wrote, or from the marker it stapled to the subject. Three sources
    // because providers use different ones, and any of them is a verdict.
    // Someone took it out of the junk (`$NotJunk`): the filter's headers stay on the
    // message for ever, and a false positive put back in the inbox from a phone stayed
    // hidden here.
    let sauve = brut.flags.contains(iris_types::Flags::NOT_JUNK)
        && folder.role != iris_store::FolderRole::Junk;
    if !sauve && is_spam(folder, &brut.content, &analyse.subject) {
        flags = flags.with(iris_types::Flags::SPAM);
    }

    let subject = if flags.contains(iris_types::Flags::SPAM) {
        // The marker has done its job once the message is filed; leaving it on every
        // row pushes the actual subject off the end of the line.
        iris_mime::strip_marker(&analyse.subject)
    } else {
        analyse.subject.clone()
    };

    let message = NewMessage {
        account,
        folder: folder.id,
        uid: brut.uid,
        rfc_message_id: analyse.rfc_message_id.map(|m| m.0),
        in_reply_to: analyse.in_reply_to.map(|m| m.0),
        references: analyse.references.into_iter().map(|m| m.0).collect(),
        subject,
        from_name: expediteur.and_then(|a| a.name.clone()).unwrap_or_default(),
        from_addr: expediteur.map(|a| a.addr.clone()).unwrap_or_default(),
        recipients_json: destinataires,
        // La date de dépôt du serveur fait foi : la date déclarée par l'expéditeur
        // peut être absurde, et l'a souvent été volontairement.
        date: if analyse.date == Timestamp::EPOCH {
            brut.internal_date
        } else {
            analyse.date
        },
        received: brut.internal_date,
        size: brut.size,
        flags,
        preview: analyse.preview,
    };
    Ok((message, copies, reponse))
}

/// Applique aux drapeaux locaux ceux annoncés par le serveur.
///
/// Les drapeaux déduits du contenu — pièce jointe, traqueur, désabonnement — sont
/// **conservés** : le serveur ne les connaît pas, et les écraser les ferait
/// disparaître à chaque synchronisation.
///
/// The rule `Store::apply_flag_changes` applies: the server's word on what IMAP
/// knows, ours on the rest (spam included).
pub fn merge_flags(local: Flags, remote: Flags) -> Flags {
    Flags((remote.0 & Flags::PROTOCOL.0) | (local.0 & !Flags::PROTOCOL.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_imap::fake::FakeServer;
    use iris_imap::{Connector, Credentials, Endpoint, FolderKind};
    use iris_store::{FolderRole, NewAccount};

    fn message(sujet: &str, id: &str) -> Vec<u8> {
        format!(
            "Subject: {sujet}\r\nFrom: Marie <marie@example.com>\r\nTo: moi@example.com\r\n\
             Message-ID: <{id}>\r\nDate: Mon, 13 Sep 2027 10:00:00 +0200\r\n\r\nCorps.\r\n"
        )
        .into_bytes()
    }

    struct Fixture {
        store: Store,
        server: FakeServer,
        account: AccountId,
    }

    fn fixture() -> Fixture {
        let store = Store::in_memory().unwrap();
        let account = store
            .create_account(
                &NewAccount::new("moi@example.com", "imap.x.fr", "smtp.x.fr"),
                Timestamp::from_millis(0),
            )
            .unwrap();
        store
            .upsert_folder(account, "INBOX", FolderRole::Inbox)
            .unwrap();
        Fixture {
            store,
            server: FakeServer::default(),
            account,
        }
    }

    impl Fixture {
        fn folder(&self) -> Folder {
            self.store
                .folders(self.account)
                .unwrap()
                .into_iter()
                .next()
                .unwrap()
        }

        async fn conn(&self) -> Box<dyn ImapConnection> {
            self.server
                .connect(
                    &Endpoint::tls("imap.x.fr", 993),
                    &Credentials::Password {
                        user: "moi@example.com".into(),
                        password: "x".into(),
                    },
                )
                .await
                .unwrap()
        }

        async fn sync(&self, options: FolderSyncOptions) -> FolderReport {
            let mut c = self.conn().await;
            sync_folder(
                c.as_mut(),
                &self.store,
                self.account,
                &self.folder(),
                options,
            )
            .await
            .unwrap()
        }
    }

    #[tokio::test]
    async fn la_premiere_synchronisation_ramene_tout() {
        let f = fixture();
        for i in 0..5 {
            f.server.deliver(
                "INBOX",
                &message(&format!("Sujet {i}"), &format!("m{i}@x")),
                Flags::NONE,
            );
        }

        let r = f.sync(FolderSyncOptions::default()).await;
        assert_eq!(r.added, 5);
        assert!(r.full_resync);
        assert_eq!(f.store.message_count().unwrap(), 5);
    }

    #[tokio::test]
    async fn une_seconde_passe_ne_ramene_que_le_nouveau() {
        let f = fixture();
        f.server
            .deliver("INBOX", &message("Un", "m1@x"), Flags::NONE);
        f.sync(FolderSyncOptions::default()).await;

        f.server
            .deliver("INBOX", &message("Deux", "m2@x"), Flags::NONE);
        let r = f.sync(FolderSyncOptions::default()).await;

        assert_eq!(r.added, 1);
        assert!(!r.full_resync, "la seconde passe doit être incrémentale");
        assert_eq!(f.store.message_count().unwrap(), 2);
    }

    #[tokio::test]
    async fn un_cycle_sans_nouveaute_ne_change_rien() {
        let f = fixture();
        f.server
            .deliver("INBOX", &message("Un", "m1@x"), Flags::NONE);
        f.sync(FolderSyncOptions::default()).await;

        let r = f.sync(FolderSyncOptions::default()).await;
        assert!(!r.changed());
        assert_eq!(r.added, 0);
    }

    #[tokio::test]
    async fn les_drapeaux_modifies_ailleurs_sont_repris() {
        let f = fixture();
        f.server
            .deliver("INBOX", &message("Un", "m1@x"), Flags::NONE);
        f.sync(FolderSyncOptions::default()).await;

        // Un autre client marque le message comme lu.
        f.server.set_flags_remotely("INBOX", 1, Flags::SEEN);
        let r = f.sync(FolderSyncOptions::default()).await;

        assert_eq!(r.flags_updated, 1);
        let message = &f.store.thread_messages(iris_types::ThreadId(1)).unwrap()[0];
        assert!(message.flags.contains(Flags::SEEN));
    }

    #[tokio::test]
    async fn un_changement_d_uidvalidity_efface_et_relit() {
        // Sans cet effacement, les anciens UID désigneraient d'autres messages et on
        // attribuerait un contenu au mauvais expéditeur.
        let f = fixture();
        f.server
            .deliver("INBOX", &message("Ancien", "ancien@x"), Flags::NONE);
        f.sync(FolderSyncOptions::default()).await;
        assert_eq!(f.store.message_count().unwrap(), 1);

        f.server.bump_uid_validity("INBOX");
        let r = f.sync(FolderSyncOptions::default()).await;

        assert!(r.uid_validity_changed);
        assert!(r.full_resync);
        // Le message est relu, pas dupliqué.
        assert_eq!(f.store.message_count().unwrap(), 1);
    }

    #[tokio::test]
    async fn un_uidvalidity_absent_n_efface_rien() {
        // Zéro n'est pas une valeur, c'est l'absence de valeur — une réponse au SELECT
        // tronquée, une connexion tombée. Le prendre pour un changement effaçait le
        // contenu local du dossier et le retéléchargeait en entier.
        //
        // Le journal de l'utilisateur en portait la trace : quatre dossiers passés à
        // « apres=0 » dans la même seconde, puis « added=88 ». Quatre boîtes ne sont
        // pas reconstruites en même temps.
        let f = fixture();
        f.server
            .deliver("INBOX", &message("Ancien", "ancien@x"), Flags::NONE);
        f.sync(FolderSyncOptions::default()).await;
        assert_eq!(f.store.message_count().unwrap(), 1);

        f.server.drop_uid_validity("INBOX");
        let r = f.sync(FolderSyncOptions::default()).await;

        assert!(
            !r.uid_validity_changed,
            "rien n'a changé, rien n'est effacé"
        );
        assert_eq!(f.store.message_count().unwrap(), 1);

        // Et la valeur connue survit : sans cela la passe suivante se croirait à sa
        // première visite et redescendrait tout le dossier.
        let dossier = f
            .store
            .folders(f.account)
            .unwrap()
            .into_iter()
            .find(|d| d.path == "INBOX")
            .unwrap();
        assert_ne!(
            dossier.uid_validity, 0,
            "la valeur connue n'est pas écrasée"
        );
    }

    #[tokio::test]
    async fn les_suppressions_distantes_sont_relevees_quand_on_les_cherche() {
        let f = fixture();
        for i in 0..4 {
            f.server.deliver(
                "INBOX",
                &message(&format!("m{i}"), &format!("m{i}@x")),
                Flags::NONE,
            );
        }
        f.sync(FolderSyncOptions::default()).await;

        f.server.remove("INBOX", 2);
        f.server
            .deliver("INBOX", &message("m4", "m4@x"), Flags::NONE);

        let r = f
            .sync(FolderSyncOptions {
                detect_deletions: true,
                ..Default::default()
            })
            .await;
        assert_eq!(r.added, 1);
        assert_eq!(r.deleted, 1);
        assert_eq!(f.store.message_count().unwrap(), 4);
    }

    #[tokio::test]
    async fn a_message_binned_elsewhere_leaves_inbox_on_the_next_pass() {
        // Moved to the bin from a phone: the inbox holds one fewer than the local copy.
        let f = fixture();
        for i in 0..3 {
            f.server.deliver(
                "INBOX",
                &message(&format!("m{i}"), &format!("m{i}@x")),
                Flags::NONE,
            );
        }
        f.sync(FolderSyncOptions::default()).await;

        f.server.remove("INBOX", 1);
        let r = f.sync(FolderSyncOptions::default()).await;
        assert_eq!(r.deleted, 1, "without waiting for the scheduled scan");
        assert_eq!(f.store.message_count().unwrap(), 2);
    }

    #[tokio::test]
    async fn a_folder_without_modseq_still_sees_its_deletions() {
        let store = Store::in_memory().unwrap();
        let account = store
            .create_account(
                &NewAccount::new("a@x.fr", "i", "s"),
                Timestamp::from_millis(0),
            )
            .unwrap();
        store
            .upsert_folder(account, "INBOX", FolderRole::Inbox)
            .unwrap();
        let server = FakeServer::legacy();
        server.deliver("INBOX", &message("Un", "m1@x"), Flags::NONE);
        let deux = server.deliver("INBOX", &message("Deux", "m2@x"), Flags::NONE);

        async fn passe(store: &Store, server: &FakeServer, account: AccountId) -> FolderReport {
            let folder = store.folders(account).unwrap().into_iter().next().unwrap();
            let mut c = server
                .connect(
                    &Endpoint::tls("x", 993),
                    &Credentials::Password {
                        user: "a@x.fr".into(),
                        password: "p".into(),
                    },
                )
                .await
                .unwrap();
            sync_folder(
                c.as_mut(),
                store,
                account,
                &folder,
                FolderSyncOptions {
                    detect_deletions: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap()
        }
        passe(&store, &server, account).await;

        server.remove("INBOX", deux);
        let r = passe(&store, &server, account).await;
        assert_eq!(r.deleted, 1);
        assert_eq!(store.message_count().unwrap(), 1);
    }

    #[tokio::test]
    async fn a_server_without_condstore_still_tells_what_was_read_elsewhere() {
        // Nothing asked: a message read on the phone stayed unread here for ever.
        let store = Store::in_memory().unwrap();
        let account = store
            .create_account(
                &NewAccount::new("a@x.fr", "i", "s"),
                Timestamp::from_millis(0),
            )
            .unwrap();
        store
            .upsert_folder(account, "INBOX", FolderRole::Inbox)
            .unwrap();
        let server = FakeServer::legacy();
        let un = server.deliver("INBOX", &message("Un", "m1@x"), Flags::NONE);

        let passe = || async {
            let folder = store.folders(account).unwrap().into_iter().next().unwrap();
            let mut c = server
                .connect(
                    &Endpoint::tls("x", 993),
                    &Credentials::Password {
                        user: "a@x.fr".into(),
                        password: "p".into(),
                    },
                )
                .await
                .unwrap();
            sync_folder(
                c.as_mut(),
                &store,
                account,
                &folder,
                FolderSyncOptions::default(),
            )
            .await
            .unwrap()
        };
        passe().await;
        server.set_flags_remotely("INBOX", un, Flags::SEEN);

        let r = passe().await;
        assert_eq!(r.flags_updated, 1);
        let folder = store.folders(account).unwrap().into_iter().next().unwrap();
        let id = store.message_by_uid(folder.id, un).unwrap().unwrap();
        let lu = store.message_by_id(id).unwrap().unwrap();
        assert!(lu.flags.contains(Flags::SEEN));
    }

    #[tokio::test]
    async fn une_passe_bornee_reprend_ou_elle_s_est_arretee() {
        // L'utilisateur doit voir sa liste se remplir, pas attendre devant un écran
        // vide pendant un quart d'heure.
        let f = fixture();
        for i in 0..25 {
            f.server.deliver(
                "INBOX",
                &message(&format!("m{i}"), &format!("m{i}@x")),
                Flags::NONE,
            );
        }

        let options = FolderSyncOptions {
            chunk_size: 5,
            max_per_pass: 10,
            ..Default::default()
        };
        let r = f.sync(options).await;
        assert!(r.more_available, "la passe doit se déclarer incomplète");
        assert_eq!(f.store.message_count().unwrap(), 10);

        // La reprise ramène la suite, sans doublon.
        let r = f.sync(options).await;
        assert!(r.more_available);
        assert_eq!(f.store.message_count().unwrap(), 20);

        f.sync(options).await;
        assert_eq!(f.store.message_count().unwrap(), 25);
    }

    #[tokio::test]
    async fn un_message_illisible_n_interrompt_pas_la_synchronisation() {
        let f = fixture();
        f.server
            .deliver("INBOX", &message("Bon", "bon@x"), Flags::NONE);
        f.server.deliver("INBOX", &[], Flags::NONE);
        f.server
            .deliver("INBOX", &message("Autre", "autre@x"), Flags::NONE);

        let r = f.sync(FolderSyncOptions::default()).await;
        assert!(r.added >= 2, "les messages lisibles doivent passer");
    }

    #[tokio::test]
    async fn les_drapeaux_deduits_du_contenu_sont_poses() {
        let f = fixture();
        let avec_desabonnement = b"Subject: Infolettre\r\nFrom: news@x.fr\r\n\
List-Unsubscribe: <https://x.fr/unsub>\r\nMessage-ID: <n@x>\r\n\r\nCorps.\r\n";
        f.server.deliver("INBOX", avec_desabonnement, Flags::NONE);

        f.sync(FolderSyncOptions::default()).await;
        let message = &f.store.thread_messages(iris_types::ThreadId(1)).unwrap()[0];
        assert!(message.flags.contains(Flags::UNSUBSCRIBABLE));
    }

    #[tokio::test]
    async fn un_serveur_sans_condstore_reste_synchronisable() {
        let store = Store::in_memory().unwrap();
        let account = store
            .create_account(
                &NewAccount::new("a@x.fr", "i", "s"),
                Timestamp::from_millis(0),
            )
            .unwrap();
        store
            .upsert_folder(account, "INBOX", FolderRole::Inbox)
            .unwrap();
        let server = FakeServer::legacy();
        server.deliver("INBOX", &message("Un", "m1@x"), Flags::NONE);

        let folder = store.folders(account).unwrap().into_iter().next().unwrap();
        let mut c = server
            .connect(
                &Endpoint::tls("x", 993),
                &Credentials::Password {
                    user: "a@x.fr".into(),
                    password: "p".into(),
                },
            )
            .await
            .unwrap();

        let r = sync_folder(
            c.as_mut(),
            &store,
            account,
            &folder,
            FolderSyncOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(r.added, 1);
    }

    #[tokio::test]
    async fn un_dossier_vide_ne_produit_rien() {
        let f = fixture();
        let r = f.sync(FolderSyncOptions::default()).await;
        assert_eq!(r.added, 0);
        assert!(!r.more_available);
    }

    #[tokio::test]
    async fn un_dossier_inconnu_du_serveur_est_une_erreur() {
        let f = fixture();
        f.store
            .upsert_folder(f.account, "Absent", FolderRole::Other)
            .unwrap();
        let dossier = f
            .store
            .folders(f.account)
            .unwrap()
            .into_iter()
            .find(|d| d.path == "Absent")
            .unwrap();

        let mut c = f.conn().await;
        let r = sync_folder(
            c.as_mut(),
            &f.store,
            f.account,
            &dossier,
            FolderSyncOptions::default(),
        )
        .await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn les_messages_deplaces_ailleurs_apparaissent_dans_leur_nouveau_dossier() {
        let f = fixture();
        f.server.add_folder("Archive", FolderKind::Archive);
        f.store
            .upsert_folder(f.account, "Archive", FolderRole::Archive)
            .unwrap();
        f.server
            .deliver("INBOX", &message("Un", "m1@x"), Flags::NONE);

        f.sync(FolderSyncOptions::default()).await;

        let mut c = f.conn().await;
        c.select("INBOX").await.unwrap();
        c.move_messages(&[1], "Archive").await.unwrap();

        let archive = f
            .store
            .folders(f.account)
            .unwrap()
            .into_iter()
            .find(|d| d.path == "Archive")
            .unwrap();
        let mut c2 = f.conn().await;
        let r = sync_folder(
            c2.as_mut(),
            &f.store,
            f.account,
            &archive,
            FolderSyncOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(r.added, 1);
    }

    #[test]
    fn les_uid_disparus_se_calculent_par_fusion() {
        assert_eq!(missing_uids(&[1, 2, 3, 4], &[1, 3]), vec![2, 4]);
        assert_eq!(missing_uids(&[1, 2], &[1, 2]), Vec::<u32>::new());
        assert_eq!(missing_uids(&[], &[1, 2]), Vec::<u32>::new());
        assert_eq!(missing_uids(&[5, 6], &[]), vec![5, 6]);
        // Des UID distants inconnus localement ne sont pas des suppressions.
        assert_eq!(missing_uids(&[2], &[1, 2, 3]), Vec::<u32>::new());
    }

    #[test]
    fn le_plan_de_tranches_est_borne_par_le_dossier() {
        // Sans borne supérieure, l'intervalle reste ouvert et ramène tout d'un coup.
        let tranches = plan_chunks(UidRange::ALL, 101, 25);
        assert_eq!(tranches.len(), 4);
        assert_eq!(tranches[0], UidRange::new(1, 25));
        assert_eq!(tranches[3], UidRange::new(76, 100));
    }

    #[test]
    fn un_dossier_vide_ne_produit_aucune_tranche() {
        assert!(plan_chunks(UidRange::ALL, 1, 100).is_empty());
        assert!(plan_chunks(UidRange::since(50), 40, 100).is_empty());
    }

    #[test]
    fn only_a_message_built_to_carry_files_is_said_to_have_one() {
        // From the headers alone every HTML newsletter looked as if it carried a file.
        assert!(!may_carry_files(
            b"Subject: x\r\nContent-Type: multipart/alternative; boundary=b\r\n\r\n--b\r\n"
        ));
        assert!(!may_carry_files(b"Subject: x\r\n\r\nBonjour"));
        assert!(may_carry_files(
            b"Content-Type: multipart/mixed; boundary=b\r\nSubject: x\r\n\r\n"
        ));
        assert!(may_carry_files(b"Content-Type: application/pdf\r\n\r\n"));
    }

    #[test]
    fn les_drapeaux_deduits_survivent_a_une_resynchronisation() {
        // Le serveur ne connaît pas « contient une pièce jointe » : les écraser les
        // ferait disparaître à chaque cycle.
        let local = Flags::SEEN | Flags::HAS_ATTACHMENT | Flags::HAS_TRACKER;
        let distant = Flags::ANSWERED;

        let fusion = merge_flags(local, distant);
        assert!(fusion.contains(Flags::ANSWERED));
        assert!(fusion.contains(Flags::HAS_ATTACHMENT));
        assert!(fusion.contains(Flags::HAS_TRACKER));
        assert!(
            !fusion.contains(Flags::SEEN),
            "le serveur fait foi pour « lu »"
        );
    }
}
