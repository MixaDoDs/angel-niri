use niri_config::Action;
use niri_config::WorkspaceReference;
use smithay::backend::input::InputTime;

use super::*;
use crate::tests::client::ClientId;

fn open(f: &mut Fixture, id: ClientId) {
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);
    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(300, 200);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
}

// (number, windows) of each workspace of the focused output, top to bottom
fn desks(f: &mut Fixture) -> Vec<(usize, usize)> {
    let layout = &f.niri().layout;
    let output = layout.active_output().unwrap().clone();
    (layout.workspaces())
        .filter(|(mon, _, _)| mon.is_some_and(|mon| *mon.output() == output))
        .map(|(mon, idx, ws)| (idx + mon.unwrap().desk_base(), ws.windows().count()))
        .collect()
}

fn active_number(f: &mut Fixture) -> usize {
    let mon = f.niri().layout.active_monitor_ref().unwrap();
    mon.active_workspace_idx() + mon.desk_base()
}

fn act(f: &mut Fixture, action: Action) {
    f.niri_state().do_action(action, false, InputTime::now());
    f.niri_complete_animations();
}

// angel-niri (desk_zero.rs): an empty desk 0 above desk 1; a window put on it makes it desk 1, and
// "desk N" keeps meaning the N-th desk with windows.
#[test]
fn clean_desk_on_top_numbered_zero() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    act(&mut f, Action::SetDeskZero(true));
    // nothing open: one empty desk, numbered 1
    assert_eq!(desks(&mut f), [(1, 0)]);

    let id = f.add_client();
    open(&mut f, id);
    assert_eq!(desks(&mut f), [(0, 0), (1, 1), (2, 0)]);
    assert_eq!(active_number(&mut f), 1);

    // up from desk 1: the clean desk 0; a window there makes it desk 1
    act(&mut f, Action::FocusWorkspaceUp);
    assert_eq!(active_number(&mut f), 0);
    open(&mut f, id);
    assert_eq!(desks(&mut f), [(0, 0), (1, 1), (2, 1), (3, 0)]);
    assert_eq!(active_number(&mut f), 1);

    // by number: desk 2 is the older one, desk 0 the clean one
    act(&mut f, Action::FocusWorkspace(WorkspaceReference::Index(2)));
    assert_eq!(active_number(&mut f), 2);
    act(&mut f, Action::FocusWorkspace(WorkspaceReference::Index(0)));
    assert_eq!(active_number(&mut f), 0);

    // off: stock niri again
    act(&mut f, Action::FocusWorkspace(WorkspaceReference::Index(1)));
    act(&mut f, Action::SetDeskZero(false));
    assert_eq!(desks(&mut f), [(1, 1), (2, 1), (3, 0)]);
}

// angel-niri (desk_zero.rs, stacking.rs): Desktop gathers the windows onto desk 1, not onto the
// clean desk 0, and gives them back to their desks after.
#[test]
fn desktop_gathers_onto_desk_one() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    act(&mut f, Action::SetDeskZero(true));
    let id = f.add_client();
    open(&mut f, id);
    act(&mut f, Action::FocusWorkspaceDown);
    open(&mut f, id);
    assert_eq!(desks(&mut f), [(0, 0), (1, 1), (2, 1), (3, 0)]);

    f.niri().layout.set_stacking(true);
    f.niri_complete_animations();
    let gathered = desks(&mut f);
    assert_eq!(gathered[..2], [(0, 0), (1, 2)], "{gathered:?}");
    assert_eq!(active_number(&mut f), 1);

    f.niri().layout.set_stacking(false);
    f.niri_complete_animations();
    assert_eq!(desks(&mut f), [(0, 0), (1, 1), (2, 1), (3, 0)]);
}
