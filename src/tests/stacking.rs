use smithay::desktop::Window;
use smithay::utils::Point;
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;
use crate::layout::LayoutElement as _;
use crate::tests::client::ClientId;

fn open(f: &mut Fixture, id: ClientId, w: u16, h: u16) -> Window {
    open_surface(f, id, w, h).0
}

fn open_surface(f: &mut Fixture, id: ClientId, w: u16, h: u16) -> (Window, WlSurface) {
    let before: Vec<_> = f
        .niri()
        .layout
        .windows()
        .map(|(_, m)| m.window.clone())
        .collect();
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(w, h);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    let mut windows = f.niri().layout.windows().map(|(_, m)| m.window.clone());
    (windows.find(|win| !before.contains(win)).unwrap(), surface)
}

// the windows of the active workspace's columns, left to right
fn columns(f: &mut Fixture) -> Vec<Vec<Window>> {
    let ws = f.niri().layout.active_workspace().unwrap();
    ws.scrolling()
        .columns()
        .map(|col| col.stacking_record().0)
        .collect()
}

fn floating(f: &mut Fixture) -> Vec<Window> {
    let ws = f.niri().layout.active_workspace().unwrap();
    ws.floating()
        .tiles()
        .map(|tile| tile.window().window.clone())
        .collect()
}

fn set_stacking(f: &mut Fixture, id: ClientId, on: bool) {
    f.niri().layout.set_stacking(on);
    f.double_roundtrip(id);
}

// on, the columns float where they are on screen; off, they come back in the same order
#[test]
fn stacking_floats_columns_and_puts_them_back() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let a = open(&mut f, id, 100, 100);
    let b = open(&mut f, id, 100, 100);
    let c = open(&mut f, id, 100, 100);
    let before = columns(&mut f);
    assert_eq!(before, [vec![a], vec![b], vec![c.clone()]]);

    let ws = f.niri().layout.active_workspace().unwrap();
    let on_screen: Vec<_> = ws
        .tiles_with_render_positions()
        .filter(|(_, _, visible)| *visible)
        .map(|(tile, pos, _)| (tile.window().window.clone(), pos))
        .collect();

    set_stacking(&mut f, id, true);
    assert!(columns(&mut f).is_empty(), "columns left");
    assert_eq!(floating(&mut f).len(), 3);
    assert_eq!(floating(&mut f)[0], c, "the active window is not on top");
    let ws = f.niri().layout.active_workspace().unwrap();
    for (tile, pos) in ws.floating().tiles_with_render_positions() {
        if let Some((_, was)) = on_screen.iter().find(|(id, _)| *id == tile.window().window) {
            assert_eq!(pos, *was, "a visible window moved");
        }
    }

    set_stacking(&mut f, id, false);
    assert_eq!(columns(&mut f), before);
    assert!(floating(&mut f).is_empty());
}

// columns scrolled off screen come in whole, each in its own place, not stuck at the edge
#[test]
fn stacking_scrolled_away_columns_cascade() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    for _ in 0..5 {
        open(&mut f, id, 900, 500);
    }
    f.niri_complete_animations();
    let ws = f.niri().layout.active_workspace().unwrap();
    let off_screen = ws
        .scrolling()
        .tiles_with_render_positions()
        .filter(|(tile, pos, _)| pos.x + tile.tile_size().w <= 0. || pos.x >= 1920.)
        .count();
    assert!(off_screen >= 2, "nothing scrolled away");

    set_stacking(&mut f, id, true);
    f.niri_complete_animations();
    let ws = f.niri().layout.active_workspace().unwrap();
    let mut seen = Vec::new();
    for (tile, pos) in ws.floating().tiles_with_render_positions() {
        assert!(!seen.contains(&pos), "two windows at {pos:?}");
        seen.push(pos);
        let right = pos.x + tile.tile_size().w;
        assert!(
            pos.x >= 0. && right <= 1920.,
            "a window half off screen at {pos:?}"
        );
    }
}

// a window opened while stacking floats, and joins the columns at the end; one that floated
// before stays floating
#[test]
fn stacking_new_windows_join_at_the_end() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let a = open(&mut f, id, 100, 100);
    let b = open(&mut f, id, 100, 100);
    f.niri().layout.toggle_window_floating(Some(&b));
    f.niri().layout.focus_tiling();

    set_stacking(&mut f, id, true);
    let c = open(&mut f, id, 300, 200);
    let d = open(&mut f, id, 300, 200);
    assert!(columns(&mut f).is_empty());
    let ws = f.niri().layout.active_workspace().unwrap();
    let pos = |id: &Window| {
        ws.floating()
            .tiles_with_render_positions()
            .find(|(tile, _)| tile.window().window == *id)
            .unwrap()
            .1
    };
    let step = pos(&d) - pos(&c);
    assert_eq!(
        (step.x, step.y),
        (crate::stacking::CASCADE, crate::stacking::CASCADE),
        "no step between new windows"
    );

    set_stacking(&mut f, id, false);
    assert_eq!(columns(&mut f), [vec![a], vec![c], vec![d]]);
    assert_eq!(floating(&mut f), [b]);
}

