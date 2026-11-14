//! ashd: tests for compositor-drawn title strips, the focus ring gap, canvas focus styles and
//! canvas regions.

use niri_config::{CanvasRegion, FocusRingCanvasStyle};

use super::*;

const STRIP: f64 = 28.;

fn options() -> Options {
    let mut options = Options::default();
    options.layout.title_bar = niri_config::TitleBar {
        off: false,
        ..Default::default()
    };
    options.layout.border.off = false;
    options.layout.border.width = 1.;
    options
}

fn tile(layout: &Layout<TestWindow>, id: usize) -> &Tile<TestWindow> {
    layout
        .workspaces()
        .flat_map(|(_, _, ws)| ws.tiles())
        .find(|tile| tile.window().0.id == id)
        .unwrap()
}

fn window(id: usize) -> Op {
    Op::AddWindow {
        params: TestWindowParams::new(id),
    }
}

fn settle(id: usize) -> [Op; 2] {
    [Op::Communicate(id), Op::CompleteAnimations]
}

#[test]
fn title_bar_off_by_default() {
    let layout = check_ops([Op::AddOutput(1), window(1), Op::Communicate(1)]);
    let tile = tile(&layout, 1);
    assert_eq!(tile.title_bar().height(), None);
    assert_eq!(tile.title_bar_rect(), None);
    assert_eq!(tile.tile_size(), tile.window_size());
}

#[test]
fn strip_tile_geometry() {
    let mut ops = vec![Op::AddOutput(1), window(1)];
    ops.extend(settle(1));
    let layout = check_ops_with_options(options(), ops);
    let tile = tile(&layout, 1);

    let window = tile.window_size();
    let size = tile.tile_size();
    assert_eq!(size.w, window.w + 2.);
    assert_eq!(size.h, window.h + 2. + STRIP);
    assert_eq!(tile.window_loc(), Point::from((1., 1. + STRIP)));
    assert_eq!(
        tile.title_bar_rect(),
        Some(Rectangle::new((1., 1.).into(), (window.w, STRIP).into()))
    );

    // The column fills the working area height (720 - 2 * 16 gaps): the window gets the rest.
    assert_eq!(size.h, 720. - 32.);
    assert_eq!(window.h, 720. - 32. - 2. - STRIP);

    // Conversions.
    assert_eq!(tile.tile_height_for_window_height(100.), 100. + 2. + STRIP);
    assert_eq!(tile.window_height_for_tile_height(100. + 2. + STRIP), 100.);
    assert_eq!(tile.tile_width_for_window_width(100.), 102.);
    assert_eq!(tile.window_width_for_tile_width(102.), 100.);
}

#[test]
fn strip_min_max_size() {
    let mut params = TestWindowParams::new(1);
    params.min_max_size = (Size::from((100, 300)), Size::from((0, 400)));
    let mut ops = vec![Op::AddOutput(1), Op::AddWindow { params }];
    ops.extend(settle(1));
    let layout = check_ops_with_options(options(), ops);
    let tile = tile(&layout, 1);

    assert_eq!(
        tile.min_size_nonfullscreen(),
        Size::from((102., 300. + 2. + STRIP))
    );
    // Unlimited width stays unlimited.
    assert_eq!(
        tile.max_size_nonfullscreen(),
        Size::from((0., 400. + 2. + STRIP))
    );

    // Without min height, the strip still needs a 1 px window.
    let mut params = TestWindowParams::new(2);
    params.min_max_size = (Size::from((0, 0)), Size::from((0, 0)));
    let layout = check_ops_with_options(options(), [Op::AddOutput(1), Op::AddWindow { params }]);
    let tile = self::tile(&layout, 2);
    assert_eq!(
        tile.min_size_nonfullscreen(),
        Size::from((3., 1. + 2. + STRIP))
    );
    assert_eq!(tile.max_size_nonfullscreen(), Size::from((0., 0.)));
}

