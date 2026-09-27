//! Updates, from the GitHub releases of the repository.
//!
//! In the core rather than a module, on purpose. A module runs in a sandbox with no
//! network, no file system and no way to start a program — which is exactly what makes
//! it safe to install one from a stranger. An updater needs all three: it downloads an
//! executable and runs it. Granting a module that would make every module suspect.
//!
//! Three steps, each its own function so each can be tested or retried alone:
//!
//! 1. [`check`] asks GitHub for the latest release and says whether it is newer;
//! 2. [`download`] fetches its installer and checks it against the digest GitHub
//!    publishes, refusing anything that does not come from this repository;
//! 3. [`launch_installer`] starts it silently. The installer closes Iris, replaces
//!    it, and starts the new version.
//!
//! What GitHub learns: that some copy of Iris asked, from this IP address, which is
//! the latest version. Nothing about the mail, the accounts, or who is asking.

use crate::changelog::{self, Release};
use iris_types::{Error, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where releases are published.
pub const REPOSITORY: &str = "Alicienn/iris";

/// This build's version.
pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// A version number, `major.minor.patch`.
///
/// The leftmost number weighs most: 0.10.0 is newer than 0.9.3. Compared as numbers,
/// never as text, where "0.10" would sort before "0.9".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// Reads `1.2.3` or `v1.2.3`. A pre-release suffix (`-beta.1`) is ignored: this
    /// project does not publish them as releases.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches('v');
        let text = text.split(['-', '+']).next()?;
        let mut parts = text.split('.').map(|p| p.parse::<u64>().ok());
        let v = Version(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(v)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The installer attached to a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installer {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// Hex, lowercase. GitHub computes it on upload; older assets may lack it.
    pub sha256: Option<String>,
}

/// A newer version, ready to be offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    pub version: Version,
    pub tag: String,
    pub installer: Installer,
    /// What changed since the running version, newest first.
    pub notes: Vec<Release>,
}

// --- What GitHub answers, reduced to what is read ---

#[derive(Debug, Deserialize)]
pub struct GithubRelease {
    pub tag_name: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
pub struct GithubAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    #[serde(default)]
    pub digest: Option<String>,
}

fn client(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        // GitHub refuses API calls without one.
        .user_agent(format!("Iris/{}", current()))
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout)
        .build()
        .map_err(|e| Error::other(format!("update client: {e}")))
}

/// Is this release newer than `running`, and installable?
///
/// Pure, so the decision is tested without a network: drafts, pre-releases, older
/// versions and releases without an installer are all "no".
pub fn evaluate(release: &GithubRelease, running: &str) -> Option<(Version, Installer)> {
    if release.draft || release.prerelease {
        return None;
    }
    let version = Version::parse(&release.tag_name)?;
    if version <= Version::parse(running)? {
        return None;
    }
    let asset = release
        .assets
        .iter()
        .find(|a| a.name.starts_with("iris-setup-") && a.name.ends_with(".exe"))?;
    Some((
        version,
        Installer {
            name: asset.name.clone(),
            url: asset.browser_download_url.clone(),
            size: asset.size,
            sha256: asset
                .digest
                .as_deref()
                .and_then(|d| d.strip_prefix("sha256:"))
                .map(str::to_ascii_lowercase),
        },
    ))
}

/// Asks GitHub whether a newer version exists.
///
/// `Ok(None)` means up to date — including when the repository has no release yet,
/// which is not an error anyone can act on.
pub async fn check() -> Result<Option<Available>> {
    let http = client(Duration::from_secs(20))?;
    let reponse = http
        .get(format!(
            "https://api.github.com/repos/{REPOSITORY}/releases/latest"
        ))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| Error::other(format!("could not reach GitHub: {e}")))?;

    if reponse.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let reponse = reponse
        .error_for_status()
        .map_err(|e| Error::other(format!("GitHub refused: {e}")))?;
    let octets = reponse
        .bytes()
        .await
        .map_err(|e| Error::other(format!("could not read GitHub's answer: {e}")))?;
    let release: GithubRelease = serde_json::from_slice(&octets)
        .map_err(|e| Error::other(format!("unreadable answer from GitHub: {e}")))?;

    let Some((version, installer)) = evaluate(&release, current()) else {
        return Ok(None);
    };

    // The notes of every version in between, from the changelog as it stands at that
    // tag. The release text only describes the last one; failing that, it will do.
    let mut notes = match changelog_at(&http, &release.tag_name).await {
        Some(texte) => changelog::newer_than(&changelog::parse(&texte), current()),
        None => Vec::new(),
    };
    if notes.is_empty() {
        let corps = release.body.clone().unwrap_or_default();
        notes = changelog::parse(&format!("## {version}\n{corps}"));
    }

    Ok(Some(Available {
        version,
        tag: release.tag_name,
        installer,
        notes,
    }))
}

