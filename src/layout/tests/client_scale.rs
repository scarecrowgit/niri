//! ashd: client scale following the canvas zoom (`crate::layout::client_scale`).

use approx::assert_abs_diff_eq;

use super::*;
use crate::layout::client_scale::CLIENT_SCALE_SETTLE;

fn window(id: usize) -> Op {
    Op::AddWindow {
        params: TestWindowParams::new(id),
    }
}

fn options(rescale: bool) -> Options {
    let mut options = Options::default();
    options.layout.canvas.rescale_clients = rescale;
    options.layout.canvas.zoom_steps = vec![0.25, 0.4, 0.5, 0.62, 0.75, 0.9, 1.0];
    options
}

/// A canvas workspace on a scale-1 output with windows 1 and 2, settled at zoom 1.
fn canvas_setup(rescale: bool) -> Layout<TestWindow> {
    check_ops_with_options(
        options(rescale),
        [
            Op::AddOutput(1),
            Op::SetWorkspaceMode(WorkspaceMode::Canvas),
            window(1),
            window(2),
            Op::Communicate(1),
            Op::Communicate(2),
            Op::CanvasSetZoom(1.0),
            Op::CompleteAnimations,
            Op::Refresh { is_active: true },
        ],
    )
}

fn win(layout: &Layout<TestWindow>, id: usize) -> &TestWindow {
    layout
        .windows()
        .map(|(_, w)| w)
        .find(|w| w.0.id == id)
        .unwrap()
}

/// (fractional, integer) scale last sent to a window.
fn client_scale(layout: &Layout<TestWindow>, id: usize) -> (f64, i32) {
    let (scale, _) = win(layout, id)
        .0
        .preferred_scale
        .get()
        .expect("no scale sent");
    (scale.fractional_scale(), scale.integer_scale())
}

fn sets(layout: &Layout<TestWindow>, id: usize) -> usize {
    win(layout, id).0.preferred_scale_sets.get()
}

/// Lets the settle time pass and refreshes.
fn after_settle() -> [Op; 2] {
    [
        Op::AdvanceAnimations {
            msec_delta: CLIENT_SCALE_SETTLE.as_millis() as i32 + 10,
        },
        Op::Refresh { is_active: true },
    ]
}

fn ws(layout: &Layout<TestWindow>) -> &Workspace<TestWindow> {
    layout.active_workspace().unwrap()
}

#[test]
fn zoomed_windows_follow_once_settled() {
    let mut layout = canvas_setup(true);
    assert_eq!(client_scale(&layout, 1), (1., 1));

    check_ops_on_layout(
        &mut layout,
        [
            Op::CanvasSetZoom(0.5),
            Op::CompleteAnimations,
            Op::Refresh { is_active: true },
        ],
    );
    // Not before the zoom has been at rest for the settle time.
    assert_eq!(client_scale(&layout, 1), (1., 1));
    assert!(layout.client_scale_deadline().is_some());

    check_ops_on_layout(&mut layout, after_settle());
    assert_eq!(client_scale(&layout, 1), (0.5, 1));
    assert_eq!(client_scale(&layout, 2), (0.5, 1));
    assert_eq!(layout.client_scale_deadline(), None);

    // One update per settle, not one per refresh.
    let before = sets(&layout, 1);
    check_ops_on_layout(
        &mut layout,
        [
            Op::Refresh { is_active: true },
            Op::AdvanceAnimations { msec_delta: 500 },
            Op::Refresh { is_active: true },
            Op::CanvasPan(CanvasPanDirection::Left),
            Op::CompleteAnimations,
            Op::Refresh { is_active: true },
        ],
    );
    assert_eq!(sets(&layout, 1), before);
}

#[test]
fn no_change_during_zoom_animation() {
    let mut layout = canvas_setup(true);
    check_ops_on_layout(
        &mut layout,
        [
            Op::CanvasSetZoom(0.5),
            Op::AdvanceAnimations { msec_delta: 16 },
            Op::Refresh { is_active: true },
            Op::AdvanceAnimations { msec_delta: 200 },
            Op::Refresh { is_active: true },
        ],
    );
    assert!(ws(&layout).canvas_view().unwrap().is_zoom_animating());
    assert_eq!(client_scale(&layout, 1), (1., 1));
}

#[test]
fn zoom_back_to_one_restores_right_away() {
    let mut layout = canvas_setup(true);
    check_ops_on_layout(
        &mut layout,
        [Op::CanvasSetZoom(0.75), Op::CompleteAnimations],
    );
    check_ops_on_layout(&mut layout, after_settle());
    assert_eq!(client_scale(&layout, 1), (0.75, 1));

    // As soon as the target is 1, before the animation is over.
    check_ops_on_layout(
        &mut layout,
        [Op::CanvasSetZoom(1.0), Op::Refresh { is_active: true }],
    );
    assert_eq!(client_scale(&layout, 1), (1., 1));
}

