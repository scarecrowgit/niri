//! rice: tests for hidden (scratchpad) workspaces.

use super::*;
use crate::layout::canvas::rect_center;

fn window(id: usize) -> Op {
    Op::AddWindow {
        params: TestWindowParams::new(id),
    }
}

fn named(ws_name: usize) -> Op {
    Op::AddNamedWorkspace {
        ws_name,
        output_names: vec![],
        layout_config: None,
    }
}

fn hidden(ws_name: usize) -> Op {
    Op::Scratch(ScratchOp::AddHiddenWorkspace {
        ws_name,
        output_names: vec![],
    })
}

fn monitor(layout: &Layout<TestWindow>) -> &Monitor<TestWindow> {
    let MonitorSet::Normal {
        monitors,
        active_monitor_idx,
        ..
    } = &layout.monitor_set
    else {
        panic!("no outputs");
    };
    &monitors[*active_monitor_idx]
}

fn active_name(layout: &Layout<TestWindow>) -> Option<String> {
    layout.active_workspace().unwrap().name().cloned()
}

fn ws_idx(layout: &Layout<TestWindow>, ws_name: usize) -> usize {
    monitor(layout)
        .find_named_workspace_index(&format!("ws{ws_name}"))
        .unwrap()
}

fn ws_of(layout: &Layout<TestWindow>, id: usize) -> &Workspace<TestWindow> {
    layout
        .workspaces()
        .find(|(_, _, ws)| ws.has_window(&id))
        .map(|(_, _, ws)| ws)
        .unwrap()
}

fn ws_named(layout: &Layout<TestWindow>, ws_name: usize) -> &Workspace<TestWindow> {
    let name = format!("ws{ws_name}");
    layout
        .workspaces()
        .find(|(_, _, ws)| ws.name() == Some(&name))
        .map(|(_, _, ws)| ws)
        .unwrap()
}

fn focused(layout: &Layout<TestWindow>) -> Option<usize> {
    layout.focus().map(|win| win.0.id)
}

/// Workspaces on output 1, top to bottom: ws3, ws2 (hidden), ws1, empty. Focus on ws3.
fn setup() -> Layout<TestWindow> {
    let layout = check_ops([
        Op::AddOutput(1),
        named(1),
        hidden(2),
        named(3),
        Op::FocusWorkspace(0),
        Op::CompleteAnimations,
    ]);
    assert_eq!(active_name(&layout).as_deref(), Some("ws3"));
    assert_eq!(ws_idx(&layout, 2), 1);
    assert!(monitor(&layout).workspaces[1].is_hidden());
    layout
}

#[test]
fn hidden_flag_needs_a_name() {
    let mut layout = setup();
    assert!(ws_named(&layout, 2).is_hidden());
    check_ops_on_layout(&mut layout, [Op::UnnameWorkspace { ws_name: 2 }]);
    assert!(layout.workspaces().all(|(_, _, ws)| !ws.is_hidden()));
}

#[test]
fn up_down_navigation_skips_hidden() {
    let mut layout = setup();

    check_ops_on_layout(&mut layout, [Op::FocusWorkspaceDown]);
    assert_eq!(active_name(&layout).as_deref(), Some("ws1"));

    check_ops_on_layout(&mut layout, [Op::FocusWorkspaceUp]);
    assert_eq!(active_name(&layout).as_deref(), Some("ws3"));

    // Up from the first workspace stays.
    check_ops_on_layout(&mut layout, [Op::FocusWorkspaceUp]);
    assert_eq!(active_name(&layout).as_deref(), Some("ws3"));

    // focus-window-or-workspace goes through the same path.
    check_ops_on_layout(&mut layout, [Op::FocusWindowOrWorkspaceDown]);
    assert_eq!(active_name(&layout).as_deref(), Some("ws1"));
}

#[test]
fn explicit_focus_of_hidden_workspace_is_a_noop() {
    let mut layout = setup();
    let hidden_idx = ws_idx(&layout, 2);

    check_ops_on_layout(&mut layout, [Op::FocusWorkspace(hidden_idx)]);
    assert_eq!(active_name(&layout).as_deref(), Some("ws3"));

    check_ops_on_layout(
        &mut layout,
        [Op::FocusWorkspaceAutoBackAndForth(hidden_idx)],
    );
    assert_eq!(active_name(&layout).as_deref(), Some("ws3"));

    // Previous workspace never points at it.
    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusWorkspaceDown,
            Op::FocusWorkspacePrevious,
            Op::FocusWorkspacePrevious,
        ],
    );
    assert_eq!(active_name(&layout).as_deref(), Some("ws1"));
}

