//! Config for the ashd fork's per-workspace infinite canvas mode.
//!
//! ```kdl
//! layout {
//!     // Mode that new workspaces start in: "strip" (stock niri) or "canvas".
//!     default-workspace-mode "strip"
//!
//!     canvas {
//!         // Zoom that a workspace gets the first time it enters canvas mode.
//!         default-zoom 0.62
//!         // Zoom levels that canvas-zoom-in / canvas-zoom-out step through.
//!         zoom-steps 0.4 0.5 0.62 0.75 0.9 1.0
//!         // Distance in logical view pixels that one canvas-pan-* action moves the view.
//!         pan-step 120
//!         // Zoom factor per mouse wheel notch of Mod+scroll over a canvas (1 turns the native
//!         // Mod+scroll zoom off, the scroll then goes to the wheel binds).
//!         wheel-zoom-factor 1.2
//!         // Supersample window contents when the canvas is zoomed out (false: plain bilinear).
//!         smooth-zoom-out true
//!         // Window title labels at a constant on-screen size above canvas windows once the zoom
//!         // drops below `title-labels-below` (they fade in over the 0.1 below it).
//!         title-labels true
//!         title-labels-below 0.7
//!         title-labels-size 12
//!     }
//! }
//!
//! animations {
//!     // Pan and fly-to animation of the canvas view.
//!     canvas-pan {
//!         spring damping-ratio=1.0 stiffness=500 epsilon=0.0001
//!     }
//!     // Zoom animation of the canvas view.
//!     canvas-zoom {
//!         spring damping-ratio=1.0 stiffness=600 epsilon=0.0001
//!     }
//!     // Zoom animation of Mod+scroll (consecutive notches retarget it and keep its velocity).
//!     canvas-wheel-zoom {
//!         spring damping-ratio=1.0 stiffness=1200 epsilon=0.0001
//!     }
//! }
//! ```
//!
//! Like other `layout` options, `default-workspace-mode` and `canvas {}` can be overridden per
//! output and per named workspace.

use knuffel::errors::DecodeError;
pub use niri_ipc::WorkspaceMode;

use crate::animations::{Animation, Kind, SpringParams};
use crate::utils::MergeWith;
use crate::FloatOrInt;

/// Smallest zoom accepted anywhere in canvas mode.
pub const CANVAS_MIN_ZOOM: f64 = 0.05;
/// Largest zoom accepted anywhere in canvas mode.
pub const CANVAS_MAX_ZOOM: f64 = 4.;
/// Zoom range below `title-labels-below` over which the title labels fade in.
pub const TITLE_LABEL_FADE: f64 = 0.1;
/// Largest accepted `wheel-zoom-factor`.
pub const CANVAS_MAX_WHEEL_ZOOM_FACTOR: f64 = 4.;

/// Final canvas config.
#[derive(Debug, Clone, PartialEq)]
pub struct Canvas {
    /// Zoom that a workspace gets the first time it enters canvas mode.
    ///
    /// Always within the `zoom_steps` range.
    pub default_zoom: f64,
    /// Zoom levels for stepping zoom in and out, sorted ascending, deduplicated, non-empty.
    pub zoom_steps: Vec<f64>,
    /// Pan distance of one pan action, in logical view pixels (divided by zoom on the plane).
    pub pan_step: f64,
    /// Zoom factor per wheel notch of the native Mod+scroll zoom; 1 turns it off.
    pub wheel_zoom_factor: f64,
    /// Supersample (box-filter) window contents drawn minified by the canvas zoom.
    pub smooth_zoom_out: bool,
    /// Constant-size title labels above canvas windows when zoomed out.
    pub title_labels: bool,
    /// Zoom below which the title labels fade in (fully visible 0.1 below).
    pub title_labels_below: f64,
    /// Text size of the title labels, in logical pixels.
    pub title_labels_size: f64,
    /// ashd: plane grid and named regions, see `crate::decorations`.
    pub decor: crate::decorations::CanvasDecor,
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            default_zoom: 0.62,
            zoom_steps: vec![0.4, 0.5, 0.62, 0.75, 0.9, 1.0],
            pan_step: 120.,
            wheel_zoom_factor: 1.2,
            smooth_zoom_out: true,
            title_labels: true,
            title_labels_below: 0.7,
            title_labels_size: 12.,
            decor: Default::default(), // ashd
        }
    }
}

