//! What changed, version by version, told to the people who use Iris.
//!
//! The source is `CHANGELOG.md` at the root of the repository, compiled into the
//! binary. One file serves three readers: the Changelog window, the notes of a GitHub
//! release, and whoever reads the repository. Three copies would disagree by the
//! second release.
//!
//! The format is deliberately narrow, so that a parser this small can read it:
//!
//! ```text
//! ## 0.2.0 — 2026-09-27
//!
//! ### New
//! - One line per change.
//! ```
//!
//! Anything else — the title, an introduction — is ignored.

/// The categories a change can fall under, in the order they are shown.
///
/// Fixed, so that two releases never file the same kind of change under two names.
pub const CATEGORIES: [&str; 4] = ["New", "Improved", "Fixed", "Removed"];

/// One version and what it changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// As written, `2026-09-27`. Empty when the heading has none.
    pub date: String,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub items: Vec<String>,
}

/// The changelog of this build.
pub const BUNDLED: &str = include_str!("../../../CHANGELOG.md");

/// Every release in the bundled changelog, newest first.
pub fn bundled() -> Vec<Release> {
    parse(BUNDLED)
}

/// Reads a changelog in the format above.
///
/// Lenient about what it does not need — spacing, the dash between version and date,
/// a `v` before the number — because a release note that fails to show is worse than
/// one that shows slightly off.
pub fn parse(markdown: &str) -> Vec<Release> {
    let mut releases: Vec<Release> = Vec::new();

    for line in markdown.lines() {
        let line = line.trim_end();

        if let Some(heading) = line.strip_prefix("## ") {
            let heading = heading.trim();
            let (version, rest) = heading
                .split_once(char::is_whitespace)
                .unwrap_or((heading, ""));
            let version = version
                .trim_matches(|c| c == '[' || c == ']')
                .trim_start_matches('v')
                .to_string();
            let date = rest
                .trim()
                .trim_start_matches(['—', '–', '-'])
                .trim()
                .to_string();
            releases.push(Release {
                version,
                date,
                sections: Vec::new(),
            });
        } else if let Some(title) = line.strip_prefix("### ") {
            if let Some(release) = releases.last_mut() {
                release.sections.push(Section {
                    title: title.trim().to_string(),
                    items: Vec::new(),
                });
            }
        } else if let Some(item) = line.trim_start().strip_prefix("- ") {
            if let Some(section) = releases.last_mut().and_then(|r| r.sections.last_mut()) {
                section.items.push(item.trim().to_string());
            }
        }
    }

    releases
}

/// The releases newer than `current`, newest first.
///
/// What an update would bring: someone three versions behind wants the three, not
/// only the last.
pub fn newer_than(releases: &[Release], current: &str) -> Vec<Release> {
    let Some(courante) = crate::update::Version::parse(current) else {
        return releases.to_vec();
    };
    releases
        .iter()
        .filter(|r| crate::update::Version::parse(&r.version).is_some_and(|v| v > courante))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXEMPLE: &str = "# Changelog\n\nIntro, ignored.\n\n\
        ## 0.2.0 — 2026-09-27\n\n### New\n- Alpha.\n- Beta.\n\n### Fixed\n- Gamma.\n\n\
        ## [v0.1.0] - 2026-09-20\n\n### New\n- First.\n";

    #[test]
    fn versions_sections_and_items_are_read() {
        let r = parse(EXEMPLE);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].version, "0.2.0");
        assert_eq!(r[0].date, "2026-09-27");
        assert_eq!(r[0].sections[0].title, "New");
        assert_eq!(r[0].sections[0].items, ["Alpha.", "Beta."]);
        assert_eq!(r[0].sections[1].items, ["Gamma."]);
        assert_eq!(r[1].version, "0.1.0", "brackets and the v are dropped");
        assert_eq!(r[1].date, "2026-09-20");
    }

    #[test]
    fn only_newer_releases_are_kept() {
        let r = parse(EXEMPLE);
        let nouvelles = newer_than(&r, "0.1.0");
        assert_eq!(nouvelles.len(), 1);
        assert_eq!(nouvelles[0].version, "0.2.0");
        assert!(newer_than(&r, "0.2.0").is_empty());
    }

    // The rules CLAUDE.md sets, held by the build rather than by memory: a version
    // bump without its entry, or an entry without its bump, fails here.
    #[test]
    fn the_bundled_changelog_starts_with_this_version() {
        let r = bundled();
        assert_eq!(
            r.first().map(|r| r.version.as_str()),
            Some(env!("CARGO_PKG_VERSION")),
            "CHANGELOG.md must open with the version in Cargo.toml"
        );
    }

    #[test]
    fn the_bundled_changelog_is_well_formed() {
        let r = bundled();
        let mut precedente: Option<crate::update::Version> = None;
        for release in &r {
            let v = crate::update::Version::parse(&release.version)
                .unwrap_or_else(|| panic!("« {} » is not a version", release.version));
            if let Some(p) = precedente {
                assert!(
                    v < p,
                    "{} must come after {}: newest first",
                    release.version,
                    p
                );
            }
            precedente = Some(v);

            assert!(!release.date.is_empty(), "{} has no date", release.version);
            assert!(
                !release.sections.is_empty(),
                "{} lists nothing",
                release.version
            );
            for section in &release.sections {
                assert!(
                    CATEGORIES.contains(&section.title.as_str()),
                    "{}: « {} » is not one of {CATEGORIES:?}",
                    release.version,
                    section.title
                );
                assert!(
                    !section.items.is_empty(),
                    "{}: empty « {} »",
                    release.version,
                    section.title
                );
            }
        }
    }
}
