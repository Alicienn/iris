//! Mise en forme des données affichées.
//!
//! Tout ce qui est calculé ici l'est **une fois, hors du rendu**. Une ligne de liste
//! qui formaterait sa propre date referait ce travail soixante fois par seconde
//! pendant un défilement, pour un texte qui ne change pas.

use iris_types::Timestamp;

/// Formate une date pour la liste, relativement à maintenant.
///
/// Trois formes selon l'ancienneté : l'heure pour aujourd'hui, le jour pour la
/// semaine écoulée, la date pour le reste. C'est la convention que tout le monde
/// connaît, et elle tient dans la largeur disponible.
pub fn relative_date(date: Timestamp, now: Timestamp) -> String {
    // Dans le fuseau de la machine. Les jours et les heures étaient comptés en UTC :
    // un message reçu à 17 h 21 à Paris s'affichait « 15:21 », et un message de 1 h du
    // matin tombait la veille.
    let local = |t: Timestamp| Timestamp::from_millis(t.millis() + local_offset_ms(t));
    relative_date_utc(local(date), local(now))
}

/// L'écart du fuseau de la machine avec UTC, à un instant donné (l'heure d'été
/// compte : il dépend de la date).
fn local_offset_ms(t: Timestamp) -> i64 {
    use chrono::{Offset, TimeZone};
    chrono::Local
        .timestamp_millis_opt(t.millis())
        .single()
        .map(|d| d.offset().fix().local_minus_utc() as i64 * 1000)
        .unwrap_or(0)
}

/// Le même calcul, sur des instants déjà décalés dans le fuseau voulu.
fn relative_date_utc(date: Timestamp, now: Timestamp) -> String {
    let ecart = now.seconds() - date.seconds();

    if ecart < 0 {
        // Une date future : l'expéditeur a mal réglé son horloge, ou triche pour
        // rester en tête de liste. On affiche la date brute plutôt que « dans 3 ans ».
        return absolute_date(date);
    }

    let jour_local = |t: Timestamp| t.seconds().div_euclid(86_400);
    let jours = jour_local(now) - jour_local(date);

    match jours {
        0 => time_of_day(date),
        1 => "Yesterday".to_string(),
        2..=6 => weekday(date).to_string(),
        _ => absolute_date(date),
    }
}

fn time_of_day(t: Timestamp) -> String {
    let reste = t.seconds().rem_euclid(86_400);
    format!("{:02}:{:02}", reste / 3600, (reste % 3600) / 60)
}

/// Day of the week. 1 January 1970 was a Thursday.
fn weekday(t: Timestamp) -> &'static str {
    const DAYS: [&str; 7] = [
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
    ];
    DAYS[t.seconds().div_euclid(86_400).rem_euclid(7) as usize]
}

fn absolute_date(t: Timestamp) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (year, month, day) = civil_from_days(t.seconds().div_euclid(86_400));
    format!("{} {day}, {year}", MONTHS[(month - 1) as usize])
}

/// Conversion jours depuis l'époque → date civile (algorithme de Howard Hinnant).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Couleur d'identité d'un compte, dérivée de son adresse.
///
/// Le thème par défaut n'a pas d'accent : c'est la seule couleur de l'interface, et
/// elle sert uniquement à distinguer cent boîtes d'un coup d'œil.
///
/// Elle était à 32 % de saturation, ce qui, sur une barre de trois pixels posée sur un
/// fond presque noir, ne se distinguait de rien. La discrétion visée était atteinte au
/// point de supprimer la fonction : une couleur qu'on ne voit pas ne distingue pas
/// cent boîtes, elle ne fait que coûter trois pixels par ligne. À 58 % elle se lit
/// sans dominer, ce qui est le point où elle commence à servir.
pub fn account_tint(email: &str) -> (u8, u8, u8) {
    // Répartition sur le cercle chromatique par un condensé stable : le même compte
    // garde sa couleur d'une session à l'autre.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in email.trim().to_lowercase().bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    let teinte = (hash % 360) as f32;
    hsl_to_rgb(teinte, 0.58, 0.66)
}

/// Les initiales d'un expéditeur, pour sa pastille dans la liste.
///
/// Deux lettres au plus : celles des deux premiers mots d'un nom, ou la première d'une
/// adresse quand il n'y a pas de nom. Ce qui n'est pas une lettre ou un chiffre est
/// sauté — « "Marie" <m@x> » donne « M », pas un guillemet.
pub fn initials(sender: &str) -> String {
    let nom = sender.split('<').next().unwrap_or(sender);
    let nom = nom.split('@').next().unwrap_or(nom);
    nom.split(|c: char| c.is_whitespace() || c == '.' || c == '_' || c == '-')
        .filter_map(|mot| mot.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    )
}

