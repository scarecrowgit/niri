//! rice: infinite canvas view for canvas-mode workspaces.
//!
//! A canvas-mode workspace keeps all of its normal windows in its `FloatingSpace`, switched into
//! *unbounded* mode where tile positions are logical coordinates on an infinite 2D plane. The
//! output shows a viewport onto that plane, defined by a [`CanvasTransform`]:
//!
//! ```text
//! view = (plane - offset) * zoom          plane = view / zoom + offset
//! ```
//!
//! Three coordinate spaces are involved, all in logical pixels:
//!
//! - **plane**: unbounded canvas coordinates. Window positions live here and do not change when the
//!   view pans or zooms.
//! - **unscaled view** (`u`): `plane - offset`. This is what the floating space reports to the rest
//!   of the layout (`tiles_with_offsets()`, `tiles_with_render_positions()`, hit-testing positions,
//!   popup target rects). At zoom 1 it equals the view, so all stock code paths work unchanged at
//!   100%.
//! - **view**: workspace view coordinates as seen on the output, `u * zoom`. Rendering applies the
//!   zoom with a `RescaleRenderElement` around the workspace origin.
//!
//! [`CanvasView`] holds the current and target transforms and animates between them with
//! springs. Pan and zoom animate separately (pan spring, zoom spring) around a *focus* point in
//! view coordinates: the plane point under the focus moves linearly from its start to its target
//! value with the pan progress, while the zoom follows its own spring, and the offset is derived
//! from both. With a fixed plane point under the focus this reduces to zooming around an anchor.

use std::time::Duration;

use niri_config::{Canvas as CanvasConfig, WorkspaceMode};
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::monitor::InsertWorkspace;
use super::scrolling::StripArrangement;
use super::workspace::{Workspace, WorkspaceId};
use super::{InteractiveMoveData, Layout, LayoutElement, MonitorSet};
use crate::animation::{Animation, Clock};
use crate::input::swipe_tracker::SwipeTracker;

/// Padding around the windows for "fit", in logical view pixels.
pub const CANVAS_FIT_PADDING: f64 = 48.;

/// Margin kept between a keyboard-moved window and the viewport edge for edge-push, in logical
/// view pixels.
pub const CANVAS_EDGE_PUSH_MARGIN: f64 = 16.;

/// Offset between consecutively opened windows that would otherwise land on the same spot, in
/// logical plane pixels.
pub const CANVAS_CASCADE_STEP: f64 = 32.;

/// Zoom values closer than this are considered equal.
const ZOOM_EPSILON: f64 = 1e-6;

/// Pan/zoom state of a canvas view: `view = (plane - offset) * zoom`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasTransform {
    /// Plane point shown at the top-left corner of the workspace view.
    pub offset: Point<f64, Logical>,
    /// Scale from plane to view; 1.0 shows windows at their real size.
    pub zoom: f64,
}

impl Default for CanvasTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl CanvasTransform {
    /// No pan, no zoom: plane coordinates equal view coordinates.
    pub const IDENTITY: Self = Self {
        offset: Point::new(0., 0.),
        zoom: 1.,
    };

    pub fn new(offset: Point<f64, Logical>, zoom: f64) -> Self {
        Self { offset, zoom }
    }

    /// Transform that shows `plane` at `view_point`, at the given zoom.
    pub fn centered_on(
        plane: Point<f64, Logical>,
        view_point: Point<f64, Logical>,
        zoom: f64,
    ) -> Self {
        Self {
            offset: plane - view_point.downscale(zoom),
            zoom,
        }
    }

    /// Converts workspace view coordinates to plane coordinates.
    pub fn view_to_plane(&self, view: Point<f64, Logical>) -> Point<f64, Logical> {
        view.downscale(self.zoom) + self.offset
    }

    /// Converts plane coordinates to workspace view coordinates.
    pub fn plane_to_view(&self, plane: Point<f64, Logical>) -> Point<f64, Logical> {
        (plane - self.offset).upscale(self.zoom)
    }

    /// Converts plane coordinates to unscaled view coordinates (`plane - offset`), the
    /// coordinates that the floating space reports in canvas mode.
    pub fn plane_to_unscaled(&self, plane: Point<f64, Logical>) -> Point<f64, Logical> {
        plane - self.offset
    }

    /// Converts unscaled view coordinates back to plane coordinates.
    pub fn unscaled_to_plane(&self, unscaled: Point<f64, Logical>) -> Point<f64, Logical> {
        unscaled + self.offset
    }

    /// Converts a view rectangle to the plane rectangle it shows.
    pub fn view_rect_to_plane(&self, rect: Rectangle<f64, Logical>) -> Rectangle<f64, Logical> {
        Rectangle::new(self.view_to_plane(rect.loc), rect.size.downscale(self.zoom))
    }

