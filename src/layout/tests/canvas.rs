//! rice: tests for the infinite canvas workspace mode.

use approx::assert_abs_diff_eq;

use super::*;
use crate::layout::canvas::{rect_center, CanvasTransform};

fn ws(layout: &Layout<TestWindow>) -> &Workspace<TestWindow> {
    layout.active_workspace().unwrap()
}

fn transform(layout: &Layout<TestWindow>) -> CanvasTransform {
    ws(layout).canvas_transform().unwrap()
}

fn plane_rect(layout: &Layout<TestWindow>, id: usize) -> Rectangle<f64, Logical> {
    ws(layout).canvas_window_plane_rect(&id).unwrap()
}

/// View rectangle of a canvas window (target position, current transform).
fn view_rect(layout: &Layout<TestWindow>, id: usize) -> Rectangle<f64, Logical> {
    transform(layout).plane_rect_to_view(plane_rect(layout, id))
}

fn working_area(layout: &Layout<TestWindow>) -> Rectangle<f64, Logical> {
    ws(layout).working_area()
}

fn window(id: usize) -> Op {
    Op::AddWindow {
        params: TestWindowParams::new(id),
    }
}

fn settle() -> [Op; 2] {
    [Op::Communicate(1), Op::CompleteAnimations]
}

/// Strip workspace with columns [1] [2] [3, 4 (tabbed)] [5 (fixed width)].
fn strip_setup() -> Vec<Op> {
    vec![
        Op::AddOutput(1),
        window(1),
        window(2),
        window(3),
        window(4),
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::ToggleColumnTabbedDisplay,
        window(5),
        Op::SetColumnWidth(SizeChange::SetFixed(400)),
        Op::FocusColumnLeft,
        Op::Communicate(1),
        Op::Communicate(2),
        Op::Communicate(3),
        Op::Communicate(4),
        Op::Communicate(5),
        Op::CompleteAnimations,
    ]
}

#[test]
fn strip_canvas_strip_preserves_columns() {
    let mut layout = check_ops(strip_setup());
    let before = ws(&layout).scrolling().snapshot_columns();
    let active_before = ws(&layout).active_window().unwrap().0.id;
    assert_eq!(before.0.len(), 4);

    check_ops_on_layout(
        &mut layout,
        [Op::ToggleWorkspaceMode, Op::CompleteAnimations],
    );
    assert_eq!(ws(&layout).mode(), WorkspaceMode::Canvas);
    assert!(ws(&layout).scrolling().is_empty());
    assert_eq!(ws(&layout).floating().tiles().count(), 5);
    assert_abs_diff_eq!(transform(&layout).zoom, 0.62);
    assert_eq!(ws(&layout).active_window().unwrap().0.id, active_before);

    check_ops_on_layout(
        &mut layout,
        [Op::ToggleWorkspaceMode, Op::CompleteAnimations],
    );
    assert_eq!(ws(&layout).mode(), WorkspaceMode::Strip);
    assert!(ws(&layout).floating().is_empty());
    let after = ws(&layout).scrolling().snapshot_columns();
    assert_eq!(
        before, after,
        "column order, widths and stacking must survive"
    );
    assert_eq!(ws(&layout).active_window().unwrap().0.id, active_before);
}

#[test]
fn entering_canvas_keeps_on_screen_positions() {
    let mut layout = check_ops(strip_setup());
    let before: Vec<(usize, Point<f64, Logical>)> = ws(&layout)
        .tiles_with_render_positions()
        .filter(|(_, _, visible)| *visible)
        .map(|(tile, pos, _)| (tile.window().0.id, pos))
        .collect();

    // No animation progress yet: the view must show exactly what the strip showed.
    check_ops_on_layout(&mut layout, [Op::ToggleWorkspaceMode]);
    assert_eq!(transform(&layout), CanvasTransform::IDENTITY);
    for (id, pos) in before {
        let rect = plane_rect(&layout, id);
        assert_abs_diff_eq!(rect.loc.x, pos.x, epsilon = 1e-6);
        assert_abs_diff_eq!(rect.loc.y, pos.y, epsilon = 1e-6);
    }

    // The view then settles at the default zoom, centred on the focused window.
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    let focused = ws(&layout).active_window().unwrap().0.id;
    let center = rect_center(view_rect(&layout, focused));
    let wa_center = rect_center(working_area(&layout));
    assert_abs_diff_eq!(center.x, wa_center.x, epsilon = 1e-6);
    assert_abs_diff_eq!(center.y, wa_center.y, epsilon = 1e-6);
}

