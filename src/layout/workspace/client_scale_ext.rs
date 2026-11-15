//! ashd: `Workspace` side of the client scale that follows the canvas zoom (see
//! [`crate::layout::client_scale`]).

use std::time::Duration;

use smithay::output;

use super::Workspace;
use crate::layout::client_scale::{
    is_unzoomed, quantize_zoom_within, same_scale, zoomed_client_scale, CLIENT_SCALE_SETTLE,
};
use crate::layout::LayoutElement;

impl<W: LayoutElement> Workspace<W> {
    /// Whether the canvas zoom is at rest: canvas mode, no zoom animation, no pinch, no mode
    /// switch. Panning does not count (it does not change the zoom).
    pub fn canvas_zoom_at_rest(&self) -> bool {
        self.is_canvas()
            && !self.canvas.view.is_zoom_animating()
            && self.canvas.zoom_gesture.is_none()
            && self.canvas.mode_switch.is_none()
    }

    /// Records since when the current zoom has been at rest. Called whenever the view changes.
    pub(super) fn track_canvas_zoom_rest(&mut self) {
        if !self.canvas_zoom_at_rest() {
            self.canvas.zoom_rest = None;
            return;
        }
        let zoom = self.canvas.view.current().zoom;
        match self.canvas.zoom_rest {
            Some((rest_zoom, _)) if rest_zoom == zoom => (),
            _ => self.canvas.zoom_rest = Some((zoom, self.clock.now_unadjusted())),
        }
    }

    /// The settled zoom, once it has been at rest for [`CLIENT_SCALE_SETTLE`].
    fn canvas_settled_client_zoom(&self) -> Option<f64> {
        let (zoom, since) = self.canvas.zoom_rest?;
        let now = self.clock.now_unadjusted();
        (now.saturating_sub(since) >= CLIENT_SCALE_SETTLE).then_some(zoom)
    }

    /// When the client scales of this workspace need another look (the zoom has been at rest,
    /// but not yet for [`CLIENT_SCALE_SETTLE`]).
    pub fn client_scale_deadline(&self) -> Option<Duration> {
        if !self.rescales_clients() {
            return None;
        }
        let (_, since) = self.canvas.zoom_rest?;
        let deadline = since + CLIENT_SCALE_SETTLE;
        (self.clock.now_unadjusted() < deadline).then_some(deadline)
    }

    fn rescales_clients(&self) -> bool {
        self.output.is_some() && self.is_canvas() && self.options.layout.canvas.rescale_clients
    }

    /// Settled zooms are kept on the grid that `wp_fractional_scale_v1` can express for this
    /// output, so that buffers of clients following the zoom match the drawn size exactly.
    pub(super) fn canvas_settle_zoom(&self, zoom: f64) -> f64 {
        let canvas = &self.options.layout.canvas;
        let zoom = canvas.clamp_zoom(zoom);
        if !canvas.rescale_clients || is_unzoomed(zoom) {
            return zoom;
        }
        quantize_zoom_within(
            zoom,
            self.scale.fractional_scale(),
            canvas.min_zoom(),
            canvas.max_zoom(),
        )
    }

    /// Scale a window of this workspace should have right now; `None`: leave it as it is (the
    /// zoom is not settled yet).
    fn client_scale_target(&self, window: &W) -> Option<output::Scale> {
        let base = self.scale;
        if !self.rescales_clients() || window.keeps_output_scale() {
            return Some(base);
        }
        let id = window.id();
        if !self.floating.has_window(id) {
            // Fullscreen / maximized windows of a canvas are drawn 1:1; a window that is being
            // added (in neither space yet) keeps what it has until it lands.
            let in_strip = self.scrolling.tiles().any(|tile| tile.window().id() == id);
            return in_strip.then_some(base);
        }
        let target = self.canvas.view.target().zoom;
        if is_unzoomed(target) {
            // Going back to 100%: re-render at full resolution right away, so the buffers are
            // there (and downscaled meanwhile) when the zoom animation arrives.
            return Some(base);
        }
        let zoom = self.canvas_settled_client_zoom()?;
        Some(zoomed_client_scale(base, zoom))
    }

    /// Scale for a window of (or being added to) this workspace.
    pub(super) fn current_client_scale(&self, window: &W) -> output::Scale {
        self.client_scale_target(window)
            .or_else(|| window.preferred_scale_transform().map(|(s, _)| s))
            // A new window got `new_canvas_window_scale()` in its initial configure.
            .unwrap_or_else(|| self.new_canvas_window_scale())
    }

    /// Scale for the initial configure of a new window that opens on this workspace's plane.
    pub(super) fn new_canvas_window_scale(&self) -> output::Scale {
        if !self.rescales_clients() || is_unzoomed(self.canvas.view.target().zoom) {
            return self.scale;
        }
        match self.canvas_settled_client_zoom() {
            Some(zoom) => zoomed_client_scale(self.scale, zoom),
            None => self.scale,
        }
    }

    /// Sends the zoomed (or the output) scale to the windows whose scale must change. Called
    /// every event loop iteration (`refresh`); does nothing for windows already at their scale.
    pub(super) fn sync_client_scales(&mut self) {
        if self.output.is_none() {
            return;
        }
        let transform = self.transform;
        for window in self.windows() {
            let Some(scale) = self.client_scale_target(window) else {
                continue;
            };
            let up_to_date = window
                .preferred_scale_transform()
                .is_some_and(|(s, t)| same_scale(s, scale) && t == transform);
            if !up_to_date {
                window.set_preferred_scale_transform(scale, transform);
            }
        }
    }
}
