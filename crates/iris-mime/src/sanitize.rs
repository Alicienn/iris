//! Assainissement du HTML des messages.
//!
//! Le corps d'un mail est du contenu hostile par défaut. Trois choses en sont
//! retirées sans condition :
//!
//! - **tout ce qui s'exécute** : scripts, gestionnaires d'événements, `javascript:` ;
//! - **tout ce qui envoie des données** : formulaires, `iframe`, `object` ;
//! - **tout ce qui appelle le réseau à l'insu du lecteur** : chaque image distante
//!   est neutralisée et remplacée par un marqueur, que l'interface décide ensuite de
//!   charger — ou non — via son proxy.
//!
//! Cette dernière règle est celle qui protège réellement : c'est par le simple
//! chargement d'une image que l'expéditeur apprend l'heure de lecture, l'adresse IP
//! et le client utilisé.

use std::collections::HashSet;

/// Résultat de l'assainissement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sanitized {
    /// HTML sûr, prêt à être rendu.
    pub html: String,
    /// Ressources distantes neutralisées, dans l'ordre d'apparition.
    pub blocked_remote: Vec<String>,
    /// Traqueurs identifiés parmi elles.
    pub trackers: Vec<Tracker>,
}

impl Sanitized {
    pub fn has_remote_content(&self) -> bool {
        !self.blocked_remote.is_empty()
    }

    pub fn is_tracked(&self) -> bool {
        !self.trackers.is_empty()
    }
}

/// Un pixel espion détecté.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tracker {
    pub url: String,
    pub reason: TrackerReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackerReason {
    /// Image d'un ou deux pixels : elle n'existe que pour être chargée.
    Invisible,
    /// Rendue invisible par une règle de style.
    Hidden,
    /// Domaine connu pour le pistage d'ouverture.
    KnownDomain,
}

impl TrackerReason {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Invisible => "image d'un pixel",
            Self::Hidden => "image masquée par une règle de style",
            Self::KnownDomain => "domaine de pistage connu",
        }
    }
}

/// Domaines dont la seule fonction est de mesurer l'ouverture des messages.
///
/// Liste volontairement courte et sûre : mieux vaut manquer un traqueur que
/// d'étiqueter à tort un expéditeur légitime.
const KNOWN_TRACKERS: &[&str] = &[
    "mailtrack.io",
    "streak.com",
    "bananatag.com",
    "yesware.com",
    "sidekickopen",
    "hubspotemail.net",
    "mailchimp.com/track",
    "list-manage.com/track",
    "sendgrid.net/wf/open",
    "mandrillapp.com/track",
    "getnotify.com",
    "didtheyreadit.com",
];

/// Assainit un corps HTML.
pub fn sanitize(html: &str) -> Sanitized {
    let inventaire = inventory(html);

    let mut builder = ammonia::Builder::default();

    // Étiquettes autorisées : de quoi rendre une mise en page de courriel, rien de
    // plus. `style` est absent volontairement — une feuille de style peut masquer du
    // contenu, appeler des polices distantes et recouvrir l'interface.
    builder
        .rm_tags([
            "script", "style", "iframe", "object", "embed", "form", "input", "button",
        ])
        .add_tags([
            "table", "thead", "tbody", "tfoot", "tr", "td", "th", "center", "font",
        ])
        .add_generic_attributes([
            "align",
            "valign",
            "bgcolor",
            "width",
            "height",
            "cellpadding",
            "cellspacing",
            "border",
            "color",
            "face",
            "size",
            "dir",
        ])
        .add_tag_attributes("img", ["alt", "title", "width", "height"])
        // Le style en ligne est conservé pour la mise en forme, mais nettoyé de ses
        // fonctions dangereuses par le filtre ci-dessous.
        .add_generic_attributes(["style"])
        .url_schemes(HashSet::from_iter([
            "http", "https", "mailto", "cid", "tel",
        ]))
        .link_rel(Some("noopener noreferrer nofollow"))
        .attribute_filter(|element, attribute, value| {
            match (element, attribute) {
                // Toute image distante est neutralisée : sa source est déplacée dans
                // un attribut inerte que l'interface pourra réactiver à la demande.
                ("img", "src") if is_remote(value) => {
                    Some(std::borrow::Cow::Borrowed("iris:blocked"))
                }
                (_, "style") => Some(std::borrow::Cow::Owned(clean_style(value))),
                _ => Some(std::borrow::Cow::Borrowed(value)),
            }
        });

    let html = builder.clean(html).to_string();

    Sanitized {
        html,
        blocked_remote: inventaire.remote,
        trackers: inventaire.trackers,
    }
}

