//! Ce qu'un module d'Iris a besoin de savoir faire, et rien d'autre.
//!
//! Trois modules livrés avec l'application répétaient sinon les mêmes quarante lignes :
//! l'allocateur, le passage de chaînes vers l'hôte, la lecture d'un champ JSON. Trois
//! copies, c'est trois occasions de diverger, et la première divergence serait
//! silencieuse — un plugin qui lit `"sujet"` là où l'hôte écrit `"subject"` ne
//! plante pas, il ne trouve simplement jamais rien.
//!
//! ## Le protocole
//!
//! L'hôte appelle `iris_alloc(n)` pour réserver `n` octets, y écrit une charge JSON,
//! puis appelle `iris_init(ptr, len)` une fois au chargement et `iris_on_event(ptr,
//! len)` à chaque événement. Le plugin répond en appelant les fonctions que l'hôte
//! importe : `act`, `log`, `notify`, `add_command`.
//!
//! ## Ce que le bac à sable n'a pas
//!
//! Pas de réseau, pas d'horloge, pas de système de fichiers, pas d'allocateur système.
//! L'allocateur ci-dessous est linéaire et ne libère rien : un appel dure quelques
//! microsecondes et la mémoire du module est jetée avec lui. Le rendre plus savant
//! serait du travail pour personne.

#![no_std]
#![allow(unsafe_code)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Ce que l'hôte met à disposition.
///
/// Chaque fonction rend `1` quand elle a accepté, `0` sinon — un refus veut dire que
/// la permission n'a pas été accordée, et le module doit continuer sans.
pub mod host {
    #[cfg(target_arch = "wasm32")]
    #[link(wasm_import_module = "iris")]
    extern "C" {
        /// Écrit une ligne dans le journal de l'application.
        pub fn log(ptr: *const u8, len: usize);
        /// Demande une action sur le fil que l'événement concernait.
        pub fn act(ptr: *const u8, len: usize) -> i32;
        /// Affiche un message, attribué au module.
        pub fn notify(ptr: *const u8, len: usize) -> i32;
        /// Déclare une commande dans la palette.
        pub fn add_command(ptr: *const u8, len: usize) -> i32;
    }

