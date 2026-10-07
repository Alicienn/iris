# Iris Notes — design

**Date:** 2026-10-07 · **Status:** approved by the owner ("implement everything") ·
**Ships as:** 4.14.0 onwards, one release per phase.

A fifth place in Iris, beside Home, Mail, Calendar and Tasks: notes in Markdown files
the user owns, organised in spaces and folders, linked to one another and to the rest
of Iris, written fast with a short syntax and shortcuts, with LaTeX for courses, and
spreadsheets beside them.

## 1. Goals

- Notes for classes and work, **as fast to write as on paper**: every mark is short,
  converts as it is typed, and has a shortcut.
- **The files are the user's**: plain Markdown in a visible folder, readable by any
  editor, openable in Obsidian, syncable by putting the folder in OneDrive.
- **Integrated**: the same components, themes, keyboard rules and Back/Forward as the
  rest of Iris; links to mail, events and tasks.
- **Spreadsheets** (`.sheet`) with formulas and formatting, embeddable in a note.

Non-goals for now: collaboration, a mobile app, handwriting, plugins for notes.

## 2. Files and spaces

```
%USERPROFILE%\Iris\                 the root (Settings › Notes › Folder)
├── Cours\                          a space = a folder
│   ├── .iris\space.json            name, colour, icon, settings (hidden)
│   ├── _Fichiers\                  pasted images and files
│   ├── _Modèles\                   templates (*.md)
│   └── Analyse\Chapitre 1.md
└── Perso\
```

- The root is visible on purpose: notes are documents; hidden folders are skipped by
  backup and sync tools. `IRIS_ROOT` puts it under `<root>/notes` (tests, measures).
- A **space** is any folder: those under the root are listed automatically; one
  elsewhere (an Obsidian vault, a OneDrive folder) is added by path and remembered in
  the settings. Removing a space from Iris never deletes its files.
- `space.json`: `{ "name", "color", "icon", "attachments": "_Fichiers",
  "templates": "_Modèles", "pinned": [paths], "folder_colors": {path: color} }`.
- **Writes are atomic** (temporary file, then rename) and **debounced** (500 ms after
  the last key, and on leaving the note). A note changed on disk while open and not
  edited here reloads; changed on both sides, the other version is kept beside it as
  `Name (conflict 2026-10-07 14.32).md`.
- **Watching**: the space is watched (`notify`); the tree and the index follow files
  made, changed, moved or deleted elsewhere.
- **Deleting** sends to the Windows Recycle Bin (`trash`), never destroys.
- **Index** in memory, per space: for each note its title, headings, links, tags,
  dates, first line, modified time. Full text is searched by reading the files on
  demand (a few thousand notes read in well under a second); nothing is cached on disk.
- Backups (4.12.0) are untouched: notes are files, and the user's own folder.

## 3. The syntax

Three layers: CommonMark + GFM; Obsidian's extensions as they are; Iris's own, short,
degrading to readable text elsewhere. Departures from Markdown chosen by the owner:
`__x__` is **underline** (not bold) and `--x--` is **strikethrough**.

### 3.1 Inline

| Effect | Typed | Shortcut |
|---|---|---|
| Bold | `**x**` | Ctrl+B |
| Italic | `*x*` | Ctrl+I |
| Underline | `__x__` | Ctrl+U |
| Strikethrough | `--x--` (also `~~x~~` read) | Ctrl+Shift+X |
| Highlight | `==x==`, coloured `=={r}x==` | Ctrl+Shift+H |
| Text colour | `{r}x{/}` — r red, o orange, y yellow, g green, b blue, p purple, n grey; full names (`{red}`) also read | Ctrl+Shift+1…7, or the colour picker |
| Superscript / subscript | `x^2^`, `H~2~O` | Ctrl+. / Ctrl+, |
| Inline maths | `$…$` | Ctrl+M |
| Inline code | `` `x` `` | Ctrl+E |
| Link to a note | `[[Note]]`, `[[Note#Heading]]`, `[[Note\|text]]` | typing `[[` |
| Web link | `[text](url)`; a URL pasted over a selection | Ctrl+L |
| Footnote | `^[text]` | — |
| Tag | `#tag`, nested `#maths/analyse` | — |
| Date | `@2026-10-08`; `@today`, `@tomorrow`, `@monday`, `@aujourdhui`, `@demain` complete to a date | — |
| Iris links | `[[mail:<message-id>]]`, `[[event:<uid>]]`, `[[task:<id>]]` | from the menus |

