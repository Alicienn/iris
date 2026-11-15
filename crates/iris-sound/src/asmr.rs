//! ASMR, v3: one generative, non-vocal ASMR soundscape.
//!
//! What the research asks for:
//! - closeness and movement around the head are the strongest tingle cues: every gesture
//!   has a place and a path (by one ear, ear to ear, coming closer, circling), rendered
//!   for headphones with a spherical-head model (Brown & Duda 1998: a delay and a head-
//!   shadow filter per ear), the near-field level difference between the ears (Brungart &
//!   Rabinowitz 1999) and a close microphone's bass lift (proximity effect);
//! - a dark sound (energy below ~1.5 kHz, lift at 40–80 Hz) with sparse, fluctuating
//!   crisp detail on top, and no steady energy around 5 kHz (a gentle dip there);
//! - soft overall, with brief swells; slightly offset layers of crisp sounds;
//! - variety: several trigger families, each played for a while in many variants, then a
//!   crossfade to the next; the listener can switch any family off;
//! - no voice, no mouth sounds (misophonia).
//!
//! Synthesis, per family (procedural, no samples):
//! - taps: a bank of resonant modes (wood, plastic, glass, cardboard) struck by a raised-
//!   cosine force pulse — long for a fingertip, short for a nail (FoleyAutomatic);
//! - scratching: noise in strokes back and forth, through the surface's resonances;
//! - crinkling: random clicks with power-law energies (crumpling statistics) through the
//!   sheet's resonances, density following the hands' squeezes (PhISEM-like);
//! - brushing: band noise swept by the stroke, a soft low whoosh, sparse bristle ticks;
//! - water: bubbles whose pitch follows their size and rises as they ring (Minnaert);
//! - sand / foam: dense tiny grains, crisp, over a soft low squish, in squeezes.

use crate::{clamp01, coef, lerp, smooth, window, Rng, Svf, SvfCo};
use std::f32::consts::PI;

pub(crate) const FAMILIES: usize = 7;
const TAP: usize = 0;
const SCRATCH: usize = 1;
const CRINKLE: usize = 2;
const BRUSH: usize = 3;
const WATER: usize = 4;
const SAND: usize = 5;
const PAGE: usize = 6;
/// Level of each family, measured so that they sit together (see bench).
const LEVEL: [f32; FAMILIES] = [0.26, 0.2, 1.3, 0.5, 0.42, 1.7, 0.36];
/// On by default: the triggers that work for most and bother the fewest. Crinkling is
/// opt-in (one of the sounds misophonic listeners name most).
pub(crate) const DEFAULT_MASK: u32 =
    (1 << TAP) | (1 << SCRATCH) | (1 << BRUSH) | (1 << WATER) | (1 << SAND) | (1 << PAGE);

const HEAD: f32 = 0.0875;
const SOUND: f32 = 343.0;
const MAX_GESTURES: usize = 12;
const DELAY: usize = 64;

