//! angel-niri: windows thrown between desks as stacking (the shell's Desktop) starts and ends.
//! `niri msg action set-desk-throw [--on false] [--strength 0|1|2]`; off until a shell turns it
//! on. The strength (calm, usual, wild) sets how far apart they come, their trails, the hit's
//! freeze and shake, and how big and long the break is (`Feel`); calm breaks nothing.
//! - Starting, the windows gathered from desks 2…N onto desk 1 fly up from below its output, each
//!   on a way of its own (`Path`, `Fly::new`): from below or in from a side, along a curve of its
//!   own bends, its own pace, turn and bank, now and then a whole spin; its app gives it a manner,
//!   every throw shakes it up. From the next desk they come quickly and land with a knock
//!   (a shake); from desks further away they come one by one, well apart, slam into the top of
//!   the output (a freeze at the hit, the screen jolting), straighten and drop into their places.
//! - Ending, the gathered windows still on desk 1 drop off the bottom before they go back to their
//!   desks. No damage then.
//! Where a far one hits, the monitor breaks — the picture itself, in screen casts too
//! (shaders/glitch.frag over the finished frame): a jolt tearing it sideways, then a black blot of
//! leaked liquid crystal with dark veins, bending the picture around its edge, long lines across
//! the whole screen, dead pixels of the monitor's own size flickering. The dead pixels and lines
//! heal first; a couple of seconds later the blot pulls back into the impact. Every hit adds its
//! own (`glitch`): the blots grow and run together into one, past `SPOTS` a hit swells the nearest.
//! The pointer goes under the damage then (it is no different from the rest of the picture).
//! The impacts also go to the shell for their sounds (`Event::DesksThrown`).
//! In flight a window is drawn turned in whole pixels of the pixel corners, with dithered
//! afterimages trailing it (shaders/throw.frag).

use std::cell::{Cell, RefCell};
use std::time::Duration;

use niri_ipc::Event;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::utils::{Logical, Point};

use crate::animation::{Animation, Clock, Curve};
use crate::niri::State;

// per thread, as stacking.rs: tests run side by side
thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
    static STRENGTH: Cell<u8> = const { Cell::new(1) };
    static GLITCHES: RefCell<Vec<Glitch>> = const { RefCell::new(Vec::new()) };
}

/// How hard the throw is, by its strength.
#[derive(Debug, Clone, Copy)]
pub struct Feel {
    near_gap_ms: u64,
    far_gap_ms: u64,
    near_ms: u64,
    far_ms: u64,
    /// Far hits break the screen.
    damage: bool,
    /// How long the break lasts after the last hit, ms.
    life_ms: f64,
    /// Every blot this much bigger.
    blot: f64,
    /// Afterimages trailing a far one, a near or leaving one.
    trail_far: usize,
    trail_near: usize,
    /// A far one freezes this long pressed into the edge it hit.
    hit_stop_ms: u64,
    /// The screen jolts this far (logical px) at a far hit.
    shake: f32,
}

const FEELS: [Feel; 3] = [
    Feel {
        near_gap_ms: 70,
        far_gap_ms: 220,
        near_ms: 420,
        far_ms: 560,
        damage: false,
        life_ms: 0.,
        blot: 1.,
        trail_far: 0,
        trail_near: 0,
        hit_stop_ms: 0,
        shake: 0.,
    },
    Feel {
        near_gap_ms: 110,
        far_gap_ms: 480,
        near_ms: 440,
        far_ms: 600,
        damage: true,
        life_ms: 8500.,
        blot: 1.,
        trail_far: 2,
        trail_near: 0,
        hit_stop_ms: 55,
        shake: 6.,
    },
    Feel {
        near_gap_ms: 160,
        far_gap_ms: 720,
        near_ms: 460,
        far_ms: 640,
        damage: true,
        life_ms: 11000.,
        blot: 1.15,
        trail_far: 3,
        trail_near: 2,
        hit_stop_ms: 95,
        shake: 12.,
    },
];