#[test]
fn moving_windows_up_down_skips_hidden() {
    let mut layout = setup();
    check_ops_on_layout(
        &mut layout,
        [window(1), Op::MoveWindowToWorkspaceDown(true)],
    );
    assert_eq!(ws_of(&layout, 1).name().map(String::as_str), Some("ws1"));
    assert_eq!(active_name(&layout).as_deref(), Some("ws1"));

    check_ops_on_layout(&mut layout, [Op::MoveWindowToWorkspaceUp(true)]);
    assert_eq!(ws_of(&layout, 1).name().map(String::as_str), Some("ws3"));

    check_ops_on_layout(
        &mut layout,
        [Op::MoveColumnToWorkspaceDown(true), Op::CompleteAnimations],
    );
    assert_eq!(ws_of(&layout, 1).name().map(String::as_str), Some("ws1"));
    check_ops_on_layout(&mut layout, [Op::MoveColumnToWorkspaceUp(false)]);
    assert_eq!(ws_of(&layout, 1).name().map(String::as_str), Some("ws3"));
}

#[test]
fn moving_workspaces_swaps_with_visible_neighbours() {
    let mut layout = setup();
    check_ops_on_layout(&mut layout, [window(1), Op::MoveWorkspaceDown]);
    let mon = monitor(&layout);
    let names: Vec<_> = mon
        .workspaces
        .iter()
        .map(|ws| ws.name().cloned().unwrap_or_default())
        .collect();
    assert_eq!(names, ["ws1", "ws2", "ws3", ""]);
    assert_eq!(active_name(&layout).as_deref(), Some("ws3"));
}

#[test]
fn hidden_workspace_takes_no_space_on_screen() {
    let mut layout = setup();
    for overview in [false, true] {
        if overview {
            check_ops_on_layout(&mut layout, [Op::ToggleOverview, Op::CompleteAnimations]);
        }
        let mon = monitor(&layout);
        let shown: Vec<_> = mon
            .workspaces_with_render_geo_cull(false)
            .map(|(ws, geo)| (ws.name().cloned(), geo))
            .collect();
        assert!(shown.iter().all(|(name, _)| name.as_deref() != Some("ws2")));
        assert_eq!(shown.len(), 3);

        // ws1 directly follows ws3.
        let ws3 = shown[0].1;
        let ws1 = shown[1].1;
        assert_eq!(shown[1].0.as_deref(), Some("ws1"));
        let gap = ws1.loc.y - (ws3.loc.y + ws3.size.h);
        assert!(gap >= 0. && gap < ws3.size.h * 0.2, "gap {gap}");
    }
}

#[test]
fn workspace_switch_gesture_skips_hidden() {
    let mut layout = setup();
    check_ops_on_layout(
        &mut layout,
        [
            Op::WorkspaceSwitchGestureBegin {
                output_idx: 1,
                is_touchpad: true,
            },
            Op::WorkspaceSwitchGestureUpdate {
                delta: 400.,
                timestamp: Duration::from_millis(10),
                is_touchpad: true,
            },
            Op::WorkspaceSwitchGestureEnd {
                is_touchpad: Some(true),
            },
        ],
    );
    assert_eq!(active_name(&layout).as_deref(), Some("ws1"));
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    assert_eq!(active_name(&layout).as_deref(), Some("ws1"));
}

#[test]
fn activating_hidden_windows_does_not_show_them() {
    let mut layout = setup();
    check_ops_on_layout(
        &mut layout,
        [
            window(1),
            Op::AddWindowToNamedWorkspace {
                params: TestWindowParams::new(2),
                ws_name: 2,
            },
            Op::FocusWindow(2),
        ],
    );
    assert_eq!(active_name(&layout).as_deref(), Some("ws3"));
    assert_eq!(focused(&layout), Some(1));
    assert!(layout.is_window_hidden(&2));
    assert!(!layout.should_trigger_focus_follows_mouse_on(&2));
}

