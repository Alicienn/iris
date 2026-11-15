//! Iris's sound engine: generative soundscapes for working, resting and sleeping.
//!
//! Nothing is recorded: everything is synthesised as it plays, from additive voices,
//! modal "taps", bubbles, coloured noise, filters, a ping-pong delay and a reverb. It never
//! repeats and never ends. The crate has no dependencies and no I/O: it fills buffers;
//! `iris-app` plays them (see `docs/ARCHITECTURE.md`, *Sound*).
//!
//! Two controls shape every sound, the pad's two axes:
//! - `x`: fluid (0) → structured (1): free timing, drifting harmony, waves and gusts on the
//!   left; a steady pulse, grid-aligned events and regular changes on the right.
//! - `y`: easy (0) → intense (1): density, tempo, brightness, number of events.
//!
//! A theme (focus, deep work, relax, sleep…) is chosen by the app; it sets a profile here:
//! a ceiling on intensity, a brightness factor and an event density, plus the modulation
//! and breathing defaults.
//!
//! What the research asks for, everywhere:
//! - no speech, no lyrics, no sudden onsets, no crescendos (irrelevant sound effect; blood
//!   pressure rises with crescendos);
//! - only pentatonic notes, so no voice can clash with another;
//! - note density and loudness kept moderate (fast *and* loud music hurts reading);
//! - the 16 Hz modulation only on the 200 Hz–1 kHz band, as in Woods et al. 2024;
//! - an optional breathing swell at 6 breaths a minute (0.1 Hz);
//! - darker and slower in the evening (time of day, a design choice, not evidence).

// Signal code walks several parallel arrays with one index (a voice's phases, amplitudes,
// decays; a reverb's lines, lengths, gains): an index reads better here than zipped
// iterators. Clippy 1.100 flags far more of these loops than 1.95 did.
#![allow(clippy::needless_range_loop)]

use std::f32::consts::PI;

mod asmr;

/// The ASMR families, in the order of [`Engine::set_asmr_mask`]'s bits.
pub const ASMR_FAMILIES: [&str; asmr::FAMILIES] = [
    "Taps",
    "Scratching",
    "Paper",
    "Brush",
    "Water",
    "Sand",
    "Pages",
];
/// The families on by default: all but paper (one of the sounds that most often bothers
/// people with misophonia).
pub const ASMR_DEFAULT_MASK: u32 = asmr::DEFAULT_MASK;

pub const BLOCK: usize = 128;
const TABLE: usize = 4096;
const NP: usize = 6;
const MAX_VOICES: usize = 48;
const MAX_BUBBLES: usize = 64;

pub const SCAPES: [&str; 14] = [
    "Drift", "Pulse", "Keys", "Rain", "Noise", "Drone", "Stream", "Shore", "Forest", "ASMR",
    "ASMR", "ASMR", "ASMR", "Silence",
];
const DRIFT: u8 = 0;
const PULSE: u8 = 1;
const KEYS: u8 = 2;
const RAIN: u8 = 3;
const NOISE: u8 = 4;
const DRONE: u8 = 5;
const STREAM: u8 = 6;
const SHORE: u8 = 7;
const FOREST: u8 = 8;
/// One non-vocal ASMR soundscape, its families mixed and switchable (see `asmr.rs`).
/// Ids 10–12 (the former separate ASMR sounds) play it too.
const ASMR: u8 = 9;
/// Nothing at all: for deep work, where silence is still the reference. Bells still ring.
const SILENCE: u8 = 13;
const LAST: u8 = SILENCE;

// Bus a voice plays into.
const PAD: u8 = 0;
const LEAD: u8 = 1;
const TEX: u8 = 2;

// Chords, in semitones above the root, all inside the major pentatonic {0, 2, 4, 7, 9}.
const CHORDS: [[i32; 4]; 4] = [
    [0, 7, 12, 16],
    [-3, 4, 9, 12],
    [-5, 2, 7, 14],
    [2, 9, 14, 16],
];
const PENTA: [i32; 5] = [0, 2, 4, 7, 9];

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}
fn mtof(m: f32) -> f32 {
    440.0 * 2f32.powf((m - 69.0) / 12.0)
}
/// One-pole smoothing coefficient reaching ~63 % of the way in `time` seconds.
fn coef(time: f32, rate: f32) -> f32 {
    1.0 - (-1.0 / (time.max(1e-4) * rate)).exp()
}
fn asmr(s: u8) -> bool {
    s == ASMR
}

struct Rng(u64);
impl Rng {
    fn u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
    fn f(&mut self) -> f32 {
        self.u32() as f32 / 4_294_967_296.0
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        lerp(a, b, self.f())
    }
    fn bi(&mut self) -> f32 {
        self.f() * 2.0 - 1.0
    }
    fn pick(&mut self, n: usize) -> usize {
        self.u32() as usize % n.max(1)
    }
    /// Waiting time of a Poisson process with this mean, in the same unit.
    fn exp(&mut self, mean: f32) -> f32 {
        -mean * (1.0 - self.f()).max(1e-6).ln()
    }
}

/// Coloured noise for one channel: white, pink (Kellet) and brown.
#[derive(Default)]
struct Noise {
    b0: f32,
    b1: f32,
    b2: f32,
    brown: f32,
}
impl Noise {
    fn next(&mut self, w: f32) -> (f32, f32, f32) {
        self.b0 = 0.99765 * self.b0 + w * 0.099_046;
        self.b1 = 0.963 * self.b1 + w * 0.296_516_4;
        self.b2 = 0.57 * self.b2 + w * 1.052_691_3;
        let pink = (self.b0 + self.b1 + self.b2 + w * 0.1848) * 0.22;
        self.brown = (self.brown + 0.02 * w) / 1.02;
        (w * 0.5, pink, self.brown * 3.2)
    }
}

/// Zero-delay state-variable filter (Simper): low-pass and band-pass.
#[derive(Default, Clone, Copy)]
struct Svf {
    ic1: f32,
    ic2: f32,
}
#[derive(Default, Clone, Copy)]
struct SvfCo {
    a1: f32,
    a2: f32,
    a3: f32,
}
impl SvfCo {
    fn new(cutoff: f32, q: f32, rate: f32) -> Self {
        let g = (PI * cutoff.clamp(10.0, rate * 0.45) / rate).tan();
        let k = 1.0 / q;
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        SvfCo { a1, a2, a3: g * a2 }
    }
}
impl Svf {
    /// (low-pass, band-pass)
    fn run(&mut self, x: f32, c: &SvfCo) -> (f32, f32) {
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, v1)
    }
    fn lp(&mut self, x: f32, c: &SvfCo) -> f32 {
        self.run(x, c).0
    }
}

