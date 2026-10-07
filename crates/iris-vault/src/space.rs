//! A space: a folder of notes, its tree, and the files in it.
//!
//! Paths inside a space are **relative and written with `/`** (`Analyse/Chapitre 1.md`)
//! everywhere above this module: they are what the tree, the links and the settings
//! keep, whatever the system writes.

use iris_types::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Where pasted images and files go.
pub const ATTACHMENTS: &str = "_Fichiers";
/// Where templates are.
pub const TEMPLATES: &str = "_Modèles";

/// A space's settings, in `.iris/space.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpaceConfig {
    pub name: String,
    /// `#rrggbb`.
    pub color: String,
    pub icon: String,
    pub attachments: String,
    pub templates: String,
    /// Notes and folders pinned at the top, by path.
    pub pinned: Vec<String>,
    /// A colour of their own for some folders, by path.
    pub folder_colors: BTreeMap<String, String>,
}

impl Default for SpaceConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            color: "#5b8def".into(),
            icon: "notes".into(),
            attachments: ATTACHMENTS.into(),
            templates: TEMPLATES.into(),
            pinned: Vec::new(),
            folder_colors: BTreeMap::new(),
        }
    }
}

/// What a file is, by its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Folder,
    Note,
    Sheet,
    Image,
    Other,
}

impl EntryKind {
    pub fn of(path: &str) -> EntryKind {
        let ext = path
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "md" | "markdown" | "txt" => EntryKind::Note,
            "sheet" => EntryKind::Sheet,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => EntryKind::Image,
            _ => EntryKind::Other,
        }
    }
}

/// A line of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Relative path, with `/`.
    pub rel: String,
    /// What is shown: a note without its `.md`.
    pub name: String,
    pub kind: EntryKind,
    pub depth: usize,
    /// Last change, in milliseconds since 1970.
    pub modified: i64,
    /// A folder with something in it.
    pub has_children: bool,
}

/// What became of a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    /// Written; the file's new modification time.
    Written(i64),
    /// The file had changed on the disk since it was read: that version was kept
    /// beside it (`saved_as`, relative), and ours written in its place.
    Conflict { saved_as: String, modified: i64 },
}

/// Where deleted files go: the Windows Recycle Bin in the application.
pub trait RecycleBin {
    fn throw(&self, path: &Path) -> Result<()>;
}

/// A folder of notes.
#[derive(Debug, Clone)]
pub struct Space {
    dir: PathBuf,
    pub config: SpaceConfig,
}

/// Whether a name is hidden from the tree: `.iris`, `.obsidian`, `.git`, temporaries.
pub(crate) fn cache(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_none_or(|n| n.starts_with('.') || n.ends_with(".iris-tmp"))
}

