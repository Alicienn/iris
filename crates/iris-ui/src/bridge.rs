//! Le pont entre le vue-modèle et l'interface Slint.
//!
//! Une seule règle : **rien n'est calculé de ce côté-ci de la frontière**. Les
//! structures transmises à l'interface sont déjà formatées, déjà triées, déjà
//! filtrées. Tout ce qui se trouve ici est une conversion de type, jamais une
//! décision.

use crate::format::{account_tint, display_subject, grouped_count, relative_date, short_count};
use crate::{
    AccountRowData, AttachmentData, BodyTileData, CommandData, MessageBlockData, MessageData,
    ThreadRowData,
};
use iris_htmlview::Rendered;
use iris_htmlview::{Block, RichText};
use iris_store::{Account, StoredMessage, ThreadRow};
use iris_theme::Theme;
use iris_types::{Flags, Timestamp};
use slint::{Color, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

/// Convertit une ligne du store en ligne affichable.
pub fn thread_row(
    row: &ThreadRow,
    account_email: &str,
    now: Timestamp,
    marked: bool,
) -> ThreadRowData {
    let (r, g, b) = account_tint(account_email);

    ThreadRowData {
        id: row.id.get() as i32,
        from: row.from_display.as_str().into(),
        subject: display_subject(&row.subject).into(),
        preview: row.preview.as_str().into(),
        date: relative_date(row.last_activity, now).into(),
        unread: row.is_unread(),
        flagged: row.flags_union.contains(Flags::FLAGGED),
        has_attachment: row.has_attachment(),
        has_tracker: row.flags_union.contains(Flags::HAS_TRACKER),
        snoozed: row.snoozed_until.is_some(),
        marked,
        message_count: row.message_count as i32,
        account_tint: Color::from_rgb_u8(r, g, b),
    }
}

/// Convertit un compte en ligne de barre latérale.
pub fn account_row(account: &Account, unread: u32, suspended: bool) -> AccountRowData {
    let (r, g, b) = account_tint(&account.email);

    AccountRowData {
        id: account.id.get() as i32,
        // Le nom d'affichage s'il existe, sinon l'adresse : sur cent boîtes, « Marie
        // — facturation » se repère mieux que « facturation@entreprise-machin.fr ».
        label: if account.display_name.trim().is_empty() {
            account.email.as_str().into()
        } else {
            account.display_name.as_str().into()
        },
        count: unread as i32,
        count_label: short_count(unread as u64).into(),
        count_full: grouped_count(unread as u64).into(),
        pinned: account.pinned,
        needs_attention: suspended,
        problem: Default::default(),
        tint: Color::from_rgb_u8(r, g, b),
    }
}

/// Convertit un corps rendu en blocs affichables.
pub fn message_blocks(rich: &RichText) -> Vec<MessageBlockData> {
    rich.blocks
        .iter()
        .map(|b| match b {
            Block::Paragraph(spans) => MessageBlockData {
                kind: "paragraph".into(),
                text: join(spans).into(),
                level: 0,
                depth: 0,
                bold: spans.iter().all(|s| s.bold) && !spans.is_empty(),
                italic: spans.iter().all(|s| s.italic) && !spans.is_empty(),
                link: spans
                    .iter()
                    .find_map(|s| s.link.clone())
                    .unwrap_or_default()
                    .into(),
                ..Default::default()
            },
            Block::Heading { level, spans } => MessageBlockData {
                kind: "heading".into(),
                text: join(spans).into(),
                level: *level as i32,
                depth: 0,
                bold: true,
                italic: false,
                link: SharedString::default(),
                ..Default::default()
            },
            Block::ListItem { depth, spans, .. } => MessageBlockData {
                kind: "list".into(),
                text: join(spans).into(),
                level: 0,
                depth: *depth as i32,
                bold: false,
                italic: false,
                link: SharedString::default(),
                ..Default::default()
            },
            Block::Quote { depth, spans } => MessageBlockData {
                kind: "quote".into(),
                text: join(spans).into(),
                level: 0,
                depth: *depth as i32,
                bold: false,
                italic: false,
                link: SharedString::default(),
                ..Default::default()
            },
            Block::Code(text) => MessageBlockData {
                kind: "code".into(),
                text: text.as_str().into(),
                level: 0,
                depth: 0,
                bold: false,
                italic: false,
                link: SharedString::default(),
                ..Default::default()
            },
            Block::Rule => MessageBlockData {
                kind: "rule".into(),
                text: SharedString::default(),
                level: 0,
                depth: 0,
                bold: false,
                italic: false,
                link: SharedString::default(),
                ..Default::default()
            },
            Block::Image {
                alt,
                blocked,
                pixels,
            } => MessageBlockData {
                kind: "image".into(),
                // Sans texte de remplacement, il n'y a pas de tiret à mettre : la
                // plupart des images bloquées sont des pixels de suivi, qui n'en
                // portent jamais, et la ligne commençait donc par « — » précédé d'une
                // espace, comme une phrase à laquelle on aurait coupé le début.
                text: if *blocked {
                    if alt.is_empty() {
                        "Remote image blocked".into()
                    } else {
                        format!("{alt} — remote image blocked").into()
                    }
                } else {
                    alt.as_str().into()
                },
                level: 0,
                depth: 0,
                bold: false,
                italic: true,
                link: SharedString::default(),
                // Le logo de signature que le message portait déjà. Sans cela, dix
                // messages signés de trois pastilles donnaient trente lignes montrant
                // une icône et le mot « image ».
                picture: pixels
                    .as_ref()
                    .and_then(|p| image_incrustee(p.width, p.height, &p.rgba))
                    .unwrap_or_default(),
                has_picture: pixels.is_some(),
            },
            Block::TableRow(cells) => MessageBlockData {
                kind: "table".into(),
                text: cells
                    .iter()
                    .map(|c| join(c))
                    .collect::<Vec<_>>()
                    .join("    ")
                    .into(),
                level: 0,
                depth: 0,
                bold: false,
                italic: false,
                link: SharedString::default(),
                ..Default::default()
            },
        })
        .collect()
}

fn join(spans: &[iris_htmlview::Inline]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

/// Une image que le message transportait lui-meme, prete a dessiner.
///
/// La copie demeure ici, et c'est normal : ces pixels-la appartiennent au bloc de
/// texte riche qui les porte, et ils pesent quelques kilo-octets — un logo de
/// signature, une pastille. Ce qui a ete supprime est la copie de l'autre image,
/// celle du corps entier, qui se compte en dizaines de megaoctets.
fn image_incrustee(width: u32, height: u32, rgba: &[u8]) -> Option<Image> {
    let attendu = (width as usize) * (height as usize) * 4;
    if rgba.len() != attendu || attendu == 0 {
        // Une image mal dimensionnee vaut mieux refusee qu'affichee de travers.
        return None;
    }
    Some(Image::from_rgba8(
        SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba, width, height),
    ))
}

/// Le tampon que le moteur de rendu remplit : celui de l'interface, directement.
///
/// La copie « inevitable » ne l'etait pas. Le moteur ecrivait dans un tampon a lui,
/// que l'interface recopiait ensuite dans le sien : deux fois la meme image en
/// memoire, et pour une infolettre longue affichee large, cinquante-huit megaoctets
/// payes deux fois, au moment precis ou l'on ouvre un message. Le moteur reclame
/// desormais ce tampon-ci une fois la hauteur connue, et peint dedans.
#[derive(Default)]
pub struct ImageSink {
    tampon: Option<SharedPixelBuffer<Rgba8Pixel>>,
}

impl std::fmt::Debug for ImageSink {
    /// Sans les pixels : un `dbg!` sur un rendu ne doit pas cracher un megaoctet.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageSink")
            .field(
                "taille",
                &self.tampon.as_ref().map(|t| (t.width(), t.height())),
            )
            .finish()
    }
}

