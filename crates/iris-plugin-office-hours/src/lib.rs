//! **Office hours** — le courrier attend que vous travailliez.
//!
//! Un message arrivé à vingt-trois heures est reporté au matin ouvré suivant. Rien
//! n'est caché : le fil est reporté, il porte l'horloge dans la liste, et il revient
//! seul.
//!
//! Ce module transforme la synchronisation d'arrière-plan de quelque chose qui
//! interrompt en quelque chose qui s'accumule, ce qui est le sens même d'une file de
//! travail. C'est aussi le plus clair usage des réglages : les heures et les jours sont
//! exactement ce qu'un module doit demander et qu'une application ne doit pas décider.
//!
//! ## Ce qu'il ne reporte pas
//!
//! Ce qui est déjà passé par ailleurs — les indésirables n'arrivent pas ici, l'hôte ne
//! les annonce pas — et **rien d'autre**. Un report a l'air anodin ; c'est la seule
//! action de tri qui retire un message de la vue sans que personne ne l'ait vu. Il vaut
//! donc mieux qu'il soit prévisible : mêmes heures, tous les expéditeurs, aucune
//! exception que l'utilisateur n'aurait pas écrite.
//!
//! ## Le fuseau
//!
//! Les heures sont en temps universel, parce que c'est ce que l'hôte sait. Iris ne
//! connaît aucun fuseau et en inventer un serait pire que de le dire ; le réglage
//! l'annonce, et quelqu'un à Paris met sept heures pour dire huit heures en hiver.

#![cfg_attr(target_arch = "wasm32", no_std)]

extern crate alloc;

use iris_plugin_sdk as sdk;

/// Les bornes lues à l'initialisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Horaires {
    /// Le module reporte-t-il quoi que ce soit ?
    ///
    /// Faux par défaut, et c'est délibéré. Les deux autres modules livrés sont inertes
    /// tant qu'on ne leur a rien écrit — une liste vide ne désigne personne, une liste
    /// de règles vide ne range rien. Celui-ci a des horaires plausibles dès le départ,
    /// donc sans cet interrupteur il se mettrait à retirer du courrier de la vue le
    /// soir de son installation, sans que personne le lui ait demandé.
    pub actif: bool,
    /// Première heure ouvrée, incluse.
    pub debut: i64,
    /// Dernière heure ouvrée, exclue.
    pub fin: i64,
    /// Le week-end compte-t-il comme hors des heures ?
    pub weekend_ferme: bool,
}

impl Default for Horaires {
    fn default() -> Self {
        // Éteint, sur des horaires de huit à dix-huit heures, samedi et dimanche
        // fermés. Les heures sont un point de départ plausible pour qu'il n'y ait rien
        // à saisir avant d'allumer ; l'interrupteur, lui, est fermé, parce que c'est
        // l'utilisateur qui décide que son courrier du soir peut attendre.
        Self {
            actif: false,
            debut: 8,
            fin: 18,
            weekend_ferme: true,
        }
    }
}

static mut HORAIRES: Horaires = Horaires {
    actif: false,
    debut: 8,
    fin: 18,
    weekend_ferme: true,
};

/// # Safety
///
/// Appelée par l'hôte, une seule fois, avant tout événement.
#[no_mangle]
pub unsafe extern "C" fn iris_init(ptr: *const u8, len: usize) {
    let reglages = sdk::charge(ptr, len);

    let heure = |nom: &str, defaut: i64| -> i64 {
        sdk::champ_texte(reglages, nom)
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(defaut)
            .clamp(0, 23)
    };

    let debut = heure("start", 8);
    let fin = heure("end", 18);

    let actif = sdk::champ_texte(reglages, "enabled")
        .map(|v| v == "true")
        .unwrap_or(false);
    if !actif {
        sdk::log("office-hours: switched off — nothing is deferred");
    }

    HORAIRES = Horaires {
        actif,
        debut,
        // Une fin avant le début décrirait une journée qui finit avant de commencer.
        // Plutôt que de refuser en silence, on retombe sur la journée entière : le
        // module ne reporte alors rien, ce qui est visible et se corrige.
        fin: if fin > debut { fin } else { 24 },
        weekend_ferme: sdk::champ_texte(reglages, "weekend")
            .map(|v| v != "false")
            .unwrap_or(true),
    };
}

/// # Safety
///
/// Appelée par l'hôte avec une charge qu'il a écrite via `iris_alloc`.
#[no_mangle]
pub unsafe extern "C" fn iris_on_event(ptr: *const u8, len: usize) {
    let charge = sdk::charge(ptr, len);

    let (Some(heure), Some(jour)) = (
        sdk::champ_entier(charge, "heure"),
        sdk::champ_entier(charge, "jour"),
    ) else {
        return;
    };

    let horaires = *core::ptr::addr_of!(HORAIRES);
    let Some(attente) = heures_jusqu_a_l_ouverture(heure, jour, horaires) else {
        return;
    };

    // Le JSON est composé à la main : le module n'a pas de sérialiseur, et la charge
    // n'a que deux champs dont l'un est un entier que nous venons de calculer.
    let mut json = alloc::string::String::from("{\"action\":\"snooze\",\"hours\":");
    json.push_str(&alloc::format!("{attente}"));
    json.push('}');
    sdk::act(&json);
}