/// The last part of a path.
pub(crate) fn nom(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// How names sort: without case or accents, numbers by their value ("Chapitre 2"
/// before "Chapitre 10").
pub(crate) fn cle_de_tri(nom: &str) -> Vec<(u8, String, u64)> {
    let mut cle = Vec::new();
    let mut chiffres = String::new();
    let mut lettres = String::new();
    let vider = |lettres: &mut String, chiffres: &mut String, cle: &mut Vec<(u8, String, u64)>| {
        if !lettres.is_empty() {
            cle.push((1, std::mem::take(lettres), 0));
        }
        if !chiffres.is_empty() {
            cle.push((0, String::new(), chiffres.parse().unwrap_or(u64::MAX)));
            chiffres.clear();
        }
    };
    for c in nom.chars() {
        if c.is_ascii_digit() {
            if !lettres.is_empty() {
                cle.push((1, std::mem::take(&mut lettres), 0));
            }
            chiffres.push(c);
        } else {
            if !chiffres.is_empty() {
                cle.push((0, String::new(), chiffres.parse().unwrap_or(u64::MAX)));
                chiffres.clear();
            }
            lettres.extend(sans_accent(c).to_lowercase());
        }
    }
    vider(&mut lettres, &mut chiffres, &mut cle);
    cle
}

/// A letter without its accent, for sorting and searching.
pub fn sans_accent(c: char) -> char {
    match c {
        'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => 'a',
        'À' | 'Â' | 'Ä' | 'Á' | 'Ã' | 'Å' => 'A',
        'ç' => 'c',
        'Ç' => 'C',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'É' | 'È' | 'Ê' | 'Ë' => 'E',
        'î' | 'ï' | 'í' | 'ì' => 'i',
        'Î' | 'Ï' | 'Í' | 'Ì' => 'I',
        'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
        'Ô' | 'Ö' | 'Ó' | 'Ò' | 'Õ' => 'O',
        'ù' | 'û' | 'ü' | 'ú' => 'u',
        'Ù' | 'Û' | 'Ü' | 'Ú' => 'U',
        'ÿ' => 'y',
        'ñ' => 'n',
        'Ñ' => 'N',
        c => c,
    }
}

/// A file or folder name as typed, made safe for Windows; refused when nothing is left.
pub(crate) fn nom_valide(name: &str) -> Result<String> {
    let propre: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let propre = propre.trim().trim_end_matches(['.', ' ']).to_string();
    if propre.is_empty() || propre.starts_with('.') {
        return Err(Error::other("this name cannot be used"));
    }
    Ok(propre)
}

/// `dir/name.ext`, or `dir/name 2.ext`, `name 3`… when taken.
pub(crate) fn libre(dir: &Path, name: &str, ext: &str) -> PathBuf {
    let avec = |n: &str| {
        if ext.is_empty() {
            dir.join(n)
        } else {
            dir.join(format!("{n}.{ext}"))
        }
    };
    let mut chemin = avec(name);
    let mut k = 2;
    while chemin.exists() {
        chemin = avec(&format!("{name} {k}"));
        k += 1;
    }
    chemin
}

fn millis(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

const COURS: &str =
    "---\ncourse: {{course}}\ndate: {{date}}\n---\n# {{title}}\n\n{{cursor}}\n\n## À retenir\n\n";
const REUNION: &str = "---\ndate: {{date}}\n---\n# {{title}}\n\n**Présents :** \n\n## Ordre du jour\n- {{cursor}}\n\n## Décisions\n\n## À faire\n- [ ] \n";

impl Space {
    /// Opens a folder as a space, reading its settings when it has some.
    pub fn open(dir: PathBuf) -> Result<Self> {
        let fichier = dir.join(".iris").join("space.json");
        let mut config: SpaceConfig = match std::fs::read_to_string(&fichier) {
            Ok(texte) => serde_json::from_str(&texte).unwrap_or_default(),
            Err(_) => SpaceConfig::default(),
        };
        if config.name.trim().is_empty() {
            config.name = nom(&dir);
        }
        Ok(Self { dir, config })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The settings, written back.
    pub fn save_config(&self) -> Result<()> {
        let dossier = self.dir.join(".iris");
        std::fs::create_dir_all(&dossier)?;
        let texte =
            serde_json::to_string_pretty(&self.config).map_err(|e| Error::other(e.to_string()))?;
        ecrire_atomique(&dossier.join("space.json"), texte.as_bytes())
    }

    /// What is known of the space's flashcards, in `.iris/review.json`, by card key.
    pub fn review(&self) -> BTreeMap<String, iris_notes::review::CardState> {
        std::fs::read_to_string(self.dir.join(".iris").join("review.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// The flashcards' state, written back.
    pub fn save_review(
        &self,
        cards: &BTreeMap<String, iris_notes::review::CardState>,
    ) -> Result<()> {
        let dossier = self.dir.join(".iris");
        std::fs::create_dir_all(&dossier)?;
        let texte = serde_json::to_string(cards).map_err(|e| Error::other(e.to_string()))?;
        ecrire_atomique(&dossier.join("review.json"), texte.as_bytes())
    }

    /// The folders every space has, and its first templates.
    pub fn make_defaults(&self) -> Result<()> {
        std::fs::create_dir_all(self.dir.join(&self.config.attachments))?;
        let modeles = self.dir.join(&self.config.templates);
        std::fs::create_dir_all(&modeles)?;
        for (nom, texte) in [("Cours.md", COURS), ("Réunion.md", REUNION)] {
            let chemin = modeles.join(nom);
            if !chemin.exists() {
                std::fs::write(chemin, texte)?;
            }
        }
        Ok(())
    }

    /// A relative path as a path on the disk. `..` and absolute parts are refused:
    /// nothing outside the space is reached through it.
    pub fn path(&self, rel: &str) -> Result<PathBuf> {
        let mut p = self.dir.clone();
        for part in rel.split('/').filter(|s| !s.is_empty()) {
            if part == ".." || part.contains(':') || part.contains('\\') {
                return Err(Error::other("this path leaves the space"));
            }
            p.push(part);
        }
        Ok(p)
    }

    /// A path on the disk as a relative one, when it is in the space.
    pub fn rel(&self, path: &Path) -> Option<String> {
        let r = path.strip_prefix(&self.dir).ok()?;
        let parts: Vec<String> = r
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        Some(parts.join("/"))
    }

    /// The tree: folders first, then files, by name; what is in `expanded` folders
    /// listed under them. Hidden names (`.iris`, `.git`) are left out.
    pub fn tree(&self, expanded: &HashSet<String>) -> Result<Vec<Entry>> {
        let mut sortie = Vec::new();
        self.lister("", 0, expanded, &mut sortie)?;
        Ok(sortie)
    }

    fn lister(
        &self,
        rel: &str,
        depth: usize,
        expanded: &HashSet<String>,
        sortie: &mut Vec<Entry>,
    ) -> Result<()> {
        let dossier = self.path(rel)?;
        let Ok(lecture) = std::fs::read_dir(&dossier) else {
            return Ok(());
        };
        let mut entrees: Vec<(bool, String, std::fs::Metadata)> = lecture
            .filter_map(|e| e.ok())
            .filter(|e| !cache(&e.path()))
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                Some((meta.is_dir(), nom(&e.path()), meta))
            })
            .collect();
        // Folders first; `_Fichiers` and `_Modèles` after the others.
        entrees.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.starts_with('_').cmp(&b.1.starts_with('_')))
                .then_with(|| cle_de_tri(&a.1).cmp(&cle_de_tri(&b.1)))
        });
        for (est_dossier, nom, meta) in entrees {
            let chemin = if rel.is_empty() {
                nom.clone()
            } else {
                format!("{rel}/{nom}")
            };
            let kind = if est_dossier {
                EntryKind::Folder
            } else {
                EntryKind::of(&nom)
            };
            let affiche = match kind {
                EntryKind::Note | EntryKind::Sheet => nom
                    .rsplit_once('.')
                    .map_or(nom.clone(), |(n, _)| n.to_string()),
                _ => nom.clone(),
            };
            let has_children = est_dossier
                && std::fs::read_dir(self.path(&chemin)?)
                    .map(|mut l| l.any(|e| e.is_ok_and(|e| !cache(&e.path()))))
                    .unwrap_or(false);
            sortie.push(Entry {
                rel: chemin.clone(),
                name: affiche,
                kind,
                depth,
                modified: millis(&meta),
                has_children,
            });
            if est_dossier && expanded.contains(&chemin) {
                self.lister(&chemin, depth + 1, expanded, sortie)?;
            }
        }
        Ok(())
    }

    /// Every note of the space (`.md`), relative, templates included.
    pub fn notes(&self) -> Vec<String> {
        let mut sortie = Vec::new();
        let mut pile = vec![String::new()];
        while let Some(rel) = pile.pop() {
            let Ok(dossier) = self.path(&rel) else {
                continue;
            };
            let Ok(lecture) = std::fs::read_dir(dossier) else {
                continue;
            };
            for e in lecture.filter_map(|e| e.ok()) {
                let p = e.path();
                if cache(&p) {
                    continue;
                }
                let n = nom(&p);
                let chemin = if rel.is_empty() {
                    n.clone()
                } else {
                    format!("{rel}/{n}")
                };
                if p.is_dir() {
                    pile.push(chemin);
                } else if EntryKind::of(&n) == EntryKind::Note {
                    sortie.push(chemin);
                }
            }
        }
        sortie.sort();
        sortie
    }

    /// A fingerprint of everything in the space (names, sizes, times): it changes when
    /// anything does, here or elsewhere, and costs one walk of the folders.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let mut pile = vec![self.dir.clone()];
        let mut vus = Vec::new();
        while let Some(d) = pile.pop() {
            let Ok(lecture) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in lecture.filter_map(|e| e.ok()) {
                let p = e.path();
                if cache(&p) {
                    continue;
                }
                if let Ok(meta) = e.metadata() {
                    // A folder by its name only: Windows changes a folder's time on its
                    // own, after the fact, and a fingerprint must not move for that.
                    if meta.is_dir() {
                        pile.push(p.clone());
                        vus.push((p, 0, 0));
                    } else {
                        vus.push((p, meta.len(), millis(&meta)));
                    }
                }
            }
        }
        vus.sort();
        vus.hash(&mut h);
        h.finish()
    }

    /// A note's text, and its modification time.
    pub fn read(&self, rel: &str) -> Result<(String, i64)> {
        let p = self.path(rel)?;
        let octets = std::fs::read(&p)?;
        let meta = std::fs::metadata(&p)?;
        Ok((String::from_utf8_lossy(&octets).into_owned(), millis(&meta)))
    }

    /// When a file last changed, `None` when it is gone.
    pub fn modified(&self, rel: &str) -> Option<i64> {
        std::fs::metadata(self.path(rel).ok()?)
            .ok()
            .map(|m| millis(&m))
    }

    /// Writes a note whole. `expected`: the time it had when read; when the file
    /// changed since, the version on the disk is kept beside as
    /// `Name (conflict 2026-10-07 14.32).md` before ours is written.
    pub fn write(&self, rel: &str, text: &str, expected: Option<i64>) -> Result<WriteOutcome> {
        let p = self.path(rel)?;
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let actuel = std::fs::metadata(&p).ok().map(|m| millis(&m));
        let conflit = match (expected, actuel) {
            (Some(attendu), Some(maintenant)) => maintenant != attendu,
            _ => false,
        };
        let mut sauve = None;
        if conflit {
            let ancien = std::fs::read(&p)?;
            if ancien != text.as_bytes() {
                let (dir, base, ext) = decouper(&p);
                let horodatage = chrono::Local::now().format("%Y-%m-%d %H.%M");
                let copie = libre(&dir, &format!("{base} (conflict {horodatage})"), &ext);
                ecrire_atomique(&copie, &ancien)?;
                sauve = self.rel(&copie);
            }
        }
        ecrire_atomique(&p, text.as_bytes())?;
        let modifie = std::fs::metadata(&p).map(|m| millis(&m))?;
        Ok(match sauve {
            Some(saved_as) => WriteOutcome::Conflict {
                saved_as,
                modified: modifie,
            },
            None => WriteOutcome::Written(modifie),
        })
    }

    /// A new note in `folder`, named `name` (`.md` added), written with `text`. Gives
    /// its path.
    pub fn create_note(&self, folder: &str, name: &str, text: &str) -> Result<String> {
        let nom = nom_valide(name)?;
        let dir = self.path(folder)?;
        std::fs::create_dir_all(&dir)?;
        let chemin = libre(&dir, &nom, "md");
        ecrire_atomique(&chemin, text.as_bytes())?;
        self.rel(&chemin)
            .ok_or_else(|| Error::other("the note is outside the space"))
    }

    /// A new file of any kind (a spreadsheet: `ext` = "sheet").
    pub fn create_file(&self, folder: &str, name: &str, ext: &str, bytes: &[u8]) -> Result<String> {
        let nom = nom_valide(name)?;
        let dir = self.path(folder)?;
        std::fs::create_dir_all(&dir)?;
        let chemin = libre(&dir, &nom, ext);
        ecrire_atomique(&chemin, bytes)?;
        self.rel(&chemin)
            .ok_or_else(|| Error::other("the file is outside the space"))
    }

    /// A new folder in `parent`.
    pub fn create_folder(&self, parent: &str, name: &str) -> Result<String> {
        let nom = nom_valide(name)?;
        let chemin = libre(&self.path(parent)?, &nom, "");
        std::fs::create_dir_all(&chemin)?;
        self.rel(&chemin)
            .ok_or_else(|| Error::other("the folder is outside the space"))
    }

    /// Renames a note (its extension kept) or a folder. Gives the new path.
    pub fn rename(&self, rel: &str, new_name: &str) -> Result<String> {
        let ancien = self.path(rel)?;
        let nom = nom_valide(new_name)?;
        let (dir, base, ext) = decouper(&ancien);
        if ancien.is_dir() {
            if nom == nom_de(rel) {
                return Ok(rel.to_string());
            }
            let nouveau = libre(&dir, &nom, "");
            std::fs::rename(&ancien, &nouveau)?;
            return self
                .rel(&nouveau)
                .ok_or_else(|| Error::other("outside the space"));
        }
        if nom == base {
            return Ok(rel.to_string());
        }
        // Only the case changed: Windows sees the same name, and keeps it.
        let nouveau = if nom.eq_ignore_ascii_case(&base) {
            if ext.is_empty() {
                dir.join(&nom)
            } else {
                dir.join(format!("{nom}.{ext}"))
            }
        } else {
            libre(&dir, &nom, &ext)
        };
        std::fs::rename(&ancien, &nouveau)?;
        self.rel(&nouveau)
            .ok_or_else(|| Error::other("outside the space"))
    }

    /// Moves a file or a folder into `folder` ("" for the top). A folder never goes
    /// into itself. Gives the new path.
    pub fn move_to(&self, rel: &str, folder: &str) -> Result<String> {
        if folder == rel || folder.starts_with(&format!("{rel}/")) {
            return Err(Error::other("a folder cannot go into itself"));
        }
        let ancien = self.path(rel)?;
        let cible = self.path(folder)?;
        if ancien.parent() == Some(cible.as_path()) {
            return Ok(rel.to_string());
        }
        std::fs::create_dir_all(&cible)?;
        let (_, base, ext) = decouper(&ancien);
        let nouveau = if ancien.is_dir() {
            libre(&cible, &nom(&ancien), "")
        } else {
            libre(&cible, &base, &ext)
        };
        std::fs::rename(&ancien, &nouveau)?;
        self.rel(&nouveau)
            .ok_or_else(|| Error::other("outside the space"))
    }

    /// A copy beside it ("Name 2"). Gives its path.
    pub fn duplicate(&self, rel: &str) -> Result<String> {
        let ancien = self.path(rel)?;
        if ancien.is_dir() {
            return Err(Error::other("folders are not duplicated"));
        }
        let (dir, base, ext) = decouper(&ancien);
        let copie = libre(&dir, &base, &ext);
        std::fs::copy(&ancien, &copie)?;
        self.rel(&copie)
            .ok_or_else(|| Error::other("outside the space"))
    }

    /// Sends a file or a folder to the bin.
    pub fn delete(&self, rel: &str, bin: &dyn RecycleBin) -> Result<()> {
        if rel.trim_matches('/').is_empty() {
            return Err(Error::other("the space itself is not deleted here"));
        }
        bin.throw(&self.path(rel)?)
    }

    /// Keeps bytes pasted or dropped into a note: `_Fichiers/<name>.<ext>`, a free
    /// name. Gives its path.
    pub fn save_attachment(&self, name: &str, ext: &str, bytes: &[u8]) -> Result<String> {
        let dir = self.dir.join(&self.config.attachments);
        std::fs::create_dir_all(&dir)?;
        let nom = nom_valide(name).unwrap_or_else(|_| "Fichier".into());
        let chemin = libre(&dir, &nom, ext);
        ecrire_atomique(&chemin, bytes)?;
        self.rel(&chemin)
            .ok_or_else(|| Error::other("outside the space"))
    }

    /// After notes moved or were renamed (`moves`: old and new paths, `before`: the
    /// space's notes as they were), every link to them in the space written again.
    /// Gives the notes that changed.
    pub fn relink(&self, moves: &[(String, String)], before: &[String]) -> Result<Vec<String>> {
        let apres = self.notes();
        let mut changees = Vec::new();
        for rel in &apres {
            let Ok((mut texte, _)) = self.read(rel) else {
                continue;
            };
            let mut change = false;
            for (ancien, nouveau) in moves {
                if let Some(t) =
                    iris_notes::links::rewrite_links(&texte, ancien, nouveau, before, &apres)
                {
                    texte = t;
                    change = true;
                }
            }
            if change {
                self.write(rel, &texte, None)?;
                changees.push(rel.clone());
            }
        }
        Ok(changees)
    }

    /// The notes moved with a folder or a note: (old path, new path) of each note.
    pub fn moves_of(before: &[String], old: &str, new: &str) -> Vec<(String, String)> {
        before
            .iter()
            .filter_map(|n| {
                if n == old {
                    Some((n.clone(), new.to_string()))
                } else {
                    n.strip_prefix(&format!("{old}/"))
                        .map(|reste| (n.clone(), format!("{new}/{reste}")))
                }
            })
            .collect()
    }

    /// The notes that link to `rel`, with the line of each link.
    pub fn backlinks(&self, rel: &str) -> Vec<(String, String)> {
        let notes = self.notes();
        let mut sortie = Vec::new();
        for n in &notes {
            if n == rel {
                continue;
            }
            let Ok((texte, _)) = self.read(n) else {
                continue;
            };
            for l in iris_notes::links::links(&texte) {
                if iris_notes::links::resolve(&l.target, &notes).as_deref() == Some(rel) {
                    sortie.push((n.clone(), l.line.clone()));
                }
            }
        }
        sortie
    }

    /// Every tag used in the space, by how often, the most used first.
    pub fn all_tags(&self) -> Vec<String> {
        let mut compte: BTreeMap<String, usize> = BTreeMap::new();
        let modeles = format!("{}/", self.config.templates);
        for n in self
            .notes()
            .into_iter()
            .filter(|n| !n.starts_with(&modeles))
        {
            if let Ok((texte, _)) = self.read(&n) {
                for t in iris_notes::meta::tags(&texte) {
                    *compte.entry(t).or_insert(0) += 1;
                }
            }
        }
        let mut tags: Vec<(String, usize)> = compte.into_iter().collect();
        tags.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        tags.into_iter().map(|(t, _)| t).collect()
    }

    /// The notes holding every word of `query` (case and accents aside), with the first
    /// line that holds one. `tag:x` keeps the notes tagged `x`, `path:x` those whose path
    /// holds `x`, `is:task` those with a checkbox left to tick.
    pub fn search(&self, query: &str, max: usize) -> Vec<(String, String)> {
        let mut mots = Vec::new();
        let mut etiquettes = Vec::new();
        let mut chemins = Vec::new();
        let mut taches = false;
        for m in query.split_whitespace() {
            if let Some(t) = m.strip_prefix("tag:").or_else(|| m.strip_prefix('#')) {
                etiquettes.push(iris_notes::fuzzy::folded(t));
            } else if let Some(p) = m.strip_prefix("path:") {
                chemins.push(iris_notes::fuzzy::folded(p));
            } else if m == "is:task" {
                taches = true;
            } else {
                mots.push(iris_notes::fuzzy::folded(m));
            }
        }
        let mut sortie = Vec::new();
        let modeles = format!("{}/", self.config.templates);
        for n in self.notes() {
            if sortie.len() >= max {
                break;
            }
            // Templates are not notes one looks for.
            if n.starts_with(&modeles) {
                continue;
            }
            let chemin = iris_notes::fuzzy::folded(&n);
            if !chemins.iter().all(|p| chemin.contains(p)) {
                continue;
            }
            let Ok((texte, _)) = self.read(&n) else {
                continue;
            };
            if !etiquettes.is_empty() {
                let ses: Vec<String> = iris_notes::meta::tags(&texte)
                    .iter()
                    .map(|t| iris_notes::fuzzy::folded(t))
                    .collect();
                if !etiquettes.iter().all(|e| {
                    ses.iter()
                        .any(|s| s == e || s.starts_with(&format!("{e}/")))
                }) {
                    continue;
                }
            }
            if taches && !texte.lines().any(|l| l.trim_start().starts_with("- [ ]")) {
                continue;
            }
            let plie = iris_notes::fuzzy::folded(&texte);
            let titre = iris_notes::fuzzy::folded(iris_notes::links::stem(&n));
            if !mots.iter().all(|m| plie.contains(m) || titre.contains(m)) {
                continue;
            }
            let ligne = texte
                .lines()
                .find(|l| {
                    let p = iris_notes::fuzzy::folded(l);
                    mots.iter().any(|m| p.contains(m))
                })
                .unwrap_or_else(|| texte.lines().find(|l| !l.trim().is_empty()).unwrap_or(""))
                .trim()
                .chars()
                .take(140)
                .collect();
            sortie.push((n, ligne));
        }
        sortie
    }

    /// The templates, by name (without `.md`).
    pub fn templates(&self) -> Vec<(String, String)> {
        let dir = self.dir.join(&self.config.templates);
        let mut sortie: Vec<(String, String)> = std::fs::read_dir(dir)
            .map(|l| {
                l.filter_map(|e| e.ok())
                    .filter(|e| EntryKind::of(&nom(&e.path())) == EntryKind::Note)
                    .map(|e| {
                        let n = nom(&e.path());
                        let base = n.rsplit_once('.').map_or(n.clone(), |(b, _)| b.to_string());
                        (base, format!("{}/{n}", self.config.templates))
                    })
                    .collect()
            })
            .unwrap_or_default();
        sortie.sort();
        sortie
    }
}

