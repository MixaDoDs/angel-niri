//! angel-niri: pixel shadows. `niri msg action set-pixel-shadow PIXEL` (0 = off): window shadows
//! turn on and shadow.frag draws them as dots of PIXEL logical px, thinning out along a Bayer
//! grid instead of a smooth blur. Colours, softness and offset stay niri's `layout { shadow }`
//! (without `on`, stock niri draws none); a window rule's `shadow { off; }` still wins.
//! A shell with pixel-art decorations sets it; nothing goes to niri's config.

use std::sync::atomic::{AtomicU32, Ordering};

static PIXEL: AtomicU32 = AtomicU32::new(0);

/// The dots' pixel, logical px; 0 when off.
pub fn pixel() -> f32 {
    f32::from_bits(PIXEL.load(Ordering::Relaxed))
}

pub fn set(pixel: f32) {
    PIXEL.store(pixel.max(0.).to_bits(), Ordering::Relaxed);
}

/// The layout's window shadow, on while pixel shadows are.
pub fn over(mut shadow: niri_config::Shadow) -> niri_config::Shadow {
    if pixel() > 0. {
        shadow.on = true;
    }
    shadow
}