#[test]
fn canvas_round_trip_restores_canvas_layout() {
    let mut layout = check_ops(strip_setup());
    check_ops_on_layout(
        &mut layout,
        [
            Op::ToggleWorkspaceMode,
            Op::CompleteAnimations,
            Op::MoveFloatingWindow {
                id: Some(2),
                x: PositionChange::AdjustFixed(-700.),
                y: PositionChange::AdjustFixed(300.),
                animate: false,
            },
            Op::CanvasZoomOut { anchor: None },
            Op::CompleteAnimations,
        ],
    );
    let plane_2 = plane_rect(&layout, 2).loc;
    let zoom = transform(&layout).zoom;
    assert_abs_diff_eq!(zoom, 0.5);

    check_ops_on_layout(
        &mut layout,
        [
            Op::ToggleWorkspaceMode,
            Op::CompleteAnimations,
            Op::ToggleWorkspaceMode,
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(plane_rect(&layout, 2).loc, plane_2);
    assert_abs_diff_eq!(transform(&layout).zoom, zoom);
}

#[test]
fn strip_floating_windows_stay_floating() {
    let mut params = TestWindowParams::new(2);
    params.is_floating = true;
    let mut layout = check_ops([
        Op::AddOutput(1),
        window(1),
        Op::AddWindow { params },
        Op::ToggleWorkspaceMode,
        Op::CompleteAnimations,
        // Toggling floating on the canvas does nothing.
        Op::ToggleWindowFloating { id: Some(1) },
        Op::ToggleWindowFloating { id: Some(2) },
        Op::ToggleWorkspaceMode,
        Op::CompleteAnimations,
    ]);
    assert!(!ws(&layout).is_floating(&1));
    assert!(ws(&layout).is_floating(&2));

    // A second round trip too.
    check_ops_on_layout(
        &mut layout,
        [Op::ToggleWorkspaceMode, Op::ToggleWorkspaceMode],
    );
    assert!(!ws(&layout).is_floating(&1));
    assert!(ws(&layout).is_floating(&2));
}

#[test]
fn new_windows_open_centred_with_cascade() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::SetWorkspaceMode(WorkspaceMode::Canvas),
        Op::CompleteAnimations,
        window(1),
        window(2),
        window(3),
    ]);
    assert_eq!(ws(&layout).mode(), WorkspaceMode::Canvas);
    let wa_center = rect_center(working_area(&layout));

    let c1 = rect_center(view_rect(&layout, 1));
    assert_abs_diff_eq!(c1.x, wa_center.x, epsilon = 1.);
    assert_abs_diff_eq!(c1.y, wa_center.y, epsilon = 1.);

    let p1 = rect_center(plane_rect(&layout, 1));
    let p2 = rect_center(plane_rect(&layout, 2));
    let p3 = rect_center(plane_rect(&layout, 3));
    assert_abs_diff_eq!(p2.x - p1.x, 32., epsilon = 1e-6);
    assert_abs_diff_eq!(p2.y - p1.y, 32., epsilon = 1e-6);
    assert_abs_diff_eq!(p3.x - p2.x, 32., epsilon = 1e-6);
    assert_eq!(ws(&layout).active_window().unwrap().0.id, 3);
    assert!(ws(&layout).scrolling().is_empty());
}

