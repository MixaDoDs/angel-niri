//! angel-niri: throwing floating windows. `niri msg action set-window-throw [--strength S]
//! [--bounce B] [--squash Q] [--wobble W]` (each 0 = off):
//! - a floating window let go of mid-drag keeps flying with the pointer's speed × S, slowing
//!   down (the stronger S, the longer it slides), and stays inside its output's working area;
//! - hitting an edge, it bounces back with B of its speed (0: stops there);
//! - and squashes against it for a moment, by Q (in whole pixels of the pixel corners);
//! - while dragged, a window W > 0 lags behind the pointer on a spring and sways when it stops,
//!   stretching toward it like jelly.
//!
//! Only floating windows fly; tiled ones move as always. A shell sets it; nothing goes to niri's
//! config.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use smithay::utils::{Logical, Point, Rectangle, Size};

static STRENGTH: AtomicU32 = AtomicU32::new(0);
static BOUNCE: AtomicU32 = AtomicU32::new(0);
static SQUASH: AtomicU32 = AtomicU32::new(0);
static WOBBLE: AtomicU32 = AtomicU32::new(0);

fn load(v: &AtomicU32) -> f64 {
    f64::from(f32::from_bits(v.load(Ordering::Relaxed)))
}

fn store(v: &AtomicU32, x: f64, max: f64) {
    v.store((x.clamp(0., max) as f32).to_bits(), Ordering::Relaxed);
}

pub fn strength() -> f64 {
    load(&STRENGTH)
}

pub fn bounce() -> f64 {
    load(&BOUNCE)
}

pub fn squash() -> f64 {
    load(&SQUASH)
}

pub fn wobble() -> f64 {
    load(&WOBBLE)
}

pub fn set(strength: f64, bounce: f64, squash: f64, wobble: f64) {
    store(&STRENGTH, strength, 3.);
    store(&BOUNCE, bounce, 0.95);
    store(&SQUASH, squash, 2.);
    store(&WOBBLE, wobble, 2.);
}

/// Slower than this (logical px/s) a let-go window just stays.
const MIN_THROW_SPEED: f64 = 300.;
/// A flying window slower than this stops.
const STOP_SPEED: f64 = 20.;
/// Pointer speed older than this at the release means the pointer had stopped.
const STALE: Duration = Duration::from_millis(60);

/// The pointer's speed during a drag, smoothed over its last few tens of ms.
#[derive(Debug, Clone, Copy, Default)]
pub struct Velocity {
    vel: Point<f64, Logical>,
    last: Option<(Duration, Point<f64, Logical>)>,
}

impl Velocity {
    pub fn add(&mut self, now: Duration, pos: Point<f64, Logical>) {
        if let Some((t, p)) = self.last {
            let dt = now.saturating_sub(t).as_secs_f64();
            if dt <= 0. {
                // Several events in one frame: keep the first time, the latest position.
                return;
            }
            if dt > STALE.as_secs_f64() {
                self.vel = Point::default();
            } else {
                let a = 1. - (-dt / 0.03).exp();
                let v = (pos - p).downscale(dt);
                self.vel = self.vel + (v - self.vel).upscale(a);
            }
        }
        self.last = Some((now, pos));
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The throw's starting speed at `now`, if the window should fly at all.
    pub fn throw(&self, now: Duration) -> Option<Point<f64, Logical>> {
        let s = strength();
        let (t, _) = self.last?;
        if s <= 0. || now.saturating_sub(t) > STALE {
            return None;
        }
        let v = self.vel.upscale(s);
        (len(v) >= MIN_THROW_SPEED).then_some(v)
    }
}

fn len(p: Point<f64, Logical>) -> f64 {
    p.x.hypot(p.y)
}

/// A floating window in flight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Throw {
    vel: Point<f64, Logical>,
    last: Duration,
}

/// An edge a flying window hit: which axis, the side (-1 left/top, 1 right/bottom), how fast.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub horizontal: bool,
    pub side: f64,
    pub speed: f64,
}

impl Throw {
    pub fn new(vel: Point<f64, Logical>, now: Duration) -> Self {
        Self { vel, last: now }
    }

    /// Moves `pos` (a `size` window's top left) on to `now` inside `area`; returns the new
    /// position, the edges hit, and whether it still flies.
    pub fn step(
        &mut self,
        now: Duration,
        mut pos: Point<f64, Logical>,
        size: Size<f64, Logical>,
        area: Rectangle<f64, Logical>,
    ) -> (Point<f64, Logical>, Vec<Hit>, bool) {
        let dt = now.saturating_sub(self.last).as_secs_f64().min(0.05);
        self.last = now;

        // The stronger the throw, the less it brakes: 0.3 barely slides, 2 crosses the screen.
        let friction = 5. / strength().max(0.2);
        self.vel = self.vel.upscale((-friction * dt).exp());
        pos += self.vel.upscale(dt);

        let bounce = bounce();
        let mut hits = vec![];
        let mut axis = |p: &mut f64, v: &mut f64, lo: f64, hi: f64, horizontal: bool| {
            if hi < lo {
                // Bigger than the area: nothing to bounce off.
                *v = 0.;
                return;
            }
            let side = if *p < lo {
                *p = lo;
                -1.
            } else if *p > hi {
                *p = hi;
                1.
            } else {
                return;
            };
            if *v * side > 0. {
                hits.push(Hit {
                    horizontal,
                    side,
                    speed: v.abs(),
                });
                *v = -*v * bounce;
            }
        };
        let max = area.loc + Point::from((area.size.w - size.w, area.size.h - size.h));
        axis(&mut pos.x, &mut self.vel.x, area.loc.x, max.x, true);
        axis(&mut pos.y, &mut self.vel.y, area.loc.y, max.y, false);

        (pos, hits, len(self.vel) >= STOP_SPEED)
    }
}

