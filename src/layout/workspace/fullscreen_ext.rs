//! ashd: workspaces created for fullscreen windows, workspace side.
//!
//! See `crate::layout::fullscreen_ws` for the semantics.

use smithay::utils::{Logical, Point};

use super::Workspace;
use crate::layout::fullscreen_ws::{FullscreenReturn, FullscreenSlot};
use crate::layout::LayoutElement;

impl<W: LayoutElement> Workspace<W> {
    /// The fullscreen slot, if this workspace was created for a fullscreen window.
    pub fn fullscreen_slot(&self) -> Option<&FullscreenSlot<W::Id>> {
        self.fullscreen_slot.as_ref()
    }

    /// Whether this workspace counts for workspace numbers (`focus-workspace N`, IPC `idx`):
    /// hidden (scratchpad) workspaces and fullscreen workspaces don't.
    pub fn is_numbered(&self) -> bool {
        !self.is_hidden() && self.fullscreen_slot.is_none()
    }

    /// Whether this workspace, once shown and settled, puts a fullscreen window above the top
    /// layer (`fullscreen_window_above_top_layer()` without waiting for the view to settle).
    pub fn fullscreen_covers_top_layer(&self) -> bool {
        !self.floating_is_active()
            && self
                .scrolling
                .active_column()
                .is_some_and(|col| col.is_pending_fullscreen())
    }

    /// Whether `window` is fullscreen in this workspace's layout, alone in its column (the
    /// windows that get a workspace of their own).
    pub fn is_layout_fullscreen(&self, window: &W::Id) -> bool {
        self.scrolling.columns().any(|col| {
            col.contains(window) && col.is_pending_fullscreen() && col.tiles().count() == 1
        })
    }

    /// Fullscreen windows that are not on a workspace of their own (yet).
    pub(in crate::layout) fn fullscreen_windows_to_dedicate(&self) -> Vec<W::Id> {
        if self.is_hidden() {
            return Vec::new();
        }
        let own = self
            .fullscreen_slot
            .as_ref()
            .and_then(|s| s.window.as_ref());
        self.scrolling
            .columns()
            .filter(|col| col.is_pending_fullscreen() && col.tiles().count() == 1)
            .filter_map(|col| {
                col.tiles()
                    .next()
                    .map(|(tile, _)| tile.window().id().clone())
            })
            .filter(|id| Some(id) != own)
            .collect()
    }

    /// Where `window` goes back to when it leaves fullscreen.
    pub(in crate::layout) fn fullscreen_return_pos(&self, window: &W::Id) -> FullscreenReturn {
        if self.is_canvas() {
            let plane = self
                .scrolling
                .tiles()
                .find(|tile| tile.window().id() == window)
                .and_then(|tile| tile.canvas_plane_pos);
            return FullscreenReturn::Plane(plane);
        }

        let columns: Vec<_> = self.scrolling.columns().collect();
        let Some(idx) = columns.iter().position(|col| col.contains(window)) else {
            return FullscreenReturn::Auto;
        };
        FullscreenReturn::Column {
            after: idx.checked_sub(1).map(|prev| columns[prev].id()),
        }
    }

    /// Column index to insert a window returning from fullscreen at, `None` for next to the
    /// focused column.
    pub(in crate::layout) fn fullscreen_return_column(
        &self,
        ret: &FullscreenReturn,
    ) -> Option<usize> {
        let FullscreenReturn::Column { after } = ret else {
            return None;
        };
        match after {
            None => Some(0),
            Some(after) => self
                .scrolling
                .columns()
                .position(|col| col.id() == *after)
                .map(|idx| idx + 1),
        }
    }

    /// Puts a window that came back from fullscreen to its plane position on a canvas.
    pub(in crate::layout) fn fullscreen_restore_plane(
        &mut self,
        window: &W::Id,
        plane: Point<f64, Logical>,
    ) {
        if self.is_canvas() && self.floating.has_window(window) {
            self.floating.set_canvas_plane_pos(window, plane, false);
        }
    }

    /// Whether `window` (fullscreen in the scrolling space) goes floating when it leaves
    /// fullscreen.
    pub(in crate::layout) fn fullscreen_restores_to_floating(&self, window: &W::Id) -> bool {
        self.scrolling
            .tiles()
            .find(|tile| tile.window().id() == window)
            .is_some_and(|tile| tile.restore_to_floating)
    }

    pub(in crate::layout) fn fullscreen_set_restore_to_floating(
        &mut self,
        window: &W::Id,
        value: bool,
    ) {
        if let Some(tile) = self
            .scrolling
            .tiles_mut()
            .find(|tile| tile.window().id() == window)
        {
            tile.restore_to_floating = value;
        }
    }

    #[cfg(test)]
    pub(in crate::layout) fn fullscreen_verify_invariants(&self) {
        let Some(slot) = &self.fullscreen_slot else {
            return;
        };
        assert!(!self.is_hidden(), "a fullscreen workspace can't be hidden");
        assert_ne!(
            slot.origin, self.id,
            "a fullscreen workspace is not its own origin"
        );
    }
}
