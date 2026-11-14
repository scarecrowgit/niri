//! ashd: unbounded (infinite canvas) mode of the floating space.
//!
//! In canvas mode the per-tile `Data` positions are plane coordinates without any clamping, and
//! everything the floating space exposes to the rest of the layout is in *unscaled view*
//! coordinates (`plane - offset`, see `crate::layout::canvas`). Pinned tiles store unscaled view
//! coordinates directly, so they stay put on screen while the view pans.
//!
//! In bounded (strip) mode every helper here reduces to the stock behaviour.

use smithay::utils::{Logical, Point, Rectangle};

use super::{Data, FloatingSpace};
use crate::layout::canvas::CanvasTransform;
use crate::layout::tile::Tile;
use crate::layout::LayoutElement;
use crate::utils::center_preferring_top_left_in_area;

/// Position of a tile in the coordinates this floating space reports (unscaled view in canvas
/// mode, working-area-relative logical in strip mode).
pub(super) fn ashd_u_pos(canvas: Option<CanvasTransform>, data: &Data) -> Point<f64, Logical> {
    match canvas {
        Some(transform) if !data.pinned => transform.plane_to_unscaled(data.logical_pos),
        _ => data.logical_pos,
    }
}

impl Data {
    fn new_canvas<W: LayoutElement>(
        working_area: Rectangle<f64, Logical>,
        tile: &Tile<W>,
        plane_pos: Point<f64, Logical>,
    ) -> Self {
        let mut rv = Self {
            pos: Point::default(),
            logical_pos: Point::default(),
            size: Default::default(),
            working_area,
            canvas: true,
            pinned: false,
        };
        rv.update(tile);
        rv.set_logical_pos(plane_pos);
        rv
    }
}

impl<W: LayoutElement> FloatingSpace<W> {
    /// Area that size-fraction positions are relative to: the working area, in the coordinates
    /// this space reports. In canvas mode, the visible part of the working area.
    pub(super) fn ashd_frac_area(&self) -> Rectangle<f64, Logical> {
        match self.canvas {
            Some(transform) => Rectangle::new(
                self.working_area.loc.downscale(transform.zoom),
                self.working_area.size.downscale(transform.zoom),
            ),
            None => self.working_area,
        }
    }

    /// Tile centre in reported coordinates (consistent across pinned and normal tiles).
    pub(super) fn ashd_center(&self, data: &Data) -> Point<f64, Logical> {
        match self.canvas {
            Some(transform) if !data.pinned => transform.plane_to_unscaled(data.center()),
            _ => data.center(),
        }
    }

    /// Data for a tile entering the canvas: exact plane position if it has one, otherwise its
    /// stored or default floating position relative to the visible area, otherwise centred.
    pub(super) fn ashd_new_data(&self, tile: &mut Tile<W>) -> Data {
        let transform = self.canvas.unwrap_or_default();
        let plane = match tile.canvas_plane_pos.take() {
            Some(plane) => plane,
            None => {
                let unscaled = self.stored_or_default_tile_pos(tile).unwrap_or_else(|| {
                    center_preferring_top_left_in_area(self.ashd_frac_area(), tile.tile_size())
                });
                transform.unscaled_to_plane(unscaled)
            }
        };
        Data::new_canvas(self.working_area, tile, plane)
    }

    /// Records the plane position of a tile leaving the canvas.
    pub(super) fn ashd_on_remove(&self, tile: &mut Tile<W>, data: &Data) {
        let unscaled = ashd_u_pos(self.canvas, data);
        let plane = self.canvas.unwrap_or_default().unscaled_to_plane(unscaled);
        tile.canvas_plane_pos = Some(plane);
        tile.floating_pos = Some(self.logical_to_size_frac(unscaled));
    }

    /// Data position for centring tile `idx` in the visible area (canvas mode).
    pub(super) fn ashd_center_pos(&self, idx: usize) -> Point<f64, Logical> {
        let data = &self.data[idx];
        let unscaled = center_preferring_top_left_in_area(self.ashd_frac_area(), data.size);
        self.ashd_unscaled_to_data(data, unscaled)
    }

    fn ashd_unscaled_to_data(
        &self,
        data: &Data,
        unscaled: Point<f64, Logical>,
    ) -> Point<f64, Logical> {
        match self.canvas {
            Some(transform) if !data.pinned => transform.unscaled_to_plane(unscaled),
            _ => unscaled,
        }
    }

    /// Whether a canvas tile at `tile_pos` is near enough to `view_rect` (both unscaled view
    /// coordinates) to be worth rendering. Always true in strip mode.
    pub(super) fn ashd_tile_visible(
        &self,
        tile: &Tile<W>,
        tile_pos: Point<f64, Logical>,
        view_rect: Rectangle<f64, Logical>,
    ) -> bool {
        if self.canvas.is_none() {
            return true;
        }

        // Generous margin: shadows, focus rings and popups extend past the tile.
        let margin = f64::max(view_rect.size.w, view_rect.size.h) / 2.;
        let mut area = view_rect;
        area.loc -= Point::from((margin, margin));
        area.size += smithay::utils::Size::from((margin * 2., margin * 2.));
        Rectangle::new(tile_pos, tile.tile_size()).overlaps(area)
    }

