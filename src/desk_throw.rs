//! angel-niri: windows thrown between desks as stacking (the shell's Desktop) starts and ends.
//! `niri msg action set-desk-throw [--on false] [--strength 0|1|2]`; off until a shell turns it
//! on. The strength (calm, usual, wild) sets how far apart they come, their trails, the hit's
//! freeze and shake, and how big and long the break is (`Feel`); calm breaks nothing.
//! - Starting, the windows gathered from desks 2…N onto desk 1 are thrown about it, the screen a
//!   table seen from above and its edges walls (`Fly::arrive`, worked out ahead): one after
//!   another each ducks where it stood and shoots off upwards, give or take, speeding up into the
//!   top edge — no slowing before it — freezes at the hit, bounces off and slides on, braking,
//!   off every edge it meets, and stays wherever it stops (till stacking ends). Never off the
//!   screen, never a jump: the layout moves it there at once and draws the flight from where it
//!   stood. Its app gives it a manner (`Path`), every throw shakes it up; from desks further away
//!   they go harder. Where it hit hardest it is dented a while, straightening in steps.
//! - Ending, the gathered windows still on desk 1 drop off the bottom before they go back to their
//!   desks. No damage then.
//! Every wall it hits sounds, as hard as it hit (`Event::DesksThrown`, also its slide and
//! whoosh). A hit from `CRACK` up cracks the monitor — the picture itself, in screen casts too
//! (shaders/glitch.frag over the finished frame): the glass cracked round the hit (jagged rays
//! with branches, a web between them, crushed shards in the middle), the harder the more; from
//! `BLOT` up a jolt (the harder the further) tearing it, a black blot of leaked liquid crystal
//! with dark veins bending the picture around its edge, long lines across the whole screen, dead
//! pixels of the monitor's own size flickering. The dead pixels and lines heal first; a couple of
//! seconds later the blot pulls back into the impact, the cracks with it. Every hit adds its own
//! (`glitch`): the blots grow and run together into one, past `SPOTS` a hard hit swells the
//! nearest. The pointer goes under the damage then (it is no different from the rest of the
//! picture).
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
    /// Leaving: the windows this far apart, ms.
    gap_ms: u64,
    /// Leaving: how long a window takes to drop off the bottom, ms.
    drop_ms: u64,
    /// How fast a window sets off, logical px/s: from the next desk, from one further away.
    near_speed: f64,
    far_speed: f64,
    /// Hard hits break the screen.
    damage: bool,
    /// How long the break lasts after the last hit, ms.
    life_ms: f64,
    /// Every blot this much bigger.
    blot: f64,
    /// Afterimages trailing a far one, a near or leaving one.
    trail_far: usize,
    trail_near: usize,
    /// A far one freezes this long at its first hit (a near one half).
    hit_stop_ms: u64,
    /// The screen jolts this far (logical px) at a hit of power 1.
    shake: f32,
}

const FEELS: [Feel; 3] = [
    Feel {
        gap_ms: 90,
        drop_ms: 420,
        near_speed: 900.,
        far_speed: 1500.,
        damage: false,
        life_ms: 0.,
        blot: 1.,
        trail_far: 0,
        trail_near: 0,
        hit_stop_ms: 0,
        shake: 0.,
    },
    Feel {
        gap_ms: 150,
        drop_ms: 520,
        near_speed: 1300.,
        far_speed: 2300.,
        damage: true,
        life_ms: 8500.,
        blot: 1.,
        trail_far: 2,
        trail_near: 0,
        hit_stop_ms: 90,
        shake: 14.,
    },
    Feel {
        gap_ms: 180,
        drop_ms: 560,
        near_speed: 1500.,
        far_speed: 2800.,
        damage: true,
        life_ms: 11000.,
        blot: 1.15,
        trail_far: 3,
        trail_near: 2,
        hit_stop_ms: 140,
        shake: 24.,
    },
];

pub fn feel() -> Feel {
    FEELS[usize::from(STRENGTH.get().min(2))]
}

pub fn set_strength(strength: u8) {
    STRENGTH.set(strength.min(2));
}

/// A hit's damage: on `output` at `pos` (logical, the edge hit), from `start` (the clock's), how
/// hard (`CRACK`…1) and the wall's inward normal; the hard hits that came past `SPOTS` and swelled
/// it instead, likewise.
#[derive(Debug, Clone)]
struct Glitch {
    output: String,
    pos: [f64; 2],
    start: Duration,
    power: f64,
    normal: [f64; 2],
    seed: f64,
    swells: Vec<(Duration, f64, [f64; 2])>,
}

/// At most this many blots on one output at once (shaders/glitch.frag `g_h0…`); more hard hits
/// swell the nearest.
pub const SPOTS: usize = 6;
/// The damage changes this often: it stutters, it doesn't flow.
const STEP_MS: f64 = 70.;

/// One frame of a broken screen: each hit as (x, y logical, its age in s or -1 for none, its
/// seed), how hard it was and how many hits swelled it, the stutter step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlitchFrame {
    pub spots: [[f32; 4]; SPOTS],
    pub power: [f32; SPOTS],
    pub swells: [f32; SPOTS],
    pub step: f32,
    /// How much bigger every blot is for the hard hits so far (1 with one) and the strength.
    pub grow: f32,
    /// Until all of it has healed, s: the hits heal together, after the last one.
    pub left: f32,
    /// The latest hit's age, s, how far it jolts the screen (logical px) and which way (the
    /// wall's inward normal).
    pub jolt: [f32; 4],
    /// The latest hard hit (its start, ms, as a key) and where, logical: the frame around it is
    /// kept once for its stuck pieces of the window that hit.
    pub snap: Option<(u64, [f32; 2])>,
}

