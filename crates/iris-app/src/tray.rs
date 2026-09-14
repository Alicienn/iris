//! L'icône de la zone de notification.
//!
//! Ce qu'elle est vraiment : le seul endroit d'où Iris reste utile une fois la fenêtre
//! fermée. Un client de courrier qui synchronise en arrière-plan et qui disparaît
//! quand on ferme sa fenêtre ne synchronise rien du tout ; l'icône est ce qui rend la
//! fermeture réversible, et donc ce qui rend l'arrière-plan honnête.
//!
//! Trois gestes, et pas un de plus :
//!
//! - **clic gauche** : la fenêtre revient ;
//! - **clic droit** : ouvrir, ou quitter — les deux seules choses qu'on demande à une
//!   icône de zone de notification ;
//! - **survol** : le nombre de messages non lus, qui est la seule information qu'on y
//!   cherche et la raison pour laquelle on la regarde.
//!
//! Windows seulement pour l'instant. Le module entier se compile à vide ailleurs,
//! plutôt que d'être absent : l'appelant n'a alors pas à savoir sur quel système il
//! tourne.

#[cfg(windows)]
mod plateforme {
    use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
    use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

    /// Ce que l'utilisateur a demandé depuis la zone de notification.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TrayCommand {
        /// Montrer la fenêtre et la mettre au premier plan.
        Open,
        /// Quitter pour de bon.
        Quit,
    }

    /// L'icône, et de quoi la tenir à jour.
    pub struct Tray {
        // Gardée en vie : lâcher l'objet retire l'icône de la barre.
        icone: TrayIcon,
        ouvrir: MenuId,
        quitter: MenuId,
        /// Le dernier nombre affiché, pour ne pas réécrire l'infobulle à chaque
        /// instantané. Réécrire une infobulle identique fait clignoter la fenêtre
        /// contextuelle de Windows quand la souris est dessus.
        dernier: Option<u32>,
    }

    impl std::fmt::Debug for Tray {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Tray").field("unread", &self.dernier).finish()
        }
    }

    impl Tray {
        /// Installe l'icône.
        ///
        /// Échouer n'est pas fatal : une session sans zone de notification — un poste
        /// verrouillé par une stratégie de groupe, un shell de remplacement — reste
        /// une session où le courrier fonctionne.
        pub fn install() -> Option<Self> {
            let ouvrir = MenuItem::new("Open Iris", true, None);
            let quitter = MenuItem::new("Quit", true, None);

            let menu = Menu::new();
            menu.append(&ouvrir).ok()?;
            menu.append(&tray_icon::menu::PredefinedMenuItem::separator())
                .ok()?;
            menu.append(&quitter).ok()?;

            let icone = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip(tooltip(0))
                .with_icon(icone_de_l_executable()?)
                // Sur Windows le menu s'ouvre au clic droit tout seul ; laisser aussi
                // le clic gauche l'ouvrir enlèverait le geste qui sert à revenir à la
                // fenêtre, qui est celui qu'on fait le plus souvent.
                .with_menu_on_left_click(false)
                .build()
                .ok()?;

            Some(Self {
                icone,
                ouvrir: ouvrir.id().clone(),
                quitter: quitter.id().clone(),
                dernier: None,
            })
        }

        /// Met à jour le nombre de non-lus affiché au survol.
        pub fn set_unread(&mut self, unread: u32) {
            if self.dernier == Some(unread) {
                return;
            }
            self.dernier = Some(unread);
            let _ = self.icone.set_tooltip(Some(tooltip(unread)));
        }

        /// Relève ce qui s'est passé depuis le dernier tour.
        ///
        /// Interrogé plutôt qu'abonné : la boucle d'événements appartient à Slint, et
        /// un rappel qui arriverait depuis un autre fil n'aurait pas le droit de
        /// toucher à la fenêtre. Un relevé par tour de minuterie coûte deux lectures
        /// non bloquantes.
        pub fn poll(&self) -> Option<TrayCommand> {
            if let Ok(evenement) = MenuEvent::receiver().try_recv() {
                if evenement.id == self.ouvrir {
                    return Some(TrayCommand::Open);
                }
                if evenement.id == self.quitter {
                    return Some(TrayCommand::Quit);
                }
            }

            while let Ok(evenement) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Left,
                    button_state: tray_icon::MouseButtonState::Up,
                    ..
                } = evenement
                {
                    return Some(TrayCommand::Open);
                }
            }

            None
        }
    }

    /// Le texte du survol.
    ///
    /// Le nombre en toutes lettres plutôt qu'un chiffre nu : l'infobulle est lue hors
    /// de tout contexte, et « 12 » tout seul ne dit pas de quoi.
    fn tooltip(unread: u32) -> String {
        match unread {
            0 => "Iris — nothing unread".into(),
            1 => "Iris — 1 unread message".into(),
            n => format!("Iris — {n} unread messages"),
        }
    }

    /// L'icône déjà présente dans l'exécutable.
    ///
    /// `build.rs` en embarque une pour l'explorateur de fichiers et la barre des
    /// tâches ; la relire ici évite d'avoir deux images à garder identiques, et le
    /// jour où l'une change l'autre suit.
    fn icone_de_l_executable() -> Option<tray_icon::Icon> {
        // 1 : l'identifiant que « winresource » donne à l'icône principale.
        tray_icon::Icon::from_resource(1, None).ok()
    }
}

#[cfg(not(windows))]
mod plateforme {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TrayCommand {
        Open,
        Quit,
    }

    #[derive(Debug)]
    pub struct Tray;

    impl Tray {
        pub fn install() -> Option<Self> {
            None
        }
        pub fn set_unread(&mut self, _unread: u32) {}
        pub fn poll(&self) -> Option<TrayCommand> {
            None
        }
    }
}

pub use plateforme::{Tray, TrayCommand};
