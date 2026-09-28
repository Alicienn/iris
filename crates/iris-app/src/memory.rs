//! D'où vient chaque mégaoctet.
//!
//! [`vitals`](crate::vitals) affiche un nombre : « 573 Mo ». C'est le bon nombre —
//! c'est celui que le système retiendra quand la mémoire manquera — mais il ne dit
//! rien de sa composition, et sans composition il n'est pas actionnable. Ce module le
//! décompose, par trois mesures indépendantes qui se recoupent :
//!
//! 1. **Le tas.** Un allocateur global qui compte. Ce que le programme a demandé et
//!    pas encore rendu, à l'octet près, et — au-dessus d'un seuil — *depuis où*.
//!    C'est la seule mesure qui nomme un coupable dans notre propre code.
//! 2. **L'espace d'adressage.** Ce que le processus a réellement engagé auprès du
//!    système, classé par nature : image (le binaire et les DLL), fichier projeté
//!    (l'index de recherche, les polices), privé. La mémoire d'un pilote graphique
//!    n'apparaît dans aucun compteur de tas : elle est engagée par le pilote, pour
//!    notre compte, et seule cette lecture-là la voit.
//! 3. **Les jalons.** Le coût de chaque étape du démarrage, pris au vol : ouvrir la
//!    base, construire la fenêtre, ouvrir un périphérique graphique. Un total ne dit
//!    pas quelle étape l'a produit ; une suite de différences, si.
//!
//! Aucune des trois ne suffit seule, et c'est le propos : le tas explique ce que
//! notre code retient, l'espace d'adressage explique l'écart entre ce chiffre et ce
//! que la machine voit, et les jalons disent quand l'écart est apparu.
//!
//! Le suivi par site d'allocation coûte une capture de pile par grosse allocation ;
//! il ne s'allume donc que sur demande, par `IRIS_MEMREPORT`. Les compteurs globaux,
//! eux, sont deux additions atomiques et tournent toujours : sans eux, il faudrait
//! reconstruire l'application pour commencer à chercher.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

/// Au-delà de cette taille, une allocation est pistée jusqu'à son site.
///
/// Le seuil ne cherche pas à tout voir : il cherche les mégaoctets. Une texture de
/// message, un segment d'index, un tampon de lecture GPU passent tous très au-dessus ;
/// le sujet d'un message passe très en dessous, et pister les millions de petites
/// allocations coûterait plus que ce qu'elles pèsent. Ce qu'elles pèsent ensemble
/// reste connu : c'est le reste, affiché comme tel.
const SITE_THRESHOLD: usize = 128 * 1024;

/// Nombre de cadres capturés par site.
const FRAMES: usize = 24;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static LIVE_BLOCKS: AtomicUsize = AtomicUsize::new(0);
static TOTAL_BYTES: AtomicU64 = AtomicU64::new(0);
static TOTAL_BLOCKS: AtomicU64 = AtomicU64::new(0);
static SITES_ON: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Empêche la comptabilité de se compter elle-même.
    ///
    /// Capturer une pile et l'enregistrer alloue. Sans ce drapeau, la première grosse
    /// allocation appellerait le pisteur, qui allouerait, ce qui appellerait le
    /// pisteur : le programme ne reviendrait jamais.
    static BUSY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// L'allocateur global de l'application.
///
/// Il délègue tout au système et se contente de tenir les comptes. Le surcoût en
/// régime normal est de quelques opérations atomiques par allocation, ce qui ne se
/// mesure pas sur les chemins où l'application passe son temps.
#[derive(Debug, Default, Clone, Copy)]
pub struct Tracking;

/// Il est déclaré dans la bibliothèque et non dans le binaire, pour que les tests de
/// ce module mesurent le même allocateur que l'application.
#[global_allocator]
static ALLOCATEUR: Tracking = Tracking;

