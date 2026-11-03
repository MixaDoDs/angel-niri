//! angel-niri: a living pointer. `niri msg action set-cursor-fx true [--size 2] [--sway 1]
//! [--bounce 1] [--shadow 1]` (`false` turns it off; the pointer is then drawn exactly as before).
//!
//! The pointer leans into its motion on a damped spring and swings back when it stops, dips on a
//! click with a ring spreading out from the tip, and sits over a hard pixel shadow one art pixel
//! away (two when it flies, tucked under on a click). The ideas follow Recordly's cursor renderer
//! (sway into motion, click bounce, ripple; github.com/webadderall/Recordly, AGPL) — written anew
//! here for a live compositor: the tip is never smoothed or delayed, so the pointer stays exactly
//! where the mouse is; only the picture around the tip moves. No smear: copies along the path
//! fall apart into a row of arrows on a fast flick (03.11, the owner: «дохуя курсоров»).
//!
//! Pictures are made on the CPU and cached: the arrow turned and scaled by supersampled nearest
//! neighbour (whole sizes only, so pixel cursors keep their pixels), its shadow the same
//! silhouette in dark grey, and the ring per step of its growth.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::input::pointer::CursorIcon;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Transform};

use crate::cursor::XCursor;

/// Lean at full speed, degrees (half of Recordly's 12°: a turned pixel arrow gets soft edges).
const SWAY_MAX_DEG: f64 = 6.;
/// Speed of full lean, logical px/s.
const SWAY_SPEED_REF: f64 = 1400.;
const SWAY_VERTICAL_WEIGHT: f64 = 0.65;
/// Click dip: 1 − sin(πp)·DIP over BOUNCE_MS (Recordly: 0.08 · bounce 2).
const DIP: f64 = 0.16;
const BOUNCE_MS: f64 = 350.;
/// Changing shape (arrow → hand) pops up by this much over POP_MS.
const POP: f64 = 0.12;
const POP_MS: f64 = 240.;
/// Ring: grows to RING_REACH × the arrow's height and fades over RING_MS.
const RING_MS: f64 = 480.;
const RING_REACH: f64 = 1.25;
const RING_STEPS: u32 = 28;
/// The shadow: the arrow's own silhouette, hard-edged, down-right by whole art pixels — one at
/// rest, two lifted (fast), none pressed. (The reference's 0.4 / 0.7 of the arrow's height read
/// as a separate dark blob on a live desktop.)
const SHADOW_REST_PX: f64 = 1.;
const SHADOW_LIFT_PX: f64 = 2.;
const SHADOW_ALPHA: f64 = 0.50;
/// Speed at which the arrow is fully lifted, logical px/s.
const LIFT_SPEED: f64 = 1800.;
/// How much bigger the arrow looks when lifted.
const LIFT_GROW: f64 = 0.06;

#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub on: bool,
    pub size: f64,
    pub sway: f64,
    pub bounce: f64,
    pub shadow: f64,
    pub shadow_dist: f64,
}

#[derive(Clone, Copy, Default)]
struct Spring {
    x: f64,
    v: f64,
}

impl Spring {
    /// Damped spring toward `target`: stiffness k, damping c, mass 1; small substeps.
    fn step(&mut self, target: f64, k: f64, c: f64, mut dt: f64) {
        while dt > 0. {
            let h = dt.min(0.002);
            let a = -k * (self.x - target) - c * self.v;
            self.v += a * h;
            self.x += self.v * h;
            dt -= h;
        }
    }

    fn settled(&self, target: f64) -> bool {
        (self.x - target).abs() < 0.002 && self.v.abs() < 0.02
    }
}

struct State {
    params: Params,
    last: Option<Duration>,
    pos: Point<f64, Logical>,
    vel: (f64, f64),
    tilt: Spring,
    lift: Spring,
    press_at: Option<Duration>,
    pop_at: Option<Duration>,
    icon: Option<CursorIcon>,
    pending_click: bool,
    ripples: Vec<(Point<f64, Logical>, Duration)>,
    fx: Option<Fx>,
    /// the last client cursor picture seen (its key), so a change of it pops like a shape change
    surface_key: Option<u64>,
}

