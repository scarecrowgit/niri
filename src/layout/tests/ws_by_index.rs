//! ashd: tests for `create-workspaces-by-index`.

use super::*;

/// Index references the way the input code resolves them: 1-based over the visible workspaces
/// of the active monitor (or of the window's monitor).
#[derive(Debug, Clone, Arbitrary)]
pub(super) enum WsIndexOp {
    Focus(#[proptest(strategy = "1..=7u8")] u8),
    FocusAutoBackAndForth(#[proptest(strategy = "1..=7u8")] u8),
    MoveWindow {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        window_id: Option<usize>,
        #[proptest(strategy = "1..=7u8")]
        index: u8,
        focus: bool,
    },
    MoveColumn(#[proptest(strategy = "1..=7u8")] u8, bool),
}

impl WsIndexOp {
    pub(super) fn apply(self, layout: &mut Layout<TestWindow>) {
        match self {
            WsIndexOp::Focus(index) => {
                let idx = layout.resolve_workspace_index(None, index);
                layout.switch_workspace(idx);
            }
            WsIndexOp::FocusAutoBackAndForth(index) => {
                let idx = layout.resolve_workspace_index(None, index);
                layout.switch_workspace_auto_back_and_forth(idx);
            }
            WsIndexOp::MoveWindow {
                window_id,
                index,
                focus,
            } => {
                let window_id = window_id.filter(|id| layout.has_window(id));
                let idx = layout.resolve_workspace_index(window_id.as_ref(), index);
                let activate = if focus {
                    ActivateWindow::Smart
                } else {
                    ActivateWindow::No
                };
                layout.move_to_workspace(window_id.as_ref(), idx, activate);
            }
            WsIndexOp::MoveColumn(index, focus) => {
                let idx = layout.resolve_workspace_index(None, index);
                layout.move_column_to_workspace(idx, focus);
            }
        }
    }
}

fn by_index() -> Options {
    Options {
        layout: niri_config::Layout {
            create_workspaces_by_index: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn by_index_part(on: bool) -> Box<niri_config::LayoutPart> {
    Box::new(niri_config::LayoutPart {
        create_workspaces_by_index: Some(Flag(on)),
        ..Default::default()
    })
}

fn window(id: usize) -> Op {
    Op::AddWindow {
        params: TestWindowParams::new(id),
    }
}

fn focus(index: u8) -> Op {
    Op::WsIndex(WsIndexOp::Focus(index))
}

fn hidden(ws_name: usize) -> Op {
    Op::Scratch(ScratchOp::AddHiddenWorkspace {
        ws_name,
        output_names: vec![],
    })
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

/// Per workspace of the output, top to bottom: the window ids, `h` for a hidden workspace.
fn shape(layout: &Layout<TestWindow>, output: usize) -> Vec<String> {
    monitor_for(layout, output)
        .workspaces
        .iter()
        .map(|ws| {
            let mut ids: Vec<_> = ws.windows().map(|win| win.0.id.to_string()).collect();
            ids.sort();
            let ids = ids.join(",");
            if ws.is_hidden() {
                format!("h{ids}")
            } else {
                ids
            }
        })
        .collect()
}

fn active_idx(layout: &Layout<TestWindow>, output: usize) -> usize {
    monitor_for(layout, output).active_workspace_idx
}

fn ws_idx_of(layout: &Layout<TestWindow>, output: usize, id: usize) -> usize {
    monitor_for(layout, output)
        .workspaces
        .iter()
        .position(|ws| ws.has_window(&id))
        .unwrap()
}

#[test]
fn focus_past_the_end_creates_workspaces() {
    let layout = check_ops_with_options(
        by_index(),
        [
            Op::AddOutput(1),
            window(1),
            focus(5),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "", "", "", ""]);
    assert_eq!(active_idx(&layout, 1), 4);
    let mon = monitor_for(&layout, 1);
    assert_eq!(layout.ipc_workspace_idx(Some(mon), 4), 5);

    // A window opened there lands on workspace 5; a new empty one follows.
    let mut layout = layout;
    check_ops_on_layout(&mut layout, [window(2), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["1", "", "", "", "2", ""]);
    assert_eq!(active_idx(&layout, 1), 4);
}

#[test]
fn focus_past_the_end_without_the_option_clamps() {
    let layout = check_ops([
        Op::AddOutput(1),
        window(1),
        focus(5),
        Op::CompleteAnimations,
    ]);
    assert_eq!(shape(&layout, 1), ["1", ""]);
    assert_eq!(active_idx(&layout, 1), 1);
}

#[test]
fn empty_workspace_in_the_middle_survives() {
    let layout = check_ops_with_options(
        by_index(),
        [
            Op::AddOutput(1),
            window(1),
            focus(3),
            window(2),
            focus(1),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "", "2", ""]);
    assert_eq!(active_idx(&layout, 1), 0);

    // Focusing the empty workspace in the middle and leaving it again keeps it.
    let mut layout = layout;
    check_ops_on_layout(
        &mut layout,
        [
            focus(2),
            Op::CompleteAnimations,
            focus(1),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "", "2", ""]);

    // Closing the window on workspace 1 keeps workspace 1 (in use: it is focused, and 3 has a
    // window), and the numbers of the others don't change.
    check_ops_on_layout(&mut layout, [Op::CloseWindow(1), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["", "", "2", ""]);
    check_ops_on_layout(&mut layout, [focus(3), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["", "", "2", ""]);
    assert_eq!(active_idx(&layout, 1), 2);
}

#[test]
fn trailing_workspaces_are_trimmed_when_the_last_window_closes() {
    let mut layout = check_ops_with_options(
        by_index(),
        [
            Op::AddOutput(1),
            window(1),
            focus(4),
            window(2),
            focus(1),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "", "", "2", ""]);

    // The highest window closes while another workspace is focused: back to one empty
    // workspace at the end.
    check_ops_on_layout(&mut layout, [Op::CloseWindow(2), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["1", ""]);
    assert_eq!(active_idx(&layout, 1), 0);
}

#[test]
fn focused_empty_workspace_keeps_the_ones_before_it() {
    let mut layout = check_ops_with_options(
        by_index(),
        [
            Op::AddOutput(1),
            window(1),
            focus(4),
            window(2),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "", "", "2", ""]);

    // Workspace 4 stays focused after its window closes, so 2 and 3 stay too.
    check_ops_on_layout(&mut layout, [Op::CloseWindow(2), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["1", "", "", "", ""]);
    assert_eq!(active_idx(&layout, 1), 3);

    // Leaving it trims everything after workspace 1.
    check_ops_on_layout(&mut layout, [focus(1), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["1", ""]);

    // Focusing a lower empty workspace trims the higher empty ones.
    check_ops_on_layout(
        &mut layout,
        [
            focus(5),
            Op::CompleteAnimations,
            focus(3),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "", "", ""]);
    assert_eq!(active_idx(&layout, 1), 2);
}

#[test]
fn move_window_and_column_past_the_end() {
    let mut layout = check_ops_with_options(
        by_index(),
        [
            Op::AddOutput(1),
            window(1),
            window(2),
            Op::WsIndex(WsIndexOp::MoveWindow {
                window_id: None,
                index: 4,
                focus: false,
            }),
            Op::CompleteAnimations,
        ],
    );
    // Window 2 (focused) moved to workspace 4, focus stays on workspace 1.
    assert_eq!(shape(&layout, 1), ["1", "", "", "2", ""]);
    assert_eq!(active_idx(&layout, 1), 0);

    check_ops_on_layout(
        &mut layout,
        [
            Op::WsIndex(WsIndexOp::MoveColumn(6, true)),
            Op::CompleteAnimations,
        ],
    );
    // Workspace 1 is now empty but stays: later workspaces are in use.
    assert_eq!(shape(&layout, 1), ["", "", "", "2", "", "1", ""]);
    assert_eq!(active_idx(&layout, 1), 5);
}

#[test]
fn nothing_to_move_creates_nothing() {
    let layout = check_ops_with_options(
        by_index(),
        [
            Op::AddOutput(1),
            Op::WsIndex(WsIndexOp::MoveWindow {
                window_id: None,
                index: 4,
                focus: true,
            }),
            Op::WsIndex(WsIndexOp::MoveColumn(5, true)),
        ],
    );
    assert_eq!(shape(&layout, 1), [""]);
}

#[test]
fn auto_back_and_forth_creates_workspaces() {
    let layout = check_ops_with_options(
        by_index(),
        [
            Op::AddOutput(1),
            window(1),
            Op::WsIndex(WsIndexOp::FocusAutoBackAndForth(3)),
            Op::CompleteAnimations,
            window(2),
            // Same workspace again: back to workspace 1.
            Op::WsIndex(WsIndexOp::FocusAutoBackAndForth(3)),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "", "2", ""]);
    assert_eq!(active_idx(&layout, 1), 0);
}

#[test]
fn hidden_workspace_does_not_count() {
    // Named workspaces are inserted at the top: scratch (hidden), then the unnamed ones.
    let mut layout = check_ops_with_options(
        by_index(),
        [
            Op::AddOutput(1),
            hidden(9),
            window(1),
            focus(3),
            window(2),
            focus(1),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["h", "1", "", "2", ""]);
    let mon = monitor_for(&layout, 1);
    assert_eq!(layout.ipc_workspace_idx(Some(mon), 0), 0);
    assert_eq!(layout.ipc_workspace_idx(Some(mon), 3), 3);

    // Hiding the window of workspace 3 empties it: 2 and 3 are trimmed, the hidden one stays.
    check_ops_on_layout(
        &mut layout,
        [
            Op::Scratch(ScratchOp::HideWindow { id: Some(2) }),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["h2", "1", ""]);

    // Unhiding returns it to the focused workspace (its own one is gone).
    check_ops_on_layout(
        &mut layout,
        [
            focus(4),
            Op::CompleteAnimations,
            Op::Scratch(ScratchOp::UnhideWindow { id: Some(2) }),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["h", "1", "", "", "2", ""]);
}

#[test]
fn hidden_workspace_does_not_keep_empty_ones_before_it() {
    let mut layout = check_ops_with_options(
        by_index(),
        [
            Op::AddOutput(1),
            window(1),
            focus(3),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "", ""]);

    // Put a hidden workspace after the empty one in the middle; it doesn't count as in use.
    check_ops_on_layout(
        &mut layout,
        [
            hidden(9),
            Op::MoveWorkspaceToIndex {
                ws_name: Some(9),
                target_idx: 2,
            },
            Op::CompleteAnimations,
        ],
    );
    assert!(shape(&layout, 1).contains(&String::from("h")));
    check_ops_on_layout(&mut layout, [focus(1), Op::CompleteAnimations]);
    let shape = shape(&layout, 1);
    assert_eq!(
        shape.iter().filter(|s| s.is_empty()).count(),
        1,
        "{shape:?}"
    );
    assert_eq!(shape.last().unwrap(), "");
}

#[test]
fn move_window_by_id_on_another_monitor() {
    let mut layout = check_ops_with_options(
        by_index(),
        [
            Op::AddOutput(1),
            Op::AddOutput(2),
            Op::FocusOutput(2),
            window(2),
            Op::FocusOutput(1),
            window(1),
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", ""]);
    assert_eq!(shape(&layout, 2), ["2", ""]);

    // The index counts on the window's monitor; only that monitor gets new workspaces.
    check_ops_on_layout(
        &mut layout,
        [
            Op::WsIndex(WsIndexOp::MoveWindow {
                window_id: Some(2),
                index: 4,
                focus: false,
            }),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", ""]);
    assert_eq!(ws_idx_of(&layout, 2, 2), 3);
    // Output 2's active workspace (1) stays, so it and the new ones before 4 are kept.
    assert_eq!(shape(&layout, 2), ["", "", "", "2", ""]);
    assert_eq!(active_idx(&layout, 2), 0);

    // focus-workspace only touches the active monitor.
    check_ops_on_layout(&mut layout, [focus(3), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["1", "", ""]);
    assert_eq!(shape(&layout, 2), ["", "", "", "2", ""]);
}

#[test]
fn removing_an_output_keeps_only_used_workspaces() {
    let mut layout = check_ops_with_options(
        by_index(),
        [
            Op::AddOutput(1),
            Op::AddOutput(2),
            Op::FocusOutput(2),
            window(2),
            focus(4),
            window(3),
            Op::FocusOutput(1),
            window(1),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 2), ["2", "", "", "3", ""]);

    // The empty workspaces don't move to the other monitor (like upstream).
    check_ops_on_layout(&mut layout, [Op::RemoveOutput(2), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["1", "2", "3", ""]);
}

#[test]
fn turning_the_option_off_removes_empty_workspaces() {
    let mut layout = check_ops_with_options(
        by_index(),
        [
            Op::AddOutput(1),
            window(1),
            focus(3),
            window(2),
            focus(1),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "", "2", ""]);

    check_ops_on_layout(
        &mut layout,
        [Op::UpdateConfig {
            layout_config: by_index_part(false),
        }],
    );
    assert_eq!(shape(&layout, 1), ["1", "2", ""]);

    check_ops_on_layout(
        &mut layout,
        [
            Op::UpdateConfig {
                layout_config: by_index_part(true),
            },
            focus(4),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 1), ["1", "2", "", ""]);
}

#[test]
fn per_output_option() {
    let mut layout = check_ops([
        Op::AddScaledOutput {
            id: 1,
            scale: 1.,
            layout_config: Some(by_index_part(true)),
        },
        Op::AddOutput(2),
        window(1),
        focus(4),
        Op::CompleteAnimations,
    ]);
    assert_eq!(shape(&layout, 1), ["1", "", "", ""]);

    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusOutput(2),
            window(2),
            focus(4),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(shape(&layout, 2), ["2", ""]);
}

#[test]
fn with_empty_workspace_above_first() {
    let options = Options {
        layout: niri_config::Layout {
            create_workspaces_by_index: true,
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut layout = check_ops_with_options(
        options,
        [Op::AddOutput(1), window(1), Op::CompleteAnimations],
    );
    assert_eq!(shape(&layout, 1), ["", "1", ""]);

    // Index 5 counts the empty workspace above the first one, like every index reference.
    check_ops_on_layout(
        &mut layout,
        [focus(5), window(2), focus(2), Op::CompleteAnimations],
    );
    assert_eq!(shape(&layout, 1), ["", "1", "", "", "2", ""]);

    check_ops_on_layout(&mut layout, [Op::CloseWindow(2), Op::CompleteAnimations]);
    assert_eq!(shape(&layout, 1), ["", "1", ""]);
}
