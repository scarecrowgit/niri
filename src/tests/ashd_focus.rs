//! ashd: workspace index references with hidden workspaces, and explicit focus actions taking
//! the keyboard from an on-demand layer surface.

use niri_config::{Action, Config, WorkspaceReference};
use smithay::backend::input::InputTime;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity,
};
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;
use crate::niri::KeyboardFocus;
use crate::tests::client::{ClientId, LayerConfigureProps};

const OUTPUT_SIZE: (u16, u16) = (1920, 1080);

fn set_up(extra: &str) -> (Fixture, ClientId) {
    let config = Config::parse_mem(&format!(
        r#"
        workspace "a"
        workspace "scratch" {{
            hidden
        }}
        workspace "b"
        animations {{
            off
        }}
        {extra}
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

/// Maps an on-demand top layer surface along the right edge (like the shell's scratchpad
/// panel). Newly mapped on-demand surfaces get the keyboard focus.
fn open_panel(f: &mut Fixture, id: ClientId) -> WlSurface {
    let layer = f.client(id).create_layer(None, Layer::Top, "panel");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(Anchor::Right | Anchor::Top | Anchor::Bottom),
        size: Some((300, 0)),
        kb_interactivity: Some(KeyboardInteractivity::OnDemand),
        ..Default::default()
    });
    layer.commit();
    f.roundtrip(id);

    let layer = f.client(id).layer(&surface);
    layer.attach_new_buffer();
    layer.set_size(300, 1080);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);
    surface
}

fn active_ws_name(f: &mut Fixture) -> Option<String> {
    f.niri().layout.active_workspace().unwrap().name().cloned()
}

fn action(f: &mut Fixture, id: ClientId, action: Action) {
    f.niri_state().do_action(action, false, InputTime::now());
    f.double_roundtrip(id);
}

fn keyboard_on_layer(f: &mut Fixture) -> bool {
    matches!(f.niri().keyboard_focus, KeyboardFocus::LayerShell { .. })
}

fn keyboard_on_window(f: &mut Fixture, window: u64) -> bool {
    let Some(focus) = f.niri().layout.focus() else {
        return false;
    };
    if focus.id().get() != window {
        return false;
    }
    let surface = focus.toplevel().wl_surface().clone();
    matches!(
        &f.niri().keyboard_focus,
        KeyboardFocus::Layout { surface: Some(s) } if *s == surface
    )
}

/// Gives the panel the on-demand focus again, as a click on it would.
fn refocus_panel(f: &mut Fixture, id: ClientId, panel: &LayerSurfaceHandle) {
    f.niri().layer_shell_on_demand_focus = Some(panel.0.clone());
    f.double_roundtrip(id);
    assert!(keyboard_on_layer(f));
}

struct LayerSurfaceHandle(smithay::desktop::LayerSurface);

fn panel_handle(f: &mut Fixture) -> LayerSurfaceHandle {
    LayerSurfaceHandle(
        f.niri()
            .layer_shell_on_demand_focus
            .clone()
            .expect("the panel has the on-demand focus"),
    )
}

#[test]
fn focus_workspace_index_skips_hidden() {
    let (mut f, id) = set_up("");
    // Workspaces: a, scratch (hidden), b, empty.
    action(
        &mut f,
        id,
        Action::FocusWorkspace(WorkspaceReference::Index(1)),
    );
    assert_eq!(active_ws_name(&mut f).as_deref(), Some("a"));
    action(
        &mut f,
        id,
        Action::FocusWorkspace(WorkspaceReference::Index(2)),
    );
    assert_eq!(active_ws_name(&mut f).as_deref(), Some("b"));
    action(
        &mut f,
        id,
        Action::FocusWorkspace(WorkspaceReference::Index(3)),
    );
    assert_eq!(active_ws_name(&mut f), None);
    // Past the end clamps to the last workspace, as upstream.
    action(
        &mut f,
        id,
        Action::FocusWorkspace(WorkspaceReference::Index(9)),
    );
    assert_eq!(active_ws_name(&mut f), None);
    // By name it stays a no-op.
    action(
        &mut f,
        id,
        Action::FocusWorkspace(WorkspaceReference::Name("scratch".into())),
    );
    assert_eq!(active_ws_name(&mut f), None);
}

#[test]
fn move_window_to_workspace_index_skips_hidden() {
    let (mut f, id) = set_up("");
    action(
        &mut f,
        id,
        Action::FocusWorkspace(WorkspaceReference::Index(1)),
    );
    let win = open_window(&mut f, id);

    // Index 2 is "b", not the scratchpad.
    action(
        &mut f,
        id,
        Action::MoveWindowToWorkspace(WorkspaceReference::Index(2), false),
    );
    let ws_name = |f: &mut Fixture| {
        f.niri()
            .layout
            .workspaces()
            .find(|(_, _, ws)| ws.windows().any(|w| w.id().get() == win))
            .and_then(|(_, _, ws)| ws.name().cloned())
    };
    assert_eq!(ws_name(&mut f).as_deref(), Some("b"));

    // By id with an index: the window's monitor.
    action(
        &mut f,
        id,
        Action::MoveWindowToWorkspaceById {
            window_id: win,
            reference: WorkspaceReference::Index(1),
            focus: false,
        },
    );
    assert_eq!(ws_name(&mut f).as_deref(), Some("a"));
}

