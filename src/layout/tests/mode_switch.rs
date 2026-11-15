//! ashd: the strip ↔ canvas mode switch transition (`animations { canvas-mode-switch }`).

use std::collections::HashMap;

use super::*;
use crate::layout::canvas::{rect_center, CanvasTransform};

fn window(id: usize) -> Op {
    Op::AddWindow {
        params: TestWindowParams::new(id),
    }
}

fn setup() -> Layout<TestWindow> {
    check_ops([
        Op::AddOutput(1),
        window(1),
        window(2),
        window(3),
        Op::FocusColumnLeft,
        Op::Communicate(1),
        Op::Communicate(2),
        Op::Communicate(3),
        Op::CompleteAnimations,
    ])
}

/// Where each window is drawn right now (workspace view coordinates, top-left of the tile).
fn drawn(layout: &Layout<TestWindow>) -> HashMap<usize, Point<f64, Logical>> {
    let ws = layout.active_workspace().unwrap();
    let canvas_zoom = ws.canvas_zoom();
    let strip_zoom = ws.strip_transition_zoom();
    ws.tiles_with_render_positions()
        .map(|(tile, pos, _)| {
            let id = tile.window().0.id;
            let pos = if ws.is_canvas() && ws.is_floating(&id) {
                pos.upscale(canvas_zoom)
            } else if let Some((zoom, anchor)) = strip_zoom {
                anchor + (pos - anchor).upscale(zoom)
            } else {
                pos
            };
            (id, pos)
        })
        .collect()
}

fn assert_same(a: &HashMap<usize, Point<f64, Logical>>, b: &HashMap<usize, Point<f64, Logical>>) {
    assert_eq!(a.len(), b.len());
    for (id, pa) in a {
        let pb = b[id];
        assert!(
            (pa.x - pb.x).abs() < 1. && (pa.y - pb.y).abs() < 1.,
            "window {id}: {pa:?} vs {pb:?}"
        );
    }
}

#[test]
fn entering_the_canvas_starts_at_the_strip() {
    let mut layout = setup();
    let before = drawn(&layout);

    check_ops_on_layout(&mut layout, [Op::ToggleWorkspaceMode]);
    let ws = layout.active_workspace().unwrap();
    assert!(ws.is_canvas());
    assert!(ws.is_mode_switch_ongoing());
    assert_eq!(ws.canvas_transform(), Some(CanvasTransform::IDENTITY));
    // The grid fades in from nothing.
    assert_eq!(ws.canvas_decor_alpha(), 0.);
    // First frame: nothing moved.
    assert_same(&before, &drawn(&layout));

    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    let ws = layout.active_workspace().unwrap();
    assert!(!ws.is_mode_switch_ongoing());
    assert_eq!(ws.canvas_decor_alpha(), 1.);
    assert_eq!(
        ws.canvas_zoom(),
        crate::layout::client_scale::quantize_zoom(0.62, 1.)
    );
}

#[test]
fn leaving_the_canvas_starts_where_the_canvas_drew_the_windows() {
    let mut layout = setup();
    check_ops_on_layout(
        &mut layout,
        [Op::ToggleWorkspaceMode, Op::CompleteAnimations],
    );
    let before = drawn(&layout);
    let ws = layout.active_workspace().unwrap();
    let anchor = rect_center(ws.working_area());

    check_ops_on_layout(&mut layout, [Op::ToggleWorkspaceMode]);
    let ws = layout.active_workspace().unwrap();
    assert!(!ws.is_canvas());
    // First frame: the strip is drawn at the canvas zoom and every window where it was.
    assert_eq!(
        ws.strip_transition_zoom(),
        Some((crate::layout::client_scale::quantize_zoom(0.62, 1.), anchor))
    );
    assert_same(&before, &drawn(&layout));
    // The grid starts fully visible and fades out.
    assert_eq!(ws.canvas_decor_alpha(), 1.);

    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    let ws = layout.active_workspace().unwrap();
    assert!(!ws.is_mode_switch_ongoing());
    assert_eq!(ws.strip_transition_zoom(), None);
    // Last frame: the plain strip.
    let settled = drawn(&layout);
    let plain: HashMap<_, _> = ws
        .tiles_with_render_positions()
        .map(|(tile, pos, _)| (tile.window().0.id, pos))
        .collect();
    assert_same(&settled, &plain);
}

#[test]
fn switching_back_midway_has_no_jump() {
    // Toggling again before the transition is over starts from where things are drawn.
    let mut layout = setup();
    check_ops_on_layout(
        &mut layout,
        [Op::ToggleWorkspaceMode, Op::CompleteAnimations],
    );
    check_ops_on_layout(&mut layout, [Op::ToggleWorkspaceMode]);
    let before = drawn(&layout);
    check_ops_on_layout(&mut layout, [Op::ToggleWorkspaceMode]);
    // The canvas re-entry starts with the windows where the zoomed strip drew them.
    assert_same(&before, &drawn(&layout));
    check_ops_on_layout(&mut layout, [Op::CompleteAnimations]);
    assert!(!layout.active_workspace().unwrap().is_mode_switch_ongoing());
}
