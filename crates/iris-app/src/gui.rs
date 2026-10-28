//! The window: everything from the first image to the last, on every system.
//!
//! The Windows binary (`main.rs`) and Android's `iris-android` start Iris through
//! [`run`]. What only a desktop has — the tray, rounded corners, the mouse's back and
//! forward buttons, a second launch handing over to the first — is under
//! `cfg(not(target_os = "android"))`; everything else is the same code.

use crate::controller::{Controller, Request};
use crate::services::{now, Services};
use crate::shell;
use iris_types::{Result, Timestamp};
use slint::ComponentHandle as _;
use std::sync::Arc;

/// How the window is started.
pub struct GuiOptions {
    /// A `mailto:` the launch came with: a draft is opened with it.
    pub mailto: Option<crate::platform::MailtoRequest>,
    /// Started by the session: the window waits in the notification area.
    pub start_in_tray: bool,
    /// This copy is the first one running (desktop): a later launch hands over to it.
    pub instance: Option<crate::single::Primary>,
}

/// Allume ou éteint le témoin de synchronisation.
///
/// Passe par la boucle d'interface : la fenêtre ne se touche que depuis son propre
/// fil, et ce chemin est le seul endroit où les deux mondes se rencontrent.
fn signaler_synchronisation(fenetre: &slint::Weak<iris_ui::AppWindow>, actif: bool) {
    let _ = fenetre.upgrade_in_event_loop(move |fenetre| {
        fenetre.set_syncing(actif);
    });
}

/// Builds the sending service. Each message leaves through the mailbox it is from
/// (`sending::AccountMailer`), never through whichever mailbox came first.
fn build_send_service(
    services: &Services,
    runtime: tokio::runtime::Handle,
) -> Result<(
    Arc<iris_sync::SendService>,
    tokio::sync::mpsc::UnboundedReceiver<iris_smtp::OutboxEvent>,
)> {
    let (outbox, evenements) =
        iris_smtp::Outbox::new(services.mailer(), iris_smtp::DEFAULT_DELAY, runtime);

    Ok((
        Arc::new(iris_sync::SendService::new(
            Arc::clone(&services.engine),
            Arc::new(outbox),
            services.bus.clone(),
        )),
        evenements,
    ))
}

/// Relie chaque envoi au fil dont il est issu, et dit à la fenêtre comment il a fini.
struct SendTracker {
    entries: std::sync::Mutex<
        std::collections::BTreeMap<
            iris_smtp::SendHandle,
            (iris_types::ThreadId, iris_types::AccountId),
        >,
    >,
    /// Where the outcome is told. An outcome only written to the log left the window
    /// saying "Message sent." over a message that never left.
    fenetre: slint::Weak<iris_ui::AppWindow>,
}

impl std::fmt::Debug for SendTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendTracker").finish_non_exhaustive()
    }
}

impl SendTracker {
    fn new(fenetre: slint::Weak<iris_ui::AppWindow>) -> Self {
        Self {
            entries: Default::default(),
            fenetre,
        }
    }

    fn tell(&self, handle: iris_smtp::SendHandle, issue: shell::IssueEnvoi) {
        let _ = self.fenetre.upgrade_in_event_loop(move |fenetre| {
            shell::envoi_termine(&fenetre, handle, issue);
        });
    }
}

impl iris_sync::SendContext for SendTracker {
    fn resolve(
        &self,
        handle: iris_smtp::SendHandle,
    ) -> Option<(iris_types::ThreadId, iris_types::AccountId)> {
        self.entries.lock().ok()?.get(&handle).copied()
    }

