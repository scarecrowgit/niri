//! ashd: pointer input to windows on a zoomed canvas.

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
    let zoom = ws_transform(&mut f).zoom;
    let expected = (pos - win).downscale(zoom);
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

/// Presses or releases the left Super key on the seat keyboard (Mod on the headless backend).
fn super_key(f: &mut Fixture, pressed: bool) {
    use smithay::backend::input::{InputTime, KeyState};
    use smithay::input::keyboard::{FilterResult, Keycode};

    let state = if pressed {
        KeyState::Pressed
    } else {
        KeyState::Released
    };
    let niri = f.niri_state();
    let keyboard = niri.niri.seat.get_keyboard().unwrap();
    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
    // KEY_LEFTMETA (125) + 8.
    keyboard.input::<(), _>(
        niri,
        Keycode::new(133),
        state,
        serial,
        InputTime::from_millis(0),
        |_, _, _| FilterResult::Forward,
    );
}

/// Sends wheel notches (`v120 / 120` each, negative = up) through the virtual pointer.
fn wheel(f: &mut Fixture, id: ClientId, v120: i32) {
    let pointer = f.client(id).state.virtual_pointer();
    pointer.axis_source(wl_pointer::AxisSource::Wheel);
    pointer.axis_discrete(
        0,
        wl_pointer::Axis::VerticalScroll,
        f64::from(v120) / 120. * 15.,
        v120.signum(),
    );
    pointer.frame();
    f.double_roundtrip(id);
}

fn ws_transform(f: &mut Fixture) -> crate::layout::canvas::CanvasTransform {
    f.niri()
        .layout
        .active_workspace()
        .unwrap()
        .canvas_target_transform()
        .unwrap()
}

#[test]
fn mod_wheel_zooms_continuously_at_the_cursor() {
    let (mut f, id, surface) = set_up(0.62);
    let anchor = move_pointer(&mut f, id, Point::from((700., 400.)));
    let before = ws_transform(&mut f);
    let plane = before.view_to_plane(anchor);
    f.client(id).state.take_pointer_events(&surface);

    super_key(&mut f, true);
    // Settled zooms stay on the fractional scale grid (`crate::layout::client_scale`).
    let q = |z: f64| crate::layout::client_scale::quantize_zoom(z, 1.);
    // One notch up: x1.2 (default wheel-zoom-factor), no step list, no cooldown.
    wheel(&mut f, id, -120);
    let t = ws_transform(&mut f);
    assert!((t.zoom - q(q(0.62) * 1.2)).abs() < 1e-9, "{t:?}");
    assert!((t.view_to_plane(anchor).x - plane.x).abs() < 1e-6);
    assert!((t.view_to_plane(anchor).y - plane.y).abs() < 1e-6);

    // Two more right away accumulate (capped at the largest zoom step, 1.0).
    wheel(&mut f, id, -120);
    let t = ws_transform(&mut f);
    assert!((t.zoom - q(q(q(0.62) * 1.2) * 1.2)).abs() < 1e-9, "{t:?}");
    wheel(&mut f, id, -120);
    assert_eq!(ws_transform(&mut f).zoom, 1.0);

    // A quarter notch down zooms by 1.2^-0.25. (The virtual pointer protocol carries whole
    // notches only, so this sends the smooth amount without a discrete step: 15 / 4.)
    let pointer = f.client(id).state.virtual_pointer();
    pointer.axis_source(wl_pointer::AxisSource::Wheel);
    pointer.axis(0, wl_pointer::Axis::VerticalScroll, 3.75);
    pointer.frame();
    f.double_roundtrip(id);
    let t = ws_transform(&mut f);
    assert!((t.zoom - q(1.2f64.powf(-0.25))).abs() < 1e-9, "{t:?}");
    let t2 = ws_transform(&mut f);
    assert!((t2.view_to_plane(anchor).x - plane.x).abs() < 1e-6);

    // Zooming out stops at the smallest step.
    for _ in 0..10 {
        wheel(&mut f, id, 120);
    }
    assert_eq!(ws_transform(&mut f).zoom, 0.5);

    // The window got none of these scroll events.
    let events = f.client(id).state.take_pointer_events(&surface);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, PointerEvent::Axis { .. })),
        "{events:?}"
    );

    // Without Mod the scroll goes to the window and the zoom stays.
    super_key(&mut f, false);
    wheel(&mut f, id, -120);
    assert_eq!(ws_transform(&mut f).zoom, 0.5);
}

