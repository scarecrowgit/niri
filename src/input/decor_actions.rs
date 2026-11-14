//! ashd: actions for canvas plane decorations (named regions).

use niri_config::{Action, CanvasRegion};

use crate::niri::State;

impl State {
    pub(super) fn do_decor_action(&mut self, action: Action) {
        match action {
            Action::CanvasAddRegion(name, x, y, w, h) => {
                self.niri.layout.canvas_add_region(CanvasRegion {
                    name,
                    x: x.0,
                    y: y.0,
                    w: w.0,
                    h: h.0,
                });
            }
            Action::CanvasRemoveRegion(name) => {
                self.niri.layout.canvas_remove_region(&name);
            }
            _ => return,
        }
        // FIXME: granular.
        self.niri.queue_redraw_all();
    }
}