#[test]
fn strip_fractional_scale() {
    let mut options = options();
    options.layout.title_bar.height = 27.;
    let mut ops = vec![
        Op::AddScaledOutput {
            id: 1,
            scale: 1.5,
            layout_config: None,
        },
        window(1),
    ];
    ops.extend(settle(1));
    let layout = check_ops_with_options(options, ops);
    let tile = tile(&layout, 1);

    // 27 * 1.5 = 40.5 rounds to 41 physical pixels.
    let height = 41. / 1.5;
    assert_eq!(tile.title_bar().height(), Some(height));
    let window = tile.window_size();
    let size = tile.tile_size();
    let border = 1. / 1.5 * 2.; // 1 px border rounds to 2 physical px at 1.5.
    approx::assert_abs_diff_eq!(size.h, window.h + border * 2. + height, epsilon = 1e-9);
    // Whole physical pixels everywhere.
    let loc = tile.window_loc();
    approx::assert_abs_diff_eq!(loc.y * 1.5, (loc.y * 1.5).round(), epsilon = 1e-9);
    approx::assert_abs_diff_eq!(size.h * 1.5, (size.h * 1.5).round(), epsilon = 1e-9);
}

#[test]
fn fullscreen_and_maximized_have_no_strip() {
    let mut ops = vec![Op::AddOutput(1), window(1)];
    ops.extend(settle(1));
    ops.push(Op::FullscreenWindow(1));
    ops.extend(settle(1));
    let layout = check_ops_with_options(options(), ops.clone());
    let t = tile(&layout, 1);
    assert_eq!(t.effective_title_bar_height(), None);
    assert_eq!(t.title_bar_rect(), None);
    assert_eq!(t.tile_size(), Size::from((1280., 720.)));
    assert_eq!(t.window_loc(), Point::from((0., 0.)));

    // Back to normal: the strip returns.
    ops.push(Op::FullscreenWindow(1));
    ops.extend(settle(1));
    let layout = check_ops_with_options(options(), ops);
    let t = tile(&layout, 1);
    assert_eq!(t.effective_title_bar_height(), Some(STRIP));
    assert_eq!(t.tile_size().h, 720. - 32.);

    let mut ops = vec![Op::AddOutput(1), window(1)];
    ops.extend(settle(1));
    ops.push(Op::MaximizeWindowToEdges { id: None });
    ops.extend(settle(1));
    let layout = check_ops_with_options(options(), ops);
    let t = tile(&layout, 1);
    assert_eq!(t.effective_title_bar_height(), None);
    assert_eq!(t.tile_size(), Size::from((1280., 720.)));
}

#[test]
fn maximize_column_keeps_strip() {
    let mut ops = vec![Op::AddOutput(1), window(1)];
    ops.extend(settle(1));
    ops.push(Op::MaximizeColumn);
    ops.extend(settle(1));
    let layout = check_ops_with_options(options(), ops);
    let t = tile(&layout, 1);
    assert_eq!(t.effective_title_bar_height(), Some(STRIP));
    assert_eq!(t.tile_size(), Size::from((1280. - 32., 720. - 32.)));
}

#[test]
fn strip_hit_testing() {
    let mut ops = vec![Op::AddOutput(1), window(1)];
    ops.extend(settle(1));
    let layout = check_ops_with_options(options(), ops);
    let t = tile(&layout, 1);

    assert!(matches!(
        t.hit(Point::from((10., 10.))),
        Some(HitType::TitleBar)
    ));
    assert!(matches!(
        t.hit(Point::from((10., 1. + STRIP - 0.5))),
        Some(HitType::TitleBar)
    ));
    // The border around the strip only activates.
    assert!(matches!(
        t.hit(Point::from((0.5, 10.))),
        Some(HitType::Activate { .. })
    ));
    // Below the strip it is the window (test windows have no input region: activate only).
    assert!(matches!(
        t.hit(Point::from((10., 1. + STRIP + 5.))),
        Some(HitType::Activate { .. })
    ));
}

