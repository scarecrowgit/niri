//! ashd: saving and restoring the strip (column) arrangement around canvas mode.
//!
//! When a workspace enters canvas mode, all tiles leave the scrolling space. To bring the strip
//! back unchanged on the way out, the column structure is snapshotted here: column order, widths,
//! full-width flags, display mode, tile order and heights within each column and the active
//! tile/column. This lives in a child module of `scrolling` so it can read the private `Column`
//! fields without widening their visibility.

use niri_ipc::ColumnDisplay;

use super::{Column, ColumnWidth, ScrollingSpace, ViewOffset, WindowHeight};
use crate::layout::tile::Tile;
use crate::layout::LayoutElement;

/// One saved column.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedColumn<Id> {
    /// Windows in the column, top to bottom, with their requested heights.
    pub tiles: Vec<(Id, WindowHeight)>,
    /// Index of the active tile in `tiles`.
    pub active_tile_idx: usize,
    pub width: ColumnWidth,
    pub preset_width_idx: Option<usize>,
    pub is_full_width: bool,
    pub display_mode: ColumnDisplay,
}

/// Strip arrangement of a workspace, saved when it enters canvas mode.
#[derive(Debug, Clone)]
pub struct StripArrangement<Id> {
    /// Columns, left to right.
    pub columns: Vec<SavedColumn<Id>>,
    /// Index of the active column.
    pub active_column_idx: usize,
    /// Windows that are floating in strip mode; they stay floating after a round trip.
    pub floating: Vec<Id>,
    /// Whether the floating layout had focus in strip mode.
    pub floating_was_active: bool,
}

impl<Id: PartialEq> StripArrangement<Id> {
    /// Whether the window is part of a saved column.
    pub fn has_tiled(&self, id: &Id) -> bool {
        self.columns
            .iter()
            .any(|col| col.tiles.iter().any(|(tile, _)| tile == id))
    }

    /// Whether the window is remembered as floating in strip mode.
    pub fn is_floating(&self, id: &Id) -> bool {
        self.floating.contains(id)
    }
}

impl<W: LayoutElement> ScrollingSpace<W> {
    /// Jumps the view offset to where it is going (the mode switch animates the windows
    /// instead).
    pub fn settle_view_offset(&mut self) {
        self.view_offset = ViewOffset::Static(self.view_offset.target());
    }

    /// Snapshots the column structure (not including floating windows).
    pub(in crate::layout) fn snapshot_columns(&self) -> (Vec<SavedColumn<W::Id>>, usize) {
        let columns = self
            .columns
            .iter()
            .map(|col| SavedColumn {
                tiles: col
                    .tiles
                    .iter()
                    .zip(&col.data)
                    .map(|(tile, data)| (tile.window().id().clone(), data.height))
                    .collect(),
                active_tile_idx: col.active_tile_idx,
                width: col.width,
                preset_width_idx: col.preset_width_idx,
                is_full_width: col.is_full_width,
                display_mode: col.display_mode,
            })
            .collect();
        (columns, self.active_column_idx)
    }

    /// Inserts a column built from `tiles` with the saved properties at `idx`.
    ///
    /// `tiles` must be non-empty; they are inserted in the given order. Heights are taken from
    /// `saved` by window id; the active tile is the saved one if present.
    pub(in crate::layout) fn insert_saved_column(
        &mut self,
        idx: usize,
        tiles: Vec<Tile<W>>,
        saved: &SavedColumn<W::Id>,
    ) {
        let mut tiles = tiles.into_iter();
        let Some(first) = tiles.next() else {
            return;
        };

        let mut column = Column::new_with_tile(
            first,
            self.view_size,
            self.working_area,
            self.parent_area,
            self.scale,
            saved.width,
            saved.is_full_width,
        );
        for tile in tiles {
            let idx = column.tiles.len();
            column.add_tile_at(idx, tile);
        }

        let preset_heights = column.options.layout.preset_window_heights.len();
        for (tile, data) in column.tiles.iter().zip(&mut column.data) {
            let saved_height = saved
                .tiles
                .iter()
                .find(|(id, _)| id == tile.window().id())
                .map(|(_, height)| *height);
            data.height = match saved_height {
                Some(WindowHeight::Preset(idx)) if idx >= preset_heights => WindowHeight::auto_1(),
                Some(height) => height,
                None => WindowHeight::auto_1(),
            };
        }
        // At most one tile may have a non-auto height; a lone tile has weight 1.
        let mut seen_non_auto = false;
        for data in &mut column.data {
            if !matches!(data.height, WindowHeight::Auto { .. }) {
                if seen_non_auto {
                    data.height = WindowHeight::auto_1();
                }
                seen_non_auto = true;
            }
        }
        if column.data.len() == 1 {
            if let WindowHeight::Auto { weight } = &mut column.data[0].height {
                *weight = 1.;
            }
        }

        if saved
            .preset_width_idx
            .is_some_and(|idx| idx < column.options.layout.preset_column_widths.len())
        {
            column.preset_width_idx = saved.preset_width_idx;
        }

        // The active tile is the saved one if it is still around.
        let active_id = saved.tiles.get(saved.active_tile_idx).map(|(id, _)| id);
        if let Some(active_idx) = active_id.and_then(|id| column.position(id)) {
            column.activate_idx(active_idx);
        }

        column.set_column_display(saved.display_mode);
        column.update_tile_sizes(false);

        let idx = idx.min(self.columns.len());
        self.add_column(Some(idx), column, false, None);
    }

    /// Number of columns.
    /// ashd IPC: strip x of every column (x = 0 at the first column, gaps included), in order.
    pub fn ipc_column_xs(&self) -> Vec<f64> {
        let mut xs: Vec<f64> = self.column_xs(self.data.iter().copied()).collect();
        // Drop the one-past-last entry.
        xs.pop();
        xs
    }

    pub(in crate::layout) fn column_count(&self) -> usize {
        self.columns.len()
    }
}

impl<W: LayoutElement> Column<W> {
    /// Takes the tiles out of a column, top to bottom.
    pub(in crate::layout) fn into_tiles(self) -> Vec<Tile<W>> {
        self.tiles
    }

    /// Mutable access to the tiles, for canvas bookkeeping on tiles that leave a workspace.
    pub(in crate::layout) fn tiles_for_canvas_mut(&mut self) -> impl Iterator<Item = &mut Tile<W>> {
        self.tiles.iter_mut()
    }
}
