//! ashd: canvas mode of a workspace.
//!
//! See `crate::layout::canvas` for the coordinate model. This child module of `workspace` holds
//! all new `impl Workspace` methods so that `workspace.rs` itself only carries short, marked
//! hooks.
//!
//! In canvas mode:
//! - all normal windows live in the floating space, switched to unbounded (plane) mode;
//! - the scrolling space only holds fullscreen/maximized windows (they cover the output at scale 1
//!   regardless of zoom and return to their plane position afterwards);
//! - `self.canvas.strip` holds the strip arrangement to restore when leaving canvas mode.

use std::rc::Rc;
use std::time::Duration;

use niri_config::WorkspaceMode;
use niri_ipc::WindowLayout;
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::{FloatingActive, Workspace, WorkspaceAddWindowTarget, WorkspaceRenderElement};
use crate::animation::Animation;
use crate::input::swipe_tracker::SwipeTracker;
use crate::layout::canvas::{
    bounding_box, compensated_frame_width, fit_transform, next_zoom_step, push_to_contain,
    rect_center, zoom_by_target, CanvasMemory, CanvasPanDirection, CanvasPanGesture,
    CanvasTransform, CanvasView, ModeSwitch, CANVAS_CASCADE_STEP, CANVAS_EDGE_PUSH_MARGIN,
    CANVAS_FIT_PADDING, CANVAS_TOUCHPAD_GESTURE_MOVEMENT,
};
use crate::layout::canvas_grid::CanvasDecorRenderElement;
use crate::layout::scrolling::{Column, ColumnWidth, StripArrangement};
use crate::layout::tile::Tile;
use crate::layout::title_bar::{LabelStyle, LABEL_GAP};
use crate::layout::{HitType, LayoutElement, Options, RemovedTile, RenderLayer};
use crate::render_helpers::minify::MinifyRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::{RenderCtx, RenderTarget};
use crate::utils::transaction::Transaction;
use crate::utils::ResizeEdge;

impl<W: LayoutElement> Workspace<W> {
    // ---------------------------------------------------------------------------------------
    // Queries.
    // ---------------------------------------------------------------------------------------

    /// Window arrangement mode of this workspace.
    pub fn mode(&self) -> WorkspaceMode {
        self.canvas.mode()
    }

    /// Whether this workspace is in canvas mode.
    pub fn is_canvas(&self) -> bool {
        self.canvas.is_canvas()
    }

    /// Canvas view (current/target transform, animation), in canvas mode.
    pub fn canvas_view(&self) -> Option<&CanvasView> {
        self.is_canvas().then_some(&self.canvas.view)
    }

    /// Canvas transform as currently displayed, in canvas mode.
    pub fn canvas_transform(&self) -> Option<CanvasTransform> {
        self.canvas_view().map(CanvasView::current)
    }

    /// Canvas transform the view is animating towards, in canvas mode.
    pub fn canvas_target_transform(&self) -> Option<CanvasTransform> {
        self.canvas_view().map(CanvasView::target)
    }

    /// Current canvas zoom; 1.0 in strip mode.
    pub fn canvas_zoom(&self) -> f64 {
        self.canvas_transform().map_or(1., |t| t.zoom)
    }

    /// Whether canvas windows are currently drawn scaled (zoom ≠ 1), so that pointer input
    /// cannot be delivered to them as is.
    pub fn canvas_is_zoomed(&self) -> bool {
        self.canvas_transform().is_some_and(|t| t.zoom != 1.)
    }

    /// Converts a workspace view position to a plane position (canvas mode).
    pub fn canvas_view_to_plane(&self, view: Point<f64, Logical>) -> Option<Point<f64, Logical>> {
        self.canvas_transform().map(|t| t.view_to_plane(view))
    }

    /// Converts a plane position to a workspace view position (canvas mode).
    pub fn canvas_plane_to_view(&self, plane: Point<f64, Logical>) -> Option<Point<f64, Logical>> {
        self.canvas_transform().map(|t| t.plane_to_view(plane))
    }

    /// Plane rectangle (tile geometry) of a canvas window.
    pub fn canvas_window_plane_rect(&self, id: &W::Id) -> Option<Rectangle<f64, Logical>> {
        if !self.is_canvas() {
            return None;
        }
        self.floating.canvas_plane_rect(id)
    }

    /// Whether a window on this canvas workspace is pinned.
    pub fn is_window_pinned(&self, id: &W::Id) -> bool {
        self.is_canvas() && self.floating.is_pinned(id)
    }

