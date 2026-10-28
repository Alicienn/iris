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

use iris_app::{accounts, paths, services};
use iris_secrets::Secret;
use iris_types::Result;
use paths::Paths;
use services::{now, Services};

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
    let chemins = Paths::system();
    match &chemins {
        Ok(chemins) => iris_app::logging::install(chemins),
        Err(_) => init_tracing(),
    }

    // Iris already waiting in the notification area: launching it again shows that
    // window (and composes the address, if one came with this launch) instead of a
    // second copy that would find the database taken.
    let instance = match (commande, &chemins) {
        ("run", Ok(chemins)) => {
            let adresse = mailto.as_ref().and(args.first().map(String::as_str));
            match iris_app::single::claim(&chemins.cache, adresse, !demarre_reduit) {
                iris_app::single::Claim::First(garde) => Some(garde),
                iris_app::single::Claim::AlreadyRunning => {
                    tracing::info!("Iris is already running: its window was asked for");
                    return Ok(());
                }
            }
        }
        _ => None,
    };

    match commande {
        "run" => iris_app::gui::run(
            iris_app::gui::GuiOptions {
                mailto,
                start_in_tray: demarre_reduit,
                instance,
            },
            || {
                select_software_renderer();
                open_services()
            },
        ),
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
        "Iris, a mail client\n\
         \n\
         Usage:\n\
         \x20 iris [run]                     Start the application\n\
         \x20 iris add-account <address>     Add an account (prompts for the password)\n\
         \x20 iris import <file>             Add accounts in bulk\n\
         \x20 iris accounts                  List configured accounts\n\
         \x20 iris sync                      Synchronise once, without the interface\n\
         \x20 iris doctor                    Check the installation\n\
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

    Ok(())
}

// --- Interface ---

/// Slint's software renderer, the only one built in, unless `SLINT_BACKEND` names a
/// backend (`winit-software` for the previews).
fn select_software_renderer() {
    if std::env::var_os("SLINT_BACKEND").is_none() {
        if let Err(e) = slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name("software".into())
            .select()
        {
            tracing::warn!(error = %e, "software renderer unavailable, using the default");
        }
    }
}
