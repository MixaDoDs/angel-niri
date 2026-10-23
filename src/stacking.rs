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

use niri_config::Color;
use smithay::backend::renderer::element::Kind;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};

/// How far a new floating window steps down and right from one already in its place.
pub const CASCADE: f64 = 32.;

/// Where a floating window sits snapped, filling part of the working area; it gets its own size
/// and place back when it leaves (`FloatingSpace::set_snap`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Snap {
    Maximized,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Snap {
    /// The part of `area` it fills.
    pub fn rect(self, area: Rectangle<f64, Logical>) -> Rectangle<f64, Logical> {
        let mut rect = area;
        let (half_w, half_h) = ((area.size.w / 2.).round(), (area.size.h / 2.).round());
        if !matches!(self, Snap::Maximized) {
            rect.size.w = half_w;
        }
        if matches!(self, Snap::Right | Snap::TopRight | Snap::BottomRight) {
            rect.loc.x += half_w;
            rect.size.w = area.size.w - half_w;
        }
        if matches!(
            self,
            Snap::TopLeft | Snap::TopRight | Snap::BottomLeft | Snap::BottomRight
        ) {
            rect.size.h = half_h;
        }
        if matches!(self, Snap::BottomLeft | Snap::BottomRight) {
            rect.loc.y += half_h;
            rect.size.h = area.size.h - half_h;
        }
        rect
    }
}

/// How near an edge of its output the pointer snaps a dragged window there.
const SNAP_EDGE: f64 = 8.;

/// Where a window dragged with the pointer at `pos` (within an output of `size`) snaps when let
/// go: to the left or right edge, a half; into a corner (an edge in its top or bottom quarter), a
/// quarter; to the top edge, the whole working area.
pub fn snap_at(pos: Point<f64, Logical>, size: Size<f64, Logical>) -> Option<Snap> {
    let left = pos.x < SNAP_EDGE;
    let right = pos.x > size.w - SNAP_EDGE;
    let upper = pos.y < size.h / 4.;
    let lower = pos.y > size.h * 3. / 4.;
    Some(match (left, right) {
        (true, _) if upper => Snap::TopLeft,
        (true, _) if lower => Snap::BottomLeft,
        (true, _) => Snap::Left,
        (_, true) if upper => Snap::TopRight,
        (_, true) if lower => Snap::BottomRight,
        (_, true) => Snap::Right,
        _ if pos.y < SNAP_EDGE => Snap::Maximized,
        _ => return None,
    })
}

/// Thickness of the snap frame's border: two art pixels of the pixel skins.
const FRAME_BORDER: f64 = 4.;

/// The pixel frame showing where a dragged window will snap: a see-through fill and a border two
/// art pixels thick, its corners cut like a pixel window's, in the insert hint's colour.
#[derive(Debug, Default)]
pub struct SnapFrame {
    area: Option<Rectangle<f64, Logical>>,
    fill: SolidColorBuffer,
    edges: [SolidColorBuffer; 4],
}

impl SnapFrame {
    /// Shows the frame over `area` (workspace coordinates), or hides it with `None`.
    pub fn update(&mut self, area: Option<Rectangle<f64, Logical>>, color: Color) {
        self.area = area;
        let Some(area) = area else {
            return;
        };
        let b = FRAME_BORDER;
        let (w, h) = (area.size.w, area.size.h);
        let solid = Color { a: 1., ..color };
        let fill = Color {
            a: color.a / 2.,
            ..color
        };
        self.fill
            .update((f64::max(w - 2. * b, 0.), f64::max(h - 2. * b, 0.)), fill);
        let across = (f64::max(w - 2. * b, 0.), b);
        let down = (b, f64::max(h - 2. * b, 0.));
        for (edge, size) in self.edges.iter_mut().zip([across, across, down, down]) {
            edge.update(size, solid);
        }
    }

    pub fn render(&self, push: &mut dyn FnMut(SolidColorRenderElement)) {
        let Some(area) = self.area else {
            return;
        };
        let b = FRAME_BORDER;
        let Point { x, y, .. } = area.loc;
        let (w, h) = (area.size.w, area.size.h);
        let locs = [
            (x + b, y),
            (x + b, y + h - b),
            (x, y + b),
            (x + w - b, y + b),
        ];
        for (edge, loc) in self.edges.iter().zip(locs) {
            push(SolidColorRenderElement::from_buffer(
                edge,
                loc,
                1.,
                Kind::Unspecified,
            ));
        }
        push(SolidColorRenderElement::from_buffer(
            &self.fill,
            (x + b, y + b),
            1.,
            Kind::Unspecified,
        ));
    }
}

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
