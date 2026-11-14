//! ashd: explicit window-focus actions take the keyboard back from an on-demand layer surface.
//!
//! A layer surface with on-demand keyboard interactivity keeps the keyboard focus after it was
//! clicked (`Niri::layer_shell_on_demand_focus`) until something else is clicked or an action
//! moves the focus. Upstream clears it in the directional focus and workspace focus actions; this
//! hook extends that to every action that explicitly focuses a window or switches the active
//! workspace:
//!
//! - `focus-floating`, `focus-tiling`, `switch-focus-between-floating-and-tiling`;
//! - any action after which the focused workspace is a different one (e.g.
//!   `move-window-to-workspace` / `move-column-to-workspace` with focus, `move-*-to-monitor`).
//!
//! `focus-window`, `focus-window-previous`, `unhide-window` and the recent-windows switcher go
//! through `State::focus_window()`, which clears it directly. Pointer motion without
//! focus-follows-mouse never goes through here and leaves the layer focus alone.

use niri_config::Action;

use crate::layout::workspace::WorkspaceId;
use crate::niri::State;

pub struct LayerFocusHook {
    /// The action explicitly focuses a window.
    focuses_window: bool,
    /// Focused workspace before the action.
    workspace_before: Option<WorkspaceId>,
}

impl LayerFocusHook {
    pub fn new(action: &Action, state: &State) -> Self {
        let focuses_window = matches!(
            action,
            Action::FocusFloating
                | Action::FocusTiling
                | Action::SwitchFocusBetweenFloatingAndTiling
        );
        Self {
            focuses_window,
            workspace_before: focused_workspace(state),
        }
    }

    pub fn finish(self, state: &mut State) {
        if state.niri.layer_shell_on_demand_focus.is_none() {
            return;
        }
        if self.focuses_window || focused_workspace(state) != self.workspace_before {
            state.niri.layer_shell_on_demand_focus = None;
            // FIXME: granular
            state.niri.queue_redraw_all();
        }
    }
}

fn focused_workspace(state: &State) -> Option<WorkspaceId> {
    state.niri.layout.active_workspace().map(|ws| ws.id())
}
