//! A preview of Notes, on a space made up for it, captured as pictures.
//!
//! ```text
//! $env:SLINT_BACKEND="winit-software"; cargo run -p iris-app --no-default-features --example apercu_notes -- <folder>
//! ```
//!
//! Writes `notes-editeur.png` (a course note, its side panel), `notes-lecture.png`
//! (reading mode), `notes-tableur.png` (a spreadsheet), `notes-graphe.png` (the
//! links) and `notes-revision.png` (a flashcard revised). `IRIS_THEME=dark` captures
//! the dark theme. Everything is written in a temporary folder.

use iris_app::services::Services;
use slint::ComponentHandle;

fn capture(f: &iris_ui::AppWindow, chemin: std::path::PathBuf) {
    match f.window().take_snapshot() {
        Ok(p) => {
            if let Some(img) =
                image::RgbaImage::from_raw(p.width(), p.height(), p.as_bytes().to_vec())
            {
                let _ = img.save(&chemin);
                println!("capture : {}", chemin.display());
            }
        }
        Err(e) => println!("capture impossible : {e}"),
    }
}

const LIMITES: &str = "---
course: Analyse
date: 2026-10-07
tags: [analyse, suites]
---
# Limites de suites

Une suite $(u_n)$ **converge** vers $\\ell$ si elle s'en approche __autant qu'on veut__. #analyse

> [!def] Convergence
> $(u_n)$ converge vers $\\ell \\in \\mathbb{R}$ si pour tout $\\varepsilon > 0$, il existe $N$ tel que $n \\geq N \\Rightarrow |u_n - \\ell| < \\varepsilon$.

> [!thm] Unicité de la limite
> Si une suite converge, sa limite est {b}unique{/}.

$$
\\lim_{n \\to \\infty} \\left(1 + \\frac{1}{n}\\right)^n = e
$$

## À retenir