pub fn feel() -> Feel {
    FEELS[usize::from(STRENGTH.get().min(2))]
}

pub fn set_strength(strength: u8) {
    STRENGTH.set(strength.min(2));
}

/// A hit's damage: on `output` at `pos` (logical, the edge hit), from `start` (the clock's); the
/// hits that came past `SPOTS` and swelled it instead, when.
#[derive(Debug, Clone)]
struct Glitch {
    output: String,
    pos: [f64; 2],
    start: Duration,
    seed: f64,
    swells: Vec<Duration>,
}

/// At most this many blots on one output at once (shaders/glitch.frag `g_h0…`); more hits swell
/// the nearest.
pub const SPOTS: usize = 6;
/// The damage changes this often: it stutters, it doesn't flow.
const STEP_MS: f64 = 70.;

/// One frame of a broken screen: each blot as (x, y logical, its age in s or -1 for none, its
/// seed) and how many hits swelled it, the stutter step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlitchFrame {
    pub spots: [[f32; 4]; SPOTS],
    pub swells: [f32; SPOTS],
    pub step: f32,
    /// How much bigger every blot is for the hits so far (1 with one) and the strength.
    pub grow: f32,
    /// Until all of it has healed, s: the hits heal together, after the last one.
    pub left: f32,
    /// The latest hit's age, s (the jolt and shake), and how far the screen shakes, logical px.
    pub jolt: [f32; 2],
    /// The latest hit (its start, ms, as a key) and where, logical: the frame around it is kept
    /// once for its stuck pieces of the window that hit.
    pub snap: Option<(u64, [f32; 2])>,
}

/// Each hit after the first makes every blot this much bigger, growing over `GROW_MS`.
const GROW_PER_HIT: f64 = 0.2;
const GROW_MS: f64 = 500.;
/// Past this many hits after the first it grows no more: the screen stays a screen.
const GROW_HITS: f64 = 4.;

/// A hit on `output` at `pos`, `delay` after `now`: it adds its damage to whatever is there and
/// makes it all bigger. Nothing with a calm throw.
pub fn glitch(output: &str, pos: [f64; 2], now: Duration, delay: Duration) {
    if !feel().damage {
        return;
    }
    GLITCHES.with_borrow_mut(|all| {
        let start = now + delay;
        let mine = all.iter().filter(|g| g.output == output).count();
        if mine >= SPOTS {
            // no room for a blot of its own: the nearest swells, they run together anyway
            let dist = |g: &Glitch| (g.pos[0] - pos[0]).hypot(g.pos[1] - pos[1]);
            let nearest = (all.iter_mut().filter(|g| g.output == output))
                .min_by(|a, b| dist(a).total_cmp(&dist(b)))
                .unwrap();
            nearest.swells.push(start);
            return;
        }
        let seed = (start.as_millis() % 9973) as f64 * 0.618;
        all.push(Glitch {
            output: output.to_owned(),
            pos,
            start,
            seed,
            swells: Vec::new(),
        });
    });
}

/// Every hit on `output`, blots and swells.
fn hits_on<'a>(all: &'a [Glitch], output: &'a str) -> impl Iterator<Item = Duration> + 'a {
    (all.iter().filter(move |g| g.output == output))
        .flat_map(|g| std::iter::once(g.start).chain(g.swells.iter().copied()))
}

/// When the damage on `output` has healed: the strength's life after its last hit.
fn healed_at(all: &[Glitch], output: &str) -> Option<Duration> {
    let last = hits_on(all, output).max()?;
    Some(last + Duration::from_secs_f64(feel().life_ms / 1000.))
}

/// Whether `output` has damage coming or showing: it keeps redrawing.
pub fn glitching(output: &str, now: Duration) -> bool {
    GLITCHES.with_borrow_mut(|all| {
        let ends: Vec<_> = all.iter().map(|g| healed_at(all, &g.output)).collect();
        let mut i = 0;
        all.retain(|_| {
            let keep = ends[i].is_some_and(|end| now < end);
            i += 1;
            keep
        });
        all.iter().any(|g| g.output == output)
    })
}

