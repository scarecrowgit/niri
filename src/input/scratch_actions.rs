//! ashd: scratchpad actions (hidden workspaces, see `crate::layout::scratch`).

use niri_config::Action;
use smithay::desktop::layer_map_for_output;
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::shell::wlr_layer::Layer;

use crate::niri::{Niri, State};
use crate::window::mapped::MappedId;

impl Niri {
    /// Whether `pos` (global) is over a top or overlay layer surface whose layer rules say
    /// `hide-dropped-windows true` (the ashd scratchpad panel): a window released there at the
    /// end of an interactive move is hidden. Uses the surface's geometry (where it is drawn,
    /// sliding edge panels included), not its input region.
    pub fn hides_dropped_windows_at(&self, pos: Point<f64, Logical>) -> bool {
        let Some((output, pos_within_output)) = self.output_under(pos) else {
            return false;
        };
        let layers = layer_map_for_output(output);
        [Layer::Overlay, Layer::Top].into_iter().any(|layer| {
            layers.layers_on(layer).any(|surface| {
                let Some(mapped) = self.mapped_layer_surfaces.get(surface) else {
                    return false;
                };
                if !mapped.rules().hide_dropped_windows {
                    return false;
                }
                let Some(geo) = layers.layer_geometry(surface) else {
                    return false;
                };
                let mut loc = geo.loc.to_f64();
                if let Some(placement) = self.edge_reveal.placement(surface) {
                    loc += placement.offset;
                }
                Rectangle::new(loc, geo.size.to_f64()).contains(pos_within_output)
            })
        })
    }
}

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