/// Sujet affichable : le sujet, ou une mention explicite s'il est vide.
pub fn display_subject(subject: &str) -> String {
    let s = subject.trim();
    if s.is_empty() {
        "(sans objet)".to_string()
    } else {
        s.to_string()
    }
}

/// Taille lisible d'une pièce jointe.
pub fn human_size(bytes: u64) -> String {
    const SEUIL: u64 = 1024;
    if bytes < SEUIL {
        return format!("{bytes} o");
    }
    let ko = bytes as f64 / 1024.0;
    if ko < 1024.0 {
        return format!("{ko:.0} ko");
    }
    let mo = ko / 1024.0;
    if mo < 1024.0 {
        return format!("{mo:.1} Mo");
    }
    format!("{:.1} Go", mo / 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_initiales_viennent_du_nom_ou_de_l_adresse() {
        assert_eq!(initials("Marie Durand"), "MD");
        assert_eq!(initials("marie.durand@example.com"), "MD");
        assert_eq!(initials("contact@example.com"), "C");
        assert_eq!(initials("\"Huile Direct\" <news@example.com>"), "HD");
        assert_eq!(initials("Jean Paul Sartre"), "JP");
        assert_eq!(initials("élodie"), "É");
        assert_eq!(initials(""), "");
    }

    /// 14 novembre 2023, 22 h 13 UTC — un mardi.
    fn now() -> Timestamp {
        Timestamp::from_millis(1_700_000_000_000)
    }

    #[test]
    fn un_seul_ne_prend_pas_de_s() {
        assert_eq!(plural(1, "remote image"), "1 remote image");
        assert_eq!(plural(2, "remote image"), "2 remote images");
    }

    #[test]
    fn zero_est_pluriel() {
        // « 0 message » se dit en français et « 0 messages » en anglais. L'application
        // est en anglais.
        assert_eq!(plural(0, "message"), "0 messages");
    }

    #[test]
    fn un_pluriel_irregulier_se_donne_en_entier() {
        assert_eq!(plural_of(1, "reply", "replies"), "1 reply");
        assert_eq!(plural_of(3, "reply", "replies"), "3 replies");
    }

    fn il_y_a(secondes: i64) -> Timestamp {
        Timestamp::from_millis(now().millis() - secondes * 1000)
    }

    #[test]
    fn aujourd_hui_affiche_l_heure() {
        let d = relative_date_utc(il_y_a(3600), now());
        assert_eq!(d, "21:13");
    }

    #[test]
    fn hier_est_nomme() {
        assert_eq!(relative_date_utc(il_y_a(86_400), now()), "Yesterday");
    }

    #[test]
    fn la_semaine_ecoulee_affiche_le_jour() {
        let d = relative_date_utc(il_y_a(3 * 86_400), now());
        assert_eq!(d, "Saturday");
    }

    #[test]
    fn au_dela_la_date_est_absolue() {
        let d = relative_date_utc(il_y_a(30 * 86_400), now());
        assert_eq!(d, "Oct 15, 2023");
    }

    #[test]
    fn une_date_future_n_est_pas_annoncee_comme_telle() {
        // Un expéditeur qui date son message en 2099 pour rester en tête de liste ne
        // doit pas obtenir un affichage privilégié.
        let futur = Timestamp::from_millis(now().millis() + 365 * 86_400_000);
        let d = relative_date_utc(futur, now());
        assert!(d.contains("2024"), "obtenu « {d} »");
    }

    #[test]
    fn les_jours_de_la_semaine_sont_corrects() {
        // 1 January 1970 was a Thursday.
        assert_eq!(weekday(Timestamp::from_millis(0)), "Thursday");
        assert_eq!(weekday(Timestamp::from_millis(86_400_000)), "Friday");
    }

    #[test]
    fn la_teinte_d_un_compte_est_stable() {
        // Le même compte doit garder sa couleur d'une session à l'autre.
        assert_eq!(
            account_tint("marie@example.com"),
            account_tint("marie@example.com")
        );
        assert_eq!(
            account_tint(" MARIE@Example.COM "),
            account_tint("marie@example.com")
        );
    }

    #[test]
    fn deux_comptes_ont_des_teintes_distinctes() {
        let a = account_tint("contact@example.com");
        let b = account_tint("facturation@example.com");
        assert_ne!(a, b);
    }

    #[test]
    fn les_teintes_sont_visibles_sans_dominer() {
        // Les deux bornes ont chacune leur défaut. Trop peu saturée, la couleur ne se
        // distingue de rien sur une barre de trois pixels et la fonction disparaît —
        // c'est ce qui se passait à 32 %. Trop saturée, elle attire l'œil que la liste
        // veut garder sur le texte.
        for adresse in ["a@x.fr", "b@y.fr", "c@z.fr", "compta@entreprise.com"] {
            let (r, g, b) = account_tint(adresse);
            let max = r.max(g).max(b) as i32;
            let min = r.min(g).min(b) as i32;
            assert!(
                max - min >= 60,
                "« {adresse} » donne une teinte invisible : {r},{g},{b}"
            );
            assert!(
                max - min < 190,
                "« {adresse} » donne une teinte trop saturée : {r},{g},{b}"
            );
            assert!(
                max > 100 && max < 240,
                "luminosité hors plage pour « {adresse} »"
            );
        }
    }

    #[test]
    fn un_sujet_vide_est_annonce() {
        assert_eq!(display_subject(""), "(sans objet)");
        assert_eq!(display_subject("   "), "(sans objet)");
        assert_eq!(display_subject(" Devis "), "Devis");
    }

    #[test]
    fn les_tailles_sont_lisibles() {
        assert_eq!(human_size(512), "512 o");
        assert_eq!(human_size(2048), "2 ko");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 Mo");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 Go");
    }

    #[test]
    fn la_conversion_de_date_civile_est_juste() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_675), (2023, 11, 14));
    }
}

