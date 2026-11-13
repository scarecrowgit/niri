//! rice: pointer input to windows on a zoomed canvas.

use client::{ClientId, PointerEvent};
use niri_config::Config;
use smithay::utils::{Logical, Point};
use wayland_client::protocol::wl_pointer;
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;

const BTN_LEFT: u32 = 0x110;
const OUTPUT_SIZE: (u16, u16) = (1920, 1080);

fn config(zoom: f64) -> Config {
    Config::parse_mem(&format!(
        r#"
        layout {{
            default-workspace-mode "canvas"
            canvas {{
                default-zoom {zoom}
                zoom-steps 0.5 0.62 1.0
            }}
        }}
        animations {{
            off
        }}
        "#
    ))
    .unwrap()
}

/// One output with a canvas workspace at `zoom` and one 400 × 300 window on it.
fn set_up(zoom: f64) -> (Fixture, ClientId, WlSurface) {
    let mut f = Fixture::with_config(config(zoom));
    f.add_output(1, OUTPUT_SIZE);

    let id = f.add_client();
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

    (f, id, surface)
}

/// Where the window's buffer is drawn on the output (view coordinates), computed from the
/// rendering positions independently of the hit-testing code.
fn window_view_pos(f: &mut Fixture) -> Point<f64, Logical> {
    let layout = &f.niri().layout;
    let ws = layout.active_workspace().unwrap();
    let zoom = ws
        .canvas_transform()
        .expect("workspace must be a canvas")
        .zoom;
    // Canvas tiles are positioned (and rounded) in unscaled view coordinates, then drawn scaled.
    let (tile, pos) = ws.floating().tiles_with_render_positions().next().unwrap();
    (pos + tile.buf_loc()).upscale(zoom)
}

/// Moves the pointer to the whole logical pixel at or before `pos`; returns that position.
fn move_pointer(f: &mut Fixture, id: ClientId, pos: Point<f64, Logical>) -> Point<f64, Logical> {
    let (x, y) = (pos.x.floor(), pos.y.floor());
    let pointer = f.client(id).state.virtual_pointer();
    pointer.motion_absolute(
        0,
        x as u32,
        y as u32,
        u32::from(OUTPUT_SIZE.0),
        u32::from(OUTPUT_SIZE.1),
    );
    pointer.frame();
    f.double_roundtrip(id);
    Point::from((x, y))
}

fn button(f: &mut Fixture, id: ClientId, pressed: bool) {
    let state = if pressed {
        wl_pointer::ButtonState::Pressed
    } else {
        wl_pointer::ButtonState::Released
    };
    let pointer = f.client(id).state.virtual_pointer();
    pointer.button(0, BTN_LEFT, state);
    pointer.frame();
    f.double_roundtrip(id);
}

/// Asserts that the last pointer position the surface got is `(x, y)`, within the precision of
/// wl_fixed (1/256) plus the error of the virtual pointer's absolute → logical conversion.
#[track_caller]
fn assert_last_position(events: &[PointerEvent], x: f64, y: f64) {
    let (ex, ey) = last_position(events).unwrap_or_else(|| panic!("no position: {events:?}"));
    assert!(
        (ex - x).abs() < 0.01 && (ey - y).abs() < 0.01,
        "expected ({x}, {y}), got {events:?}"
    );
}

fn last_position(events: &[PointerEvent]) -> Option<(f64, f64)> {
    events.iter().rev().find_map(|event| match event {
        PointerEvent::Enter { x, y } | PointerEvent::Motion { x, y } => Some((*x, *y)),
        _ => None,
    })
}

