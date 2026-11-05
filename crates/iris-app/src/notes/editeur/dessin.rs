//! Each kind of block laid out and drawn: its words in their zones, and what its kind
//! adds — a bullet, a number, a box to tick, a quote's bar, a callout's frame, a
//! plate, a table's grid, a picture.
//!
//! The block being written shows its source (its inline marks dimmed; the mark that
//! starts a one-line block stays hidden, drawn as what it makes); the others read as
//! they will be printed.

use super::riche::{self, Pinceau, Riche, Troncon};
use super::{poser, Action, BlocVu, Cible, Contexte, Dessin, Grille, Zone};
use crate::notes::render;
use iris_notes::block::{Block, BlockKind};

/// The text's size, a fixed-width block's, a callout's (logical pixels).
pub const CORPS: f32 = 15.0;
pub const MONO: f32 = 13.0;
pub const ENCADRE: f32 = 14.5;
/// How far a nested list item goes in.
const RETRAIT: f32 = 22.0;

fn taille_titre(n: u8) -> f32 {
    match n {
        1 => 27.0,
        2 => 21.5,
        3 => 18.0,
        _ => 15.5,
    }
}

/// Every shown part's source moved by `d` (the zone's text starts `d` bytes into the
/// block).
fn decaler(r: &mut Riche, d: usize) {
    for t in &mut r.carte {
        t.source.start += d;
        t.source.end += d;
    }
}

/// Text shown as it is, in one style, from source offset `debut`.
fn brut(texte: &str, debut: usize, p: Pinceau) -> Riche {
    Riche {
        texte: texte.to_string(),
        styles: vec![(0..texte.len(), p)],
        formules: Vec::new(),
        carte: vec![Troncon {
            affiche: 0..texte.len(),
            source: debut..debut + texte.len(),
        }],
        liens: Vec::new(),
    }
}

/// A block's lines, each with where it starts in the block (line endings left out).
fn lignes(content: &str) -> Vec<(usize, &str)> {
    let mut sortie = Vec::new();
    let mut k = 0;
    for l in content.split('\n') {
        sortie.push((k, l.trim_end_matches('\r')));
        k += l.len() + 1;
    }
    sortie
}

/// A whole multi-line source, each line read as the line written (marks dimmed), or
/// all of it in one style (`plat`, for code).
fn source_entiere(content: &str, base: Pinceau, plat: bool, cx: &Contexte) -> Riche {
    let mut r = Riche::default();
    for (debut, l) in lignes(content) {
        let ligne = if plat {
            brut(l, 0, base)
        } else {
            riche::source(l, base, cx.couleurs)
        };
        r.ajouter_ligne(ligne, debut, base);
    }
    r
}

/// A picture embedded, decoded once per file and kept while the file does not change.
fn image_de(chemin: &std::path::Path) -> Option<slint::Image> {
    thread_local! {
        static IMAGES: std::cell::RefCell<
            std::collections::HashMap<std::path::PathBuf, (Option<std::time::SystemTime>, slint::Image)>,
        > = std::cell::RefCell::new(std::collections::HashMap::new());
    }
    let quand = std::fs::metadata(chemin).and_then(|m| m.modified()).ok();
    if let Some(img) = IMAGES.with_borrow(|m| {
        m.get(chemin)
            .filter(|(q, _)| *q == quand)
            .map(|(_, i)| i.clone())
    }) {
        return Some(img);
    }
    let img = render::picture(chemin)?;
    IMAGES.with_borrow_mut(|m| {
        if m.len() > 64 {
            m.clear();
        }
        m.insert(chemin.to_path_buf(), (quand, img.clone()));
    });
    Some(img)
}

/// The width an embed asks its picture to be drawn at (`![[a.png|300]]`).
pub fn largeur_demandee(target: &str) -> Option<f32> {
    target
        .split('|')
        .skip(1)
        .find_map(|o| o.trim().parse::<f32>().ok())
        .filter(|w| *w >= 16.0)
}

/// The box an embed asks its picture to fit in (`![[a.png|300x200]]`).
pub fn boite_demandee(target: &str) -> Option<(f32, f32)> {
    target.split('|').skip(1).find_map(|o| {
        let (w, h) = o.trim().split_once(['x', 'X'])?;
        let (w, h) = (w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?);
        (w >= 16.0 && h >= 16.0).then_some((w, h))
    })
}