/// What to draw this frame.
#[derive(Clone, Debug)]
pub struct Fx {
    pub size: f64,
    pub angle_deg: f64,
    pub scale: f64,
    /// shadow offset (whole art pixels: × `size` for logical) and alpha
    pub shadow: (f64, f64, f32),
    pub shadow_on: bool,
    /// rings: global position and progress 0..1
    pub ripples: Vec<(Point<f64, Logical>, f64)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        params: Params {
            on: false,
            size: 1.,
            sway: 1.,
            bounce: 1.,
            shadow: 1.,
            shadow_dist: 1.,
        },
        last: None,
        pos: Point::from((0., 0.)),
        vel: (0., 0.),
        tilt: Spring::default(),
        lift: Spring::default(),
        press_at: None,
        pop_at: None,
        icon: None,
        pending_click: false,
        ripples: vec![],
        fx: None,
        surface_key: None,
    });
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

pub fn set(params: Params) {
    let p = Params {
        on: params.on,
        // whole sizes only: at 1.5× a pixel arrow gets pixels of uneven width
        size: finite(params.size, 1.).round().clamp(1., 4.),
        sway: finite(params.sway, 1.).clamp(0., 3.),
        bounce: finite(params.bounce, 1.).clamp(0., 3.),
        shadow: finite(params.shadow, 1.).clamp(0., 3.),
        shadow_dist: finite(params.shadow_dist, 1.).clamp(0., 3.),
    };
    STATE.with_borrow_mut(|s| {
        s.params = p;
        s.last = None;
        s.fx = None;
        s.ripples.clear();
    });
    clear_cache();
}

fn finite(x: f64, or: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        or
    }
}

pub fn is_on() -> bool {
    STATE.with_borrow(|s| s.params.on)
}

/// A mouse button went down or up.
pub fn button(pressed: bool, now: Duration) {
    STATE.with_borrow_mut(|s| {
        if s.params.on && pressed {
            s.press_at = Some(now);
            s.pending_click = true;
        }
    });
}

/// Advance to `now` with the pointer at global `pos` showing `icon` (None for a client's own
/// picture). Called once per output per frame; only the first call of a frame moves things on.
pub fn update(
    now: Duration,
    pos: Point<f64, Logical>,
    icon: Option<CursorIcon>,
    visible: bool,
) -> Option<Fx> {
    STATE.with_borrow_mut(|s| {
        if !s.params.on {
            return None;
        }
        // a second output, or a frame drawn for a capture (screencopy, screencast) whose clock is
        // a little behind the screen's: nothing moves on, the picture is the one worked out
        if let (Some(fx), Some(last)) = (&s.fx, s.last) {
            if now <= last {
                return Some(fx.clone());
            }
        }
        let p = s.params;

        let dt = match s.last {
            Some(last) if now > last => (now - last).as_secs_f64(),
            _ => 0.,
        };
        // a pause without frames: nothing was moving, start afresh
        let dt = if dt > 0.1 { 0. } else { dt };

        if dt > 0. {
            let (dx, dy) = (pos.x - s.pos.x, pos.y - s.pos.y);
            // a jump (warp, other output) is not motion
            if dx.hypot(dy) > 400. {
                s.vel = (0., 0.);
            } else {
                let raw = (dx / dt, dy / dt);
                let a = 1. - (-dt / 0.03).exp();
                s.vel = (s.vel.0 + (raw.0 - s.vel.0) * a, s.vel.1 + (raw.1 - s.vel.1) * a);
            }
        } else {
            s.vel = (0., 0.);
        }
        s.pos = pos;
        s.last = Some(now);

        // lean into the motion
        let (vx, vy) = s.vel;
        let speed = vx.hypot(vy);
        let tilt_target = if speed > 1. {
            let bias = ((vx + vy * SWAY_VERTICAL_WEIGHT) / speed).clamp(-1., 1.);
            bias * (speed / SWAY_SPEED_REF).clamp(0., 1.) * SWAY_MAX_DEG * p.sway
        } else {
            0.
        };
        // a lively spring (ζ ≈ 0.45): it swings back a little once the pointer stops
        s.tilt.step(tilt_target, 260., 14.5, dt);
        let lift_target = (speed / LIFT_SPEED).clamp(0., 1.) * p.shadow.min(1.);
        s.lift.step(lift_target, 120., 20., dt);

        // shape changes pop
        if s.icon.is_some() && icon.is_some() && s.icon != icon {
            s.pop_at = Some(now);
        }
        if icon.is_some() {
            s.icon = icon;
        }

        // clicks
        if s.pending_click {
            s.pending_click = false;
            if visible && p.bounce > 0. {
                s.ripples.push((pos, now));
            }
        }
        let ring = Duration::from_millis(RING_MS as u64);
        s.ripples.retain(|(_, at)| now < *at + ring);

        let progress = |at: Option<Duration>, ms: f64| -> Option<f64> {
            let t = now.checked_sub(at?)?.as_secs_f64() * 1000. / ms;
            (t < 1.).then_some(t)
        };
        let dip = progress(s.press_at, BOUNCE_MS)
            .map_or(0., |t| (t * PI).sin() * DIP * p.bounce.min(2.));
        let pop = progress(s.pop_at, POP_MS).map_or(0., |t| (t * PI).sin() * POP);
        let lift = s.lift.x.clamp(0., 1.);

        let scale = ((1. - dip) * (1. + pop) * (1. + LIFT_GROW * lift)).max(0.6);

        let pressed = (dip / DIP.max(1e-6)).clamp(0., 1.);
        let dist = ((SHADOW_REST_PX + (SHADOW_LIFT_PX - SHADOW_REST_PX) * lift)
            * (1. - pressed)
            * p.shadow_dist)
            .round();
        let sa = (SHADOW_ALPHA * (1. - 0.12 * lift + 0.10 * pressed) * p.shadow).min(0.9);

        let fx = Fx {
            size: p.size,
            angle_deg: s.tilt.x,
            scale,
            shadow: (dist, dist, sa as f32),
            shadow_on: p.shadow > 0.,
            ripples: s
                .ripples
                .iter()
                .map(|(at, t0)| {
                    let t = now.saturating_sub(*t0).as_secs_f64() * 1000. / RING_MS;
                    (*at, t.clamp(0., 1.))
                })
                .collect(),
        };
        s.fx = Some(fx.clone());
        Some(fx)
    })
}

