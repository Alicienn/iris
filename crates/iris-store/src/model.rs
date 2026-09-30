//! Les structures échangées avec le store.

use iris_types::{AccountId, Flags, FolderId, MessageId, ThreadId, Timestamp, WorkflowState};
use serde::{Deserialize, Serialize};

/// Comment un compte s'authentifie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    Password,
    OAuthGoogle,
    OAuthMicrosoft,
}

impl AuthKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::OAuthGoogle => "oauth_google",
            Self::OAuthMicrosoft => "oauth_microsoft",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "oauth_google" => Self::OAuthGoogle,
            "oauth_microsoft" => Self::OAuthMicrosoft,
            _ => Self::Password,
        }
    }
}

/// Un compte configuré. Ne contient jamais de secret : ceux-ci vivent dans le
/// trousseau, désignés par l'adresse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: AccountId,
    pub email: String,
    pub display_name: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_tls: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_tls: bool,
    pub auth: AuthKind,
    /// Groupe libre choisi par l'utilisateur, pour ranger cent boîtes.
    pub group: Option<String>,
    pub pinned: bool,
    pub enabled: bool,
    pub created_at: Timestamp,
    pub last_activity_at: Timestamp,
    /// Ce qui est ajouté au bas des messages envoyés depuis ce compte.
    ///
    /// Par compte, parce que c'est la raison d'en avoir plusieurs : on ne signe pas une
    /// facture comme on écrit à sa sœur. Vide par défaut — personne ne veut découvrir
    /// une signature inventée par le programme au bas d'un message déjà parti.
    pub signature: String,
}

/// Description d'un compte à créer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAccount {
    pub email: String,
    pub display_name: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_tls: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_tls: bool,
    pub auth: AuthKind,
    pub group: Option<String>,
}

impl NewAccount {
    /// Configuration usuelle : IMAPS sur 993, SMTP sur 587 en STARTTLS.
    pub fn new(
        email: impl Into<String>,
        imap_host: impl Into<String>,
        smtp_host: impl Into<String>,
    ) -> Self {
        Self {
            email: email.into(),
            display_name: String::new(),
            imap_host: imap_host.into(),
            imap_port: 993,
            imap_tls: true,
            smtp_host: smtp_host.into(),
            smtp_port: 587,
            smtp_tls: true,
            auth: AuthKind::Password,
            group: None,
        }
    }
}

/// Rôle d'un dossier, déduit des attributs IMAP spéciaux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderRole {
    Inbox,
    Sent,
    Drafts,
    Trash,
    Junk,
    Archive,
    Other,
}

impl FolderRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Sent => "sent",
            Self::Drafts => "drafts",
            Self::Trash => "trash",
            Self::Junk => "junk",
            Self::Archive => "archive",
            Self::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "inbox" => Self::Inbox,
            "sent" => Self::Sent,
            "drafts" => Self::Drafts,
            "trash" => Self::Trash,
            "junk" => Self::Junk,
            "archive" => Self::Archive,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub id: FolderId,
    pub account: AccountId,
    pub path: String,
    pub role: FolderRole,
    /// `UIDVALIDITY` du dossier. Un changement invalide tous les UID connus.
    pub uid_validity: u32,
    pub uid_next: u32,
    /// `HIGHESTMODSEQ` pour la synchronisation incrémentale CONDSTORE.
    pub highest_modseq: u64,
}

/// Un message à insérer, tel que produit par la couche protocole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    pub account: AccountId,
    pub folder: FolderId,
    pub uid: u32,
    pub rfc_message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub subject: String,
    pub from_name: String,
    pub from_addr: String,
    /// Destinataires sérialisés, conservés tels quels : ils ne servent qu'à
    /// l'affichage du fil, jamais au filtrage rapide.
    pub recipients_json: String,
    pub date: Timestamp,
    pub received: Timestamp,
    pub size: u64,
    pub flags: Flags,
    pub preview: String,
}

