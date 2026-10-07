//! Each message leaves through the mailbox it is from.
//!
//! There was one sender for the whole application, built at start-up from the first
//! enabled mailbox with its password. Whatever address a message was written from, it
//! went out through that mailbox's server, signed in as it: refused, or delivered as
//! mail one server sends for another's domain, which is what spam filters look for. A
//! mailbox signed in with Google or Microsoft has no password at all, so it could not
//! send in any case.
//!
//! The sender is now chosen per message, from its `From`: the mailbox with that
//! address, or the one the address is an alias of. Its credentials are read at the
//! moment it goes, so an OAuth token is fresh and a password changed since start-up
//! is the new one.

use iris_imap::Credentials;
use iris_smtp::{LettreMailer, Mailer, Outgoing, SendOutcome, SmtpLogin};
use iris_store::{Account, Store};
use iris_sync::send::sender_account;
use iris_sync::CredentialsProvider;
use iris_types::Result;
use std::sync::Arc;

/// Builds the sender for one mailbox, signed in as it.
pub type Connect = Arc<dyn Fn(&Account, SmtpLogin) -> Result<Arc<dyn Mailer>> + Send + Sync>;

/// Sends each message through its own mailbox's server.
pub struct AccountMailer {
    store: Arc<Store>,
    credentials: Arc<dyn CredentialsProvider>,
    connect: Connect,
}

impl std::fmt::Debug for AccountMailer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountMailer").finish_non_exhaustive()
    }
}

impl AccountMailer {
    /// Over the network, with `lettre`.
    pub fn new(store: Arc<Store>, credentials: Arc<dyn CredentialsProvider>) -> Self {
        Self::with_connect(
            store,
            credentials,
            Arc::new(|compte: &Account, login: SmtpLogin| {
                let expediteur = LettreMailer::new(
                    &compte.smtp_host,
                    compte.smtp_port,
                    compte.smtp_tls,
                    &compte.email,
                    &login,
                )?;
                Ok(Arc::new(expediteur) as Arc<dyn Mailer>)
            }),
        )
    }

    /// With a sender of one's own, for the tests.
    pub fn with_connect(
        store: Arc<Store>,
        credentials: Arc<dyn CredentialsProvider>,
        connect: Connect,
    ) -> Self {
        Self {
            store,
            credentials,
            connect,
        }
    }
}

#[async_trait::async_trait]
impl Mailer for AccountMailer {
    async fn send(&self, message: &Outgoing) -> Result<SendOutcome> {
        let compte = sender_account(&self.store, &message.from.addr)?;
        let login = match self
            .credentials
            .credentials(compte.id, &compte.email)
            .await?
        {
            Credentials::Password { password, .. } => SmtpLogin::Password(password),
            Credentials::OAuth2 { token, .. } => SmtpLogin::OAuth2(token),
        };
        (self.connect)(&compte, login)?.send(message).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_smtp::FakeMailer;
    use iris_store::{AuthKind, NewAccount};
    use iris_types::{AccountId, Address, Timestamp};
    use std::sync::Mutex;

    /// Each mailbox signs in as itself: a password of its own, or a token.
    #[derive(Debug)]
    struct PerAccount;

    #[async_trait::async_trait]
    impl CredentialsProvider for PerAccount {
        async fn credentials(&self, _account: AccountId, email: &str) -> Result<Credentials> {
            Ok(if email.ends_with("@gmail.example.com") {
                Credentials::OAuth2 {
                    user: email.into(),
                    token: format!("token of {email}"),
                }
            } else {
                Credentials::Password {
                    user: email.into(),
                    password: format!("password of {email}"),
                }
            })
        }
    }

    struct Fixture {
        mailer: AccountMailer,
        /// Which server each message went through, signed in how.
        through: Arc<Mutex<Vec<(String, SmtpLogin)>>>,
        sent: Arc<FakeMailer>,
        store: Arc<Store>,
    }

    fn fixture() -> Fixture {
        let store = Arc::new(Store::in_memory().unwrap());
        for (email, smtp, auth) in [
            (
                "first@example.com",
                "smtp.first.example.com",
                AuthKind::Password,
            ),
            (
                "second@example.org",
                "smtp.second.example.org",
                AuthKind::Password,
            ),
            (
                "me@gmail.example.com",
                "smtp.gmail.example.com",
                AuthKind::OAuthGoogle,
            ),
        ] {
            let mut nouveau = NewAccount::new(email, "imap.example.com", smtp);
            nouveau.auth = auth;
            store.create_account(&nouveau, Timestamp::EPOCH).unwrap();
        }

        let through = Arc::new(Mutex::new(Vec::new()));
        let sent = Arc::new(FakeMailer::new());
        let (t, s) = (Arc::clone(&through), Arc::clone(&sent));
        let mailer = AccountMailer::with_connect(
            Arc::clone(&store),
            Arc::new(PerAccount),
            Arc::new(move |compte: &Account, login: SmtpLogin| {
                t.lock().unwrap().push((compte.smtp_host.clone(), login));
                Ok(Arc::clone(&s) as Arc<dyn Mailer>)
            }),
        );
        Fixture {
            mailer,
            through,
            sent,
            store,
        }
    }

    fn from(address: &str) -> Outgoing {
        Outgoing::new(
            Address::new(address),
            vec![Address::new("someone@example.net")],
            "Hello",
        )
    }

    #[tokio::test]
    async fn a_message_leaves_through_its_own_mailbox_not_the_first() {
        let f = fixture();
        f.mailer.send(&from("second@example.org")).await.unwrap();

        let through = f.through.lock().unwrap().clone();
        assert_eq!(
            through,
            [(
                "smtp.second.example.org".to_string(),
                SmtpLogin::Password("password of second@example.org".into())
            )]
        );
        assert_eq!(f.sent.count(), 1);
    }

    #[tokio::test]
    async fn a_google_mailbox_sends_with_its_token() {
        let f = fixture();
        f.mailer.send(&from("Me@Gmail.Example.com")).await.unwrap();

        let through = f.through.lock().unwrap().clone();
        assert_eq!(
            through,
            [(
                "smtp.gmail.example.com".to_string(),
                SmtpLogin::OAuth2("token of me@gmail.example.com".into())
            )]
        );
    }

    #[tokio::test]
    async fn an_alias_leaves_through_the_mailbox_it_belongs_to() {
        let f = fixture();
        let second = sender_account(&f.store, "second@example.org").unwrap();
        f.store
            .add_alias(second.id, "sales@example.org", "Sales")
            .unwrap();

        f.mailer.send(&from("sales@example.org")).await.unwrap();
        assert_eq!(f.through.lock().unwrap()[0].0, "smtp.second.example.org");
    }

    #[tokio::test]
    async fn an_address_no_mailbox_owns_is_not_sent() {
        let f = fixture();
        let erreur = f
            .mailer
            .send(&from("stranger@example.net"))
            .await
            .unwrap_err();
        assert!(erreur.to_string().contains("no mailbox"), "{erreur}");
        assert_eq!(f.sent.count(), 0);
    }
}
