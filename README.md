<p align="center">
  <img src="docs/assets/logo.svg" width="96" height="96" alt="Iris">
</p>

<h1 align="center">Iris</h1>

<p align="center">
  Un client de messagerie de bureau, rapide et sobre.<br>
  Pensé pour traiter son courrier, pas pour le ranger.
</p>

<p align="center">
  <a href="https://github.com/Alicienn/iris/releases/latest"><img src="https://img.shields.io/github/v/release/Alicienn/iris?label=version&color=555" alt="Version"></a>
  <img src="https://img.shields.io/badge/Windows-10%20%7C%2011-555" alt="Windows 10 et 11">
  <img src="https://img.shields.io/badge/licence-MIT%20%7C%20Apache--2.0-555" alt="Licence">
</p>

---

## Points forts

| | |
|---|---|
| <img src="docs/assets/icons/inbox.svg" width="20" alt=""> | **Une file de travail.** Chaque conversation est *à traiter*, *en attente* ou *traitée*. Un geste suffit à la faire avancer. |
| <img src="docs/assets/icons/users.svg" width="20" alt=""> | **Tous vos comptes au même endroit.** Une boîte unifiée, ou un compte à la fois, sans que cela ralentisse. |
| <img src="docs/assets/icons/zap.svg" width="20" alt=""> | **Immédiat.** Ouvrir, archiver et chercher se font sans attente, même avec des centaines de milliers de messages. |
| <img src="docs/assets/icons/shield.svg" width="20" alt=""> | **Respect de la vie privée.** Les images distantes et les pixels espions sont bloqués par défaut. Aucune télémétrie. |
| <img src="docs/assets/icons/layers.svg" width="20" alt=""> | **Modules.** Des règles et des extensions ajoutent des fonctions, et chacune est isolée du reste de l'application. |
| <img src="docs/assets/icons/droplet.svg" width="20" alt=""> | **Thèmes.** Plusieurs apparences sont fournies, sombres et claires. |

## Installation

<img src="docs/assets/icons/download.svg" width="20" alt=""> Téléchargez `iris-setup-<version>.exe` depuis la page
[**Releases**](https://github.com/Alicienn/iris/releases/latest), puis lancez-le.

- L'installation ne demande pas de droits administrateur.
- Si Windows affiche « Windows a protégé votre ordinateur », cliquez sur
  *Informations complémentaires*, puis sur *Exécuter quand même*. Cet avertissement
  apparaît parce que l'installateur n'est pas encore signé.
- La désinstallation se fait depuis *Paramètres › Applications*. Vos messages et vos
  réglages sont conservés.

## Premiers pas

1. Cliquez sur **+** en haut de la colonne des comptes.
2. Saisissez votre adresse et votre mot de passe. Iris trouve seul les réglages du
   serveur, et vous indique d'où il les tient.
3. Vos messages arrivent, en commençant par la boîte de réception.

Vos mots de passe restent dans le trousseau de Windows.

## Raccourcis utiles

| Touche | Action |
|---|---|
| `E` | Marquer comme traité |
| `A` | Archiver |
| `S` | Reporter à demain |
| `R` | Marquer comme lu ou non lu |
| `F` | Suivre (étoile) |
| `Maj`+`3` | Supprimer |
| `F5` | Synchroniser tous les comptes |
| `Ctrl`+`,` | Réglages |

Survolez un bouton pour afficher son raccourci.

## Limites actuelles

- Windows uniquement pour l'instant.
- Les comptes IMAP avec mot de passe fonctionnent directement. Pour Gmail, utilisez
  un [mot de passe d'application](https://myaccount.google.com/apppasswords). La
  connexion par le navigateur (Gmail, Outlook) n'est pas encore activée dans les
  versions publiées.

## Signaler un problème

Ouvrez une [issue](https://github.com/Alicienn/iris/issues) en décrivant ce que vous
faisiez et ce qui s'est passé. N'y joignez jamais un mot de passe ni le contenu d'un
message privé.

---

<sub>Pour contribuer ou comprendre comment Iris est construit, lisez
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Distribué sous licence MIT ou
Apache-2.0, au choix.</sub>