impl Canvas {
    /// Smallest configured zoom step.
    pub fn min_zoom(&self) -> f64 {
        self.zoom_steps.first().copied().unwrap_or(1.)
    }

    /// Largest configured zoom step.
    pub fn max_zoom(&self) -> f64 {
        self.zoom_steps.last().copied().unwrap_or(1.)
    }

    /// Clamps a zoom value into the configured step range.
    pub fn clamp_zoom(&self, zoom: f64) -> f64 {
        if !zoom.is_finite() {
            return self.default_zoom;
        }
        zoom.clamp(self.min_zoom(), self.max_zoom())
    }

    fn normalize(&mut self) {
        let mut steps: Vec<f64> = self
            .zoom_steps
            .iter()
            .copied()
            .filter(|z| z.is_finite())
            .map(|z| z.clamp(CANVAS_MIN_ZOOM, CANVAS_MAX_ZOOM))
            .collect();
        steps.sort_by(f64::total_cmp);
        steps.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        if steps.is_empty() {
            steps = Canvas::default().zoom_steps;
        }
        self.zoom_steps = steps;

        self.default_zoom = self.clamp_zoom(self.default_zoom);

        if !self.pan_step.is_finite() || self.pan_step < 1. {
            self.pan_step = Canvas::default().pan_step;
        }

        if !self.wheel_zoom_factor.is_finite() {
            self.wheel_zoom_factor = Canvas::default().wheel_zoom_factor;
        }
        self.wheel_zoom_factor = self
            .wheel_zoom_factor
            .clamp(1., CANVAS_MAX_WHEEL_ZOOM_FACTOR);
    }

    /// Opacity of the title labels at `zoom`: 0 at or above `title-labels-below`, rising
    /// linearly to 1 at [`TITLE_LABEL_FADE`] below it.
    pub fn title_label_alpha(&self, zoom: f64) -> f64 {
        if !self.title_labels || !zoom.is_finite() {
            return 0.;
        }
        ((self.title_labels_below - zoom) / TITLE_LABEL_FADE).clamp(0., 1.)
    }

    /// Whether Mod+scroll over a canvas zooms natively (instead of going to the wheel binds).
    pub fn wheel_zoom_enabled(&self) -> bool {
        self.wheel_zoom_factor > 1. + 1e-6
    }
}

/// One `canvas {}` section as parsed from the config.
#[derive(knuffel::Decode, Debug, Default, Clone, PartialEq)]
pub struct CanvasPart {
    #[knuffel(child, unwrap(argument))]
    pub default_zoom: Option<FloatOrInt<0, 4>>,
    #[knuffel(child, unwrap(arguments))]
    pub zoom_steps: Option<Vec<FloatOrInt<0, 4>>>,
    #[knuffel(child, unwrap(argument))]
    pub pan_step: Option<FloatOrInt<1, 65535>>,
    #[knuffel(child, unwrap(argument))]
    pub wheel_zoom_factor: Option<FloatOrInt<1, 4>>,
    #[knuffel(child, unwrap(argument))]
    pub smooth_zoom_out: Option<bool>,
    #[knuffel(child, unwrap(argument))]
    pub title_labels: Option<bool>,
    #[knuffel(child, unwrap(argument))]
    pub title_labels_below: Option<FloatOrInt<0, 4>>,
    #[knuffel(child, unwrap(argument))]
    pub title_labels_size: Option<FloatOrInt<4, 72>>,
    /// ashd: plane grid and named regions, see `crate::decorations`.
    #[knuffel(flatten(child))]
    pub decor: crate::decorations::CanvasDecorPart,
}