/// How far a dragged window may lag behind the pointer, logical px at wobble 1.
const MAX_LAG: f64 = 48.;

/// A dragged window lagging behind the pointer on an underdamped spring.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Wobble {
    pub offset: Point<f64, Logical>,
    vel: Point<f64, Logical>,
    last: Option<Duration>,
}

impl Wobble {
    /// The pointer moved by `delta`: the window stays behind it.
    pub fn pull(&mut self, delta: Point<f64, Logical>) {
        let max = MAX_LAG * wobble();
        let o = self.offset - delta;
        let l = len(o);
        self.offset = if l > max && l > 0. {
            o.upscale(max / l)
        } else {
            o
        };
    }

    /// Runs the spring on to `now`; false once it has settled.
    pub fn step(&mut self, now: Duration) -> bool {
        let dt = self
            .last
            .map_or(0., |t| now.saturating_sub(t).as_secs_f64().min(0.05));
        self.last = Some(now);

        // Stiffness and damping per second; ~2 Hz sway that dies out in about a second.
        const K: f64 = 170.;
        const C: f64 = 9.;
        let steps = (dt / 0.004).ceil().max(1.) as usize;
        let h = dt / steps as f64;
        for _ in 0..steps {
            let acc = self.offset.upscale(-K) - self.vel.upscale(C);
            self.vel += acc.upscale(h);
            self.offset += self.vel.upscale(h);
        }

        len(self.offset) >= 0.3 || len(self.vel) >= 5.
    }

    /// The stretch toward the pointer, (x, y) scale around the grab point.
    pub fn stretch(&self, size: Size<f64, Logical>) -> (f64, f64) {
        let w = wobble();
        let sx = (self.offset.x.abs() / size.w.max(1.)).min(0.15) * w;
        let sy = (self.offset.y.abs() / size.h.max(1.)).min(0.15) * w;
        (1. + sx - sy * 0.5, 1. + sy - sx * 0.5)
    }
}

/// The squash of a window against an edge, `v` going 1 → 0 with an overshoot.
pub fn squash_scale(hit: &Hit, v: f64) -> (f64, f64) {
    let amount = squash() * (hit.speed / 3000.).clamp(0.05, 0.2);
    let along = 1. - amount * v;
    let across = 1. + amount * v * 0.5;
    if hit.horizontal {
        (along, across)
    } else {
        (across, along)
    }
}

/// `scale` of a `size` side rounded so the side changes by whole pixels of the pixel corners
/// (1 logical px without them): the window steps rather than smears.
pub fn quantize(scale: f64, size: f64) -> f64 {
    let px = f64::from(crate::pixel_corners::pixel()).max(1.);
    if size <= 0. {
        return 1.;
    }
    1. + ((scale - 1.) * size / px).round() * px / size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throw_bounces_and_stops() {
        set(1., 0.5, 1., 1.);
        let area = Rectangle::new(Point::from((0., 0.)), Size::from((1000., 800.)));
        let size = Size::from((200., 100.));
        let mut throw = Throw::new(Point::from((4000., 0.)), Duration::ZERO);
        let mut pos = Point::from((700., 100.));
        let mut hits = 0;
        let mut t = Duration::ZERO;
        loop {
            t += Duration::from_millis(16);
            let (p, h, flying) = throw.step(t, pos, size, area);
            pos = p;
            hits += h.len();
            assert!(pos.x >= 0. && pos.x <= 800.);
            if !flying {
                break;
            }
            assert!(t < Duration::from_secs(10));
        }
        assert_eq!(hits, 1);
        assert!(pos.x < 800.);
    }

    #[test]
    fn no_bounce_stops_at_edge() {
        set(1., 0., 1., 1.);
        let area = Rectangle::new(Point::from((0., 0.)), Size::from((1000., 800.)));
        let mut throw = Throw::new(Point::from((-5000., 0.)), Duration::ZERO);
        let (pos, hits, flying) = throw.step(
            Duration::from_millis(50),
            Point::from((10., 10.)),
            Size::from((100., 100.)),
            area,
        );
        assert_eq!(pos.x, 0.);
        assert_eq!(hits.len(), 1);
        assert!(!flying);
    }

    #[test]
    fn wobble_lags_then_settles() {
        set(1., 0.5, 1., 1.);
        let mut w = Wobble::default();
        w.step(Duration::ZERO);
        w.pull(Point::from((200., 0.)));
        assert_eq!(w.offset.x, -MAX_LAG);
        let mut t = Duration::ZERO;
        let mut overshot = false;
        while w.step(t) {
            overshot |= w.offset.x > 1.;
            t += Duration::from_millis(16);
            assert!(t < Duration::from_secs(5));
        }
        assert!(overshot);
    }

    #[test]
    fn quantize_steps() {
        crate::pixel_corners::set(4., 2);
        let s = quantize(0.97, 200.);
        assert_eq!(((1. - s) * 200.).round() % 4., 0.);
        crate::pixel_corners::set(0., 2);
    }
}