// maximized while stacking (the client asks), a window fills the working area above the others
// and stays floating; asked again (its client never hears it is maximized), it gets its place
// back
#[test]
fn stacking_maximize_stays_floating() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    set_stacking(&mut f, id, true);
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(300, 200);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    let pos = |f: &mut Fixture| {
        let ws = f.niri().layout.active_workspace().unwrap();
        let (tile, pos) = ws.floating().tiles_with_offsets().next().unwrap();
        (pos, ws.floating().snap(&tile.window().window))
    };
    let before = pos(&mut f);
    let _ = f.client(id).window(&surface).format_recent_configures();

    f.client(id).window(&surface).set_maximized();
    f.double_roundtrip(id);
    let window = f.client(id).window(&surface);
    let configures = window.format_recent_configures();
    assert!(configures.contains("size: 1920 × 1080"), "{configures}");
    window.set_size(1920, 1080);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    let (at, snap) = pos(&mut f);
    assert_eq!((at.x, at.y), (0., 0.));
    assert_eq!(snap, Some(crate::stacking::Snap::Maximized));
    assert!(columns(&mut f).is_empty(), "maximized into the columns");

    f.client(id).window(&surface).set_maximized();
    f.double_roundtrip(id);
    let configures = f.client(id).window(&surface).format_recent_configures();
    assert!(configures.contains("size: 300 × 200"), "{configures}");
    assert_eq!(pos(&mut f), before);
}

// dragged to the left edge, a window snaps to the left half; dragged off it, it gets its size
// back
#[test]
fn stacking_drag_to_edge_snaps() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    set_stacking(&mut f, id, true);
    let (a, surface) = open_surface(&mut f, id, 300, 200);
    let output = f.niri_output(1);
    let snap = |f: &mut Fixture| {
        let ws = f.niri().layout.active_workspace().unwrap();
        let (tile, pos) = ws.floating().tiles_with_offsets().next().unwrap();
        (
            ws.floating().snap(&tile.window().window),
            pos,
            tile.window().requested_size(),
        )
    };

    let layout = &mut f.niri().layout;
    layout.interactive_move_begin(a.clone(), &output, Point::from((960., 540.)));
    // the first motion starts the move, the next ones move it
    layout.interactive_move_update(
        &a,
        Point::from((-950., 0.)),
        output.clone(),
        (10., 540.).into(),
    );
    layout.interactive_move_update(
        &a,
        Point::from((-8., 0.)),
        output.clone(),
        (2., 540.).into(),
    );
    layout.interactive_move_end(&a);
    f.double_roundtrip(id);
    let (at, pos, size) = snap(&mut f);
    assert_eq!(at, Some(crate::stacking::Snap::Left));
    assert_eq!((pos.x, pos.y), (0., 0.));
    assert_eq!(size.map(|s| (s.w, s.h)), Some((960, 1080)));
    let window = f.client(id).window(&surface);
    window.set_size(960, 1080);
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    let layout = &mut f.niri().layout;
    layout.interactive_move_begin(a.clone(), &output, Point::from((100., 100.)));
    layout.interactive_move_update(
        &a,
        Point::from((600., 300.)),
        output.clone(),
        (700., 400.).into(),
    );
    layout.interactive_move_update(&a, Point::from((100., 0.)), output, (800., 400.).into());
    layout.interactive_move_end(&a);
    f.double_roundtrip(id);
    let (at, _, size) = snap(&mut f);
    assert_eq!(at, None);
    assert_eq!(size.map(|s| (s.w, s.h)), Some((300, 200)));
}

// a window minimized from its column before stacking comes back into that column after it
#[test]
fn stacking_keeps_the_column_of_a_minimized_window() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let a = open(&mut f, id, 100, 100);
    let b = open(&mut f, id, 100, 100);
    let c = open(&mut f, id, 100, 100);
    assert!(f.niri().layout.minimize_window(&a));

    set_stacking(&mut f, id, true);
    set_stacking(&mut f, id, false);
    assert!(f.niri().layout.unminimize_window(&a, false, None));
    f.double_roundtrip(id);
    assert_eq!(columns(&mut f), [vec![a], vec![b], vec![c]]);
}
