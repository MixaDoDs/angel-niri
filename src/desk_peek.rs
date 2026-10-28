//! angel-niri: «Show Desktop» as macOS does it. `niri msg action show-desktop [--output O]`: the
//! windows of the output's workspace part to the nearer side edge in one motion, a strip of each
//! left in sight; again — they come back. A click on a window (its strip, a taskbar, Alt-Tab), a
//! window opened or closed there, another workspace or another window focused bring them back too.
//! Meanwhile no window has the keyboard (`Monitor::is_desk_peek`). The windows only move as drawn
//! (`Tile::render_offset`): their places in the layout stay as they were.
//!
//! Also `set-hide-windows-layer --namespace NS [--on false]`: while a layer surface of that
//! namespace is mapped on an output, the output's windows are not drawn — a shell's Mission
//! Control shows the desktop under its cards. Nothing goes to niri's config.

use std::sync::Mutex;

use crate::animation::{Animation, Clock, Curve};
use crate::layout::workspace::WorkspaceId;

/// How much of a window stays in sight at the edge, logical px.
pub const STRIP: f64 = 32.;
/// Out at once and settling (the windows make way), back a little quicker.
const OUT_MS: u64 = 380;
const BACK_MS: u64 = 300;

/// How far across a window at `x`, `w` wide, goes in a view `width` wide: to the nearer side, its
/// strip left in sight; one already off that side stays.
pub fn shift(x: f64, w: f64, width: f64) -> f64 {
    if x + w / 2. < width / 2. {
        (STRIP - (x + w)).min(0.)
    } else {
        (width - STRIP - x).max(0.)
    }
}

/// The parting's progress, 0 in place .. 1 at the edge, from `from` to `to`.
pub fn animation(clock: Clock, from: f64, to: f64) -> Animation {
    let ms = if to > from { OUT_MS } else { BACK_MS };
    Animation::ease(clock, from, to, 0., ms, Curve::EaseOutCubic)
}

/// What the output had as its windows parted: once anything of it changes, they come back.
#[derive(Debug, Clone)]
pub struct Peek<Id> {
    pub workspace: WorkspaceId,
    pub windows: Vec<Id>,
    pub focus: Option<Id>,
}

impl<Id: PartialEq> Peek<Id> {
    pub fn still(&self, workspace: WorkspaceId, windows: &[Id], focus: Option<&Id>) -> bool {
        self.workspace == workspace
            && self.windows.len() == windows.len()
            && windows.iter().all(|w| self.windows.contains(w))
            && self.focus.as_ref() == focus
    }
}

static HIDE_NAMESPACES: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn set_hide_layer(namespace: String, on: bool) {
    let mut namespaces = HIDE_NAMESPACES.lock().unwrap();
    namespaces.retain(|ns| *ns != namespace);
    if on {
        namespaces.push(namespace);
    }
}

/// Whether a mapped layer surface of this namespace hides its output's windows.
pub fn hides_windows(namespace: &str) -> bool {
    HIDE_NAMESPACES
        .lock()
        .unwrap()
        .iter()
        .any(|ns| ns == namespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_to_the_nearer_side() {
        // left half: its right edge comes to the strip
        assert_eq!(shift(100., 600., 2000.), STRIP - 700.);
        // right half: its left edge comes to width − strip
        assert_eq!(shift(1200., 600., 2000.), 2000. - STRIP - 1200.);
        // already off the side: stays
        assert_eq!(shift(-900., 600., 2000.), 0.);
        assert_eq!(shift(1990., 600., 2000.), 0.);
    }
}