async fn changelog_at(http: &reqwest::Client, tag: &str) -> Option<String> {
    let url = format!("https://raw.githubusercontent.com/{REPOSITORY}/{tag}/CHANGELOG.md");
    let reponse = http.get(url).send().await.ok()?.error_for_status().ok()?;
    reponse.text().await.ok()
}

/// Where installers are downloaded. Temporary: the installer is not kept once run.
pub fn download_dir() -> PathBuf {
    std::env::temp_dir().join("iris-update")
}

/// Downloads the installer and verifies it. Returns where it was written.
///
/// `progress(received, total)` is called as bytes arrive.
///
/// Refused outright: a URL outside this repository's releases, a file name that could
/// climb out of `dir`, a size or digest that does not match. A corrupted or substituted
/// installer is never left under the final name, so nothing can run it by mistake.
pub async fn download(
    installer: &Installer,
    dir: &Path,
    progress: impl Fn(u64, u64) + Send,
) -> Result<PathBuf> {
    let attendu = format!("https://github.com/{REPOSITORY}/releases/download/");
    if !installer.url.starts_with(&attendu) {
        return Err(Error::other(format!(
            "refusing an installer from outside {REPOSITORY}"
        )));
    }
    if installer.name.contains(['/', '\\']) || installer.name.contains("..") {
        return Err(Error::other("refusing an installer with an odd name"));
    }

    std::fs::create_dir_all(dir).map_err(|e| Error::other(format!("{}: {e}", dir.display())))?;
    let final_path = dir.join(&installer.name);
    let partiel = dir.join(format!("{}.part", installer.name));

    let http = client(Duration::from_secs(15 * 60))?;
    let mut reponse = http
        .get(&installer.url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| Error::other(format!("download failed: {e}")))?;

    let total = reponse.content_length().unwrap_or(installer.size);
    let mut fichier = std::fs::File::create(&partiel)
        .map_err(|e| Error::other(format!("{}: {e}", partiel.display())))?;
    let mut empreinte = ring::digest::Context::new(&ring::digest::SHA256);
    let mut recus: u64 = 0;

    while let Some(morceau) = reponse
        .chunk()
        .await
        .map_err(|e| Error::other(format!("download interrupted: {e}")))?
    {
        use std::io::Write;
        fichier
            .write_all(&morceau)
            .map_err(|e| Error::other(format!("{}: {e}", partiel.display())))?;
        empreinte.update(&morceau);
        recus += morceau.len() as u64;
        progress(recus, total);
    }
    drop(fichier);

    let verifie = verify(installer, recus, empreinte.finish().as_ref());
    if let Err(e) = verifie {
        let _ = std::fs::remove_file(&partiel);
        return Err(e);
    }

    let _ = std::fs::remove_file(&final_path);
    std::fs::rename(&partiel, &final_path)
        .map_err(|e| Error::other(format!("{}: {e}", final_path.display())))?;
    Ok(final_path)
}

/// Does what arrived match what GitHub announced?
fn verify(installer: &Installer, received: u64, sha256: &[u8]) -> Result<()> {
    if received != installer.size {
        return Err(Error::other(format!(
            "the download is {received} bytes, {} were announced",
            installer.size
        )));
    }
    if let Some(attendue) = &installer.sha256 {
        if hex::encode(sha256) != *attendue {
            return Err(Error::other(
                "the download does not match its published digest",
            ));
        }
    }
    Ok(())
}

