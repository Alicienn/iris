# Iris

Un client de messagerie de bureau, en Rust, conçu pour tenir **une centaine de boîtes
et un million de messages** sur une machine, sans jamais cesser d'être immédiat.

Trois partis pris le distinguent :

1. **L'axe d'organisation est le travail, pas le classement.** Une conversation est
   *à traiter*, *en attente* ou *traitée*. Aucun rangement thématique n'est imposé.
2. **La performance est une contrainte de conception**, pas une optimisation. Elle
   dicte l'architecture — pagination par curseur, coalescence des événements,
   vue-modèle hors du fil d'affichage — et non l'inverse.
3. **Tout est module.** Le noyau ignore ce qu'est un mail. Thèmes, règles, protocoles
   et extensions sont des modules, et un plugin tiers voit le même modèle de
   permissions qu'un module interne.

---

## État

Le socle est complet et éprouvé : **779 tests** passent, dont ceux qui mettent
réellement en faute la synchronisation, le bac à sable des plugins et le moteur de
rendu.

```bash
cargo test --workspace          # la suite complète
cargo run -p iris-app -- doctor # vérifie l'installation
cargo run -p iris-app           # lance l'application
```

### Ligne de commande

```
iris [run]                     Lance l'application
iris add-account <adresse>     Ajoute un compte (mot de passe demandé)
iris import <fichier>          Ajoute des comptes en lot
iris accounts                  Liste les comptes configurés
iris sync                      Synchronise une fois, sans interface
iris doctor                    Vérifie l'installation
```

Une adresse et un mot de passe suffisent : la découverte enchaîne table embarquée,
autoconfiguration du domaine, base Mozilla, DNS `SRV`, `MX` et sondage — et **dit
d'où vient la configuration proposée**, pour qu'une conjecture ne passe jamais pour
une certitude.

---

## Mesures

Relevées sur un poste de développement ordinaire, avec
`cargo test -p iris-app --release --test budgets -- --ignored` :

| Mesure | Budget visé | Constaté |
|---|---|---|
| Première liste affichée, 100 000 fils | ≤ 400 ms | **45 ms** |
| Lignes détenues en mémoire, 100 000 fils | proportionnel à l'affiché | **60** |
| Page de liste servie pendant le défilement | ≤ 10 ms | **339 µs** |
| Recherche plein texte, 200 000 documents | ≤ 80 ms | **1 à 4 ms** |
| Action de triage | imperceptible | **41 µs** |
| Ouverture à froid de tous les services | ≤ 2 s | **2,5 ms** |
| Insertion, 100 000 messages | — | **0,92 s** |

La dernière ligne mérite son histoire : la première version mettait **192 secondes**.
Le rattachement d'une réponse arrivée avant son original interrogeait
`messages.in_reply_to` sans index, ce qui rendait la synchronisation quadratique. Un
test de non-régression garde désormais cette propriété.

---

## Architecture

Cinq couches, plus un noyau transverse. Aucune dépendance en diagonale : une couche
ne connaît que le contrat de celle qui la précède.

```
  Présentation   iris-ui · iris-htmlview · iris-theme        jamais d'entrée-sortie
  Vue-modèle     iris-viewmodel                              testable sans interface
  Domaine        iris-workflow · iris-thread · iris-rules    pur, sans entrée-sortie
                 iris-search · iris-mime
  Données        iris-store (SQLite) · iris-index (Tantivy)  vérité locale
                 iris-blobs (zstd + LRU)
  Réseau         iris-sync · iris-imap · iris-smtp           tokio
                 iris-discover · iris-secrets · iris-oauth
  Transverse     iris-kernel · iris-types · iris-plugins
```

### Les quatre invariants

Ils priment sur toute autre considération, et chacun est tenu par une **propriété de
structure**, pas par de la discipline :

1. **Zéro entrée-sortie sur le fil d'affichage.** Le vue-modèle vit dans son propre
   fil et ne communique que par messages : le code qui interroge la base ne s'exécute
   pas là où se dessinent les frames.
2. **Rien n'est chargé en entier.** La liste ne détient qu'un préfixe, étendu par
   curseur. La mémoire suit ce qui est affiché, jamais ce qui est stocké.