#[test]
fn fullscreen_returns_to_plane_position() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        window(1),
        window(2),
        Op::ToggleWorkspaceMode,
        Op::CompleteAnimations,
        Op::CanvasPan(CanvasPanDirection::Right),
        Op::CompleteAnimations,
    ]);
    let before = plane_rect(&layout, 1);

    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusWindow(1),
            Op::FullscreenWindow(1),
            Op::Communicate(1),
            Op::CompleteAnimations,
        ],
    );
    assert!(!ws(&layout).is_floating(&1));
    assert!(ws(&layout)
        .scrolling()
        .tiles()
        .any(|t| t.window().0.id == 1));

    // The fullscreen window renders unscaled at the output origin.
    let (_, pos, _) = ws(&layout)
        .tiles_with_render_positions()
        .find(|(tile, _, _)| tile.window().0.id == 1)
        .unwrap();
    assert_eq!(pos, Point::from((0., 0.)));

    check_ops_on_layout(
        &mut layout,
        [
            Op::FullscreenWindow(1),
            Op::Communicate(1),
            Op::CompleteAnimations,
        ],
    );
    assert!(ws(&layout).is_floating(&1));
    assert_eq!(plane_rect(&layout, 1).loc, before.loc);
}

#[test]
fn pan_zoom_home_and_clamping() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        window(1),
        Op::ToggleWorkspaceMode,
        Op::CompleteAnimations,
    ]);
    let t0 = transform(&layout);
    assert_abs_diff_eq!(t0.zoom, 0.62);

    check_ops_on_layout(
        &mut layout,
        [
            Op::CanvasPan(CanvasPanDirection::Right),
            Op::CompleteAnimations,
        ],
    );
    let t1 = transform(&layout);
    assert_abs_diff_eq!(t1.offset.x - t0.offset.x, 120. / 0.62, epsilon = 1e-9);
    assert_abs_diff_eq!(t1.offset.y, t0.offset.y, epsilon = 1e-9);

    check_ops_on_layout(
        &mut layout,
        [
            Op::CanvasPan(CanvasPanDirection::Up),
            Op::CompleteAnimations,
        ],
    );
    assert_abs_diff_eq!(
        transform(&layout).offset.y - t0.offset.y,
        -120. / 0.62,
        epsilon = 1e-9
    );

    check_ops_on_layout(&mut layout, [Op::CanvasHome, Op::CompleteAnimations]);
    assert_eq!(transform(&layout).offset, Point::from((0., 0.)));
    assert_abs_diff_eq!(transform(&layout).zoom, 0.62);

    check_ops_on_layout(
        &mut layout,
        [Op::CanvasPanTo { x: -50., y: 75.5 }, Op::CompleteAnimations],
    );
    assert_eq!(transform(&layout).offset, Point::from((-50., 75.5)));

    for (op, zoom) in [
        (Op::CanvasZoomIn { anchor: None }, 0.75),
        (Op::CanvasZoomIn { anchor: None }, 0.9),
        (Op::CanvasZoomIn { anchor: None }, 1.0),
        (Op::CanvasZoomIn { anchor: None }, 1.0),
        (Op::CanvasSetZoom(0.1), 0.4),
        (Op::CanvasZoomOut { anchor: None }, 0.4),
        (Op::CanvasSetZoom(7.), 1.0),
        (Op::CanvasZoomOut { anchor: None }, 0.9),
    ] {
        check_ops_on_layout(&mut layout, [op, Op::CompleteAnimations]);
        assert_abs_diff_eq!(transform(&layout).zoom, zoom);
    }
}

