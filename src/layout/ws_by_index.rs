//! ashd: workspaces created by index (`layout { create-workspaces-by-index; }`).
//!
//! Upstream niri keeps exactly one empty workspace at the end of every monitor and removes every
//! other empty unnamed workspace, so `focus-workspace 5` with two workspaces lands on the third
//! (empty) one. With this option:
//!
//! - an index reference past the end (`focus-workspace N`, `move-window-to-workspace N`,
//!   `move-column-to-workspace N`, N counted over the visible workspaces like every index reference
//!   in the fork) first appends empty workspaces until index N exists;
//! - an empty unnamed workspace survives as long as a workspace *after* it is in use: has windows,
//!   has a name (and is not hidden), or is the active workspace. Only the trailing empty workspaces
//!   are removed, back to the single empty workspace niri always keeps at the end.
//!
//! Hidden (scratchpad) workspaces are named, so they are never removed, and they don't keep the
//! empty workspaces before them alive either: they don't count for numbering.
//!
//! With `empty-workspace-above-first` index 1 is the empty workspace above, and creating
//! workspace 2 on an all-empty monitor (one workspace) also adds the empty one at the end, so the
//! monitor never has exactly two workspaces.

use super::monitor::Monitor;
use super::LayoutElement;

impl<W: LayoutElement> Monitor<W> {
    /// Whether `create-workspaces-by-index` is on for this monitor.
    pub(super) fn creates_workspaces_by_index(&self) -> bool {
        self.options.layout.create_workspaces_by_index
    }

    /// Appends empty workspaces until the (real) index `idx` exists, if the option is on.
    pub(super) fn ensure_workspace_for_index(&mut self, idx: usize) {
        if !self.creates_workspaces_by_index() {
            return;
        }
        while self.workspaces.len() <= idx {
            self.add_workspace_bottom();
        }

        // With empty-workspace-above-first a monitor has 1 or 3+ workspaces: the single
        // workspace of an all-empty monitor is both the empty one above the first and the empty
        // one at the end. Creating index 1 from there makes workspace 2 the first real one, so
        // it needs an empty workspace after it (like upstream once a window opens on it).
        if self.options.layout.empty_workspace_above_first && self.workspaces.len() == 2 {
            self.add_workspace_bottom();
        }
    }

    /// Like [`Self::ensure_workspace_for_index`], but only if there is something to move:
    /// `window`, or the active window of the active workspace.
    pub(super) fn ensure_workspace_for_move(&mut self, window: Option<&W::Id>, idx: usize) {
        let has_window = window.is_some() || self.active_workspace_ref().active_window().is_some();
        if has_window {
            self.ensure_workspace_for_index(idx);
        }
    }

    /// Index of the first workspace that clean-up may remove with the option on: everything
    /// before it is kept (empty workspaces included) because a later workspace is in use.
    pub(super) fn by_index_trim_start(&self) -> usize {
        let range_start = usize::from(self.options.layout.empty_workspace_above_first);
        let last_used = self
            .workspaces
            .iter()
            .enumerate()
            .skip(range_start)
            .rev()
            .find(|(idx, ws)| {
                *idx == self.active_workspace_idx
                    || (!ws.is_hidden() && ws.has_windows_or_name())
                    || self.is_fullscreen_origin(*idx)
            })
            .map(|(idx, _)| idx);
        last_used.map_or(range_start, |idx| idx + 1)
    }

    /// `clean_up_workspaces()` with the option on: removes the empty unnamed workspaces after the
    /// last used one, except the last workspace.
    pub(super) fn clean_up_workspaces_by_index(&mut self) {
        let trim_start = self.by_index_trim_start();
        let last = self.workspaces.len() - 1;
        for idx in (trim_start..last).rev() {
            if !self.workspaces[idx].has_windows_or_name() {
                self.workspaces.remove(idx);
                // The active workspace is before trim_start or the last one.
                if self.active_workspace_idx > idx {
                    self.active_workspace_idx -= 1;
                }
            }
        }

        // Special case handling when empty_workspace_above_first is set and all workspaces
        // are empty (same as upstream).
        if self.options.layout.empty_workspace_above_first && self.workspaces.len() == 2 {
            assert!(!self.workspaces[0].has_windows_or_name());
            assert!(!self.workspaces[1].has_windows_or_name());
            self.workspaces.remove(1);
            self.active_workspace_idx = 0;
        }
    }

    #[cfg(test)]
    pub(super) fn verify_by_index_invariants(&self) {
        let trim_start = self.by_index_trim_start();
        let last = self.workspaces.len() - 1;
        for idx in trim_start..last {
            assert!(
                self.workspaces[idx].has_windows_or_name(),
                "with create-workspaces-by-index, empty unnamed workspaces must be followed by a \
                 used one (idx {idx}, trim start {trim_start})"
            );
        }
    }
}
