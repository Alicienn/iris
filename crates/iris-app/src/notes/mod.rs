//! Notes: Markdown files in spaces, written block by block.
//!
//! The place lists a space's tree on the left and shows the open note on the right.
//! The note is held here as its text and its blocks (`iris_notes::block`); every key
//! that changes it goes through a pure edit (`iris_notes::edit`), the text is parsed
//! again, and only the blocks whose rendering changed are given back to the window.
//! It is written to its file half a second after the last key, atomically, and read
//! again when another program changes it.

mod bin;
mod export;
pub mod render;
mod sheet;

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
/// The colours a folder can take, kept as written in `space.json`.
const COULEURS_DOSSIERS: [&str; 8] = [
    "#e5534b", "#f0883e", "#d4a72c", "#3fb950", "#4f8cff", "#a371f7", "#db61a2", "#8b949e",
];

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
    /// The version before this session of editing is kept already.
    versionnee: bool,
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
    /// Only the note on screen: no tree, no side panel.
    focus_mode: bool,
    /// The flashcards being revised.
    revision: Option<Revision>,
    /// The note open beside, read-only.
    beside: Option<String>,
    /// The spreadsheet open, in place of a note.
    sheet: Option<sheet::Feuille>,
    /// For the tasks a checkbox is linked to.
    services: Services,
    /// The history shown: the note, and when each of its versions was kept.
    historique: Option<(String, Vec<i64>)>,
}

