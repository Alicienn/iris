//! Sound: generative soundscapes to work, rest or sleep to.
//!
//! Seven themes, one per goal (focus, deep work, blocking noise, relaxing, a break, falling
//! asleep, ASMR), each with a few sounds made by `iris-sound` as they play. A pad tunes the
//! sound on two axes; each theme remembers its own sound, pad and options.
//!
//! - **The engine** (`iris-sound`) fills buffers; here a `cpal` stream on its own thread
//!   (`iris-sound`) plays them. The thread, the stream and the engine exist only while
//!   sound plays: paused, nothing runs and nothing is held.
//! - **The session clock** (a timer on the interface's thread, twice a second) fades a
//!   session in and out, rings a break's bell, darkens sleep as it goes, reminds deep
//!   work of a break, and follows the time of day.
//! - **The settings** are this device's (`Settings::sound`), saved a second after the
//!   last change rather than at every move of the pad.
//!
//! The assistant reaches the same state through `mcp::sound` (never around it).

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// What is written in `iris.toml`. Levels and positions are per mille (the settings are
/// `Eq`: no floats).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundSettings {
    pub theme: String,
    pub volume: u32,
    pub themes: BTreeMap<String, ThemeSettings>,
}

impl Default for SoundSettings {
    fn default() -> Self {
        Self {
            theme: "focus".into(),
            volume: 600,
            themes: BTreeMap::new(),
        }
    }
}

/// One theme's own choices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ThemeSettings {
    /// The engine's sound id.
    pub sound: u8,
    pub x: u32,
    pub y: u32,
    /// Switches (0 / 1), minutes, the ASMR families' mask.
    pub options: BTreeMap<String, u32>,
}

/// A theme: a goal, its sounds, how the engine is held for it, the words on its pad.
pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub icon: &'static str,
    pub line: &'static str,
    pub sounds: &'static [u8],
    /// Ceiling on intensity, brightness, density of events, beat swell.
    profile: [f32; 4],
    pad: (u32, u32),
    /// Left, right, bottom, top.
    axes: [&'static str; 4],
    /// Options and their defaults.
    options: &'static [(&'static str, u32)],
    /// Lengths offered (minutes, 0 = off) and the fade at the end (seconds).
    session: Option<(&'static str, &'static [u32], f32)>,
    /// The two colours of the light that goes round the pad on a change.
    hues: (u32, u32),
}

const DEFAULT_MASK: u32 = iris_sound::ASMR_DEFAULT_MASK;
/// Deep work's break reminder: off, every 50 or every 90 minutes.
const REMIND: [u32; 3] = [0, 50, 90];
const SILENCE: u8 = 13;

pub const THEMES: [Theme; 7] = [
    Theme {
        id: "focus",
        name: "Focus",
        icon: "M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 0 0 0-17zM12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8z",
        line: "Everyday work: steady, wordless, never in the way",
        sounds: &[0, 1, 2, 3],
        profile: [1.0, 1.0, 1.0, 1.0],
        pad: (500, 450),
        axes: ["Fluid", "Structured", "Easy", "Intense"],
        options: &[("boost", 0), ("day", 1)],
        session: None,
        hues: (0x5b8cff, 0x9a7bff),
    },
    Theme {
        id: "deep",
        name: "Deep work",
        icon: "M12 3.5 21 8l-9 4.5L3 8zM3 12l9 4.5 9-4.5M3 16l9 4.5 9-4.5",
        line: "Reading, writing, hard problems: almost nothing happens",
        sounds: &[5, 6, 4, SILENCE],
        profile: [0.55, 0.85, 0.15, 0.5],
        pad: (350, 250),
        axes: ["Drifting", "Steady", "Sparse", "Fuller"],
        options: &[("remind", 0)],
        session: None,
        hues: (0x6c5fd6, 0x3f8fd9),
    },
    Theme {
        id: "mask",
        name: "Block noise",
        icon: "M12 3.5 19.5 6v6c0 4.4-3.2 7.6-7.5 8.5C7.7 19.6 4.5 16.4 4.5 12V6z",
        line: "Open office, café, neighbours: cover the voices around you",
        sounds: &[4, 6, 3, 7],
        profile: [1.0, 1.0, 1.0, 1.0],
        pad: (850, 500),
        axes: ["Waves", "Steady", "Warm", "Bright"],
        options: &[],
        session: None,
        hues: (0x2fa39a, 0x4a90d9),
    },
    Theme {
        id: "relax",
        name: "Relax",
        icon: "M5 19c0-8 5-14 15-14 0 10-6 15-14 15M5 19l8-8",
        line: "Unwind: slower, softer, ending in silence",
        sounds: &[0, 2, 7, 6],
        profile: [0.6, 0.8, 0.6, 0.3],
        pad: (350, 350),
        axes: ["Fluid", "Structured", "Calm", "Fuller"],
        options: &[("breath", 1), ("session", 20), ("day", 1)],
        session: Some(("Length", &[10, 20, 30], 120.0)),
        hues: (0x3fae7a, 0x9cc84a),
    },
    Theme {
        id: "break",
        name: "Break",
        icon: "M5 9h11v5a5 5 0 0 1-5 5h-1a5 5 0 0 1-5-5zM16 10.5h1.5a2.5 2.5 0 0 1 0 5H16M8 3.5v2.5M11.5 3.5v2.5",
        line: "A few minutes away from the task, then a soft bell",
        sounds: &[8, 7, 6],
        profile: [0.8, 1.0, 1.0, 1.0],
        pad: (400, 500),
        axes: ["Wild", "Regular", "Quiet", "Lively"],
        options: &[("session", 10), ("bell", 1)],
        session: Some(("Length", &[5, 10, 15], 90.0)),
        hues: (0xe0a23c, 0xe36f4f),
    },
    Theme {
        id: "sleep",
        name: "Sleep",
        icon: "M20 14.5A8.5 8.5 0 1 1 9.5 4a7 7 0 0 0 10.5 10.5z",
        line: "Falling asleep: the lowest energy, darker as it plays, then off",
        sounds: &[5, 3, 7, 4],
        profile: [0.35, 0.6, 0.3, 0.0],
        pad: (300, 400),
        axes: ["Drifting", "Still", "Deeper", "Lighter"],
        options: &[("session", 45), ("breath", 0), ("darken", 1)],
        session: Some(("Stop after", &[15, 30, 45, 60, 0], 600.0)),
        hues: (0x7a63d6, 0x3c4fb0),
    },
    Theme {
        id: "asmr",
        name: "ASMR",
        icon: "M12 20.5s-7.5-4.6-7.5-10A4.5 4.5 0 0 1 12 7.5a4.5 4.5 0 0 1 7.5 3c0 5.4-7.5 10-7.5 10z",
        line: "Close, quiet sounds that tingle. Best with headphones",
        sounds: &[9],
        profile: [1.0, 1.0, 1.0, 1.0],
        pad: (300, 400),
        axes: ["Still", "Around you", "Sparse", "Dense"],
        options: &[("session", 0), ("mask", DEFAULT_MASK)],
        session: Some(("Stop after", &[0, 20, 40], 120.0)),
        hues: (0xe078a8, 0xb07ae0),
    },
];