#[test]
fn settled_zoom_is_on_the_fractional_scale_grid() {
    let mut layout = canvas_setup(true);
    check_ops_on_layout(
        &mut layout,
        [Op::CanvasSetZoom(0.62), Op::CompleteAnimations],
    );
    let zoom = ws(&layout).canvas_zoom();
    assert_abs_diff_eq!(zoom, 74. / 120., epsilon = 1e-12);
    check_ops_on_layout(&mut layout, after_settle());
    let (scale, _) = client_scale(&layout, 1);
    assert_abs_diff_eq!(scale, 74. / 120., epsilon = 1e-12);

    // Without client rescaling the zoom is left alone.
    let mut layout = canvas_setup(false);
    check_ops_on_layout(
        &mut layout,
        [Op::CanvasSetZoom(0.62), Op::CompleteAnimations],
    );
    assert_eq!(ws(&layout).canvas_zoom(), 0.62);
    check_ops_on_layout(&mut layout, after_settle());
    assert_eq!(client_scale(&layout, 1), (1., 1));
}

#[test]
fn mode_switch_restores_and_reapplies() {
    let mut layout = canvas_setup(true);
    check_ops_on_layout(
        &mut layout,
        [Op::CanvasSetZoom(0.5), Op::CompleteAnimations],
    );
    check_ops_on_layout(&mut layout, after_settle());
    assert_eq!(client_scale(&layout, 1), (0.5, 1));

    // Strip mode: the output scale right away.
    check_ops_on_layout(
        &mut layout,
        [Op::ToggleWorkspaceMode, Op::Refresh { is_active: true }],
    );
    assert_eq!(client_scale(&layout, 1), (1., 1));
    assert_eq!(client_scale(&layout, 2), (1., 1));

    // Back to the canvas (it remembers 0.5): the zoomed scale once that has settled.
    check_ops_on_layout(
        &mut layout,
        [
            Op::ToggleWorkspaceMode,
            Op::Refresh { is_active: true },
            Op::CompleteAnimations,
            Op::Refresh { is_active: true },
        ],
    );
    assert_eq!(client_scale(&layout, 1), (1., 1));
    check_ops_on_layout(&mut layout, after_settle());
    assert_eq!(client_scale(&layout, 1), (0.5, 1));
}

#[test]
fn leaving_the_plane_restores() {
    let mut layout = canvas_setup(true);
    check_ops_on_layout(
        &mut layout,
        [Op::CanvasSetZoom(0.5), Op::CompleteAnimations],
    );
    check_ops_on_layout(&mut layout, after_settle());

    // Fullscreen windows are drawn 1:1.
    check_ops_on_layout(
        &mut layout,
        [
            Op::FullscreenWindow(1),
            Op::Communicate(1),
            Op::Refresh { is_active: true },
        ],
    );
    assert_eq!(client_scale(&layout, 1), (1., 1));
    assert_eq!(client_scale(&layout, 2), (0.5, 1));

    // Back on the plane: the zoom is long settled, so right away.
    check_ops_on_layout(
        &mut layout,
        [
            Op::FullscreenWindow(1),
            Op::Communicate(1),
            Op::CompleteAnimations,
            Op::Refresh { is_active: true },
        ],
    );
    assert_eq!(client_scale(&layout, 1), (0.5, 1));

    // Moved to a strip workspace.
    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusWindow(2),
            Op::MoveWindowToWorkspaceDown(true),
            Op::Refresh { is_active: true },
        ],
    );
    assert_eq!(client_scale(&layout, 2), (1., 1));
    assert_eq!(client_scale(&layout, 1), (0.5, 1));
}

#[test]
fn output_scale_change_rescales() {
    let mut layout = check_ops_with_options(
        options(true),
        [
            Op::AddScaledOutput {
                id: 1,
                scale: 2.,
                layout_config: None,
            },
            Op::SetWorkspaceMode(WorkspaceMode::Canvas),
            window(1),
            Op::Communicate(1),
            Op::CanvasSetZoom(0.5),
            Op::CompleteAnimations,
        ],
    );
    check_ops_on_layout(&mut layout, after_settle());
    // 2 × 0.5: integer-only clients render at 1 as well.
    assert_eq!(client_scale(&layout, 1), (1., 1));

    let output = layout.outputs().next().unwrap().clone();
    output.change_current_state(
        None,
        None,
        Some(smithay::output::Scale::Fractional(1.5)),
        None,
    );
    layout.update_output_size(&output);
    check_ops_on_layout(
        &mut layout,
        [Op::CompleteAnimations, Op::Refresh { is_active: true }],
    );
    check_ops_on_layout(&mut layout, after_settle());
    let zoom = ws(&layout).canvas_zoom();
    assert_abs_diff_eq!(zoom, 0.5, epsilon = 1e-12);
    assert_eq!(client_scale(&layout, 1), (0.75, 1));
}

#[test]
fn rescale_clients_off_keeps_output_scale() {
    let mut layout = canvas_setup(false);
    check_ops_on_layout(
        &mut layout,
        [Op::CanvasSetZoom(0.5), Op::CompleteAnimations],
    );
    check_ops_on_layout(&mut layout, after_settle());
    assert_eq!(client_scale(&layout, 1), (1., 1));
    assert_eq!(layout.client_scale_deadline(), None);
}
