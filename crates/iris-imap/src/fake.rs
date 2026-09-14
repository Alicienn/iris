//! Un serveur IMAP simulé, en mémoire.
//!
//! La synchronisation est la partie la plus subtile du projet : reprise après
//! coupure, `UIDVALIDITY` qui change, drapeaux modifiés ailleurs, suppressions
//! constatées après coup. Aucun de ces cas ne se reproduit à volonté contre un vrai
//! serveur — d'où cette doublure, qui les met tous à portée d'un test.

use crate::{
    Capabilities, Connector, Credentials, Endpoint, FolderKind, IdleOutcome, ImapConnection,
    RawMessage, RemoteFolder, SelectedFolder, UidRange,
};
use async_trait::async_trait;
use iris_types::{Error, Flags, Result, Timestamp};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Un message stocké par le serveur simulé.
#[derive(Debug, Clone)]
pub struct FakeMessage {
    pub uid: u32,
    pub flags: Flags,
    pub internal_date: Timestamp,
    pub content: Vec<u8>,
}

/// Un dossier du serveur simulé.
#[derive(Debug, Clone)]
pub struct FakeFolder {
    pub kind: FolderKind,
    pub uid_validity: u32,
    pub messages: BTreeMap<u32, FakeMessage>,
    pub highest_modseq: u64,
    /// Numéro de modification de chaque message, pour `CONDSTORE`.
    pub modseqs: BTreeMap<u32, u64>,
}

impl Default for FakeFolder {
    fn default() -> Self {
        Self {
            kind: FolderKind::Other,
            uid_validity: 1,
            messages: BTreeMap::new(),
            highest_modseq: 1,
            modseqs: BTreeMap::new(),
        }
    }
}

/// L'état partagé du serveur simulé.
#[derive(Debug, Default)]
pub struct FakeState {
    pub folders: BTreeMap<String, FakeFolder>,
    /// Erreur à renvoyer à la prochaine commande, puis effacée.
    pub next_error: Option<String>,
    /// Refuser les connexions, pour éprouver la reprise.
    pub refuse_connections: bool,
    /// Résultat que doit rendre la prochaine attente `IDLE`.
    pub idle_result: Option<IdleOutcome>,
}

/// Serveur IMAP simulé.
#[derive(Debug, Clone)]
pub struct FakeServer {
    state: Arc<Mutex<FakeState>>,
    capabilities: Capabilities,
    connections: Arc<AtomicUsize>,
    next_uid: Arc<AtomicU32>,
}

impl Default for FakeServer {
    fn default() -> Self {
        Self::new(Capabilities {
            condstore: true,
            qresync: true,
            idle: true,
            r#move: true,
            uidplus: true,
        })
    }
}

impl FakeServer {
    pub fn new(capabilities: Capabilities) -> Self {
        let mut state = FakeState::default();
        state.folders.insert(
            "INBOX".into(),
            FakeFolder {
                kind: FolderKind::Inbox,
                ..Default::default()
            },
        );
        Self {
            state: Arc::new(Mutex::new(state)),
            capabilities,
            connections: Arc::new(AtomicUsize::new(0)),
            next_uid: Arc::new(AtomicU32::new(1)),
        }
    }

    /// Serveur dépourvu des extensions modernes, pour éprouver les replis.
    pub fn legacy() -> Self {
        Self::new(Capabilities::default())
    }

    pub fn add_folder(&self, path: &str, kind: FolderKind) {
        self.state.lock().unwrap().folders.insert(
            path.to_string(),
            FakeFolder {
                kind,
                ..Default::default()
            },
        );
    }

