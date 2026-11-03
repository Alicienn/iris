//! The unread count on Iris's taskbar button.
//!
//! The same figure as the notification area's tooltip — unread mail, junk and the bin
//! left out — drawn over the button's corner (`ITaskbarList3::SetOverlayIcon`), with
//! nothing when there is nothing to read. Tasks have their own count on the rail and
//! stay off it: a button that also counts what is due tomorrow stops saying whether
//! mail came.
//!
//! It looks like the badge Windows draws for the apps of its Store (7.6.0): a neutral
//! disc, light grey with dark figures on a dark taskbar and the reverse on a light one,
//! the figures in Segoe UI. Those badges need an app with a package identity; Iris
//! installs without one, so it draws its own. The figures are laid out by GDI at four
//! times the size and the disc is smoothed from those samples. It is drawn again only
//! when the count or the taskbar's mode changes, and when Explorer makes the button
//! again (it restarted, the window came back from the notification area): Windows then
//! sends `TaskbarButtonCreated`, and an overlay set before is gone.
//!
//! Elsewhere than Windows there is no such button and nothing is done.

/// A colour as three channels.
pub type Rgb = (u8, u8, u8);

/// What the badge reads: nothing for none, the number up to 99, then "99+" — a
/// fourth character would not be read at that size.
pub fn label(unread: u32) -> Option<String> {
    match unread {
        0 => None,
        1..=99 => Some(unread.to_string()),
        _ => Some("99+".into()),
    }
}

/// What a screen reader says of it.
pub fn description(unread: u32) -> String {
    format!("{unread} unread")
}

/// The disc and its figures, as Windows colours its own badges: light grey and nearly
/// black on a dark taskbar, dark grey and white on a light one.
pub fn colours(taskbar_dark: bool) -> (Rgb, Rgb) {
    if taskbar_dark {
        ((0xa6, 0xa6, 0xa6), (0x0b, 0x0b, 0x0b))
    } else {
        ((0x5c, 0x5c, 0x5c), (0xff, 0xff, 0xff))
    }
}

/// How many samples a pixel has on a side: the figures are laid out at four times
/// the badge's size, and each pixel is the mean of its sixteen samples.
pub const SAMPLES: usize = 4;

/// The figures and the plus, three units wide and five high, a row per byte (the
/// high bit on the left): where GDI is not there to draw them (the tests, another
/// system), or refused.
fn glyphe(c: char) -> [u8; 5] {
    match c {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b001, 0b001, 0b001],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        _ => [0; 5],
    }
}

/// The figures of `texte` as blocks, sample by sample: `SAMPLES` × `taille` a side,
/// a row after another, true where the figures are.
pub fn block_figures(texte: &str, taille: usize) -> Vec<bool> {
    let glyphes: Vec<[u8; 5]> = texte.chars().map(glyphe).collect();
    let d = taille as f32;
    // In units of the glyphs: three wide, one between them.
    let largeur = (4 * glyphes.len().max(1) - 1) as f32;
    // Half the disc's height for one or two figures; narrower for three, so they
    // keep clear of its edge.
    let echelle = (d * 0.105).min(d * 0.78 / largeur);
    let x0 = (d - largeur * echelle) / 2.0;
    let y0 = (d - 5.0 * echelle) / 2.0;
    let n = taille * SAMPLES;
    let mut cases = vec![false; n * n];
    for sy in 0..n {
        for sx in 0..n {
            let fx = (sx as f32 + 0.5) / SAMPLES as f32;
            let fy = (sy as f32 + 0.5) / SAMPLES as f32;
            let u = (fx - x0) / echelle;
            let v = (fy - y0) / echelle;
            if u < 0.0 || !(0.0..5.0).contains(&v) {
                continue;
            }
            let (colonne, rangee) = (u as usize, v as usize);
            let (i, c) = (colonne / 4, colonne % 4);
            cases[sy * n + sx] =
                c < 3 && i < glyphes.len() && ((glyphes[i][rangee] >> (2 - c)) & 1) == 1;
        }
    }
    cases
}

