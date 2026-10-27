use smithay::desktop::Window;
use smithay::utils::Point;
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;
use crate::layout::{ActivateWindow, LayoutElement as _};
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

    // the places once settled: the windows hop as the mode switches (Tile::hop)
    f.niri_complete_animations();
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
    f.niri_complete_animations();
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

// in Desktop a window going full screen gets a workspace of its own right after its own, the view
// along; back from full screen it floats on its old workspace again and the other one goes
#[test]
fn stacking_fullscreen_gets_its_own_workspace() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    set_stacking(&mut f, id, true);
    let _a = open(&mut f, id, 300, 200);
    let b = open(&mut f, id, 300, 200);
    let ws_of = |f: &mut Fixture, win: &Window| {
        let layout = &f.niri().layout;
        let active = layout.monitors().next().unwrap().active_workspace_idx();
        let all: Vec<_> = layout
            .workspaces()
            .map(|(_, idx, ws)| (idx, ws.has_window(win)))
            .collect();
        let idx = all.iter().find(|(_, has)| *has).map(|(idx, _)| *idx);
        (idx, active, all.len())
    };
    assert_eq!(ws_of(&mut f, &b), (Some(0), 0, 2));

    f.niri().layout.set_fullscreen(&b, true);
    f.double_roundtrip(id);
    assert_eq!(
        ws_of(&mut f, &b),
        (Some(1), 1, 3),
        "not on a workspace of its own"
    );

    f.niri().layout.set_fullscreen(&b, false);
    f.double_roundtrip(id);
    let (idx, active, _) = ws_of(&mut f, &b);
    assert_eq!((idx, active), (Some(0), 0), "not back where it was");
    assert_eq!(floating(&mut f).len(), 2, "not floating again");
}

// on and off, the windows of the second workspace stay there and the monitor stays on it, as
// Windows keeps its desktops
// on, each monitor's windows gather onto its first workspace, in sight, none onto another
// monitor (the owner, 26.10); off, they go back to their workspaces and columns
#[test]
fn stacking_gathers_desks_onto_the_first() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1920, 1080));
    let id = f.add_client();
    f.niri_focus_output(1);
    let a = open(&mut f, id, 100, 100);
    let b = open(&mut f, id, 100, 100);
    let c = open(&mut f, id, 100, 100);
    for win in [&b, &c] {
        f.niri()
            .layout
            .move_to_workspace(Some(win), 1, ActivateWindow::No);
    }
    f.niri_focus_output(2);
    let d = open(&mut f, id, 100, 100);
    f.niri()
        .layout
        .move_to_workspace(Some(&d), 1, ActivateWindow::No);
    f.niri().layout.switch_workspace(1);
    f.double_roundtrip(id);
    let out = [f.niri_output(1).name(), f.niri_output(2).name()];

    let on = |f: &mut Fixture, win: &Window| {
        let layout = &f.niri().layout;
        let (mon, idx, _) = (layout.workspaces())
            .find(|(_, _, ws)| ws.has_window(win))
            .unwrap();
        let mon = mon.unwrap();
        let n = out.iter().position(|o| o == mon.output_name()).unwrap() + 1;
        (
            n,
            idx,
            mon.active_workspace_idx(),
            f.niri().layout.is_minimized(win),
        )
    };

    set_stacking(&mut f, id, true);
    f.niri_complete_animations();
    f.double_roundtrip(id);
    assert_eq!(on(&mut f, &a), (1, 0, 0, false));
    assert_eq!(on(&mut f, &b), (1, 0, 0, false), "not gathered");
    assert_eq!(on(&mut f, &c), (1, 0, 0, false), "not gathered");
    assert_eq!(
        on(&mut f, &d),
        (2, 0, 0, false),
        "not gathered on its own monitor"
    );
    let desks: Vec<_> = (f.niri().layout.monitors())
        .map(|mon| mon.last_reachable_workspace())
        .collect();
    assert_eq!(desks, [0, 0], "the emptied desks stayed");

    set_stacking(&mut f, id, false);
    assert_eq!(on(&mut f, &a), (1, 0, 0, false));
    assert_eq!(on(&mut f, &b), (1, 1, 0, false), "not back on its desk");
    assert_eq!(on(&mut f, &d), (2, 1, 0, false), "not back on its desk");
    let layout = &f.niri().layout;
    let ws = (layout.workspaces())
        .find(|(_, _, ws)| ws.has_window(&b))
        .unwrap()
        .2;
    let columns: Vec<_> = (ws.scrolling().columns())
        .map(|col| col.stacking_record().0)
        .collect();
    assert_eq!(
        columns,
        [vec![b], vec![c]],
        "the columns came back out of order"
    );
}

