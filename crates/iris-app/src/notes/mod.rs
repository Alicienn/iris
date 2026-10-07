//! Notes: Markdown files in spaces, written block by block.
//!
//! The place lists a space's tree on the left and shows the open note on the right.
//! The note is held here as its text and its blocks (`iris_notes::block`); every key
//! that changes it goes through a pure edit (`iris_notes::edit`), the text is parsed
//! again, and only the blocks whose rendering changed are given back to the window.
//! It is written to its file half a second after the last key, atomically, and read
//! again when another program changes it.

mod bin;
pub mod render;

use crate::services::Services;
use iris_notes::block::{self, Block, BlockKind};
use iris_notes::edit::{self, Enter, NoteEdit};
use iris_notes::inline::{Colour, Palette};
use iris_ui::{AppWindow, NoteBlockData, NoteFoundData, NoteSpaceData, NoteTreeRowData};
use iris_vault::{EntryKind, Space, Vault, WriteOutcome};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How long after the last key the note is written.
const ECRITURE: Duration = Duration::from_millis(500);
/// Keys closer together than this are one step of undo.
const PAS: Duration = Duration::from_millis(700);
/// How often the space is looked at for changes made elsewhere.
const VEILLE: Duration = Duration::from_secs(2);
/// How many steps of undo a note keeps.
const ANNULATIONS: usize = 200;

/// The text and the cursor at a step of undo.
#[derive(Debug, Clone)]
struct Instantane {
    text: String,
    block: usize,
    cursor: usize,
}

/// The open note.
#[derive(Debug)]
struct Ouverte {
    rel: String,
    text: String,
    blocks: Vec<Block>,
    /// The file's time when it was read or written last.
    modified: Option<i64>,
    /// Changed here and not written yet.
    dirty: bool,
    undo: Vec<Instantane>,
    redo: Vec<Instantane>,
    dernier_pas: Option<Instant>,
}

/// Everything the place holds.
struct Etat {
    vault: Vault,
    spaces: Vec<Space>,
    space: usize,
    expanded: HashSet<String>,
    selected: Option<String>,
    filter: String,
    note: Option<Ouverte>,
    focus: i32,
    serial: i32,
    title_serial: i32,
    fingerprint: u64,
    model: Rc<VecModel<NoteBlockData>>,
    signatures: Vec<u64>,
    palette: Palette,
    tree_keys: Vec<(String, EntryKind)>,
    /// Where the cursor is: its block, its offset, the selection's anchor, and its
    /// place in the window.
    caret: Curseur,
    /// The completion list open over the cursor.
    completion: Option<Completion>,
    /// The side panel and reading mode.
    side: bool,
    reading: bool,
    /// The space's tags, read once for the `#` popup.
    tags: Option<Vec<String>>,
}

/// Where the cursor is.
#[derive(Debug, Default, Clone, Copy)]
struct Curseur {
    block: usize,
    cursor: usize,
    anchor: usize,
    x: f32,
    y: f32,
}

/// A completion list: what is being typed, what is offered, what is chosen.
#[derive(Debug, Clone)]
struct Completion {
    block: usize,
    trigger: iris_notes::complete::Trigger,
    candidates: Vec<iris_notes::complete::Candidate>,
    highlight: usize,
}

/// A note's blocks: never none, so an empty note has a line to type on.
fn blocs_de(text: &str) -> Vec<Block> {
    let b = block::parse(text);
    if b.is_empty() {
        vec![Block {
            kind: BlockKind::Blank,
            text: String::new(),
        }]
    } else {
        b
    }
}

/// The folder a path is in ("" for the top).
fn parent_of(rel: &str) -> String {
    rel.rsplit_once('/')
        .map(|(p, _)| p.to_string())
        .unwrap_or_default()
}

/// A note's name without its folder and `.md`.
fn stem(rel: &str) -> String {
    let nom = rel.rsplit('/').next().unwrap_or(rel);
    nom.rsplit_once('.')
        .map_or(nom.to_string(), |(b, _)| b.to_string())
}

/// Where the spaces are: the setting, else `<IRIS_ROOT>/notes`, else `%USERPROFILE%\Iris`.
fn racine(services: &Services) -> PathBuf {
    let reglage = crate::settings::current().notes_root;
    if !reglage.trim().is_empty() {
        return PathBuf::from(reglage);
    }
    if std::env::var_os("IRIS_ROOT").is_some() {
        if let Some(r) = services.paths.data.parent() {
            return r.join("notes");
        }
    }
    directories::UserDirs::new()
        .map(|u| u.home_dir().join("Iris"))
        .unwrap_or_else(|| services.paths.data.join("notes"))
}

fn signature(b: &Block, numero: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", b.kind).hash(&mut h);
    b.text.hash(&mut h);
    numero.hash(&mut h);
    h.finish()
}

impl Etat {
    fn espace(&self) -> Option<&Space> {
        self.spaces.get(self.space)
    }

    fn espace_mut(&mut self) -> Option<&mut Space> {
        self.spaces.get_mut(self.space)
    }

    /// Where a new note or folder goes: the folder selected, or the folder of the note
    /// selected, or the top.
    fn dossier_courant(&self) -> String {
        match &self.selected {
            Some(k) => match self.tree_keys.iter().find(|(r, _)| r == k) {
                Some((_, EntryKind::Folder)) => k.clone(),
                _ => parent_of(k),
            },
            None => self
                .note
                .as_ref()
                .map(|n| parent_of(&n.rel))
                .unwrap_or_default(),
        }
    }
}

// --- What the window shows -------------------------------------------------------------

fn montrer_espaces(f: &AppWindow, e: &Etat) {
    let donnees: Vec<NoteSpaceData> = e
        .spaces
        .iter()
        .map(|s| NoteSpaceData {
            name: s.config.name.as_str().into(),
            color: crate::calendar::couleur(&s.config.color),
        })
        .collect();
    f.set_notes_spaces(ModelRc::new(VecModel::from(donnees)));
    f.set_notes_space(e.space as i32);
}

fn montrer_arbre(f: &AppWindow, e: &mut Etat) {
    let Some(espace) = e.espace() else {
        f.set_notes_tree(ModelRc::default());
        return;
    };
    let config = espace.config.clone();
    let entrees: Vec<iris_vault::Entry> = if e.filter.trim().is_empty() {
        espace.tree(&e.expanded).unwrap_or_default()
    } else {
        let notes = espace.notes();
        let noms: Vec<String> = notes.iter().map(|n| stem(n)).collect();
        let refs: Vec<&str> = noms.iter().map(String::as_str).collect();
        iris_notes::fuzzy::rank(&e.filter, &refs, 200)
            .into_iter()
            .map(|i| iris_vault::Entry {
                rel: notes[i].clone(),
                name: noms[i].clone(),
                kind: EntryKind::Note,
                depth: 0,
                modified: 0,
                has_children: false,
            })
            .collect()
    };
    e.tree_keys = entrees.iter().map(|x| (x.rel.clone(), x.kind)).collect();
    let ouverte = e.note.as_ref().map(|n| n.rel.clone());
    let lignes: Vec<NoteTreeRowData> = entrees
        .iter()
        .map(|x| NoteTreeRowData {
            key: x.rel.as_str().into(),
            name: x.name.as_str().into(),
            depth: x.depth as i32,
            kind: match x.kind {
                EntryKind::Folder => 0,
                EntryKind::Note => 1,
                EntryKind::Sheet => 2,
                EntryKind::Image => 3,
                EntryKind::Other => 4,
            },
            expanded: e.expanded.contains(&x.rel),
            has_children: x.has_children,
            selected: e.selected.as_deref() == Some(x.rel.as_str())
                || (e.selected.is_none() && ouverte.as_deref() == Some(x.rel.as_str())),
            color: config
                .folder_colors
                .get(&x.rel)
                .map(|c| crate::calendar::couleur(c))
                .unwrap_or(slint::Color::from_argb_u8(0, 0, 0, 0)),
            pinned: config.pinned.contains(&x.rel),
        })
        .collect();
    f.set_notes_tree(ModelRc::new(VecModel::from(lignes)));
}

