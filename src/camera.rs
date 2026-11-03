//! angel-niri: a camera that follows the pointer. `niri msg action toggle-cursor-camera` zooms
//! the screen in on the pointer and keeps it in view; again — back out. `set-cursor-camera
//! true|false [--zoom 1.8] [--smooth 1] [--blur 1]` sets it up (the shell does, from Settings).
//!
//! As Recordly's cursor-follow camera (github.com/webadderall/Recordly, AGPL — the ideas and
//! numbers, written anew): the view holds still while the pointer stays inside an inner safe zone
//! (a quarter of the view in from each edge); once it leaves, the camera's goal jumps onto the
//! pointer and a spring (stiffness 100, damping 21, mass 1 — barely overdamped) carries the view
//! there; zooming in and out rides the same spring; the view never leaves the screen; fast pans
//! smear along their way and zooming smears outward (shaders/camera.frag).
//!
//! Only the picture moves: the pointer, clicks and windows stay where they are in the layout; the
//! compositor draws the finished frame of the output zoomed (render_helpers/framebuffer_effect)
//! and the pointer over it at its place in the zoomed view, crisp, a little bigger with the zoom.

use std::cell::RefCell;
use std::time::Duration;

use smithay::utils::{Logical, Point, Size};

/// The safe zone: this share of the view in from each edge.
const SAFE: f64 = 0.25;
/// Camera shutter (Recordly: motion blur 0.35 × 4), and the longest smear on screen (logical).
const SHUTTER: f64 = 1.4;
const MAX_SMEAR: f64 = 42.;
/// How much the pointer grows with the zoom: 1 + (zoom − 1) × this.
pub const POINTER_GROW: f64 = 0.6;

#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub allowed: bool,
    pub zoom: f64,
    pub smooth: f64,
    pub blur: f64,
}

#[derive(Clone, Copy, Default)]
struct Spring {
    x: f64,
    v: f64,
}

impl Spring {
    /// a·m = −k(x − target) − c·v, small substeps.
    fn step(&mut self, target: f64, k: f64, c: f64, m: f64, mut dt: f64) {
        while dt > 0. {
            let h = dt.min(0.002);
            let a = (-k * (self.x - target) - c * self.v) / m;
            self.v += a * h;
            self.x += self.v * h;
            dt -= h;
        }
    }

    fn settled(&self, target: f64, eps: f64) -> bool {
        (self.x - target).abs() < eps && self.v.abs() < eps * 20.
    }

    fn snap(&mut self, x: f64) {
        self.x = x;
        self.v = 0.;
    }
}

/// What the camera shows of an output this frame.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub zoom: f64,
    /// the content point at the centre of the screen, output-local logical
    pub focus: Point<f64, Logical>,
    /// half the output, logical
    pub half: (f64, f64),
    /// the smear: along the pan, content logical px (whole length); outward, share per side
    pub smear: (f64, f64),
    pub radial: f64,
}

impl Frame {
    /// Where an output-local point of the content shows on the screen.
    pub fn to_screen(&self, p: Point<f64, Logical>) -> Point<f64, Logical> {
        Point::from((
            (p.x - self.focus.x) * self.zoom + self.half.0,
            (p.y - self.focus.y) * self.zoom + self.half.1,
        ))
    }

    /// How much bigger the pointer is drawn.
    pub fn pointer_grow(&self) -> f64 {
        1. + (self.zoom - 1.) * POINTER_GROW
    }
}

struct State {
    params: Params,
    on: bool,
    output: Option<String>,
    last: Option<Duration>,
    zoom: Spring,
    fx: Spring,
    fy: Spring,
    /// where the camera is heading (Recordly's persistent focus)
    goal: (f64, f64),
    prev: Option<(f64, f64, f64)>,
    frame: Option<(String, Frame)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        params: Params { allowed: false, zoom: 1.8, smooth: 1., blur: 1. },
        on: false,
        output: None,
        last: None,
        zoom: Spring { x: 1., v: 0. },
        fx: Spring::default(),
        fy: Spring::default(),
        goal: (0., 0.),
        prev: None,
        frame: None,
    });
}