    /// Converts a plane rectangle to where it appears in the view.
    pub fn plane_rect_to_view(&self, rect: Rectangle<f64, Logical>) -> Rectangle<f64, Logical> {
        Rectangle::new(self.plane_to_view(rect.loc), rect.size.upscale(self.zoom))
    }

    /// Plane rectangle visible in a view of the given size.
    pub fn visible_plane_rect(&self, view_size: Size<f64, Logical>) -> Rectangle<f64, Logical> {
        self.view_rect_to_plane(Rectangle::from_size(view_size))
    }

    /// Same transform zoomed to `zoom` such that the plane point under `anchor` (view
    /// coordinates) stays under `anchor`.
    pub fn zoomed_around(&self, anchor: Point<f64, Logical>, zoom: f64) -> Self {
        Self::centered_on(self.view_to_plane(anchor), anchor, zoom)
    }

    /// Same transform panned by `delta` in view pixels.
    pub fn panned_by_view(&self, delta: Point<f64, Logical>) -> Self {
        Self {
            offset: self.offset + delta.downscale(self.zoom),
            zoom: self.zoom,
        }
    }

    pub fn is_finite(&self) -> bool {
        self.offset.x.is_finite() && self.offset.y.is_finite() && self.zoom.is_finite()
    }

    fn approx_eq(&self, other: &Self) -> bool {
        (self.zoom - other.zoom).abs() < ZOOM_EPSILON
            && (self.offset.x - other.offset.x).abs() < 1e-6
            && (self.offset.y - other.offset.y).abs() < 1e-6
    }
}

/// Animated canvas view: current transform, target transform and the animation between them.
#[derive(Debug, Clone)]
pub struct CanvasView {
    /// Transform as of the last `advance()`; this is what is rendered and hit-tested.
    current: CanvasTransform,
    /// Transform that the view is animating towards (equal to `current` when static).
    target: CanvasTransform,
    /// Ongoing animation.
    anim: Option<ViewAnimation>,
}

#[derive(Debug, Clone)]
struct ViewAnimation {
    /// View point around which the animation is computed.
    focus: Point<f64, Logical>,
    /// Plane point under `focus` at the start.
    from_plane: Point<f64, Logical>,
    /// Plane point under `focus` at the target.
    to_plane: Point<f64, Logical>,
    /// Pan progress 0 → 1, or `None` when the plane point under the focus does not move.
    pan: Option<Animation>,
    /// Zoom value animation, or `None` when the zoom does not change.
    zoom: Option<Animation>,
}

impl CanvasView {
    pub fn new(transform: CanvasTransform) -> Self {
        Self {
            current: transform,
            target: transform,
            anim: None,
        }
    }

    /// Transform as currently displayed.
    pub fn current(&self) -> CanvasTransform {
        self.current
    }

    /// Transform that the view will settle at.
    pub fn target(&self) -> CanvasTransform {
        self.target
    }

    pub fn is_animating(&self) -> bool {
        self.anim.is_some()
    }

    /// Whether the zoom (as opposed to only the offset) is still animating.
    pub fn is_zoom_animating(&self) -> bool {
        self.anim.as_ref().is_some_and(|anim| anim.zoom.is_some())
    }

    /// Sets the view instantly, cancelling any animation.
    pub fn jump_to(&mut self, transform: CanvasTransform) {
        self.current = transform;
        self.target = transform;
        self.anim = None;
    }

    /// Starts animating from the current transform to `target` around the view point `focus`.
    ///
    /// Animations that are `off` in the config complete instantly.
    pub fn animate_to(
        &mut self,
        clock: &Clock,
        target: CanvasTransform,
        focus: Point<f64, Logical>,
        pan_config: niri_config::Animation,
        zoom_config: niri_config::Animation,
    ) {
        self.animate_to_with_velocity(clock, target, focus, 0., pan_config, zoom_config);
    }

    /// Like [`Self::animate_to`], with an initial pan velocity in pan progress per second (the
    /// pan progress goes from 0 to 1).
    fn animate_to_with_velocity(
        &mut self,
        clock: &Clock,
        target: CanvasTransform,
        focus: Point<f64, Logical>,
        pan_velocity: f64,
        pan_config: niri_config::Animation,
        zoom_config: niri_config::Animation,
    ) {
        let current = self.current;
        if current.approx_eq(&target) {
            self.jump_to(target);
            return;
        }

        let from_plane = current.view_to_plane(focus);
        let to_plane = target.view_to_plane(focus);

        let pan_moves =
            (from_plane.x - to_plane.x).abs() > 1e-6 || (from_plane.y - to_plane.y).abs() > 1e-6;
        let zoom_changes = (current.zoom - target.zoom).abs() > ZOOM_EPSILON;

        let pan = (pan_moves && !pan_config.off)
            .then(|| Animation::new(clock.clone(), 0., 1., pan_velocity, pan_config));
        let zoom = (zoom_changes && !zoom_config.off)
            .then(|| Animation::new(clock.clone(), current.zoom, target.zoom, 0., zoom_config));

        if pan.is_none() && zoom.is_none() {
            self.jump_to(target);
            return;
        }

        self.target = target;
        self.anim = Some(ViewAnimation {
            focus,
            from_plane,
            to_plane,
            pan,
            zoom,
        });
        // Keep current consistent with a possibly partial animation (e.g. pan off, zoom on).
        self.advance();
    }