#[test]
fn zoom_keeps_anchor_fixed() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        window(1),
        Op::ToggleWorkspaceMode,
        Op::CompleteAnimations,
    ]);
    let anchor = Point::from((200., 150.));
    let before = transform(&layout).view_to_plane(anchor);

    check_ops_on_layout(
        &mut layout,
        [
            Op::CanvasZoomIn {
                anchor: Some((anchor.x, anchor.y)),
            },
            // Mid-animation too.
            Op::AdvanceAnimations { msec_delta: 30 },
        ],
    );
    let mid = transform(&layout);
    assert!(mid.zoom > 0.62 && mid.zoom < 0.75);
    let p = mid.view_to_plane(anchor);
    assert_abs_diff_eq!(p.x, before.x, epsilon = 1e-6);
    assert_abs_diff_eq!(p.y, before.y, epsilon = 1e-6);

    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    let after = transform(&layout);
    assert_abs_diff_eq!(after.zoom, 0.75);
    let p = after.view_to_plane(anchor);
    assert_abs_diff_eq!(p.x, before.x, epsilon = 1e-6);
    assert_abs_diff_eq!(p.y, before.y, epsilon = 1e-6);
}

#[test]
fn fit_shows_all_windows() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        window(1),
        window(2),
        window(3),
        Op::ToggleWorkspaceMode,
        Op::CompleteAnimations,
        Op::MoveFloatingWindow {
            id: Some(1),
            x: PositionChange::AdjustFixed(-3000.),
            y: PositionChange::AdjustFixed(-200.),
            animate: false,
        },
        Op::MoveFloatingWindow {
            id: Some(3),
            x: PositionChange::AdjustFixed(1500.),
            y: PositionChange::AdjustFixed(900.),
            animate: false,
        },
        Op::CanvasFit,
        Op::CompleteAnimations,
    ]);
    let wa = working_area(&layout);
    let zoom = transform(&layout).zoom;
    assert!((0.4..=1.0).contains(&zoom));
    if zoom > 0.4 {
        for id in 1..=3 {
            let rect = view_rect(&layout, id);
            assert!(
                wa.contains_rect(rect),
                "window {id} at {rect:?} outside {wa:?}"
            );
        }
    }

    // Fitting a single small window never zooms above 100%.
    check_ops_on_layout(
        &mut layout,
        [
            Op::CloseWindow(1),
            Op::CloseWindow(3),
            Op::CanvasFit,
            Op::CompleteAnimations,
        ],
    );
    assert_abs_diff_eq!(transform(&layout).zoom, 1.0);
}

#[test]
fn keyboard_focus_reveals_window() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        window(1),
        window(2),
        Op::ToggleWorkspaceMode,
        Op::CompleteAnimations,
        Op::MoveFloatingWindow {
            id: Some(1),
            x: PositionChange::AdjustFixed(-4000.),
            y: PositionChange::AdjustFixed(0.),
            animate: false,
        },
    ]);
    assert_eq!(ws(&layout).active_window().unwrap().0.id, 2);

    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusColumnLeft,
            Op::CanvasRevealWindow(1),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(ws(&layout).active_window().unwrap().0.id, 1);
    let center = rect_center(view_rect(&layout, 1));
    let wa_center = rect_center(working_area(&layout));
    assert_abs_diff_eq!(center.x, wa_center.x, epsilon = 1e-6);
    assert_abs_diff_eq!(center.y, wa_center.y, epsilon = 1e-6);

    // Pointer-style activation (no reveal) leaves the view alone.
    let t = transform(&layout);
    layout.activate_window(&2);
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    assert_eq!(transform(&layout), t);
}

#[test]
fn keyboard_move_pushes_view() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        window(1),
        Op::ToggleWorkspaceMode,
        Op::CompleteAnimations,
    ]);
    let t0 = transform(&layout);
    for _ in 0..40 {
        check_ops_on_layout(&mut layout, [Op::MoveColumnRight, Op::CanvasEdgePush(1)]);
    }
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    assert!(transform(&layout).offset.x > t0.offset.x);
    let wa = working_area(&layout);
    let rect = view_rect(&layout, 1);
    assert!(rect.loc.x + rect.size.w <= wa.loc.x + wa.size.w + 1e-6);
    assert!(rect.loc.x >= wa.loc.x - 1e-6);
}

