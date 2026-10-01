//! Les réglages de l'utilisateur.
//!
//! Trois choses seulement, parce que ce sont les trois que l'utilisateur a demandé à
//! pouvoir changer : **le thème**, **la densité** de la liste, et **les automatismes**
//! du flux de travail. Tout le reste est décidé par le thème lui-même, qui est un
//! fichier, et se modifie en le rouvrant.
//!
//! Le fichier vit dans le répertoire de configuration, séparé des données : le perdre
//! ne coûte que de refaire trois clics.
//!
//! Un fichier illisible ne bloque pas le démarrage. Refuser d'ouvrir l'application
//! parce qu'une virgule manque dans un réglage cosmétique serait une punition
//! disproportionnée ; on repart des valeurs par défaut et on le dit dans le journal.

use iris_types::{AutomationSettings, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// La densité de la liste.
///
/// Elle multiplie la hauteur de ligne du thème plutôt que de la remplacer : un thème
/// qui a choisi des lignes hautes reste plus aéré que les autres à densité égale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    /// Le maximum de lignes à l'écran : pour trier vite.
    Compact,
    #[default]
    Normal,
    /// Pour lire longtemps.
    Comfortable,
}

impl Density {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Normal, Self::Comfortable];

    pub fn factor(self) -> f32 {
        match self {
            // 45 px: two tight lines, no faces. 56: two lines and faces. 73: the
            // excerpt gets a line of its own.
            Self::Compact => 0.8,
            Self::Normal => 1.0,
            Self::Comfortable => 1.3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compacte",
            Self::Normal => "Normale",
            Self::Comfortable => "Confortable",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Compact => 0,
            Self::Normal => 1,
            Self::Comfortable => 2,
        }
    }

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

/// Les réglages, tels qu'ils sont écrits sur le disque.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Light, dark, or as Windows is set. Replaces the theme name of the versions with
    /// several themes; the old key is simply no longer read.
    pub appearance: iris_theme::Appearance,
    /// The first name Home greets. Empty: a greeting without a name.
    #[serde(default)]
    pub first_name: String,
    pub density: Density,
    pub automation: AutomationSettings,
    /// Identifiants clients OAuth. Absents du binaire à dessein : un secret
    /// distribué à tout le monde n'en est pas un, et celui d'Iris doit être
    /// enregistré auprès de chaque fournisseur, ce qu'un fichier source ne fait pas.
    pub oauth: crate::oauth::OAuthSettings,
    /// Prévenir à l'arrivée du courrier.
    ///
    /// Par défaut : oui. C'est la raison d'être d'une synchronisation d'arrière-plan,
    /// et une notification qu'il faut aller chercher dans les réglages pour l'allumer
    /// est une notification que personne n'aura.
    #[serde(default = "vrai")]
    pub notifications: bool,
    /// Démarrer à l'ouverture de session.
    ///
    /// L'inscription vit dans le registre ; ceci n'en est que le reflet, pour que
    /// l'interrupteur montre le bon état au démarrage sans interroger Windows à
    /// chaque image.
    #[serde(default)]
    pub start_at_login: bool,
    /// Iris est inscrite comme client de courrier possible du système.
    #[serde(default)]
    pub handle_mailto: bool,
    /// Continuer en arrière-plan quand on ferme la fenêtre.
    ///
    /// Par défaut : oui, et c'est ce qui donne son sens à l'icône de la zone de
    /// notification — un client qui se synchronise en arrière-plan et qui s'arrête
    /// quand on ferme sa fenêtre ne se synchronise pas.
    ///
    /// Mais c'est un choix, pas une évidence. Une application qui refuse de partir
    /// quand on lui demande de partir est une application dont on se méfie, et
    /// quelqu'un qui relève son courrier deux fois par jour n'a aucune raison de la
    /// laisser tourner entre les deux. Refuser éteint aussi l'icône : la laisser
    /// promettrait un programme qui tourne encore.
    #[serde(default = "vrai")]
    pub keep_running: bool,
    /// Combien de secondes un message envoyé peut encore être rattrapé.
    #[serde(default = "cinq")]
    pub undo_send_seconds: u32,
    /// Les comptes de la colonne de gauche sont rangés sous leurs tags.
    ///
    /// On by default since 1.0, under a new name: the old `group_accounts_by_tag` was
    /// written `false` into every settings file, asked for or not, and reading it
    /// would have kept the grouping off for everyone who already had Iris.
    #[serde(default = "vrai")]
    pub accounts_by_tag: bool,
    /// The tags whose accounts are folded away in the accounts column.
    #[serde(default)]
    pub folded_tags: Vec<i64>,
    /// Iris opens on the Home screen. Off, it opens on the mail; Home stays one click
    /// away on the name in the title bar.
    #[serde(default = "vrai")]
    pub home_at_startup: bool,
    /// The calendar's view: 0 month, 1 week, 2 day. The last one chosen.
    #[serde(default = "semaine")]
    pub calendar_view: i32,
    /// The last day the inbox was emptied (`YYYY-MM-DD`), and how many days in a row
    /// it has been: the inbox zero screen counts them.
    #[serde(default)]
    pub inbox_zero_day: String,
    #[serde(default)]
    pub inbox_zero_streak: u32,
    /// The window's buttons as three coloured lights at the top left, as on a Mac;
    /// off, Windows' three at the top right. Each system's own by default.
    #[serde(default = "boutons_mac")]
    pub mac_window_buttons: bool,
}