/// An additive voice: up to six partials, each with its own decay, one envelope.
#[derive(Clone, Copy, Default)]
struct Voice {
    on: bool,
    bus: u8,
    /// 1 for the chord's sustained voices, released at the next chord.
    group: u8,
    np: usize,
    ph: [f32; NP],
    inc: [f32; NP],
    amp: [f32; NP],
    dec: [f32; NP],
    glide: f32,
    env: f32,
    target: f32,
    att: f32,
    rel: f32,
    releasing: bool,
    gain: f32,
    pl: f32,
    pr: f32,
}
impl Voice {
    fn level(&self) -> f32 {
        self.env * self.amp[0] * self.gain
    }
    fn release(&mut self, time: f32, rate: f32) {
        self.releasing = true;
        self.target = 0.0;
        self.rel = coef(time / 4.6, rate);
    }
}

/// A bubble in running water: one sine, rising, dying in a few milliseconds.
#[derive(Clone, Copy, Default)]
struct Bubble {
    on: bool,
    ph: f32,
    inc: f32,
    glide: f32,
    amp: f32,
    dec: f32,
    pl: f32,
    pr: f32,
}

/// Something to play later: the notes of a bird's song, the taps of a finger roll.
#[derive(Clone, Copy)]
enum Ev {
    Chirp {
        f: f32,
        to: f32,
        dur: f32,
        gain: f32,
        pan: f32,
    },
    Bell {
        midi: i32,
        boost: f32,
    },
}

/// Feedback delay network: 8 lines, Hadamard mixing, damping in the loop.
struct Reverb {
    buf: Vec<f32>,
    start: [usize; 8],
    len: [usize; 8],
    pos: [usize; 8],
    lp: [f32; 8],
    g: [f32; 8],
    damp: f32,
}
impl Reverb {
    fn new(rate: f32) -> Self {
        const BASE: [f32; 8] = [
            1601.0, 1867.0, 2053.0, 2251.0, 2399.0, 2617.0, 2797.0, 3011.0,
        ];
        let mut start = [0; 8];
        let mut len = [0; 8];
        let mut total = 0;
        for i in 0..8 {
            len[i] = (BASE[i] * rate / 48_000.0) as usize;
            start[i] = total;
            total += len[i];
        }
        Reverb {
            buf: vec![0.0; total],
            start,
            len,
            pos: [0; 8],
            lp: [0.0; 8],
            g: [0.0; 8],
            damp: 0.5,
        }
    }
    fn set(&mut self, t60: f32, damp: f32, rate: f32) {
        for i in 0..8 {
            self.g[i] = 10f32.powf(-3.0 * self.len[i] as f32 / (rate * t60));
        }
        self.damp = damp;
    }
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let mut o = [0.0f32; 8];
        for (i, o) in o.iter_mut().enumerate() {
            *o = self.buf[self.start[i] + self.pos[i]];
        }
        let (ol, or) = (o[0] - o[2] + o[4] - o[6], o[1] - o[3] + o[5] - o[7]);
        for (i, o) in o.iter_mut().enumerate() {
            self.lp[i] += (*o - self.lp[i]) * self.damp;
            *o = self.lp[i];
        }
        // Fast Walsh–Hadamard, normalised.
        let mut h = 1;
        while h < 8 {
            let mut i = 0;
            while i < 8 {
                for j in i..i + h {
                    let (a, b) = (o[j], o[j + h]);
                    o[j] = a + b;
                    o[j + h] = a - b;
                }
                i += h * 2;
            }
            h *= 2;
        }
        let n = 0.353_553_4;
        for (i, o) in o.iter().enumerate() {
            let input = if i % 2 == 0 { l } else { r };
            self.buf[self.start[i] + self.pos[i]] = o * n * self.g[i] + input;
            self.pos[i] += 1;
            if self.pos[i] >= self.len[i] {
                self.pos[i] = 0;
            }
        }
        (ol * 0.25, or * 0.25)
    }
}

/// Stereo ping-pong delay for the lead voices.
struct Delay {
    l: Vec<f32>,
    r: Vec<f32>,
    pos: usize,
    time: usize,
}
impl Delay {
    fn process(&mut self, x: f32, fb: f32) -> (f32, f32) {
        let n = self.l.len();
        let rd = (self.pos + n - self.time.min(n - 1)) % n;
        let (dl, dr) = (self.l[rd], self.r[rd]);
        self.l[self.pos] = x + dr * fb;
        self.r[self.pos] = dl * fb;
        self.pos = (self.pos + 1) % n;
        (dl, dr)
    }
}

/// Smooth step, 0 → 1.
fn smooth(u: f32) -> f32 {
    let u = clamp01(u);
    u * u * (3.0 - 2.0 * u)
}
/// A gesture's envelope over its life `p` (0 → 1): rises over `a`, falls over `r`.
fn window(p: f32, a: f32, r: f32) -> f32 {
    smooth(p / a) * smooth((1.0 - p) / r)
}

pub struct Engine {
    rate: f32,
    rng: Rng,
    sine: Vec<f32>,
    voices: [Voice; MAX_VOICES],
    bubbles: [Bubble; MAX_BUBBLES],
    queue: Vec<(f64, Ev)>,
    // Controls: target and smoothed.
    tx: f32,
    ty: f32,
    py: f32,
    x: f32,
    y: f32,
    tvol: f32,
    vol: f32,
    tam: f32,
    am: f32,
    tbreath: f32,
    breath: f32,
    hour: f32,
    scape: u8,
    // Theme profile.
    cap: f32,
    bright_mul: f32,
    events: f32,
    pulse: f32,
    // Time, in samples.
    t: f64,
    beat: f32,
    root: i32,
    chord: usize,
    next_chord: f64,
    next_note: f64,
    next_tex: f64,
    next_gesture: f64,
    loops: [(i32, f32, f32, f64); 6],
    step: usize,
    bird_kind: usize,
    // Derived per block.
    bright: f32,
    trim: f32,
    // Signal path.
    nl: Noise,
    nr: Noise,
    bed_level: f32,
    bed_svf: [Svf; 2],
    bed_hp: [f32; 2],
    gust: f32,
    gust_target: f32,
    wave_ph: f32,
    wave_period: f32,
    pad_svf: [Svf; 2],
    lead_svf: [Svf; 2],
    rev_lp: [f32; 2],
    pad_lfo: f32,
    pulse_ph: f32,
    am_ph: f32,
    am_svf: [Svf; 2],
    breath_ph: f32,
    asmr: asmr::Asmr,
    delay: Delay,
    reverb: Reverb,
    dc: [f32; 2],
}

