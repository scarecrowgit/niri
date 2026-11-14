//! ashd: canvas plane grid and named regions of a workspace (see `crate::layout::canvas_grid`).

use niri_config::CanvasRegion;

use super::{Workspace, WorkspaceRenderElement};
use crate::layout::LayoutElement;
use crate::render_helpers::renderer::NiriRenderer;

impl<W: LayoutElement> Workspace<W> {
    /// Regions currently shown on this workspace's plane (config plus runtime changes).
    pub fn canvas_regions(&self) -> Vec<CanvasRegion> {
        self.canvas
            .decor
            .effective_regions(&self.options.layout.canvas.decor)
    }

    /// Adds (or replaces by name) a region on the plane.
    pub fn canvas_add_region(&mut self, region: CanvasRegion) {
        if !(region.x.is_finite()
            && region.y.is_finite()
            && region.w.is_finite()
            && region.h.is_finite())
            || region.w <= 0.
            || region.h <= 0.
        {
            return;
        }
        self.canvas.decor.add_region(region);
    }

    /// Removes a region by name.
    pub fn canvas_remove_region(&mut self, name: &str) {
        self.canvas
            .decor
            .remove_region(name, &self.options.layout.canvas.decor);
    }

    /// Recomputes the grid and region elements for the current canvas view.
    pub(super) fn canvas_update_decorations(&mut self) {
        let transform = self.canvas_transform();
        self.canvas.decor.update_render_elements(
            transform,
            self.view_size,
            self.scale.fractional_scale(),
            self.canvas.frame_zoom,
            &self.options.layout.canvas.decor,
            self.canvas.work_area.clip(),
        );
    }

    /// Renders the grid and regions (canvas mode only), below all windows.
    pub(super) fn render_canvas_decorations<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        push: &mut dyn FnMut(WorkspaceRenderElement<R>),
    ) {
        if !self.is_canvas() {
            return;
        }
        let scale = self.scale.fractional_scale();
        self.canvas
            .decor
            .render(renderer, scale, &mut |elem| push(elem.into()));
    }
}

#[cfg(test)]
impl<W: LayoutElement> Workspace<W> {
    pub fn canvas_visible_region_names(&self) -> Vec<String> {
        self.canvas.decor.visible_region_names()
    }
}