/// The damage to draw on `output` now, if any.
pub fn glitch_at(output: &str, now: Duration) -> Option<GlitchFrame> {
    GLITCHES.with_borrow(|all| {
        let end = healed_at(all, output)?;
        if now >= end {
            return None;
        }
        let grown = |at: Duration| (now.saturating_sub(at).as_secs_f64() * 1000. / GROW_MS).min(1.);
        let mut spots = [[0., 0., -1., 0.]; SPOTS];
        let mut swells = [0.; SPOTS];
        let mine = all.iter().filter(|g| g.output == output && g.start <= now);
        for ((spot, swell), g) in spots.iter_mut().zip(swells.iter_mut()).zip(mine) {
            let age = (now - g.start).as_secs_f64();
            *spot = [g.pos[0] as f32, g.pos[1] as f32, age as f32, g.seed as f32];
            *swell = g
                .swells
                .iter()
                .filter(|at| **at <= now)
                .map(|at| grown(*at))
                .sum::<f64>() as f32;
        }
        let came: Vec<_> = hits_on(all, output).filter(|at| *at <= now).collect();
        let snap = (all.iter().filter(|g| g.output == output))
            .flat_map(|g| {
                std::iter::once(g.start)
                    .chain(g.swells.iter().copied())
                    .map(move |at| (at, g.pos))
            })
            .filter(|(at, _)| *at <= now)
            .max_by_key(|(at, _)| *at)
            .map(|(at, pos)| (at.as_millis() as u64, [pos[0] as f32, pos[1] as f32]));
        let hits: f64 = came.iter().map(|at| grown(*at)).sum();
        if hits <= 0. {
            return None;
        }
        let latest = came
            .iter()
            .max()
            .map_or(-1., |at| (now - *at).as_secs_f32());
        let feel = feel();
        let step = (now.as_secs_f64() * 1000. / STEP_MS).floor() as f32;
        Some(GlitchFrame {
            spots,
            swells,
            step,
            grow: ((1. + GROW_PER_HIT * (hits - 1.).clamp(0., GROW_HITS)) * feel.blot) as f32,
            left: (end - now).as_secs_f32(),
            jolt: [latest, feel.shake],
            snap,
        })
    })
}

pub fn on() -> bool {
    ON.get()
}

pub fn set(on: bool) {
    ON.set(on);
}

impl State {
    /// Stacking on or off (`set-stacking-mode`): ending it waits for the thrown windows to fly
    /// off first. The shell hears of every throw.
    pub fn switch_stacking(&mut self, on: bool) {
        let out = if on {
            None
        } else {
            self.niri.layout.throw_out()
        };
        if let Some((wait, n)) = out {
            self.niri
                .event_loop
                .insert_source(Timer::from_duration(wait), move |_, _, state| {
                    if state.niri.layout.throw_out_pending(n) {
                        state.niri.layout.set_stacking(false);
                        state.stacking_decorations();
                        state.niri.queue_redraw_all();
                    }
                    TimeoutAction::Drop
                })
                .unwrap();
        } else {
            self.niri.layout.set_stacking(on);
            self.stacking_decorations();
        }
        let throws = std::mem::take(&mut self.niri.layout.desk_throws);
        let now = self.niri.clock.now();
        for hit in throws.iter().filter(|hit| hit.far && !hit.bottom) {
            glitch(&hit.output, hit.pos, now, Duration::from_millis(hit.in_ms));
        }
        if !throws.is_empty() {
            if let Some(server) = &self.niri.ipc_server {
                server.send_transient_event(Event::DesksThrown { throws });
            }
        }
        self.niri.queue_redraw_all();
    }
}

