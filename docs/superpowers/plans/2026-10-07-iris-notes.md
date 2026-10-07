# Iris Notes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A fifth place in Iris — notes in Markdown files organised in spaces, written
fast with a short syntax, live preview, LaTeX, links, and spreadsheets — shipped in
phases as releases 4.14.0 onwards.

**Architecture:** Pure crates hold the logic (`iris-notes` for blocks, inline marks,
links and course features; `iris-math` for LaTeX images; `iris-sheets` for the
spreadsheet engine); `iris-vault` holds the file system (spaces, tree, watching,
Recycle Bin, index). `iris-app/src/notes/` wires them to new Slint screens built from
the shared components. The editor is a virtual list of blocks: the focused block is a
`TextInput` with its source, every other block a `StyledText` rendering.

**Tech Stack:** Rust 2021 workspace, Slint 1.17 (software renderer, `StyledText`,
`TextInput` with `key-pressed`, `set-selection-offsets`, `cursor-position-changed`),
`latex-rust` (LaTeX → PNG, STIX Two Math embedded), `notify` (watching), `trash`
(Recycle Bin), `calamine` / `rust_xlsxwriter` (spreadsheets), Blitz (reading mode).

**Spec:** `docs/superpowers/specs/2026-10-07-iris-notes-design.md`

## Global Constraints

- Never run tests, clippy or builds locally: `cargo fmt --all`, commit, push, read CI
  (`gh run list -R Alicienn/iris`, `gh run view <id> --log-failed`). "Run the test"
  steps below mean: push and read CI.
- CI runs clippy with `-D warnings` (type_complexity, too_many_arguments,
  collapsible_if, redundant_closure_call, cloned_ref_to_slice_refs…): alias complex
  types, keep functions ≤ 7 arguments, no immediately-invoked closures.