#[test]
fn pinned_window_stays_on_screen() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        window(1),
        window(2),
        Op::ToggleWorkspaceMode,
        Op::CompleteAnimations,
        Op::ToggleWindowPinned { id: Some(1) },
    ]);
    assert!(layout.is_window_pinned(&1));
    let view_1 = view_rect(&layout, 1);
    let view_2 = view_rect(&layout, 2);

    check_ops_on_layout(
        &mut layout,
        [
            Op::CanvasPan(CanvasPanDirection::Down),
            Op::CanvasPan(CanvasPanDirection::Left),
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(view_rect(&layout, 1).loc, view_1.loc);
    assert_ne!(view_rect(&layout, 2).loc, view_2.loc);

    // Unpinning keeps it in place, after which it pans with the canvas.
    check_ops_on_layout(&mut layout, [Op::ToggleWindowPinned { id: Some(1) }]);
    assert!(!layout.is_window_pinned(&1));
    let loc = view_rect(&layout, 1).loc;
    assert_abs_diff_eq!(loc.x, view_1.loc.x, epsilon = 1e-6);
    assert_abs_diff_eq!(loc.y, view_1.loc.y, epsilon = 1e-6);
}

#[test]
fn canvas_actions_are_noops_on_strip() {
    let mut layout = check_ops(strip_setup());
    let before = ws(&layout).scrolling().snapshot_columns();
    check_ops_on_layout(
        &mut layout,
        [
            Op::CanvasPan(CanvasPanDirection::Left),
            Op::CanvasPanTo { x: 10., y: 10. },
            Op::CanvasZoomIn { anchor: None },
            Op::CanvasZoomOut { anchor: None },
            Op::CanvasSetZoom(0.5),
            Op::CanvasFit,
            Op::CanvasHome,
            Op::CanvasFlyToWindow { id: Some(1) },
            Op::CanvasRevealWindow(1),
            Op::CanvasEdgePush(1),
            Op::ToggleWindowPinned { id: Some(1) },
            Op::CompleteAnimations,
        ],
    );
    assert_eq!(ws(&layout).mode(), WorkspaceMode::Strip);
    assert!(ws(&layout).canvas_transform().is_none());
    assert!(!layout.is_window_pinned(&1));
    assert_eq!(ws(&layout).scrolling().snapshot_columns(), before);
}

#[test]
fn default_workspace_mode_canvas() {
    let options = Options {
        layout: niri_config::Layout {
            default_workspace_mode: WorkspaceMode::Canvas,
            ..Default::default()
        },
        ..Default::default()
    };
    let layout = check_ops_with_options(options, [Op::AddOutput(1), window(1), window(2)]);
    assert_eq!(ws(&layout).mode(), WorkspaceMode::Canvas);
    assert!(ws(&layout).is_floating(&1));
    assert!(ws(&layout).is_floating(&2));
    assert_abs_diff_eq!(transform(&layout).zoom, 0.62);

    // The empty workspace below is a canvas too.
    let modes: Vec<_> = layout.workspaces().map(|(_, _, ws)| ws.mode()).collect();
    assert!(modes.iter().all(|m| *m == WorkspaceMode::Canvas));
}

#[test]
fn moving_windows_between_strip_and_canvas() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        window(1),
        window(2),
        Op::ToggleWorkspaceMode,
        Op::CompleteAnimations,
        // Canvas window to the strip workspace below: it gets tiled there.
        Op::MoveWindowToWorkspaceDown(true),
        Op::CompleteAnimations,
    ]);
    assert_eq!(ws(&layout).mode(), WorkspaceMode::Strip);
    assert!(ws(&layout).has_window(&2));
    assert!(!ws(&layout).is_floating(&2));

    // And back up: it lands in the viewport centre of the canvas.
    check_ops_on_layout(
        &mut layout,
        [Op::MoveWindowToWorkspaceUp(true), Op::CompleteAnimations],
    );
    assert_eq!(ws(&layout).mode(), WorkspaceMode::Canvas);
    assert!(ws(&layout).is_floating(&2));
    let center = rect_center(view_rect(&layout, 2));
    let wa_center = rect_center(working_area(&layout));
    // Cascade may shift it off the exact centre if window 1 sits there.
    assert!((center.x - wa_center.x).abs() <= 32. * 0.62 + 1.);
    assert!((center.y - wa_center.y).abs() <= 32. * 0.62 + 1.);
}