/// Into the top edge, the part of the flight going up.
const UP: f64 = 0.5;
/// Below the output, how far past its edge a window starts or ends.
const MARGIN: f64 = 16.;
/// The afterimages lag this far behind each other, ms of the flight.
const GHOST_MS: f64 = 24.;
const GHOST_ALPHA: [f32; 3] = [0.5, 0.3, 0.16];

/// A window's own way of flying: twelve rolls, each part its app's liking and part this throw's
/// chance (so an app keeps a manner, yet no two throws are alike). `Fly::new` makes a way out of
/// them: where it comes from (below, or in from a side), the bends of its curve, its pace, how far
/// it turns and banks, now and then a whole spin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Path {
    pub r: [f64; 12],
}

impl Path {
    /// The way the `order`th window of a throw flies, of app `app`, in the throw `nonce`.
    pub fn new(app: &str, order: usize, nonce: u64) -> Self {
        let app = app.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
        let mut r = [0.; 12];
        for (k, r) in r.iter_mut().enumerate() {
            let k = k as u64 + 1;
            let liked = unit(app ^ k.wrapping_mul(0x9e37_79b9_7f4a_7c15));
            let now = unit(
                nonce
                    ^ (order as u64 + 1).wrapping_mul(0xbf58_476d_1ce4_e5b9)
                    ^ k.wrapping_mul(0x94d0_49bb_1331_11eb),
            );
            *r = 0.35 * liked + 0.65 * now;
        }
        // left and right in turn, so a crowd spreads out
        if order % 2 == 1 {
            r[1] = 1. - r[1];
        }
        Self { r }
    }
}

/// How far above its top edge a window of `size` turned by `angle` reaches (its higher corner).
fn corner_rise(size: (f64, f64), angle: f64) -> f64 {
    let (w, h) = (size.0 / 2., size.1 / 2.);
    w * angle.sin().abs() + h * angle.cos() - h
}

/// 0…1 from `x`, well mixed (splitmix64).
fn unit(x: u64) -> f64 {
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// A cubic Bézier through offsets, at `s` (past 1 it goes on the way it ended: an overshoot).
fn bezier(p: &[(f64, f64); 4], s: f64) -> (f64, f64) {
    if s > 1. {
        let (end, before) = (p[3], p[2]);
        return (
            end.0 + (s - 1.) * 3. * (end.0 - before.0),
            end.1 + (s - 1.) * 3. * (end.1 - before.1),
        );
    }
    let u = 1. - s;
    let (a, b, c, d) = (u * u * u, 3. * u * u * s, 3. * u * s * s, s * s * s);
    (
        a * p[0].0 + b * p[1].0 + c * p[2].0 + d * p[3].0,
        a * p[0].1 + b * p[1].1 + c * p[2].1 + d * p[3].1,
    )
}

/// An afterimage trailing a window in flight: from where the window is drawn now, turned so, this
/// opaque (dithered).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ghost {
    pub offset: Point<f64, Logical>,
    pub angle: f64,
    pub alpha: f32,
}

/// One window's flight; offsets are from where it rests.
#[derive(Debug)]
pub struct Fly {
    anim: Animation,
    /// The flight, ms, and the freeze at its hit (far ones arriving), ms.
    ms: f64,
    hold: f64,
    pub leaving: bool,
    pub far: bool,
    /// Its way: arriving, from its start to the hit (far: its turned top corner just touches the
    /// top edge there — where the screen breaks) or to its place; leaving, from its place down
    /// past the bottom.
    way: [(f64, f64); 4],
    /// The turn it hits with (far) or sets off with (near), radians; how much it banks in its
    /// bends; whole spins on the way.
    spin: f64,
    bank: f64,
    spins: f64,
    /// Leaving: the hop up before the drop.
    hop: f64,
    ghosts: usize,
}