/// Combien d'heures avant la prochaine heure ouvrée, ou `None` si on y est déjà.
///
/// Le cœur du module, et la seule chose qui mérite d'être testée. La fonction est pure
/// et prend l'heure et le jour en argument : la vérifier n'exige ni horloge ni bac à
/// sable, et les cas qui se trompent — le vendredi soir, minuit, la fin de semaine —
/// sont ceux qu'on n'obtiendrait qu'en attendant le bon moment de la vraie semaine.
pub fn heures_jusqu_a_l_ouverture(heure: i64, jour: i64, h: Horaires) -> Option<i64> {
    if !h.actif {
        return None;
    }

    let ouvre = |jour: i64| !(h.weekend_ferme && jour >= 5);

    if ouvre(jour) && heure >= h.debut && heure < h.fin {
        return None;
    }

    // Avant l'ouverture, le même jour : on attend le début.
    if ouvre(jour) && heure < h.debut {
        return Some(h.debut - heure);
    }

    // Sinon, le prochain jour ouvré. Sept essais couvrent la semaine entière ; au-delà
    // c'est que tous les jours sont fermés, ce que la boucle ne peut pas résoudre et
    // qu'un report d'une semaine ne résoudrait pas non plus.
    let mut attente = 24 - heure + h.debut;
    let mut prochain = (jour + 1).rem_euclid(7);

    for _ in 0..7 {
        if ouvre(prochain) {
            return Some(attente);
        }
        attente += 24;
        prochain = (prochain + 1).rem_euclid(7);
    }
    None
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    const LUNDI: i64 = 0;
    const VENDREDI: i64 = 4;
    const SAMEDI: i64 = 5;
    const DIMANCHE: i64 = 6;

    /// Les horaires par défaut, mais allumés : c'est le seul état dans lequel le reste
    /// des cas a quelque chose à dire.
    fn defaut() -> Horaires {
        Horaires {
            actif: true,
            ..Horaires::default()
        }
    }

    #[test]
    fn eteint_il_ne_reporte_rien() {
        // L'état à l'installation. Les deux autres modules livrés sont inertes tant
        // qu'on ne leur a rien écrit ; celui-ci a des horaires plausibles dès le
        // départ, donc il lui faut un interrupteur pour ne pas retirer du courrier de
        // la vue le soir où on l'installe.
        let eteint = Horaires::default();
        assert!(!eteint.actif);
        for jour in 0..7 {
            for heure in 0..24 {
                assert_eq!(heures_jusqu_a_l_ouverture(heure, jour, eteint), None);
            }
        }
    }

    #[test]
    fn pendant_les_heures_rien_n_est_reporte() {
        assert_eq!(heures_jusqu_a_l_ouverture(10, LUNDI, defaut()), None);
        assert_eq!(heures_jusqu_a_l_ouverture(8, LUNDI, defaut()), None);
        assert_eq!(
            heures_jusqu_a_l_ouverture(17, VENDREDI, defaut()),
            None,
            "dix-sept heures un vendredi est encore ouvré"
        );
    }

    #[test]
    fn le_matin_tot_attend_l_ouverture_du_jour() {
        assert_eq!(heures_jusqu_a_l_ouverture(6, LUNDI, defaut()), Some(2));
    }

    #[test]
    fn le_soir_attend_le_lendemain() {
        // Vingt-trois heures un lundi : une heure jusqu'à minuit, puis huit.
        assert_eq!(heures_jusqu_a_l_ouverture(23, LUNDI, defaut()), Some(9));
    }

    #[test]
    fn le_vendredi_soir_saute_le_week_end() {
        // C'est le cas qu'on n'obtient qu'en attendant vendredi soir pour l'essayer,
        // et celui qu'une implémentation naïve rate.
        // 19 h vendredi → 5 h jusqu'à minuit, + 8 h = 13, plus deux jours fermés.
        assert_eq!(
            heures_jusqu_a_l_ouverture(19, VENDREDI, defaut()),
            Some(13 + 48)
        );
    }

    #[test]
    fn le_samedi_attend_lundi() {
        // 10 h samedi → 14 h jusqu'à minuit + 8 h = 22, plus un jour fermé.
        assert_eq!(
            heures_jusqu_a_l_ouverture(10, SAMEDI, defaut()),
            Some(22 + 24)
        );
        assert_eq!(heures_jusqu_a_l_ouverture(10, DIMANCHE, defaut()), Some(22));
    }

    #[test]
    fn le_week_end_ouvert_ne_saute_rien() {
        let sept_jours = Horaires {
            weekend_ferme: false,
            ..defaut()
        };
        assert_eq!(heures_jusqu_a_l_ouverture(10, SAMEDI, sept_jours), None);
        assert_eq!(heures_jusqu_a_l_ouverture(23, SAMEDI, sept_jours), Some(9));
    }

    #[test]
    fn une_journee_entiere_ne_reporte_jamais() {
        // Ce que donne une fin avant le début : le module ne fait plus rien, ce qui
        // est visible et se corrige, plutôt que de refuser en silence.
        let toujours = Horaires {
            actif: true,
            debut: 0,
            fin: 24,
            weekend_ferme: false,
        };
        for heure in 0..24 {
            assert_eq!(heures_jusqu_a_l_ouverture(heure, LUNDI, toujours), None);
        }
    }

    #[test]
    fn minuit_pile_est_hors_des_heures() {
        // La borne qu'on oublie : minuit est zéro, ce qui est plus petit que tout, et
        // une comparaison mal posée en fait une heure ouvrée.
        assert_eq!(heures_jusqu_a_l_ouverture(0, LUNDI, defaut()), Some(8));
    }

    #[test]
    fn le_report_ne_depasse_jamais_une_semaine() {
        // La borne qui protège d'un réglage absurde : reporter n'est pas faire
        // disparaître.
        for jour in 0..7 {
            for heure in 0..24 {
                if let Some(h) = heures_jusqu_a_l_ouverture(heure, jour, defaut()) {
                    assert!(h > 0 && h <= 24 * 7, "{heure} h, jour {jour} → {h} h");
                }
            }
        }
    }
}
