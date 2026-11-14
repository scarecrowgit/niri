//! ashd: a workspace of its own for every fullscreen window (`layout { fullscreen-workspaces; }`).
//!
//! When a window becomes fullscreen (the `fullscreen-window` action, a client request, or a window
//! that maps fullscreen), it moves to a new workspace inserted right after the one it was on (its
//! *origin*), and the view follows it if it was the focused window. The new workspace carries a
//! [`FullscreenSlot`]:
//!
//! - it doesn't count for workspace numbers (`focus-workspace N`, `move-…-to-workspace N`, IPC
//!   `idx` 0), like the hidden scratchpad, but it is visible: rendered, reachable with workspace
//!   up/down and gestures, and reported over IPC (`Workspace::fullscreen_window_id`,
//!   `fullscreen_origin_id`) so the shell can show it as an app rather than a numbered desk;
//! - its origin is kept alive even when the fullscreen window was its only window, so the numbers
//!   of the other workspaces don't shift;
//! - when the window leaves fullscreen it goes back to its origin: into a column right of the
//!   column that was left of it (or first, if it was first; next to the focused column if that
//!   column is gone), or to its old plane position on a canvas; the view follows if the fullscreen
//!   workspace was shown. If the origin is gone, it goes to the nearest numbered workspace before
//!   (else after) the fullscreen workspace on the same monitor;
//! - when the window closes or is moved elsewhere the workspace is removed (after the switch
//!   animation if it was shown, which then goes to the origin); any other windows on it (dialogs)
//!   go to the origin first;
//! - windows opened while it is focused open on the origin workspace and take the focus (child
//!   windows of the fullscreen window open next to it, as usual);
//! - a fullscreen window moved by an action to another workspace stays fullscreen and gets a new
//!   workspace after that one.
//!
//! Only columns with a single window qualify (a fullscreen tabbed column stays where it is). The
//! state is reconciled by [`Layout::sync_fullscreen_workspaces`], called right after the
//! fullscreen actions, after adding windows and on every `Layout::refresh()`, so client-side
//! changes (unfullscreen requests, closing) are picked up in the same frame.

use smithay::utils::{Logical, Point};

use super::monitor::Monitor;
use super::workspace::WorkspaceId;
use super::{ActivateWindow, Layout, LayoutElement, MonitorSet};
use crate::layout::scrolling::ColumnId;

/// Marks a workspace created for one fullscreen window.
#[derive(Debug, Clone, PartialEq)]
pub struct FullscreenSlot<Id> {
    /// The fullscreen window; `None` once it has left (the workspace is removed as soon as it is
    /// no longer shown).
    pub window: Option<Id>,
    /// The workspace the window came from and returns to.
    pub origin: WorkspaceId,
    /// Where it returns to.
    pub ret: FullscreenReturn,
}

/// Where a window goes back to on its origin workspace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FullscreenReturn {
    /// A strip workspace: a new column right of `after` (`None`: the first column).
    Column { after: Option<ColumnId> },
    /// A canvas: this plane position (top-left of the tile), if known.
    Plane(Option<Point<f64, Logical>>),
    /// Next to the focused column.
    Auto,
}

impl<W: LayoutElement> Monitor<W> {
    /// Whether the edge panels of this output are (about to be) covered by a fullscreen window:
    /// the active workspace (the target of a running switch) shows one. Used by `edge-reveal`
    /// to slide the panels out together with the switch instead of after it.
    pub fn edge_panels_covered(&self) -> bool {
        !self.overview_open
            && self.workspaces[self.active_workspace_idx].fullscreen_covers_top_layer()
    }

    /// Whether `fullscreen-workspaces` is on for this monitor.
    pub(super) fn has_fullscreen_workspaces(&self) -> bool {
        self.options.layout.fullscreen_workspaces
    }

    /// Whether the workspace at `idx` is the origin of a fullscreen workspace whose window is
    /// still there.
    pub(super) fn is_fullscreen_origin(&self, idx: usize) -> bool {
        let Some(id) = self.workspaces.get(idx).map(|ws| ws.id()) else {
            return false;
        };
        self.workspaces.iter().any(|ws| {
            ws.fullscreen_slot()
                .is_some_and(|slot| slot.origin == id && slot.window.is_some())
        })
    }