impl ImageSink {
    /// L'image peinte, si elle a bien la taille que le rendu annonce.
    ///
    /// La verification reste : un tampon d'une taille et un rendu d'une autre
    /// signifient qu'un moteur s'est trompe, et une image affichee de travers est
    /// pire qu'une absence d'image.
    pub fn image(self, width: u32, height: u32) -> Option<Image> {
        let tampon = self.tampon?;
        if tampon.width() != width || tampon.height() != height || width == 0 || height == 0 {
            return None;
        }
        Some(Image::from_rgba8(tampon))
    }
}

impl iris_htmlview::PixelSink for ImageSink {
    fn rgba(&mut self, width: u32, height: u32) -> &mut [u8] {
        self.tampon
            .insert(SharedPixelBuffer::new(width, height))
            .make_mut_bytes()
    }
}

/// Compose la vue d'un message a partir d'un rendu, quelle qu'en soit la forme.
///
/// Un document peint par tuiles n'apporte ici que sa forme : les tuiles, toutes a
/// leur place et sans pixels. C'est l'appelant, qui garde le document, qui les
/// peindra quand elles approcheront de l'ecran.
pub fn message_view_rendered(
    message: &StoredMessage,
    rendered: &Rendered,
    attachments: &[AttachmentData],
    now: Timestamp,
) -> MessageData {
    match rendered {
        Rendered::Blocks(blocs) => message_view(message, blocs, attachments, now),
        Rendered::Document(document) => {
            let mut vue = message_view(message, &RichText::default(), attachments, now);
            vue.body_tiles = ModelRc::new(VecModel::from(tile_placeholders(document.as_ref())));
            vue.body_is_image = true;
            vue
        }
    }
}

