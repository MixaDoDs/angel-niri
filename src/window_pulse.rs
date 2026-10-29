//! angel-niri: the windows spring to the music. `niri msg action window-pulse PX [--ms MS]`
//! grows every window by PX logical pixels on each side and back over MS milliseconds — the
//! shell sends it on each kick of the bass (services/WindowPulse). Drawn only: the layout, the
//! focus and the input regions don't move. Whole pixels of the pixel corners' size, so pixel art
//! stays sharp; fullscreen windows (games, videos) are left alone. Nothing goes to niri's config.

use std::cell::Cell;
use std::time::Duration;

#[derive(Clone, Copy)]
struct Pulse {
    px: f64,
    start: Duration,
    ms: u32,
}

thread_local! {
    static PULSE: Cell<Pulse> = const {
        Cell::new(Pulse {
            px: 0.,
            start: Duration::ZERO,
            ms: 0,
        })
    };
}

pub const MAX: f64 = 8.;

pub fn set(px: f64, ms: u32, now: Duration) {
    let px = if px.is_finite() { px.clamp(0., MAX) } else { 0. };
    PULSE.set(Pulse { px, start: now, ms });
}

/// How far the windows have grown at `now`, logical px: up in the first fifth, back down after.
pub fn grow(now: Duration) -> f64 {
    let p = PULSE.get();
    if p.ms == 0 || p.px <= 0. {
        return 0.;
    }
    let t = now.saturating_sub(p.start).as_secs_f64() * 1000. / f64::from(p.ms);
    if t >= 1. {
        return 0.;
    }
    let e = if t < 0.2 { t / 0.2 } else { (1. - t) / 0.8 };
    p.px * e
}

pub fn animating(now: Duration) -> bool {
    let p = PULSE.get();
    p.ms > 0 && p.px > 0. && now < p.start + Duration::from_millis(u64::from(p.ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rises_and_falls_back_to_nothing() {
        let at = |ms| Duration::from_millis(ms);
        set(2., 200, at(1000));
        assert_eq!(grow(at(1000)), 0.);
        assert!((grow(at(1040)) - 2.).abs() < 1e-9);
        assert!(grow(at(1120)) > 0. && grow(at(1120)) < 2.);
        assert_eq!(grow(at(1200)), 0.);
        assert!(animating(at(1199)));
        assert!(!animating(at(1200)));
        set(f64::NAN, 200, at(0));
        assert!(!animating(at(10)));
    }
}
