//! rice: canvas actions and the keyboard-action hooks that keep the canvas view on the focus.
//!
//! - [`State::do_canvas_action`] executes the canvas actions themselves.
//! - [`CanvasActionHook`] wraps every action in `do_action()`: keyboard-driven focus changes
//!   recentre a canvas view on the newly focused window ("reveal"), and keyboard window moves push
//!   the view along when the window leaves the viewport ("edge-push"). Pointer-driven focus never
//!   goes through `do_action()`, so it does not recentre.

use niri_config::Action;
use smithay::desktop::Window;
use smithay::utils::Point;

use crate::layout::canvas::CanvasPanDirection;
use crate::niri::State;
use crate::window::mapped::MappedId;

/// What to do after an action ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookKind {
    /// Recentre on the focused window if the focus changed.
    Reveal,
    /// Keep the moved window inside the viewport.
    EdgePush,
}

/// Captures the focus before a keyboard action and applies canvas reveal/edge-push after it.
pub struct CanvasActionHook {
    kind: Option<HookKind>,
    /// Window the action targets explicitly (by id), if any.
    target: Option<u64>,
    focus_before: Option<Window>,
}

impl CanvasActionHook {
    pub fn new(action: &Action, state: &State) -> Self {
        let (kind, target) = match action {
            Action::FocusColumnLeft
            | Action::FocusColumnLeftUnderMouse
            | Action::FocusColumnRight
            | Action::FocusColumnRightUnderMouse
            | Action::FocusColumnFirst
            | Action::FocusColumnLast
            | Action::FocusColumnRightOrFirst
            | Action::FocusColumnLeftOrLast
            | Action::FocusColumn(_)
            | Action::FocusWindowOrMonitorUp
            | Action::FocusWindowOrMonitorDown
            | Action::FocusColumnOrMonitorLeft
            | Action::FocusColumnOrMonitorRight
            | Action::FocusWindowDown
            | Action::FocusWindowUp
            | Action::FocusWindowDownOrColumnLeft
            | Action::FocusWindowDownOrColumnRight
            | Action::FocusWindowUpOrColumnLeft
            | Action::FocusWindowUpOrColumnRight
            | Action::FocusWindowOrWorkspaceDown
            | Action::FocusWindowOrWorkspaceUp
            | Action::FocusWindowInColumn(_)
            | Action::FocusWindowTop
            | Action::FocusWindowBottom
            | Action::FocusWindowDownOrTop
            | Action::FocusWindowUpOrBottom
            | Action::FocusFloating
            | Action::FocusTiling
            | Action::SwitchFocusBetweenFloatingAndTiling => (Some(HookKind::Reveal), None),
            // FocusWindow and FocusWindowPrevious reveal through State::focus_window().
            Action::MoveColumnLeft
            | Action::MoveColumnRight
            | Action::MoveWindowDown
            | Action::MoveWindowUp
            | Action::MoveWindowDownOrToWorkspaceDown
            | Action::MoveWindowUpOrToWorkspaceUp
            | Action::MoveColumnLeftOrToMonitorLeft
            | Action::MoveColumnRightOrToMonitorRight => (Some(HookKind::EdgePush), None),
            Action::MoveFloatingWindowById { id, .. } => (Some(HookKind::EdgePush), *id),
            _ => (None, None),
        };

        let focus_before = kind
            .is_some()
            .then(|| state.niri.layout.focus().map(|win| win.window.clone()))
            .flatten();

        Self {
            kind,
            target,
            focus_before,
        }
    }

    pub fn finish(self, state: &mut State) {
        let Some(kind) = self.kind else {
            return;
        };

        let layout = &mut state.niri.layout;
        match kind {
            HookKind::Reveal => {
                let Some(focus) = layout.focus().map(|win| win.window.clone()) else {
                    return;
                };
                if Some(&focus) == self.focus_before.as_ref() {
                    return;
                }
                layout.canvas_reveal_window(&focus);
            }
            HookKind::EdgePush => {
                let window = match self.target {
                    Some(id) => state.niri.find_window_by_id(MappedId::specific(id)),
                    None => layout.focus().map(|win| win.window.clone()),
                };
                let Some(window) = window else {
                    return;
                };
                state.niri.layout.canvas_edge_push(&window);
            }
        }
        state.niri.queue_redraw_all();
    }
}

impl State {
    /// Executes a canvas action. Returns `false` for non-canvas actions.
    pub fn do_canvas_action(&mut self, action: Action) -> bool {
        let find = |state: &State, id: u64| state.niri.find_window_by_id(MappedId::specific(id));

        match action {
            Action::ToggleWorkspaceMode => self.niri.layout.toggle_workspace_mode(),
            Action::SetWorkspaceMode(mode) => self.niri.layout.set_workspace_mode(mode),
            Action::CanvasPanLeft => self.niri.layout.canvas_pan(CanvasPanDirection::Left),
            Action::CanvasPanRight => self.niri.layout.canvas_pan(CanvasPanDirection::Right),
            Action::CanvasPanUp => self.niri.layout.canvas_pan(CanvasPanDirection::Up),
            Action::CanvasPanDown => self.niri.layout.canvas_pan(CanvasPanDirection::Down),
            Action::CanvasPanTo(x, y) => self.niri.layout.canvas_pan_to(Point::from((x.0, y.0))),
            Action::CanvasZoomIn => self.niri.layout.canvas_zoom_in(None),
            Action::CanvasZoomOut => self.niri.layout.canvas_zoom_out(None),
            Action::CanvasSetZoom(zoom) => self.niri.layout.canvas_set_zoom(zoom.0, None),
            Action::CanvasFit => self.niri.layout.canvas_fit(),
            Action::CanvasHome => self.niri.layout.canvas_home(),
            Action::CanvasFlyToWindow => self.niri.layout.canvas_fly_to_window(None),
            Action::CanvasFlyToWindowById(id) => {
                if let Some(window) = find(self, id) {
                    self.niri.layout.canvas_fly_to_window(Some(&window));
                }
            }
            Action::ToggleWindowPinned => self.niri.layout.toggle_window_pinned(None),
            Action::ToggleWindowPinnedById(id) => {
                if let Some(window) = find(self, id) {
                    self.niri.layout.toggle_window_pinned(Some(&window));
                }
            }
            _ => return false,
        }

        // FIXME: granular
        self.niri.queue_redraw_all();
        true
    }
}
