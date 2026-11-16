//! ashd: xdg-activation after the keyboard left a client. A client must not take the focus back
//! with a serial from before the user switched away from it (Claude Desktop did that right after
//! a Super+Tab switch), while the usual ways of handing the focus on keep working: a focused app
//! handing its token to another app, a launcher (layer surface) activating an app, a click on a
//! notification.

use niri_config::Config;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity,
};
use wayland_client::protocol::wl_pointer;
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;
use crate::tests::client::{ClientId, LayerConfigureProps};

const HONOR_INVALID: &str = "debug { honor-xdg-activation-with-invalid-serial; }";
const BTN_LEFT: u32 = 0x110;

fn set_up(extra: &str) -> Fixture {
    let config = Config::parse_mem(&format!(
        r#"
        animations {{
            off
        }}
        {extra}
        "#
    ))
    .unwrap();
    let mut f = Fixture::with_config(config);
    f.add_output(1, (1920, 1080));
    f
}

fn open_window(f: &mut Fixture, id: ClientId) -> (u64, WlSurface) {
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

    (f.niri().layout.focus().unwrap().id().get(), surface)
}

fn open_layer(
    f: &mut Fixture,
    id: ClientId,
    kb: KeyboardInteractivity,
    anchor: Anchor,
) -> WlSurface {
    let layer = f.client(id).create_layer(None, Layer::Top, "layer");
    let surface = layer.surface.clone();
    layer.set_configure_props(LayerConfigureProps {
        anchor: Some(anchor),
        size: Some((300, 200)),
        kb_interactivity: Some(kb),
        ..Default::default()
    });
    layer.commit();
    f.roundtrip(id);

    let layer = f.client(id).layer(&surface);
    layer.attach_new_buffer();
    layer.set_size(300, 200);
    layer.ack_last_and_commit();
    f.double_roundtrip(id);
    surface
}

fn focused(f: &mut Fixture) -> Option<u64> {
    f.niri().layout.focus().map(|win| win.id().get())
}

/// A token from client `id` with `serial` (and `surface`).
fn token(f: &mut Fixture, id: ClientId, serial: u32, surface: Option<&WlSurface>) -> String {
    f.client(id).state.request_activation_token(serial, surface);
    f.double_roundtrip(id);
    f.client(id).state.activation_tokens.last().unwrap().clone()
}

/// Client `id` activates its `surface` with `token`.
fn activate(f: &mut Fixture, id: ClientId, token: &str, surface: &WlSurface) {
    f.client(id).state.activate(token, surface);
    f.double_roundtrip(id);
    f.niri_complete_animations();
}

fn last_serial(f: &mut Fixture, id: ClientId) -> u32 {
    f.client(id).state.last_serial
}

/// The pointer of client `id`'s virtual pointer to `(x, y)` of the output.
fn move_pointer(f: &mut Fixture, id: ClientId, x: u32, y: u32) {
    let pointer = f.client(id).state.virtual_pointer();
    pointer.motion_absolute(0, x, y, 1920, 1080);
    pointer.frame();
    f.double_roundtrip(id);
}

fn click(f: &mut Fixture, id: ClientId) {
    let pointer = f.client(id).state.virtual_pointer();
    pointer.button(0, BTN_LEFT, wl_pointer::ButtonState::Pressed);
    pointer.frame();
    pointer.button(0, BTN_LEFT, wl_pointer::ButtonState::Released);
    pointer.frame();
    f.double_roundtrip(id);
}

#[test]
fn self_activation_with_a_serial_from_before_switching_away_is_refused() {
    for extra in ["", HONOR_INVALID] {
        let mut f = set_up(extra);
        let (app, other) = (f.add_client(), f.add_client());
        let (a, a_surface) = open_window(&mut f, app);
        let serial = last_serial(&mut f, app);
        // The user goes elsewhere (here: a new window of another app takes the focus).
        let (b, _) = open_window(&mut f, other);
        assert_eq!(focused(&mut f), Some(b));

        let t = token(&mut f, app, serial, Some(&a_surface));
        activate(&mut f, app, &t, &a_surface);
        assert_eq!(focused(&mut f), Some(b), "{extra}");
        assert_ne!(focused(&mut f), Some(a));
    }
}

