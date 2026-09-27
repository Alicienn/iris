//! Ce que coûtent, en mémoire, les deux choses que l'application fait le plus.
//!
//! Le compteur de la barre d'état annonçait cinq cents mégaoctets pour une boîte de
//! huit mégaoctets sur le disque. L'écart ne s'explique pas par les données, et un
//! compteur de tas ne le montre pas non plus : le tas vivant tient en quelques
//! mégaoctets. Il faut donc mesurer ce que le processus **engage** — ce que les
//! pilotes graphiques prennent pour notre compte — et le rattacher à un geste précis.
//!
//! Ces tests ne vérifient pas un budget : ils **impriment une facture**. Ils sont
//! ignorés par défaut parce qu'ils exigent une carte graphique et parce que leur
//! résultat est un tableau à lire, pas un booléen :
//!
//! ```text
//! cargo test -p iris-app --test memoire -- --ignored --nocapture
//! ```

use iris_app::memory;

/// Un relevé lisible de l'espace engagé.
fn releve(quoi: &str) -> memory::Regions {
    let r = memory::regions();
    println!(
        "{quoi:<34} privé {:>7.1} Mo   écr.comb. {:>7.1} Mo   image {:>7.1} Mo   résident {:>7.1} Mo",
        r.private_rw as f64 / 1048576.0,
        r.private_wc as f64 / 1048576.0,
        r.image as f64 / 1048576.0,
        iris_app::vitals::resident_bytes().unwrap_or(0) as f64 / 1048576.0,
    );
    r
}