/// One resonant mode: a two-pole resonator.
#[derive(Clone, Copy, Default)]
struct Mode {
    a1: f32,
    a2: f32,
    g: f32,
    y1: f32,
    y2: f32,
}
impl Mode {
    fn set(&mut self, f: f32, decay: f32, gain: f32, rate: f32) {
        let f = f.min(rate * 0.45);
        let r = (-1.0 / (decay.max(0.001) * rate)).exp();
        let w = 2.0 * PI * f / rate;
        self.a1 = 2.0 * r * w.cos();
        self.a2 = -r * r;
        self.g = gain * w.sin();
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.g * x + self.a1 * self.y1 + self.a2 * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// A bubble: a sine whose pitch rises a little as it dies.
#[derive(Clone, Copy, Default)]
struct Bubble {
    on: bool,
    ph: f32,
    inc: f32,
    glide: f32,
    amp: f32,
    dec: f32,
}

/// Where a gesture is, and how it moves: azimuth (degrees, 0 in front, +90 right ear,
/// ±180 behind) and distance from the centre of the head (m; the ear is at 8.75 cm).
#[derive(Clone, Copy, Default)]
struct Path {
    az0: f32,
    az1: f32,
    r0: f32,
    r1: f32,
}

/// Headphone rendering of one mono source near the head.
#[derive(Clone, Copy)]
struct Space {
    buf: [f32; DELAY],
    pos: usize,
    // Per ear: delay (samples), gain, head-shadow filter (b0, b1, a1) and its state,
    // proximity shelf (coefficient, gain) and its state. Index 0 left, 1 right.
    d: [f32; 2],
    d1: [f32; 2],
    g: [f32; 2],
    g1: [f32; 2],
    hs: [[f32; 3]; 2],
    hx: [f32; 2],
    hy: [f32; 2],
    sk: [f32; 2],
    sg: [f32; 2],
    sl: [f32; 2],
}
impl Default for Space {
    fn default() -> Self {
        Space {
            buf: [0.0; DELAY],
            pos: 0,
            d: [0.0; 2],
            d1: [0.0; 2],
            g: [0.0; 2],
            g1: [0.0; 2],
            hs: [[1.0, 0.0, 0.0]; 2],
            hx: [0.0; 2],
            hy: [0.0; 2],
            sk: [0.0; 2],
            sg: [1.0; 2],
            sl: [0.0; 2],
        }
    }
}
impl Space {
    /// Sets where the source is for this block.
    fn place(&mut self, az: f32, r: f32, rate: f32, first: bool) {
        let r = r.max(HEAD + 0.015);
        let (sx, sy) = ((az * PI / 180.0).sin() * r, (az * PI / 180.0).cos() * r);
        let w0 = SOUND / HEAD;
        let tk = 2.0 * rate / (2.0 * w0);
        for e in 0..2 {
            let side = if e == 0 { -1.0 } else { 1.0 };
            // Angle between the source and this ear's axis (ears at ±100°).
            let mut th = (az - side * 100.0).abs() % 360.0;
            if th > 180.0 {
                th = 360.0 - th;
            }
            // Head shadow (Brown & Duda): one pole, one zero moving with the angle.
            let alpha = (1.0 + 0.05) + (1.0 - 0.05) * (th / 150.0 * PI).cos();
            let n = 1.0 + tk;
            self.hs[e] = [
                (1.0 + alpha * tk) / n,
                (1.0 - alpha * tk) / n,
                (1.0 - tk) / n,
            ];
            // Woodworth delay, offset so it never goes negative.
            let t = if th < 90.0 {
                -(HEAD / SOUND) * (th * PI / 180.0).cos()
            } else {
                (HEAD / SOUND) * ((th - 90.0) * PI / 180.0)
            };
            self.d1[e] = (t + HEAD / SOUND) * rate + 1.0;
            // Distance to this ear: the near-field level difference, all frequencies.
            let dist = ((sx - side * HEAD).powi(2) + sy * sy).sqrt().max(0.015);
            self.g1[e] = (0.06 / dist).min(2.0);
            // A close microphone's bass lift: corner and gain from the distance.
            let fc = (0.5 * SOUND / (2.0 * PI * dist)).clamp(120.0, 1500.0);
            let db = (10.0 * (1.0 + 0.25 * (SOUND / (2.0 * PI * 100.0 * dist)).powi(2)).log10())
                .min(9.0);
            self.sk[e] = coef(1.0 / (2.0 * PI * fc), rate);
            self.sg[e] = 10f32.powf(db / 20.0) - 1.0;
        }
        if first {
            self.d = self.d1;
            self.g = self.g1;
        }
    }

    /// Renders `x` (one sample, mono) to the two ears; `k` moves delay and gain smoothly.
    fn tick(&mut self, x: f32, k: f32) -> (f32, f32) {
        self.buf[self.pos] = x;
        let mut out = [0.0; 2];
        for (e, o) in out.iter_mut().enumerate() {
            self.d[e] += (self.d1[e] - self.d[e]) * k;
            self.g[e] += (self.g1[e] - self.g[e]) * k;
            let d = self.d[e].clamp(1.0, (DELAY - 2) as f32);
            let i = d as usize;
            let fr = d - i as f32;
            let a = self.buf[(self.pos + DELAY - i) % DELAY];
            let b = self.buf[(self.pos + DELAY - i - 1) % DELAY];
            let s = a + (b - a) * fr;
            let [b0, b1, a1] = self.hs[e];
            let y = b0 * s + b1 * self.hx[e] - a1 * self.hy[e];
            self.hx[e] = s;
            self.hy[e] = y;
            self.sl[e] += (y - self.sl[e]) * self.sk[e];
            *o = (y + self.sl[e] * self.sg[e]) * self.g[e];
        }
        self.pos = (self.pos + 1) % DELAY;
        (out[0], out[1])
    }
}

/// One gesture: a family, a sound, a place and a path, a life with a smooth envelope.
#[derive(Clone, Copy)]
struct Gesture {
    on: bool,
    kind: usize,
    start: f64,
    len: f64,
    level: f32,
    path: Path,
    space: Space,
    modes: [Mode; 6],
    nm: usize,
    // Taps: when (samples after start), how hard, how long the contact (samples).
    taps: [(f32, f32, f32); 14],
    ntaps: usize,
    tap_i: usize,
    pulse_t: f32,
    pulse_len: f32,
    pulse_amp: f32,
    // Continuous families: filters, rates, phases, slow controls.
    svf: [Svf; 3],
    co: [SvfCo; 3],
    mix: [f32; 3],
    rate: f32,
    ph: f32,
    stroke: f32,
    squeeze: f32,
    squeeze_t: f32,
    next_change: f64,
    lp: f32,
    c0: f32,
    c1: f32,
    bubbles: [Bubble; 12],
}
impl Default for Gesture {
    fn default() -> Self {
        Gesture {
            on: false,
            kind: 0,
            start: 0.0,
            len: 1.0,
            level: 1.0,
            path: Path::default(),
            space: Space::default(),
            modes: [Mode::default(); 6],
            nm: 0,
            taps: [(0.0, 0.0, 1.0); 14],
            ntaps: 0,
            tap_i: 0,
            pulse_t: 0.0,
            pulse_len: 1.0,
            pulse_amp: 0.0,
            svf: [Svf::default(); 3],
            co: [SvfCo::default(); 3],
            mix: [0.0; 3],
            rate: 0.0,
            ph: 0.0,
            stroke: 1.0,
            squeeze: 0.0,
            squeeze_t: 0.0,
            next_change: 0.0,
            lp: 0.0,
            c0: 1000.0,
            c1: 1000.0,
            bubbles: [Bubble::default(); 12],
        }
    }
}

/// A lane plays one family for a while, then moves to the one heard least recently.
/// Several lanes run side by side, so every family switched on is actually heard.
#[derive(Clone, Copy, Default)]
struct Lane {
    on: bool,
    kind: usize,
    start: f64,
    end: f64,
    next: f64,
}

pub(crate) struct Asmr {
    gestures: [Gesture; MAX_GESTURES],
    /// Which families may play (bit per family).
    pub(crate) mask: u32,
    /// The mask last seen, to bring in at once a family just switched on.
    seen: u32,
    lanes: [Lane; 3],
    /// When each family last started playing (samples), to rotate fairly.
    last: [f64; FAMILIES],
    /// Gestures started per family (for checking that each one switched on plays).
    pub(crate) spawned: [u32; FAMILIES],
    side: f32,
    // Under everything: hands near the microphone, the distance breathing.
    hand: f32,
    hand_target: f32,
    hand_next: f64,
    hand_svf: [Svf; 2],
    near: f32,
    near_target: f32,
    near_next: f64,
    // The bus: a gentle dip around 5 kHz, a high-pass under 35 Hz.
    dip: [Svf; 2],
    hp: [f32; 2],
}

impl Default for Asmr {
    fn default() -> Self {
        Asmr {
            gestures: [Gesture::default(); MAX_GESTURES],
            mask: DEFAULT_MASK,
            seen: u32::MAX,
            lanes: [Lane::default(); 3],
            last: [-1.0; FAMILIES],
            spawned: [0; FAMILIES],
            side: 1.0,
            hand: 0.0,
            hand_target: 0.0,
            hand_next: 0.0,
            hand_svf: [Svf::default(); 2],
            near: 1.0,
            near_target: 1.0,
            near_next: 0.0,
            dip: [Svf::default(); 2],
            hp: [0.0; 2],
        }
    }
}

impl Asmr {
    fn enabled(&self, k: usize) -> bool {
        self.mask & (1 << k) != 0
    }
    /// The family heard least recently that no other lane is playing.
    fn pick_family(&self, rng: &mut Rng, lane: usize, want: usize) -> usize {
        let busy =
            |k: usize| (0..want).any(|i| i != lane && self.lanes[i].on && self.lanes[i].kind == k);
        let mut best = None;
        let mut best_t = f64::MAX;
        for k in (0..FAMILIES).filter(|&k| self.enabled(k) && !busy(k)) {
            // A little randomness so equal ages do not always resolve the same way.
            let age = self.last[k] + rng.f() as f64;
            if age < best_t {
                best_t = age;
                best = Some(k);
            }
        }
        best.unwrap_or_else(|| (0..FAMILIES).find(|&k| self.enabled(k)).unwrap_or(TAP))
    }

    /// Plans gestures up to `end` (samples). `x`: still (0) → around you (1);
    /// `y`: sparse (0) → dense (1).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn schedule(
        &mut self,
        rng: &mut Rng,
        t: f64,
        end: f64,
        rate: f32,
        x: f32,
        y: f32,
        bright: f32,
    ) {
        let r = rate as f64;
        if self.mask == 0 {
            return;
        }
        // One lane for one family, two for two or three, three from four on: what is
        // switched on is heard (two or three triggers at once is what most prefer).
        let want = match self.mask.count_ones() {
            1 => 1,
            2 | 3 => 2,
            _ => 3,
        };
        for lane in self.lanes.iter_mut().skip(want) {
            lane.on = false;
        }
        // A family just switched on comes in at once, in place of the lane playing longest.
        let fresh = if self.seen == u32::MAX {
            0
        } else {
            self.mask & !self.seen
        };
        self.seen = self.mask;
        for k in (0..FAMILIES).filter(|&k| fresh & (1 << k) != 0) {
            let i = (0..want).find(|&i| !self.lanes[i].on).unwrap_or_else(|| {
                (0..want)
                    .min_by(|&a, &b| self.lanes[a].start.total_cmp(&self.lanes[b].start))
                    .unwrap_or(0)
            });
            let secs = lerp(90.0, 40.0, y) * rng.range(0.7, 1.3);
            self.lanes[i] = Lane {
                on: true,
                kind: k,
                start: t,
                end: t + secs as f64 * r,
                next: t + 0.2 * r,
            };
            self.last[k] = t;
        }
        for i in 0..want {
            let mut lane = self.lanes[i];
            if !lane.on || t >= lane.end || !self.enabled(lane.kind) {
                let first = !lane.on;
                lane.kind = self.pick_family(rng, i, want);
                self.last[lane.kind] = t;
                // Each family stays 40–90 s on its lane; lanes start staggered so they
                // never all change at once.
                // Shorter when many families are on, so each comes round within ~1.5 min.
                let share = (want as f32 / self.mask.count_ones() as f32).sqrt();
                let secs = lerp(90.0, 40.0, y)
                    * share
                    * rng.range(0.7, 1.3)
                    * if first { 1.0 - 0.3 * i as f32 } else { 1.0 };
                lane.start = t;
                lane.end = t + secs as f64 * r;
                lane.next = lane
                    .next
                    .max(t + if first { i as f64 * 0.7 * r } else { 0.0 });
                lane.on = true;
            }
            while lane.next < end {
                // In over 3 s, out over 3 s: lanes crossfade, nothing starts at full.
                let a = ((lane.next - lane.start) / r) as f32;
                let b = ((lane.end - lane.next) / r) as f32;
                let fade = smooth(a / 3.0) * smooth(b / 3.0);
                let weight = if i == 0 { 1.0 } else { 0.8 };
                let level = rng.range(0.7, 1.0) * lerp(0.5, 1.0, fade) * weight;
                self.spawn(rng, lane.kind, lane.next, rate, x, y, bright, level);
                // Gestures at a fairly steady pace, ~0.6 a second overall in the middle:
                // each lane sparser when there are more of them. Now and then a pause.
                let mean = lerp(2.2, 0.8, y) * lerp(1.0, 1.8, (want - 1) as f32 / 2.0);
                let gap = if rng.f() < 0.1 {
                    mean * rng.range(1.8, 3.0)
                } else {
                    mean * rng.range(0.7, 1.35)
                };
                lane.next += gap as f64 * r;
            }
            self.lanes[i] = lane;
        }
    }

    /// A new gesture of family `k`.
    #[allow(clippy::too_many_arguments)]
    fn spawn(
        &mut self,
        rng: &mut Rng,
        k: usize,
        at: f64,
        rate: f32,
        x: f32,
        y: f32,
        bright: f32,
        level: f32,
    ) {
        // A free slot; if none, the one nearest its end (already fading), never a gesture
        // in full swing. A family is never silently dropped because others are busy.
        let i = match self.gestures.iter().position(|g| !g.on) {
            Some(i) => i,
            None => {
                let progress = |g: &Gesture| (at - g.start) / g.len;
                let (i, p) = self
                    .gestures
                    .iter()
                    .enumerate()
                    .map(|(i, g)| (i, progress(g)))
                    .fold((0, f64::MIN), |a, b| if b.1 > a.1 { b } else { a });
                if p < 0.7 {
                    return;
                }
                i
            }
        };
        self.spawned[k] += 1;
        let mut g = Gesture {
            on: true,
            kind: k,
            start: at,
            level: level * LEVEL[k],
            ..Gesture::default()
        };
        let r = rate as f64;
        // Inside a gesture, timing is always a little loose (hands, not a sequencer).
        let fluid = 0.6;
        // Where, and how it moves. `x` is "still" (0) → "around you" (1): movement around
        // the head raises tingles but lowers comfort, so it is the listener's call.
        self.side = if rng.f() < 0.5 {
            -self.side
        } else {
            rng.bi().signum()
        };
        let s = self.side;
        let near = rng.range(0.105, 0.16);
        let moving = rng.f() < lerp(0.15, 0.9, x);
        g.path = match if moving { 1 + rng.pick(4) } else { 0 } {
            0 => {
                let az = s * rng.range(55.0, 115.0);
                Path {
                    az0: az,
                    az1: az + rng.bi() * 20.0,
                    r0: near,
                    r1: near + rng.bi().abs() * 0.02,
                }
            }
            1 => Path {
                az0: -s * rng.range(70.0, 100.0),
                az1: s * rng.range(70.0, 100.0),
                r0: near + 0.03,
                r1: near + 0.03,
            },
            2 => {
                let az = s * rng.range(40.0, 120.0);
                Path {
                    az0: az,
                    az1: az,
                    r0: rng.range(0.3, 0.45),
                    r1: near,
                }
            }
            3 => {
                let az = rng.range(-180.0, 180.0);
                Path {
                    az0: az,
                    az1: az + s * rng.range(180.0, 300.0),
                    r0: near + 0.04,
                    r1: near + 0.04,
                }
            }
            _ => Path {
                az0: s * rng.range(170.0, 190.0),
                az1: s * rng.range(100.0, 150.0),
                r0: near + 0.02,
                r1: near,
            },
        };
        match k {
            TAP => {
                // Material: wood, plastic, glass, cardboard; fingertip or nail.
                // Mostly fingertips on wood and cardboard (soft); plastic, nails and glass
                // now and then (crisper, and bothersome to more people).
                let m = rng.f();
                let (f0, ratios, decays, nail): (f32, [f32; 6], [f32; 6], bool) = if m < 0.42 {
                    (
                        rng.range(280.0, 900.0),
                        [1.0, 2.3, 3.9, 5.2, 6.8, 8.1],
                        [0.06, 0.04, 0.03, 0.022, 0.016, 0.012],
                        rng.f() < 0.2,
                    )
                } else if m < 0.7 {
                    (
                        rng.range(140.0, 340.0),
                        [1.0, 1.5, 2.2, 3.1, 4.4, 5.9],
                        [0.05, 0.04, 0.03, 0.025, 0.02, 0.015],
                        false,
                    )
                } else if m < 0.9 {
                    (
                        rng.range(700.0, 1900.0),
                        [1.0, 1.7, 2.9, 4.1, 5.6, 7.0],
                        [0.04, 0.03, 0.022, 0.016, 0.012, 0.009],
                        rng.f() < 0.4,
                    )
                } else {
                    (
                        rng.range(1300.0, 3000.0),
                        [1.0, 2.76, 5.4, 8.9, 1.5, 3.9],
                        [0.45, 0.25, 0.12, 0.06, 0.18, 0.1],
                        true,
                    )
                };
                let fb = f0 * lerp(0.85, 1.1, bright);
                g.nm = 6;
                for m in 0..6 {
                    let amp = 1.0 / (1.0 + m as f32 * 0.7) * rng.range(0.6, 1.0);
                    g.modes[m].set(
                        fb * ratios[m] * rng.range(0.98, 1.02),
                        decays[m] * rng.range(0.8, 1.25),
                        amp,
                        rate,
                    );
                }
                // Pattern: roll, gallop, two fingers, slow taps, a flurry.
                let n = match rng.pick(5) {
                    0 => 4 + rng.pick(8),
                    1 => 6 + rng.pick(6),
                    2 => 4 + rng.pick(6),
                    3 => 2 + rng.pick(3),
                    _ => 8 + rng.pick(6),
                }
                .min(14);
                let mut step = lerp(0.2, 0.08, y) * rng.range(0.6, 1.5) * rate;
                let shape = rng.pick(4);
                let mut at_s = 0.0;
                let tau = if nail {
                    rng.range(0.00012, 0.0004)
                } else {
                    rng.range(0.0012, 0.004)
                } * rate;
                let peak = rng.range(0.2, 0.8);
                for (j, tap) in g.taps.iter_mut().enumerate().take(n) {
                    let u = (j as f32 + 0.5) / n as f32;
                    let contour = 0.4 + 0.6 * (1.0 - ((u - peak) / 0.6).powi(2)).max(0.0);
                    let alt = if shape == 2 && j % 2 == 1 { 0.75 } else { 1.0 };
                    *tap = (
                        at_s,
                        contour * alt * rng.range(0.7, 1.0),
                        tau * rng.range(0.8, 1.25),
                    );
                    let mut d = step;
                    match shape {
                        0 => step *= 0.88,
                        1 => d = if j % 2 == 0 { step * 0.45 } else { step * 1.55 },
                        3 => step *= 1.12,
                        _ => {}
                    }
                    at_s += d * (1.0 + rng.bi() * 0.45 * fluid);
                }
                g.ntaps = n;
                g.len = (at_s + 0.8 * rate) as f64;
            }
            SCRATCH => {
                g.len = rng.range(1.5, 5.0) as f64 * r;
                g.rate = rng.range(2.0, 6.0) * lerp(0.8, 1.2, y);
                let base = rng.range(900.0, 2600.0) * lerp(0.85, 1.1, bright);
                g.co = [
                    SvfCo::new(base, rng.range(2.5, 5.0), rate),
                    SvfCo::new(base * rng.range(1.6, 2.4), rng.range(2.0, 4.0), rate),
                    SvfCo::new(base * 0.35, 1.0, rate),
                ];
                g.mix = [1.0, 0.5, 0.4];
            }
            CRINKLE => {
                g.len = rng.range(1.2, 4.5) as f64 * r;
                g.rate = lerp(250.0, 1400.0, y) * rng.range(0.6, 1.4);
                let paper = rng.f() < 0.6;
                let res = if paper {
                    rng.range(2000.0, 3400.0)
                } else {
                    rng.range(3500.0, 5500.0)
                };
                g.co = [
                    SvfCo::new(res, 3.0, rate),
                    SvfCo::new(res * rng.range(1.7, 2.3), 2.0, rate),
                    SvfCo::new(rng.range(350.0, 800.0), 1.2, rate),
                ];
                g.mix = [1.0, if paper { 0.35 } else { 0.6 }, 0.5];
            }
            BRUSH => {
                // A soft brush on the microphone, or hands / fabric rubbing (lower, slower).
                let rub = rng.f() < 0.35;
                g.len = if rub {
                    rng.range(1.5, 4.0)
                } else {
                    rng.range(0.9, 3.5)
                } as f64
                    * r;
                g.c0 = if rub {
                    rng.range(250.0, 700.0)
                } else {
                    rng.range(500.0, 1600.0)
                } * lerp(0.85, 1.1, bright);
                g.c1 = g.c0 * rng.range(0.5, 2.2);
                g.rate = if rub {
                    rng.range(20.0, 90.0)
                } else {
                    rng.range(80.0, 350.0)
                };
                g.co = [
                    SvfCo::new(g.c0, 0.8, rate),
                    SvfCo::new(260.0, 0.7, rate),
                    SvfCo::new(5200.0, 1.0, rate),
                ];
                g.mix = [
                    1.0,
                    if rub {
                        rng.range(0.9, 1.3)
                    } else {
                        rng.range(0.5, 1.0)
                    },
                    if rub { 0.15 } else { 0.3 },
                ];
            }
            PAGE => {
                // A page lifted, turned through the air, laid down.
                g.len = rng.range(1.1, 2.4) as f64 * r;
                g.rate = rng.range(300.0, 900.0);
                g.co = [
                    SvfCo::new(rng.range(900.0, 2200.0), 0.9, rate),
                    SvfCo::new(rng.range(2600.0, 3800.0), 2.5, rate),
                    SvfCo::new(rng.range(140.0, 260.0), 0.9, rate),
                ];
                g.mix = [1.0, 0.5, 1.0];
            }
            WATER => {
                g.len = rng.range(1.5, 5.0) as f64 * r;
                // Dripping (a few big bubbles) or squishing (bursts of small ones).
                g.stroke = if rng.f() < 0.4 { 0.0 } else { 1.0 };
                g.rate = if g.stroke == 0.0 {
                    rng.range(1.0, 3.5)
                } else {
                    rng.range(30.0, 90.0) * lerp(0.7, 1.3, y)
                };
                g.co = [
                    SvfCo::new(rng.range(350.0, 700.0), 0.7, rate),
                    SvfCo::default(),
                    SvfCo::default(),
                ];
            }
            _ => {
                // Sand or foam, squeezed.
                g.len = rng.range(2.0, 5.5) as f64 * r;
                g.rate = lerp(500.0, 2200.0, y) * rng.range(0.6, 1.3);
                g.squeeze_t = rng.range(0.6, 1.5);
                let f = rng.range(2600.0, 4200.0);
                g.co = [
                    SvfCo::new(f, 1.6, rate),
                    SvfCo::new(f * rng.range(1.6, 2.0), 1.6, rate),
                    SvfCo::new(rng.range(180.0, 420.0), 0.8, rate),
                ];
                g.mix = [1.0, 0.45, rng.range(0.5, 0.9)];
            }
        }
        let (az, rr) = (g.path.az0, g.path.r0);
        g.space.place(az, rr, rate, true);
        self.gestures[i] = g;
    }

    /// Adds this block of ASMR into `out`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render(
        &mut self,
        rng: &mut Rng,
        t: f64,
        rate: f32,
        active: bool,
        x: f32,
        bright: f32,
        out: &mut [[f32; 2]],
        frames: usize,
    ) {
        let block = frames as f32 / rate;
        let now_s = t / rate as f64;
        let n = frames.max(1) as f32;
        let k_space = coef(0.004, rate);
        let mut bus = [[0.0f32; 2]; crate::BLOCK];

        for g in self.gestures.iter_mut().filter(|g| g.on) {
            let p0 = ((t - g.start) / g.len) as f32;
            if p0 > 1.0 {
                g.on = false;
                continue;
            }
            if p0 >= 0.0 || t + frames as f64 > g.start {
                let p = smooth(clamp01(p0));
                let az = lerp(g.path.az0, g.path.az1, p);
                let rr = lerp(g.path.r0, g.path.r1, p);
                g.space.place(az, rr, rate, false);
            }
            for (f, o) in bus.iter_mut().take(frames).enumerate() {
                let now = t + f as f64;
                if now < g.start {
                    continue;
                }
                let p = ((now - g.start) / g.len) as f32;
                if p >= 1.0 {
                    break;
                }
                let s = voice(g, rng, now, p, rate) * g.level;
                let (l, r) = g.space.tick(s, k_space);
                o[0] += l;
                o[1] += r;
            }
        }

        // Handling under everything: changes every 0.3–2.5 s, sometimes to nothing.
        if !active {
            self.hand_target = 0.0;
        } else if now_s >= self.hand_next {
            self.hand_target = if rng.f() < 0.3 {
                0.0
            } else {
                rng.range(0.2, 1.0) * lerp(1.0, 0.6, x) + lerp(0.0, 0.25, x)
            };
            self.hand_next = now_s + rng.range(0.3, 2.5) as f64 * lerp(1.0, 2.0, x) as f64;
        }
        let hand0 = self.hand;
        self.hand += (self.hand_target - self.hand) * coef(0.35, 1.0 / block);
        if now_s >= self.near_next {
            self.near_target = rng.range(0.8, 1.15);
            self.near_next = now_s + rng.range(2.0, 7.0) as f64;
        }
        let near0 = self.near;
        self.near += (self.near_target - self.near) * coef(1.8, 1.0 / block);
        let hand_co = SvfCo::new(lerp(700.0, 1300.0, bright), 0.5, rate);
        let dip_co = SvfCo::new(4800.0, 1.1, rate);
        let hp = coef(1.0 / (2.0 * PI * 35.0), rate);
        let hand_on = self.hand > 1e-4 || hand0 > 1e-4;
        for (f, o) in out.iter_mut().take(frames).enumerate() {
            let u = f as f32 / n;
            let mut l = bus[f][0];
            let mut r = bus[f][1];
            if hand_on {
                let h = lerp(hand0, self.hand, u) * 0.04;
                l += self.hand_svf[0].run(rng.bi() * h, &hand_co).1;
                r += self.hand_svf[1].run(rng.bi() * h, &hand_co).1;
            }
            // No steady presence around 5 kHz: about −4 dB there.
            l -= self.dip[0].run(l, &dip_co).1 * 0.37;
            r -= self.dip[1].run(r, &dip_co).1 * 0.37;
            self.hp[0] += (l - self.hp[0]) * hp;
            self.hp[1] += (r - self.hp[1]) * hp;
            let g = lerp(near0, self.near, u);
            o[0] += (l - self.hp[0]) * g;
            o[1] += (r - self.hp[1]) * g;
        }
    }
}