/// Une ligne de la liste principale.
///
/// Tout y est **pré-rendu** : aucune jointure, aucune analyse, aucune allocation
/// supplémentaire au moment du défilement. C'est la structure qui matérialise
/// l'invariant « zéro entrée-sortie sur le thread d'affichage ».
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRow {
    pub id: ThreadId,
    pub state: WorkflowState,
    pub last_activity: Timestamp,
    pub from_display: String,
    pub subject: String,
    pub preview: String,
    pub message_count: u32,
    pub unread_count: u32,
    pub flags_union: Flags,
    pub snoozed_until: Option<Timestamp>,
    /// Le compte du dernier message du fil.
    ///
    /// Dénormalisé comme l'expéditeur et le sujet, et pour la même raison : la liste
    /// doit se servir sans jointure. Il porte la pastille de couleur, qui est la seule
    /// chose disant de quelle boîte un message vient quand on les regarde toutes.
    pub account: AccountId,
    /// Mis à la corbeille ici, et pas encore rangé par le serveur : hors des files.
    pub put_aside: bool,
}

impl ThreadRow {
    pub fn is_unread(&self) -> bool {
        self.unread_count > 0
    }

    pub fn has_attachment(&self) -> bool {
        self.flags_union.contains(Flags::HAS_ATTACHMENT)
    }

    /// Position de la ligne dans l'ordre de la liste, servant de curseur.
    pub fn cursor(&self) -> ListCursor {
        ListCursor {
            last_activity: self.last_activity,
            id: self.id,
        }
    }
}

/// Position dans une liste triée par `(last_activity DESC, id DESC)`.
///
/// Une pagination par curseur — et non par `OFFSET` — est ce qui garantit que la
/// millionième ligne coûte autant que la première, et qu'une insertion pendant le
/// défilement ne fait pas sauter ni répéter de ligne.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListCursor {
    pub last_activity: Timestamp,
    pub id: ThreadId,
}

/// Ce qu'une liste montre : une file de travail, ou un dossier.
///
/// Le courrier mis de côté — corbeille, indésirables — n'est **pas** un état du flux
/// de travail. C'était le cas, et c'était une erreur de conception que l'usage a
/// rendue visible tout de suite : la corbeille se retrouvait rangée dans « Terminé »,
/// où huit cents messages jetés noyaient les quelques dizaines que l'utilisateur avait
/// réellement traités. « Terminé » veut dire « je m'en suis occupé », pas « je l'ai
/// jeté ».
///
/// Un dossier est donc un dossier. Les trois files l'ignorent ; on y va en le
/// choisissant dans l'arborescence, et alors on voit ce qu'il contient.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Scope {
    /// Une file de travail : tout, sauf ce qui est mis de côté.
    #[default]
    Queue,
    /// Un dossier de rôle — boîte de réception, envoyés, indésirables, corbeille —
    /// tous comptes confondus. Le rôle plutôt que le chemin, parce qu'un serveur peut
    /// avoir deux dossiers d'indésirables et que l'utilisateur n'en a qu'un en tête.
    Role(FolderRole),
    /// Un dossier que quelqu'un a créé, par son nom.
    ///
    /// Un **nom**, pas un identifiant : un dossier est la même idée sur les cent
    /// boîtes — « Devis » est « Devis » — et le filtre le suit à travers elles.
    Path(String),
}

impl Scope {
    /// La liste doit-elle écarter le courrier mis de côté ?
    ///
    /// Non quand c'est précisément ce qu'on est allé chercher : ouvrir la corbeille
    /// pour la voir vide serait une plaisanterie.
    pub fn hides_put_aside(&self) -> bool {
        !matches!(
            self,
            Self::Role(FolderRole::Trash) | Self::Role(FolderRole::Junk)
        )
    }
}

/// The order of a list. By date it pages by cursor (the index's order); the others
/// page by position, which costs more deep in a list but a queue is not that deep.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    /// Newest first.
    #[default]
    Date,
    /// By the name of who wrote last, A to Z.
    Sender,
    /// By subject, A to Z.
    Subject,
    /// The biggest message first.
    Size,
}

impl Sort {
    pub fn from_index(i: i32) -> Self {
        match i {
            1 => Sort::Sender,
            2 => Sort::Subject,
            3 => Sort::Size,
            _ => Sort::Date,
        }
    }
}

