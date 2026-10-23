//! angel-niri: the stacking mode — windows as in a usual desktop.
//!
//! `set-stacking-mode` floats every column on every workspace, each window where it is on
//! screen, and new windows open floating too; turned off, the columns come back as they were
//! (their order, width and workspace) and the windows opened meanwhile join them at the end.
//! Windows that floated before stay floating. The layout does the moving
//! (`Layout::set_stacking`); this keeps the switch, read wherever floating behaves differently
//! while it is on: new windows step down from one another, focus follows clicks, not the mouse,
//! and a window let go of mid-drag is not thrown.
//!
//! The switch is per thread: the compositor runs on one, and the layout tests (one thread each)
//! don't see each other's.

use std::cell::Cell;

/// How far a new floating window steps down and right from one already in its place.
pub const CASCADE: f64 = 32.;

thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
}

/// Whether the stacking mode is on.
pub fn on() -> bool {
    ON.with(Cell::get)
}

pub(crate) fn set_flag(on: bool) {
    ON.with(|cell| cell.set(on));
}