/// A block laid out. `numero`: what the note's numbering gave it (a callout's
/// "Theorem 2", a list item's "3."); `actif`: the cursor's offset in it when it is the
/// block being written.
pub fn bloc(b: &Block, numero: &str, actif: Option<usize>, cx: &Contexte) -> BlocVu {
    let curseur = actif;
    let actif = curseur.is_some();
    let content = b.content();
    let c = cx.couleurs;
    let mut d = Dessin::default();
    let mut zones = Vec::new();
    let mut cibles = Vec::new();
    let mut grille = None;
    let mut objet = None;
    let base = Pinceau {
        couleur: c.texte,
        ..Default::default()
    };
    // A heading's `#` can be gone into (they show when the cursor is at its words'
    // start): only the other marks keep the cursor out.
    let marque = if actif && !matches!(b.kind, BlockKind::Heading(_)) {
        render::prefixe(&b.kind, content)
    } else {
        0
    };
    let w = cx.largeur;

    let hauteur = match &b.kind {
        BlockKind::Code { .. } | BlockKind::Properties => {
            let mono = Pinceau {
                famille: 1,
                couleur: if b.kind == BlockKind::Properties {
                    c.muet
                } else {
                    c.texte
                },
                ..base
            };
            let r = if actif || b.kind == BlockKind::Properties {
                source_entiere(content, mono, true, cx)
            } else {
                // The inside, its fences off.
                let premiere = content.find('\n').map_or(content.len(), |k| k + 1);
                let interieur = render::interieur(content);
                brut(&interieur, premiere.min(content.len()), mono)
            };
            plaque(r, 1, &mut zones, &mut d, cx)
        }
        BlockKind::Math => {
            let latex = render::interieur(content);
            let dessin = if actif {
                None
            } else {
                (cx.formule_bloc)(&latex)
            };
            match dessin {
                Some(p) => {
                    let e = cx.echelle.max(1.0);
                    let (iw, ih) = (p.width as f32 / e, p.height as f32 / e);
                    let iw2 = iw.min(w);
                    let ih2 = ih * iw2 / iw.max(1.0);
                    let x = (w - iw2) / 2.0;
                    d.images.push(iris_ui::NoteImageData {
                        x,
                        y: 6.0,
                        w: iw2,
                        h: ih2,
                        image: slint::Image::from_rgba8(slint::SharedPixelBuffer::<
                            slint::Rgba8Pixel,
                        >::clone_from_slice(
                            &p.rgba, p.width, p.height
                        )),
                    });
                    ih2 + 12.0
                }
                None => {
                    let mono = Pinceau {
                        famille: 1,
                        couleur: if actif { c.texte } else { c.maths },
                        ..base
                    };
                    plaque(
                        source_entiere(content, mono, true, cx),
                        1,
                        &mut zones,
                        &mut d,
                        cx,
                    )
                }
            }
        }
        // Written as it reads, a grid: the cursor goes from cell to cell, its words
        // with their marks; no `|` to type around.
        BlockKind::Table => table(content, actif, &mut zones, &mut d, &mut cibles, cx),
        BlockKind::Callout {
            kind,
            folded,
            title,
        } => callout(
            content,
            kind,
            *folded,
            title,
            numero,
            actif,
            &mut zones,
            &mut d,
            &mut cibles,
            cx,
        ),
        BlockKind::Quote | BlockKind::Container { .. }
            if content.contains('\n') || matches!(b.kind, BlockKind::Container { .. }) =>
        {
            let quote = b.kind == BlockKind::Quote;
            let p = Pinceau {
                couleur: if quote { c.secondaire } else { c.texte },
                ..base
            };
            let r = if actif {
                source_entiere(content, p, false, cx)
            } else {
                corps_multiligne(content, &b.kind, p, cx)
            };
            let x = if quote { 12.0 } else { 0.0 };
            let z = poser(r, CORPS, 0, x, 2.0, w - x, false, cx, &mut d);
            let h = z.hauteur + 4.0;
            if quote {
                d.rect(0.0, 2.0, 3.0, h - 4.0, c.bordure_forte, 1.5);
            }
            zones.push(z);
            h
        }
        BlockKind::Rule if !actif => {
            d.rect(0.0, 9.0, w, 1.0, c.bordure, 0.0);
            // Somewhere to click, to write it.
            zones.push(poser(
                brut("", 0, base),
                4.0,
                0,
                0.0,
                0.0,
                w,
                false,
                cx,
                &mut Dessin::default(),
            ));
            19.0
        }
        // A spreadsheet's chart is drawn whether the cursor is on it or not; the cursor
        // on it shows what makes it above it, to change or take away.
        BlockKind::Embed { target } if render::graphique_nomme(target).is_some() => {
            match cx.dir.and_then(|dir| render::graphique_insere(target, dir)) {
                Some(plot) => {
                    let haut = if actif {
                        ligne_simple(b, numero, curseur, &mut zones, &mut d, &mut cibles, cx) + 4.0
                    } else {
                        // Somewhere to click, to write it.
                        zones.push(poser(
                            brut("", 0, base),
                            4.0,
                            0,
                            0.0,
                            0.0,
                            w,
                            false,
                            cx,
                            &mut Dessin::default(),
                        ));
                        0.0
                    };
                    haut + graphique(&plot, haut, &mut d, cx)
                }
                None => ligne_simple(b, numero, curseur, &mut zones, &mut d, &mut cibles, cx),
            }
        }
        // An embedded spreadsheet is drawn whether the cursor is on it or not: it is
        // one thing, selected as a whole (its frame says so), as a table in Word.
        BlockKind::Embed { target } if render::est_tableur(target) => {
            match tableur(target, &mut d, &mut cibles, cx) {
                Some((h, g)) => {
                    if actif {
                        let largeur: f32 = g.largeurs.iter().sum::<f32>() * g.echelle;
                        d.cadre(-3.0, -1.0, largeur + 6.0, h - 2.0, 0, c.accent, 2.0, 8.0);
                    }
                    grille = Some(g);
                    h
                }
                None => ligne_simple(b, numero, curseur, &mut zones, &mut d, &mut cibles, cx),
            }
        }
        // A picture is drawn, never its mark; the cursor on it selects it, with
        // handles at its corners to resize it, and it is dragged elsewhere whole.
        BlockKind::Embed { target } if render::is_picture(target) => {
            let image = cx
                .dir
                .and_then(|dir| render::embedded_file(target, dir))
                .and_then(|p| image_de(&p));
            match image {
                Some(img) => {
                    let taille = img.size();
                    let naturelle = taille.width as f32;
                    let rapport = taille.height as f32 / naturelle.max(1.0);
                    // Never stretched: the width asked for (or a box, `|300x200`, the
                    // picture fitted inside it), narrowed to the column, the height
                    // following.
                    let iw = match boite_demandee(target) {
                        Some((bw, bh)) => bw.min(bh / rapport.max(0.001)),
                        None => largeur_demandee(target).unwrap_or(naturelle),
                    }
                    .min(w)
                    .max(16.0);
                    let ih = iw * rapport;
                    d.images.push(iris_ui::NoteImageData {
                        x: 0.0,
                        y: 4.0,
                        w: iw,
                        h: ih,
                        image: img,
                    });
                    cibles.push(Cible {
                        x: 0.0,
                        y: 4.0,
                        w: iw,
                        h: ih,
                        action: Action::Image,
                        curseur: 3,
                    });
                    // Its left and right edges resize it, selected or not: the pointer
                    // says so anywhere along them, not only at a corner.
                    let bord = 10.0f32.min(iw / 4.0);
                    for (k, x) in [(2u8, -6.0), (3u8, iw - bord)] {
                        cibles.push(Cible {
                            x,
                            y: 4.0,
                            w: bord + 6.0,
                            h: ih,
                            action: Action::Poignee(k),
                            curseur: 2,
                        });
                    }
                    if actif {
                        d.cadre(-2.0, 2.0, iw + 4.0, ih + 4.0, 0, c.accent, 2.0, 2.0);
                        for (k, (px, py)) in
                            [(0.0, 4.0), (iw, 4.0), (0.0, 4.0 + ih), (iw, 4.0 + ih)]
                                .into_iter()
                                .enumerate()
                        {
                            d.cadre(
                                px - 5.0,
                                py - 5.0,
                                10.0,
                                10.0,
                                0xffff_ffff,
                                c.accent,
                                1.5,
                                2.0,
                            );
                            cibles.push(Cible {
                                x: px - 10.0,
                                y: py - 10.0,
                                w: 20.0,
                                h: 20.0,
                                action: Action::Poignee(k as u8),
                                curseur: 2,
                            });
                        }
                        // A bar on each side's middle: the edge can be taken there too.
                        for x in [-2.5, iw - 1.5] {
                            d.rect(x, 4.0 + ih / 2.0 - 14.0, 4.0, 28.0, c.accent, 2.0);
                        }
                    }
                    objet = Some((0.0, 4.0, iw, ih));
                    ih + 8.0
                }
                None => ligne_simple(b, numero, curseur, &mut zones, &mut d, &mut cibles, cx),
            }
        }
        _ => ligne_simple(b, numero, curseur, &mut zones, &mut d, &mut cibles, cx),
    };

    BlocVu {
        hauteur,
        dessin: d,
        zones,
        cibles,
        grille,
        objet,
        marque,
    }
}