// Un allocateur global s'écrit en `unsafe` : le contrat est celui de `GlobalAlloc`,
// et il n'en existe pas de version sûre. Tout ce qui suit délègue à `System` sans
// jamais toucher à la mémoire rendue.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            note_alloc(ptr as usize, layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        if !ptr.is_null() {
            note_alloc(ptr as usize, layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note_free(ptr as usize, layout.size());
        System.dealloc(ptr, layout);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let nouveau = System.realloc(ptr, layout, new_size);
        if !nouveau.is_null() {
            note_free(ptr as usize, layout.size());
            note_alloc(nouveau as usize, new_size);
        }
        nouveau
    }
}

fn note_alloc(ptr: usize, size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
    LIVE_BLOCKS.fetch_add(1, Ordering::Relaxed);
    TOTAL_BYTES.fetch_add(size as u64, Ordering::Relaxed);
    TOTAL_BLOCKS.fetch_add(1, Ordering::Relaxed);

    if size >= SITE_THRESHOLD && SITES_ON.load(Ordering::Relaxed) {
        with_guard(|| sites().enregistre(ptr, size));
    }
}

fn note_free(ptr: usize, size: usize) {
    LIVE.fetch_sub(size, Ordering::Relaxed);
    LIVE_BLOCKS.fetch_sub(1, Ordering::Relaxed);

    if size >= SITE_THRESHOLD && SITES_ON.load(Ordering::Relaxed) {
        with_guard(|| sites().oublie(ptr));
    }
}

/// Exécute la comptabilité, sauf si on y est déjà.
fn with_guard(f: impl FnOnce()) {
    let _ = BUSY.try_with(|busy| {
        if busy.get() {
            return;
        }
        busy.set(true);
        f();
        busy.set(false);
    });
}

// --- Les sites ---

#[derive(Debug, Default)]
struct Site {
    frames: Vec<usize>,
    live: usize,
    live_blocks: usize,
    total: u64,
    peak: usize,
}

#[derive(Debug, Default)]
struct Sites {
    /// L'allocation vivante : d'où elle vient, et ce qu'elle pèse.
    blocs: HashMap<usize, (u32, usize)>,
    connus: HashMap<Vec<usize>, u32>,
    sites: Vec<Site>,
}

impl Sites {
    fn enregistre(&mut self, ptr: usize, size: usize) {
        let mut frames = Vec::with_capacity(FRAMES);
        backtrace::trace(|frame| {
            frames.push(frame.ip() as usize);
            frames.len() < FRAMES
        });

        let id = match self.connus.get(&frames) {
            Some(id) => *id,
            None => {
                let id = self.sites.len() as u32;
                self.connus.insert(frames.clone(), id);
                self.sites.push(Site {
                    frames,
                    ..Default::default()
                });
                id
            }
        };

        let site = &mut self.sites[id as usize];
        site.live += size;
        site.live_blocks += 1;
        site.total += size as u64;
        site.peak = site.peak.max(site.live);
        self.blocs.insert(ptr, (id, size));
    }

    fn oublie(&mut self, ptr: usize) {
        if let Some((id, size)) = self.blocs.remove(&ptr) {
            let site = &mut self.sites[id as usize];
            site.live = site.live.saturating_sub(size);
            site.live_blocks = site.live_blocks.saturating_sub(1);
        }
    }
}

static SITES: Mutex<Option<Sites>> = Mutex::new(None);

fn sites() -> impl std::ops::DerefMut<Target = Sites> {
    struct Garde(std::sync::MutexGuard<'static, Option<Sites>>);
    impl std::ops::Deref for Garde {
        type Target = Sites;
        fn deref(&self) -> &Sites {
            self.0.as_ref().expect("initialisé juste avant")
        }
    }
    impl std::ops::DerefMut for Garde {
        fn deref_mut(&mut self) -> &mut Sites {
            self.0.as_mut().expect("initialisé juste avant")
        }
    }

    let mut garde = SITES.lock().unwrap_or_else(|e| e.into_inner());
    if garde.is_none() {
        *garde = Some(Sites::default());
    }
    Garde(garde)
}

/// Ce que le tas retient.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Heap {
    pub live: usize,
    /// Le plus haut niveau atteint depuis le démarrage, ou depuis [`reset_peak`].
    pub peak: usize,
    pub blocks: usize,
}

