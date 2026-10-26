//! angel-niri: windows thrown between desks as stacking (the shell's Desktop) starts and ends.
//! `niri msg action set-desk-throw [--on false]`; off until a shell turns it on.
//! - Starting, the windows gathered from desks 2…N onto desk 1 fly up from below its output, each
//!   from its own side. From the next desk they come quickly and land with a knock (a shake); from
//!   desks further away they come one by one, well apart, slam into the top of the output turning
//!   a little, straighten and drop into their places.
//! - Ending, the gathered windows still on desk 1 drop off the bottom quickly before they go back
//!   to their desks. No damage then.
//! Where a far one hits, the monitor breaks — the picture itself, in screen casts too
//! (shaders/glitch.frag over the finished frame): a jolt tearing it sideways, then a black blot of
//! leaked liquid crystal with dark veins, long lines across the whole screen, dead pixels of the
//! monitor's own size flickering; it calms down and heals, the blot pulling back into the impact.
//! Every hit adds its own (`glitch`, up to `SPOTS` at once). The impacts also go to the shell
//! (`Event::DesksThrown`).
//! In flight a window is drawn turned in whole pixels of the pixel corners (shaders/throw.frag).

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
    static GLITCHES: RefCell<Vec<Glitch>> = const { RefCell::new(Vec::new()) };
}

/// A hit's damage: on `output` at `pos` (logical, the edge hit), from `start` (the clock's).
#[derive(Debug, Clone)]
struct Glitch {
    output: String,
    pos: [f64; 2],
    start: Duration,
    seed: f64,
}

/// How long the damage lasts after the last hit, healing at the end (all hits together).
const LIFE_MS: f64 = 6500.;
/// At most this many hits' damage on one output at once (shaders/glitch.frag `g_h0…`).
pub const SPOTS: usize = 6;
/// The damage changes this often: it stutters, it doesn't flow.
const STEP_MS: f64 = 70.;

/// One frame of a broken screen: each hit's damage as (x, y logical, its age in s or -1 for none,
/// its seed), and the stutter step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlitchFrame {
    pub spots: [[f32; 4]; SPOTS],
    pub step: f32,
    /// How much bigger every blot is for the hits so far (1 with one).
    pub grow: f32,
    /// Until all of it has healed, s: the hits heal together, after the last one.
    pub left: f32,
}

/// Each hit after the first makes every blot this much bigger, growing over `GROW_MS`.
const GROW_PER_HIT: f64 = 0.45;
const GROW_MS: f64 = 500.;
/// Past this many hits after the first it grows no more: the screen stays a screen.
const GROW_HITS: f64 = 4.;

/// A hit on `output` at `pos`, `delay` after `now`: it adds its damage to whatever is there and
/// makes it all bigger.
pub fn glitch(output: &str, pos: [f64; 2], now: Duration, delay: Duration) {
    GLITCHES.with_borrow_mut(|all| {
        let start = now + delay;
        let seed = (start.as_millis() % 9973) as f64 * 0.618;
        all.push(Glitch {
            output: output.to_owned(),
            pos,
            start,
            seed,
        });
        // the oldest go first when too many
        let mine = all.iter().filter(|g| g.output == output).count();
        if mine > SPOTS {
            let first = all.iter().position(|g| g.output == output).unwrap();
            all.remove(first);
        }
    });
}