/// A count, shortened so it fits in a tab.
///
/// A queue of 1 240 000 is not usefully different from 1 230 000, and the two spellings
/// differ by six characters that push the label out of its pill. Three significant
/// figures is the most anyone reads off a counter at a glance; the exact number is
/// still one hover away, which is where an exact number belongs.
pub fn short_count(n: u64) -> String {
    // Only the trailing zero is dropped: "1.2k" is worth four characters, "1.0k" is
    // not — it says "one thousand" in the space where "1k" says it better.
    fn trim(whole: u64, frac: u64, digits: usize, suffix: char) -> String {
        if frac == 0 {
            format!("{whole}{suffix}")
        } else {
            format!("{whole}.{frac:0digits$}{suffix}")
        }
    }

    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => trim(n / 1_000, (n % 1_000) / 100, 1, 'k'),
        1_000_000..=999_999_999 => trim(n / 1_000_000, (n % 1_000_000) / 10_000, 2, 'M'),
        _ => trim(n / 1_000_000_000, (n % 1_000_000_000) / 10_000_000, 2, 'G'),
    }
}

/// A count and its noun, with the noun in the right number.
///
/// "1 remote image(s) blocked" is the kind of thing a program says and a person never
/// does. The parenthesis is there because writing the branch felt like work, and it
/// appears in the one place the user is being asked to make a decision — whether to
/// load something a stranger sent. Sounding like a form at that moment is not free.
///
/// Only regular plurals. An irregular one — "one reply, two replies" — is passed whole
/// as `plural`, which is shorter than any rule that would guess it right.
pub fn plural(n: u64, singular: &str) -> String {
    plural_of(n, singular, &format!("{singular}s"))
}

/// The same, when the plural is not the singular plus an *s*.
pub fn plural_of(n: u64, singular: &str, plural: &str) -> String {
    format!("{n} {}", if n == 1 { singular } else { plural })
}

/// The same count in full, grouped in threes.
///
/// A thin space rather than a comma or a full stop: both of those mean the decimal
/// separator to half the people who will read this, and the group separator to the
/// other half.
pub fn grouped_count(n: u64) -> String {
    let chiffres = n.to_string();
    let mut out = String::with_capacity(chiffres.len() + chiffres.len() / 3);

    for (i, c) in chiffres.chars().enumerate() {
        if i > 0 && (chiffres.len() - i) % 3 == 0 {
            out.push('\u{202f}');
        }
        out.push(c);
    }

    out
}

#[cfg(test)]
mod tests_counts {
    use super::*;