#[test]
fn title_bar_window_rule_off() {
    let mut params = TestWindowParams::new(1);
    params.rules = Some(ResolvedWindowRules {
        title_bar: Some(false),
        ..Default::default()
    });
    let mut ops = vec![Op::AddOutput(1), Op::AddWindow { params }];
    ops.extend(settle(1));
    let layout = check_ops_with_options(options(), ops);
    let t = tile(&layout, 1);
    assert_eq!(t.title_bar().height(), None);
    assert_eq!(t.tile_size().h, t.window_size().h + 2.);

    // And `title-bar true` turns it on without a layout title bar.
    let mut params = TestWindowParams::new(1);
    params.rules = Some(ResolvedWindowRules {
        title_bar: Some(true),
        ..Default::default()
    });
    let mut ops = vec![Op::AddOutput(1), Op::AddWindow { params }];
    ops.extend(settle(1));
    let layout = check_ops(ops);
    let t = tile(&layout, 1);
    assert_eq!(t.title_bar().height(), Some(STRIP));
}

#[test]
fn tabbed_column_tiles_all_have_strips() {
    let mut ops = vec![Op::AddOutput(1), window(1), window(2)];
    ops.push(Op::ConsumeOrExpelWindowLeft { id: None });
    ops.push(Op::ToggleColumnTabbedDisplay);
    ops.push(Op::Communicate(1));
    ops.extend(settle(2));
    let layout = check_ops_with_options(options(), ops);
    for id in [1, 2] {
        let t = tile(&layout, id);
        assert_eq!(t.effective_title_bar_height(), Some(STRIP));
        assert_eq!(t.tile_size().h, t.window_size().h + 2. + STRIP);
    }
}

#[test]
fn canvas_windows_have_strips_and_canvas_focus_style() {
    let mut options = options();
    options.layout.focus_ring.canvas_style = FocusRingCanvasStyle::Border;
    let ring_color = options.layout.focus_ring.active_color;
    assert_ne!(options.layout.border.active_color, ring_color);

    let mut ops = vec![Op::AddOutput(1), window(1)];
    ops.extend(settle(1));
    let layout = check_ops_with_options(options.clone(), ops.clone());
    let t = tile(&layout, 1);
    assert!(!t.focus_ring().is_off());

    ops.push(Op::ToggleWorkspaceMode);
    ops.extend(settle(1));
    let layout = check_ops_with_options(options.clone(), ops.clone());
    let ws = layout.active_workspace().unwrap();
    assert!(ws.is_canvas());
    let t = tile(&layout, 1);
    assert_eq!(t.effective_title_bar_height(), Some(STRIP));
    assert!(t.focus_ring().is_off());
    assert_eq!(t.border().config().active_color, ring_color);
    assert!(t.options().canvas_tile_zoom.is_some());

    // Back in strip mode, the ring is back.
    ops.push(Op::ToggleWorkspaceMode);
    ops.extend(settle(1));
    let layout = check_ops_with_options(options.clone(), ops);
    let t = tile(&layout, 1);
    assert!(!t.focus_ring().is_off());
    assert_ne!(t.border().config().active_color, ring_color);
    assert_eq!(t.options().canvas_tile_zoom, None);

    // "off" hides the ring on canvases only.
    options.layout.focus_ring.canvas_style = FocusRingCanvasStyle::Off;
    let mut ops = vec![Op::AddOutput(1), window(1), Op::ToggleWorkspaceMode];
    ops.extend(settle(1));
    let layout = check_ops_with_options(options, ops);
    assert!(tile(&layout, 1).focus_ring().is_off());
}