/// Ce qu'on demande à la liste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListQuery {
    /// The order, and where a list not by date resumes (rows already read).
    pub sort: Sort,
    pub offset: u32,
    pub state: WorkflowState,
    /// Restriction à certains comptes. Vide signifie « tous ».
    pub accounts: Vec<AccountId>,
    /// Masquer les fils reportés dont l'échéance n'est pas atteinte.
    pub hide_snoozed_until: Option<Timestamp>,
    pub limit: u32,
    /// Reprendre après cette position. `None` pour commencer au début.
    pub after: Option<ListCursor>,
    /// Une file de travail, ou un dossier. Voir [`Scope`].
    pub scope: Scope,
    /// Ce qui réduit la liste sans changer d'endroit. Voir [`Filters`].
    pub filters: Filters,
}

/// Les filtres rapides, au-dessus de la liste.
///
/// La recherche répond à « où est ce message dont je me souviens » ; un filtre répond à
/// « lesquels de ces deux cents me restent ». Ce sont deux gestes différents, et sur une
/// file de cent quatre-vingt-huit c'est le second qu'on fait le plus souvent.
///
/// Cumulatifs, et par un **et** : « non lus avec une pièce jointe » est la question
/// qu'on se pose, jamais « non lus ou avec une pièce jointe ».
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Filters {
    pub unread: bool,
    pub attachments: bool,
    pub starred: bool,
}

impl Filters {
    pub fn is_empty(&self) -> bool {
        !self.unread && !self.attachments && !self.starred
    }

    /// Combien sont allumés. Sert à le dire à l'écran sans recompter ailleurs.
    pub fn count(&self) -> usize {
        [self.unread, self.attachments, self.starred]
            .iter()
            .filter(|b| **b)
            .count()
    }
}

impl ListQuery {
    pub fn new(state: WorkflowState, limit: u32) -> Self {
        Self {
            sort: Sort::Date,
            offset: 0,
            state,
            accounts: Vec::new(),
            hide_snoozed_until: None,
            limit,
            after: None,
            scope: Scope::Queue,
            filters: Filters::default(),
        }
    }

    /// Ne montrer que ce que ces filtres laissent passer.
    pub fn filtered(mut self, filters: Filters) -> Self {
        self.filters = filters;
        self
    }

    /// Ne montrer que ce dossier, sur tous les comptes qui en ont un.
    pub fn in_folder(mut self, folder: impl Into<String>) -> Self {
        let nom = folder.into();
        self.scope = if nom.trim().is_empty() {
            Scope::Queue
        } else {
            Scope::Path(nom)
        };
        self
    }

    /// Ne montrer qu'un dossier de rôle, tous comptes confondus.
    pub fn in_role(mut self, role: FolderRole) -> Self {
        self.scope = Scope::Role(role);
        self
    }

    pub fn for_accounts(mut self, accounts: Vec<AccountId>) -> Self {
        self.accounts = accounts;
        self
    }

    pub fn after(mut self, cursor: ListCursor) -> Self {
        self.after = Some(cursor);
        self
    }

    pub fn hiding_snoozed(mut self, now: Timestamp) -> Self {
        self.hide_snoozed_until = Some(now);
        self
    }
}

/// Nature d'une opération en attente de réconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    SetFlags,
    MoveMessage,
    DeleteMessage,
    SendMessage,
    AppendMessage,
    CreateFolder,
}

impl OpKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SetFlags => "set_flags",
            Self::MoveMessage => "move_message",
            Self::DeleteMessage => "delete_message",
            Self::SendMessage => "send_message",
            Self::AppendMessage => "append_message",
            Self::CreateFolder => "create_folder",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "set_flags" => Some(Self::SetFlags),
            "move_message" => Some(Self::MoveMessage),
            "delete_message" => Some(Self::DeleteMessage),
            "send_message" => Some(Self::SendMessage),
            "append_message" => Some(Self::AppendMessage),
            "create_folder" => Some(Self::CreateFolder),
            _ => None,
        }
    }

    /// Une opération rejouable peut être tentée plusieurs fois sans dommage.
    /// L'envoi ne l'est pas : il doit être confirmé avant toute nouvelle tentative.
    pub const fn is_naturally_idempotent(self) -> bool {
        !matches!(self, Self::SendMessage)
    }
}