/// A plate (code, maths, a table written, properties) holding its text in a
/// fixed-width font, with room around it.
fn plaque(r: Riche, famille: u8, zones: &mut Vec<Zone>, d: &mut Dessin, cx: &Contexte) -> f32 {
    let w = cx.largeur;
    let mut fond = Dessin::default();
    let z = poser(r, MONO, famille, 12.0, 10.0, w - 24.0, false, cx, &mut fond);
    let h = z.hauteur + 20.0;
    d.rect(0.0, 2.0, w, h - 4.0, cx.couleurs.plaque, 6.0);
    d.runs.extend(fond.runs);
    d.rects.extend(fond.rects);
    d.images.extend(fond.images);
    zones.push(z);
    h
}

/// The words of a multi-line quote or a container, read: their `>` and `:::` lines off.
fn corps_multiligne(content: &str, kind: &BlockKind, p: Pinceau, cx: &Contexte) -> Riche {
    let mut r = Riche::default();
    let toutes = lignes(content);
    let n = toutes.len();
    for (i, (debut, l)) in toutes.into_iter().enumerate() {
        let (saut, garde) = match kind {
            BlockKind::Container { .. } => {
                let t = l.trim();
                (
                    0,
                    i > 0 && !(i + 1 == n && t == ":::") && !t.starts_with(":::col"),
                )
            }
            _ => {
                let espaces = l.len() - l.trim_start().len();
                let apres = &l[espaces..];
                let s = if let Some(x) = apres.strip_prefix("> ") {
                    l.len() - x.len()
                } else if let Some(x) = apres.strip_prefix('>') {
                    l.len() - x.len()
                } else {
                    0
                };
                (s, true)
            }
        };
        if !garde {
            continue;
        }
        let ligne = riche::rendu(&l[saut..], p, CORPS, cx.echelle, cx.couleurs, cx.formule);
        r.ajouter_ligne(ligne, debut + saut, p);
    }
    r
}

