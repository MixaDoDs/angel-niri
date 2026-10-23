//! angel-niri: inactive windows dimmed. `niri msg action set-inactive-dim PIXEL [--alpha A]
//! [--output O]...` (0 = off): every window but the focused one gets a vignette of dark dots
//! (dim.frag), PIXEL logical px each, ordered-dithered from dense at the edges to none in the
//! middle, at opacity A; with outputs, only on those (the others are for work).
//! Not while fullscreen or maximized.
//! A shell with pixel-art decorations sets it; nothing goes to niri's config.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use smithay::output::Output;

static PIXEL: AtomicU32 = AtomicU32::new(0);
static ALPHA: AtomicU32 = AtomicU32::new(0);
// whether the workspace being updated is on an output with the dots
static HERE: AtomicBool = AtomicBool::new(true);

thread_local! {
    // the outputs with the dots; empty: every output
    static OUTPUTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The dots' pixel, logical px, on the output being updated (`enter`); 0 when off.
pub fn pixel() -> f32 {
    if !HERE.load(Ordering::Relaxed) {
        return 0.;
    }
    f32::from_bits(PIXEL.load(Ordering::Relaxed))
}

/// A workspace's render elements are updated next, on this output.
pub fn enter(output: Option<&Output>) {
    let here = OUTPUTS.with_borrow(|o| {
        o.is_empty() || output.is_some_and(|out| o.iter().any(|n| *n == out.name()))
    });
    HERE.store(here, Ordering::Relaxed);
}

/// The dots' opacity.
pub fn alpha() -> f32 {
    f32::from_bits(ALPHA.load(Ordering::Relaxed))
}

pub fn set(pixel: f32, alpha: f32, outputs: Vec<String>) {
    PIXEL.store(pixel.max(0.).to_bits(), Ordering::Relaxed);
    ALPHA.store(alpha.clamp(0., 1.).to_bits(), Ordering::Relaxed);
    OUTPUTS.set(outputs);
}

#[cfg(test)]
mod tests {
    use super::*;

    // only on the outputs named (none named: everywhere)
    #[test]
    fn dots_only_on_their_outputs() {
        set(2., 0.5, Vec::new());
        enter(None);
        assert_eq!(pixel(), 2.);
        set(2., 0.5, vec![String::from("DP-1")]);
        enter(None);
        assert_eq!(pixel(), 0.);
        set(0., 0.5, Vec::new());
        enter(None);
    }
}
