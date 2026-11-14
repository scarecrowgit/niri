//! ashd: tests for `fullscreen-workspaces`.

use super::*;

fn options(by_index: bool) -> Options {
    Options {
        layout: niri_config::Layout {
            fullscreen_workspaces: true,
            create_workspaces_by_index: by_index,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn window(id: usize) -> Op {
    Op::AddWindow {
        params: TestWindowParams::new(id),
    }
}

fn fullscreen(id: usize) -> Op {
    Op::FullscreenWindow(id)
}

fn monitor_for(layout: &Layout<TestWindow>, output: usize) -> &Monitor<TestWindow> {
    let MonitorSet::Normal { monitors, .. } = &layout.monitor_set else {
        panic!("no outputs");
    };
    let name = format!("output{output}");
    monitors
        .iter()
        .find(|mon| mon.output_name() == &name)
        .unwrap()
}

/// Per workspace of the output: strip columns in order (`1,2`), `+3` for floating windows,
/// prefixed with `F` for a fullscreen workspace and `h` for a hidden one.
fn shape(layout: &Layout<TestWindow>, output: usize) -> Vec<String> {
    monitor_for(layout, output)
        .workspaces
        .iter()
        .map(|ws| {
            let mut s = String::new();
            if ws.fullscreen_slot().is_some() {
                s.push('F');
            }
            if ws.is_hidden() {
                s.push('h');
            }
            let cols: Vec<String> = ws
                .scrolling()
                .columns()
                .map(|col| {
                    col.tiles()
                        .map(|(tile, _)| tile.window().0.id.to_string())
                        .collect::<Vec<_>>()
                        .join("|")
                })
                .collect();
            s.push_str(&cols.join(","));
            let mut floating: Vec<usize> = ws.floating().tiles().map(|t| t.window().0.id).collect();
            floating.sort();
            for id in floating {
                s.push('+');
                s.push_str(&id.to_string());
            }
            s
        })
        .collect()
}

fn active_idx(layout: &Layout<TestWindow>, output: usize) -> usize {
    monitor_for(layout, output).active_workspace_idx
}

fn focused(layout: &Layout<TestWindow>) -> Option<usize> {
    layout.focus().map(|win| win.0.id)
}

fn is_fullscreen(layout: &Layout<TestWindow>, id: usize) -> bool {
    layout
        .windows()
        .find(|(_, win)| win.0.id == id)
        .is_some_and(|(_, win)| win.pending_sizing_mode().is_fullscreen())
}

fn ipc_idx(layout: &Layout<TestWindow>, output: usize) -> Vec<u8> {
    let mon = monitor_for(layout, output);
    (0..mon.workspaces.len())
        .map(|idx| layout.ipc_workspace_idx(Some(mon), idx))
        .collect()
}

#[test]
fn enter_and_leave_from_a_strip() {
    let mut layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            window(1),
            window(2),
            window(3),
            Op::FocusColumnLeft,
        ],
    );
    assert_eq!(focused(&layout), Some(2));

    check_ops_on_layout(&mut layout, [fullscreen(2), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["1,3", "F2", ""]);
    assert_eq!(active_idx(&layout, 1), 1);
    assert_eq!(focused(&layout), Some(2));
    assert!(is_fullscreen(&layout, 2));
    // The fullscreen workspace has no number.
    assert_eq!(ipc_idx(&layout, 1), [1, 0, 2]);
    let mon = monitor_for(&layout, 1);
    let slot = mon.workspaces[1].fullscreen_slot().unwrap();
    assert_eq!(slot.origin, mon.workspaces[0].id());
    assert_eq!(slot.window, Some(2));

    // Back between 1 and 3, the view follows.
    check_ops_on_layout(&mut layout, [fullscreen(2), Op::CompleteAnimations]);
    assert!(!is_fullscreen(&layout, 2));
    assert_eq!(shape(&layout, 1), ["1,2,3", ""]);
    assert_eq!(active_idx(&layout, 1), 0);
    assert_eq!(focused(&layout), Some(2));
}

#[test]
fn only_window_keeps_its_origin_and_numbers() {
    let mut layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            window(1),
            Op::FocusWorkspaceDown,
            window(2),
            Op::FocusWorkspaceUp,
            Op::CompleteAnimations,
            fullscreen(1),
            Op::CompleteAnimations,
        ],
    );
    // The origin (now empty) stays, so workspace 2 is still number 2.
    assert_eq!(shape(&layout, 1), ["", "F1", "2", ""]);
    assert_eq!(ipc_idx(&layout, 1), [1, 0, 2, 3]);
    let idx = layout.resolve_workspace_index(None, 2);
    assert_eq!(idx, 2);

    check_ops_on_layout(&mut layout, [fullscreen(1), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["1", "2", ""]);
}

#[test]
fn up_down_navigation_reaches_the_fullscreen_workspace() {
    let mut layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            window(1),
            window(2),
            fullscreen(2),
            Op::FocusWorkspaceUp,
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(active_idx(&layout, 1), 0);
    assert_eq!(focused(&layout), Some(1));
    check_ops_on_layout(
        &mut layout,
        [Op::FocusWorkspaceDown, Op::CompleteAnimations],
    );
    assert_eq!(active_idx(&layout, 1), 1);
    assert_eq!(focused(&layout), Some(2));
}

#[test]
fn close_while_fullscreen() {
    let mut layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            window(1),
            window(2),
            fullscreen(2),
            Op::CompleteAnimations,
            Op::CloseWindow(2),
            Op::Refresh { is_active: true },
        ],
    );
    // The view goes back to the origin; the empty workspace goes once the switch is done.
    assert_eq!(active_idx(&layout, 1), 0);
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["1", ""]);
    assert_eq!(focused(&layout), Some(1));
}

