//! Iris — a mail client.
//!
//! One executable serves two purposes, which on Windows pull in opposite directions.
//! A graphical application must not open a console window when it is double-clicked;
//! a command-line tool must be able to print. Building for the "windows" subsystem
//! settles the first, and `attach_parent_console` settles the second by borrowing the
//! terminal the user ran the command from, when there is one.
//!
//! The subsystem is only switched in release builds. During development the console
//! is where the logs go, and losing it would trade a papercut nobody suffers for one
//! everybody does.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
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
        eprintln!("iris: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Windows nous lance avec l'URL en premier argument quand on clique une adresse
    // dans un navigateur. Ce n'est pas une sous-commande : c'est « ouvre-toi et
    // prépare ce message », et le confondre avec un nom de commande donnerait
    // « unknown command: mailto:marie@example.com ».
    let mailto = args
        .first()
        .and_then(|a| iris_app::platform::MailtoRequest::parse(a));

    let commande = match (&mailto, args.first().map(String::as_str)) {
        (Some(_), _) => "run",
        // Démarré par la session : la fenêtre attend dans la zone de notification.
        // Ouvrir par-dessus ce que l'utilisateur fait dans les premières secondes de
        // sa session serait exactement ce qu'on lui reprocherait.
        (None, Some("--tray")) => "run",
        (None, Some(c)) => c,
        (None, None) => "run",
    };
    let demarre_reduit = args.first().map(String::as_str) == Some("--tray");

    // Anything but the graphical mode is meant to be read. Attach to the calling
    // terminal before the first line is logged: Rust caches its standard output
    // handle on first use, so borrowing the console afterwards would be too late.
    if commande != "run" {
        attach_parent_console();
    }

    // Les sous-commandes qui n'ouvrent pas de fenêtre et n'ouvrent pas la base non
    // plus : elles écrivent dans le registre et rendent la main.
    match commande {
        "register" => return cmd_register(true),
        "unregister" => return cmd_register(false),
        _ => {}
    }

    // The log goes to a file from the very first line. A release build has no console
    // and aborts on panic: without this, a crash leaves nothing behind but an event
    // viewer entry saying `0xc0000409` at an offset in a stripped binary.
    match Paths::system() {
        Ok(chemins) => iris_app::logging::install(&chemins),
        Err(_) => init_tracing(),
    }

    match commande {
        "run" => run_gui(mailto, demarre_reduit),
        "add-account" => cmd_add_account(&args[1..]),
        "import" => cmd_import(&args[1..]),
        "accounts" => cmd_list_accounts(),
        "sync" => cmd_sync(),
        "doctor" => cmd_doctor(),
        "memory" => cmd_memory(),
        "--help" | "-h" | "help" => {
            print_help();
            Ok(())
        }
        autre => {
            eprintln!("unknown command: \"{autre}\"\n");
            print_help();
            std::process::exit(2);
        }
    }
}

/// Borrows the terminal that launched us, when there is one.
///
/// A "windows" subsystem process starts with no console attached. `AttachConsole`
/// with `ATTACH_PARENT_PROCESS` gives it the caller's, so `iris doctor` prints where
/// the user typed it. Started from Explorer there is no parent console and the call
/// fails, which is the right outcome: nothing to print to, nothing printed.
#[cfg(all(windows, not(debug_assertions)))]
#[allow(unsafe_code)]
fn attach_parent_console() {
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;

    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }

    // SAFETY: a call into kernel32 taking one integer and returning one. It has no
    // preconditions, cannot be passed an invalid pointer, and reports failure through
    // its return value, which we deliberately ignore — no console is not an error.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// Everywhere else the process already owns its console.
#[cfg(not(all(windows, not(debug_assertions))))]
fn attach_parent_console() {}

/// Inscrit ou retire Iris auprès de Windows.
///
/// Une sous-commande plutôt qu'une case à cocher **en plus** de la case à cocher :
/// c'est ce que l'installateur appelle, et il n'a pas d'interface. Les deux écrivent
/// exactement la même chose, au même endroit.
fn cmd_register(inscrire: bool) -> Result<()> {
    if inscrire {
        iris_app::platform::register_mailto()?;
        println!("Iris is now offered as a mail client. Windows still decides the default.");
    } else {
        iris_app::platform::unregister_mailto()?;
        println!("Iris is no longer registered.");
    }
    Ok(())
}