    /// Continues a released pan gesture: glides by `plane_delta` starting at `plane_speed`
    /// plane pixels per second along it. Does nothing while animating or with animations off.
    pub fn glide(
        &mut self,
        clock: &Clock,
        plane_delta: Point<f64, Logical>,
        plane_speed: f64,
        focus: Point<f64, Logical>,
        pan_config: niri_config::Animation,
    ) {
        let dist = plane_delta.x.hypot(plane_delta.y);
        if self.anim.is_some() || pan_config.off || dist < 1e-6 || !plane_speed.is_finite() {
            return;
        }
        let target = CanvasTransform::new(self.current.offset + plane_delta, self.current.zoom);
        let velocity = plane_speed / dist;
        self.animate_to_with_velocity(clock, target, focus, velocity, pan_config, pan_config);
    }

    /// Moves the view by `delta` plane pixels right away, keeping any ongoing animation (the
    /// plane points it moves between shift along with it).
    pub fn pan_by_plane(&mut self, delta: Point<f64, Logical>) {
        if !(delta.x.is_finite() && delta.y.is_finite()) {
            return;
        }
        self.current.offset += delta;
        self.target.offset += delta;
        if let Some(anim) = &mut self.anim {
            anim.from_plane += delta;
            anim.to_plane += delta;
        }
    }

    /// Advances the animation to the clock's current time. Returns whether anything changed.
    pub fn advance(&mut self) -> bool {
        let Some(anim) = &self.anim else {
            return false;
        };

        let pan_done = anim.pan.as_ref().is_none_or(Animation::is_done);
        let zoom_done = anim.zoom.as_ref().is_none_or(Animation::is_done);
        if pan_done && zoom_done {
            self.current = self.target;
            self.anim = None;
            return true;
        }

        let progress = anim.pan.as_ref().map_or(1., Animation::value);
        let zoom = anim
            .zoom
            .as_ref()
            .map_or(self.target.zoom, Animation::value)
            // Protect against degenerate springs.
            .max(0.01);
        let plane = anim.from_plane + (anim.to_plane - anim.from_plane).upscale(progress);
        let current = CanvasTransform::centered_on(plane, anim.focus, zoom);

        let changed = current != self.current;
        self.current = current;
        changed
    }
}

/// Zoom step to go to from `current` when zooming in or out through `steps` (sorted ascending).
///
/// Values not exactly on a step go to the nearest step in the requested direction; at the end of
/// the range the boundary step is returned.
pub fn next_zoom_step(steps: &[f64], current: f64, zoom_in: bool) -> f64 {
    const EPS: f64 = 1e-3;
    let (Some(&min), Some(&max)) = (steps.first(), steps.last()) else {
        return current;
    };
    if zoom_in {
        steps
            .iter()
            .copied()
            .find(|step| *step > current + EPS)
            .unwrap_or(max)
    } else {
        steps
            .iter()
            .rev()
            .copied()
            .find(|step| *step < current - EPS)
            .unwrap_or(min)
    }
}

/// Transform that shows `bbox` (plane) centred in `area` (view) with `padding`, zoom clamped to
/// `[min_zoom, min(max_zoom, 1.0)]`.
pub fn fit_transform(
    bbox: Rectangle<f64, Logical>,
    area: Rectangle<f64, Logical>,
    padding: f64,
    min_zoom: f64,
    max_zoom: f64,
) -> CanvasTransform {
    let avail_w = f64::max(area.size.w - 2. * padding, 1.);
    let avail_h = f64::max(area.size.h - 2. * padding, 1.);
    let zoom = f64::min(
        avail_w / f64::max(bbox.size.w, 1.),
        avail_h / f64::max(bbox.size.h, 1.),
    );
    let hi = f64::max(f64::min(max_zoom, 1.), min_zoom);
    let zoom = zoom.clamp(min_zoom, hi);
    CanvasTransform::centered_on(rect_center(bbox), rect_center(area), zoom)
}