#[test]
fn close_while_not_shown() {
    let layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            window(1),
            window(2),
            fullscreen(2),
            Op::FocusWorkspaceUp,
            Op::CompleteAnimations,
            Op::CloseWindow(2),
            Op::Refresh { is_active: true },
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", ""]);
    assert_eq!(active_idx(&layout, 1), 0);
}

#[test]
fn unfullscreen_by_the_client_returns_on_refresh() {
    let mut layout = check_ops_with_options(
        options(false),
        [Op::AddOutput(1), window(1), window(2), fullscreen(2)],
    );
    // Like a client unfullscreen request handled outside the layout's own actions: the column
    // state changes, the next refresh moves the window back.
    {
        let MonitorSet::Normal { monitors, .. } = &mut layout.monitor_set else {
            unreachable!()
        };
        monitors[0].workspaces[1].set_fullscreen(&2, false);
    }
    check_ops_on_layout(
        &mut layout,
        [Op::Refresh { is_active: true }, Op::CompleteAnimations],
    );
    assert_eq!(shape(&layout, 1), ["1,2", ""]);
}

#[test]
fn origin_column_gone_returns_next_to_the_focused_column() {
    let mut layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            window(1),
            window(2),
            window(3),
            Op::FocusColumnLeft,
            fullscreen(2),
            Op::CloseWindow(1),
            Op::FocusWorkspaceUp,
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["3", "F2", ""]);
    check_ops_on_layout(&mut layout, [fullscreen(2), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["3,2", ""]);
}

#[test]
fn first_column_returns_first() {
    let mut layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            window(1),
            window(2),
            Op::FocusColumnFirst,
            fullscreen(1),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["2", "F1", ""]);
    check_ops_on_layout(&mut layout, [fullscreen(1), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["1,2", ""]);
}

#[test]
fn origin_moved_to_another_output() {
    let mut layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            Op::AddOutput(2),
            Op::FocusOutput(1),
            window(1),
            window(2),
            Op::FocusWorkspaceDown,
            window(3),
            Op::FocusWorkspaceUp,
            Op::FocusColumnRight,
            fullscreen(2),
            Op::FocusWorkspaceUp,
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "F2", "3", ""]);

    // The origin goes to output 2; the window then returns to the nearest numbered workspace
    // before its fullscreen workspace.
    check_ops_on_layout(
        &mut layout,
        [
            Op::MoveWorkspaceToOutput(2),
            Op::CompleteAnimations,
            Op::FocusOutput(1),
        ],
    );
    let shape1 = shape(&layout, 1);
    assert!(shape1.contains(&String::from("F2")), "{shape1:?}");
    check_ops_on_layout(&mut layout, [fullscreen(2), Op::CompleteAnimations]);
    let shape1 = shape(&layout, 1);
    assert!(!shape1.iter().any(|s| s.starts_with('F')), "{shape1:?}");
    assert!(shape1.iter().any(|s| s.contains('2')), "{shape1:?}");
}