// in Desktop the empty workspace at the end is out of reach and hidden; "+" opens it, with a
// window too; back in the Strip every desk keeps its number
#[test]
fn stacking_desks_come_from_plus() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let a = open(&mut f, id, 100, 100);
    let b = open(&mut f, id, 100, 100);
    let d = open(&mut f, id, 100, 100);
    f.niri()
        .layout
        .move_to_workspace(Some(&b), 1, ActivateWindow::No);
    f.double_roundtrip(id);

    set_stacking(&mut f, id, true);
    let active = |f: &mut Fixture| {
        let mon = f.niri().layout.monitors().next().unwrap();
        (mon.active_workspace_idx(), mon.last_reachable_workspace())
    };
    // b came onto the first desk with the rest: one desk, the spare one out of reach
    f.niri_complete_animations();
    f.double_roundtrip(id);
    assert_eq!(active(&mut f), (0, 0), "the spare workspace is reachable");
    f.niri().layout.switch_workspace(2);
    f.niri()
        .layout
        .move_to_workspace(Some(&d), 2, ActivateWindow::No);
    assert_eq!(
        active(&mut f),
        (0, 0),
        "switched or moved onto the spare one"
    );

    f.niri().layout.open_spare_workspace(None, Some(&d));
    f.double_roundtrip(id);
    assert_eq!(
        active(&mut f),
        (1, 1),
        "\"+\" didn't take the window to a new desk"
    );

    set_stacking(&mut f, id, false);
    let layout = &f.niri().layout;
    let on = |win: &Window| {
        (layout.workspaces())
            .find(|(_, _, ws)| ws.has_window(win))
            .map(|(_, idx, _)| idx)
    };
    assert_eq!(
        (on(&a), on(&b), on(&d)),
        (Some(0), Some(1), Some(2)),
        "the desks are out of order"
    );
}

// with the throw on, the gathered windows fly in (the one from desk 3 hits the top), and ending
// stacking waits for them to fly off the bottom before they go back
#[test]
fn stacking_throws_the_gathered_windows() {
    crate::desk_throw::set(true);
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let a = open(&mut f, id, 100, 100);
    let b = open(&mut f, id, 100, 100);
    let c = open(&mut f, id, 100, 100);
    f.niri()
        .layout
        .move_to_workspace(Some(&b), 1, ActivateWindow::No);
    f.niri()
        .layout
        .move_to_workspace(Some(&c), 2, ActivateWindow::No);
    f.double_roundtrip(id);

    let flying = |f: &mut Fixture| -> Vec<Window> {
        let ws = f.niri().layout.active_workspace().unwrap();
        ws.tiles()
            .filter(|tile| tile.fly().is_some())
            .map(|tile| tile.window().window.clone())
            .collect()
    };

    set_stacking(&mut f, id, true);
    let fly = flying(&mut f);
    assert!(fly.len() == 2 && fly.contains(&b) && fly.contains(&c));
    let throws = std::mem::take(&mut f.niri().layout.desk_throws);
    let launch: Vec<_> = throws.iter().filter(|t| t.launch).collect();
    assert_eq!(launch.len(), 2, "every one sets off");
    assert!(
        launch.iter().all(|t| !t.bottom && t.pos[1] < 1.),
        "into the top first"
    );
    let hardest = |far: bool| {
        (throws.iter().filter(|t| t.far == far && !t.slide))
            .map(|t| t.power)
            .fold(0., f64::max)
    };
    assert!(hardest(true) > hardest(false), "the far one hits harder");
    // they stay where they stopped, on the screen
    let before: Vec<_> = (f.niri().layout.active_workspace().unwrap().tiles())
        .map(|t| t.window().id().clone())
        .collect();
    f.niri_complete_animations();
    assert!(flying(&mut f).is_empty());
    let ws = f.niri().layout.active_workspace().unwrap();
    for (tile, pos, _) in ws.tiles_with_render_positions() {
        let size = tile.tile_size();
        assert!(pos.x >= -0.5 && pos.y >= -0.5, "{pos:?}");
        assert!(
            pos.x + size.w <= 1920.5 && pos.y + size.h <= 1080.5,
            "{pos:?}"
        );
    }
    assert_eq!(before.len(), 3);

    let (wait, n) = f.niri().layout.throw_out().unwrap();
    assert!(wait <= std::time::Duration::from_millis(1500));
    assert!(crate::stacking::on(), "ended before they flew off");
    assert_eq!(flying(&mut f).len(), 2);
    let throws = std::mem::take(&mut f.niri().layout.desk_throws);
    assert!(
        throws.len() == 2 && throws.iter().all(|t| t.bottom),
        "leaving they drop, breaking nothing"
    );
    assert!(f.niri().layout.throw_out_pending(n));

    set_stacking(&mut f, id, false);
    assert!(!f.niri().layout.throw_out_pending(n));
    assert!(flying(&mut f).is_empty(), "still flying on their desks");
    let ws = f.niri().layout.active_workspace().unwrap();
    assert!(ws.has_window(&a) && !ws.has_window(&b) && !ws.has_window(&c));
    crate::desk_throw::set(false);
}