/// Each hard hit after the first makes every blot this much bigger, growing over `GROW_MS`.
const GROW_PER_HIT: f64 = 0.2;
const GROW_MS: f64 = 500.;
/// Past this many hits after the first it grows no more: the screen stays a screen.
const GROW_HITS: f64 = 4.;

/// A hit on `output` at `pos`, `power` hard (0…1) into a wall of inward `normal`, `delay` after
/// `now`: it adds its damage to whatever is there; a hard one makes it all bigger. Nothing with a
/// calm throw or below `CRACK`.
pub fn glitch(
    output: &str,
    pos: [f64; 2],
    power: f64,
    normal: [f64; 2],
    now: Duration,
    delay: Duration,
) {
    if !feel().damage || power < CRACK {
        return;
    }
    GLITCHES.with_borrow_mut(|all| {
        let start = now + delay;
        let mine = all.iter().filter(|g| g.output == output).count();
        if mine >= SPOTS {
            // no room of its own: a hard one swells the nearest, they run together anyway
            if power < BLOT {
                return;
            }
            let dist = |g: &Glitch| (g.pos[0] - pos[0]).hypot(g.pos[1] - pos[1]);
            let nearest = (all.iter_mut().filter(|g| g.output == output))
                .min_by(|a, b| dist(a).total_cmp(&dist(b)))
                .unwrap();
            nearest.swells.push((start, power, normal));
            return;
        }
        let seed = (start.as_millis() % 9973) as f64 * 0.618;
        all.push(Glitch {
            output: output.to_owned(),
            pos,
            start,
            power,
            normal,
            seed,
            swells: Vec::new(),
        });
    });
}

/// Every hit on `output`, blots and swells: when, how hard, which way, where.
fn hits_on<'a>(
    all: &'a [Glitch],
    output: &'a str,
) -> impl Iterator<Item = (Duration, f64, [f64; 2], [f64; 2])> + 'a {
    (all.iter().filter(move |g| g.output == output)).flat_map(|g| {
        std::iter::once((g.start, g.power, g.normal))
            .chain(g.swells.iter().copied())
            .map(move |(at, power, normal)| (at, power, normal, g.pos))
    })
}

