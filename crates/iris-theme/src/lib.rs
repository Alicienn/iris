//! `iris-theme` — the design tokens.
//!
//! Two themes ship, **light** and **dark**, and nothing else: one look, in the two
//! lights people read in. A theme is still a file (colours, radii, type sizes, row
//! height, durations), which keeps every value in one place; it is compiled in, not
//! read from the user's disk.
//!
//! Which one shows is the user's [`Appearance`]: light, dark, or whatever Windows is
//! set to.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod color;
mod tokens;

pub use color::Color;
pub use tokens::{
    ColorTokens, DensityTokens, MotionTokens, RadiusTokens, SpacingTokens, Theme, TypographyTokens,
};

use iris_types::{Error, Result};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// The two themes.
const BUILTIN: &[(&str, &str)] = &[
    ("light", include_str!("../themes/light.toml")),
    ("dark", include_str!("../themes/dark.toml")),
];

/// What the user asked for: a theme, or the system's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    /// Light or dark as Windows is set, following it when it changes.
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|a| *a == self).unwrap_or(0)
    }

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    /// The theme to show, given whether the system is dark.
    pub fn theme_name(self, system_dark: bool) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::System if system_dark => "dark",
            Self::System => "light",
        }
    }
}

/// The two themes, and the one showing.
#[derive(Debug)]
pub struct ThemeRegistry {
    light: Arc<Theme>,
    dark: Arc<Theme>,
    active: RwLock<Arc<Theme>>,
}

impl ThemeRegistry {
    pub fn builtin() -> Result<Self> {
        let charger = |name: &str| -> Result<Arc<Theme>> {
            let source = BUILTIN
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, s)| *s)
                .ok_or_else(|| Error::Config(format!("theme “{name}” missing")))?;
            Theme::from_toml(source)
                .map(Arc::new)
                .map_err(|e| Error::Config(format!("theme “{name}”: {e}")))
        };
        let light = charger("light")?;
        let dark = charger("dark")?;
        Ok(Self {
            active: RwLock::new(Arc::clone(&light)),
            light,
            dark,
        })
    }

    /// The theme showing. A pointer copy: cheap enough to call anywhere.
    pub fn active(&self) -> Arc<Theme> {
        Arc::clone(&self.active.read().expect("active theme"))
    }

    /// Shows the theme `appearance` calls for, and returns it.
    pub fn apply(&self, appearance: Appearance, system_dark: bool) -> Arc<Theme> {
        let theme = self.get(appearance.theme_name(system_dark));
        *self.active.write().expect("active theme") = Arc::clone(&theme);
        theme
    }

    pub fn get(&self, name: &str) -> Arc<Theme> {
        if name == "dark" {
            Arc::clone(&self.dark)
        } else {
            Arc::clone(&self.light)
        }
    }

    pub fn names(&self) -> Vec<String> {
        BUILTIN.iter().map(|(n, _)| (*n).to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_themes_ship() {
        let r = ThemeRegistry::builtin().unwrap();
        assert_eq!(r.names(), ["light", "dark"]);
        assert!(!r.get("light").dark);
        assert!(r.get("dark").dark);
    }

    #[test]
    fn each_theme_passes_its_own_check() {
        // A shipped theme with an unreadable contrast would be our mistake.
        let r = ThemeRegistry::builtin().unwrap();
        for name in r.names() {
            let complaints = r.get(&name).lint();
            assert!(complaints.is_empty(), "{name}: {complaints:?}");
        }
    }

    #[test]
    fn light_shows_first() {
        let r = ThemeRegistry::builtin().unwrap();
        assert_eq!(r.active().name, "light");
    }

    #[test]
    fn the_system_decides_when_asked_to() {
        let r = ThemeRegistry::builtin().unwrap();
        assert_eq!(r.apply(Appearance::System, true).name, "dark");
        assert_eq!(r.apply(Appearance::System, false).name, "light");
        assert_eq!(r.apply(Appearance::Dark, false).name, "dark");
        assert_eq!(r.apply(Appearance::Light, true).name, "light");
        assert_eq!(r.active().name, "light");
    }

    #[test]
    fn the_appearance_is_written_as_a_word() {
        #[derive(Serialize, Deserialize)]
        struct S {
            appearance: Appearance,
        }
        let t = toml::to_string(&S {
            appearance: Appearance::Dark,
        })
        .unwrap();
        assert_eq!(t.trim(), "appearance = \"dark\"");
        let s: S = toml::from_str("appearance = \"system\"").unwrap();
        assert_eq!(s.appearance, Appearance::System);
    }

    #[test]
    fn the_index_goes_both_ways() {
        for a in Appearance::ALL {
            assert_eq!(Appearance::from_index(a.index()), Some(a));
        }
    }
}