impl Fly {
    /// A flight starting `delay` from now; `rest`: where the window's top left rests in its
    /// output, `size`: the window's, `view`: the output's; `path`: its rolls.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        clock: Clock,
        delay: Duration,
        leaving: bool,
        far: bool,
        rest: (f64, f64),
        size: (f64, f64),
        view: (f64, f64),
        path: Path,
    ) -> Self {
        let feel = feel();
        let r = path.r;
        let (x, y) = rest;
        let w = size.0;
        let (vw, vh) = view;
        let pace = 0.8 + 0.4 * r[9];
        let base = if far && !leaving {
            feel.far_ms
        } else {
            feel.near_ms
        };
        let ms = (base as f64 * pace).round();
        let hold = if far && !leaving {
            feel.hit_stop_ms as f64
        } else {
            0.
        };
        let left = r[1] < 0.5;
        // the turn: from barely to hard, mostly leaning the way it travels
        let lean = if left { 1. } else { -1. } * if r[8] < 0.2 { -1. } else { 1. };
        let spin = lean * (0.03 + 0.25 * r[6] * r[6] + 0.06 * r[7]);
        let bank = (r[10] * 2. - 1.) * 0.22;
        let spins = if far && !leaving && r[11] > 0.86 {
            lean
        } else {
            0.
        };

        // screen points (the window's top left), then offsets from its rest
        let below = vh + MARGIN;
        let from = if leaving {
            (x, y)
        } else if r[0] < 0.5 {
            // from below, anywhere along the bottom
            (
                lerp(
                    -w * 0.3,
                    vw - w * 0.7,
                    if left { r[2] * 0.6 } else { 0.4 + r[2] * 0.6 },
                ),
                below,
            )
        } else if r[0] < 0.8 {
            // in from a side, low down
            (
                if left { -w - MARGIN } else { vw + MARGIN },
                lerp(vh * 0.35, vh * 0.95, r[2]),
            )
        } else {
            // from a bottom corner, swerving across
            (if left { -w * 0.6 } else { vw - w * 0.4 }, below)
        };
        let to = if leaving {
            let off = (r[2] * 2. - 1.) * vw * 0.25 + if left { -0.2 } else { 0.2 } * w;
            (x + off, below + 40.)
        } else if far {
            let shift = (r[3] * 2. - 1.) * (vw * 0.2).min(300.);
            (x + shift, corner_rise(size, spin))
        } else {
            (x, y)
        };
        // the bends: somewhere along the way, pulled off it sideways, either way
        let swing = (vw * 0.45).min(700.);
        let c1 = (
            lerp(from.0, to.0, 0.3) + (r[4] * 2. - 1.) * swing,
            lerp(from.1, to.1, 0.25 + 0.3 * r[5]),
        );
        let c2 = (
            lerp(from.0, to.0, 0.7) + (r[5] * 2. - 1.) * swing * 0.6,
            lerp(from.1, to.1, 0.6 + 0.3 * r[4]),
        );
        let off = |p: (f64, f64)| (p.0 - x, p.1 - y);
        let anim =
            Animation::ease(clock, 0., 1., 0., (ms + hold) as u64, Curve::Linear).delayed(delay);
        Self {
            anim,
            ms,
            hold,
            leaving,
            far,
            way: [off(from), off(c1), off(c2), off(to)],
            spin,
            bank,
            spins,
            hop: 14. + 22. * r[7],
            ghosts: if far && !leaving {
                feel.trail_far
            } else {
                feel.trail_near
            },
        }
    }

    pub fn is_done(&self) -> bool {
        self.anim.is_done()
    }

    /// Where the flight is `at` ms into it: 0…1, holding still at the hit for the freeze.
    fn t_at(&self, at: f64) -> f64 {
        let hit = UP * self.ms;
        let at = if at < hit {
            at
        } else if at < hit + self.hold {
            hit
        } else {
            at - self.hold
        };
        (at / self.ms).clamp(0., 1.)
    }

    /// How many ms into the flight it is (0 before it sets off).
    fn elapsed(&self) -> f64 {
        self.anim.value().clamp(0., 1.) * (self.ms + self.hold)
    }

    fn t(&self) -> f64 {
        self.t_at(self.elapsed())
    }

    /// Where it is drawn, from its place.
    pub fn offset(&self) -> Point<f64, Logical> {
        let (x, y) = self.at(self.t());
        Point::from((x, y))
    }

    /// The turn it is drawn with, radians.
    pub fn angle(&self) -> f64 {
        self.angle_at(self.t())
    }

    /// Its afterimages now, nearest first: where it was a moment ago, from where it is.
    pub fn trail(&self) -> Vec<Ghost> {
        let at = self.elapsed();
        let t = self.t();
        if self.ghosts == 0 || t <= 0. || t >= 1. {
            return Vec::new();
        }
        let (x, y) = self.at(t);
        (1..=self.ghosts)
            .filter_map(|k| {
                let back = at - k as f64 * GHOST_MS;
                if back <= 0. {
                    return None;
                }
                let tb = self.t_at(back);
                let (bx, by) = self.at(tb);
                let offset = Point::from((bx - x, by - y));
                // standing (the freeze) or barely moving: nothing trails
                (offset.x.hypot(offset.y) >= 6.).then(|| Ghost {
                    offset,
                    angle: self.angle_at(tb),
                    alpha: GHOST_ALPHA[(k - 1).min(2)],
                })
            })
            .collect()
    }

    fn angle_at(&self, t: f64) -> f64 {
        if t <= 0. || t >= 1. {
            return 0.;
        }
        let tau = std::f64::consts::TAU;
        if self.leaving {
            return -self.spin * t * 1.5 + self.bank * (t * std::f64::consts::PI).sin();
        }
        if !self.far {
            let e = 1. - t;
            return self.spin * 1.4 * e * e + self.bank * (t * std::f64::consts::PI).sin() * e;
        }
        if t < UP {
            let s = t / UP;
            // the turn it hits with, banking in its bends, a whole spin for some
            self.spin * s.powf(0.8)
                + self.bank * (s * std::f64::consts::PI).sin()
                + self.spins * tau * (1. - (1. - s) * (1. - s))
        } else {
            // straightens at the hit, swinging back once
            let s = (t - UP) / (1. - UP);
            self.spin * (1. - s).powi(3) * (s * tau).cos()
        }
    }

    fn at(&self, t: f64) -> (f64, f64) {
        if t >= 1. && !self.leaving {
            return (0., 0.);
        }
        if self.leaving {
            // a hop up, then down past the bottom along its way, faster and faster
            let hop = if t < 0.35 {
                self.hop * (t / 0.35 * std::f64::consts::PI).sin()
            } else {
                0.
            };
            let (x, y) = bezier(&self.way, t.powi(3));
            return (x, y - hop);
        }
        if !self.far {
            // into place along its way, a little past it and back, then a knock
            let (x, y) = bezier(&self.way, ease_out_back(t));
            return (x + shake(t, 0.7, 5.), y);
        }
        let hit = self.way[3];
        if t < UP {
            let s = t / UP;
            // fast, slowing a little near the top: it hits with speed
            bezier(&self.way, s * (1.6 - 0.6 * s))
        } else {
            let s = (t - UP) / (1. - UP);
            let e = ease_out_bounce(s);
            (
                hit.0 * (1. - e.min(1.)) + shake(t, UP, 8.),
                hit.1 * (1. - e),
            )
        }
    }

    /// When it sets off, from now.
    pub fn off_in(&self, now: Duration) -> Duration {
        self.anim.start_time().saturating_sub(now)
    }

    /// When it makes its sound, from now, and where from its top left, sideways: a far one
    /// arriving hits the top with its higher corner, a near one knocks into place, a leaving one
    /// starts to drop.
    pub fn hit_in(&self, now: Duration, size: (f64, f64)) -> (Duration, f64) {
        let t = if self.leaving {
            0.3
        } else if self.far {
            UP
        } else {
            0.7
        };
        let start = self.anim.start_time();
        let at = (start + Duration::from_secs_f64(self.ms * t / 1000.)).saturating_sub(now);
        let mut x = self.at(t).0 + size.0 / 2.;
        if self.far && !self.leaving {
            // the top corners turned about the middle (as shaders/throw.frag draws it): the higher
            let (w, h) = (size.0 / 2., size.1 / 2.);
            let (sin, cos) = self.angle_at(t).sin_cos();
            let left = (-w * cos + h * sin, -w * sin - h * cos);
            let right = (w * cos + h * sin, w * sin - h * cos);
            x += if left.1 <= right.1 { left.0 } else { right.0 };
        }
        (at, x)
    }
}