#[test]
fn output_removed() {
    let mut layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            Op::AddOutput(2),
            Op::FocusOutput(2),
            window(1),
            window(2),
            fullscreen(2),
            Op::CompleteAnimations,
            Op::RemoveOutput(2),
            Op::CompleteAnimations,
        ],
    );
    // Both workspaces moved to output 1, in order, still linked.
    assert_eq!(shape(&layout, 1), ["1", "F2", ""]);
    check_ops_on_layout(
        &mut layout,
        [Op::FocusWorkspace(1), fullscreen(2), Op::CompleteAnimations],
    );
    assert_eq!(shape(&layout, 1), ["1,2", ""]);
}

#[test]
fn moving_a_fullscreen_window_by_action() {
    let mut layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            window(1),
            Op::FocusWorkspaceDown,
            window(3),
            Op::FocusWorkspaceUp,
            window(2),
            fullscreen(2),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "F2", "3", ""]);

    // To workspace 2 (numbered): it stays fullscreen and gets a workspace after that one.
    check_ops_on_layout(
        &mut layout,
        [
            Op::WsIndex(ws_by_index::WsIndexOp::MoveWindow {
                window_id: None,
                index: 2,
                focus: true,
            }),
            Op::Refresh { is_active: true },
            Op::CompleteAnimations,
        ],
    );
    assert!(is_fullscreen(&layout, 2));
    assert_eq!(shape(&layout, 1), ["1", "3", "F2", ""]);
    assert_eq!(focused(&layout), Some(2));

    // Back from fullscreen: to workspace 2, next to 3.
    check_ops_on_layout(&mut layout, [fullscreen(2), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["1", "3,2", ""]);
}

#[test]
fn moving_a_fullscreen_window_to_another_output() {
    let layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            Op::AddOutput(2),
            Op::FocusOutput(1),
            window(1),
            window(2),
            fullscreen(2),
            Op::CompleteAnimations,
            Op::MoveWindowToOutput {
                window_id: None,
                output_id: 2,
                target_ws_idx: None,
            },
            Op::Refresh { is_active: true },
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", ""]);
    assert_eq!(shape(&layout, 2), ["", "F2", ""]);
    assert!(is_fullscreen(&layout, 2));
}

#[test]
fn canvas_origin_returns_to_the_plane_position() {
    let mut layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            Op::SetWorkspaceMode(WorkspaceMode::Canvas),
            window(1),
            window(2),
            Op::CompleteAnimations,
        ],
    );
    let plane_before = layout
        .active_workspace()
        .unwrap()
        .canvas_window_plane_rect(&2);
    assert!(plane_before.is_some());

    check_ops_on_layout(&mut layout, [fullscreen(2), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["+1", "F2", ""]);
    let mon = monitor_for(&layout, 1);
    assert!(
        !mon.workspaces[1].is_canvas(),
        "fullscreen workspaces are strips"
    );

    check_ops_on_layout(&mut layout, [fullscreen(2), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["+1+2", ""]);
    let ws = layout.active_workspace().unwrap();
    assert!(ws.is_canvas());
    assert_eq!(
        ws.canvas_window_plane_rect(&2).map(|r| r.loc),
        plane_before.map(|r| r.loc)
    );
}

#[test]
fn floating_window_on_a_strip_returns_floating() {
    let mut layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            window(1),
            Op::AddWindow {
                params: TestWindowParams {
                    is_floating: true,
                    ..TestWindowParams::new(2)
                },
            },
            fullscreen(2),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "F2", ""]);
    check_ops_on_layout(&mut layout, [fullscreen(2), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["1+2", ""]);
}

