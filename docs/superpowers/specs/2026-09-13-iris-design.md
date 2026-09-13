# Iris — Spécification de conception

**Date :** 2026-09-13
**Statut :** validé
**Périmètre de ce document :** l'architecture cible complète, et le contenu précis du premier lot d'implémentation.

---

## 1. Intention

Iris est un client mail de bureau natif, conçu pour un usage à grande échelle : jusqu'à
**~100 boîtes et ~1 million de messages** sur une seule machine, sans que l'interface
ne cesse d'être immédiate.

Trois partis pris fondent le produit :

1. **L'axe d'organisation est le travail, pas le classement.** Une conversation est
   *à traiter*, *en attente* ou *traitée*. Aucun rangement thématique n'est imposé.
2. **La performance est une contrainte de conception, pas une optimisation.** Elle
   dicte l'architecture (voir §4), pas l'inverse.
3. **Tout est module.** Le noyau ignore ce qu'est un mail. Thèmes, règles, vues et
   protocoles sont des modules remplaçables, et un plugin tiers voit la même API que
   les modules internes.

### Non-objectifs (v1)

Explicitement hors périmètre, pour éviter toute dérive :

- Aucune fonctionnalité d'IA.
- Pas de calendrier, de contacts, ni de tâches.
- Pas de chiffrement PGP/S-MIME.
- Pas d'éditeur WYSIWYG complet.
- Pas de store de plugins en ligne.
- Pas de synchronisation multi-appareils propriétaire.

---

## 2. Décisions techniques

| Domaine | Décision | Raison |
|---|---|---|
| Langage | Rust | Un seul langage du socle à l'UI. |
| UI | **Slint** (backend GPU) | Interop wgpu officielle, RAM basse, hot-reload du design, 100 % Rust. |
| Rendu du corps HTML | **Blitz** (moteur HTML/CSS Rust, Vello/wgpu) | Aucune webview, rendu partageant le device GPU de Slint. |
| Métadonnées | **SQLite** en WAL | Requêtes relationnelles, transactions, fiabilité éprouvée. |
| Recherche | **Tantivy** | Pertinence et vitesse hors de portée de FTS5 à 1 M de documents. |
| Corps et pièces jointes | Fichiers **zstd**, cache LRU borné | Sort les gros objets de la base, borne l'espace disque. |
| Protocoles v1 | IMAP + SMTP, OAuth2 Google/Microsoft | Couvre l'intégralité des cas, une seule implémentation à optimiser. |
| Extensibilité | Crates compilées (cœur) + **plugins WASM** wasmtime | Zéro surcoût sur les chemins chauds, bac à sable pour le tiers. |
| Secrets | Trousseau de l'OS, repli fichier chiffré | Aucun secret en clair sur le disque. |
| Plateforme v1 | Windows, puis macOS | Environnement de développement immédiat. |

### Risques assumés

- **Blitz est jeune.** Son support CSS est partiel. Atténuation : les corps sont
  sanitisés et normalisés avant rendu ; l'intégration passe par un trait
  `HtmlRenderer`, ce qui permet de remplacer le moteur sans toucher au reste.
- **L'intégration Slint × Blitz** repose sur le partage d'une texture wgpu. Elle est
  validée par un prototype dédié avant tout engagement (voir §8, lot 0).
- **Slint ne fournit presque aucun widget.** Le shell, la liste virtualisée et la
  palette sont à construire. C'est prévu au plan.

---

## 3. Architecture

Cinq couches, plus un noyau transverse. Aucune dépendance en diagonale : une couche ne
connaît que le contrat de celle qui la précède.