#[test]
fn focus_window_takes_keyboard_from_on_demand_layer() {
    let (mut f, id) = set_up("");
    let first = open_window(&mut f, id);
    let second = open_window(&mut f, id);
    let _panel = open_panel(&mut f, id);
    assert!(keyboard_on_layer(&mut f));
    let panel = panel_handle(&mut f);

    // focus-window --id, also for the window the layout already has focused.
    action(&mut f, id, Action::FocusWindow(second));
    assert!(keyboard_on_window(&mut f, second));
    refocus_panel(&mut f, id, &panel);
    action(&mut f, id, Action::FocusWindow(first));
    assert!(keyboard_on_window(&mut f, first));

    // focus-window-previous.
    refocus_panel(&mut f, id, &panel);
    action(&mut f, id, Action::FocusWindowPrevious);
    assert!(keyboard_on_window(&mut f, second));

    // focus-tiling.
    refocus_panel(&mut f, id, &panel);
    action(&mut f, id, Action::FocusTiling);
    assert!(keyboard_on_window(&mut f, second));

    // Directional focus (upstream behaviour).
    refocus_panel(&mut f, id, &panel);
    action(&mut f, id, Action::FocusColumnLeft);
    assert!(keyboard_on_window(&mut f, first));

    // Actions that don't focus anything keep the layer focus.
    refocus_panel(&mut f, id, &panel);
    action(&mut f, id, Action::CenterColumn);
    assert!(keyboard_on_layer(&mut f));
}

#[test]
fn unhide_and_workspace_moves_take_keyboard_from_on_demand_layer() {
    let (mut f, id) = set_up("");
    let win = open_window(&mut f, id);
    let _panel = open_panel(&mut f, id);
    let panel = panel_handle(&mut f);

    action(&mut f, id, Action::HideWindowById(win));
    // Hiding doesn't focus anything.
    assert!(keyboard_on_layer(&mut f));
    action(&mut f, id, Action::UnhideWindowById(win));
    assert!(keyboard_on_window(&mut f, win));

    // A workspace switch caused by an action (moving the window with focus).
    refocus_panel(&mut f, id, &panel);
    assert_eq!(active_ws_name(&mut f).as_deref(), Some("a"));
    action(
        &mut f,
        id,
        Action::MoveWindowToWorkspace(WorkspaceReference::Index(2), true),
    );
    assert_eq!(active_ws_name(&mut f).as_deref(), Some("b"));
    assert!(keyboard_on_window(&mut f, win));
}

fn move_pointer(f: &mut Fixture, id: ClientId, x: u32, y: u32) {
    let pointer = f.client(id).state.virtual_pointer();
    pointer.motion_absolute(0, x, y, u32::from(OUTPUT_SIZE.0), u32::from(OUTPUT_SIZE.1));
    pointer.frame();
    f.double_roundtrip(id);
}

fn window_center(f: &mut Fixture, window: u64) -> (u32, u32) {
    let layout = &f.niri().layout;
    let ws = layout.active_workspace().unwrap();
    let (tile, pos) = ws
        .tiles_with_render_positions()
        .find(|(tile, _, _)| tile.window().id().get() == window)
        .map(|(tile, pos, _)| (tile, pos))
        .unwrap();
    let size = tile.tile_size();
    ((pos.x + size.w / 2.) as u32, (pos.y + size.h / 2.) as u32)
}

#[test]
fn pointer_motion_without_ffm_keeps_layer_focus() {
    let (mut f, id) = set_up("");
    let win = open_window(&mut f, id);
    let _panel = open_panel(&mut f, id);
    assert!(keyboard_on_layer(&mut f));

    move_pointer(&mut f, id, 1800, 500);
    let (x, y) = window_center(&mut f, win);
    move_pointer(&mut f, id, x, y);
    assert!(keyboard_on_layer(&mut f));
}

#[test]
fn focus_follows_mouse_into_the_focused_window_takes_keyboard() {
    let (mut f, id) = set_up("input { focus-follows-mouse; }");
    let win = open_window(&mut f, id);
    let _panel = open_panel(&mut f, id);

    // Over the panel, then back over the window the layout has focused all along.
    move_pointer(&mut f, id, 1800, 500);
    assert!(keyboard_on_layer(&mut f));
    let (x, y) = window_center(&mut f, win);
    move_pointer(&mut f, id, x, y);
    assert!(keyboard_on_window(&mut f, win));
}