/// Whether the next frames still move (so the compositor keeps drawing).
pub fn animating(now: Duration) -> bool {
    STATE.with_borrow(|s| {
        if !s.params.on {
            return false;
        }
        let busy = |at: Option<Duration>, ms: f64| {
            at.is_some_and(|at| now < at + Duration::from_millis(ms as u64 + 20))
        };
        !s.tilt.settled(0.)
            || !s.lift.settled(0.)
            || s.vel.0.hypot(s.vel.1) > 2.
            || !s.ripples.is_empty()
            || s.pending_click
            || busy(s.press_at, BOUNCE_MS)
            || busy(s.pop_at, POP_MS)
    })
}

// ---- pictures ----

#[derive(Clone)]
pub struct Frame {
    pub buf: MemoryRenderBuffer,
    /// the tip inside `buf`, logical
    pub hot: Point<f64, Logical>,
    /// the arrow's silhouette in dark grey, same size and tip as `buf`
    pub shadow: MemoryRenderBuffer,
    /// arrow height, logical (for the ring and shadow distances)
    pub height: f64,
}

#[derive(Default)]
struct Cache {
    frames: HashMap<(usize, CursorIcon, i32, usize, i32, i32), Frame>,
    rings: HashMap<(i32, i32, u32), (MemoryRenderBuffer, f64)>,
}

pub fn clear_cache() {
    CACHE.with_borrow_mut(|c| {
        c.frames.clear();
        c.rings.clear();
    });
}

/// The arrow `cursor` (frame `idx`, buffer scale `scale`) turned and scaled as `fx` says.
pub fn frame(icon: CursorIcon, scale: i32, cursor: &XCursor, idx: usize, fx: &Fx) -> Frame {
    // steps: half a degree, 1/64 of size
    let a_step = (fx.angle_deg * 2.).round() as i32;
    let k_step = (fx.scale * fx.size * 64.).round().max(16.) as i32;
    let key = (
        cursor as *const XCursor as usize,
        icon,
        scale,
        idx,
        a_step,
        k_step,
    );
    CACHE.with_borrow_mut(|c| {
        if c.frames.len() > 900 {
            c.frames.clear();
        }
        c.frames
            .entry(key)
            .or_insert_with(|| {
                let img = &cursor.frames()[idx];
                build_frame(
                    &img.pixels_rgba,
                    img.width as usize,
                    img.height as usize,
                    (f64::from(img.xhot), f64::from(img.yhot)),
                    f64::from(a_step) / 2.,
                    f64::from(k_step) / 64.,
                    scale,
                )
            })
            .clone()
    })
}