```
                         ┌──────────────────────────────────┐
  Présentation           │ iris-ui · iris-htmlview · theme  │  jamais d'I/O
                         ├──────────────────────────────────┤
  Vue-modèle             │ iris-viewmodel                   │  testable sans UI
                         ├──────────────────────────────────┤
  Domaine                │ workflow · thread · rules ·      │  pur, sans I/O
                         │ search · mime                    │
                         ├──────────────────────────────────┤
  Données                │ store (SQLite) · index (Tantivy) │  vérité locale
                         │ blobs (zstd + LRU)               │
                         ├──────────────────────────────────┤
  Réseau                 │ sync · imap · smtp · discover ·  │  tokio
                         │ secrets                          │
                         └──────────────────────────────────┘
  Transverse : iris-kernel (registre, bus, capacités) · iris-types (contrat)
               iris-plugin-host (wasmtime) · iris-plugin-api (WIT)
```

### Découpage en crates

| Crate | Responsabilité | Dépend de |
|---|---|---|
| `iris-types` | Identifiants, états, erreurs, structures partagées | — |
| `iris-kernel` | Registre de modules, cycle de vie, bus d'événements, capacités | types |
| `iris-store` | Schéma SQLite, migrations, requêtes par curseur, journal d'opérations | types |
| `iris-index` | Index Tantivy, écriture incrémentale, requêtes | types |
| `iris-blobs` | Stockage compressé des corps et PJ, cache LRU | types |
| `iris-mime` | Parsing MIME, sanitisation HTML, détection de traqueurs | types |
| `iris-thread` | Regroupement en fils (JWZ), option inter-comptes | types |
| `iris-workflow` | Machine à états, snooze, relances, automatismes | types, store |
| `iris-rules` | Moteur de règles, simulation à blanc | types, store |
| `iris-search` | Langage de requête, planification store + index | types, store, index |
| `iris-discover` | ISPDB, autoconfig, SRV, MX, sondage de ports | types |
| `iris-secrets` | Trousseau OS, repli chiffré | types |
| `iris-imap` / `iris-smtp` | Protocoles, pool de connexions | types, secrets |
| `iris-sync` | Ordonnanceur, IDLE/polling, réconciliation, rejeu | tout ce qui précède |
| `iris-viewmodel` | État observable, sélecteurs, fenêtre de lignes | domaine, store |
| `iris-theme` | Chargement des tokens, hot-reload | types |
| `iris-htmlview` | Adaptateur Blitz → texture wgpu | types, mime |
| `iris-ui` | Shell Slint, composants, palette | viewmodel, theme, htmlview |
| `iris-plugin-api` | Contrat WIT versionné | — |
| `iris-plugin-host` | wasmtime, permissions, quotas | kernel, plugin-api |
| `iris-app` | Assemblage, configuration, point d'entrée | tout |

### Le noyau

`iris-kernel` ignore ce qu'est un mail. Il expose :

- un **registre de modules** : chaque module déclare un nom, une version, les
  capacités requises et un cycle de vie (`init`, `start`, `stop`) ;
- un **bus d'événements typé**, asynchrone, avec diffusion multi-abonnés et
  coalescence temporelle ;
- des **capacités** distribuées explicitement : un module ne peut atteindre que ce
  qu'il a déclaré vouloir. C'est ce qui rend le modèle de permissions des plugins
  identique à celui des modules internes.

---

## 4. Les quatre invariants de performance

Ils priment sur toute autre considération et sont vérifiés par des tests.

1. **Zéro I/O sur le thread UI.** Aucune requête SQL, aucun parsing MIME, aucun appel
   réseau pendant une frame. L'UI ne consomme que des diffs pré-calculés.
2. **Rien n'est chargé en entier.** La liste ne détient qu'une fenêtre de lignes
   autour du visible, paginée **par curseur** (`keyset`) et jamais par `OFFSET`. La
   mémoire suit ce qui est affiché, pas ce qui est stocké.
3. **Toute action locale est instantanée, puis réconciliée.** Écriture locale
   immédiate, inscription au journal d'opérations idempotent, rejeu vers le serveur.
   Jamais d'attente réseau devant l'utilisateur.
4. **Les événements sont groupés.** Les diffs sont coalescés par fenêtres de 16 ms :
   une synchronisation de 100 comptes ne réveille pas l'interface mille fois par
   seconde.

