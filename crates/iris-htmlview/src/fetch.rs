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
//!   la forme sous laquelle un message transporte ses propres images. Toujours
//!   autorisé.
//! - `http:` et `https:` — **uniquement** quand le lecteur l'a demandé pour ce
//!   message. Une image distante est un accusé de lecture adressé à l'expéditeur ;
//!   c'est une décision, pas un détail de rendu.
//! - tout le reste — refusé sans bruit. Un moteur de rendu n'a rien à faire dans le
//!   système de fichiers.
//!
//! Les octets sont remis au gestionnaire que Blitz fournit avec chaque requête, qui
//! les décode et les dépose dans la file du document ; `resolve` les y reprend.
//!
//! Une image démesurée est réduite **avant** d'arriver au moteur. Décodée telle quelle,
//! une photo de 6000 × 4000 envoyée en pièce incrustée coûte 96 Mo de pixels pour être
//! affichée sur 800 de large. Elle est ramenée ici à [`MAX_DIMENSION`] de côté, et le
//! moteur ne voit jamais l'original.

use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request};

/// Au-delà, ce n'est plus un logo de signature.
///
/// Une limite en octets et non en pixels : ce qui coûte ici est le transfert, pas la
/// surface, et un fichier de dix mégaoctets retient le message pendant qu'on le
/// télécharge.
const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Le plus grand côté d'une image remise au moteur, en pixels.
///
/// Le corps est mis en page sur 800 points de large, 1600 pixels sur un écran à 200 %.
/// Rien de plus grand ne peut s'afficher à sa taille.
pub const MAX_DIMENSION: u32 = 1600;

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

impl NetProvider for MailNetProvider {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url;

        let octets = match url.scheme() {
            "data" => decode_data_url(url.as_str()),
            "http" | "https" if self.allow_remote => fetch_remote(url.as_str()),
            _ => None,
        };

        let Some(octets) = octets else {
            return;
        };

        // Le décodage appartient au gestionnaire — c'est lui qui sait si ces octets
        // sont une image, une police ou une feuille de style. Seules les images trop
        // grandes sont retouchées avant.
        handler.bytes(url.to_string(), Bytes::from(reduce_if_huge(octets)));
    }
}

/// Réduit une image dont un côté dépasse [`MAX_DIMENSION`].
///
/// Les dimensions se lisent dans l'en-tête, sans décoder : une image normale ne coûte
/// ici que cette lecture. Ce qui n'est pas une image lisible — une feuille de style, une
/// police — repart tel quel.
fn reduce_if_huge(octets: Vec<u8>) -> Vec<u8> {
    let Ok(lecteur) = image::ImageReader::new(std::io::Cursor::new(&octets)).with_guessed_format()
    else {
        return octets;
    };
    if lecteur.format().is_none() {
        return octets;
    }
    let Ok((l, h)) = lecteur.into_dimensions() else {
        return octets;
    };
    if l.max(h) <= MAX_DIMENSION {
        return octets;
    }

    let Ok(image) = image::load_from_memory(&octets) else {
        return octets;
    };
    let reduite = image.thumbnail(MAX_DIMENSION, MAX_DIMENSION);
    let mut sortie = std::io::Cursor::new(Vec::new());
    match reduite.write_to(&mut sortie, image::ImageFormat::Png) {
        Ok(()) => sortie.into_inner(),
        Err(_) => octets,
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
/// requête est aussi anonyme que possible — pas de redirection, pas de référent, pas
/// de cookie — parce que tout ce qui l'accompagne est une information de plus donnée
/// à qui cherchait déjà à savoir si le message avait été ouvert.
#[cfg(feature = "remote-images")]
fn fetch_remote(url: &str) -> Option<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        // Une image qui renvoie ailleurs est le comportement d'un pixel de suivi, et
        // suivre la chaîne revient à confirmer la lecture deux fois plutôt qu'une.
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
        // « Hi » en base64.
        assert_eq!(
            decode_data_url("data:image/png;base64,SGk=").as_deref(),
            Some(&b"Hi"[..])
        );
    }

    #[test]
    fn les_blancs_ne_cassent_pas_le_decodage() {
        assert_eq!(
            decode_data_url("data:image/png;base64,SG\n k =").as_deref(),
            Some(&b"Hi"[..])
        );
    }

    #[test]
    fn une_url_data_sans_base64_est_refusee() {
        // Refusée plutôt qu'interprétée : deviner l'encodage reviendrait à décider à
        // la place de l'expéditeur ce qu'il a voulu envoyer.
        assert_eq!(decode_data_url("data:text/plain,bonjour"), None);
    }

    #[test]
    fn le_distant_est_refuse_sans_autorisation() {
        // C'est la propriété que le bandeau de contenu bloqué promet : tant que le
        // lecteur n'a rien demandé, rien ne part sur le réseau.
        assert!(!MailNetProvider::new(false).allow_remote);
    }

    fn png(l: u32, h: u32) -> Vec<u8> {
        let mut sortie = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(l, h)
            .write_to(&mut sortie, image::ImageFormat::Png)
            .unwrap();
        sortie.into_inner()
    }

    #[test]
    fn une_image_demesuree_est_reduite_avant_le_moteur() {
        let reduite = reduce_if_huge(png(4000, 1000));
        let (l, h) = image::load_from_memory(&reduite)
            .unwrap()
            .to_rgb8()
            .dimensions();
        assert_eq!(l, MAX_DIMENSION);
        assert_eq!(h, 400, "les proportions sont gardées");
    }

    #[test]
    fn une_image_raisonnable_passe_intacte() {
        let octets = png(600, 200);
        assert_eq!(reduce_if_huge(octets.clone()), octets);
    }

    #[test]
    fn ce_qui_n_est_pas_une_image_passe_intact() {
        let css = b"body { color: red }".to_vec();
        assert_eq!(reduce_if_huge(css.clone()), css);
    }
}