Rules: a mark needs a non-space character on both inner sides (`a -- b` stays a dash,
`2 * 3` stays a product); `\` escapes any mark; marks do not span blocks. `--x--`
never conflicts with `---` (a rule is a line of its own).

### 3.2 Blocks (typed at the start of a line; converted when the space is typed)

| Typed | Block |
|---|---|
| `# ` … `###### ` | Heading 1–6 (Ctrl+H cycles H1→H2→H3→paragraph) |
| `- ` or `* `, `1. ` | Bullet, numbered item (Tab / Shift+Tab nest) |
| `[] ` or `- [ ] ` | Checkbox (Ctrl+Enter ticks) |
| `> ` | Quote |
| `>def `, `>thm `, `>prop `, `>lem `, `>cor `, `>preuve `/`>proof `, `>ex `, `>exo `, `>sol `, `>q `, `>! `, `>res `, `>note `, `>tip `, `>warn ` | Callout, stored `> [!def] Title` (Obsidian's form); def, thm, prop, lem, cor and exo numbered per note ("Théorème 3") |
| `>-thm ` (any type with `-`) | The same callout, folded (`> [!thm]-`) |
| `$$` | Display maths block, up to the closing `$$` |
| ```` ``` ```` + language | Code block |
| `---` | Rule |
| `\|a\|b\|` | Markdown table |
| `:::cols` … `:::` (columns split by `:::col`) | Columns |
| `:::fold Title` … `:::` | Foldable block |
| `Question :: Answer` | Flashcard |
| `![[file]]`, `![[img.png\|50%]]`, `![[Budget.sheet]]` | Embedded image, note or spreadsheet |
| `/` | The block menu, filtered as typed (`/thm`, `/table`, `/math`, `/sheet`…) |
| `---` at the very top | YAML properties (title, tags, course, date, prof) |

### 3.3 Typing aids

- Lists continue on Enter; Enter on an empty item ends the list.
- Pairs close themselves: `( [ { $ ** == __`.
- Autocompletion popups, placed at the cursor: `[[` notes then `#` headings; `#` tags;
  `@` dates; `\` LaTeX commands; `/` blocks; `{` colours.
- LaTeX snippets inside maths, expanded by Tab: `//`→`\frac{}{}`, `sq`→`\sqrt{}`,
  `vec`→`\vec{}`, `sum`→`\sum_{}^{}`, `int`→`\int_{}^{}`, `lim`→`\lim_{ \to }`,
  `mat2`/`mat3`→a matrix, `cases`, `align`; Tab moves to the next `{}`.
- Paste: an image becomes `_Fichiers\Image 2026-10-07 14.32.png` and its embed; a URL
  over a selection becomes a link; HTML from the clipboard becomes Markdown.

## 4. The editor

### 4.1 Model

`iris-notes` (pure) parses a note into **blocks** without losing a byte: each block
keeps its source slice, and joining the slices gives the file back exactly. A block has
a kind (paragraph, heading(level), bullet(depth), numbered(depth, n),
task(depth, done), quote, callout(kind, folded, title), math, code(lang), rule, table,
image/embed, columns, fold, properties, flashcard, blank) and its source.

### 4.2 Live preview, block by block

The editor is a virtual list of blocks. **The block holding the cursor shows its
source** in a `TextInput` (multi-line, wrapped, in the block's own size: a heading is
large while typed). **Every other block shows its rendering**: inline marks with
Slint's `StyledText` (bold, italic, underline, strikethrough, code, links, colours),
block decorations drawn by Slint (bullets, checkboxes, callout frames with icon and
number, quote bar, rule, table grid, code background), images and display maths as
images. Highlight shows as coloured text in the editor (StyledText has no background)
and as a real background in reading mode. Inline maths shows as Unicode text (`x²`,
`α`, `∫`, `ℝ`, `a/b`) in the preview; hovering it shows the true rendering.

A click on a rendered block puts its source in the input, cursor at the end (a click
on a checkbox ticks, on a link opens, on a fold arrow folds). Up/Down at the first or
last line move to the neighbour block; Enter splits, Backspace at the start joins;
Tab/Shift+Tab nest; Alt+Up/Down move the block; Ctrl+D duplicates it.

### 4.3 Reading mode

Ctrl+R (or the eye button) shows the whole note rendered as HTML by the full engine
(Blitz, painted by tiles like a mail body): real highlight backgrounds, inline maths
as images in the line, tables, everything. It is also what PDF export prints.

### 4.4 Undo

A note keeps its own undo stack of whole-text snapshots (edits coalesced over 700 ms,
structural operations each one step); Ctrl+Z / Ctrl+Y go through it, across blocks.

### 4.5 Toolbars and tooltips

- A **selection bubble** above selected text: B, I, U, S, highlight, colour, link,
  maths, code; each button's tooltip names it and its shortcut.
- The **colour picker**: clicking (or pointing at, for a moment) coloured text opens a
  popover of the seven colours and *Default*, with a highlight row; choosing one
  rewrites the span's mark. The same picker opens from the bubble and Ctrl+Shift+C.
- **Hover cards**: a note link shows the note's first lines; maths its rendering; a
  footnote its text; a date the agenda of that day; a tag how many notes carry it; an
  Iris link the message, event or task.
- The page header: the note's path (each part clickable), word count, reading mode,
  focus mode, the outline toggle, More (export, history, copy link, reveal).

## 5. The place

- **Side column**: the space picker at its top (colour dot, name; *New space*, *Open a
  folder as a space*); *Recent* and *Pinned*; the tree; a filter field.
- **Tree**: folders and notes (`.md`), spreadsheets (`.sheet`), images and other files;
  drag and drop to move (folders included), multi-select (Ctrl/Shift), drop files from
  Explorer to import, inline rename (F2), right-click menu (new note, new spreadsheet,
  new folder, rename, duplicate, move to…, pin, colour, copy link, reveal in Explorer,
  delete). Sort by name, modified, or by hand (kept in `space.json`).
- **Renaming or moving a note rewrites every link to it** in the space.
- **Right panel** (toggle): the outline (headings, click to jump, fold), backlinks
  ("Mentioned in", with the line), outgoing links, the note's tags and properties.
- **Quick open** Ctrl+O: fuzzy over the space's notes, then headings with `#`.
  **Search** Ctrl+Shift+F: full text, with `tag:`, `path:`, `is:task` filters.
- Ctrl+4 opens Notes on the last note; Ctrl+N new note in the current folder;
  F11/Escape focus mode (columns folded, text centred).
- Back/Forward know notes (`Place::Note(space, path)`).

## 6. Courses

- **Lecture note from the calendar**: an event happening now or within the hour offers
  *Take notes*; the note is made from the *Cours* template in `<space>/<calendar or
  event title>/`, named "Title — 7 oct.", with properties (course, date, place) and a
  link back to the event; the event shows *Open notes*.
- **Numbered callouts** per note: Définition 1, Théorème 2…, referencable as
  `[[#thm-2]]`.
- **Flashcards** (`Question :: Answer`, and `>q` callouts) feed a **revision** mode per
  note, folder or tag: card, reveal, Again/Hard/Good/Easy, spaced repetition (SM-2),
  state kept in `.iris/review.json`; due cards counted on Home.
- **Split view**: two notes side by side (Ctrl+\\).
- **Templates** in `_Modèles`: `{{title}}`, `{{date}}`, `{{time}}`, `{{course}}`,
  `{{cursor}}`; *New from template* in the menus and `/template`.

## 7. Spreadsheets (`.sheet`)

- File: JSON — `{ "version": 1, "sheets": [{ "name", "cols": {letter: width},
  "rows": {n: height}, "cells": {"A1": {"v": "=SUM(B1:B3)", "f": {format}}},
  "frozen": {"rows": 1, "cols": 0} }] }`. The input is kept (formulas as typed);
  values are computed on open.
- Formulas: numbers, text, booleans, references `A1`, `$A$1`, ranges `A1:B3`, other
  sheet `Sheet2!A1`, operators `+ - * / ^ & = <> < <= > >=`, percent; functions SUM,
  AVERAGE, MIN, MAX, COUNT, COUNTA, IF, AND, OR, NOT, ROUND, ROUNDUP, ROUNDDOWN, ABS,
  SQRT, POWER, MOD, INT, CONCAT, LEN, LEFT, RIGHT, MID, UPPER, LOWER, TRIM, TODAY, NOW,
  DATE, SUMIF, COUNTIF, AVERAGEIF, VLOOKUP, INDEX, MATCH, IFERROR. Errors `#DIV/0!`,
  `#REF!`, `#NAME?`, `#VALUE!`, `#CYCLE!`. Recalculation by dependency order.
- Formatting: bold, italic, underline, strikethrough, text and fill colour, alignment,
  wrap, borders, number formats (general, number with decimals, percent, currency €,
  date, time), column widths and row heights, frozen header row.
- Editing: arrows, Tab, Enter, F2/double-click to edit, typing replaces, Delete clears,
  Ctrl+C/X/V (TSV with Excel and Google Sheets), fill down (Ctrl+D), selection by
  drag and Shift, insert/delete rows and columns, sort by a column, sum/average/count of
  the selection in the status line, undo.
- Import/export: CSV, and `.xlsx` (read with `calamine`, written with
  `rust_xlsxwriter`).
- Embedded in a note with `![[Budget.sheet]]` (or `![[Budget.sheet#A1:D10]]`): a
  read-only table of its values in the editor and in reading mode; a click opens it.

## 8. Links with the rest of Iris

- `[[mail:…]]`, `[[event:…]]`, `[[task:…]]` open the conversation, the event, the task;
  *Copy link to note* from a conversation, an event and a task.
- A checkbox line can become a task (*Make a task*); the task keeps a link to the note,
  and ticking either ticks both.
- Home shows the last notes and the cards due.

## 9. Also

- Version history per note: the previous versions kept in `.iris/history/<note>/`
  (one per editing session, the last 30), shown and restored from the More menu.
- Export a note to PDF and HTML (reading mode's HTML; PDF by printing it).
- Graph of links for a space (a force layout drawn with Slint paths).

## 10. Architecture

| Unit | Kind | Does |
|---|---|---|
| `iris-notes` | pure | blocks (parse/serialise exact), inline marks (parse → spans; spans → Slint markdown; → HTML), links/tags/dates/footnotes extraction, link rewriting on rename, callout numbering, outline, LaTeX→Unicode, snippets, templates, quick-open fuzzy match, flashcards and SM-2 |
| `iris-math` | pure | LaTeX → RGBA image (`latex-rust`, STIX Two Math embedded), with a cache |
| `iris-vault` | I/O | spaces (list, create, open, `space.json`), tree, atomic read/write, watching (`notify`), Recycle Bin (`trash`), attachments, index and search, history |
| `iris-sheets` | pure | cells, formula parser and evaluator, dependencies, formats, CSV; `.sheet` JSON |
| `iris-app/src/notes/` | wiring | the place, tree, editor, popups, sheets view, search, quick open, revision, calendar link |
| `iris-ui/ui/screens/notes*.slint`, `sheet.slint` | UI | the screens, from the shared components |

The display thread never reads a whole space: the index is built on a worker and
handed over; a note is read and parsed when opened (milliseconds).

Memory: the editor holds the open note's blocks (text) and the rendered images of the
visible maths and pictures only; reading mode tiles like a mail body. Measured with
`mesure` after the editor lands, and kept within the budget.

## 11. Testing

- `iris-notes`: round-trip of every block kind (parse then join gives the input back,
  property-style over a corpus), inline parsing of every mark and its escapes, Slint
  markdown and HTML outputs, link rewriting, numbering, LaTeX→Unicode, snippets.
- `iris-sheets`: every function, references, ranges, cycles, errors, recalculation
  order, CSV round trip.
- `iris-vault`: atomic writes, conflicts, moves rewriting links, Recycle Bin call
  (behind a trait), index updates.
- `iris-ui/tests/saisie.rs`: Ctrl+4, typing in a new note, Enter splitting, Escape
  closing every popup, the keyboard reaching nothing behind an open menu.