/// Les tuiles d'un document, a leur place et sans pixels.
pub fn tile_placeholders(document: &dyn iris_htmlview::TiledDocument) -> Vec<BodyTileData> {
    let (largeur, _) = document.size();
    (0..document.tile_count())
        .map(|i| BodyTileData {
            image: Image::default(),
            ready: false,
            aspect: document.tile_extent(i) as f32 / largeur.max(1) as f32,
        })
        .collect()
}

/// Compose la vue d'un message.
pub fn message_view(
    message: &StoredMessage,
    body: &RichText,
    attachments: &[AttachmentData],
    now: Timestamp,
) -> MessageData {
    MessageData {
        // Sans rendu par image, ces deux champs restent inertes.
        body_tiles: ModelRc::default(),
        body_is_image: false,
        body_loading: false,
        from: if message.from_name.trim().is_empty() {
            message.from_addr.as_str().into()
        } else {
            message.from_name.as_str().into()
        },
        from_address: message.from_addr.as_str().into(),
        to: SharedString::default(),
        date: relative_date(message.received, now).into(),
        subject: display_subject(&message.subject).into(),
        blocks: ModelRc::new(VecModel::from(message_blocks(body))),
        blocked_images: body.blocked_images as i32,
        has_tracker: message.flags.contains(Flags::HAS_TRACKER),
        attachments: ModelRc::new(VecModel::from(attachments.to_vec())),
        id: message.id.get() as i32,
        // Déplié par défaut : cette fonction n'est appelée que pour un message dont on
        // veut le corps. Les messages repliés passent par `message_header`, qui ne
        // rend rien.
        expanded: true,
        preview: message.preview.as_str().into(),
    }
}

/// L'en-tête seul d'un message, sans son corps.
///
/// Ce qu'un fil montre de ses messages précédents. La distinction n'est pas cosmétique
/// : rendre un corps coûte une rasterisation, et un échange de douze messages dont on
/// lit un en paierait onze pour rien. L'aperçu suffit à retrouver le bon, et le corps
/// arrive quand on le déplie.
pub fn message_header(message: &StoredMessage, now: Timestamp) -> MessageData {
    MessageData {
        body_tiles: ModelRc::default(),
        body_is_image: false,
        body_loading: false,
        from: if message.from_name.trim().is_empty() {
            message.from_addr.as_str().into()
        } else {
            message.from_name.as_str().into()
        },
        from_address: message.from_addr.as_str().into(),
        to: SharedString::default(),
        date: relative_date(message.received, now).into(),
        subject: display_subject(&message.subject).into(),
        blocks: ModelRc::new(VecModel::from(Vec::<MessageBlockData>::new())),
        blocked_images: 0,
        has_tracker: message.flags.contains(Flags::HAS_TRACKER),
        attachments: ModelRc::new(VecModel::from(Vec::<AttachmentData>::new())),
        id: message.id.get() as i32,
        expanded: false,
        preview: message.preview.as_str().into(),
    }
}

/// Convertit une commande filtrée en entrée de palette.
pub fn command_row(command: &crate::commands::Command) -> CommandData {
    CommandData {
        id: command.id.as_str().into(),
        label: command.label.as_str().into(),
        shortcut: command.shortcut.as_str().into(),
        group: command.group.as_str().into(),
    }
}