/// Lit les compteurs du tas.
pub fn heap() -> Heap {
    Heap {
        live: LIVE.load(Ordering::Relaxed),
        peak: PEAK.load(Ordering::Relaxed),
        blocks: LIVE_BLOCKS.load(Ordering::Relaxed),
    }
}

/// Ramène le maximum au niveau courant, et rend ce niveau.
///
/// Un maximum depuis le démarrage ne dit rien du coût d'une opération précise. Le
/// remettre à plat juste avant permet de mesurer le pic d'un rendu de message —
/// c'est-à-dire, exactement, le nombre de copies que ce rendu fait de son image.
pub fn reset_peak() -> usize {
    let courant = LIVE.load(Ordering::Relaxed);
    PEAK.store(courant, Ordering::Relaxed);
    courant
}

/// Allume le suivi par site d'allocation.
///
/// À appeler tôt : ce qui a été alloué avant reste compté dans le total mais sans
/// provenance, et apparaîtra donc comme « non attribué ».
pub fn track_sites(on: bool) {
    SITES_ON.store(on, Ordering::Relaxed);
}

/// Le suivi détaillé est-il demandé, et à quelle cadence ?
///
/// `IRIS_MEMREPORT` vaut un nombre de secondes entre deux rapports.
pub fn requested_interval() -> Option<std::time::Duration> {
    let valeur = std::env::var("IRIS_MEMREPORT").ok()?;
    let secondes: u64 = valeur.trim().parse().ok()?;
    (secondes > 0).then(|| std::time::Duration::from_secs(secondes))
}

// --- Les jalons ---

#[derive(Debug, Clone)]
struct Jalon {
    label: &'static str,
    heap: usize,
    rss: u64,
    prive: u64,
}

static JALONS: Mutex<Vec<Jalon>> = Mutex::new(Vec::new());

/// Note ce que coûte l'étape qui vient de s'achever.
///
/// Un total ne dit pas quelle étape l'a produit. Quelques lignes prises au bon moment
/// valent mieux qu'un profileur lancé après coup, parce qu'elles séparent ce que
/// l'application a alloué de ce que le système a engagé pour elle — ouvrir un
/// périphérique graphique ne coûte presque rien au tas et beaucoup au processus.
pub fn mark(label: &'static str) {
    let regions = regions();
    let jalon = Jalon {
        label,
        heap: LIVE.load(Ordering::Relaxed),
        rss: crate::vitals::resident_bytes().unwrap_or(0),
        prive: regions.private_rw + regions.private_wc,
    };
    JALONS.lock().unwrap_or_else(|e| e.into_inner()).push(jalon);
}

// --- L'espace d'adressage ---

/// Ce que le processus a engagé, par nature.
#[derive(Debug, Default, Clone, Copy)]
pub struct Regions {
    /// Le binaire et les bibliothèques chargées. Partagé avec les autres processus
    /// qui les utilisent : compté dans l'ensemble résident, mais pas à notre charge
    /// seuls.
    pub image: u64,
    /// Fichiers projetés en mémoire — l'index de recherche, les caches de polices.
    /// La page vient du disque et y retourne sans coûter d'écriture.
    pub mapped: u64,
    /// Mémoire privée ordinaire : le tas, les piles, les arènes.
    pub private_rw: u64,
    /// Mémoire privée en écriture combinée. C'est la signature d'un pilote
    /// graphique : de la mémoire visible du processeur et destinée à la carte.
    /// Aucun compteur de tas ne la voit, et elle se paie pourtant.
    pub private_wc: u64,
    /// Le reste du privé : pages exécutables, gardes de pile.
    pub private_other: u64,
    pub regions: u32,
    /// Ce que les tas Windows du processus ont engagé, tous confondus.
    ///
    /// La distinction tranche une question qu'aucune autre mesure ne tranche : une
    /// mémoire privée qui n'est dans aucun tas n'a pas été demandée par un `malloc`
    /// — ni le nôtre, ni celui d'une DLL. Elle a été prise à la page, directement au
    /// système, ce que font les pilotes et eux à peu près seuls.
    pub heaps_committed: u64,
    pub heaps: u32,
}