    fn finished(&self, handle: iris_smtp::SendHandle, outcome: Result<iris_sync::SentOutcome>) {
        let mut copie_manquante = None;
        let issue = match outcome {
            Ok(bilan) => {
                tracing::info!(
                    send = handle.0,
                    archived = bilan.archived,
                    waiting = bilan.moved_to_waiting,
                    "message sent"
                );
                if !bilan.archived {
                    copie_manquante = bilan.note;
                }
                shell::IssueEnvoi::Parti
            }
            Err(e) => {
                tracing::warn!(send = handle.0, error = %e, "send failed");
                shell::IssueEnvoi::Echec(e.to_string(), e.is_transient())
            }
        };
        if let Ok(mut e) = self.entries.lock() {
            e.remove(&handle);
        }
        self.tell(handle, issue);
        // Gone, but its copy is not in Sent: said, where it was only logged.
        if let Some(note) = copie_manquante {
            let _ = self.fenetre.upgrade_in_event_loop(move |f| {
                f.set_status(
                    format!("Message sent, but its copy is not in your Sent folder: {note}").into(),
                );
            });
        }
    }

    fn cancelled(&self, handle: iris_smtp::SendHandle) {
        if let Ok(mut e) = self.entries.lock() {
            e.remove(&handle);
        }
    }
}