/// When the damage on `output` has healed: the strength's life after its last hit.
fn healed_at(all: &[Glitch], output: &str) -> Option<Duration> {
    let last = hits_on(all, output).map(|h| h.0).max()?;
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
        let mut power = [0.; SPOTS];
        let mut swells = [0.; SPOTS];
        let mine = all.iter().filter(|g| g.output == output && g.start <= now);
        for (i, g) in mine.enumerate() {
            let age = (now - g.start).as_secs_f64();
            spots[i] = [g.pos[0] as f32, g.pos[1] as f32, age as f32, g.seed as f32];
            power[i] = g.power as f32;
            swells[i] = (g.swells.iter())
                .filter(|(at, _, _)| *at <= now)
                .map(|(at, _, _)| grown(*at))
                .sum::<f64>() as f32;
        }
        let came: Vec<_> = hits_on(all, output).filter(|h| h.0 <= now).collect();
        let latest = came.iter().max_by_key(|h| h.0)?;
        let hard: Vec<_> = came.iter().filter(|h| h.1 >= BLOT).collect();
        let snap = (hard.iter().max_by_key(|h| h.0))
            .map(|h| (h.0.as_millis() as u64, [h.3[0] as f32, h.3[1] as f32]));
        let hits: f64 = hard.iter().map(|h| grown(h.0)).sum();
        let feel = feel();
        let step = (now.as_secs_f64() * 1000. / STEP_MS).floor() as f32;
        Some(GlitchFrame {
            spots,
            power,
            swells,
            step,
            grow: ((1. + GROW_PER_HIT * (hits - 1.).clamp(0., GROW_HITS)) * feel.blot) as f32,
            left: (end - now).as_secs_f32(),
            jolt: [
                (now - latest.0).as_secs_f32(),
                feel.shake * (0.25 + 0.75 * latest.1 as f32),
                latest.2[0] as f32,
                latest.2[1] as f32,
            ],
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
        for hit in throws.iter().filter(|hit| !hit.bottom && !hit.slide) {
            let delay = Duration::from_millis(hit.in_ms);
            glitch(&hit.output, hit.pos, hit.power, hit.normal, now, delay);
        }
        if !throws.is_empty() {
            if let Some(server) = &self.niri.ipc_server {
                server.send_transient_event(Event::DesksThrown { throws });
            }
        }
        self.niri.queue_redraw_all();
    }
}

/// Below the output, how far past its edge a leaving window ends.
const MARGIN: f64 = 16.;
/// The afterimages lag this far behind each other, ms of the flight.
const GHOST_MS: f64 = 24.;
const GHOST_ALPHA: [f32; 3] = [0.5, 0.3, 0.16];
/// An arriving flight is worked out ahead, one step this long, ms.
const SIM_MS: f64 = 1000. / 240.;
/// Arriving, a window comes up from below the screen this far under its bottom edge, logical px.
const BELOW: f64 = 8.;
/// Until its first hit it speeds up by this, logical px/s², up to `TOP_SPEED` times as fast as it
/// set off: it hits at its fastest.
const BOOST: f64 = 2600.;
const TOP_SPEED: f64 = 1.45;
/// Sliding after it, it brakes: a share of its speed each second (`Fly::arrive`) and this, px/s².
const BRAKE: f64 = 260.;
/// Slower than this (px/s) it has stopped.
const STOP: f64 = 22.;
/// Into a wall this fast (px/s) is a hit of power 1; slower than `QUIET` makes no sound.
const HARD: f64 = 2400.;
const QUIET: f64 = 110.;
/// A hit this hard and up (0…1) cracks the glass; one past `BLOT` leaks a blot as well, a jolt
/// and the freeze.
pub const CRACK: f64 = 0.2;
pub const BLOT: f64 = 0.55;
/// A dent stays this long, then straightens in steps over `DENT_HEAL`, ms.
const DENT_HOLD: f64 = 1300.;
const DENT_HEAL: f64 = 700.;
/// Sliding longer than this after its first hit, ms, it is braked harder and harder.
const SLIDE_MS: f64 = 700.;
/// Never longer than this, ms: it is braked to a stop by then.
const LONGEST_MS: f64 = 3200.;

/// A window's own way of flying: twelve rolls, each part its app's liking and part this throw's
/// chance (so an app keeps a manner, yet no two throws are alike). `Fly::new` makes a flight out
/// of them: which way it sets off, how fast, how it leans, how springy and slippery it is.
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

/// Half the width and height of the box a window of `size` turned by `angle` takes.
fn half(size: (f64, f64), angle: f64) -> (f64, f64) {
    let (s, c) = (angle.sin().abs(), angle.cos().abs());
    (
        (size.0 * c + size.1 * s) / 2.,
        (size.0 * s + size.1 * c) / 2.,
    )
}

fn turn(p: (f64, f64), angle: f64) -> (f64, f64) {
    let (s, c) = angle.sin_cos();
    (p.0 * c - p.1 * s, p.0 * s + p.1 * c)
}

/// 0…1 from `x`, well mixed (splitmix64).
fn unit(x: u64) -> f64 {
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// A cubic Bézier through offsets, at `s`.
fn bezier(p: &[(f64, f64); 4], s: f64) -> (f64, f64) {
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

/// Where an arriving window hits a wall, worked out ahead: ms into its flight, where on the
/// screen (logical), how hard (0…1), the wall's inward normal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contact {
    pub at: f64,
    pub pos: (f64, f64),
    pub power: f64,
    pub normal: (f64, f64),
}

/// A sound a flight makes, from now: where on the screen, how hard; a slide (it scrapes on after
/// its first hit), the one of it that sets off (its whoosh then).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sound {
    pub at: Duration,
    pub pos: [f64; 2],
    pub power: f64,
    pub normal: [f64; 2],
    pub slide: bool,
    pub launch: bool,
}

/// The window crushed where it hit hardest: ms into the flight, the spot on its edge and the way
/// into it (its own, unturned), how deep and how far along the edge, logical px.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Dent {
    at: f64,
    spot: (f64, f64),
    inward: (f64, f64),
    depth: f64,
    reach: f64,
}

/// One window's flight; offsets are from where it comes to rest.
///
/// Arriving, it is worked out ahead (`arrive`): on the screen as a table seen from above, the
/// window ducks, sets off from where it stood, speeds up into the top edge, freezes at the hit,
/// bounces off it and slides on, braking, off every edge it meets, and stops wherever it gets to
/// (the layout moves it there at once, `shift`; the flight is drawn from where it stood). It
/// never leaves the screen. Leaving, it hops and drops off the bottom along a curve.
#[derive(Debug)]
pub struct Fly {
    anim: Animation,
    /// All of it, ms.
    ms: f64,
    pub leaving: bool,
    pub far: bool,
    /// Arriving: where it is drawn every `SIM_MS` and its turn; the walls it hits; when it sets
    /// off (after ducking) and when it scrapes on after its first hit (ms, how hard).
    poses: Vec<(f64, f64, f64)>,
    contacts: Vec<Contact>,
    launch: f64,
    slide: Option<(f64, f64)>,
    dent: Option<Dent>,
    /// Arriving: where it comes to rest, from where it stood.
    pub shift: (f64, f64),
    /// Leaving: its way down past the bottom, its lean and bank, the hop up before the drop.
    way: [(f64, f64); 4],
    spin: f64,
    bank: f64,
    hop: f64,
    rest: (f64, f64),
    size: (f64, f64),
    view: (f64, f64),
    ghosts: usize,
}