/// Minimal pan of `transform` so that `rect` (plane) is inside `area` (view) shrunk by `margin`.
///
/// If `rect` is larger than the area along an axis, its left/top edge is aligned instead.
pub fn push_to_contain(
    transform: CanvasTransform,
    rect: Rectangle<f64, Logical>,
    area: Rectangle<f64, Logical>,
    margin: f64,
) -> CanvasTransform {
    let mut area = area;
    let margin_x = f64::min(margin, area.size.w / 4.);
    let margin_y = f64::min(margin, area.size.h / 4.);
    area.loc += Point::from((margin_x, margin_y));
    area.size -= Size::from((margin_x * 2., margin_y * 2.));

    let visible = transform.view_rect_to_plane(area);

    let axis = |start: f64, len: f64, vis_start: f64, vis_len: f64| {
        if len > vis_len || start < vis_start {
            start - vis_start
        } else if start + len > vis_start + vis_len {
            start + len - (vis_start + vis_len)
        } else {
            0.
        }
    };

    let dx = axis(rect.loc.x, rect.size.w, visible.loc.x, visible.size.w);
    let dy = axis(rect.loc.y, rect.size.h, visible.loc.y, visible.size.h);
    CanvasTransform::new(transform.offset + Point::from((dx, dy)), transform.zoom)
}

pub fn rect_center(rect: Rectangle<f64, Logical>) -> Point<f64, Logical> {
    rect.loc + rect.size.downscale(2.).to_point()
}

/// Smallest rectangle containing all `rects`.
pub fn bounding_box(
    rects: impl IntoIterator<Item = Rectangle<f64, Logical>>,
) -> Option<Rectangle<f64, Logical>> {
    rects.into_iter().reduce(|a, b| a.merge(b))
}

/// Border/focus ring width that stays at least one physical pixel wide when scaled by `zoom` on
/// an output with the given scale.
pub fn compensated_frame_width(width: f64, zoom: f64, output_scale: f64) -> f64 {
    if zoom >= 1. || width <= 0. {
        return width;
    }
    let min = (1. / (zoom * output_scale)).ceil();
    f64::max(width, min)
}

/// What a workspace remembers from its last canvas session while it is in strip mode.
#[derive(Debug, Clone)]
pub struct CanvasMemory<Id> {
    /// Target transform at the time canvas mode was left.
    pub transform: CanvasTransform,
    /// Plane positions (top-left of the tile) of the windows at the time canvas mode was left.
    pub plane_positions: Vec<(Id, Point<f64, Logical>)>,
}

/// Per-workspace canvas state, owned by `Workspace`.
#[derive(Debug)]
pub struct WorkspaceCanvas<Id> {
    pub(super) mode: WorkspaceMode,
    /// View of the plane. Meaningful only in canvas mode.
    pub(super) view: CanvasView,
    /// Strip arrangement saved when entering canvas mode; `Some` exactly in canvas mode.
    pub(super) strip: Option<StripArrangement<Id>>,
    /// Remembered from the last canvas session, while in strip mode.
    pub(super) memory: Option<CanvasMemory<Id>>,
    /// Zoom that the floating options' frame widths are currently compensated for.
    pub(super) frame_zoom: f64,
    /// Plane position of the last window placed by the new-window cascade.
    pub(super) cascade: Option<Point<f64, Logical>>,
    /// Ongoing direct-manipulation pan (mouse drag or touchpad swipe).
    pub(super) pan_gesture: Option<CanvasPanGesture>,
    /// Zoom at the start of an ongoing pinch zoom gesture.
    pub(super) zoom_gesture: Option<f64>,
    /// rice: plane grid and named regions (runtime regions and render state).
    pub(super) decor: super::canvas_grid::CanvasDecorations,
    /// rice: working area outline and clip (not canvas-specific; per-workspace rice state).
    pub(super) work_area: super::work_area::WorkAreaDecor,
}

/// State of a direct-manipulation pan of a canvas view.
#[derive(Debug)]
pub struct CanvasPanGesture {
    pub(super) tracker_x: SwipeTracker,
    pub(super) tracker_y: SwipeTracker,
    pub(super) is_touchpad: bool,
    /// Factor from gesture units to view pixels.
    pub(super) norm_factor: f64,
}

/// Touchpad swipe distance that pans the canvas by one working area width, like the scrolling
/// view-offset gesture.
pub const CANVAS_TOUCHPAD_GESTURE_MOVEMENT: f64 = 1200.;

impl<Id> Default for WorkspaceCanvas<Id> {
    fn default() -> Self {
        Self {
            mode: WorkspaceMode::Strip,
            view: CanvasView::new(CanvasTransform::IDENTITY),
            strip: None,
            memory: None,
            frame_zoom: 1.,
            cascade: None,
            pan_gesture: None,
            zoom_gesture: None,
            decor: Default::default(),     // rice
            work_area: Default::default(), // rice
        }
    }
}

impl<Id> WorkspaceCanvas<Id> {
    pub fn mode(&self) -> WorkspaceMode {
        self.mode
    }

    pub fn is_canvas(&self) -> bool {
        self.mode == WorkspaceMode::Canvas
    }
}

/// Direction of a canvas pan step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasPanDirection {
    Left,
    Right,
    Up,
    Down,
}

impl CanvasPanDirection {
    /// Unit vector of the view movement (the view moves, windows appear to move the other way).
    pub fn unit(self) -> Point<f64, Logical> {
        match self {
            CanvasPanDirection::Left => Point::from((-1., 0.)),
            CanvasPanDirection::Right => Point::from((1., 0.)),
            CanvasPanDirection::Up => Point::from((0., -1.)),
            CanvasPanDirection::Down => Point::from((0., 1.)),
        }
    }
}

