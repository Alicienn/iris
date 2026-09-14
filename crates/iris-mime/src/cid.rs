//! Les images qu'un message transporte lui-même.
//!
//! Une signature d'entreprise est presque toujours un petit tableau HTML avec un logo,
//! et ce logo n'est pas une adresse sur le web : c'est une partie du message, jointe
//! au reste et référencée par `<img src="cid:quelquechose">`. Rien ne peut l'aller
//! chercher — il est déjà là.
//!
//! Ces images étaient donc invisibles, et pour une raison qui n'a rien à voir avec le
//! blocage du contenu distant : elles ne sont pas distantes. C'est ce module qui les
//! remet dans le corps, en remplaçant chaque `cid:` par les octets correspondants sous
//! forme d'URL `data:`.
//!
//! Fait ici, sur le HTML déjà assaini, plutôt que dans le moteur de rendu : le moteur
//! n'a pas le message sous la main, et lui donner un accès au stockage pour aller
//! chercher une pièce jointe reviendrait à lui donner un accès au stockage.

use crate::InlinePart;

/// Remplace chaque `src="cid:…"` par les octets de la partie correspondante.
///
/// Ce qu'on ne trouve pas est laissé tel quel. Un `cid:` orphelin — la partie a été
/// perdue, le message est mal formé — donne une image cassée, ce qui est exactement ce
/// qu'il est ; le remplacer par autre chose serait inventer.
pub fn inline_images(html: &str, parts: &[InlinePart]) -> String {
    if parts.is_empty() || !html.contains("cid:") {
        return html.to_string();
    }

    let mut out = String::with_capacity(html.len());
    let mut reste = html;

    while let Some(debut) = reste.find("cid:") {
        out.push_str(&reste[..debut]);
        let apres = &reste[debut + 4..];

        // L'identifiant court jusqu'au guillemet, à l'espace ou au chevron qui ferme
        // l'attribut. Un `Content-ID` n'en contient aucun.
        let fin = apres
            .find(|c: char| c == '"' || c == '\'' || c == '>' || c.is_whitespace())
            .unwrap_or(apres.len());
        let (identifiant, suite) = apres.split_at(fin);

        match parts.iter().find(|p| p.content_id == identifiant) {
            Some(partie) => {
                out.push_str("data:");
                out.push_str(&partie.mime_type);
                out.push_str(";base64,");
                out.push_str(&encode(&partie.bytes));
            }
            None => {
                out.push_str("cid:");
                out.push_str(identifiant);
            }
        }

        reste = suite;
    }

    out.push_str(reste);
    out
}

/// Encodage base64, sans dépendance.
///
/// Vingt lignes contre une caisse de plus dans l'arbre : cet encodeur n'a qu'un
/// appelant, ne voit que des octets que nous venons de lire nous-mêmes, et n'a aucun
/// cas d'erreur — il n'y a rien à se tromper dans un encodage sans entrée invalide.
fn encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for morceau in bytes.chunks(3) {
        let b = [
            morceau[0],
            *morceau.get(1).unwrap_or(&0),
            *morceau.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);

        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if morceau.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if morceau.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn partie(id: &str, bytes: &[u8]) -> InlinePart {
        InlinePart {
            content_id: id.into(),
            mime_type: "image/png".into(),
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn un_logo_de_signature_est_remis_dans_le_corps() {
        let html = r#"<p>Cordialement</p><img src="cid:logo@boite" alt="logo">"#;
        let out = inline_images(html, &[partie("logo@boite", b"Hi")]);
        assert!(out.contains("src=\"data:image/png;base64,SGk=\""), "{out}");
        assert!(!out.contains("cid:"));
    }

    #[test]
    fn un_cid_orphelin_reste_tel_quel() {
        // La partie manque : l'image est cassée, et c'est exactement ce qu'elle est.
        // Mettre autre chose à sa place serait inventer un contenu.
        let html = r#"<img src="cid:disparu">"#;
        assert_eq!(inline_images(html, &[partie("autre", b"x")]), html);
    }

    #[test]
    fn plusieurs_images_sont_toutes_remises() {
        let html = r#"<img src="cid:a"><img src="cid:b">"#;
        let out = inline_images(html, &[partie("a", b"Hi"), partie("b", b"Yo")]);
        assert!(out.contains("SGk="), "{out}");
        assert!(out.contains("WW8="), "{out}");
    }

    #[test]
    fn un_corps_sans_cid_n_est_pas_recopie_inutilement() {
        let html = "<p>rien à faire</p>";
        assert_eq!(inline_images(html, &[partie("a", b"x")]), html);
    }

    #[test]
    fn sans_partie_le_corps_est_rendu_intact() {
        let html = r#"<img src="cid:a">"#;
        assert_eq!(inline_images(html, &[]), html);
    }

    #[test]
    fn l_encodage_suit_le_rfc() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn les_octets_hauts_survivent() {
        // Un PNG commence par 0x89 : un encodeur qui traiterait les octets comme du
        // texte signé le corromprait dès le premier.
        assert_eq!(encode(&[0x89, 0x50, 0x4e, 0x47]), "iVBORw==");
    }
}
