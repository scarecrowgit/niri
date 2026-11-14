//! rice: tests for the working area outline and clip.

use niri_config::{CanvasRegion, WorkArea};

use super::*;

fn options(work_area: WorkArea) -> Options {
    let mut options = Options::default();
    options.layout.work_area = work_area;
    options.layout.struts.top = FloatOrInt(40.);
    options.layout.struts.left = FloatOrInt(80.);
    options
}

fn clip_on() -> WorkArea {
    WorkArea {
        outline_width: 1.,
        corner_radius: 12.,
        clip: true,
        ..Default::default()
    }
}

fn window(id: usize) -> Op {
    Op::AddWindow {
        params: TestWindowParams::new(id),
    }
}

fn settle(layout: &mut Layout<TestWindow>) {
    check_ops_on_layout(layout, [Op::CompleteAnimations, Op::UpdateRenderElements]);
}

#[test]
fn off_by_default() {
    let mut layout = check_ops([Op::AddOutput(1), window(1)]);
    settle(&mut layout);
    let ws = layout.active_workspace().unwrap();
    assert_eq!(ws.work_area_clip(), None);
    assert!(!ws.has_work_area_decor());
}

#[test]
fn clip_is_the_working_area() {
    let mut layout = check_ops_with_options(
        options(clip_on()),
        [Op::AddOutput(1), window(1), Op::Communicate(1)],
    );
    settle(&mut layout);
    let ws = layout.active_workspace().unwrap();
    // The working area with the struts.
    let mut area = ws.working_area();
    area.loc += Point::from((80., 40.));
    area.size -= Size::from((80., 40.));
    assert_eq!(ws.work_area_clip(), Some(area));
    assert!(ws.has_work_area_decor());
}

#[test]
fn outline_without_clip() {
    let work_area = WorkArea {
        outline_width: 1.,
        ..Default::default()
    };
    let mut layout = check_ops_with_options(options(work_area), [Op::AddOutput(1)]);
    settle(&mut layout);
    let ws = layout.active_workspace().unwrap();
    assert_eq!(ws.work_area_clip(), None);
    assert!(ws.has_work_area_decor());
}

#[test]
fn fullscreen_turns_the_clip_off() {
    let mut layout = check_ops_with_options(
        options(clip_on()),
        [Op::AddOutput(1), window(1), Op::Communicate(1)],
    );
    settle(&mut layout);
    assert!(layout
        .active_workspace()
        .unwrap()
        .work_area_clip()
        .is_some());

    check_ops_on_layout(&mut layout, [Op::FullscreenWindow(1), Op::Communicate(1)]);
    settle(&mut layout);
    let ws = layout.active_workspace().unwrap();
    assert_eq!(ws.work_area_clip(), None);
    assert!(!ws.has_work_area_decor());

    check_ops_on_layout(&mut layout, [Op::FullscreenWindow(1), Op::Communicate(1)]);
    settle(&mut layout);
    assert!(layout
        .active_workspace()
        .unwrap()
        .work_area_clip()
        .is_some());
}

#[test]
fn canvas_workspaces_are_clipped_too() {
    let mut options = options(clip_on());
    options.layout.canvas.decor.regions = vec![CanvasRegion {
        name: String::from("left"),
        x: -2000.,
        y: 0.,
        w: 4000.,
        h: 300.,
    }];
    let mut layout = check_ops_with_options(
        options,
        [
            Op::AddOutput(1),
            window(1),
            Op::Communicate(1),
            Op::ToggleWorkspaceMode,
        ],
    );
    settle(&mut layout);
    let ws = layout.active_workspace().unwrap();
    assert!(ws.is_canvas());
    let clip = ws.work_area_clip().unwrap();
    assert_eq!(clip.loc, ws.working_area().loc + Point::from((80., 40.)));
    // The region sticks out of the view on both sides and is still visible.
    assert_eq!(ws.canvas_visible_region_names(), ["left"]);
}

#[test]
fn radius_larger_than_the_area_is_clamped() {
    let work_area = WorkArea {
        outline_width: 600.,
        corner_radius: 10000.,
        clip: true,
        ..Default::default()
    };
    let mut layout = check_ops_with_options(options(work_area), [Op::AddOutput(1)]);
    settle(&mut layout);
    assert!(layout.active_workspace().unwrap().has_work_area_decor());
}