/// When the damage on `output` has healed: `LIFE_MS` after its last hit.
fn healed_at(all: &[Glitch], output: &str) -> Option<Duration> {
    let last = (all.iter().filter(|g| g.output == output))
        .map(|g| g.start)
        .max()?;
    Some(last + Duration::from_secs_f64(LIFE_MS / 1000.))
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
        let mut spots = [[0., 0., -1., 0.]; SPOTS];
        let mut hits = 0.;
        let mine = all.iter().filter(|g| g.output == output && g.start <= now);
        for (spot, g) in spots.iter_mut().zip(mine) {
            let age = (now - g.start).as_secs_f64();
            *spot = [g.pos[0] as f32, g.pos[1] as f32, age as f32, g.seed as f32];
            hits += (age * 1000. / GROW_MS).min(1.);
        }
        if hits <= 0. {
            return None;
        }
        let step = (now.as_secs_f64() * 1000. / STEP_MS).floor() as f32;
        Some(GlitchFrame {
            spots,
            step,
            grow: (1. + GROW_PER_HIT * (hits - 1.).clamp(0., GROW_HITS)) as f32,
            left: (end - now).as_secs_f32(),
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

const FAR_MS: u64 = 560;
const NEAR_MS: u64 = 420;
/// Near ones (and all leaving) one after another this far apart; the far ones much further, so
/// each hit shows on its own and adds to the damage.
const NEAR_GAP_MS: u64 = 70;
const FAR_GAP_MS: u64 = 380;
/// The turn at its widest, radians (about 7°).
const SPIN: f64 = 0.12;
/// Into the top edge, the part of the flight going up.
const UP: f64 = 0.5;
/// At the hit, the part of its sideways start still left.
const AT_HIT: f64 = 0.4;
/// Leaving, the hop up before the drop, logical px.
const HOP: f64 = 24.;
/// Below the output, how far past its edge a window starts or ends.
const MARGIN: f64 = 16.;

/// One window's flight; offsets are from where it rests.
#[derive(Debug)]
pub struct Fly {
    anim: Animation,
    pub leaving: bool,
    pub far: bool,
    /// Down to below the output.
    below: f64,
    /// Up to the output's top edge (far ones arriving).
    top: f64,
    /// Sideways, where it comes from (or goes to).
    side: f64,
    spin: f64,
}

impl Fly {
    /// A flight starting `delay` from now; `y`, `h`: the window's top and height in its output,
    /// `view_h`: the output's height; `side`: from how far left (-) or right (+) it comes.
    pub fn new(
        clock: Clock,
        delay: Duration,
        leaving: bool,
        far: bool,
        y: f64,
        view_h: f64,
        side: f64,
    ) -> Self {
        let ms = if far && !leaving { FAR_MS } else { NEAR_MS };
        let anim = Animation::ease(clock, 0., 1., 0., ms, Curve::Linear).delayed(delay);
        Self {
            anim,
            leaving,
            far,
            below: view_h - y + MARGIN,
            top: -y,
            side,
            // it turns the way it travels
            spin: if side < 0. { SPIN } else { -SPIN },
        }
    }

    pub fn is_done(&self) -> bool {
        self.anim.is_done()
    }

    fn t(&self) -> f64 {
        self.anim.value().clamp(0., 1.)
    }

    /// Where it is drawn, from its place.
    pub fn offset(&self) -> Point<f64, Logical> {
        let (x, y) = self.at(self.t());
        Point::from((x, y))
    }

    /// The turn it is drawn with, radians.
    pub fn angle(&self) -> f64 {
        let t = self.t();
        if t <= 0. || t >= 1. {
            return 0.;
        }
        if self.leaving {
            return -self.spin * t;
        }
        if !self.far {
            return self.spin * 0.6 * (1. - t) * (1. - t);
        }
        if t < UP {
            self.spin * (t / UP * std::f64::consts::FRAC_PI_2).sin()
        } else {
            // straightens at the hit, swinging back once
            let s = (t - UP) / (1. - UP);
            self.spin * (1. - s).powi(3) * (s * std::f64::consts::TAU).cos()
        }
    }

    fn at(&self, t: f64) -> (f64, f64) {
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
            return (self.side * 0.5 * t * t, self.below * t.powi(3) - hop);
        }
        if !self.far {
            // up into place from its side, a little past it and back, then a knock
            let e = ease_out_back(t);
            return (
                self.side * (1. - e) + shake(t, 0.7, 5.),
                self.below * (1. - e),
            );
        }
        if t < UP {
            let s = t / UP;
            // fast, slowing a little near the top: it hits with speed
            let s = s * (1.6 - 0.6 * s);
            (
                self.side * (1. - (1. - AT_HIT) * s),
                self.below + (self.top - self.below) * s,
            )
        } else {
            let s = (t - UP) / (1. - UP);
            let e = ease_out_bounce(s);
            (
                self.side * AT_HIT * (1. - e.min(1.)) + shake(t, UP, 8.),
                self.top * (1. - e),
            )
        }
    }

    /// When a far one arriving hits the top, from now, and how far sideways from its place.
    pub fn hit_in(&self, now: Duration) -> Option<(Duration, f64)> {
        if !self.far || self.leaving {
            return None;
        }
        let start = self.anim.start_time();
        let at = (start + self.anim.duration().mul_f64(UP)).saturating_sub(now);
        Some((at, self.side * AT_HIT))
    }
}

/// When each of a row of windows (`far` flags, the near ones first) sets off, from now.
pub fn delays(far: &[bool], leaving: bool) -> Vec<Duration> {
    let mut out = Vec::with_capacity(far.len());
    let mut at = 0;
    let mut last_far = None;
    for (i, far) in far.iter().enumerate() {
        if i > 0 {
            at += if leaving || !far {
                NEAR_GAP_MS
            } else if last_far.is_some() {
                FAR_GAP_MS
            } else {
                // the first far one after the near ones: a breath first
                NEAR_GAP_MS * 2
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
    Duration::from_millis(FAR_MS.max(NEAR_MS))
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

    fn fly(leaving: bool, far: bool) -> Fly {
        Fly::new(
            Clock::with_time(Duration::ZERO),
            Duration::ZERO,
            leaving,
            far,
            200.,
            1000.,
            -150.,
        )
    }

    #[test]
    fn flights_start_below_from_their_side_and_end_in_place() {
        let f = fly(false, true);
        assert_eq!(f.at(0.), (-150., 816.));
        assert_eq!(f.at(UP).1, -200.);
        assert_eq!(f.at(1.), (0., 0.));
        let (_, x) = f.hit_in(Duration::ZERO).unwrap();
        assert_eq!(x, -60.);

        let near = fly(false, false);
        assert_eq!(near.at(1.), (0., 0.));
        assert!(near.at(0.).1 > 800.);
        assert!(near.hit_in(Duration::ZERO).is_none());
    }

    #[test]
    fn leaving_ends_below_without_a_hit() {
        let f = fly(true, true);
        assert_eq!(f.at(0.), (0., 0.));
        assert_eq!(f.at(1.).1, 816.);
        assert!(f.hit_in(Duration::ZERO).is_none());
    }

    #[test]
    fn far_ones_come_well_apart_near_ones_quickly() {
        let d = delays(&[false, false, true, true, true], false);
        assert_eq!(d[1], Duration::from_millis(NEAR_GAP_MS));
        assert!(d[3] - d[2] >= Duration::from_millis(300));
        assert!(d[4] - d[3] >= Duration::from_millis(300));
        let back = delays(&[false, true, true], true);
        assert_eq!(back[2], Duration::from_millis(NEAR_GAP_MS * 2));
    }

    #[test]
    fn every_hit_makes_it_bigger_and_all_heals_together() {
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
        // the first heals with the last, not on its own
        let late = Duration::from_millis(100 + LIFE_MS as u64 + 100);
        assert!(glitch_at("A", late).is_some_and(|f| f.spots[0][2] > 0.));
        let end = Duration::from_millis(500 + LIFE_MS as u64 + 10);
        assert!(!glitching("A", end));
        assert_eq!(glitch_at("A", end), None);
        for i in 0..10 {
            glitch("B", [i as f64, 0.], Duration::ZERO, Duration::ZERO);
        }
        assert_eq!(
            GLITCHES.with_borrow(|all| all.iter().filter(|g| g.output == "B").count()),
            SPOTS
        );
    }
}