/// A revision under way: the cards left, the one shown, and how it went.
struct Revision {
    /// (key, question, answer), the one shown first.
    cards: std::collections::VecDeque<(String, String, String)>,
    states: std::collections::BTreeMap<String, iris_notes::review::CardState>,
    revealed: bool,
    total: usize,
    done: usize,
    again: usize,
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
    // Formulas in the text's own colour, at the screen's scale.
    let encre = f.global::<iris_ui::Tokens>().get_text();
    let echelle = f.window().scale_factor();
    let formule = move |latex: &str| render::formula_picture(latex, encre, echelle);
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
            .map(|(b, n)| render::render(b, n, &e.palette, dir, &formule))
            .collect();
        e.model.set_vec(lignes);
    } else {
        for (i, (b, n)) in note.blocks.iter().zip(&numeros).enumerate() {
            if e.signatures.get(i) != Some(&sigs[i]) {
                e.model
                    .set_row_data(i, render::render(b, n, &e.palette, dir, &formule));
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
            f.set_note_has_cards(!iris_notes::meta::flashcards(&n.text).is_empty());
        }
        None => {
            f.set_note_has_cards(false);
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
    sheet::ecrire(f, e);
    let Some(espace) = e.spaces.get(e.space).cloned() else {
        return;
    };
    let Some(note) = &mut e.note else { return };
    if !note.dirty {
        return;
    }
    f.set_note_has_cards(!iris_notes::meta::flashcards(&note.text).is_empty());
    // The note as it was before this session of editing, kept once.
    if !note.versionnee {
        note.versionnee = true;
        if let Ok((avant, quand)) = espace.read(&note.rel) {
            if avant != note.text && !avant.trim().is_empty() {
                let _ = espace.save_version(&note.rel, &avant, quand);
            }
        }
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
    if EntryKind::of(rel) == EntryKind::Sheet {
        ouvrir_tableur(f, e, rel);
        return;
    }
    if e.sheet.is_some() {
        sheet::fermer(f, e);
    }
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
                versionnee: false,
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
            aligner_taches(f, e);
            crate::settings::update(|s| s.notes_last = format!("{dir}|{rel}"));
            crate::nav::note(f, "note", rel);
            fermer_popups(f, e);
            maj_cote(f, e);
        }
        Err(err) => f.set_status(format!("Could not open the note: {err}").into()),
    }
}

/// Everything open written and closed: the note, the spreadsheet, the note beside.
fn fermer_tout(f: &AppWindow, e: &mut Etat) {
    ecrire(f, e);
    if e.sheet.is_some() {
        sheet::fermer(f, e);
    }
    e.note = None;
    e.beside = None;
    montrer_a_cote(f, e);
    sans_focus(f, e);
}

/// Opens a spreadsheet of the space in place of the note.
fn ouvrir_tableur(f: &AppWindow, e: &mut Etat, rel: &str) {
    if e.sheet.is_some() {
        sheet::fermer(f, e);
    }
    e.note = None;
    sans_focus(f, e);
    fermer_popups(f, e);
    montrer_note(f, e);
    sheet::ouvrir(f, e, rel);
    if e.sheet.is_none() {
        return;
    }
    e.selected = Some(rel.to_string());
    let mut p = parent_of(rel);
    while !p.is_empty() {
        e.expanded.insert(p.clone());
        p = parent_of(&p);
    }
    let espace = e
        .espace()
        .map(|s| s.config.name.clone())
        .unwrap_or_default();
    let dossier = parent_of(rel);
    f.set_note_path(
        if dossier.is_empty() {
            format!("{espace} › {}", stem(rel))
        } else {
            format!("{espace} › {} › {}", dossier.replace('/', " › "), stem(rel))
        }
        .into(),
    );
    if let Some(dir) = e.espace().map(|s| s.dir().display().to_string()) {
        crate::settings::update(|s| s.notes_last = format!("{dir}|{rel}"));
    }
    crate::nav::note(f, "note", rel);
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
            cocher_tache(e, &t);
            true
        }
        "ctrl+shift+t" | "ctrl+shift+T" => {
            faire_tache(f, e, i);
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
        "f11" => {
            mode_focus(f, e);
            true
        }
        "ctrl+g" | "ctrl+G" => {
            graphe(f, e);
            true
        }
        "ctrl+shift+f" | "ctrl+shift+F" => {
            f.set_notes_quick_beside(false);
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
        "ctrl+v" | "ctrl+V" => {
            coller_image(f, e, i, cursor) || coller_texte(f, e, i, cursor, anchor)
        }
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
            let ailleurs = lien_iris(&nom).map(|(genre, _)| match genre {
                "mail" => "A conversation of your mail: Open shows it.",
                "task" => "A task: Open shows it.",
                _ => "An event of your calendar: Open shows it.",
            });
            f.set_note_card_text(
                match (ailleurs, apercu) {
                    (Some(a), _) => a.to_string(),
                    (None, Some(t)) => t,
                    (None, None) => "Not written yet: Open makes it.".into(),
                }
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
            let encre = f.global::<iris_ui::Tokens>().get_text();
            // At one pixel a point: the card shows it at its own size.
            match render::formula_picture(source, encre, 1.0) {
                Some((image, _)) => {
                    f.set_note_card_picture(image);
                    f.set_note_card_text(SharedString::default());
                }
                None => {
                    f.set_note_card_picture(slint::Image::default());
                    f.set_note_card_text(
                        format!(
                            "Not read as LaTeX: {}",
                            iris_notes::math::to_unicode(source)
                        )
                        .into(),
                    );
                }
            }
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
    let mut candidat = candidat.clone();
    // `/sheet`: a spreadsheet made beside the note, and embedded.
    if candidat.insert == "![[Sheet.sheet]]" {
        let note = e.note.as_ref().map(|n| n.rel.clone()).unwrap_or_default();
        let fait = e.espace().and_then(|s| {
            s.create_file(
                &parent_of(&note),
                &format!("{} table", stem(&note)),
                "sheet",
                iris_sheets::Workbook::default().to_json().as_bytes(),
            )
            .ok()
        });
        if let Some(rel) = fait {
            let nom = rel.rsplit('/').next().unwrap_or(&rel).to_string();
            candidat.insert = format!("![[{nom}]]");
            candidat.cursor = candidat.insert.len();
            montrer_arbre(f, e);
        }
    }
    let r = iris_notes::complete::apply(&texte, curseur, &c.trigger, &candidat);
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

/// Text from the clipboard made Markdown: a web address over selected words links
/// them, a page's or a document's formatting is kept. False for plain text (the field
/// pastes it).
fn coller_texte(f: &AppWindow, e: &mut Etat, i: usize, cursor: usize, anchor: usize) -> bool {
    let Some((kind, texte)) = contenu(e, i) else {
        return false;
    };
    if matches!(kind, BlockKind::Code { .. } | BlockKind::Math) {
        return false;
    }
    let Ok(mut presse) = arboard::Clipboard::new() else {
        return false;
    };
    let (debut, fin) = (
        cursor.min(anchor).min(texte.len()),
        cursor.max(anchor).min(texte.len()),
    );
    if !texte.is_char_boundary(debut) || !texte.is_char_boundary(fin) {
        return false;
    }
    if debut < fin {
        if let Ok(t) = presse.get_text() {
            if iris_notes::paste::is_url(&t) {
                let lien = format!("[{}]({})", &texte[debut..fin], t.trim());
                let nouveau = format!("{}{lien}{}", &texte[..debut], &texte[fin..]);
                let c = debut + lien.len();
                remplacer(f, e, i, &nouveau, c, c);
                return true;
            }
        }
    }
    let Ok(html) = presse.get().html() else {
        return false;
    };
    if !iris_notes::paste::is_rich(&html) {
        return false;
    }
    let md = iris_notes::paste::html_to_markdown(&html);
    if md.trim().is_empty() {
        return false;
    }
    let nouveau = format!("{}{md}{}", &texte[..debut], &texte[fin..]);
    let c = debut + md.len();
    remplacer(f, e, i, &nouveau, c, c);
    true
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

// --- Focus, beside, revising, pages ---------------------------------------------------

/// Focus mode on or off: only the note, the tree and the side panel put away.
fn mode_focus(f: &AppWindow, e: &mut Etat) {
    e.focus_mode = !e.focus_mode;
    f.set_note_focus_mode(e.focus_mode);
}

/// The note beside drawn again (`None`: closed).
fn montrer_a_cote(f: &AppWindow, e: &mut Etat) {
    let Some(rel) = e.beside.clone() else {
        f.set_note_beside(ModelRc::default());
        f.set_note_beside_title(SharedString::default());
        return;
    };
    let Some(espace) = e.espace() else { return };
    let dir = espace.dir().to_path_buf();
    let Ok((texte, _)) = espace.read(&rel) else {
        e.beside = None;
        f.set_note_beside(ModelRc::default());
        return;
    };
    let encre = f.global::<iris_ui::Tokens>().get_text();
    let echelle = f.window().scale_factor();
    let formule = move |latex: &str| render::formula_picture(latex, encre, echelle);
    let blocs = blocs_de(&texte);
    let numeros = render::numbering(&blocs);
    let lignes: Vec<NoteBlockData> = blocs
        .iter()
        .zip(&numeros)
        .map(|(b, n)| render::render(b, n, &e.palette, Some(&dir), &formule))
        .collect();
    f.set_note_beside(ModelRc::new(VecModel::from(lignes)));
    f.set_note_beside_title(stem(&rel).into());
}

fn aujourdhui() -> chrono::NaiveDate {
    chrono::Local::now().date_naive()
}

/// Starts revising the open note's flashcards: those due, or all of them when none is.
fn reviser(f: &AppWindow, e: &mut Etat) {
    ecrire(f, e);
    let Some(rel) = e.note.as_ref().map(|n| n.rel.clone()) else {
        return;
    };
    reviser_notes(f, e, &[rel]);
}

/// Revising the flashcards of every note in `folder` ("" for the whole space).
fn reviser_dossier(f: &AppWindow, e: &mut Etat, folder: &str) {
    ecrire(f, e);
    let Some(espace) = e.espace() else { return };
    let modeles = format!("{}/", espace.config.templates);
    let prefixe = format!("{folder}/");
    let notes: Vec<String> = espace
        .notes()
        .into_iter()
        .filter(|n| !n.starts_with(&modeles) && (folder.is_empty() || n.starts_with(&prefixe)))
        .collect();
    reviser_notes(f, e, &notes);
}

/// Revising the flashcards of `notes`: those due, or all of them when none is.
fn reviser_notes(f: &AppWindow, e: &mut Etat, notes: &[String]) {
    let Some(espace) = e.espace() else { return };
    let mut toutes: Vec<(String, String, String)> = Vec::new();
    for rel in notes {
        // The note open as it is now; the others as written.
        let texte = match &e.note {
            Some(n) if n.rel == *rel => n.text.clone(),
            _ => match espace.read(rel) {
                Ok((t, _)) => t,
                Err(_) => continue,
            },
        };
        toutes.extend(
            iris_notes::meta::flashcards(&texte)
                .into_iter()
                .map(|(q, r)| (iris_notes::review::card_key(rel, &q), q, r)),
        );
    }
    if toutes.is_empty() {
        f.set_toast(
            if notes.len() == 1 {
                "This note has no flashcards: write “Question :: Answer”."
            } else {
                "No flashcards here: write “Question :: Answer” in a note."
            }
            .into(),
        );
        return;
    }
    let etats = espace.review();
    let jour = aujourdhui();
    let dues: std::collections::VecDeque<_> = toutes
        .iter()
        .filter(|(k, _, _)| etats.get(k).is_none_or(|s| s.due <= jour))
        .cloned()
        .collect();
    let cards = if dues.is_empty() {
        toutes.into_iter().collect()
    } else {
        dues
    };
    e.revision = Some(Revision {
        total: cards.len(),
        cards,
        states: etats,
        revealed: false,
        done: 0,
        again: 0,
    });
    montrer_revision(f, e);
    f.set_notes_review_open(true);
}

fn montrer_revision(f: &AppWindow, e: &Etat) {
    let Some(r) = &e.revision else { return };
    match r.cards.front() {
        Some((_, q, a)) => {
            f.set_notes_review_finished(false);
            f.set_notes_review_question(q.as_str().into());
            f.set_notes_review_answer(a.as_str().into());
            f.set_notes_review_revealed(r.revealed);
            f.set_notes_review_progress(
                format!("{} of {}", (r.done + 1).min(r.total), r.total).into(),
            );
        }
        None => {
            f.set_notes_review_finished(true);
            f.set_notes_review_progress(
                match r.again {
                    0 => format!("{} cards revised. Well done.", r.total),
                    1 => format!("{} cards revised, one seen twice.", r.total),
                    n => format!("{} cards revised, {n} seen twice.", r.total),
                }
                .into(),
            );
        }
    }
}

/// The card shown graded; the next one shown. A card forgotten comes back at the end.
fn noter(f: &AppWindow, e: &mut Etat, note: i32) {
    let Some(grade) = iris_notes::review::Grade::from_index(note) else {
        return;
    };
    let Some(r) = &mut e.revision else { return };
    if !r.revealed {
        return;
    }
    let Some(carte) = r.cards.pop_front() else {
        return;
    };
    let jour = aujourdhui();
    let avant = r
        .states
        .get(&carte.0)
        .cloned()
        .unwrap_or_else(|| iris_notes::review::CardState::new(jour));
    r.states.insert(
        carte.0.clone(),
        iris_notes::review::grade(&avant, grade, jour),
    );
    if grade == iris_notes::review::Grade::Again {
        r.again += 1;
        r.cards.push_back(carte);
    } else {
        r.done += 1;
    }
    r.revealed = false;
    let etats = r.states.clone();
    if let Some(Err(err)) = e.espace().map(|s| s.save_review(&etats)) {
        f.set_status(format!("Could not keep the revision: {err}").into());
    }
    montrer_revision(f, e);
}

/// The note `rel` as a web page: saved where the user says, or opened to be printed.
fn exporter(f: &AppWindow, e: &mut Etat, rel: &str, imprimer: bool) {
    ecrire(f, e);
    let Some(espace) = e.espace() else { return };
    let texte = match espace.read(rel) {
        Ok((t, _)) => t,
        Err(err) => {
            f.set_status(format!("Could not read the note: {err}").into());
            return;
        }
    };
    let titre = stem(rel);
    // Printed on paper: the light palette, whatever the window's theme.
    let html = export::page(&texte, &titre, espace.dir(), &Palette::default(), imprimer);
    let chemin = if imprimer {
        let dossier = std::env::temp_dir().join("iris-print");
        let _ = std::fs::create_dir_all(&dossier);
        dossier.join(format!("{}.html", nom_sur(&titre)))
    } else {
        let Some(c) = rfd::FileDialog::new()
            .set_title("Export as a web page")
            .set_file_name(format!("{}.html", nom_sur(&titre)))
            .add_filter("Web page", &["html"])
            .save_file()
        else {
            return;
        };
        c
    };
    if let Err(err) = std::fs::write(&chemin, html) {
        f.set_status(format!("Could not write the page: {err}").into());
        return;
    }
    if imprimer {
        if let Err(err) = crate::platform::open_path(&chemin) {
            f.set_status(format!("Could not open the page: {err}").into());
        }
    } else {
        f.set_toast(format!("“{titre}” is saved as a web page.").into());
    }
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
        // `[[#thm-2]]`, `[[#Heading]]`: a place in this note.
        let ancre = cible
            .split_once('#')
            .map(|(_, a)| a.trim().to_string())
            .filter(|a| !a.is_empty() && lien_iris(&nom).is_none());
        if nom.is_empty() {
            if let Some(a) = ancre {
                aller_ancre(f, e, &a);
            }
            return;
        }
        // The rest of Iris: a conversation, a task, an event.
        if let Some((genre, id)) = lien_iris(&nom) {
            ecrire(f, e);
            match genre {
                "mail" => match id.parse::<i32>() {
                    Ok(n) => f.invoke_home_thread_opened(n),
                    Err(_) => f.set_status("This link to a conversation is not valid.".into()),
                },
                "task" => match id.parse::<i32>() {
                    Ok(n) => f.invoke_home_task_opened(n),
                    Err(_) => f.set_status("This link to a task is not valid.".into()),
                },
                _ => f.invoke_home_event_opened(id.into()),
            }
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
            Some(rel) => {
                ouvrir(f, e, &rel);
                if let Some(a) = ancre {
                    aller_ancre(f, e, &a);
                }
            }
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

// --- History and graph -----------------------------------------------------------------

/// A moment as the history lists it: `7 Oct 2026, 14:32`.
fn quand(millis: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(millis)
        .single()
        .map(|t| t.format("%-d %b %Y, %H:%M").to_string())
        .unwrap_or_default()
}

/// The past versions of `rel`, listed.
fn historique(f: &AppWindow, e: &mut Etat, rel: &str) {
    ecrire(f, e);
    let Some(espace) = e.espace() else { return };
    let versions = espace.versions(rel);
    let lignes: Vec<NoteFoundData> = versions
        .iter()
        .map(|(t, _)| {
            let mots = espace
                .read_version(rel, *t)
                .map(|x| iris_notes::meta::word_count(&x))
                .unwrap_or(0);
            NoteFoundData {
                key: t.to_string().into(),
                title: quand(*t).into(),
                detail: format!("{mots} words").into(),
            }
        })
        .collect();
    e.historique = Some((rel.to_string(), versions.iter().map(|(t, _)| *t).collect()));
    f.set_notes_history(ModelRc::new(VecModel::from(lignes)));
    f.set_notes_history_chosen(-1);
    f.set_notes_history_preview(SharedString::default());
    f.set_notes_history_open(true);
}

/// The graph of the space: its notes placed by their links.
fn graphe(f: &AppWindow, e: &mut Etat) {
    ecrire(f, e);
    let Some(espace) = e.espace() else { return };
    let modeles = format!("{}/", espace.config.templates);
    let notes: Vec<String> = espace
        .notes()
        .into_iter()
        .filter(|n| !n.starts_with(&modeles))
        .collect();
    if notes.is_empty() {
        f.set_toast("This space has no note yet.".into());
        return;
    }
    let index: std::collections::HashMap<&str, usize> = notes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let mut aretes: HashSet<(usize, usize)> = HashSet::new();
    for (i, rel) in notes.iter().enumerate() {
        let Ok((texte, _)) = espace.read(rel) else {
            continue;
        };
        for l in iris_notes::links::links(&texte) {
            if let Some(j) = iris_notes::links::resolve(&l.target, &notes)
                .and_then(|c| index.get(c.as_str()).copied())
            {
                if i != j {
                    aretes.insert((i.min(j), i.max(j)));
                }
            }
        }
    }
    let aretes: Vec<(usize, usize)> = aretes.into_iter().collect();
    let places = iris_notes::graph::layout(notes.len(), &aretes);
    let mut degre = vec![0usize; notes.len()];
    let mut chemin = String::new();
    for &(a, b) in &aretes {
        degre[a] += 1;
        degre[b] += 1;
        let (pa, pb) = (places[a], places[b]);
        chemin.push_str(&format!(
            "M {:.1} {:.1} L {:.1} {:.1} ",
            pa.0 * 1000.0,
            pa.1 * 1000.0,
            pb.0 * 1000.0,
            pb.1 * 1000.0
        ));
    }
    let max = degre.iter().copied().max().unwrap_or(0).max(1) as f32;
    let ouverte = e.note.as_ref().map(|n| n.rel.clone());
    let noeuds: Vec<iris_ui::NoteGraphNodeData> = notes
        .iter()
        .enumerate()
        .map(|(i, rel)| iris_ui::NoteGraphNodeData {
            key: rel.as_str().into(),
            label: stem(rel).into(),
            x: places[i].0,
            y: places[i].1,
            size: (degre[i] as f32 / max).sqrt(),
            current: ouverte.as_deref() == Some(rel.as_str()),
        })
        .collect();
    f.set_notes_graph_nodes(ModelRc::new(VecModel::from(noeuds)));
    f.set_notes_graph_edges(chemin.into());
    f.set_notes_graph_open(true);
}

// --- Checkboxes and the tasks of Iris ---------------------------------------------------

/// The task a checkbox line is linked to (`[[task:7]]`), and whether the line is ticked.
fn tache_de_ligne(ligne: &str) -> Option<(i64, bool)> {
    let debut = ligne.find("[[task:")?;
    let reste = &ligne[debut + "[[task:".len()..];
    let fin = reste.find([']', '|'])?;
    let id = reste[..fin].trim().parse().ok()?;
    let t = ligne.trim_start();
    Some((id, t.starts_with("- [x]") || t.starts_with("- [X]")))
}

/// A checkbox ticked or unticked in a note: its task follows.
fn cocher_tache(e: &Etat, ligne: &str) {
    let Some((id, coche)) = tache_de_ligne(ligne) else {
        return;
    };
    if let Ok(Some(t)) = e.services.store.task(id) {
        if t.is_done() != coche {
            crate::tasks::toggle_done(&e.services, id);
        }
    }
}

/// A checkbox line made a task of Iris, each ticking the other.
fn faire_tache(f: &AppWindow, e: &mut Etat, i: usize) {
    let Some((kind, texte)) = contenu(e, i) else {
        return;
    };
    if !matches!(kind, BlockKind::Task { .. }) {
        f.set_toast("Put the cursor on a checkbox line (- [ ] …) to make it a task.".into());
        return;
    }
    if tache_de_ligne(&texte).is_some() {
        f.set_toast("This line is a task already.".into());
        return;
    }
    let ligne = texte.trim_start();
    let apres = ligne.get(6..).unwrap_or("");
    let titre = iris_notes::inline::plain_text(&iris_notes::inline::parse_inline(apres))
        .trim()
        .to_string();
    if titre.is_empty() {
        f.set_toast("Write the task on the line first.".into());
        return;
    }
    let note = e.note.as_ref().map(|n| stem(&n.rel)).unwrap_or_default();
    let Some(liste) = e
        .services
        .store
        .task_lists()
        .ok()
        .and_then(|l| l.first().map(|x| x.id))
    else {
        f.set_status("Make a list in Tasks first.".into());
        return;
    };
    let t = iris_store::NewTask {
        list_id: liste,
        title: titre,
        notes: format!("From the note “{note}”."),
        source: format!("Note: {note}"),
        ..Default::default()
    };
    match e.services.store.insert_task(&t, crate::services::now()) {
        Ok(id) => {
            if ligne.starts_with("- [x]") || ligne.starts_with("- [X]") {
                let _ = e
                    .services
                    .store
                    .set_task_done(id, Some(crate::services::now()));
            }
            let nouveau = format!("{} [[task:{id}]]", texte.trim_end());
            let c = nouveau.len();
            remplacer(f, e, i, &nouveau, c, c);
            f.set_toast("Task made: ticking one ticks the other.".into());
        }
        Err(err) => f.set_status(format!("Could not make the task: {err}").into()),
    }
}

/// The checkboxes linked to tasks ticked as their tasks are now (a task may have been
/// done in Tasks since).
fn aligner_taches(f: &AppWindow, e: &mut Etat) {
    let Some(note) = &mut e.note else { return };
    let mut blocs = note.blocks.clone();
    let mut change = false;
    let mut k = 0;
    while k < blocs.len() {
        let c = blocs[k].content().to_string();
        let lie = matches!(blocs[k].kind, BlockKind::Task { .. })
            .then(|| tache_de_ligne(&c))
            .flatten();
        if let Some((id, coche)) = lie {
            if let Ok(Some(t)) = e.services.store.task(id) {
                if t.is_done() != coche {
                    let ed = edit::replace_block(&blocs, k, &edit::toggle_task(&c), 0);
                    blocs = blocs_de(&ed.text);
                    change = true;
                }
            }
        }
        k += 1;
    }
    if change {
        note.text = block::join(&blocs);
        note.blocks = blocs;
        note.dirty = true;
        rendre(f, e, true);
        planifier_ecriture(f);
    }
}

/// The cursor put on the block a link's `#…` names in the open note.
fn aller_ancre(f: &AppWindow, e: &mut Etat, ancre: &str) {
    let Some(i) = e
        .note
        .as_ref()
        .and_then(|n| render::ancre_bloc(&n.blocks, ancre))
    else {
        f.set_toast(format!("Nothing called “{ancre}” in this note.").into());
        return;
    };
    if e.reading {
        lecture(f, e);
    }
    focaliser(f, e, i, 0, 0);
    rendre(f, e, false);
}

/// `[[target|title]]` on the clipboard, to paste in a note.
fn copier_lien(f: &AppWindow, cible: &str, titre: &str) {
    let lien = lien_ecrit(cible, titre);
    match arboard::Clipboard::new().and_then(|mut c| c.set_text(lien)) {
        Ok(()) => f.set_toast("Link copied: paste it in a note.".into()),
        Err(err) => f.set_status(format!("Could not copy the link: {err}").into()),
    }
}

/// `[[target|title]]`, the title kept from closing the link early.
fn lien_ecrit(cible: &str, titre: &str) -> String {
    let titre: String = titre
        .chars()
        .map(|c| {
            if matches!(c, '[' | ']' | '|' | '\n' | '\r') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let titre = titre.split_whitespace().collect::<Vec<_>>().join(" ");
    if titre.is_empty() {
        format!("[[{cible}]]")
    } else {
        format!("[[{cible}|{titre}]]")
    }
}

/// A link to the rest of Iris: `mail:12`, `task:7`, `event:<key>`.
fn lien_iris(cible: &str) -> Option<(&'static str, &str)> {
    let (genre, id) = cible.split_once(':')?;
    let genre = match genre.trim().to_ascii_lowercase().as_str() {
        "mail" => "mail",
        "task" => "task",
        "event" => "event",
        _ => return None,
    };
    let id = id.trim();
    (!id.is_empty()).then_some((genre, id))
}

/// The note open moved or renamed: it follows.
fn suivre_deplacement(e: &mut Etat, avant: &str, apres: &str) {
    let suivre = |rel: &mut String| {
        if rel == avant {
            *rel = apres.to_string();
        } else if let Some(reste) = rel.strip_prefix(&format!("{avant}/")) {
            *rel = format!("{apres}/{reste}");
        }
    };
    if let Some(n) = &mut e.note {
        suivre(&mut n.rel);
    }
    if let Some(s) = &mut e.sheet {
        suivre(&mut s.rel);
    }
    if let Some(b) = &mut e.beside {
        suivre(b);
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

/// Home's Notes widget: the notes of the space shown changed last, and the flashcards
/// due today.
pub fn refresh_home(f: &AppWindow) {
    let etat = ETAT.with(|e| e.borrow().as_ref().cloned());
    let Some(etat) = etat else { return };
    let Ok(e) = etat.try_borrow() else { return };
    let Some(s) = e.espace() else {
        f.set_home_notes(ModelRc::default());
        f.set_home_cards_due(0);
        return;
    };
    let modeles = format!("{}/", s.config.templates);
    let mut notes: Vec<(i64, String)> = s
        .notes()
        .into_iter()
        .filter(|n| !n.starts_with(&modeles))
        .map(|n| (s.modified(&n).unwrap_or(0), n))
        .collect();
    notes.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    let aujourd = chrono::Local::now().date_naive();
    let items: Vec<iris_ui::HomeItemData> = notes
        .into_iter()
        .take(4)
        .map(|(t, rel)| {
            use chrono::TimeZone;
            let quand = chrono::Local.timestamp_millis_opt(t).single();
            let meta = match quand {
                Some(q) if q.date_naive() == aujourd => q.format("%H:%M").to_string(),
                Some(q) => q.format("%-d %b").to_string(),
                None => String::new(),
            };
            iris_ui::HomeItemData {
                key: rel.as_str().into(),
                title: stem(&rel).into(),
                meta: meta.into(),
                ..Default::default()
            }
        })
        .collect();
    let dues = s.review().values().filter(|c| c.due <= aujourd).count();
    f.set_home_notes(ModelRc::new(VecModel::from(items)));
    f.set_home_cards_due(dues as i32);
}

/// What an event's note is made from.
#[derive(Debug, Clone)]
pub struct Lecture {
    pub title: String,
    pub day: chrono::NaiveDate,
    pub place: String,
    pub uid: String,
}

/// A name as the vault makes it safe for Windows.
fn nom_sur(nom: &str) -> String {
    let propre: String = nom
        .trim()
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    propre.trim().trim_end_matches(['.', ' ']).to_string()
}

/// An event's note, in Notes: in a folder named after the event ("Analyse"), named
/// after it and its day ("Analyse — 7 Oct"), made from the space's *Cours* template
/// the first time (with the day, the place and the event), opened again after.
pub fn lecture_note(f: &AppWindow, l: &Lecture) {
    let Some(etat) = ETAT.with(|e| e.borrow().as_ref().cloned()) else {
        return;
    };
    let Ok(mut guard) = etat.try_borrow_mut() else {
        return;
    };
    let e: &mut Etat = &mut guard;
    let titre = if l.title.trim().is_empty() {
        "Event".to_string()
    } else {
        l.title.trim().to_string()
    };
    let dossier = nom_sur(&titre);
    let nom = nom_sur(&format!("{titre} — {}", l.day.format("%-d %b %Y")));
    let rel = format!("{dossier}/{nom}.md");
    f.set_workspace(4);
    f.invoke_workspace_changed(4);
    let Some(espace) = e.espace() else { return };
    if espace.modified(&rel).is_some() {
        ouvrir(f, e, &rel);
        montrer_arbre(f, e);
        return;
    }
    let modele = espace
        .templates()
        .into_iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("cours"))
        .and_then(|(_, r)| espace.read(&r).ok())
        .map(|(t, _)| t)
        .unwrap_or_else(|| "# {{title}}\n\n{{cursor}}\n".into());
    let (mut texte, mut curseur) = iris_notes::template::expand(
        &modele,
        &iris_notes::template::Values {
            title: nom.clone(),
            date: l.day.format("%Y-%m-%d").to_string(),
            time: chrono::Local::now().format("%H:%M").to_string(),
            course: titre.clone(),
        },
    );
    // The event and its place, among the properties.
    let mut ajout = format!("event: {}\n", l.uid);
    if !l.place.trim().is_empty() {
        ajout.push_str(&format!("place: {}\n", l.place.trim()));
    }
    if let Some(reste) = texte.strip_prefix("---\n") {
        texte = format!("---\n{ajout}{reste}");
    } else {
        texte = format!("---\n{ajout}---\n{texte}");
    }
    curseur += if texte.starts_with(&format!("---\n{ajout}---\n")) {
        ajout.len() + 8
    } else {
        ajout.len()
    };
    match espace.create_note(&dossier, &nom, &texte) {
        Ok(rel) => {
            e.expanded.insert(parent_of(&rel));
            ouvrir(f, e, &rel);
            if let Some(note) = &e.note {
                let (bloc, local) = block::locate(&note.blocks, curseur.min(note.text.len()));
                focaliser(f, e, bloc, local, local);
                rendre(f, e, false);
            }
            montrer_arbre(f, e);
        }
        Err(err) => f.set_status(format!("Could not make the note: {err}").into()),
    }
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
        focus_mode: false,
        revision: None,
        beside: None,
        sheet: None,
        services: services.clone(),
        historique: None,
    }));
    sheet::wire(f, &etat);
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
        fermer_tout(&f, e);
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
                fermer_tout(&f, e);
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
                fermer_tout(&f, e);
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
            Some(EntryKind::Note | EntryKind::Sheet) => ouvrir(&f, e, &k),
            Some(EntryKind::Image | EntryKind::Other) => {
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
                let teinte = e
                    .espace()
                    .and_then(|s| s.config.folder_colors.get(&cle).cloned())
                    .and_then(|h| {
                        COULEURS_DOSSIERS
                            .iter()
                            .position(|x| x.eq_ignore_ascii_case(&h))
                    })
                    .map_or(-1, |i| i as i32);
                f.set_notes_row_menu_colour(teinte);
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
            "beside" => {
                e.beside = Some(cle);
                montrer_a_cote(&f, e);
            }
            "new-sheet" => {
                let dossier = if genre == EntryKind::Folder {
                    cle.clone()
                } else {
                    parent_of(&cle)
                };
                if let Some(rel) = sheet::nouvelle(&f, e, &dossier) {
                    if !dossier.is_empty() {
                        e.expanded.insert(dossier);
                    }
                    ouvrir(&f, e, &rel);
                    montrer_arbre(&f, e);
                    f.set_notes_renaming(rel.as_str().into());
                }
            }
            "import-sheet" => {
                if let Some(rel) = sheet::importer(&f, e, &cle) {
                    ouvrir(&f, e, &rel);
                    montrer_arbre(&f, e);
                }
            }
            "history" => historique(&f, e, &cle),
            "revise" => reviser_dossier(&f, e, &cle),
            a if a.starts_with("colour:") => {
                let choisie = a
                    .strip_prefix("colour:")
                    .and_then(|n| n.parse::<usize>().ok())
                    .and_then(|i| COULEURS_DOSSIERS.get(i));
                if let Some(s) = e.espace_mut() {
                    match choisie {
                        Some(h) => {
                            s.config.folder_colors.insert(cle.clone(), h.to_string());
                        }
                        None => {
                            s.config.folder_colors.remove(&cle);
                        }
                    }
                    if let Err(err) = s.save_config() {
                        f.set_status(format!("Could not keep the colour: {err}").into());
                    }
                }
                montrer_arbre(&f, e);
            }
            "export" => exporter(&f, e, &cle, false),
            "print" => exporter(&f, e, &cle, true),
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
                let dedans = |rel: &str| rel == cle || rel.starts_with(&format!("{cle}/"));
                let ouverte_dedans = e.note.as_ref().is_some_and(|n| dedans(&n.rel));
                if ouverte_dedans {
                    e.note = None;
                    sans_focus(&f, e);
                    montrer_note(&f, e);
                }
                if e.sheet.as_ref().is_some_and(|s| dedans(&s.rel)) {
                    e.sheet = None;
                    f.set_note_sheet_open(false);
                }
                if e.beside.as_deref().is_some_and(dedans) {
                    e.beside = None;
                    montrer_a_cote(&f, e);
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

    f.set_notes_folder_palette(ModelRc::new(VecModel::from(
        COULEURS_DOSSIERS
            .iter()
            .map(|h| crate::calendar::couleur(h))
            .collect::<Vec<_>>(),
    )));

    // Settings › Notes: where the spaces are.
    f.set_notes_folder(etat.borrow().vault.root().display().to_string().into());
    geste!(on_notes_folder_open, |f, e| {
        if let Err(err) = crate::platform::open_path(e.vault.root()) {
            f.set_status(format!("Could not open the folder: {err}").into());
        }
    });
    geste!(on_notes_folder_change, |f, e| {
        let Some(dossier) = rfd::FileDialog::new()
            .set_title("Where your notes' spaces go")
            .pick_folder()
        else {
            return;
        };
        match Vault::open(dossier.clone()) {
            Ok(v) => {
                fermer_tout(&f, e);
                e.vault = v;
                let chemin = dossier.display().to_string();
                crate::settings::update(|s| s.notes_root = chemin.clone());
                e.space = 0;
                e.expanded.clear();
                e.selected = None;
                e.fingerprint = 0;
                e.tags = None;
                charger_espaces(e);
                montrer_note(&f, e);
                montrer_espaces(&f, e);
                montrer_arbre(&f, e);
                f.set_notes_folder(chemin.into());
                f.set_toast("Your notes' spaces are now in this folder.".into());
            }
            Err(err) => f.set_status(format!("Could not use this folder: {err}").into()),
        }
    });

    // A note picked on Home.
    {
        let faible = f.as_weak();
        f.on_home_note_opened(move |k| {
            let Some(f) = faible.upgrade() else { return };
            f.set_workspace(4);
            f.invoke_workspace_changed(4);
            f.invoke_notes_quick_chosen(k);
        });
    }

    // A conversation, an event, a task: a link to it copied, to paste in a note.
    {
        let (services, faible) = (services.clone(), f.as_weak());
        f.on_thread_copy_link(move |id| {
            let Some(f) = faible.upgrade() else { return };
            let sujet = services
                .store
                .thread_row(iris_types::ThreadId(i64::from(id)))
                .ok()
                .flatten()
                .map(|l| l.subject)
                .unwrap_or_default();
            copier_lien(&f, &format!("mail:{id}"), &sujet);
        });
    }
    {
        let faible = f.as_weak();
        f.on_event_copy_link(move || {
            let Some(f) = faible.upgrade() else { return };
            let d = f.get_event_detail();
            copier_lien(&f, &format!("event:{}", d.key), &d.title);
        });
    }
    {
        let faible = f.as_weak();
        f.on_task_copy_link(move || {
            let Some(f) = faible.upgrade() else { return };
            let id = f.get_task_detail().id;
            copier_lien(&f, &format!("task:{id}"), &f.get_task_detail_title());
        });
    }
    geste!(on_notes_new_sheet, |f, e| {
        let dossier = e.dossier_courant();
        if let Some(rel) = sheet::nouvelle(&f, e, &dossier) {
            if !dossier.is_empty() {
                e.expanded.insert(dossier);
            }
            ouvrir(&f, e, &rel);
            montrer_arbre(&f, e);
            f.set_notes_renaming(rel.as_str().into());
        }
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
            recents.sort_by_key(|a| std::cmp::Reverse(a.0));
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
        let a_cote = f.get_notes_quick_beside();
        f.set_notes_quick_beside(false);
        match k.strip_prefix("new:") {
            Some(nom) => nouvelle_note(&f, e, nom, ""),
            // Ctrl+\: beside the note open, to read while writing.
            None if a_cote && EntryKind::of(&k) == EntryKind::Note => {
                e.beside = Some(k.to_string());
                montrer_a_cote(&f, e);
            }
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
        // A spreadsheet embedded: a click opens it (the arrows reach its line).
        if let Some((BlockKind::Embed { target }, _)) = contenu(e, i) {
            if render::est_tableur(&target) {
                let nom = target
                    .split(['|', '#'])
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let rel = e
                    .espace()
                    .and_then(|s| render::embedded_file(&nom, s.dir()).and_then(|p| s.rel(&p)));
                if let Some(rel) = rel {
                    ouvrir(&f, e, &rel);
                    montrer_arbre(&f, e);
                    return;
                }
            }
        }
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
            cocher_tache(e, &n);
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
    geste!(on_note_focus_mode_toggled, |f, e| {
        mode_focus(&f, e);
    });
    geste!(on_notes_graph_requested, |f, e| {
        graphe(&f, e);
    });
    geste!(on_notes_graph_chosen, |f, e, k| {
        let k = k.to_string();
        ouvrir(&f, e, &k);
        montrer_arbre(&f, e);
    });
    geste!(on_notes_history_picked, |f, e, i| {
        let Some((rel, temps)) = &e.historique else {
            return;
        };
        let Some(t) = temps.get(i.max(0) as usize).copied() else {
            return;
        };
        let texte = e
            .espace()
            .and_then(|s| s.read_version(rel, t).ok())
            .unwrap_or_default();
        f.set_notes_history_preview(texte.into());
        f.set_notes_history_chosen(i);
    });
    geste!(on_notes_history_restore, |f, e| {
        let Some((rel, temps)) = e.historique.take() else {
            return;
        };
        let Some(t) = temps
            .get(f.get_notes_history_chosen().max(0) as usize)
            .copied()
        else {
            return;
        };
        let Some(Ok(texte)) = e.espace().map(|s| s.read_version(&rel, t)) else {
            return;
        };
        if e.note.as_ref().map(|n| n.rel.as_str()) != Some(rel.as_str()) {
            ouvrir(&f, e, &rel);
        }
        if e.note.as_ref().map(|n| n.rel.as_str()) != Some(rel.as_str()) {
            return;
        }
        // Like an edit: undone with Ctrl+Z, and what it replaces kept in the history.
        retenir(e, true);
        appliquer(
            &f,
            e,
            NoteEdit {
                text: texte,
                block: 0,
                cursor: 0,
            },
            false,
            None,
        );
        sans_focus(&f, e);
        rendre(&f, e, true);
        f.set_toast(format!("The version of {} is back.", quand(t)).into());
    });
    geste!(on_note_beside_closed, |f, e| {
        e.beside = None;
        montrer_a_cote(&f, e);
    });
    geste!(on_note_revise, |f, e| {
        reviser(&f, e);
    });
    geste!(on_notes_review_reveal, |f, e| {
        if let Some(r) = &mut e.revision {
            r.revealed = true;
        }
        montrer_revision(&f, e);
    });
    geste!(on_notes_review_graded, |f, e, g| {
        noter(&f, e, g);
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
    fn links_to_the_rest_of_iris() {
        assert_eq!(lien_iris("mail:12"), Some(("mail", "12")));
        assert_eq!(lien_iris("Task: 7"), Some(("task", "7")));
        assert_eq!(
            lien_iris("event:abc:1760000000000"),
            Some(("event", "abc:1760000000000"))
        );
        assert_eq!(lien_iris("Chapitre 1"), None);
        assert_eq!(lien_iris("mail:"), None);
        assert_eq!(lien_iris("http://x"), None);
        assert_eq!(
            lien_ecrit("mail:3", "Re: [devis] | v2"),
            "[[mail:3|Re: devis v2]]"
        );
        assert_eq!(lien_ecrit("task:9", "  "), "[[task:9]]");
        assert_eq!(
            tache_de_ligne("- [ ] Rendre le TD [[task:12]]"),
            Some((12, false))
        );
        assert_eq!(
            tache_de_ligne("  - [x] Fait [[task:3|ici]]"),
            Some((3, true))
        );
        assert_eq!(tache_de_ligne("- [ ] Sans lien"), None);
    }

    #[test]
    fn colour_keys_on_both_keyboards() {
        assert_eq!(couleur_de_touche("1"), Some(Colour::Red));
        assert_eq!(couleur_de_touche("&"), Some(Colour::Red));
        assert_eq!(couleur_de_touche("è"), Some(Colour::Grey));
        assert_eq!(couleur_de_touche("9"), None);
    }
}
