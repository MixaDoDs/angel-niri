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

use std::cell::{Cell, RefCell};
use std::time::Duration;

use niri_config::Color;
use niri_ipc::Event;
use smithay::backend::input::MouseButton;
use smithay::backend::renderer::element::Kind;
use smithay::desktop::Window;
use smithay::input::pointer::{
    CursorIcon, CursorImageStatus, Focus, GrabStartData as PointerGrabStartData,
};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::utils::{Logical, Point, Rectangle, Serial, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::ToplevelSurface;

use crate::input::move_grab::MoveGrab;
use crate::input::resize_grab::ResizeGrab;
use crate::input::{AnyStartData, DOUBLE_CLICK_TIME};
use crate::niri::State;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::stacking_title::FramePart;
use crate::utils::get_monotonic_time;

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

/// An arrow key with Mod.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrow {
    Left,
    Right,
    Up,
    Down,
}

/// What Mod+arrow does to a window snapped to `snap` (or not), like Windows' Win+arrows: to a
/// side, half the screen (back from the other side); up, a half becomes a quarter, a quarter a
/// half, a free window maximizes; down the other way, a free window minimizes (`None`).
pub fn arrow(snap: Option<Snap>, arrow: Arrow) -> Option<Option<Snap>> {
    use Snap::*;
    let to = match (arrow, snap) {
        (Arrow::Left, Some(Right | TopRight | BottomRight)) => None,
        (Arrow::Left, Some(Left | TopLeft | BottomLeft)) => snap,
        (Arrow::Left, _) => Some(Left),
        (Arrow::Right, Some(Left | TopLeft | BottomLeft)) => None,
        (Arrow::Right, Some(Right | TopRight | BottomRight)) => snap,
        (Arrow::Right, _) => Some(Right),
        (Arrow::Up, Some(Left)) => Some(TopLeft),
        (Arrow::Up, Some(Right)) => Some(TopRight),
        (Arrow::Up, Some(BottomLeft)) => Some(Left),
        (Arrow::Up, Some(BottomRight)) => Some(Right),
        (Arrow::Up, None) => Some(Maximized),
        (Arrow::Up, _) => snap,
        (Arrow::Down, Some(Maximized)) => None,
        (Arrow::Down, Some(Left)) => Some(BottomLeft),
        (Arrow::Down, Some(Right)) => Some(BottomRight),
        (Arrow::Down, Some(TopLeft)) => Some(Left),
        (Arrow::Down, Some(TopRight)) => Some(Right),
        (Arrow::Down, None) => return None,
        (Arrow::Down, _) => snap,
    };
    Some(to)
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

/// A shake: this many turns, each after this much travel, within this time.
const SHAKE_TURNS: usize = 4;
const SHAKE_TRAVEL: f64 = 40.;
const SHAKE_TIME: Duration = Duration::from_millis(800);

/// Spots a window shaken left and right while dragged.
#[derive(Debug, Default)]
pub struct Shake {
    dir: f64,
    travel: f64,
    back: f64,
    turns: Vec<Duration>,
    done: bool,
}

impl Shake {
    /// The pointer moved by `dx` at `now`; true once, when that made a shake.
    pub fn feed(&mut self, dx: f64, now: Duration) -> bool {
        if self.done || dx.abs() < 0.5 {
            return false;
        }
        if dx.signum() == self.dir {
            self.travel += dx.abs();
            self.back = 0.;
            return false;
        }
        // A few pixels back are a shaky hand, not a turn.
        self.back += dx.abs();
        if self.back < SHAKE_TRAVEL {
            return false;
        }
        if self.travel >= SHAKE_TRAVEL {
            self.turns.push(now);
        }
        self.dir = dx.signum();
        self.travel = self.back;
        self.back = 0.;
        self.turns
            .retain(|turn| now.saturating_sub(*turn) <= SHAKE_TIME);
        if self.turns.len() >= SHAKE_TURNS {
            self.done = true;
        }
        self.done
    }
}

thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
    /// Whether the pointer shows a resize arrow over a window's edge.
    static EDGE_CURSOR: Cell<bool> = const { Cell::new(false) };
    /// The window shaken last, and the ones that shake minimized.
    static SHAKEN: RefCell<Option<(Window, Vec<Window>)>> = const { RefCell::new(None) };
}

/// Whether the stacking mode is on.
pub fn on() -> bool {
    ON.with(Cell::get)
}

pub(crate) fn set_flag(on: bool) {
    ON.with(|cell| cell.set(on));
}

/// The decoration mode a client asked for, kept while stacking overrides it.
struct WantedDecoration(Cell<Option<DecorationMode>>);

