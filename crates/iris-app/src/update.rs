//! Updates, from the GitHub releases of the repository.
//!
//! In the core rather than a module, on purpose. A module runs in a sandbox with no
//! network, no file system and no way to start a program — which is exactly what makes
//! it safe to install one from a stranger. An updater needs all three: it downloads an
//! executable and runs it. Granting a module that would make every module suspect.
//!
//! Three steps, each its own function so each can be tested or retried alone:
//!
//! 1. [`check`] reads the latest release's manifest, checks the project's signature
//!    on it, and says whether it is newer;
//! 2. [`download`] fetches its installer and checks it against the size and digest the
//!    manifest gives, refusing anything that does not come from this repository;
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

/// The public half of the key the release workflow signs `latest.json` with (Ed25519).
/// The private half lives in the repository's secrets (`UPDATE_SIGNING_KEY`) and in its
/// owner's keeping, never in the repository.
pub const UPDATE_PUBLIC_KEY: [u8; 32] = [
    0x23, 0xf5, 0xc0, 0x49, 0x4e, 0xaa, 0x38, 0x31, 0xd0, 0x6a, 0x50, 0xe8, 0x96, 0x71, 0x4f, 0x3b,
    0x8a, 0x71, 0x3d, 0x44, 0x03, 0x52, 0xbb, 0x4b, 0x3a, 0x94, 0xcb, 0xa5, 0x9b, 0x1d, 0xfe, 0x03,
];

/// What each release publishes beside its installer, signed: `latest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub tag: String,
    pub installer: String,
    pub size: u64,
    pub sha256: String,
}

/// Reads a manifest, once its signature is checked against `key`. A manifest that does
/// not verify, or whose fields do not agree with each other (an installer named for
/// another version, a digest that is not one), is refused as a whole.
pub fn read_manifest(json: &[u8], signature: &[u8], key: &[u8]) -> Result<Manifest> {
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(json, signature)
        .map_err(|_| Error::other("the update's signature does not match: not installed"))?;
    let m: Manifest = serde_json::from_slice(json)
        .map_err(|e| Error::other(format!("unreadable update manifest: {e}")))?;
    let version = Version::parse(&m.version)
        .ok_or_else(|| Error::other("the update manifest has no version"))?;
    if m.tag != format!("v{version}")
        || m.installer != format!("iris-setup-{version}.exe")
        || m.sha256.len() != 64
        || !m.sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(Error::other("the update manifest does not hold together"));
    }
    Ok(m)
}

/// The installer a manifest describes, when it is newer than `running`.
pub fn from_manifest(m: &Manifest, running: &str) -> Option<(Version, Installer)> {
    let version = Version::parse(&m.version)?;
    if version <= Version::parse(running)? {
        return None;
    }
    Some((
        version,
        Installer {
            name: m.installer.clone(),
            url: format!(
                "https://github.com/{REPOSITORY}/releases/download/{}/{}",
                m.tag, m.installer
            ),
            size: m.size,
            sha256: Some(m.sha256.to_ascii_lowercase()),
        },
    ))
}

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

fn client(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        // Said plainly: which program is asking.
        .user_agent(format!("Iris/{}", current()))
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout)
        .build()
        .map_err(|e| Error::other(format!("update client: {e}")))
}

/// Asks whether a newer version exists, from the latest release's signed manifest.
///
/// Read from the site (`releases/latest/download/latest.json`), not from GitHub's API:
/// the API allows sixty calls an hour to each address without an account, and a school
/// or an office shares one address among everyone in it. Signed by the project, so a
/// release altered on GitHub is refused, not merely one GitHub itself vouches against.
///
/// `Ok(None)` means up to date. That includes a latest release without a manifest: every
/// version that reads manifests is newer than every release that lacks one. A manifest
/// that is there but does not verify is an error, never a reason to look elsewhere.
pub async fn check() -> Result<Option<Available>> {
    let http = client(Duration::from_secs(20))?;
    let Some(json) = latest_file(&http, "latest.json").await? else {
        return Ok(None);
    };
    let Some(signature) = latest_file(&http, "latest.json.sig").await? else {
        return Err(Error::other(
            "the update's signature is missing: not installed",
        ));
    };
    let m = read_manifest(&json, &signature, &UPDATE_PUBLIC_KEY)?;
    let Some((version, installer)) = from_manifest(&m, current()) else {
        return Ok(None);
    };
    // The notes of every version in between, from the changelog as it stands at that
    // tag.
    let notes = match changelog_at(&http, &m.tag).await {
        Some(texte) => changelog::newer_than(&changelog::parse(&texte), current()),
        None => Vec::new(),
    };
    Ok(Some(Available {
        version,
        tag: m.tag,
        installer,
        notes,
    }))
}