    /// Canvas transform, if the space is in canvas mode.
    pub fn canvas_transform(&self) -> Option<CanvasTransform> {
        self.canvas
    }

    /// Switches between canvas (`Some`) and strip (`None`) mode.
    ///
    /// Entering canvas mode maps the current on-screen positions through `transform` onto the
    /// plane, so nothing jumps when `transform` matches what is on screen (normally identity).
    /// Leaving canvas mode maps the current view positions back (clamped to the working area),
    /// and unpins everything.
    pub fn set_canvas_mode(&mut self, transform: Option<CanvasTransform>) {
        match (self.canvas, transform) {
            (None, Some(transform)) => {
                for data in &mut self.data {
                    let plane = transform.view_to_plane(data.logical_pos);
                    data.canvas = true;
                    data.pinned = false;
                    data.set_logical_pos(plane);
                }
                self.canvas = Some(transform);
            }
            (Some(current), None) => {
                for data in &mut self.data {
                    let view = ashd_u_pos(Some(current), data).upscale(current.zoom);
                    data.canvas = false;
                    data.pinned = false;
                    data.set_logical_pos(view);
                }
                self.canvas = None;
            }
            (Some(_), Some(transform)) => self.canvas = Some(transform),
            (None, None) => (),
        }
    }

    /// Updates the view transform (every frame while the canvas view animates).
    pub fn set_canvas_transform(&mut self, transform: CanvasTransform) {
        if let Some(old) = self.canvas {
            // Closing windows stay where they were on the plane while the view pans: they are
            // positioned in unscaled view coordinates, `plane - offset`.
            let shift = old.offset - transform.offset;
            if shift != Point::from((0., 0.)) {
                for closing in &mut self.closing_windows {
                    closing.offset_pos(shift);
                }
            }
            self.canvas = Some(transform);
        }
    }

    /// Tiles with their plane rectangles (tile size, at the target position).
    pub fn canvas_plane_rects(
        &self,
    ) -> impl Iterator<Item = (&Tile<W>, Rectangle<f64, Logical>)> + '_ {
        let transform = self.canvas.unwrap_or_default();
        self.tiles.iter().zip(&self.data).map(move |(tile, data)| {
            let unscaled = ashd_u_pos(Some(transform), data);
            let plane = transform.unscaled_to_plane(unscaled);
            (tile, Rectangle::new(plane, data.size))
        })
    }

    /// ashd IPC: per tile, in tile order, whether it is pinned and its stored (target, not
    /// animated) position: the plane position, or the unscaled view position of a pinned tile.
    pub fn canvas_stored_positions(
        &self,
    ) -> impl Iterator<Item = (bool, Point<f64, Logical>)> + '_ {
        self.data.iter().map(|data| (data.pinned, data.logical_pos))
    }

    /// Plane rectangle of a tile.
    pub fn canvas_plane_rect(&self, id: &W::Id) -> Option<Rectangle<f64, Logical>> {
        self.canvas_plane_rects()
            .find(|(tile, _)| tile.window().id() == id)
            .map(|(_, rect)| rect)
    }

    /// Moves a tile to a plane position (top-left).
    pub fn set_canvas_plane_pos(&mut self, id: &W::Id, plane: Point<f64, Logical>, animate: bool) {
        let Some(idx) = self.idx_of(id) else {
            return;
        };
        let transform = self.canvas.unwrap_or_default();
        let data = &self.data[idx];
        let pos = if data.pinned {
            transform.plane_to_unscaled(plane)
        } else {
            plane
        };
        self.move_to(idx, pos, animate);
    }

    pub fn is_pinned(&self, id: &W::Id) -> bool {
        self.idx_of(id).is_some_and(|idx| self.data[idx].pinned)
    }

    /// Pins or unpins a canvas tile without moving it on screen.
    pub fn set_pinned(&mut self, id: &W::Id, pinned: bool) {
        let Some(transform) = self.canvas else {
            return;
        };
        let Some(idx) = self.idx_of(id) else {
            return;
        };
        let data = &mut self.data[idx];
        if data.pinned == pinned {
            return;
        }

        let unscaled = ashd_u_pos(Some(transform), data);
        data.pinned = pinned;
        let pos = if pinned {
            unscaled
        } else {
            transform.unscaled_to_plane(unscaled)
        };
        data.set_logical_pos(pos);
    }

    #[cfg(test)]
    pub(super) fn canvas_verify_invariants(&self) {
        match self.canvas {
            Some(transform) => {
                assert!(transform.is_finite(), "canvas transform must be finite");
                assert!(transform.zoom > 0., "canvas zoom must be positive");
                for data in &self.data {
                    assert!(data.canvas, "canvas floating space must be unbounded");
                    assert!(
                        data.logical_pos.x.is_finite() && data.logical_pos.y.is_finite(),
                        "canvas positions must be finite"
                    );
                }
            }
            None => {
                for data in &self.data {
                    assert!(!data.canvas, "strip floating space must be bounded");
                    assert!(!data.pinned, "only canvas tiles can be pinned");
                }
            }
        }
    }
}