#[test]
fn becoming_hidden_moves_the_active_workspace_away() {
    let mut layout = setup();
    check_ops_on_layout(
        &mut layout,
        [
            window(1),
            Op::Scratch(ScratchOp::SetWorkspaceHidden {
                ws_name: 3,
                hidden: true,
            }),
        ],
    );
    assert_ne!(active_name(&layout).as_deref(), Some("ws3"));
    assert!(layout.is_window_hidden(&1));
    // Its window counts as hidden from now on.
    assert_eq!(layout.hidden_windows()[0].id, 1);

    check_ops_on_layout(
        &mut layout,
        [Op::Scratch(ScratchOp::SetWorkspaceHidden {
            ws_name: 3,
            hidden: false,
        })],
    );
    assert!(!layout.is_window_hidden(&1));
}

#[test]
fn hide_and_unhide_on_strip() {
    let mut layout = setup();
    check_ops_on_layout(
        &mut layout,
        [window(1), window(2), window(3), Op::FocusColumn(2)],
    );
    let origin = layout.active_workspace().unwrap().id();
    assert_eq!(focused(&layout), Some(2));

    check_ops_on_layout(
        &mut layout,
        [Op::Scratch(ScratchOp::HideWindow { id: None })],
    );
    assert!(layout.is_window_hidden(&2));
    assert_eq!(ws_of(&layout, 2).name().map(String::as_str), Some("ws2"));
    assert_eq!(active_name(&layout).as_deref(), Some("ws3"));
    assert_ne!(focused(&layout), Some(2));
    let hidden = layout.hidden_windows();
    assert_eq!(hidden.len(), 1);
    assert_eq!(hidden[0].origin, Some(origin));

    // Hiding a hidden window does nothing.
    check_ops_on_layout(
        &mut layout,
        [Op::Scratch(ScratchOp::HideWindow { id: Some(2) })],
    );
    assert!(layout.is_window_hidden(&2));

    // Back as a new column right of the focused one, focused.
    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusColumn(1),
            Op::Scratch(ScratchOp::UnhideWindow { id: None }),
        ],
    );
    assert!(!layout.is_window_hidden(&2));
    assert_eq!(focused(&layout), Some(2));
    let ws = layout.active_workspace().unwrap();
    assert_eq!(ws.id(), origin);
    let order: Vec<_> = ws
        .scrolling()
        .columns()
        .map(|col| col.tiles().next().unwrap().0.window().0.id)
        .collect();
    assert_eq!(order, [1, 2, 3]);
    assert!(layout.hidden_windows().is_empty());
}

#[test]
fn unhide_goes_to_the_focused_workspace_without_origin() {
    let mut layout = setup();
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddWindowToNamedWorkspace {
                params: TestWindowParams::new(1),
                ws_name: 2,
            },
            Op::FocusWorkspaceDown,
            Op::Scratch(ScratchOp::UnhideWindow { id: Some(1) }),
        ],
    );
    assert_eq!(ws_of(&layout, 1).name().map(String::as_str), Some("ws1"));
    assert_eq!(focused(&layout), Some(1));
}

#[test]
fn unhide_falls_back_when_the_origin_is_gone() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        hidden(2),
        window(1),
        Op::Scratch(ScratchOp::HideWindow { id: None }),
    ]);
    // The unnamed origin workspace was cleaned up when it became empty... unless it was active;
    // go elsewhere so that it disappears.
    check_ops_on_layout(
        &mut layout,
        [named(5), Op::FocusWorkspace(0), Op::CompleteAnimations],
    );
    assert_eq!(layout.hidden_windows()[0].origin, None);
    check_ops_on_layout(
        &mut layout,
        [Op::Scratch(ScratchOp::UnhideWindow { id: None })],
    );
    assert_eq!(ws_of(&layout, 1).name().map(String::as_str), Some("ws5"));
}

