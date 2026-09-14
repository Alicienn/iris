# Iris — Plan d'implémentation

Référence : [spécification de conception](superpowers/specs/2026-09-13-iris-design.md).

Ordre imposé par les dépendances : le contrat, puis le noyau, puis les données, puis
le domaine, puis le réseau, puis la présentation. Chaque story est terminée quand
elle compile, que ses tests passent, et qu'elle est commitée.

**Convention de test** : chaque crate porte ses tests unitaires dans `src/`, ses tests
d'intégration dans `tests/`. Aucun test ne dépend du réseau ; les protocoles sont
testés contre des serveurs simulés en mémoire.

---

## E0 — Fondations

- [x] **S0.1** Dépôt git, `.gitignore`, spécification, plan.
- [x] **S0.2** Workspace Cargo, profils de compilation, lints partagés.
- [x] **S0.3** Vérification de l'accès au registre de paquets et des dépendances lourdes.

## E1 — `iris-types` : le contrat partagé

- [x] **S1.1** Identifiants typés (`AccountId`, `MessageId`, `ThreadId`, `FolderId`, `BlobId`).
- [x] **S1.2** Adresse, en-têtes, enveloppe de message, drapeaux.
- [x] **S1.3** États de workflow et transitions légales.
- [x] **S1.4** Type d'erreur commun et conversions.

## E2 — `iris-kernel` : noyau modulaire

- [x] **S2.1** Bus d'événements typé, multi-abonnés, asynchrone.
- [x] **S2.2** Coalescence temporelle des diffs (fenêtre de 16 ms).
- [x] **S2.3** Registre de modules et cycle de vie (`init` / `start` / `stop`).
- [x] **S2.4** Capacités : déclaration, octroi, refus.
- [x] **S2.5** Configuration typée, rechargeable.

## E3 — `iris-store` : métadonnées

- [x] **S3.1** Ouverture SQLite en WAL, réglages de performance, migrations versionnées.
- [x] **S3.2** Schéma : comptes, dossiers, messages, fils, états, correspondants connus.
- [x] **S3.3** Pagination par curseur (`keyset`) sur la liste principale.
- [x] **S3.4** Journal d'opérations idempotent : écriture, lecture, acquittement.
- [x] **S3.5** Jeu de données synthétique et mesure sur 1 M de messages.

## E4 — `iris-blobs` : corps et pièces jointes

- [x] **S4.1** Écriture et lecture compressées zstd, adressage par identifiant.
- [x] **S4.2** Cache LRU borné en taille, éviction, statistiques.

## E5 — `iris-index` : recherche plein texte

- [x] **S5.1** Schéma Tantivy, écrivain incrémental, validation.
- [x] **S5.2** Requêtes, pagination, surlignage des correspondances.

## E6 — `iris-mime` : analyse des messages

- [x] **S6.1** Parsing MIME : enveloppe, parties, pièces jointes, encodages.
- [x] **S6.2** Sanitisation HTML : suppression des scripts, formulaires, ressources externes.
- [x] **S6.3** Détection des pixels espions et des traqueurs connus.
- [x] **S6.4** Extraction du désabonnement (RFC 2369 et RFC 8058).

## E7 — `iris-thread` : regroupement en fils

- [x] **S7.1** Algorithme JWZ sur `Message-ID`, `In-Reply-To`, `References`.
- [x] **S7.2** Repli par sujet normalisé et fenêtre temporelle.
- [x] **S7.3** Recollage inter-comptes, activable.

## E8 — `iris-workflow` : machine à états

- [x] **S8.1** Transitions manuelles avec pile d'annulation.
- [x] **S8.2** Transitions automatiques, chacune désactivable.
- [x] **S8.3** Report (snooze) et relance à échéance.

## E9 — `iris-rules` : moteur de règles

- [x] **S9.1** Modèle de règle : conditions, actions, ordre, arrêt.
- [x] **S9.2** Évaluation sur un message.
- [x] **S9.3** Simulation à blanc sur l'historique, avec échantillon.

## E10 — `iris-search` : langage de requête

- [x] **S10.1** Analyse lexicale et syntaxique (`from:`, `has:`, `older_than:`, `state:`, texte libre).
- [x] **S10.2** Planification : ce qui va au store, ce qui va à l'index.
- [x] **S10.3** Recherches épinglées comme vues persistantes.

## E11 — `iris-discover` : configuration automatique

- [x] **S11.1** Base ISPDB de Mozilla et autoconfig du domaine.
- [x] **S11.2** Enregistrements SRV, puis MX, puis sondage des ports usuels.
- [x] **S11.3** Repli manuel guidé et validation de la configuration.

## E12 — `iris-secrets`

- [x] **S12.1** Trousseau de l'OS.
- [x] **S12.2** Repli chiffré (Argon2 + AEAD).

## E13 — Protocoles