/// When each of a row of windows (`far` flags, the near ones first) sets off, from now.
pub fn delays(far: &[bool], leaving: bool) -> Vec<Duration> {
    let feel = feel();
    let mut out = Vec::with_capacity(far.len());
    let mut at = 0;
    let mut last_far = None;
    for (i, far) in far.iter().enumerate() {
        if i > 0 {
            at += if leaving || !far {
                feel.near_gap_ms
            } else if last_far.is_some() {
                feel.far_gap_ms
            } else {
                // the first far one after the near ones: a breath first
                feel.near_gap_ms * 2
            };
        }
        if *far && !leaving {
            last_far = Some(i);
        }
        out.push(Duration::from_millis(at));
    }
    out
}

/// The longest any flight takes.
pub fn longest() -> Duration {
    let feel = feel();
    let ms = feel.far_ms.max(feel.near_ms) as f64 * 1.02;
    Duration::from_millis(ms.ceil() as u64 + feel.hit_stop_ms)
}

/// A sideways shake from `from` on, dying out by the end, `amp` px at its widest.
fn shake(t: f64, from: f64, amp: f64) -> f64 {
    if t <= from {
        return 0.;
    }
    let s = (t - from) / (1. - from);
    (amp * (1. - s) * (s * 6. * std::f64::consts::TAU).sin()).round()
}

