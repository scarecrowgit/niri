//! rice: canvas actions and the keyboard-action hooks that keep the canvas view on the focus.
//!
//! - [`State::do_canvas_action`] executes the canvas actions themselves.
//! - [`CanvasActionHook`] wraps every action in `do_action()`: keyboard-driven focus changes
//!   recentre a canvas view on the newly focused window ("reveal"), and keyboard window moves push
//!   the view along when the window leaves the viewport ("edge-push"). Pointer-driven focus never
//!   goes through `do_action()`, so it does not recentre.

use niri_config::Action;
use smithay::desktop::Window;
use smithay::output::Output;
use smithay::utils::{Logical, Point};
use smithay::wayland::shell::wlr_layer::Layer;

use crate::input::modifiers_from_state;
use crate::layout::canvas::CanvasPanDirection;
use crate::layout::workspace::WorkspaceId;
use crate::niri::{Niri, State};
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
    /// Starts a canvas pinch zoom if a touchpad pinch begins over a canvas workspace: with 3 or
    /// more fingers, with the Mod key held, or over the empty plane. Two-finger pinches over
    /// windows go to the window as usual.
    pub fn canvas_pinch_begin(&mut self, fingers: u32) -> bool {
        if self.niri.is_locked()
            || self.niri.layout.is_overview_open()
            || self.niri.screenshot_ui.is_open()
            || self.niri.window_mru_ui.is_open()
            || self.niri.pointer_over_top_layer()
        {
            return false;
        }
        let Some((_, ws_id, _)) = self.niri.canvas_under_cursor() else {
            return false;
        };

        let mod_key = self.backend.mod_key(&self.niri.config.borrow());
        let mods = self.niri.seat.get_keyboard().unwrap().modifier_state();
        let mod_down = modifiers_from_state(mods).contains(mod_key.to_modifiers());
        let over_window = self.niri.pointer_contents.window.is_some();
        if !(fingers >= 3 || mod_down || !over_window) {
            return false;
        }

        self.niri.layout.canvas_zoom_gesture_begin(ws_id)
    }

    /// Updates a canvas pinch zoom; `scale` is relative to the start of the pinch. Returns
    /// whether a canvas pinch is ongoing.
    pub fn canvas_pinch_update(&mut self, scale: f64) -> bool {
        let sensitivity = self
            .niri
            .config
            .borrow()
            .input
            .touchpad
            .pinch_sensitivity
            .map_or(1., |x| x.0);
        let anchor = self.niri.canvas_under_cursor().map(|(_, _, anchor)| anchor);
        let res = self
            .niri
            .layout
            .canvas_zoom_gesture_update(scale.powf(sensitivity), anchor);
        match res {
            Some(output) => {
                if let Some(output) = output {
                    self.niri.queue_redraw(&output);
                }
                true
            }
            None => false,
        }
    }

    /// Zooms by one step: with `at_cursor`, the canvas workspace under the cursor around the
    /// cursor (the plane point under it stays put); otherwise (or when the cursor is not over a
    /// canvas) the focused workspace around its working area centre.
    fn canvas_zoom_step(&mut self, zoom_in: bool, at_cursor: bool) {
        let under = at_cursor.then(|| self.niri.canvas_under_cursor()).flatten();
        let layout = &mut self.niri.layout;
        match under {
            Some((_, ws_id, anchor)) => layout.canvas_zoom_step_at(ws_id, zoom_in, Some(anchor)),
            None if zoom_in => layout.canvas_zoom_in(None),
            None => layout.canvas_zoom_out(None),
        }
    }

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
            Action::CanvasZoomIn(at_cursor) => self.canvas_zoom_step(true, at_cursor),
            Action::CanvasZoomOut(at_cursor) => self.canvas_zoom_step(false, at_cursor),
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

impl Niri {
    /// Canvas workspace under the cursor, with the cursor position in its view coordinates (the
    /// anchor for zooming at the cursor).
    pub fn canvas_under_cursor(&self) -> Option<(Output, WorkspaceId, Point<f64, Logical>)> {
        let pos = self.seat.get_pointer()?.current_location();
        let (output, pos_within_output) = self.output_under(pos)?;
        let (ws_id, anchor) = self
            .layout
            .canvas_workspace_under(output, pos_within_output)?;
        Some((output.clone(), ws_id, anchor))
    }

    /// Whether the pointer is over a top or overlay layer surface (bars, launchers), as opposed
    /// to the workspace or a background/bottom layer surface (wallpaper) behind it.
    pub fn pointer_over_top_layer(&self) -> bool {
        self.pointer_contents
            .layer
            .as_ref()
            .is_some_and(|layer| matches!(layer.layer(), Layer::Top | Layer::Overlay))
    }
}
