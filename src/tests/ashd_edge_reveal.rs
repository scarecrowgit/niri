//! ashd: edge panels revealed over a fullscreen window (`edge-reveal`), client level.

use niri_config::Config;
use smithay::desktop::{layer_map_for_output, LayerSurface};
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;
use wayland_client::protocol::wl_pointer;
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;
use crate::tests::client::{ClientId, LayerConfigureProps};

const OUTPUT_SIZE: (u16, u16) = (1920, 1080);

fn set_up(extra: &str) -> (Fixture, ClientId) {
    let config = Config::parse_mem(&format!(
        r#"
        animations {{
            off
        }}
        edge-reveal {{
            on
            dwell-ms 0
            hide-delay-ms 0
            {extra}
        }}
        "#
    ))
    .unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, OUTPUT_SIZE);
    let id = f.add_client();
    (f, id)
}

fn open_window(f: &mut Fixture, id: ClientId) -> WlSurface {
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
    surface
}

/// A rail along the left edge: top layer, exclusive zone 64.
fn open_rail(f: &mut Fixture, id: ClientId, namespace: &str) {
    let layer = f.client(id).create_layer(None, Layer::Top, namespace);
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Left | Anchor::Top | Anchor::Bottom),
        size: Some((64, 0)),
        exclusive_zone: Some(64),
        ..Default::default()
    });
    layer.commit();
    f.roundtrip(id);

    let layer = f.client(id).layer(&surface);
    layer.attach_new_buffer();
    layer.set_size(64, OUTPUT_SIZE.1);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);
}

fn rail(f: &mut Fixture) -> LayerSurface {
    let output = f.niri_output(1);
    let map = layer_map_for_output(&output);
    let surface = map.layers().next().unwrap().clone();
    surface
}

fn fullscreen_focused(f: &mut Fixture, id: ClientId, surface: &WlSurface) {
    let window = f.niri().layout.focus().unwrap().window.clone();
    f.niri().layout.set_fullscreen(&window, true);
    f.double_roundtrip(id);
    // The client acks the fullscreen configure.
    let window = f.client(id).window(surface);
    window.set_size(OUTPUT_SIZE.0, OUTPUT_SIZE.1);
    window.ack_last_and_commit();
    f.double_roundtrip(id);
    f.niri_complete_animations();
    f.double_roundtrip(id);
}

fn move_pointer(f: &mut Fixture, id: ClientId, x: u32, y: u32) {
    let pointer = f.client(id).state.virtual_pointer();
    pointer.motion_absolute(0, x, y, u32::from(OUTPUT_SIZE.0), u32::from(OUTPUT_SIZE.1));
    pointer.frame();
    f.double_roundtrip(id);
}

fn button(f: &mut Fixture, id: ClientId, pressed: bool) {
    let state = if pressed {
        wl_pointer::ButtonState::Pressed
    } else {
        wl_pointer::ButtonState::Released
    };
    let pointer = f.client(id).state.virtual_pointer();
    pointer.button(0, 0x110, state);
    pointer.frame();
    f.double_roundtrip(id);
}

/// Placement of the rail after the slide animation settled.
fn rail_placement(f: &mut Fixture) -> Option<crate::edge_reveal::Placement> {
    let rail = rail(f);
    let niri = f.niri();
    niri.clock.set_complete_instantly(true);
    niri.update_edge_reveal_placements(None);
    niri.clock.set_complete_instantly(false);
    niri.edge_reveal.placement(&rail)
}

fn covered(f: &mut Fixture) -> bool {
    let output = f.niri_output(1);
    f.niri()
        .layout
        .monitor_for_output(&output)
        .unwrap()
        .render_above_top_layer()
}

fn pointer_on_rail(f: &mut Fixture) -> bool {
    let rail = rail(f);
    f.niri().pointer_contents.layer.as_ref() == Some(&rail)
}

#[test]
fn edge_push_reveals_and_leaving_hides() {
    let (mut f, id) = set_up("");
    open_rail(&mut f, id, "rail");
    let win = open_window(&mut f, id);
    fullscreen_focused(&mut f, id, &win);
    assert!(covered(&mut f));
    // Covered and slid out: not drawn.
    move_pointer(&mut f, id, 900, 500);
    assert_eq!(rail_placement(&mut f), None);
    assert!(!pointer_on_rail(&mut f));

    // Against the left edge: the rail is drawn above the fullscreen window and gets the pointer.
    move_pointer(&mut f, id, 0, 500);
    let placement = rail_placement(&mut f).unwrap();
    assert!(placement.above);
    assert_eq!(placement.offset.x, 0.);
    move_pointer(&mut f, id, 30, 500);
    assert!(pointer_on_rail(&mut f));

    // Clicking on it keeps it.
    button(&mut f, id, true);
    button(&mut f, id, false);
    assert!(rail_placement(&mut f).is_some());

    // Away from it: slides out.
    move_pointer(&mut f, id, 900, 500);
    f.double_roundtrip(id);
    assert_eq!(rail_placement(&mut f), None);
    assert!(!pointer_on_rail(&mut f));
}

#[test]
fn held_button_blocks_the_reveal() {
    let (mut f, id) = set_up("");
    open_rail(&mut f, id, "rail");
    let win = open_window(&mut f, id);
    fullscreen_focused(&mut f, id, &win);

    // A drag (button held) reaching the edge doesn't reveal.
    move_pointer(&mut f, id, 900, 500);
    button(&mut f, id, true);
    move_pointer(&mut f, id, 0, 500);
    f.double_roundtrip(id);
    assert_eq!(rail_placement(&mut f), None);

    // Released at the edge: now it does.
    button(&mut f, id, false);
    f.double_roundtrip(id);
    assert!(rail_placement(&mut f).is_some_and(|p| p.above));
}

#[test]
fn namespace_filter() {
    let (mut f, id) = set_up(r#"namespace "^ashd-rail$""#);
    open_rail(&mut f, id, "other-panel");
    let win = open_window(&mut f, id);
    fullscreen_focused(&mut f, id, &win);
    move_pointer(&mut f, id, 0, 500);
    assert_eq!(rail_placement(&mut f), None);
}

#[test]
fn not_covered_is_untouched() {
    let (mut f, id) = set_up("");
    open_rail(&mut f, id, "rail");
    open_window(&mut f, id);
    assert!(!covered(&mut f));
    move_pointer(&mut f, id, 0, 500);
    // Drawn exactly as upstream: no placement at all.
    assert_eq!(rail_placement(&mut f), None);
    move_pointer(&mut f, id, 30, 500);
    assert!(pointer_on_rail(&mut f));
}

#[test]
fn off_by_default() {
    let config = Config::parse_mem("animations { off; }").unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, OUTPUT_SIZE);
    let id = f.add_client();
    open_rail(&mut f, id, "rail");
    let win = open_window(&mut f, id);
    fullscreen_focused(&mut f, id, &win);
    move_pointer(&mut f, id, 0, 500);
    assert_eq!(rail_placement(&mut f), None);
    move_pointer(&mut f, id, 30, 500);
    assert!(!pointer_on_rail(&mut f));
}