/// Runs Iris's window until it is quit. `prepare` chooses the renderer and opens the
/// services: the caller's, because a desktop and a phone do both their own way (the
/// software renderer through winit and a master password typed in a console; the
/// Android backend and a key from the keystore).
pub fn run(options: GuiOptions, prepare: impl FnOnce() -> Result<Services>) -> Result<()> {
    let GuiOptions {
        mailto,
        start_in_tray: demarre_reduit,
        instance,
    } = options;

    // The renderer is chosen before the first window, and the services opened (see
    // `prepare` in main.rs: why the software renderer, measured).
    let services = prepare()?;

    // L'exécuteur asynchrone tourne dans ses propres fils : la synchronisation ne
    // partage rien avec l'affichage.
    let runtime = crate::services::runtime()?;

    // Les réglages sont lus avant la fenêtre : l'apparence choisie doit être là dès
    // la première image, et non apparaître après un clignotement.
    let mut reglages = crate::settings::Settings::load(services.paths.settings());
    // VIP, Office hours and Filer were plugins until 7.0: what they were told comes
    // over once, and their folder goes.
    if reglages.migrate_plugins(&services.paths.data) {
        if let Err(e) = reglages.save(services.paths.settings()) {
            tracing::warn!(error = %e, "saving the settings");
        }
    }
    services
        .themes
        .apply(reglages.appearance, crate::platform::system_dark());

    let fenetre = shell::build(&services)?;
    // No mailbox yet: Iris opens on its welcome page.
    if services.store.accounts().is_ok_and(|v| v.is_empty()) {
        fenetre.set_first_run(true);
    }
    // Iris stopped last time: what and where, to report it.
    if let Some(panne) = crate::logging::take_last_panic(&services.paths) {
        fenetre.set_toast(format!("Iris stopped last time: {panne}").into());
    }
    let theme = services.themes.active();
    fenetre
        .global::<iris_ui::Layout>()
        .set_text_scale(reglages.text_scale());
    shell::appliquer_apparence(&fenetre, &theme, reglages.density);
    shell::appliquer_accent(&fenetre, reglages.accent, theme.dark);
    services.engine.set_automation(reglages.automation);
    services.engine.set_triage(reglages.triage.clone());
    services.workflow.set_settings(reglages.automation);
    // Les identifiants clients OAuth passent aux services : c'est le fournisseur
    // d'identifiants qui s'en sert, à chaque renouvellement de jeton.
    *services.oauth.write().expect("réglages OAuth empoisonnés") = reglages.oauth.clone();

    // Le moteur de rendu des corps est construit une fois : ouvrir un peripherique
    // graphique par message serait absurde.
    let renderer: Arc<dyn iris_htmlview::HtmlRenderer> = Arc::new(shell::build_renderer());

    // La sélection courante, partagée entre le puits d'instantanés et la zone de
    // réponse : répondre s'adresse au fil affiché.
    let selection: Arc<std::sync::Mutex<Option<iris_types::ThreadId>>> =
        Arc::new(std::sync::Mutex::new(None));

    // Le chargeur de corps a besoin du contrôleur, qui a besoin du puits
    // d'instantanés, qui a besoin du chargeur. Le cycle se casse par une cellule
    // remplie une seule fois, plutôt que par un verrou permanent.
    let chargeur: Arc<std::sync::OnceLock<Arc<shell::BodyLoader>>> =
        Arc::new(std::sync::OnceLock::new());

    let puits = {
        let chargeur = Arc::clone(&chargeur);
        let services_puits = services.clone();
        let renderer = Arc::clone(&renderer);
        let selection = Arc::clone(&selection);
        let faible = fenetre.as_weak();
        move |snapshot: crate::Snapshot| {
            *selection.lock().expect("sélection empoisonnée") = snapshot.selected;
            if let Some(chargeur) = chargeur.get() {
                chargeur.request_if_needed(&snapshot);
            }
            let services = services_puits.clone();
            let renderer = Arc::clone(&renderer);
            let _ = faible.upgrade_in_event_loop(move |fenetre| {
                shell::apply_snapshot(&fenetre, &services, renderer.as_ref(), &snapshot);
            });
        }
    };

    let (controller, _fil) = Controller::spawn_with_index(
        Arc::clone(&services.store),
        Some(Arc::clone(&services.index)),
        Arc::clone(&services.workflow),
        now(),
        puits,
    );
    let controller = Arc::new(controller);

    let _ = chargeur.set(Arc::new(shell::BodyLoader::new(
        Arc::clone(&services.engine),
        Arc::clone(&controller),
        runtime.handle().clone(),
    )));

    shell::wire_window_controls(&fenetre);
    shell::wire_body_tiles(&fenetre);
    shell::wire_sync(
        &fenetre,
        &services,
        Arc::clone(&controller),
        runtime.handle().clone(),
    );
    shell::wire_callbacks(
        &fenetre,
        Arc::clone(&controller),
        iris_ui::Keymap::standard(),
    );
    shell::wire_settings(
        &fenetre,
        &services,
        Arc::clone(&controller),
        Arc::clone(&services.engine),
        runtime.handle().clone(),
        reglages.clone(),
        services.paths.settings(),
    );
    shell::wire_updates(&fenetre, Arc::clone(&controller), runtime.handle().clone());
    crate::calendar::wire_calendar(&fenetre, &services, runtime.handle().clone());
    crate::notes::wire_notes(&fenetre, &services);
    crate::backup::wire_backup(&fenetre, &services, runtime.handle().clone());
    crate::caldav::wire_caldav(&fenetre, &services, runtime.handle().clone());
    crate::tasks::wire_tasks(&fenetre, &services, Arc::clone(&controller));
    crate::habits::wire_habits(&fenetre, &services);
    crate::devices::wire_devices(&fenetre, &services, runtime.handle().clone());

    // L'envoi : composition, délai d'annulation, dépôt dans les messages envoyés,
    // passage du fil en attente. Le suivi tourne en tâche de fond, pour que ce qui
    // doit arriver après un envoi arrive même si la fenêtre se ferme entre-temps.
    let mut envoi_a_finir: Option<Arc<iris_sync::SendService>> = None;
    match build_send_service(&services, runtime.handle().clone()) {
        Ok((envoi, evenements)) => {
            // One notice to take a message back, whatever it was written in.
            let avis = shell::wire_send_notice(&fenetre, Arc::clone(&envoi));
            shell::wire_reply(&fenetre, Arc::clone(&envoi), Arc::clone(&selection));
            // Writing a new message uses the same outbox as a reply, so the delay to
            // change your mind behaves identically. Every enabled mailbox is offered
            // as a sender, and its aliases after it (the composer reads them itself).
            shell::wire_compose(
                &fenetre,
                &services,
                Arc::clone(&envoi),
                avis,
                runtime.handle().clone(),
            );
            // Answering an invitation sends mail too.
            shell::wire_invite_answers(
                &fenetre,
                &services,
                Arc::clone(&envoi),
                Arc::clone(&controller),
            );
            // So may leaving a mailing list.
            shell::wire_unsubscribe(
                &fenetre,
                &services,
                Arc::clone(&envoi),
                Arc::clone(&controller),
                runtime.handle().clone(),
            );
            let contexte: Arc<dyn iris_sync::SendContext> =
                Arc::new(SendTracker::new(fenetre.as_weak()));
            envoi_a_finir = Some(Arc::clone(&envoi));
            runtime.spawn(iris_sync::pump_outbox(envoi, evenements, contexte));
        }
        // Sans compte configuré, il n'y a rien à envoyer : l'application reste
        // parfaitement utilisable pour lire.
        Err(e) => tracing::info!(reason = %e, "sending unavailable"),
    }

    controller.send(Request::Bootstrap);

    // Le bus alimente le contrôleur, à travers la coalescence.
    {
        let bus = services.bus.clone();
        let controller_bus = Arc::clone(&controller);
        runtime.spawn(async move {
            crate::controller::pump(&bus, controller_bus).await;
        });
    }

    shell::wire_attachments(&fenetre, &services, Arc::clone(&selection));
    shell::wire_attachment_open(&fenetre, &services, Arc::clone(&selection));
    shell::wire_invitations(&fenetre, &services, Arc::clone(&controller));
    shell::wire_source(&fenetre, &services, Arc::clone(&selection));
    shell::wire_account_setup(
        &fenetre,
        &services,
        Arc::clone(&controller),
        runtime.handle().clone(),
    );
    shell::wire_account_recovery(&fenetre, &services, runtime.handle().clone());
    shell::wire_account_menu(
        &fenetre,
        &services,
        Arc::clone(&controller),
        runtime.handle().clone(),
    );
    shell::wire_remote_images(&fenetre, services.clone(), Arc::clone(&renderer));
    shell::wire_folders(&fenetre, &services, Arc::clone(&controller));
    shell::wire_bulk(&fenetre, Arc::clone(&controller));
    shell::wire_modules(&fenetre, &services);

    // News pushed by the server (IMAP IDLE) for the mailbox on screen: synced as it
    // comes rather than at the next poll. Watched again on another mailbox when the
    // one on screen changes; a server that cannot IDLE is left to polling.
    {
        let engine = Arc::clone(&services.engine);
        runtime.spawn(async move {
            loop {
                let Some(compte) = engine.watched_account().await else {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    continue;
                };
                let change = async {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                        if engine.watched_account().await != Some(compte) {
                            break;
                        }
                    }
                };
                tokio::select! {
                    fin = engine.watch_inbox(compte) => {
                        let pause = match fin {
                            // No IDLE there: polling does it, asked again later.
                            Ok(()) => 15 * 60,
                            Err(e) => {
                                tracing::debug!(error = %e, "waiting for news");
                                60
                            }
                        };
                        tokio::time::sleep(std::time::Duration::from_secs(pause)).await;
                    }
                    () = change => {}
                }
            }
        });
    }

    // La boucle de synchronisation.
    {
        let engine = Arc::clone(&services.engine);
        let services_sync = services.clone();
        let faible = fenetre.as_weak();
        runtime.spawn(async move {
            if let Err(e) = engine.load_accounts(now()).await {
                tracing::error!(error = %e, "loading accounts");
            }
            loop {
                // Relu à chaque tour plutôt que capturé une fois : couper les
                // notifications dans les réglages doit les couper maintenant, pas au
                // prochain démarrage.
                let notifications_actives =
                    crate::settings::Settings::load(services_sync.paths.settings()).notifications;

                // Le travail que fait le temps précède celui du réseau : un report
                // échu doit réapparaître même quand le serveur est injoignable.
                match engine.run_maintenance(now()) {
                    Ok(m) if m.changed() => {
                        tracing::info!(woken = m.woken, followed_up = m.followed_up, "due work")
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "due work"),
                }

                // « En cours » est allumé pendant le tour, éteint après : sans ce
                // signal, une synchronisation lente est indiscernable d'une
                // application qui ne fait rien, et l'utilisateur relance.
                signaler_synchronisation(&faible, true);
                let rapport = engine.tick(now()).await;
                signaler_synchronisation(&faible, false);

                if rapport.changed() {
                    tracing::info!(
                        added = rapport.messages_added,
                        flags = rapport.flags_updated,
                        "synchronisation"
                    );
                }
                crate::home::count_arrivals(rapport.inbox_arrivals as u64);

                // Actions the server kept refusing were given up: they looked done
                // here, so it is said, not only logged.
                if !rapport.actions_refused.is_empty() {
                    let ligne = iris_sync::refused_line(rapport.actions_refused.len());
                    let _ = faible.upgrade_in_event_loop(move |f| f.set_status(ligne.into()));
                }

                // La bulle d'arrivée. Une seule pour tout le tour, et seulement quand
                // du courrier est réellement arrivé : un drapeau modifié sur le
                // serveur n'est pas une arrivée, et prévenir pour un message qu'on
                // vient de marquer comme lu ailleurs serait un contresens.
                //
                // Le dernier message est relu ici plutôt que remonté par le moteur :
                // le moteur compte, il ne met pas en forme, et une seule lecture par
                // tour de synchronisation ne se mesure pas.
                // New mail in an inbox only, each message once: every new row was
                // counted, so Gmail's counted twice, sent and archived copies counted,
                // and each pass of a first sync announced thousands.
                if rapport.inbox_arrivals > 0 && notifications_actives {
                    if let Some(dernier) = services_sync.store.latest_unread(now()).ok().flatten() {
                        let arrivee = crate::notify::Arrival {
                            count: rapport.inbox_arrivals,
                            sender: dernier.0,
                            subject: dernier.1,
                        };
                        if !crate::notify::show(&arrivee) {
                            // Une bulle qui n'apparaît pas est presque toujours la
                            // même cause : la copie n'est pas installée, donc Windows
                            // ne connaît pas son identité. Le dire une fois, dans le
                            // journal, vaut mieux que de le répéter à chaque tour.
                            tracing::debug!("notification refused by the system");
                        }
                    }
                }
                // La barre latérale reflète l'état de l'ordonnanceur : un compte
                // qui ne se synchronise plus doit le dire là où on regarde les
                // comptes, et non seulement dans un journal.
                let suspendus = engine.suspended_accounts().await;
                {
                    let services = services_sync.clone();
                    let _ = faible.upgrade_in_event_loop(move |fenetre| {
                        shell::refresh_accounts(&fenetre, &services, &suspendus);
                        // Les dossiers aussi : un tour peut en découvrir ou en oublier,
                        // et la colonne n'était relue qu'au démarrage et après un geste
                        // de l'utilisateur. Un dossier retiré du serveur y restait
                        // jusqu'au prochain lancement.
                        shell::refresh_folders(&fenetre, &services);
                        crate::home::refresh_if_shown(&fenetre, &services);
                    });
                }

                // On dort exactement le temps utile plutôt que de se réveiller
                // chaque seconde pour ne rien faire.
                let attente = engine
                    .next_wakeup(now())
                    .await
                    .unwrap_or(std::time::Duration::from_secs(60))
                    .max(std::time::Duration::from_secs(1));
                tokio::time::sleep(attente).await;
            }
        });
    }

    // Réveil périodique de l'horloge : dates relatives et reports échus.
    //
    // The same timer carries the status bar's two live figures. They are cheap to
    // read and neither justifies a thread of its own.
    {
        let faible_vitals = fenetre.as_weak();
        let lecteur = Arc::new(std::sync::Mutex::new(crate::vitals::VitalsReader::new()));
        let store_vitals = Arc::clone(&services.store);

        runtime.spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                let derniere = store_vitals
                    .accounts()
                    .ok()
                    .and_then(|c| c.iter().map(|a| a.last_activity_at).max())
                    .filter(|t| t.millis() > 0);
                let lecteur = Arc::clone(&lecteur);
                let _ = faible_vitals.upgrade_in_event_loop(move |fenetre| {
                    if let Ok(mut l) = lecteur.lock() {
                        shell::refresh_vitals(&fenetre, &mut l, derniere);
                    }
                });
            }
        });
    }

    {
        let controller_horloge = Arc::clone(&controller);
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                controller_horloge.send(Request::Tick(Timestamp::from_millis(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0),
                )));
            }
        });
    }

    // L'icône de la zone de notification.
    //
    // Créée ici, sur le fil principal : sous Windows elle vit dans une fenêtre cachée
    // dont les messages sont distribués par la boucle d'événements, et celle-ci
    // appartient à Slint. Elle est **interrogée** par une minuterie plutôt que de
    // rappeler l'application : un rappel arrivé d'un autre fil n'aurait pas le droit
    // de toucher à la fenêtre, et deux relevés non bloquants toutes les deux cents
    // millisecondes ne se mesurent pas.
    let _minuterie_zone = {
        let mut zone = crate::tray::Tray::install();
        if zone.is_none() {
            tracing::info!("no notification area: the tray icon is unavailable");
        }
        // Ce que « fermer » va vouloir dire dépend de cette réponse.
        fenetre.set_tray_available(zone.is_some());

        let faible = fenetre.as_weak();
        let store_zone = Arc::clone(&services.store);
        let controller_zone = Arc::clone(&controller);
        let minuterie = slint::Timer::default();
        let mut precedent = u32::MAX;
        // L'état de l'icône tel qu'on l'a laissé. `None` : jamais réglé, donc à régler.
        let mut visible: Option<bool> = None;

        minuterie.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(200),
            move || {
                let Some(zone) = zone.as_mut() else {
                    return;
                };

                // L'icône suit le réglage « continuer en arrière-plan ».
                //
                // Relue ici plutôt que poussée depuis l'écran des réglages : la zone de
                // notification ne vit que dans cette fermeture, et lui faire traverser
                // la moitié de l'application pour un interrupteur reviendrait à ouvrir
                // un chemin de plus vers un objet qui n'a pas le droit de quitter ce
                // fil. Une comparaison par tour ne se mesure pas ; l'appel qui suit
                // n'a lieu qu'au changement.
                if let Some(fenetre) = faible.upgrade() {
                    let voulue = fenetre.get_keep_running();
                    if visible != Some(voulue) {
                        zone.set_visible(voulue);
                        visible = Some(voulue);
                    }
                }

                match zone.poll() {
                    Some(crate::tray::TrayCommand::Open) => {
                        if let Some(fenetre) = faible.upgrade() {
                            montre_la_fenetre(&fenetre);
                        }
                    }
                    Some(crate::tray::TrayCommand::Quit) => {
                        let c = Arc::clone(&controller_zone);
                        shell::quit_after_sends(faible.upgrade().as_ref(), move || c.shutdown());
                    }
                    None => {}
                }

                // Le décompte est relu à chaque tour, mais l'infobulle n'est réécrite
                // que s'il a changé : la réécrire à l'identique fait clignoter la
                // fenêtre contextuelle de Windows quand la souris est dessus.
                let unread = store_zone.unread_count().unwrap_or(0);
                if unread != precedent {
                    precedent = unread;
                    zone.set_unread(unread);
                }
            },
        );

        minuterie
    };

    // Home and the history of places. Home opens first unless it was switched off;
    // the history starts where the window does.
    crate::home::wire_home(&fenetre, &services, Arc::clone(&controller));
    crate::partout::wire(&fenetre, &services);
    let depart = if reglages.home_at_startup { 3 } else { 0 };
    crate::nav::wire_navigation(
        &fenetre,
        crate::nav::Place::start(depart, reglages.calendar_view.clamp(0, 4)),
    );
    fenetre.set_workspace(depart);
    fenetre.invoke_workspace_changed(depart);
    ecoute_les_boutons_de_la_souris(&fenetre);

    // Une adresse cliquée dans un navigateur ouvre un brouillon, déjà rempli.
    //
    // Fait après tout le câblage : `wire_compose` installe les rappels de la fenêtre
    // de composition, et remplir les champs avant qu'ils existent les remplirait pour
    // rien.
    if let Some(demande) = mailto {
        shell::open_mailto_draft(&fenetre, &demande);
    }

    // A later launch of Iris shows this window, with the address it came with.
    if let Some(instance) = &instance {
        let faible = fenetre.as_weak();
        let cache = services.paths.cache.clone();
        instance.listen(move || {
            let cache = cache.clone();
            let _ = faible.upgrade_in_event_loop(move |fenetre| {
                montre_la_fenetre(&fenetre);
                if let Some(demande) = crate::single::take_pending_mailto(&cache)
                    .and_then(|a| crate::platform::MailtoRequest::parse(&a))
                {
                    shell::open_mailto_draft(&fenetre, &demande);
                }
            });
        });
    }

    // `run_event_loop_until_quit` et non `run` : la boucle doit survivre à la
    // fermeture de la fenêtre, sinon ranger l'application dans la zone de notification
    // la ferait disparaître au lieu de la ranger. On sort par « Quitter », et par là
    // seulement.
    if !demarre_reduit {
        // Opened full size: a mail client is where one spends the day, not a small
        // window to enlarge by hand at every start.
        fenetre.window().set_maximized(true);
        fenetre
            .show()
            .map_err(|e| iris_types::Error::other(format!("affichage : {e}")))?;
        fenetre.window().set_maximized(true);
        arrondir_les_coins(&fenetre);
    }

    slint::run_event_loop_until_quit()
        .map_err(|e| iris_types::Error::other(format!("boucle d'interface : {e}")))?;

    // A message sent before quitting still leaves. The outbox lives in memory, and
    // dropping the runtime dropped whatever waited out its undo delay or was on its
    // way, although its window had already been emptied.
    if let Some(envoi) = envoi_a_finir {
        if envoi.in_flight() > 0 {
            tracing::info!(count = envoi.in_flight(), "finishing sends before quitting");
            let limite = std::time::Duration::from_secs(
                u64::from(*crate::settings::UNDO_SEND_RANGE.end()) + 90,
            );
            if !runtime.block_on(envoi.wait_idle(limite)) {
                tracing::warn!(count = envoi.in_flight(), "quit with messages still unsent");
            }
        }
    }

    controller.shutdown();
    Ok(())
}

