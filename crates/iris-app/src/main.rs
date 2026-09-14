//! Iris — client de messagerie.

#![deny(unsafe_code)]

use iris_app::controller::{Controller, Request};
use iris_app::{accounts, paths, services, shell};
use iris_secrets::Secret;
use iris_types::{Result, Timestamp};
use paths::Paths;
use services::{now, Services};
use slint::ComponentHandle as _;
use std::sync::Arc;

fn main() {
    if let Err(e) = run() {
        eprintln!("iris : {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    init_tracing();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let commande = args.first().map(String::as_str).unwrap_or("run");

    match commande {
        "run" => run_gui(),
        "add-account" => cmd_add_account(&args[1..]),
        "import" => cmd_import(&args[1..]),
        "accounts" => cmd_list_accounts(),
        "sync" => cmd_sync(),
        "doctor" => cmd_doctor(),
        "--help" | "-h" | "help" => {
            print_help();
            Ok(())
        }
        autre => {
            eprintln!("commande inconnue : « {autre} »\n");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "Iris — client de messagerie\n\
         \n\
         Usage :\n\
         \x20 iris [run]                     Lance l'application\n\
         \x20 iris add-account <adresse>     Ajoute un compte (mot de passe demandé)\n\
         \x20 iris import <fichier>          Ajoute des comptes en lot\n\
         \x20 iris accounts                  Liste les comptes configurés\n\
         \x20 iris sync                      Synchronise une fois, sans interface\n\
         \x20 iris doctor                    Vérifie l'installation\n\
         \n\
         Format d'import : une ligne par compte, « adresse;mot de passe;groupe ».\n"
    );
}

fn init_tracing() {
    // Le niveau se règle par la variable d'environnement usuelle ; par défaut, on
    // n'affiche que ce qui mérite l'attention de l'utilisateur.
    let filtre = std::env::var("IRIS_LOG").unwrap_or_else(|_| "warn,iris=info".into());
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filtre))
        .with_target(false)
        .init();
}

/// Ouvre les services, en demandant le mot de passe maître si nécessaire.
fn open_services() -> Result<Services> {
    let chemins = Paths::system()?;

    // On tente d'abord sans mot de passe maître : avec un trousseau système, il n'en
    // faut aucun, et le demander pour rien serait absurde.
    match Services::open(chemins.clone(), None) {
        Ok(s) => Ok(s),
        Err(_) => {
            let master = prompt_secret("Mot de passe maître du coffre : ")?;
            Services::open(chemins, Some(master))
        }
    }
}

fn prompt_secret(invite: &str) -> Result<Secret> {
    use std::io::Write as _;
    print!("{invite}");
    std::io::stdout().flush()?;
    let motdepasse = rpassword::read_password()
        .map_err(|e| iris_types::Error::Config(format!("lecture du mot de passe : {e}")))?;
    Ok(Secret::new(motdepasse))
}

// --- Commandes ---

fn cmd_add_account(args: &[String]) -> Result<()> {
    let Some(adresse) = args.first() else {
        return Err(iris_types::Error::Config("usage : iris add-account <adresse>".into()));
    };

    let services = open_services()?;
    let motdepasse = prompt_secret(&format!("Mot de passe pour {adresse} : "))?;

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| iris_types::Error::other(format!("exécuteur : {e}")))?;

    let ajoute = runtime.block_on(accounts::add_account(
        &services.store,
        services.secrets.as_ref(),
        adresse,
        motdepasse.expose(),
        None,
        now(),
    ))?;

    println!("Compte ajouté : {}", ajoute.email);
    println!(
        "  IMAP  {}:{}\n  SMTP  {}:{}\n  Source : {}",
        ajoute.config.imap_host,
        ajoute.config.imap_port,
        ajoute.config.smtp_host,
        ajoute.config.smtp_port,
        ajoute.source.describe()
    );
    if ajoute.needs_review {
        println!(
            "\n  ⚠ Cette configuration est déduite, pas certifiée. Vérifiez-la si la \n\
             \x20   synchronisation échoue."
        );
    }
    if let Some(note) = &ajoute.config.note {
        println!("\n  {note}");
    }
    Ok(())
}