    /// Dépose un message et retourne son UID.
    pub fn deliver(&self, folder: &str, content: &[u8], flags: Flags) -> u32 {
        let uid = self.next_uid.fetch_add(1, Ordering::SeqCst);
        let mut state = self.state.lock().unwrap();
        let f = state.folders.entry(folder.to_string()).or_default();
        f.highest_modseq += 1;
        let modseq = f.highest_modseq;
        f.messages.insert(
            uid,
            FakeMessage {
                uid,
                flags,
                internal_date: Timestamp::from_millis(1_700_000_000_000 + uid as i64 * 1000),
                content: content.to_vec(),
            },
        );
        f.modseqs.insert(uid, modseq);
        uid
    }

    /// Modifie les drapeaux côté serveur, comme le ferait un autre client.
    pub fn set_flags_remotely(&self, folder: &str, uid: u32, flags: Flags) {
        let mut state = self.state.lock().unwrap();
        if let Some(f) = state.folders.get_mut(folder) {
            f.highest_modseq += 1;
            let modseq = f.highest_modseq;
            if let Some(m) = f.messages.get_mut(&uid) {
                m.flags = flags;
            }
            f.modseqs.insert(uid, modseq);
        }
    }

    pub fn remove(&self, folder: &str, uid: u32) {
        let mut state = self.state.lock().unwrap();
        if let Some(f) = state.folders.get_mut(folder) {
            f.messages.remove(&uid);
            f.modseqs.remove(&uid);
            f.highest_modseq += 1;
        }
    }

    /// Simule une boîte reconstruite côté serveur : tous les UID connus deviennent
    /// caducs.
    pub fn bump_uid_validity(&self, folder: &str) {
        let mut state = self.state.lock().unwrap();
        if let Some(f) = state.folders.get_mut(folder) {
            f.uid_validity += 1;
        }
    }

    pub fn fail_next(&self, message: &str) {
        self.state.lock().unwrap().next_error = Some(message.to_string());
    }

    pub fn refuse_connections(&self, refuse: bool) {
        self.state.lock().unwrap().refuse_connections = refuse;
    }

    pub fn set_idle_result(&self, outcome: IdleOutcome) {
        self.state.lock().unwrap().idle_result = Some(outcome);
    }

    /// Nombre de connexions ouvertes depuis la création.
    pub fn connection_count(&self) -> usize {
        self.connections.load(Ordering::Relaxed)
    }

    pub fn message_count(&self, folder: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .folders
            .get(folder)
            .map(|f| f.messages.len())
            .unwrap_or(0)
    }
}

#[async_trait]
impl Connector for FakeServer {
    async fn connect(
        &self,
        _endpoint: &Endpoint,
        credentials: &Credentials,
    ) -> Result<Box<dyn ImapConnection>> {
        if self.state.lock().unwrap().refuse_connections {
            return Err(Error::network("serveur simulé injoignable"));
        }
        if credentials.user().is_empty() {
            return Err(Error::AuthFailed {
                account: "(vide)".into(),
            });
        }
        self.connections.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(FakeConnection {
            state: Arc::clone(&self.state),
            capabilities: self.capabilities,
            selected: None,
        }))
    }
}

/// Une connexion au serveur simulé.
#[derive(Debug)]
pub struct FakeConnection {
    state: Arc<Mutex<FakeState>>,
    capabilities: Capabilities,
    selected: Option<String>,
}

impl FakeConnection {
    fn take_error(&self) -> Result<()> {
        if let Some(message) = self.state.lock().unwrap().next_error.take() {
            return Err(Error::Protocol {
                protocol: "IMAP",
                message,
            });
        }
        Ok(())
    }

    fn current(&self) -> Result<String> {
        self.selected.clone().ok_or_else(|| Error::Protocol {
            protocol: "IMAP",
            message: "aucun dossier sélectionné".into(),
        })
    }
}

#[async_trait]
impl ImapConnection for FakeConnection {
    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    async fn list_folders(&mut self) -> Result<Vec<RemoteFolder>> {
        self.take_error()?;
        let state = self.state.lock().unwrap();
        Ok(state
            .folders
            .iter()
            .map(|(path, f)| RemoteFolder {
                path: path.clone(),
                kind: f.kind,
            })
            .collect())
    }

