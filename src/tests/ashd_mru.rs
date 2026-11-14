//! ashd: the recent-windows switcher with quick taps (Super+Tab tapped and released before the
//! switcher shows), in strip and canvas workspaces.

use niri_config::{Action, Config, WorkspaceReference};
use smithay::backend::input::{InputEvent, InputTime, KeyState, Keycode};

use super::*;
use crate::tests::client::ClientId;
use crate::tests::test_input_backend::{TestInputBackend, TestKeyboardKeyEvent};

// evdev + 8.
const LWIN: u32 = 125 + 8;
const LFSH: u32 = 42 + 8;
const TAB: u32 = 15 + 8;

fn set_up(extra: &str) -> (Fixture, ClientId) {
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

fn focused(f: &mut Fixture) -> Option<u64> {
    f.niri().layout.focus().map(|win| win.id().get())
}

fn key(f: &mut Fixture, id: ClientId, code: u32, pressed: bool) {
    let state = f.niri_state();
    state.process_input_event(InputEvent::<TestInputBackend>::Keyboard {
        event: TestKeyboardKeyEvent {
            time: InputTime::from_micros(0),
            code: Keycode::new(code),
            state: if pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            },
            count: 1,
        },
    });
    // Like the real event loop: refresh (keyboard focus update) after every input event.
    f.double_roundtrip(id);
}

/// Super+Tab tapped quickly: every key event comes long before the switcher open delay and the
/// focus debounce.
fn tap_super_tab(f: &mut Fixture, id: ClientId) {
    key(f, id, LWIN, true);
    key(f, id, TAB, true);
    key(f, id, TAB, false);
    key(f, id, LWIN, false);
}

fn check_quick_taps(f: &mut Fixture, id: ClientId) {
    let a = open_window(f, id);
    let b = open_window(f, id);
    let c = open_window(f, id);
    assert_eq!(focused(f), Some(c));

    // A quick tap goes to the previous window, the next ones toggle between the two most
    // recent windows.
    tap_super_tab(f, id);
    assert_eq!(focused(f), Some(b));
    tap_super_tab(f, id);
    assert_eq!(focused(f), Some(c));
    tap_super_tab(f, id);
    assert_eq!(focused(f), Some(b));

    // Holding Super and tapping Tab twice walks the list: b -> c -> a.
    key(f, id, LWIN, true);
    key(f, id, TAB, true);
    key(f, id, TAB, false);
    key(f, id, TAB, true);
    key(f, id, TAB, false);
    key(f, id, LWIN, false);
    assert_eq!(focused(f), Some(a));

    // Super+Shift+Tab goes backwards: the least recent window (c).
    key(f, id, LWIN, true);
    key(f, id, LFSH, true);
    key(f, id, TAB, true);
    key(f, id, TAB, false);
    key(f, id, LFSH, false);
    key(f, id, LWIN, false);
    assert_eq!(focused(f), Some(c));
}

#[test]
fn quick_taps_in_strip() {
    let (mut f, id) = set_up("");
    check_quick_taps(&mut f, id);
}

#[test]
fn quick_taps_in_canvas() {
    let (mut f, id) = set_up(r#"layout { default-workspace-mode "canvas"; }"#);
    check_quick_taps(&mut f, id);
}

#[test]
fn quick_taps_with_super_only_binds() {
    let (mut f, id) = set_up(
        r#"
        recent-windows {
            binds {
                Mod+Tab { next-window; }
                Mod+Shift+Tab { previous-window; }
            }
        }
        "#,
    );
    check_quick_taps(&mut f, id);
}

const W: u32 = 17 + 8;

/// Two windows on workspace 1, one window (focused) alone on workspace 2.
fn set_up_two_workspaces(extra: &str) -> (Fixture, ClientId, u64, u64) {
    let (mut f, id) = set_up(extra);
    let _a = open_window(&mut f, id);
    let b = open_window(&mut f, id);
    let c = open_window(&mut f, id);
    f.niri_state().do_action(
        Action::MoveWindowToWorkspace(WorkspaceReference::Index(2), true),
        false,
        InputTime::from_micros(0),
    );
    f.double_roundtrip(id);
    f.niri_complete_animations();
    assert_eq!(focused(&mut f), Some(c));
    (f, id, b, c)
}

/// Presses W while the switcher is open (the switcher's own "workspace scope" key; Super+W is
/// also a common close-window bind).
fn super_tab_then_w(f: &mut Fixture, id: ClientId) {
    key(f, id, LWIN, true);
    key(f, id, TAB, true);
    key(f, id, TAB, false);
    key(f, id, W, true);
    key(f, id, W, false);
    key(f, id, LWIN, false);
}

#[test]
fn scope_sticks_with_default_binds() {
    // The cause of "quick Super+Tab does nothing": niri remembers the scope chosen inside the
    // switcher (W: workspace, O: output, S: cycle) for later opens by binds without `scope`. A
    // workspace with a single window then has nothing to switch to.
    let (mut f, id, b, c) = set_up_two_workspaces("");
    tap_super_tab(&mut f, id);
    assert_eq!(focused(&mut f), Some(b));
    tap_super_tab(&mut f, id);
    assert_eq!(focused(&mut f), Some(c));

    super_tab_then_w(&mut f, id);
    assert_eq!(focused(&mut f), Some(c));
    tap_super_tab(&mut f, id);
    assert_eq!(
        focused(&mut f),
        Some(c),
        "upstream behaviour: the workspace scope sticks"
    );
}

#[test]
fn scope_all_binds_reset_the_scope() {
    // What the ashd config does: every open starts with all windows.
    let (mut f, id, b, c) = set_up_two_workspaces(
        r#"
        recent-windows {
            binds {
                Mod+Tab { next-window scope="all"; }
                Mod+Shift+Tab { previous-window scope="all"; }
            }
        }
        "#,
    );
    super_tab_then_w(&mut f, id);
    assert_eq!(focused(&mut f), Some(c));
    tap_super_tab(&mut f, id);
    assert_eq!(focused(&mut f), Some(b));
    tap_super_tab(&mut f, id);
    assert_eq!(focused(&mut f), Some(c));
}