pub fn remember_decoration(toplevel: &ToplevelSurface, mode: DecorationMode) {
    with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .insert_if_missing(|| WantedDecoration(Cell::new(None)));
        states
            .data_map
            .get::<WantedDecoration>()
            .unwrap()
            .0
            .set(Some(mode));
    });
}

/// Apps that draw nothing of their own when they go without server side decorations, so would sit
/// bare while stacking: kitty with `hide_window_decorations` (the owner's). Every other app gets
/// what it asked for: one that draws its own bar (Discord, Helium, frameless Electron) keeps it,
/// with no second set of buttons from niri (the owner's call, 25.10).
// shortcut: a fixed list, nothing tells such an app apart over the protocol; a setting when a
// second app needs it.
const BARE_APPS: &[&str] = &["kitty"];

/// Whether stacking gives the window niri's title bar even though it asked to go without.
pub fn forces_title(toplevel: &ToplevelSurface) -> bool {
    crate::utils::with_toplevel_role(toplevel, |role| {
        role.app_id
            .as_deref()
            .is_some_and(|id| BARE_APPS.contains(&id))
    })
}

fn wanted_decoration(toplevel: &ToplevelSurface) -> Option<DecorationMode> {
    with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<WantedDecoration>()
            .and_then(|wanted| wanted.0.get())
    })
}

impl State {
    /// While stacking, the mapped windows of `BARE_APPS` get server side decorations (so a pixel
    /// title bar) even though they asked for their own; every other window has what it asked for,
    /// and out of stacking all do. Only mapped windows: forcing a mode on SDL2 while it creates
    /// its window leaves the window hidden for good (see `request_mode`).
    pub fn stacking_decorations(&mut self) {
        let on = on();
        let mut changed = Vec::new();
        for (_, mapped) in self.niri.layout.windows() {
            let toplevel = mapped.toplevel();
            let want = if on && forces_title(toplevel) {
                DecorationMode::ServerSide
            } else {
                wanted_decoration(toplevel).unwrap_or(DecorationMode::ServerSide)
            };
            let differs = toplevel.with_pending_state(|state| {
                // None: the client does not speak xdg-decoration (or niri hid it).
                let differs = state.decoration_mode.is_some_and(|mode| mode != want);
                if differs {
                    state.decoration_mode = Some(want);
                }
                differs
            });
            if differs {
                changed.push(mapped.window.clone());
            }
        }
        for window in changed {
            if let Some((mapped, _)) = self
                .niri
                .layout
                .find_window_and_output_mut(window.toplevel().unwrap().wl_surface())
            {
                mapped.set_needs_configure();
            }
        }
    }

    /// A press on a titled window's frame while stacking: dragging the title bar moves the
    /// window, double-clicking it maximizes, a button is done when let go over it, an edge
    /// resizes; a right click on the title bar asks the shell for the window menu. Returns
    /// whether the press was on a frame.
    pub fn stacking_frame_press(
        &mut self,
        button: Option<MouseButton>,
        button_code: u32,
        serial: Serial,
    ) -> bool {
        let pointer = self.niri.seat.get_pointer().unwrap();
        if pointer.is_grabbed() || self.niri.layout.is_overview_open() {
            return false;
        }
        let location = pointer.current_location();
        let Some((output, pos)) = self.niri.output_under(location) else {
            return false;
        };
        let output = output.clone();
        let Some((window, part)) = self.niri.layout.frame_part_under(&output, pos) else {
            return false;
        };
        let Some((_, mapped)) = self.niri.layout.windows().find(|(_, m)| m.window == window) else {
            return false;
        };
        let start_data = || {
            AnyStartData::Pointer(PointerGrabStartData {
                focus: None,
                button: button_code,
                location,
            })
        };

        match (button, part) {
            (Some(MouseButton::Left), FramePart::Title) => {
                let now = get_monotonic_time();
                let last = mapped.last_title_click().replace(Some(now));
                if last.is_some_and(|last| now.saturating_sub(last) <= DOUBLE_CLICK_TIME) {
                    mapped.last_title_click().set(None);
                    self.niri.layout.activate_window(&window);
                    if !self.unshade(&window) {
                        self.niri.layout.toggle_maximized(&window);
                    }
                } else if let Some(grab) = MoveGrab::new(
                    self,
                    start_data(),
                    window,
                    false,
                    Some(CursorIcon::Grabbing),
                ) {
                    pointer.set_grab(self, grab, serial, Focus::Clear);
                }
            }
            (Some(MouseButton::Left), FramePart::Edge(edges)) => {
                self.niri.layout.activate_window(&window);
                if self
                    .niri
                    .layout
                    .interactive_resize_begin(window.clone(), edges)
                {
                    let grab = ResizeGrab::new(start_data(), window);
                    pointer.set_grab(self, grab, serial, Focus::Clear);
                    self.niri
                        .cursor_manager
                        .set_cursor_image(CursorImageStatus::Named(edges.cursor_icon()));
                }
            }
            (Some(MouseButton::Left), part) => mapped.title_button_down().set(Some(part)),
            (Some(MouseButton::Right), FramePart::Title) => {
                let id = mapped.id().get();
                if let Some(server) = &self.niri.ipc_server {
                    server.send_transient_event(Event::WindowMenuRequested {
                        id,
                        output: output.name(),
                        pos: [pos.x, pos.y],
                    });
                }
            }
            _ => return false,
        }

        self.niri.queue_redraw_all();
        true
    }

