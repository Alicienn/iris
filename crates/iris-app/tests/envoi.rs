//! Sending, and taking it back.
//!
//! Send closes the window; a notice at the bottom counts down and offers Undo; Undo
//! stops the message and brings the window back as it was. Driven through a real
//! window (without a screen) and a real outbox whose mailer only records.

use iris_app::services::Services;
use iris_smtp::{FakeMailer, Mailer, Outbox, Outgoing};
use iris_types::Address;
use slint::ComponentHandle;
use std::sync::Arc;
use std::time::Duration;

fn message(sujet: &str) -> Outgoing {
    Outgoing::new(
        Address::new("moi@example.com"),
        vec![Address::new("marie@example.com")],
        sujet,
    )
}

#[test]
fn undo_stops_the_message_and_brings_the_window_back() {
    i_slint_backend_testing::init_no_event_loop();

    let dir = tempfile::tempdir().unwrap();
    let services = Services::open(
        iris_app::paths::Paths::under(dir.path()),
        Some(iris_secrets::Secret::new("test")),
    )
    .unwrap();
    let runtime = iris_app::services::runtime().unwrap();
    let mailer = Arc::new(FakeMailer::new());
    let (outbox, _evenements) = Outbox::new(
        Arc::clone(&mailer) as Arc<dyn Mailer>,
        Duration::from_secs(10),
        runtime.handle().clone(),
    );
    let envoi = Arc::new(iris_sync::SendService::new(
        Arc::clone(&services.engine),
        Arc::new(outbox),
        services.bus.clone(),
    ));

    let f = iris_ui::AppWindow::new().unwrap();
    let avis = iris_app::shell::wire_send_notice(&f, Arc::clone(&envoi));
    f.set_undo_send_seconds(5);

    // Sent: the notice counts from the setting.
    avis.envoyer(&f, message("Devis"), "Sending “Devis”".into(), |f| {
        f.set_compose_subject("Devis".into());
        f.set_compose_open(true);
    })
    .unwrap();
    assert!(f.get_send_notice_open(), "the notice shows");
    assert_eq!(f.get_send_notice_seconds(), 5);
    assert!(!f.get_compose_open());

    // Undo: the message stays, the window comes back as it was.
    f.invoke_send_undone();
    assert!(!f.get_send_notice_open(), "the notice goes");
    assert!(f.get_compose_open(), "the window is back");
    assert_eq!(f.get_compose_subject().as_str(), "Devis");

    runtime.block_on(async { tokio::time::sleep(Duration::from_millis(200)).await });
    assert!(mailer.sent().is_empty(), "nothing left");
}

#[test]
fn with_no_delay_nothing_is_offered_back() {
    i_slint_backend_testing::init_no_event_loop();

    let dir = tempfile::tempdir().unwrap();
    let services = Services::open(
        iris_app::paths::Paths::under(dir.path()),
        Some(iris_secrets::Secret::new("test")),
    )
    .unwrap();
    let runtime = iris_app::services::runtime().unwrap();
    let mailer = Arc::new(FakeMailer::new());
    let (outbox, _evenements) = Outbox::new(
        Arc::clone(&mailer) as Arc<dyn Mailer>,
        Duration::from_secs(10),
        runtime.handle().clone(),
    );
    let envoi = Arc::new(iris_sync::SendService::new(
        Arc::clone(&services.engine),
        Arc::new(outbox),
        services.bus.clone(),
    ));

    let f = iris_ui::AppWindow::new().unwrap();
    let avis = iris_app::shell::wire_send_notice(&f, Arc::clone(&envoi));
    f.set_undo_send_seconds(0);

    avis.envoyer(&f, message("Tout de suite"), "Sending".into(), |_| {})
        .unwrap();
    assert!(
        !f.get_send_notice_open(),
        "no notice: it is already leaving"
    );

    let parti = runtime.block_on(async {
        for _ in 0..50 {
            if !mailer.sent().is_empty() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    });
    assert!(parti, "the message left at once");
}

#[test]
fn a_sent_message_leaves_new_message_empty() {
    i_slint_backend_testing::init_no_event_loop();

    let dir = tempfile::tempdir().unwrap();
    let services = Services::open(
        iris_app::paths::Paths::under(dir.path()),
        Some(iris_secrets::Secret::new("test")),
    )
    .unwrap();
    // The composer offers it as a sender, read from the base.
    services
        .store
        .create_account(
            &iris_store::NewAccount::new("moi@example.com", "imap.example.com", "smtp.example.com"),
            iris_app::services::now(),
        )
        .unwrap();
    let runtime = iris_app::services::runtime().unwrap();
    let mailer = Arc::new(FakeMailer::new());
    let (outbox, _evenements) = Outbox::new(
        Arc::clone(&mailer) as Arc<dyn Mailer>,
        Duration::from_secs(10),
        runtime.handle().clone(),
    );
    let envoi = Arc::new(iris_sync::SendService::new(
        Arc::clone(&services.engine),
        Arc::new(outbox),
        services.bus.clone(),
    ));

    let f = iris_ui::AppWindow::new().unwrap();
    f.window().set_size(slint::LogicalSize::new(1280.0, 800.0));
    f.show().unwrap();
    let avis = iris_app::shell::wire_send_notice(&f, Arc::clone(&envoi));
    iris_app::shell::wire_compose(
        &f,
        &services,
        Arc::clone(&envoi),
        avis,
        runtime.handle().clone(),
    );
    f.set_undo_send_seconds(5);

    // Typed, the way a person does: the window opens with the cursor in To.
    f.set_compose_open(true);
    taper(&f, "marie@example.com");
    f.set_compose_subject("Devis".into());
    f.set_compose_body("Bonjour".into());
    f.invoke_compose_send();

    assert!(!f.get_compose_open(), "Send closes the window");
    // New message, again: what the fields show, not only what the window holds.
    f.set_compose_open(true);
    assert_eq!(f.get_compose_to().as_str(), "");
    assert_eq!(f.get_compose_subject().as_str(), "");
    assert_eq!(
        f.get_compose_body().as_str(),
        "",
        "a sent message is not a draft"
    );
    for champ in ["To", "Subject"] {
        let valeur = i_slint_backend_testing::ElementQuery::from_root(&f)
            .match_descendants()
            .match_accessible_role(i_slint_backend_testing::AccessibleRole::TextInput)
            .find_all()
            .into_iter()
            .find(|e| e.accessible_label().as_deref() == Some(champ))
            .and_then(|e| e.accessible_value())
            .unwrap_or_default();
        assert_eq!(valeur.as_str(), "", "the {champ} field shows nothing");
    }
}

fn taper(f: &iris_ui::AppWindow, texte: &str) {
    use slint::platform::WindowEvent;
    for c in texte.chars() {
        let t = slint::SharedString::from(c.to_string());
        f.window()
            .dispatch_event(WindowEvent::KeyPressed { text: t.clone() });
        f.window()
            .dispatch_event(WindowEvent::KeyReleased { text: t });
    }
}
