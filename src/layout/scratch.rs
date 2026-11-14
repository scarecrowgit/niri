//! rice: hidden (scratchpad) workspaces.
//!
//! A named workspace with `hidden` in its config entry is a *hidden* workspace. It keeps its place
//! in its monitor's workspace list (the layout code works with real indices), but:
//!
//! - it doesn't count for index references: `focus-workspace N`, `move-window-to-workspace N`,
//!   `move-workspace-to-index N` and IPC `Workspace::idx` use 1-based *visual* indices over the
//!   visible workspaces of the monitor (see [`Layout::resolve_workspace_index`],
//!   [`Layout::ipc_workspace_idx`]); hidden workspaces have IPC `idx` 0 and are addressed by name
//!   or id;
//! - it is not announced through the ext-workspace protocol;
//! - it never becomes the active workspace of its monitor (activation requests for it are ignored,
//!   up/down navigation and workspace-switch gestures skip it);
//! - it is never rendered, neither normally nor in the overview, and takes up no space there
//!   (workspace switch animations and gestures run in *visual* indices, see `Monitor`);
//! - its windows can't be focused through activation (focus by id, xdg-activation), are left out of
//!   the recent-windows switcher, and focus-follows-mouse never reaches them.
//!
//! Windows are moved there and back with the regular move actions or with
//! [`Layout::hide_window`] / [`Layout::unhide_window`]. Every tile carries a [`ScratchOrigin`]
//! across workspaces: the workspace (and canvas plane position) it was last removed from. A tile
//! that returns from a hidden workspace to its origin canvas goes back to its plane position;
//! other returns to a canvas land at the viewport centre, returns to a strip workspace become a
//! new column right of the focused one.

use std::sync::atomic::{AtomicU64, Ordering};

use smithay::utils::{Logical, Point};

use super::monitor::Monitor;
use super::workspace::{Workspace, WorkspaceId};
use super::{ActivateWindow, InteractiveMoveState, Layout, LayoutElement, MonitorSet};

/// Where a tile came from before it landed on a hidden workspace.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ScratchOrigin {
    /// Workspace the tile was last removed from (outside hidden workspaces); `None` for windows
    /// that opened directly on a hidden workspace.
    pub workspace: Option<WorkspaceId>,
    /// Plane position (tile top-left) on that workspace, if it was a canvas.
    pub plane_pos: Option<Point<f64, Logical>>,
    /// Set once the tile is on a hidden workspace; increases with every hide.
    pub hidden_seq: Option<u64>,
}

static HIDDEN_SEQ: AtomicU64 = AtomicU64::new(1);

impl ScratchOrigin {
    pub(super) fn next_hidden_seq() -> u64 {
        HIDDEN_SEQ.fetch_add(1, Ordering::Relaxed)
    }
}

/// A window on a hidden workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct HiddenWindow<Id> {
    pub id: Id,
    /// Hidden workspace the window is on.
    pub workspace: WorkspaceId,
    /// Existing workspace it was hidden from, if known.
    pub origin: Option<WorkspaceId>,
    /// Hide order (larger is more recent; 0 if unknown).
    pub hidden_seq: u64,
}

/// Visual index (0-based, counting only visible workspaces) of the workspace at real index `idx`
/// in `workspaces`, or `None` if it is hidden.
pub fn visual_ws_idx<W: LayoutElement>(workspaces: &[Workspace<W>], idx: usize) -> Option<usize> {
    let ws = workspaces.get(idx)?;
    if ws.is_hidden() {
        return None;
    }
    Some(
        workspaces[..idx]
            .iter()
            .filter(|ws| !ws.is_hidden())
            .count(),
    )
}

/// Real index of the workspace with the 0-based *visual* index `visual_idx` in `workspaces`.
///
/// Indices past the last visible workspace map past the end of `workspaces` by the same amount,
/// so that the out-of-range handling of the caller (usually clamping to the last workspace, which
/// is never hidden) stays as it is.
pub fn real_ws_idx<W: LayoutElement>(workspaces: &[Workspace<W>], visual_idx: usize) -> usize {
    let mut visible = 0;
    for (idx, ws) in workspaces.iter().enumerate() {
        if ws.is_hidden() {
            continue;
        }
        if visible == visual_idx {
            return idx;
        }
        visible += 1;
    }
    workspaces.len() + (visual_idx - visible)
}

impl<W: LayoutElement> Layout<W> {
    /// Workspace index for IPC (`niri_ipc::Workspace::idx`): 1-based among the visible
    /// workspaces of the monitor, `0` for hidden workspaces.
    pub fn ipc_workspace_idx(&self, mon: Option<&Monitor<W>>, ws_idx: usize) -> u8 {
        let visual = match (mon, &self.monitor_set) {
            (Some(mon), _) => visual_ws_idx(&mon.workspaces, ws_idx),
            (None, MonitorSet::NoOutputs { workspaces }) => visual_ws_idx(workspaces, ws_idx),
            (None, MonitorSet::Normal { .. }) => None,
        };
        visual.map_or(0, |idx| u8::try_from(idx + 1).unwrap_or(u8::MAX))
    }

