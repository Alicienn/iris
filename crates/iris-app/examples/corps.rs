//! Rend un message de la base locale comme le fait le lecteur, en une image.
//!
//! Pour les messages qui s'affichent mal : on voit ce que Blitz peint, tuile par
//! tuile, à la largeur du lecteur, sans ouvrir la fenêtre.
//!
//! ```text
//! cargo run --release -p iris-app --features blitz --example corps -- "<sujet>" <dossier> [largeur] [échelle]
//! ```
//!
//! Écrit, pour chaque message dont le sujet contient le texte donné, le HTML tel qu'il
//! part au moteur (`<id>.html`) et l'image des tuiles mises bout à bout (`<id>.png`).
//! Ce sont des données personnelles : le dossier ne va jamais dans le dépôt.

use iris_htmlview::{BlitzRenderer, HtmlRenderer, Rendered, VecSink};

fn main() {
    let mut args = std::env::args().skip(1);
    let sujet = args.next().expect("un morceau de sujet");
    let dossier = std::path::PathBuf::from(args.next().expect("un dossier de sortie"));
    let largeur: f32 = args.next().map(|s| s.parse().unwrap()).unwrap_or(900.0);
    let echelle: f32 = args.next().map(|s| s.parse().unwrap()).unwrap_or(1.0);
    std::fs::create_dir_all(&dossier).unwrap();

    let services =
        iris_app::services::Services::open(iris_app::paths::Paths::system().unwrap(), None)
            .expect("services");
    let moteur = BlitzRenderer::new(echelle, false);

    for message in services.store.latest_messages(20_000).unwrap() {
        if !message.subject.contains(&sujet) {
            continue;
        }
        let Some(hex) = &message.body_blob else {
            continue;
        };
        let brut = iris_types::BlobId::from_hex(hex)
            .and_then(|id| services.blobs.get(id).ok().flatten())
            .unwrap_or_default();
        let analyse = iris_mime::parse_with(&brut, false).unwrap();
        let Some(html) = analyse.html_body.as_ref() else {
            println!("{} : pas de HTML", message.id);
            continue;
        };
        let html = iris_mime::inline_images(&html.html, &analyse.inline_parts);
        let id = message.id.get();
        std::fs::write(dossier.join(format!("{id}.html")), &html).unwrap();

        let Rendered::Document(mut document) =
            moteur.render_for(&html, false, largeur, echelle).unwrap()
        else {
            println!("{id} : rendu en blocs, sans le moteur complet");
            continue;
        };
        let (l, h) = document.size();
        let mut image = image::RgbaImage::new(l, h);
        let mut y = 0;
        for i in 0..document.tile_count() {
            let mut tampon = VecSink::default();
            document.paint_tile(i, &mut tampon).unwrap();
            let t = document.tile_extent(i);
            let tuile = image::RgbaImage::from_raw(l, t, tampon.0).unwrap();
            image::imageops::overlay(&mut image, &tuile, 0, y as i64);
            y += t;
        }
        image.save(dossier.join(format!("{id}.png"))).unwrap();
        println!("{id} : {l}×{h}, {} tuiles", document.tile_count());
    }
}