/// The note's blocks to the window: all of them when their number changed or `tout`,
/// else only those whose rendering changed.
fn rendre(f: &AppWindow, e: &mut Etat, tout: bool) {
    let dir = e.espace().map(|s| s.dir().to_path_buf());
    let dir = dir.as_deref();
    let Some(note) = &e.note else {
        e.model.set_vec(Vec::new());
        e.signatures.clear();
        return;
    };
    let numeros = render::numbering(&note.blocks);
    let sigs: Vec<u64> = note
        .blocks
        .iter()
        .zip(&numeros)
        .map(|(b, n)| signature(b, n))
        .collect();
    if tout || sigs.len() != e.model.row_count() {
        let lignes: Vec<NoteBlockData> = note
            .blocks
            .iter()
            .zip(&numeros)
            .map(|(b, n)| render::render(b, n, &e.palette, dir))
            .collect();
        e.model.set_vec(lignes);
    } else {
        for (i, (b, n)) in note.blocks.iter().zip(&numeros).enumerate() {
            if e.signatures.get(i) != Some(&sigs[i]) {
                e.model
                    .set_row_data(i, render::render(b, n, &e.palette, dir));
            }
        }
    }
    e.signatures = sigs;
    f.set_note_status(if note.dirty { "Editing" } else { "" }.into());
}

/// Puts the cursor in block `block` at `cursor` (`anchor` for a selection), the
/// block's source given to its field again.
fn focaliser(f: &AppWindow, e: &mut Etat, block: usize, anchor: usize, cursor: usize) {
    e.focus = block as i32;
    e.serial += 1;
    f.set_note_focus_cursor(cursor as i32);
    f.set_note_focus_anchor(anchor as i32);
    f.set_note_focus(e.focus);
    f.set_note_focus_serial(e.serial);
}

fn sans_focus(f: &AppWindow, e: &mut Etat) {
    e.focus = -1;
    f.set_note_focus(-1);
}

fn montrer_note(f: &AppWindow, e: &mut Etat) {
    match &e.note {
        Some(n) => {
            f.set_note_open(true);
            f.set_note_title(stem(&n.rel).into());
            let dossier = parent_of(&n.rel);
            let espace = e
                .espace()
                .map(|s| s.config.name.clone())
                .unwrap_or_default();
            f.set_note_path(
                if dossier.is_empty() {
                    espace
                } else {
                    format!("{espace} › {}", dossier.replace('/', " › "))
                }
                .into(),
            );
        }
        None => {
            f.set_note_open(false);
            f.set_note_title(SharedString::default());
            f.set_note_path(SharedString::default());
        }
    }
    rendre(f, e, true);
}

// --- Files ----------------------------------------------------------------------------

/// Writes the open note if it changed.
fn ecrire(f: &AppWindow, e: &mut Etat) {
    let Some(espace) = e.spaces.get(e.space).cloned() else {
        return;
    };
    let Some(note) = &mut e.note else { return };
    if !note.dirty {
        return;
    }
    match espace.write(&note.rel, &note.text, note.modified) {
        Ok(WriteOutcome::Written(m)) => {
            note.modified = Some(m);
            note.dirty = false;
            f.set_note_status("Saved".into());
        }
        Ok(WriteOutcome::Conflict { saved_as, modified }) => {
            note.modified = Some(modified);
            note.dirty = false;
            f.set_toast(
                format!(
                    "This note was changed elsewhere too: that version is kept as “{}”.",
                    stem(&saved_as)
                )
                .into(),
            );
            e.fingerprint = 0;
        }
        Err(err) => f.set_status(format!("Could not save the note: {err}").into()),
    }
}

/// Opens a note of the space, writing the one open before.
fn ouvrir(f: &AppWindow, e: &mut Etat, rel: &str) {
    ecrire(f, e);
    let Some(espace) = e.espace() else { return };
    match espace.read(rel) {
        Ok((text, modified)) => {
            let dir = espace.dir().display().to_string();
            e.note = Some(Ouverte {
                rel: rel.to_string(),
                blocks: blocs_de(&text),
                text,
                modified: Some(modified),
                dirty: false,
                undo: Vec::new(),
                redo: Vec::new(),
                dernier_pas: None,
            });
            e.selected = Some(rel.to_string());
            // Its folders unfolded, so it shows in the tree.
            let mut p = parent_of(rel);
            while !p.is_empty() {
                e.expanded.insert(p.clone());
                p = parent_of(&p);
            }
            sans_focus(f, e);
            montrer_note(f, e);
            crate::settings::update(|s| s.notes_last = format!("{dir}|{rel}"));
            crate::nav::note(f, "note", rel);
            fermer_popups(f, e);
            maj_cote(f, e);
        }
        Err(err) => f.set_status(format!("Could not open the note: {err}").into()),
    }
}

/// A new note in the current folder, opened with its title to type.
fn nouvelle_note(f: &AppWindow, e: &mut Etat, nom: &str, texte: &str) {
    let dossier = e.dossier_courant();
    let Some(espace) = e.espace() else { return };
    match espace.create_note(&dossier, nom, texte) {
        Ok(rel) => {
            if !dossier.is_empty() {
                e.expanded.insert(dossier);
            }
            ouvrir(f, e, &rel);
            montrer_arbre(f, e);
            e.title_serial += 1;
            f.set_note_title_serial(e.title_serial);
        }
        Err(err) => f.set_status(format!("Could not make the note: {err}").into()),
    }
}

/// Reads the spaces again, keeping the one shown when it is still there.
fn charger_espaces(e: &mut Etat) {
    let externes: Vec<PathBuf> = crate::settings::current()
        .notes_spaces
        .iter()
        .map(PathBuf::from)
        .collect();
    let avant = e.espace().map(|s| s.dir().to_path_buf());
    match e.vault.spaces(&externes) {
        Ok(s) => e.spaces = s,
        Err(err) => tracing::warn!(error = %err, "notes: spaces"),
    }
    e.space = avant
        .and_then(|d| e.spaces.iter().position(|s| s.dir() == d.as_path()))
        .unwrap_or(0);
}

// --- Editing --------------------------------------------------------------------------

/// Keeps a step of undo before a change, unless the last was a moment ago (typing).
fn retenir(e: &mut Etat, force: bool) {
    let focus = e.focus.max(0) as usize;
    let Some(note) = &mut e.note else { return };
    let maintenant = Instant::now();
    if !force && note.dernier_pas.is_some_and(|t| maintenant - t < PAS) {
        note.dernier_pas = Some(maintenant);
        return;
    }
    note.dernier_pas = Some(maintenant);
    note.undo.push(Instantane {
        text: note.text.clone(),
        block: focus,
        cursor: 0,
    });
    if note.undo.len() > ANNULATIONS {
        note.undo.remove(0);
    }
    note.redo.clear();
}

/// The note's text replaced, its blocks parsed again, the window told.
fn appliquer(f: &AppWindow, e: &mut Etat, edit: NoteEdit, pousser: bool, anchor: Option<usize>) {
    let Some(note) = &mut e.note else { return };
    let avant = note.blocks.len();
    note.text = edit.text;
    note.blocks = blocs_de(&note.text);
    note.dirty = true;
    let structure = note.blocks.len() != avant;
    let block = edit.block.min(note.blocks.len().saturating_sub(1));
    if pousser || structure || block as i32 != e.focus {
        focaliser(f, e, block, anchor.unwrap_or(edit.cursor), edit.cursor);
    }
    rendre(f, e, false);
    planifier_ecriture(f);
}

thread_local! {
    /// The write waiting for typing to stop.
    static MINUTERIE: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
    /// The place's state, for the timers.
    static ETAT: RefCell<Option<Rc<RefCell<Etat>>>> = const { RefCell::new(None) };
}

fn planifier_ecriture(f: &AppWindow) {
    let faible = f.as_weak();
    MINUTERIE.with(|m| {
        let mut m = m.borrow_mut();
        let minuterie = m.get_or_insert_with(slint::Timer::default);
        minuterie.start(slint::TimerMode::SingleShot, ECRITURE, move || {
            let Some(f) = faible.upgrade() else { return };
            ETAT.with(|e| {
                if let Some(etat) = e.borrow().as_ref() {
                    ecrire(&f, &mut etat.borrow_mut());
                }
            });
        });
    });
}