    // Hors WebAssembly, il n'y a pas d'hôte : ces quatre fonctions n'existent nulle
    // part et l'éditeur de liens le dirait. On les remplace par des refus.
    //
    // Ce n'est pas une commodité de compilation, c'est ce qui rend un module testable :
    // sans ces bouchons, la seule façon de vérifier qu'une règle de tri range au bon
    // endroit serait de compiler vers wasm, lancer l'application et s'envoyer un
    // message. Un refus est aussi la bonne valeur de retour — un module qui tourne
    // hors du bac à sable ne doit rien pouvoir faire à du vrai courrier.
    #[cfg(not(target_arch = "wasm32"))]
    mod bouchons {
        /// # Safety
        ///
        /// Aucune : la fonction ne touche pas au pointeur.
        pub unsafe fn log(_: *const u8, _: usize) {}
        /// # Safety
        ///
        /// Aucune : la fonction ne touche pas au pointeur.
        pub unsafe fn act(_: *const u8, _: usize) -> i32 {
            0
        }
        /// # Safety
        ///
        /// Aucune : la fonction ne touche pas au pointeur.
        pub unsafe fn notify(_: *const u8, _: usize) -> i32 {
            0
        }
        /// # Safety
        ///
        /// Aucune : la fonction ne touche pas au pointeur.
        pub unsafe fn add_command(_: *const u8, _: usize) -> i32 {
            0
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub use bouchons::{act, add_command, log, notify};
}

/// Écrit une ligne dans le journal.
pub fn log(message: &str) {
    unsafe { host::log(message.as_ptr(), message.len()) }
}

/// Demande une action. La charge est le JSON attendu par l'hôte.
pub fn act(json: &str) -> bool {
    unsafe { host::act(json.as_ptr(), json.len()) == 1 }
}

/// Affiche un message à l'utilisateur.
pub fn notify(message: &str) -> bool {
    unsafe { host::notify(message.as_ptr(), message.len()) == 1 }
}

// --- L'allocateur ---

/// Le tas du module : une zone fixe, distribuée en avançant.
///
/// Deux cent cinquante-six kibioctets couvrent largement une charge d'événement et les
/// chaînes qu'on en tire. Au-delà, `alloc` rend un pointeur nul et l'hôte voit un appel
/// qui échoue — ce qui est ce qu'il faut : un module qui déborde doit s'arrêter, pas
/// écrire à côté.
const TAS: usize = 256 * 1024;
static mut MEMOIRE: [u8; TAS] = [0; TAS];
static mut SUIVANT: usize = 0;

/// Réserve de la place pour l'hôte.
///
/// # Safety
///
/// Appelée par l'hôte, sur un module à fil unique. WebAssembly n'a pas de fils ici :
/// il n'existe pas deux appels simultanés à protéger l'un de l'autre.
#[no_mangle]
pub unsafe extern "C" fn iris_alloc(taille: usize) -> *mut u8 {
    reserve(taille, 1)
}

/// Où le curseur revient entre deux événements, une fois posée.
static mut MARQUE: Option<usize> = None;

/// Rend au module la mémoire qu'il a prise pour l'événement précédent.
///
/// L'hôte appelle cette fonction avant chaque événement, jamais avant
/// l'initialisation. Le premier appel a donc lieu juste après que le module a rangé ses
/// réglages : il pose la marque là, et tous les suivants y ramènent le curseur.
///
/// C'est ce qui rend un allocateur qui ne libère rien tenable sur la durée. Sans elle,
/// chaque message consommerait un morceau du tas et le module cesserait de pouvoir
/// recevoir une charge après quelques centaines de messages — un après-midi, pour une
/// boîte occupée, et sans rien qui explique pourquoi le tri s'est arrêté.
///
/// # Safety
///
/// Appelée par l'hôte, entre deux appels, sur un module à fil unique. Toute référence
/// obtenue d'un appel précédent est invalidée : le kit n'en garde aucune, et un module
/// qui garderait un `&str` tiré de la charge le garderait déjà à tort — la charge de
/// l'événement suivant écrirait par-dessus.
#[no_mangle]
pub unsafe extern "C" fn iris_reset() {
    // Lu et écrit par pointeur brut : `get_or_insert` prendrait une référence mutable
    // vers un statique mutable, ce que l'édition 2024 refuse à juste titre.
    let marque = core::ptr::addr_of_mut!(MARQUE);
    if (*marque).is_none() {
        *marque = Some(SUIVANT);
    }
    SUIVANT = (*marque).unwrap_or(0);
}

/// Avance le curseur, en respectant un alignement.
///
/// # Safety
///
/// Module à fil unique : il n'existe pas deux appels simultanés à protéger l'un de
/// l'autre. `alignement` doit être une puissance de deux, ce que le contrat de
/// [`core::alloc::Layout`] garantit et ce que l'appel de l'hôte fixe à un.
unsafe fn reserve(taille: usize, alignement: usize) -> *mut u8 {
    let base = core::ptr::addr_of_mut!(MEMOIRE).cast::<u8>();

    // L'alignement se calcule sur l'adresse réelle, pas sur le décalage : le tableau
    // statique n'est pas forcément aligné sur huit, et un `u64` posé à une adresse
    // impaire est un plantage sur presque toute machine — sur wasm, un résultat faux.
    let curseur = base as usize + SUIVANT;
    let comble = curseur.wrapping_neg() & (alignement - 1);

    let Some(debut) = SUIVANT.checked_add(comble) else {
        return core::ptr::null_mut();
    };
    let Some(fin) = debut.checked_add(taille) else {
        return core::ptr::null_mut();
    };
    if fin > TAS {
        return core::ptr::null_mut();
    }
    SUIVANT = fin;
    base.add(debut)
}

/// L'allocateur que `alloc` utilise : le même curseur, qui ne rend jamais rien.
///
/// Un module vit le temps d'un appel et sa mémoire est jetée avec lui ; libérer coûte
/// du code et du carburant pour un tas qui va disparaître. Le prix est qu'une boucle
/// qui alloue finit par échouer plutôt que par recycler — ce qui est le bon échec, et
/// ce à quoi sert la borne de carburant du manifeste.
#[cfg(target_arch = "wasm32")]
struct Curseur;

#[cfg(target_arch = "wasm32")]
unsafe impl core::alloc::GlobalAlloc for Curseur {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        reserve(layout.size(), layout.align())
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {}
}

#[cfg(target_arch = "wasm32")]
#[global_allocator]
static ALLOCATEUR: Curseur = Curseur;

/// Rend la charge que l'hôte vient d'écrire.
///
/// # Safety
///
/// `ptr` et `len` viennent de l'hôte et désignent une zone qu'il a remplie via
/// [`iris_alloc`]. Une charge qui n'est pas de l'UTF-8 valide rend une chaîne vide
/// plutôt que de faire paniquer le module : l'hôte compte les échecs, et paniquer sur
/// un octet mal formé mettrait le module hors circuit pour une raison qui n'est pas la
/// sienne.
pub unsafe fn charge<'a>(ptr: *const u8, len: usize) -> &'a str {
    if ptr.is_null() || len == 0 {
        return "";
    }
    core::str::from_utf8(core::slice::from_raw_parts(ptr, len)).unwrap_or("")
}

// --- La lecture du JSON ---

/// La valeur d'un champ texte, sans analyser tout le document.
///
/// Un analyseur complet coûterait dix fois ce fichier pour lire quatre champs d'un
/// objet plat que nous écrivons nous-mêmes. La charge vient de l'hôte, sa forme est
/// connue, et un champ absent rend `None` — ce qui est le seul cas d'erreur qui
/// intéresse un module.
pub fn champ_texte(json: &str, nom: &str) -> Option<String> {
    let debut = position_valeur(json, nom)?;
    let reste = &json[debut..];
    let sans_espaces = reste.trim_start();
    if !sans_espaces.starts_with('"') {
        return None;
    }

    let contenu = &sans_espaces[1..];
    let mut sortie = String::new();
    let mut echappe = false;

    for c in contenu.chars() {
        if echappe {
            // Les échappements qu'un sujet de courrier peut porter. Le reste est
            // recopié tel quel : mal décoder un caractère exotique vaut mieux que
            // perdre le champ entier.
            sortie.push(match c {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                autre => autre,
            });
            echappe = false;
            continue;
        }
        match c {
            '\\' => echappe = true,
            '"' => return Some(sortie),
            autre => sortie.push(autre),
        }
    }
    None
}

/// La valeur d'un champ entier.
pub fn champ_entier(json: &str, nom: &str) -> Option<i64> {
    let debut = position_valeur(json, nom)?;
    let reste = json[debut..].trim_start();

    let mut chiffres = String::new();
    for c in reste.chars() {
        // Le signe n'est accepté qu'en tête : « 4-2 » n'est pas un nombre.
        if !(c.is_ascii_digit() || (c == '-' && chiffres.is_empty())) {
            break;
        }
        chiffres.push(c);
    }
    chiffres.parse().ok()
}

/// Les chaînes d'un tableau de texte.
pub fn champ_liste(json: &str, nom: &str) -> Vec<String> {
    let Some(debut) = position_valeur(json, nom) else {
        return Vec::new();
    };
    let reste = json[debut..].trim_start();
    if !reste.starts_with('[') {
        return Vec::new();
    }

    let Some(fin) = reste.find(']') else {
        return Vec::new();
    };
    let contenu = &reste[1..fin];

    let mut sortie = Vec::new();
    let mut dedans = false;
    let mut courant = String::new();
    let mut echappe = false;

    for c in contenu.chars() {
        if echappe {
            courant.push(c);
            echappe = false;
        } else if c == '\\' {
            echappe = true;
        } else if c == '"' {
            if dedans {
                sortie.push(core::mem::take(&mut courant));
            }
            dedans = !dedans;
        } else if dedans {
            courant.push(c);
        }
    }
    sortie
}

/// Où commence la valeur d'un champ.
///
/// La clé est cherchée sous la forme `"nom"` suivie de deux-points : chercher le nom
/// seul trouverait une clé dont le nom est un préfixe, ou pire, le nom apparaissant
/// dans le sujet d'un message.
fn position_valeur(json: &str, nom: &str) -> Option<usize> {
    let mut motif = String::with_capacity(nom.len() + 2);
    motif.push('"');
    motif.push_str(nom);
    motif.push('"');

    let mut depuis = 0;
    while let Some(trouve) = json[depuis..].find(&motif) {
        let apres = depuis + trouve + motif.len();
        let suite = json[apres..].trim_start();
        if suite.starts_with(':') {
            let deux_points = json[apres..].find(':')? + apres + 1;
            return Some(deux_points);
        }
        depuis = apres;
    }
    None
}

/// Découpe une valeur de réglage en liste, sur les virgules et les retours à la ligne.
///
/// La forme qu'un champ de texte prend quand il contient une liste. Les blancs sont
/// retirés et les entrées vides ignorées : quelqu'un qui laisse une virgule en fin de
/// ligne ne demande pas une entrée vide.
pub fn liste_de_reglage(valeur: &str) -> Vec<String> {
    valeur
        .split([',', '\n', ';'])
        .map(|e| e.trim())
        .filter(|e| !e.is_empty())
        .map(|e| e.to_string())
        .collect()
}

/// Une adresse correspond-elle à une entrée de liste ?
///
/// Une entrée qui commence par `@` est un domaine et couvre tout ce qui s'y termine ;
/// sinon c'est une adresse, comparée en entier. Insensible à la casse des deux côtés,
/// parce que les adresses le sont dans la pratique et que personne ne tape la sienne
/// deux fois de la même façon.
pub fn adresse_correspond(adresse: &str, entree: &str) -> bool {
    let adresse = adresse.trim().to_ascii_lowercase();
    let entree = entree.trim().to_ascii_lowercase();

    if entree.is_empty() {
        return false;
    }
    if let Some(domaine) = entree.strip_prefix('@') {
        return adresse.ends_with(&{
            let mut avec = String::from("@");
            avec.push_str(domaine);
            avec
        });
    }
    adresse == entree
}

/// Le texte contient-il le motif, sans tenir compte de la casse ?
pub fn contient_insensible(foin: &str, aiguille: &str) -> bool {
    let (foin, aiguille) = (foin.as_bytes(), aiguille.as_bytes());

    if aiguille.is_empty() || aiguille.len() > foin.len() {
        return false;
    }

    // Octet par octet, sans rien allouer.
    //
    // La version qui minusculait les deux chaînes tenait en trois lignes et coûtait une
    // copie du sujet **par appel**. Un module de rangement appelle cette fonction une
    // fois par règle et par pièce jointe — six cents fois pour un seul message chargé —
    // sur un tas qui ne se libère jamais. Le résultat était un module qui épuisait son
    // carburant, échouait trois fois et se désactivait tout seul.
    //
    // La comparaison porte sur des octets et non sur des caractères : c'est licite parce
    // qu'un octet d'une séquence UTF-8 multi-octets vaut au moins 0x80, que le pliage de
    // casse ASCII ne le touche pas, et qu'aucun premier octet de séquence valide ne peut
    // coïncider avec un octet de continuation. Une aiguille accentuée se compare donc
    // exactement, ce qui est ce qu'on attend : « Facture » et « facture » se trouvent,
    // « Éte » et « été » non.
    let premier = aiguille[0].to_ascii_lowercase();
    foin.windows(aiguille.len()).any(|fenetre| {
        fenetre[0].to_ascii_lowercase() == premier
            && fenetre
                .iter()
                .zip(aiguille)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

/// Ce que fait un module quand il panique : rien de bruyant.
///
/// Un module ne peut pas dérouler la pile, et un `panic` non traité arrête l'instance.
/// L'hôte compte l'échec et met le module hors circuit après quelques-uns, ce qui est
/// le bon comportement — mais il n'y a rien à écrire ici, puisque l'hôte le dira.
#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panique(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn un_champ_texte_se_lit() {
        let json = r#"{"de":"marie@x.fr","sujet":"Devis"}"#;
        assert_eq!(champ_texte(json, "de").as_deref(), Some("marie@x.fr"));
        assert_eq!(champ_texte(json, "sujet").as_deref(), Some("Devis"));
    }

    #[test]
    fn un_champ_absent_rend_rien() {
        assert_eq!(champ_texte(r#"{"a":"b"}"#, "c"), None);
    }

    #[test]
    fn une_cle_ne_se_confond_pas_avec_une_valeur() {
        // « sujet » apparaît dans la valeur de « de » : chercher le mot seul
        // trouverait la mauvaise position.
        let json = r#"{"de":"le sujet du jour","sujet":"vrai"}"#;
        assert_eq!(champ_texte(json, "sujet").as_deref(), Some("vrai"));
    }

    #[test]
    fn les_echappements_courants_sont_rendus() {
        let json = r#"{"sujet":"Bonjour\nMarie \"la grande\""}"#;
        assert_eq!(
            champ_texte(json, "sujet").as_deref(),
            Some("Bonjour\nMarie \"la grande\"")
        );
    }

    #[test]
    fn un_entier_se_lit_avec_son_signe() {
        assert_eq!(champ_entier(r#"{"heure":7}"#, "heure"), Some(7));
        assert_eq!(champ_entier(r#"{"ecart":-3}"#, "ecart"), Some(-3));
        assert_eq!(champ_entier(r#"{"heure":"7"}"#, "heure"), None);
    }

    #[test]
    fn une_liste_se_lit() {
        let json = r#"{"pieces":["devis.pdf","plan.png"],"autre":1}"#;
        assert_eq!(champ_liste(json, "pieces"), ["devis.pdf", "plan.png"]);
    }

    #[test]
    fn une_liste_vide_se_lit_comme_telle() {
        assert!(champ_liste(r#"{"pieces":[]}"#, "pieces").is_empty());
        assert!(champ_liste(r#"{"a":1}"#, "pieces").is_empty());
    }

    #[test]
    fn un_reglage_se_decoupe_sur_les_separateurs_usuels() {
        assert_eq!(
            liste_de_reglage("marie@x.fr, @client.fr\n luc@y.fr ;"),
            ["marie@x.fr", "@client.fr", "luc@y.fr"]
        );
    }

    #[test]
    fn un_domaine_couvre_ses_adresses() {
        assert!(adresse_correspond("marie@client.fr", "@client.fr"));
        assert!(!adresse_correspond("marie@autre.fr", "@client.fr"));
        // Et ne couvre pas un domaine qui se termine pareil sans en être un.
        assert!(!adresse_correspond("marie@faux-client.fr", "@client.fr"));
    }

    #[test]
    fn une_adresse_se_compare_en_entier_et_sans_casse() {
        assert!(adresse_correspond("Marie@X.fr", "marie@x.fr"));
        assert!(!adresse_correspond("marie@x.fr", "mari@x.fr"));
    }

    #[test]
    fn une_entree_vide_ne_correspond_a_rien() {
        // Sinon une virgule en trop dans la liste ferait correspondre tout le monde.
        assert!(!adresse_correspond("marie@x.fr", ""));
        assert!(!adresse_correspond("marie@x.fr", "   "));
    }

    #[test]
    fn la_recherche_ignore_la_casse() {
        assert!(contient_insensible("Facture N°42", "facture"));
        assert!(!contient_insensible("Devis", "facture"));
        assert!(!contient_insensible("Devis", ""));
    }
}