fn cmd_import(args: &[String]) -> Result<()> {
    let Some(fichier) = args.first() else {
        return Err(iris_types::Error::Config("usage : iris import <fichier>".into()));
    };

    let contenu = std::fs::read_to_string(fichier)?;
    let (entrees, erreurs) = accounts::parse_bulk(&contenu);

    for e in &erreurs {
        eprintln!("  ignoré : {e}");
    }
    if entrees.is_empty() {
        return Err(iris_types::Error::Config("aucun compte exploitable dans le fichier".into()));
    }

    println!("{} compte(s) à ajouter…", entrees.len());
    let services = open_services()?;
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| iris_types::Error::other(format!("exécuteur : {e}")))?;

    let rapport = runtime.block_on(accounts::add_bulk(
        Arc::clone(&services.store),
        Arc::clone(&services.secrets),
        &entrees,
        now(),
    ));

    for compte in &rapport.added {
        println!("  ✓ {}", compte.email);
    }
    for (adresse, raison) in &rapport.failed {
        println!("  ✗ {adresse} : {raison}");
    }
    println!("\n{}", rapport.summary());
    Ok(())
}

fn cmd_list_accounts() -> Result<()> {
    let services = open_services()?;
    let comptes = services.store.accounts()?;

    if comptes.is_empty() {
        println!("Aucun compte configuré. « iris add-account <adresse> » pour commencer.");
        return Ok(());
    }

    for c in comptes {
        let marque = if c.pinned { "★" } else { " " };
        let etat = if c.enabled { "" } else { "  (désactivé)" };
        println!("{marque} {:<38} {}:{}{etat}", c.email, c.imap_host, c.imap_port);
    }
    Ok(())
}

fn cmd_sync() -> Result<()> {
    let services = open_services()?;
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| iris_types::Error::other(format!("exécuteur : {e}")))?;

    runtime.block_on(async {
        let inscrits = services.engine.load_accounts(now()).await?;
        println!("{inscrits} compte(s) à synchroniser…");

        let rapport = services.engine.tick(now()).await;
        println!(
            "{} compte(s) synchronisé(s), {} message(s) ajouté(s), {} drapeau(x) mis à jour.",
            rapport.accounts_synced, rapport.messages_added, rapport.flags_updated
        );
        for (compte, erreur) in &rapport.failures {
            println!("  ✗ compte {compte} : {erreur}");
        }
        Ok::<(), iris_types::Error>(())
    })?;

    Ok(())
}

fn cmd_doctor() -> Result<()> {
    let chemins = Paths::system()?;
    println!("Emplacements");
    println!("  données        {}", chemins.data.display());
    println!("  cache          {}", chemins.cache.display());
    println!("  configuration  {}", chemins.config.display());

    let services = open_services()?;
    println!("\nServices");
    println!("  base           schéma {}", services.store.schema_version()?);
    println!("  secrets        {}", services.secrets_backend());
    println!("  index          {} document(s)", services.index.document_count());
    println!("  contenus       {} objet(s)", services.blobs.stats()?.count);
    println!("  thèmes         {}", services.themes.names().join(", "));

    let comptes = services.store.accounts()?;
    println!("\nComptes         {}", comptes.len());
    println!("Messages        {}", services.store.message_count()?);
    println!("En attente      {} opération(s)", services.store.pending_op_count()?);

    // Un thème invalide ne bloque pas le démarrage, mais l'utilisateur doit pouvoir
    // savoir pourquoi son thème n'a pas l'air de fonctionner.
    let avertissements = services.themes.active().lint();
    if !avertissements.is_empty() {
        println!("\nThème actif : {} avertissement(s)", avertissements.len());
        for a in avertissements {
            println!("  • {a}");
        }
    }
    Ok(())
}

/// Construit le service d'envoi à partir du premier compte actif.
///
/// Un seul expéditeur pour l'instant : choisir l'identité d'envoi demande une
/// décision d'interface qui n'est pas encore prise, et ouvrir une connexion SMTP par
/// compte coûterait cher pour rien.
fn build_send_service(
    services: &Services,
) -> Result<(
    Arc<iris_sync::SendService>,
    tokio::sync::mpsc::UnboundedReceiver<iris_smtp::OutboxEvent>,
)> {
    let compte = services
        .store
        .accounts()?
        .into_iter()
        .find(|c| c.enabled)
        .ok_or_else(|| iris_types::Error::Config("aucun compte configuré".into()))?;

    let motdepasse = services
        .secrets
        .get(&compte.email, iris_secrets::SecretKind::Password)?
        .ok_or_else(|| iris_types::Error::AuthFailed { account: compte.email.clone() })?;

    let expediteur = iris_sync::send::mailer_for(&compte, motdepasse.expose())?;
    let (outbox, evenements) = iris_smtp::Outbox::new(expediteur, iris_smtp::DEFAULT_DELAY);

    Ok((
        Arc::new(iris_sync::SendService::new(
            Arc::clone(&services.engine),
            Arc::new(outbox),
            services.bus.clone(),
        )),
        evenements,
    ))
}