struct Inventory {
    remote: Vec<String>,
    trackers: Vec<Tracker>,
}

/// Recense les ressources distantes et repère les traqueurs, avant nettoyage.
///
/// L'analyse se fait sur le HTML d'origine : après assainissement, les attributs de
/// dimension et de style qui trahissent un pixel espion ont pu disparaître.
fn inventory(html: &str) -> Inventory {
    let mut remote = Vec::new();
    let mut trackers = Vec::new();

    for balise in img_tags(html) {
        let Some(src) = attr(&balise, "src") else {
            continue;
        };
        if !is_remote(&src) {
            continue;
        }
        if !remote.contains(&src) {
            remote.push(src.clone());
        }

        let largeur = attr(&balise, "width").and_then(|v| parse_dim(&v));
        let hauteur = attr(&balise, "height").and_then(|v| parse_dim(&v));
        let style = attr(&balise, "style").unwrap_or_default().to_lowercase();

        let raison = if matches!(largeur, Some(d) if d <= 2) || matches!(hauteur, Some(d) if d <= 2)
        {
            Some(TrackerReason::Invisible)
        } else if style.contains("display:none")
            || style.contains("display: none")
            || style.contains("visibility:hidden")
            || style.contains("visibility: hidden")
        {
            Some(TrackerReason::Hidden)
        } else if is_known_tracker(&src) {
            Some(TrackerReason::KnownDomain)
        } else {
            None
        };

        if let Some(reason) = raison {
            if !trackers.iter().any(|t: &Tracker| t.url == src) {
                trackers.push(Tracker { url: src, reason });
            }
        }
    }

    Inventory { remote, trackers }
}

/// Extrait grossièrement les balises `img`. Suffisant pour l'inventaire : on cherche
/// des indices, pas à interpréter le document — c'est le rôle du moteur de rendu.
fn img_tags(html: &str) -> Vec<String> {
    let lower = html.to_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(start) = lower[from..].find("<img") {
        let start = from + start;
        let end = match html[start..].find('>') {
            Some(e) => start + e + 1,
            None => break,
        };
        out.push(html[start..end].to_string());
        from = end;
    }
    out
}

/// Lit un attribut dans une balise, entre guillemets simples, doubles ou sans.
///
/// Analyse volontairement sommaire : elle ne sert qu'a l'inventaire des ressources
/// et des traqueurs. L'interpretation reelle du document appartient au moteur de
/// rendu, pas a nous.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    // La mise en minuscules peut changer la longueur en octets pour quelques
    // caracteres exotiques. Dans ce cas les positions ne sont plus comparables : on
    // renvoie alors la valeur en minuscules, ce qui suffit a l'inventaire.
    let memes_positions = lower.len() == tag.len();

    let mut from = 0;
    while let Some(rel) = lower[from..].find(name) {
        let debut_nom = from + rel;
        let fin_nom = debut_nom + name.len();

        // Le nom doit etre un mot entier : sans cette garde, « width » trouverait
        // « data-width ».
        let precede_ok = debut_nom == 0
            || lower[..debut_nom]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);

        if precede_ok {
            let apres = &lower[fin_nom..];
            let sans_espaces = apres.trim_start();
            if let Some(valeur_et_suite) = sans_espaces.strip_prefix('=') {
                let decale = fin_nom + (apres.len() - sans_espaces.len()) + 1;
                let suite = valeur_et_suite;
                let saut = suite.len() - suite.trim_start().len();
                let debut_valeur = decale + saut;
                let suite = suite.trim_start();

                let (offset, longueur) = if let Some(reste) = suite.strip_prefix('"') {
                    (1, reste.find('"').unwrap_or(reste.len()))
                } else if let Some(reste) = suite.strip_prefix('\'') {
                    (1, reste.find('\'').unwrap_or(reste.len()))
                } else {
                    let brut = suite
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .trim_end_matches('>');
                    (0, brut.len())
                };

                let d = debut_valeur + offset;
                let f = d + longueur;
                let source = if memes_positions { tag } else { lower.as_str() };
                return source.get(d..f).map(str::to_string);
            }
        }
        from = fin_nom;
    }
    None
}