/// The engine's sounds: name and icon, by id.
pub const SOUNDS: [(&str, &str); 14] = [
    ("Drift", "M3 13c2.5-4 5.5-4 9 0s6.5 4 9 0"),
    ("Pulse", "M5 10v4M9 7v10M13 9v6M17 5v14M21 10v4"),
    ("Keys", "M4 5.5h16v13H4zM9 5.5v8M15 5.5v8M12 13.5v5"),
    ("Rain", "M8 4.5s-3 4-3 6.2a3 3 0 0 0 6 0C11 8.5 8 4.5 8 4.5zM16.5 10s-2.5 3.3-2.5 5.1a2.5 2.5 0 0 0 5 0c0-1.8-2.5-5.1-2.5-5.1z"),
    ("Noise", "M3 12h1.5l1.5-4 2 8 2-10 2 12 2-9 2 6 1.5-4 1.5 3H21"),
    ("Drone", "M3 9.5h18M3 14.5h18"),
    ("Stream", "M3 9c3-2 6 2 9 0s6-2 9 0M3 15c3-2 6 2 9 0s6-2 9 0"),
    ("Shore", "M3 15c2-3 4.5-5 8-5 2.5 0 4 1.5 4 3.2 0 1.2-1 1.8-2 1.8M3 19h18"),
    ("Forest", "M12 3.5 6.5 12h3L6 17.5h12L14.5 12h3zM12 17.5v3"),
    ("ASMR", "M12 20.5s-7.5-4.6-7.5-10A4.5 4.5 0 0 1 12 7.5a4.5 4.5 0 0 1 7.5 3c0 5.4-7.5 10-7.5 10z"),
    ("ASMR", ""),
    ("ASMR", ""),
    ("ASMR", ""),
    ("Silence", "M4 9.5h3.5L12 5.5v13l-4.5-4H4zM16 9.5l5 5M21 9.5l-5 5"),
];

/// The ASMR families' icons, in `iris_sound::ASMR_FAMILIES` order.
const TRIGGER_ICONS: [&str; 7] = [
    "M9 11V5.5a1.5 1.5 0 0 1 3 0V11m0-1.5a1.5 1.5 0 0 1 3 0V12m0-1a1.5 1.5 0 0 1 3 0v4a6 6 0 0 1-6 6h-.5A5.5 5.5 0 0 1 6 15.5l-1.4-3a1.4 1.4 0 0 1 2.4-1.4L9 13",
    "M5 17 17 5M8 19 19 8M4 13 13 4",
    "M6 3.5h8l4 4v13H6zM9.5 9l2 2.5-1.5 2 2.5 2.5M14 9.5l-1 2",
    "M14.5 4.5l5 5-7.5 7.5-5-5zM7 12l-3.5 3.5a2.1 2.1 0 0 0 3 3L10 15",
    "M12 3.5s-5.5 6.3-5.5 10.3a5.5 5.5 0 0 0 11 0c0-4-5.5-10.3-5.5-10.3z",
    "M4 18c2.5-4 5-6 8-6s5.5 2 8 6zM9 8.5h.01M13 6.5h.01M16 9h.01M11 10h.01",
    "M4 6c3-1.5 5.5-1.5 8 0v13c-2.5-1.5-5-1.5-8 0zM12 6c2.5-1.5 5-1.5 8 0v13c-3-1.5-5.5-1.5-8 0",
];

/// Switches: key, title, the line under it. What users read: what it does, never a
/// study (CLAUDE.md, *no sources in the app*).
const SWITCHES: [(&str, &str, &str); 4] = [
    (
        "boost",
        "Focus boost",
        "A fast, light pulse in the sound that can help hold attention. Not with epilepsy.",
    ),
    (
        "day",
        "Follow the time of day",
        "Darker and slower in the evening",
    ),
    (
        "breath",
        "Breathing guide",
        "Swells six times a minute: in for 4 s, out for 6 s",
    ),
    ("bell", "End with a bell", "Fades almost to silence first"),
];
const DARKEN: (&str, &str, &str) = (
    "darken",
    "Darken as it plays",
    "The sound slowly closes toward the end",
);