#[test]
fn interactive_move_drop_on_zoomed_canvas() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        window(1),
        window(2),
        Op::ToggleWorkspaceMode,
        Op::CompleteAnimations,
    ]);
    let zoom = transform(&layout).zoom;
    assert!(zoom < 1.);

    let start = rect_center(view_rect(&layout, 2));
    check_ops_on_layout(
        &mut layout,
        [
            Op::InteractiveMoveBegin {
                window: 2,
                output_idx: 1,
                px: start.x,
                py: start.y,
            },
            Op::InteractiveMoveUpdate {
                window: 2,
                dx: 300.,
                dy: 200.,
                output_idx: 1,
                px: start.x + 300.,
                py: start.y + 200.,
            },
            Op::InteractiveMoveUpdate {
                window: 2,
                dx: 0.,
                dy: 0.,
                output_idx: 1,
                px: start.x + 300.,
                py: start.y + 200.,
            },
        ],
    );

    // Where the dragged tile is drawn right before the drop (view coordinates).
    let drop_loc = match &layout.interactive_move {
        Some(InteractiveMoveState::Moving(move_)) => move_.tile_render_location(1.),
        other => panic!("expected an ongoing move, got {other:?}"),
    };

    check_ops_on_layout(&mut layout, [Op::InteractiveMoveEnd { window: 2 }]);
    assert_eq!(ws(&layout).mode(), WorkspaceMode::Canvas);
    assert!(ws(&layout).is_floating(&2));
    // The drop position was converted view -> plane: the window lands where it was drawn.
    let landed = view_rect(&layout, 2).loc;
    assert_abs_diff_eq!(landed.x, drop_loc.x, epsilon = 1.);
    assert_abs_diff_eq!(landed.y, drop_loc.y, epsilon = 1.);
}

#[test]
fn frame_width_compensated_when_zoom_settles() {
    let mut options = Options::default();
    options.layout.focus_ring.width = 1.;
    options.layout.border.width = 1.;
    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            window(1),
            Op::ToggleWorkspaceMode,
            Op::CompleteAnimations,
        ],
    );
    let floating_options = ws(&layout).floating().options().clone();
    assert_eq!(floating_options.layout.focus_ring.width, 2.);
    assert_eq!(floating_options.layout.border.width, 2.);

    check_ops_on_layout(&mut layout, [Op::CanvasSetZoom(1.), Op::CompleteAnimations]);
    assert_eq!(ws(&layout).floating().options().layout.focus_ring.width, 1.);

    check_ops_on_layout(
        &mut layout,
        [Op::CanvasSetZoom(0.4), Op::CompleteAnimations],
    );
    assert_eq!(ws(&layout).floating().options().layout.focus_ring.width, 3.);

    check_ops_on_layout(
        &mut layout,
        [Op::ToggleWorkspaceMode, Op::CompleteAnimations],
    );
    assert_eq!(
        ws(&layout).scrolling().options().layout.focus_ring.width,
        1.
    );
}