    async fn select(&mut self, path: &str) -> Result<SelectedFolder> {
        self.take_error()?;
        let state = self.state.lock().unwrap();
        let f = state.folders.get(path).ok_or_else(|| Error::Protocol {
            protocol: "IMAP",
            message: format!("dossier « {path} » inconnu"),
        })?;

        let selected = SelectedFolder {
            uid_validity: f.uid_validity,
            uid_next: f.messages.keys().next_back().map(|u| u + 1).unwrap_or(1),
            exists: f.messages.len() as u32,
            highest_modseq: if self.capabilities.condstore {
                f.highest_modseq
            } else {
                0
            },
        };
        drop(state);
        self.selected = Some(path.to_string());
        Ok(selected)
    }

    async fn fetch_envelopes(&mut self, range: UidRange) -> Result<Vec<RawMessage>> {
        self.take_error()?;
        let path = self.current()?;
        let state = self.state.lock().unwrap();
        let f = state
            .folders
            .get(&path)
            .ok_or_else(|| Error::store("dossier disparu"))?;

        Ok(f.messages
            .range(range.from..=range.to)
            .map(|(_, m)| RawMessage {
                uid: m.uid,
                flags: m.flags,
                internal_date: m.internal_date,
                size: m.content.len() as u64,
                // Seuls les en-têtes : tout ce qui précède la ligne vide.
                content: headers_of(&m.content),
            })
            .collect())
    }

    async fn fetch_body(&mut self, uid: u32) -> Result<Vec<u8>> {
        self.take_error()?;
        let path = self.current()?;
        let state = self.state.lock().unwrap();
        state
            .folders
            .get(&path)
            .and_then(|f| f.messages.get(&uid))
            .map(|m| m.content.clone())
            .ok_or_else(|| Error::Protocol {
                protocol: "IMAP",
                message: format!("UID {uid} absent"),
            })
    }

    async fn existing_uids(&mut self, range: UidRange) -> Result<Vec<u32>> {
        self.take_error()?;
        let path = self.current()?;
        let state = self.state.lock().unwrap();
        let f = state
            .folders
            .get(&path)
            .ok_or_else(|| Error::store("dossier disparu"))?;
        Ok(f.messages
            .range(range.from..=range.to)
            .map(|(u, _)| *u)
            .collect())
    }

    async fn flags_changed_since(&mut self, modseq: u64) -> Result<Vec<(u32, Flags)>> {
        self.take_error()?;
        if !self.capabilities.condstore {
            return Err(Error::Protocol {
                protocol: "IMAP",
                message: "CONDSTORE non disponible".into(),
            });
        }
        let path = self.current()?;
        let state = self.state.lock().unwrap();
        let f = state
            .folders
            .get(&path)
            .ok_or_else(|| Error::store("dossier disparu"))?;

        Ok(f.modseqs
            .iter()
            .filter(|(_, m)| **m > modseq)
            .filter_map(|(uid, _)| f.messages.get(uid).map(|m| (*uid, m.flags)))
            .collect())
    }

    async fn store_flags(&mut self, uids: &[u32], flags: Flags, add: bool) -> Result<()> {
        self.take_error()?;
        let path = self.current()?;
        let mut state = self.state.lock().unwrap();
        let f = state
            .folders
            .get_mut(&path)
            .ok_or_else(|| Error::store("dossier disparu"))?;

        for uid in uids {
            f.highest_modseq += 1;
            let modseq = f.highest_modseq;
            if let Some(m) = f.messages.get_mut(uid) {
                m.flags = if add {
                    m.flags.with(flags)
                } else {
                    m.flags.without(flags)
                };
            }
            f.modseqs.insert(*uid, modseq);
        }
        Ok(())
    }