impl Regions {
    pub fn total(&self) -> u64 {
        self.image + self.mapped + self.private_rw + self.private_wc + self.private_other
    }
}

/// Lit l'espace d'adressage du processus courant.
pub fn regions() -> Regions {
    platform::regions()
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod platform {
    use super::Regions;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct MemoryBasicInformation {
        base: usize,
        allocation_base: usize,
        allocation_protect: u32,
        partition_id: u16,
        alignment: u16,
        region_size: usize,
        state: u32,
        protect: u32,
        kind: u32,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct HeapSummary {
        cb: u32,
        allocated: usize,
        committed: usize,
        reserved: usize,
        max_reserve: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn VirtualQuery(
            address: *const core::ffi::c_void,
            buffer: *mut MemoryBasicInformation,
            length: usize,
        ) -> usize;
        fn GetProcessHeaps(count: u32, heaps: *mut isize) -> u32;
        fn HeapSummary(heap: isize, flags: u32, summary: *mut HeapSummary) -> i32;
    }

    /// Ce que les tas du processus ont engagé, et combien il y en a.
    fn heaps() -> (u64, u32) {
        // Deux appels : le premier pour compter, le second pour lire. Un tas peut
        // naître entre les deux ; la borne du second évite d'écrire au-delà du
        // tampon si cela arrive.
        // SAFETY: un appel de comptage, qui n'écrit rien avec un tampon nul.
        let nombre = unsafe { GetProcessHeaps(0, core::ptr::null_mut()) };
        if nombre == 0 {
            return (0, 0);
        }

        let mut poignees = vec![0isize; nombre as usize];
        // SAFETY: le tampon appartient à cette pile et sa longueur est celle passée.
        let lus = unsafe { GetProcessHeaps(nombre, poignees.as_mut_ptr()) }.min(nombre);

        let mut engage = 0u64;
        for poignee in &poignees[..lus as usize] {
            let mut resume = HeapSummary {
                cb: core::mem::size_of::<HeapSummary>() as u32,
                ..Default::default()
            };
            // SAFETY: une poignée rendue par l'appel précédent, et une structure de
            // sortie possédée par cette pile dont la taille est renseignée.
            if unsafe { HeapSummary(*poignee, 0, &mut resume) } != 0 {
                engage += resume.committed as u64;
            }
        }
        (engage, lus)
    }

    const MEM_COMMIT: u32 = 0x1000;
    const MEM_PRIVATE: u32 = 0x0002_0000;
    const MEM_MAPPED: u32 = 0x0004_0000;
    const MEM_IMAGE: u32 = 0x0100_0000;
    const PAGE_WRITECOMBINE: u32 = 0x400;
    const PAGE_GUARD: u32 = 0x100;
    const PAGE_READWRITE: u32 = 0x04;

    pub fn regions() -> Regions {
        let mut total = Regions::default();
        let mut adresse: usize = 0;
        let taille = core::mem::size_of::<MemoryBasicInformation>();

        // La limite de l'espace utilisateur sur Windows 64 bits. La boucle s'arrête de
        // toute façon dès que la requête échoue ; la borne évite seulement de tourner
        // indéfiniment si une région déclarait une taille nulle.
        while adresse < 0x7FFF_FFFF_0000 {
            let mut info = MemoryBasicInformation::default();

            // SAFETY: une structure de sortie possédée par cette pile, dont la taille
            // est passée à l'appel. L'adresse interrogée n'est jamais déréférencée.
            let lu = unsafe { VirtualQuery(adresse as *const _, &mut info, taille) };
            if lu == 0 || info.region_size == 0 {
                break;
            }

            if info.state == MEM_COMMIT {
                let octets = info.region_size as u64;
                total.regions += 1;
                match info.kind {
                    MEM_IMAGE => total.image += octets,
                    MEM_MAPPED => total.mapped += octets,
                    MEM_PRIVATE => {
                        if info.protect & PAGE_WRITECOMBINE != 0 {
                            total.private_wc += octets;
                        } else if info.protect & PAGE_GUARD == 0
                            && info.protect & PAGE_READWRITE != 0
                        {
                            total.private_rw += octets;
                        } else {
                            total.private_other += octets;
                        }
                    }
                    _ => total.private_other += octets,
                }
            }

            adresse = info.base.saturating_add(info.region_size);
        }

        let (engage, nombre) = heaps();
        total.heaps_committed = engage;
        total.heaps = nombre;
        total
    }
}

#[cfg(not(windows))]
mod platform {
    use super::Regions;

    /// Sur les autres systèmes, `/proc/self/smaps` donnerait l'équivalent. Rien ne
    /// l'implémente ici tant que l'application n'y tourne pas.
    pub fn regions() -> Regions {
        Regions::default()
    }
}

// --- Le rapport ---

fn mo(octets: u64) -> String {
    format!("{:>9.1} Mo", octets as f64 / (1024.0 * 1024.0))
}

/// Écrit la décomposition complète.
pub fn report() -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(8192);

    let regions = regions();
    let rss = crate::vitals::resident_bytes().unwrap_or(0);
    let live = LIVE.load(Ordering::Relaxed);

    let _ = writeln!(s, "=== Mémoire ===");
    let _ = writeln!(s, "  ensemble résident      {}", mo(rss));
    let _ = writeln!(s, "  engagé, total          {}", mo(regions.total()));
    let _ = writeln!(s);
    let _ = writeln!(s, "  image (exe + DLL)      {}", mo(regions.image));
    let _ = writeln!(s, "  fichiers projetés      {}", mo(regions.mapped));
    let _ = writeln!(s, "  privé, ordinaire       {}", mo(regions.private_rw));
    let _ = writeln!(
        s,
        "  privé, écr. combinée   {}   (pilote graphique)",
        mo(regions.private_wc)
    );
    let _ = writeln!(s, "  privé, autre           {}", mo(regions.private_other));
    let _ = writeln!(s, "  régions engagées       {:>9}", regions.regions);

    // Ce que le privé doit à un `malloc` quelconque, et ce qu'il doit à des pages
    // prises directement au système. La seconde ligne est celle qui désigne un
    // pilote : rien d'autre, dans ce processus, ne se sert ainsi.
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "  dont tas Windows       {}   ({} tas, le nôtre compris)",
        mo(regions.heaps_committed),
        regions.heaps
    );
    let _ = writeln!(
        s,
        "  dont pages nues        {}   (VirtualAlloc direct : pilotes)",
        mo((regions.private_rw + regions.private_wc).saturating_sub(regions.heaps_committed))
    );

    let _ = writeln!(s);
    let _ = writeln!(s, "=== Tas ===");
    let _ = writeln!(s, "  vivant                 {}", mo(live as u64));
    let _ = writeln!(
        s,
        "  maximum atteint        {}",
        mo(PEAK.load(Ordering::Relaxed) as u64)
    );
    let _ = writeln!(
        s,
        "  blocs vivants          {:>9}",
        LIVE_BLOCKS.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        s,
        "  alloué depuis le début {}  en {} blocs",
        mo(TOTAL_BYTES.load(Ordering::Relaxed)),
        TOTAL_BLOCKS.load(Ordering::Relaxed)
    );
    // L'écart entre le tas vivant et le privé ordinaire est ce que l'allocateur garde
    // sans nous le prêter : segments réservés, fragmentation, piles des quarante fils.
    // Le nommer évite de le chercher dans le code.
    let _ = writeln!(
        s,
        "  écart privé − tas      {}   (fragmentation, piles, arènes)",
        mo(regions.private_rw.saturating_sub(live as u64))
    );

    let jalons = JALONS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if !jalons.is_empty() {
        let _ = writeln!(s);
        let _ = writeln!(s, "=== Étapes du démarrage (différences) ===");
        let _ = writeln!(
            s,
            "  {:<28} {:>12} {:>12} {:>12}",
            "", "tas", "privé", "résident"
        );
        let mut precedent: Option<&Jalon> = None;
        for jalon in &jalons {
            let (dh, dp, dr) = match precedent {
                Some(p) => (
                    jalon.heap as i64 - p.heap as i64,
                    jalon.prive as i64 - p.prive as i64,
                    jalon.rss as i64 - p.rss as i64,
                ),
                None => (jalon.heap as i64, jalon.prive as i64, jalon.rss as i64),
            };
            let d = |v: i64| format!("{:>+9.1} Mo", v as f64 / (1024.0 * 1024.0));
            let _ = writeln!(s, "  {:<28} {} {} {}", jalon.label, d(dh), d(dp), d(dr));
            precedent = Some(jalon);
        }
    }

    if SITES_ON.load(Ordering::Relaxed) {
        s.push_str(&sites_report(live));
    } else {
        let _ = writeln!(s);
        let _ = writeln!(
            s,
            "(IRIS_MEMREPORT n'est pas posé : les gros blocs ne sont pas rattachés à\n\
             leur site d'allocation.)"
        );
    }

    s
}