/// A line of words: a paragraph, a heading, a list item, a checkbox, a quote, a card,
/// an empty line — and an embed or a rule while written.
#[allow(clippy::too_many_arguments)]
fn ligne_simple(
    b: &Block,
    numero: &str,
    actif: Option<usize>,
    zones: &mut Vec<Zone>,
    d: &mut Dessin,
    cibles: &mut Vec<Cible>,
    cx: &Contexte,
) -> f32 {
    let content = b.content();
    let c = cx.couleurs;
    let p = render::prefixe(&b.kind, content);
    let (taille, gras, haut) = match &b.kind {
        BlockKind::Heading(n) => (taille_titre(*n), true, if *n <= 2 { 14.0 } else { 8.0 }),
        _ => (CORPS, false, 2.0),
    };
    let (indent, largeur_marque) = match &b.kind {
        BlockKind::Bullet { indent } => (*indent, 22.0),
        BlockKind::Numbered { indent, .. } | BlockKind::Task { indent, .. } => (*indent, 26.0),
        BlockKind::Quote => (0, 12.0),
        _ => (0, 0.0),
    };
    let retrait = f32::from(indent) * RETRAIT;
    let fait = matches!(b.kind, BlockKind::Task { done: true, .. });
    let base = Pinceau {
        couleur: if fait || b.kind == BlockKind::Quote {
            c.secondaire
        } else {
            c.texte
        },
        gras,
        ..Default::default()
    };
    // The mark that starts the line is hidden either way: drawn as what it makes.
    let mots = &content[p..];
    let mut r = if let Some(curseur) = actif {
        // Written: only the mark the cursor touches shows its signs.
        riche::ecriture(
            mots,
            base,
            curseur.saturating_sub(p),
            taille,
            cx.echelle,
            c,
            cx.formule,
        )
    } else {
        let mut r = riche::rendu(mots, base, taille, cx.echelle, c, cx.formule);
        if fait {
            for (_, s) in &mut r.styles {
                s.barre = true;
            }
        }
        r
    };
    decaler(&mut r, p);
    // A heading's `#` shown, dimmed, while the cursor is at its words' start or in
    // them: they can be changed there (they are not printed).
    if let (BlockKind::Heading(_), Some(curseur)) = (&b.kind, actif) {
        if p > 0 && curseur <= p {
            let mut tete = brut(
                &content[..p],
                0,
                Pinceau {
                    couleur: c.muet,
                    ..base
                },
            );
            tete.joindre(r);
            r = tete;
        }
    }
    let x = retrait + largeur_marque;
    let z = poser(r, taille, 0, x, haut, cx.largeur - x, false, cx, d);
    let h_ligne = taille * super::rapport(&cx.polices.corps);
    let marque_p = Pinceau {
        couleur: c.secondaire,
        ..Default::default()
    };
    match &b.kind {
        BlockKind::Bullet { indent } => {
            let puce = match indent {
                0 => "•",
                1 => "◦",
                _ => "▪",
            };
            d.mot(retrait + 4.0, haut, puce, taille, marque_p, cx.polices);
        }
        BlockKind::Numbered { .. } => {
            let lw = super::largeur_de(numero, taille, 0, cx);
            d.mot(
                retrait + 20.0 - lw,
                haut,
                numero,
                taille,
                marque_p,
                cx.polices,
            );
        }
        BlockKind::Task { done, .. } => {
            let (bx, by) = (retrait + 1.0, haut + (h_ligne - 16.0) / 2.0);
            if *done {
                d.cadre(bx, by, 16.0, 16.0, c.accent, c.accent, 1.5, 4.0);
                d.icone(bx + 1.0, by + 1.0, 14.0, "check", 0xffff_ffff);
            } else {
                d.cadre(bx, by, 16.0, 16.0, 0, c.bordure_forte, 1.5, 4.0);
            }
            cibles.push(Cible {
                x: bx - 3.0,
                y: by - 3.0,
                w: 22.0,
                h: 22.0,
                action: Action::Cocher,
                curseur: 1,
            });
        }
        BlockKind::Quote => d.rect(retrait, haut, 3.0, z.hauteur, c.bordure_forte, 1.5),
        _ => {}
    }
    let h = haut + z.hauteur + 2.0;
    zones.push(z);
    h
}

