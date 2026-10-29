//! angel-niri: Aero Peek, as Windows has it on its taskbar. `niri msg action peek-window --id N`:
//! the other windows on that window's output melt away through a 4×4 Bayer of art pixels (the
//! throw shader's cells, `Tile::render`) until none of them is left, so N alone is seen where it
//! stands, nothing over it; without `--id`, they come back. Only while N is in sight on its
//! output's workspace in front; the other outputs (the stream's one too) keep their windows.
//! Nothing moves, focus and input stay as they were. A shell calls it while the pointer rests on
//! a taskbar button; nothing goes to niri's config.

use crate::animation::{Animation, Clock, Curve};

/// What is left of a ghost: this much of its Bayer cells (nothing: dots of a window above the
/// peeked one would speckle it).
pub const LEFT: f64 = 0.;
/// How much of the peeked window must be in the view, logical px each way: a column scrolled
/// out of the Strip would leave nothing to see.
pub const SEEN: f64 = 64.;
/// Melting away, coming back a little quicker.
const OUT_MS: u64 = 180;
const BACK_MS: u64 = 140;

/// The melting, 0 whole .. 1 a ghost, from `from` to `to`.
pub fn animation(clock: Clock, from: f64, to: f64) -> Animation {
    let ms = if to > from { OUT_MS } else { BACK_MS };
    Animation::ease(clock, from, to, 0., ms, Curve::EaseOutCubic)
}

/// The share of a window's Bayer cells still drawn, melted `ghost` (0 .. 1) of the way.
pub fn keep(ghost: f64) -> f64 {
    1. - ghost.clamp(0., 1.) * (1. - LEFT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ghost_melts_away() {
        assert_eq!(keep(0.), 1.);
        assert_eq!(keep(1.), LEFT);
        assert!(keep(0.5) < 1. && keep(0.5) > LEFT);
        assert!(keep(1.) <= 0.);
    }
}