/// Starts the installer and returns at once. The caller then quits.
///
/// Silent: a progress window, no questions — the choices made at the first install
/// are kept. `/UPDATE` tells the installer to start Iris again when it is done, which
/// a silent install otherwise never does.
pub fn launch_installer(path: &Path) -> Result<()> {
    if !cfg!(windows) {
        return Err(Error::other("updates install on Windows only"));
    }
    std::process::Command::new(path)
        .args([
            "/SILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/CLOSEAPPLICATIONS",
            "/UPDATE",
        ])
        .spawn()
        .map(|_| ())
        .map_err(|e| Error::other(format!("could not start the installer: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, assets: &[&str]) -> GithubRelease {
        GithubRelease {
            tag_name: tag.into(),
            body: None,
            draft: false,
            prerelease: false,
            assets: assets
                .iter()
                .map(|n| GithubAsset {
                    name: (*n).into(),
                    browser_download_url: format!(
                        "https://github.com/{REPOSITORY}/releases/download/{tag}/{n}"
                    ),
                    size: 10,
                    digest: Some("sha256:ABCD".into()),
                })
                .collect(),
        }
    }

    #[test]
    fn versions_compare_as_numbers() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("0.10.0") > v("0.9.3"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("0.2.1") > v("0.2.0"));
        assert_eq!(v("v1.2.3"), Version(1, 2, 3));
        assert_eq!(v("1.2.3-beta.1"), Version(1, 2, 3));
        assert_eq!(Version::parse("1.2"), None);
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert_eq!(Version::parse("latest"), None);
    }

    #[test]
    fn a_newer_release_with_an_installer_is_offered() {
        let r = release("v0.3.0", &["iris-setup-0.3.0.exe", "notes.txt"]);
        let (version, installer) = evaluate(&r, "0.2.0").unwrap();
        assert_eq!(version, Version(0, 3, 0));
        assert_eq!(installer.name, "iris-setup-0.3.0.exe");
        assert_eq!(installer.sha256.as_deref(), Some("abcd"));
    }

    #[test]
    fn nothing_is_offered_when_it_is_not_newer_or_not_installable() {
        assert!(evaluate(&release("v0.2.0", &["iris-setup-0.2.0.exe"]), "0.2.0").is_none());
        assert!(evaluate(&release("v0.1.0", &["iris-setup-0.1.0.exe"]), "0.2.0").is_none());
        assert!(evaluate(&release("v0.3.0", &["source.zip"]), "0.2.0").is_none());

        let mut brouillon = release("v0.3.0", &["iris-setup-0.3.0.exe"]);
        brouillon.draft = true;
        assert!(evaluate(&brouillon, "0.2.0").is_none());
        let mut essai = release("v0.3.0", &["iris-setup-0.3.0.exe"]);
        essai.prerelease = true;
        assert!(evaluate(&essai, "0.2.0").is_none());
    }

    #[test]
    fn a_download_that_does_not_match_is_refused() {
        let installer = Installer {
            name: "iris-setup-0.3.0.exe".into(),
            url: String::new(),
            size: 3,
            sha256: Some(hex::encode(ring::digest::digest(&ring::digest::SHA256, b"abc"))),
        };
        let bon = ring::digest::digest(&ring::digest::SHA256, b"abc");
        let mauvais = ring::digest::digest(&ring::digest::SHA256, b"abd");
        assert!(verify(&installer, 3, bon.as_ref()).is_ok());
        assert!(verify(&installer, 3, mauvais.as_ref()).is_err());
        assert!(verify(&installer, 2, bon.as_ref()).is_err());
    }

    #[tokio::test]
    async fn an_installer_from_elsewhere_is_never_downloaded() {
        let installer = Installer {
            name: "iris-setup-9.9.9.exe".into(),
            url: "https://example.com/iris-setup-9.9.9.exe".into(),
            size: 1,
            sha256: None,
        };
        let dir = std::env::temp_dir().join("iris-update-test");
        assert!(download(&installer, &dir, |_, _| {}).await.is_err());
    }
}