- Une suite croissante et ==majorée== converge.
- Les suites {r}divergentes{/} ne sont pas toutes non bornées : --toutes-- voir $(-1)^n$.
- Voir aussi [[Séries]] et le [[#thm-1]].

| Suite | Limite |
|---|---|
| $1/n$ | $0$ |
| $(1+1/n)^n$ | $e$ |

- [ ] Refaire l'exercice 3 [[task:1]]
- [x] Relire le cours

Limite de $1/n$ ? :: $0$
";

const SERIES: &str = "# Séries

Une série est la suite de ses sommes partielles : voir [[Limites de suites]].

> [!ex] Série géométrique
> $\\sum q^n = \\frac{1}{1-q}$ pour $|q| < 1$.

Somme de $\\sum 1/2^n$ ? :: $2$
";

fn main() {
    let sortie = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let dir = tempfile::tempdir().unwrap();
    // The notes in the temporary folder, never in the user's own.
    std::env::set_var("IRIS_ROOT", dir.path());
    let services = Services::open(
        iris_app::paths::Paths::under(dir.path()),
        Some(iris_secrets::Secret::new("apercu")),
    )
    .unwrap();

    // A space of course notes.
    let racine = services
        .paths
        .data
        .parent()
        .unwrap_or(dir.path())
        .join("notes");
    let vault = iris_vault::Vault::open(racine).unwrap();
    let espace = vault.create_space("Cours").unwrap();
    espace.create_folder("", "Analyse").unwrap();
    espace.create_folder("", "Physique").unwrap();
    espace
        .create_note("Analyse", "Limites de suites", LIMITES)
        .unwrap();
    espace.create_note("Analyse", "Séries", SERIES).unwrap();
    espace
        .create_note(
            "Analyse",
            "Dérivées",
            "# Dérivées\n\nVoir [[Limites de suites]].\n",
        )
        .unwrap();
    espace
        .create_note(
            "Physique",
            "Mécanique",
            "# Mécanique\n\n$F = ma$, et [[Dérivées]].\n",
        )
        .unwrap();
    let mut budget = iris_sheets::Workbook::default();
    {
        let s = &mut budget.sheets[0];
        let mut pose = |a: &str, v: &str| s.set_input(iris_sheets::Addr::parse(a).unwrap(), v);
        for (a, v) in [
            ("A1", "Poste"),
            ("B1", "Prévu"),
            ("C1", "Dépensé"),
            ("D1", "Reste"),
            ("A2", "Livres"),
            ("B2", "120"),
            ("C2", "86.5"),
            ("A3", "Calculatrice"),
            ("B3", "90"),
            ("C3", "99"),
            ("A4", "Impressions"),
            ("B4", "30"),
            ("C4", "12"),
            ("A5", "Total"),
            ("B5", "=SUM(B2:B4)"),
            ("C5", "=SUM(C2:C4)"),
        ] {
            pose(a, v);
        }
        for r in 2..=5 {
            pose(&format!("D{r}"), &format!("=B{r}-C{r}"));
        }
        s.set_format(iris_sheets::Range::parse("A1:D1").unwrap(), |f| {
            f.bold = true;
            f.fill = Some("#e8f0fe".into());
        });
        s.set_format(iris_sheets::Range::parse("B2:D5").unwrap(), |f| {
            f.number = iris_sheets::NumberFormat::Currency;
        });
        s.set_format(iris_sheets::Range::parse("A5:D5").unwrap(), |f| {
            f.bold = true
        });
        s.col_widths.insert(0, 140.0);
        s.frozen_rows = 1;
    }
    espace
        .create_file("Analyse", "Budget", "sheet", budget.to_json().as_bytes())
        .unwrap();

    let f = iris_ui::AppWindow::new().unwrap();
    f.window().set_size(slint::LogicalSize::new(1280.0, 800.0));
    let sombre = std::env::var("IRIS_THEME").as_deref() == Ok("dark");
    let theme = services.themes.apply(
        if sombre {
            iris_theme::Appearance::Dark
        } else {
            iris_theme::Appearance::Light
        },
        false,
    );
    iris_app::shell::appliquer_apparence(&f, &theme, iris_app::settings::Density::Normal);
    iris_app::notes::wire_notes(&f, &services);
    f.set_workspace(4);
    f.invoke_workspace_changed(4);
    f.show().unwrap();

    let etapes = slint::Timer::default();
    let faible = f.as_weak();
    let mut tour = 0;
    etapes.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(900),
        move || {
            let Some(f) = faible.upgrade() else { return };
            tour += 1;
            match tour {
                1 => {
                    f.invoke_notes_row_toggled("Analyse".into());
                    f.invoke_notes_row_toggled("Physique".into());
                    f.invoke_notes_quick_chosen("Analyse/Limites de suites.md".into());
                    f.invoke_note_side_toggled();
                }
                2 => {
                    capture(&f, sortie.join("notes-editeur.png"));
                    f.invoke_note_side_toggled();
                    f.invoke_note_reading_toggled();
                }
                3 => {
                    capture(&f, sortie.join("notes-lecture.png"));
                    f.invoke_note_reading_toggled();
                    f.invoke_notes_quick_chosen("Analyse/Budget.sheet".into());
                }
                4 => {
                    capture(&f, sortie.join("notes-tableur.png"));
                    f.invoke_notes_quick_chosen("Analyse/Limites de suites.md".into());
                    f.invoke_notes_graph_requested();
                }
                5 => {
                    capture(&f, sortie.join("notes-graphe.png"));
                    f.set_notes_graph_open(false);
                    f.invoke_note_revise();
                    f.invoke_notes_review_reveal();
                }
                6 => {
                    capture(&f, sortie.join("notes-revision.png"));
                    let _ = slint::quit_event_loop();
                }
                _ => {}
            }
        },
    );
    slint::run_event_loop_until_quit().unwrap();
}