fn finite(x: f64, or: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        or
    }
}

pub fn set(params: Params) {
    STATE.with_borrow_mut(|s| {
        s.params = Params {
            allowed: params.allowed,
            zoom: finite(params.zoom, 1.8).clamp(1.1, 5.),
            smooth: finite(params.smooth, 1.).clamp(0.3, 3.),
            blur: finite(params.blur, 1.).clamp(0., 3.),
        };
        if !params.allowed {
            s.on = false;
        }
    });
}

/// In or out. Not allowed (Settings) — stays out.
pub fn toggle() {
    STATE.with_borrow_mut(|s| {
        if !s.params.allowed {
            s.on = false;
            return;
        }
        s.on = !s.on;
        if s.on && s.zoom.x <= 1.001 {
            // the output under the pointer is picked by its next frame
            s.output = None;
        }
    });
}

pub fn is_on() -> bool {
    STATE.with_borrow(|s| s.on)
}

/// Advance the camera of `output` to `now`; `pointer` is the pointer on it (output-local), if
/// it is there. None: this output is shown as is.
pub fn update(
    output: &str,
    now: Duration,
    pointer: Option<Point<f64, Logical>>,
    size: Size<f64, Logical>,
) -> Option<Frame> {
    STATE.with_borrow_mut(|s| {
        let resting = !s.on && s.zoom.x <= 1.001 && s.zoom.v.abs() < 0.01;
        if resting {
            s.output = None;
            s.frame = None;
            s.last = None;
            s.prev = None;
            s.zoom.snap(1.);
            return None;
        }
        match &s.output {
            Some(o) if o != output => return None,
            Some(_) => (),
            None => {
                // starting: the output the pointer is on, the view centred on the pointer
                let p = pointer?;
                s.output = Some(output.to_owned());
                s.fx.snap(p.x);
                s.fy.snap(p.y);
                s.goal = (p.x, p.y);
                s.zoom.snap(1.);
                s.last = None;
                s.prev = None;
            }
        }
        // a second look at the same frame, or a capture (screencopy, screencast) whose clock is
        // a little behind the screen's: the camera doesn't move for it
        if let (Some(last), Some((o, f))) = (s.last, &s.frame) {
            if now <= last && o == output {
                return Some(*f);
            }
        }

        let dt = match s.last {
            Some(last) if now > last => (now - last).as_secs_f64().min(0.1),
            _ => 0.,
        };
        s.last = Some(now);

        let p = s.params;
        let target_zoom = if s.on { p.zoom } else { 1. };
        let (w, h) = (size.w, size.h);

        // the safe zone of the view at the zoom it is going to: the goal recentres on the
        // pointer only when it leaves it (zooming out the camera holds still)
        if s.on {
            if let Some(ptr) = pointer {
                let half = (w / (2. * target_zoom), h / (2. * target_zoom));
                let inset = (half.0 * 2. * SAFE, half.1 * 2. * SAFE);
                if ptr.x < s.goal.0 - half.0 + inset.0 || ptr.x > s.goal.0 + half.0 - inset.0 {
                    s.goal.0 = ptr.x;
                }
                if ptr.y < s.goal.1 - half.1 + inset.1 || ptr.y > s.goal.1 + half.1 - inset.1 {
                    s.goal.1 = ptr.y;
                }
                s.goal.0 = s.goal.0.clamp(half.0, (w - half.0).max(half.0));
                s.goal.1 = s.goal.1.clamp(half.1, (h - half.1).max(half.1));
            }
        }

        // Recordly's zoom spring: stiffness 100/smooth, damping 21, mass smooth
        let (k, c, m) = (100. / p.smooth, 21., p.smooth);
        s.zoom.step(target_zoom, k, c, m, dt);
        s.fx.step(s.goal.0, k, c, m, dt);
        s.fy.step(s.goal.1, k, c, m, dt);

        let zoom = s.zoom.x.max(1.);
        // never past the edges at the zoom it is at now
        let half_view = (w / (2. * zoom), h / (2. * zoom));
        let focus = Point::from((
            s.fx.x.clamp(half_view.0, (w - half_view.0).max(half_view.0)),
            s.fy.x.clamp(half_view.1, (h - half_view.1).max(half_view.1)),
        ));

        // the smear: how far the view went this frame, at 60 fps, through the shutter
        let mut smear = (0., 0.);
        let mut radial = 0.;
        if let (Some((px, py, pz)), true) = (s.prev, dt > 0.) {
            let per60 = 1. / (60. * dt);
            let shutter = SHUTTER * p.blur * per60;
            let (mut sx, mut sy) = ((focus.x - px) * shutter, (focus.y - py) * shutter);
            let on_screen = sx.hypot(sy) * zoom;
            if on_screen > MAX_SMEAR {
                let k = MAX_SMEAR / on_screen;
                sx *= k;
                sy *= k;
            }
            smear = (sx, sy);
            radial = ((zoom - pz) / zoom * shutter).clamp(-0.12, 0.12);
        }
        s.prev = Some((focus.x, focus.y, zoom));

        let frame = Frame {
            zoom,
            focus,
            half: (w / 2., h / 2.),
            smear,
            radial,
        };
        s.frame = Some((output.to_owned(), frame));
        Some(frame)
    })
}

