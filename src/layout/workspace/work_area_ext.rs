//! ashd: working area outline and clip of a workspace (see `crate::layout::work_area`).

use smithay::utils::{Logical, Rectangle};

use super::{Workspace, WorkspaceRenderElement};
use crate::layout::LayoutElement;
use crate::render_helpers::renderer::NiriRenderer;

impl<W: LayoutElement> Workspace<W> {
    /// Whether a fullscreen (or maximized-to-edges) tile is in view, which turns the working
    /// area outline and clip off.
    fn work_area_fullscreen_in_view(&self) -> bool {
        if self.fullscreen_window_above_top_layer().is_some() {
            return true;
        }
        let view = Rectangle::from_size(self.view_size);
        self.scrolling
            .tiles_with_render_positions()
            .any(|(tile, pos, _)| {
                let mode = tile.sizing_mode();
                (mode.is_fullscreen() || mode.is_maximized())
                    && Rectangle::new(pos, tile.tile_size()).overlaps(view)
            })
    }

    /// Recomputes the working area outline, masks and clip.
    pub(super) fn update_work_area_decor(&mut self) {
        let config = self.options.layout.work_area;
        let active = config.is_on() && !self.work_area_fullscreen_in_view();
        let mask_color = self.options.layout.background_color;
        let scale = self.scale.fractional_scale();
        // The struts are part of the theme's layout: floating themes inset the area with them.
        let area = self.scrolling.working_area();
        self.canvas
            .work_area
            .update(&config, area, mask_color, scale, active);
    }

    /// Whether the workspace draws a working area outline or corner masks.
    pub fn has_work_area_decor(&self) -> bool {
        self.canvas.work_area.has_elements()
    }

    /// Clip rectangle for the content of this workspace (workspace view coordinates), if any.
    pub fn work_area_clip(&self) -> Option<Rectangle<f64, Logical>> {
        self.canvas.work_area.clip()
    }

    /// Whether an element of this workspace's normal render layer is clipped to the working area:
    /// canvas windows and plane decorations on canvas workspaces, scrolling tiles on strip ones.
    pub fn work_area_clips<R: NiriRenderer>(&self, elem: &WorkspaceRenderElement<R>) -> bool {
        match elem {
            WorkspaceRenderElement::Canvas(_) | WorkspaceRenderElement::CanvasDecor(_) => true,
            WorkspaceRenderElement::Scrolling(_) => !self.is_canvas(),
            _ => false,
        }
    }

    /// Renders the outline and corner masks, above all content of the workspace.
    pub fn render_work_area<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        push: &mut dyn FnMut(WorkspaceRenderElement<R>),
    ) {
        self.canvas
            .work_area
            .render(renderer, &mut |elem| push(elem.into()));
    }
}