    async fn move_messages(&mut self, uids: &[u32], target: &str) -> Result<()> {
        self.take_error()?;
        let path = self.current()?;
        let mut state = self.state.lock().unwrap();

        if !state.folders.contains_key(target) {
            return Err(Error::Protocol {
                protocol: "IMAP",
                message: format!("dossier cible « {target} » inconnu"),
            });
        }

        let mut deplaces = Vec::new();
        if let Some(source) = state.folders.get_mut(&path) {
            for uid in uids {
                if let Some(m) = source.messages.remove(uid) {
                    source.modseqs.remove(uid);
                    source.highest_modseq += 1;
                    deplaces.push(m);
                }
            }
        }

        if let Some(cible) = state.folders.get_mut(target) {
            for m in deplaces {
                cible.highest_modseq += 1;
                let modseq = cible.highest_modseq;
                cible.modseqs.insert(m.uid, modseq);
                cible.messages.insert(m.uid, m);
            }
        }
        Ok(())
    }

    async fn append(&mut self, folder: &str, raw: &[u8], flags: Flags) -> Result<Option<u32>> {
        self.take_error()?;
        let mut state = self.state.lock().unwrap();
        if !state.folders.contains_key(folder) {
            return Err(Error::Protocol {
                protocol: "IMAP",
                message: format!("dossier « {folder} » inconnu"),
            });
        }

        // Le serveur simulé attribue l'UID suivant du dossier, comme le ferait un
        // vrai serveur doté de UIDPLUS.
        let f = state
            .folders
            .get_mut(folder)
            .expect("dossier vérifié juste avant");
        let uid = f.messages.keys().next_back().map(|u| u + 1).unwrap_or(1);
        f.highest_modseq += 1;
        let modseq = f.highest_modseq;
        f.messages.insert(
            uid,
            FakeMessage {
                uid,
                flags,
                internal_date: Timestamp::from_millis(1_700_000_000_000 + uid as i64 * 1000),
                content: raw.to_vec(),
            },
        );
        f.modseqs.insert(uid, modseq);
        Ok(Some(uid))
    }

    async fn idle(&mut self, _timeout: Duration) -> Result<IdleOutcome> {
        self.take_error()?;
        if !self.capabilities.idle {
            return Err(Error::Protocol {
                protocol: "IMAP",
                message: "IDLE non disponible".into(),
            });
        }
        Ok(self
            .state
            .lock()
            .unwrap()
            .idle_result
            .take()
            .unwrap_or(IdleOutcome::TimedOut))
    }

    async fn logout(&mut self) -> Result<()> {
        self.selected = None;
        Ok(())
    }
}