/// A table: its cells in a grid, the first row as headers. Written (`ecrit`), each
/// cell shows its source (its marks dimmed) and two buttons add a row and a column.
fn table(
    content: &str,
    ecrit: bool,
    zones: &mut Vec<Zone>,
    d: &mut Dessin,
    cibles: &mut Vec<Cible>,
    cx: &Contexte,
) -> f32 {
    let c = cx.couleurs;
    // The cells and where each starts in the block.
    let mut rangees: Vec<Vec<(usize, &str)>> = Vec::new();
    for (i, (debut, l)) in lignes(content).into_iter().enumerate() {
        let t = l.trim();
        if i == 1 && t.chars().all(|ch| matches!(ch, '|' | '-' | ':' | ' ')) {
            continue;
        }
        if t.is_empty() {
            continue;
        }
        let mut cases = Vec::new();
        let mut k = 0;
        let morceaux: Vec<&str> = l.split('|').collect();
        let n = morceaux.len();
        for (j, m) in morceaux.iter().enumerate() {
            let ici = k;
            k += m.len() + 1;
            // The parts before the first `|` and after the last are not cells.
            if (j == 0 || j + 1 == n) && m.trim().is_empty() {
                continue;
            }
            // An empty cell is written one space in (`edit::table_cells`).
            let espaces = if m.trim().is_empty() {
                m.len().min(1)
            } else {
                m.len() - m.trim_start().len()
            };
            cases.push((debut + ici + espaces, m.trim()));
        }
        rangees.push(cases);
    }
    let colonnes = rangees.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let wc = cx.largeur / colonnes as f32;
    let mut y = 2.0;
    for (r, rangee) in rangees.iter().enumerate() {
        let mut dessin = Dessin::default();
        let mut zs = Vec::new();
        let mut haut = 0.0f32;
        for k in 0..colonnes {
            let (debut, texte) = rangee.get(k).copied().unwrap_or((0, ""));
            let p = Pinceau {
                couleur: c.texte,
                gras: r == 0,
                ..Default::default()
            };
            let mut rr = if ecrit {
                riche::source(texte, p, c)
            } else {
                riche::rendu(texte, p, 13.5, cx.echelle, c, cx.formule)
            };
            // An empty cell still has a place, for the cursor.
            if rr.carte.is_empty() {
                rr.carte.push(Troncon {
                    affiche: 0..0,
                    source: 0..0,
                });
            }
            decaler(&mut rr, debut);
            let z = poser(
                rr,
                13.5,
                0,
                k as f32 * wc + 8.0,
                y + 5.0,
                wc - 16.0,
                false,
                cx,
                &mut dessin,
            );
            haut = haut.max(z.hauteur + 10.0);
            zs.push(z);
        }
        for k in 0..colonnes {
            d.cadre(
                k as f32 * wc,
                y,
                wc,
                haut,
                if r == 0 { c.plaque } else { 0 },
                c.bordure,
                0.5,
                0.0,
            );
        }
        d.runs.extend(dessin.runs);
        d.rects.extend(dessin.rects);
        d.images.extend(dessin.images);
        zones.extend(zs);
        y += haut;
    }
    if ecrit {
        // A row under the last, a column after the last: a small `+` on the edge,
        // over the border rather than beside it, so the table does not move.
        let plaque = (c.plaque & 0x00ff_ffff) | 0xff00_0000;
        let boutons = [
            (cx.largeur / 2.0 - 9.0, y - 9.0, Action::AjouterLigne),
            (
                cx.largeur - 9.0,
                2.0 + (y - 2.0) / 2.0 - 9.0,
                Action::AjouterColonne,
            ),
        ];
        for (bx, by, action) in boutons {
            d.cadre(bx, by, 18.0, 18.0, plaque, c.bordure_forte, 1.0, 9.0);
            d.icone(bx + 3.0, by + 3.0, 12.0, "plus", c.secondaire);
            cibles.push(Cible {
                x: bx - 2.0,
                y: by - 2.0,
                w: 22.0,
                h: 22.0,
                action,
                curseur: 1,
            });
        }
    }
    y + 4.0
}

