//! rice: scratchpad actions (hidden workspaces, see `crate::layout::scratch`).

use niri_config::Action;

use crate::niri::State;
use crate::window::mapped::MappedId;

impl State {
    /// Executes a scratchpad action. Returns `false` for other actions.
    pub fn do_scratch_action(&mut self, action: Action) -> bool {
        let find = |state: &State, id: u64| state.niri.find_window_by_id(MappedId::specific(id));

        match action {
            Action::HideWindow => {
                self.niri.layout.hide_window(None);
            }
            Action::HideWindowById(id) => {
                if let Some(window) = find(self, id) {
                    self.niri.layout.hide_window(Some(&window));
                }
            }
            Action::UnhideWindow | Action::UnhideWindowById(_) => {
                let window = match action {
                    Action::UnhideWindowById(id) => match find(self, id) {
                        Some(window) => Some(window),
                        None => return true,
                    },
                    _ => None,
                };
                if let Some(window) = self.niri.layout.unhide_window(window.as_ref()) {
                    // Same as focusing by id: activates, reveals on a canvas, warps the cursor.
                    self.focus_window(&window);
                }
            }
            _ => return false,
        }

        // FIXME: granular
        self.niri.queue_redraw_all();
        true
    }
}