/// Extrait les en-têtes : tout ce qui précède la première ligne vide.
fn headers_of(content: &[u8]) -> Vec<u8> {
    let texte = String::from_utf8_lossy(content);
    match texte.find("\r\n\r\n") {
        Some(i) => content[..i + 4].to_vec(),
        None => match texte.find("\n\n") {
            Some(i) => content[..i + 2].to_vec(),
            None => content.to_vec(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(sujet: &str) -> Vec<u8> {
        format!("Subject: {sujet}\r\nFrom: a@x.fr\r\n\r\nCorps du message.\r\n").into_bytes()
    }

    async fn connexion(s: &FakeServer) -> Box<dyn ImapConnection> {
        s.connect(
            &Endpoint::tls("imap.x.fr", 993),
            &Credentials::Password {
                user: "a@x.fr".into(),
                password: "x".into(),
            },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn un_message_depose_est_recupere() {
        let s = FakeServer::default();
        s.deliver("INBOX", &message("Devis"), Flags::NONE);

        let mut c = connexion(&s).await;
        c.select("INBOX").await.unwrap();
        let messages = c.fetch_envelopes(UidRange::ALL).await.unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].uid, 1);
        assert!(String::from_utf8_lossy(&messages[0].content).contains("Devis"));
    }

    #[tokio::test]
    async fn les_enveloppes_ne_contiennent_pas_le_corps() {
        // La synchronisation initiale ne doit jamais télécharger les corps.
        let s = FakeServer::default();
        s.deliver("INBOX", &message("Devis"), Flags::NONE);

        let mut c = connexion(&s).await;
        c.select("INBOX").await.unwrap();
        let enveloppe = &c.fetch_envelopes(UidRange::ALL).await.unwrap()[0];
        assert!(!String::from_utf8_lossy(&enveloppe.content).contains("Corps du message"));

        let complet = c.fetch_body(1).await.unwrap();
        assert!(String::from_utf8_lossy(&complet).contains("Corps du message"));
    }

    #[tokio::test]
    async fn une_selection_rapporte_l_etat_du_dossier() {
        let s = FakeServer::default();
        for i in 0..3 {
            s.deliver("INBOX", &message(&format!("m{i}")), Flags::NONE);
        }

        let mut c = connexion(&s).await;
        let etat = c.select("INBOX").await.unwrap();
        assert_eq!(etat.exists, 3);
        assert_eq!(etat.uid_next, 4);
        assert!(etat.highest_modseq > 0);
    }

    #[tokio::test]
    async fn un_serveur_ancien_n_annonce_pas_de_modseq() {
        let s = FakeServer::legacy();
        s.deliver("INBOX", &message("m"), Flags::NONE);

        let mut c = connexion(&s).await;
        let etat = c.select("INBOX").await.unwrap();
        assert_eq!(etat.highest_modseq, 0);
        assert!(c.flags_changed_since(0).await.is_err());
    }

    #[tokio::test]
    async fn les_drapeaux_modifies_ailleurs_sont_detectes() {
        let s = FakeServer::default();
        s.deliver("INBOX", &message("un"), Flags::NONE);
        s.deliver("INBOX", &message("deux"), Flags::NONE);

        let mut c = connexion(&s).await;
        let etat = c.select("INBOX").await.unwrap();
        let repere = etat.highest_modseq;

        // Un autre client marque le premier message comme lu.
        s.set_flags_remotely("INBOX", 1, Flags::SEEN);

        let changements = c.flags_changed_since(repere).await.unwrap();
        assert_eq!(changements, [(1, Flags::SEEN)]);
    }

    #[tokio::test]
    async fn les_suppressions_se_constatent_par_les_uid_restants() {
        let s = FakeServer::default();
        for i in 0..5 {
            s.deliver("INBOX", &message(&format!("m{i}")), Flags::NONE);
        }
        s.remove("INBOX", 3);

        let mut c = connexion(&s).await;
        c.select("INBOX").await.unwrap();
        assert_eq!(c.existing_uids(UidRange::ALL).await.unwrap(), [1, 2, 4, 5]);
    }

    #[tokio::test]
    async fn un_changement_d_uidvalidity_est_visible() {
        let s = FakeServer::default();
        s.deliver("INBOX", &message("m"), Flags::NONE);

        let mut c = connexion(&s).await;
        let avant = c.select("INBOX").await.unwrap().uid_validity;
        s.bump_uid_validity("INBOX");
        let apres = c.select("INBOX").await.unwrap().uid_validity;

        assert_ne!(avant, apres);
    }

    #[tokio::test]
    async fn le_deplacement_transfere_le_message() {
        let s = FakeServer::default();
        s.add_folder("Archive", FolderKind::Archive);
        s.deliver("INBOX", &message("m"), Flags::NONE);

        let mut c = connexion(&s).await;
        c.select("INBOX").await.unwrap();
        c.move_messages(&[1], "Archive").await.unwrap();

        assert_eq!(s.message_count("INBOX"), 0);
        assert_eq!(s.message_count("Archive"), 1);
    }

    #[tokio::test]
    async fn un_deplacement_vers_un_dossier_inconnu_echoue() {
        let s = FakeServer::default();
        s.deliver("INBOX", &message("m"), Flags::NONE);

        let mut c = connexion(&s).await;
        c.select("INBOX").await.unwrap();
        assert!(c.move_messages(&[1], "Inexistant").await.is_err());
        assert_eq!(
            s.message_count("INBOX"),
            1,
            "le message ne doit pas disparaître"
        );
    }

    #[tokio::test]
    async fn poser_un_drapeau_est_visible_ensuite() {
        let s = FakeServer::default();
        s.deliver("INBOX", &message("m"), Flags::NONE);

        let mut c = connexion(&s).await;
        c.select("INBOX").await.unwrap();
        c.store_flags(&[1], Flags::SEEN, true).await.unwrap();
        assert_eq!(
            c.fetch_envelopes(UidRange::ALL).await.unwrap()[0].flags,
            Flags::SEEN
        );

        c.store_flags(&[1], Flags::SEEN, false).await.unwrap();
        assert_eq!(
            c.fetch_envelopes(UidRange::ALL).await.unwrap()[0].flags,
            Flags::NONE
        );
    }

    #[tokio::test]
    async fn une_commande_avant_selection_echoue() {
        let s = FakeServer::default();
        let mut c = connexion(&s).await;
        assert!(c.fetch_envelopes(UidRange::ALL).await.is_err());
    }

    #[tokio::test]
    async fn une_erreur_programmee_ne_frappe_qu_une_fois() {
        let s = FakeServer::default();
        s.fail_next("serveur occupé");

        let mut c = connexion(&s).await;
        assert!(c.list_folders().await.is_err());
        assert!(
            c.list_folders().await.is_ok(),
            "l'erreur ne doit pas persister"
        );
    }

    #[tokio::test]
    async fn un_serveur_injoignable_refuse_la_connexion() {
        let s = FakeServer::default();
        s.refuse_connections(true);

        let r = s
            .connect(
                &Endpoint::tls("x", 993),
                &Credentials::Password {
                    user: "a".into(),
                    password: "b".into(),
                },
            )
            .await;
        assert!(r.unwrap_err().is_transient());
    }

    #[tokio::test]
    async fn les_connexions_sont_comptees() {
        let s = FakeServer::default();
        let _a = connexion(&s).await;
        let _b = connexion(&s).await;
        assert_eq!(s.connection_count(), 2);
    }

    #[tokio::test]
    async fn l_attente_rend_le_resultat_programme() {
        let s = FakeServer::default();
        let mut c = connexion(&s).await;

        assert_eq!(
            c.idle(Duration::from_secs(1)).await.unwrap(),
            IdleOutcome::TimedOut
        );
        s.set_idle_result(IdleOutcome::Changed);
        assert_eq!(
            c.idle(Duration::from_secs(1)).await.unwrap(),
            IdleOutcome::Changed
        );
    }

    #[tokio::test]
    async fn un_message_depose_apparait_dans_le_dossier() {
        let s = FakeServer::default();
        s.add_folder("Sent", FolderKind::Sent);
        let mut c = connexion(&s).await;

        let uid = c
            .append("Sent", &message("Ma réponse"), Flags::SEEN)
            .await
            .unwrap();
        assert_eq!(uid, Some(1));
        assert_eq!(s.message_count("Sent"), 1);
    }

    #[tokio::test]
    async fn deposer_dans_un_dossier_inconnu_echoue() {
        let s = FakeServer::default();
        let mut c = connexion(&s).await;
        assert!(c.append("Inexistant", b"x", Flags::NONE).await.is_err());
    }

    #[test]
    fn l_extraction_des_en_tetes_gere_les_deux_conventions() {
        assert_eq!(headers_of(b"A: 1\r\n\r\ncorps"), b"A: 1\r\n\r\n");
        assert_eq!(headers_of(b"A: 1\n\ncorps"), b"A: 1\n\n");
        // Un message sans corps reste entier.
        assert_eq!(headers_of(b"A: 1\r\n"), b"A: 1\r\n");
    }
}
