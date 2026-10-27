//! angel-niri: windows thrown between desks as stacking (the shell's Desktop) starts and ends.
//! `niri msg action set-desk-throw [--on false] [--strength 0|1|2]`; off until a shell turns it
//! on. The strength (calm, usual, wild) sets how far apart they come, their trails, the hit's
//! freeze and shake, and how big and long the break is (`Feel`); calm breaks nothing.
//! - Starting, the windows gathered from desks 2…N onto desk 1 fly up from below its output, each
//!   on a path of its own: its app sets the side, the bow and the turn it likes, every throw
//!   shakes them up a little (`Path`). From the next desk they come quickly and land with a knock
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

/// The turn at its widest, radians (about 7°), before a path's own.
const SPIN: f64 = 0.12;
/// Into the top edge, the part of the flight going up.
const UP: f64 = 0.5;
/// Leaving, the hop up before the drop, logical px.
const HOP: f64 = 24.;
/// Below the output, how far past its edge a window starts or ends.
const MARGIN: f64 = 16.;
/// The afterimages lag this far behind each other, ms of the flight.
const GHOST_MS: f64 = 24.;
const GHOST_ALPHA: [f32; 3] = [0.5, 0.3, 0.16];

/// A window's own way of flying: its app's liking (the side, the bow, the turn, the pace) shaken
/// up a little by every throw, so no two fly alike.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Path {
    /// Sideways, where it comes from (or goes to), logical px: - left, + right.
    pub side: f64,
    /// How far its way bows sideways in between, logical px, either way.
    pub bow: f64,
    /// Its turn at the widest, radians, signed.
    pub spin: f64,
    /// Its flight takes this times as long.
    pub pace: f64,
    /// At the hit, the part of its sideways start still left.
    pub at_hit: f64,
}

impl Path {
    /// The way the `order`th window of a throw flies, of app `app`, in the throw `nonce`.
    pub fn new(app: &str, order: usize, nonce: u64) -> Self {
        let app = app.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        });
        let liked = |k: u64| unit(app ^ k.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let now = |k: u64| {
            unit(
                nonce
                    ^ (order as u64 + 1).wrapping_mul(0xbf58_476d_1ce4_e5b9)
                    ^ k.wrapping_mul(0x94d0_49bb_1331_11eb),
            )
        };
        // left and right in turn, so a crowd spreads out; the app picks which first
        let left = (order % 2 == 0) != (liked(1) < 0.5);
        let reach = 130. + 150. * liked(2) + 110. * now(3);
        // it mostly turns the way it travels, now and then the other way
        let turn = SPIN * (0.55 + 0.7 * liked(6) + 0.35 * now(7));
        let with = now(8) >= 0.22;
        Self {
            side: if left { -reach } else { reach },
            bow: (liked(4) * 2. - 1.) * 70. + (now(5) * 2. - 1.) * 60.,
            spin: if left == with { turn } else { -turn },
            pace: 0.86 + 0.14 * liked(9) + 0.16 * now(10),
            at_hit: 0.25 + 0.3 * now(11),
        }
    }
}

/// 0…1 from `x`, well mixed (splitmix64).
fn unit(x: u64) -> f64 {
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
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
    /// Down to below the output.
    below: f64,
    /// Up to the output's top edge (far ones arriving).
    top: f64,
    path: Path,
    ghosts: usize,
}