    #[test]
    fn shortens_the_way_a_reader_expects() {
        assert_eq!(short_count(0), "0");
        assert_eq!(short_count(999), "999");
        assert_eq!(short_count(1_000), "1k");
        assert_eq!(short_count(1_250), "1.2k");
        assert_eq!(short_count(12_540), "12.5k");
        assert_eq!(short_count(999_999), "999.9k");
        assert_eq!(short_count(1_000_000), "1M");
        assert_eq!(short_count(1_240_000), "1.24M");
        assert_eq!(short_count(1_204_000), "1.20M");
    }

    #[test]
    fn groups_in_threes() {
        assert_eq!(grouped_count(1), "1");
        assert_eq!(grouped_count(999), "999");
        assert_eq!(grouped_count(1_250), "1\u{202f}250");
        assert_eq!(grouped_count(1_240_000), "1\u{202f}240\u{202f}000");
    }
}

/// Le type d'une pièce jointe, en un mot et en une icône.
///
/// Réduit à ce qu'un lecteur a besoin de savoir avant d'ouvrir : est-ce un document,
/// une image, une feuille de calcul, une archive. Le type MIME complet —
/// `application/vnd.openxmlformats-officedocument.spreadsheetml.sheet` — est exact et
/// n'aide personne à décider s'il faut cliquer.
///
/// Le nom de l'icône est un mot, pas un chemin : le jeu d'icônes appartient à
/// l'interface, et faire traverser des données SVG dans ce sens mettrait des dessins
/// dans du code Rust, où plus personne ne penserait à les tenir à jour.
pub fn attachment_kind(filename: &str, mime: &str) -> (&'static str, &'static str) {
    let extension = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();

    // Le type déclaré d'abord, l'extension ensuite. Un serveur qui annonce `image/png`
    // sait mieux que le nom du fichier ; un fichier nommé `.pdf` et servi en
    // `application/octet-stream` — ce que font beaucoup de serveurs — se rattrape par
    // son extension.
    if mime.starts_with("image/") {
        return ("Image", "image");
    }
    if mime.starts_with("video/") {
        return ("Video", "image");
    }
    if mime.starts_with("audio/") {
        return ("Audio", "image");
    }

    match extension.as_str() {
        "pdf" => ("PDF", "file-text"),
        "doc" | "docx" | "odt" | "rtf" => ("Document", "file-text"),
        "xls" | "xlsx" | "ods" | "csv" => ("Spreadsheet", "table"),
        "ppt" | "pptx" | "odp" => ("Slides", "image"),
        "zip" | "rar" | "7z" | "tar" | "gz" => ("Archive", "archive"),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "heic" => ("Image", "image"),
        "txt" | "md" | "log" => ("Text", "file-text"),
        "eml" | "msg" => ("Message", "envelope-closed"),
        "" => ("File", "paperclip"),
        autre if autre.len() <= 4 => ("File", "paperclip"),
        _ => ("File", "paperclip"),
    }
}

#[cfg(test)]
mod tests_attachments {
    use super::*;

    #[test]
    fn le_type_declare_prime_sur_le_nom() {
        // Un serveur qui annonce « image/png » sait mieux que le nom du fichier.
        assert_eq!(attachment_kind("scan.dat", "image/png").0, "Image");
    }

    #[test]
    fn l_extension_rattrape_un_type_generique() {
        // Beaucoup de serveurs servent tout en « application/octet-stream ».
        assert_eq!(
            attachment_kind("devis.pdf", "application/octet-stream").0,
            "PDF"
        );
        assert_eq!(
            attachment_kind("comptes.xlsx", "application/octet-stream").0,
            "Spreadsheet"
        );
    }

    #[test]
    fn la_casse_de_l_extension_est_ignoree() {
        assert_eq!(attachment_kind("DEVIS.PDF", "").0, "PDF");
    }

    #[test]
    fn un_fichier_sans_extension_reste_un_fichier() {
        // Et garde le trombone : inventer un type serait affirmer ce qu'on ignore.
        let (nom, icone) = attachment_kind("LISEZMOI", "");
        assert_eq!(nom, "File");
        assert_eq!(icone, "paperclip");
    }

    #[test]
    fn chaque_type_a_une_icone() {
        for (fichier, mime) in [
            ("a.pdf", ""),
            ("a.png", ""),
            ("a.zip", ""),
            ("a.csv", ""),
            ("a", "image/gif"),
        ] {
            assert!(!attachment_kind(fichier, mime).1.is_empty());
        }
    }
}
