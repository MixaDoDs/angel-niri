//! angel-niri: a bigger pointer for a moment. `niri msg action set-cursor-zoom ZOOM [--ms MS]`
//! grows the arrow to ZOOM times its size (1 = back) over MS milliseconds — the shell's
//! "shake to find the pointer", as macOS does it: drawn by the compositor, so the pointer keeps
//! working over every window (hover, clicks, drags) while it is big. The arrow is scaled up by
//! nearest neighbour (pixel cursors stay crisp); a client's own cursor surface gives way to the
//! theme's arrow while zoomed. Nothing goes to niri's config.

use std::cell::Cell;
use std::time::Duration;

#[derive(Clone, Copy)]
struct Zoom {
    from: f64,
    to: f64,
    start: Duration,
    ms: u32,
}

thread_local! {
    static ZOOM: Cell<Zoom> = const {
        Cell::new(Zoom {
            from: 1.,
            to: 1.,
            start: Duration::ZERO,
            ms: 0,
        })
    };
}

pub const MAX: f64 = 8.;

/// Grow (or shrink back) from where it is now.
pub fn set(to: f64, ms: u32, now: Duration) {
    let from = value(now);
    let to = if to.is_finite() {
        to.clamp(1., MAX)
    } else {
        1.
    };
    ZOOM.set(Zoom {
        from,
        to,
        start: now,
        ms,
    });
}

/// The zoom at `now`: eased out, so it pops up and settles.
pub fn value(now: Duration) -> f64 {
    let z = ZOOM.get();
    if z.ms == 0 {
        return z.to;
    }
    let t = (now.saturating_sub(z.start).as_secs_f64() * 1000. / f64::from(z.ms)).clamp(0., 1.);
    let e = 1. - (1. - t).powi(3);
    z.from + (z.to - z.from) * e
}

pub fn animating(now: Duration) -> bool {
    let z = ZOOM.get();
    z.ms > 0 && now < z.start + Duration::from_millis(u64::from(z.ms))
}

/// Whether the arrow is drawn bigger at all.
pub fn on(now: Duration) -> bool {
    value(now) > 1.01
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_and_settles() {
        let t0 = Duration::from_secs(10);
        set(4., 200, t0);
        assert_eq!(value(t0), 1.);
        assert!(animating(t0 + Duration::from_millis(100)));
        let mid = value(t0 + Duration::from_millis(100));
        assert!(mid > 2.5 && mid < 4.);
        assert_eq!(value(t0 + Duration::from_millis(300)), 4.);
        assert!(!animating(t0 + Duration::from_millis(300)));
        // back from wherever it is, clamped
        set(0.5, 0, t0 + Duration::from_millis(300));
        assert_eq!(value(t0 + Duration::from_millis(300)), 1.);
        assert!(!on(t0 + Duration::from_millis(300)));
    }
}