impl Engine {
    pub fn new(rate: f32, seed: u64) -> Self {
        let sine = (0..=TABLE)
            .map(|i| (i as f32 / TABLE as f32 * 2.0 * PI).sin())
            .collect();
        let mut rng = Rng(seed | 1);
        let roots = [48, 50, 51, 53, 55];
        let root = roots[rng.pick(roots.len())];
        let mut e = Engine {
            rate,
            rng,
            sine,
            voices: [Voice::default(); MAX_VOICES],
            bubbles: [Bubble::default(); MAX_BUBBLES],
            queue: Vec::with_capacity(64),
            tx: 0.5,
            ty: 0.4,
            py: 0.4,
            x: 0.5,
            y: 0.4,
            tvol: 0.8,
            vol: 0.0,
            tam: 1.0,
            am: 1.0,
            tbreath: 0.0,
            breath: 0.0,
            hour: 14.0,
            scape: DRIFT,
            cap: 1.0,
            bright_mul: 1.0,
            events: 1.0,
            pulse: 1.0,
            t: 0.0,
            beat: rate,
            root,
            chord: 0,
            next_chord: 0.0,
            next_note: 0.0,
            next_tex: 0.0,
            next_gesture: 0.0,
            loops: [(0, 0.0, 0.0, 0.0); 6],
            step: 0,
            bird_kind: 0,
            bright: 0.5,
            trim: 0.5,
            nl: Noise::default(),
            nr: Noise::default(),
            bed_level: 0.0,
            bed_svf: [Svf::default(); 2],
            bed_hp: [0.0; 2],
            gust: 1.0,
            gust_target: 1.0,
            wave_ph: 0.0,
            wave_period: 10.0,
            pad_svf: [Svf::default(); 2],
            lead_svf: [Svf::default(); 2],
            rev_lp: [0.0; 2],
            pad_lfo: 0.0,
            pulse_ph: 0.0,
            am_ph: 0.0,
            am_svf: [Svf::default(); 2],
            breath_ph: 0.0,
            asmr: asmr::Asmr::default(),
            delay: Delay {
                l: vec![0.0; rate as usize * 2],
                r: vec![0.0; rate as usize * 2],
                pos: 0,
                time: rate as usize / 2,
            },
            reverb: Reverb::new(rate),
            dc: [0.0; 2],
        };
        e.init_loops();
        e
    }

    // ------------------------------------------------------------------ Controls

    pub fn set_pad(&mut self, x: f32, y: f32) {
        self.tx = clamp01(x);
        self.py = clamp01(y);
        self.ty = self.py * self.cap;
    }
    pub fn set_volume(&mut self, v: f32) {
        self.tvol = clamp01(v);
    }
    /// Depth of the 16 Hz focus modulation, 0 (off) to 1 (as designed).
    pub fn set_modulation(&mut self, a: f32) {
        self.tam = clamp01(a);
    }
    /// Depth of the breathing swell (6 a minute), 0 (off) to 1.
    pub fn set_breath(&mut self, b: f32) {
        self.tbreath = clamp01(b);
    }
    pub fn set_hour(&mut self, h: f32) {
        self.hour = h.rem_euclid(24.0);
    }
    /// A theme's profile: ceiling on intensity, brightness factor, density of events, and
    /// how much the beat may swell (0 for sleep: slow pulses delay sleep onset).
    pub fn set_profile(&mut self, cap: f32, bright: f32, events: f32, pulse: f32) {
        self.cap = cap.clamp(0.05, 1.0);
        self.bright_mul = bright.clamp(0.2, 1.2);
        self.events = events.clamp(0.0, 2.0);
        self.pulse = clamp01(pulse);
        self.ty = self.py * self.cap;
    }
    /// A soft two-note bell: the end of a break, a reminder to take one. Rings whatever
    /// the sound, about as loud everywhere; `boost` makes up for a fade under way (the
    /// page passes 1 / its fade factor, so the bell keeps the user's volume).
    pub fn chime(&mut self, boost: f32) {
        let r = self.root + 24;
        let boost = boost.clamp(1.0, 12.0);
        self.queue.push((self.t, Ev::Bell { midi: r, boost }));
        self.queue.push((
            self.t + self.rate as f64 * 0.45,
            Ev::Bell { midi: r + 7, boost },
        ));
    }
    /// Which ASMR families may play, one bit each in [`ASMR_FAMILIES`] order.
    pub fn set_asmr_mask(&mut self, mask: u32) {
        self.asmr.mask = mask & ((1 << asmr::FAMILIES) - 1);
    }
    /// How many gestures ASMR family `k` has started so far.
    pub fn asmr_count(&self, k: usize) -> u32 {
        self.asmr.spawned.get(k).copied().unwrap_or(0)
    }
    /// Switches sound: the voices playing fade out, the new ones fade in.
    pub fn set_scape(&mut self, s: u8) {
        let s = s.min(LAST);
        let s = if (10..=12).contains(&s) { ASMR } else { s };
        if s == self.scape {
            return;
        }
        self.scape = s;
        let rate = self.rate;
        for v in self.voices.iter_mut().filter(|v| v.on) {
            v.release(3.0, rate);
        }
        // What was under way (the rest of a finger roll, a stroke, a song) finishes on its
        // own and overlaps the new sound: nothing is cut.
        self.next_note = self.t + self.rate as f64 * 1.5;
        self.next_tex = self.t;
        self.next_gesture = self.t + self.rate as f64 * 0.6;
        self.init_loops();
        self.on_chord();
    }

    // ------------------------------------------------------------------ Voices