// with the throw on, the gathered windows come up from below one at a time: until its turn each
// waits out of sight under the screen, and each sets off and hits the top a pause after the one
// before, the pauses growing 0.3, 0.6, 1 s (a slow one doesn't land with a fast one)
#[test]
fn stacking_throws_one_window_at_a_time() {
    crate::desk_throw::set(true);
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    let _a = open(&mut f, id, 100, 100);
    let mut thrown = Vec::new();
    let mut surfaces = Vec::new();
    for desk in [1, 1, 2, 3] {
        let (win, surface) = open_surface(&mut f, id, 300, 200);
        surfaces.push(surface);
        f.niri()
            .layout
            .move_to_workspace(Some(&win), desk, ActivateWindow::No);
        thrown.push(win);
    }
    f.double_roundtrip(id);

    let set_time = |f: &mut Fixture, ms: u64| {
        let niri = f.niri();
        let now = niri.clock.now();
        niri.clock.set_unadjusted(now);
        let _ = niri.clock.now();
        niri.clock.set_unadjusted(std::time::Duration::ZERO);
        niri.clock.set_rate(1.0);
        let _ = niri.clock.now();
        niri.clock
            .set_unadjusted(std::time::Duration::from_millis(ms));
        let _ = niri.clock.now();
        niri.clock.set_rate(0.0);
    };
    let in_sight = |f: &mut Fixture| -> usize {
        let ws = f.niri().layout.active_workspace().unwrap();
        ws.tiles_with_render_positions()
            .filter(|(tile, _, _)| thrown.contains(&tile.window().window))
            .filter(|(tile, pos, _)| pos.y < 1080. && pos.y + tile.tile_size().h > 0.)
            .count()
    };

    set_time(&mut f, 0);
    set_stacking(&mut f, id, true);
    set_time(&mut f, 0);
    assert_eq!(in_sight(&mut f), 0, "all wait below at first");
    let throws = std::mem::take(&mut f.niri().layout.desk_throws);
    let mut offs: Vec<u64> = throws.iter().map(|t| t.off_ms).collect();
    offs.sort();
    offs.dedup();
    let hits: Vec<u64> = (throws.iter().filter(|t| t.launch))
        .map(|t| t.in_ms)
        .collect();
    assert_eq!(offs.len(), 4, "{offs:?}");
    assert_eq!(hits.len(), 4, "{hits:?}");
    let mut hits_sorted = hits.clone();
    hits_sorted.sort();
    // the pauses grow: 0.3, 0.6, 1 s
    for (i, w) in offs.windows(2).enumerate() {
        let gap = [300, 600, 1000][i];
        assert!(w[1] >= w[0] + gap - 1, "{offs:?}");
        let h = &hits_sorted[i..i + 2];
        assert!(h[1] >= h[0] + gap - 1, "{hits:?}");
    }
    // one more in sight at each one's turn
    for (k, off) in offs.iter().enumerate() {
        set_time(&mut f, off + 40);
        assert_eq!(in_sight(&mut f), k + 1, "{k} at {off}");
    }
    // crushed for real at each hit: the window is asked to be squashed (lower, wider) and then
    // its own size again, every client acking and committing as it goes
    let mut sizes: Vec<Vec<(i32, i32)>> = vec![Vec::new(); surfaces.len()];
    let step = |f: &mut Fixture, sizes: &mut Vec<Vec<(i32, i32)>>, ms: u64| {
        set_time(f, ms);
        f.niri().advance_animations();
        f.niri().layout.refresh(true);
        f.double_roundtrip(id);
        for (k, surface) in surfaces.iter().enumerate() {
            let win = f.client(id).window(surface);
            let new: Vec<_> = win.recent_configures().map(|c| c.size).collect();
            if !new.is_empty() {
                win.ack_last_and_commit();
            }
            sizes[k].extend(new);
        }
        f.double_roundtrip(id);
    };
    for ms in (0..hits_sorted[3] + 3000).step_by(20) {
        step(&mut f, &mut sizes, ms);
    }
    for (k, sizes) in sizes.iter().enumerate() {
        let (w, h) = (300, 200);
        assert!(
            sizes.iter().any(|&(sw, sh)| sh < h && sw > w),
            "{k} never squashed {sizes:?}"
        );
        assert_eq!(*sizes.last().unwrap(), (w, h), "{k} not back {sizes:?}");
    }
    // spread about the screen to be handy: none on top of another, all on the screen
    let ws = f.niri().layout.active_workspace().unwrap();
    let rects: Vec<_> = (ws.tiles_with_render_positions())
        .filter(|(tile, _, _)| thrown.contains(&tile.window().window))
        .map(|(tile, pos, _)| (pos, tile.tile_size()))
        .collect();
    assert_eq!(rects.len(), 4);
    for (i, (a, sa)) in rects.iter().enumerate() {
        assert!(a.x >= -0.5 && a.y >= -0.5 && a.x + sa.w <= 1920.5 && a.y + sa.h <= 1080.5);
        for (b, sb) in &rects[i + 1..] {
            let w = (a.x + sa.w).min(b.x + sb.w) - a.x.max(b.x);
            let h = (a.y + sa.h).min(b.y + sb.h) - a.y.max(b.y);
            assert!(w <= 0.5 || h <= 0.5, "{rects:?}");
        }
    }
    crate::desk_throw::set(false);
}
