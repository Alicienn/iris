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

- [ ] **S3.1** Ouverture SQLite en WAL, réglages de performance, migrations versionnées.
- [ ] **S3.2** Schéma : comptes, dossiers, messages, fils, états, correspondants connus.
- [ ] **S3.3** Pagination par curseur (`keyset`) sur la liste principale.
- [ ] **S3.4** Journal d'opérations idempotent : écriture, lecture, acquittement.
- [ ] **S3.5** Jeu de données synthétique et mesure sur 1 M de messages.

## E4 — `iris-blobs` : corps et pièces jointes

- [ ] **S4.1** Écriture et lecture compressées zstd, adressage par identifiant.
- [ ] **S4.2** Cache LRU borné en taille, éviction, statistiques.

## E5 — `iris-index` : recherche plein texte

- [ ] **S5.1** Schéma Tantivy, écrivain incrémental, validation.
- [ ] **S5.2** Requêtes, pagination, surlignage des correspondances.

## E6 — `iris-mime` : analyse des messages

- [ ] **S6.1** Parsing MIME : enveloppe, parties, pièces jointes, encodages.
- [ ] **S6.2** Sanitisation HTML : suppression des scripts, formulaires, ressources externes.
- [ ] **S6.3** Détection des pixels espions et des traqueurs connus.
- [ ] **S6.4** Extraction du désabonnement (RFC 2369 et RFC 8058).

## E7 — `iris-thread` : regroupement en fils

- [ ] **S7.1** Algorithme JWZ sur `Message-ID`, `In-Reply-To`, `References`.
- [ ] **S7.2** Repli par sujet normalisé et fenêtre temporelle.
- [ ] **S7.3** Recollage inter-comptes, activable.

## E8 — `iris-workflow` : machine à états

- [ ] **S8.1** Transitions manuelles avec pile d'annulation.
- [ ] **S8.2** Transitions automatiques, chacune désactivable.
- [ ] **S8.3** Report (snooze) et relance à échéance.

## E9 — `iris-rules` : moteur de règles

- [ ] **S9.1** Modèle de règle : conditions, actions, ordre, arrêt.
- [ ] **S9.2** Évaluation sur un message.
- [ ] **S9.3** Simulation à blanc sur l'historique, avec échantillon.

## E10 — `iris-search` : langage de requête

- [ ] **S10.1** Analyse lexicale et syntaxique (`from:`, `has:`, `older_than:`, `state:`, texte libre).
- [ ] **S10.2** Planification : ce qui va au store, ce qui va à l'index.
- [ ] **S10.3** Recherches épinglées comme vues persistantes.

## E11 — `iris-discover` : configuration automatique

- [ ] **S11.1** Base ISPDB de Mozilla et autoconfig du domaine.
- [ ] **S11.2** Enregistrements SRV, puis MX, puis sondage des ports usuels.
- [ ] **S11.3** Repli manuel guidé et validation de la configuration.

## E12 — `iris-secrets`

- [ ] **S12.1** Trousseau de l'OS.
- [ ] **S12.2** Repli chiffré (Argon2 + AEAD).

## E13 — Protocoles

- [ ] **S13.1** Client IMAP : connexion, capacités, sélection, récupération d'en-têtes.
- [ ] **S13.2** IDLE, CONDSTORE, QRESYNC avec repli sur `UIDVALIDITY`.
- [ ] **S13.3** Pool de connexions borné, quotas par serveur.
- [ ] **S13.4** SMTP : envoi, authentification, gestion des erreurs.
- [ ] **S13.5** OAuth2 Google et Microsoft, rafraîchissement des jetons.

## E14 — `iris-sync` : orchestration

- [ ] **S14.1** Ordonnanceur par priorité de compte.
- [ ] **S14.2** Attribution de l'IDLE et polling adaptatif.
- [ ] **S14.3** Réconciliation incrémentale et détection des divergences.
- [ ] **S14.4** Rejeu du journal d'opérations et résolution de conflits.

## E15 — `iris-viewmodel`

- [ ] **S15.1** Fenêtre de lignes, préchargement, invalidation ciblée.
- [ ] **S15.2** Sélecteurs d'état et diffs pour l'interface.
- [ ] **S15.3** Actions utilisateur : application locale immédiate, journalisation.

## E16 — `iris-theme`

- [ ] **S16.1** Schéma des tokens et chargement TOML.
- [ ] **S16.2** Les trois thèmes livrés : `mono`, `ice`, `sand`.
- [ ] **S16.3** Rechargement à chaud par surveillance de fichiers.

## E17 — Plugins

- [ ] **S17.1** Contrat WIT versionné.
- [ ] **S17.2** Hôte wasmtime, permissions, quotas CPU et mémoire.
- [ ] **S17.3** Plugin d'exemple et tests de bout en bout.

## E18 — `iris-ui` : interface Slint

- [ ] **S18.1** Shell à trois colonnes, ancrage des tokens de thème.
- [ ] **S18.2** Barre latérale : unifié, épinglés, groupes, recherche de compte.
- [ ] **S18.3** Liste virtualisée alimentée par la fenêtre de lignes.
- [ ] **S18.4** Vue de conversation et fils repliés.
- [ ] **S18.5** Palette de commandes.
- [ ] **S18.6** Réponse inline et envoi avec annulation de 10 s.
- [ ] **S18.7** Matériau verre : flou, arêtes, grain.

## E19 — `iris-htmlview` : rendu du corps

- [ ] **S19.1** Trait `HtmlRenderer` et implémentation de repli en texte riche.
- [ ] **S19.2** Adaptateur Blitz vers texture wgpu partagée.

## E20 — `iris-app`

- [ ] **S20.1** Assemblage des modules, configuration, points d'entrée.
- [ ] **S20.2** Ajout de compte de bout en bout.
- [ ] **S20.3** Mesures de performance face aux budgets de la spécification.