/// A client's own cursor picture (X11 apps through xwayland-satellite — Steam, Wine/Proton
/// games — and anything else not using cursor-shape): its pixels read from the shm buffer,
/// premultiplied B G R A like an XCursor's. `key` names the picture for the cache.
pub struct RawCursor {
    pub key: u64,
    pub px: Vec<u8>,
    pub w: usize,
    pub h: usize,
    pub scale: i32,
}

/// The client cursor `surface`'s picture, when it can be read: one shm buffer (ARGB / XRGB),
/// no subsurfaces, no transform or viewport. Otherwise None and the cursor is drawn as before.
pub fn surface_pixels(surface: &WlSurface) -> Option<RawCursor> {
    use std::hash::{Hash, Hasher};

    use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
    use smithay::reexports::wayland_server::protocol::wl_shm;
    use smithay::wayland::compositor::{get_children, with_states};
    use smithay::wayland::shm::with_buffer_contents;

    if !get_children(surface).is_empty() {
        return None;
    }
    with_states(surface, |states| {
        let data = states.data_map.get::<RendererSurfaceStateUserData>()?;
        let data = data.lock().ok()?;
        if data.buffer_transform() != Transform::Normal {
            return None;
        }
        let scale = data.buffer_scale().max(1);
        if data.surface_size() != data.buffer_size() {
            return None;
        }
        let buffer = data.buffer()?;
        with_buffer_contents(buffer, |ptr, len, d| {
            let opaque = match d.format {
                wl_shm::Format::Argb8888 => false,
                wl_shm::Format::Xrgb8888 => true,
                _ => return None,
            };
            let (w, h, stride) = (d.width as usize, d.height as usize, d.stride as usize);
            if w == 0 || h == 0 || w > 512 || h > 512 || stride < w * 4 {
                return None;
            }
            let off = d.offset as usize;
            if off + stride * (h - 1) + w * 4 > len {
                return None;
            }
            // SAFETY: the range was checked against the pool's length just above
            let pool = unsafe { std::slice::from_raw_parts(ptr, len) };
            let mut px = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                let row = off + y * stride;
                px.extend_from_slice(&pool[row..row + w * 4]);
            }
            if opaque {
                for a in px.iter_mut().skip(3).step_by(4) {
                    *a = 255;
                }
            }
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (w, h, scale).hash(&mut hasher);
            px.hash(&mut hasher);
            Some(RawCursor {
                key: hasher.finish(),
                px,
                w,
                h,
                scale,
            })
        })
        .ok()
        .flatten()
    })
}

/// The client's picture changed (a new shape): pop it like a theme cursor's shape change.
pub fn surface_shape(now: Duration, key: u64) {
    STATE.with_borrow_mut(|s| {
        if s.params.on && s.surface_key.is_some_and(|k| k != key) {
            s.pop_at = Some(now);
        }
        s.surface_key = Some(key);
    });
}

/// A client's cursor picture (tip at `hot`, logical) turned and scaled as `fx` says.
pub fn frame_raw(raw: &RawCursor, hot: Point<i32, Logical>, fx: &Fx) -> Frame {
    let a_step = (fx.angle_deg * 2.).round() as i32;
    let k_step = (fx.scale * fx.size * 64.).round().max(16.) as i32;
    // the high bit keeps client pictures apart from the theme's (keyed by address)
    let key = (
        (raw.key as usize) | (1 << (usize::BITS - 1)),
        CursorIcon::Default,
        raw.scale,
        0,
        a_step,
        k_step,
    );
    CACHE.with_borrow_mut(|c| {
        if c.frames.len() > 900 {
            c.frames.clear();
        }
        c.frames
            .entry(key)
            .or_insert_with(|| {
                let s = f64::from(raw.scale);
                build_frame(
                    &raw.px,
                    raw.w,
                    raw.h,
                    (f64::from(hot.x) * s, f64::from(hot.y) * s),
                    f64::from(a_step) / 2.,
                    f64::from(k_step) / 64.,
                    raw.scale,
                )
            })
            .clone()
    })
}

