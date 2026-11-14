//! rice: pointer input on compositor-drawn title strips.

use niri_config::Config;
use smithay::utils::{Logical, Point};
use wayland_client::protocol::wl_pointer;

use super::*;
use crate::tests::client::ClientId;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const OUTPUT_SIZE: (u16, u16) = (1920, 1080);

fn set_up(mode: &str) -> (Fixture, ClientId) {
    let config = Config::parse_mem(&format!(
        r#"
        layout {{
            default-workspace-mode "{mode}"
            canvas {{
                default-zoom 0.5
                zoom-steps 0.5 1.0
            }}
            title-bar {{
                height 28
            }}
            border {{
                width 1
            }}
        }}
        animations {{
            off
        }}
        "#
    ))
    .unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, OUTPUT_SIZE);
    let id = f.add_client();
    (f, id)
}

fn open_window(f: &mut Fixture, id: ClientId) -> u64 {
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(400, 300);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    f.niri_complete_animations();

    f.niri().layout.focus().unwrap().id().get()
}

fn focused(f: &mut Fixture) -> u64 {
    f.niri().layout.focus().unwrap().id().get()
}

/// View position of a window's tile on the active workspace, and the canvas zoom.
fn tile_view_pos(f: &mut Fixture, window: u64) -> (Point<f64, Logical>, f64) {
    let ws = f.niri().layout.active_workspace().unwrap();
    let zoom = ws.canvas_zoom();
    let pos = ws
        .tiles_with_render_positions()
        .find(|(tile, _, _)| tile.window().id().get() == window)
        .map(|(_, pos, _)| pos)
        .unwrap();
    (pos.upscale(zoom), zoom)
}

fn plane_pos(f: &mut Fixture, window: u64) -> Point<f64, Logical> {
    let layout = &f.niri().layout;
    let (_, mapped) = layout
        .windows()
        .find(|(_, m)| m.id().get() == window)
        .unwrap();
    let id = mapped.window.clone();
    let ws = layout.active_workspace().unwrap();
    ws.canvas_window_plane_rect(&id).unwrap().loc
}

/// A point on the title strip of `window`, `x` logical px from the tile's left edge.
fn strip_point(f: &mut Fixture, window: u64, x: f64) -> Point<f64, Logical> {
    let (pos, zoom) = tile_view_pos(f, window);
    pos + Point::from((x, 1. + 14.)).upscale(zoom)
}

fn move_to(f: &mut Fixture, id: ClientId, pos: Point<f64, Logical>) {
    let pointer = f.client(id).state.virtual_pointer();
    pointer.motion_absolute(
        0,
        pos.x.round() as u32,
        pos.y.round() as u32,
        u32::from(OUTPUT_SIZE.0),
        u32::from(OUTPUT_SIZE.1),
    );
    pointer.frame();
    f.double_roundtrip(id);
}

fn button(f: &mut Fixture, id: ClientId, button: u32, pressed: bool) {
    let state = if pressed {
        wl_pointer::ButtonState::Pressed
    } else {
        wl_pointer::ButtonState::Released
    };
    let pointer = f.client(id).state.virtual_pointer();
    pointer.button(0, button, state);
    pointer.frame();
    f.double_roundtrip(id);
}

#[test]
fn left_press_activates_other_buttons_do_nothing() {
    let (mut f, id) = set_up("strip");
    let first = open_window(&mut f, id);
    let second = open_window(&mut f, id);
    assert_eq!(focused(&mut f), second);

    let p = strip_point(&mut f, first, 100.);
    move_to(&mut f, id, p);
    button(&mut f, id, BTN_RIGHT, true);
    button(&mut f, id, BTN_RIGHT, false);
    assert_eq!(focused(&mut f), second);

    button(&mut f, id, BTN_LEFT, true);
    button(&mut f, id, BTN_LEFT, false);
    assert_eq!(focused(&mut f), first);
}

#[test]
fn canvas_strip_drag_keeps_the_grab_point() {
    let (mut f, id) = set_up("canvas");
    let window = open_window(&mut f, id);
    let (_, zoom) = tile_view_pos(&mut f, window);
    assert_eq!(zoom, 0.5);
    let before = plane_pos(&mut f, window);

    let start = strip_point(&mut f, window, 100.);
    move_to(&mut f, id, start);
    button(&mut f, id, BTN_LEFT, true);
    for step in 1..=10 {
        let p = start + Point::from((20. * f64::from(step), 10. * f64::from(step)));
        move_to(&mut f, id, p);
    }
    button(&mut f, id, BTN_LEFT, false);
    f.niri_complete_animations();

    let after = plane_pos(&mut f, window);
    let moved = after - before;
    // The tile moved with the pointer, (2, 1) in direction, without jumping vertically by the
    // strip height (30 plane px at zoom 0.5). The test pointer's first counted motion is lost
    // (events with timestamp 0), so the distance itself is not exact here; the nested-session
    // run with real timestamps moves by exactly the pointer delta.
    assert!(moved.x > 350., "moved by {moved:?}");
    assert!((moved.x - 2. * moved.y).abs() <= 4., "moved by {moved:?}");
}