/// A callout: its frame, its title line, its words unless folded.
#[allow(clippy::too_many_arguments)]
fn callout(
    content: &str,
    kind: &str,
    folded: Option<bool>,
    title: &str,
    numero: &str,
    actif: bool,
    zones: &mut Vec<Zone>,
    d: &mut Dessin,
    cibles: &mut Vec<Cible>,
    cx: &Contexte,
) -> f32 {
    let c = cx.couleurs;
    let w = cx.largeur;
    let (_, teinte, _) = render::callout_label(kind);
    let teinte = teinte.as_argb_encoded() | 0xff00_0000;
    let fond = (teinte & 0x00ff_ffff) | 0x1a00_0000;
    let mut dessin = Dessin::default();
    let h = if actif {
        let p = Pinceau {
            couleur: c.texte,
            ..Default::default()
        };
        let z = poser(
            source_entiere(content, p, false, cx),
            ENCADRE,
            0,
            14.0,
            10.0,
            w - 24.0,
            false,
            cx,
            &mut dessin,
        );
        let h = z.hauteur + 20.0;
        zones.push(z);
        h
    } else {
        // The title line: the kind's name and number, then the title as written.
        let entete = format!("{numero}{}", if title.is_empty() { "" } else { " — " });
        let p_titre = Pinceau {
            couleur: teinte,
            gras: true,
            ..Default::default()
        };
        let lw = if entete.is_empty() {
            0.0
        } else {
            dessin.mot(14.0, 10.0, &entete, 14.0, p_titre, cx.polices);
            super::largeur_de(&entete, 14.0, 0, cx)
        };
        let premiere = content.split('\n').next().unwrap_or("");
        let debut_titre = premiere
            .find(title)
            .filter(|_| !title.is_empty())
            .unwrap_or(premiere.len());
        let mut rt = riche::rendu(title, p_titre, 14.0, cx.echelle, c, cx.formule);
        decaler(&mut rt, debut_titre);
        let zt = poser(
            rt,
            14.0,
            0,
            14.0 + lw,
            10.0,
            w - 52.0 - lw,
            false,
            cx,
            &mut dessin,
        );
        let mut h = 10.0 + zt.hauteur.max(14.0 * super::rapport(&cx.polices.corps));
        zones.push(zt);
        // The fold arrow.
        let plie = folded.unwrap_or(false);
        dessin.icone(
            w - 30.0,
            11.0,
            14.0,
            if plie {
                "chevron-right"
            } else {
                "chevron-down"
            },
            teinte,
        );
        cibles.push(Cible {
            x: w - 36.0,
            y: 4.0,
            w: 28.0,
            h: 26.0,
            action: Action::Plier,
            curseur: 1,
        });
        if !plie {
            let p = Pinceau {
                couleur: c.texte,
                ..Default::default()
            };
            let mut r = Riche::default();
            for (debut, l) in lignes(content).into_iter().skip(1) {
                let espaces = l.len() - l.trim_start().len();
                let apres = &l[espaces..];
                let saut = if let Some(x) = apres.strip_prefix("> ") {
                    l.len() - x.len()
                } else if let Some(x) = apres.strip_prefix('>') {
                    l.len() - x.len()
                } else {
                    0
                };
                let ligne = riche::rendu(&l[saut..], p, ENCADRE, cx.echelle, c, cx.formule);
                r.ajouter_ligne(ligne, debut + saut, p);
            }
            if !r.texte.is_empty() || !r.carte.is_empty() {
                let z = poser(
                    r,
                    ENCADRE,
                    0,
                    14.0,
                    h + 4.0,
                    w - 24.0,
                    false,
                    cx,
                    &mut dessin,
                );
                h += 4.0 + z.hauteur;
                zones.push(z);
            }
        }
        h + 10.0
    };
    d.rect(0.0, 2.0, w, h - 4.0, fond, 6.0);
    d.rect(0.0, 2.0, 3.0, h - 4.0, teinte, 1.5);
    d.runs.extend(dessin.runs);
    d.rects.extend(dessin.rects);
    d.images.extend(dessin.images);
    d.icones.extend(dessin.icones);
    h
}

