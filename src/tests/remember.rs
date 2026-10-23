use super::*;
use crate::remember::Spot;
use crate::tests::client::ClientId;

fn pin(f: &mut Fixture, floating: bool) {
    f.niri().set_remember_windows(true);
    f.niri().set_app_place(
        String::from("remember.test"),
        Some(Spot {
            output: String::from("headless-2"),
            workspace: None,
            workspace_idx: 0,
            floating,
        }),
    );
}

fn open(f: &mut Fixture, id: ClientId) {
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.set_app_id("remember.test");
    window.commit();
    f.roundtrip(id);
    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(300, 200);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
}

// the outputs of the open windows, sorted
fn outputs(f: &mut Fixture) -> Vec<String> {
    let mut out: Vec<_> = (f.niri().layout.windows())
        .filter_map(|(mon, _)| mon.map(|m| m.output().name()))
        .collect();
    out.sort();
    out
}

// angel-niri (remember.rs): a pinned program's first window opens on its output, floating as
// pinned, and the focus goes there; nothing is remembered on its own.
#[test]
fn pinned_program_opens_on_its_place() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1280, 720));
    pin(&mut f, true);
    f.niri().is_at_startup = false;
    f.niri_focus_output(1);

    let id = f.add_client();
    open(&mut f, id);
    assert_eq!(outputs(&mut f), ["headless-2"]);
    let (_, m) = f.niri().layout.windows().next().unwrap();
    assert!(m.is_floating(), "not floating as pinned");
    let focused = f.niri().layout.active_output().map(|o| o.name());
    assert_eq!(focused.as_deref(), Some("headless-2"), "the focus stayed");

    // a second window of the program: as always (the focused output)
    f.niri_focus_output(1);
    open(&mut f, id);
    assert_eq!(outputs(&mut f), ["headless-1", "headless-2"]);
    assert_eq!(f.niri().remember.pins.len(), 1);
}

// turned off, the pin is kept but not used
#[test]
fn pins_off_open_as_always() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1280, 720));
    pin(&mut f, false);
    f.niri().set_remember_windows(false);
    f.niri_focus_output(1);

    let id = f.add_client();
    open(&mut f, id);
    assert_eq!(outputs(&mut f), ["headless-1"]);
    assert!(f.niri().remember.pins.contains_key("remember.test"));

    // forgotten
    f.niri().set_app_place(String::from("remember.test"), None);
    assert!(f.niri().remember.pins.is_empty());
}

// on another workspace of the same output: the view goes there with it
#[test]
fn pinned_workspace_is_shown() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.niri().is_at_startup = false;
    let id = f.add_client();
    // something on the first workspace, so that a second (empty) one is there
    let window = f.client(id).create_window();
    let other = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let window = f.client(id).window(&other);
    window.attach_new_buffer();
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    f.niri().set_remember_windows(true);
    f.niri().set_app_place(
        String::from("remember.test"),
        Some(Spot {
            output: String::from("headless-1"),
            workspace: None,
            workspace_idx: 1,
            floating: false,
        }),
    );
    open(&mut f, id);
    let active = |f: &mut Fixture| {
        let (_, idx, _) = (f.niri().layout.workspaces())
            .find(|(mon, idx, _)| mon.is_some_and(|m| m.active_workspace_idx() == *idx))
            .unwrap();
        idx
    };
    assert_eq!(active(&mut f), 1, "the view stayed on the first workspace");
}