    /// IPC layouts of the floating tiles. On a canvas: `pos_in_canvas`, `is_pinned`, and
    /// `tile_pos_in_workspace_view` in real view coordinates at the *settled* (target) view,
    /// unset for tiles outside it, so that view animations don't spam layout changes (like the
    /// stationary view offset of the scrolling layout).
    pub(super) fn canvas_floating_ipc_layouts(
        &self,
    ) -> impl Iterator<Item = (&Tile<W>, WindowLayout)> + '_ {
        let target = self.canvas_target_transform();
        let scale = self.scale.fractional_scale();
        let view = Rectangle::from_size(self.view_size);
        let positions = self.floating.canvas_stored_positions();
        self.floating.tiles_with_ipc_layouts().zip(positions).map(
            move |((tile, mut layout), (pinned, stored))| {
                let Some(target) = target else {
                    return (tile, layout);
                };
                // Pinned tiles store unscaled view positions, the others plane positions.
                let (plane, view_pos) = if pinned {
                    (
                        target.unscaled_to_plane(stored),
                        stored.upscale(target.zoom),
                    )
                } else {
                    (stored, target.plane_to_view(stored))
                };
                let view_pos = view_pos.to_physical_precise_round(scale).to_logical(scale);
                let size = Size::from(layout.tile_size).upscale(target.zoom);
                let on_screen = Rectangle::new(view_pos, size).overlaps(view);
                layout.tile_pos_in_workspace_view = on_screen.then_some(view_pos.into());
                layout.pos_in_canvas = Some(plane.into());
                layout.is_pinned = pinned;
                (tile, layout)
            },
        )
    }

    // ---------------------------------------------------------------------------------------
    // Mode switching.
    // ---------------------------------------------------------------------------------------

    /// Switches between strip and canvas mode.
    pub fn toggle_mode(&mut self) {
        match self.mode() {
            WorkspaceMode::Strip => self.set_mode(WorkspaceMode::Canvas),
            WorkspaceMode::Canvas => self.set_mode(WorkspaceMode::Strip),
        }
    }

    /// Sets the window arrangement mode.
    ///
    /// Strip → canvas saves the strip arrangement, puts every tile on the plane at its current
    /// on-screen position (or its remembered plane position from the previous canvas session)
    /// and animates the view to the canvas zoom centred on the focused window.
    ///
    /// Canvas → strip rebuilds the saved columns (unknown windows are appended ordered by plane
    /// x), keeps windows that were floating in strip mode floating, and remembers plane positions
    /// and zoom for the next canvas session.
    pub fn set_mode(&mut self, mode: WorkspaceMode) {
        if self.mode() == mode {
            return;
        }
        match mode {
            WorkspaceMode::Canvas => self.enter_canvas(),
            WorkspaceMode::Strip => self.exit_canvas(),
        }
    }

    /// Applies `default-workspace-mode` to a freshly created (empty) workspace.
    pub(super) fn apply_default_mode(&mut self) {
        if self.options.layout.default_workspace_mode != WorkspaceMode::Canvas {
            return;
        }

        let zoom = self.options.layout.canvas.default_zoom;
        self.canvas.mode = WorkspaceMode::Canvas;
        self.canvas.strip = Some(StripArrangement {
            columns: Vec::new(),
            active_column_idx: 0,
            floating: Vec::new(),
            floating_was_active: false,
        });
        self.floating
            .set_canvas_mode(Some(CanvasTransform::IDENTITY));
        self.canvas
            .view
            .jump_to(CanvasTransform::new(Point::from((0., 0.)), zoom));
        self.sync_canvas();
    }

    fn enter_canvas(&mut self) {
        let active_id = self.active_window().map(|win| win.id().clone());

        // Save the strip arrangement.
        let (columns, active_column_idx) = self.scrolling.snapshot_columns();
        let strip = StripArrangement {
            columns,
            active_column_idx,
            floating: self
                .floating
                .tiles()
                .map(|tile| tile.window().id().clone())
                .collect(),
            floating_was_active: self.floating_is_active.get(),
        };

        // Current on-screen positions of the tiled windows.
        let tiled: Vec<(W::Id, Point<f64, Logical>, bool)> = self
            .scrolling
            .tiles_with_render_positions()
            .map(|(tile, pos, _)| {
                let id = tile.window().id().clone();
                (id, pos, tile.window().pending_sizing_mode().is_normal())
            })
            .collect();

        let memory = self.canvas.memory.take();
        let remembered = |id: &W::Id| {
            memory.as_ref().and_then(|memory| {
                memory
                    .plane_positions
                    .iter()
                    .find(|(mem_id, _)| mem_id == id)
                    .map(|(_, pos)| *pos)
            })
        };

        // The view starts out showing exactly what is on screen: plane = view (or, in the middle
        // of a canvas → strip transition, the zoomed strip as it is drawn).
        let start = match self.strip_transition_zoom() {
            Some((zoom, anchor)) => CanvasTransform::new(anchor - anchor.downscale(zoom), zoom),
            None => CanvasTransform::IDENTITY,
        };
        self.canvas.mode = WorkspaceMode::Canvas;
        self.canvas.strip = Some(strip);
        self.canvas.view.jump_to(start);
        self.canvas.cascade = None;
        self.floating.set_canvas_mode(Some(start));

        // Floating windows go back to where they were on the canvas last time.
        let floating_ids: Vec<W::Id> = self
            .floating
            .tiles()
            .map(|tile| tile.window().id().clone())
            .collect();
        for id in &floating_ids {
            if let Some(plane) = remembered(id) {
                self.floating.set_canvas_plane_pos(id, plane, true);
            }
        }

        // Tiled windows go onto the plane.
        for (id, pos, is_normal) in tiled {
            if !is_normal {
                // Fullscreen and maximized windows stay where they are, but return to the canvas
                // when they are restored.
                let tile = self
                    .scrolling
                    .tiles_mut()
                    .find(|tile| tile.window().id() == &id)
                    .unwrap();
                tile.restore_to_floating = true;
                tile.floating_pos = None;
                tile.canvas_plane_pos = remembered(&id);
                continue;
            }

            let mut removed = self.scrolling.remove_tile(&id, Transaction::new());
            removed.tile.stop_move_animations();
            let target = remembered(&id);
            removed.tile.canvas_plane_pos = Some(target.unwrap_or(pos));
            removed.tile.restore_to_floating = true;
            if target.is_none() {
                // Keep the current (strip) size so nothing jumps.
                removed.tile.floating_window_size = None;
            }
            self.floating.add_tile(removed.tile, false);

            if target.is_some() {
                let config = self.options.animations.canvas_mode_switch.0;
                let (tile, new_pos) = self
                    .floating
                    .tiles_with_render_positions_mut(false)
                    .find(|(tile, _)| tile.window().id() == &id)
                    .unwrap();
                // Same curve as the view: one coherent transition. `pos - offset` is where the
                // window is drawn now in unscaled view coordinates.
                tile.animate_move_from_with_config(pos - start.offset - new_pos, config);
            }
        }

        // Keep the focus where it was.
        if let Some(id) = &active_id {
            if self.floating.has_window(id) {
                self.floating.activate_window(id);
                self.floating_is_active = FloatingActive::Yes;
            }
        }
        self.canvas_fix_floating_active();

        // Animate into the canvas zoom, centred on the focused window.
        let zoom = memory
            .as_ref()
            .map_or(self.options.layout.canvas.default_zoom, |m| {
                m.transform.zoom
            });
        let zoom = self.options.layout.canvas.clamp_zoom(zoom);
        let center = rect_center(self.working_area);
        let focused_rect = active_id
            .as_ref()
            .and_then(|id| self.floating.canvas_plane_rect(id));
        let target = match (focused_rect, &memory) {
            (Some(rect), _) => CanvasTransform::centered_on(rect_center(rect), center, zoom),
            (None, Some(memory)) => CanvasTransform::new(memory.transform.offset, zoom),
            (None, None) => CanvasTransform::IDENTITY.zoomed_around(center, zoom),
        };
        self.canvas_animate_mode_switch(target, center, true, 1., CanvasTransform::IDENTITY);
    }

    /// Starts the strip ↔ canvas transition (see [`ModeSwitch`]); on a canvas the view goes to
    /// `target` along the same curve.
    fn canvas_animate_mode_switch(
        &mut self,
        target: CanvasTransform,
        focus: Point<f64, Logical>,
        to_canvas: bool,
        from_zoom: f64,
        decor_transform: CanvasTransform,
    ) {
        let config = self.options.animations.canvas_mode_switch.0;
        if to_canvas && target.is_finite() {
            let zoom = self.options.layout.canvas.clamp_zoom(target.zoom);
            let target = CanvasTransform::new(target.offset, zoom);
            self.canvas
                .view
                .animate_to(&self.clock, target, focus, config, config);
            self.sync_canvas();
        }
        self.canvas.mode_switch = Some(ModeSwitch {
            anim: Animation::new(self.clock.clone(), 0., 1., 0., config),
            to_canvas,
            from_zoom,
            anchor: focus,
            decor_transform,
        });
    }

    /// Canvas → strip transition: the zoom the strip is drawn at and its anchor, while it differs
    /// from 1.
    pub fn strip_transition_zoom(&self) -> Option<(f64, Point<f64, Logical>)> {
        let (zoom, anchor) = self.canvas.mode_switch.as_ref()?.strip_zoom()?;
        ((zoom - 1.).abs() > 1e-6).then_some((zoom, anchor))
    }

    /// Opacity of the grid while the mode switch fades it in or out.
    pub fn canvas_decor_alpha(&self) -> f32 {
        self.canvas
            .mode_switch
            .as_ref()
            .map_or(1., |m| m.decor_alpha() as f32)
    }

    /// Zoom a tile of this workspace is drawn at: the canvas zoom on the plane, 1 elsewhere.
    pub(super) fn canvas_tile_zoom(&self, id: &W::Id) -> f64 {
        if self.is_canvas() && self.floating.has_window(id) {
            self.canvas_zoom()
        } else {
            1.
        }
    }

    /// Whether a mode switch animation is running.
    pub fn is_mode_switch_ongoing(&self) -> bool {
        self.canvas.mode_switch.is_some()
    }

    fn exit_canvas(&mut self) {
        let Some(strip) = self.canvas.strip.take() else {
            self.canvas.mode = WorkspaceMode::Strip;
            return;
        };

        let active_id = self.active_window().map(|win| win.id().clone());
        let current = self.canvas.view.current();
        let target = self.canvas.view.target();
        // Fullscreen / maximized windows were not drawn zoomed; don't zoom the strip then.
        let from_zoom = if self.scrolling.is_empty() {
            current.zoom
        } else {
            1.
        };
        let anchor = rect_center(self.working_area);

        // Remember the canvas layout for the next canvas session.
        let mut plane_positions: Vec<(W::Id, Point<f64, Logical>)> = self
            .floating
            .canvas_plane_rects()
            .map(|(tile, rect)| (tile.window().id().clone(), rect.loc))
            .collect();
        for tile in self.scrolling.tiles() {
            if let Some(pos) = tile.canvas_plane_pos {
                plane_positions.push((tile.window().id().clone(), pos));
            }
        }

        // Where the windows are on screen right now, for the transition animation.
        let view_positions: Vec<(W::Id, Point<f64, Logical>)> = self
            .floating
            .tiles_with_render_positions()
            .map(|(tile, pos)| (tile.window().id().clone(), pos.upscale(current.zoom)))
            .collect();

        let stays_floating = |strip: &StripArrangement<W::Id>, tile: &Tile<W>| {
            strip.is_floating(tile.window().id())
                || tile.window().rules().open_floating == Some(true)
        };

        // Take the windows that go back into columns out of the canvas.
        let to_tile: Vec<W::Id> = self
            .floating
            .tiles()
            .filter(|tile| !stays_floating(&strip, tile))
            .map(|tile| tile.window().id().clone())
            .collect();
        let mut removed: Vec<RemovedTile<W>> = to_tile
            .iter()
            .map(|id| {
                let mut removed = self.floating.remove_tile(id);
                removed.tile.canvas_plane_pos = None;
                removed.tile.restore_to_floating = false;
                removed.tile.stop_move_animations();
                removed
            })
            .collect();

        // Back to a bounded floating space for the strip-floating windows.
        self.floating.set_canvas_mode(None);
        self.canvas.mode = WorkspaceMode::Strip;
        self.canvas.memory = Some(CanvasMemory {
            transform: target,
            plane_positions,
        });
        self.canvas.view.jump_to(CanvasTransform::IDENTITY);
        self.canvas.cascade = None;
        self.canvas.pan_gesture = None;
        self.canvas.zoom_gesture = None;
        self.sync_canvas();

        // Windows that stayed in the scrolling space (fullscreen) restore per strip rules.
        for tile in self.scrolling.tiles_mut() {
            tile.restore_to_floating = strip.is_floating(tile.window().id());
            tile.canvas_plane_pos = None;
        }

        // Rebuild the saved columns, in order, in front of any remaining columns.
        let mut col_idx = 0;
        for saved in &strip.columns {
            let mut tiles = Vec::new();
            for (id, _) in &saved.tiles {
                if let Some(idx) = removed.iter().position(|r| r.tile.window().id() == id) {
                    tiles.push(removed.remove(idx).tile);
                }
            }
            if tiles.is_empty() {
                continue;
            }
            self.scrolling.insert_saved_column(col_idx, tiles, saved);
            col_idx += 1;
        }

        // Unknown windows are appended, ordered by plane x.
        let plane_x = |id: &W::Id| {
            self.canvas
                .memory
                .as_ref()
                .and_then(|m| m.plane_positions.iter().find(|(i, _)| i == id))
                .map_or(0., |(_, pos)| pos.x)
        };
        let mut rest: Vec<(f64, RemovedTile<W>)> = removed
            .into_iter()
            .map(|r| (plane_x(r.tile.window().id()), r))
            .collect();
        rest.sort_by(|(a, _), (b, _)| f64::total_cmp(a, b));
        for (_, r) in rest {
            let idx = self.scrolling.column_count();
            self.scrolling
                .add_tile(Some(idx), r.tile, false, r.width, r.is_full_width, None);
        }

        // Keep the focus where it was.
        self.floating_is_active = FloatingActive::No;
        if let Some(id) = &active_id {
            self.activate_window(id);
        } else if strip.floating_was_active && !self.floating.is_empty() {
            self.floating_is_active = FloatingActive::Yes;
        }
        self.canvas_fix_floating_active();

        // One transition: the strip is drawn zoomed from the canvas zoom to 1 around `anchor`
        // while the windows move from where the canvas drew them along the same curve, so the
        // first frame matches the canvas and the last one the strip.
        self.scrolling.settle_view_offset();
        self.canvas_animate_mode_switch(target, anchor, false, from_zoom, current);
        let config = self.options.animations.canvas_mode_switch.0;
        for (tile, new_pos) in self.tiles_with_render_positions_mut(false) {
            if let Some((_, old)) = view_positions
                .iter()
                .find(|(id, _)| id == tile.window().id())
            {
                let from = anchor + (*old - anchor).downscale(from_zoom);
                tile.animate_move_from_with_config(from - new_pos, config);
            }
        }
    }

    /// Keeps `floating_is_active` consistent with which spaces have windows.
    fn canvas_fix_floating_active(&mut self) {
        if self.floating.is_empty() {
            self.floating_is_active = FloatingActive::No;
        } else if self.scrolling.is_empty() {
            self.floating_is_active = FloatingActive::Yes;
        }
    }

    // ---------------------------------------------------------------------------------------
    // View operations. All of these are no-ops in strip mode.
    // ---------------------------------------------------------------------------------------

    /// Pans by one `pan-step` (view pixels) in a direction.
    pub fn canvas_pan(&mut self, direction: CanvasPanDirection) {
        let Some(target) = self.canvas_target_transform() else {
            return;
        };
        let step = self.options.layout.canvas.pan_step;
        let target = target.panned_by_view(direction.unit().upscale(step));
        self.canvas_animate_to(target, rect_center(self.working_area));
    }

    /// Pans so that the view offset (plane point at the view's top-left) becomes `plane`.
    pub fn canvas_pan_to(&mut self, plane: Point<f64, Logical>) {
        let Some(target) = self.canvas_target_transform() else {
            return;
        };
        self.canvas_animate_to(
            CanvasTransform::new(plane, target.zoom),
            rect_center(self.working_area),
        );
    }

    /// Steps the zoom in or out through `zoom-steps`, keeping `anchor` (view coordinates;
    /// default: working area centre) fixed.
    pub fn canvas_zoom_step(&mut self, zoom_in: bool, anchor: Option<Point<f64, Logical>>) {
        let Some(target) = self.canvas_target_transform() else {
            return;
        };
        let zoom = next_zoom_step(&self.options.layout.canvas.zoom_steps, target.zoom, zoom_in);
        self.canvas_set_zoom(zoom, anchor);
    }

    /// Sets the zoom (clamped to the `zoom-steps` range), keeping `anchor` fixed.
    pub fn canvas_set_zoom(&mut self, zoom: f64, anchor: Option<Point<f64, Logical>>) {
        let Some(target) = self.canvas_target_transform() else {
            return;
        };
        if !zoom.is_finite() {
            return;
        }
        let zoom = self.options.layout.canvas.clamp_zoom(zoom);
        let anchor = anchor.unwrap_or_else(|| rect_center(self.working_area));
        self.canvas_animate_to(target.zoomed_around(anchor, zoom), anchor);
    }

    /// Zooms by `factor` relative to the target zoom (clamped to the `zoom-steps` range), with
    /// the plane point currently under `anchor` (view coordinates; default: working area centre)
    /// staying under it. Uses the `canvas-wheel-zoom` animation; consecutive calls accumulate.
    pub fn canvas_zoom_by(&mut self, factor: f64, anchor: Option<Point<f64, Logical>>) {
        let (Some(current), Some(target)) =
            (self.canvas_transform(), self.canvas_target_transform())
        else {
            return;
        };
        if self.canvas.zoom_gesture.is_some() {
            return;
        }
        let anchor = anchor.unwrap_or_else(|| rect_center(self.working_area));
        if !anchor.x.is_finite() || !anchor.y.is_finite() {
            return;
        }
        let canvas = &self.options.layout.canvas;
        let new = zoom_by_target(
            current,
            target,
            anchor,
            factor,
            canvas.min_zoom(),
            canvas.max_zoom(),
        );
        if new.approx_eq(&target) {
            return;
        }
        let zoom_config = self.options.animations.canvas_wheel_zoom.0;
        self.canvas_animate_to_with(new, anchor, zoom_config);
    }

    /// Picks offset and zoom so that all (non-pinned) windows are visible.
    pub fn canvas_fit(&mut self) {
        if !self.is_canvas() {
            return;
        }
        let bbox = bounding_box(
            self.floating
                .canvas_plane_rects()
                .filter(|(tile, _)| !self.floating.is_pinned(tile.window().id()))
                .map(|(_, rect)| rect),
        );
        let Some(bbox) = bbox else {
            return;
        };
        let canvas = &self.options.layout.canvas;
        let target = fit_transform(
            bbox,
            self.working_area,
            CANVAS_FIT_PADDING,
            canvas.min_zoom(),
            canvas.max_zoom(),
        );
        self.canvas_animate_to(target, rect_center(self.working_area));
    }

    /// Pans back to offset (0, 0), keeping the zoom.
    pub fn canvas_home(&mut self) {
        self.canvas_pan_to(Point::from((0., 0.)));
    }

    /// Centres the view on a window, keeping the zoom.
    pub fn canvas_fly_to_window(&mut self, id: &W::Id) {
        let Some(target) = self.canvas_target_transform() else {
            return;
        };
        if self.floating.is_pinned(id) {
            // Pinned windows are always on screen.
            return;
        }
        let Some(rect) = self.floating.canvas_plane_rect(id) else {
            return;
        };
        let center = rect_center(self.working_area);
        let target = CanvasTransform::centered_on(rect_center(rect), center, target.zoom);
        self.canvas_animate_to(target, center);
    }

    /// Recentres the view on a window that received keyboard focus.
    pub fn canvas_reveal_window(&mut self, id: &W::Id) {
        self.canvas_fly_to_window(id);
    }

    /// Pans the view minimally so that a (keyboard-moved) window is inside the viewport.
    pub fn canvas_edge_push(&mut self, id: &W::Id) {
        let Some(target) = self.canvas_target_transform() else {
            return;
        };
        if self.floating.is_pinned(id) {
            return;
        }
        let Some(rect) = self.floating.canvas_plane_rect(id) else {
            return;
        };
        let pushed = push_to_contain(target, rect, self.working_area, CANVAS_EDGE_PUSH_MARGIN);
        if pushed != target {
            self.canvas_animate_to(pushed, rect_center(self.working_area));
        }
    }

    /// Toggles whether a canvas window stays fixed on screen while the view pans.
    pub fn toggle_window_pinned(&mut self, id: &W::Id) {
        if !self.is_canvas() || !self.floating.has_window(id) {
            return;
        }
        let pinned = self.floating.is_pinned(id);
        self.floating.set_pinned(id, !pinned);
    }

    fn canvas_animate_to(&mut self, target: CanvasTransform, focus: Point<f64, Logical>) {
        let zoom_config = self.options.animations.canvas_zoom.0;
        self.canvas_animate_to_with(target, focus, zoom_config);
    }

    fn canvas_animate_to_with(
        &mut self,
        target: CanvasTransform,
        focus: Point<f64, Logical>,
        zoom_config: niri_config::Animation,
    ) {
        if !self.is_canvas() || !target.is_finite() {
            return;
        }
        let zoom = self.options.layout.canvas.clamp_zoom(target.zoom);
        let target = CanvasTransform::new(target.offset, zoom);
        let pan = self.options.animations.canvas_pan.0;
        self.canvas
            .view
            .animate_to(&self.clock, target, focus, pan, zoom_config);
        self.sync_canvas();
    }

    // ---------------------------------------------------------------------------------------
    // Hooks called from workspace.rs.
    // ---------------------------------------------------------------------------------------

    /// Advances the canvas view animation and pushes the transform into the floating space.
    pub(super) fn canvas_advance_animations(&mut self) {
        if self
            .canvas
            .mode_switch
            .as_ref()
            .is_some_and(|m| m.anim.is_done())
        {
            self.canvas.mode_switch = None;
        }
        if !self.is_canvas() {
            return;
        }
        self.canvas.view.advance();
        self.sync_canvas();
    }

    /// Re-applies config-dependent canvas state after a config change.
    pub(super) fn canvas_update_config(&mut self) {
        let Some(target) = self.canvas_target_transform() else {
            return;
        };
        let zoom = self.options.layout.canvas.clamp_zoom(target.zoom);
        if zoom != target.zoom {
            self.canvas_set_zoom(zoom, None);
        }
    }

    /// Pushes the current transform into the floating space, and re-applies frame width
    /// compensation once the zoom has settled.
    fn sync_canvas(&mut self) {
        let transform = self.canvas.view.current();
        self.floating.set_canvas_transform(transform);

        let frame_zoom = if self.is_canvas() { transform.zoom } else { 1. };
        let zoom_settled =
            !self.canvas.view.is_zoom_animating() && self.canvas.zoom_gesture.is_none();
        // ashd: tiles get their canvas options (decorations) as soon as the mode changes.
        let mode_changed = self.floating.options().canvas_tile_zoom.is_some() != self.is_canvas();
        if (zoom_settled && frame_zoom != self.canvas.frame_zoom) || mode_changed {
            if zoom_settled {
                self.canvas.frame_zoom = frame_zoom;
            }
            let options = self.floating_options(&self.options);
            self.floating.update_config(
                self.view_size,
                self.working_area,
                self.scale.fractional_scale(),
                options,
            );
        }
    }

    /// Options for the floating space: in canvas mode, border and focus ring widths are widened
    /// so that they stay at least one physical pixel wide at the settled zoom.
    pub(super) fn floating_options(&self, options: &Rc<Options>) -> Rc<Options> {
        let zoom = self.canvas.frame_zoom;
        if !self.is_canvas() {
            return options.clone();
        }

        let mut adjusted = Options::clone(options);
        // ashd: canvas tiles know the settled zoom (focus style, crisp title text).
        adjusted.canvas_tile_zoom = Some(zoom);
        if zoom < 1. {
            let scale = self.scale.fractional_scale();
            let layout = &options.layout;
            adjusted.layout.border.width =
                compensated_frame_width(layout.border.width, zoom, scale);
            adjusted.layout.focus_ring.width =
                compensated_frame_width(layout.focus_ring.width, zoom, scale);
        }
        Rc::new(adjusted)
    }

    /// Visible part of the floating space, in the coordinates it reports.
    pub(super) fn floating_view_rect(&self) -> Rectangle<f64, Logical> {
        let zoom = self.canvas_zoom();
        Rectangle::from_size(self.view_size.downscale(zoom))
    }

    /// Adjusts where a window added to a canvas workspace goes: normal windows go on the plane
    /// (new ones centred in the viewport with a small cascade), fullscreen/maximized ones to the
    /// scrolling space from where they return to the plane.
    pub(super) fn canvas_adjust_add_target<'a>(
        &mut self,
        tile: &mut Tile<W>,
        target: WorkspaceAddWindowTarget<'a, W>,
        is_floating: bool,
    ) -> (WorkspaceAddWindowTarget<'a, W>, bool) {
        // Plane positions from another workspace are meaningless here.
        tile.canvas_plane_pos = None;

        if !self.is_canvas() {
            return (target, is_floating);
        }

        let target = match target {
            WorkspaceAddWindowTarget::NewColumnAt(_) => WorkspaceAddWindowTarget::Auto,
            other => other,
        };

        // Windows without a position of their own open centred in the viewport.
        if matches!(target, WorkspaceAddWindowTarget::Auto)
            && tile.floating_pos.is_none()
            && tile.window().rules().default_floating_position.is_none()
        {
            tile.canvas_plane_pos = Some(self.canvas_cascade_pos(tile.tile_size()));
        }

        (target, true)
    }

    /// Plane position for a new window: centred in the (target) viewport, shifted diagonally
    /// while it would land on the centre of another window.
    fn canvas_cascade_pos(&mut self, size: Size<f64, Logical>) -> Point<f64, Logical> {
        let target = self.canvas.view.target();
        let center = target.view_to_plane(rect_center(self.working_area));
        let mut candidate = center;
        for _ in 0..16 {
            let taken = self.floating.canvas_plane_rects().any(|(_, rect)| {
                let other = rect_center(rect);
                (other.x - candidate.x).abs() < 8. && (other.y - candidate.y).abs() < 8.
            });
            if !taken {
                break;
            }
            candidate += Point::from((CANVAS_CASCADE_STEP, CANVAS_CASCADE_STEP));
        }
        self.canvas.cascade = Some(candidate);
        candidate - size.downscale(2.).to_point()
    }

    /// Adds tiles (e.g. from a column moved here) to the canvas.
    pub(super) fn canvas_add_tiles(
        &mut self,
        tiles: Vec<Tile<W>>,
        activate: bool,
        anim: Option<niri_config::Animation>,
    ) {
        for (idx, mut tile) in tiles.into_iter().enumerate() {
            tile.floating_pos = None;
            let activate = if activate && idx == 0 {
                crate::layout::ActivateWindow::Yes
            } else {
                crate::layout::ActivateWindow::No
            };
            let width = ColumnWidth::Fixed(tile.tile_size().w);
            self.add_tile(
                tile,
                WorkspaceAddWindowTarget::Auto,
                activate,
                width,
                false,
                true,
                anim,
            );
        }
    }

    /// Fixes up a tile leaving this workspace.
    pub(super) fn canvas_on_tile_removed(&mut self, removed: &mut RemovedTile<W>) {
        removed.tile.canvas_plane_pos = None;
        if let Some(strip) = &self.canvas.strip {
            // The position was relative to this canvas view.
            removed.tile.floating_pos = None;
            // Canvas windows are only "floating" for the rest of niri if they were floating in
            // strip mode, so that moving them to a strip workspace tiles them.
            let id = removed.tile.window().id();
            removed.is_floating =
                strip.is_floating(id) || removed.tile.window().rules().open_floating == Some(true);
        }
    }

    /// Moves windows that are neither fullscreen nor maximized any more (e.g. a column action
    /// un-maximized them) from the scrolling space back onto the canvas.
    pub(super) fn canvas_return_restored_windows(&mut self) {
        if !self.is_canvas() {
            return;
        }
        let restored: Vec<W::Id> = self
            .scrolling
            .tiles()
            .filter(|tile| tile.window().pending_sizing_mode().is_normal())
            .map(|tile| tile.window().id().clone())
            .collect();
        for id in restored {
            self.toggle_window_floating_impl(Some(&id));
        }
    }

    /// Fixes up the tiles of a column leaving this workspace.
    pub(super) fn canvas_on_column_removed(&mut self, column: &mut Column<W>) {
        let strip = self.canvas.strip.as_ref();
        for tile in column.tiles_for_canvas_mut() {
            tile.canvas_plane_pos = None;
            if let Some(strip) = strip {
                tile.floating_pos = None;
                tile.restore_to_floating = strip.is_floating(tile.window().id());
            }
        }
    }

    /// Whether `toggle_window_floating` must not move the window out of the canvas.
    pub(super) fn canvas_blocks_floating_toggle(&self, id: Option<&W::Id>) -> bool {
        if !self.is_canvas() {
            return false;
        }
        match id {
            Some(id) => self.floating.has_window(id),
            None => self.floating_is_active.get(),
        }
    }

    /// Active window rectangle in view coordinates clamped to the working area.
    ///
    /// Computed at the target transform, so that warp-mouse-to-focus lands where the window ends
    /// up once a reveal animation finishes.
    pub(super) fn canvas_active_window_visual_rectangle(&self) -> Option<Rectangle<f64, Logical>> {
        let current = self.canvas_transform()?;
        let target = self.canvas_target_transform()?;
        let active = self.floating.active_window()?.id().clone();
        let (tile, pos) = self
            .floating
            .tiles_with_offsets()
            .find(|(tile, _)| tile.window().id() == &active)?;
        // Pinned tiles keep their unscaled view position; others keep their plane position.
        let pos = if self.floating.is_pinned(&active) {
            pos
        } else {
            target.plane_to_unscaled(current.unscaled_to_plane(pos))
        };
        let window_pos = pos + tile.window_loc();
        let zoom = target.zoom;
        let rect = Rectangle::new(window_pos.upscale(zoom), tile.window_size().upscale(zoom));
        self.working_area.intersection(rect)
    }

    /// Renders the canvas (floating space) through the canvas zoom.
    pub(super) fn render_canvas<R: NiriRenderer>(
        &self,
        ctx: RenderCtx<R>,
        xray_pos: XrayPos,
        focus_ring: bool,
        layer: RenderLayer,
        push: &mut dyn FnMut(WorkspaceRenderElement<R>),
    ) {
        let zoom = self.canvas_zoom();
        let view_rect = self.floating_view_rect();
        let focus_ring = focus_ring && self.floating_is_active();

        // Title labels go on top of the windows (elements pushed first are drawn on top).
        if layer.is_normal() {
            self.render_canvas_labels(ctx.renderer, ctx.target, focus_ring, push);
        }

        // Tiles render at unscaled view positions; the rescale maps them to view = u * zoom.
        let xray_pos = XrayPos {
            pos_in_backdrop: xray_pos.pos_in_backdrop.downscale(zoom),
            zoom: xray_pos.zoom * zoom,
        };

        if zoom == 1. {
            self.floating
                .render(ctx, xray_pos, view_rect, focus_ring, layer, &mut |elem| {
                    push(elem.into())
                });
        } else {
            // Zoomed out, window contents are supersampled instead of skipping texels.
            let minify = zoom < 1. && self.options.layout.canvas.smooth_zoom_out;
            self.floating
                .render(ctx, xray_pos, view_rect, focus_ring, layer, &mut |elem| {
                    let elem = RescaleRenderElement::from_element(elem, Point::from((0, 0)), zoom);
                    let elem = MinifyRenderElement::new(elem, minify);
                    push(WorkspaceRenderElement::Canvas(elem))
                });
        }
    }

    /// Constant-size title labels above the canvas windows once the zoom drops below
    /// `title-labels-below` (fading in over the next 0.1). Labels are not hit-tested.
    fn render_canvas_labels<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        target: RenderTarget,
        focus_ring: bool,
        push: &mut dyn FnMut(WorkspaceRenderElement<R>),
    ) {
        let zoom = self.canvas_zoom();
        let alpha = self.options.layout.canvas.title_label_alpha(zoom)
            * f64::from(self.canvas_decor_alpha());
        let active = focus_ring
            .then(|| self.floating.active_window())
            .flatten()
            .map(|win| win.id().clone());
        let tiles = self
            .floating
            .tiles_with_render_positions()
            .map(|(tile, pos)| (tile, pos.upscale(zoom), tile.tile_size().w * zoom));
        self.push_title_labels(renderer, target, active, alpha as f32, tiles, push);
    }

    /// The canvas title labels while a canvas turns back into a strip: they follow the strip
    /// zoom (and fade out with it).
    pub(super) fn render_mode_switch_labels<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        target: RenderTarget,
        zoom: f64,
        anchor: Point<f64, Logical>,
        push: &mut dyn FnMut(WorkspaceRenderElement<R>),
    ) {
        let alpha = self.options.layout.canvas.title_label_alpha(zoom)
            * f64::from(self.canvas_decor_alpha());
        if alpha <= 0. {
            return;
        }
        let tiles = self
            .scrolling
            .tiles_with_render_positions()
            .map(|(tile, pos, _)| {
                let top_left = anchor + (pos - anchor).upscale(zoom);
                (tile, top_left, tile.tile_size().w * zoom)
            });
        self.push_title_labels(renderer, target, None, alpha as f32, tiles, push);
    }

    /// Pushes title labels for `(tile, top-left in view, drawn width)`.
    fn push_title_labels<'a, R: NiriRenderer>(
        &self,
        renderer: &mut R,
        target: RenderTarget,
        active: Option<W::Id>,
        alpha: f32,
        tiles: impl Iterator<Item = (&'a Tile<W>, Point<f64, Logical>, f64)>,
        push: &mut dyn FnMut(WorkspaceRenderElement<R>),
    ) where
        W: 'a,
    {
        if alpha <= 0. {
            return;
        }
        let canvas = &self.options.layout.canvas;
        let title_bar = &self.options.layout.title_bar;
        let style = LabelStyle {
            size: canvas.title_labels_size,
            text_color: title_bar.text_color,
            background_color: title_bar.background_color,
        };
        let accent = LabelStyle {
            text_color: self.options.layout.focus_ring.active_color,
            ..style
        };
        let view = Rectangle::from_size(self.view_size);
        let label_room = canvas.title_labels_size * 2. + LABEL_GAP;

        for (tile, top_left, width) in tiles {
            let room = Rectangle::new(
                top_left - Point::from((0., label_room)),
                Size::from((width, label_room)),
            );
            if !room.overlaps(view) {
                continue;
            }
            let window = tile.window();
            if target.should_block_out(window.rules().block_out_from) {
                continue;
            }
            let style = if active.as_ref() == Some(window.id()) {
                accent
            } else {
                style
            };
            let renderer = renderer.as_gles_renderer();
            if let Some(elem) = tile
                .title_bar()
                .label_element(renderer, style, top_left, width, alpha)
            {
                push(WorkspaceRenderElement::CanvasDecor(
                    CanvasDecorRenderElement::Label(elem),
                ));
            }
        }
    }

    /// Hit-tests a canvas workspace at `pos` (workspace view coordinates).
    ///
    /// Canvas tiles are hit-tested in unscaled view coordinates (`pos / zoom`). At zoom 1 this
    /// returns regular `HitType::Input` hits; at zoom ≠ 1 input hits become
    /// `HitType::ScaledInput { win_pos: win_pos_u * zoom, zoom }`, and the pointer focus code
    /// (`Niri::contents_under()`) computes surface-local coordinates as `(pos - win_pos) / zoom`.
    pub(super) fn canvas_window_under(&self, pos: Point<f64, Logical>) -> Option<(&W, HitType)> {
        if self.is_floating_visible() {
            let zoom = self.canvas_zoom();
            let unscaled = pos.downscale(zoom);
            let hit = self
                .floating
                .tiles_with_render_positions()
                .find_map(|(tile, tile_pos)| HitType::hit_tile(tile, tile_pos, unscaled));
            if let Some((win, hit)) = hit {
                return Some((win, hit.scaled(zoom)));
            }
        }

        self.scrolling.window_under(pos)
    }

    /// Interactive resize edges under `pos` (workspace view coordinates) on a canvas workspace,
    /// consistent with [`Self::canvas_window_under`].
    pub(super) fn canvas_resize_edges_under(&self, pos: Point<f64, Logical>) -> Option<ResizeEdge> {
        let zoom = self.canvas_zoom();
        let floating = self
            .floating
            .tiles_with_render_positions()
            .filter(|_| self.is_floating_visible())
            .map(|(tile, tile_pos)| (tile, tile_pos, pos.downscale(zoom)));
        let scrolling = self
            .scrolling
            .tiles_with_render_positions()
            .filter(|(_, _, visible)| *visible)
            .map(|(tile, tile_pos, _)| (tile, tile_pos, pos));

        floating.chain(scrolling).find_map(|(tile, tile_pos, pos)| {
            let pos_within_tile = pos - tile_pos;
            tile.hit(pos_within_tile)?;

            let size = tile.tile_size();
            let mut edges = ResizeEdge::empty();
            if pos_within_tile.x < size.w / 3. {
                edges |= ResizeEdge::LEFT;
            } else if 2. * size.w / 3. < pos_within_tile.x {
                edges |= ResizeEdge::RIGHT;
            }
            if pos_within_tile.y < size.h / 3. {
                edges |= ResizeEdge::TOP;
            } else if 2. * size.h / 3. < pos_within_tile.y {
                edges |= ResizeEdge::BOTTOM;
            }
            Some(edges)
        })
    }

    /// Scale of pointer deltas for interactive resizes of a window: canvas tiles are drawn at the
    /// canvas zoom, so view deltas are divided by it.
    pub(super) fn canvas_resize_delta(
        &self,
        window: &W::Id,
        delta: Point<f64, Logical>,
    ) -> Point<f64, Logical> {
        // Pinned tiles are drawn at the canvas zoom too; fullscreen/maximized windows (in the
        // scrolling space) are not.
        if self.is_canvas() && self.floating.has_window(window) {
            delta.downscale(self.canvas_zoom())
        } else {
            delta
        }
    }

    // ---------------------------------------------------------------------------------------
    // Direct manipulation (mouse pan grab, touchpad swipe and pinch).
    // ---------------------------------------------------------------------------------------

    /// Whether a pan gesture is ongoing (of the given kind, if any).
    pub fn has_canvas_pan_gesture(&self, is_touchpad: Option<bool>) -> bool {
        self.canvas
            .pan_gesture
            .as_ref()
            .is_some_and(|g| is_touchpad.is_none_or(|t| g.is_touchpad == t))
    }

    /// Whether a pinch zoom gesture is ongoing.
    pub fn has_canvas_zoom_gesture(&self) -> bool {
        self.canvas.zoom_gesture.is_some()
    }

    /// Starts a pan gesture; the view stops where it is (ongoing zoom animations continue).
    pub fn canvas_pan_gesture_begin(&mut self, is_touchpad: bool) -> bool {
        if !self.is_canvas() {
            return false;
        }
        // Stop a glide or pan animation, but let a zoom animation finish.
        if self.canvas.view.is_animating() && !self.canvas.view.is_zoom_animating() {
            let current = self.canvas.view.current();
            self.canvas.view.jump_to(current);
        }
        let norm_factor = if is_touchpad {
            self.working_area.size.w / CANVAS_TOUCHPAD_GESTURE_MOVEMENT
        } else {
            1.
        };
        self.canvas.pan_gesture = Some(CanvasPanGesture {
            tracker_x: SwipeTracker::new(),
            tracker_y: SwipeTracker::new(),
            is_touchpad,
            norm_factor,
        });
        self.sync_canvas();
        true
    }

    /// Pans by `delta` gesture units (view pixels for the pointer); positive values move the
    /// view towards positive plane coordinates (the content moves the other way).
    pub fn canvas_pan_gesture_update(
        &mut self,
        delta: Point<f64, Logical>,
        timestamp: Duration,
        is_touchpad: bool,
    ) -> bool {
        let Some(gesture) = &mut self.canvas.pan_gesture else {
            return false;
        };
        if gesture.is_touchpad != is_touchpad {
            return false;
        }
        gesture.tracker_x.push(delta.x, timestamp);
        gesture.tracker_y.push(delta.y, timestamp);
        let view_delta = delta.upscale(gesture.norm_factor);

        let zoom = self.canvas.view.current().zoom;
        self.canvas.view.pan_by_plane(view_delta.downscale(zoom));
        self.sync_canvas();
        true
    }

    /// Ends the pan gesture. With a release `timestamp` the view glides on with the release
    /// velocity; without one it stays where it is.
    pub fn canvas_pan_gesture_end(&mut self, timestamp: Option<Duration>) -> bool {
        let Some(mut gesture) = self.canvas.pan_gesture.take() else {
            return false;
        };
        let Some(timestamp) = timestamp else {
            return true;
        };

        // Drop history older than the release, so that holding still before releasing does
        // not glide.
        gesture.tracker_x.push(0., timestamp);
        gesture.tracker_y.push(0., timestamp);

        let norm = gesture.norm_factor;
        let velocity = Point::<f64, Logical>::from((
            gesture.tracker_x.velocity(),
            gesture.tracker_y.velocity(),
        ))
        .upscale(norm);
        let glide = Point::<f64, Logical>::from((
            gesture.tracker_x.projected_end_pos() - gesture.tracker_x.pos(),
            gesture.tracker_y.projected_end_pos() - gesture.tracker_y.pos(),
        ))
        .upscale(norm);

        let zoom = self.canvas.view.current().zoom;
        let speed = velocity.x.hypot(velocity.y) / zoom;
        let pan = self.options.animations.canvas_pan.0;
        let focus = rect_center(self.working_area);
        self.canvas
            .view
            .glide(&self.clock, glide.downscale(zoom), speed, focus, pan);
        self.sync_canvas();
        true
    }

    /// Starts a pinch zoom gesture.
    pub fn canvas_zoom_gesture_begin(&mut self) -> bool {
        if !self.is_canvas() {
            return false;
        }
        let current = self.canvas.view.current();
        self.canvas.view.jump_to(current);
        self.canvas.zoom_gesture = Some(current.zoom);
        self.sync_canvas();
        true
    }

    /// Sets the zoom to `scale` times the zoom at the gesture start (clamped to the configured
    /// range), keeping the plane point under `anchor` (view coordinates; default: working area
    /// centre) fixed.
    pub fn canvas_zoom_gesture_update(
        &mut self,
        scale: f64,
        anchor: Option<Point<f64, Logical>>,
    ) -> bool {
        let Some(start) = self.canvas.zoom_gesture else {
            return false;
        };
        let anchor = anchor.unwrap_or_else(|| rect_center(self.working_area));
        let zoom = start * scale;
        if !zoom.is_finite() || !anchor.x.is_finite() || !anchor.y.is_finite() {
            return true;
        }
        let zoom = self.options.layout.canvas.clamp_zoom(zoom);
        let current = self.canvas.view.current();
        self.canvas
            .view
            .jump_to(current.zoomed_around(anchor, zoom));
        self.sync_canvas();
        true
    }

    /// Ends the pinch zoom gesture.
    pub fn canvas_zoom_gesture_end(&mut self) -> bool {
        if self.canvas.zoom_gesture.take().is_none() {
            return false;
        }
        self.sync_canvas();
        true
    }

    #[cfg(test)]
    pub(super) fn canvas_verify_invariants(&self) {
        if self.is_canvas() {
            assert!(
                self.floating.canvas_transform().is_some(),
                "canvas workspace must have an unbounded floating space"
            );
            if let Some(gesture) = &self.canvas.pan_gesture {
                assert!(gesture.norm_factor.is_finite() && gesture.norm_factor > 0.);
            }
            assert!(
                self.canvas.strip.is_some(),
                "canvas workspace must have a saved strip arrangement"
            );

            let canvas = &self.options.layout.canvas;
            let current = self.canvas.view.current();
            let target = self.canvas.view.target();
            assert!(current.is_finite(), "canvas offset/zoom must be finite");
            assert!(target.is_finite(), "canvas target must be finite");
            assert!(current.zoom > 0.);
            assert!(
                canvas.min_zoom() - 1e-9 <= target.zoom && target.zoom <= canvas.max_zoom() + 1e-9,
                "canvas target zoom {} must be within the configured range",
                target.zoom
            );
            if !self.canvas.view.is_animating() {
                assert_eq!(current, target);
                assert!(
                    canvas.min_zoom() - 1e-9 <= current.zoom
                        && current.zoom <= canvas.max_zoom() + 1e-9,
                    "settled canvas zoom must be within the configured range"
                );
            }
            assert_eq!(
                self.floating.canvas_transform(),
                Some(current),
                "floating space must use the current canvas transform"
            );
            assert_eq!(
                **self.floating.options(),
                *self.floating_options(&self.options),
                "canvas floating options must be the frame-compensated workspace options"
            );

            for tile in self.floating.tiles() {
                assert!(
                    tile.canvas_plane_pos.is_none(),
                    "canvas tiles keep their plane position in the floating space data"
                );
            }
            for tile in self.scrolling.tiles() {
                assert!(
                    tile.restore_to_floating,
                    "canvas scrolling space may only hold windows that return to the canvas"
                );
            }
        } else {
            assert!(self.floating.canvas_transform().is_none());
            assert!(self.canvas.strip.is_none());
            assert!(
                self.canvas.pan_gesture.is_none(),
                "strip workspaces have no canvas gestures"
            );
            assert!(
                self.canvas.zoom_gesture.is_none(),
                "strip workspaces have no canvas gestures"
            );
            assert_eq!(self.canvas.frame_zoom, 1.);
            for tile in self.tiles() {
                assert!(
                    tile.canvas_plane_pos.is_none(),
                    "strip workspaces have no plane positions"
                );
            }
        }
    }
}