    /// The wheel over a titled window's title bar: up shades it, down unrolls it. Returns
    /// whether the wheel was taken.
    pub fn stacking_frame_wheel(&mut self, vertical: f64) -> bool {
        if !on() || vertical == 0. {
            return false;
        }
        let pointer = self.niri.seat.get_pointer().unwrap();
        if pointer.is_grabbed() || self.niri.layout.is_overview_open() {
            return false;
        }
        let Some((output, pos)) = self.niri.output_under(pointer.current_location()) else {
            return false;
        };
        let output = output.clone();
        let Some((window, part)) = self.niri.layout.frame_part_under(&output, pos) else {
            return false;
        };
        // the title bar itself: not its buttons, badges or edges
        if part != FramePart::Title {
            return false;
        }
        let id = (self.niri.layout.windows())
            .find(|(_, m)| m.window == window)
            .map(|(_, m)| m.id().get());
        // a negative amount is the wheel turned up
        self.set_window_flags(id, Some(vertical < 0.), None, None);
        true
    }

    /// Mod+arrow while stacking: snaps the focused floating window (see `arrow()`). Returns
    /// whether it did something, so the key does not also move the focus.
    pub fn stacking_arrow(&mut self, key: Arrow) -> bool {
        if !on() {
            return false;
        }
        let Some(window) = self.niri.layout.focus().map(|m| m.window.clone()) else {
            return false;
        };
        let Some(snap) = self.niri.layout.floating_snap(&window) else {
            return false;
        };
        match arrow(snap, key) {
            Some(to) => self.niri.layout.set_floating_snap(&window, to),
            None => self.request_minimize(&window),
        }
        self.niri.queue_redraw_all();
        true
    }

    /// The pointer moved: over a titled window's edge it shows the resize arrow for it.
    pub fn stacking_frame_hover(&mut self) {
        let pointer = self.niri.seat.get_pointer().unwrap();
        if pointer.is_grabbed() {
            return;
        }
        let under = on()
            .then(|| {
                let (output, pos) = self.niri.output_under(pointer.current_location())?;
                let output = output.clone();
                self.niri.layout.frame_part_under(&output, pos)
            })
            .flatten();
        let edges = match under {
            Some((_, FramePart::Edge(edges))) => Some(edges),
            _ => None,
        };

        // the buttons show their glyphs while the pointer is over any of them
        let hovered = match &under {
            Some((window, FramePart::Close | FramePart::Minimize | FramePart::Maximize)) => {
                Some(window)
            }
            _ => None,
        };
        let mut changed = false;
        for (_, mapped) in self.niri.layout.windows() {
            changed |= mapped.set_title_hover(hovered == Some(&mapped.window));
        }
        if changed {
            self.niri.queue_redraw_all();
        }

        let cursor = &mut self.niri.cursor_manager;
        match edges {
            Some(edges) => {
                cursor.set_cursor_image(CursorImageStatus::Named(edges.cursor_icon()));
                EDGE_CURSOR.with(|cell| cell.set(true));
            }
            None if EDGE_CURSOR.with(|cell| cell.replace(false)) => {
                cursor.set_cursor_image(CursorImageStatus::default_named());
            }
            None => (),
        }
    }