/// `cases` (`n` samples a side) moved so that what is drawn sits in the middle: a
/// font's line box puts the figures low, and a "1" leans left of its advance, where
/// Windows' badges centre the ink itself.
pub fn centre_ink(cases: &[bool], n: usize) -> Vec<bool> {
    let (mut x0, mut x1, mut y0, mut y1) = (n, 0, n, 0);
    for (i, _) in cases.iter().enumerate().filter(|(_, &c)| c) {
        let (x, y) = (i % n, i / n);
        x0 = x0.min(x);
        x1 = x1.max(x);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    if x0 > x1 {
        return cases.to_vec();
    }
    let dx = ((n - (x1 - x0 + 1)) / 2) as isize - x0 as isize;
    let dy = ((n - (y1 - y0 + 1)) / 2) as isize - y0 as isize;
    let mut sortie = vec![false; n * n];
    for (i, _) in cases.iter().enumerate().filter(|(_, &c)| c) {
        let x = (i % n) as isize + dx;
        let y = (i / n) as isize + dy;
        if (0..n as isize).contains(&x) && (0..n as isize).contains(&y) {
            sortie[y as usize * n + x as usize] = true;
        }
    }
    sortie
}

/// The badge with block figures: what `badge` draws where GDI does not.
pub fn pixels(texte: &str, taille: usize, fond: Rgb, encre: Rgb) -> Vec<[u8; 4]> {
    badge(&block_figures(texte, taille), taille, fond, encre)
}

/// The badge: a disc in `fond`, `taille` pixels a side, with `figures` (samples, as
/// `block_figures` gives them) in `encre` across it. Top row first, each pixel blue,
/// green, red, alpha with the alpha straight, as an icon holds them.
///
/// Each pixel is the mean of its samples, so the disc's edge and the figures stay
/// smooth at any size the taskbar asks for.
pub fn badge(figures: &[bool], taille: usize, fond: Rgb, encre: Rgb) -> Vec<[u8; 4]> {
    let s = SAMPLES;
    let n = taille * s;
    let r = taille as f32 / 2.0;
    let mut sortie = Vec::with_capacity(taille * taille);
    for y in 0..taille {
        for x in 0..taille {
            let (mut disque, mut encre_n) = (0u32, 0u32);
            for sy in 0..s {
                for sx in 0..s {
                    let fx = x as f32 + (sx as f32 + 0.5) / s as f32;
                    let fy = y as f32 + (sy as f32 + 0.5) / s as f32;
                    let (dx, dy) = (fx - r, fy - r);
                    if dx * dx + dy * dy > r * r {
                        continue;
                    }
                    disque += 1;
                    if figures
                        .get((y * s + sy) * n + x * s + sx)
                        .copied()
                        .unwrap_or(false)
                    {
                        encre_n += 1;
                    }
                }
            }
            if disque == 0 {
                sortie.push([0, 0, 0, 0]);
                continue;
            }
            let melange = |a: u8, b: u8| {
                ((u32::from(a) * (disque - encre_n) + u32::from(b) * encre_n) / disque) as u8
            };
            sortie.push([
                melange(fond.2, encre.2),
                melange(fond.1, encre.1),
                melange(fond.0, encre.0),
                (disque * 255 / (s * s) as u32) as u8,
            ]);
        }
    }
    sortie
}

#[cfg(windows)]
pub use plateforme::draw;
pub use plateforme::{refresh, show_unread};

#[cfg(windows)]
#[allow(unsafe_code)]
mod plateforme {
    use super::Rgb;
    use iris_ui::AppWindow;
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::winit_030::WinitWindowAccessor;
    use slint::ComponentHandle;
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use windows::core::{Interface, HSTRING};
    use windows::Win32::UI::Shell::ITaskbarList3;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS,
    };
    use windows_sys::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateIconIndirect, DestroyIcon, GetSystemMetrics, RegisterWindowMessageW, HICON, ICONINFO,
        SM_CXSMICON, WM_NCDESTROY,
    };

    /// The overlay must be set again: Explorer made the button anew, or the window
    /// came back. Set from the window's procedure, read by the next update.
    static A_REFAIRE: AtomicBool = AtomicBool::new(true);
    /// `TaskbarButtonCreated`, as this session numbers it.
    static BOUTON_CREE: AtomicU32 = AtomicU32::new(0);
    /// Our subclass of the window, told apart from anyone else's.
    const SOUS_CLASSE: usize = 0x1215_0001;

    struct Etat {
        /// The window the overlay was set on; 0 for none yet.
        hwnd: isize,
        /// What the button shows: the count and the two colours it was drawn in.
        pose: Option<(u32, Rgb, Rgb)>,
        /// The taskbar's interface, made once.
        barre: Option<ITaskbarList3>,
    }

    thread_local! {
        // The interface thread's: the window, its procedure and COM's apartment all
        // live there.
        static ETAT: RefCell<Etat> = const {
            RefCell::new(Etat {
                hwnd: 0,
                pose: None,
                barre: None,
            })
        };
    }

    pub fn refresh() {
        A_REFAIRE.store(true, Ordering::Relaxed);
    }

    /// Shows `unread` on the button. Called often (the tray's timer): it does nothing
    /// unless what the button shows has to change.
    pub fn show_unread(f: &AppWindow, unread: u32) {
        // Before the system window exists (started in the notification area) there
        // is no button.
        let Some(hwnd) = poignee(f) else {
            return;
        };
        let (fond, encre) = super::colours(crate::platform::taskbar_dark());
        let voulu = (unread, fond, encre);
        ETAT.with(|etat| {
            let Ok(mut etat) = etat.try_borrow_mut() else {
                return;
            };
            if etat.hwnd != hwnd {
                etat.hwnd = hwnd;
                etat.pose = None;
                sous_classer(hwnd);
                donner_l_icone(hwnd);
            }
            if A_REFAIRE.swap(false, Ordering::Relaxed) {
                etat.pose = None;
            }
            // Hidden in the notification area, the window has no button to draw on;
            // the overlay is set when it comes back.
            if etat.pose == Some(voulu) || !f.window().is_visible() {
                return;
            }
            if etat.barre.is_none() {
                etat.barre = barre_des_taches();
            }
            if let Some(barre) = &etat.barre {
                poser(barre, hwnd, voulu);
            }
            // Noted even when refused (the button not made yet): it is tried again
            // when Explorer says the button exists, not five times a second.
            etat.pose = Some(voulu);
        });
    }

    /// Gives the window the executable's icon. winit's window class has none, and
    /// Alt+Tab draws a window's own icon, not its program's: Iris showed a blank page
    /// there while the taskbar, which reads the executable, showed the right one.
    fn donner_l_icone(hwnd: isize) {
        use windows_sys::Win32::UI::Shell::ExtractIconExW;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SendMessageW, ICON_BIG, ICON_SMALL, WM_SETICON,
        };
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let chemin: Vec<u16> = exe
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let (mut grande, mut petite): (HICON, HICON) = (std::ptr::null_mut(), std::ptr::null_mut());
        // SAFETY: a null-terminated path and two out-pointers to locals. The icons are
        // kept for the window's life: WM_SETICON does not take a copy.
        let n = unsafe { ExtractIconExW(chemin.as_ptr(), 0, &mut grande, &mut petite, 1) };
        if n == 0 || n == u32::MAX {
            tracing::debug!("the executable gave no icon");
            return;
        }
        // SAFETY: a live window and icons we own.
        unsafe {
            if !grande.is_null() {
                SendMessageW(
                    hwnd as HWND,
                    WM_SETICON,
                    ICON_BIG as WPARAM,
                    grande as LPARAM,
                );
            }
            if !petite.is_null() {
                SendMessageW(
                    hwnd as HWND,
                    WM_SETICON,
                    ICON_SMALL as WPARAM,
                    petite as LPARAM,
                );
            }
        }
    }

    /// The window's handle, once the system window exists.
    fn poignee(f: &AppWindow) -> Option<isize> {
        f.window()
            .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
                RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
                _ => None,
            })
            .flatten()
    }

    /// The taskbar's interface, on this thread's apartment.
    fn barre_des_taches() -> Option<ITaskbarList3> {
        const CLSID_TASKBAR_LIST: windows_sys::core::GUID =
            windows_sys::core::GUID::from_u128(0x56fdf344_fd6d_11d0_958a_006097c9a090);
        let iid = windows_sys::core::GUID::from_u128(ITaskbarList3::IID.to_u128());
        let mut p: *mut c_void = std::ptr::null_mut();
        // SAFETY: COM calls with valid pointers to locals. Joining the apartment winit
        // already made is harmless (`S_FALSE`); a thread in another one refuses, and
        // the creation then fails and is reported as no taskbar.
        let hr = unsafe {
            CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
            CoCreateInstance(
                &CLSID_TASKBAR_LIST,
                std::ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                &iid,
                &mut p,
            )
        };
        if hr < 0 || p.is_null() {
            tracing::debug!(hr, "no taskbar interface");
            return None;
        }
        // SAFETY: `p` is an `ITaskbarList3` CoCreateInstance gave us, its reference
        // now owned by the wrapper.
        let barre = unsafe { ITaskbarList3::from_raw(p) };
        // SAFETY: a method of a live interface.
        if let Err(e) = unsafe { barre.HrInit() } {
            tracing::debug!(error = %e, "the taskbar refused");
            return None;
        }
        Some(barre)
    }

    /// Sets (or removes) the overlay.
    fn poser(barre: &ITaskbarList3, hwnd: isize, (n, fond, encre): (u32, Rgb, Rgb)) {
        let fenetre = windows::Win32::Foundation::HWND(hwnd as *mut c_void);
        let Some(texte) = super::label(n) else {
            // SAFETY: a live window; a null icon removes the overlay.
            let _ = unsafe {
                barre.SetOverlayIcon(
                    fenetre,
                    windows::Win32::UI::WindowsAndMessaging::HICON(std::ptr::null_mut()),
                    &HSTRING::new(),
                )
            };
            return;
        };
        // The small icon's size, at the window's scale: the overlay is drawn at it.
        // SAFETY: no arguments to get wrong.
        let taille = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64) as usize;
        let Some(icone) = icone(&draw(&texte, taille, fond, encre), taille) else {
            return;
        };
        let description = HSTRING::from(super::description(n));
        // SAFETY: a live window and an icon we own.
        let r = unsafe {
            barre.SetOverlayIcon(
                fenetre,
                windows::Win32::UI::WindowsAndMessaging::HICON(icone),
                &description,
            )
        };
        // The taskbar keeps a copy of its own: ours goes now, and none is left
        // behind at each change.
        // SAFETY: the icon made above, used by nothing else.
        unsafe { DestroyIcon(icone) };
        if let Err(e) = r {
            tracing::debug!(error = %e, "the taskbar's overlay");
        }
    }

    /// The badge as the button shows it, `taille` pixels a side (BGRA, straight
    /// alpha): the figures in Segoe UI, in blocks if GDI cannot draw them.
    pub fn draw(texte: &str, taille: usize, fond: Rgb, encre: Rgb) -> Vec<[u8; 4]> {
        let figures =
            chiffres(texte, taille).unwrap_or_else(|| super::block_figures(texte, taille));
        super::badge(&figures, taille, fond, encre)
    }

    /// The figures of `texte` in Segoe UI, as `block_figures` gives its blocks: laid
    /// out by GDI white on black in a bitmap `SAMPLES` times the badge's size, without
    /// smoothing (the samples do it), then read back and centred on their ink. Regular
    /// weight in Segoe UI Variable Text, the figures half the disc high, as Windows'
    /// badges (measured beside WhatsApp's, 7.8.1); narrowed when three characters
    /// would touch the disc's edge.
    fn chiffres(texte: &str, taille: usize) -> Option<Vec<bool>> {
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::Graphics::Gdi::{
            CreateCompatibleDC, CreateFontW, DeleteDC, DrawTextW, GdiFlush, SelectObject,
            SetBkMode, SetTextColor, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH,
            DT_CALCRECT, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_NORMAL,
            NONANTIALIASED_QUALITY, OUT_TT_PRECIS, TRANSPARENT,
        };

        let n = taille * super::SAMPLES;
        let mots: Vec<u16> = texte.encode_utf16().collect();
        let police: Vec<u16> = "Segoe UI Variable Text"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let entete = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: n as i32,
            biHeight: -(n as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        };
        let info = BITMAPINFO {
            bmiHeader: entete,
            ..Default::default()
        };
        // SAFETY: GDI calls on a memory DC and a bitmap made here, every object
        // selected out and deleted before returning; `bits` holds n² four-byte pixels
        // (CreateDIBSection zeroes them: black) and is read only after GdiFlush.
        unsafe {
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            if dc.is_null() {
                return None;
            }
            let mut bits: *mut c_void = std::ptr::null_mut();
            let image = CreateDIBSection(
                dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            if image.is_null() || bits.is_null() {
                DeleteDC(dc);
                return None;
            }
            let ancienne_image = SelectObject(dc, image);
            // The em: figures about half the disc high, as Windows' own (a figure is
            // seven tenths of the em).
            let mut em = (n as f32 * 0.74) as i32;
            let mut police_choisie: *mut c_void = std::ptr::null_mut();
            let mut ancienne_police: *mut c_void = std::ptr::null_mut();
            for _ in 0..2 {
                let p = CreateFontW(
                    -em,
                    0,
                    0,
                    0,
                    FW_NORMAL as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET as u32,
                    OUT_TT_PRECIS as u32,
                    CLIP_DEFAULT_PRECIS as u32,
                    NONANTIALIASED_QUALITY as u32,
                    DEFAULT_PITCH as u32,
                    police.as_ptr(),
                );
                if p.is_null() {
                    break;
                }
                let avant = SelectObject(dc, p);
                if ancienne_police.is_null() {
                    ancienne_police = avant;
                }
                if !police_choisie.is_null() {
                    DeleteObject(police_choisie);
                }
                police_choisie = p;
                let mut mesure = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                DrawTextW(
                    dc,
                    mots.as_ptr(),
                    mots.len() as i32,
                    &mut mesure,
                    DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
                );
                // Wider than eight tenths of the disc: smaller, once.
                let large = (n as f32 * 0.8) as i32;
                if mesure.right <= large || mesure.right == 0 {
                    break;
                }
                em = em * large / mesure.right;
            }
            if police_choisie.is_null() {
                SelectObject(dc, ancienne_image);
                DeleteObject(image);
                DeleteDC(dc);
                return None;
            }
            SetTextColor(dc, 0x00ff_ffff);
            SetBkMode(dc, TRANSPARENT as i32);
            let mut cadre = RECT {
                left: 0,
                top: 0,
                right: n as i32,
                bottom: n as i32,
            };
            DrawTextW(
                dc,
                mots.as_ptr(),
                mots.len() as i32,
                &mut cadre,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            GdiFlush();
            let octets = std::slice::from_raw_parts(bits.cast::<u8>(), n * n * 4);
            let cases: Vec<bool> = octets.chunks_exact(4).map(|p| p[1] > 0x7f).collect();
            SelectObject(dc, ancienne_police);
            DeleteObject(police_choisie);
            SelectObject(dc, ancienne_image);
            DeleteObject(image);
            DeleteDC(dc);
            // Nothing drawn (no such font, nothing to show): the blocks instead.
            cases
                .iter()
                .any(|&c| c)
                .then(|| super::centre_ink(&cases, n))
        }
    }

    /// An icon of `taille` pixels a side from `pixels` (BGRA, straight alpha, top
    /// row first). The bitmaps it is made from are deleted before returning.
    fn icone(pixels: &[[u8; 4]], taille: usize) -> Option<HICON> {
        if pixels.len() != taille * taille {
            return None;
        }
        let entete = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: taille as i32,
            // Negative: the rows go top to bottom, as `pixels` holds them.
            biHeight: -(taille as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        };
        let info = BITMAPINFO {
            bmiHeader: entete,
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: `info` describes a 32-bit bitmap of `taille`², and the copy below
        // writes exactly that many pixels into the memory the call gives back.
        let couleur = unsafe {
            CreateDIBSection(
                std::ptr::null_mut(),
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            )
        };
        if couleur.is_null() || bits.is_null() {
            if !couleur.is_null() {
                // SAFETY: the bitmap made above.
                unsafe { DeleteObject(couleur) };
            }
            return None;
        }
        // SAFETY: see above; `pixels` holds `taille`² four-byte pixels.
        unsafe {
            std::ptr::copy_nonoverlapping(
                pixels.as_ptr().cast::<u8>(),
                bits.cast::<u8>(),
                taille * taille * 4,
            );
        }
        // The mask is all transparent-to-nothing: with an alpha channel in the colour
        // bitmap, Windows reads that instead. Monochrome rows are padded to 16 bits.
        let zeros = vec![0u8; taille.div_ceil(16) * 2 * taille];
        // SAFETY: `zeros` holds the rows the call reads.
        let masque =
            unsafe { CreateBitmap(taille as i32, taille as i32, 1, 1, zeros.as_ptr().cast()) };
        let info_icone = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: masque,
            hbmColor: couleur,
        };
        // SAFETY: two bitmaps we own; the icon copies them, so they go right after.
        let icone = unsafe { CreateIconIndirect(&info_icone) };
        unsafe {
            DeleteObject(couleur);
            if !masque.is_null() {
                DeleteObject(masque);
            }
        }
        (!icone.is_null()).then_some(icone)
    }

    /// Listens for `TaskbarButtonCreated` on the window.
    fn sous_classer(hwnd: isize) {
        if BOUTON_CREE.load(Ordering::Relaxed) == 0 {
            let nom: Vec<u16> = "TaskbarButtonCreated"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            // SAFETY: a string terminated by a zero, alive for the call.
            let id = unsafe { RegisterWindowMessageW(nom.as_ptr()) };
            BOUTON_CREE.store(id, Ordering::Relaxed);
        }
        // SAFETY: a live window of this thread; the same subclass set twice is only
        // updated, and it removes itself when the window goes.
        let ok = unsafe { SetWindowSubclass(hwnd as HWND, Some(procedure), SOUS_CLASSE, 0) };
        if ok == 0 {
            tracing::debug!(
                "no subclass on the window: the overlay is not set again after Explorer restarts"
            );
        }
    }

    unsafe extern "system" fn procedure(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _donnees: usize,
    ) -> LRESULT {
        let cree = BOUTON_CREE.load(Ordering::Relaxed);
        if cree != 0 && message == cree {
            // Not set from here: the window's procedure may run in the middle of a
            // call to the taskbar. The next update does it.
            A_REFAIRE.store(true, Ordering::Relaxed);
        }
        if message == WM_NCDESTROY {
            // SAFETY: our own subclass of this window, which is going.
            unsafe { RemoveWindowSubclass(hwnd, Some(procedure), SOUS_CLASSE) };
        }
        // SAFETY: the arguments as the window received them.
        unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
    }
}

#[cfg(not(windows))]
mod plateforme {
    pub fn refresh() {}

    pub fn show_unread(_f: &iris_ui::AppWindow, _unread: u32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_unread_shows_nothing_and_beyond_99_shows_99_plus() {
        assert_eq!(label(0), None);
        assert_eq!(label(7).as_deref(), Some("7"));
        assert_eq!(label(99).as_deref(), Some("99"));
        assert_eq!(label(100).as_deref(), Some("99+"));
        assert_eq!(description(3), "3 unread");
    }

    #[test]
    fn the_badge_is_a_disc_with_the_figures_across_it() {
        let fond = (0x00, 0x7a, 0xff);
        let encre = (0xff, 0xff, 0xff);
        for (texte, taille) in [("7", 16), ("42", 24), ("99+", 32)] {
            let px = pixels(texte, taille, fond, encre);
            assert_eq!(px.len(), taille * taille);
            // The corners are outside the disc, its middle row inside.
            assert_eq!(px[0][3], 0, "{texte}: a corner is transparent");
            assert_eq!(px[taille * taille - 1][3], 0);
            assert!(px[taille * (taille / 2)][3] > 0);
            assert_eq!(px[taille * (taille / 2) + taille / 2][3], 255);
            // Some pixels are the figures', some the disc's.
            let blancs = px
                .iter()
                .filter(|p| p[0] > 0xf0 && p[1] > 0xf0 && p[3] == 255);
            let bleus = px
                .iter()
                .filter(|p| p[0] == 0xff && p[2] == 0 && p[3] == 255);
            assert!(blancs.count() > 4, "{texte}: the figures are drawn");
            assert!(bleus.count() > taille, "{texte}: the disc is drawn");
        }
    }

    #[test]
    fn the_badge_takes_the_taskbar_s_contrast_as_windows_own_do() {
        let (fond, encre) = colours(true);
        assert!(fond.0 > 0x80 && encre.0 < 0x20, "light on a dark taskbar");
        let (fond, encre) = colours(false);
        assert!(fond.0 < 0x80 && encre.0 == 0xff, "dark on a light one");
    }

    #[test]
    fn the_figures_are_centred_on_their_ink() {
        let n = 8;
        // A bar two wide, three high, in the bottom-left corner.
        let mut cases = vec![false; n * n];
        for y in 5..8 {
            for x in 0..2 {
                cases[y * n + x] = true;
            }
        }
        let c = centre_ink(&cases, n);
        let encre: Vec<(usize, usize)> = (0..n * n)
            .filter(|&i| c[i])
            .map(|i| (i % n, i / n))
            .collect();
        assert_eq!(encre.len(), 6);
        assert_eq!(encre.iter().map(|p| p.0).min(), Some(3));
        assert_eq!(encre.iter().map(|p| p.1).min(), Some(2));
        assert_eq!(centre_ink(&[false; 4], 2), vec![false; 4]);
    }

    #[test]
    fn a_wider_label_is_drawn_smaller_so_it_stays_inside_the_disc() {
        let taille = 32;
        let encre = |texte: &str| {
            let px = pixels(texte, taille, (0, 0, 0), (255, 255, 255));
            // The columns holding ink at all.
            (0..taille)
                .filter(|x| (0..taille).any(|y| px[y * taille + x][2] > 0x80))
                .collect::<Vec<_>>()
        };
        let trois = encre("99+");
        assert!(*trois.first().unwrap() > 0 && *trois.last().unwrap() < taille - 1);
    }
}
