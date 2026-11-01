//! Le badge du bouton de la barre des tâches, dessiné comme Iris le pose, sur une
//! barre sombre puis claire, agrandi huit fois pour être regardé.
//!
//! ```text
//! cargo run -p iris-app --no-default-features --example apercu_badge -- <dossier>
//! ```
//!
//! Écrit `badge.png` : 3, 42 et 99+ aux tailles de 100 %, 150 % et 200 %.

#[cfg(windows)]
fn main() {
    let sortie = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let tailles = [16usize, 24, 32];
    let textes = ["3", "42", "99+"];
    // Une case par badge, sa barre derrière : sombre en haut, claire en bas.
    let case = 40usize;
    let (largeur, hauteur) = (case * tailles.len() * textes.len(), case * 2);
    let mut image = image::RgbaImage::new(largeur as u32, hauteur as u32);
    for (rangee, sombre) in [true, false].into_iter().enumerate() {
        let barre = if sombre {
            [0x20, 0x20, 0x20]
        } else {
            [0xee, 0xee, 0xee]
        };
        let (fond, encre) = iris_app::taskbar::colours(sombre);
        for (k, (texte, taille)) in textes
            .iter()
            .flat_map(|t| tailles.iter().map(move |s| (*t, *s)))
            .enumerate()
        {
            let px = iris_app::taskbar::draw(texte, taille, fond, encre);
            let (x0, y0) = (k * case, rangee * case);
            for y in 0..case {
                for x in 0..case {
                    image.put_pixel(
                        (x0 + x) as u32,
                        (y0 + y) as u32,
                        image::Rgba([barre[0], barre[1], barre[2], 255]),
                    );
                }
            }
            let decale = (case - taille) / 2;
            for y in 0..taille {
                for x in 0..taille {
                    let [b, v, r, a] = px[y * taille + x];
                    let melange = |c: u8, fond: u8| {
                        ((u32::from(c) * u32::from(a) + u32::from(fond) * (255 - u32::from(a)))
                            / 255) as u8
                    };
                    image.put_pixel(
                        (x0 + decale + x) as u32,
                        (y0 + decale + y) as u32,
                        image::Rgba([
                            melange(r, barre[0]),
                            melange(v, barre[1]),
                            melange(b, barre[2]),
                            255,
                        ]),
                    );
                }
            }
        }
    }
    let grand = image::imageops::resize(
        &image,
        largeur as u32 * 8,
        hauteur as u32 * 8,
        image::imageops::FilterType::Nearest,
    );
    let chemin = sortie.join("badge.png");
    grand.save(&chemin).unwrap();
    println!("capture : {}", chemin.display());
}

#[cfg(not(windows))]
fn main() {}
