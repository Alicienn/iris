//! Which note files to copy, from the two sides' manifests and the one of the last
//! sync with that device.
//!
//! Each side plans for itself, the other side plans the mirror image, so each only
//! ever *fetches* or *deletes here*: a file new or changed there is fetched; a file
//! deleted there and unchanged here since is deleted here; a file changed on both
//! sides is fetched by the side whose copy is older, which keeps its own copy as a
//! revision of the note. A rename is a deletion and a new file.

use crate::protocol::{ManifestEntry, NoteCopy};
use std::collections::{BTreeMap, BTreeSet};

/// A file as a manifest knows it: its path in the space (`/` between folders), size,
/// time and BLAKE3 hash.
pub fn entry_for(path: &str, bytes: &[u8], mtime_ms: i64) -> ManifestEntry {
    ManifestEntry {
        path: path.to_string(),
        size: bytes.len() as u64,
        mtime_ms,
        hash_hex: blake3::hash(bytes).to_hex().to_string(),
    }
}

/// Whether a path sent by the other device stays inside the space: relative, no `..`,
/// no hidden part (`.iris`, `.trash`), no drive.
pub fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && path
            .split('/')
            .all(|p| !p.is_empty() && !p.starts_with('.'))
}

/// What this side does with one space, `last` being the manifest both had after
/// their previous sync (empty the first time).
pub fn plan_copies(
    here: &[ManifestEntry],
    there: &[ManifestEntry],
    last: &[ManifestEntry],
) -> Vec<NoteCopy> {
    let par_chemin = |v: &[ManifestEntry]| -> BTreeMap<String, ManifestEntry> {
        v.iter().map(|e| (e.path.clone(), e.clone())).collect()
    };
    let (ici, la, avant) = (par_chemin(here), par_chemin(there), par_chemin(last));
    let chemins: BTreeSet<&String> = ici.keys().chain(la.keys()).collect();
    let mut plan = Vec::new();
    for chemin in chemins {
        let (i, l, a) = (ici.get(chemin), la.get(chemin), avant.get(chemin));
        let change = |e: &ManifestEntry| a.is_none_or(|a| a.hash_hex != e.hash_hex);
        match (i, l) {
            (Some(i), Some(l)) if i.hash_hex == l.hash_hex => {}
            (Some(i), Some(l)) => match (change(i), change(l)) {
                (false, true) => plan.push(NoteCopy::Fetch(chemin.clone())),
                (true, false) => {}
                // Changed on both sides, or met for the first time unlike.
                _ => {
                    let older_is_here = i.mtime_ms < l.mtime_ms
                        || (i.mtime_ms == l.mtime_ms && i.hash_hex < l.hash_hex);
                    plan.push(NoteCopy::KeepBoth {
                        path: chemin.clone(),
                        older_is_here,
                    });
                }
            },
            // There only: new there, or deleted here. Deleted here but changed
            // there since: it comes back.
            (None, Some(l)) => {
                if a.is_none() || change(l) {
                    plan.push(NoteCopy::Fetch(chemin.clone()));
                }
            }
            // Here only: deleted there if it was there before and is unchanged here.
            (Some(i), None) => {
                if a.is_some() && !change(i) {
                    plan.push(NoteCopy::DeleteHere(chemin.clone()));
                }
            }
            (None, None) => {}
        }
    }
    plan
}

/// The manifest both sides have once the plans are carried out: what `last` is next
/// time.
pub fn after_sync(
    here: &[ManifestEntry],
    there: &[ManifestEntry],
    plan: &[NoteCopy],
) -> Vec<ManifestEntry> {
    let mut v: BTreeMap<String, ManifestEntry> =
        here.iter().map(|e| (e.path.clone(), e.clone())).collect();
    let la: BTreeMap<&String, &ManifestEntry> = there.iter().map(|e| (&e.path, e)).collect();
    for c in plan {
        match c {
            NoteCopy::Fetch(p)
            | NoteCopy::KeepBoth {
                path: p,
                older_is_here: true,
            } => {
                if let Some(e) = la.get(p) {
                    v.insert(p.clone(), (*e).clone());
                }
            }
            NoteCopy::DeleteHere(p) => {
                v.remove(p);
            }
            _ => {}
        }
    }
    v.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(path: &str, hash: &str, mtime: i64) -> ManifestEntry {
        ManifestEntry {
            path: path.into(),
            size: 1,
            mtime_ms: mtime,
            hash_hex: hash.into(),
        }
    }

    #[test]
    fn a_path_from_the_other_side_stays_in_the_space() {
        assert!(safe_path("Cours/Chapitre 2.md"));
        for mauvais in [
            "",
            "../x.md",
            "a/../../x",
            "/etc/passwd",
            "C:/x",
            "a\\b",
            ".iris/space.json",
            "a/.trash/b",
            "a//b",
        ] {
            assert!(!safe_path(mauvais), "{mauvais}");
        }
    }

    #[test]
    fn new_there_is_fetched_new_here_is_left_for_the_other_side() {
        let p = plan_copies(&[e("a.md", "1", 1)], &[e("b.md", "2", 1)], &[]);
        assert_eq!(p, [NoteCopy::Fetch("b.md".into())]);
    }

    #[test]
    fn changed_there_only_is_fetched() {
        let avant = [e("a.md", "1", 1)];
        let p = plan_copies(&[e("a.md", "1", 1)], &[e("a.md", "2", 5)], &avant);
        assert_eq!(p, [NoteCopy::Fetch("a.md".into())]);
        let p = plan_copies(&[e("a.md", "2", 5)], &[e("a.md", "1", 1)], &avant);
        assert!(p.is_empty(), "changed here only: the other side fetches");
    }

    #[test]
    fn deleted_there_and_unchanged_here_goes() {
        let avant = [e("a.md", "1", 1)];
        assert_eq!(
            plan_copies(&[e("a.md", "1", 1)], &[], &avant),
            [NoteCopy::DeleteHere("a.md".into())]
        );
        assert!(
            plan_copies(&[e("a.md", "3", 9)], &[], &avant).is_empty(),
            "changed here since: kept, and the other side fetches it back"
        );
        assert_eq!(
            plan_copies(&[], &[e("a.md", "3", 9)], &avant),
            [NoteCopy::Fetch("a.md".into())],
            "deleted here, changed there: it comes back"
        );
        assert!(plan_copies(&[], &[e("a.md", "1", 1)], &avant).is_empty());
    }

    #[test]
    fn changed_on_both_the_older_side_fetches_and_keeps_a_revision() {
        let avant = [e("a.md", "1", 1)];
        let ici = [e("a.md", "2", 10)];
        let la = [e("a.md", "3", 20)];
        assert_eq!(
            plan_copies(&ici, &la, &avant),
            [NoteCopy::KeepBoth {
                path: "a.md".into(),
                older_is_here: true
            }]
        );
        assert_eq!(
            plan_copies(&la, &ici, &avant),
            [NoteCopy::KeepBoth {
                path: "a.md".into(),
                older_is_here: false
            }]
        );
    }

    #[test]
    fn a_rename_is_a_deletion_and_a_new_file() {
        let avant = [e("Old.md", "1", 1)];
        let p = plan_copies(&[e("Old.md", "1", 1)], &[e("New.md", "1", 2)], &avant);
        assert_eq!(
            p,
            [
                NoteCopy::Fetch("New.md".into()),
                NoteCopy::DeleteHere("Old.md".into())
            ]
        );
        let apres = after_sync(&[e("Old.md", "1", 1)], &[e("New.md", "1", 2)], &p);
        assert_eq!(apres, [e("New.md", "1", 2)]);
    }
}