/// The content of block `i`.
fn contenu(e: &Etat, i: usize) -> Option<(BlockKind, String)> {
    let b = e.note.as_ref()?.blocks.get(i)?;
    Some((b.kind.clone(), b.content().to_string()))
}

/// Block `i`'s source replaced, the cursor at `cursor` (`anchor` for a selection).
fn remplacer(f: &AppWindow, e: &mut Etat, i: usize, nouveau: &str, anchor: usize, cursor: usize) {
    let Some(note) = &e.note else { return };
    let ed = edit::replace_block(&note.blocks, i, nouveau, cursor);
    let local_anchor = anchor.min(nouveau.len());
    retenir(e, true);
    appliquer(f, e, ed, true, Some(local_anchor));
}

/// The colour a shortcut names: Ctrl+Shift+1…7, or the AZERTY keys under the digits.
fn couleur_de_touche(t: &str) -> Option<Colour> {
    let i = match t {
        "1" | "&" => 0,
        "2" | "é" => 1,
        "3" | "\"" => 2,
        "4" | "'" => 3,
        "5" | "(" => 4,
        "6" | "-" => 5,
        "7" | "è" => 6,
        _ => return None,
    };
    Colour::ALL.get(i).copied()
}

/// A key in block `i`. True when it was used here.
fn touche(f: &AppWindow, e: &mut Etat, i: usize, nom: &str, cursor: usize, anchor: usize) -> bool {
    let Some((kind, texte)) = contenu(e, i) else {
        return false;
    };
    let n = e.note.as_ref().map_or(0, |n| n.blocks.len());
    let marque = |m: &str| Some(m.to_string());
    let a_marquer = match nom.to_ascii_lowercase().as_str() {
        "ctrl+b" => marque("bold"),
        "ctrl+i" => marque("italic"),
        "ctrl+u" => marque("underline"),
        "ctrl+e" => marque("code"),
        "ctrl+m" => marque("math"),
        "ctrl+shift+x" => marque("strike"),
        "ctrl+shift+h" => marque("highlight"),
        "ctrl+." => marque("sup"),
        "ctrl+," => marque("sub"),
        "ctrl+l" => marque("link"),
        _ => None,
    };
    if let Some(m) = a_marquer {
        let r = edit::toggle_mark(&texte, anchor, cursor, &m);
        remplacer(f, e, i, &r.text, r.anchor, r.cursor);
        return true;
    }
    if let Some(reste) = nom.strip_prefix("ctrl+shift+") {
        if let Some(c) = couleur_de_touche(reste) {
            let r = edit::set_colour(&texte, anchor, cursor, Some(c));
            remplacer(f, e, i, &r.text, r.anchor, r.cursor);
            return true;
        }
        if reste.eq_ignore_ascii_case("0") || reste == "à" {
            let r = edit::set_colour(&texte, anchor, cursor, None);
            remplacer(f, e, i, &r.text, r.anchor, r.cursor);
            return true;
        }
    }
    match nom {
        "escape" => {
            sans_focus(f, e);
            rendre(f, e, false);
            true
        }
        "enter" | "shift+enter" => {
            let Some(note) = &e.note else { return false };
            let resultat = if nom == "shift+enter" && !kind.is_multiline() {
                Enter::Split {
                    stays: texte[..cursor.min(texte.len())].to_string(),
                    goes: texte[cursor.min(texte.len())..].to_string(),
                    cursor: 0,
                }
            } else {
                edit::on_enter(&kind, &texte, cursor)
            };
            let ed = match resultat {
                Enter::Split {
                    stays,
                    goes,
                    cursor: c,
                } => {
                    // An empty item ends its list: the line stays, empty.
                    if stays.is_empty() && goes.is_empty() && !texte.is_empty() {
                        edit::replace_block(&note.blocks, i, "", 0)
                    } else {
                        edit::split_block(&note.blocks, i, &stays, &goes, c)
                    }
                }
                Enter::Insert(r) => edit::replace_block(&note.blocks, i, &r.text, r.cursor),
            };
            retenir(e, true);
            appliquer(f, e, ed, true, None);
            true
        }
        "ctrl+enter" => {
            let t = edit::toggle_task(&texte);
            let c = t.len();
            remplacer(f, e, i, &t, c, c);
            true
        }
        "backspace-start" => {
            let Some(note) = &e.note else { return false };
            match edit::join_with_previous(&note.blocks, i) {
                Some(ed) => {
                    retenir(e, true);
                    appliquer(f, e, ed, true, None);
                    true
                }
                None => false,
            }
        }
        "delete" => {
            if cursor < texte.len() || i + 1 >= n {
                return false;
            }
            let Some(note) = &e.note else { return false };
            match edit::join_with_previous(&note.blocks, i + 1) {
                Some(ed) => {
                    retenir(e, true);
                    appliquer(f, e, ed, true, None);
                    true
                }
                None => false,
            }
        }
        "up" | "left-start" => {
            if i == 0 {
                return false;
            }
            let fin = contenu(e, i - 1).map_or(0, |(_, t)| t.len());
            focaliser(f, e, i - 1, fin, fin);
            rendre(f, e, false);
            true
        }
        "down" => {
            if i + 1 >= n {
                return false;
            }
            focaliser(f, e, i + 1, 0, 0);
            rendre(f, e, false);
            true
        }
        "right" => {
            if cursor < texte.len() || i + 1 >= n {
                return false;
            }
            focaliser(f, e, i + 1, 0, 0);
            rendre(f, e, false);
            true
        }
        "tab" | "shift+tab" => {
            let dehors = nom == "shift+tab";
            // In maths: a snippet typed just before becomes its LaTeX, else the cursor
            // goes to the next place to fill.
            if !dehors && dans_les_maths(&kind, &texte, cursor) {
                let c = cursor.min(texte.len());
                if let Some((longueur, insert, dedans)) =
                    iris_notes::math::snippet_before(&texte[..c])
                {
                    let debut = c - longueur;
                    let t = format!("{}{insert}{}", &texte[..debut], &texte[c..]);
                    remplacer(f, e, i, &t, debut + dedans, debut + dedans);
                    return true;
                }
                if let Some(suivant) = iris_notes::math::next_slot(&texte, c) {
                    focaliser(f, e, i, suivant, suivant);
                    return true;
                }
            }
            if matches!(kind, BlockKind::Code { .. }) && !dehors {
                let c = cursor.min(texte.len());
                let t = format!("{}    {}", &texte[..c], &texte[c..]);
                remplacer(f, e, i, &t, c + 4, c + 4);
                return true;
            }
            let t = edit::indent(&texte, dehors);
            if t != texte {
                let delta = t.len() as isize - texte.len() as isize;
                let c = (cursor as isize + delta).max(0) as usize;
                remplacer(f, e, i, &t, c, c);
            }
            true
        }
        "alt+up" | "alt+down" => {
            let Some(note) = &e.note else { return false };
            match edit::move_block(&note.blocks, i, nom == "alt+up") {
                Some(mut ed) => {
                    ed.cursor = cursor.min(texte.len());
                    retenir(e, true);
                    appliquer(f, e, ed, true, None);
                    true
                }
                None => true,
            }
        }
        "ctrl+d" => {
            let Some(note) = &e.note else { return false };
            let mut ed = edit::duplicate_block(&note.blocks, i);
            ed.cursor = cursor.min(texte.len());
            retenir(e, true);
            appliquer(f, e, ed, true, None);
            true
        }
        "ctrl+h" => {
            let t = edit::cycle_heading(&texte);
            let c = t.len();
            remplacer(f, e, i, &t, c, c);
            true
        }
        "ctrl+z" => {
            annuler(f, e, false);
            true
        }
        "ctrl+y" | "ctrl+shift+z" | "ctrl+shift+Z" => {
            annuler(f, e, true);
            true
        }
        "ctrl+s" => {
            ecrire(f, e);
            true
        }
        "ctrl+n" => {
            nouvelle_note(f, e, "Untitled", "");
            true
        }
        "ctrl+r" | "ctrl+R" => {
            lecture(f, e);
            true
        }
        "ctrl+shift+f" | "ctrl+shift+F" => {
            f.set_notes_quick_search(true);
            f.set_notes_quick_query(SharedString::default());
            f.set_notes_quick_highlight(0);
            f.set_notes_quick_open(true);
            f.invoke_notes_quick_edited(SharedString::default());
            true
        }
        "ctrl+shift+c" | "ctrl+shift+C" => {
            e.completion = None;
            f.set_note_completion(ModelRc::default());
            f.set_note_bubble(false);
            f.set_note_card("colour".into());
            f.set_note_card_title("Colour".into());
            f.set_note_card_text(SharedString::default());
            true
        }
        "ctrl+v" | "ctrl+V" => coller_image(f, e, i, cursor),
        "popup-up" | "popup-down" => {
            if let Some(c) = &mut e.completion {
                let n = c.candidates.len().max(1);
                c.highlight = if nom == "popup-up" {
                    (c.highlight + n - 1) % n
                } else {
                    (c.highlight + 1) % n
                };
                f.set_note_completion_highlight(c.highlight as i32);
            }
            true
        }
        "popup-accept" => {
            let h = e.completion.as_ref().map(|c| c.highlight);
            match h {
                Some(h) => choisir(f, e, h),
                None => false,
            }
        }
        "popup-close" => {
            fermer_popups(f, e);
            true
        }
        " " => {
            let c = cursor.min(texte.len());
            let avec = format!("{} {}", &texte[..c], &texte[c..]);
            match edit::line_start_conversion(&avec, c + 1) {
                Some(r) => {
                    remplacer(f, e, i, &r.text, r.cursor, r.cursor);
                    true
                }
                None => false,
            }
        }
        _ => {
            let mut car = nom.chars();
            if let (Some(c), None) = (car.next(), car.next()) {
                if let Some(r) = edit::auto_pair(&texte, anchor, cursor, c) {
                    remplacer(f, e, i, &r.text, r.anchor, r.cursor);
                    return true;
                }
            }
            false
        }
    }
}