fn print_help() {
    println!(
        "Iris — a mail client\n\
         \n\
         Usage:\n\
         \x20 iris [run]                     Start the application\n\
         \x20 iris add-account <address>     Add an account (prompts for the password)\n\
         \x20 iris import <file>             Add accounts in bulk\n\
         \x20 iris accounts                  List configured accounts\n\
         \x20 iris sync                      Synchronise once, without the interface\n\
         \x20 iris doctor                    Check the installation\n\
         \x20 iris memory                    Break down what the data layer costs\n\
         \x20 iris register                  Offer Iris to Windows as a mail client\n\
         \x20 iris unregister                Withdraw that offer\n\
         \x20 iris --tray                    Start into the notification area\n\
         \x20 iris mailto:<address>          Start and compose to that address\n\
         \n\
         Import format: one line per account, \"address;password;group\".\n"
    );
}

/// Le repli, quand on ne sait même pas où écrire.
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

        // Une seule erreur appelle un mot de passe : celle qui dit qu'il en faut un.
        //
        // Le repli était écrit `Err(_)`, et toute panne d'ouverture devenait une
        // demande de mot de passe maître : une base verrouillée par une autre instance,
        // un index illisible, un disque plein. Lancée depuis un raccourci, sans entrée
        // standard, la demande échouait à son tour — et l'application mourait sur
        // « Descripteur non valide (os error 6) », qui ne décrit aucune des trois.
        Err(e) if besoin_d_un_mot_de_passe(&e) => {
            let master = prompt_secret("Mot de passe maître du coffre : ")?;
            Services::open(chemins, Some(master))
        }

        Err(e) => Err(e),
    }
}

/// L'ouverture a-t-elle échoué faute de mot de passe maître, ou pour autre chose ?
///
/// Reconnu au message, faute d'un type d'erreur qui le distingue. C'est fragile et
/// c'est assumé : le seul autre choix serait une variante d'erreur traversant quatre
/// caisses pour un cas qui n'arrive qu'ici, et se tromper coûte une demande de mot de
/// passe superflue plutôt qu'un démarrage impossible.
fn besoin_d_un_mot_de_passe(e: &iris_types::Error) -> bool {
    matches!(e, iris_types::Error::Config(m) if m.contains("mot de passe maître"))
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
        return Err(iris_types::Error::Config(
            "usage : iris add-account <adresse>".into(),
        ));
    };

    let services = open_services()?;
    let motdepasse = prompt_secret(&format!("Mot de passe pour {adresse} : "))?;

    let runtime = iris_app::services::runtime()?;

    let ajoute = runtime.block_on(accounts::add_account(
        &services.store,
        services.secrets.as_ref(),
        adresse,
        motdepasse.expose(),
        None,
        now(),
    ))?;

    println!("Account added: {}", ajoute.email);
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
        return Err(iris_types::Error::Config(
            "usage : iris import <fichier>".into(),
        ));
    };

    let contenu = std::fs::read_to_string(fichier)?;
    let (entrees, erreurs) = accounts::parse_bulk(&contenu);

    for e in &erreurs {
        eprintln!("  skipped: {e}");
    }
    if entrees.is_empty() {
        return Err(iris_types::Error::Config(
            "aucun compte exploitable dans le fichier".into(),
        ));
    }

    println!("{} account(s) to add…", entrees.len());
    let services = open_services()?;
    let runtime = iris_app::services::runtime()?;

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
        println!("  ✗ {adresse}: {raison}");
    }
    println!("\n{}", rapport.summary());
    Ok(())
}

fn cmd_list_accounts() -> Result<()> {
    let services = open_services()?;
    let comptes = services.store.accounts()?;

    if comptes.is_empty() {
        println!("No accounts configured. Run \"iris add-account <address>\" to begin.");
        return Ok(());
    }

    for c in comptes {
        let marque = if c.pinned { "★" } else { " " };
        let etat = if c.enabled { "" } else { "  (désactivé)" };
        println!(
            "{marque} {:<38} {}:{}{etat}",
            c.email, c.imap_host, c.imap_port
        );
    }
    Ok(())
}

