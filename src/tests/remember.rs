use super::*;
use crate::layout::LayoutElement as _;

// angel-niri (remember.rs): a program's window opens again on the output where it was closed,
// with the width it had.
#[test]
fn window_opens_where_its_program_was() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    f.add_output(2, (1280, 720));
    f.niri().set_remember_windows(true);
    f.niri_focus_output(2);

    let id = f.add_client();
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

    let on_output = |f: &mut Fixture| {
        let (mon, _) = f.niri().layout.windows().next().unwrap();
        mon.map(|m| m.output().name())
    };
    assert_eq!(on_output(&mut f).as_deref(), Some("headless-2"));
    let width = f.niri().layout.windows().next().unwrap().1.size().w as i32;

    // closed (unmapped) on output 2; the focus goes to output 1
    let window = f.client(id).window(&surface);
    window.attach_null();
    window.commit();
    f.double_roundtrip(id);
    f.niri_focus_output(1);
    let spot = f.niri().remember.apps.get("remember.test").cloned();
    assert_eq!(spot.as_ref().map(|s| s.output.as_str()), Some("headless-2"));
    assert_eq!(spot.and_then(|s| s.tiled_width), Some(f64::from(width)));
    // (a width the output's default would not give)
    let width = 333;
    f.niri()
        .remember
        .apps
        .get_mut("remember.test")
        .unwrap()
        .tiled_width = Some(f64::from(width));

    // opened again (an unmapped toplevel starts afresh, its app-id too): on output 2, as wide
    let window = f.client(id).window(&surface);
    window.set_app_id("remember.test");
    window.commit();
    f.double_roundtrip(id);
    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    assert_eq!(on_output(&mut f).as_deref(), Some("headless-2"));
    let window = f.client(id).window(&surface);
    let last = window.format_recent_configures();
    assert!(
        last.contains(&format!("size: {width} ×")),
        "not configured {width} wide: {last}"
    );
}