#[allow(clippy::too_many_arguments)]
fn build_frame(
    src: &[u8],
    w: usize,
    h: usize,
    hot: (f64, f64),
    angle_deg: f64,
    k: f64,
    scale: i32,
) -> Frame {
    let a = angle_deg.to_radians();
    let (sin, cos) = a.sin_cos();
    // where the corners go (tip at the origin; y down, positive = clockwise)
    let fwd = |x: f64, y: f64| ((x * cos - y * sin) * k, (x * sin + y * cos) * k);
    let corners = [(0., 0.), (w as f64, 0.), (0., h as f64), (w as f64, h as f64)];
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (cx, cy) in corners {
        let (x, y) = fwd(cx - hot.0, cy - hot.1);
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    let ox = (-x0).ceil() as i64 + 1;
    let oy = (-y0).ceil() as i64 + 1;
    let nw = (x1.ceil() as i64 + ox + 1).max(1) as usize;
    let nh = (y1.ceil() as i64 + oy + 1).max(1) as usize;

    // 4×4 samples per pixel, each the nearest source pixel
    const SS: usize = 4;
    let mut px = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0u32; 4];
            for j in 0..SS {
                for i in 0..SS {
                    let dx = x as f64 + (i as f64 + 0.5) / SS as f64 - ox as f64;
                    let dy = y as f64 + (j as f64 + 0.5) / SS as f64 - oy as f64;
                    // back: unscale, turn the other way
                    let ux = (dx * cos + dy * sin) / k + hot.0;
                    let uy = (-dx * sin + dy * cos) / k + hot.1;
                    if ux < 0. || uy < 0. {
                        continue;
                    }
                    let (sx, sy) = (ux as usize, uy as usize);
                    if sx >= w || sy >= h {
                        continue;
                    }
                    let s = (sy * w + sx) * 4;
                    for c in 0..4 {
                        acc[c] += u32::from(src[s + c]);
                    }
                }
            }
            let d = (y * nw + x) * 4;
            for c in 0..4 {
                px[d + c] = (acc[c] / (SS * SS) as u32) as u8;
            }
        }
    }

    // the shadow: the same silhouette, neutral dark grey (premultiplied B G R A)
    let height_px = h as f64 * k;
    let mut spx = vec![0u8; nw * nh * 4];
    for i in 0..nw * nh {
        let a = px[i * 4 + 3];
        spx[i * 4..i * 4 + 3].fill((u32::from(a) * 4 / 255) as u8);
        spx[i * 4 + 3] = a;
    }

    let s = f64::from(scale);
    let buf = MemoryRenderBuffer::from_slice(
        &px,
        Fourcc::Argb8888,
        (nw as i32, nh as i32),
        scale,
        Transform::Normal,
        None,
    );
    let shadow = MemoryRenderBuffer::from_slice(
        &spx,
        Fourcc::Argb8888,
        (nw as i32, nh as i32),
        scale,
        Transform::Normal,
        None,
    );
    Frame {
        buf,
        hot: Point::from((ox as f64 / s, oy as f64 / s)),
        shadow,
        height: height_px / s,
    }
}

/// The ring at progress `t` (0..1) for an arrow `height` logical px high: the picture, the
/// offset of its centre (logical) and its alpha.
pub fn ring(scale: i32, height: f64, t: f64) -> (MemoryRenderBuffer, f64, f32) {
    let step = ((t * f64::from(RING_STEPS)).round() as u32).min(RING_STEPS);
    let hkey = (height * 4.).round() as i32;
    let (buf, centre) = CACHE.with_borrow_mut(|c| {
        if c.rings.len() > 400 {
            c.rings.clear();
        }
        c.rings
            .entry((scale, hkey, step))
            .or_insert_with(|| build_ring(scale, height, f64::from(step) / f64::from(RING_STEPS)))
            .clone()
    });
    // remaining = 1 − t; fade as Recordly: remaining³
    let rem = 1. - t;
    let alpha = (rem * rem * rem * 0.75).clamp(0., 1.) as f32;
    (buf, centre, alpha)
}

