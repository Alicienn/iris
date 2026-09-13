//! Le registre de modules et leur cycle de vie.
//!
//! Le noyau ignore ce qu'est un mail. Il sait démarrer des modules dans l'ordre,
//! leur remettre exactement les capacités qu'ils ont demandées et qui leur ont été
//! accordées, et les arrêter dans l'ordre inverse.
//!
//! Un module qui échoue au démarrage est **désactivé, pas fatal**. Un client mail
//! dont l'indexation plein texte refuse de démarrer doit continuer à afficher les
//! messages ; c'est exactement ce que garantit cette politique.

use crate::bus::EventBus;
use crate::capability::{Capability, CapabilitySet};
use crate::config::Config;
use async_trait::async_trait;
use iris_types::{Error, Result};
use std::collections::BTreeMap;
use std::fmt;

/// Carte d'identité d'un module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleManifest {
    /// Identifiant stable, en minuscules, unique dans l'application.
    pub id: String,
    pub name: String,
    pub version: String,
    /// Ce que le module a besoin de faire.
    pub requires: Vec<Capability>,
    /// Modules devant être démarrés avant celui-ci.
    pub depends_on: Vec<String>,
}

impl ModuleManifest {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            requires: Vec::new(),
            depends_on: Vec::new(),
        }
    }

    pub fn requiring<I: IntoIterator<Item = Capability>>(mut self, caps: I) -> Self {
        self.requires.extend(caps);
        self
    }

    pub fn after<I, S>(mut self, ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.depends_on.extend(ids.into_iter().map(Into::into));
        self
    }
}

/// Ce qu'un module reçoit du noyau. Rien d'autre ne lui est accessible.
#[derive(Debug, Clone)]
pub struct ModuleContext {
    bus: EventBus,
    config: Config,
    granted: CapabilitySet,
    module_id: String,
}

impl ModuleContext {
    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn module_id(&self) -> &str {
        &self.module_id
    }

    /// Accès au bus, sous réserve d'avoir la capacité correspondante.
    pub fn bus(&self, intent: Capability) -> Result<&EventBus> {
        self.require(&intent)?;
        Ok(&self.bus)
    }

    /// Vérifie une capacité et produit une erreur nommant le demandeur.
    pub fn require(&self, cap: &Capability) -> Result<()> {
        if self.granted.allows(cap) {
            Ok(())
        } else {
            Err(Error::CapabilityDenied {
                capability: cap.to_string(),
                requester: self.module_id.clone(),
            })
        }
    }

    pub fn granted(&self) -> &CapabilitySet {
        &self.granted
    }
}

/// Un module d'Iris.
///
/// `init` prépare sans rien démarrer (ouverture de fichiers, migrations) ; `start`
/// lance les tâches de fond ; `stop` les arrête proprement. La séparation permet de
/// détecter une configuration invalide avant d'avoir lancé quoi que ce soit.
#[async_trait]
pub trait Module: Send + Sync {
    fn manifest(&self) -> ModuleManifest;

    async fn init(&mut self, ctx: &ModuleContext) -> Result<()>;

    async fn start(&mut self, _ctx: &ModuleContext) -> Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleState {
    Registered,
    Initialized,
    Started,
    Stopped,
    /// Le module a échoué et a été mis hors circuit. L'application continue.
    Disabled,
}

struct Entry {
    module: Box<dyn Module>,
    manifest: ModuleManifest,
    context: ModuleContext,
    state: ModuleState,
    failure: Option<String>,
}

impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("id", &self.manifest.id)
            .field("state", &self.state)
            .field("failure", &self.failure)
            .finish()
    }
}

/// Rapport d'une phase du cycle de vie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleReport {
    pub started: Vec<String>,
    pub disabled: Vec<(String, String)>,
}