/// Recopie un thème dans les tokens de l'interface.
///
/// C'est le seul endroit où le thème traverse la frontière. Après cet appel,
/// l'ensemble de l'interface se redessine avec les nouvelles valeurs, sans qu'aucun
/// composant n'ait à être prévenu.
pub fn apply_theme(tokens: &crate::Tokens<'_>, theme: &Theme) {
    let couleur = |c: iris_theme::Color| Color::from_argb_u8(c.a, c.r, c.g, c.b);

    tokens.set_background(couleur(theme.color.background));
    tokens.set_glow(couleur(theme.color.glow));
    tokens.set_surface_high(couleur(theme.color.surface_high));
    tokens.set_surface(couleur(theme.color.surface));
    tokens.set_surface_low(couleur(theme.color.surface_low));
    tokens.set_surface_hover(couleur(theme.color.surface_hover));
    tokens.set_surface_active(couleur(theme.color.surface_active));
    tokens.set_border(couleur(theme.color.border));
    tokens.set_border_strong(couleur(theme.color.border_strong));
    tokens.set_edge_light(couleur(theme.color.edge_light));
    tokens.set_text(couleur(theme.color.text));
    tokens.set_text_secondary(couleur(theme.color.text_secondary));
    tokens.set_text_muted(couleur(theme.color.text_muted));
    tokens.set_accent(couleur(theme.color.accent));
    tokens.set_accent_text(couleur(theme.color.accent_text));
    tokens.set_error(couleur(theme.color.error));
    tokens.set_warning(couleur(theme.color.warning));
    tokens.set_success(couleur(theme.color.success));

    tokens.set_radius_small(theme.radius.small);
    tokens.set_radius_medium(theme.radius.medium);
    tokens.set_radius_large(theme.radius.large);
    tokens.set_unit(theme.spacing.unit);

    tokens.set_font_family(theme.typography.family.as_str().into());
    tokens.set_font_mono(theme.typography.family_mono.as_str().into());
    tokens.set_size_small(theme.typography.size_small);
    tokens.set_size_body(theme.typography.size_body);
    tokens.set_size_title(theme.typography.size_title);
    tokens.set_weight_body(theme.typography.weight_body as i32);
    tokens.set_weight_bold(theme.typography.weight_bold as i32);
    tokens.set_line_height(theme.typography.line_height);

    tokens.set_row_height(theme.density.row_height);
    tokens.set_row_padding_x(theme.density.row_padding_x);

    tokens.set_glass_opacity(theme.glass.opacity);
    tokens.set_grain(theme.glass.grain);

    // Slint exprime les durées en millisecondes entières, comme les tokens.
    tokens.set_instant(theme.motion.instant.round() as i64);
    tokens.set_quick(theme.motion.quick.round() as i64);
    tokens.set_moderate(theme.motion.moderate.round() as i64);
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_htmlview::Inline;
    use iris_types::{AccountId, FolderId, MessageId, ThreadId, WorkflowState};

    fn now() -> Timestamp {
        Timestamp::from_millis(1_700_000_000_000)
    }

    fn ligne() -> ThreadRow {
        ThreadRow {
            id: ThreadId(7),
            state: WorkflowState::Todo,
            last_activity: now(),
            from_display: "Marie".into(),
            subject: "Devis refonte".into(),
            preview: "Bonjour, voici…".into(),
            message_count: 3,
            unread_count: 1,
            flags_union: Flags::HAS_ATTACHMENT | Flags::FLAGGED,
            snoozed_until: None,
            account: iris_types::AccountId(1),
        }
    }

    #[test]
    fn une_ligne_est_convertie_avec_ses_marqueurs() {
        let d = thread_row(&ligne(), "marie@example.com", now(), false);
        assert_eq!(d.id, 7);
        assert_eq!(d.from.as_str(), "Marie");
        assert_eq!(d.subject.as_str(), "Devis refonte");
        assert!(d.unread);
        assert!(d.flagged);
        assert!(d.has_attachment);
        assert!(!d.has_tracker);
        assert_eq!(d.message_count, 3);
    }

    #[test]
    fn la_date_arrive_deja_formatee() {
        // L'interface ne doit jamais formater : elle referait ce travail à chaque
        // frame de défilement.
        let d = thread_row(&ligne(), "a@x.fr", now(), false);
        assert_eq!(d.date.as_str(), "22:13");
    }

    #[test]
    fn un_sujet_vide_est_annonce() {
        let mut l = ligne();
        l.subject = String::new();
        assert_eq!(
            thread_row(&l, "a@x.fr", now(), false).subject.as_str(),
            "(sans objet)"
        );
    }

    #[test]
    fn deux_comptes_donnent_deux_teintes() {
        let a = thread_row(&ligne(), "contact@x.fr", now(), false).account_tint;
        let b = thread_row(&ligne(), "facturation@x.fr", now(), false).account_tint;
        assert_ne!(
            (a.red(), a.green(), a.blue()),
            (b.red(), b.green(), b.blue())
        );
    }

    fn compte(email: &str, nom: &str) -> Account {
        Account {
            id: AccountId(1),
            email: email.into(),
            display_name: nom.into(),
            imap_host: "i".into(),
            imap_port: 993,
            imap_tls: true,
            smtp_host: "s".into(),
            smtp_port: 465,
            smtp_tls: true,
            auth: iris_store::AuthKind::Password,
            group: None,
            pinned: true,
            enabled: true,
            created_at: Timestamp::EPOCH,
            last_activity_at: Timestamp::EPOCH,
            signature: String::new(),
        }
    }

    #[test]
    fn un_compte_affiche_son_nom_quand_il_en_a_un() {
        let d = account_row(
            &compte("facturation@entreprise.fr", "Facturation"),
            4,
            false,
        );
        assert_eq!(d.label.as_str(), "Facturation");
        assert_eq!(d.count, 4);
        assert!(d.pinned);
        assert!(!d.needs_attention);
    }

    #[test]
    fn un_compte_sans_nom_affiche_son_adresse() {
        let d = account_row(&compte("facturation@entreprise.fr", "  "), 0, false);
        assert_eq!(d.label.as_str(), "facturation@entreprise.fr");
    }

    #[test]
    fn un_compte_suspendu_est_signale() {
        let d = account_row(&compte("a@x.fr", "A"), 0, true);
        assert!(d.needs_attention);
    }

    #[test]
    fn les_blocs_du_corps_sont_convertis_avec_leur_nature() {
        let rich = RichText {
            blocks: vec![
                Block::Heading {
                    level: 2,
                    spans: vec![Inline::plain("Titre")],
                },
                Block::Paragraph(vec![Inline::plain("Corps")]),
                Block::Quote {
                    depth: 2,
                    spans: vec![Inline::plain("Cité")],
                },
                Block::Rule,
            ],
            blocked_images: 0,
        };
        let blocs = message_blocks(&rich);

        assert_eq!(blocs[0].kind.as_str(), "heading");
        assert_eq!(blocs[0].level, 2);
        assert_eq!(blocs[1].kind.as_str(), "paragraph");
        assert_eq!(blocs[2].kind.as_str(), "quote");
        assert_eq!(blocs[2].depth, 2);
        assert_eq!(blocs[3].kind.as_str(), "rule");
    }

    #[test]
    fn une_image_bloquee_est_annoncee_explicitement() {
        let rich = RichText {
            blocks: vec![Block::Image {
                alt: "Bannière".into(),
                blocked: true,
                pixels: None,
            }],
            blocked_images: 1,
        };
        let blocs = message_blocks(&rich);
        assert!(blocs[0].text.as_str().contains("blocked"));
        assert!(
            !blocs[0].has_picture,
            "une image bloquée n'a rien à montrer, c'est tout l'intérêt"
        );
    }

    #[test]
    fn les_cellules_d_un_tableau_sont_aplaties() {
        let rich = RichText {
            blocks: vec![Block::TableRow(vec![
                vec![Inline::plain("Janvier")],
                vec![Inline::plain("1200 €")],
            ])],
            blocked_images: 0,
        };
        let blocs = message_blocks(&rich);
        assert!(blocs[0].text.as_str().contains("Janvier"));
        assert!(blocs[0].text.as_str().contains("1200 €"));
    }

    #[test]
    fn un_corps_vide_ne_produit_aucun_bloc() {
        assert!(message_blocks(&RichText::default()).is_empty());
    }

    #[test]
    fn la_vue_d_un_message_reprend_l_expediteur_et_les_pieces_jointes() {
        let message = StoredMessage {
            id: MessageId(1),
            account: AccountId(1),
            folder: FolderId(1),
            thread: ThreadId(1),
            uid: 1,
            rfc_message_id: None,
            subject: "Devis".into(),
            from_name: "Marie".into(),
            from_addr: "marie@example.com".into(),
            date: now(),
            received: now(),
            size: 100,
            flags: Flags::HAS_TRACKER,
            preview: String::new(),
            body_blob: None,
        };
        let vue = message_view(
            &message,
            &RichText::default(),
            &[AttachmentData {
                name: "devis.pdf".into(),
                size: "2,4 Mo".into(),
                kind: "PDF".into(),
                icon: "file-text".into(),
            }],
            now(),
        );

        assert_eq!(vue.from.as_str(), "Marie");
        assert_eq!(vue.from_address.as_str(), "marie@example.com");
        assert!(vue.has_tracker);
    }

    #[test]
    fn un_corps_rendu_en_image_est_transmis_comme_tel() {
        let message = StoredMessage {
            id: MessageId(1),
            account: AccountId(1),
            folder: FolderId(1),
            thread: ThreadId(1),
            uid: 1,
            rfc_message_id: None,
            subject: "Infolettre".into(),
            from_name: "Boutique".into(),
            from_addr: "news@x.fr".into(),
            date: now(),
            received: now(),
            size: 100,
            flags: Flags::NONE,
            preview: String::new(),
            body_blob: None,
        };
        #[derive(Debug)]
        struct Doc;
        impl iris_htmlview::TiledDocument for Doc {
            fn size(&self) -> (u32, u32) {
                (400, 1000)
            }
            fn tile_height(&self) -> u32 {
                512
            }
            fn paint_tile(
                &mut self,
                _i: usize,
                _p: &mut dyn iris_htmlview::PixelSink,
            ) -> iris_types::Result<()> {
                Ok(())
            }
            fn release(&mut self) {}
        }
        use slint::Model;
        let rendu = Rendered::Document(Box::new(Doc));
        let vue = message_view_rendered(&message, &rendu, &[], now());

        assert!(vue.body_is_image);
        assert_eq!(vue.body_tiles.row_count(), 2);
        let premiere = vue.body_tiles.row_data(0).unwrap();
        assert!(!premiere.ready, "aucun pixel avant d'être demandé");
        assert!((premiere.aspect - 512.0 / 400.0).abs() < 1e-6);
        let derniere = vue.body_tiles.row_data(1).unwrap();
        assert!((derniere.aspect - 488.0 / 400.0).abs() < 1e-6);
    }

    #[test]
    fn une_image_mal_dimensionnee_est_refusee() {
        // Mieux vaut retomber sur les blocs qu'afficher une image de travers.
        use iris_htmlview::PixelSink;

        let mut pixels = ImageSink::default();
        pixels.rgba(4, 2);
        // Le rendu annonce une taille que le tampon n'a pas.
        assert!(pixels.image(8, 2).is_none());

        assert!(ImageSink::default().image(4, 2).is_none(), "aucun tampon");

        let mut vide = ImageSink::default();
        vide.rgba(0, 0);
        assert!(vide.image(0, 0).is_none());
    }

    #[test]
    fn un_corps_en_blocs_n_active_pas_l_image() {
        let message = StoredMessage {
            id: MessageId(1),
            account: AccountId(1),
            folder: FolderId(1),
            thread: ThreadId(1),
            uid: 1,
            rfc_message_id: None,
            subject: "Devis".into(),
            from_name: "Marie".into(),
            from_addr: "marie@x.fr".into(),
            date: now(),
            received: now(),
            size: 10,
            flags: Flags::NONE,
            preview: String::new(),
            body_blob: None,
        };
        let vue =
            message_view_rendered(&message, &Rendered::Blocks(RichText::default()), &[], now());
        assert!(!vue.body_is_image);
    }

    #[test]
    fn une_commande_est_convertie_pour_la_palette() {
        let commandes = crate::commands::builtin_commands();
        let d = command_row(&commandes[0]);
        assert!(!d.id.is_empty());
        assert!(!d.label.is_empty());
    }
}
