//! Types for the ashd fork's per-workspace infinite canvas mode.
//!
//! A workspace is either in [`WorkspaceMode::Strip`] (stock niri scrollable tiling) or in
//! [`WorkspaceMode::Canvas`], where all normal windows float on an unbounded 2D plane and the
//! output shows a pannable, zoomable viewport onto it.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Window arrangement mode of a workspace.
#[derive(Serialize, Deserialize, Debug, Default, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub enum WorkspaceMode {
    /// Stock niri scrollable tiling (columns on an infinite horizontal strip).
    #[default]
    Strip,
    /// Infinite canvas: windows float on an unbounded plane, viewed through a pannable and
    /// zoomable viewport.
    Canvas,
}

impl FromStr for WorkspaceMode {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "strip" => Ok(Self::Strip),
            "canvas" => Ok(Self::Canvas),
            _ => Err(r#"invalid workspace mode, can be "strip" or "canvas""#),
        }
    }
}

/// View state of a canvas-mode workspace.
///
/// The view maps plane coordinates (where canvas windows live, see
/// [`WindowLayout::pos_in_canvas`](crate::WindowLayout::pos_in_canvas)) to workspace view
/// coordinates (logical pixels relative to the top-left corner of the workspace on its output):
///
/// ```text
/// view  = (plane - offset) * zoom
/// plane = view / zoom + offset
/// ```
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct CanvasViewState {
    /// Id of the workspace.
    pub workspace_id: u64,
    /// Plane point currently shown at the top-left corner of the workspace view.
    pub offset: (f64, f64),
    /// Current zoom (scale from plane to view); 1.0 shows windows at their real size.
    pub zoom: f64,
    /// Offset that the view is animating towards (equal to `offset` when settled).
    pub target_offset: (f64, f64),
    /// Zoom that the view is animating towards (equal to `zoom` when settled).
    pub target_zoom: f64,
    /// Size of the workspace view in logical pixels (the output size).
    pub view_size: (f64, f64),
    /// Whether the view is currently animating towards the target.
    pub animating: bool,
    /// Named regions on the plane of this workspace (from the config and
    /// `canvas-add-region`), in drawing order.
    #[serde(default)]
    pub regions: Vec<CanvasRegion>,
}

/// A named rectangle on the canvas plane (drawn dashed with a label under the windows).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct CanvasRegion {
    /// Name, shown as the label.
    pub name: String,
    /// Plane X of the top-left corner.
    pub x: f64,
    /// Plane Y of the top-left corner.
    pub y: f64,
    /// Width in plane units (logical pixels at zoom 1).
    pub w: f64,
    /// Height in plane units.
    pub h: f64,
}

/// Scroll state of a strip-mode (scrolling layout) workspace.
///
/// All values are logical pixels in *strip coordinates*: x = 0 is the left edge of the first
/// column, columns follow each other separated by the configured gaps. A column with strip x
/// `column_xs[i]` currently appears at workspace view x `column_xs[i] - view_offset`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
pub struct StripViewState {
    /// Id of the workspace.
    pub workspace_id: u64,
    /// Strip x shown at the left edge of the workspace view right now (animated).
    pub view_offset: f64,
    /// Strip x that the view will settle at.
    pub target_view_offset: f64,
    /// Width of the workspace working area (output minus exclusive layer-shell zones).
    pub working_area_width: f64,
    /// Strip x of every column, in column order (index 0 is column 1 of
    /// [`WindowLayout::pos_in_scrolling_layout`](crate::WindowLayout::pos_in_scrolling_layout)).
    #[serde(default)]
    pub column_xs: Vec<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_workspace_mode() {
        assert_eq!("strip".parse(), Ok(WorkspaceMode::Strip));
        assert_eq!("canvas".parse(), Ok(WorkspaceMode::Canvas));
        assert!("Canvas".parse::<WorkspaceMode>().is_err());
    }

    #[test]
    fn workspace_mode_defaults_for_old_clients() {
        // Old servers do not send `mode`; new clients must still parse their replies.
        #[derive(Deserialize)]
        struct Probe {
            #[serde(default)]
            mode: WorkspaceMode,
        }
        let probe: Probe = serde_json::from_str("{}").unwrap();
        assert_eq!(probe.mode, WorkspaceMode::Strip);
    }

    #[test]
    fn canvas_view_state_round_trip() {
        let view = CanvasViewState {
            workspace_id: 3,
            offset: (-120.5, 40.),
            zoom: 0.62,
            target_offset: (0., 0.),
            target_zoom: 0.5,
            view_size: (1920., 1080.),
            animating: true,
            regions: vec![CanvasRegion {
                name: String::from("notes"),
                x: -400.,
                y: 0.,
                w: 800.,
                h: 600.,
            }],
        };
        let json = serde_json::to_string(&view).unwrap();
        assert_eq!(
            json,
            r#"{"workspace_id":3,"offset":[-120.5,40.0],"zoom":0.62,"target_offset":[0.0,0.0],"target_zoom":0.5,"view_size":[1920.0,1080.0],"animating":true,"regions":[{"name":"notes","x":-400.0,"y":0.0,"w":800.0,"h":600.0}]}"#
        );
        let back: CanvasViewState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, view);

        // `regions` is optional.
        let json = r#"{"workspace_id":3,"offset":[0,0],"zoom":1,"target_offset":[0,0],"target_zoom":1,"view_size":[10,10],"animating":false}"#;
        let back: CanvasViewState = serde_json::from_str(json).unwrap();
        assert!(back.regions.is_empty());
    }

    #[test]
    fn strip_view_state_round_trip() {
        let view = StripViewState {
            workspace_id: 1,
            view_offset: 812.25,
            target_view_offset: 900.,
            working_area_width: 1920.,
            column_xs: vec![0., 976., 1952.],
        };
        let json = serde_json::to_string(&view).unwrap();
        let back: StripViewState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, view);

        // column_xs is optional.
        let back: StripViewState = serde_json::from_str(
            r#"{"workspace_id":1,"view_offset":0,"target_view_offset":0,"working_area_width":10}"#,
        )
        .unwrap();
        assert!(back.column_xs.is_empty());
    }
}