pub fn theme(id: &str) -> &'static Theme {
    THEMES.iter().find(|t| t.id == id).unwrap_or(&THEMES[0])
}

fn theme_settings(set: &SoundSettings, t: &Theme) -> ThemeSettings {
    let mut s = set
        .themes
        .get(t.id)
        .cloned()
        .unwrap_or_else(|| ThemeSettings {
            sound: t.sounds[0],
            x: t.pad.0,
            y: t.pad.1,
            options: BTreeMap::new(),
        });
    if !t.sounds.contains(&s.sound) {
        s.sound = t.sounds[0];
    }
    for (k, v) in t.options {
        s.options.entry(k.to_string()).or_insert(*v);
    }
    s.x = s.x.min(1000);
    s.y = s.y.min(1000);
    s
}

// ---------------------------------------------------------------------- State

struct Run {
    start: Instant,
    bell: Option<Instant>,
    reminded: u32,
}

struct State {
    set: SoundSettings,
    playing: bool,
    run: Option<Run>,
    player: player::Player,
    window: slint::Weak<iris_ui::AppWindow>,
    /// Bumped at each change of sound: the light goes round the pad.
    ring: i32,
    dirty: Option<Instant>,
    error: Option<String>,
}

fn state() -> MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE
        .get_or_init(|| {
            Mutex::new(State {
                set: crate::settings::current().sound,
                playing: false,
                run: None,
                player: player::Player::default(),
                window: slint::Weak::default(),
                ring: 0,
                dirty: None,
                error: None,
            })
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

impl State {
    fn theme(&self) -> &'static Theme {
        theme(&self.set.theme)
    }
    fn current(&self) -> ThemeSettings {
        theme_settings(&self.set, self.theme())
    }
    fn change(&mut self, f: impl FnOnce(&mut ThemeSettings)) {
        let t = self.theme();
        let mut s = self.current();
        f(&mut s);
        self.set.themes.insert(t.id.to_string(), s);
        self.dirty = Some(Instant::now());
    }
    fn option(&self, key: &str) -> u32 {
        self.current().options.get(key).copied().unwrap_or(0)
    }
    fn session_secs(&self) -> Option<f32> {
        self.theme()
            .session
            .map(|_| self.option("session") as f32 * 60.0)
            .filter(|s| *s > 0.0)
    }

    /// Volume factor and light now, and the seconds left of a session.
    fn shape(&self) -> (f32, f32, Option<f32>) {
        let Some(run) = &self.run else {
            return (1.0, 1.0, None);
        };
        let el = run.start.elapsed().as_secs_f32();
        let Some(len) = self.session_secs() else {
            return (1.0, 1.0, None);
        };
        let left = len - el;
        let fade = self
            .theme()
            .session
            .map(|s| s.2)
            .unwrap_or(60.0)
            .min(len * 0.5);
        let mut g = 1.0;
        if left < fade {
            let u = left.max(0.0) / fade;
            g = if self.theme().id == "break" && self.option("bell") == 1 {
                0.12 + 0.88 * u
            } else {
                u
            };
        }
        let light = if self.option("darken") == 1 {
            1.0 - 0.45 * (el / len).min(1.0)
        } else {
            1.0
        };
        (g, light, Some(left))
    }

    fn hour(&self) -> f32 {
        if self.option("day") == 1 {
            use chrono::Timelike;
            let now = chrono::Local::now();
            now.hour() as f32 + now.minute() as f32 / 60.0
        } else {
            14.0
        }
    }

    /// Everything the engine must know, now.
    fn messages(&self) -> Vec<player::Msg> {
        use player::Msg;
        let t = self.theme();
        let s = self.current();
        let (g, light, _) = self.shape();
        let p = t.profile;
        vec![
            Msg::Profile([p[0], p[1] * light, p[2], p[3]]),
            Msg::Pad(s.x as f32 / 1000.0, s.y as f32 / 1000.0),
            Msg::Scape(s.sound),
            Msg::Hour(self.hour()),
            Msg::Modulation(if t.id == "focus" && self.option("boost") == 1 {
                1.0
            } else {
                0.0
            }),
            Msg::Breath(if self.option("breath") == 1 { 1.0 } else { 0.0 }),
            Msg::Mask(
                theme_settings(&self.set, theme("asmr"))
                    .options
                    .get("mask")
                    .copied()
                    .unwrap_or(DEFAULT_MASK),
            ),
            Msg::Volume(if self.playing {
                self.set.volume as f32 / 1000.0 * g
            } else {
                0.0
            }),
        ]
    }

    fn send_all(&self) {
        for m in self.messages() {
            self.player.send(m);
        }
    }

    fn play(&mut self) {
        self.error = None;
        let mut first = self.messages();
        first.retain(|m| !matches!(m, player::Msg::Volume(_)));
        if let Err(e) = self.player.start(first) {
            tracing::warn!("sound: {e}");
            self.error = Some(e);
            self.playing = false;
            return;
        }
        self.playing = true;
        self.run = Some(Run {
            start: Instant::now(),
            bell: None,
            reminded: 0,
        });
        self.send_all();
    }

    fn pause(&mut self) {
        self.playing = false;
        self.run = None;
        self.player.send(player::Msg::Volume(0.0));
        // The fade takes ~0.3 s; then the stream and its thread go.
        self.player.stop_after(Duration::from_millis(600));
    }
}

// ---------------------------------------------------------------------- What others call

/// Plays or pauses.
pub fn toggle() {
    let mut s = state();
    if s.playing {
        s.pause();
    } else {
        s.play();
        // The light goes round the pad: sound has begun.
        s.ring += 1;
    }
    drop(s);
    refresh_later();
}

/// What the assistant and the tests read.
pub struct Status {
    pub playing: bool,
    pub theme: &'static str,
    pub sound: &'static str,
    pub volume: u32,
    pub x: u32,
    pub y: u32,
    pub minutes_left: Option<u32>,
    pub error: Option<String>,
}

pub fn status() -> Status {
    let s = state();
    let c = s.current();
    let (_, _, left) = s.shape();
    Status {
        playing: s.playing,
        theme: s.theme().id,
        sound: SOUNDS[c.sound as usize].0,
        volume: s.set.volume / 10,
        x: c.x / 10,
        y: c.y / 10,
        minutes_left: left.map(|l| (l.max(0.0) / 60.0).ceil() as u32),
        error: s.error.clone(),
    }
}

/// Changes what plays, then plays it. Every field is optional; `sound` is a sound's
/// name within the theme; `volume`, `x`, `y` are 0–100; `minutes` a session's length.
pub struct Play<'a> {
    pub theme: Option<&'a str>,
    pub sound: Option<&'a str>,
    pub volume: Option<u32>,
    pub x: Option<u32>,
    pub y: Option<u32>,
    pub minutes: Option<u32>,
}