    /// Resolves a 1-based workspace index from an index reference (`focus-workspace N`,
    /// `move-window-to-workspace N`, …) to a real index. The index counts the visible workspaces
    /// of the monitor of `window` (the active monitor if `None`).
    pub fn resolve_workspace_index(&self, window: Option<&W::Id>, index: u8) -> usize {
        let visual_idx = usize::from(index.saturating_sub(1));
        match &self.monitor_set {
            MonitorSet::Normal {
                monitors,
                active_monitor_idx,
                ..
            } => {
                let mon = window
                    .and_then(|win| monitors.iter().find(|mon| mon.has_window(win)))
                    .unwrap_or(&monitors[*active_monitor_idx]);
                real_ws_idx(&mon.workspaces, visual_idx)
            }
            MonitorSet::NoOutputs { workspaces } => real_ws_idx(workspaces, visual_idx),
        }
    }

    /// Windows on hidden workspaces, most recently hidden first.
    pub fn hidden_windows(&self) -> Vec<HiddenWindow<W::Id>> {
        let mut hidden = Vec::new();
        for (_, _, ws) in self.workspaces() {
            if !ws.is_hidden() {
                continue;
            }
            for tile in ws.tiles() {
                let origin = tile.scratch_origin.unwrap_or_default();
                let origin_ws = origin
                    .workspace
                    .filter(|id| self.find_workspace_by_id(*id).is_some());
                hidden.push(HiddenWindow {
                    id: tile.window().id().clone(),
                    workspace: ws.id(),
                    origin: origin_ws,
                    hidden_seq: origin.hidden_seq.unwrap_or(0),
                });
            }
        }
        hidden.sort_by_key(|win| std::cmp::Reverse(win.hidden_seq));
        hidden
    }

    /// Whether a window is on a hidden workspace.
    pub fn is_window_hidden(&self, window: &W::Id) -> bool {
        self.workspaces()
            .any(|(_, _, ws)| ws.is_hidden() && ws.has_window(window))
    }

    /// Moves a window (the focused one if `None`) to the first hidden workspace of its monitor,
    /// or to the first hidden workspace anywhere if its monitor has none.
    ///
    /// The window's workspace is remembered and shows up as
    /// `niri_ipc::Window::hidden_from_workspace_id`. Returns whether the window was moved.
    pub fn hide_window(&mut self, window: Option<&W::Id>) -> bool {
        let id = match window {
            Some(id) => id.clone(),
            None => match self.focus() {
                Some(win) => win.id().clone(),
                None => return false,
            },
        };

        if let Some(InteractiveMoveState::Moving(move_)) = &self.interactive_move {
            if move_.tile.window().id() == &id {
                return false;
            }
        }

        let MonitorSet::Normal { monitors, .. } = &mut self.monitor_set else {
            return false;
        };

        let Some((mon_idx, ws_idx)) = monitors.iter().enumerate().find_map(|(mon_idx, mon)| {
            let ws_idx = mon.workspaces.iter().position(|ws| ws.has_window(&id))?;
            Some((mon_idx, ws_idx))
        }) else {
            return false;
        };
        if monitors[mon_idx].workspaces[ws_idx].is_hidden() {
            return false;
        }

        if let Some(hidden_idx) = monitors[mon_idx].first_hidden_workspace_idx() {
            monitors[mon_idx].move_to_workspace(Some(&id), hidden_idx, ActivateWindow::No);
            return true;
        }

        let Some((output, hidden_idx)) = monitors.iter().find_map(|mon| {
            let idx = mon.first_hidden_workspace_idx()?;
            Some((mon.output.clone(), idx))
        }) else {
            return false;
        };
        self.move_to_output(Some(&id), &output, Some(hidden_idx), ActivateWindow::No);
        true
    }

    /// Moves a hidden window (the most recently hidden one if `None`) back to the workspace it
    /// was hidden from, or to the focused workspace if that no longer exists, and focuses it.
    ///
    /// Returns the window that was unhidden.
    pub fn unhide_window(&mut self, window: Option<&W::Id>) -> Option<W::Id> {
        let hidden = self.hidden_windows();
        let entry = match window {
            Some(id) => hidden.into_iter().find(|win| &win.id == id)?,
            None => hidden.into_iter().next()?,
        };
        let id = entry.id;

        let MonitorSet::Normal {
            monitors,
            active_monitor_idx,
            ..
        } = &mut self.monitor_set
        else {
            return None;
        };

        let source_mon_idx = monitors.iter().position(|mon| mon.has_window(&id))?;

        // Target: the origin workspace if it still exists and is visible, else the focused one.
        let origin = entry.origin.and_then(|ws_id| {
            monitors.iter().enumerate().find_map(|(mon_idx, mon)| {
                let ws_idx = mon.idx_of_ws(ws_id)?;
                (!mon.workspaces[ws_idx].is_hidden()).then_some((mon_idx, ws_idx))
            })
        });
        let (target_mon_idx, target_ws_idx) = origin.unwrap_or_else(|| {
            let mon = &monitors[*active_monitor_idx];
            (*active_monitor_idx, mon.active_workspace_idx)
        });

        if target_mon_idx == source_mon_idx {
            monitors[source_mon_idx].move_to_workspace(
                Some(&id),
                target_ws_idx,
                ActivateWindow::Yes,
            );
        } else {
            let output = monitors[target_mon_idx].output.clone();
            self.move_to_output(Some(&id), &output, Some(target_ws_idx), ActivateWindow::Yes);
        }

        self.activate_window(&id);
        // Bring it into view on a canvas without moving the view more than needed.
        self.canvas_edge_push(&id);

        Some(id)
    }
}