impl MergeWith<CanvasPart> for Canvas {
    fn merge_with(&mut self, part: &CanvasPart) {
        if let Some(x) = part.default_zoom {
            self.default_zoom = x.0;
        }
        if let Some(steps) = &part.zoom_steps {
            self.zoom_steps = steps.iter().map(|x| x.0).collect();
        }
        if let Some(x) = part.pan_step {
            self.pan_step = x.0;
        }
        if let Some(x) = part.wheel_zoom_factor {
            self.wheel_zoom_factor = x.0;
        }
        if let Some(x) = part.smooth_zoom_out {
            self.smooth_zoom_out = x;
        }
        if let Some(x) = part.title_labels {
            self.title_labels = x;
        }
        if let Some(x) = part.title_labels_below {
            self.title_labels_below = x.0;
        }
        if let Some(x) = part.title_labels_size {
            self.title_labels_size = x.0;
        }
        self.decor.merge_with(&part.decor); // ashd
        self.normalize();
    }
}

/// Animation of canvas panning and flying to a window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasPanAnim(pub Animation);

impl Default for CanvasPanAnim {
    fn default() -> Self {
        Self(Animation {
            off: false,
            kind: Kind::Spring(SpringParams {
                damping_ratio: 1.,
                stiffness: 500,
                epsilon: 0.0001,
            }),
        })
    }
}

/// Animation of canvas zooming.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasZoomAnim(pub Animation);

impl Default for CanvasZoomAnim {
    fn default() -> Self {
        Self(Animation {
            off: false,
            kind: Kind::Spring(SpringParams {
                damping_ratio: 1.,
                stiffness: 600,
                epsilon: 0.0001,
            }),
        })
    }
}

/// Animation of the native Mod+scroll zoom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasWheelZoomAnim(pub Animation);

impl Default for CanvasWheelZoomAnim {
    fn default() -> Self {
        Self(Animation {
            off: false,
            kind: Kind::Spring(SpringParams {
                damping_ratio: 1.,
                stiffness: 1200,
                epsilon: 0.0001,
            }),
        })
    }
}

impl<S> knuffel::Decode<S> for CanvasWheelZoomAnim
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let default = Self::default().0;
        Ok(Self(Animation::decode_node(node, ctx, default, |_, _| {
            Ok(false)
        })?))
    }
}

impl<S> knuffel::Decode<S> for CanvasPanAnim
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let default = Self::default().0;
        Ok(Self(Animation::decode_node(node, ctx, default, |_, _| {
            Ok(false)
        })?))
    }
}