fn build_ring(scale: i32, height: f64, t: f64) -> (MemoryRenderBuffer, f64) {
    let s = f64::from(scale);
    let max_r = height * RING_REACH * s;
    let rem = 1. - t;
    let radius = ((1. - rem * rem * rem) * max_r).max(0.5 * s);
    let stroke = (2.2 * rem.powf(1.5)).max(1.) * s;
    let half = (max_r + stroke * 2. + 2.).ceil() as usize;
    let n = half * 2;
    let mut px = vec![0u8; n * n * 4];
    let c = half as f64;
    for y in 0..n {
        for x in 0..n {
            let d = ((x as f64 + 0.5 - c).powi(2) + (y as f64 + 0.5 - c).powi(2)).sqrt();
            // the ring, and a very faint fill inside it
            let ring = (stroke / 2. - (d - radius).abs() + 0.5).clamp(0., 1.);
            let fill = if d < radius { 0.10 * rem } else { 0. };
            let a = ring.max(fill);
            if a <= 0. {
                continue;
            }
            let i = (y * n + x) * 4;
            // light grey with a lilac hint, premultiplied B G R A
            px[i] = (a * 214.) as u8;
            px[i + 1] = (a * 200.) as u8;
            px[i + 2] = (a * 204.) as u8;
            px[i + 3] = (a * 255.) as u8;
        }
    }
    let buf = MemoryRenderBuffer::from_slice(
        &px,
        Fourcc::Argb8888,
        (n as i32, n as i32),
        scale,
        Transform::Normal,
        None,
    );
    (buf, c / s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_swings_and_settles() {
        let mut sp = Spring::default();
        sp.x = 10.;
        let mut crossed = false;
        for _ in 0..200 {
            sp.step(0., 260., 14.5, 1. / 60.);
            crossed |= sp.x < 0.;
        }
        assert!(crossed, "a lively spring swings past rest");
        assert!(sp.settled(0.));
    }

    #[test]
    fn a_capture_behind_the_screen_after_a_click_is_fine() {
        // a recording (screencopy) draws with a clock a little behind the screen's: a click
        // seen on the screen first must not make its ring older than the capture's frame
        set(Params {
            on: true,
            size: 1.,
            sway: 1.,
            bounce: 1.,
            shadow: 1.,
            shadow_dist: 1.,
        });
        let t = Duration::from_secs(100);
        let at = Point::from((10., 10.));
        update(t, at, None, true);
        button(true, t + Duration::from_millis(16));
        let screen = update(t + Duration::from_millis(16), at, None, true).unwrap();
        assert_eq!(screen.ripples.len(), 1);
        let capture = update(t + Duration::from_millis(5), at, None, true).unwrap();
        assert_eq!(capture.ripples.len(), 1);
        assert!(update(t + Duration::from_millis(33), at, None, true).is_some());
        set(Params {
            on: false,
            size: 1.,
            sway: 1.,
            bounce: 1.,
            shadow: 1.,
            shadow_dist: 1.,
        });
    }

    #[test]
    fn shadow_one_art_pixel_away_none_on_a_click_and_whole_sizes() {
        let params = |on| Params {
            on,
            size: 1.5,
            sway: 1.,
            bounce: 1.,
            shadow: 1.,
            shadow_dist: 1.,
        };
        set(params(true));
        let t = Duration::from_secs(200);
        let at = Point::from((50., 50.));
        let rest = update(t, at, None, true).unwrap();
        assert_eq!(rest.size, 2., "1.5 is drawn at a whole size");
        assert_eq!((rest.shadow.0, rest.shadow.1), (1., 1.));
        button(true, t + Duration::from_millis(10));
        // the deepest point of the dip, half way through it
        let mid = t + Duration::from_millis(10 + BOUNCE_MS as u64 / 2);
        update(t + Duration::from_millis(10), at, None, true);
        let pressed = update(mid, at, None, true).unwrap();
        assert_eq!((pressed.shadow.0, pressed.shadow.1), (0., 0.));
        set(params(false));
    }

    #[test]
    fn turned_frame_keeps_the_tip() {
        // 4×4 opaque square, tip at the corner, scaled 2×, not turned
        let src = vec![255u8; 4 * 4 * 4];
        let f = build_frame(&src, 4, 4, (0., 0.), 0., 2., 1);
        assert_eq!(f.hot, Point::from((1., 1.)));
    }

    #[test]
    fn a_client_picture_keeps_its_tip_and_its_own_cache_slot() {
        // a client's 8×8 picture at buffer scale 2, tip at (2, 3) logical = (4, 6) px
        let raw = RawCursor {
            key: 42,
            px: vec![255u8; 8 * 8 * 4],
            w: 8,
            h: 8,
            scale: 2,
        };
        let fx = Fx {
            size: 1.,
            angle_deg: 0.,
            scale: 1.,
            shadow: (0., 0., 0.),
            shadow_on: false,
            ripples: vec![],
        };
        let f = frame_raw(&raw, Point::from((2, 3)), &fx);
        // the tip: its 4×6 px of picture above-left plus the 1 px margin, logical at scale 2
        assert_eq!(f.hot, Point::from((2.5, 3.5)));
        assert_eq!(f.height, 4.);
        let other = RawCursor { key: 43, ..raw };
        let g = frame_raw(&other, Point::from((0, 0)), &fx);
        assert_eq!(g.hot, Point::from((0.5, 0.5)));
    }
}