/// Clamp helper shared by the workspace code.
pub fn clamp_zoom(config: &CanvasConfig, zoom: f64) -> f64 {
    config.clamp_zoom(zoom)
}

/// Layout-level canvas API, operating on the focused workspace unless a window is given.
///
/// Canvas-only operations are no-ops on strip workspaces.
impl<W: LayoutElement> Layout<W> {
    /// Toggles the focused workspace between strip and canvas mode.
    pub fn toggle_workspace_mode(&mut self) {
        if let Some(ws) = self.active_workspace_mut() {
            ws.toggle_mode();
        }
    }

    /// Sets the mode of the focused workspace.
    pub fn set_workspace_mode(&mut self, mode: WorkspaceMode) {
        if let Some(ws) = self.active_workspace_mut() {
            ws.set_mode(mode);
        }
    }

    /// Pans the focused canvas workspace by one pan step.
    pub fn canvas_pan(&mut self, direction: CanvasPanDirection) {
        if let Some(ws) = self.active_workspace_mut() {
            ws.canvas_pan(direction);
        }
    }

    /// Pans the focused canvas workspace so that its view offset becomes `plane`.
    pub fn canvas_pan_to(&mut self, plane: Point<f64, Logical>) {
        if let Some(ws) = self.active_workspace_mut() {
            ws.canvas_pan_to(plane);
        }
    }

    /// Zooms the focused canvas workspace in by one step, keeping `anchor` (view coordinates
    /// relative to the workspace) fixed; `None` anchors at the working area centre.
    pub fn canvas_zoom_in(&mut self, anchor: Option<Point<f64, Logical>>) {
        if let Some(ws) = self.active_workspace_mut() {
            ws.canvas_zoom_step(true, anchor);
        }
    }

    /// Zooms the focused canvas workspace out by one step, see [`Self::canvas_zoom_in`].
    pub fn canvas_zoom_out(&mut self, anchor: Option<Point<f64, Logical>>) {
        if let Some(ws) = self.active_workspace_mut() {
            ws.canvas_zoom_step(false, anchor);
        }
    }

    /// Sets the zoom of the focused canvas workspace (clamped to the configured range).
    pub fn canvas_set_zoom(&mut self, zoom: f64, anchor: Option<Point<f64, Logical>>) {
        if let Some(ws) = self.active_workspace_mut() {
            ws.canvas_set_zoom(zoom, anchor);
        }
    }

    /// Fits all windows of the focused canvas workspace into the view.
    pub fn canvas_fit(&mut self) {
        if let Some(ws) = self.active_workspace_mut() {
            ws.canvas_fit();
        }
    }

    /// Pans the focused canvas workspace back to offset (0, 0), keeping the zoom.
    pub fn canvas_home(&mut self) {
        if let Some(ws) = self.active_workspace_mut() {
            ws.canvas_home();
        }
    }

    /// Centres the view of the window's canvas workspace on the window (focused window if
    /// `None`).
    pub fn canvas_fly_to_window(&mut self, window: Option<&W::Id>) {
        let id = match window {
            Some(id) => id.clone(),
            None => match self.focus() {
                Some(win) => win.id().clone(),
                None => return,
            },
        };
        if let Some(ws) = self.workspaces_mut().find(|ws| ws.has_window(&id)) {
            ws.canvas_fly_to_window(&id);
        }
    }

    /// Recentres the canvas view on a window that just got keyboard focus.
    ///
    /// Called for keyboard-driven focus changes only (directional focus, focus by id, alt-tab);
    /// pointer focus must not call this.
    pub fn canvas_reveal_window(&mut self, window: &W::Id) {
        if let Some(ws) = self.workspaces_mut().find(|ws| ws.has_window(window)) {
            ws.canvas_reveal_window(window);
        }
    }

    /// Pans the canvas view along with a keyboard-moved window that left the viewport.
    pub fn canvas_edge_push(&mut self, window: &W::Id) {
        if let Some(ws) = self.workspaces_mut().find(|ws| ws.has_window(window)) {
            ws.canvas_edge_push(window);
        }
    }

    /// Toggles pinning of a canvas window (focused window if `None`).
    pub fn toggle_window_pinned(&mut self, window: Option<&W::Id>) {
        let id = match window {
            Some(id) => id.clone(),
            None => match self.focus() {
                Some(win) => win.id().clone(),
                None => return,
            },
        };
        if let Some(ws) = self.workspaces_mut().find(|ws| ws.has_window(&id)) {
            ws.toggle_window_pinned(&id);
        }
    }