// --- What pops up over the cursor -----------------------------------------------------

/// Whether the cursor is in maths: a `$$` block, or `$…$` in a line.
fn dans_les_maths(kind: &BlockKind, texte: &str, cursor: usize) -> bool {
    if *kind == BlockKind::Math {
        return true;
    }
    let c = cursor.min(texte.len());
    let noeuds = iris_notes::inline::parse_inline(texte);
    iris_notes::inline::marks_at(&noeuds, c.saturating_sub(1))
        .iter()
        .any(|(m, r)| *m == iris_notes::inline::Mark::Math && r.start < c && c < r.end)
}

fn fermer_popups(f: &AppWindow, e: &mut Etat) {
    e.completion = None;
    f.set_note_completion(ModelRc::default());
    f.set_note_bubble(false);
    f.set_note_card(SharedString::default());
}

/// Reading mode on or off: every block drawn, links followed with one click.
fn lecture(f: &AppWindow, e: &mut Etat) {
    ecrire(f, e);
    e.reading = !e.reading;
    f.set_note_reading(e.reading);
    fermer_popups(f, e);
    sans_focus(f, e);
    rendre(f, e, false);
}

/// The candidates for what is being typed.
fn candidats(
    e: &mut Etat,
    trigger: &iris_notes::complete::Trigger,
) -> Vec<iris_notes::complete::Candidate> {
    use iris_notes::complete::{self as c, Trigger};
    let Some(espace) = e.spaces.get(e.space) else {
        return Vec::new();
    };
    match trigger {
        Trigger::Note { query, .. } => {
            let notes = espace.notes();
            match query.split_once('#') {
                // `[[Note#…`: its headings.
                Some((note, titre)) => {
                    let cible = if note.is_empty() {
                        e.note.as_ref().map(|n| n.rel.clone())
                    } else {
                        iris_notes::links::resolve(note, &notes)
                    };
                    let Some(cible) = cible else {
                        return Vec::new();
                    };
                    let texte = espace.read(&cible).map(|t| t.0).unwrap_or_default();
                    let plie = iris_notes::fuzzy::folded(titre);
                    iris_notes::links::outline(&texte)
                        .into_iter()
                        .filter(|(_, h, _)| iris_notes::fuzzy::folded(h).contains(&plie))
                        .take(10)
                        .map(|(_, h, _)| {
                            let insert = format!("[[{note}#{h}]]");
                            c::Candidate {
                                label: h.clone(),
                                detail: String::new(),
                                cursor: insert.len(),
                                insert,
                            }
                        })
                        .collect()
                }
                None => c::note_candidates(query, &notes, 8),
            }
        }
        Trigger::Tag { query, .. } => {
            if e.tags.is_none() {
                e.tags = Some(espace.all_tags());
            }
            c::tag_candidates(query, e.tags.as_deref().unwrap_or_default(), 8)
        }
        Trigger::Date { query, .. } => c::date_candidates(query, chrono::Local::now().date_naive())
            .into_iter()
            .take(6)
            .collect(),
        Trigger::Command { query, .. } => c::command_candidates(query, 8),
        Trigger::Colour { query, .. } => c::colour_candidates(query),
        Trigger::Block { query, .. } => {
            let mut liste = c::block_candidates(query);
            // The space's templates, filled in for this note.
            let q = iris_notes::fuzzy::folded(query);
            let titre = e.note.as_ref().map(|n| stem(&n.rel)).unwrap_or_default();
            let cours = e
                .note
                .as_ref()
                .map(|n| stem(&parent_of(&n.rel)))
                .unwrap_or_default();
            for (nom, rel) in espace.templates() {
                if !(q.is_empty()
                    || "template".starts_with(&q)
                    || iris_notes::fuzzy::folded(&nom).starts_with(&q))
                {
                    continue;
                }
                let Ok((modele, _)) = espace.read(&rel) else {
                    continue;
                };
                let maintenant = chrono::Local::now();
                let (texte, curseur) = iris_notes::template::expand(
                    &modele,
                    &iris_notes::template::Values {
                        title: titre.clone(),
                        date: maintenant.format("%Y-%m-%d").to_string(),
                        time: maintenant.format("%H:%M").to_string(),
                        course: cours.clone(),
                    },
                );
                liste.push(c::Candidate {
                    label: format!("Template: {nom}"),
                    detail: String::new(),
                    insert: texte.trim_end_matches('\n').to_string(),
                    cursor: curseur,
                });
            }
            liste
        }
    }
}

/// What pops up after the text or the cursor moved: the completion list for what is
/// being typed, the bubble over a selection, a card about what the cursor is in.
fn apres_curseur(f: &AppWindow, e: &mut Etat) {
    let Curseur {
        block,
        cursor,
        anchor,
        x,
        y,
    } = e.caret;
    f.set_note_popup_x(x);
    f.set_note_popup_y(y);
    let texte = match contenu(e, block) {
        Some((_, t)) if e.focus == block as i32 && !e.reading => t,
        _ => {
            fermer_popups(f, e);
            return;
        }
    };
    let selection = anchor != cursor;
    f.set_note_bubble(selection);

    let declencheur = if selection {
        None
    } else {
        iris_notes::complete::trigger_at(&texte, cursor)
    };
    match declencheur {
        Some(t) => {
            let liste = candidats(e, &t);
            if liste.is_empty() {
                e.completion = None;
                f.set_note_completion(ModelRc::default());
            } else {
                let donnees: Vec<NoteFoundData> = liste
                    .iter()
                    .enumerate()
                    .map(|(k, c)| NoteFoundData {
                        key: k.to_string().into(),
                        title: c.label.as_str().into(),
                        detail: c.detail.as_str().into(),
                    })
                    .collect();
                f.set_note_completion(ModelRc::new(VecModel::from(donnees)));
                f.set_note_completion_highlight(0);
                e.completion = Some(Completion {
                    block,
                    trigger: t,
                    candidates: liste,
                    highlight: 0,
                });
            }
        }
        None => {
            e.completion = None;
            f.set_note_completion(ModelRc::default());
        }
    }

    carte(f, e, &texte, cursor, selection);
}