#[test]
fn click_at_half_zoom_arrives_at_surface_local_coords() {
    let (mut f, id, surface) = set_up(0.5);

    let ws = f.niri().layout.active_workspace().unwrap();
    assert_eq!(ws.canvas_zoom(), 0.5);

    // The window is drawn at half size: 400 × 300 → 200 × 150 view pixels.
    let win = window_view_pos(&mut f);
    assert_eq!(win, Point::from((860., 465.)));

    // A click 100 × 60 view pixels into the window is 200 × 120 surface pixels into it.
    move_pointer(&mut f, id, win + Point::from((100., 60.)));
    button(&mut f, id, true);
    button(&mut f, id, false);

    let events = f.client(id).state.take_pointer_events(&surface);
    assert!(
        matches!(events.first(), Some(PointerEvent::Enter { .. })),
        "expected pointer enter first: {events:?}"
    );
    assert_last_position(&events, 200., 120.);
    let buttons: Vec<_> = events
        .iter()
        .filter(|event| matches!(event, PointerEvent::Button { .. }))
        .cloned()
        .collect();
    assert_eq!(
        buttons,
        [
            PointerEvent::Button {
                button: BTN_LEFT,
                pressed: true
            },
            PointerEvent::Button {
                button: BTN_LEFT,
                pressed: false
            },
        ]
    );

    // Plain motion is scaled as well.
    move_pointer(&mut f, id, win + Point::from((150., 10.)));
    let events = f.client(id).state.take_pointer_events(&surface);
    assert_last_position(&events, 300., 20.);

    // Leaving the drawn window area leaves the surface, although the unscaled window would
    // still be under the pointer.
    move_pointer(&mut f, id, win + Point::from((250., 10.)));
    let events = f.client(id).state.take_pointer_events(&surface);
    assert_eq!(events, [PointerEvent::Leave]);
}

#[test]
fn drag_at_zoom_moves_at_scaled_speed() {
    let (mut f, id, surface) = set_up(0.5);
    let win = window_view_pos(&mut f);

    move_pointer(&mut f, id, win + Point::from((20., 20.)));
    button(&mut f, id, true);
    let _ = f.client(id).state.take_pointer_events(&surface);

    // While the button is held (implicit grab), motion keeps going to the surface scaled down,
    // including outside of the drawn window.
    move_pointer(&mut f, id, win + Point::from((70., 40.)));
    let events = f.client(id).state.take_pointer_events(&surface);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_last_position(&events, 140., 80.);

    move_pointer(&mut f, id, win + Point::from((260., 200.)));
    let events = f.client(id).state.take_pointer_events(&surface);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_last_position(&events, 520., 400.);

    button(&mut f, id, false);
    let events = f.client(id).state.take_pointer_events(&surface);
    assert_eq!(
        events.first(),
        Some(&PointerEvent::Button {
            button: BTN_LEFT,
            pressed: false
        })
    );
}

#[test]
fn input_at_zoom_one_is_unchanged() {
    let (mut f, id, surface) = set_up(1.);
    let win = window_view_pos(&mut f);

    move_pointer(&mut f, id, win + Point::from((100., 60.)));
    let events = f.client(id).state.take_pointer_events(&surface);
    assert_last_position(&events, 100., 60.);

    let (_, hit) = f.niri().pointer_contents.window.clone().unwrap();
    assert!(matches!(hit, crate::layout::HitType::Input { .. }));
}

#[test]
fn scroll_reaches_scaled_window() {
    let (mut f, id, surface) = set_up(0.62);
    let win = window_view_pos(&mut f);

    let pos = move_pointer(&mut f, id, win + Point::from((50., 50.)));
    let expected = (pos - win).downscale(0.62);
    let events = f.client(id).state.take_pointer_events(&surface);
    let (x, y) = last_position(&events).unwrap();
    // wl_fixed has a precision of 1/256.
    assert!((x - expected.x).abs() < 0.01, "{events:?} vs {expected:?}");
    assert!((y - expected.y).abs() < 0.01, "{events:?} vs {expected:?}");

    let pointer = f.client(id).state.virtual_pointer();
    pointer.axis(0, wl_pointer::Axis::VerticalScroll, 15.);
    pointer.frame();
    f.double_roundtrip(id);

    let events = f.client(id).state.take_pointer_events(&surface);
    assert!(
        events.iter().any(|event| matches!(
            event,
            PointerEvent::Axis {
                axis: wl_pointer::Axis::VerticalScroll,
                ..
            }
        )),
        "{events:?}"
    );
}