/// One sample of a gesture's sound, before it is placed in space.
fn voice(g: &mut Gesture, rng: &mut Rng, now: f64, p: f32, rate: f32) -> f32 {
    let el = (now - g.start) as f32;
    match g.kind {
        TAP => {
            while g.tap_i < g.ntaps && el >= g.taps[g.tap_i].0 {
                let (_, amp, tau) = g.taps[g.tap_i];
                g.pulse_t = 0.0;
                g.pulse_len = tau.max(2.0);
                g.pulse_amp = amp;
                g.tap_i += 1;
            }
            let mut exc = 0.0;
            if g.pulse_t < g.pulse_len {
                exc = g.pulse_amp * 0.5 * (1.0 - (2.0 * PI * g.pulse_t / g.pulse_len).cos())
                    / (g.pulse_len * 0.5);
                g.pulse_t += 1.0;
            }
            // The skin: a breath of noise with the contact.
            let skin = g.svf[0].lp(rng.bi() * exc * 6.0, &SvfCo::new(2500.0, 0.6, rate));
            let mut s = skin * 0.15;
            for m in g.modes.iter_mut().take(g.nm) {
                s += m.tick(exc);
            }
            s * 0.5
        }
        SCRATCH => {
            // Back and forth: each stroke its own pressure, now and then a pause.
            g.ph += g.rate / rate;
            if g.ph >= 1.0 {
                g.ph -= 1.0;
                g.stroke = if rng.f() < 0.12 {
                    0.0
                } else {
                    rng.range(0.45, 1.0)
                };
            }
            let env = (g.ph * PI).sin().powf(0.6) * g.stroke * window(p, 0.12, 0.2);
            let n = rng.bi() * env;
            let a = g.svf[0].run(n, &g.co[0]).1;
            let b = g.svf[1].run(n, &g.co[1]).1;
            let c = g.svf[2].run(n, &g.co[2]).1;
            (a * g.mix[0] + b * g.mix[1] + c * g.mix[2]) * 0.35
        }
        CRINKLE => {
            // The hands squeeze, let go, squeeze: density follows.
            if now >= g.next_change {
                g.squeeze_t = if rng.f() < 0.25 {
                    0.0
                } else {
                    rng.range(0.3, 1.0)
                };
                g.next_change = now + rng.range(0.08, 0.35) as f64 * rate as f64;
            }
            g.squeeze += (g.squeeze_t - g.squeeze) * 0.0015;
            let env = window(p, 0.15, 0.25);
            let dens = g.rate * g.squeeze * env;
            let mut imp = 0.0;
            if rng.f() < dens / rate {
                // Crumpling: energies follow a power law (exponent ~1.4), heavy-tailed.
                let e = (1.0 - rng.f()).max(1e-3).powf(-1.0 / 0.4).min(12.0);
                imp = e.sqrt() * 0.05 * rng.bi().signum();
            }
            g.lp += (g.squeeze * env - g.lp) * 0.002;
            let a = g.svf[0].run(imp, &g.co[0]).1;
            let b = g.svf[1].run(imp, &g.co[1]).1;
            let body = g.svf[2].run(rng.bi() * g.lp * 0.08, &g.co[2]).1;
            a * g.mix[0] + b * g.mix[1] + body * g.mix[2]
        }
        BRUSH => {
            let env = window(p, 0.25, 0.32);
            let cut = lerp(g.c0, g.c1, smooth(p));
            if now >= g.next_change {
                g.co[0] = SvfCo::new(cut, 0.8, rate);
                g.next_change = now + 64.0;
                // Bristle grain: the level roughens a little, block by block.
                g.squeeze_t = rng.range(0.6, 1.0);
            }
            g.squeeze += (g.squeeze_t - g.squeeze) * 0.01;
            let w = rng.bi() * env * g.squeeze;
            let swish = g.svf[0].run(w, &g.co[0]).1;
            let whoosh = g.svf[1].lp(w, &g.co[1]);
            // Sparse bristle ticks on top: crisp, fluctuating.
            let tick = if rng.f() < g.rate * env / rate {
                rng.bi() * 0.6
            } else {
                0.0
            };
            let top = g.svf[2].run(tick, &g.co[2]).1;
            (swish * g.mix[0] + whoosh * g.mix[1] * 0.8 + top * g.mix[2]) * 0.6
        }
        WATER => {
            let env = window(p, 0.1, 0.3);
            // Squishing comes in squeezes; dripping is steady.
            let burst = if g.stroke == 0.0 {
                1.0
            } else {
                if now >= g.next_change {
                    g.squeeze_t = if rng.f() < 0.4 { 0.0 } else { 1.0 };
                    g.next_change = now + rng.range(0.15, 0.5) as f64 * rate as f64;
                }
                g.squeeze += (g.squeeze_t - g.squeeze) * 0.004;
                g.squeeze
            };
            if rng.f() < g.rate * env * burst / rate {
                if let Some(b) = g.bubbles.iter_mut().find(|b| !b.on) {
                    // Size sets pitch (f · r ≈ 3 m/s): many small, few large.
                    let big = g.stroke == 0.0;
                    let f = if big {
                        rng.range(350.0, 900.0)
                    } else {
                        (600.0 * rng.f().max(0.05).powf(-0.45)).min(3200.0)
                    };
                    let tau = 0.014 * (800.0 / f).sqrt() * if big { 2.2 } else { 1.0 };
                    *b = Bubble {
                        on: true,
                        ph: 0.0,
                        inc: f / rate,
                        glide: (1.0f32 + 0.25).powf(1.0 / (tau * 3.0 * rate)),
                        amp: rng.range(0.05, 0.16) * if big { 1.6 } else { 1.0 },
                        dec: (-1.0 / (tau * rate)).exp(),
                    };
                }
            }
            let mut s = 0.0;
            for b in g.bubbles.iter_mut().filter(|b| b.on) {
                s += (b.ph * 2.0 * PI).sin() * b.amp;
                b.ph += b.inc;
                if b.ph >= 1.0 {
                    b.ph -= 1.0;
                }
                b.inc *= b.glide;
                b.amp *= b.dec;
                if b.amp < 1e-4 {
                    b.on = false;
                }
            }
            // The water itself, a soft low slosh with the squeezes.
            let slosh = g.svf[0].lp(rng.bi() * env * burst * 0.25, &g.co[0]);
            s + slosh
        }
        PAGE => {
            // Lift (air swelling), turn (the sheet flexing, a few crumple clicks), and the
            // soft low flap as it lands.
            let air = window(p, 0.35, 0.45);
            let flex = window(((p - 0.25) / 0.55).clamp(0.0, 1.0), 0.3, 0.4)
                * if (0.25..0.8).contains(&p) { 1.0 } else { 0.0 };
            let land = if (0.74..0.9).contains(&p) {
                (((p - 0.74) / 0.16) * PI).sin()
            } else {
                0.0
            };
            let w = rng.bi();
            let a = g.svf[0].run(w * (air * 0.35 + flex * 0.6), &g.co[0]).1;
            let mut imp = 0.0;
            if rng.f() < g.rate * flex * flex / rate {
                imp = rng.range(0.2, 1.0) * rng.bi().signum() * 0.12;
            }
            let b = g.svf[1].run(imp, &g.co[1]).1;
            let c = g.svf[2].lp(w * land * 0.5, &g.co[2]);
            (a * g.mix[0] + b * g.mix[1] + c * g.mix[2]) * 0.7
        }
        _ => {
            // Sand / foam: squeezes; in each, a crisp crowd of grains over a low squish.
            g.ph += 1.0 / (g.squeeze_t * rate);
            if g.ph >= 1.0 {
                g.ph -= 1.0;
            }
            let sq = (g.ph * PI).sin().powi(2) * window(p, 0.12, 0.25);
            let mut imp = 0.0;
            if rng.f() < g.rate * sq / rate {
                imp = rng.range(0.2, 1.0) * rng.bi().signum() * 0.12;
            }
            let a = g.svf[0].run(imp, &g.co[0]).1;
            let b = g.svf[1].run(imp, &g.co[1]).1;
            let squish = g.svf[2].lp(rng.bi() * sq * 0.18, &g.co[2]);
            a * g.mix[0] + b * g.mix[1] + squish * g.mix[2]
        }
    }
}