- [x] **S13.1** Client IMAP : connexion, capacités, sélection, récupération d'en-têtes.
- [x] **S13.2** IDLE, CONDSTORE, QRESYNC avec repli sur `UIDVALIDITY`.
- [x] **S13.3** Pool de connexions borné, quotas par serveur.
- [x] **S13.4** SMTP : envoi, authentification, gestion des erreurs.
- [x] **S13.5** OAuth2 Google et Microsoft, rafraîchissement des jetons.

## E14 — `iris-sync` : orchestration

- [x] **S14.1** Ordonnanceur par priorité de compte.
- [x] **S14.2** Attribution de l'IDLE et polling adaptatif.
- [x] **S14.3** Réconciliation incrémentale et détection des divergences.
- [x] **S14.4** Rejeu du journal d'opérations et résolution de conflits.

## E15 — `iris-viewmodel`

- [x] **S15.1** Fenêtre de lignes, préchargement, invalidation ciblée.
- [x] **S15.2** Sélecteurs d'état et diffs pour l'interface.
- [x] **S15.3** Actions utilisateur : application locale immédiate, journalisation.

## E16 — `iris-theme`

- [x] **S16.1** Schéma des tokens et chargement TOML.
- [x] **S16.2** Les trois thèmes livrés : `mono`, `ice`, `sand`.
- [x] **S16.3** Rechargement à chaud par surveillance de fichiers.

## E17 — Plugins

- [x] **S17.1** Contrat WIT versionné.
- [x] **S17.2** Hôte wasmtime, permissions, quotas CPU et mémoire.
- [x] **S17.3** Plugin d'exemple et tests de bout en bout.

## E18 — `iris-ui` : interface Slint

- [x] **S18.1** Shell à trois colonnes, ancrage des tokens de thème.
- [x] **S18.2** Barre latérale : unifié, épinglés, groupes, recherche de compte.
- [x] **S18.3** Liste virtualisée alimentée par la fenêtre de lignes.
- [x] **S18.4** Vue de conversation et fils repliés.
- [x] **S18.5** Palette de commandes.
- [x] **S18.6** Réponse inline et envoi avec annulation de 10 s.
- [x] **S18.7** Matériau verre : flou, arêtes, grain.

## E19 — `iris-htmlview` : rendu du corps

- [x] **S19.1** Trait `HtmlRenderer` et implémentation de repli en texte riche.
- [x] **S19.2** Moteur Blitz, rendu hors écran vers une image.

## E20 — `iris-app`

- [x] **S20.1** Assemblage des modules, configuration, points d'entrée.
- [x] **S20.2** Ajout de compte de bout en bout.
- [x] **S20.3** Mesures de performance face aux budgets de la spécification.

---

## E21 — Boucler les promesses de la spécification

Le socle est complet, mais trois chaînes s'arrêtent avant leur dernier maillon : les
corps ne sont jamais téléchargés, rien n'est indexé, et répondre n'envoie rien. Tant
qu'elles ne sont pas fermées, la lecture, la recherche et le workflow ne fonctionnent
qu'en théorie.

- [x] **S21.1** Indexation à la synchronisation, puis réindexation à l'arrivée du corps.
- [x] **S21.2** Téléchargement du corps à l'ouverture, mise en cache et rattachement.
- [x] **S21.3** Envoi d'une réponse : composition, file d'envoi, passage en attente.
- [x] **S21.4** Réveil des reports échus et relances, dans la boucle de fond.
- [x] **S21.5** Recherche depuis l'interface, branchée sur le planificateur.

## E22 — Les écrans manquants

- [x] **S22.1** Configuration manuelle d'un compte, quand la découverte échoue.
- [x] **S22.2** Réglages : thème, densité, automatismes du workflow.
- [x] **S22.3** Comptes suspendus : signalement et reprise.

## E23 — Ce qui est construit mais que personne n'atteint

Un audit de la couche application révèle quatre chaînes complètes, testées, et
jamais appelées depuis l'interface. Ce ne sont pas des manques de conception : le
code existe et fonctionne, mais rien ne l'invoque. Une fonctionnalité inatteignable
coûte le même prix qu'une fonctionnalité absente, et ment en plus sur ce que
l'application sait faire.

Deux d'entre elles touchent des exigences posées dès le départ : le support de
Google, et « tout est un module ».

- [x] **S23.1** Les plugins tournent : chargés au démarrage, abonnés au bus, leurs
      commandes dans la palette.
- [x] **S23.2** OAuth Google et Microsoft depuis l'écran d'ajout de compte.
- [x] **S23.3** Pièces jointes : recensées au téléchargement, listées, enregistrables.
- [ ] **S23.4** Compteurs par compte dans la barre latérale.
- [ ] **S23.5** L'état de la synchronisation, visible pendant qu'elle a lieu.
- [ ] **S23.6** Les commentaires devenus faux depuis que les écrans existent.
