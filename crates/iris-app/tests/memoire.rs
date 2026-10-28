//! Ce que coûtent, en mémoire, les deux choses que l'application fait le plus.
//!
//! Le compteur de la barre d'état annonçait cinq cents mégaoctets pour une boîte de
//! huit mégaoctets sur le disque. L'écart ne s'explique pas par les données, et un
//! compteur de tas ne le montre pas non plus : le tas vivant tient en quelques
//! mégaoctets. Il faut donc mesurer ce que le processus **garde en mémoire** — ce que
//! les pilotes graphiques prennent pour notre compte compris — et le rattacher à un
//! geste précis.
//!
//! Deux de ces tests ne vérifient pas un budget : ils **impriment une facture**. Ils
//! sont ignorés par défaut parce qu'ils exigent une carte graphique et parce que leur
//! résultat est un tableau à lire, pas un booléen :
//!
//! ```text
//! cargo test -p iris-app --test memoire -- --ignored --nocapture
//! ```
//!
//! The third holds a budget on the heap's peak, counted by this test binary's own
//! allocator: the application no longer carries one.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Counts what this test binary holds on the heap, and the most it held.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn noted(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

// Everything is handed to `System`; only the counts are ours.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            noted(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        if !ptr.is_null() {
            noted(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        System.dealloc(ptr, layout);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let nouveau = System.realloc(ptr, layout, new_size);
        if !nouveau.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            noted(new_size);
        }
        nouveau
    }
}

/// Brings the peak down to what is held now, and returns it: the peak of one operation
/// is then what it took.
#[cfg(feature = "blitz")]
fn reset_peak() -> usize {
    let courant = LIVE.load(Ordering::Relaxed);
    PEAK.store(courant, Ordering::Relaxed);
    courant
}

/// Ce que le processus garde en mémoire, en mégaoctets.
fn releve(quoi: &str) -> f64 {
    let mo = iris_app::vitals::resident_bytes().unwrap_or(0) as f64 / 1048576.0;
    println!("{quoi:<34} résident {mo:>7.1} Mo");
    mo
}

fn ecart(avant: f64, apres: f64, quoi: &str) {
    println!("  → {quoi:<30} résident {:>+7.1} Mo", apres - avant);
}

/// Un message d'infolettre, aussi long qu'on le demande.
///
/// Seuls les rendus Blitz s'en servent : sans le moteur, elle serait morte, et
/// l'intégration continue compile sans lui avec les avertissements en erreurs.
#[cfg(feature = "blitz")]
fn message(paragraphes: usize) -> String {
    let mut html = String::from("<html><body style=\"font-family:sans-serif\">");
    for i in 0..paragraphes {
        html.push_str(&format!(
            "<table width=\"100%\"><tr><td><h2>Section {i}</h2>\
             <p>Bonjour, je reviens vers vous concernant le point évoqué la semaine \
             dernière. Voici le détail, en quelques lignes, de ce que nous proposons \
             et de ce qu'il resterait à décider avant la fin du mois.</p></td></tr></table>"
        ));
    }
    html.push_str("</body></html>");
    html
}

#[test]
#[ignore = "imprime une facture au lieu de vérifier un budget"]
#[cfg(feature = "blitz")]
fn ce_que_coute_le_moteur_html() {
    use iris_htmlview::{BlitzRenderer, HtmlRenderer, Rendered};

    let depart = releve("au départ");
    let moteur = BlitzRenderer::new(1.0, false);

    let mut precedent = depart;
    for (nom, paragraphes, largeur) in [
        ("message court", 2usize, 700.0f32),
        ("message moyen", 20, 700.0),
        ("message long", 200, 700.0),
        ("message long, large", 200, 2400.0),
        ("à nouveau le court", 2, 700.0),
    ] {
        match moteur.render(&message(paragraphes), largeur) {
            Ok(Rendered::Document(mut d)) => {
                let (l, h) = d.size();
                // Ce que l'interface peint à l'ouverture : l'écran, et l'écran suivant.
                let mut pixels = iris_htmlview::VecSink::default();
                for i in 0..d.tile_count().min(2) {
                    let _ = d.paint_tile(i, &mut pixels);
                }
                println!(
                    "\n{nom} : document {l}×{h}, {} tuiles, 2 peintes",
                    d.tile_count()
                );
            }
            Ok(_) => println!("\n{nom} : rendu en blocs"),
            Err(e) => println!("\n{nom} : refusé ({e})"),
        }
        let apres = releve(&format!("après « {nom} »"));
        ecart(precedent, apres, nom);
        precedent = apres;
    }
}

/// Une infolettre de vingt mille pixels ne coûte que ce qu'on en regarde.
///
/// Le moteur rendait une image de la hauteur du message : pour celle-ci, plus de
/// soixante mégaoctets de pixels, même lue en haut, et sur un device graphique dont la
/// mémoire n'apparaissait nulle part. Il peint maintenant par tuiles, sur le
/// processeur, et l'interface ne demande que celles qui approchent de l'écran.
///
/// Le budget porte sur le pic du tas pendant la peinture des deux premières tuiles —
/// l'ouverture d'un message. Il vaut quatre tuiles : les deux peintes, et de quoi
/// peindre. Au-delà, quelque chose peint plus que ce qu'on voit.
#[test]
#[cfg(feature = "blitz")]
fn une_longue_infolettre_ne_coute_que_ce_qu_on_en_regarde() {
    use iris_htmlview::{BlitzRenderer, HtmlRenderer, Rendered};

    let moteur = BlitzRenderer::new(1.0, false);
    let Ok(Rendered::Document(mut d)) = moteur.render(&message(400), 800.0) else {
        panic!("attendu un document");
    };
    let (largeur, hauteur) = d.size();
    let image_entiere = largeur as usize * hauteur as usize * 4;
    assert!(
        image_entiere > 60 * 1024 * 1024,
        "le message doit être assez long pour que la différence compte ({hauteur} px)"
    );

    let socle = reset_peak();
    let mut pixels = Vec::new();
    for i in 0..2 {
        let mut tampon = iris_ui::bridge::ImageSink::default();
        d.paint_tile(i, &mut tampon).expect("tuile");
        pixels.push(tampon);
    }
    let pic = PEAK.load(Ordering::Relaxed).saturating_sub(socle);
    let tuile = largeur as usize * d.tile_height() as usize * 4;

    println!(
        "document {largeur}×{hauteur} : l'image entière ferait {:.1} Mo ; \
         deux tuiles peintes, pic du tas {:.1} Mo",
        image_entiere as f64 / 1048576.0,
        pic as f64 / 1048576.0
    );
    assert!(
        pic < tuile * 4,
        "peindre deux tuiles a demandé {pic} octets, le budget est de {}",
        tuile * 4
    );
}

#[test]
#[ignore = "exige une carte graphique ; imprime une facture au lieu de vérifier un budget"]
fn ce_que_coute_la_fenetre() {
    use slint::ComponentHandle;

    let depart = releve("au départ");

    let fenetre = iris_ui::AppWindow::new().expect("la fenêtre doit se construire");
    let construite = releve("après construction de la fenêtre");
    ecart(depart, construite, "construire la fenêtre");

    // La fenêtre n'ouvre son périphérique graphique qu'à l'affichage. Ne pas la
    // montrer mesurerait une fenêtre qui n'a pas encore coûté ce qu'elle coûte.
    fenetre.show().expect("affichage");
    slint::platform::update_timers_and_animations();
    let affichee = releve("après affichage");
    ecart(construite, affichee, "afficher (device + textures)");

    fenetre.hide().expect("masquage");
}
