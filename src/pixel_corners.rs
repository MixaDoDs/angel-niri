//! angel-niri: pixel corners. `niri msg action set-pixel-corners PIXEL [--steps N]` (0 = off):
//! windows get staircase corners of N steps of PIXEL logical px (a window rule's radius of 0, or
//! none, becomes N × PIXEL), drawn with hard edges by rounding_alpha.frag, their border too.
//! A shell with pixel-art decorations sets it; nothing goes to niri's config.

use std::sync::atomic::{AtomicU32, Ordering};

static PIXEL: AtomicU32 = AtomicU32::new(0);
static STEPS: AtomicU32 = AtomicU32::new(2);

/// The staircase's pixel, logical px; 0 when off.
pub fn pixel() -> f32 {
    f32::from_bits(PIXEL.load(Ordering::Relaxed))
}

/// The radius a window without its own gets.
pub fn default_radius() -> f32 {
    pixel() * STEPS.load(Ordering::Relaxed) as f32
}

pub fn set(pixel: f32, steps: u32) {
    PIXEL.store(pixel.max(0.).to_bits(), Ordering::Relaxed);
    STEPS.store(steps.max(1), Ordering::Relaxed);
}