fn parse_dim(v: &str) -> Option<u32> {
    v.trim().trim_end_matches("px").trim().parse().ok()
}

fn is_remote(url: &str) -> bool {
    let u = url.trim().to_lowercase();
    u.starts_with("http://") || u.starts_with("https://") || u.starts_with("//")
}

fn is_known_tracker(url: &str) -> bool {
    let u = url.to_lowercase();
    KNOWN_TRACKERS.iter().any(|t| u.contains(t))
}

/// Retire d'une déclaration de style ce qui peut exécuter du code ou appeler le
/// réseau, en conservant le reste de la mise en forme.
fn clean_style(style: &str) -> String {
    style
        .split(';')
        .filter(|decl| {
            let d = decl.to_lowercase();
            !d.contains("url(")
                && !d.contains("expression(")
                && !d.contains("javascript:")
                && !d.contains("@import")
                && !d.contains("position:fixed")
                && !d.contains("position: fixed")
        })
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_scripts_disparaissent() {
        let s = sanitize("<p>Bonjour</p><script>alert('vole tes données')</script>");
        assert!(s.html.contains("Bonjour"));
        assert!(!s.html.to_lowercase().contains("script"));
        assert!(!s.html.contains("alert"));
    }

    #[test]
    fn les_gestionnaires_d_evenements_disparaissent() {
        let s = sanitize(r#"<div onclick="voler()" onmouseover="pister()">Texte</div>"#);
        assert!(!s.html.contains("onclick"));
        assert!(!s.html.contains("onmouseover"));
        assert!(s.html.contains("Texte"));
    }

    #[test]
    fn les_formulaires_et_cadres_disparaissent() {
        let s = sanitize(
            r#"<form action="https://pirate.example"><input name="mdp"></form>
               <iframe src="https://pirate.example"></iframe>"#,
        );
        assert!(!s.html.contains("<form"));
        assert!(!s.html.contains("<input"));
        assert!(!s.html.contains("<iframe"));
    }

    #[test]
    fn un_lien_javascript_est_neutralise() {
        let s = sanitize(r#"<a href="javascript:voler()">Cliquez</a>"#);
        assert!(!s.html.contains("javascript:"));
        assert!(s.html.contains("Cliquez"));
    }

    #[test]
    fn la_mise_en_page_par_tableaux_survit() {
        // Les infolettres réelles sont faites de tableaux imbriqués : les retirer
        // détruirait la mise en page de la majorité des messages commerciaux.
        let html = r#"<table cellpadding="8" width="600"><tr><td align="center">
                      <b>Contenu</b></td></tr></table>"#;
        let s = sanitize(html);
        assert!(s.html.contains("<table"));
        assert!(s.html.contains("<td"));
        assert!(s.html.contains("cellpadding"));
        assert!(s.html.contains("<b>Contenu</b>"));
    }

    #[test]
    fn une_image_distante_est_neutralisee_et_recensee() {
        let s = sanitize(r#"<img src="https://cdn.example.com/photo.jpg" alt="photo">"#);
        assert!(!s.html.contains("cdn.example.com"));
        assert!(s.html.contains("iris:blocked"));
        assert_eq!(s.blocked_remote, ["https://cdn.example.com/photo.jpg"]);
        assert!(s.has_remote_content());
    }

    #[test]
    fn une_image_embarquee_est_conservee() {
        // Les parties `cid:` viennent du message lui-même : les charger ne révèle
        // rien à personne.
        let s = sanitize(r#"<img src="cid:logo@entreprise" alt="logo">"#);
        assert!(s.html.contains("cid:logo@entreprise"));
        assert!(s.blocked_remote.is_empty());
    }

    #[test]
    fn un_pixel_d_un_pixel_est_signale() {
        let s = sanitize(r#"<img src="https://pisteur.example/o.gif" width="1" height="1">"#);
        assert_eq!(s.trackers.len(), 1);
        assert_eq!(s.trackers[0].reason, TrackerReason::Invisible);
        assert!(s.is_tracked());
    }

    #[test]
    fn une_image_masquee_par_style_est_signalee() {
        let s = sanitize(
            r#"<img src="https://pisteur.example/o.png" style="display:none" width="100">"#,
        );
        assert_eq!(s.trackers.len(), 1);
        assert_eq!(s.trackers[0].reason, TrackerReason::Hidden);
    }

    #[test]
    fn un_domaine_de_pistage_connu_est_signale() {
        let s = sanitize(r#"<img src="https://mailtrack.io/trace/abc" width="60" height="60">"#);
        assert_eq!(s.trackers[0].reason, TrackerReason::KnownDomain);
    }

    #[test]
    fn une_vraie_image_n_est_pas_prise_pour_un_traqueur() {
        // Bloquée, oui — signalée comme pistage, non. Confondre les deux rendrait
        // l'avertissement inutile.
        let s = sanitize(
            r#"<img src="https://cdn.example.com/banniere.jpg" width="600" height="200">"#,
        );
        assert!(s.has_remote_content());
        assert!(!s.is_tracked());
    }

    #[test]
    fn la_meme_image_n_est_recensee_qu_une_fois() {
        let s =
            sanitize(r#"<img src="https://x.example/a.png"><img src="https://x.example/a.png">"#);
        assert_eq!(s.blocked_remote.len(), 1);
    }

    #[test]
    fn le_style_perd_ses_appels_reseau_et_garde_la_mise_en_forme() {
        let s = sanitize(
            r#"<div style="color:#333; background:url(https://pisteur.example/p.gif); font-size:14px">T</div>"#,
        );
        assert!(s.html.contains("color:#333"));
        assert!(s.html.contains("font-size:14px"));
        assert!(!s.html.contains("pisteur.example"));
    }

    #[test]
    fn le_style_ne_peut_pas_recouvrir_l_interface() {
        let s = sanitize(r#"<div style="position:fixed; top:0; left:0">Recouvrement</div>"#);
        assert!(!s.html.contains("position:fixed"));
        assert!(s.html.contains("Recouvrement"));
    }

    #[test]
    fn un_corps_vide_reste_vide() {
        let s = sanitize("");
        assert_eq!(s.html, "");
        assert!(!s.has_remote_content());
        assert!(!s.is_tracked());
    }

    #[test]
    fn le_html_malforme_ne_fait_pas_paniquer() {
        let s = sanitize("<div><p>texte<img src=\"https://x.example/a.png\" <<>");
        assert!(s.html.contains("texte"));
    }

    #[test]
    fn lecture_d_attribut_sans_confusion_de_prefixe() {
        let tag = r#"<img data-width="9" width="1" src="https://x/a.gif">"#;
        assert_eq!(attr(tag, "width").as_deref(), Some("1"));
        assert_eq!(attr(tag, "src").as_deref(), Some("https://x/a.gif"));
        assert_eq!(attr(tag, "height"), None);
    }

    #[test]
    fn les_dimensions_en_pixels_sont_comprises() {
        assert_eq!(parse_dim("1"), Some(1));
        assert_eq!(parse_dim(" 1px "), Some(1));
        assert_eq!(parse_dim("100%"), None);
        assert_eq!(parse_dim("auto"), None);
    }
}
