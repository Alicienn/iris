//! Leaving a mailing list from the message it sent.
//!
//! The list says how in its headers (`List-Unsubscribe`, RFC 2369), read by
//! `iris-mime` into one of three ways:
//!
//! - **one click** (RFC 8058): a POST to the list's address, done here without opening
//!   anything — the list promises that this alone unsubscribes;
//! - **a web page**: opened in the browser, where the list asks what it asks;
//! - **a message**: sent from the mailbox the list wrote to, as the list asked.

use crate::services::Services;
use iris_types::{Error, MessageId, Result, Unsubscribe};

/// How long the list's server is given to answer a one-click request.
const DELAI: std::time::Duration = std::time::Duration::from_secs(20);

/// How the list wants to be left, read again from the message: the headers are in its
/// stored source, whose body has been downloaded by the time it is read.
pub fn way_of(services: &Services, message: MessageId) -> Result<Unsubscribe> {
    let stored = services
        .store
        .message_by_id(message)?
        .ok_or_else(|| Error::other("the message is gone"))?;
    let hex = stored
        .body_blob
        .as_deref()
        .ok_or_else(|| Error::other("the message is still downloading"))?;
    let brut = iris_types::BlobId::from_hex(hex)
        .and_then(|id| services.blobs.get(id).ok().flatten())
        .ok_or_else(|| Error::other("the message could not be read"))?;
    iris_mime::parse(&brut)?
        .unsubscribe
        .ok_or_else(|| Error::other("this message does not say how to unsubscribe"))
}

/// The one-click request of RFC 8058: a POST of `List-Unsubscribe=One-Click` to the
/// list's HTTPS address. No cookie, no page: the address alone identifies the reader.
pub async fn one_click(url: &str) -> Result<()> {
    if !url.starts_with("https://") {
        return Err(Error::other("the list's address is not secure"));
    }
    let client = reqwest::Client::builder()
        .user_agent(format!("Iris/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(DELAI)
        .timeout(DELAI)
        .build()
        .map_err(|e| Error::other(format!("client: {e}")))?;
    let reponse = client
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("List-Unsubscribe=One-Click")
        .send()
        .await
        .map_err(|e| Error::other(format!("the list could not be reached ({e})")))?;
    if reponse.status().is_success() {
        Ok(())
    } else {
        Err(Error::other(format!(
            "the list answered {}",
            reponse.status()
        )))
    }
}

/// The message a list asks for, from the mailbox it wrote to: its subject when it
/// gives one, else "unsubscribe", which is what such addresses read.
pub fn request_by_mail(
    send: &iris_sync::SendService,
    account: iris_types::AccountId,
    addr: &str,
    subject: Option<&str>,
) -> Result<iris_smtp::SendHandle> {
    let sujet = subject
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("unsubscribe");
    let message = send.compose_new(account, addr, sujet, "unsubscribe")?;
    send.set_delay(std::time::Duration::ZERO);
    send.queue(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_http_address_is_not_posted_to() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let e = rt
            .block_on(one_click("http://lists.example.com/u/123"))
            .unwrap_err();
        assert!(e.to_string().contains("not secure"));
    }
}