impl<S> knuffel::Decode<S> for CanvasZoomAnim
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        let default = Self::default().0;
        Ok(Self(Animation::decode_node(node, ctx, default, |_, _| {
            Ok(false)
        })?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animations::EasingParams;
    use crate::Config;

    #[test]
    fn canvas_defaults_without_config() {
        let config = Config::parse_mem("").unwrap();
        assert_eq!(config.layout.default_workspace_mode, WorkspaceMode::Strip);
        assert_eq!(config.layout.canvas, Canvas::default());
        assert_eq!(config.animations.canvas_pan, CanvasPanAnim::default());
        assert_eq!(config.animations.canvas_zoom, CanvasZoomAnim::default());
        assert_eq!(
            config.animations.canvas_wheel_zoom,
            CanvasWheelZoomAnim::default()
        );
        assert_eq!(config.layout.canvas.wheel_zoom_factor, 1.2);
        assert!(config.layout.canvas.smooth_zoom_out);
        assert!(config.layout.canvas.title_labels);
        assert_eq!(config.layout.canvas.title_labels_below, 0.7);
        assert_eq!(config.layout.canvas.title_labels_size, 12.);
    }

    #[test]
    fn parse_title_labels_and_fade() {
        let config = Config::parse_mem(
            r#"
            layout {
                canvas {
                    title-labels-below 0.5
                    title-labels-size 14
                }
            }
            "#,
        )
        .unwrap();
        let canvas = &config.layout.canvas;
        assert_eq!(canvas.title_labels_size, 14.);
        assert_eq!(canvas.title_label_alpha(0.5), 0.);
        assert_eq!(canvas.title_label_alpha(0.62), 0.);
        assert!((canvas.title_label_alpha(0.45) - 0.5).abs() < 1e-9);
        assert!((canvas.title_label_alpha(0.4) - 1.).abs() < 1e-9);
        assert_eq!(canvas.title_label_alpha(0.1), 1.);

        let config = Config::parse_mem("layout { canvas { title-labels false; }; }").unwrap();
        assert_eq!(config.layout.canvas.title_label_alpha(0.1), 0.);
        assert!(Config::parse_mem("layout { canvas { title-labels-size 2; }; }").is_err());
    }

    #[test]
    fn parse_wheel_zoom_and_filter() {
        let config = Config::parse_mem(
            r#"
            layout {
                canvas {
                    wheel-zoom-factor 1.25
                    smooth-zoom-out false
                }
            }
            animations {
                canvas-wheel-zoom {
                    spring damping-ratio=1.0 stiffness=2000 epsilon=0.0001
                }
            }
            "#,
        )
        .unwrap();
        assert_eq!(config.layout.canvas.wheel_zoom_factor, 1.25);
        assert!(config.layout.canvas.wheel_zoom_enabled());
        assert!(!config.layout.canvas.smooth_zoom_out);
        assert_eq!(
            config.animations.canvas_wheel_zoom.0,
            Animation {
                off: false,
                kind: Kind::Spring(SpringParams {
                    damping_ratio: 1.,
                    stiffness: 2000,
                    epsilon: 0.0001,
                }),
            }
        );

        // 1 turns the native wheel zoom off; out-of-range values are rejected.
        let config = Config::parse_mem("layout { canvas { wheel-zoom-factor 1; }; }").unwrap();
        assert!(!config.layout.canvas.wheel_zoom_enabled());
        assert!(Config::parse_mem("layout { canvas { wheel-zoom-factor 0.5; }; }").is_err());
        assert!(Config::parse_mem("layout { canvas { wheel-zoom-factor 5; }; }").is_err());
    }

    #[test]
    fn parse_canvas_layout() {
        let config = Config::parse_mem(
            r#"
            layout {
                default-workspace-mode "canvas"
                canvas {
                    default-zoom 0.5
                    zoom-steps 1 0.25 0.5 0.5
                    pan-step 200
                }
            }
            "#,
        )
        .unwrap();

        assert_eq!(config.layout.default_workspace_mode, WorkspaceMode::Canvas);
        assert_eq!(
            config.layout.canvas,
            Canvas {
                default_zoom: 0.5,
                // Sorted and deduplicated.
                zoom_steps: vec![0.25, 0.5, 1.0],
                pan_step: 200.,
                ..Default::default() // ashd
            }
        );
    }

    #[test]
    fn canvas_default_zoom_is_clamped_to_steps() {
        let config = Config::parse_mem(
            r#"
            layout {
                canvas {
                    default-zoom 0.1
                    zoom-steps 0.5 0.75
                }
            }
            "#,
        )
        .unwrap();
        assert_eq!(config.layout.canvas.default_zoom, 0.5);
    }

    #[test]
    fn canvas_parts_merge() {
        // Parts merge field by field, like per-output and per-workspace layout overrides.
        let mut canvas = Canvas::default();
        canvas.merge_with(&CanvasPart {
            pan_step: Some(FloatOrInt(300.)),
            ..Default::default()
        });
        canvas.merge_with(&CanvasPart {
            default_zoom: Some(FloatOrInt(0.75)),
            ..Default::default()
        });
        assert_eq!(
            canvas,
            Canvas {
                default_zoom: 0.75,
                zoom_steps: Canvas::default().zoom_steps,
                pan_step: 300.,
                ..Default::default() // ashd
            }
        );

        // Changing the steps re-clamps the default zoom.
        canvas.merge_with(&CanvasPart {
            zoom_steps: Some(vec![FloatOrInt(0.25), FloatOrInt(0.5)]),
            ..Default::default()
        });
        assert_eq!(canvas.default_zoom, 0.5);
    }

    #[test]
    fn parse_invalid_workspace_mode() {
        assert!(Config::parse_mem(r#"layout { default-workspace-mode "grid"; }"#).is_err());
    }

    #[test]
    fn parse_canvas_animations() {
        let config = Config::parse_mem(
            r#"
            animations {
                canvas-pan {
                    spring damping-ratio=0.8 stiffness=300 epsilon=0.001
                }
                canvas-zoom {
                    duration-ms 200
                    curve "linear"
                }
            }
            "#,
        )
        .unwrap();

        assert_eq!(
            config.animations.canvas_pan.0,
            Animation {
                off: false,
                kind: Kind::Spring(SpringParams {
                    damping_ratio: 0.8,
                    stiffness: 300,
                    epsilon: 0.001,
                }),
            }
        );
        assert_eq!(
            config.animations.canvas_zoom.0,
            Animation {
                off: false,
                kind: Kind::Easing(EasingParams {
                    duration_ms: 200,
                    curve: crate::animations::Curve::Linear,
                }),
            }
        );
    }

    #[test]
    fn parse_canvas_binds() {
        use crate::binds::{Action, BoundAction};

        let config = Config::parse_mem(
            r#"
            binds {
                Mod+C { toggle-workspace-mode; }
                Mod+Shift+C { set-workspace-mode "canvas"; }
                Mod+Left { canvas-pan-left; }
                Mod+Right { canvas-pan-right; }
                Mod+Up { canvas-pan-up; }
                Mod+Down { canvas-pan-down; }
                Mod+H { canvas-pan-to -100 250.5; }
                Mod+Equal { canvas-zoom-in; }
                Mod+Minus { canvas-zoom-out; }
                Mod+0 { canvas-set-zoom 1; }
                Mod+F { canvas-fit; }
                Mod+Home { canvas-home; }
                Mod+G { canvas-fly-to-window; }
                Mod+P { toggle-window-pinned; }
                Mod+WheelScrollUp cooldown-ms=50 { canvas-zoom-in at-cursor=true; }
                Mod+WheelScrollDown { canvas-zoom-out at-cursor=true; }
                Mod+Ctrl+WheelScrollDown cooldown-ms=150 {
                    if-canvas {
                        canvas-zoom-out at-cursor=true
                        focus-workspace-down
                    }
                }
                Mod+Ctrl+WheelScrollUp { if-canvas { canvas-zoom-in; }; }
            }
            "#,
        )
        .unwrap();

        let actions: Vec<_> = config
            .binds
            .0
            .iter()
            .map(|b| match &b.action {
                BoundAction::Press(action) => action.clone(),
                other => panic!("unexpected bound action: {other:?}"),
            })
            .collect();
        assert_eq!(
            actions,
            vec![
                Action::ToggleWorkspaceMode,
                Action::SetWorkspaceMode(WorkspaceMode::Canvas),
                Action::CanvasPanLeft,
                Action::CanvasPanRight,
                Action::CanvasPanUp,
                Action::CanvasPanDown,
                Action::CanvasPanTo(FloatOrInt(-100.), FloatOrInt(250.5)),
                Action::CanvasZoomIn(false),
                Action::CanvasZoomOut(false),
                Action::CanvasSetZoom(FloatOrInt(1.)),
                Action::CanvasFit,
                Action::CanvasHome,
                Action::CanvasFlyToWindow,
                Action::ToggleWindowPinned,
                Action::CanvasZoomIn(true),
                Action::CanvasZoomOut(true),
                Action::IfCanvas(vec![
                    Action::CanvasZoomOut(true),
                    Action::FocusWorkspaceDown
                ]),
                Action::IfCanvas(vec![Action::CanvasZoomIn(false)]),
            ]
        );
    }

    #[test]
    fn if_canvas_arity() {
        let config = Config::parse_mem(
            r#"
            binds {
                Mod+X { if-canvas { canvas-fit; canvas-home; canvas-zoom-in; }; }
            }
            "#,
        );
        assert!(config.is_err());

        let config = Config::parse_mem(
            r#"
            binds {
                Mod+X { if-canvas; }
            }
            "#,
        );
        assert!(config.is_err());
    }
}