#[test]
fn two_fullscreen_windows_from_the_same_origin() {
    let mut layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            window(1),
            window(2),
            window(3),
            fullscreen(3),
            Op::FocusWorkspaceUp,
            Op::CompleteAnimations,
            fullscreen(2),
            Op::CompleteAnimations,
        ],
    );
    // The newest fullscreen workspace is nearest to the origin.
    assert_eq!(shape(&layout, 1), ["1", "F2", "F3", ""]);
    assert_eq!(ipc_idx(&layout, 1), [1, 0, 0, 2]);
    assert_eq!(layout.resolve_workspace_index(None, 2), 3);

    check_ops_on_layout(
        &mut layout,
        [
            fullscreen(2),
            Op::CompleteAnimations,
            Op::FocusWorkspaceDown,
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1,2", "F3", ""]);
    check_ops_on_layout(&mut layout, [fullscreen(3), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["1,2,3", ""]);
}

#[test]
fn new_window_opens_on_the_origin() {
    let layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            window(1),
            window(2),
            fullscreen(2),
            Op::CompleteAnimations,
            window(3),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1,3", "F2", ""]);
    assert_eq!(active_idx(&layout, 1), 0);
    assert_eq!(focused(&layout), Some(3));
}

#[test]
fn dialog_stays_and_goes_back_with_its_parent() {
    let mut layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            window(1),
            window(2),
            fullscreen(2),
            Op::CompleteAnimations,
            Op::AddWindowNextTo {
                params: TestWindowParams::new(3),
                next_to_id: 2,
            },
            Op::CompleteAnimations,
        ],
    );
    let shape1 = shape(&layout, 1);
    assert!(
        shape1[1].starts_with('F') && shape1[1].contains('3'),
        "{shape1:?}"
    );
    check_ops_on_layout(
        &mut layout,
        [Op::FullscreenWindow(2), Op::CompleteAnimations],
    );
    let shape1 = shape(&layout, 1);
    assert!(!shape1.iter().any(|s| s.starts_with('F')), "{shape1:?}");
    assert!(
        shape1[0].contains('2') && shape1[0].contains('3'),
        "{shape1:?}"
    );
}

#[test]
fn windowed_fullscreen_is_not_a_workspace() {
    let mut layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            window(1),
            window(2),
            Op::ToggleWindowedFullscreen(2),
        ],
    );
    assert_eq!(shape(&layout, 1), ["1,2", ""]);

    // Real fullscreen to windowed fullscreen goes back.
    check_ops_on_layout(
        &mut layout,
        [
            Op::ToggleWindowedFullscreen(2),
            fullscreen(2),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "F2", ""]);
    check_ops_on_layout(
        &mut layout,
        [Op::ToggleWindowedFullscreen(2), Op::CompleteAnimations],
    );
    assert_eq!(shape(&layout, 1), ["1,2", ""]);
}

#[test]
fn with_create_workspaces_by_index() {
    let mut layout = check_ops_with_options(
        options(true),
        [
            Op::AddOutput(1),
            window(1),
            Op::WsIndex(ws_by_index::WsIndexOp::Focus(3)),
            window(2),
            fullscreen(2),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "", "", "F2", ""]);
    assert_eq!(ipc_idx(&layout, 1), [1, 2, 3, 0, 4]);

    // Focusing workspace 4 skips the fullscreen workspace.
    check_ops_on_layout(
        &mut layout,
        [
            Op::WsIndex(ws_by_index::WsIndexOp::Focus(4)),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(active_idx(&layout, 1), 4);

    check_ops_on_layout(
        &mut layout,
        [
            Op::WsIndex(ws_by_index::WsIndexOp::Focus(1)),
            Op::CompleteAnimations,
            fullscreen(2),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "", "2", ""]);
}

#[test]
fn hidden_workspace_windows_stay() {
    let layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            window(1),
            window(2),
            Op::Scratch(ScratchOp::AddHiddenWorkspace {
                ws_name: 9,
                output_names: vec![],
            }),
            Op::Scratch(ScratchOp::HideWindow { id: Some(2) }),
            Op::SetFullscreenWindow {
                window: 2,
                is_fullscreen: true,
            },
            Op::CompleteAnimations,
        ],
    );
    assert!(!shape(&layout, 1).iter().any(|s| s.starts_with('F')));
}

#[test]
fn turning_the_option_off() {
    let mut layout = check_ops_with_options(
        options(false),
        [
            Op::AddOutput(1),
            window(1),
            window(2),
            fullscreen(2),
            Op::CompleteAnimations,
        ],
    );
    check_ops_on_layout(
        &mut layout,
        [
            Op::UpdateConfig {
                layout_config: Box::default(),
            },
            Op::Refresh { is_active: true },
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "2", ""]);
    check_ops_on_layout(&mut layout, [fullscreen(2), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["1", "2", ""]);
}

#[test]
fn option_off_is_upstream() {
    let layout = check_ops([
        Op::AddOutput(1),
        window(1),
        window(2),
        fullscreen(2),
        Op::Refresh { is_active: true },
        Op::CompleteAnimations,
    ]);
    assert_eq!(shape(&layout, 1), ["1,2", ""]);
}