    fn alloc(&mut self) -> usize {
        if let Some(i) = self.voices.iter().position(|v| !v.on) {
            return i;
        }
        let mut best = 0;
        for i in 1..MAX_VOICES {
            if self.voices[i].level() < self.voices[best].level() {
                best = i;
            }
        }
        best
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn(
        &mut self,
        bus: u8,
        group: u8,
        freq: f32,
        ratios: &[f32],
        amps: &[f32],
        decays: &[f32],
        attack: f32,
        gain: f32,
        pan: f32,
    ) -> usize {
        let i = self.alloc();
        let rate = self.rate;
        let mut v = Voice {
            on: true,
            bus,
            group,
            np: ratios.len().min(NP),
            glide: 1.0,
            target: 1.0,
            att: coef(attack / 4.6, rate),
            gain,
            ..Voice::default()
        };
        let p = (pan.clamp(-1.0, 1.0) + 1.0) * 0.25 * PI;
        v.pl = p.cos();
        v.pr = p.sin();
        for k in 0..v.np {
            v.inc[k] = (freq * ratios[k] / rate).min(0.45);
            v.amp[k] = if freq * ratios[k] > rate * 0.45 {
                0.0
            } else {
                amps[k]
            };
            v.dec[k] = if decays[k] > 0.0 {
                (-1.0 / (decays[k] * rate)).exp()
            } else {
                1.0
            };
            v.ph[k] = self.rng.f();
        }
        self.voices[i] = v;
        i
    }

    /// A sustained, slightly detuned tone: its partials beat slowly against each other.
    /// The detuned copy of the fundamental stays quiet (0.25): at 0.7 it beat against the
    /// fundamental every 4–6 s in the bass and the level pumped by ~15 dB (measured), a
    /// slow tremolo that the ear follows. Now the swing is under 4.5 dB.
    fn pad(&mut self, midi: i32, attack: f32, gain: f32, pan: f32, group: u8) {
        let roll = lerp(2.8, 1.4, self.bright.min(1.0));
        let d = self.rng.range(0.0025, 0.0045);
        let ratios = [1.0, 1.0 + d, 2.0 - d * 0.5, 3.0 + d, 4.0, 5.0 - d];
        let mut amps = [0.0; NP];
        for (k, a) in amps.iter_mut().enumerate() {
            let n = ratios[k].round();
            *a = (1.0 / n).powf(roll) * if k == 1 { 0.25 } else { 1.0 };
        }
        self.spawn(
            PAD,
            group,
            mtof(midi as f32),
            &ratios,
            &amps,
            &[0.0; NP],
            attack,
            gain,
            pan,
        );
    }

    /// A soft plucked or struck tone: higher partials die first, as on a felt piano.
    fn pluck(&mut self, bus: u8, midi: i32, decay: f32, attack: f32, gain: f32, pan: f32) {
        // Darker than a real piano: the upper partials fall in the ear's most sensitive band.
        let roll = lerp(3.0, 2.0, self.bright.min(1.0));
        let b = 0.0004;
        let mut ratios = [0.0; NP];
        let mut amps = [0.0; NP];
        let mut decs = [0.0; NP];
        for k in 0..NP {
            let n = (k + 1) as f32;
            ratios[k] = n * (1.0 + b * n * n).sqrt();
            amps[k] = (1.0 / n).powf(roll);
            decs[k] = decay / (1.0 + 0.7 * (n - 1.0));
        }
        self.spawn(
            bus,
            0,
            mtof(midi as f32),
            &ratios,
            &amps,
            &decs,
            attack,
            gain,
            pan,
        );
    }

    /// A water drop on glass: a short sine whose pitch rises, like a bubble closing.
    fn drop(&mut self) {
        let f = self.rng.range(1100.0, 3000.0) * lerp(0.75, 1.0, self.bright.min(1.0));
        let d = self.rng.range(0.015, 0.04);
        let g = self.rng.range(0.004, 0.03);
        let pan = self.rng.bi() * 0.9;
        let i = self.spawn(TEX, 0, f, &[1.0], &[1.0], &[d], 0.001, g, pan);
        self.voices[i].glide = 1.4f32.powf(1.0 / (0.03 * self.rate));
    }

    /// A bubble in a stream (own pool, much cheaper than a voice).
    fn bubble(&mut self, lo: f32, hi: f32, gain: f32) {
        let Some(i) = self.bubbles.iter().position(|b| !b.on) else {
            return;
        };
        let f = self.rng.range(lo, hi) * self.rng.range(1.0, 1.8);
        let d = self.rng.range(0.006, 0.022);
        let pan = (self.rng.bi() * 0.85 + 1.0) * 0.25 * PI;
        self.bubbles[i] = Bubble {
            on: true,
            ph: 0.0,
            inc: f / self.rate,
            glide: self.rng.range(1.3, 2.0).powf(1.0 / (0.02 * self.rate)),
            amp: gain * self.rng.range(0.3, 1.0),
            dec: (-1.0 / (d * self.rate)).exp(),
            pl: pan.cos(),
            pr: pan.sin(),
        };
    }

    /// A bird's chirp: a whistle sliding from one pitch to another.
    fn chirp(&mut self, f: f32, to: f32, dur: f32, gain: f32, pan: f32) {
        let i = self.spawn(
            TEX,
            0,
            f,
            &[1.0, 2.0],
            &[1.0, 0.12],
            &[dur * 0.45, dur * 0.3],
            0.006,
            gain,
            pan,
        );
        self.voices[i].glide = (to / f).powf(1.0 / (dur * self.rate));
    }

    fn release_group(&mut self, time: f32) {
        let rate = self.rate;
        for v in self
            .voices
            .iter_mut()
            .filter(|v| v.on && v.group == 1 && !v.releasing)
        {
            v.release(time, rate);
        }
    }

    // ------------------------------------------------------------------ Score

    fn init_loops(&mut self) {
        // Keys: Eno-style loops of incommensurate lengths, one note each. The six notes stay
        // within one octave above the pads (the pentatonic and its octave), shuffled: no
        // high note stands out of the texture.
        const FLUID: [f32; 6] = [19.7, 23.3, 27.1, 31.9, 17.3, 29.3];
        const GRID: [f32; 6] = [6.0, 8.0, 10.0, 12.0, 7.0, 9.0];
        let mut notes = [
            12 + PENTA[0],
            12 + PENTA[1],
            12 + PENTA[2],
            12 + PENTA[3],
            12 + PENTA[4],
            24,
        ];
        for i in (1..6).rev() {
            let j = self.rng.pick(i + 1);
            notes.swap(i, j);
        }
        for (i, &n) in notes.iter().enumerate() {
            let first = self.t + (self.rng.range(0.5, 9.0) * self.rate) as f64;
            self.loops[i] = (n, FLUID[i], GRID[i], first);
        }
    }

    fn on_chord(&mut self) {
        let x = self.x;
        let y = self.y;
        let c = CHORDS[self.chord];
        let r = self.root;
        let attack = lerp(5.0, 1.5, x);
        self.release_group(lerp(7.0, 2.5, x));
        match self.scape {
            DRIFT => {
                let n = 2 + (y * 2.99) as usize;
                for &s in c.iter().take(n) {
                    let pan = self.rng.bi() * 0.5;
                    self.pad(r + s, attack, 0.11, pan, 1);
                }
                self.pad(r + c[0] - 12, attack, 0.09, 0.0, 1);
                // Towards intense, a held voice an octave up: colour without onsets.
                if y > 0.35 {
                    let g = 0.035 * (y - 0.35) / 0.65;
                    let pan = self.rng.bi() * 0.6;
                    self.pad(r + c[1] + 12, attack * 1.6, g, pan, 1);
                }
            }
            DRONE => {
                // Deep work: few voices, low, very slow, nothing that moves.
                let n = 2 + (y * 1.99) as usize;
                for &s in c.iter().take(n) {
                    let pan = self.rng.bi() * 0.35;
                    self.pad(r + s - 12, lerp(9.0, 4.0, x), 0.12, pan, 1);
                }
                self.pad(r + c[0] - 24, lerp(9.0, 4.0, x), 0.1, 0.0, 1);
            }
            PULSE => {
                self.pad(r + c[0] - 12, 2.0, 0.08, -0.2, 1);
                self.pad(r + c[1] - 12, 2.0, 0.05, 0.2, 1);
            }
            KEYS => {
                self.pad(r + c[0] - 12, 4.0, 0.045, 0.0, 1);
                self.pad(r + c[2] - 12, 4.0, 0.03, 0.3, 1);
            }
            RAIN if x > 0.25 => {
                let g = 0.05 * (x - 0.25) / 0.75;
                for &s in c.iter().take(3) {
                    let pan = self.rng.bi() * 0.6;
                    self.pad(r + s, 4.0, g, pan, 1);
                }
            }
            _ => {}
        }
    }

    /// Runs what is due before `end` (in samples).
    fn schedule(&mut self, end: f64) {
        let rate = self.rate as f64;
        let x = self.x;
        let y = self.y;
        let ev = self.events;
        let beat = self.beat as f64;

        if self.t >= self.next_chord {
            if self.next_chord > 0.0 {
                self.chord = if self.rng.f() < x {
                    (self.chord + 1) % 4
                } else {
                    (self.chord + 1 + self.rng.pick(3)) % 4
                };
            }
            self.on_chord();
            let slow = if self.scape == DRONE { 4.0 } else { 1.0 };
            let free = self.rng.range(10.0, 22.0) as f64 * rate * slow;
            let grid = 8.0 * beat * slow;
            self.next_chord = self.t + free + (grid - free) * x as f64;
        }

        // What was planned: the rest of a song, the rest of a finger roll.
        let mut i = 0;
        while i < self.queue.len() {
            if self.queue[i].0 < end {
                let (_, e) = self.queue.swap_remove(i);
                match e {
                    Ev::Chirp {
                        f,
                        to,
                        dur,
                        gain,
                        pan,
                    } => self.chirp(f, to, dur, gain, pan),
                    Ev::Bell { midi, boost } => self.pluck(
                        LEAD,
                        midi,
                        5.0,
                        0.006,
                        0.09 * boost / self.trim.max(0.3),
                        0.0,
                    ),
                }
            } else {
                i += 1;
            }
        }

        match self.scape {
            // Drift has no isolated high notes any more: a bright note two octaves over a dark
            // pad, at a random moment after a silence, is what captures attention most
            // (pitch jump, brightness, onset, unpredictability). Its upper colour is now a
            // held voice that fades in with the chords (see `on_chord`).
            PULSE => {
                // At most about 2–3 onsets a second: 8ths, 16ths only at the very top and
                // softer (speed and loudness together are what hurts reading).
                let sixteenths = y > 0.85;
                while self.next_note < end {
                    let unit = if sixteenths { beat / 4.0 } else { beat / 2.0 };
                    if self.next_note > 0.0 {
                        let tick = (self.next_note / unit).round() as i64;
                        let on_grid = (self.next_note / unit - tick as f64).abs() < 0.01;
                        let c = CHORDS[self.chord];
                        let mut tones = [0i32; 4];
                        for (k, s) in c.iter().enumerate() {
                            tones[k] = self.root + 12 + s;
                        }
                        tones.sort();
                        let (m, vel) = if on_grid {
                            const PAT: [usize; 6] = [0, 1, 2, 3, 2, 1];
                            self.step += 1;
                            let accent = if tick % 8 == 0 {
                                1.0
                            } else if tick % 2 == 0 {
                                0.75
                            } else {
                                0.55
                            };
                            (tones[PAT[self.step % 6]], accent)
                        } else {
                            // Free notes stay in the same octave: no leaps.
                            (tones[self.rng.pick(4)], self.rng.range(0.4, 0.85))
                        };
                        let play = !on_grid || self.rng.f() < lerp(0.45, 0.9, y) * ev.min(1.0);
                        if play {
                            let pan = self.rng.bi() * 0.5;
                            let soft = if sixteenths { 0.75 } else { 1.0 };
                            self.pluck(LEAD, m, lerp(2.2, 1.2, y), 0.02, 0.12 * vel * soft, pan);
                        }
                    }
                    if self.rng.f() < x {
                        self.next_note = ((self.next_note.max(self.t) / unit).floor() + 1.0) * unit;
                    } else {
                        self.next_note = self.next_note.max(self.t)
                            + self.rng.exp(1.0 / (lerp(0.6, 2.2, y) * ev.max(0.1))) as f64 * rate;
                    }
                }
            }
            KEYS => {
                let active = ((3.0 + y * 3.0) * ev.min(1.0)).max(2.0) as usize;
                for i in 0..6 {
                    while self.loops[i].3 < end {
                        let (n, fluid, grid, at) = self.loops[i];
                        if i < active && at >= self.t - rate {
                            let pan = (i as f32 / 5.0) * 1.2 - 0.6;
                            self.pluck(LEAD, self.root + n, lerp(5.5, 3.2, y), 0.025, 0.1, pan);
                        }
                        let free = fluid as f64 * rate / (0.8 + 1.6 * y as f64);
                        let snapped = grid as f64 * beat;
                        self.loops[i].3 = at + free + (snapped - free) * x as f64;
                    }
                }
            }
            RAIN => {
                while self.next_tex < end {
                    if self.next_tex > 0.0 {
                        self.drop();
                    }
                    let rate_s = lerp(3.0, 30.0, y) * ev.max(0.05);
                    let unit = beat / 4.0;
                    self.next_tex = if self.rng.f() < x * 0.6 {
                        ((self.next_tex.max(self.t) / unit).floor() + 1.0) * unit
                    } else {
                        self.next_tex.max(self.t) + self.rng.exp(1.0 / rate_s) as f64 * rate
                    };
                }
            }
            STREAM | FOREST => {
                // Bubbles: a steady flow when structured, in surges when fluid.
                let flow = if self.scape == STREAM {
                    lerp(70.0, 420.0, y)
                } else {
                    25.0
                };
                let surge = 1.0 + (self.gust - 1.0) * 1.5 * (1.0 - x);
                let n = flow * surge.max(0.2) * (end - self.t) as f32 / self.rate;
                let mut k = n.floor() as usize + usize::from(self.rng.f() < n.fract());
                let (lo, hi, g) = if self.scape == STREAM {
                    (350.0, lerp(900.0, 1500.0, self.bright), 0.022)
                } else {
                    (500.0, 1200.0, 0.006)
                };
                while k > 0 {
                    self.bubble(lo, hi, g);
                    k -= 1;
                }
                if self.scape == FOREST {
                    while self.next_note < end {
                        if self.next_note > 0.0 {
                            self.song();
                        }
                        let mean = lerp(9.0, 2.5, y) / ev.max(0.05);
                        self.next_note = self.next_note.max(self.t)
                            + if self.rng.f() < x {
                                mean * 0.8
                            } else {
                                self.rng.exp(mean)
                            } as f64
                                * rate;
                    }
                }
            }
            ASMR => {
                let (t, rate, bright) = (self.t, self.rate, self.bright.min(1.0));
                self.asmr
                    .schedule(&mut self.rng, t, end, rate, x, y, bright);
            }
            _ => {}
        }
    }

    /// A bird: one of three kinds of song, from somewhere among the trees.
    fn song(&mut self) {
        let rate = self.rate as f64;
        if self.rng.f() > self.x * 0.7 {
            self.bird_kind = self.rng.pick(3);
        }
        let pan = self.rng.bi() * 0.85;
        let gain = self.rng.range(0.012, 0.035);
        let mut at = self.t;
        match self.bird_kind {
            0 => {
                // Falling whistles.
                let n = 3 + self.rng.pick(4);
                let mut f = self.rng.range(2600.0, 3800.0);
                for _ in 0..n {
                    let dur = self.rng.range(0.08, 0.14);
                    self.queue.push((
                        at,
                        Ev::Chirp {
                            f,
                            to: f * 0.72,
                            dur,
                            gain,
                            pan,
                        },
                    ));
                    at += (dur + self.rng.range(0.06, 0.12)) as f64 * rate;
                    f *= self.rng.range(0.92, 1.0);
                }
            }
            1 => {
                // A trill.
                let n = 8 + self.rng.pick(7);
                let f = self.rng.range(4000.0, 5200.0);
                for _ in 0..n {
                    self.queue.push((
                        at,
                        Ev::Chirp {
                            f,
                            to: f * 1.12,
                            dur: 0.03,
                            gain: gain * 0.7,
                            pan,
                        },
                    ));
                    at += 0.055 * rate;
                }
            }
            _ => {
                // Up, then down.
                let f = self.rng.range(1800.0, 2400.0);
                self.queue.push((
                    at,
                    Ev::Chirp {
                        f,
                        to: f * 1.4,
                        dur: 0.2,
                        gain,
                        pan,
                    },
                ));
                at += 0.32 * rate;
                self.queue.push((
                    at,
                    Ev::Chirp {
                        f: f * 1.2,
                        to: f * 0.85,
                        dur: 0.24,
                        gain,
                        pan,
                    },
                ));
            }
        }
    }

    // ------------------------------------------------------------------ Audio

    fn control(&mut self) {
        let cr = self.rate / BLOCK as f32;
        let k = coef(1.2, cr);
        self.x += (self.tx - self.x) * k;
        self.y += (self.ty - self.y) * k;
        self.am += (self.tam - self.am) * k;
        self.breath += (self.tbreath - self.breath) * coef(2.0, cr);
        let (x, y) = (self.x, self.y);
        // Evening and night: darker, a little slower.
        let night = clamp01(((self.hour - 13.5).abs() - 3.5) / 6.0);
        self.bright = (0.2 + 0.65 * y) * (1.0 - 0.3 * night) * self.bright_mul;
        let bpm = lerp(56.0, 86.0, y) * (1.0 - 0.08 * night);
        self.beat = 60.0 / bpm * self.rate;
        self.delay.time = (self.beat * 0.75) as usize;
        // ASMR: a small, soft room, so every sound has a short tail and gaps are never dead.
        let t60 = if asmr(self.scape) {
            1.3
        } else {
            lerp(7.5, 3.5, x)
        };
        self.reverb
            .set(t60, lerp(0.35, 0.6, self.bright.min(1.0)), self.rate);
        // Noise bed: where it sits in each sound.
        let target = match self.scape {
            DRIFT => 0.05,
            PULSE => 0.03,
            KEYS => 0.025,
            RAIN => 0.5,
            NOISE => 0.75,
            DRONE => 0.06,
            STREAM => 0.22,
            SHORE => 0.8,
            FOREST => 0.12,
            SILENCE => 0.0,
            // ASMR: the room's own quiet hiss, under the handling sound.
            _ => 0.02,
        };
        self.bed_level += (target - self.bed_level) * coef(2.0, cr);
        // Every sound about as loud as the others (measured with bench.mjs, ~-20 dBFS).
        let trim = match self.scape {
            DRIFT => 0.65,
            PULSE => 1.0,
            KEYS => 1.2,
            RAIN => 0.55,
            NOISE => 0.5,
            DRONE => 0.65,
            STREAM => 1.35,
            SHORE => 0.6,
            FOREST => 2.4,
            // Sparse sounds: matched by their peaks (about -8 dBFS), not their RMS.
            // Soft by design (the studies played it at 40–45 dBA), a few dB under the rest.
            ASMR => 1.5,
            _ => 1.0,
        };
        self.trim += (trim - self.trim) * coef(1.5, cr);
        // Gusts (Rain, Forest, Stream) and waves (Noise) when fluid; steady when structured.
        if self.rng.f() < 1.0 / (cr * 3.0) {
            self.gust_target = 1.0 + self.rng.bi() * 0.45 * (1.0 - x);
        }
        self.gust += (self.gust_target - self.gust) * coef(2.5, cr);
    }

    /// Fills `out` (interleaved stereo) with `out.len() / 2` frames.
    pub fn render(&mut self, out: &mut [f32]) {
        for chunk in out.chunks_mut(BLOCK * 2) {
            self.control();
            let frames = chunk.len() / 2;
            let end = self.t + frames as f64;
            self.schedule(end);
            self.render_block(chunk);
            self.t = end;
        }
    }

    fn render_block(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        let rate = self.rate;
        let (x, y) = (self.x, self.y);
        let mut pad = [[0.0f32; 2]; BLOCK];
        let mut lead = [[0.0f32; 2]; BLOCK];
        let mut tex = [[0.0f32; 2]; BLOCK];
        let mut close = [[0.0f32; 2]; BLOCK];

        // Voices.
        let sine = &self.sine;
        let look = |ph: f32| {
            let pos = ph * TABLE as f32;
            let i = pos as usize;
            sine[i] + (sine[i + 1] - sine[i]) * (pos - i as f32)
        };
        for v in self.voices.iter_mut().filter(|v| v.on) {
            let bus = match v.bus {
                PAD => &mut pad,
                LEAD => &mut lead,
                TEX => &mut tex,
                _ => &mut close,
            };
            for frame in bus.iter_mut().take(frames) {
                let k = if v.releasing { v.rel } else { v.att };
                v.env += (v.target - v.env) * k;
                let mut s = 0.0;
                for p in 0..v.np {
                    s += look(v.ph[p]) * v.amp[p];
                    v.ph[p] += v.inc[p];
                    if v.ph[p] >= 1.0 {
                        v.ph[p] -= 1.0;
                    }
                    v.amp[p] *= v.dec[p];
                    v.inc[p] = (v.inc[p] * v.glide).min(0.45);
                }
                let s = s * v.env * v.gain;
                frame[0] += s * v.pl;
                frame[1] += s * v.pr;
            }
            let loudest = v.amp[..v.np].iter().fold(0.0f32, |m, a| m.max(*a));
            if (v.releasing && v.env < 1e-4) || loudest < 1e-4 {
                v.on = false;
            }
        }
        for b in self.bubbles.iter_mut().filter(|b| b.on) {
            for frame in tex.iter_mut().take(frames) {
                let s = look(b.ph) * b.amp;
                b.ph += b.inc;
                if b.ph >= 1.0 {
                    b.ph -= 1.0;
                }
                b.inc = (b.inc * b.glide).min(0.45);
                b.amp *= b.dec;
                frame[0] += s * b.pl;
                frame[1] += s * b.pr;
            }
            if b.amp < 1e-5 {
                b.on = false;
            }
        }
        // ASMR textures, the handling sound and the distance, over the taps.
        let active = asmr(self.scape);
        self.asmr.render(
            &mut self.rng,
            self.t,
            rate,
            active,
            x,
            self.bright.min(1.0),
            &mut close,
            frames,
        );

        // Per-block coefficients.
        let br = self.bright.min(1.0);
        self.pad_lfo += BLOCK as f32 / rate * 0.035;
        if self.pad_lfo > 1.0 {
            self.pad_lfo -= 1.0;
        }
        let lfo = (self.pad_lfo * 2.0 * PI).sin() * 0.3 * (1.0 - x);
        let pad_co = SvfCo::new(
            lerp(450.0, 2400.0, br * br)
                * (1.0 + lfo)
                * if self.scape == DRONE { 0.6 } else { 1.0 },
            0.6,
            rate,
        );
        // Shore: a wave every ~10 s when structured (6 a minute, a breath), irregular when fluid.
        let wave_inc = 1.0 / (self.wave_period * rate);
        let (bed_cut, mix) = match self.scape {
            RAIN => (lerp(900.0, 5200.0, br), [0.25, 0.75, 0.6]),
            NOISE => {
                let c = lerp(380.0, 9000.0, y.powf(1.6)) * (1.0 + (self.gust - 1.0) * 0.8);
                let w = if y < 0.5 {
                    [0.0, y * 2.0, (1.0 - y * 2.0) * 2.0]
                } else {
                    [(y - 0.5) * 1.2, 1.0 - (y - 0.5) * 1.4, 0.0]
                };
                (c, w)
            }
            STREAM => (lerp(1800.0, 4200.0, br), [0.15, 0.8, 0.2]),
            SHORE => (lerp(500.0, 2600.0, br), [0.1, 0.7, 0.8]),
            FOREST => (lerp(600.0, 1400.0, br) * self.gust, [0.0, 0.8, 0.5]),
            _ => (lerp(500.0, 2500.0, br), [0.0, 0.6, 0.6]),
        };
        let bed_co = SvfCo::new(bed_cut, 0.55, rate);
        let shore = self.scape == SHORE;
        let gusty = matches!(self.scape, RAIN | NOISE | FOREST);
        let bed_gain = self.bed_level * if gusty { self.gust } else { 1.0 };
        let hp = coef(1.0 / (2.0 * PI * 55.0), rate);
        let rev_hp = coef(1.0 / (2.0 * PI * 200.0), rate);
        let pulse_depth = self.pulse
            * if self.scape == DRIFT {
                0.35 * x * x
            } else if self.scape == DRONE {
                0.15 * x * x
            } else {
                0.0
            };
        let pulse_inc = 1.0 / self.beat;
        // Shallow: depth matters more than rate for annoyance.
        let am_depth = self.am * lerp(0.2, 0.4, y);
        let am_inc = 16.0 / rate;
        let am_co = SvfCo::new(450.0, 0.55, rate);
        let breath_inc = 0.1 / rate;
        let breath_depth = self.breath * 0.35;
        let send = if asmr(self.scape) {
            0.5
        } else {
            lerp(0.55, 0.3, x)
        };
        let wet = lerp(0.9, 0.55, x);
        let fb = lerp(0.2, 0.42, x);
        // The echo multiplies onsets: kept low.
        let echo = lerp(0.06, 0.18, x);
        // Notes never brighter than ~2.6 kHz: low-passed before the echo.
        let lead_co = SvfCo::new(lerp(1400.0, 2600.0, br), 0.6, rate);
        let vk = coef(0.08, rate);

        for (f, o) in out.chunks_mut(2).enumerate() {
            // Noise bed.
            let wl = self.rng.bi();
            let wr = self.rng.bi();
            let (w0, p0, b0) = self.nl.next(wl);
            let (w1, p1, b1) = self.nr.next(wr);
            let mut nl = self.bed_svf[0].lp(w0 * mix[0] + p0 * mix[1] + b0 * mix[2], &bed_co);
            let mut nr = self.bed_svf[1].lp(w1 * mix[0] + p1 * mix[1] + b1 * mix[2], &bed_co);
            self.bed_hp[0] += (nl - self.bed_hp[0]) * hp;
            self.bed_hp[1] += (nr - self.bed_hp[1]) * hp;
            let mut g = bed_gain;
            if shore {
                self.wave_ph += wave_inc;
                if self.wave_ph >= 1.0 {
                    self.wave_ph -= 1.0;
                    self.wave_period = lerp(self.rng.range(7.0, 13.0), 10.0, x);
                }
                // Rises over 40 %, falls slowly after.
                let p = self.wave_ph;
                let env = if p < 0.4 {
                    (p / 0.4 * PI * 0.5).sin().powi(2)
                } else {
                    (-(p - 0.4) * 4.0).exp()
                };
                g *= 0.22 + 0.78 * env;
            }
            nl = (nl - self.bed_hp[0]) * g;
            nr = (nr - self.bed_hp[1]) * g;

            // Pads: filtered, swelling with the beat when structured.
            self.pulse_ph += pulse_inc;
            if self.pulse_ph >= 1.0 {
                self.pulse_ph -= 1.0;
            }
            let swell = 1.0 - pulse_depth * (1.0 - (self.pulse_ph * 2.0 * PI).cos()) * 0.5;
            let pl = self.pad_svf[0].lp(pad[f][0], &pad_co) * swell;
            let pr = self.pad_svf[1].lp(pad[f][1], &pad_co) * swell;

            // Lead: ping-pong echo.
            let l0 = self.lead_svf[0].lp(lead[f][0], &lead_co);
            let r0 = self.lead_svf[1].lp(lead[f][1], &lead_co);
            let (el, er) = self.delay.process((l0 + r0) * 0.5, fb);
            let ll = l0 + el * echo;
            let lr = r0 + er * echo;

            let dry_l = pl + ll + tex[f][0] + nl + close[f][0];
            let dry_r = pr + lr + tex[f][1] + nr + close[f][1];
            // The reverb hears nothing below ~200 Hz: its low modes otherwise rang on some bass
            // notes (one 55 Hz pad came out 23 dB above its level, measured), so the level
            // jumped with each chord. Basses stay dry, as in any long hall mix.
            let in_l = (pl + ll * 1.3 + tex[f][0] + close[f][0] * 0.3) * send + nl * 0.08;
            let in_r = (pr + lr * 1.3 + tex[f][1] + close[f][1] * 0.3) * send + nr * 0.08;
            self.rev_lp[0] += (in_l - self.rev_lp[0]) * rev_hp;
            self.rev_lp[1] += (in_r - self.rev_lp[1]) * rev_hp;
            let (rl, rr) = self
                .reverb
                .process(in_l - self.rev_lp[0], in_r - self.rev_lp[1]);
            let mut l = dry_l + rl * wet;
            let mut r = dry_r + rr * wet;

            // Focus modulation, 16 Hz, on the 200 Hz – 1 kHz band only (Woods et al. 2024).
            self.am_ph += am_inc;
            if self.am_ph >= 1.0 {
                self.am_ph -= 1.0;
            }
            let m = am_depth * (1.0 - (self.am_ph * 2.0 * PI).cos()) * 0.5;
            l -= self.am_svf[0].run(l, &am_co).1 * m;
            r -= self.am_svf[1].run(r, &am_co).1 * m;

            // Breathing: six a minute, 4 s in (the sound rises), 6 s out (it falls).
            self.breath_ph += breath_inc;
            if self.breath_ph >= 1.0 {
                self.breath_ph -= 1.0;
            }
            let p = self.breath_ph;
            let s = if p < 0.4 {
                (p / 0.4 * PI * 0.5).sin().powi(2)
            } else {
                ((p - 0.4) / 0.6 * PI * 0.5).cos().powi(2)
            };
            let b = 1.0 - breath_depth * (1.0 - s);

            self.vol += (self.tvol - self.vol) * vk;
            let g = b * self.vol * 1.6 * self.trim;
            l *= g;
            r *= g;
            self.dc[0] += (l - self.dc[0]) * 0.0005;
            self.dc[1] += (r - self.dc[1]) * 0.0005;
            l -= self.dc[0];
            r -= self.dc[1];
            o[0] = soft(l);
            o[1] = soft(r);
        }
    }
}

/// Gentle limiter: linear below 0.5, never above 1.
fn soft(x: f32) -> f32 {
    let a = x.abs();
    if a < 0.5 {
        x
    } else {
        x.signum() * (0.5 + 0.5 * ((a - 0.5) * 2.0).tanh())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    /// Renders `secs` of sound and returns (RMS dBFS, peak dBFS), checking every sample.
    fn play(e: &mut Engine, secs: f32) -> (f32, f32) {
        let mut buf = vec![0.0f32; (secs * RATE) as usize * 2];
        e.render(&mut buf);
        let mut sum = 0.0f64;
        let mut peak = 0.0f32;
        for v in &buf {
            assert!(v.is_finite(), "a sample is not a number");
            sum += (*v as f64) * (*v as f64);
            peak = peak.max(v.abs());
        }
        let rms = (sum / buf.len() as f64).sqrt() as f32;
        (20.0 * rms.max(1e-9).log10(), 20.0 * peak.max(1e-9).log10())
    }

    #[test]
    fn every_sound_plays_within_bounds_at_the_corners_of_the_pad() {
        for s in 0..SCAPES.len() as u8 {
            for (x, y) in [(0.5, 0.5), (0.0, 0.0), (1.0, 1.0)] {
                let mut e = Engine::new(RATE, 7 + s as u64);
                e.set_scape(s);
                e.set_pad(x, y);
                e.set_volume(0.6);
                play(&mut e, 3.0);
                let (rms, peak) = play(&mut e, 6.0);
                assert!(
                    peak <= 0.0,
                    "{} at ({x}, {y}) clips: {peak} dBFS",
                    SCAPES[s as usize]
                );
                if s != 13 {
                    assert!(
                        rms > -60.0,
                        "{} at ({x}, {y}) is silent: {rms} dBFS",
                        SCAPES[s as usize]
                    );
                }
            }
        }
    }

    #[test]
    fn silence_is_silent_and_a_bell_still_rings() {
        let mut e = Engine::new(RATE, 3);
        e.set_scape(13);
        e.set_volume(0.6);
        let (quiet, _) = play(&mut e, 8.0);
        assert!(quiet < -60.0, "Silence plays {quiet} dBFS");
        e.chime(1.0);
        let (_, peak) = play(&mut e, 2.0);
        assert!(peak > -40.0, "the bell is not heard: {peak} dBFS");
    }

    #[test]
    fn volume_zero_fades_to_nothing() {
        let mut e = Engine::new(RATE, 5);
        e.set_volume(0.8);
        play(&mut e, 4.0);
        e.set_volume(0.0);
        play(&mut e, 1.0);
        let (rms, _) = play(&mut e, 1.0);
        assert!(rms < -80.0, "still {rms} dBFS a second after volume 0");
    }

    #[test]
    fn every_asmr_family_switched_on_is_heard() {
        let mut e = Engine::new(RATE, 9);
        e.set_scape(9);
        e.set_asmr_mask((1 << ASMR_FAMILIES.len()) - 1);
        e.set_volume(0.6);
        play(&mut e, 120.0);
        for (k, name) in ASMR_FAMILIES.iter().enumerate() {
            assert!(e.asmr_count(k) > 0, "{name} never played in two minutes");
        }
    }

    #[test]
    fn an_asmr_family_switched_on_comes_in_at_once() {
        let mut e = Engine::new(RATE, 4);
        e.set_scape(9);
        e.set_asmr_mask(ASMR_DEFAULT_MASK);
        play(&mut e, 30.0);
        let paper = ASMR_FAMILIES.iter().position(|f| *f == "Paper").unwrap();
        let before = e.asmr_count(paper);
        e.set_asmr_mask(ASMR_DEFAULT_MASK | (1 << paper));
        play(&mut e, 2.0);
        assert!(
            e.asmr_count(paper) > before,
            "paper did not start within 2 s"
        );
    }

    #[test]
    fn a_switched_off_asmr_family_never_plays() {
        let mut e = Engine::new(RATE, 12);
        e.set_scape(9);
        let paper = ASMR_FAMILIES.iter().position(|f| *f == "Paper").unwrap();
        e.set_asmr_mask(ASMR_DEFAULT_MASK);
        play(&mut e, 60.0);
        assert_eq!(e.asmr_count(paper), 0, "paper is off by default");
    }
}
