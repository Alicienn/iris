//! Un aperçu du courrier — la colonne des comptes rangés par tag et la liste des
//! messages, dans les trois densités — sur des données inventées, capturé en images.
//!
//! ```text
//! $env:SLINT_BACKEND="winit-software"; cargo run -p iris-app --example apercu_courrier -- <dossier>
//! ```
//!
//! Écrit `courrier-compact.png`, `courrier-normal.png` et `courrier-confort.png`.

use iris_app::services::{now, Services};
use iris_app::settings::Density;
use iris_store::NewAccount;
use slint::{ComponentHandle, ModelRc, VecModel};

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

fn ligne(
    id: i32,
    de: &str,
    sujet: &str,
    extrait: &str,
    date: &str,
    non_lu: bool,
) -> iris_ui::ThreadRowData {
    iris_ui::ThreadRowData {
        id,
        from: de.into(),
        subject: sujet.into(),
        preview: extrait.into(),
        date: date.into(),
        unread: non_lu,
        message_count: 1,
        account_tint: slint::Color::from_rgb_u8(0x5b, 0x8d, 0xef),
        initials: de.chars().take(1).collect::<String>().to_uppercase().into(),
        sender_tint: slint::Color::from_rgb_u8(0xe0, 0x79, 0x5b),
        ..Default::default()
    }
}

fn main() {
    let sortie = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let dir = tempfile::tempdir().unwrap();
    let services = Services::open(
        iris_app::paths::Paths::under(dir.path()),
        Some(iris_secrets::Secret::new("apercu")),
    )
    .unwrap();

    let compte = |adresse: &str| {
        services
            .store
            .create_account(
                &NewAccount::new(adresse, "imap.example.com", "smtp.example.com"),
                now(),
            )
            .unwrap()
    };
    let contact = compte("contact@atelier.example.com");
    let factures = compte("factures@atelier.example.com");
    let moi = compte("marie@example.com");
    compte("archives@example.com");
    let clients = services
        .store
        .create_account_tag("Clients", iris_app::tags::PALETTE[2], now())
        .unwrap();
    let perso = services
        .store
        .create_account_tag("Personal", iris_app::tags::PALETTE[0], now())
        .unwrap();
    for (c, t) in [(contact, clients), (factures, clients), (moi, perso)] {
        services.store.set_account_tagged(c, t, true).unwrap();
    }

    let f = iris_ui::AppWindow::new().unwrap();
    f.window().set_size(slint::LogicalSize::new(1280.0, 800.0));
    f.set_group_by_tags(true);
    iris_app::shell::refresh_accounts(&f, &services, &[]);
    f.set_loading(false);
    f.set_rows(ModelRc::new(VecModel::from(vec![
        ligne(
            1,
            "Jérôme Gauthier",
            "Devis façade — ajustements",
            "Bonjour, je joins la version corrigée, avec les quantités",
            "14:32",
            true,
        ),
        ligne(
            2,
            "Agnès Joly",
            "Réunion de jeudi",
            "Pouvez-vous confirmer l'horaire ? J'aurai besoin du projecteur",
            "12:05",
            false,
        ),
        ligne(
            3,
            "Banque",
            "Votre relevé de septembre",
            "Votre relevé est disponible dans votre espace client, rubrique",
            "hier",
            false,
        ),
        ligne(
            4,
            "Paul Jégou",
            "Logo — dernière version",
            "Voilà le logo en SVG, j'ai ajusté l'espacement du jambage",
            "lun.",
            true,
        ),
    ])));
    f.set_counts(ModelRc::new(VecModel::from(vec![18, 4, 0])));
    f.set_count_labels(ModelRc::new(VecModel::from(vec![
        slint::SharedString::from("18"),
        "4".into(),
        "0".into(),
    ])));

    // An open conversation: an earlier message folded, the last one read.
    let paragraphe = |t: &str| iris_ui::MessageBlockData {
        kind: "paragraph".into(),
        text: t.into(),
        ..Default::default()
    };
    let dernier = iris_ui::MessageData {
        id: 11,
        from: "Jérôme Gauthier".into(),
        from_address: "jerome@facades.example.com".into(),
        to: "me".into(),
        date: "Today 14:32".into(),
        subject: "Devis façade — ajustements".into(),
        expanded: true,
        blocks: ModelRc::new(VecModel::from(vec![
            paragraphe("Bonjour,"),
            paragraphe(
                "Je joins la version corrigée avec les quantités d'enduit revues à la \
                 baisse, et l'option avec échafaudage compris.",
            ),
            paragraphe("Bien à vous,\nJérôme"),
        ])),
        attachments: ModelRc::new(VecModel::from(vec![iris_ui::AttachmentData {
            name: "devis-facade-v2.pdf".into(),
            size: "240 KB".into(),
            kind: "PDF".into(),
            icon: "file-text".into(),
        }])),
        ..Default::default()
    };
    let premier = iris_ui::MessageData {
        id: 10,
        from: "Jérôme Gauthier".into(),
        date: "Thu".into(),
        preview: "Voici le devis pour la façade, en deux options comme convenu".into(),
        expanded: false,
        ..Default::default()
    };
    f.set_selected_thread(1);
    f.set_message(dernier.clone());
    f.set_messages(ModelRc::new(VecModel::from(vec![premier, dernier])));
    f.set_conversation_empty(false);
    f.show().unwrap();

    let theme = services.themes.active();
    let etapes = slint::Timer::default();
    let faible = f.as_weak();
    let mut tour = 0;
    etapes.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(600),
        move || {
            let Some(f) = faible.upgrade() else { return };
            tour += 1;
            match tour {
                1 => iris_app::shell::appliquer_apparence(&f, &theme, Density::Compact),
                2 => {
                    capture(&f, sortie.join("courrier-compact.png"));
                    iris_app::shell::appliquer_apparence(&f, &theme, Density::Normal);
                }
                3 => {
                    capture(&f, sortie.join("courrier-normal.png"));
                    iris_app::shell::appliquer_apparence(&f, &theme, Density::Comfortable);
                }
                4 => {
                    capture(&f, sortie.join("courrier-confort.png"));
                    let _ = slint::quit_event_loop();
                }
                _ => {}
            }
        },
    );
    slint::run_event_loop_until_quit().unwrap();
}