3. **Toute action locale est instantanée, puis réconciliée.** Écriture immédiate,
   journal d'opérations idempotent, rejeu vers le serveur. Aucun sablier pour un geste
   de l'utilisateur.
4. **Les événements sont groupés.** Coalescence par fenêtres de 16 ms, et un lot qui
   touche plus de 256 fils dégénère en rafraîchissement complet — le coût côté
   interface reste borné quelle que soit l'intensité de la synchronisation.

---

## Interface

Trois colonnes : comptes, file de travail, conversation avec réponse intégrée.
Le thème par défaut, `mono`, est un verre profond **monochrome** : la hiérarchie ne
repose que sur la luminosité et la transparence. Deux presets accompagnent —
`ice` et `sand` — chacun avec un unique accent désaturé.

Un mot sur le verre, parce que c'est contre-intuitif : **il n'y a aucun flou**. Un
flou d'arrière-plan sert à rendre lisible un panneau posé sur du contenu détaillé ;
nos panneaux fixes sont posés sur notre propre dégradé, et flouter un dégradé lisse ne
change rien à l'image tout en coûtant plusieurs millisecondes par frame. Ce qui produit
l'impression de verre, ce sont la translucidité, l'arête lumineuse d'un pixel, et le
grain — trois lignes de shader.

Le corps des messages est rendu en texte riche par défaut, et par un **moteur HTML
complet** (Blitz) lorsque la mise en page l'exige — infolettres en tableaux
imbriqués, mise en page au pixel. Le choix est mesuré, pas deviné : payer un moteur
complet pour un message écrit à la main serait absurde. Blitz rend **dans une image**
plutôt que dans une texture partagée avec l'interface : un corps se redessine à
l'ouverture, pas à chaque frame, et le partage de device lierait nos versions de wgpu
à celles de deux projets tiers indépendants. Sans périphérique graphique compatible,
l'application reste pleinement fonctionnelle en texte riche et le dit au démarrage.

Un thème est **un fichier**, pas du code : couleurs, rayons, espacements, typographie,
densité, durées et paramètres du verre vivent dans un TOML rechargé à chaud. Les trois
thèmes livrés ne sont que trois fichiers parmi d'autres ; si le thème par défaut avait
le moindre privilège dans le code, la modularité annoncée serait une fiction.

---

## Extensions

Les plugins sont du WebAssembly, exécuté en bac à sable. La question « un plugin
peut-il nuire ? » a trois réponses, chacune vérifiée par un test qui met réellement un
plugin en faute :

- **le carburant** borne le temps d'exécution — une boucle infinie s'arrête en
  quelques microsecondes, et le plugin est désactivé ;
- **la mémoire** est plafonnée — un plugin gourmand est refusé, pas la machine ;
- **les capacités** bornent ce qu'il peut demander, et le trousseau ne lui est jamais
  accordé : un plugin capable de lire les mots de passe n'est plus un plugin.

Un plugin défaillant est désactivé, jamais fatal. Le contrat vit dans
[`wit/iris.wit`](crates/iris-plugins/wit/iris.wit) ; un exemple complet et exécuté par
les tests se trouve dans
[`examples/marquer-infolettres`](crates/iris-plugins/examples/marquer-infolettres).

---

## Vie privée

- Images distantes bloquées par défaut ; c'est par leur simple chargement que
  l'expéditeur apprend l'heure de lecture et l'adresse IP.
- Pixels espions distingués des images légitimes par leurs dimensions, leur style ou
  leur domaine — confondre les deux rendrait l'avertissement inutile.
- HTML assaini avant rendu : ni script, ni formulaire, ni ressource externe.
- Secrets dans le trousseau du système, avec repli sur un coffre Argon2id +
  XChaCha20-Poly1305. Le type qui les transporte n'affiche jamais son contenu, pas même
  en débogage.
- Aucune télémétrie.

---

## Documentation

- [Spécification de conception](docs/superpowers/specs/2026-09-13-iris-design.md) —
  les décisions et leurs raisons.
- [Plan d'implémentation](docs/PLAN.md) — epics et stories, avec leur état.

## Licence

MIT ou Apache-2.0, au choix.
