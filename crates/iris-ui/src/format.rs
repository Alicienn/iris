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
        1 => "hier".to_string(),
        2..=6 => weekday(date).to_string(),
        _ => absolute_date(date),
    }
}

fn time_of_day(t: Timestamp) -> String {
    let reste = t.seconds().rem_euclid(86_400);
    format!("{:02}:{:02}", reste / 3600, (reste % 3600) / 60)
}

/// Jour de la semaine. Le 1ᵉʳ janvier 1970 était un jeudi.
fn weekday(t: Timestamp) -> &'static str {
    const JOURS: [&str; 7] = [
        "jeudi", "vendredi", "samedi", "dimanche", "lundi", "mardi", "mercredi",
    ];
    JOURS[t.seconds().div_euclid(86_400).rem_euclid(7) as usize]
}

fn absolute_date(t: Timestamp) -> String {
    const MOIS: [&str; 12] = [
        "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
        "déc.",
    ];
    let (annee, mois, jour) = civil_from_days(t.seconds().div_euclid(86_400));
    format!("{jour} {} {annee}", MOIS[(mois - 1) as usize])
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
/// elle sert uniquement à distinguer cent boîtes d'un coup d'œil. Les teintes sont
/// donc **peu saturées et de luminosité constante** — assez pour se différencier,
/// pas assez pour attirer l'œil ni pour rompre la neutralité du thème.
pub fn account_tint(email: &str) -> (u8, u8, u8) {
    // Répartition sur le cercle chromatique par un condensé stable : le même compte
    // garde sa couleur d'une session à l'autre.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in email.trim().to_lowercase().bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    let teinte = (hash % 360) as f32;
    hsl_to_rgb(teinte, 0.32, 0.62)
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

    /// 14 novembre 2023, 22 h 13 UTC — un mardi.
    fn now() -> Timestamp {
        Timestamp::from_millis(1_700_000_000_000)
    }

    fn il_y_a(secondes: i64) -> Timestamp {
        Timestamp::from_millis(now().millis() - secondes * 1000)
    }

    #[test]
    fn aujourd_hui_affiche_l_heure() {
        let d = relative_date(il_y_a(3600), now());
        assert_eq!(d, "21:13");
    }

    #[test]
    fn hier_est_nomme() {
        assert_eq!(relative_date(il_y_a(86_400), now()), "hier");
    }

    #[test]
    fn la_semaine_ecoulee_affiche_le_jour() {
        let d = relative_date(il_y_a(3 * 86_400), now());
        assert_eq!(d, "samedi");
    }

    #[test]
    fn au_dela_la_date_est_absolue() {
        let d = relative_date(il_y_a(30 * 86_400), now());
        assert_eq!(d, "15 oct. 2023");
    }

    #[test]
    fn une_date_future_n_est_pas_annoncee_comme_telle() {
        // Un expéditeur qui date son message en 2099 pour rester en tête de liste ne
        // doit pas obtenir un affichage privilégié.
        let futur = Timestamp::from_millis(now().millis() + 365 * 86_400_000);
        let d = relative_date(futur, now());
        assert!(d.contains("2024"), "obtenu « {d} »");
    }

    #[test]
    fn les_jours_de_la_semaine_sont_corrects() {
        // 1ᵉʳ janvier 1970 : un jeudi.
        assert_eq!(weekday(Timestamp::from_millis(0)), "jeudi");
        assert_eq!(weekday(Timestamp::from_millis(86_400_000)), "vendredi");
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
    fn les_teintes_restent_discretes() {
        // Assez pour distinguer, pas assez pour rompre la neutralité du thème.
        for adresse in ["a@x.fr", "b@y.fr", "c@z.fr", "compta@entreprise.com"] {
            let (r, g, b) = account_tint(adresse);
            let max = r.max(g).max(b) as i32;
            let min = r.min(g).min(b) as i32;
            assert!(
                max - min < 110,
                "« {adresse} » donne une teinte trop saturée : {r},{g},{b}"
            );
            assert!(
                max > 100 && max < 220,
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