/// Relie chaque envoi au fil dont il est issu.
#[derive(Debug, Default)]
struct SendTracker {
    entries: std::sync::Mutex<
        std::collections::BTreeMap<
            iris_smtp::SendHandle,
            (iris_types::ThreadId, iris_types::AccountId),
        >,
    >,
}

impl iris_sync::SendContext for SendTracker {
    fn resolve(
        &self,
        handle: iris_smtp::SendHandle,
    ) -> Option<(iris_types::ThreadId, iris_types::AccountId)> {
        self.entries.lock().ok()?.get(&handle).copied()
    }

    fn finished(&self, handle: iris_smtp::SendHandle, outcome: Result<iris_sync::SentOutcome>) {
        match outcome {
            Ok(bilan) => tracing::info!(
                envoi = handle.0,
                archive = bilan.archived,
                en_attente = bilan.moved_to_waiting,
                "message envoyé"
            ),
            Err(e) => tracing::warn!(envoi = handle.0, erreur = %e, "envoi en échec"),
        }
        if let Ok(mut e) = self.entries.lock() {
            e.remove(&handle);
        }
    }

    fn cancelled(&self, handle: iris_smtp::SendHandle) {
        if let Ok(mut e) = self.entries.lock() {
            e.remove(&handle);
        }
    }
}

// --- Interface ---

fn run_gui() -> Result<()> {
    let services = open_services()?;

    // L'exécuteur asynchrone tourne dans ses propres fils : la synchronisation ne
    // partage rien avec l'affichage.
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| iris_types::Error::other(format!("exécuteur : {e}")))?;

    let fenetre = shell::build(&services)?;

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
        move |snapshot: iris_app::Snapshot| {
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
        iris_types::AutomationSettings::default(),
        now(),
        puits,
    );
    let controller = Arc::new(controller);

    let _ = chargeur.set(Arc::new(shell::BodyLoader::new(
        Arc::clone(&services.engine),
        Arc::clone(&controller),
        runtime.handle().clone(),
    )));

    shell::wire_callbacks(&fenetre, Arc::clone(&controller), iris_ui::Keymap::standard());

    // L'envoi : composition, délai d'annulation, dépôt dans les messages envoyés,
    // passage du fil en attente. Le suivi tourne en tâche de fond, pour que ce qui
    // doit arriver après un envoi arrive même si la fenêtre se ferme entre-temps.
    match build_send_service(&services) {
        Ok((envoi, evenements)) => {
            shell::wire_reply(&fenetre, Arc::clone(&envoi), Arc::clone(&selection));
            let contexte: Arc<dyn iris_sync::SendContext> = Arc::new(SendTracker::default());
            runtime.spawn(iris_sync::pump_outbox(envoi, evenements, contexte));
        }
        // Sans compte configuré, il n'y a rien à envoyer : l'application reste
        // parfaitement utilisable pour lire.
        Err(e) => tracing::info!(raison = %e, "envoi indisponible"),
    }

    controller.send(Request::Bootstrap);

    // Le bus alimente le contrôleur, à travers la coalescence.
    {
        let bus = services.bus.clone();
        let controller_bus = Arc::clone(&controller);
        runtime.spawn(async move {
            iris_app::controller::pump(&bus, controller_bus).await;
        });
    }

    // La boucle de synchronisation.
    {
        let engine = Arc::clone(&services.engine);
        runtime.spawn(async move {
            if let Err(e) = engine.load_accounts(now()).await {
                tracing::error!(erreur = %e, "chargement des comptes");
            }
            loop {
                // Le travail que fait le temps précède celui du réseau : un report
                // échu doit réapparaître même quand le serveur est injoignable.
                match engine.run_maintenance(now()) {
                    Ok(m) if m.changed() => {
                        tracing::info!(reveilles = m.woken, relances = m.followed_up, "échéances")
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(erreur = %e, "échéances"),
                }

                let rapport = engine.tick(now()).await;
                if rapport.changed() {
                    tracing::info!(
                        ajoutes = rapport.messages_added,
                        drapeaux = rapport.flags_updated,
                        "synchronisation"
                    );
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

    fenetre
        .run()
        .map_err(|e| iris_types::Error::other(format!("boucle d'interface : {e}")))?;

    controller.shutdown();
    Ok(())
}