/// Le détail par site, du plus lourd au plus léger.
fn sites_report(live: usize) -> String {
    use std::fmt::Write;

    // La copie est prise sous verrou, puis le verrou est rendu : résoudre des symboles
    // prend des dizaines de millisecondes, et les autres fils allouent pendant ce
    // temps.
    let (mut classement, suivis) = {
        let sites = sites();
        let mut v: Vec<(Vec<usize>, usize, usize, usize)> = sites
            .sites
            .iter()
            .filter(|s| s.live > 0)
            .map(|s| (s.frames.clone(), s.live, s.live_blocks, s.peak))
            .collect();
        v.sort_by_key(|(_, live, _, _)| std::cmp::Reverse(*live));
        let suivis: usize = sites.sites.iter().map(|s| s.live).sum();
        (v, suivis)
    };
    classement.truncate(20);

    let mut s = String::new();
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "=== Gros blocs vivants, par site (≥ {} Ko) ===",
        SITE_THRESHOLD / 1024
    );
    let _ = writeln!(s, "  attribué               {}", mo(suivis as u64));
    let _ = writeln!(
        s,
        "  reste (petits blocs)   {}",
        mo(live.saturating_sub(suivis) as u64)
    );

    for (frames, vivant, blocs, pic) in classement {
        let _ = writeln!(s);
        let _ = writeln!(
            s,
            "  {}  en {blocs} bloc(s), pic {}",
            mo(vivant as u64),
            mo(pic as u64)
        );
        for ligne in symbolise(&frames) {
            let _ = writeln!(s, "      {ligne}");
        }
    }

    s
}