impl Fly {
    /// A flight starting `delay` from now; `y`: the window's top in its output, `view_h`: the
    /// output's height; `path`: its own way.
    pub fn new(
        clock: Clock,
        delay: Duration,
        leaving: bool,
        far: bool,
        y: f64,
        view_h: f64,
        path: Path,
    ) -> Self {
        let feel = feel();
        let base = if far && !leaving {
            feel.far_ms
        } else {
            feel.near_ms
        };
        let ms = (base as f64 * path.pace).round();
        let hold = if far && !leaving {
            feel.hit_stop_ms as f64
        } else {
            0.
        };
        let anim =
            Animation::ease(clock, 0., 1., 0., (ms + hold) as u64, Curve::Linear).delayed(delay);
        Self {
            anim,
            ms,
            hold,
            leaving,
            far,
            below: view_h - y + MARGIN,
            top: -y,
            path,
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
        let spin = self.path.spin;
        if t <= 0. || t >= 1. {
            return 0.;
        }
        if self.leaving {
            return -spin * t;
        }
        if !self.far {
            return spin * 0.6 * (1. - t) * (1. - t);
        }
        if t < UP {
            spin * (t / UP * std::f64::consts::FRAC_PI_2).sin()
        } else {
            // straightens at the hit, swinging back once
            let s = (t - UP) / (1. - UP);
            spin * (1. - s).powi(3) * (s * std::f64::consts::TAU).cos()
        }
    }

    fn at(&self, t: f64) -> (f64, f64) {
        let Path {
            side, bow, at_hit, ..
        } = self.path;
        let arc = |s: f64| bow * (s.clamp(0., 1.) * std::f64::consts::PI).sin();
        if t >= 1. && !self.leaving {
            return (0., 0.);
        }
        if self.leaving {
            // a hop up, then down past the bottom and off to its side, faster and faster
            let hop = if t < 0.35 {
                HOP * (t / 0.35 * std::f64::consts::PI).sin()
            } else {
                0.
            };
            return (
                side * 0.5 * t * t + arc(t) * 0.5,
                self.below * t.powi(3) - hop,
            );
        }
        if !self.far {
            // up into place from its side, a little past it and back, then a knock
            let e = ease_out_back(t);
            return (
                side * (1. - e) + arc(e) * 0.6 + shake(t, 0.7, 5.),
                self.below * (1. - e),
            );
        }
        if t < UP {
            let s = t / UP;
            // fast, slowing a little near the top: it hits with speed
            let s = s * (1.6 - 0.6 * s);
            (
                side * (1. - (1. - at_hit) * s) + arc(s),
                self.below + (self.top - self.below) * s,
            )
        } else {
            let s = (t - UP) / (1. - UP);
            let e = ease_out_bounce(s);
            (
                side * at_hit * (1. - e.min(1.)) + shake(t, UP, 8.),
                self.top * (1. - e),
            )
        }
    }

    /// When it makes its sound, from now, and how far sideways from its place: a far one
    /// arriving hits the top, a near one knocks into place, a leaving one starts to drop.
    pub fn hit_in(&self, now: Duration) -> (Duration, f64) {
        let t = if self.leaving {
            0.3
        } else if self.far {
            UP
        } else {
            0.7
        };
        let start = self.anim.start_time();
        let at = (start + Duration::from_secs_f64(self.ms * t / 1000.)).saturating_sub(now);
        (at, self.at(t).0)
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

    const PATH: Path = Path {
        side: -150.,
        bow: 0.,
        spin: SPIN,
        pace: 1.,
        at_hit: 0.4,
    };

    fn fly(leaving: bool, far: bool) -> Fly {
        Fly::new(
            Clock::with_time(Duration::ZERO),
            Duration::ZERO,
            leaving,
            far,
            200.,
            1000.,
            PATH,
        )
    }

    #[test]
    fn flights_start_below_from_their_side_and_end_in_place() {
        let f = fly(false, true);
        assert_eq!(f.at(0.), (-150., 816.));
        assert_eq!(f.at(UP).1, -200.);
        assert_eq!(f.at(1.), (0., 0.));
        let (_, x) = f.hit_in(Duration::ZERO);
        assert_eq!(x, -60.);

        let near = fly(false, false);
        assert_eq!(near.at(1.), (0., 0.));
        assert!(near.at(0.).1 > 800.);
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
        assert_eq!(f.at(1.).1, 816.);
    }

    #[test]
    fn paths_differ_by_app_and_throw_but_keep_to_their_app() {
        let a = Path::new("kitty", 0, 7);
        assert_eq!(a, Path::new("kitty", 0, 7));
        assert_ne!(a, Path::new("kitty", 0, 8));
        assert_ne!(a, Path::new("helium", 0, 7));
        // a crowd spreads to both sides
        let sides: Vec<_> = (0..6).map(|i| Path::new("kitty", i, 7).side < 0.).collect();
        assert!(sides.contains(&true) && sides.contains(&false));
        for i in 0..40 {
            let p = Path::new("x", i, i as u64 * 31);
            assert!((130.0..=390.).contains(&p.side.abs()), "{p:?}");
            assert!((0.86..=1.16).contains(&p.pace), "{p:?}");
        }
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