### Budgets chiffrés

| Métrique | Cible |
|---|---|
| Mémoire, 100 comptes / 1 M messages, au repos | ≤ 400 Mo |
| Mémoire au démarrage à froid | ≤ 150 Mo |
| Temps jusqu'à la première liste affichée | ≤ 400 ms |
| Frame budget côté UI | ≤ 1,5 ms CPU |
| Défilement | 120 fps constants, aucune image sautée |
| Recherche sur 1 M de messages | ≤ 80 ms jusqu'aux premiers résultats |
| CPU au repos, 100 comptes synchronisés | ≤ 1 % |

---

## 5. Modèle de données

Entités principales : `account`, `folder`, `message`, `thread`, `thread_state`,
`blob_ref`, `op_journal`, `rule`, `snooze`, `contact_seen`.

Points structurants :

- **L'état de workflow porte sur le fil**, pas sur le message. On ne traite pas un
  message isolé, on traite un échange.
- `contact_seen` mémorise les correspondants à qui l'utilisateur a déjà répondu.
  C'est la seule « mémoire » du système, et elle sert aux règles.
- `op_journal` contient toute action locale non encore confirmée par le serveur,
  avec une clé d'idempotence.
- Les index sont pensés pour la pagination par curseur : `(state, last_activity_at
  DESC, thread_id)` est l'index directeur de la liste principale.

### Machine à états

```
                 réponse envoyée
    À traiter ────────────────────▶ En attente
        ▲  │                            │
        │  │ action manuelle            │ délai dépassé sans réponse
        │  ▼                            │
        │ Traité ◀──────────────────────┘  (relance)
        │    │
        └────┘ nouveau message reçu
```

`Reporté` (snooze) est **orthogonal** : un fil reporté conserve son état et
réapparaît à l'échéance. Chaque transition automatique est désactivable
individuellement dans les réglages.

---

## 6. Synchronisation

- **Ordonnancement par priorité** : compte actif > épinglés > actifs récemment > le
  reste.
- **Pool borné** d'environ 16 connexions vivantes, attribuées en IDLE aux comptes
  prioritaires, avec rotation. Respecte les quotas des fournisseurs.
- **Polling adaptatif** pour les autres : l'intervalle suit l'activité réelle du
  compte, de 1 à 60 minutes.
- **Sync incrémentale** via CONDSTORE/QRESYNC lorsque le serveur les annonce ; repli
  sur `UIDVALIDITY` + plages d'UID.
- **Cache** : tous les en-têtes sont synchronisés ; les corps sont téléchargés à
  l'ouverture puis conservés compressés sous LRU borné ; les pièces jointes ne sont
  jamais téléchargées automatiquement.