#[test]
fn unhide_most_recent_first() {
    let mut layout = setup();
    check_ops_on_layout(
        &mut layout,
        [
            window(1),
            window(2),
            window(3),
            Op::Scratch(ScratchOp::HideWindow { id: Some(2) }),
            Op::Scratch(ScratchOp::HideWindow { id: Some(1) }),
            Op::Scratch(ScratchOp::HideWindow { id: Some(3) }),
        ],
    );
    let order: Vec<_> = layout.hidden_windows().into_iter().map(|w| w.id).collect();
    assert_eq!(order, [3, 1, 2]);

    check_ops_on_layout(
        &mut layout,
        [Op::Scratch(ScratchOp::UnhideWindow { id: None })],
    );
    assert_eq!(focused(&layout), Some(3));
    check_ops_on_layout(
        &mut layout,
        [Op::Scratch(ScratchOp::UnhideWindow { id: None })],
    );
    assert_eq!(focused(&layout), Some(1));
    // A specific window regardless of order.
    check_ops_on_layout(
        &mut layout,
        [Op::Scratch(ScratchOp::UnhideWindow { id: Some(2) })],
    );
    assert_eq!(focused(&layout), Some(2));
    // Nothing left.
    check_ops_on_layout(
        &mut layout,
        [Op::Scratch(ScratchOp::UnhideWindow { id: None })],
    );
    assert_eq!(focused(&layout), Some(2));
}

#[test]
fn unhide_to_canvas_restores_plane_position() {
    let mut layout = setup();
    check_ops_on_layout(
        &mut layout,
        [
            window(1),
            window(2),
            Op::Communicate(1),
            Op::Communicate(2),
            Op::ToggleWorkspaceMode,
            Op::CompleteAnimations,
            Op::MoveFloatingWindow {
                id: Some(2),
                x: PositionChange::AdjustFixed(-300.),
                y: PositionChange::AdjustFixed(200.),
                animate: false,
            },
        ],
    );
    let canvas = layout.active_workspace().unwrap().id();
    let before = ws_of(&layout, 2).canvas_window_plane_rect(&2).unwrap();

    check_ops_on_layout(
        &mut layout,
        [
            Op::Scratch(ScratchOp::HideWindow { id: Some(2) }),
            // The view moves meanwhile.
            Op::CanvasPanTo { x: 5000., y: -800. },
            Op::CompleteAnimations,
            Op::Scratch(ScratchOp::UnhideWindow { id: None }),
            Op::CompleteAnimations,
        ],
    );
    let ws = ws_of(&layout, 2);
    assert_eq!(ws.id(), canvas);
    let after = ws.canvas_window_plane_rect(&2).unwrap();
    assert_eq!(after.loc, before.loc);
    assert_eq!(focused(&layout), Some(2));

    // It is in view again.
    let t = ws.canvas_target_transform().unwrap();
    let view = t.plane_rect_to_view(after);
    assert!(view.overlaps(ws.working_area()));
}

#[test]
fn generic_moves_through_the_scratchpad_restore_plane_position() {
    let mut layout = setup();
    check_ops_on_layout(
        &mut layout,
        [
            window(1),
            window(2),
            Op::ToggleWorkspaceMode,
            Op::CompleteAnimations,
            Op::MoveFloatingWindow {
                id: Some(1),
                x: PositionChange::AdjustFixed(700.),
                y: PositionChange::AdjustFixed(-50.),
                animate: false,
            },
        ],
    );
    let canvas_idx = monitor(&layout).active_workspace_idx();
    let before = ws_of(&layout, 1).canvas_window_plane_rect(&1).unwrap();
    let hidden_idx = ws_idx(&layout, 2);

    // move-window-to-workspace --window-id 1 scratch --focus false, then back by index.
    check_ops_on_layout(
        &mut layout,
        [
            Op::MoveWindowToWorkspace {
                window_id: Some(1),
                workspace_idx: hidden_idx,
            },
            Op::CanvasPanTo { x: -3000., y: 0. },
            Op::CompleteAnimations,
        ],
    );
    assert!(layout.is_window_hidden(&1));
    let canvas_id = monitor(&layout).workspaces[canvas_idx].id();
    assert_eq!(layout.hidden_windows()[0].origin, Some(canvas_id));

    check_ops_on_layout(
        &mut layout,
        [
            Op::MoveWindowToWorkspace {
                window_id: Some(1),
                workspace_idx: canvas_idx,
            },
            Op::FocusWindow(1),
            Op::CompleteAnimations,
        ],
    );
    let after = ws_of(&layout, 1).canvas_window_plane_rect(&1).unwrap();
    assert_eq!(after.loc, before.loc);
    assert_eq!(focused(&layout), Some(1));
}