/// The camera of `output` as last worked out (for the pointer, drawn after it).
pub fn current(output: &str) -> Option<Frame> {
    STATE.with_borrow(|s| match &s.frame {
        Some((o, f)) if o == output && s.output.as_deref() == Some(output) => Some(*f),
        _ => None,
    })
}

/// Still moving (so the compositor keeps drawing).
pub fn animating() -> bool {
    STATE.with_borrow(|s| {
        if s.output.is_none() {
            return s.on;
        }
        let tz = if s.on { s.params.zoom } else { 1. };
        !s.zoom.settled(tz, 0.0005)
            || !s.fx.settled(s.goal.0, 0.05)
            || !s.fy.settled(s.goal.1, 0.05)
            || s.frame.as_ref().is_some_and(|(_, f)| {
                f.smear.0.hypot(f.smear.1) > 0.05 || f.radial.abs() > 0.0005
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_lands_without_wobble() {
        let mut sp = Spring { x: 1., v: 0. };
        let mut over = false;
        for _ in 0..240 {
            sp.step(1.8, 100., 21., 1., 1. / 60.);
            over |= sp.x > 1.81;
        }
        assert!(!over);
        assert!(sp.settled(1.8, 0.001));
    }

    #[test]
    fn a_capture_behind_the_screen_does_not_move_the_camera() {
        set(Params { allowed: true, zoom: 2., smooth: 1., blur: 1. });
        toggle();
        let size = Size::from((800., 600.));
        let t = Duration::from_secs(50);
        let p = Some(Point::from((400., 300.)));
        update("o", t, p, size).unwrap();
        let screen = update("o", t + Duration::from_millis(16), p, size).unwrap();
        let capture = update("o", t + Duration::from_millis(4), p, size).unwrap();
        assert_eq!(screen.zoom, capture.zoom);
        assert!(update("o", t + Duration::from_millis(33), p, size).is_some());
        toggle();
        set(Params { allowed: false, zoom: 1.8, smooth: 1., blur: 1. });
    }

    #[test]
    fn to_screen_keeps_focus_central() {
        let f = Frame {
            zoom: 2.,
            focus: Point::from((100., 50.)),
            half: (400., 300.),
            smear: (0., 0.),
            radial: 0.,
        };
        assert_eq!(f.to_screen(Point::from((100., 50.))), Point::from((400., 300.)));
        assert_eq!(f.to_screen(Point::from((110., 50.))), Point::from((420., 300.)));
    }
}