fn cmd_sync() -> Result<()> {
    let services = open_services()?;
    let runtime = iris_app::services::runtime()?;

    runtime.block_on(async {
        let inscrits = services.engine.load_accounts(now()).await?;
        println!("{inscrits} account(s) to synchronise…");

        let rapport = services.engine.tick(now()).await;
        println!(
            "{} compte(s) synchronisé(s), {} message(s) ajouté(s), {} drapeau(x) mis à jour.",
            rapport.accounts_synced, rapport.messages_added, rapport.flags_updated
        );
        for (compte, erreur) in &rapport.failures {
            println!("  ✗ account {compte}: {erreur}");
        }
        Ok::<(), iris_types::Error>(())
    })?;

    Ok(())
}

/// La place restante sur le volume qui porte ce chemin.
///
/// `None` quand le système refuse de le dire : c'est un diagnostic, et un diagnostic
/// qui échoue ne doit pas emporter le rapport dont il fait partie.
#[cfg(windows)]
#[allow(unsafe_code)]
fn free_space(path: &std::path::Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;

    extern "system" {
        fn GetDiskFreeSpaceExW(
            directory: *const u16,
            free_to_caller: *mut u64,
            total: *mut u64,
            free: *mut u64,
        ) -> i32;
    }

    let large: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut disponible: u64 = 0;

    // SÛRETÉ : `large` vit jusqu'au retour, et les trois sorties sont des entiers que
    // nous possédons. L'appel ne conserve rien.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            large.as_ptr(),
            &mut disponible,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(disponible)
}

#[cfg(not(windows))]
fn free_space(_path: &std::path::Path) -> Option<u64> {
    None
}

/// Décompose la mémoire de la couche de données, sans interface.
///
/// La moitié de la question « pourquoi cinq cents mégaoctets » se règle ici : ce que
/// coûtent la base, l'index et le coffre *seuls*, sans fenêtre, sans police et sans
/// pilote graphique. Le reste de la facture est la différence entre ce chiffre et
/// celui de l'application ouverte — et cette différence, mesurée plutôt que supposée,
/// désigne le coupable.
fn cmd_memory() -> Result<()> {
    iris_app::memory::track_sites(true);
    iris_app::memory::mark("processus démarré");

    let services = open_services()?;
    iris_app::memory::mark("base, index, coffre");

    // Lire vraiment : un index ouvert et jamais interrogé n'a encore rien chargé, et
    // mesurer cela reviendrait à mesurer un fichier fermé.
    let comptes = services.store.accounts()?;
    let messages = services.store.message_count()?;
    iris_app::memory::mark("comptes et messages lus");

    let documents = services.index.document_count();
    iris_app::memory::mark("index interrogé");

    println!(
        "{} compte(s), {messages} message(s), {documents} document(s) indexés",
        comptes.len()
    );
    println!("{}", iris_app::memory::report());
    Ok(())
}