    /// Whether the window is on a canvas workspace and pinned.
    pub fn is_window_pinned(&self, window: &W::Id) -> bool {
        self.workspaces()
            .find(|(_, _, ws)| ws.has_window(window))
            .is_some_and(|(_, _, ws)| ws.is_window_pinned(window))
    }

    // -------------------------------------------------------------------------------------------
    // Pointer and gesture input.
    // -------------------------------------------------------------------------------------------

    /// Canvas workspace under a point of an output, with the point in its view coordinates
    /// (the anchor for zoom-at-cursor).
    pub fn canvas_workspace_under(
        &self,
        output: &Output,
        pos_within_output: Point<f64, Logical>,
    ) -> Option<(WorkspaceId, Point<f64, Logical>)> {
        let mon = self.monitor_for_output(output)?;
        let (ws, geo) = mon.workspace_under(pos_within_output)?;
        if !ws.is_canvas() {
            return None;
        }
        let anchor = (pos_within_output - geo.loc).downscale(mon.overview_zoom());
        Some((ws.id(), anchor))
    }

    /// Zooms a canvas workspace in or out by one step around `anchor` (its view coordinates).
    pub fn canvas_zoom_step_at(
        &mut self,
        ws_id: WorkspaceId,
        zoom_in: bool,
        anchor: Option<Point<f64, Logical>>,
    ) {
        if let Some(ws) = self.workspaces_mut().find(|ws| ws.id() == ws_id) {
            ws.canvas_zoom_step(zoom_in, anchor);
        }
    }

    /// Starts panning a canvas workspace by direct manipulation. Cancels other canvas pan
    /// gestures.
    pub fn canvas_pan_gesture_begin(&mut self, ws_id: WorkspaceId, is_touchpad: bool) -> bool {
        let mut started = false;
        for ws in self.workspaces_mut() {
            if ws.id() == ws_id {
                started = ws.canvas_pan_gesture_begin(is_touchpad);
            } else {
                ws.canvas_pan_gesture_end(None);
            }
        }
        started
    }

    /// Pans the canvas with the ongoing pan gesture by `delta` (view pixels for the pointer,
    /// touchpad units for the touchpad; positive moves the view towards positive plane
    /// coordinates).
    ///
    /// Returns `None` if there is no such gesture, and the output to redraw otherwise.
    pub fn canvas_pan_gesture_update(
        &mut self,
        delta: Point<f64, Logical>,
        timestamp: Duration,
        is_touchpad: bool,
    ) -> Option<Option<Output>> {
        let (ws, output) =
            self.canvas_gesture_ws_mut(|ws| ws.has_canvas_pan_gesture(Some(is_touchpad)))?;
        ws.canvas_pan_gesture_update(delta, timestamp, is_touchpad)
            .then_some(output)
    }

    /// Ends the ongoing pan gesture; with a `timestamp` (release time) the view glides on with
    /// the release velocity, without one the gesture is cancelled in place.
    pub fn canvas_pan_gesture_end(
        &mut self,
        timestamp: Option<Duration>,
        is_touchpad: Option<bool>,
    ) -> Option<Option<Output>> {
        let (ws, output) =
            self.canvas_gesture_ws_mut(|ws| ws.has_canvas_pan_gesture(is_touchpad))?;
        ws.canvas_pan_gesture_end(timestamp).then_some(output)
    }

    /// Starts a pinch zoom of a canvas workspace.
    pub fn canvas_zoom_gesture_begin(&mut self, ws_id: WorkspaceId) -> bool {
        let mut started = false;
        for ws in self.workspaces_mut() {
            if ws.id() == ws_id {
                started = ws.canvas_zoom_gesture_begin();
            } else {
                ws.canvas_zoom_gesture_end();
            }
        }
        started
    }

    /// Updates the ongoing pinch zoom: `scale` is relative to the start of the gesture, `anchor`
    /// is the view point to zoom around (pointer position; working area centre if `None`).
    pub fn canvas_zoom_gesture_update(
        &mut self,
        scale: f64,
        anchor: Option<Point<f64, Logical>>,
    ) -> Option<Option<Output>> {
        let (ws, output) = self.canvas_gesture_ws_mut(|ws| ws.has_canvas_zoom_gesture())?;
        ws.canvas_zoom_gesture_update(scale, anchor)
            .then_some(output)
    }

    /// Ends the ongoing pinch zoom.
    pub fn canvas_zoom_gesture_end(&mut self) -> Option<Option<Output>> {
        let (ws, output) = self.canvas_gesture_ws_mut(|ws| ws.has_canvas_zoom_gesture())?;
        ws.canvas_zoom_gesture_end().then_some(output)
    }

    /// Workspace (and its output) matching `pred`.
    fn canvas_gesture_ws_mut(
        &mut self,
        pred: impl Fn(&Workspace<W>) -> bool,
    ) -> Option<(&mut Workspace<W>, Option<Output>)> {
        match &mut self.monitor_set {
            MonitorSet::Normal { monitors, .. } => monitors.iter_mut().find_map(|mon| {
                let output = mon.output.clone();
                mon.workspaces
                    .iter_mut()
                    .find(|ws| pred(ws))
                    .map(|ws| (ws, Some(output)))
            }),
            MonitorSet::NoOutputs { workspaces, .. } => workspaces
                .iter_mut()
                .find(|ws| pred(ws))
                .map(|ws| (ws, None)),
        }
    }