/// The Mac's lights on a Mac, Windows' buttons elsewhere.
fn boutons_mac() -> bool {
    cfg!(target_os = "macos")
}

/// How many days in a row the inbox has been emptied, `today` included: the same
/// count on a day already counted, one more the day after, one again after a gap.
pub fn inbox_zero_streak(
    today: chrono::NaiveDate,
    last: Option<chrono::NaiveDate>,
    streak: u32,
) -> u32 {
    match last {
        Some(d) if d == today => streak.max(1),
        Some(d) if d.succ_opt() == Some(today) => streak + 1,
        _ => 1,
    }
}

/// The calendar opens on the week: what is coming, hour by hour.
fn semaine() -> i32 {
    1
}

/// La valeur par défaut d'un réglage qui doit être allumé.
fn vrai() -> bool {
    true
}

/// Le délai d'annulation par défaut : assez pour voir l'erreur, pas assez pour
/// attendre.
fn cinq() -> u32 {
    5
}

/// Les bornes du délai d'annulation. Au-delà d'une demi-minute, un message « envoyé »
/// ne l'est pas encore quand on referme l'ordinateur.
pub const UNDO_SEND_RANGE: std::ops::RangeInclusive<u32> = 0..=30;

impl Default for Settings {
    fn default() -> Self {
        Self {
            appearance: iris_theme::Appearance::System,
            first_name: String::new(),
            density: Density::default(),
            automation: AutomationSettings::default(),
            oauth: Default::default(),
            notifications: true,
            start_at_login: false,
            handle_mailto: false,
            keep_running: true,
            undo_send_seconds: cinq(),
            accounts_by_tag: true,
            folded_tags: Vec::new(),
            home_at_startup: true,
            calendar_view: semaine(),
            inbox_zero_day: String::new(),
            inbox_zero_streak: 0,
            mac_window_buttons: boutons_mac(),
        }
    }
}

/// The settings in use, and where they are written.
///
/// One copy for the whole application. The settings panel had its own, and a second
/// one elsewhere (the calendar remembering its view) would have written over the
/// panel's changes with a stale file, or the other way round.
type Partages = (
    std::sync::Arc<std::sync::Mutex<Settings>>,
    std::path::PathBuf,
);
static PARTAGES: std::sync::OnceLock<Partages> = std::sync::OnceLock::new();

/// Makes `current` the settings every module reads and changes. Called once, by the
/// settings panel's wiring.
pub fn share(current: std::sync::Arc<std::sync::Mutex<Settings>>, path: std::path::PathBuf) {
    let _ = PARTAGES.set((current, path));
}

/// The settings in use; the defaults before they are shared (tests, previews).
pub fn current() -> Settings {
    PARTAGES
        .get()
        .and_then(|(s, _)| s.lock().ok().map(|s| s.clone()))
        .unwrap_or_default()
}

/// Changes the settings in use and writes them. Nothing happens before they are
/// shared.
pub fn update(change: impl FnOnce(&mut Settings)) {
    let Some((courant, chemin)) = PARTAGES.get() else {
        return;
    };
    let Ok(mut reglages) = courant.lock() else {
        return;
    };
    change(&mut reglages);
    if let Err(e) = reglages.save(chemin) {
        tracing::warn!(error = %e, "saving the settings");
    }
}

impl Settings {
    /// Lit les réglages. Un fichier absent ou illisible rend les valeurs par défaut.
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let Ok(texte) = std::fs::read_to_string(path) else {
            return Self::default();
        };

