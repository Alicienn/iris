//! L'hôte WebAssembly.
//!
//! La question à laquelle ce module doit répondre par un « non » définitif est :
//! *un plugin peut-il nuire à l'application ?* Trois mécanismes s'en chargent, et
//! chacun est vérifié par un test qui met réellement un plugin en faute :
//!
//! - **le carburant** borne le temps d'exécution. Une boucle infinie s'arrête d'elle
//!   même, en quelques microsecondes, et le plugin est désactivé ;
//! - **la limite de mémoire** borne l'allocation. Un plugin qui réclame un gigaoctet
//!   se voit refuser, pas la machine ;
//! - **les capacités** bornent ce qu'il peut demander. Une fonction hôte appelée sans
//!   la permission correspondante échoue, et l'échec est tracé.
//!
//! Le protocole d'échange est volontairement rudimentaire : du JSON, passé par la
//! mémoire linéaire. Un plugin de messagerie n'échange pas des mégaoctets par appel,
//! et une interface simple est une interface qu'on peut vérifier.

use crate::manifest::{Limits, Manifest};
use iris_kernel::{Capability, CapabilitySet};
use iris_types::{Error, Result};
use std::sync::{Arc, Mutex};
use wasmtime::{
    Caller, Engine, Extern, Instance, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, Val,
};

/// Ce que l'hôte retient d'un appel.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CallTrace {
    /// Lignes écrites par le plugin.
    pub logs: Vec<String>,
    /// Actions demandées, sous leur forme brute.
    pub actions: Vec<String>,
    /// Permissions refusées pendant l'appel.
    pub denied: Vec<String>,
}

/// L'état accessible aux fonctions hôtes.
#[derive(Debug)]
struct HostState {
    plugin: String,
    caps: CapabilitySet,
    trace: Arc<Mutex<CallTrace>>,
    limits: StoreLimits,
}

/// L'instance vivante d'un plugin : sa mémoire, entre deux appels.
///
/// Elle survit d'un appel au suivant, et c'est **le** point du contrat : l'hôte remet
/// ses réglages à un module une seule fois, à l'initialisation, et le module les garde.
/// Une instance neuve à chaque événement rendrait cette phrase fausse — un module lirait
/// sa liste d'expéditeurs, la rangerait dans une variable, et la retrouverait vide au
/// premier message. C'est exactement ce qui arrivait, sans rien dire : le module se
/// chargeait, s'initialisait, tournait sans erreur, et ne faisait jamais rien.
struct Vivant {
    store: Store<HostState>,
    instance: Instance,
    /// La trace, partagée avec l'état de l'hôte, vidée avant chaque appel.
    trace: Arc<Mutex<CallTrace>>,
}

/// Un plugin chargé, prêt à être appelé.
pub struct Plugin {
    manifest: Manifest,
    module: Module,
    engine: Engine,
    /// L'instance, construite au premier appel et gardée ensuite.
    vivant: Option<Vivant>,
    /// Échecs consécutifs. Au-delà du seuil, le plugin est mis hors circuit.
    failures: u32,
    disabled: Option<String>,
}

impl std::fmt::Debug for Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugin")
            .field("id", &self.manifest.id)
            .field("failures", &self.failures)
            .field("disabled", &self.disabled)
            .finish()
    }
}