/// The card about what the cursor is in: a colour, a link, a footnote, maths.
fn carte(f: &AppWindow, e: &mut Etat, texte: &str, cursor: usize, selection: bool) {
    use iris_notes::inline::{self, Mark};
    let vide = |f: &AppWindow| {
        f.set_note_card(SharedString::default());
        f.set_note_card_title(SharedString::default());
        f.set_note_card_text(SharedString::default());
        f.set_note_card_picture(slint::Image::default());
    };
    if selection {
        // The colour card asked from the bubble stays while the words are selected.
        if f.get_note_card() != "colour" {
            vide(f);
        }
        return;
    }
    let noeuds = inline::parse_inline(texte);
    let marques = inline::marks_at(&noeuds, cursor);
    let Some((marque, plage)) = marques.last().cloned() else {
        vide(f);
        return;
    };
    // The cursor at the very edge of a mark is not in it.
    if cursor <= plage.start || cursor >= plage.end {
        vide(f);
        return;
    }
    match marque {
        Mark::Colour(c) => {
            f.set_note_card("colour".into());
            f.set_note_card_title(format!("Colour: {}", c.name()).into());
            f.set_note_card_text(SharedString::default());
        }
        Mark::NoteLink { target, .. } => {
            let nom = target.split('#').next().unwrap_or(&target).to_string();
            let apercu = e.spaces.get(e.space).and_then(|s| {
                let notes = s.notes();
                let rel = iris_notes::links::resolve(&nom, &notes)?;
                let texte = s.read(&rel).ok()?.0;
                Some(
                    texte
                        .lines()
                        .filter(|l| !l.trim().is_empty() && !l.starts_with("---"))
                        .take(5)
                        .map(|l| inline::plain_text(&inline::parse_inline(l)))
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            });
            f.set_note_card("link".into());
            f.set_note_card_title(nom.as_str().into());
            f.set_note_card_text(
                apercu
                    .unwrap_or_else(|| "Not written yet: Open makes it.".into())
                    .into(),
            );
            f.set_note_card_picture(slint::Image::default());
        }
        Mark::Footnote => {
            let n = marques
                .last()
                .map(|(_, r)| texte[r.start + 2..r.end - 1].to_string())
                .unwrap_or_default();
            f.set_note_card("footnote".into());
            f.set_note_card_title("Footnote".into());
            f.set_note_card_text(n.into());
        }
        Mark::Math => {
            let source = &texte[plage.start + 1..plage.end - 1];
            f.set_note_card("math".into());
            f.set_note_card_title(SharedString::default());
            f.set_note_card_text(iris_notes::math::to_unicode(source).into());
        }
        _ => vide(f),
    }
}

/// The candidate `k` of the completion list written in place of what was typed.
fn choisir(f: &AppWindow, e: &mut Etat, k: usize) -> bool {
    let Some(c) = e.completion.take() else {
        return false;
    };
    let Some(candidat) = c.candidates.get(k) else {
        return false;
    };
    let Some((_, texte)) = contenu(e, c.block) else {
        return false;
    };
    let curseur = e.caret.cursor.min(texte.len());
    let r = iris_notes::complete::apply(&texte, curseur, &c.trigger, candidat);
    f.set_note_completion(ModelRc::default());
    remplacer(f, e, c.block, &r.text, r.cursor, r.cursor);
    true
}

/// An image on the clipboard, pasted into the note: written to the space's
/// attachments, embedded on a line of its own. False when the clipboard holds none
/// (the field pastes its text).
fn coller_image(f: &AppWindow, e: &mut Etat, i: usize, cursor: usize) -> bool {
    let Ok(mut presse) = arboard::Clipboard::new() else {
        return false;
    };
    let Ok(img) = presse.get_image() else {
        return false;
    };
    let Some(tampon) =
        image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
    else {
        return false;
    };
    let mut png = Vec::new();
    if image::DynamicImage::ImageRgba8(tampon)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .is_err()
    {
        return false;
    }
    let nom = format!("Image {}", chrono::Local::now().format("%Y-%m-%d %H.%M.%S"));
    let Some(espace) = e.espace() else {
        return false;
    };
    match espace.save_attachment(&nom, "png", &png) {
        Ok(rel) => {
            let fichier = rel.rsplit('/').next().unwrap_or(&rel).to_string();
            let Some((_, texte)) = contenu(e, i) else {
                return false;
            };
            let c = cursor.min(texte.len());
            let ligne = format!("![[{fichier}]]");
            // On its own line: before what follows the cursor, after what precedes it.
            let avant = texte[..c].trim_end();
            let apres = texte[c..].trim_start();
            let mut t = String::new();
            if !avant.is_empty() {
                t.push_str(avant);
                t.push('\n');
            }
            t.push_str(&ligne);
            let curseur = t.len();
            if !apres.is_empty() {
                t.push('\n');
                t.push_str(apres);
            }
            remplacer(f, e, i, &t, curseur, curseur);
            e.fingerprint = 0;
            true
        }
        Err(err) => {
            f.set_status(format!("Could not keep the image: {err}").into());
            true
        }
    }
}

/// A mark, a colour or a link from the bubble or the colour card, on the selection
/// (or the word at the cursor for a colour).
fn formater(f: &AppWindow, e: &mut Etat, action: &str) {
    let Curseur {
        block,
        cursor,
        anchor,
        ..
    } = e.caret;
    let Some((_, texte)) = contenu(e, block) else {
        return;
    };
    if action == "colour" {
        f.set_note_bubble(false);
        f.set_note_card("colour".into());
        f.set_note_card_title("Colour".into());
        f.set_note_card_text(SharedString::default());
        return;
    }
    let r = match action.strip_prefix("colour:") {
        Some(reste) => {
            let teinte = reste
                .parse::<usize>()
                .ok()
                .and_then(|i| Colour::ALL.get(i).copied());
            f.set_note_card(SharedString::default());
            edit::set_colour(&texte, anchor, cursor, teinte)
        }
        None => edit::toggle_mark(&texte, anchor, cursor, action),
    };
    remplacer(f, e, block, &r.text, r.anchor, r.cursor);
}

/// The side panel: the outline, what mentions the note, its tags and length.
fn maj_cote(f: &AppWindow, e: &Etat) {
    if !e.side {
        return;
    }
    let Some(note) = &e.note else {
        f.set_note_outline(ModelRc::default());
        f.set_note_backlinks(ModelRc::default());
        f.set_note_facts(SharedString::default());
        return;
    };
    let plan: Vec<NoteFoundData> = iris_notes::links::outline(&note.text)
        .into_iter()
        .map(|(niveau, titre, bloc)| NoteFoundData {
            key: bloc.to_string().into(),
            title: titre.into(),
            detail: niveau.to_string().into(),
        })
        .collect();
    f.set_note_outline(ModelRc::new(VecModel::from(plan)));
    let retours: Vec<NoteFoundData> = e
        .espace()
        .map(|s| s.backlinks(&note.rel))
        .unwrap_or_default()
        .into_iter()
        .map(|(rel, ligne)| NoteFoundData {
            title: stem(&rel).into(),
            key: rel.into(),
            detail: ligne.into(),
        })
        .collect();
    f.set_note_backlinks(ModelRc::new(VecModel::from(retours)));
    let mots = iris_notes::meta::word_count(&note.text);
    let etiquettes = iris_notes::meta::tags(&note.text);
    let mut faits = format!("{mots} words");
    if !etiquettes.is_empty() {
        faits.push_str(" · ");
        faits.push_str(
            &etiquettes
                .iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    f.set_note_facts(faits.into());
}

/// Undo (or redo): the note as it was a step ago.
fn annuler(f: &AppWindow, e: &mut Etat, refaire: bool) {
    let focus = e.focus.max(0) as usize;
    let Some(note) = &mut e.note else { return };
    let (depuis, vers) = if refaire {
        (&mut note.redo, &mut note.undo)
    } else {
        (&mut note.undo, &mut note.redo)
    };
    let Some(pas) = depuis.pop() else { return };
    vers.push(Instantane {
        text: note.text.clone(),
        block: focus,
        cursor: 0,
    });
    note.dernier_pas = None;
    let blocs = blocs_de(&pas.text);
    let bloc = pas.block.min(blocs.len().saturating_sub(1));
    let fin = blocs.get(bloc).map_or(0, |b| b.content().len());
    let ed = NoteEdit {
        text: pas.text,
        block: bloc,
        cursor: pas.cursor.max(fin),
    };
    appliquer(f, e, ed, true, None);
}

/// A link clicked in a note.
fn suivre(f: &AppWindow, e: &mut Etat, lien: &str) {
    if let Some(cible) = lien.strip_prefix("iris-note:") {
        let cible = iris_notes::inline::decode_target(cible);
        let nom = cible.split('#').next().unwrap_or(&cible).trim().to_string();
        if nom.is_empty() {
            return;
        }
        let Some(espace) = e.espace() else { return };
        let notes = espace.notes();
        let trouvee = notes
            .iter()
            .find(|n| n.eq_ignore_ascii_case(&format!("{nom}.md")))
            .or_else(|| notes.iter().find(|n| stem(n).eq_ignore_ascii_case(&nom)))
            .cloned();
        match trouvee {
            Some(rel) => ouvrir(f, e, &rel),
            // A link to a note not written yet makes it, as Obsidian does.
            None => {
                let nom_seul = nom.rsplit('/').next().unwrap_or(&nom).to_string();
                nouvelle_note(f, e, &nom_seul, "");
            }
        }
    } else if let Some(jour) = lien.strip_prefix("iris-date:") {
        f.invoke_calendar_day_chosen(jour.into());
        f.set_workspace(1);
        f.invoke_workspace_changed(1);
    } else if let Some(texte) = lien.strip_prefix("iris-footnote:") {
        f.set_toast(iris_notes::inline::decode_target(texte).into());
    } else if let Some(tag) = lien.strip_prefix("iris-tag:") {
        let t = format!("#{}", iris_notes::inline::decode_target(tag));
        f.set_notes_filter(t.as_str().into());
        e.filter = t;
        montrer_arbre(f, e);
    } else if lien.starts_with("https://") {
        if let Err(err) = crate::platform::open_url(lien) {
            f.set_status(format!("Could not open the link: {err}").into());
        }
    } else {
        f.set_status("Only secure web links (https://) are opened.".into());
    }
}

/// The note open moved or renamed: it follows.
fn suivre_deplacement(e: &mut Etat, avant: &str, apres: &str) {
    if let Some(n) = &mut e.note {
        if n.rel == avant {
            n.rel = apres.to_string();
        } else if let Some(reste) = n.rel.strip_prefix(&format!("{avant}/")) {
            n.rel = format!("{apres}/{reste}");
        }
    }
    let deplies: Vec<String> = e.expanded.iter().cloned().collect();
    for d in deplies {
        if d == avant {
            e.expanded.remove(&d);
            e.expanded.insert(apres.to_string());
        } else if let Some(reste) = d.strip_prefix(&format!("{avant}/")) {
            e.expanded.remove(&d);
            e.expanded.insert(format!("{apres}/{reste}"));
        }
    }
    if e.selected.as_deref() == Some(avant) {
        e.selected = Some(apres.to_string());
    }
}

/// Looks at the space for changes made elsewhere: the tree follows, and the open note
/// is read again when it was not being changed here.
fn veiller(f: &AppWindow, e: &mut Etat) {
    let Some(espace) = e.espace() else { return };
    let empreinte = espace.fingerprint();
    if empreinte == e.fingerprint {
        return;
    }
    let relire = e.note.as_ref().and_then(|n| {
        if n.dirty {
            return None;
        }
        match espace.modified(&n.rel) {
            None => Some(None),
            Some(m) if Some(m) != n.modified => Some(Some(n.rel.clone())),
            _ => None,
        }
    });
    e.fingerprint = empreinte;
    e.tags = None;
    match relire {
        Some(None) => {
            e.note = None;
            sans_focus(f, e);
            montrer_note(f, e);
        }
        Some(Some(rel)) => {
            let focus = e.focus;
            if let Some(Ok((text, m))) = e.espace().map(|s| s.read(&rel)) {
                if let Some(n) = &mut e.note {
                    n.blocks = blocs_de(&text);
                    n.text = text;
                    n.modified = Some(m);
                }
                if focus >= 0 {
                    sans_focus(f, e);
                }
                rendre(f, e, true);
            }
        }
        None => {}
    }
    montrer_arbre(f, e);
}

/// The place: its spaces, its tree, the note open, and what every key does.
pub fn wire_notes(f: &AppWindow, services: &Services) {
    let vault = match Vault::open(racine(services)) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(error = %err, "notes: the folder could not be opened");
            return;
        }
    };
    let model = Rc::new(VecModel::<NoteBlockData>::default());
    f.set_note_blocks(ModelRc::from(Rc::clone(&model)));
    let etat = Rc::new(RefCell::new(Etat {
        vault,
        spaces: Vec::new(),
        space: 0,
        expanded: HashSet::new(),
        selected: None,
        filter: String::new(),
        note: None,
        focus: -1,
        serial: 0,
        title_serial: 0,
        fingerprint: 0,
        model,
        signatures: Vec::new(),
        palette: Palette::default(),
        tree_keys: Vec::new(),
        caret: Curseur::default(),
        completion: None,
        side: false,
        reading: false,
        tags: None,
    }));
    ETAT.with(|e| *e.borrow_mut() = Some(Rc::clone(&etat)));

    // The spaces, and the note open last.
    {
        let mut e = etat.borrow_mut();
        charger_espaces(&mut e);
        let derniere = crate::settings::current().notes_last;
        if let Some((dir, rel)) = derniere.split_once('|') {
            if let Some(i) = e
                .spaces
                .iter()
                .position(|s| s.dir() == std::path::Path::new(dir))
            {
                e.space = i;
                if e.spaces[i].modified(rel).is_some() {
                    let rel = rel.to_string();
                    ouvrir(f, &mut e, &rel);
                }
            }
        }
        montrer_espaces(f, &e);
        montrer_arbre(f, &mut e);
    }

    // The place drawn again when it is shown; the space looked at while it is.
    {
        let (etat, faible) = (Rc::clone(&etat), f.as_weak());
        crate::workspace::follow(f, move |w| {
            let Some(f) = faible.upgrade() else { return };
            let mut e = etat.borrow_mut();
            if w == 4 {
                e.fingerprint = 0;
                veiller(&f, &mut e);
            } else {
                ecrire(&f, &mut e);
            }
        });
    }
    {
        let (etat, faible) = (Rc::clone(&etat), f.as_weak());
        let minuterie = slint::Timer::default();
        minuterie.start(slint::TimerMode::Repeated, VEILLE, move || {
            let Some(f) = faible.upgrade() else { return };
            if f.get_workspace() != 4 || !f.window().is_visible() {
                return;
            }
            if let Ok(mut e) = etat.try_borrow_mut() {
                veiller(&f, &mut e);
            }
        });
        std::mem::forget(minuterie);
    }

    macro_rules! geste {
        ($installer:ident, |$f:ident, $e:ident $(, $arg:ident)*| $corps:block) => {{
            let (etat, faible) = (Rc::clone(&etat), f.as_weak());
            f.$installer(move |$($arg),*| {
                let Some($f) = faible.upgrade() else { return };
                let mut guard = etat.borrow_mut();
                let $e: &mut Etat = &mut guard;
                $corps
            });
        }};
    }

    geste!(on_notes_space_chosen, |f, e, i| {
        ecrire(&f, e);
        if (i as usize) < e.spaces.len() {
            e.space = i as usize;
            e.note = None;
            e.expanded.clear();
            e.selected = None;
            e.fingerprint = 0;
            sans_focus(&f, e);
            montrer_note(&f, e);
            montrer_espaces(&f, e);
            montrer_arbre(&f, e);
        }
    });
    geste!(on_notes_space_new, |f, e, nom| {
        match e.vault.create_space(&nom) {
            Ok(s) => {
                let dir = s.dir().to_path_buf();
                charger_espaces(e);
                e.space = e
                    .spaces
                    .iter()
                    .position(|x| x.dir() == dir.as_path())
                    .unwrap_or(0);
                e.note = None;
                e.expanded.clear();
                montrer_note(&f, e);
                montrer_espaces(&f, e);
                montrer_arbre(&f, e);
                f.set_notes_name_asked("".into());
                f.set_notes_name_error("".into());
            }
            Err(err) => f.set_notes_name_error(err.to_string().into()),
        }
    });
    geste!(on_notes_space_open_folder, |f, e| {
        let Some(dossier) = rfd::FileDialog::new()
            .set_title("Open a folder as a space")
            .pick_folder()
        else {
            return;
        };
        match e.vault.add_external(&dossier) {
            Ok(_) => {
                let chemin = dossier.display().to_string();
                crate::settings::update(|s| {
                    if !s.notes_spaces.contains(&chemin) {
                        s.notes_spaces.push(chemin.clone());
                    }
                });
                charger_espaces(e);
                e.space = e
                    .spaces
                    .iter()
                    .position(|x| x.dir() == dossier.as_path())
                    .unwrap_or(e.space);
                e.note = None;
                montrer_note(&f, e);
                montrer_espaces(&f, e);
                montrer_arbre(&f, e);
            }
            Err(err) => f.set_status(format!("Could not open the folder: {err}").into()),
        }
    });
    geste!(on_notes_space_reveal, |f, e| {
        if let Some(s) = e.espace() {
            if let Err(err) = crate::platform::open_path(s.dir()) {
                f.set_status(format!("Could not open the folder: {err}").into());
            }
        }
    });
    geste!(on_notes_row_clicked, |f, e, k| {
        let k = k.to_string();
        let genre = e.tree_keys.iter().find(|(r, _)| *r == k).map(|(_, g)| *g);
        e.selected = Some(k.clone());
        match genre {
            Some(EntryKind::Folder) => {
                if !e.expanded.remove(&k) {
                    e.expanded.insert(k);
                }
            }
            Some(EntryKind::Note) => ouvrir(&f, e, &k),
            Some(EntryKind::Image | EntryKind::Other | EntryKind::Sheet) => {
                if let Some(Ok(p)) = e.espace().map(|s| s.path(&k)) {
                    if let Err(err) = crate::platform::open_path(&p) {
                        f.set_status(format!("Could not open it: {err}").into());
                    }
                }
            }
            None => {}
        }
        montrer_arbre(&f, e);
    });
    geste!(on_notes_row_toggled, |f, e, k| {
        let k = k.to_string();
        if !e.expanded.remove(&k) {
            e.expanded.insert(k);
        }
        montrer_arbre(&f, e);
    });
    geste!(on_notes_row_action, |f, e, action, k| {
        let mut cle = k.to_string();
        if cle.is_empty() {
            cle = e
                .selected
                .clone()
                .or_else(|| e.note.as_ref().map(|n| n.rel.clone()))
                .unwrap_or_default();
        }
        if cle.is_empty() {
            return;
        }
        let genre = e
            .tree_keys
            .iter()
            .find(|(r, _)| *r == cle)
            .map(|(_, g)| *g)
            .unwrap_or(EntryKind::Note);
        match action.as_str() {
            "menu" => {
                f.set_notes_row_menu_kind(match genre {
                    EntryKind::Folder => 0,
                    EntryKind::Note => 1,
                    EntryKind::Sheet => 2,
                    EntryKind::Image => 3,
                    EntryKind::Other => 4,
                });
                let epingle = e.espace().is_some_and(|s| s.config.pinned.contains(&cle));
                f.set_notes_row_menu_pinned(epingle);
                e.selected = Some(cle);
                montrer_arbre(&f, e);
            }
            "rename" => {
                f.set_notes_renaming(cle.as_str().into());
            }
            "new-note" => {
                e.selected = Some(cle);
                nouvelle_note(&f, e, "Untitled", "");
            }
            "duplicate" => {
                if let Some(Err(err)) = e.espace().map(|s| s.duplicate(&cle)) {
                    f.set_status(format!("Could not duplicate it: {err}").into());
                }
                montrer_arbre(&f, e);
            }
            "pin" => {
                if let Some(s) = e.espace_mut() {
                    if let Some(p) = s.config.pinned.iter().position(|x| *x == cle) {
                        s.config.pinned.remove(p);
                    } else {
                        s.config.pinned.push(cle);
                    }
                    let _ = s.save_config();
                }
                montrer_arbre(&f, e);
            }
            "reveal" => {
                if let Some(Ok(p)) = e.espace().map(|s| s.path(&cle)) {
                    if let Err(err) = bin::reveal(&p) {
                        f.set_status(format!("{err}").into());
                    }
                }
            }
            "delete" => {
                let ouverte_dedans = e
                    .note
                    .as_ref()
                    .is_some_and(|n| n.rel == cle || n.rel.starts_with(&format!("{cle}/")));
                if ouverte_dedans {
                    e.note = None;
                    sans_focus(&f, e);
                    montrer_note(&f, e);
                }
                match e.espace().map(|s| s.delete(&cle, &bin::SystemBin)) {
                    Some(Ok(())) => {
                        f.set_toast(format!("“{}” is in the Recycle Bin.", stem(&cle)).into())
                    }
                    Some(Err(err)) => f.set_status(format!("Could not delete it: {err}").into()),
                    None => {}
                }
                e.selected = None;
                montrer_arbre(&f, e);
            }
            _ => {}
        }
    });
    geste!(on_notes_renamed, |f, e, k, nom| {
        f.set_notes_renaming(SharedString::default());
        let k = k.to_string();
        let Some(espace) = e.espace() else { return };
        match espace.rename(&k, &nom) {
            Ok(nouveau) => {
                suivre_deplacement(e, &k, &nouveau);
                if e.note.as_ref().is_some_and(|n| n.rel == nouveau) {
                    montrer_note(&f, e);
                }
            }
            Err(err) => f.set_status(format!("Could not rename it: {err}").into()),
        }
        montrer_arbre(&f, e);
    });
    geste!(on_notes_rename_cancelled, |f, e| {
        f.set_notes_renaming(SharedString::default());
        montrer_arbre(&f, e);
    });
    geste!(on_notes_moved, |f, e, k, cible| {
        let (k, cible) = (k.to_string(), cible.to_string());
        let dossier = match e.tree_keys.iter().find(|(r, _)| *r == cible) {
            Some((_, EntryKind::Folder)) => cible,
            _ if cible.is_empty() => String::new(),
            _ => parent_of(&cible),
        };
        if parent_of(&k) == dossier || k == dossier {
            return;
        }
        ecrire(&f, e);
        let Some(espace) = e.espace() else { return };
        match espace.move_to(&k, &dossier) {
            Ok(nouveau) => {
                suivre_deplacement(e, &k, &nouveau);
                if !dossier.is_empty() {
                    e.expanded.insert(dossier);
                }
                montrer_note(&f, e);
            }
            Err(err) => f.set_status(format!("Could not move it: {err}").into()),
        }
        montrer_arbre(&f, e);
    });
    geste!(on_notes_new_note, |f, e| {
        nouvelle_note(&f, e, "Untitled", "");
    });
    geste!(on_notes_new_folder, |f, e, nom| {
        let parent = e.dossier_courant();
        let Some(espace) = e.espace() else { return };
        match espace.create_folder(&parent, &nom) {
            Ok(rel) => {
                if !parent.is_empty() {
                    e.expanded.insert(parent);
                }
                e.selected = Some(rel);
                f.set_notes_name_asked("".into());
                f.set_notes_name_error("".into());
                montrer_arbre(&f, e);
            }
            Err(err) => f.set_notes_name_error(err.to_string().into()),
        }
    });
    geste!(on_notes_filter_edited, |f, e, t| {
        e.filter = t.to_string();
        montrer_arbre(&f, e);
    });
    geste!(on_notes_quick_edited, |f, e, q| {
        let Some(espace) = e.espace() else { return };
        // Searching the words: the notes holding them, with the line found.
        if f.get_notes_quick_search() {
            let trouves: Vec<NoteFoundData> = if q.trim().is_empty() {
                Vec::new()
            } else {
                espace
                    .search(&q, 40)
                    .into_iter()
                    .map(|(rel, ligne)| NoteFoundData {
                        title: stem(&rel).into(),
                        key: rel.into(),
                        detail: ligne.into(),
                    })
                    .collect()
            };
            f.set_notes_quick_results(ModelRc::new(VecModel::from(trouves)));
            return;
        }
        let notes = espace.notes();
        let noms: Vec<String> = notes.iter().map(|n| stem(n)).collect();
        let refs: Vec<&str> = noms.iter().map(String::as_str).collect();
        let ordre = if q.trim().is_empty() {
            // Nothing typed: the notes changed last.
            let mut recents: Vec<(i64, usize)> = notes
                .iter()
                .enumerate()
                .map(|(i, n)| (espace.modified(n).unwrap_or(0), i))
                .collect();
            recents.sort_by(|a, b| b.0.cmp(&a.0));
            recents.into_iter().take(20).map(|(_, i)| i).collect()
        } else {
            iris_notes::fuzzy::rank(&q, &refs, 30)
        };
        let trouves: Vec<NoteFoundData> = ordre
            .into_iter()
            .map(|i| NoteFoundData {
                key: notes[i].as_str().into(),
                title: noms[i].as_str().into(),
                detail: parent_of(&notes[i]).replace('/', " › ").into(),
            })
            .collect();
        f.set_notes_quick_results(ModelRc::new(VecModel::from(trouves)));
    });
    geste!(on_notes_quick_chosen, |f, e, k| {
        match k.strip_prefix("new:") {
            Some(nom) => nouvelle_note(&f, e, nom, ""),
            None => {
                let k = k.to_string();
                ouvrir(&f, e, &k);
                montrer_arbre(&f, e);
            }
        }
    });

    // --- The note ---
    geste!(on_note_block_edited, |f, e, i, texte, curseur| {
        let i = i.max(0) as usize;
        let Some(note) = &e.note else { return };
        if i >= note.blocks.len() {
            return;
        }
        let ed = edit::replace_block(&note.blocks, i, &texte, curseur.max(0) as usize);
        let (bloc, position) = (ed.block, ed.cursor);
        retenir(e, false);
        appliquer(&f, e, ed, false, None);
        e.caret.block = bloc;
        e.caret.cursor = position;
        e.caret.anchor = position;
        apres_curseur(&f, e);
    });
    {
        let (etat, faible) = (Rc::clone(&etat), f.as_weak());
        f.on_note_block_key(move |i, nom, curseur, ancre, _premiere, _derniere| {
            let Some(f) = faible.upgrade() else {
                return false;
            };
            let Ok(mut e) = etat.try_borrow_mut() else {
                return false;
            };
            touche(
                &f,
                &mut e,
                i.max(0) as usize,
                &nom,
                curseur.max(0) as usize,
                ancre.max(0) as usize,
            )
        });
    }
    geste!(on_note_block_clicked, |f, e, i| {
        let i = i.max(0) as usize;
        let fin = contenu(e, i).map_or(0, |(_, t)| t.len());
        focaliser(&f, e, i, fin, fin);
        rendre(&f, e, false);
    });
    geste!(on_note_end_clicked, |f, e| {
        let Some(note) = &e.note else { return };
        let dernier = note.blocks.len().saturating_sub(1);
        if note
            .blocks
            .last()
            .is_some_and(|b| b.content().trim().is_empty())
        {
            focaliser(&f, e, dernier, 0, 0);
            rendre(&f, e, false);
            return;
        }
        let le = edit::line_ending_of(&note.text);
        let texte = if note.text.is_empty() || note.text.ends_with('\n') {
            note.text.clone()
        } else {
            format!("{}{le}", note.text)
        };
        let blocs = blocs_de(&texte);
        let ed = NoteEdit {
            block: blocs.len(),
            cursor: 0,
            text: texte,
        };
        retenir(e, true);
        appliquer(&f, e, ed, true, None);
    });
    geste!(on_note_link_clicked, |f, e, lien| {
        f.set_notes_ctrl_held(false);
        suivre(&f, e, &lien);
    });
    geste!(on_note_task_toggled, |f, e, i| {
        let i = i.max(0) as usize;
        if let Some((_, t)) = contenu(e, i) {
            let n = edit::toggle_task(&t);
            let Some(note) = &e.note else { return };
            let ed = edit::replace_block(&note.blocks, i, &n, 0);
            retenir(e, true);
            appliquer(&f, e, ed, false, None);
        }
    });
    geste!(on_note_fold_toggled, |f, e, i| {
        let i = i.max(0) as usize;
        let Some((BlockKind::Callout { folded, .. }, t)) = contenu(e, i) else {
            return;
        };
        let premiere = t.lines().next().unwrap_or("");
        let Some(fin) = premiere.find(']') else {
            return;
        };
        let apres = &premiere[fin + 1..];
        let nouvelle = match folded {
            Some(true) => format!(
                "{}{}",
                &premiere[..=fin],
                apres.strip_prefix('-').unwrap_or(apres)
            ),
            _ => format!(
                "{}-{}",
                &premiere[..=fin],
                apres.strip_prefix('+').unwrap_or(apres)
            ),
        };
        let reste = &t[premiere.len()..];
        let n = format!("{nouvelle}{reste}");
        let Some(note) = &e.note else { return };
        let ed = edit::replace_block(&note.blocks, i, &n, 0);
        retenir(e, true);
        appliquer(&f, e, ed, false, None);
    });
    geste!(on_note_caret, |f, e, i, x, y, curseur, ancre| {
        e.caret = Curseur {
            block: i.max(0) as usize,
            cursor: curseur.max(0) as usize,
            anchor: ancre.max(0) as usize,
            x,
            y,
        };
        apres_curseur(&f, e);
    });
    geste!(on_note_completion_chosen, |f, e, k| {
        choisir(&f, e, k.max(0) as usize);
    });
    geste!(on_note_format, |f, e, action| {
        formater(&f, e, &action);
    });
    geste!(on_note_card_open, |f, e| {
        let cible = f.get_note_card_title().to_string();
        fermer_popups(&f, e);
        suivre(
            &f,
            e,
            &format!("iris-note:{}", iris_notes::inline::encode_target(&cible)),
        );
    });
    geste!(on_note_side_toggled, |f, e| {
        e.side = !e.side;
        f.set_note_side(e.side);
        maj_cote(&f, e);
    });
    geste!(on_note_outline_chosen, |f, e, k| {
        if let Ok(bloc) = k.parse::<usize>() {
            if e.reading {
                lecture(&f, e);
            }
            focaliser(&f, e, bloc, 0, 0);
            rendre(&f, e, false);
        }
    });
    geste!(on_note_backlink_chosen, |f, e, k| {
        let k = k.to_string();
        ouvrir(&f, e, &k);
        montrer_arbre(&f, e);
    });
    geste!(on_note_reading_toggled, |f, e| {
        lecture(&f, e);
    });
    geste!(on_note_title_accepted, |f, e, nom| {
        let Some(note) = &e.note else { return };
        let rel = note.rel.clone();
        if nom.trim().is_empty() || nom.trim() == stem(&rel) {
            f.set_note_title(stem(&rel).into());
            return;
        }
        ecrire(&f, e);
        let Some(espace) = e.espace() else { return };
        match espace.rename(&rel, &nom) {
            Ok(nouveau) => {
                suivre_deplacement(e, &rel, &nouveau);
                montrer_note(&f, e);
                montrer_arbre(&f, e);
            }
            Err(err) => {
                f.set_status(format!("Could not rename the note: {err}").into());
                f.set_note_title(stem(&rel).into());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_split_as_shown() {
        assert_eq!(parent_of("Analyse/Chapitre 1.md"), "Analyse");
        assert_eq!(parent_of("Accueil.md"), "");
        assert_eq!(stem("Analyse/Chapitre 1.md"), "Chapitre 1");
        assert_eq!(stem("Budget.sheet"), "Budget");
    }

    #[test]
    fn an_empty_note_still_has_a_line() {
        let b = blocs_de("");
        assert_eq!(b.len(), 1);
        assert_eq!(block::join(&b), "");
    }

    #[test]
    fn colour_keys_on_both_keyboards() {
        assert_eq!(couleur_de_touche("1"), Some(Colour::Red));
        assert_eq!(couleur_de_touche("&"), Some(Colour::Red));
        assert_eq!(couleur_de_touche("è"), Some(Colour::Grey));
        assert_eq!(couleur_de_touche("9"), None);
    }
}