    /// The left button let go: a title bar button held down on some window is done if the
    /// pointer is still over it.
    pub fn stacking_frame_release(&mut self) {
        let held = self.niri.layout.windows().find_map(|(_, m)| {
            m.title_button_down()
                .take()
                .map(|part| (m.window.clone(), part))
        });
        let Some((window, part)) = held else {
            return;
        };
        let location = self.niri.seat.get_pointer().unwrap().current_location();
        let under = self
            .niri
            .output_under(location)
            .map(|(output, pos)| (output.clone(), pos))
            .and_then(|(output, pos)| self.niri.layout.frame_part_under(&output, pos));
        if under != Some((window.clone(), part)) {
            return;
        }

        match part {
            FramePart::Minimize => self.request_minimize(&window),
            FramePart::Maximize => {
                self.niri.layout.activate_window(&window);
                if !self.unshade(&window) {
                    self.niri.layout.toggle_maximized(&window);
                }
            }
            FramePart::OnTop | FramePart::Sticky => {
                let id = (self.niri.layout.windows())
                    .find(|(_, m)| m.window == window)
                    .map(|(_, m)| m.id().get());
                if part == FramePart::OnTop {
                    self.set_window_flags(id, None, Some(false), None);
                } else {
                    self.set_window_flags(id, None, None, Some(false));
                }
            }
            FramePart::Close => {
                if let Some((_, mapped)) =
                    self.niri.layout.windows().find(|(_, m)| m.window == window)
                {
                    mapped.toplevel().send_close();
                }
            }
            FramePart::Title | FramePart::Edge(_) => (),
        }
        self.niri.queue_redraw_all();
    }

    /// A window was shaken while dragged: the others on its workspace minimize, like Windows 7's
    /// Aero Shake; shaken again, they come back.
    pub fn stacking_shake(&mut self, window: &Window, others: Vec<Window>) {
        let again = SHAKEN.with(|cell| {
            let mut shaken = cell.borrow_mut();
            match shaken.take() {
                Some((last, minimized)) if &last == window => Some(minimized),
                _ => {
                    *shaken = Some((window.clone(), others.clone()));
                    None
                }
            }
        });

        match again {
            Some(minimized) => {
                for other in &minimized {
                    // Shaken back before the shell took it: it is not minimized yet, and won't be.
                    if let Some((_, mapped)) =
                        self.niri.layout.windows().find(|(_, m)| &m.window == other)
                    {
                        let id = mapped.id().get();
                        self.niri.minimize_pending.remove(&id);
                    }
                    self.unminimize_window(other, false, None);
                }
            }
            None => {
                for other in &others {
                    self.request_minimize(other);
                }
            }
        }
        self.niri.queue_redraw_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rectangle<f64, Logical> {
        Rectangle::new(Point::from((0., 40.)), Size::from((1920., 1040.)))
    }

    #[test]
    fn snap_rects() {
        let rect = |snap: Snap| {
            let r = snap.rect(area());
            (r.loc.x, r.loc.y, r.size.w, r.size.h)
        };
        assert_eq!(rect(Snap::Maximized), (0., 40., 1920., 1040.));
        assert_eq!(rect(Snap::Left), (0., 40., 960., 1040.));
        assert_eq!(rect(Snap::Right), (960., 40., 960., 1040.));
        assert_eq!(rect(Snap::BottomRight), (960., 560., 960., 520.));
    }

    #[test]
    fn snap_zones() {
        let size = Size::from((1920., 1080.));
        let at = |x: f64, y: f64| snap_at(Point::from((x, y)), size);
        assert_eq!(at(2., 540.), Some(Snap::Left));
        assert_eq!(at(1919., 540.), Some(Snap::Right));
        assert_eq!(at(2., 100.), Some(Snap::TopLeft));
        assert_eq!(at(1919., 1000.), Some(Snap::BottomRight));
        assert_eq!(at(960., 2.), Some(Snap::Maximized));
        assert_eq!(at(960., 540.), None);
    }

    #[test]
    fn arrows_like_windows() {
        use Snap::*;
        assert_eq!(arrow(None, Arrow::Left), Some(Some(Left)));
        assert_eq!(arrow(Some(Right), Arrow::Left), Some(None));
        assert_eq!(arrow(Some(Left), Arrow::Up), Some(Some(TopLeft)));
        assert_eq!(arrow(Some(TopLeft), Arrow::Down), Some(Some(Left)));
        assert_eq!(arrow(None, Arrow::Up), Some(Some(Maximized)));
        assert_eq!(arrow(Some(Maximized), Arrow::Down), Some(None));
        assert_eq!(arrow(None, Arrow::Down), None);
    }

    #[test]
    fn shake_needs_four_wide_turns() {
        let mut shake = Shake::default();
        let ms = Duration::from_millis;
        // a shaky hand: small wiggles are no turns
        for i in 0..20 {
            assert!(!shake.feed(if i % 2 == 0 { 3. } else { -3. }, ms(i * 10)));
        }
        let mut t = 300;
        let mut shaken = false;
        for i in 0..6 {
            let dx = if i % 2 == 0 { 60. } else { -60. };
            shaken |= shake.feed(dx, ms(t));
            t += 100;
        }
        assert!(shaken);
        // once per drag
        assert!(!shake.feed(-60., ms(t)));
    }
}