impl Plugin {
    /// Compile un plugin à partir de son manifeste et de son code.
    pub fn load(manifest: Manifest, wasm: &[u8]) -> Result<Self> {
        manifest.validate()?;

        let mut config = wasmtime::Config::new();
        // Sans carburant, rien ne peut interrompre une boucle infinie.
        config.consume_fuel(true);
        // Les types de référence restent **actifs**, et il a fallu s'y reprendre à
        // deux fois pour le comprendre.
        //
        // Cette ligne les coupait, dans l'intention d'écarter les fils partagés — qui
        // sont une tout autre fonctionnalité, `wasm_threads`, et qui n'est pas compilée
        // dans cette configuration de wasmtime. Le seul effet réel était de refuser
        // tout module produit par un rustc récent : depuis la version 1.82, la cible
        // `wasm32-unknown-unknown` encode ses appels indirects sous la forme que les
        // types de référence introduisent. Les trois modules livrés avec Iris ne se
        // chargeaient pas, et le message disait « zero byte expected » au milieu d'une
        // fonction de formatage de `core` — ce qui ne mène nulle part.
        //
        // Il n'y a donc rien à couper ici. Ce qu'un module peut faire est décidé par
        // les fonctions que l'hôte lui importe, pas par le jeu d'instructions qu'il a
        // le droit d'employer pour appeler les siennes.

        let engine = Engine::new(&config)
            .map_err(|e| plugin_error(&manifest.id, format!("moteur : {e}")))?;

        // `{e:#}` et non `{e}` : wasmtime rend une erreur en chaîne, et le premier
        // maillon ne dit que « failed to compile <nom mangé de la fonction> ». La
        // raison — une fonctionnalité wasm refusée, un octet invalide — est le second.
        // Sans le dièse, le message affiché nomme précisément la seule chose qui
        // n'aide pas.
        let module = Module::new(&engine, wasm)
            .map_err(|e| plugin_error(&manifest.id, format!("compilation : {e:#}")))?;

        Ok(Self {
            manifest,
            module,
            engine,
            vivant: None,
            failures: 0,
            disabled: None,
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled.is_some()
    }

    pub fn disabled_reason(&self) -> Option<&str> {
        self.disabled.as_deref()
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// Réactive un plugin mis hors circuit, après intervention de l'utilisateur.
    pub fn enable(&mut self) {
        self.disabled = None;
        self.failures = 0;
    }

    /// Appelle une fonction du plugin en lui passant une charge JSON.
    ///
    /// Retourne la trace de l'appel : ce que le plugin a demandé, et ce qui lui a
    /// été refusé.
    pub fn call(&mut self, export: &str, payload: &str) -> Result<CallTrace> {
        if let Some(raison) = &self.disabled {
            return Err(plugin_error(
                &self.manifest.id,
                format!("désactivé : {raison}"),
            ));
        }

        match self.call_inner(export, payload) {
            Ok(trace) => {
                self.failures = 0;
                Ok(trace)
            }
            Err(e) => {
                self.failures += 1;
                if self.failures >= self.manifest.limits.failure_threshold {
                    // Un plugin qui échoue systématiquement est mis hors circuit :
                    // l'application continue, lui non.
                    self.disabled = Some(e.to_string());
                    tracing::warn!(
                        plugin = %self.manifest.id,
                        error = %e,
                        "plugin disabled after repeated failures"
                    );
                }
                Err(e)
            }
        }
    }

    /// Construit l'instance : la mémoire du module, et le lien vers l'hôte.
    fn instancier(&self) -> Result<Vivant> {
        let limits: Limits = self.manifest.limits;
        let trace = Arc::new(Mutex::new(CallTrace::default()));

        let etat = HostState {
            plugin: self.manifest.id.clone(),
            caps: self.manifest.granted_capabilities(),
            trace: Arc::clone(&trace),
            limits: StoreLimitsBuilder::new()
                .memory_size(limits.memory_bytes())
                // Une seule mémoire et une seule table : un plugin n'a aucune raison
                // d'en avoir plusieurs, et les borner ferme une voie d'épuisement.
                .memories(1)
                .tables(1)
                .build(),
        };

        let mut store = Store::new(&self.engine, etat);
        store.limiter(|s| &mut s.limits);

        let mut linker = Linker::new(&self.engine);
        register_host_functions(&mut linker, &self.manifest.id)?;

        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| plugin_error(&self.manifest.id, format!("instanciation : {e}")))?;

        Ok(Vivant {
            store,
            instance,
            trace,
        })
    }

    fn call_inner(&mut self, export: &str, payload: &str) -> Result<CallTrace> {
        let limits: Limits = self.manifest.limits;

        if self.vivant.is_none() {
            self.vivant = Some(self.instancier()?);
        }
        // Une erreur laisse l'instance dans un état dont on ne sait rien — une pile
        // interrompue au milieu d'un emprunt, un tas à moitié écrit. On la jette et le
        // prochain appel repart d'une instance neuve, qui aura perdu ses réglages : ce
        // n'est pas gratuit, mais continuer sur une mémoire dont l'invariant est rompu
        // le serait encore moins.
        let mut echoue = true;
        let resultat = self.appeler(export, payload, limits, &mut echoue);
        if echoue {
            self.vivant = None;
        }
        resultat
    }

    fn appeler(
        &mut self,
        export: &str,
        payload: &str,
        limits: Limits,
        echoue: &mut bool,
    ) -> Result<CallTrace> {
        let id = self.manifest.id.clone();
        let vivant = self.vivant.as_mut().expect("instanciée juste au-dessus");
        let store = &mut vivant.store;
        let instance = vivant.instance;

        // Le carburant est un budget **par appel**, pas par instance : le remettre ici
        // est ce qui rend cette phrase vraie maintenant que l'instance dure.
        store
            .set_fuel(limits.fuel_per_call)
            .map_err(|e| plugin_error(&id, format!("carburant : {e}")))?;

        // Le tas du module ne se libère pas de lui-même — un allocateur linéaire est ce
        // qu'un plugin peut se payer. Sans ce rappel, chaque événement en consommerait
        // un morceau et le module finirait par ne plus pouvoir recevoir de charge, après
        // quelques centaines de messages, en pleine journée. L'export est facultatif :
        // un module écrit à la main peut ne rien allouer du tout.
        //
        // Il n'est pas appelé avant l'initialisation, ce qui donne son sens à la marque
        // que le kit pose : ce qu'un module range à l'initialisation reste, ce qu'il
        // alloue pour un message part avec lui.
        if export != crate::entry_points::INIT {
            if let Ok(reset) = instance.get_typed_func::<(), ()>(&mut *store, "iris_reset") {
                reset
                    .call(&mut *store, ())
                    .map_err(|e| translate_trap(&id, e))?;
            }
        }

        if let Ok(mut t) = vivant.trace.lock() {
            *t = CallTrace::default();
        }

        // La charge est déposée dans la mémoire du plugin, à un emplacement qu'il a
        // lui-même réservé : l'hôte n'écrit jamais à un endroit qu'il a choisi seul.
        let (ptr, len) = write_payload(store, &instance, payload, &id)?;

        let fonction = instance
            .get_func(&mut *store, export)
            .ok_or_else(|| plugin_error(&id, format!("« {export} » absent")))?;

        // Le tampon de sortie doit faire exactement la taille que la fonction annonce,
        // sans quoi wasmtime refuse l'appel avant même de l'exécuter.
        //
        // Un point d'entrée ne rend rien d'utile — l'hôte apprend ce qu'un module veut
        // par les fonctions qu'il appelle, pas par sa valeur de retour. Mais le nombre
        // de résultats dépend de la façon dont le module a été écrit : le plugin
        // d'exemple est en WebAssembly textuel et rend un `i32` ; ceux compilés depuis
        // Rust ne rendent rien, parce que c'est ce qu'écrit une fonction qui ne rend
        // rien. Un tampon d'une seule case supposait la première forme et refusait la
        // seconde, avec « expected 0 results, got 1 » — un message qui semble accuser
        // le module alors qu'il décrit l'hôte.
        let arite = fonction.ty(&*store).results().len();
        let mut resultats = vec![Val::I32(0); arite];
        fonction
            .call(&mut *store, &[Val::I32(ptr), Val::I32(len)], &mut resultats)
            .map_err(|e| translate_trap(&id, e))?;

        let trace = vivant
            .trace
            .lock()
            .map_err(|_| plugin_error(&id, "trace"))?
            .clone();

        *echoue = false;
        Ok(trace)
    }
}

/// Écrit la charge dans la mémoire du plugin.
fn write_payload(
    store: &mut Store<HostState>,
    instance: &Instance,
    payload: &str,
    plugin: &str,
) -> Result<(i32, i32)> {
    let octets = payload.as_bytes();
    let taille = octets.len() as i32;

    let alloc = instance
        .get_typed_func::<i32, i32>(&mut *store, "iris_alloc")
        .map_err(|_| {
            plugin_error(
                plugin,
                "« iris_alloc » absent : le plugin ne peut rien recevoir",
            )
        })?;

    let ptr = alloc
        .call(&mut *store, taille)
        .map_err(|e| translate_trap(plugin, e))?;

    if ptr <= 0 {
        return Err(plugin_error(plugin, "allocation refusée par le plugin"));
    }

    let memoire = instance
        .get_memory(&mut *store, "memory")
        .ok_or_else(|| plugin_error(plugin, "mémoire exportée absente"))?;

    memoire
        .write(&mut *store, ptr as usize, octets)
        .map_err(|e| plugin_error(plugin, format!("écriture de la charge : {e}")))?;

    Ok((ptr, taille))
}

/// Déclare les fonctions que le plugin peut appeler.
fn register_host_functions(linker: &mut Linker<HostState>, plugin: &str) -> Result<()> {
    // Journalisation. Toujours autorisée : elle ne révèle rien et rend les plugins
    // débogables.
    linker
        .func_wrap(
            "iris",
            "log",
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| {
                let texte = read_string(&mut caller, ptr, len).unwrap_or_default();
                if let Ok(mut t) = caller.data().trace.lock() {
                    t.logs.push(texte);
                }
            },
        )
        .map_err(|e| plugin_error(plugin, format!("déclaration de « log » : {e}")))?;

    // Demande d'action sur un fil. Exige d'écrire dans le courrier.
    linker
        .func_wrap(
            "iris",
            "act",
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| -> i32 {
                let demande = read_string(&mut caller, ptr, len).unwrap_or_default();
                guard(&mut caller, Capability::WriteMail, |t| {
                    t.actions.push(demande)
                })
            },
        )
        .map_err(|e| plugin_error(plugin, format!("déclaration de « act » : {e}")))?;

    // Ajout d'une commande à la palette.
    linker
        .func_wrap(
            "iris",
            "add_command",
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| -> i32 {
                let commande = read_string(&mut caller, ptr, len).unwrap_or_default();
                guard(&mut caller, Capability::Commands, |t| {
                    t.actions.push(format!("command:{commande}"))
                })
            },
        )
        .map_err(|e| plugin_error(plugin, format!("déclaration de « add_command » : {e}")))?;

    // Notification système.
    linker
        .func_wrap(
            "iris",
            "notify",
            |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| -> i32 {
                let texte = read_string(&mut caller, ptr, len).unwrap_or_default();
                guard(&mut caller, Capability::Notifications, |t| {
                    t.actions.push(format!("notify:{texte}"))
                })
            },
        )
        .map_err(|e| plugin_error(plugin, format!("déclaration de « notify » : {e}")))?;

    Ok(())
}

/// Vérifie une capacité, exécute l'effet si elle est accordée.
///
/// Retourne 1 en cas de succès, 0 en cas de refus : le plugin apprend qu'il a été
/// refusé plutôt que de croire à un succès silencieux.
fn guard(
    caller: &mut Caller<'_, HostState>,
    capability: Capability,
    effet: impl FnOnce(&mut CallTrace),
) -> i32 {
    let etat = caller.data();
    if !etat.caps.allows(&capability) {
        tracing::debug!(
            plugin = %etat.plugin,
            capacite = %capability,
            "appel refusé faute de permission"
        );
        if let Ok(mut t) = etat.trace.lock() {
            t.denied.push(capability.to_string());
        }
        return 0;
    }
    if let Ok(mut t) = etat.trace.lock() {
        effet(&mut t);
    }
    1
}

fn read_string(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Option<String> {
    // Une longueur absurde est un plugin fautif, pas une raison de paniquer.
    if ptr < 0 || !(0..=1 << 20).contains(&len) {
        return None;
    }
    let memoire = match caller.get_export("memory") {
        Some(Extern::Memory(m)) => m,
        _ => return None,
    };
    let mut tampon = vec![0u8; len as usize];
    Memory::read(&memoire, caller, ptr as usize, &mut tampon).ok()?;
    String::from_utf8(tampon).ok()
}

/// Traduit une interruption d'exécution en erreur lisible.
///
/// Le message brut de wasmtime est une trace d'exécution WebAssembly, illisible pour
/// qui n'écrit pas de plugin. Le code d'interruption, lui, dit précisément ce qui
/// s'est passé — et c'est ce que l'utilisateur doit voir dans la liste des plugins
/// désactivés.
fn translate_trap(plugin: &str, e: wasmtime::Error) -> Error {
    use wasmtime::Trap;

    let message = match e.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => "budget d'exécution épuisé (boucle infinie ?)".to_string(),
        Some(Trap::MemoryOutOfBounds) | Some(Trap::HeapMisaligned) => {
            "accès mémoire hors limites".to_string()
        }
        Some(Trap::StackOverflow) => "débordement de pile (récursion sans fin ?)".to_string(),
        Some(Trap::UnreachableCodeReached) => "le plugin s'est arrêté sur une erreur".to_string(),
        Some(Trap::IntegerDivisionByZero) => "division par zéro".to_string(),
        Some(autre) => format!("interruption : {autre}"),
        None => e.to_string(),
    };
    plugin_error(plugin, message)
}

fn plugin_error(plugin: &str, message: impl Into<String>) -> Error {
    Error::Plugin {
        plugin: plugin.to_string(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un plugin minimal, écrit en WebAssembly textuel.
    ///
    /// Il exporte de quoi recevoir une charge et appelle les fonctions hôtes que le
    /// test veut éprouver. L'écrire à la main plutôt que de compiler un vrai plugin
    /// garde les tests rapides et sans chaîne de compilation supplémentaire.
    fn module_wat(corps: &str) -> Vec<u8> {
        let source = format!(
            r#"(module
                 (import "iris" "log" (func $log (param i32 i32)))
                 (import "iris" "act" (func $act (param i32 i32) (result i32)))
                 (import "iris" "add_command" (func $add_command (param i32 i32) (result i32)))
                 (import "iris" "notify" (func $notify (param i32 i32) (result i32)))
                 (memory (export "memory") 1)
                 (global $next (mut i32) (i32.const 1024))
                 (func (export "iris_alloc") (param $taille i32) (result i32)
                   (local $ptr i32)
                   (local.set $ptr (global.get $next))
                   (global.set $next (i32.add (global.get $next) (local.get $taille)))
                   (local.get $ptr))
                 {corps}
               )"#
        );
        wat::parse_str(&source).expect("WAT valide")
    }

    fn manifeste(permissions: &str) -> Manifest {
        Manifest::from_toml(&format!(
            "id = \"essai\"\nname = \"Essai\"\nversion = \"1\"\n[permissions]\n{permissions}"
        ))
        .unwrap()
    }

    #[test]
    fn un_plugin_minimal_se_charge_et_s_appelle() {
        let wasm = module_wat(
            r#"(func (export "iris_on_event") (param i32 i32) (result i32)
                 (call $log (local.get 0) (local.get 1))
                 (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste("read_mail = true\n"), &wasm).unwrap();

        let trace = p
            .call(crate::entry_points::ON_EVENT, "{\"kind\":\"message\"}")
            .unwrap();
        assert_eq!(trace.logs, ["{\"kind\":\"message\"}"]);
        assert!(trace.denied.is_empty());
    }

    #[test]
    fn une_boucle_infinie_est_interrompue() {
        // La question « un plugin peut-il figer l'application ? » doit avoir une
        // réponse définitive.
        let wasm = module_wat(
            r#"(func (export "iris_on_event") (param i32 i32) (result i32)
                 (loop $encore (br $encore))
                 (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();

        let debut = std::time::Instant::now();
        let e = p.call(crate::entry_points::ON_EVENT, "{}").unwrap_err();
        let duree = debut.elapsed();

        assert!(e.to_string().contains("budget d'exécution"));
        assert!(
            duree < std::time::Duration::from_secs(2),
            "interrompu en {duree:?}"
        );
    }

    #[test]
    fn un_plugin_qui_echoue_est_desactive_apres_le_seuil() {
        let wasm = module_wat(
            r#"(func (export "iris_on_event") (param i32 i32) (result i32)
                 (loop $encore (br $encore))
                 (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();

        for _ in 0..3 {
            let _ = p.call(crate::entry_points::ON_EVENT, "{}");
        }
        assert!(p.is_disabled());
        assert!(p.disabled_reason().unwrap().contains("budget d'exécution"));

        // Une fois désactivé, il n'est plus appelé du tout.
        let e = p.call(crate::entry_points::ON_EVENT, "{}").unwrap_err();
        assert!(e.to_string().contains("désactivé"));
    }

    #[test]
    fn un_plugin_desactive_peut_etre_reactive() {
        let wasm = module_wat(
            r#"(func (export "iris_on_event") (param i32 i32) (result i32) (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();
        p.disabled = Some("essai".into());

        p.enable();
        assert!(!p.is_disabled());
        assert!(p.call(crate::entry_points::ON_EVENT, "{}").is_ok());
    }

    #[test]
    fn un_appel_reussi_remet_le_compteur_d_echecs_a_zero() {
        let wasm = module_wat(
            r#"(func (export "iris_ok") (param i32 i32) (result i32) (i32.const 0))
               (func (export "iris_casse") (param i32 i32) (result i32) (unreachable))"#,
        );
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();

        let _ = p.call("iris_casse", "{}");
        assert_eq!(p.failures(), 1);
        p.call("iris_ok", "{}").unwrap();
        assert_eq!(p.failures(), 0);
    }

    #[test]
    fn une_action_sans_permission_est_refusee() {
        let wasm = module_wat(
            r#"(func (export "iris_on_event") (param i32 i32) (result i32)
                 (drop (call $act (local.get 0) (local.get 1)))
                 (i32.const 0))"#,
        );
        // Lecture seule : l'action doit être refusée.
        let mut p = Plugin::load(manifeste("read_mail = true\n"), &wasm).unwrap();

        let trace = p
            .call(crate::entry_points::ON_EVENT, "{\"action\":\"done\"}")
            .unwrap();
        assert!(trace.actions.is_empty(), "aucune action ne doit passer");
        assert_eq!(trace.denied, ["write_mail"]);
    }

    #[test]
    fn une_action_avec_permission_est_acceptee() {
        let wasm = module_wat(
            r#"(func (export "iris_on_event") (param i32 i32) (result i32)
                 (drop (call $act (local.get 0) (local.get 1)))
                 (i32.const 0))"#,
        );
        let mut p =
            Plugin::load(manifeste("read_mail = true\nwrite_mail = true\n"), &wasm).unwrap();

        let trace = p
            .call(crate::entry_points::ON_EVENT, "{\"action\":\"done\"}")
            .unwrap();
        assert_eq!(trace.actions, ["{\"action\":\"done\"}"]);
        assert!(trace.denied.is_empty());
    }

    #[test]
    fn le_plugin_apprend_qu_il_a_ete_refuse() {
        // Un refus silencieux le laisserait croire à un succès.
        let wasm = module_wat(
            r#"(global $resultat (mut i32) (i32.const -1))
               (func (export "iris_on_event") (param i32 i32) (result i32)
                 (global.set $resultat (call $act (local.get 0) (local.get 1)))
                 (global.get $resultat))"#,
        );
        let mut p = Plugin::load(manifeste("read_mail = true\n"), &wasm).unwrap();
        let trace = p.call(crate::entry_points::ON_EVENT, "{}").unwrap();
        assert_eq!(trace.denied.len(), 1);
    }

    #[test]
    fn les_notifications_et_les_commandes_sont_gardees_separement() {
        let wasm = module_wat(
            r#"(func (export "iris_on_event") (param i32 i32) (result i32)
                 (drop (call $add_command (local.get 0) (local.get 1)))
                 (drop (call $notify (local.get 0) (local.get 1)))
                 (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste("commands = true\n"), &wasm).unwrap();

        let trace = p.call(crate::entry_points::ON_EVENT, "x").unwrap();
        assert_eq!(trace.actions, ["command:x"], "la commande passe");
        assert_eq!(
            trace.denied,
            ["notifications"],
            "la notification est refusée"
        );
    }

    #[test]
    fn une_fonction_absente_est_signalee_clairement() {
        let wasm = module_wat(
            r#"(func (export "iris_autre") (param i32 i32) (result i32) (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();

        let e = p.call(crate::entry_points::ON_EVENT, "{}").unwrap_err();
        assert!(e.to_string().contains("on_event"));
    }

    #[test]
    fn un_plugin_sans_allocateur_est_refuse() {
        let source = r#"(module
            (memory (export "memory") 1)
            (func (export "iris_on_event") (param i32 i32) (result i32) (i32.const 0)))"#;
        let wasm = wat::parse_str(source).unwrap();
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();

        let e = p.call(crate::entry_points::ON_EVENT, "{}").unwrap_err();
        assert!(e.to_string().contains("iris_alloc"));
    }

    #[test]
    fn un_binaire_invalide_est_refuse_au_chargement() {
        let e = Plugin::load(manifeste(""), b"pas du wasm").unwrap_err();
        assert!(e.to_string().contains("compilation"));
    }

    #[test]
    fn un_acces_memoire_hors_limites_est_intercepte() {
        let wasm = module_wat(
            r#"(func (export "iris_on_event") (param i32 i32) (result i32)
                 (i32.store (i32.const 1000000) (i32.const 1))
                 (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();

        let e = p.call(crate::entry_points::ON_EVENT, "{}").unwrap_err();
        assert!(e.to_string().contains("hors limites"));
    }

    #[test]
    fn la_memoire_est_plafonnee() {
        // Un plugin qui réclame un gigaoctet se voit refuser, pas la machine.
        let source = r#"(module
            (import "iris" "log" (func $log (param i32 i32)))
            (import "iris" "act" (func $act (param i32 i32) (result i32)))
            (import "iris" "add_command" (func $add (param i32 i32) (result i32)))
            (import "iris" "notify" (func $notify (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (global $next (mut i32) (i32.const 1024))
            (func (export "iris_alloc") (param i32) (result i32)
              (local.get 0) (drop) (global.get $next))
            (func (export "iris_on_event") (param i32 i32) (result i32)
              ;; Réclame 10 000 pages, soit 640 Mio : bien au-delà du plafond.
              (drop (memory.grow (i32.const 10000)))
              (i32.const 0)))"#;
        let wasm = wat::parse_str(source).unwrap();

        let mut manifeste = manifeste("");
        manifeste.limits.memory_pages = 4;
        let mut p = Plugin::load(manifeste, &wasm).unwrap();

        // `memory.grow` renvoie -1 quand il est refusé : le plugin continue avec sa
        // mémoire initiale, l'application n'a rien alloué.
        let trace = p.call(crate::entry_points::ON_EVENT, "{}").unwrap();
        assert!(trace.logs.is_empty());
    }

    #[test]
    fn la_charge_est_transmise_telle_quelle() {
        let wasm = module_wat(
            r#"(func (export "iris_on_event") (param i32 i32) (result i32)
                 (call $log (local.get 0) (local.get 1))
                 (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();

        let charge = r#"{"thread":42,"subject":"Devis « refonte »"}"#;
        let trace = p.call(crate::entry_points::ON_EVENT, charge).unwrap();
        assert_eq!(trace.logs, [charge]);
    }

    #[test]
    fn une_charge_vide_est_admise() {
        let wasm = module_wat(
            r#"(func (export "iris_on_event") (param i32 i32) (result i32) (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();
        assert!(p.call(crate::entry_points::ON_EVENT, "").is_ok());
    }
}