impl Fly {
    /// A flight starting `delay` from now; `rest`: where the window's top left is in its output,
    /// `size`: the window's, `view`: the output's; `path`: its rolls.
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
        let mut fly = Self {
            anim: Animation::ease(clock.clone(), 0., 1., 0., 1, Curve::Linear),
            ms: 1.,
            leaving,
            far,
            poses: Vec::new(),
            contacts: Vec::new(),
            launch: 0.,
            slide: None,
            dent: None,
            shift: (0., 0.),
            way: [(0., 0.); 4],
            spin: 0.,
            bank: 0.,
            hop: 0.,
            rest,
            size,
            view,
            ghosts: if far && !leaving {
                feel.trail_far
            } else {
                feel.trail_near
            },
        };
        if leaving {
            fly.leave(path.r, feel);
        } else {
            fly.arrive(path.r, feel);
        }
        fly.anim =
            Animation::ease(clock, 0., 1., 0., fly.ms.ceil() as u64, Curve::Linear).delayed(delay);
        fly
    }

    /// Down past the bottom along a curve of its own, faster and faster, after a hop.
    fn leave(&mut self, r: [f64; 12], feel: Feel) {
        let (x, y) = self.rest;
        let w = self.size.0;
        let (vw, vh) = self.view;
        self.ms = (feel.drop_ms as f64 * (0.8 + 0.4 * r[9])).round();
        let left = r[1] < 0.5;
        let lean = if left { 1. } else { -1. } * if r[8] < 0.2 { -1. } else { 1. };
        self.spin = lean * (0.025 + 0.07 * r[6] + 0.03 * r[7]);
        self.bank = (r[10] * 2. - 1.) * 0.06;
        self.hop = 14. + 22. * r[7];
        let from = (x, y);
        let off = (r[2] * 2. - 1.) * vw * 0.25 + if left { -0.2 } else { 0.2 } * w;
        let to = (x + off, vh + MARGIN + 40.);
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
        self.way = [off(from), off(c1), off(c2), off(to)];
    }

    /// Works the flight out: the screen a table seen from above, its edges walls.
    fn arrive(&mut self, r: [f64; 12], feel: Feel) {
        let size = self.size;
        let (w, h) = size;
        let (vw, vh) = self.view;
        let dt = SIM_MS / 1000.;
        // from below the screen, about under where it stood
        let x = self.rest.0 + w / 2. + (r[2] * 2. - 1.) * vw * 0.15;
        let x = if w < vw {
            x.clamp(w / 2., vw - w / 2.)
        } else {
            vw / 2.
        };
        let mut c = (x, vh + h / 2. + BELOW);

        // straight up give or take 50°, leaning the way it goes; never so far aside that it
        // meets a side before the top
        let up = (c.1 - h / 2.).max(1.);
        let aside = if r[1] < 0.5 {
            c.0 - w / 2.
        } else {
            vw - c.0 - w / 2.
        };
        let tilt = (r[1] * 2. - 1.) * 0.85;
        let tilt = tilt.signum() * tilt.abs().min((0.7 * aside.max(0.) / up).atan());
        let lean = if tilt < 0. { -1. } else { 1. } * (0.03 + 0.06 * r[6]);
        let speed = if self.far {
            feel.far_speed
        } else {
            feel.near_speed
        } * (0.88 + 0.24 * r[9]);
        let mut v = (tilt.sin() * speed, -tilt.cos() * speed);
        // how springy its first hit is, how slippery it is after
        let first_bounce = 0.3 + 0.3 * r[3];
        let slip = 1.0 + 1.6 * r[4];
        // and which way, how hard the top throws it aside (it hit with a corner)
        let glance = if r[10] < 0.5 { -1. } else { 1. } * (0.2 + 0.5 * r[11]);

        let top = speed * TOP_SPEED;
        let mut a = -lean * 0.5;
        // all of it on the screen sideways, leaning as it is
        let hx0 = half(size, a).0;
        if hx0 < vw / 2. {
            c.0 = c.0.clamp(hx0, vw - hx0);
        }
        let mut poses = vec![(c.0, c.1, a)];
        let mut spin = 0.;
        let mut t = SIM_MS;
        self.launch = 0.;

        // below the screen, it may stay out as far as it is until it comes in: the walls there
        // close in behind it
        let (hx0, hy0) = half(size, a);
        let over = |p: f64, half: f64, len: f64| ((half - p).max(0.), (p + half - len).max(0.));
        let mut slack = [over(c.1, hy0, vh), over(c.0, hx0, vw)];
        let mut hit: Option<f64> = None;
        let mut contacts = Vec::new();
        let mut dent: Option<Dent> = None;
        let mut slide = None;
        loop {
            let since = hit.map_or(0., |at| t - at);
            let s = v.0.hypot(v.1);
            if hit.is_none() {
                if s > 0. {
                    let k = (s + BOOST * dt).min(top.max(s)) / s;
                    v = (v.0 * k, v.1 * k);
                }
            } else {
                // braked harder and harder past `SLIDE_MS`: the show stays short
                let grip = slip + 10. * ((since - SLIDE_MS) / 1000.).max(0.);
                let k = (-grip * dt).exp();
                let s2 = (s * k - BRAKE * dt).max(0.);
                let k = if s > 0. { s2 / s } else { 0. };
                v = (v.0 * k, v.1 * k);
            }
            // its lean, on a spring: into its way until the hit, flat after
            let target = if hit.is_none() { lean } else { 0. };
            let stiff = if since > SLIDE_MS { 400. } else { 140. };
            spin += (-stiff * (a - target) - 9. * spin) * dt;
            a = (a + spin * dt).clamp(-0.4, 0.4);
            let (hx, hy) = half(size, a);
            // out there after its hit, it is drawn in
            if hit.is_some() {
                let push = |p: f64, half: f64, len: f64| {
                    if p - half < 0. {
                        1.
                    } else if p + half > len {
                        -1.
                    } else {
                        0.
                    }
                };
                v.0 += push(c.0, hx, vw) * 900. * dt;
                v.1 += push(c.1, hy, vh) * 900. * dt;
            }
            c = (c.0 + v.0 * dt, c.1 + v.1 * dt);
            // the walls close in behind it as it comes in
            let room = [(hy, vh - hy), (hx, vw - hx)];
            let mut walls = room;
            for (k, p) in [c.1, c.0].into_iter().enumerate() {
                let (lo, hi) = room[k];
                slack[k].0 = slack[k].0.min((lo - p).max(0.));
                slack[k].1 = slack[k].1.min((p - hi).max(0.));
                walls[k] = (lo - slack[k].0, hi + slack[k].1);
            }
            let corners = [
                (-w / 2., -h / 2.),
                (w / 2., -h / 2.),
                (w / 2., h / 2.),
                (-w / 2., h / 2.),
            ]
            .map(|p| turn(p, a));
            let mut hold = 0.;
            // top, left, right, bottom
            for (axis, side) in [(1, -1.), (0, -1.), (0, 1.), (1, 1.)] {
                let (p, vel, (lo, hi)) = if axis == 0 {
                    (&mut c.0, &mut v.0, walls[1])
                } else {
                    (&mut c.1, &mut v.1, walls[0])
                };
                if hi < lo {
                    // bigger than the screen this way: nothing to bounce off
                    *p = (lo + hi) / 2.;
                    *vel = 0.;
                    if axis == 1 && hit.is_none() {
                        hit = Some(t);
                    }
                    continue;
                }
                let past = if side < 0. { *p < lo } else { *p > hi };
                if !past {
                    continue;
                }
                *p = if side < 0. { lo } else { hi };
                if *vel * side <= 0. {
                    continue;
                }
                let vn = vel.abs();
                let bounce = if hit.is_none() { first_bounce } else { 0.5 };
                *vel = -*vel * bounce;
                if axis == 0 {
                    v.1 *= 0.84;
                } else {
                    v.0 *= 0.84;
                    if hit.is_none() {
                        v.0 += glance * vn;
                    }
                }
                // the corner that touched, from the middle; the wall pushes it back by `j`
                let key = |q: &(f64, f64)| if axis == 0 { q.0 * side } else { q.1 * side };
                let corner = corners
                    .iter()
                    .copied()
                    .max_by(|p, q| key(p).total_cmp(&key(q)))
                    .unwrap();
                let normal = if axis == 0 { (-side, 0.) } else { (0., -side) };
                let push = (1. + bounce) * vn;
                let j = (normal.0 * push, normal.1 * push);
                spin += 0.9 * (corner.0 * j.1 - corner.1 * j.0) / (hx * hx + hy * hy).max(1.);
                if vn < QUIET {
                    continue;
                }
                let power = ((vn - QUIET) / (HARD - QUIET)).clamp(0., 1.);
                let pos = (
                    (c.0 + corner.0).clamp(0., vw),
                    (c.1 + corner.1).clamp(0., vh),
                );
                contacts.push(Contact {
                    at: t,
                    pos,
                    power,
                    normal,
                });
                if power >= 0.3 && dent.is_none_or(|d| power * 60. > d.depth) {
                    let depth = (power * 0.16 * w.min(h)).min(power * 60.);
                    let local = turn(corner, -a);
                    dent = Some(Dent {
                        at: t,
                        spot: (local.0 + w / 2., local.1 + h / 2.),
                        inward: turn(normal, -a),
                        depth,
                        reach: depth * 3.2 + 20.,
                    });
                }
                let first = hit.is_none();
                hold = f64::max(
                    hold,
                    if first {
                        feel.hit_stop_ms as f64 * if self.far { 1. } else { 0.5 }
                    } else if power > BLOT {
                        35.
                    } else {
                        0.
                    },
                );
                if first {
                    hit = Some(t);
                    let after = v.0.hypot(v.1);
                    slide = Some((t + hold + 30., (after / 1600.).min(1.)));
                }
            }

            let pose = (c.0, c.1, a);
            poses.push(pose);
            let n = (hold / SIM_MS).round() as usize;
            poses.extend(std::iter::repeat_n(pose, n));
            t += SIM_MS * (1 + n) as f64;

            let still = v.0.hypot(v.1) < STOP && a.abs() < 0.003 && spin.abs() < 0.05;
            if (hit.is_some() && still) || t > LONGEST_MS {
                break;
            }
        }

        // at rest: flat, where it got to; everything from there
        let end = (c.0 - w / 2., c.1 - h / 2.);
        self.poses = poses
            .into_iter()
            .map(|(x, y, a)| (x - w / 2. - end.0, y - h / 2. - end.1, a))
            .chain(std::iter::once((0., 0., 0.)))
            .collect();
        self.shift = (end.0 - self.rest.0, end.1 - self.rest.1);
        self.contacts = contacts;
        self.slide = slide;
        self.dent = dent;
        self.ms = (self.poses.len() - 1) as f64 * SIM_MS;
        if let Some(d) = dent {
            self.ms = self.ms.max(d.at + DENT_HOLD + DENT_HEAL);
        }
    }

    pub fn is_done(&self) -> bool {
        self.anim.is_done()
    }

    /// How many ms into the flight it is (0 before it sets off).
    fn elapsed(&self) -> f64 {
        self.anim.value().clamp(0., 1.) * self.ms
    }

    /// Where it is drawn `at` ms into the flight, from where it comes to rest, and its turn.
    fn pose(&self, at: f64) -> (f64, f64, f64) {
        if self.leaving {
            let t = (at / self.ms).clamp(0., 1.);
            // a hop up, then down past the bottom along its way, faster and faster
            let hop = if t < 0.35 {
                self.hop * (t / 0.35 * std::f64::consts::PI).sin()
            } else {
                0.
            };
            let (x, y) = bezier(&self.way, t.powi(3));
            let angle = if t <= 0. || t >= 1. {
                0.
            } else {
                -self.spin * t + self.bank * (t * std::f64::consts::PI).sin()
            };
            // never through the top edge: up against it
            let top = self.rest.1 - self.rest.1.min(0.);
            let y = (y - hop).max(corner_rise(self.size, angle) - top);
            return (x, y, angle);
        }
        let k = at.max(0.) / SIM_MS;
        let i = k.floor() as usize;
        if i + 1 >= self.poses.len() {
            return (0., 0., 0.);
        }
        let (a, b) = (self.poses[i], self.poses[i + 1]);
        let f = k - i as f64;
        (lerp(a.0, b.0, f), lerp(a.1, b.1, f), lerp(a.2, b.2, f))
    }

    /// Where it is drawn, from where it comes to rest.
    pub fn offset(&self) -> Point<f64, Logical> {
        let (x, y, _) = self.pose(self.elapsed());
        Point::from((x, y))
    }

    /// The turn it is drawn with, radians.
    pub fn angle(&self) -> f64 {
        self.pose(self.elapsed()).2
    }

    /// The dent now, if any: the spot on its edge, how deep, how far along it (logical px, its
    /// own unturned box), the way into it.
    pub fn dent(&self) -> Option<([f32; 4], [f32; 2])> {
        let d = self.dent?;
        let at = self.elapsed() - d.at;
        if at < 0. || self.is_done() {
            return None;
        }
        // in at once, out in four steps
        let left = 1. - ((at - DENT_HOLD) / DENT_HEAL).clamp(0., 1.);
        let k = (at / 40.).min(1.) * (left * 4.).ceil() / 4.;
        (k > 0.).then(|| {
            (
                [
                    d.spot.0 as f32,
                    d.spot.1 as f32,
                    (d.depth * k) as f32,
                    d.reach as f32,
                ],
                [d.inward.0 as f32, d.inward.1 as f32],
            )
        })
    }

    /// Its afterimages now, nearest first: where it was a moment ago, from where it is.
    pub fn trail(&self) -> Vec<Ghost> {
        let at = self.elapsed();
        if self.ghosts == 0 || at <= 0. || at >= self.ms {
            return Vec::new();
        }
        let (x, y, _) = self.pose(at);
        (1..=self.ghosts)
            .filter_map(|k| {
                let back = at - k as f64 * GHOST_MS;
                if back <= 0. {
                    return None;
                }
                let (bx, by, angle) = self.pose(back);
                let offset = Point::from((bx - x, by - y));
                // standing (the freeze) or barely moving: nothing trails
                (offset.x.hypot(offset.y) >= 6.).then(|| Ghost {
                    offset,
                    angle,
                    alpha: GHOST_ALPHA[(k - 1).min(2)],
                })
            })
            .collect()
    }

    /// When it sets off, from now.
    pub fn off_in(&self, now: Duration) -> Duration {
        let at = self.anim.start_time() + Duration::from_secs_f64(self.launch / 1000.);
        at.saturating_sub(now)
    }

    /// Its sounds, from now: arriving, every wall it hits and its slide after the first; leaving,
    /// the drop.
    pub fn sounds(&self, now: Duration) -> Vec<Sound> {
        let start = self.anim.start_time();
        let from_now =
            |ms: f64| (start + Duration::from_secs_f64(ms.max(0.) / 1000.)).saturating_sub(now);
        if self.leaving {
            let (x, _, _) = self.pose(self.ms * 0.3);
            return vec![Sound {
                at: from_now(self.ms * 0.3),
                pos: [self.rest.0 + x + self.size.0 / 2., self.view.1],
                power: 1.,
                normal: [0., -1.],
                slide: false,
                launch: false,
            }];
        }
        let mut out: Vec<_> = (self.contacts.iter())
            .map(|c| Sound {
                at: from_now(c.at),
                pos: [c.pos.0, c.pos.1],
                power: c.power,
                normal: [c.normal.0, c.normal.1],
                slide: false,
                launch: false,
            })
            .collect();
        if let (Some((at, power)), Some(first)) = (self.slide, self.contacts.first()) {
            if power > 0.05 {
                out.push(Sound {
                    at: from_now(at),
                    pos: [first.pos.0, first.pos.1],
                    power,
                    normal: [first.normal.0, first.normal.1],
                    slide: true,
                    launch: false,
                });
            }
        }
        if let Some(first) = out.first_mut() {
            first.launch = true;
        }
        out
    }
}