#[test]
fn scratchpad_to_another_canvas_lands_in_the_viewport_centre() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        hidden(2),
        window(1),
        Op::Scratch(ScratchOp::HideWindow { id: None }),
        Op::FocusWorkspaceDown,
        Op::ToggleWorkspaceMode,
        Op::CanvasPanTo {
            x: 10000.,
            y: 10000.,
        },
        Op::CompleteAnimations,
        Op::Scratch(ScratchOp::UnhideWindow { id: None }),
        Op::CompleteAnimations,
    ]);
    let ws = ws_of(&layout, 1);
    assert!(ws.is_canvas());
    let t = ws.canvas_target_transform().unwrap();
    let center = rect_center(t.plane_rect_to_view(ws.canvas_window_plane_rect(&1).unwrap()));
    let wa_center = rect_center(ws.working_area());
    assert!((center.x - wa_center.x).abs() < 1.);
    assert!((center.y - wa_center.y).abs() < 1.);
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
}

#[test]
fn hide_uses_another_monitor_without_a_local_hidden_workspace() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::Scratch(ScratchOp::AddHiddenWorkspace {
            ws_name: 2,
            output_names: vec![2],
        }),
        Op::FocusOutput(1),
        window(1),
        Op::Scratch(ScratchOp::HideWindow { id: None }),
    ]);
    assert!(layout.is_window_hidden(&1));
    assert_eq!(ws_of(&layout, 1).name().map(String::as_str), Some("ws2"));

    // Back on output 1 where it came from.
    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusOutput(2),
            Op::Scratch(ScratchOp::UnhideWindow { id: None }),
        ],
    );
    assert!(!layout.is_window_hidden(&1));
    assert_eq!(layout.active_output().unwrap().name(), "output1");
    assert_eq!(focused(&layout), Some(1));
}

#[test]
fn closing_hidden_windows_and_removing_outputs() {
    check_ops([
        Op::AddOutput(1),
        hidden(2),
        window(1),
        window(2),
        Op::Scratch(ScratchOp::HideWindow { id: Some(1) }),
        Op::Scratch(ScratchOp::HideWindow { id: Some(2) }),
        Op::CloseWindow(1),
        Op::RemoveOutput(1),
        Op::Scratch(ScratchOp::UnhideWindow { id: None }),
        Op::AddOutput(1),
        Op::Scratch(ScratchOp::UnhideWindow { id: None }),
    ]);
}

#[test]
fn unnaming_clears_the_hidden_flag() {
    // Found by the randomized tests: a formerly hidden workspace that became active after being
    // unnamed must not turn hidden again when it gets a new name.
    let layout = check_ops([
        hidden(2),
        Op::AddOutput(1),
        named(3),
        Op::FocusWindowOrWorkspaceUp,
        Op::UnnameWorkspace { ws_name: 2 },
        Op::FocusWorkspaceAutoBackAndForth(0),
        Op::FocusWindowOrWorkspaceUp,
        Op::SetWorkspaceName {
            new_ws_name: 1,
            ws_name: None,
        },
    ]);
    assert!(layout.workspaces().all(|(_, _, ws)| !ws.is_hidden()));
}

#[test]
fn hiding_during_a_workspace_switch_cleans_up() {
    // Found by the randomized tests: a workspace becoming hidden shifts the visual indices of an
    // ongoing switch animation; the switch is dropped and empty workspaces cleaned up.
    check_ops([
        Op::AddOutput(1),
        named(5),
        Op::MoveWorkspaceUp,
        Op::FocusWorkspace(2),
        Op::Scratch(ScratchOp::SetWorkspaceHidden {
            ws_name: 5,
            hidden: true,
        }),
    ]);
}

fn empty_workspace_above_first() -> Options {
    Options {
        layout: niri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn unnaming_during_a_workspace_switch() {
    // Found by the randomized tests: unnaming shifts the visual indices of an ongoing switch.
    check_ops_with_options(
        empty_workspace_above_first(),
        [
            hidden(5),
            Op::AddOutput(1),
            Op::FocusWindowOrWorkspaceDown,
            Op::UnnameWorkspace { ws_name: 5 },
        ],
    );
}

#[test]
fn moving_a_hidden_workspace_to_the_top() {
    // Found by the randomized tests: moving workspaces around must not make a hidden one active.
    check_ops_with_options(
        empty_workspace_above_first(),
        [
            Op::Scratch(ScratchOp::AddHiddenWorkspace {
                ws_name: 3,
                output_names: vec![4],
            }),
            Op::AddOutput(1),
            Op::AddOutput(1),
            Op::MoveWorkspaceToIndex {
                ws_name: Some(3),
                target_idx: 0,
            },
        ],
    );
}
