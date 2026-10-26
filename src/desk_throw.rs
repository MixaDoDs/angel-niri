//! angel-niri: windows thrown between desks as stacking (the shell's Desktop) starts and ends.
//! `niri msg action set-desk-throw [--on false]`; off until a shell turns it on.
//! - Starting, each window gathered from desks 2…N onto desk 1 flies up from below its output,
//!   one after another, the whole show within a second. From the next desk it lands with a knock
//!   (a shake); from desks further away it slams into the top of the output, spinning a little,
//!   straightens and drops into its place.
//! - Ending, the gathered windows still on desk 1 drop off the bottom one after another before
//!   they go back to their desks; the far ones punch through it.
//! Where a far one hits, the screen breaks like a cyberpunk implant glitching: the picture itself
//! tears into shifted bands, splits its colours, goes to pixels and flickering broken blocks, from
//! the impact out, holds a while, then pulls back into it the way it came (`glitch_at`,
//! shaders/glitch.frag over the finished frame — in screen casts too). The impacts also go to the
//! shell (`Event::DesksThrown`).
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

/// A broken screen: on `output` around `pos` (logical, the edge hit), from `start` (the clock's).
#[derive(Debug, Clone)]
struct Glitch {
    output: String,
    pos: [f64; 2],
    start: Duration,
}

/// Spreading out of the impact and pulling back into it, the same way; holding in between.
const SPREAD_MS: f64 = 650.;
const HOLD_MS: f64 = 3400.;
/// A new picture of the glitch this often: it stutters, it doesn't flow.
const STEP_MS: f64 = 70.;

/// One frame of a broken screen: where (logical, on its output), how far it reaches (a share of
/// the output's width), how hard, and which stutter step it is on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlitchFrame {
    pub pos: [f64; 2],
    pub reach: f64,
    pub power: f64,
    pub step: f64,
}

/// The screen breaks on `output` at `pos` `delay` after `now`. One already broken there goes on
/// from the new spot, at full size.
pub fn glitch(output: &str, pos: [f64; 2], now: Duration, delay: Duration) {
    GLITCHES.with_borrow_mut(|all| {
        let start = now + delay;
        if let Some(g) = all.iter_mut().find(|g| g.output == output) {
            let t = start.saturating_sub(g.start).as_secs_f64() * 1000.;
            if t < SPREAD_MS + HOLD_MS + SPREAD_MS {
                g.pos = pos;
                g.start = start.saturating_sub(Duration::from_secs_f64(SPREAD_MS.min(t) / 1000.));
                return;
            }
        }
        all.retain(|g| g.output != output);
        all.push(Glitch {
            output: output.to_owned(),
            pos,
            start,
        });
    });
}

/// Whether `output` has a broken screen coming or showing: it keeps redrawing.
pub fn glitching(output: &str, now: Duration) -> bool {
    GLITCHES.with_borrow_mut(|all| {
        let end = Duration::from_secs_f64((SPREAD_MS * 2. + HOLD_MS) / 1000.);
        all.retain(|g| now < g.start + end);
        all.iter().any(|g| g.output == output)
    })
}

/// The broken screen to draw on `output` now, if any.
pub fn glitch_at(output: &str, now: Duration) -> Option<GlitchFrame> {
    GLITCHES.with_borrow(|all| {
        let g = all.iter().find(|g| g.output == output)?;
        if now < g.start {
            return None;
        }
        let t = (now - g.start).as_secs_f64() * 1000.;
        let step = (t / STEP_MS).floor();
        let rnd = hash(step);
        // out of the impact, holding (stuttering, a tear now and then, almost healing for a
        // moment), back into it
        let (grow, power) = if t < SPREAD_MS {
            (t / SPREAD_MS, 1.)
        } else if t < SPREAD_MS + HOLD_MS {
            let flicker = if rnd < 0.08 {
                0.25
            } else if rnd > 0.93 {
                1.35
            } else {
                0.85 + 0.15 * hash(step + 0.5)
            };
            (1., flicker)
        } else if t < SPREAD_MS * 2. + HOLD_MS {
            (1. - (t - SPREAD_MS - HOLD_MS) / SPREAD_MS, 1.)
        } else {
            return None;
        };
        // in steps too: the edge jumps outward block by block
        let grow = (grow * 8.).ceil() / 8.;
        let reach = 0.42 * (1. - (1. - grow).powi(2)) * power.min(1.2);
        (reach > 0.).then_some(GlitchFrame {
            pos: g.pos,
            reach,
            power: power.min(1.),
            step,
        })
    })
}

