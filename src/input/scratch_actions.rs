//! ashd: scratchpad actions (hidden workspaces, see `crate::layout::scratch`).

use niri_config::Action;
use smithay::desktop::layer_map_for_output;
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::shell::wlr_layer::Layer;

use crate::niri::{Niri, State};
use crate::render_helpers::solid_color::SolidColorBuffer;
use crate::window::mapped::MappedId;

impl Niri {
    /// Whether `pos` (global) is over a top or overlay layer surface whose layer rules say
    /// `hide-dropped-windows true` (the ashd scratchpad panel): a window released there at the
    /// end of an interactive move is hidden. Uses the surface's geometry (where it is drawn,
    /// sliding edge panels included), not its input region.
    pub fn hides_dropped_windows_at(&self, pos: Point<f64, Logical>) -> bool {
        self.dropped_windows_target_at(pos).is_some()
    }

    /// The `hide-dropped-windows` surface under `pos` (global): its output and its drawn
    /// rectangle in output-local logical coordinates.
    pub fn dropped_windows_target_at(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(Output, Rectangle<f64, Logical>)> {
        let (output, pos_within_output) = self.output_under(pos)?;
        let layers = layer_map_for_output(output);
        let rect = [Layer::Overlay, Layer::Top].into_iter().find_map(|layer| {
            layers.layers_on(layer).find_map(|surface| {
                let mapped = self.mapped_layer_surfaces.get(surface)?;
                if !mapped.rules().hide_dropped_windows {
                    return None;
                }
                let geo = layers.layer_geometry(surface)?;
                let mut loc = geo.loc.to_f64();
                if let Some(placement) = self.edge_reveal.placement(surface) {
                    loc += placement.offset;
                }
                let rect = Rectangle::new(loc, geo.size.to_f64());
                rect.contains(pos_within_output).then_some(rect)
            })
        })?;
        Some((output.clone(), rect))
    }

    /// While a window is moved over a `hide-dropped-windows` surface, that surface is tinted
    /// with the insert-hint colour (the moved window itself is drawn under the top and
    /// overlay layers, so it cannot be seen there): releasing hides the window.
    pub fn update_drop_hint(&mut self, output: Option<&Output>) {
        let pointer = self.seat.get_pointer().unwrap().current_location();
        let hint = self.config.borrow().layout.insert_hint;
        let target = (!hint.off)
            .then(|| self.dropped_windows_target_at(pointer))
            .flatten()
            .filter(|(o, _)| self.layout.interactive_move_is_moving_above_output(o))
            .filter(|(o, _)| output.is_none_or(|out| out == o));
        match target {
            Some((o, rect)) => {
                let color = hint.color.to_array_premul();
                match &mut self.drop_hint {
                    Some(h) => {
                        h.buffer.update(rect.size, color);
                        h.output = o;
                        h.loc = rect.loc;
                    }
                    None => {
                        self.drop_hint = Some(DropHint {
                            output: o,
                            buffer: SolidColorBuffer::new(rect.size, color),
                            loc: rect.loc,
                        })
                    }
                }
            }
            None => {
                if self
                    .drop_hint
                    .as_ref()
                    .is_some_and(|h| output.is_none_or(|out| *out == h.output))
                {
                    self.drop_hint = None;
                }
            }
        }
    }
}

/// The tint over a `hide-dropped-windows` surface while a window is moved over it.
#[derive(Debug)]
pub struct DropHint {
    pub output: Output,
    pub buffer: SolidColorBuffer,
    /// Output-local logical position.
    pub loc: Point<f64, Logical>,
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