        match toml::from_str(&texte) {
            Ok(reglages) => Self::sanitize(reglages),
            Err(e) => {
                tracing::warn!(fichier = %path.display(), error = %e, "settings could not be read");
                Self::default()
            }
        }
    }

    /// Écrit les réglages, en créant le répertoire au besoin.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let texte = toml::to_string_pretty(self)
            .map_err(|e| iris_types::Error::Config(format!("réglages : {e}")))?;
        std::fs::write(path, texte)?;
        Ok(())
    }

    /// Ramène les valeurs aberrantes dans le domaine du raisonnable.
    ///
    /// Un délai de relance de zéro jour ramènerait tous les fils en attente au
    /// premier passage de maintenance ; un délai de dix ans est une désactivation
    /// déguisée, qui a déjà son interrupteur.
    fn sanitize(mut self) -> Self {
        self.automation.follow_up_days = self.automation.follow_up_days.clamp(1, 365);
        self.undo_send_seconds = self
            .undo_send_seconds
            .clamp(*UNDO_SEND_RANGE.start(), *UNDO_SEND_RANGE.end());
        self.calendar_view = self.calendar_view.clamp(0, 2);
        self.first_name = self.first_name.trim().chars().take(40).collect();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fichier() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("config").join("iris.toml");
        (dir, chemin)
    }

    #[test]
    fn un_fichier_absent_rend_les_valeurs_par_defaut() {
        let (_d, chemin) = fichier();
        assert_eq!(Settings::load(&chemin), Settings::default());
    }

    #[test]
    fn les_reglages_font_l_aller_retour() {
        let (_d, chemin) = fichier();
        let reglages = Settings {
            appearance: iris_theme::Appearance::Dark,
            first_name: "Camille".into(),
            density: Density::Compact,
            automation: AutomationSettings::MANUAL_ONLY,
            oauth: crate::oauth::OAuthSettings {
                google_client_id: "abc.apps.googleusercontent.com".into(),
                google_client_secret: "GOCSPX-exemple".into(),
                microsoft_client_id: String::new(),
            },
            notifications: false,
            start_at_login: true,
            handle_mailto: true,
            keep_running: false,
            undo_send_seconds: 12,
            accounts_by_tag: false,
            folded_tags: vec![3, 7],
            home_at_startup: false,
            calendar_view: 0,
            inbox_zero_day: "2026-10-01".into(),
            inbox_zero_streak: 4,
            mac_window_buttons: true,
        };

        reglages.save(&chemin).unwrap();
        assert_eq!(Settings::load(&chemin), reglages);
    }

    #[test]
    fn inbox_zero_counts_days_in_a_row() {
        let d = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        assert_eq!(
            inbox_zero_streak(d("2026-10-01"), None, 0),
            1,
            "a first day"
        );
        assert_eq!(
            inbox_zero_streak(d("2026-10-01"), Some(d("2026-10-01")), 3),
            3,
            "the same day counts once"
        );
        assert_eq!(
            inbox_zero_streak(d("2026-10-02"), Some(d("2026-10-01")), 3),
            4,
            "the day after, one more"
        );
        assert_eq!(
            inbox_zero_streak(d("2026-10-05"), Some(d("2026-10-01")), 3),
            1,
            "after a gap, it starts again"
        );
    }

    #[test]
    fn le_delai_d_annulation_reste_dans_ses_bornes() {
        let (_d, chemin) = fichier();
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(&chemin, "theme = \"mono\"\nundo_send_seconds = 900\n").unwrap();
        assert_eq!(Settings::load(&chemin).undo_send_seconds, 30);
        // Absent of an older file: the default.
        std::fs::write(&chemin, "theme = \"mono\"\n").unwrap();
        assert_eq!(Settings::load(&chemin).undo_send_seconds, 5);
    }

    #[test]
    fn an_older_file_opens_on_home_grouped_by_tag_on_the_week() {
        // A file from 0.8 said `group_accounts_by_tag = false` without anyone having
        // chosen it: 1.0 groups by tag all the same.
        let (_d, chemin) = fichier();
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(
            &chemin,
            "theme = \"mono\"\ngroup_accounts_by_tag = false\ncalendar_view = 7\n",
        )
        .unwrap();
        let r = Settings::load(&chemin);
        assert!(r.accounts_by_tag);
        assert!(r.home_at_startup);
        assert_eq!(r.calendar_view, 2, "out of range: brought back");
        std::fs::write(&chemin, "theme = \"mono\"\n").unwrap();
        assert_eq!(Settings::load(&chemin).calendar_view, 1);
    }

    #[test]
    fn un_fichier_illisible_ne_bloque_pas_le_demarrage() {
        // Refuser d'ouvrir l'application pour une virgule manquante dans un réglage
        // cosmétique serait disproportionné.
        let (_d, chemin) = fichier();
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(&chemin, "ceci = = n'est pas du toml").unwrap();

        assert_eq!(Settings::load(&chemin), Settings::default());
    }

    #[test]
    fn un_fichier_anterieur_continue_de_tourner_en_arriere_plan() {
        // Le réglage est nouveau ; le fichier de quelqu'un qui utilise déjà Iris ne le
        // contient pas. Son absence doit vouloir dire « comme avant », c'est-à-dire
        // continuer — un défaut à faux ferait quitter l'application à la première
        // fermeture de fenêtre, sans que rien n'ait été demandé.
        let (_d, chemin) = fichier();
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(&chemin, "theme = \"mono\"\n").unwrap();

        assert!(Settings::load(&chemin).keep_running);
    }

    #[test]
    fn un_delai_de_relance_nul_est_ramene_a_un_jour() {
        // Sinon tous les fils en attente reviendraient au premier passage.
        let (_d, chemin) = fichier();
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(&chemin, "[automation]\nfollow_up_days = 0\n").unwrap();

        assert_eq!(Settings::load(&chemin).automation.follow_up_days, 1);
    }

    #[test]
    fn un_delai_absurde_est_borne() {
        let (_d, chemin) = fichier();
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(&chemin, "[automation]\nfollow_up_days = 5000\n").unwrap();

        assert_eq!(Settings::load(&chemin).automation.follow_up_days, 365);
    }

    #[test]
    fn a_file_from_the_themes_follows_windows() {
        // Before 3.0 the file named one of five themes. The key is no longer read:
        // whoever had one now gets the system's light or dark.
        let (_d, chemin) = fichier();
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(&chemin, "theme = \"sand\"\n").unwrap();

        assert_eq!(
            Settings::load(&chemin).appearance,
            iris_theme::Appearance::System
        );
    }

    #[test]
    fn the_first_name_is_trimmed() {
        let (_d, chemin) = fichier();
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(&chemin, "first_name = \"  Camille \"\n").unwrap();

        assert_eq!(Settings::load(&chemin).first_name, "Camille");
    }

    #[test]
    fn un_fichier_partiel_garde_les_defauts_pour_le_reste() {
        // Ajouter un réglage plus tard ne doit pas invalider les fichiers existants.
        let (_d, chemin) = fichier();
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(&chemin, "appearance = \"dark\"\n").unwrap();

        let reglages = Settings::load(&chemin);
        assert_eq!(reglages.appearance, iris_theme::Appearance::Dark);
        assert_eq!(reglages.density, Density::Normal);
        assert_eq!(reglages.automation, AutomationSettings::default());
    }

    #[test]
    fn les_identifiants_oauth_survivent_a_l_aller_retour() {
        // Sans eux, la connexion à Google est impossible : les perdre à chaque
        // écriture des réglages déconnecterait le compte sans raison visible.
        let (_d, chemin) = fichier();
        let mut reglages = Settings::default();
        reglages.oauth.google_client_id = "client-google".into();
        reglages.save(&chemin).unwrap();

        assert_eq!(
            Settings::load(&chemin).oauth.google_client_id,
            "client-google"
        );
    }

    #[test]
    fn un_fichier_sans_section_oauth_reste_lisible() {
        let (_d, chemin) = fichier();
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(&chemin, "theme = \"mono\"\n").unwrap();

        assert_eq!(Settings::load(&chemin).oauth, Default::default());
    }

    #[test]
    fn l_ecriture_cree_le_repertoire_manquant() {
        let (_d, chemin) = fichier();
        assert!(!chemin.parent().unwrap().exists());
        Settings::default().save(&chemin).unwrap();
        assert!(chemin.exists());
    }

    #[test]
    fn la_densite_multiplie_au_lieu_de_remplacer() {
        // Un thème aux lignes hautes doit rester plus aéré que les autres à densité
        // égale.
        assert!(Density::Compact.factor() < Density::Normal.factor());
        assert!(Density::Comfortable.factor() > Density::Normal.factor());
        assert_eq!(Density::Normal.factor(), 1.0);
    }

    #[test]
    fn les_densites_font_l_aller_retour_par_leur_indice() {
        for densite in Density::ALL {
            assert_eq!(Density::from_index(densite.index()), Some(densite));
        }
        assert_eq!(Density::from_index(9), None);
    }
}