#[test]
fn hit_testing_under_zoom_is_activate_only() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        window(1),
        Op::ToggleWorkspaceMode,
        Op::CompleteAnimations,
    ]);
    let center = rect_center(view_rect(&layout, 1));
    let (win, hit) = ws(&layout).window_under(center).unwrap();
    assert_eq!(win.0.id, 1);
    assert!(matches!(hit, HitType::Activate { .. }));
    assert!(ws(&layout).resize_edges_under(center).is_none());

    // At 100% hits are passed through unchanged (test windows have no input region, so compare
    // with the raw tile hit).
    check_ops_on_layout(&mut layout, [Op::CanvasSetZoom(1.), Op::CompleteAnimations]);
    let center = rect_center(view_rect(&layout, 1));
    let (win, hit) = ws(&layout).window_under(center).unwrap();
    assert_eq!(win.0.id, 1);
    let raw = ws(&layout)
        .floating()
        .tiles_with_render_positions()
        .find_map(|(tile, pos)| HitType::hit_tile(tile, pos, center))
        .map(|(_, hit)| hit);
    assert_eq!(Some(hit), raw);
}

#[test]
fn canvas_random_sequences() {
    // A few hand-picked sequences that mix canvas with strip operations.
    let ops = [
        Op::AddOutput(1),
        window(1),
        window(2),
        Op::FullscreenWindow(2),
        Op::ToggleWorkspaceMode,
        Op::FullscreenWindow(2),
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(3)
            },
        },
        Op::MaximizeWindowToEdges { id: Some(1) },
        Op::CanvasFit,
        Op::AdvanceAnimations { msec_delta: 20 },
        Op::ToggleWindowPinned { id: Some(3) },
        Op::ToggleWorkspaceMode,
        Op::ToggleWorkspaceMode,
        Op::MoveColumnToWorkspaceDown(true),
        Op::MoveColumnToWorkspaceUp(true),
        Op::ToggleOverview,
        Op::CanvasZoomIn {
            anchor: Some((10., 10.)),
        },
        Op::AddOutput(2),
        Op::MoveWindowToOutput {
            window_id: Some(1),
            output_id: 2,
            target_ws_idx: None,
        },
        Op::RemoveOutput(1),
        Op::CompleteAnimations,
    ];
    let mut layout = check_ops(ops);
    check_ops_on_layout(&mut layout, settle());
}

#[test]
fn unmaximized_by_column_action_returns_to_canvas() {
    // Found by random_operations_dont_panic: a column action un-maximizes a window that went
    // from the canvas to the scrolling space, then fullscreen must not forget that it belongs
    // on the canvas.
    let options = Options {
        layout: niri_config::Layout {
            default_workspace_mode: WorkspaceMode::Canvas,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut layout = check_ops_with_options(
        options,
        [
            window(3),
            Op::AddOutput(1),
            Op::MaximizeWindowToEdges { id: None },
            Op::SwitchPresetColumnWidth,
            Op::FullscreenWindow(3),
            Op::FullscreenWindow(3),
            Op::Refresh { is_active: true },
        ],
    );
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    assert!(ws(&layout).is_floating(&3));
    assert!(ws(&layout).scrolling().is_empty());
}

#[test]
fn focused_maximized_window_covers_canvas() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        window(1),
        window(2),
        Op::ToggleWorkspaceMode,
        Op::CompleteAnimations,
        Op::MaximizeWindowToEdges { id: Some(2) },
        Op::Communicate(2),
        Op::CompleteAnimations,
    ]);
    assert!(!ws(&layout).is_floating(&2));
    assert!(!ws(&layout).is_floating_visible());
    // Everything under the pointer is the maximized window.
    let (win, _) = ws(&layout).window_under(Point::from((5., 5.))).unwrap();
    assert_eq!(win.0.id, 2);

    // Focusing the canvas brings it back on top.
    check_ops_on_layout(&mut layout, [Op::FocusFloating]);
    assert!(ws(&layout).is_floating_visible());

    check_ops_on_layout(
        &mut layout,
        [
            Op::MaximizeWindowToEdges { id: Some(2) },
            Op::Communicate(2),
            Op::CompleteAnimations,
        ],
    );
    assert!(ws(&layout).is_floating(&2));
}