/// Rounded corners when the window is not maximised, as the mockup draws it.
///
/// We draw the frame ourselves, so the system's rounding has to be asked for: Windows
/// 11 rounds a frameless window, and gives it its shadow and hairline, once its corner
/// preference says so. It keeps maximised windows square on its own. Earlier versions
/// of Windows ignore the attribute.
///
/// Asked once the system window exists, which `show` does not mean: before the event
/// loop runs, Slint only notes the window is to be shown and creates it later. Asked
/// then, the preference reached no window, and a window opened maximised came back
/// square when restored.
#[allow(unsafe_code)]
fn arrondir_les_coins(fenetre: &iris_ui::AppWindow) {
    #[cfg(windows)]
    {
        use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use slint::winit_030::WinitWindowAccessor;
        use windows_sys::Win32::Graphics::Dwm::{
            DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        };
        let faible = fenetre.as_weak();
        let tache = slint::spawn_local(async move {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Ok(w) = fenetre.window().winit_window().await else {
                return;
            };
            let Ok(poignee) = w.window_handle() else {
                return;
            };
            if let RawWindowHandle::Win32(h) = poignee.as_raw() {
                let preference = DWMWCP_ROUND;
                // SAFETY: a live window handle, and a pointer to a value of the size given.
                unsafe {
                    DwmSetWindowAttribute(
                        h.hwnd.get() as _,
                        DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                        &preference as *const _ as *const core::ffi::c_void,
                        std::mem::size_of_val(&preference) as u32,
                    );
                }
            }
        });
        if let Err(e) = tache {
            tracing::warn!(error = %e, "rounding the window's corners");
        }
    }
    #[cfg(not(windows))]
    let _ = fenetre;
}

