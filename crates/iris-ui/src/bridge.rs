//! Le pont entre le vue-modèle et l'interface Slint.
//!
//! Une seule règle : **rien n'est calculé de ce côté-ci de la frontière**. Les
//! structures transmises à l'interface sont déjà formatées, déjà triées, déjà
//! filtrées. Tout ce qui se trouve ici est une conversion de type, jamais une
//! décision.

use crate::format::{account_tint, display_subject, relative_date};
use crate::{AccountRowData, CommandData, MessageBlockData, MessageData, ThreadRowData};
use iris_htmlview::{Block, RichText};
use iris_store::{Account, StoredMessage, ThreadRow};
use iris_theme::Theme;
use iris_types::{Flags, Timestamp};
use slint::{Color, ModelRc, SharedString, VecModel};

/// Convertit une ligne du store en ligne affichable.
pub fn thread_row(row: &ThreadRow, account_email: &str, now: Timestamp) -> ThreadRowData {
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
        pinned: account.pinned,
        needs_attention: suspended,
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
            },
            Block::Heading { level, spans } => MessageBlockData {
                kind: "heading".into(),
                text: join(spans).into(),
                level: *level as i32,
                depth: 0,
                bold: true,
                italic: false,
                link: SharedString::default(),
            },
            Block::ListItem { depth, spans, .. } => MessageBlockData {
                kind: "list".into(),
                text: join(spans).into(),
                level: 0,
                depth: *depth as i32,
                bold: false,
                italic: false,
                link: SharedString::default(),
            },
            Block::Quote { depth, spans } => MessageBlockData {
                kind: "quote".into(),
                text: join(spans).into(),
                level: 0,
                depth: *depth as i32,
                bold: false,
                italic: false,
                link: SharedString::default(),
            },
            Block::Code(text) => MessageBlockData {
                kind: "code".into(),
                text: text.as_str().into(),
                level: 0,
                depth: 0,
                bold: false,
                italic: false,
                link: SharedString::default(),
            },
            Block::Rule => MessageBlockData {
                kind: "rule".into(),
                text: SharedString::default(),
                level: 0,
                depth: 0,
                bold: false,
                italic: false,
                link: SharedString::default(),
            },
            Block::Image { alt, blocked } => MessageBlockData {
                kind: "image".into(),
                text: if *blocked {
                    format!("{alt} — image distante bloquée").into()
                } else {
                    alt.as_str().into()
                },
                level: 0,
                depth: 0,
                bold: false,
                italic: true,
                link: SharedString::default(),
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
            },
        })
        .collect()
}

fn join(spans: &[iris_htmlview::Inline]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

/// Compose la vue d'un message.
pub fn message_view(
    message: &StoredMessage,
    body: &RichText,
    attachments: &[String],
    now: Timestamp,
) -> MessageData {
    MessageData {
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
        attachments: ModelRc::new(VecModel::from(
            attachments.iter().map(SharedString::from).collect::<Vec<_>>(),
        )),
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
        }
    }

    #[test]
    fn une_ligne_est_convertie_avec_ses_marqueurs() {
        let d = thread_row(&ligne(), "marie@example.com", now());
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
        let d = thread_row(&ligne(), "a@x.fr", now());
        assert_eq!(d.date.as_str(), "22:13");
    }

    #[test]
    fn un_sujet_vide_est_annonce() {
        let mut l = ligne();
        l.subject = String::new();
        assert_eq!(thread_row(&l, "a@x.fr", now()).subject.as_str(), "(sans objet)");
    }

    #[test]
    fn deux_comptes_donnent_deux_teintes() {
        let a = thread_row(&ligne(), "contact@x.fr", now()).account_tint;
        let b = thread_row(&ligne(), "facturation@x.fr", now()).account_tint;
        assert_ne!((a.red(), a.green(), a.blue()), (b.red(), b.green(), b.blue()));
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
        }
    }

    #[test]
    fn un_compte_affiche_son_nom_quand_il_en_a_un() {
        let d = account_row(&compte("facturation@entreprise.fr", "Facturation"), 4, false);
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
                Block::Heading { level: 2, spans: vec![Inline::plain("Titre")] },
                Block::Paragraph(vec![Inline::plain("Corps")]),
                Block::Quote { depth: 2, spans: vec![Inline::plain("Cité")] },
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
            blocks: vec![Block::Image { alt: "Bannière".into(), blocked: true }],
            blocked_images: 1,
        };
        let blocs = message_blocks(&rich);
        assert!(blocs[0].text.as_str().contains("bloquée"));
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
        let vue = message_view(&message, &RichText::default(), &["devis.pdf".to_string()], now());

        assert_eq!(vue.from.as_str(), "Marie");
        assert_eq!(vue.from_address.as_str(), "marie@example.com");
        assert!(vue.has_tracker);
    }

    #[test]
    fn une_commande_est_convertie_pour_la_palette() {
        let commandes = crate::commands::builtin_commands();
        let d = command_row(&commandes[0]);
        assert!(!d.id.is_empty());
        assert!(!d.label.is_empty());
    }
}
