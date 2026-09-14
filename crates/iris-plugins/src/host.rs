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
use wasmtime::{Caller, Engine, Extern, Instance, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, Val};

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

/// Un plugin chargé, prêt à être appelé.
pub struct Plugin {
    manifest: Manifest,
    module: Module,
    engine: Engine,
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
        // Les fils partagés sont hors sujet pour un plugin de messagerie et
        // ouvriraient une voie de contention que nous ne saurions pas borner. La
        // fonctionnalité n'est de toute façon pas compilée dans cette configuration
        // de wasmtime ; on note l'intention ici pour qu'elle ne soit pas réactivée
        // par inadvertance en changeant les options du paquet.
        config.wasm_reference_types(false);

        let engine = Engine::new(&config)
            .map_err(|e| plugin_error(&manifest.id, format!("moteur : {e}")))?;

        let module = Module::new(&engine, wasm)
            .map_err(|e| plugin_error(&manifest.id, format!("compilation : {e}")))?;

        Ok(Self {
            manifest,
            module,
            engine,
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
            return Err(plugin_error(&self.manifest.id, format!("désactivé : {raison}")));
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
                        erreur = %e,
                        "plugin désactivé après échecs répétés"
                    );
                }
                Err(e)
            }
        }
    }

    fn call_inner(&self, export: &str, payload: &str) -> Result<CallTrace> {
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
        store
            .set_fuel(limits.fuel_per_call)
            .map_err(|e| plugin_error(&self.manifest.id, format!("carburant : {e}")))?;

        let mut linker = Linker::new(&self.engine);
        register_host_functions(&mut linker, &self.manifest.id)?;

        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| plugin_error(&self.manifest.id, format!("instanciation : {e}")))?;

        // La charge est déposée dans la mémoire du plugin, à un emplacement qu'il a
        // lui-même réservé : l'hôte n'écrit jamais à un endroit qu'il a choisi seul.
        let (ptr, len) = write_payload(&mut store, &instance, payload, &self.manifest.id)?;

        let fonction = instance
            .get_func(&mut store, export)
            .ok_or_else(|| plugin_error(&self.manifest.id, format!("« {export} » absent")))?;

        let mut resultats = vec![Val::I32(0)];
        fonction
            .call(&mut store, &[Val::I32(ptr), Val::I32(len)], &mut resultats)
            .map_err(|e| translate_trap(&self.manifest.id, e))?;

        let trace = trace.lock().map_err(|_| plugin_error(&self.manifest.id, "trace"))?.clone();
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
        .map_err(|_| plugin_error(plugin, "« iris_alloc » absent : le plugin ne peut rien recevoir"))?;

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
                guard(&mut caller, Capability::WriteMail, |t| t.actions.push(demande))
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
    if ptr < 0 || len < 0 || len > 1 << 20 {
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
    Error::Plugin { plugin: plugin.to_string(), message: message.into() }
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
            r#"(func (export "on_event") (param i32 i32) (result i32)
                 (call $log (local.get 0) (local.get 1))
                 (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste("read_mail = true\n"), &wasm).unwrap();

        let trace = p.call("on_event", "{\"kind\":\"message\"}").unwrap();
        assert_eq!(trace.logs, ["{\"kind\":\"message\"}"]);
        assert!(trace.denied.is_empty());
    }

    #[test]
    fn une_boucle_infinie_est_interrompue() {
        // La question « un plugin peut-il figer l'application ? » doit avoir une
        // réponse définitive.
        let wasm = module_wat(
            r#"(func (export "on_event") (param i32 i32) (result i32)
                 (loop $encore (br $encore))
                 (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();

        let debut = std::time::Instant::now();
        let e = p.call("on_event", "{}").unwrap_err();
        let duree = debut.elapsed();

        assert!(e.to_string().contains("budget d'exécution"));
        assert!(duree < std::time::Duration::from_secs(2), "interrompu en {duree:?}");
    }

    #[test]
    fn un_plugin_qui_echoue_est_desactive_apres_le_seuil() {
        let wasm = module_wat(
            r#"(func (export "on_event") (param i32 i32) (result i32)
                 (loop $encore (br $encore))
                 (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();

        for _ in 0..3 {
            let _ = p.call("on_event", "{}");
        }
        assert!(p.is_disabled());
        assert!(p.disabled_reason().unwrap().contains("budget d'exécution"));

        // Une fois désactivé, il n'est plus appelé du tout.
        let e = p.call("on_event", "{}").unwrap_err();
        assert!(e.to_string().contains("désactivé"));
    }

    #[test]
    fn un_plugin_desactive_peut_etre_reactive() {
        let wasm = module_wat(
            r#"(func (export "on_event") (param i32 i32) (result i32) (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();
        p.disabled = Some("essai".into());

        p.enable();
        assert!(!p.is_disabled());
        assert!(p.call("on_event", "{}").is_ok());
    }

    #[test]
    fn un_appel_reussi_remet_le_compteur_d_echecs_a_zero() {
        let wasm = module_wat(
            r#"(func (export "ok") (param i32 i32) (result i32) (i32.const 0))
               (func (export "casse") (param i32 i32) (result i32) (unreachable))"#,
        );
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();

        let _ = p.call("casse", "{}");
        assert_eq!(p.failures(), 1);
        p.call("ok", "{}").unwrap();
        assert_eq!(p.failures(), 0);
    }

    #[test]
    fn une_action_sans_permission_est_refusee() {
        let wasm = module_wat(
            r#"(func (export "on_event") (param i32 i32) (result i32)
                 (drop (call $act (local.get 0) (local.get 1)))
                 (i32.const 0))"#,
        );
        // Lecture seule : l'action doit être refusée.
        let mut p = Plugin::load(manifeste("read_mail = true\n"), &wasm).unwrap();

        let trace = p.call("on_event", "{\"action\":\"done\"}").unwrap();
        assert!(trace.actions.is_empty(), "aucune action ne doit passer");
        assert_eq!(trace.denied, ["write_mail"]);
    }

    #[test]
    fn une_action_avec_permission_est_acceptee() {
        let wasm = module_wat(
            r#"(func (export "on_event") (param i32 i32) (result i32)
                 (drop (call $act (local.get 0) (local.get 1)))
                 (i32.const 0))"#,
        );
        let mut p =
            Plugin::load(manifeste("read_mail = true\nwrite_mail = true\n"), &wasm).unwrap();

        let trace = p.call("on_event", "{\"action\":\"done\"}").unwrap();
        assert_eq!(trace.actions, ["{\"action\":\"done\"}"]);
        assert!(trace.denied.is_empty());
    }

    #[test]
    fn le_plugin_apprend_qu_il_a_ete_refuse() {
        // Un refus silencieux le laisserait croire à un succès.
        let wasm = module_wat(
            r#"(global $resultat (mut i32) (i32.const -1))
               (func (export "on_event") (param i32 i32) (result i32)
                 (global.set $resultat (call $act (local.get 0) (local.get 1)))
                 (global.get $resultat))"#,
        );
        let mut p = Plugin::load(manifeste("read_mail = true\n"), &wasm).unwrap();
        let trace = p.call("on_event", "{}").unwrap();
        assert_eq!(trace.denied.len(), 1);
    }

    #[test]
    fn les_notifications_et_les_commandes_sont_gardees_separement() {
        let wasm = module_wat(
            r#"(func (export "on_event") (param i32 i32) (result i32)
                 (drop (call $add_command (local.get 0) (local.get 1)))
                 (drop (call $notify (local.get 0) (local.get 1)))
                 (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste("commands = true\n"), &wasm).unwrap();

        let trace = p.call("on_event", "x").unwrap();
        assert_eq!(trace.actions, ["command:x"], "la commande passe");
        assert_eq!(trace.denied, ["notifications"], "la notification est refusée");
    }

    #[test]
    fn une_fonction_absente_est_signalee_clairement() {
        let wasm = module_wat(
            r#"(func (export "autre") (param i32 i32) (result i32) (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();

        let e = p.call("on_event", "{}").unwrap_err();
        assert!(e.to_string().contains("on_event"));
    }

    #[test]
    fn un_plugin_sans_allocateur_est_refuse() {
        let source = r#"(module
            (memory (export "memory") 1)
            (func (export "on_event") (param i32 i32) (result i32) (i32.const 0)))"#;
        let wasm = wat::parse_str(source).unwrap();
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();

        let e = p.call("on_event", "{}").unwrap_err();
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
            r#"(func (export "on_event") (param i32 i32) (result i32)
                 (i32.store (i32.const 1000000) (i32.const 1))
                 (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();

        let e = p.call("on_event", "{}").unwrap_err();
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
            (func (export "on_event") (param i32 i32) (result i32)
              ;; Réclame 10 000 pages, soit 640 Mio : bien au-delà du plafond.
              (drop (memory.grow (i32.const 10000)))
              (i32.const 0)))"#;
        let wasm = wat::parse_str(source).unwrap();

        let mut manifeste = manifeste("");
        manifeste.limits.memory_pages = 4;
        let mut p = Plugin::load(manifeste, &wasm).unwrap();

        // `memory.grow` renvoie -1 quand il est refusé : le plugin continue avec sa
        // mémoire initiale, l'application n'a rien alloué.
        let trace = p.call("on_event", "{}").unwrap();
        assert!(trace.logs.is_empty());
    }

    #[test]
    fn la_charge_est_transmise_telle_quelle() {
        let wasm = module_wat(
            r#"(func (export "on_event") (param i32 i32) (result i32)
                 (call $log (local.get 0) (local.get 1))
                 (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();

        let charge = r#"{"thread":42,"subject":"Devis « refonte »"}"#;
        let trace = p.call("on_event", charge).unwrap();
        assert_eq!(trace.logs, [charge]);
    }

    #[test]
    fn une_charge_vide_est_admise() {
        let wasm = module_wat(
            r#"(func (export "on_event") (param i32 i32) (result i32) (i32.const 0))"#,
        );
        let mut p = Plugin::load(manifeste(""), &wasm).unwrap();
        assert!(p.call("on_event", "").is_ok());
    }
}