#[test]
fn mod_wheel_zoom_can_be_turned_off() {
    let mut config = config(0.62);
    config.layout.canvas.wheel_zoom_factor = 1.;
    let mut f = Fixture::with_config(config);
    f.add_output(1, OUTPUT_SIZE);
    let id = f.add_client();
    move_pointer(&mut f, id, Point::from((700., 400.)));
    super_key(&mut f, true);
    wheel(&mut f, id, -120);
    let zoom = ws_transform(&mut f).zoom;
    assert_eq!(zoom, crate::layout::client_scale::quantize_zoom(0.62, 1.));
}

fn canvas_cursor(f: &mut Fixture) -> Option<smithay::input::pointer::CursorIcon> {
    f.niri().cursor_manager.override_icon()
}

#[test]
fn canvas_cursors_follow_the_rule() {
    use smithay::input::pointer::CursorIcon;

    let (mut f, id, surface) = set_up(0.5);
    let win = window_view_pos(&mut f);

    // Empty canvas: grab.
    move_pointer(&mut f, id, Point::from((100., 100.)));
    assert_eq!(canvas_cursor(&mut f), Some(CursorIcon::Grab));

    // Mod held: zoom-in; released: grab again.
    super_key(&mut f, true);
    f.double_roundtrip(id);
    assert_eq!(canvas_cursor(&mut f), Some(CursorIcon::ZoomIn));
    super_key(&mut f, false);
    f.double_roundtrip(id);
    assert_eq!(canvas_cursor(&mut f), Some(CursorIcon::Grab));

    // Over the window the client's cursor wins, Mod or not.
    move_pointer(&mut f, id, win + Point::from((50., 50.)));
    assert_eq!(canvas_cursor(&mut f), None);
    super_key(&mut f, true);
    f.double_roundtrip(id);
    assert_eq!(canvas_cursor(&mut f), None);
    super_key(&mut f, false);
    f.double_roundtrip(id);
    f.client(id).state.take_pointer_events(&surface);

    // Leaving the window: grab.
    move_pointer(&mut f, id, Point::from((100., 100.)));
    assert_eq!(canvas_cursor(&mut f), Some(CursorIcon::Grab));

    // Mod+LMB on the empty plane pans: grabbing, also when the pointer crosses the window;
    // the end of the pan gives the regular canvas cursor back.
    super_key(&mut f, true);
    f.double_roundtrip(id);
    button(&mut f, id, true);
    assert!(f.niri().layout.has_canvas_pan_gesture());
    assert_eq!(canvas_cursor(&mut f), Some(CursorIcon::Grabbing));
    move_pointer(&mut f, id, win + Point::from((10., 10.)));
    assert_eq!(canvas_cursor(&mut f), Some(CursorIcon::Grabbing));
    button(&mut f, id, false);
    assert!(!f.niri().layout.has_canvas_pan_gesture());
    super_key(&mut f, false);
    f.double_roundtrip(id);
    move_pointer(&mut f, id, Point::from((5., 5.)));
    assert_eq!(canvas_cursor(&mut f), Some(CursorIcon::Grab));
}

#[test]
fn canvas_cursor_only_on_canvases() {
    let mut f = Fixture::with_config(Config::default());
    f.add_output(1, OUTPUT_SIZE);
    let id = f.add_client();
    move_pointer(&mut f, id, Point::from((100., 100.)));
    assert_eq!(canvas_cursor(&mut f), None);
}

#[test]
fn zoomed_window_gets_the_zoomed_scale() {
    use crate::layout::client_scale::CLIENT_SCALE_SETTLE;
    use crate::layout::LayoutElement as _;

    let (mut f, id, _surface) = set_up(0.5);
    let scale = |f: &mut Fixture| {
        let (_, mapped) = f.niri().layout.windows().next().unwrap();
        let (scale, _) = mapped.preferred_scale_transform().unwrap();
        (scale.fractional_scale(), scale.integer_scale())
    };

    std::thread::sleep(CLIENT_SCALE_SETTLE + std::time::Duration::from_millis(30));
    f.double_roundtrip(id);
    assert_eq!(scale(&mut f), (0.5, 1));

    // Back to 100%: the output scale right away.
    f.niri().layout.canvas_set_zoom(1., None);
    f.double_roundtrip(id);
    assert_eq!(scale(&mut f), (1., 1));
}