- **Hors ligne** : le journal d'opérations est rejoué à la reconnexion, de façon
  idempotente, avec résolution de conflits déterministe (le serveur gagne sur les
  drapeaux, le local gagne sur l'état de workflow, qui lui est propre).

---

## 7. Interface et design system

**Structure** : trois colonnes. Barre latérale (comptes : unifié, épinglés, groupes,
recherche de compte) · liste des conversations surmontée des onglets d'état
(À traiter / En attente / Traité) · conversation avec zone de réponse intégrée en bas
du fil.

**Direction visuelle par défaut** : verre profond monochrome. Fond sombre neutre,
panneaux réellement translucides, arêtes lumineuses de 1 px, **aucune couleur
d'accent** : la hiérarchie repose sur la luminosité et la transparence. Un grain fin
est appliqué au verre pour supprimer les bandes de dégradé. Les signaux fonctionnels
(erreur, urgence) passent par la forme, la graisse et l'icône plutôt que par la
teinte, à l'exception du rouge d'erreur.

**Thèmes livrés** : `mono` (défaut), `ice` (accent froid désaturé), `sand` (accent
chaud désaturé).

**Design tokens** : couleurs, rayons, espacements, typographie, densité, durées
d'animation et paramètres du verre (flou, opacité, saturation, grain) vivent dans des
fichiers TOML, rechargés à chaud sans redémarrage. Un thème est un fichier, pas du
code.

**Icônes** : un jeu unique, soigné, livré avec l'application. Non remplaçable en v1.
**Polices** : librement choisies parmi celles du système ou par fichier, avec réglage
de graisse, taille, interlettrage et densité.

**Le verre a un coût.** Il est appliqué aux panneaux fixes (barre latérale, liste) et
aux surfaces flottantes (palette, popovers, menus). Il n'est jamais appliqué à un
élément qui se déplace pendant un défilement.

---

## 8. Extensibilité

Le cœur est constitué de crates compilées, pour la performance. Par-dessus,
`iris-plugin-host` exécute des plugins WebAssembly via wasmtime, décrits par un
contrat WIT versionné.

Capacités offertes aux plugins en v1 :

1. **S'abonner aux événements et agir sur les mails** — étiqueter, changer d'état,
   déplacer, notifier.
2. **Ajouter des commandes à la palette** et des raccourcis clavier.
3. **Fournir des vues et des panneaux** rendus par l'application à partir d'une
   description déclarative (aucun accès direct au GPU).
4. **Accéder au réseau et à un espace de stockage propre**, sous permission explicite
   accordée par l'utilisateur à l'installation.

Chaque plugin est limité en temps CPU et en mémoire. Un plugin défaillant est
désactivé, jamais l'application.

**Mises à jour** : le cœur est mis à jour d'un bloc, signé et vérifié, par deltas.
Thèmes et plugins s'installent, se mettent à jour et se désactivent à chaud,
indépendamment de la version de l'application.

---

## 9. Sécurité et vie privée

- Images distantes bloquées par défaut, chargées via un proxy local qui masque
  l'adresse IP et supprime les en-têtes identifiants.
- Détection et signalement des pixels espions ; l'expéditeur est marqué comme
  traqueur dans l'interface.
- Le HTML est sanitisé avant rendu : ni script, ni formulaire, ni ressource externe
  non autorisée.
- Désabonnement en un clic conforme au RFC 8058, préféré au lien HTTP quand
  l'en-tête `List-Unsubscribe-Post` est présent.
- Aucun secret en clair : trousseau de l'OS, repli sur un fichier chiffré par clé
  dérivée en Argon2.
- Aucune télémétrie.

---

## 10. Premier lot : noyau plus tranche verticale

**Objectif** : l'architecture complète est construite *et prouvée de bout en bout*,
pas seulement dessinée.

Contenu :

- Le noyau complet : registre de modules, bus d'événements, capacités, configuration.
- Les couches données (SQLite, Tantivy, blobs) avec leurs migrations et leurs tests.
- Le domaine : parsing MIME, threading, machine à états du workflow.
- Le réseau : découverte automatique, secrets, IMAP en lecture, SMTP en envoi.
- L'ordonnanceur de synchronisation avec pool borné et journal d'opérations.
- Le vue-modèle et sa fenêtre de lignes paginée par curseur.
- Le moteur de thèmes avec les trois presets et le rechargement à chaud.
- L'hôte de plugins avec le contrat WIT et un plugin d'exemple.
- L'interface Slint : shell à trois colonnes, liste virtualisée, palette de
  commandes, réponse inline, envoi avec annulation de 10 s.
- L'intégration Blitz pour le rendu du corps des messages.

**Lot 0, préalable** : trois prototypes jetables, chacun levant un risque —
(a) une texture Blitz composée dans une scène Slint, (b) une liste de 1 M d'entrées
défilant à 120 fps, (c) 50 connexions IMAP simultanées sous pool borné.

**Critère de fin** : ajouter un compte réel avec seulement une adresse et un mot de
passe, voir arriver ses messages, lire un fil correctement rendu, y répondre, et
constater que la conversation est passée en *En attente* — le tout dans les budgets
de performance du §4.