/// Une opération locale en attente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOp {
    pub id: iris_types::OpId,
    pub account: AccountId,
    pub kind: OpKind,
    pub payload: String,
    pub idempotency_key: String,
    pub created_at: Timestamp,
    pub attempts: u32,
    pub next_attempt_at: Timestamp,
    pub last_error: Option<String>,
}

/// Quelqu'un à qui on a déjà eu affaire.
///
/// La seule mémoire que l'application garde des gens, et elle est faite d'un compte et
/// d'une date : rien d'inféré, rien de deviné, seulement ce qui est arrivé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    /// L'adresse, en minuscules — c'est la clé.
    pub address: String,
    /// Le nom affiché la dernière fois qu'il en portait un. Souvent vide.
    pub display: String,
    /// Combien de messages reçus. Sert à classer, pas à juger.
    pub seen: u32,
}

impl Contact {
    /// Ce qu'on écrit dans un champ de destinataire.
    pub fn to_header(&self) -> String {
        if self.display.trim().is_empty() {
            self.address.clone()
        } else {
            format!("{} <{}>", self.display.trim(), self.address)
        }
    }
}

/// Un message tel que relu du store, pour l'affichage d'un fil.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMessage {
    pub id: MessageId,
    pub account: AccountId,
    pub folder: FolderId,
    pub thread: ThreadId,
    pub uid: u32,
    pub rfc_message_id: Option<String>,
    pub subject: String,
    pub from_name: String,
    pub from_addr: String,
    pub date: Timestamp,
    pub received: Timestamp,
    pub size: u64,
    pub flags: Flags,
    pub preview: String,
    pub body_blob: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_enumerations_font_un_aller_retour() {
        for k in [
            AuthKind::Password,
            AuthKind::OAuthGoogle,
            AuthKind::OAuthMicrosoft,
        ] {
            assert_eq!(AuthKind::parse(k.as_str()), k);
        }
        for r in [
            FolderRole::Inbox,
            FolderRole::Sent,
            FolderRole::Drafts,
            FolderRole::Trash,
            FolderRole::Junk,
            FolderRole::Archive,
            FolderRole::Other,
        ] {
            assert_eq!(FolderRole::parse(r.as_str()), r);
        }
        for o in [
            OpKind::SetFlags,
            OpKind::MoveMessage,
            OpKind::DeleteMessage,
            OpKind::SendMessage,
            OpKind::AppendMessage,
        ] {
            assert_eq!(OpKind::parse(o.as_str()), Some(o));
        }
    }

    #[test]
    fn une_valeur_inconnue_retombe_sur_un_defaut_sur() {
        // Une base écrite par une version future ne doit pas faire paniquer.
        assert_eq!(AuthKind::parse("quantique"), AuthKind::Password);
        assert_eq!(FolderRole::parse("quantique"), FolderRole::Other);
        // Une opération inconnue, en revanche, ne peut pas être devinée.
        assert_eq!(OpKind::parse("quantique"), None);
    }

    #[test]
    fn l_envoi_n_est_pas_naturellement_idempotent() {
        assert!(!OpKind::SendMessage.is_naturally_idempotent());
        assert!(OpKind::SetFlags.is_naturally_idempotent());
    }

    #[test]
    fn les_valeurs_par_defaut_d_un_compte_sont_les_usages_courants() {
        let a = NewAccount::new("moi@example.com", "imap.example.com", "smtp.example.com");
        assert_eq!(a.imap_port, 993);
        assert_eq!(a.smtp_port, 587);
        assert!(a.imap_tls && a.smtp_tls);
    }

    #[test]
    fn le_curseur_d_une_ligne_la_designe() {
        let row = ThreadRow {
            id: ThreadId(7),
            state: WorkflowState::Todo,
            last_activity: Timestamp::from_millis(500),
            from_display: "Marie".into(),
            subject: "Devis".into(),
            preview: "Bonjour".into(),
            message_count: 3,
            unread_count: 1,
            flags_union: Flags::HAS_ATTACHMENT,
            snoozed_until: None,
            account: AccountId(1),
            put_aside: false,
        };
        assert!(row.is_unread());
        assert!(row.has_attachment());
        assert_eq!(row.cursor().id, ThreadId(7));
        assert_eq!(row.cursor().last_activity, Timestamp::from_millis(500));
    }
}