/// A chart laid out by `iris_sheets::plot`, drawn from `y` down at the column's width
/// (a little wider than it was laid out for at most); the room it takes.
fn graphique(p: &iris_sheets::plot::Plot, y: f32, d: &mut Dessin, cx: &Contexte) -> f32 {
    use iris_sheets::plot::{Align, Paint};
    let c = cx.couleurs;
    let haut = y + 8.0;
    if !p.message.is_empty() {
        let pinceau = Pinceau {
            couleur: c.muet,
            ..Default::default()
        };
        d.mot(
            0.0,
            haut,
            &format!("⧉ {}", p.message),
            13.5,
            pinceau,
            cx.polices,
        );
        return 34.0;
    }
    let w = cx.largeur.min(p.width * 1.15).max(80.0);
    let e = w / p.width.max(1.0);
    let h = p.height * e;
    let rien = slint::Color::default();
    for s in &p.shapes {
        let (fill, stroke, epais) = match s.paint {
            Paint::Colour(rgb, a) => {
                let teinte = slint::Color::from_argb_u8(
                    (a.clamp(0.0, 1.0) * 255.0).round() as u8,
                    (rgb >> 16) as u8,
                    (rgb >> 8) as u8,
                    rgb as u8,
                );
                if s.stroke > 0.0 {
                    (rien, teinte, s.stroke * e)
                } else {
                    (teinte, rien, 0.0)
                }
            }
            Paint::Grid => (super::couleur(c.bordure), rien, 0.0),
        };
        d.traces.push(iris_ui::NotePathData {
            x: 0.0,
            y: haut,
            w,
            h,
            vw: p.width,
            vh: p.height,
            commands: s.path.as_str().into(),
            fill,
            stroke,
            stroke_width: epais,
        });
    }
    let ligne = super::rapport(cx.polices.nom(0));
    for l in &p.labels {
        let taille = (l.size * e).max(9.5);
        let place = l.width * e;
        let mut texte = l.text.clone();
        let mut tw = super::largeur_de(&texte, taille, 0, cx);
        // Cut where it no longer fits.
        if tw > place && place > 0.0 {
            while !texte.is_empty()
                && super::largeur_de(&format!("{texte}…"), taille, 0, cx) > place
            {
                texte.pop();
            }
            texte.push('…');
            tw = super::largeur_de(&texte, taille, 0, cx);
        }
        let x = l.x * e
            + match l.align {
                Align::Left => 0.0,
                Align::Center => (place - tw) / 2.0,
                Align::Right => place - tw,
            };
        let pinceau = Pinceau {
            couleur: if l.muted { c.muet } else { c.texte },
            gras: l.strong,
            ..Default::default()
        };
        d.mot(
            x,
            haut + l.y * e - taille * ligne / 2.0,
            &texte,
            taille,
            pinceau,
            cx.polices,
        );
    }
    h + 16.0
}