/// The mouse's back and forward buttons walk the history of places.
///
/// Read from the window's own events rather than from the interface: every row and
/// button under the pointer would otherwise take the press before a handler beneath
/// them could see it.
///
/// A right-click on the words of a message shown as text opens the menu of their link
/// the same way: the words take the press themselves, and nothing beneath them sees it.
#[cfg(not(target_os = "android"))]
fn ecoute_les_boutons_de_la_souris(fenetre: &iris_ui::AppWindow) {
    use slint::winit_030::winit::event::{ElementState, MouseButton, WindowEvent};
    use slint::winit_030::{EventResult, WinitWindowAccessor};
    let faible = fenetre.as_weak();
    // Where the pointer is, in the window's logical pixels.
    let pointeur = std::cell::Cell::new((0.0f32, 0.0f32));
    fenetre
        .window()
        .on_winit_window_event(move |fenetre_slint, evenement| {
            if let WindowEvent::CursorMoved { position, .. } = evenement {
                let echelle = f64::from(fenetre_slint.scale_factor().max(0.1));
                pointeur.set(((position.x / echelle) as f32, (position.y / echelle) as f32));
                // The words under the pointer say so again at every move (conversation.slint):
                // words that went away with their message never say they were left.
                if let Some(f) = faible.upgrade() {
                    f.set_hovered_link("".into());
                }
            }
            if let WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } = evenement
            {
                let (x, y) = pointeur.get();
                // Looked at once the press has gone by: the words have had their turn to
                // say they are under the pointer, a menu the press opened elsewhere is up,
                // and the press does not land on this menu's own veil and close it.
                let faible = faible.clone();
                slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                    let Some(f) = faible.upgrade() else {
                        return;
                    };
                    let lien = f.get_hovered_link();
                    if !lien.is_empty() && !f.get_modal_open() {
                        let mots = f.get_hovered_link_text();
                        shell::open_link_menu(&f, &lien, &mots, false, x, y);
                    }
                });
            }
            if let WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } = evenement
            {
                let sens = match button {
                    MouseButton::Back => Some(false),
                    MouseButton::Forward => Some(true),
                    _ => None,
                };
                if let (Some(avant), Some(f)) = (sens, faible.upgrade()) {
                    if avant {
                        crate::nav::forward(&f);
                    } else {
                        crate::nav::back(&f);
                    }
                    return EventResult::PreventDefault;
                }
            }
            EventResult::Propagate
        });
}

/// A phone has no mouse buttons to listen to: its Back reaches `nav::system_back`.
#[cfg(target_os = "android")]
fn ecoute_les_boutons_de_la_souris(_fenetre: &iris_ui::AppWindow) {}

/// Brings the window back from the notification area, or from the taskbar, in front.
fn montre_la_fenetre(fenetre: &iris_ui::AppWindow) {
    // `show` seul ne suffit pas sur une fenêtre réduite : elle reste dans la barre des
    // tâches. La restaurer d'abord est ce qui la ramène sous les yeux.
    fenetre.window().set_minimized(false);
    let _ = fenetre.show();
    // Started in the notification area, the window is created only now.
    arrondir_les_coins(fenetre);
    shell::came_back(fenetre);
    fenetre.window().set_fullscreen(false);
    crate::single::bring_to_front();
}
