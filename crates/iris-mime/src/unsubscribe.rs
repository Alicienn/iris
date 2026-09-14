//! Extraction du désabonnement.
//!
//! Deux mécanismes coexistent dans la nature :
//!
//! - **RFC 2369** (`List-Unsubscribe`) : une liste de liens `http` ou `mailto`. Le
//!   lien web ouvre un navigateur et aboutit souvent à un formulaire.
//! - **RFC 8058** (`List-Unsubscribe-Post`) : ajoute la promesse qu'une simple
//!   requête `POST` suffit. C'est le seul cas où l'on peut désabonner l'utilisateur
//!   sans le sortir de l'application, et il est donc toujours préféré.

use iris_types::Unsubscribe;

/// Analyse les deux en-têtes et retourne le meilleur moyen disponible.
pub fn parse(
    list_unsubscribe: Option<&str>,
    list_unsubscribe_post: Option<&str>,
) -> Option<Unsubscribe> {
    let raw = list_unsubscribe?;
    let entries = split_entries(raw);

    let one_click = list_unsubscribe_post
        .map(|v| v.to_lowercase().contains("one-click"))
        .unwrap_or(false);

    // Le lien web d'abord s'il est utilisable en un clic, sinon on préfère quand même
    // le web au mailto : ouvrir un navigateur reste moins déroutant qu'envoyer un
    // message dont l'utilisateur ne verra jamais la réponse.
    let http = entries.iter().find(|e| {
        let l = e.to_lowercase();
        l.starts_with("http://") || l.starts_with("https://")
    });

    if let Some(url) = http {
        return Some(if one_click {
            Unsubscribe::OneClick { url: url.clone() }
        } else {
            Unsubscribe::Http { url: url.clone() }
        });
    }

    let mailto = entries
        .iter()
        .find(|e| e.to_lowercase().starts_with("mailto:"))?;
    let sans_schema = &mailto[7..];
    let (addr, query) = match sans_schema.split_once('?') {
        Some((a, q)) => (a, Some(q)),
        None => (sans_schema, None),
    };
    let subject = query.and_then(|q| {
        q.split('&').find_map(|p| {
            let (k, v) = p.split_once('=')?;
            k.eq_ignore_ascii_case("subject").then(|| percent_decode(v))
        })
    });

    Some(Unsubscribe::Mailto {
        addr: addr.to_string(),
        subject,
    })
}

/// Découpe la valeur de l'en-tête en entrées, chacune entourée de chevrons.
fn split_entries(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut reste = raw;
    while let Some(debut) = reste.find('<') {
        let Some(fin) = reste[debut..].find('>') else {
            break;
        };
        let entree = reste[debut + 1..debut + fin].trim();
        if !entree.is_empty() {
            out.push(entree.to_string());
        }
        reste = &reste[debut + fin + 1..];
    }
    out
}

/// Décodage des séquences `%XX`, suffisant pour un sujet d'en-tête.
fn percent_decode(s: &str) -> String {
    let bytes = s.replace('+', " ");
    let bytes = bytes.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sans_en_tete_il_n_y_a_pas_de_desabonnement() {
        assert_eq!(parse(None, None), None);
        assert_eq!(parse(Some("   "), None), None);
    }

    #[test]
    fn un_lien_web_seul_donne_un_desabonnement_web() {
        let u = parse(Some("<https://exemple.fr/unsub?id=42>"), None).unwrap();
        assert_eq!(
            u,
            Unsubscribe::Http {
                url: "https://exemple.fr/unsub?id=42".into()
            }
        );
    }

    #[test]
    fn l_en_tete_post_transforme_le_lien_en_un_clic() {
        let u = parse(
            Some("<https://exemple.fr/unsub?id=42>"),
            Some("List-Unsubscribe=One-Click"),
        )
        .unwrap();
        assert_eq!(
            u,
            Unsubscribe::OneClick {
                url: "https://exemple.fr/unsub?id=42".into()
            }
        );
    }

    #[test]
    fn le_lien_web_est_prefere_au_mailto() {
        let u = parse(
            Some("<mailto:stop@exemple.fr>, <https://exemple.fr/unsub>"),
            None,
        )
        .unwrap();
        assert_eq!(
            u,
            Unsubscribe::Http {
                url: "https://exemple.fr/unsub".into()
            }
        );
    }

    #[test]
    fn un_mailto_seul_est_analyse_avec_son_sujet() {
        let u = parse(
            Some("<mailto:stop@exemple.fr?subject=D%C3%A9sabonnement>"),
            None,
        )
        .unwrap();
        assert_eq!(
            u,
            Unsubscribe::Mailto {
                addr: "stop@exemple.fr".into(),
                subject: Some("Désabonnement".into()),
            }
        );
    }

    #[test]
    fn un_mailto_sans_sujet_reste_valide() {
        let u = parse(Some("<mailto:stop@exemple.fr>"), None).unwrap();
        assert_eq!(
            u,
            Unsubscribe::Mailto {
                addr: "stop@exemple.fr".into(),
                subject: None
            }
        );
    }

    #[test]
    fn un_en_tete_sans_chevrons_est_ignore() {
        // Le RFC les impose ; sans eux on ne peut pas délimiter les entrées de façon
        // fiable, et deviner produirait des désabonnements vers de mauvaises adresses.
        assert_eq!(parse(Some("https://exemple.fr/unsub"), None), None);
    }

    #[test]
    fn les_espaces_et_retours_a_la_ligne_sont_tolerés() {
        let u = parse(
            Some("< https://exemple.fr/unsub >,\r\n <mailto:x@y.fr>"),
            None,
        )
        .unwrap();
        assert_eq!(
            u,
            Unsubscribe::Http {
                url: "https://exemple.fr/unsub".into()
            }
        );
    }

    #[test]
    fn le_signe_plus_vaut_espace_dans_le_sujet() {
        let u = parse(Some("<mailto:s@x.fr?subject=Me+retirer>"), None).unwrap();
        match u {
            Unsubscribe::Mailto { subject, .. } => {
                assert_eq!(subject.as_deref(), Some("Me retirer"))
            }
            other => panic!("attendu un mailto, obtenu {other:?}"),
        }
    }

    #[test]
    fn un_pourcentage_invalide_ne_fait_pas_paniquer() {
        let u = parse(Some("<mailto:s@x.fr?subject=100%zz>"), None).unwrap();
        match u {
            Unsubscribe::Mailto { subject, .. } => assert_eq!(subject.as_deref(), Some("100%zz")),
            other => panic!("attendu un mailto, obtenu {other:?}"),
        }
    }
}
