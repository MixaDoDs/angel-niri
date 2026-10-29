//! angel-niri: a window see-through in dots. `set-dither-wheel` lets Alt+wheel over a window step
//! how much of it is drawn — a 4×4 Bayer screen door of art pixels (`pixel_corners::pixel()`, one
//! physical pixel when off), no blending and no blur: the pixel look. `set-window-dither LEVEL
//! [--id N]` sets it outright. Drawn by the clipped-surface shader (`niri_dither`); a fullscreen
//! window stays whole and keeps its wheel. Nothing is remembered. Nothing goes to niri's config.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::layout::LayoutElement as _;
use crate::niri::State;

static WHEEL: AtomicBool = AtomicBool::new(false);

/// A wheel notch is an eighth (two of the sixteen Bayer levels); a quarter is the least, so the
/// window never vanishes under the pointer.
const STEP: f32 = 0.125;
const LEAST: f32 = 0.25;

thread_local! {
    /// The wheel turned since the last step, v120 (hi-res wheels send parts of a notch).
    static TURNED: Cell<f64> = const { Cell::new(0.) };
}

pub fn set_wheel(on: bool) {
    WHEEL.store(on, Ordering::Relaxed);
}

/// How much of a window to draw at `level`: between a quarter and whole, in eighths.
pub fn level(level: f32) -> f32 {
    ((level / STEP).round() * STEP).clamp(LEAST, 1.)
}

impl State {
    /// Alt+wheel over a window: down draws less of it, up more. Returns whether the wheel was
    /// taken.
    pub fn dither_wheel(&mut self, v120: f64) -> bool {
        if !WHEEL.load(Ordering::Relaxed) || v120 == 0. {
            return false;
        }
        let pointer = self.niri.seat.get_pointer().unwrap();
        if pointer.is_grabbed() || self.niri.layout.is_overview_open() {
            return false;
        }
        let under = self.niri.contents_under(pointer.current_location());
        let Some((window, _)) = under.window else {
            return false;
        };
        let mut fullscreen = true;
        let mut now = 1.;
        self.niri.layout.with_windows(|mapped, _, _, _| {
            if mapped.window == window {
                fullscreen = mapped.sizing_mode().is_fullscreen();
                now = mapped.dither();
            }
        });
        if fullscreen {
            return false;
        }

        let turned = TURNED.get() + v120;
        let notches = (turned / 120.).trunc();
        TURNED.set(turned - notches * 120.);
        if notches != 0. {
            // a negative amount is the wheel turned up: more of the window
            let to = level(now - notches as f32 * STEP);
            self.niri.layout.with_windows_mut(|mapped, _| {
                if mapped.window == window {
                    mapped.set_dither(to);
                }
            });
            self.niri.queue_redraw_all();
        }
        true
    }

    /// `set-window-dither`: this window (the focused one if none) drawn at `level`.
    pub fn set_window_dither(&mut self, id: Option<u64>, level: f32) {
        let focused = self.niri.layout.focus().map(|m| m.id().get());
        let Some(id) = id.or(focused) else {
            return;
        };
        self.niri.layout.with_windows_mut(|mapped, _| {
            if mapped.id().get() == id {
                mapped.set_dither(self::level(level));
            }
        });
        self.niri.queue_redraw_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_eighths_from_a_quarter() {
        assert_eq!(level(1.), 1.);
        assert_eq!(level(0.6), 0.625);
        assert_eq!(level(0.), LEAST);
        assert_eq!(level(3.), 1.);
        assert_eq!(level(1. - 3. * STEP), 0.625);
    }
}
