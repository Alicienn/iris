//! Le chargeur de ressources du moteur complet.
//!
//! Blitz ne va chercher aucune ressource par lui-même : tout passe par un
//! `NetProvider`, et tant qu'aucun n'était installé, **aucune image ne s'affichait
//! jamais** — ni les images distantes, ni celles que le message transporte lui-même.
//! Le bandeau « N images bloquées » avait donc raison sur le fond et son bouton
//! « Montrer » n'avait nulle part où aller.
//!
//! Celui-ci résout trois choses et refuse tout le reste :
//!
//! - `data:` — les octets sont dans l'URL. Aucun réseau, donc aucun risque, et c'est
//!   la forme sous laquelle les pièces incrustées d'un message arrivent une fois
//!   inlinées. Toujours autorisé.
//! - `http:` et `https:` — **uniquement** quand le lecteur l'a demandé pour ce
//!   message. Une image distante est un accusé de lecture adressé à l'expéditeur ;
//!   c'est une décision, pas un détail de rendu.
//! - tout le reste — refusé sans bruit. Un moteur de rendu n'a rien à faire dans le
//!   système de fichiers.

use blitz_traits::net::{BoxedHandler, NetProvider, Request, SharedCallback};
use std::sync::Arc;

/// Au-delà, ce n'est plus un logo de signature.
///
/// Une limite en octets et non en pixels : ce qui coûte ici est le transfert, pas la
/// surface, et un fichier de dix mégaoctets bloque le rendu du message pendant qu'on
/// le télécharge.
const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Un message doit s'afficher, même quand le serveur d'en face ne répond pas.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);

/// Le fournisseur de ressources.
#[derive(Debug)]
pub struct MailNetProvider {
    allow_remote: bool,
}

impl MailNetProvider {
    pub fn new(allow_remote: bool) -> Self {
        Self { allow_remote }
    }
}

impl<D: 'static> NetProvider<D> for MailNetProvider {
    fn fetch(&self, doc_id: usize, request: Request, handler: BoxedHandler<D>) {
        let url = request.url;

        let octets = match url.scheme() {
            "data" => decode_data_url(url.as_str()),
            "http" | "https" if self.allow_remote => fetch_remote(url.as_str()),
            _ => None,
        };

        // Le rendu est synchrone : la ressource doit être livrée avant que la mise en
        // page ne reprenne, sinon l'image arrive après l'image qui la contient.
        if let Some(octets) = octets {
            handler.bytes(
                doc_id,
                octets.into(),
                Arc::new(|_, _| {}) as SharedCallback<D>,
            );
        }
    }
}

/// Décode une URL `data:`.
///
/// Base64 seulement, et déclaré comme tel : les autres formes existent et n'arrivent
/// jamais dans du courrier. Deviner ce que veut dire une URL mal formée reviendrait à
/// interpréter à la place de l'expéditeur.
fn decode_data_url(url: &str) -> Option<Vec<u8>> {
    let (entete, charge) = url.strip_prefix("data:")?.split_once(',')?;
    if !entete.to_ascii_lowercase().contains("base64") {
        return None;
    }
    base64_decode(charge.trim())
}

/// Décodage base64, sans dépendance.
///
/// Trente lignes contre une caisse entière : ce décodeur n'a qu'un appelant, ne voit
/// que des chaînes déjà passées par l'assainissement, et se trompe en renvoyant
/// `None`, ce qui vaut une image manquante et rien d'autre.
fn base64_decode(texte: &str) -> Option<Vec<u8>> {
    fn valeur(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a') as u32 + 26),
            b'0'..=b'9' => Some((c - b'0') as u32 + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    }

    let mut out = Vec::with_capacity(texte.len() / 4 * 3);
    let mut accumulateur: u32 = 0;
    let mut bits = 0;

    for c in texte.bytes() {
        // Les blancs et le remplissage sont ignorés : un `data:` recopié sur
        // plusieurs lignes reste un `data:` valide.
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = valeur(c)?;
        accumulateur = (accumulateur << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulateur >> bits) as u8);
        }
    }

    Some(out)
}

/// Va chercher une ressource distante.
///
/// Bloquant, et volontairement : la mise en page a besoin des octets maintenant. La
/// requête est anonyme au possible — pas de cookie, pas de redirection vers un autre
/// schéma, pas de référent — parce que tout ce qui l'accompagne est une information
/// de plus donnée à qui cherchait déjà à savoir si le message avait été ouvert.
#[cfg(feature = "remote-images")]
fn fetch_remote(url: &str) -> Option<Vec<u8>> {
    // Un client par appel : on en fait un par image demandée, ce qui n'arrive que
    // lorsque le lecteur a cliqué, et jamais pendant un défilement.
    let client = reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        // Pas de redirection : une image qui renvoie ailleurs est le comportement des
        // pixels de suivi, et suivre la chaîne revient à confirmer la lecture deux
        // fois plutôt qu'une.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;

    let reponse = client.get(url).header("Accept", "image/*").send().ok()?;
    if !reponse.status().is_success() {
        return None;
    }

    // La taille annoncée d'abord, quand elle l'est : refuser après avoir tout
    // téléchargé ne protège de rien.
    if reponse.content_length().is_some_and(|n| n > MAX_BYTES) {
        return None;
    }

    let octets = reponse.bytes().ok()?;
    (octets.len() as u64 <= MAX_BYTES).then(|| octets.to_vec())
}

#[cfg(not(feature = "remote-images"))]
fn fetch_remote(_url: &str) -> Option<Vec<u8>> {
    let _ = (MAX_BYTES, TIMEOUT);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_une_image_inline() {
        // "Hi" en base64.
        let url = "data:image/png;base64,SGk=";
        assert_eq!(decode_data_url(url).as_deref(), Some(&b"Hi"[..]));
    }

    #[test]
    fn les_blancs_ne_cassent_pas_le_decodage() {
        let url = "data:image/png;base64,SG\n k =";
        assert_eq!(decode_data_url(url).as_deref(), Some(&b"Hi"[..]));
    }

    #[test]
    fn une_url_data_sans_base64_est_refusee() {
        // Refusée plutôt qu'interprétée : deviner l'encodage reviendrait à décider à
        // la place de l'expéditeur ce qu'il a voulu envoyer.
        assert_eq!(decode_data_url("data:text/plain,bonjour"), None);
    }

    #[test]
    fn le_distant_est_refuse_par_defaut() {
        // Sans autorisation explicite du lecteur, rien ne part sur le réseau. C'est
        // la propriété que le bandeau de contenu bloqué promet.
        let provider = MailNetProvider::new(false);
        assert!(!provider.allow_remote);
    }
}