fn cmd_doctor() -> Result<()> {
    let chemins = Paths::system()?;
    println!("Locations");
    println!("  data           {}", chemins.data.display());
    println!("  cache          {}", chemins.cache.display());
    println!("  config         {}", chemins.config.display());

    let services = open_services()?;
    println!("\nServices");
    println!(
        "  database       schema {}",
        services.store.schema_version()?
    );
    println!("  secrets        {}", services.secrets_backend());
    println!(
        "  index          {} document(s)",
        services.index.document_count()
    );
    println!(
        "  blobs          {} object(s)",
        services.blobs.stats()?.count
    );
    println!("  themes         {}", services.themes.names().join(", "));

    let comptes = services.store.accounts()?;
    println!("\nAccounts        {}", comptes.len());
    println!("Messages        {}", services.store.message_count()?);
    println!(
        "Pending         {} operation(s)",
        services.store.pending_op_count()?
    );

    // La place disponible.
    //
    // Elle est ici parce qu'elle a manqué : un disque plein fait échouer une écriture
    // SQLite, et une écriture SQLite qui échoue au mauvais endroit fait tomber
    // l'application. C'est la panne la moins soupçonnée et la plus facile à vérifier,
    // et le seul écran qui existe pour vérifier des choses est celui-ci.
    match free_space(&chemins.data) {
        Some(octets) => {
            let gio = octets as f64 / (1024.0 * 1024.0 * 1024.0);
            println!("\nDisk            {gio:.1} GiB free");
            if gio < 1.0 {
                println!("  ⚠ Under a gibibyte. Writes will start failing.");
            }
        }
        None => println!("\nDisk            free space unknown"),
    }

    println!(
        "Log             {}",
        iris_app::logging::current(&chemins).display()
    );

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

/// Allume ou éteint le témoin de synchronisation.
///
/// Passe par la boucle d'interface : la fenêtre ne se touche que depuis son propre
/// fil, et ce chemin est le seul endroit où les deux mondes se rencontrent.
fn signaler_synchronisation(fenetre: &slint::Weak<iris_ui::AppWindow>, actif: bool) {
    let _ = fenetre.upgrade_in_event_loop(move |fenetre| {
        fenetre.set_syncing(actif);
    });
}

/// Construit le service d'envoi à partir du premier compte actif.
///
/// Un seul expéditeur : choisir l'identité d'envoi demande une décision d'interface
/// qui n'est pas prise, et ouvrir une connexion SMTP par compte coûterait cher pour
/// rien tant que personne ne peut choisir laquelle utiliser.
fn build_send_service(
    services: &Services,
    runtime: tokio::runtime::Handle,
) -> Result<(
    Arc<iris_sync::SendService>,
    tokio::sync::mpsc::UnboundedReceiver<iris_smtp::OutboxEvent>,
)> {
    let compte = services
        .store
        .accounts()?
        .into_iter()
        .find(|c| c.enabled)
        .ok_or_else(|| iris_types::Error::Config("no account configured".into()))?;

    let motdepasse = services
        .secrets
        .get(&compte.email, iris_secrets::SecretKind::Password)?
        .ok_or_else(|| iris_types::Error::AuthFailed {
            account: compte.email.clone(),
        })?;

    let expediteur = iris_sync::send::mailer_for(&compte, motdepasse.expose())?;
    let (outbox, evenements) =
        iris_smtp::Outbox::new(expediteur, iris_smtp::DEFAULT_DELAY, runtime);

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
                send = handle.0,
                archived = bilan.archived,
                waiting = bilan.moved_to_waiting,
                "message sent"
            ),
            Err(e) => tracing::warn!(send = handle.0, error = %e, "send failed"),
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

fn run_gui(mailto: Option<iris_app::platform::MailtoRequest>, demarre_reduit: bool) -> Result<()> {
    // La décomposition mémoire se règle avant tout le reste : ce qui est alloué avant
    // qu'elle soit allumée est compté, mais sans provenance.
    let rapport_memoire = iris_app::memory::requested_interval();
    iris_app::memory::track_sites(rapport_memoire.is_some());
    iris_app::memory::mark("processus démarré");

    let services = open_services()?;
    iris_app::memory::mark("base, index, coffre");

    // L'exécuteur asynchrone tourne dans ses propres fils : la synchronisation ne
    // partage rien avec l'affichage.
    let runtime = iris_app::services::runtime()?;
    iris_app::memory::mark("exécuteur asynchrone");

    // Les réglages sont lus avant la fenêtre : l'apparence choisie doit être là dès
    // la première image, et non apparaître après un clignotement.
    let reglages = iris_app::settings::Settings::load(services.paths.settings());
    if let Ok(theme) = services.themes.set_active(&reglages.theme) {
        let _ = theme;
    } else {
        tracing::warn!(theme = %reglages.theme, "theme not found, falling back to the default");
    }

    let fenetre = shell::build(&services)?;
    iris_app::memory::mark("fenêtre construite");
    shell::appliquer_apparence(&fenetre, &services.themes.active(), reglages.density);
    services.engine.set_automation(reglages.automation);
    services.workflow.set_settings(reglages.automation);
    // Les identifiants clients OAuth passent aux services : c'est le fournisseur
    // d'identifiants qui s'en sert, à chaque renouvellement de jeton.
    *services.oauth.write().expect("réglages OAuth empoisonnés") = reglages.oauth.clone();

    // Le moteur de rendu des corps est construit une fois : ouvrir un peripherique
    // graphique par message serait absurde.
    let renderer: Arc<dyn iris_htmlview::HtmlRenderer> = Arc::new(shell::build_renderer());
    // Ce jalon-ci est le plus instructif des six : construire le moteur ouvre un
    // second périphérique graphique, et ce que cela engage n'apparaît dans aucun
    // compteur de tas.
    iris_app::memory::mark("moteur HTML (périph. GPU)");

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
    let carnet = shell::wire_callbacks(
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

    // L'envoi : composition, délai d'annulation, dépôt dans les messages envoyés,
    // passage du fil en attente. Le suivi tourne en tâche de fond, pour que ce qui
    // doit arriver après un envoi arrive même si la fenêtre se ferme entre-temps.
    match build_send_service(&services, runtime.handle().clone()) {
        Ok((envoi, evenements)) => {
            shell::wire_reply(&fenetre, Arc::clone(&envoi), Arc::clone(&selection));
            // Writing a new message uses the same outbox as a reply, so the delay to
            // change your mind behaves identically. Every enabled mailbox is offered
            // as a sender: with a hundred of them, sending from the wrong one is the
            // mistake that costs.
            let identites: Vec<(iris_types::AccountId, String)> = services
                .store
                .accounts()
                .unwrap_or_default()
                .into_iter()
                .filter(|c| c.enabled)
                .map(|c| (c.id, c.email))
                .collect();
            shell::wire_compose(&fenetre, &services, Arc::clone(&envoi), identites);
            let contexte: Arc<dyn iris_sync::SendContext> = Arc::new(SendTracker::default());
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
            iris_app::controller::pump(&bus, controller_bus).await;
        });
    }

    shell::wire_attachments(&fenetre, &services, Arc::clone(&selection));
    shell::wire_attachment_open(&fenetre, &services, Arc::clone(&selection));
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
    shell::wire_plugin_browser(&fenetre, &services);

    // Les plugins. Leur fil est indépendant : un plugin qui part en boucle consomme
    // son carburant, pas une frame ni un tour de synchronisation.
    {
        let controller_plugins = Arc::clone(&controller);
        let carnet_effets = Arc::clone(&carnet);
        let faible = fenetre.as_weak();
        let (service, _fil) = iris_app::plugins::PluginService::spawn(
            iris_app::plugins::ensure_dir(services.paths.plugins()),
            Arc::clone(&services.store),
            move |effet| {
                // Une commande déclarée entre dans le carnet : c'est ce qui la rend
                // atteignable depuis la palette, sans redémarrage.
                if let iris_app::plugins::PluginEffect::Command { plugin, spec } = &effet {
                    if carnet_effets.add_plugin(plugin, spec).is_none() {
                        tracing::warn!(plugin = %plugin, "invalid plugin command");
                    }
                }

                let message = iris_app::plugins::apply_effect(&effet, &controller_plugins);
                if let Some(message) = message {
                    let _ = faible.upgrade_in_event_loop(move |fenetre| {
                        fenetre.set_status(message.into());
                    });
                }
            },
        );

        // What the modules screen shows: every plugin the registry accepted, with the
        // powers its manifest asked for.
        let vues = iris_app::modules::plugin_views(&service.manifests());
        shell::wire_modules(&fenetre, &services, vues);

        let rapport = service.report();
        if !rapport.loaded.is_empty() || !rapport.rejected.is_empty() {
            tracing::info!(bilan = %rapport.summary(), "plugins");
        }

        if _fil.is_some() {
            let service = Arc::new(service);

            // La palette rend les commandes de plugin à leur auteur.
            {
                let service = Arc::clone(&service);
                carnet.set_sink(move |plugin, spec| {
                    tracing::info!(plugin = %plugin, "plugin command invoked");
                    service.invoke(spec, None);
                });
            }

            let bus = services.bus.clone();
            let store = Arc::clone(&services.store);
            runtime.spawn(async move {
                iris_app::plugins::pump(&bus, store, service).await;
            });
        }
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
                    iris_app::settings::Settings::load(services_sync.paths.settings())
                        .notifications;

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

                // La bulle d'arrivée. Une seule pour tout le tour, et seulement quand
                // du courrier est réellement arrivé : un drapeau modifié sur le
                // serveur n'est pas une arrivée, et prévenir pour un message qu'on
                // vient de marquer comme lu ailleurs serait un contresens.
                //
                // Le dernier message est relu ici plutôt que remonté par le moteur :
                // le moteur compte, il ne met pas en forme, et une seule lecture par
                // tour de synchronisation ne se mesure pas.
                if rapport.messages_added > 0 && notifications_actives {
                    if let Some(dernier) = services_sync.store.latest_unread(now()).ok().flatten() {
                        let arrivee = iris_app::notify::Arrival {
                            count: rapport.messages_added,
                            sender: dernier.0,
                            subject: dernier.1,
                        };
                        if !iris_app::notify::show(&arrivee) {
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
        let lecteur = Arc::new(std::sync::Mutex::new(iris_app::vitals::VitalsReader::new()));
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
        let mut zone = iris_app::tray::Tray::install();
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
                    Some(iris_app::tray::TrayCommand::Open) => {
                        if let Some(fenetre) = faible.upgrade() {
                            // `show` seul ne suffit pas sur une fenêtre réduite : elle
                            // reste dans la barre des tâches. La restaurer d'abord est
                            // ce qui la ramène sous les yeux.
                            fenetre.window().set_minimized(false);
                            let _ = fenetre.show();
                            shell::came_back(&fenetre);
                            fenetre.window().set_fullscreen(false);
                        }
                    }
                    Some(iris_app::tray::TrayCommand::Quit) => {
                        controller_zone.shutdown();
                        let _ = slint::quit_event_loop();
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

    // Une adresse cliquée dans un navigateur ouvre un brouillon, déjà rempli.
    //
    // Fait après tout le câblage : `wire_compose` installe les rappels de la fenêtre
    // de composition, et remplir les champs avant qu'ils existent les remplirait pour
    // rien.
    if let Some(demande) = mailto {
        fenetre.set_compose_to(demande.to.as_str().into());
        fenetre.set_compose_cc(demande.cc.as_str().into());
        fenetre.set_compose_bcc(demande.bcc.as_str().into());
        fenetre.set_compose_subject(demande.subject.as_str().into());
        fenetre.set_compose_body(demande.body.as_str().into());
        // Les copies sont dépliées seulement si elles portent quelque chose : un
        // « mailto: » nu ne doit pas ouvrir deux champs vides de plus.
        fenetre.set_compose_show_cc(!demande.cc.is_empty() || !demande.bcc.is_empty());
        fenetre.set_compose_open(true);
    }

    iris_app::memory::mark("câblage terminé");

    // Le jalon qui manquait.
    //
    // Les cinq précédents sont pris avant la boucle d'événements, donc avant que la
    // fenêtre soit réellement présentée — et c'est la présentation qui ouvre le
    // périphérique graphique. Sans ce relevé-ci, la plus grosse dépense du processus
    // tombait dans l'intervalle entre le dernier jalon et le premier rapport, c'est-à-dire
    // nulle part. Deux secondes : le temps que les premières images soient passées.
    if rapport_memoire.is_some() {
        slint::Timer::single_shot(std::time::Duration::from_secs(2), || {
            iris_app::memory::mark("premières images affichées");
        });
    }

    // Le rapport périodique, sur son propre fil.
    //
    // Sur un fil à lui parce que la question posée est « que retient l'application
    // quand elle ne fait rien » : la poser depuis la boucle d'interface ne la ferait
    // jamais poser pendant que la boucle travaille, c'est-à-dire précisément aux
    // moments qui coûtent.
    if let Some(intervalle) = rapport_memoire {
        let fichier = services.paths.cache.join("memoire.txt");
        std::thread::Builder::new()
            .name("memoire".into())
            .spawn(move || loop {
                std::thread::sleep(intervalle);
                let rapport = iris_app::memory::report();
                let _ = std::fs::write(&fichier, &rapport);
                tracing::info!("décomposition mémoire écrite dans {}", fichier.display());
            })
            .map_err(|e| iris_types::Error::other(format!("fil de rapport : {e}")))?;
    }

    // `run_event_loop_until_quit` et non `run` : la boucle doit survivre à la
    // fermeture de la fenêtre, sinon ranger l'application dans la zone de notification
    // la ferait disparaître au lieu de la ranger. On sort par « Quitter », et par là
    // seulement.
    if !demarre_reduit {
        fenetre
            .show()
            .map_err(|e| iris_types::Error::other(format!("affichage : {e}")))?;
    }

    slint::run_event_loop_until_quit()
        .map_err(|e| iris_types::Error::other(format!("boucle d'interface : {e}")))?;

    controller.shutdown();
    Ok(())
}