fn hash(x: f64) -> f64 {
    ((x * 12.9898).sin() * 43758.5453).rem_euclid(1.)
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
        for hit in throws.iter().filter(|hit| hit.far) {
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

/// The whole show, every window on an output, ms.
const SHOW_MS: u64 = 1000;
const FAR_MS: u64 = 560;
const NEAR_MS: u64 = 420;
/// One window after another, at most this far apart.
const STEP_MAX_MS: u64 = 110;
/// The turn at its widest, radians (about 7°).
const SPIN: f64 = 0.12;
/// Into the top edge, the part of the flight going up.
const UP: f64 = 0.5;
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
    /// Down until its bottom edge meets the output's (leaving).
    edge: f64,
    spin: f64,
}

impl Fly {
    /// A flight starting `delay` from now; `y`, `h`: the window's top and height in its output,
    /// `view_h`: the output's height.
    pub fn new(
        clock: Clock,
        delay: Duration,
        leaving: bool,
        far: bool,
        y: f64,
        h: f64,
        view_h: f64,
        spin_right: bool,
    ) -> Self {
        let ms = if far { FAR_MS } else { NEAR_MS };
        let anim = Animation::ease(clock, 0., 1., 0., ms, Curve::Linear).delayed(delay);
        Self {
            anim,
            leaving,
            far,
            below: view_h - y + MARGIN,
            top: -y,
            edge: view_h - (y + h),
            spin: if spin_right { SPIN } else { -SPIN },
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
            return self.spin * t;
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
            // a hop up, then down past the bottom, faster and faster
            let hop = if t < 0.35 {
                HOP * (t / 0.35 * std::f64::consts::PI).sin()
            } else {
                0.
            };
            return (0., self.below * t.powi(3) - hop);
        }
        if !self.far {
            // up into place, a little past it and back, then a knock
            let y = self.below * (1. - ease_out_back(t));
            return (shake(t, 0.7, 5.), y);
        }
        if t < UP {
            let s = t / UP;
            // fast, slowing a little near the top: it hits with speed
            let s = s * (1.6 - 0.6 * s);
            (0., self.below + (self.top - self.below) * s)
        } else {
            let s = (t - UP) / (1. - UP);
            (shake(t, UP, 8.), self.top * (1. - ease_out_bounce(s)))
        }
    }

    /// When the impact comes, from now: arriving far, it hits the top; leaving far, its bottom
    /// edge reaches the output's. Near ones have none.
    pub fn hit_in(&self, now: Duration) -> Option<Duration> {
        if !self.far {
            return None;
        }
        let frac = if self.leaving {
            // the first t where its bottom passes the output's (the drop is monotonic there)
            let mut t = 0.35;
            while t < 1. && self.at(t).1 < self.edge {
                t += 0.005;
            }
            t
        } else {
            UP
        };
        let start = self.anim.start_time();
        Some((start + self.anim.duration().mul_f64(frac)).saturating_sub(now))
    }
}

/// For n windows on one output, the delay of the i-th and how long the longest flight lasts.
pub fn step(n: usize, far: bool) -> Duration {
    let ms = if far { FAR_MS } else { NEAR_MS };
    if n <= 1 {
        return Duration::ZERO;
    }
    let room = SHOW_MS.saturating_sub(ms) / (n as u64 - 1);
    Duration::from_millis(room.clamp(30, STEP_MAX_MS))
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

    #[test]
    fn flights_start_below_and_end_in_place() {
        let fly = Fly::new(
            Clock::with_time(Duration::ZERO),
            Duration::ZERO,
            false,
            true,
            200.,
            300.,
            1000.,
            true,
        );
        assert_eq!(fly.at(0.).1, 816.);
        assert_eq!(fly.at(UP).1, -200.);
        assert_eq!(fly.at(1.), (0., 0.));

        let near = Fly { far: false, ..fly };
        assert_eq!(near.at(1.), (0., 0.));
        assert!(near.at(0.).1 > 800.);
    }

    #[test]
    fn leaving_ends_below() {
        let fly = Fly::new(
            Clock::with_time(Duration::ZERO),
            Duration::ZERO,
            true,
            true,
            200.,
            300.,
            1000.,
            false,
        );
        assert_eq!(fly.at(0.).1, 0.);
        assert_eq!(fly.at(1.).1, 816.);
        assert!(fly.hit_in(Duration::ZERO).is_some());
    }

    #[test]
    fn the_glitch_spreads_holds_and_pulls_back_the_same_way() {
        let at = |ms: u64| glitch_at("A", Duration::from_millis(ms)).map(|g| g.reach);
        glitch("A", [100., 0.], Duration::ZERO, Duration::from_millis(100));
        assert_eq!(at(50), None, "before the hit");
        let spread = SPREAD_MS as u64;
        let back = 100 + spread + HOLD_MS as u64;
        // the way out and the way back are mirror images
        for ms in [100, 300, 500] {
            assert_eq!(at(100 + ms - 100), at(back + spread - (ms - 100)));
        }
        assert!(at(100 + spread + 10).is_some_and(|r| r > 0.2));
        assert_eq!(at(back + spread + 10), None, "still broken");
        assert!(glitching("A", Duration::from_millis(back)));
        assert!(!glitching("A", Duration::from_millis(back + spread + 10)));
    }

    #[test]
    fn the_show_fits_in_a_second() {
        let n = 8;
        let last = step(n, true) * (n as u32 - 1) + Duration::from_millis(FAR_MS);
        assert!(last <= Duration::from_millis(SHOW_MS));
    }
}