pub fn play(p: Play) -> Result<(), String> {
    let mut s = state();
    if let Some(id) = p.theme {
        if !THEMES.iter().any(|t| t.id == id) {
            return Err(format!(
                "no theme {id}: {}",
                THEMES.iter().map(|t| t.id).collect::<Vec<_>>().join(", ")
            ));
        }
        s.set.theme = id.to_string();
        s.ring += 1;
    }
    let t = s.theme();
    if let Some(name) = p.sound {
        let Some(id) = t
            .sounds
            .iter()
            .copied()
            .find(|i| SOUNDS[*i as usize].0.eq_ignore_ascii_case(name))
        else {
            return Err(format!(
                "{} has no sound {name}: {}",
                t.name,
                t.sounds
                    .iter()
                    .map(|i| SOUNDS[*i as usize].0)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        };
        s.change(|c| c.sound = id);
        s.ring += 1;
    }
    if let Some(v) = p.volume {
        s.set.volume = v.min(100) * 10;
    }
    if p.x.is_some() || p.y.is_some() {
        s.change(|c| {
            c.x = p.x.map(|v| v.min(100) * 10).unwrap_or(c.x);
            c.y = p.y.map(|v| v.min(100) * 10).unwrap_or(c.y);
        });
    }
    if let Some(m) = p.minutes {
        if t.session.is_none() {
            return Err(format!("{} has no session length", t.name));
        }
        s.change(|c| {
            c.options.insert("session".into(), m);
        });
    }
    s.dirty = Some(Instant::now());
    if s.playing {
        s.run = Some(Run {
            start: Instant::now(),
            bell: None,
            reminded: 0,
        });
        s.send_all();
    } else {
        s.play();
    }
    let e = s.error.clone();
    drop(s);
    refresh_later();
    e.map_or(Ok(()), Err)
}

pub fn stop() {
    let mut s = state();
    if s.playing {
        s.pause();
    }
    drop(s);
    refresh_later();
}

// ---------------------------------------------------------------------- The window

use iris_ui::{AppWindow, SoundItemData, SoundOptionData};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

/// Wires the Sound panel. Called once, at startup.
pub fn wire_sound(f: &AppWindow) {
    state().window = f.as_weak();
    f.set_sound_available(player::AVAILABLE);

    f.on_sound_play(toggle);
    let w = f.as_weak();
    f.on_sound_theme(move |i| {
        let Some(t) = THEMES.get(i as usize) else {
            return;
        };
        let mut s = state();
        if s.set.theme != t.id {
            s.set.theme = t.id.to_string();
            s.dirty = Some(Instant::now());
            s.ring += 1;
            if s.playing {
                s.run = Some(Run {
                    start: Instant::now(),
                    bell: None,
                    reminded: 0,
                });
                s.send_all();
            }
        }
        drop(s);
        if let Some(f) = w.upgrade() {
            refresh(&f);
        }
    });
    let w = f.as_weak();
    f.on_sound_pick(move |i| {
        let mut s = state();
        let Some(id) = s.theme().sounds.get(i as usize).copied() else {
            return;
        };
        if s.current().sound != id {
            s.change(|c| c.sound = id);
            s.ring += 1;
        }
        if s.playing {
            s.player.send(player::Msg::Scape(id));
        } else {
            s.play();
        }
        drop(s);
        if let Some(f) = w.upgrade() {
            refresh(&f);
        }
    });
    let w = f.as_weak();
    f.on_sound_trigger(move |i| {
        let mut s = state();
        let asmr = theme("asmr");
        let mut a = theme_settings(&s.set, asmr);
        let mask = a.options.get("mask").copied().unwrap_or(DEFAULT_MASK) ^ (1 << i);
        // The last family on stays on.
        if mask == 0 {
            return;
        }
        a.options.insert("mask".into(), mask);
        s.set.themes.insert(asmr.id.into(), a);
        s.dirty = Some(Instant::now());
        s.player.send(player::Msg::Mask(mask));
        if !s.playing {
            s.play();
        }
        drop(s);
        if let Some(f) = w.upgrade() {
            refresh(&f);
        }
    });
    let w = f.as_weak();
    f.on_sound_move(move |x, y| {
        let mut s = state();
        let (x, y) = (
            (x.clamp(0.0, 1.0) * 1000.0) as u32,
            (y.clamp(0.0, 1.0) * 1000.0) as u32,
        );
        s.change(|c| {
            c.x = x;
            c.y = y;
        });
        s.player
            .send(player::Msg::Pad(x as f32 / 1000.0, y as f32 / 1000.0));
        if !s.playing {
            s.play();
        }
        drop(s);
        if let Some(f) = w.upgrade() {
            refresh(&f);
        }
    });
    let w = f.as_weak();
    f.on_sound_volume_set(move |v| {
        let mut s = state();
        s.set.volume = (v.clamp(0.0, 1.0) * 1000.0) as u32;
        s.dirty = Some(Instant::now());
        if s.playing {
            let (g, _, _) = s.shape();
            s.player
                .send(player::Msg::Volume(s.set.volume as f32 / 1000.0 * g));
        }
        drop(s);
        if let Some(f) = w.upgrade() {
            refresh(&f);
        }
    });
    let w = f.as_weak();
    f.on_sound_option(move |key, value| {
        let mut s = state();
        let key = key.to_string();
        // A switch says 0 or 1; a row of choices says which one, by its place.
        let value = value.max(0) as usize;
        let value = match key.as_str() {
            "session" => s
                .theme()
                .session
                .and_then(|x| x.1.get(value).copied())
                .unwrap_or(0),
            "remind" => REMIND.get(value).copied().unwrap_or(0),
            _ => value as u32,
        };
        s.change(|c| {
            c.options.insert(key, value);
        });
        if s.run.is_some() {
            // A new length counts from now.
            s.run = Some(Run {
                start: Instant::now(),
                bell: None,
                reminded: 0,
            });
        }
        if s.playing {
            s.send_all();
        }
        drop(s);
        if let Some(f) = w.upgrade() {
            refresh(&f);
        }
    });

    refresh(f);

    // The session clock: fades, bells, the time of day; and the settings, saved a
    // second after the last change.
    let w = f.as_weak();
    let clock = Box::leak(Box::new(slint::Timer::default()));
    clock.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(500),
        move || {
            let changed = tick();
            if let Some(f) = w.upgrade() {
                if changed {
                    refresh(&f);
                } else if f.get_sound_open() {
                    f.set_sound_state(state_line(&state()).into());
                }
            }
        },
    );
}

/// Twice a second. Returns whether the panel must be redrawn (a session ended).
fn tick() -> bool {
    let mut s = state();
    if let Some(at) = s.dirty {
        if at.elapsed() > Duration::from_secs(1) {
            s.dirty = None;
            let sound = s.set.clone();
            crate::settings::update(move |r| r.sound = sound);
        }
    }
    if !s.playing {
        return false;
    }
    let (g, light, left) = s.shape();
    let volume = s.set.volume as f32 / 1000.0 * g;
    let p = s.theme().profile;
    s.player.send(player::Msg::Volume(volume));
    s.player
        .send(player::Msg::Profile([p[0], p[1] * light, p[2], p[3]]));
    let hour = s.hour();
    s.player.send(player::Msg::Hour(hour));
    let boost = 1.0 / g.max(0.08);
    // Deep work: a soft bell every 50 or 90 minutes, if asked.
    let remind = s.option("remind");
    if s.theme().id == "deep" && remind > 0 {
        let el = s
            .run
            .as_ref()
            .map(|r| r.start.elapsed().as_secs())
            .unwrap_or(0);
        let due = s
            .run
            .as_ref()
            .map(|r| el >= (r.reminded as u64 + 1) * remind as u64 * 60)
            .unwrap_or(false);
        if due {
            if let Some(r) = s.run.as_mut() {
                r.reminded += 1;
            }
            s.player.send(player::Msg::Chime(boost));
        }
    }
    let Some(left) = left else { return false };
    if left > 0.0 {
        return false;
    }
    // A break ends with its bell, then a few seconds later the sound stops.
    if s.theme().id == "break" && s.option("bell") == 1 {
        let rang = s.run.as_ref().and_then(|r| r.bell);
        match rang {
            None => {
                if let Some(r) = s.run.as_mut() {
                    r.bell = Some(Instant::now());
                }
                s.player.send(player::Msg::Chime(boost));
                return false;
            }
            Some(at) if at.elapsed() < Duration::from_secs(6) => return false,
            Some(_) => {}
        }
    }
    s.pause();
    true
}

fn state_line(s: &State) -> String {
    let t = s.theme();
    if let Some(e) = &s.error {
        return e.clone();
    }
    if !s.playing {
        return t.line.to_string();
    }
    let c = s.current();
    if c.sound == SILENCE {
        return "Silence".into();
    }
    let name = SOUNDS[c.sound as usize].0;
    match s.shape().2 {
        Some(left) if left > 0.0 => {
            let m = (left / 60.0).ceil() as u32;
            if m >= 60 {
                format!("{name} · {} h {:02} left", m / 60, m % 60)
            } else {
                format!("{name} · {m} min left")
            }
        }
        Some(_) => format!("{name} · ending"),
        None => format!("{name} · playing"),
    }
}

fn rgb(c: u32) -> slint::Color {
    slint::Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn items(v: Vec<SoundItemData>) -> ModelRc<SoundItemData> {
    ModelRc::new(VecModel::from(v))
}

/// Puts the state into the window. On the interface's thread.
pub fn refresh(f: &AppWindow) {
    let s = state();
    let t = s.theme();
    let c = s.current();
    f.set_sound_playing(s.playing);
    f.set_sound_state(state_line(&s).into());
    // Two rows of chips: four, then the rest (a Slint layout does not wrap).
    let themes: Vec<SoundItemData> = THEMES
        .iter()
        .map(|x| SoundItemData {
            name: x.name.into(),
            icon: x.icon.into(),
            on: x.id == t.id,
        })
        .collect();
    f.set_sound_themes_a(items(themes[..4].to_vec()));
    f.set_sound_themes_b(items(themes[4..].to_vec()));
    let names: Vec<SharedString> = t
        .sounds
        .iter()
        .map(|i| SOUNDS[*i as usize].0.into())
        .collect();
    f.set_sound_names(ModelRc::new(VecModel::from(names)));
    f.set_sound_index(t.sounds.iter().position(|i| *i == c.sound).unwrap_or(0) as i32);
    let asmr = t.id == "asmr";
    f.set_sound_asmr(asmr);
    let mask = c.options.get("mask").copied().unwrap_or(DEFAULT_MASK);
    let triggers: Vec<SoundItemData> = iris_sound::ASMR_FAMILIES
        .iter()
        .enumerate()
        .map(|(k, n)| SoundItemData {
            name: (*n).into(),
            icon: TRIGGER_ICONS[k].into(),
            on: mask & (1 << k) != 0,
        })
        .collect();
    f.set_sound_triggers_a(items(triggers[..4].to_vec()));
    f.set_sound_triggers_b(items(triggers[4..].to_vec()));
    let axes: Vec<SharedString> = t.axes.iter().map(|a| (*a).into()).collect();
    f.set_sound_axes(ModelRc::new(VecModel::from(axes)));
    f.set_sound_x(c.x as f32 / 1000.0);
    f.set_sound_y(c.y as f32 / 1000.0);
    f.set_sound_volume(s.set.volume as f32 / 1000.0);
    f.set_sound_silent(c.sound == SILENCE);
    f.set_sound_hue_a(rgb(t.hues.0));
    f.set_sound_hue_b(rgb(t.hues.1));
    if f.get_sound_ring() != s.ring {
        f.set_sound_ring(s.ring);
    }
    let mut rows = Vec::new();
    for (k, _) in t.options {
        let on = c.options.get(*k).copied().unwrap_or(0);
        let row = |key: &str,
                   title: &str,
                   detail: &str,
                   toggle: bool,
                   choices: Vec<String>,
                   chosen: i32| SoundOptionData {
            key: key.into(),
            title: title.into(),
            detail: detail.into(),
            toggle,
            on: on == 1,
            choices: ModelRc::new(VecModel::from(
                choices
                    .into_iter()
                    .map(SharedString::from)
                    .collect::<Vec<_>>(),
            )),
            chosen,
        };
        match *k {
            "session" => {
                if let Some((label, choices, _)) = t.session {
                    let names = choices
                        .iter()
                        .map(|m| {
                            if *m == 0 {
                                "Off".to_string()
                            } else {
                                format!("{m} min")
                            }
                        })
                        .collect();
                    let chosen = choices.iter().position(|m| *m == on).unwrap_or(0) as i32;
                    rows.push(row("session", label, "", false, names, chosen));
                }
            }
            "remind" => {
                let choices = REMIND;
                let names = choices
                    .iter()
                    .map(|m| {
                        if *m == 0 {
                            "Off".to_string()
                        } else {
                            format!("{m} min")
                        }
                    })
                    .collect();
                let chosen = choices.iter().position(|m| *m == on).unwrap_or(0) as i32;
                rows.push(row(
                    "remind",
                    "Break reminder",
                    "A soft bell, nothing else",
                    false,
                    names,
                    chosen,
                ));
            }
            "mask" => {}
            "darken" => rows.push(row(DARKEN.0, DARKEN.1, DARKEN.2, true, Vec::new(), 0)),
            key => {
                if let Some((k, title, detail)) = SWITCHES.iter().find(|w| w.0 == key) {
                    rows.push(row(k, title, detail, true, Vec::new(), 0));
                }
            }
        }
    }
    f.set_sound_options(ModelRc::new(VecModel::from(rows)));

    #[cfg(windows)]
    {
        let title = if c.sound == SILENCE {
            t.name.to_string()
        } else {
            format!("{} · {}", t.name, SOUNDS[c.sound as usize].0)
        };
        let playing = s.playing;
        drop(s);
        media::sync(f, playing, &title);
    }
}

/// From another thread (the assistant): redraws the panel on the interface's thread.
fn refresh_later() {
    let w = state().window.clone();
    let _ = w.upgrade_in_event_loop(|f| refresh(&f));
}

/// The values a session's length may take, for the assistant.
pub fn session_choices(id: &str) -> Option<&'static [u32]> {
    theme(id).session.map(|s| s.1)
}

/// Play or pause asked from outside Iris: Windows's media controls, the keyboard's key.
#[cfg(windows)]
fn from_system(play: bool) {
    let mut s = state();
    if play && !s.playing {
        s.play();
    } else if !play && s.playing {
        s.pause();
    }
    drop(s);
    refresh_later();
}

/// Windows's media controls: Iris shows in the volume flyout and on the lock screen as
/// Spotify or a browser do ("Focus · Drift", by Iris), and their play and pause buttons
/// — and the keyboard's media key — play and pause it. Shown from the first play on.
#[cfg(windows)]
#[allow(unsafe_code)]
mod media {
    use std::sync::Mutex;

    use iris_ui::AppWindow;
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::winit_030::WinitWindowAccessor;
    use slint::ComponentHandle;
    use windows::core::{factory, HSTRING};
    use windows::Foundation::TypedEventHandler;
    use windows::Media::{
        MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls,
        SystemMediaTransportControlsButton, SystemMediaTransportControlsButtonPressedEventArgs,
    };
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;
    use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

    struct Shown {
        controls: SystemMediaTransportControls,
        playing: bool,
        title: String,
    }

    static SHOWN: Mutex<Option<Shown>> = Mutex::new(None);

    fn handle(f: &AppWindow) -> Option<isize> {
        f.window()
            .with_winit_window(|w| match w.window_handle().ok()?.as_raw() {
                RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
                _ => None,
            })
            .flatten()
    }

    fn attach(f: &AppWindow) -> windows::core::Result<SystemMediaTransportControls> {
        let Some(hwnd) = handle(f) else {
            return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                -2147467259,
            )));
        };
        // SAFETY: joining the apartment winit already made on this thread is harmless
        // (`S_FALSE`); the interop call takes the window's valid handle.
        let controls: SystemMediaTransportControls = unsafe {
            CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
            let interop =
                factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>()?;
            interop.GetForWindow(HWND(hwnd as *mut core::ffi::c_void))?
        };
        controls.SetIsPlayEnabled(true)?;
        controls.SetIsPauseEnabled(true)?;
        let pressed = TypedEventHandler::<
            SystemMediaTransportControls,
            SystemMediaTransportControlsButtonPressedEventArgs,
        >::new(|_, args| {
            let button = args.ok()?.Button()?;
            if button == SystemMediaTransportControlsButton::Play {
                super::from_system(true);
            } else if button == SystemMediaTransportControlsButton::Pause {
                super::from_system(false);
            }
            Ok(())
        });
        controls.ButtonPressed(&pressed)?;
        Ok(controls)
    }

    /// Shows what plays, or that it is paused. On the interface's thread. Nothing
    /// before the first play: Iris does not sit in the flyout for nothing.
    pub fn sync(f: &AppWindow, playing: bool, title: &str) {
        let mut shown = SHOWN.lock().unwrap_or_else(|e| e.into_inner());
        if shown.is_none() {
            if !playing {
                return;
            }
            match attach(f) {
                Ok(controls) => {
                    *shown = Some(Shown {
                        controls,
                        playing: !playing,
                        title: String::new(),
                    })
                }
                Err(e) => {
                    tracing::warn!("sound: Windows's media controls: {e}");
                    return;
                }
            }
        }
        let Some(s) = shown.as_mut() else { return };
        if s.playing == playing && s.title == title {
            return;
        }
        if let Err(e) = show(&s.controls, playing, title) {
            tracing::warn!("sound: Windows's media controls: {e}");
        }
        s.playing = playing;
        s.title = title.to_string();
    }

    fn show(
        controls: &SystemMediaTransportControls,
        playing: bool,
        title: &str,
    ) -> windows::core::Result<()> {
        controls.SetIsEnabled(true)?;
        controls.SetPlaybackStatus(if playing {
            MediaPlaybackStatus::Playing
        } else {
            MediaPlaybackStatus::Paused
        })?;
        let shows = controls.DisplayUpdater()?;
        shows.SetType(MediaPlaybackType::Music)?;
        let music = shows.MusicProperties()?;
        music.SetTitle(&HSTRING::from(title))?;
        music.SetArtist(&HSTRING::from("Iris"))?;
        shows.Update()
    }
}