/// An embedded spreadsheet: its values in a grid, its columns as wide as in the sheet
/// (narrowed together when they do not fit), a button to open it and one for long
/// words; each cell and each column's edge clickable.
fn tableur(
    target: &str,
    d: &mut Dessin,
    cibles: &mut Vec<Cible>,
    cx: &Contexte,
) -> Option<(f32, Grille)> {
    let dir = cx.dir?;
    let (cellules, colonnes) = render::tableau_insere(target, dir)?;
    let c = cx.couleurs;
    let lignes = cellules.len().checked_div(colonnes).unwrap_or(0);
    let largeurs = render::largeurs_tableur(target, dir).unwrap_or_else(|| vec![100.0; colonnes]);
    let total: f32 = largeurs.iter().sum::<f32>().max(1.0);
    let echelle = (cx.largeur / total).min(1.0);
    let couper = render::options_tableur(target).contains(&"clip");
    let xs: Vec<f32> = largeurs
        .iter()
        .scan(0.0, |s, w| {
            let x = *s;
            *s += w * echelle;
            Some(x)
        })
        .collect();
    let mut cases = Vec::with_capacity(cellules.len());
    let mut y = 2.0;
    let mut dessin = Dessin::default();
    for r in 0..lignes {
        let mut haut = 0.0f32;
        let mut mots = Vec::new();
        for k in 0..colonnes {
            let wc = largeurs.get(k).copied().unwrap_or(100.0) * echelle;
            let texte = &cellules[r * colonnes + k];
            let p = Pinceau {
                couleur: c.texte,
                gras: r == 0,
                ..Default::default()
            };
            let affiche = if couper {
                // One line, cut where it no longer fits.
                let mut t = texte.clone();
                while !t.is_empty() && super::largeur_de(&t, 13.5, 0, cx) > wc - 16.0 {
                    t.pop();
                }
                if t.len() < texte.len() && !t.is_empty() {
                    t.pop();
                    t.push('…');
                }
                t
            } else {
                texte.clone()
            };
            let z = poser(
                brut(&affiche, 0, p),
                13.5,
                0,
                xs[k] + 8.0,
                y + 5.0,
                wc - 16.0,
                false,
                cx,
                &mut dessin,
            );
            haut = haut.max(z.hauteur + 10.0);
            mots.push(z);
        }
        for (k, x) in xs.iter().enumerate().take(colonnes) {
            let wc = largeurs.get(k).copied().unwrap_or(100.0) * echelle;
            d.cadre(
                *x,
                y,
                wc,
                haut,
                if r == 0 { c.plaque } else { 0 },
                c.bordure,
                0.5,
                0.0,
            );
            cases.push((*x, y, wc, haut));
            cibles.push(Cible {
                x: *x,
                y,
                w: wc,
                h: haut,
                action: Action::Cellule(r as u32, k as u32),
                curseur: 3,
            });
        }
        y += haut;
    }
    let largeur_totale = total * echelle;
    // The columns' edges, over the cells, to carry.
    for (k, x) in xs.iter().enumerate().take(colonnes) {
        let wc = largeurs.get(k).copied().unwrap_or(100.0) * echelle;
        cibles.push(Cible {
            x: x + wc - 4.0,
            y: 2.0,
            w: 8.0,
            h: y - 2.0,
            action: Action::Bord(k),
            curseur: 2,
        });
    }
    // Its buttons, at its top right.
    for (i, (nom, action)) in [
        ("settings", Action::CouperMots),
        ("open-external", Action::OuvrirTableur),
    ]
    .into_iter()
    .enumerate()
    {
        let bx = largeur_totale - 24.0 * (i as f32 + 1.0) - 2.0;
        dessin.rect(
            bx,
            5.0,
            20.0,
            20.0,
            (c.plaque & 0x00ff_ffff) | 0xe000_0000,
            4.0,
        );
        dessin.icone(
            bx + 3.0,
            8.0,
            14.0,
            nom,
            if couper && i == 0 {
                c.accent
            } else {
                c.secondaire
            },
        );
        cibles.push(Cible {
            x: bx,
            y: 5.0,
            w: 20.0,
            h: 20.0,
            action,
            curseur: 1,
        });
    }
    // The frame around it.
    d.cadre(0.0, 2.0, largeur_totale, y - 2.0, 0, c.bordure, 1.0, 6.0);
    d.runs.extend(dessin.runs);
    d.rects.extend(dessin.rects);
    d.icones.extend(dessin.icones);
    Some((
        y + 4.0,
        Grille {
            lignes: lignes as u32,
            colonnes: colonnes as u32,
            cases,
            largeurs,
            echelle,
        },
    ))
}