/// Traduit une pile en noms, en écartant notre propre plomberie.
fn symbolise(frames: &[usize]) -> Vec<String> {
    let mut lignes = Vec::new();

    for ip in frames {
        backtrace::resolve(*ip as *mut core::ffi::c_void, |symbole| {
            let nom = symbole
                .name()
                .map(|n| n.to_string())
                .unwrap_or_else(|| format!("{ip:#x}"));

            // L'allocateur, le pisteur et les enveloppes de la bibliothèque standard
            // sont sur toutes les piles et n'en distinguent aucune.
            let bruit = nom.contains("iris_app::memory")
                || nom.contains("alloc::alloc")
                || nom.contains("alloc::raw_vec")
                || nom.contains("GlobalAlloc")
                || nom.contains("__rust_alloc")
                || nom.contains("realloc");
            if bruit || lignes.len() >= 8 {
                return;
            }

            match (symbole.filename(), symbole.lineno()) {
                (Some(f), Some(l)) => {
                    let court = f
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    lignes.push(format!("{nom}  ({court}:{l})"));
                }
                _ => lignes.push(nom),
            }
        });
    }

    if lignes.is_empty() {
        lignes.push("(pile non résolue)".into());
    }
    lignes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un bloc franc, très au-dessus du bruit.
    ///
    /// Les compteurs appartiennent au processus, et le harnais de Rust fait tourner
    /// les autres tests en même temps — dont ceux qui ouvrent une base et un index,
    /// c'est-à-dire qui prennent et rendent des mégaoctets pendant que celui-ci
    /// mesure. Un bloc de quatre mégaoctets s'y noyait : le test a échoué une fois
    /// sur cette seule raison. Soixante-quatre mégaoctets, et des affirmations qui
    /// tiennent quoi que fassent les voisins.
    const BLOC: usize = 64 * 1024 * 1024;

    #[test]
    fn une_allocation_est_comptee() {
        let avant = TOTAL_BYTES.load(Ordering::Relaxed);
        let bloc = vec![0u8; BLOC];

        // Le total ne fait que croître : ce qu'un autre fil alloue pendant ce temps
        // ne peut que rendre l'écart plus grand, jamais le masquer.
        let alloue = TOTAL_BYTES.load(Ordering::Relaxed) - avant;
        assert!(
            alloue >= BLOC as u64,
            "le compteur a vu {alloue} octets pour un bloc de {BLOC}"
        );
        // Et le bloc est vivant : le vivant ne peut pas être en dessous de lui.
        assert!(LIVE.load(Ordering::Relaxed) >= BLOC);

        drop(bloc);
    }

    #[test]
    fn une_liberation_est_comptee() {
        let bloc = vec![0u8; BLOC];
        let pendant = LIVE.load(Ordering::Relaxed);
        drop(bloc);
        let apres = LIVE.load(Ordering::Relaxed);

        // Il faudrait qu'un autre fil retienne trente-deux mégaoctets pendant les
        // quelques microsecondes de cette libération pour masquer le résultat.
        assert!(
            apres + BLOC / 2 < pendant,
            "la libération doit se voir : {pendant} puis {apres}"
        );
    }

    #[test]
    // Windows seulement : ailleurs, la lecture n'est pas écrite et rend zéro, ce
    // que le module dit lui-même. Le tester sur macOS vérifiait une promesse que
    // personne n'a faite.
    #[cfg(windows)]
    fn l_espace_d_adressage_se_lit() {
        let r = regions();
        // Le binaire lui-même est chargé : un total nul voudrait dire que la lecture a
        // échoué silencieusement.
        assert!(r.total() > 0, "aucune région engagée");
        if cfg!(windows) {
            assert!(r.image > 0, "le binaire est pourtant en mémoire");
        }
    }

    #[test]
    fn un_rapport_se_produit_sans_suivi_detaille() {
        let texte = report();
        assert!(texte.contains("=== Tas ==="));
        assert!(texte.contains("ensemble résident"));
    }

    #[test]
    fn les_jalons_apparaissent_dans_l_ordre() {
        mark("premier jalon");
        mark("second jalon");
        let texte = report();
        let un = texte.find("premier jalon").expect("le premier jalon");
        let deux = texte.find("second jalon").expect("le second jalon");
        assert!(un < deux);
    }
}
