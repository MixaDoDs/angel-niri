//! angel-niri: inactive windows dimmed. `niri msg action set-inactive-dim PIXEL [--alpha A]`
//! (0 = off): every window but the focused one gets a sparse grid of dark dots (dim.frag), one of
//! PIXEL logical px in every two by two, at opacity A. Not while fullscreen or maximized.
//! A shell with pixel-art decorations sets it; nothing goes to niri's config.

use std::sync::atomic::{AtomicU32, Ordering};

static PIXEL: AtomicU32 = AtomicU32::new(0);
static ALPHA: AtomicU32 = AtomicU32::new(0);

/// The dots' pixel, logical px; 0 when off.
pub fn pixel() -> f32 {
    f32::from_bits(PIXEL.load(Ordering::Relaxed))
}

/// The dots' opacity.
pub fn alpha() -> f32 {
    f32::from_bits(ALPHA.load(Ordering::Relaxed))
}

pub fn set(pixel: f32, alpha: f32) {
    PIXEL.store(pixel.max(0.).to_bits(), Ordering::Relaxed);
    ALPHA.store(alpha.clamp(0., 1.).to_bits(), Ordering::Relaxed);
}