// ---------------------------------------------------------------------- Output

#[cfg(not(target_os = "android"))]
mod player {
    //! The output: a `cpal` stream on a thread of its own, the engine inside its
    //! callback, orders through a channel the callback empties without waiting.

    use std::sync::mpsc;
    use std::time::Duration;

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    pub const AVAILABLE: bool = true;

    pub enum Msg {
        Pad(f32, f32),
        Scape(u8),
        Volume(f32),
        Modulation(f32),
        Breath(f32),
        Hour(f32),
        Profile([f32; 4]),
        Chime(f32),
        Mask(u32),
    }

    #[derive(Default)]
    pub struct Player {
        tx: Option<mpsc::Sender<Msg>>,
        stop: Option<mpsc::Sender<Duration>>,
    }

    impl Player {
        /// Opens the output and starts the engine, unless it already runs.
        pub fn start(&mut self, first: Vec<Msg>) -> Result<(), String> {
            if self.tx.is_some() {
                for m in first {
                    self.send(m);
                }
                return Ok(());
            }
            let (tx, rx) = mpsc::channel::<Msg>();
            let (stop, stopped) = mpsc::channel::<Duration>();
            let (ready, answer) = mpsc::channel::<Result<(), String>>();
            for m in first {
                let _ = tx.send(m);
            }
            // Detached: it ends by itself once told to stop.
            let _thread = std::thread::Builder::new()
                .name("iris-sound".into())
                .spawn(move || match open(rx) {
                    Ok(stream) => {
                        let _ = ready.send(Ok(()));
                        // Until told to stop (after the fade), or the player is gone.
                        if let Ok(after) = stopped.recv() {
                            std::thread::sleep(after);
                        }
                        drop(stream);
                    }
                    Err(e) => {
                        let _ = ready.send(Err(e));
                    }
                })
                .map_err(|e| e.to_string())?;
            answer
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| "The sound output did not answer.".to_string())??;
            self.tx = Some(tx);
            self.stop = Some(stop);
            Ok(())
        }