#[test]
fn canvas_regions_runtime_changes() {
    let mut options = options();
    options.layout.canvas.decor.regions = vec![CanvasRegion {
        name: String::from("dots"),
        x: 0.,
        y: 0.,
        w: 400.,
        h: 300.,
    }];
    let mut layout = check_ops_with_options(options, [Op::AddOutput(1), Op::ToggleWorkspaceMode]);
    let names = |layout: &Layout<TestWindow>| -> Vec<String> {
        layout
            .active_workspace()
            .unwrap()
            .canvas_regions()
            .into_iter()
            .map(|r| r.name)
            .collect()
    };
    assert_eq!(names(&layout), ["dots"]);

    layout.canvas_add_region(CanvasRegion {
        name: String::from("research"),
        x: -1000.,
        y: 0.,
        w: 100.,
        h: 100.,
    });
    assert_eq!(names(&layout), ["dots", "research"]);

    // Replacing a config region by name.
    layout.canvas_add_region(CanvasRegion {
        name: String::from("dots"),
        x: 10.,
        y: 10.,
        w: 10.,
        h: 10.,
    });
    assert_eq!(names(&layout), ["research", "dots"]);

    layout.canvas_remove_region("dots");
    assert_eq!(names(&layout), ["research"]);
    layout.canvas_remove_region("research");
    assert!(names(&layout).is_empty());

    // Invalid sizes are ignored.
    layout.canvas_add_region(CanvasRegion {
        name: String::from("bad"),
        x: 0.,
        y: 0.,
        w: 0.,
        h: 10.,
    });
    assert!(names(&layout).is_empty());

    // Visibility follows the view.
    layout.canvas_add_region(CanvasRegion {
        name: String::from("near"),
        x: 0.,
        y: 0.,
        w: 100.,
        h: 100.,
    });
    layout.canvas_add_region(CanvasRegion {
        name: String::from("far"),
        x: 100000.,
        y: 0.,
        w: 100.,
        h: 100.,
    });
    layout.clock.set_complete_instantly(true);
    layout.advance_animations();
    layout.clock.set_complete_instantly(false);
    layout.update_render_elements(None);
    let ws = layout.active_workspace().unwrap();
    assert_eq!(ws.canvas_visible_region_names(), ["near"]);
}

// ashd: window-rule border widths are compensated under the canvas zoom like the layout ones.
#[test]
fn canvas_compensates_window_rule_border_width() {
    let mut options = Options::default();
    options.layout.border.off = true;
    options.layout.canvas.default_zoom = 0.5;
    options.layout.canvas.zoom_steps = vec![0.5, 1.];
    let rule_window = Op::AddWindow {
        params: TestWindowParams {
            rules: Some(ResolvedWindowRules {
                border: niri_config::BorderRule {
                    on: true,
                    width: Some(FloatOrInt(1.)),
                    ..Default::default()
                },
                ..ResolvedWindowRules::default()
            }),
            ..TestWindowParams::new(1)
        },
    };
    let mut ops = vec![Op::AddOutput(1), rule_window];
    ops.extend(settle(1));
    ops.push(Op::ToggleWorkspaceMode);
    ops.extend(settle(1));
    let layout = check_ops_with_options(options, ops);
    assert_eq!(layout.active_workspace().unwrap().canvas_zoom(), 0.5);
    // One physical pixel at zoom 0.5 needs a 2 px border on the plane.
    assert_eq!(tile(&layout, 1).effective_border_width(), Some(2.));
}

// ashd: the tab indicator sits outside a focus ring with a gap.
#[test]
fn tab_indicator_moves_out_past_the_ring_gap() {
    let mut options = Options::default();
    options.layout.focus_ring.gap = 4.;
    options.layout.focus_ring.width = 1.;
    options.layout.tab_indicator.gap = 5.;
    options.layout.tab_indicator.place_within_column = true;
    let ops = [
        Op::AddOutput(1),
        window(1),
        window(2),
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::ToggleColumnTabbedDisplay,
        Op::CompleteAnimations,
    ];
    let with_gap = check_ops_with_options(options.clone(), ops.clone());
    options.layout.focus_ring.gap = 0.;
    let without_gap = check_ops_with_options(options, ops);
    // The tiles make room for the indicator plus the ring's gap and width.
    let x = |layout: &Layout<TestWindow>| {
        let ws = layout.active_workspace().unwrap();
        ws.tiles_with_render_positions()
            .find(|(tile, _, _)| tile.window().0.id == 2)
            .map(|(_, pos, _)| pos.x)
            .unwrap()
    };
    assert_eq!(x(&with_gap) - x(&without_gap), 5.);
}
