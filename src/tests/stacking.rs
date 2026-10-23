use super::*;
use crate::tests::client::ClientId;
use smithay::desktop::Window;

fn open(f: &mut Fixture, id: ClientId, w: u16, h: u16) -> Window {
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
    windows.find(|win| !before.contains(win)).unwrap()
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
