//! ashd: hidden (scratchpad) workspaces, workspace side.
//!
//! See `crate::layout::scratch` for the semantics. The hooks here keep every tile's
//! [`ScratchOrigin`] up to date as it moves between workspaces.

use smithay::utils::{Logical, Point};

use super::Workspace;
use crate::layout::scratch::ScratchOrigin;
use crate::layout::scrolling::Column;
use crate::layout::tile::Tile;
use crate::layout::LayoutElement;

impl<W: LayoutElement> Workspace<W> {
    /// Whether this is a hidden (scratchpad) workspace: a named workspace whose config entry has
    /// `hidden`.
    pub fn is_hidden(&self) -> bool {
        self.hidden && self.name.is_some()
    }

    /// Sets the `hidden` flag from the workspace's config entry.
    pub fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
        if self.is_hidden() {
            // Windows that were already here count as hidden now.
            for tile in self.tiles_mut() {
                let origin = tile
                    .scratch_origin
                    .get_or_insert_with(ScratchOrigin::default);
                if origin.hidden_seq.is_none() {
                    origin.hidden_seq = Some(ScratchOrigin::next_hidden_seq());
                }
            }
        }
    }

    /// Records where a tile leaving this workspace came from. Call before the canvas hook clears
    /// the tile's plane position.
    pub(super) fn scratch_on_tile_removed(&self, tile: &mut Tile<W>) {
        if self.is_hidden() {
            // Keep the origin from before the tile was hidden.
            return;
        }
        let plane_pos = if self.is_canvas() {
            tile.canvas_plane_pos
        } else {
            None
        };
        tile.scratch_origin = Some(ScratchOrigin {
            workspace: Some(self.id),
            plane_pos,
            hidden_seq: None,
        });
    }

    /// Same as [`Self::scratch_on_tile_removed`] for the tiles of a column.
    pub(super) fn scratch_on_column_removed(&self, column: &mut Column<W>) {
        if self.is_hidden() {
            return;
        }
        for tile in column.tiles_for_canvas_mut() {
            tile.scratch_origin = Some(ScratchOrigin {
                workspace: Some(self.id),
                plane_pos: None,
                hidden_seq: None,
            });
        }
    }

    /// Updates the origin of a tile entering this workspace. Call before the canvas hook.
    ///
    /// Returns the plane position to restore when the tile comes back from a hidden workspace to
    /// the canvas it was hidden from; the caller sets it after the canvas placement.
    pub(super) fn scratch_on_tile_added(&self, tile: &mut Tile<W>) -> Option<Point<f64, Logical>> {
        if self.is_hidden() {
            let origin = tile
                .scratch_origin
                .get_or_insert_with(ScratchOrigin::default);
            if origin.hidden_seq.is_none() {
                origin.hidden_seq = Some(ScratchOrigin::next_hidden_seq());
            }
            return None;
        }

        let origin = tile.scratch_origin.take()?;
        origin.hidden_seq?;
        if !self.is_canvas() {
            return None;
        }

        if origin.workspace == Some(self.id) {
            if let Some(plane_pos) = origin.plane_pos {
                return Some(plane_pos);
            }
        }

        // Returning from the scratchpad without a plane position here: open at the viewport
        // centre rather than at a position relative to some other view.
        tile.floating_pos = None;
        None
    }

    /// Updates the origin of the tiles of a column entering this workspace.
    pub(super) fn scratch_on_column_added(&self, column: &mut Column<W>) {
        if !self.is_hidden() {
            for tile in column.tiles_for_canvas_mut() {
                tile.scratch_origin = None;
            }
            return;
        }
        for tile in column.tiles_for_canvas_mut() {
            let origin = tile
                .scratch_origin
                .get_or_insert_with(ScratchOrigin::default);
            if origin.hidden_seq.is_none() {
                origin.hidden_seq = Some(ScratchOrigin::next_hidden_seq());
            }
        }
    }

    #[cfg(test)]
    pub(super) fn scratch_verify_invariants(&self) {
        if !self.is_hidden() {
            return;
        }
        for tile in self.tiles() {
            assert!(
                tile.scratch_origin.is_some_and(|o| o.hidden_seq.is_some()),
                "tiles on a hidden workspace must have a hide sequence number"
            );
        }
    }
}