- Every user-facing change: `CHANGELOG.md` (English, one line per change, New /
  Improved / Fixed / Removed) and a version bump in `[workspace.package]` and the
  `iris-*` entries of `Cargo.lock`; docs updated in the same commit (README,
  ARCHITECTURE, PLAN, COMPARISON, spec's *What changed since*).
- English only in the repository; test data uses `example.com`; never commit a real
  mailbox, account, note or screenshot of one.
- UI: shared components only (`Button`, `Link`, `Modal`, `Popover`, `NavItem`…); every
  modal/panel/menu joins `modal-open` and the Escape chain in `app.slint`; text fields
  use `controls/field.slint`; no `Ctrl+Alt` shortcut (AltGr on AZERTY); typing accepts
  every character; closing hands the keyboard back to `shortcuts.focus()`.
- New callbacks are wired in a file listed in `crates/iris-app/tests/cablage.rs`.
- Syntax decided by the owner: underline `__x__`, strikethrough `--x--`.

## Review Focus

- A file edited in Obsidian or OneDrive while open: reload when not edited here, keep
  both when edited on both sides — never lose a keystroke (Task A4 tests conflicts).
- Exact round-trip: opening and saving a note never changes bytes it did not edit
  (CRLF files, trailing spaces, no final newline, odd Markdown) — Task A1 tests a
  corpus with these.
- Marks inside words and maths: `a -- b`, `2*3*4`, `snake_case_name`, `$a*b$`, URLs
  with `__` or `--` stay plain — Task A2 tests each.
- Renaming a note whose name others mention in plain text, or that shares its name
  with a note in another folder: only real links change, ambiguous ones keep a path —
  Task B5 tests both.
- A huge note (5,000 lines) or a 2 MB pasted text: typing stays responsive (the model
  re-parses only the focused block unless structure changes) — Task A6 measures
  parse time in a test (< 20 ms for 5,000 lines).

---

## Phase A — Foundations (release 4.14.0)

### Task A1: `iris-notes` blocks — exact parse and join

**Files:**
- Create: `crates/iris-notes/Cargo.toml`, `crates/iris-notes/src/lib.rs`,
  `crates/iris-notes/src/block.rs`
- Modify: `Cargo.toml` (workspace member dependency `iris-notes`)

**Interfaces:**
- Produces:
  ```rust
  pub struct Block { pub kind: BlockKind, pub text: String } // exact source, with its line endings
  pub enum BlockKind { Blank, Paragraph, Heading(u8), Bullet { indent: u8 },
      Numbered { indent: u8, n: u32 }, Task { indent: u8, done: bool }, Quote,
      Callout { kind: String, folded: Option<bool>, title: String }, Math, Code { lang: String },
      Rule, Table, Embed { target: String }, Container { kind: String, title: String },
      Properties, Flashcard }
  pub fn parse(text: &str) -> Vec<Block>;
  pub fn join(blocks: &[Block]) -> String;          // join(parse(x)) == x for every x
  impl Block { pub fn content(&self) -> &str;        // text without its final line ending
               pub fn line_ending(&self) -> &str; }
  ```
- [x] Step 1: tests — round trip over a corpus (LF, CRLF, no final newline, trailing
  spaces, nested lists, fenced code containing `#`, `$$` blocks, callouts with several
  `>` lines, tables, `:::` containers, YAML front matter only at offset 0, blank runs),
  and the kind of each block.
- [x] Step 2: implement the line classifier and the multi-line groupings (front
  matter, fenced code, `$$`, callouts/quotes, tables, `:::` with nesting).
- [x] Step 3: fmt, commit, push, read CI.

### Task A2: inline marks

**Files:** Create `crates/iris-notes/src/inline.rs`

**Interfaces:**
- Produces:
  ```rust
  pub enum Mark { Bold, Italic, Underline, Strike, Highlight(Option<Colour>), Colour(Colour),
                  Sup, Sub, Code, Math, NoteLink { target: String, label: Option<String> },
                  WebLink { url: String }, Tag(String), Date(chrono::NaiveDate), Footnote(String) }
  pub enum Colour { Red, Orange, Yellow, Green, Blue, Purple, Grey }
  pub struct Span { pub text: String, pub marks: Vec<Mark>, pub range: std::ops::Range<usize> }
  pub fn parse_inline(src: &str) -> Vec<Span>;
  pub fn to_slint_markdown(spans: &[Span], palette: &Palette) -> String; // only Slint's subset
  pub fn to_html(spans: &[Span]) -> String;
  pub struct Palette { pub colours: [u32; 7], pub highlight: u32, pub link: u32, pub tag: u32 }
  ```
- [x] Step 1: tests for every mark, nesting (`**__x__**`), escapes (`\*`), the
  non-conflicts of the Review Focus, Slint markdown output (escaping `<`, `*`, `[`),
  HTML output escaping.
- [x] Step 2: implement a single-pass scanner with a mark stack; flanking rules.
- [x] Step 3: fmt, commit, push, read CI.

### Task A3: editing operations

**Files:** Create `crates/iris-notes/src/edit.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct Edit { pub text: String, pub cursor: usize, pub anchor: usize }
  pub fn toggle_mark(text: &str, anchor: usize, cursor: usize, mark: &str) -> Edit; // "**", "*", "__", "--", "==", "`", "$", "^", "~"
  pub fn set_colour(text: &str, anchor: usize, cursor: usize, colour: Option<Colour>) -> Edit;
  pub fn line_start_conversion(text: &str, cursor: usize) -> Option<Edit>; // "[] "→"- [ ] ", ">def "→"> [!def] ", ">-thm "→"> [!thm]- "
  pub fn continue_on_enter(block: &str, cursor: usize) -> (String, String); // (stays, goes to the new block): lists, tasks, quotes
  pub fn cycle_heading(text: &str) -> String;
  pub fn indent(text: &str, out: bool) -> String;
  pub fn toggle_task(text: &str) -> String;
  pub fn auto_pair(text: &str, cursor: usize, typed: char) -> Option<Edit>;
  ```
- [x] Step 1: tests for each (selection wrap and unwrap, empty selection inserting a
  pair with the cursor inside, list numbering continuing, empty item ending a list).
- [x] Step 2: implement; fmt, commit, push, read CI.

### Task A4: `iris-vault` — spaces, tree, atomic files, watching, Recycle Bin

**Files:** Create `crates/iris-vault/{Cargo.toml,src/lib.rs,src/space.rs,src/tree.rs,src/files.rs,src/watch.rs}`

**Interfaces:**
- Produces:
  ```rust
  pub struct Vault { root: PathBuf }                       // %USERPROFILE%\Iris or IRIS_ROOT/notes
  impl Vault { pub fn open(root: PathBuf) -> Result<Self>; pub fn spaces(&self) -> Result<Vec<Space>>;
               pub fn create_space(&self, name: &str) -> Result<Space>; pub fn add_external(&self, path: &Path) -> Result<Space>; }
  pub struct Space { pub dir: PathBuf, pub config: SpaceConfig }
  pub struct SpaceConfig { pub name: String, pub color: String, pub icon: String, pub attachments: String,
                           pub templates: String, pub pinned: Vec<String>, pub folder_colors: BTreeMap<String,String>, pub order: BTreeMap<String, Vec<String>> }
  pub enum EntryKind { Folder, Note, Sheet, Image, Other }
  pub struct Entry { pub rel: String, pub name: String, pub kind: EntryKind, pub depth: usize, pub modified: i64 }
  impl Space { pub fn tree(&self, expanded: &HashSet<String>) -> Result<Vec<Entry>>;
      pub fn read(&self, rel: &str) -> Result<String>;
      pub fn write(&self, rel: &str, text: &str, expected_modified: Option<i64>) -> Result<WriteOutcome>; // Written(modified) | Conflict(saved_as)
      pub fn create_note(&self, folder: &str, name: &str, text: &str) -> Result<String>;
      pub fn create_folder(&self, parent: &str, name: &str) -> Result<String>;
      pub fn rename(&self, rel: &str, new_name: &str) -> Result<String>;
      pub fn move_to(&self, rel: &str, folder: &str) -> Result<String>;
      pub fn delete(&self, rel: &str, bin: &dyn RecycleBin) -> Result<()>;
      pub fn save_config(&self) -> Result<()>; }
  pub trait RecycleBin { fn throw(&self, path: &Path) -> Result<()>; }   // `trash` in the app, a fake in tests
  pub fn watch(dir: &Path, on_change: impl Fn(Vec<PathBuf>) + Send + 'static) -> Result<Watcher>;
  ```
- [x] Step 1: tests with `tempfile`: space created with `.iris/space.json`, `_Fichiers`,
  `_Modèles`; tree order (folders first, then by name; hidden `.iris` skipped);
  atomic write leaves no `.tmp`; write with a stale `expected_modified` saves a
  conflict copy; rename/move collision gets " 2"; delete calls the bin.
- [x] Step 2: implement; fmt, commit, push, read CI.

### Task A5: the place — workspace 4, side column, tree

**Files:**
- Create: `crates/iris-ui/ui/screens/notes.slint` (NotesView: SidebarFrame with space picker, Recent, tree; page), `crates/iris-app/src/notes/mod.rs`, `crates/iris-app/src/notes/tree.rs`
- Modify: `crates/iris-ui/ui/app.slint` (workspace 4, Ctrl+4, properties/callbacks), `crates/iris-ui/ui/shell/chrome.slint` (PlaceTab Notes), `crates/iris-ui/ui/types.slint` (`NoteTreeRowData`, `NoteSpaceData`), `crates/iris-app/src/main.rs`, `crates/iris-app/src/nav.rs` (`Place::Note`), `crates/iris-app/src/settings.rs` (`notes_root`, `notes_spaces_external`, `notes_last`), `crates/iris-app/tests/cablage.rs`

**Interfaces:**
- Produces Slint: `notes-spaces: [NoteSpaceData]`, `notes-space: int`, `notes-tree: [NoteTreeRowData]`, callbacks `notes-space-chosen(int)`, `notes-row-clicked(string)`, `notes-row-toggled(string)`, `notes-row-menu(string, length, length)`, `notes-new-note()`, `notes-new-folder()`, `notes-move(string, string)`, `notes-rename(string, string)`, `notes-delete(string)`.
  `struct NoteTreeRowData { key: string, name: string, depth: int, kind: int /*0 folder 1 note 2 sheet 3 image 4 other*/, expanded: bool, selected: bool, color: color, pinned: bool }`
- [x] Step 1: saisie scenario — Ctrl+4 shows Notes; typing in the tree filter reaches the field; Escape closes the row menu.
- [x] Step 2: implement the place, the tree (expand/collapse, select, rename inline, menu, drag to move with the TouchArea carry pattern of the calendar's *To plan*), spaces (create, choose, open folder via `rfd`).
- [x] Step 3: fmt, commit, push, read CI.

### Task A6: the block editor

**Files:**
- Create: `crates/iris-ui/ui/screens/notes-editor.slint` (`NoteEditor`, `NoteBlock`), `crates/iris-app/src/notes/editor.rs`
- Modify: `types.slint` (`NoteBlockData`)

**Interfaces:**
- `struct NoteBlockData { kind: string, level: int, indent: int, source: string, rich: styled-text, done: bool, number: string, title: string, lang: string, picture: image, has-picture: bool, folded: bool }`
- Slint: `note-blocks: [NoteBlockData]`, `note-focus: int`, `note-focus-cursor: int`, `note-focus-serial: int` (bumped to ask the delegate to take focus), callbacks `note-block-edited(int, string, int)`, `note-block-key(int, string /*key name*/, int /*cursor*/, int /*anchor*/, bool /*first line*/, bool /*last line*/) -> bool`, `note-block-clicked(int)`, `note-link-clicked(string)`, `note-title-edited(string)`.
- Rust: `struct OpenNote { space: usize, rel: String, text: String, blocks: Vec<Block>, modified: i64, undo: Vec<String>, redo: Vec<String>, dirty: bool }`; `fn render_block(&Block, &Palette) -> NoteBlockData`; save debounced 500 ms with `slint::Timer`.
- [x] Step 1: tests in `iris-notes` for the model operations used by the editor (`replace_block`, `split_block`, `join_with_previous`, `move_block`) keeping `join` exact; a 5,000-line parse under 20 ms.
- [x] Step 2: saisie scenario — a new note: typing "Hello" lands in the first block; Enter makes a second block; Backspace at its start joins.
- [x] Step 3: implement the editor: focus model, key handling (Enter, Backspace, Up/Down at edges, Tab, Alt+Up/Down, Ctrl+D, Ctrl+B/I/U/E, Ctrl+Shift+X/H, Ctrl+H, Ctrl+Enter, Ctrl+Z/Y), line-start conversions, autosave, title = file name (rename on title edit), reload on outside change.
- [x] Step 4: fmt, commit, push, read CI.

### Task A7: quick open, settings, docs, release 4.14.0

**Files:** `crates/iris-app/src/notes/search.rs`, `crates/iris-notes/src/fuzzy.rs`, `screens/notes.slint` (QuickOpen modal), `screens/settings.slint` (Notes group: folder, open folder), docs, CHANGELOG, version.
- [x] Step 1: tests for fuzzy ranking (prefix > word start > subsequence; accents folded).
- [x] Step 2: implement Ctrl+O; Settings › Notes; docs; 4.14.0; push; CI green; release.

## Phase B — The whole syntax, links and search (release 4.15.0)

### Task B1: callouts, lists, tasks, quotes, tables, rules, code, containers rendered
- Files: `notes-editor.slint` (block decorations), `notes/editor.rs` (render), `iris-notes/src/callout.rs` (kinds, labels FR/EN, icons, numbering).
- Tests: numbering (Définition 1, Théorème 1, Définition 2), folded state round trip, table parsing to cells.
- [x] Implement, fmt, commit, CI.

### Task B2: colours, highlight and the colour picker; selection bubble
- Files: `notes-editor.slint` (`SelectionBubble`, `ColourPicker` popovers), `iris-notes/src/edit.rs` (`set_colour`, `colour_at`), `notes/editor.rs`.
- Tests: `colour_at(text, offset)` finds the span; `set_colour` rewrites `{r}x{/}` to `{b}x{/}` and to plain for Default; highlight colours.
- [x] Implement (hover 600 ms or click on coloured text opens the picker; Ctrl+Shift+C; Ctrl+Shift+1…7), fmt, commit, CI.

### Task B3: autocompletion popups and the slash menu
- Files: `notes-editor.slint` (`CompletionPopup`), `iris-notes/src/complete.rs` (trigger detection at cursor: `[[`, `#`, `@`, `\`, `/`, `{`; candidates; insertion edit).
- Tests: trigger detection and insertion for each, LaTeX snippet expansion with Tab stops.
- [x] Implement, fmt, commit, CI.

### Task B4: links — resolve, hover cards, backlinks, outline panel
- Files: `iris-notes/src/links.rs`, `iris-vault/src/index.rs`, `notes-editor.slint` (`HoverCard`, right panel), `notes/editor.rs`.
- Tests: resolution (stem unique, path, heading anchor, alias), backlinks with the line, outline.
- [x] Implement, fmt, commit, CI.

### Task B5: rename and move rewrite links
- Files: `iris-notes/src/links.rs` (`rewrite_links(text, old, new) -> Option<String>`), `iris-vault` (`rename` / `move_to` return the notes to rewrite).
- Tests: Review Focus case (same name in two folders; plain text mention untouched; alias and heading kept).
- [x] Implement, fmt, commit, CI.

### Task B6: tags, dates, footnotes, properties; search Ctrl+Shift+F
- Files: `iris-notes/src/meta.rs`, `notes/search.rs`, `notes.slint` (search panel).
- Tests: tag extraction (not in code, not `#` headings, nested), date words FR/EN, search filters.
- [x] Implement, fmt, commit, CI.

### Task B7: images, paste, attachments, templates, docs, release 4.15.0
- Files: `notes/editor.rs` (paste: clipboard image via `arboard`? — use Slint's clipboard text and `rfd` for files; image from clipboard via Windows API through `arboard`), `iris-vault` (save attachment), `iris-notes/src/template.rs`.
- Tests: template expansion, attachment naming.
- [x] Implement; docs; 4.15.0; CI; release.

## Phase C — LaTeX and reading mode (release 4.16.0)

### Task C1: `iris-math`
- Files: `crates/iris-math/{Cargo.toml,src/lib.rs}` — `pub fn render(latex: &str, display: bool, size_px: f32, colour: [u8;3]) -> Result<RgbaImage>` with an LRU cache; `crates/iris-notes/src/math.rs` — `latex_to_unicode(&str) -> String`, snippets.
- Tests: a fraction renders a non-empty image; errors become a red message image; Unicode conversion of a corpus.
- [x] Implement, fmt, commit, CI.

### Task C2: maths in the editor
- Display maths blocks as images; inline maths as Unicode in StyledText; hover card with the true rendering; floating live preview while typing in `$…$` / `$$`.
- [x] Implement, fmt, commit, CI.

### Task C3: reading mode and HTML
- `iris-notes/src/html.rs` (`note_to_html(blocks, resolver) -> String` with inline maths as `<img src="data:image/png;base64,…">`), editor's eye toggle Ctrl+R rendering through `HtmlRenderer` tiles.
- [x] Implement; docs; 4.16.0; CI; release.

## Phase D — Courses (release 4.17.0)

### Task D1: lecture notes from the calendar
- Event panel *Take notes* / *Open notes*; template *Cours*; properties; link to the event.
- [x] Implement, tests on naming and template, CI.

### Task D2: flashcards and revision
- `iris-notes/src/review.rs` (cards from `::` and `>q`, SM-2), `.iris/review.json`, Revision modal, Home count.
- [x] Implement, tests on SM-2 intervals, CI.

### Task D3: split view, focus mode, word count, export HTML/PDF; docs; 4.17.0
- [x] Implement; docs; release.

## Phase E — Spreadsheets (release 4.18.0)

### Task E1: `iris-sheets` engine
- `src/cell.rs` (addresses), `src/formula.rs` (lexer, parser, AST), `src/eval.rs` (functions, errors, dependency order, cycles), `src/format.rs`, `src/file.rs` (`.sheet` JSON), `src/csv.rs`.
- Tests: every function, cycles, errors, CSV round trip.
- [x] Implement, CI.

### Task E2: the grid view
- `screens/sheet.slint` (virtual grid, header row/column, selection, editor overlay, toolbar: bold/italic/underline/strike, colours, fill, alignment, wrap, number format, borders), `notes/sheet.rs`.
- [x] Implement editing, keyboard, clipboard (TSV), fill, insert/delete rows/cols, sort, status sums, undo; CI.

### Task E3: xlsx, embedding in notes; docs; 4.18.0
- `calamine` import, `rust_xlsxwriter` export; `![[x.sheet]]` table block.
- [x] Implement; docs; release.

## Phase F — Links with Iris and extras (release 4.19.0)

### Task F1: Iris links and checkbox → task
- [x] `[[mail:]]`/`[[event:]]`/`[[task:]]` open; *Copy link to note*; *Make a task* from a checkbox, ticked both ways.

### Task F2: history, graph, Home widget; docs; 4.19.0
- [x] Versions in `.iris/history`, restore; graph view; Home *Notes* widget; release.