fn ease_out_back(t: f64) -> f64 {
    let c1 = 1.2;
    let c3 = c1 + 1.;
    1. + c3 * (t - 1.).powi(3) + c1 * (t - 1.).powi(2)
}

fn ease_out_bounce(t: f64) -> f64 {
    // one small bounce: down to the place, up a bit, back
    if t < 0.7 {
        let s = t / 0.7;
        s * s
    } else {
        let s = (t - 0.7) / 0.3;
        1. - 0.06 * (s * std::f64::consts::PI).sin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fly_with(leaving: bool, far: bool, r: [f64; 12]) -> Fly {
        Fly::new(
            Clock::with_time(Duration::ZERO),
            Duration::ZERO,
            leaving,
            far,
            (300., 200.),
            (400., 300.),
            (1920., 1080.),
            Path { r },
        )
    }

    fn fly(leaving: bool, far: bool) -> Fly {
        fly_with(leaving, far, [0.3; 12])
    }

    #[test]
    fn flights_start_off_screen_and_end_in_place() {
        for i in 0..30 {
            let path = Path::new("kitty", i, i as u64 * 977);
            let f = fly_with(false, true, path.r);
            let (x, y) = f.at(0.);
            // its top left is off the screen: below it, or past a side
            assert!(
                y + 200. > 1080. || x + 300. < -399. || x + 300. > 1920.,
                "{x} {y}"
            );
            assert_eq!(f.at(1.), (0., 0.));
            // it stops where its turned corner touches the top
            let top = f.at(UP).1 + 200.;
            assert!(
                (top - corner_rise((400., 300.), f.spin)).abs() < 1e-9,
                "{top}"
            );
            let near = fly_with(false, false, path.r);
            assert_eq!(near.at(1.), (0., 0.));
        }
    }

    #[test]
    fn a_far_one_freezes_at_the_hit() {
        let f = fly(false, true);
        let hit = UP * f.ms;
        assert_eq!(f.t_at(hit), UP);
        assert_eq!(f.t_at(hit + f.hold * 0.5), UP);
        assert!(f.t_at(hit + f.hold + 10.) > UP);
        assert_eq!(f.t_at(f.ms + f.hold), 1.);
    }

    #[test]
    fn leaving_ends_below() {
        let f = fly(true, true);
        assert_eq!(f.at(0.), (0., 0.));
        assert!(f.at(1.).1 + 200. > 1080.);
    }

    #[test]
    fn ways_differ_by_throw_and_spread_out() {
        let ways: Vec<_> = (0..8)
            .map(|i| fly_with(false, true, Path::new("kitty", i, 7).r).way)
            .collect();
        for (i, a) in ways.iter().enumerate() {
            for b in &ways[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_ne!(Path::new("kitty", 0, 7), Path::new("kitty", 0, 8));
        assert_ne!(Path::new("kitty", 0, 7), Path::new("helium", 0, 7));
        // from both sides of the screen
        let starts: Vec<_> = ways.iter().map(|w| w[0].0 + 300. < 760.).collect();
        assert!(
            starts.contains(&true) && starts.contains(&false),
            "{starts:?}"
        );
    }

    #[test]
    fn far_ones_come_well_apart_near_ones_quickly() {
        let near = feel().near_gap_ms;
        let d = delays(&[false, false, true, true, true], false);
        assert_eq!(d[1], Duration::from_millis(near));
        assert!(d[3] - d[2] >= Duration::from_millis(400));
        assert!(d[4] - d[3] >= Duration::from_millis(400));
        let back = delays(&[false, true, true], true);
        assert_eq!(back[2], Duration::from_millis(near * 2));
    }

    #[test]
    fn every_hit_makes_it_bigger_and_all_heals_together() {
        let life = feel().life_ms as u64;
        glitch("A", [100., 0.], Duration::ZERO, Duration::from_millis(100));
        glitch("A", [500., 0.], Duration::ZERO, Duration::from_millis(500));
        assert_eq!(
            glitch_at("A", Duration::from_millis(50)),
            None,
            "before the hit"
        );
        let one = glitch_at("A", Duration::from_millis(400)).unwrap();
        assert_eq!(one.grow, 1.);
        let two = glitch_at("A", Duration::from_millis(1100)).unwrap();
        assert_eq!(two.spots.iter().filter(|s| s[2] >= 0.).count(), 2);
        assert!(
            (two.grow - (1. + GROW_PER_HIT as f32)).abs() < 1e-5,
            "{}",
            two.grow
        );
        assert!((two.jolt[0] - 0.6).abs() < 1e-5, "the latest hit jolts");
        // the first heals with the last, not on its own
        let late = Duration::from_millis(100 + life + 100);
        assert!(glitch_at("A", late).is_some_and(|f| f.spots[0][2] > 0.));
        let end = Duration::from_millis(500 + life + 10);
        assert!(!glitching("A", end));
        assert_eq!(glitch_at("A", end), None);
    }

    #[test]
    fn past_the_spots_a_hit_swells_the_nearest() {
        for i in 0..SPOTS {
            glitch("B", [i as f64 * 300., 0.], Duration::ZERO, Duration::ZERO);
        }
        glitch("B", [905., 0.], Duration::ZERO, Duration::from_millis(100));
        let f = glitch_at("B", Duration::from_millis(1000)).unwrap();
        assert_eq!(f.spots.iter().filter(|s| s[2] >= 0.).count(), SPOTS);
        assert_eq!(f.swells[3], 1., "{:?}", f.swells);
        assert_eq!(f.swells.iter().sum::<f32>(), 1.);
        // and it keeps the damage longer
        let life = feel().life_ms as u64;
        assert!(glitch_at("B", Duration::from_millis(life + 50)).is_some());
    }

    #[test]
    fn calm_breaks_nothing() {
        set_strength(0);
        glitch("C", [0., 0.], Duration::ZERO, Duration::ZERO);
        assert_eq!(glitch_at("C", Duration::from_millis(100)), None);
        assert!(fly(false, true).trail().is_empty());
        set_strength(1);
    }
}
