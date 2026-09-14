;; Plugin d'exemple : marque comme traitées les infolettres.
;;
;; Écrit directement en WebAssembly textuel pour rester lisible et vérifiable sans
;; chaîne de compilation. Un vrai plugin serait écrit en Rust et compilé vers
;; wasm32-unknown-unknown ; le contrat exposé serait exactement le même, et c'est
;; précisément ce que cet exemple démontre.
;;
;; Protocole : l'hôte appelle `iris_alloc` pour réserver de la place, y dépose la
;; charge JSON de l'événement, puis appelle `iris_on_event(ptr, len)`.

(module
  ;; --- Ce que l'hôte fournit ---
  (import "iris" "log" (func $log (param i32 i32)))
  (import "iris" "act" (func $act (param i32 i32) (result i32)))
  (import "iris" "add_command" (func $add_command (param i32 i32) (result i32)))
  (import "iris" "notify" (func $notify (param i32 i32) (result i32)))

  (memory (export "memory") 1)

  ;; Les constantes du plugin vivent au début de la mémoire ; l'allocateur ne
  ;; distribue qu'au-delà.
  (data (i32.const 0) "unsubscribe")
  (data (i32.const 16) "{\"action\":\"done\"}")
  (data (i32.const 64) "infolettre marquee traitee")

  (global $unsubscribe_ptr i32 (i32.const 0))
  (global $unsubscribe_len i32 (i32.const 11))
  (global $action_ptr i32 (i32.const 16))
  (global $action_len i32 (i32.const 17))
  (global $log_ptr i32 (i32.const 64))
  (global $log_len i32 (i32.const 26))

  ;; Allocateur linéaire, sans libération : un appel est court, et la mémoire du
  ;; plugin est jetée à la fin. Le rendre plus savant serait du travail pour rien.
  (global $next (mut i32) (i32.const 1024))

  (func (export "iris_alloc") (param $taille i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $next))
    (global.set $next (i32.add (global.get $next) (local.get $taille)))
    (local.get $ptr))

  ;; Cherche une sous-chaîne dans la charge. Retourne 1 si trouvée.
  ;;
  ;; Comparaison naïve : la charge fait quelques centaines d'octets, et un
  ;; algorithme sophistiqué coûterait plus en instructions qu'il n'en économiserait.
  (func $contient (param $foin i32) (param $foin_len i32)
                  (param $aiguille i32) (param $aiguille_len i32) (result i32)
    (local $i i32)
    (local $j i32)
    (local $ok i32)

    (if (i32.gt_u (local.get $aiguille_len) (local.get $foin_len))
      (then (return (i32.const 0))))

    (local.set $i (i32.const 0))
    (block $fini
      (loop $balayage
        (br_if $fini (i32.gt_u
          (i32.add (local.get $i) (local.get $aiguille_len))
          (local.get $foin_len)))

        (local.set $j (i32.const 0))
        (local.set $ok (i32.const 1))

        (block $suivant
          (loop $comparaison
            (br_if $suivant (i32.ge_u (local.get $j) (local.get $aiguille_len)))
            (if (i32.ne
                  (i32.load8_u (i32.add (local.get $foin) (i32.add (local.get $i) (local.get $j))))
                  (i32.load8_u (i32.add (local.get $aiguille) (local.get $j))))
              (then
                (local.set $ok (i32.const 0))
                (br $suivant)))
            (local.set $j (i32.add (local.get $j) (i32.const 1)))
            (br $comparaison)))

        (if (local.get $ok) (then (return (i32.const 1))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $balayage)))

    (i32.const 0))

  ;; Point d'entrée : appelé pour chaque message reçu.
  (func (export "iris_on_event") (param $ptr i32) (param $len i32) (result i32)
    ;; Le message propose-t-il un désabonnement ? C'est la marque d'une infolettre,
    ;; et elle est déterministe : elle vient d'un en-tête, pas d'une devinette.
    (if (call $contient (local.get $ptr) (local.get $len)
                        (global.get $unsubscribe_ptr) (global.get $unsubscribe_len))
      (then
        (drop (call $act (global.get $action_ptr) (global.get $action_len)))
        (call $log (global.get $log_ptr) (global.get $log_len))))
    (i32.const 0))

  ;; Appelé au chargement. Rien à préparer ici.
  (func (export "iris_init") (param i32 i32) (result i32) (i32.const 0))
)
