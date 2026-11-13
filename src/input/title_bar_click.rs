//! rice: pointer presses on compositor-drawn title bars.
//!
//! - Press: activates the window and starts a move grab: dragging moves the window like Mod+drag,
//!   releasing without a drag just activates it.
//! - Double-click: in strip mode toggles the full width of the column (`maximize-column`); on a
//!   canvas zooms to 100% centred on the window, or fits all windows when already at 100%.
//! - Other buttons do nothing for now.

use smithay::input::pointer::{CursorIcon, Focus, GrabStartData as PointerGrabStartData};
use smithay::utils::{Logical, Point, Serial};

use super::move_grab::MoveGrab;
use super::{AnyStartData, DOUBLE_CLICK_TIME};
use crate::layout::HitType;
use crate::niri::State;
use crate::utils::get_monotonic_time;

impl State {
    /// Handles a left button press at `location`; returns whether it was on a title bar.
    pub(super) fn title_bar_press(
        &mut self,
        button_code: u32,
        serial: Serial,
        location: Point<f64, Logical>,
    ) -> bool {
        let Some((window, HitType::TitleBar)) = self.niri.contents_under(location).window else {
            return false;
        };

        // Double-click detection, per window.
        let time = get_monotonic_time();
        let is_double = {
            let Some((_, mapped)) = self.niri.layout.windows().find(|(_, m)| m.window == window)
            else {
                return false;
            };
            let cell = mapped.last_title_bar_click();
            let is_double = cell
                .get()
                .is_some_and(|last| time.saturating_sub(last) <= DOUBLE_CLICK_TIME);
            // A third click starts over.
            cell.set((!is_double).then_some(time));
            is_double
        };

        self.niri.layout.activate_window(&window);

        if is_double {
            let canvas_zoom = self
                .niri
                .layout
                .workspaces()
                .find(|(_, _, ws)| ws.windows().any(|w| w.window == window))
                .and_then(|(_, _, ws)| ws.canvas_target_transform())
                .map(|t| t.zoom);
            match canvas_zoom {
                Some(zoom) if zoom < 1. => {
                    self.niri.layout.canvas_set_zoom(1., None);
                    self.niri.layout.canvas_fly_to_window(Some(&window));
                }
                Some(_) => self.niri.layout.canvas_fit(),
                None => self.niri.layout.toggle_full_width(),
            }
            self.niri.queue_redraw_all();
            return true;
        }

        let pointer = self.niri.seat.get_pointer().unwrap();
        let start_data = PointerGrabStartData {
            focus: None,
            button: button_code,
            location,
        };
        let start_data = AnyStartData::Pointer(start_data);
        if let Some(grab) = MoveGrab::new(
            self,
            start_data,
            window.clone(),
            false,
            Some(CursorIcon::Grabbing),
        ) {
            pointer.set_grab(self, grab, serial, Focus::Clear);
        }
        self.niri.queue_redraw_all();
        true
    }
}