fn ecart(avant: &memory::Regions, apres: &memory::Regions, quoi: &str) {
    let mo = |a: u64, b: u64| (b as i64 - a as i64) as f64 / 1048576.0;
    println!(
        "  → {quoi:<30} privé {:>+7.1} Mo   écr.comb. {:>+7.1} Mo",
        mo(avant.private_rw, apres.private_rw),
        mo(avant.private_wc, apres.private_wc),
    );
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
#[ignore = "exige une carte graphique ; imprime une facture au lieu de vérifier un budget"]
#[cfg(feature = "blitz")]
fn ce_que_coute_le_moteur_html() {
    use iris_htmlview::{BlitzRenderer, HtmlRenderer};

    let depart = releve("au départ");

    let Some(moteur) = BlitzRenderer::probe(1.0, true) else {
        println!("aucun périphérique graphique : rien à mesurer");
        return;
    };
    let construit = releve("après la sonde, device gardé");
    ecart(&depart, &construit, "ouvrir le device");

    // Un message court, puis un long. C'est le second qui compte : le rendeur est
    // gardé d'un message à l'autre et redimensionné, jamais rétréci, de sorte que la
    // facture d'une session est celle du plus grand message ouvert — et non celle du
    // message affiché.
    let mut precedent = construit;
    for (nom, paragraphes, largeur) in [
        ("message court", 2usize, 700.0f32),
        ("message moyen", 20, 700.0),
        ("message long", 60, 700.0),
        ("message long, large", 60, 2400.0),
        ("à nouveau le court", 2, 700.0),
    ] {
        let html = message(paragraphes);
        let mut pixels = iris_htmlview::VecSink::default();
        match moteur.render(&html, largeur, &mut pixels) {
            Ok(iris_htmlview::Rendered::Texture { width, height }) => {
                println!(
                    "\n{nom} : texture {width}×{height}, soit {:.1} Mo de pixels rendus",
                    pixels.0.len() as f64 / 1048576.0
                );
            }
            Ok(_) => println!("\n{nom} : rendu sans texture"),
            Err(e) => println!("\n{nom} : refusé ({e})"),
        }
        let apres = releve(&format!("après « {nom} »"));
        ecart(&precedent, &apres, nom);
        precedent = apres;
    }

    drop(moteur);
    let libere = releve("après abandon du moteur");
    ecart(&precedent, &libere, "abandonner le moteur");
}

/// Le premier message ouvert paie-t-il encore l'ouverture d'un périphérique ?
///
/// La sonde du démarrage ouvrait un device de 64 pixels de côté puis le jetait ; le
/// premier message en rouvrait un autre, et cette seconde ouverture se payait sur le
/// fil de l'interface, au moment où quelqu'un venait de cliquer. `probe` garde
/// désormais ce qu'elle a ouvert. Les deux moitiés vivent dans deux processus, parce
/// qu'un device ouvert dans l'un se sentirait dans l'autre.
///
/// ```text
/// cargo test -p iris-app --test memoire -- --ignored --nocapture --exact le_premier_rendu_apres_la_sonde
/// cargo test -p iris-app --test memoire -- --ignored --nocapture --exact le_premier_rendu_sans_sonde
/// ```
#[test]
#[ignore = "moitié d'une comparaison entre deux processus"]
#[cfg(feature = "blitz")]
fn le_premier_rendu_apres_la_sonde() {
    use iris_htmlview::{BlitzRenderer, HtmlRenderer};

    let depart = releve("au départ");
    let chrono = std::time::Instant::now();
    let Some(moteur) = BlitzRenderer::probe(1.0, true) else {
        println!("aucun périphérique graphique : rien à mesurer");
        return;
    };
    println!("la sonde a pris {:?}", chrono.elapsed());

    let chrono = std::time::Instant::now();
    let _ = moteur.render(&message(20), 700.0, &mut iris_htmlview::VecSink::default());
    println!("PREMIER RENDU après la sonde : {:?}", chrono.elapsed());

    let apres = releve("après un rendu");
    ecart(&depart, &apres, "TOTAL avec sonde gardée");
}

#[test]
#[ignore = "moitié d'une comparaison entre deux processus"]
#[cfg(feature = "blitz")]
fn le_premier_rendu_sans_sonde() {
    use iris_htmlview::{BlitzRenderer, HtmlRenderer};

    let depart = releve("au départ");
    // `new` n'ouvre rien : le device sera ouvert par le premier rendu, comme avant.
    let moteur = BlitzRenderer::new(1.0, true);

    let chrono = std::time::Instant::now();
    let _ = moteur.render(&message(20), 700.0, &mut iris_htmlview::VecSink::default());
    println!("PREMIER RENDU sans sonde : {:?}", chrono.elapsed());

    let chrono = std::time::Instant::now();
    let _ = moteur.render(&message(20), 700.0, &mut iris_htmlview::VecSink::default());
    println!("second rendu, device déjà ouvert : {:?}", chrono.elapsed());

    let apres = releve("après un rendu");
    ecart(&depart, &apres, "TOTAL sans sonde");
}

/// L'image d'un message n'existe-t-elle qu'une fois ?
///
/// Elle existait deux fois : le moteur peignait dans un `Vec` à lui, que l'interface
/// recopiait dans le sien. Pour une infolettre longue affichée large — deux mille
/// quatre cents pixels sur six mille — cela faisait cinquante-huit mégaoctets
/// alloués deux fois, à l'ouverture du message, sur le fil de l'interface.
///
/// Le pic du tas pendant le rendu est la mesure exacte de ce nombre de copies. Le
/// seuil est à une copie et demie : au-delà, quelqu'un en a réintroduit une.
#[test]
#[ignore = "exige une carte graphique"]
#[cfg(feature = "blitz")]
fn une_image_de_message_n_existe_qu_une_fois() {
    use iris_htmlview::{BlitzRenderer, HtmlRenderer};

    let Some(moteur) = BlitzRenderer::probe(1.0, true) else {
        println!("aucun périphérique graphique : test ignoré");
        return;
    };

    // Un premier rendu pour que les réserves du moteur soient déjà prises : ce qu'on
    // mesure est le coût d'un message, pas celui du premier message.
    let mut amorce = iris_ui::bridge::ImageSink::default();
    let _ = moteur.render_with(&message(4), 700.0, false, &mut amorce);
    drop(amorce);

    let mut pixels = iris_ui::bridge::ImageSink::default();
    let socle = memory::reset_peak();
    let rendu = moteur
        .render_with(&message(60), 2400.0, false, &mut pixels)
        .expect("le rendu doit aboutir");

    let pic = memory::heap().peak.saturating_sub(socle);
    let iris_htmlview::Rendered::Texture { width, height } = rendu else {
        panic!("attendu une image");
    };
    let une_image = (width as usize) * (height as usize) * 4;

    println!(
        "texture {width}×{height} : une image = {:.1} Mo, pic du tas = {:.1} Mo ({:.2} image)",
        une_image as f64 / 1048576.0,
        pic as f64 / 1048576.0,
        pic as f64 / une_image as f64,
    );

    // L'ancien chemin, pour que le gain soit mesuré et non calculé : le moteur
    // peignait dans un tampon à lui, que l'interface recopiait ensuite dans le sien.
    let mut vec_sink = iris_htmlview::VecSink::default();
    let socle_avant = memory::reset_peak();
    let _ = moteur.render_with(&message(60), 2400.0, false, &mut vec_sink);
    let copie =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&vec_sink.0, width, height);
    let pic_ancien = memory::heap().peak.saturating_sub(socle_avant);
    println!(
        "   pour mémoire, l'ancien chemin : pic {:.1} Mo ({:.2} image)",
        pic_ancien as f64 / 1048576.0,
        pic_ancien as f64 / une_image as f64,
    );
    drop(copie);
    drop(vec_sink);

    assert!(
        pic < une_image * 3 / 2,
        "le rendu a retenu {pic} octets pour une image de {une_image} : une copie de trop"
    );
    assert!(
        pixels.image(width, height).is_some(),
        "et l'image doit être utilisable telle quelle"
    );
}

#[test]
#[ignore = "exige une carte graphique ; imprime une facture au lieu de vérifier un budget"]
fn ce_que_coute_la_fenetre() {
    use slint::ComponentHandle;

    let depart = releve("au départ");

    let fenetre = iris_ui::AppWindow::new().expect("la fenêtre doit se construire");
    let construite = releve("après construction de la fenêtre");
    ecart(&depart, &construite, "construire la fenêtre");

    // La fenêtre n'ouvre son périphérique graphique qu'à l'affichage. Ne pas la
    // montrer mesurerait une fenêtre qui n'a pas encore coûté ce qu'elle coûte.
    fenetre.show().expect("affichage");
    slint::platform::update_timers_and_animations();
    let affichee = releve("après affichage");
    ecart(&construite, &affichee, "afficher (device + textures)");

    fenetre.hide().expect("masquage");
}