impl LifecycleReport {
    pub fn all_started(&self) -> bool {
        self.disabled.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct ModuleRegistry {
    bus: EventBus,
    config: Config,
    entries: Vec<Entry>,
    index: BTreeMap<String, usize>,
}

impl ModuleRegistry {
    pub fn new(bus: EventBus, config: Config) -> Self {
        Self { bus, config, entries: Vec::new(), index: BTreeMap::new() }
    }

    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    /// Enregistre un module avec les capacités qui lui sont accordées.
    ///
    /// Échoue immédiatement si le module réclame une capacité non accordée : mieux
    /// vaut refuser au montage qu'échouer plus tard, au milieu d'une synchronisation.
    pub fn register(&mut self, module: Box<dyn Module>, granted: CapabilitySet) -> Result<()> {
        let manifest = module.manifest();

        if self.index.contains_key(&manifest.id) {
            return Err(Error::Config(format!("module « {} » déjà enregistré", manifest.id)));
        }

        if let Err(missing) = granted.check_all(manifest.requires.iter()) {
            return Err(Error::CapabilityDenied {
                capability: missing.to_string(),
                requester: manifest.id,
            });
        }

        for dep in &manifest.depends_on {
            if !self.index.contains_key(dep) {
                return Err(Error::Config(format!(
                    "module « {} » dépend de « {dep} », qui n'est pas enregistré avant lui",
                    manifest.id
                )));
            }
        }

        let context = ModuleContext {
            bus: self.bus.clone(),
            config: self.config.clone(),
            granted,
            module_id: manifest.id.clone(),
        };

        self.index.insert(manifest.id.clone(), self.entries.len());
        self.entries.push(Entry {
            module,
            manifest,
            context,
            state: ModuleState::Registered,
            failure: None,
        });
        Ok(())
    }

    pub fn state_of(&self, id: &str) -> Option<ModuleState> {
        self.index.get(id).map(|&i| self.entries[i].state)
    }

    pub fn failure_of(&self, id: &str) -> Option<&str> {
        self.index.get(id).and_then(|&i| self.entries[i].failure.as_deref())
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.manifest.id.as_str())
    }

    /// Initialise puis démarre tous les modules, dans l'ordre d'enregistrement.
    ///
    /// Un module dont une dépendance a été désactivée est désactivé à son tour, sans
    /// même tenter de démarrer : le laisser s'exécuter sans son socle produirait des
    /// erreurs bien plus difficiles à diagnostiquer.
    pub async fn start_all(&mut self) -> LifecycleReport {
        let mut report = LifecycleReport { started: Vec::new(), disabled: Vec::new() };
        let mut down: Vec<String> = Vec::new();

        for entry in &mut self.entries {
            if let Some(dep) = entry.manifest.depends_on.iter().find(|d| down.contains(d)) {
                let raison = format!("dépendance « {dep} » indisponible");
                entry.state = ModuleState::Disabled;
                entry.failure = Some(raison.clone());
                down.push(entry.manifest.id.clone());
                report.disabled.push((entry.manifest.id.clone(), raison));
                continue;
            }

            let outcome = match entry.module.init(&entry.context).await {
                Ok(()) => {
                    entry.state = ModuleState::Initialized;
                    entry.module.start(&entry.context).await
                }
                Err(e) => Err(e),
            };

            match outcome {
                Ok(()) => {
                    entry.state = ModuleState::Started;
                    report.started.push(entry.manifest.id.clone());
                }
                Err(e) => {
                    let raison = e.to_string();
                    tracing::error!(module = %entry.manifest.id, error = %raison, "module désactivé");
                    entry.state = ModuleState::Disabled;
                    entry.failure = Some(raison.clone());
                    down.push(entry.manifest.id.clone());
                    report.disabled.push((entry.manifest.id.clone(), raison));
                }
            }
        }

        report
    }

    /// Arrête les modules démarrés, dans l'ordre inverse du démarrage.
    ///
    /// Une erreur d'arrêt est consignée mais n'interrompt pas la séquence : lors d'une
    /// fermeture, il faut donner sa chance à chaque module de vider ses tampons.
    pub async fn stop_all(&mut self) -> Vec<(String, String)> {
        let mut errors = Vec::new();
        for entry in self.entries.iter_mut().rev() {
            if entry.state != ModuleState::Started {
                continue;
            }
            if let Err(e) = entry.module.stop().await {
                errors.push((entry.manifest.id.clone(), e.to_string()));
            }
            entry.state = ModuleState::Stopped;
        }
        errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;
    use iris_types::AccountId;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    /// Journal partagé pour observer l'ordre réel des appels de cycle de vie.
    #[derive(Debug, Default, Clone)]
    struct Journal(Arc<std::sync::Mutex<Vec<String>>>);

    impl Journal {
        fn note(&self, s: impl Into<String>) {
            self.0.lock().unwrap().push(s.into());
        }
        fn entries(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    struct Spy {
        id: &'static str,
        journal: Journal,
        requires: Vec<Capability>,
        depends_on: Vec<String>,
        fail_on_init: bool,
        fail_on_start: bool,
        fail_on_stop: bool,
    }

    impl Spy {
        fn new(id: &'static str, journal: Journal) -> Self {
            Self {
                id,
                journal,
                requires: vec![],
                depends_on: vec![],
                fail_on_init: false,
                fail_on_start: false,
                fail_on_stop: false,
            }
        }
        fn boxed(self) -> Box<dyn Module> {
            Box::new(self)
        }
    }

    #[async_trait]
    impl Module for Spy {
        fn manifest(&self) -> ModuleManifest {
            ModuleManifest::new(self.id, self.id)
                .requiring(self.requires.clone())
                .after(self.depends_on.clone())
        }
        async fn init(&mut self, _ctx: &ModuleContext) -> Result<()> {
            self.journal.note(format!("{}:init", self.id));
            if self.fail_on_init {
                return Err(Error::other("init refusé"));
            }
            Ok(())
        }
        async fn start(&mut self, _ctx: &ModuleContext) -> Result<()> {
            self.journal.note(format!("{}:start", self.id));
            if self.fail_on_start {
                return Err(Error::other("start refusé"));
            }
            Ok(())
        }
        async fn stop(&mut self) -> Result<()> {
            self.journal.note(format!("{}:stop", self.id));
            if self.fail_on_stop {
                return Err(Error::other("stop refusé"));
            }
            Ok(())
        }
    }

    fn registry() -> (ModuleRegistry, Journal) {
        (ModuleRegistry::new(EventBus::new(), Config::default()), Journal::default())
    }

    #[tokio::test]
    async fn le_cycle_de_vie_suit_l_ordre_attendu() {
        let (mut reg, j) = registry();
        reg.register(Spy::new("a", j.clone()).boxed(), CapabilitySet::all()).unwrap();
        reg.register(Spy::new("b", j.clone()).boxed(), CapabilitySet::all()).unwrap();

        let rapport = reg.start_all().await;
        assert!(rapport.all_started());
        reg.stop_all().await;

        // init/start dans l'ordre, stop dans l'ordre inverse.
        assert_eq!(
            j.entries(),
            ["a:init", "a:start", "b:init", "b:start", "b:stop", "a:stop"]
        );
    }

    #[tokio::test]
    async fn un_module_ne_peut_pas_etre_enregistre_deux_fois() {
        let (mut reg, j) = registry();
        reg.register(Spy::new("a", j.clone()).boxed(), CapabilitySet::all()).unwrap();
        let e = reg
            .register(Spy::new("a", j.clone()).boxed(), CapabilitySet::all())
            .unwrap_err();
        assert!(e.to_string().contains("déjà enregistré"));
    }

    #[tokio::test]
    async fn une_capacite_manquante_est_refusee_a_l_enregistrement() {
        let (mut reg, j) = registry();
        let mut m = Spy::new("indexeur", j);
        m.requires = vec![Capability::WriteMail];

        let e = reg.register(m.boxed(), CapabilitySet::from_iter([Capability::ReadMail]));
        match e.unwrap_err() {
            Error::CapabilityDenied { capability, requester } => {
                assert_eq!(capability, "write_mail");
                assert_eq!(requester, "indexeur");
            }
            other => panic!("erreur inattendue : {other}"),
        }
    }

    #[tokio::test]
    async fn un_echec_desactive_le_module_sans_arreter_l_application() {
        let (mut reg, j) = registry();
        let mut casse = Spy::new("index", j.clone());
        casse.fail_on_start = true;

        reg.register(casse.boxed(), CapabilitySet::all()).unwrap();
        reg.register(Spy::new("ui", j.clone()).boxed(), CapabilitySet::all()).unwrap();

        let rapport = reg.start_all().await;
        assert_eq!(rapport.started, ["ui"]);
        assert_eq!(rapport.disabled.len(), 1);
        assert_eq!(reg.state_of("index"), Some(ModuleState::Disabled));
        assert_eq!(reg.state_of("ui"), Some(ModuleState::Started));
        assert!(reg.failure_of("index").unwrap().contains("start refusé"));
    }

    #[tokio::test]
    async fn un_module_dont_la_dependance_est_tombee_ne_demarre_pas() {
        let (mut reg, j) = registry();
        let mut socle = Spy::new("store", j.clone());
        socle.fail_on_init = true;
        let mut dependant = Spy::new("sync", j.clone());
        dependant.depends_on = vec!["store".into()];

        reg.register(socle.boxed(), CapabilitySet::all()).unwrap();
        reg.register(dependant.boxed(), CapabilitySet::all()).unwrap();

        let rapport = reg.start_all().await;
        assert!(rapport.started.is_empty());
        assert_eq!(reg.state_of("sync"), Some(ModuleState::Disabled));
        // Il n'a même pas été initialisé : aucun effet de bord parasite.
        assert_eq!(j.entries(), ["store:init"]);
        assert!(reg.failure_of("sync").unwrap().contains("store"));
    }

    #[tokio::test]
    async fn une_dependance_non_enregistree_est_refusee_tot() {
        let (mut reg, j) = registry();
        let mut m = Spy::new("sync", j);
        m.depends_on = vec!["store".into()];
        let e = reg.register(m.boxed(), CapabilitySet::all()).unwrap_err();
        assert!(e.to_string().contains("store"));
    }

    #[tokio::test]
    async fn l_arret_continue_malgre_une_erreur() {
        let (mut reg, j) = registry();
        let mut recalcitrant = Spy::new("a", j.clone());
        recalcitrant.fail_on_stop = true;
        reg.register(recalcitrant.boxed(), CapabilitySet::all()).unwrap();
        reg.register(Spy::new("b", j.clone()).boxed(), CapabilitySet::all()).unwrap();

        reg.start_all().await;
        let erreurs = reg.stop_all().await;

        assert_eq!(erreurs.len(), 1);
        assert_eq!(erreurs[0].0, "a");
        // « b » a bien été arrêté malgré l'échec de « a ».
        assert!(j.entries().contains(&"b:stop".to_string()));
        assert_eq!(reg.state_of("b"), Some(ModuleState::Stopped));
    }

    #[tokio::test]
    async fn le_contexte_refuse_ce_qui_n_a_pas_ete_accorde() {
        struct Curieux;
        static VERDICT: AtomicU32 = AtomicU32::new(0);

        #[async_trait]
        impl Module for Curieux {
            fn manifest(&self) -> ModuleManifest {
                ModuleManifest::new("curieux", "Curieux")
                    .requiring([Capability::SubscribeEvents])
            }
            async fn init(&mut self, ctx: &ModuleContext) -> Result<()> {
                // Accordé : passe.
                ctx.bus(Capability::SubscribeEvents)?
                    .publish(Event::AccountAdded(AccountId(1)));
                // Non accordé : refusé, même si le bus est techniquement là.
                if ctx.bus(Capability::WriteMail).is_err() {
                    VERDICT.store(1, Ordering::SeqCst);
                }
                Ok(())
            }
        }

        let (mut reg, _) = registry();
        reg.register(
            Box::new(Curieux),
            CapabilitySet::from_iter([Capability::SubscribeEvents]),
        )
        .unwrap();
        let rapport = reg.start_all().await;

        assert!(rapport.all_started());
        assert_eq!(VERDICT.load(Ordering::SeqCst), 1, "la capacité absente doit être refusée");
        assert_eq!(reg.bus().published_count(), 1);
    }
}