        pub fn send(&self, m: Msg) {
            if let Some(tx) = &self.tx {
                let _ = tx.send(m);
            }
        }

        /// Closes the output after `after` (the fade under way).
        pub fn stop_after(&mut self, after: Duration) {
            self.tx = None;
            if let Some(s) = self.stop.take() {
                let _ = s.send(after);
            }
        }
    }

    fn open(rx: mpsc::Receiver<Msg>) -> Result<cpal::Stream, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "No sound output on this computer.".to_string())?;
        let supported = device
            .default_output_config()
            .map_err(|e| format!("The sound output refused: {e}"))?;
        let format = supported.sample_format();
        let config = supported.config();
        let stream = match format {
            cpal::SampleFormat::F32 => build::<f32>(&device, config, rx, |v| v),
            cpal::SampleFormat::I16 => build::<i16>(&device, config, rx, |v| {
                (v.clamp(-1.0, 1.0) * 32767.0) as i16
            }),
            other => {
                return Err(format!(
                    "The sound output wants {other:?} samples, which Iris does not write."
                ))
            }
        }?;
        stream
            .play()
            .map_err(|e| format!("The sound output would not start: {e}"))?;
        Ok(stream)
    }

    fn build<T: cpal::SizedSample + Send + 'static>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        rx: mpsc::Receiver<Msg>,
        convert: fn(f32) -> T,
    ) -> Result<cpal::Stream, String> {
        let channels = config.channels.max(1) as usize;
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        let mut engine = iris_sound::Engine::new(config.sample_rate as f32, seed);
        engine.set_volume(0.0);
        let mut buf: Vec<f32> = vec![0.0; 4096];
        device
            .build_output_stream::<T, _, _>(
                config,
                move |out: &mut [T], _| {
                    while let Ok(m) = rx.try_recv() {
                        apply(&mut engine, m);
                    }
                    let frames = out.len() / channels;
                    if buf.len() < frames * 2 {
                        buf.resize(frames * 2, 0.0);
                    }
                    let b = &mut buf[..frames * 2];
                    engine.render(b);
                    for (f, frame) in out.chunks_mut(channels).enumerate() {
                        let (l, r) = (b[2 * f], b[2 * f + 1]);
                        for (c, s) in frame.iter_mut().enumerate() {
                            *s = convert(match (channels, c) {
                                (1, _) => (l + r) * 0.5,
                                (_, 0) => l,
                                (_, 1) => r,
                                _ => 0.0,
                            });
                        }
                    }
                },
                |e| tracing::warn!("sound output: {e}"),
                None,
            )
            .map_err(|e| format!("The sound output would not open: {e}"))
    }

    fn apply(e: &mut iris_sound::Engine, m: Msg) {
        match m {
            Msg::Pad(x, y) => e.set_pad(x, y),
            Msg::Scape(s) => e.set_scape(s),
            Msg::Volume(v) => e.set_volume(v),
            Msg::Modulation(a) => e.set_modulation(a),
            Msg::Breath(b) => e.set_breath(b),
            Msg::Hour(h) => e.set_hour(h),
            Msg::Profile(p) => e.set_profile(p[0], p[1], p[2], p[3]),
            Msg::Chime(b) => e.chime(b),
            Msg::Mask(m) => e.set_asmr_mask(m),
        }
    }
}

