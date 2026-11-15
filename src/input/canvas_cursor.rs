//! ashd: pointer cursors on canvas workspaces.
//!
//! The rule (one place, [`canvas_cursor`]):
//!
//! - while a canvas pan is in progress (Mod+LMB / Mod+MMB drag, touchpad pan): `grabbing`, wherever
//!   the pointer is;
//! - otherwise only over *empty* canvas space, i.e. a canvas workspace with no window (surface,
//!   border, title strip) and no top/overlay layer surface under the pointer (background and bottom
//!   layers count as empty, like for Mod+LMB panning):
//!   - Mod held alone (the Mod+scroll zoom modifier, when wheel zoom is enabled): `zoom-out` while
//!     that canvas is animating its zoom downwards (a zoom-out step or wheel zoom in progress),
//!     else `zoom-in`;
//!   - no modifier: `grab` (the plane can be dragged with Mod+LMB / Mod+MMB);
//! - everywhere else nothing is overridden: over windows the client's cursor wins, other grabs
//!   (interactive move/resize, …) set their own, and leaving the canvas, a client surface coming
//!   under the pointer, releasing Mod or ending the pan restores the regular cursor on the next
//!   event loop iteration.
//!
//! The override lives in `CursorManager` and replaces the regular cursor image only while it is
//! set; it never touches the client's cursor state, so nothing needs restoring.

use smithay::input::pointer::CursorIcon;
use smithay::wayland::shell::wlr_layer::Layer;

use super::modifiers_from_state;
use crate::niri::State;

/// What the canvas cursor depends on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CanvasCursorInput {
    /// A canvas pan (pointer or touchpad) is in progress.
    pub panning: bool,
    /// Some other pointer grab is active (it sets its own cursor).
    pub other_grab: bool,
    /// Overlays that own the pointer (overview, screenshot UI, lock screen, switcher, …), or a
    /// hidden pointer.
    pub blocked: bool,
    /// The pointer is over a canvas workspace.
    pub over_canvas: bool,
    /// A window (surface, border, title strip) or a top/overlay layer surface is under the
    /// pointer.
    pub over_something: bool,
    /// Exactly the Mod modifier is held and Mod+scroll zooms.
    pub zoom_modifier: bool,
    /// The canvas under the pointer is animating its zoom downwards.
    pub zooming_out: bool,
}

/// The cursor to show instead of the regular one, if any (see the module docs).
pub fn canvas_cursor(input: CanvasCursorInput) -> Option<CursorIcon> {
    if input.blocked {
        return None;
    }
    if input.panning {
        return Some(CursorIcon::Grabbing);
    }
    if input.other_grab || !input.over_canvas || input.over_something {
        return None;
    }
    if input.zoom_modifier {
        if input.zooming_out {
            Some(CursorIcon::ZoomOut)
        } else {
            Some(CursorIcon::ZoomIn)
        }
    } else {
        Some(CursorIcon::Grab)
    }
}

impl State {
    /// Recomputes the canvas cursor override; called every event loop iteration (after the
    /// pointer contents are refreshed).
    pub fn refresh_canvas_cursor(&mut self) {
        let input = self.canvas_cursor_input();
        let icon = canvas_cursor(input);
        if self.niri.cursor_manager.set_override(icon) {
            // FIXME: redraw only the output under the pointer.
            self.niri.queue_redraw_all();
        }
    }

    fn canvas_cursor_input(&self) -> CanvasCursorInput {
        let niri = &self.niri;
        let Some(pointer) = niri.seat.get_pointer() else {
            return CanvasCursorInput {
                blocked: true,
                ..Default::default()
            };
        };
        let panning = niri.layout.has_canvas_pan_gesture();
        let blocked = !niri.pointer_visibility.is_visible()
            || niri.is_locked()
            || niri.screenshot_ui.is_open()
            || niri.exit_confirm_dialog.is_open()
            || niri.window_mru_ui.is_open()
            || niri.layout.is_overview_open()
            || niri.pointer_contents.overview
            || niri.pointer_contents.hot_corner
            || niri.tablet_cursor_location.is_some();

        let canvas = niri.canvas_under_cursor();
        let contents = &niri.pointer_contents;
        let over_layer = contents
            .layer
            .as_ref()
            .is_some_and(|layer| matches!(layer.layer(), Layer::Top | Layer::Overlay));
        let over_something = contents.window.is_some() || over_layer;

        let config = niri.config.borrow();
        let mod_key = self.backend.mod_key(&config);
        let mods = niri
            .seat
            .get_keyboard()
            .map(|kb| modifiers_from_state(kb.modifier_state()));
        let zoom_modifier =
            mods == Some(mod_key.to_modifiers()) && config.layout.canvas.wheel_zoom_enabled();
        drop(config);

        let zooming_out = canvas
            .as_ref()
            .and_then(|(_, ws_id, _)| niri.layout.canvas_zoom_direction(*ws_id))
            .is_some_and(|dir| dir < 0.);

        CanvasCursorInput {
            panning,
            other_grab: pointer.is_grabbed() && !panning,
            blocked,
            over_canvas: canvas.is_some(),
            over_something,
            zoom_modifier,
            zooming_out,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_canvas() -> CanvasCursorInput {
        CanvasCursorInput {
            over_canvas: true,
            ..Default::default()
        }
    }

    #[test]
    fn grab_over_empty_canvas() {
        assert_eq!(canvas_cursor(empty_canvas()), Some(CursorIcon::Grab));
    }

    #[test]
    fn zoom_cursors_with_mod() {
        let input = CanvasCursorInput {
            zoom_modifier: true,
            ..empty_canvas()
        };
        assert_eq!(canvas_cursor(input), Some(CursorIcon::ZoomIn));
        let input = CanvasCursorInput {
            zooming_out: true,
            ..input
        };
        assert_eq!(canvas_cursor(input), Some(CursorIcon::ZoomOut));
        // Zooming out without Mod (a key bind): plain grab.
        let input = CanvasCursorInput {
            zooming_out: true,
            ..empty_canvas()
        };
        assert_eq!(canvas_cursor(input), Some(CursorIcon::Grab));
    }

    #[test]
    fn grabbing_while_panning_anywhere() {
        for input in [
            empty_canvas(),
            CanvasCursorInput {
                over_something: true,
                ..empty_canvas()
            },
            CanvasCursorInput {
                over_canvas: false,
                ..empty_canvas()
            },
            CanvasCursorInput {
                zoom_modifier: true,
                ..empty_canvas()
            },
        ] {
            let input = CanvasCursorInput {
                panning: true,
                ..input
            };
            assert_eq!(canvas_cursor(input), Some(CursorIcon::Grabbing));
        }
    }

    #[test]
    fn client_and_other_cursors_win() {
        // A window or top layer surface under the pointer: the client's cursor.
        let input = CanvasCursorInput {
            over_something: true,
            zoom_modifier: true,
            ..empty_canvas()
        };
        assert_eq!(canvas_cursor(input), None);
        // Strip workspaces and other outputs.
        let input = CanvasCursorInput {
            over_canvas: false,
            ..empty_canvas()
        };
        assert_eq!(canvas_cursor(input), None);
        // Other grabs (interactive move/resize) set their own cursor.
        let input = CanvasCursorInput {
            other_grab: true,
            ..empty_canvas()
        };
        assert_eq!(canvas_cursor(input), None);
        // Overview, screenshot UI, lock screen, hidden pointer: never, even while panning.
        let input = CanvasCursorInput {
            blocked: true,
            panning: true,
            ..empty_canvas()
        };
        assert_eq!(canvas_cursor(input), None);
    }
}