    /// Removes fullscreen workspaces that are empty (their window left), unless shown.
    pub(super) fn remove_retired_fullscreen_workspaces(&mut self) {
        let last = self.workspaces.len() - 1;
        for idx in (0..self.workspaces.len()).rev() {
            let ws = &self.workspaces[idx];
            if ws.fullscreen_slot().is_none() || ws.has_windows() {
                continue;
            }
            if idx == last {
                // Can't happen in practice; the last workspace is a plain empty one.
                self.workspaces[idx].fullscreen_slot = None;
                continue;
            }
            if idx == self.active_workspace_idx {
                continue;
            }
            self.workspaces.remove(idx);
            if self.active_workspace_idx > idx {
                self.active_workspace_idx -= 1;
            }
        }
    }

    /// Index of the workspace a window leaving the fullscreen workspace at `idx` goes to.
    fn fullscreen_return_idx(&self, idx: usize) -> usize {
        let origin = self.workspaces[idx]
            .fullscreen_slot()
            .map(|slot| slot.origin);
        if let Some(origin_idx) = origin.and_then(|id| self.idx_of_ws(id)) {
            if !self.workspaces[origin_idx].is_hidden() {
                return origin_idx;
            }
        }
        let numbered = |i: &usize| self.workspaces[*i].is_numbered();
        (0..idx)
            .rev()
            .find(numbered)
            .or_else(|| (idx + 1..self.workspaces.len()).find(numbered))
            .unwrap_or(self.workspaces.len() - 1)
    }

    /// Moves the windows of the fullscreen workspace with `ws_id` other than `except` to
    /// `target_id`, without focusing them.
    fn fullscreen_move_leftovers(&mut self, ws_id: WorkspaceId, target_id: WorkspaceId) {
        loop {
            let Some(idx) = self.idx_of_ws(ws_id) else {
                return;
            };
            let Some(window) = self.workspaces[idx]
                .windows()
                .next()
                .map(|w| w.id().clone())
            else {
                return;
            };
            let Some(target_idx) = self.idx_of_ws(target_id) else {
                return;
            };
            self.move_to_workspace(Some(&window), target_idx, ActivateWindow::No);
        }
    }