/// Android has no output yet: playing in the background needs a foreground service.
#[cfg(target_os = "android")]
mod player {
    use std::time::Duration;

    pub const AVAILABLE: bool = false;

    #[allow(dead_code)]
    pub enum Msg {
        Pad(f32, f32),
        Scape(u8),
        Volume(f32),
        Modulation(f32),
        Breath(f32),
        Hour(f32),
        Profile([f32; 4]),
        Chime(f32),
        Mask(u32),
    }

    #[derive(Default)]
    pub struct Player;

    impl Player {
        pub fn start(&mut self, _first: Vec<Msg>) -> Result<(), String> {
            Err("Sound isn't available on Android yet.".into())
        }
        pub fn send(&self, _m: Msg) {}
        pub fn stop_after(&mut self, _after: Duration) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_offers_sounds_the_engine_has_and_options_the_panel_knows() {
        for t in &THEMES {
            assert!(!t.sounds.is_empty(), "{} has no sound", t.id);
            for s in t.sounds {
                assert!(
                    (*s as usize) < iris_sound::SCAPES.len(),
                    "{}: no sound {s}",
                    t.id
                );
            }
            for (k, _) in t.options {
                let known = SWITCHES.iter().any(|w| w.0 == *k)
                    || ["session", "remind", "mask", "darken"].contains(k);
                assert!(known, "{}: the panel does not know the option {k}", t.id);
            }
            assert_eq!(
                t.session.is_some(),
                t.options.iter().any(|(k, _)| *k == "session"),
                "{}: a session needs its option",
                t.id
            );
        }
    }

    #[test]
    fn a_theme_falls_back_to_its_first_sound_and_its_defaults() {
        let mut set = SoundSettings::default();
        set.themes.insert(
            "asmr".into(),
            ThemeSettings {
                sound: 10,
                x: 5000,
                y: 2,
                options: BTreeMap::new(),
            },
        );
        let s = theme_settings(&set, theme("asmr"));
        assert_eq!(s.sound, 9, "a former ASMR sound plays the one ASMR");
        assert_eq!(s.x, 1000, "the pad stays on the pad");
        assert_eq!(s.options.get("mask"), Some(&DEFAULT_MASK));
        assert_eq!(theme("nothing").id, "focus");
    }
}