    /// Canvas zoom of the workspace that an interactively moved tile is over (1 elsewhere): the
    /// tile is drawn at that zoom so that it lands exactly where it is dropped.
    pub(super) fn interactive_move_canvas_zoom(&self, move_: &InteractiveMoveData<W>) -> f64 {
        let Some(mon) = self.monitor_for_output(&move_.output) else {
            return 1.;
        };
        let (insert_ws, _) = mon.insert_position(move_.pointer_pos_within_output);
        let InsertWorkspace::Existing(id) = insert_ws else {
            return 1.;
        };
        mon.workspaces
            .iter()
            .find(|ws| ws.id() == id)
            .map_or(1., |ws| ws.canvas_zoom())
    }
}

#[cfg(test)]
mod tests {
    use approx::assert_abs_diff_eq;

    use super::*;

    fn pt(x: f64, y: f64) -> Point<f64, Logical> {
        Point::from((x, y))
    }

    fn assert_pt_eq(a: Point<f64, Logical>, b: Point<f64, Logical>) {
        assert_abs_diff_eq!(a.x, b.x, epsilon = 1e-9);
        assert_abs_diff_eq!(a.y, b.y, epsilon = 1e-9);
    }

    #[test]
    fn transform_round_trip() {
        let transforms = [
            CanvasTransform::IDENTITY,
            CanvasTransform::new(pt(-300., 1250.5), 0.62),
            CanvasTransform::new(pt(1e5, -1e5), 0.4),
            CanvasTransform::new(pt(7., 3.), 1.),
        ];
        let points = [
            pt(0., 0.),
            pt(1920., 1080.),
            pt(-55.5, 12.25),
            pt(1e4, -3e3),
        ];

        for t in transforms {
            for p in points {
                assert_pt_eq(t.plane_to_view(t.view_to_plane(p)), p);
                assert_pt_eq(t.view_to_plane(t.plane_to_view(p)), p);
                assert_pt_eq(t.unscaled_to_plane(t.plane_to_unscaled(p)), p);
                // view = unscaled * zoom.
                assert_pt_eq(t.plane_to_view(p), t.plane_to_unscaled(p).upscale(t.zoom));
            }

            let rect = Rectangle::new(pt(10., 20.), Size::from((300., 200.)));
            let back = t.view_rect_to_plane(t.plane_rect_to_view(rect));
            assert_pt_eq(back.loc, rect.loc);
            assert_abs_diff_eq!(back.size.w, rect.size.w, epsilon = 1e-9);
            assert_abs_diff_eq!(back.size.h, rect.size.h, epsilon = 1e-9);
        }
    }

    #[test]
    fn view_definition() {
        let t = CanvasTransform::new(pt(100., 50.), 0.5);
        // view = (plane - offset) * zoom
        assert_pt_eq(t.plane_to_view(pt(300., 250.)), pt(100., 100.));
        assert_pt_eq(t.view_to_plane(pt(0., 0.)), pt(100., 50.));
        let visible = t.visible_plane_rect(Size::from((1920., 1080.)));
        assert_pt_eq(visible.loc, pt(100., 50.));
        assert_abs_diff_eq!(visible.size.w, 3840., epsilon = 1e-9);
        assert_abs_diff_eq!(visible.size.h, 2160., epsilon = 1e-9);
    }

    #[test]
    fn zoom_anchor_invariance() {
        let t = CanvasTransform::new(pt(-420., 77.), 0.62);
        for anchor in [pt(0., 0.), pt(960., 540.), pt(1919., 3.), pt(-10., 2000.)] {
            for zoom in [0.4, 0.5, 0.75, 0.9, 1.0] {
                let before = t.view_to_plane(anchor);
                let z = t.zoomed_around(anchor, zoom);
                assert_abs_diff_eq!(z.zoom, zoom);
                assert_pt_eq(z.view_to_plane(anchor), before);
            }
        }
    }

    #[test]
    fn animated_zoom_keeps_anchor_fixed() {
        let clock = Clock::with_time(std::time::Duration::ZERO);
        let mut clock = clock;
        let start = CanvasTransform::new(pt(10., 20.), 1.);
        let mut view = CanvasView::new(start);
        let anchor = pt(300., 200.);
        let plane_under_anchor = start.view_to_plane(anchor);

        let target = start.zoomed_around(anchor, 0.5);
        let config = niri_config::Animation {
            off: false,
            kind: niri_config::animations::Kind::Spring(niri_config::animations::SpringParams {
                damping_ratio: 1.,
                stiffness: 600,
                epsilon: 0.0001,
            }),
        };
        view.animate_to(&clock, target, anchor, config, config);
        assert!(view.is_animating());

        for ms in [5, 16, 33, 50, 100, 200, 400, 1000, 5000] {
            clock.set_unadjusted(std::time::Duration::from_millis(ms));
            view.advance();
            let current = view.current();
            assert!(current.zoom <= 1. + 1e-9 && current.zoom >= 0.5 - 1e-9);
            assert_pt_eq(current.view_to_plane(anchor), plane_under_anchor);
        }
        assert!(!view.is_animating());
        assert_eq!(view.current(), target);
    }