    /// Reconciles the fullscreen workspaces of this monitor with the fullscreen state of its
    /// windows.
    pub(super) fn sync_fullscreen_workspaces(&mut self) {
        if !self.has_fullscreen_workspaces() {
            if self
                .workspaces
                .iter()
                .any(|ws| ws.fullscreen_slot().is_some())
            {
                for ws in &mut self.workspaces {
                    ws.fullscreen_slot = None;
                }
                if self.workspace_switch.is_none() {
                    self.clean_up_workspaces();
                }
            }
            return;
        }

        // Fullscreen workspaces whose window left fullscreen or left the workspace.
        let slots: Vec<WorkspaceId> = self
            .workspaces
            .iter()
            .filter(|ws| {
                ws.fullscreen_slot()
                    .is_some_and(|slot| slot.window.is_some())
            })
            .map(|ws| ws.id())
            .collect();
        for ws_id in slots {
            let Some(idx) = self.idx_of_ws(ws_id) else {
                continue;
            };
            let ws = &self.workspaces[idx];
            let slot = ws.fullscreen_slot().unwrap().clone();
            let window = slot.window.clone().unwrap();
            let present = ws.has_window(&window);
            if present && ws.is_layout_fullscreen(&window) {
                continue;
            }

            let was_active = self.active_workspace_idx == idx;
            let target_idx = self.fullscreen_return_idx(idx);
            let target_id = self.workspaces[target_idx].id();
            self.workspaces[idx]
                .fullscreen_slot
                .as_mut()
                .unwrap()
                .window = None;

            if present {
                // Back to the origin; the view follows if the fullscreen workspace was shown.
                let activate = if was_active {
                    ActivateWindow::Yes
                } else {
                    ActivateWindow::No
                };
                self.move_to_workspace_placed(Some(&window), target_idx, activate, Some(&slot.ret));
            }

            self.fullscreen_move_leftovers(ws_id, target_id);

            if let (Some(idx), Some(target_idx)) =
                (self.idx_of_ws(ws_id), self.idx_of_ws(target_id))
            {
                if self.active_workspace_idx == idx {
                    self.activate_workspace(target_idx);
                }
            }
        }

        // Fullscreen windows that are not on a workspace of their own.
        let candidates: Vec<(WorkspaceId, W::Id)> = self
            .workspaces
            .iter()
            .flat_map(|ws| {
                let id = ws.id();
                ws.fullscreen_windows_to_dedicate()
                    .into_iter()
                    .map(move |win| (id, win))
            })
            .collect();
        for (ws_id, window) in candidates {
            let Some(src_idx) = self.idx_of_ws(ws_id) else {
                continue;
            };
            let src = &self.workspaces[src_idx];
            if !src.has_window(&window) || !src.is_layout_fullscreen(&window) {
                continue;
            }

            // A fullscreen window landing on another fullscreen workspace belongs to that one's
            // origin.
            let origin = src.fullscreen_slot().map_or(ws_id, |slot| slot.origin);
            let ret = src.fullscreen_return_pos(&window);
            let activate = self.active_workspace_idx == src_idx
                && self.active_window().is_some_and(|win| win.id() == &window);

            let new_idx = src_idx + 1;
            self.add_workspace_at(new_idx);
            let ws = &mut self.workspaces[new_idx];
            if ws.is_canvas() {
                ws.set_mode(niri_config::WorkspaceMode::Strip);
            }
            ws.fullscreen_slot = Some(FullscreenSlot {
                window: Some(window.clone()),
                origin,
                ret,
            });

            let activate = if activate {
                ActivateWindow::Yes
            } else {
                ActivateWindow::No
            };
            // Adding a tile resets whether it unfullscreens into floating; keep it, except for
            // canvas windows: they leave fullscreen as a column of the fullscreen workspace (one
            // resize animation) and land on the plane when they move back, instead of first
            // jumping to a strip floating position.
            let to_floating = self.workspaces[src_idx].fullscreen_restores_to_floating(&window)
                && !self.workspaces[src_idx].is_canvas();
            self.move_to_workspace(Some(&window), new_idx, activate);
            if let Some(idx) = self.workspaces.iter().position(|ws| ws.has_window(&window)) {
                self.workspaces[idx].fullscreen_set_restore_to_floating(&window, to_floating);
            }
        }

        if self.workspace_switch.is_none() {
            self.clean_up_workspaces();
        }
    }

    /// Where a new window opened while a fullscreen workspace is focused goes: its origin.
    pub(super) fn fullscreen_redirect_new_window(&self) -> Option<WorkspaceId> {
        if !self.has_fullscreen_workspaces() {
            return None;
        }
        let ws = &self.workspaces[self.active_workspace_idx];
        let slot = ws.fullscreen_slot()?;
        slot.window.as_ref()?;
        self.idx_of_ws(slot.origin)?;
        Some(slot.origin)
    }

    #[cfg(test)]
    pub(super) fn verify_fullscreen_invariants(&self) {
        let mut seen = Vec::new();
        for ws in &self.workspaces {
            ws.fullscreen_verify_invariants();
            if let Some(window) = ws.fullscreen_slot().and_then(|slot| slot.window.as_ref()) {
                assert!(
                    !seen.contains(&window),
                    "a window has at most one fullscreen workspace"
                );
                seen.push(window);
            }
        }
    }
}

impl<W: LayoutElement> Layout<W> {
    /// Reconciles the fullscreen workspaces of all monitors (see the module docs).
    pub fn sync_fullscreen_workspaces(&mut self) {
        if self.interactive_move.is_some() {
            return;
        }
        if let MonitorSet::Normal { monitors, .. } = &mut self.monitor_set {
            for mon in monitors {
                mon.sync_fullscreen_workspaces();
            }
        }
    }
}