/// When each of a row of windows (`far` flags, the near ones first) sets off, from now. Arriving:
/// one every `GAP_MS`, however many (a show, not a hurry). Leaving: quickly, all of them off
/// within `SPREAD_MS`.
pub fn delays(far: &[bool], leaving: bool) -> Vec<Duration> {
    let n = far.len() as u64;
    let gap = if !leaving {
        GAP_MS as f64
    } else if n > 1 {
        (feel().gap_ms as f64).min(SPREAD_MS as f64 / (n - 1) as f64)
    } else {
        0.
    };
    (0..n)
        .map(|i| Duration::from_millis((i as f64 * gap).round() as u64))
        .collect()
}

/// Arriving, a window sets off this long after the one before, ms.
const GAP_MS: u64 = 300;
/// Leaving, a throw's windows all set off within this, ms.
const SPREAD_MS: u64 = 1100;

/// The longest a leaving flight takes.
pub fn longest() -> Duration {
    Duration::from_millis((feel().drop_ms as f64 * 1.21).ceil() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: (f64, f64) = (1920., 1080.);
    const SIZE: (f64, f64) = (400., 300.);

    fn fly_at(leaving: bool, far: bool, rest: (f64, f64), r: [f64; 12]) -> Fly {
        Fly::new(
            Clock::with_time(Duration::ZERO),
            Duration::ZERO,
            leaving,
            far,
            rest,
            SIZE,
            VIEW,
            Path { r },
        )
    }

    fn fly_with(leaving: bool, far: bool, r: [f64; 12]) -> Fly {
        fly_at(leaving, far, (300., 200.), r)
    }

    fn fly(leaving: bool, far: bool) -> Fly {
        fly_with(leaving, far, [0.3; 12])
    }

    #[test]
    fn it_comes_up_from_below_stays_on_the_screen_and_stops_anywhere() {
        let mut ends = Vec::new();
        for i in 0..40 {
            let rest = ((100 + 37 * i) as f64 % 1500., (i * 61 % 700) as f64);
            let far = i % 2 == 0;
            let f = fly_at(false, far, rest, Path::new("kitty", i, i as u64 * 977).r);
            // from below the screen, out of sight
            let end = (rest.0 + f.shift.0, rest.1 + f.shift.1);
            let (_, y, _) = f.pose(0.);
            assert!(end.1 + y >= VIEW.1, "{i}");
            assert_eq!(f.pose(f.ms + 1.), (0., 0., 0.));
            // the screen's edges are walls once it is in; it moves, it never jumps
            let mut inside = false;
            for k in 0..f.poses.len() {
                let (x, y, a) = f.poses[k];
                if k > 0 {
                    let (px, py, _) = f.poses[k - 1];
                    assert!((x - px).hypot(y - py) < 20., "{i} {k} jumps");
                }
                let (hx, hy) = half(SIZE, a);
                let (cx, cy) = (end.0 + x + SIZE.0 / 2., end.1 + y + SIZE.1 / 2.);
                assert!(cx - hx > -0.5 && cx + hx < VIEW.0 + 0.5, "{i} {k} x {cx}");
                assert!(cy - hy > -0.5, "{i} {k} y {cy}");
                inside |= cy + hy < VIEW.1 + 0.5;
                assert!(!inside || cy + hy < VIEW.1 + 0.5, "{i} {k} y {cy}");
            }
            // the first thing it hits is the top
            let first = f.contacts[0];
            assert_eq!(first.normal, (0., 1.), "{i}");
            assert!(first.pos.1 < 1.);
            // done within the show (the dent heals after, standing)
            assert!(f.poses.len() as f64 * SIM_MS <= 2600., "{i} {}", f.ms);
            assert!(f.ms <= 3200., "{i} {}", f.ms);
            ends.push(end);
        }
        // anywhere: they don't all end up in one spot
        let spread = |k: fn(&(f64, f64)) -> f64| {
            let (lo, hi) = (
                ends.iter().map(k).fold(f64::MAX, f64::min),
                ends.iter().map(k).fold(f64::MIN, f64::max),
            );
            hi - lo
        };
        assert!(spread(|e| e.0) > 600. && spread(|e| e.1) > 300.);
    }

    #[test]
    fn partly_off_the_screen_it_comes_in_without_a_jump() {
        for (i, rest) in [(1700., 300.), (-150., 500.), (800., 900.)]
            .into_iter()
            .enumerate()
        {
            let f = fly_at(false, true, rest, Path::new("kitty", i, 3).r);
            for w in f.poses.windows(2) {
                assert!((w[1].0 - w[0].0).hypot(w[1].1 - w[0].1) < 20., "{i}");
            }
            let end = (rest.0 + f.shift.0, rest.1 + f.shift.1);
            assert!(
                end.0 >= -0.5 && end.0 + SIZE.0 <= VIEW.0 + 0.5,
                "{i} {end:?}"
            );
            assert!(
                end.1 >= -0.5 && end.1 + SIZE.1 <= VIEW.1 + 0.5,
                "{i} {end:?}"
            );
        }
    }

    #[test]
    fn it_hits_the_top_at_its_fastest_far_ones_harder() {
        for i in 0..20 {
            let r = Path::new("kitty", i, 5).r;
            let far = fly_at(false, true, (700., 500.), r);
            let near = fly_at(false, false, (700., 500.), r);
            assert!(far.contacts[0].power > near.contacts[0].power, "{i}");
            assert!(far.contacts[0].power > BLOT, "{i}");
            // speeding up to it: every step up longer than the one before
            let up: Vec<_> = far.poses.windows(2).map(|w| w[0].1 - w[1].1).collect();
            let hit = (far.contacts[0].at / SIM_MS).round() as usize;
            for k in 1..hit.saturating_sub(1) {
                assert!(up[k] >= up[k - 1] - 1e-6, "{i} {k}");
            }
        }
    }

    #[test]
    fn standing_at_the_top_it_still_hits_hard() {
        let f = fly_at(false, true, (600., 0.), [0.5; 12]);
        assert!(f.contacts[0].power > BLOT);
    }

    #[test]
    fn a_hard_hit_freezes_and_dents_it() {
        let f = fly(false, true);
        let c = f.contacts[0];
        let k = (c.at / SIM_MS).round() as usize;
        assert_eq!(f.poses[k + 1], f.poses[k + 3], "it stands at the hit");
        let d = f.dent.unwrap();
        assert!(d.depth > 10. && d.at == c.at);
        // a dent shows after the hit, straightens later
        let mut clock = Clock::with_time(Duration::ZERO);
        let f = Fly::new(
            clock.clone(),
            Duration::ZERO,
            false,
            true,
            (300., 200.),
            SIZE,
            VIEW,
            Path { r: [0.3; 12] },
        );
        let at = |ms: f64| Duration::from_secs_f64(ms / 1000.);
        clock.set_unadjusted(at(d.at - 5.));
        assert_eq!(f.dent(), None);
        clock.set_unadjusted(at(d.at + 300.));
        let deep = f.dent().unwrap().0[2];
        assert!((f64::from(deep) - d.depth).abs() < 1e-3);
        clock.set_unadjusted(at(d.at + DENT_HOLD + DENT_HEAL * 0.6));
        assert!(f.dent().unwrap().0[2] < deep);
        clock.set_unadjusted(at(d.at + DENT_HOLD + DENT_HEAL + 10.));
        assert_eq!(f.dent(), None);
    }

    #[test]
    fn its_sounds_are_its_hits_and_a_slide() {
        let f = fly(false, true);
        let sounds = f.sounds(Duration::ZERO);
        assert_eq!(sounds.iter().filter(|s| !s.slide).count(), f.contacts.len());
        assert_eq!(sounds.iter().filter(|s| s.slide).count(), 1);
        assert!(sounds[0].launch && sounds.iter().filter(|s| s.launch).count() == 1);
        assert_eq!(
            f.off_in(Duration::ZERO),
            Duration::from_secs_f64(f.launch / 1000.)
        );
    }

    #[test]
    fn leaving_ends_below() {
        let f = fly(true, true);
        assert_eq!(f.pose(0.).0, 0.);
        assert!(f.pose(f.ms).1 + 200. > 1080.);
        let s = f.sounds(Duration::ZERO);
        assert!(s.len() == 1 && s[0].pos[1] == 1080.);
    }

    #[test]
    fn arriving_one_every_gap_leaving_all_within_the_spread() {
        let many = delays(&[true; 12], false);
        for (i, d) in many.iter().enumerate() {
            assert_eq!(*d, Duration::from_millis(i as u64 * GAP_MS));
        }
        let back = delays(&[false, true, true], true);
        assert_eq!(back[2], Duration::from_millis(feel().gap_ms * 2));
        let back = delays(&[true; 12], true);
        assert!(back[11] <= Duration::from_millis(SPREAD_MS));
    }

    #[test]
    fn every_hard_hit_makes_it_bigger_and_all_heals_together() {
        let life = feel().life_ms as u64;
        let up = [0., 1.];
        glitch(
            "A",
            [100., 0.],
            1.,
            up,
            Duration::ZERO,
            Duration::from_millis(100),
        );
        glitch(
            "A",
            [500., 0.],
            1.,
            up,
            Duration::ZERO,
            Duration::from_millis(500),
        );
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
        assert_eq!([two.jolt[2], two.jolt[3]], [0., 1.]);
        // the first heals with the last, not on its own
        let late = Duration::from_millis(100 + life + 100);
        assert!(glitch_at("A", late).is_some_and(|f| f.spots[0][2] > 0.));
        let end = Duration::from_millis(500 + life + 10);
        assert!(!glitching("A", end));
        assert_eq!(glitch_at("A", end), None);
    }

    #[test]
    fn a_light_hit_only_cracks_and_jolts_less() {
        let up = [0., 1.];
        glitch("L", [100., 0.], 0.1, up, Duration::ZERO, Duration::ZERO);
        assert_eq!(
            glitch_at("L", Duration::from_millis(100)),
            None,
            "too light to crack"
        );
        glitch("L", [100., 0.], 0.3, up, Duration::ZERO, Duration::ZERO);
        let f = glitch_at("L", Duration::from_millis(100)).unwrap();
        assert_eq!(f.power[0], 0.3);
        assert_eq!(f.grow, 1., "no blot to grow");
        assert_eq!(f.snap, None);
        assert!(f.jolt[1] < feel().shake * 0.6);
    }

    #[test]
    fn past_the_spots_a_hard_hit_swells_the_nearest() {
        let up = [0., 1.];
        for i in 0..SPOTS {
            glitch(
                "B",
                [i as f64 * 300., 0.],
                1.,
                up,
                Duration::ZERO,
                Duration::ZERO,
            );
        }
        glitch(
            "B",
            [905., 0.],
            0.3,
            up,
            Duration::ZERO,
            Duration::from_millis(50),
        );
        glitch(
            "B",
            [905., 0.],
            1.,
            up,
            Duration::ZERO,
            Duration::from_millis(100),
        );
        let f = glitch_at("B", Duration::from_millis(1000)).unwrap();
        assert_eq!(f.spots.iter().filter(|s| s[2] >= 0.).count(), SPOTS);
        assert_eq!(f.swells[3], 1., "{:?}", f.swells);
        assert_eq!(
            f.swells.iter().sum::<f32>(),
            1.,
            "a light one past them is let go"
        );
        let life = feel().life_ms as u64;
        assert!(glitch_at("B", Duration::from_millis(life + 50)).is_some());
    }

    #[test]
    fn calm_breaks_nothing() {
        set_strength(0);
        glitch("C", [0., 0.], 1., [0., 1.], Duration::ZERO, Duration::ZERO);
        assert_eq!(glitch_at("C", Duration::from_millis(100)), None);
        assert!(fly(false, true).trail().is_empty());
        set_strength(1);
    }
}