    #[test]
    fn animation_off_jumps() {
        let clock = Clock::with_time(std::time::Duration::ZERO);
        let mut view = CanvasView::new(CanvasTransform::IDENTITY);
        let target = CanvasTransform::new(pt(500., 0.), 0.5);
        let off = niri_config::Animation::new_off();
        view.animate_to(&clock, target, pt(0., 0.), off, off);
        assert!(!view.is_animating());
        assert_eq!(view.current(), target);
    }

    #[test]
    fn zoom_steps() {
        let steps = [0.4, 0.5, 0.62, 0.75, 0.9, 1.0];
        assert_eq!(next_zoom_step(&steps, 0.62, true), 0.75);
        assert_eq!(next_zoom_step(&steps, 0.62, false), 0.5);
        assert_eq!(next_zoom_step(&steps, 1.0, true), 1.0);
        assert_eq!(next_zoom_step(&steps, 0.4, false), 0.4);
        // Off-step values go to the neighbouring step.
        assert_eq!(next_zoom_step(&steps, 0.55, true), 0.62);
        assert_eq!(next_zoom_step(&steps, 0.55, false), 0.5);
        assert_eq!(next_zoom_step(&steps, 0.1, true), 0.4);
        assert_eq!(next_zoom_step(&steps, 3.0, false), 1.0);
    }

    #[test]
    fn fit_contains_bbox_and_clamps() {
        let area = Rectangle::new(pt(0., 30.), Size::from((1920., 1050.)));
        let bbox = Rectangle::new(pt(-1000., -500.), Size::from((4000., 1500.)));
        let t = fit_transform(bbox, area, CANVAS_FIT_PADDING, 0.4, 1.0);
        assert!(t.zoom >= 0.4 && t.zoom <= 1.0);
        let view = t.plane_rect_to_view(bbox);
        assert!(view.loc.x >= area.loc.x - 1e-6);
        assert!(view.loc.x + view.size.w <= area.loc.x + area.size.w + 1e-6);
        assert_pt_eq(rect_center(view), rect_center(area));

        // A tiny bbox never zooms above 100%.
        let small = Rectangle::new(pt(5., 5.), Size::from((100., 100.)));
        assert_eq!(fit_transform(small, area, 48., 0.4, 2.0).zoom, 1.0);
        // A huge bbox stops at the minimum step.
        let huge = Rectangle::new(pt(0., 0.), Size::from((1e6, 1e6)));
        assert_eq!(fit_transform(huge, area, 48., 0.4, 1.0).zoom, 0.4);
    }

    #[test]
    fn edge_push_is_minimal() {
        let area = Rectangle::from_size(Size::from((1000., 800.)));
        let t = CanvasTransform::new(pt(0., 0.), 0.5);
        // Visible plane: 0..2000 x 0..1600 (minus margins).
        let inside = Rectangle::new(pt(500., 500.), Size::from((200., 200.)));
        assert_eq!(push_to_contain(t, inside, area, 0.), t);

        let past_right = Rectangle::new(pt(1900., 500.), Size::from((200., 200.)));
        let pushed = push_to_contain(t, past_right, area, 0.);
        assert_abs_diff_eq!(pushed.offset.x, 100.);
        assert_abs_diff_eq!(pushed.offset.y, 0.);
        let visible = pushed.view_rect_to_plane(area);
        assert!(visible.contains_rect(past_right));

        let past_top = Rectangle::new(pt(500., -300.), Size::from((200., 200.)));
        let pushed = push_to_contain(t, past_top, area, 0.);
        assert_abs_diff_eq!(pushed.offset.y, -300.);
    }

    #[test]
    fn frame_width_compensation() {
        assert_eq!(compensated_frame_width(1., 1., 1.), 1.);
        assert_eq!(compensated_frame_width(1., 0.62, 1.), 2.);
        assert_eq!(compensated_frame_width(1., 0.4, 1.), 3.);
        assert_eq!(compensated_frame_width(4., 0.62, 1.), 4.);
        assert_eq!(compensated_frame_width(1., 0.62, 2.), 1.);
        // Physical width stays >= 1 px.
        for zoom in [0.4, 0.5, 0.62, 0.75, 0.9] {
            assert!(compensated_frame_width(1., zoom, 1.) * zoom >= 1.);
        }
    }
}