#[test]
fn the_resting_pointer_does_not_let_it_through_either() {
    // The pointer still rests on the window the user switched away from: its serials are no
    // older than the pointer's enter, which passed the pointer check.
    let mut f = set_up("");
    let (app, other) = (f.add_client(), f.add_client());
    let (_a, a_surface) = open_window(&mut f, app);
    move_pointer(&mut f, app, 100, 100);
    let serial = last_serial(&mut f, app);
    let (b, _) = open_window(&mut f, other);
    assert_eq!(focused(&mut f), Some(b));

    let t = token(&mut f, app, serial, Some(&a_surface));
    activate(&mut f, app, &t, &a_surface);
    assert_eq!(focused(&mut f), Some(b));
}

#[test]
fn a_focused_app_hands_its_token_to_another_app() {
    // A link clicked in the focused app opens it in the browser, which is already running.
    let mut f = set_up("");
    let (browser, app) = (f.add_client(), f.add_client());
    let (b, b_surface) = open_window(&mut f, browser);
    let (a, a_surface) = open_window(&mut f, app);
    assert_eq!(focused(&mut f), Some(a));

    let serial = last_serial(&mut f, app);
    let t = token(&mut f, app, serial, Some(&a_surface));
    activate(&mut f, browser, &t, &b_surface);
    assert_eq!(focused(&mut f), Some(b));
}

#[test]
fn a_launcher_layer_activates_an_app() {
    // The launcher (a layer surface with the keyboard) hands its token to the app, also to an
    // already running instance (a second launch focusing the existing window).
    for extra in ["", HONOR_INVALID] {
        let mut f = set_up(extra);
        let (launcher, app) = (f.add_client(), f.add_client());
        let (a, a_surface) = open_window(&mut f, app);
        let (c, _) = open_window(&mut f, app);
        assert_eq!(focused(&mut f), Some(c));

        let layer = open_layer(
            &mut f,
            launcher,
            KeyboardInteractivity::OnDemand,
            Anchor::Top,
        );
        let serial = last_serial(&mut f, launcher);
        let t = token(&mut f, launcher, serial, Some(&layer));
        activate(&mut f, app, &t, &a_surface);
        assert_eq!(focused(&mut f), Some(a), "{extra}");
    }
}

#[test]
fn a_notification_click_activates_the_app() {
    // The notification daemon's popup takes no keyboard; the click's serial goes into the token
    // it hands to the app.
    let mut f = set_up("");
    let (daemon, app, other) = (f.add_client(), f.add_client(), f.add_client());
    let (a, a_surface) = open_window(&mut f, app);
    let (b, _) = open_window(&mut f, other);
    assert_eq!(focused(&mut f), Some(b));

    let popup = open_layer(
        &mut f,
        daemon,
        KeyboardInteractivity::None,
        Anchor::Bottom | Anchor::Right,
    );
    move_pointer(&mut f, daemon, 1900, 1060);
    click(&mut f, daemon);
    let serial = last_serial(&mut f, daemon);
    let t = token(&mut f, daemon, serial, Some(&popup));
    activate(&mut f, app, &t, &a_surface);
    assert_eq!(focused(&mut f), Some(a));
}

#[test]
fn with_honor_invalid_serial_old_tokens_work_again_after_a_moment() {
    // Discord and Telegram activate themselves with old serials when their notification is
    // clicked, well after the user left them: `honor-xdg-activation-with-invalid-serial` keeps
    // that working once the switch away is a second old.
    let mut f = set_up(HONOR_INVALID);
    let (app, other) = (f.add_client(), f.add_client());
    let (a, a_surface) = open_window(&mut f, app);
    let serial = last_serial(&mut f, app);
    let (b, _) = open_window(&mut f, other);
    assert_eq!(focused(&mut f), Some(b));

    std::thread::sleep(std::time::Duration::from_millis(1100));
    let t = token(&mut f, app, serial, Some(&a_surface));
    activate(&mut f, app, &t, &a_surface);
    assert_eq!(focused(&mut f), Some(a));
}