/// The last part of a relative path.
fn nom_de(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// A file's folder, its name without the extension, and its extension.
fn decouper(p: &Path) -> (PathBuf, String, String) {
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let n = nom(p);
    match n.rsplit_once('.') {
        Some((b, e)) if !b.is_empty() => (dir, b.to_string(), e.to_string()),
        _ => (dir, n, String::new()),
    }
}

/// Writes beside, then renames over: never half a file.
pub(crate) fn ecrire_atomique(path: &Path, bytes: &[u8]) -> Result<()> {
    let provisoire = path.with_file_name(format!("{}.iris-tmp", nom(path)));
    std::fs::write(&provisoire, bytes)?;
    std::fs::rename(&provisoire, path).map_err(|e| {
        let _ = std::fs::remove_file(&provisoire);
        Error::from(e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn espace() -> (tempfile::TempDir, Space) {
        let dir = tempfile::tempdir().unwrap();
        let s = Space::open(dir.path().to_path_buf()).unwrap();
        s.save_config().unwrap();
        s.make_defaults().unwrap();
        (dir, s)
    }

    struct Corbeille(RefCell<Vec<PathBuf>>);
    impl RecycleBin for Corbeille {
        fn throw(&self, path: &Path) -> Result<()> {
            self.0.borrow_mut().push(path.to_path_buf());
            std::fs::remove_file(path).or_else(|_| std::fs::remove_dir_all(path))?;
            Ok(())
        }
    }

    #[test]
    fn flashcard_states_are_kept() {
        let (_d, s) = espace();
        assert!(s.review().is_empty());
        let jour = chrono::NaiveDate::from_ymd_opt(2026, 10, 7).unwrap();
        let mut cartes = BTreeMap::new();
        cartes.insert(
            "Cours.md|Limite ?".to_string(),
            iris_notes::review::CardState::new(jour),
        );
        s.save_review(&cartes).unwrap();
        assert_eq!(s.review(), cartes);
    }

    #[test]
    fn the_tree_lists_folders_first_numbers_by_value() {
        let (_d, s) = espace();
        s.create_folder("", "Analyse").unwrap();
        s.create_note("Analyse", "Chapitre 10", "").unwrap();
        s.create_note("Analyse", "Chapitre 2", "").unwrap();
        s.create_note("", "Accueil", "").unwrap();
        let ouverts: HashSet<String> = ["Analyse".to_string()].into();
        let t = s.tree(&ouverts).unwrap();
        let noms: Vec<(&str, usize)> = t.iter().map(|e| (e.name.as_str(), e.depth)).collect();
        assert_eq!(
            noms,
            [
                ("Analyse", 0),
                ("Chapitre 2", 1),
                ("Chapitre 10", 1),
                (ATTACHMENTS, 0),
                (TEMPLATES, 0),
                ("Accueil", 0)
            ]
        );
        assert!(t[0].has_children);
        assert_eq!(t[1].rel, "Analyse/Chapitre 2.md");
        assert_eq!(t[1].kind, EntryKind::Note);
    }

    #[test]
    fn writes_are_whole_and_conflicts_keep_both() {
        let (_d, s) = espace();
        let rel = s.create_note("", "Note", "un").unwrap();
        let (texte, t0) = s.read(&rel).unwrap();
        assert_eq!(texte, "un");
        // Changed elsewhere after it was read.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(s.path(&rel).unwrap(), "ailleurs").unwrap();
        let issue = s.write(&rel, "ici", Some(t0)).unwrap();
        let WriteOutcome::Conflict { saved_as, .. } = issue else {
            panic!("a conflict: {issue:?}")
        };
        assert!(saved_as.starts_with("Note (conflict "), "{saved_as}");
        assert_eq!(s.read(&saved_as).unwrap().0, "ailleurs");
        assert_eq!(s.read(&rel).unwrap().0, "ici");
        // No temporary file left.
        assert!(std::fs::read_dir(s.dir())
            .unwrap()
            .all(|e| !nom(&e.unwrap().path()).ends_with(".iris-tmp")));
        // Read again, no conflict.
        let (_, t1) = s.read(&rel).unwrap();
        assert!(matches!(
            s.write(&rel, "encore", Some(t1)).unwrap(),
            WriteOutcome::Written(_)
        ));
    }

    #[test]
    fn names_are_free_and_safe() {
        let (_d, s) = espace();
        assert_eq!(
            s.create_note("", "Cours: 1/2", "").unwrap(),
            "Cours- 1-2.md"
        );
        assert_eq!(s.create_note("", "A", "").unwrap(), "A.md");
        assert_eq!(s.create_note("", "A", "").unwrap(), "A 2.md");
        assert!(s.create_note("", "  ", "").is_err());
        assert!(s.path("../dehors").is_err());
    }

    #[test]
    fn rename_move_duplicate_delete() {
        let (_d, s) = espace();
        let n = s.create_note("", "Brouillon", "x").unwrap();
        let n = s.rename(&n, "Propre").unwrap();
        assert_eq!(n, "Propre.md");
        let n = s.rename(&n, "propre").unwrap();
        assert_eq!(n, "propre.md", "only the case changed");
        let f = s.create_folder("", "Archive").unwrap();
        let n = s.move_to(&n, &f).unwrap();
        assert_eq!(n, "Archive/propre.md");
        assert!(s.move_to("Archive", "Archive").is_err());
        let copie = s.duplicate(&n).unwrap();
        assert_eq!(copie, "Archive/propre 2.md");
        let bin = Corbeille(RefCell::new(Vec::new()));
        s.delete(&copie, &bin).unwrap();
        assert_eq!(bin.0.borrow().len(), 1);
        assert!(s.delete("", &bin).is_err());
        assert_eq!(
            s.notes()
                .iter()
                .filter(|n| !n.starts_with(TEMPLATES))
                .count(),
            1
        );
    }

    #[test]
    fn the_fingerprint_follows_changes() {
        let (_d, s) = espace();
        let avant = s.fingerprint();
        assert_eq!(avant, s.fingerprint());
        s.create_note("", "Nouvelle", "x").unwrap();
        assert_ne!(avant, s.fingerprint());
    }

    #[test]
    fn renaming_a_note_rewrites_the_links_to_it() {
        let (_d, s) = espace();
        s.create_folder("", "Analyse").unwrap();
        let cible = s.create_note("Analyse", "Limites", "# Limites\n").unwrap();
        s.create_note("", "Cours", "Voir [[Limites#Def|déf]] et [[Autre]].\n")
            .unwrap();
        let avant = s.notes();
        let nouveau = s.rename(&cible, "Limites et suites").unwrap();
        let changees = s
            .relink(&Space::moves_of(&avant, &cible, &nouveau), &avant)
            .unwrap();
        assert_eq!(changees, ["Cours.md"]);
        assert_eq!(
            s.read("Cours.md").unwrap().0,
            "Voir [[Limites et suites#Def|déf]] et [[Autre]].\n"
        );
        assert_eq!(
            s.backlinks(&nouveau),
            [(
                "Cours.md".to_string(),
                "Voir [[Limites et suites#Def|déf]] et [[Autre]].".to_string()
            )]
        );
        // A folder moved: its notes move with it.
        let avant = s.notes();
        let dossier = s.create_folder("", "Maths").unwrap();
        let deplace = s.move_to("Analyse", &dossier).unwrap();
        let m = Space::moves_of(&avant, "Analyse", &deplace);
        assert_eq!(
            m,
            [(
                "Analyse/Limites et suites.md".to_string(),
                "Maths/Analyse/Limites et suites.md".to_string()
            )]
        );
    }

    #[test]
    fn search_tags_and_paths() {
        let (_d, s) = espace();
        s.create_note(
            "",
            "Suites",
            "Une suite #maths converge.\n- [ ] exercice 3\n",
        )
        .unwrap();
        s.create_note("", "Recette", "Une tarte #cuisine.\n")
            .unwrap();
        let r = s.search("suite", 10);
        assert_eq!(r[0].0, "Suites.md");
        assert_eq!(r[0].1, "Une suite #maths converge.");
        assert_eq!(s.search("tag:cuisine", 10)[0].0, "Recette.md");
        assert_eq!(s.search("is:task", 10).len(), 1);
        assert!(s.search("introuvable", 10).is_empty());
        assert_eq!(s.all_tags(), ["cuisine", "maths"]);
    }

    #[test]
    fn attachments_and_templates() {
        let (_d, s) = espace();
        let a = s.save_attachment("Image", "png", b"\x89PNG").unwrap();
        assert_eq!(a, format!("{ATTACHMENTS}/Image.png"));
        let noms: Vec<String> = s.templates().into_iter().map(|t| t.0).collect();
        assert_eq!(noms, ["Cours", "Réunion"]);
    }
}