/// A file of the latest release, from the site; `None` when it has no such file.
async fn latest_file(http: &reqwest::Client, name: &str) -> Result<Option<Vec<u8>>> {
    let r = http
        .get(format!(
            "https://github.com/{REPOSITORY}/releases/latest/download/{name}"
        ))
        .send()
        .await
        .map_err(|e| Error::other(format!("could not reach GitHub: {e}")))?;
    if r.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let r = r
        .error_for_status()
        .map_err(|e| Error::other(format!("GitHub refused: {e}")))?;
    r.bytes()
        .await
        .map(|b| Some(b.to_vec()))
        .map_err(|e| Error::other(format!("could not read GitHub's answer: {e}")))
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

    /// A key of the tests' own, and a manifest signed with it: the project's private
    /// key is never here.
    fn signe(json: &str) -> (Vec<u8>, Vec<u8>) {
        use ring::signature::KeyPair;
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let cles = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        (
            cles.sign(json.as_bytes()).as_ref().to_vec(),
            cles.public_key().as_ref().to_vec(),
        )
    }

    const DIGEST: &str = "1f9c3a0b7d2e4c5f6a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f";

    fn manifeste(version: &str) -> String {
        format!(
            r#"{{"version":"{version}","tag":"v{version}","installer":"iris-setup-{version}.exe","size":18859571,"sha256":"{DIGEST}"}}"#
        )
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
    fn a_signed_manifest_for_a_newer_version_is_offered() {
        let json = manifeste("3.9.0");
        let (sig, cle) = signe(&json);
        let m = read_manifest(json.as_bytes(), &sig, &cle).unwrap();
        let (version, installer) = from_manifest(&m, "3.8.0").unwrap();
        assert_eq!(version, Version(3, 9, 0));
        assert_eq!(
            installer.url,
            format!(
                "https://github.com/{REPOSITORY}/releases/download/v3.9.0/iris-setup-3.9.0.exe"
            )
        );
        assert_eq!(installer.size, 18_859_571);
        assert_eq!(installer.sha256.as_deref(), Some(DIGEST));
        // Not newer: nothing.
        assert!(from_manifest(&m, "3.9.0").is_none());
        assert!(from_manifest(&m, "4.0.0").is_none());
    }

    #[test]
    fn a_manifest_that_does_not_verify_is_refused() {
        let json = manifeste("3.9.0");
        let (sig, cle) = signe(&json);
        // Changed after signing: the size, one byte of it.
        let change = json.replace("18859571", "18859572");
        assert!(read_manifest(change.as_bytes(), &sig, &cle).is_err());
        // Signed by another key than the one Iris holds.
        assert!(read_manifest(json.as_bytes(), &sig, &UPDATE_PUBLIC_KEY).is_err());
        // No signature.
        assert!(read_manifest(json.as_bytes(), &[], &cle).is_err());
    }

    /// A manifest signed as the release workflow signs it (`openssl pkeyutl -sign
    /// -rawin`) verifies against the key Iris holds:
    /// `IRIS_MANIFEST=<json> IRIS_MANIFEST_SIG=<sig> cargo test -p iris-app --lib -- --ignored signed_by_the_workflow`.
    #[test]
    #[ignore]
    fn a_manifest_signed_by_the_workflow_verifies() {
        let json = std::fs::read(std::env::var("IRIS_MANIFEST").unwrap()).unwrap();
        let sig = std::fs::read(std::env::var("IRIS_MANIFEST_SIG").unwrap()).unwrap();
        read_manifest(&json, &sig, &UPDATE_PUBLIC_KEY).unwrap();
    }

    #[test]
    fn a_signed_manifest_that_does_not_hold_together_is_refused() {
        // Signed, but its installer is named for another version.
        let json = manifeste("3.9.0").replace("iris-setup-3.9.0.exe", "iris-setup-1.0.0.exe");
        let (sig, cle) = signe(&json);
        assert!(read_manifest(json.as_bytes(), &sig, &cle).is_err());
        // Signed, but its digest is not one.
        let json = manifeste("3.9.0").replace(DIGEST, "abcd");
        let (sig, cle) = signe(&json);
        assert!(read_manifest(json.as_bytes(), &sig, &cle).is_err());
    }

    #[test]
    fn a_download_that_does_not_match_is_refused() {
        let installer = Installer {
            name: "iris-setup-0.3.0.exe".into(),
            url: String::new(),
            size: 3,
            sha256: Some(hex::encode(ring::digest::digest(
                &ring::digest::SHA256,
                b"abc",
            ))),
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
