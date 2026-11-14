//! rice: working area outline and clipping for "floating" shell themes.
//!
//! Floating themes draw the shell panels as separate rounded boxes with a gap between them, and
//! the main window area as one more rounded box: a thin outline around the working area, with
//! nothing drawn in the gaps. With `layout { work-area { ... } }`:
//!
//! - `clip true`: strip-mode tiles (with their title strips, borders and focus rings) and the
//!   canvas plane (windows, grid, regions) are cropped to the working area. Floating windows on
//!   strip workspaces, windows moving between workspaces, the interactive-move tile and fullscreen
//!   or maximized-to-edges windows are not clipped; while a fullscreen window is in view the
//!   workspace is not clipped at all.
//! - `corner-radius`: the clip has rounded corners. Content is cropped to the rectangle, and the
//!   four corner squares get a mask outside the rounded corner in the layout background colour
//!   (what the gaps show). With a wallpaper that colour shows in those few pixels instead of the
//!   wallpaper.
//! - `outline-width` / `outline-color`: an outline along the inside of the working area, above the
//!   content.
//!
//! Cost: cropping is `CropRenderElement` (no extra pass, damage stays per element); the outline is
//! drawn like a focus ring (four edges and four corner pieces), the masks are four corner-sized
//! border-shader quads. All of them only change when the working area or the config changes, so
//! a static workspace gets no extra damage. No offscreen rendering is involved: an offscreen pass
//! would allow an exact rounded clip over any background, but costs a full extra render of the
//! workspace every frame it changes.
//!
//! Everything is per workspace in workspace-local coordinates, so the clip follows each workspace
//! through workspace switch animations and the overview zoom.

use niri_config::{Color, CornerRadius, GradientInterpolation, WorkArea};
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::focus_ring::{FocusRing, FocusRingRenderElement};
use crate::niri_render_elements;
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::renderer::NiriRenderer;

niri_render_elements! {
    WorkAreaRenderElement => {
        Outline = FocusRingRenderElement,
        Mask = BorderRenderElement,
    }
}

/// Outline, corner masks and clip rectangle of one workspace.
#[derive(Debug)]
pub struct WorkAreaDecor {
    /// Clip rectangle in workspace view coordinates; `None` when not clipping.
    clip: Option<Rectangle<f64, Logical>>,
    outline: FocusRing,
    outline_loc: Point<f64, Logical>,
    show_outline: bool,
    masks: [BorderRenderElement; 4],
    mask_locs: [Point<f64, Logical>; 4],
    show_masks: bool,
}

impl Default for WorkAreaDecor {
    fn default() -> Self {
        Self {
            clip: None,
            outline: FocusRing::new(outline_config(&WorkArea::default())),
            outline_loc: Point::default(),
            show_outline: false,
            masks: Default::default(),
            mask_locs: Default::default(),
            show_masks: false,
        }
    }
}

fn outline_config(config: &WorkArea) -> niri_config::FocusRing {
    niri_config::FocusRing {
        off: false,
        width: config.outline_width,
        active_color: config.outline_color,
        inactive_color: config.outline_color,
        urgent_color: config.outline_color,
        ..Default::default()
    }
}

impl WorkAreaDecor {
    /// Recomputes everything for the working area `area`; `active` is false while nothing should
    /// be drawn or clipped (off in the config, fullscreen window in view).
    pub fn update(
        &mut self,
        config: &WorkArea,
        area: Rectangle<f64, Logical>,
        mask_color: Color,
        scale: f64,
        active: bool,
    ) {
        let usable = active && area.size.w > 0. && area.size.h > 0.;
        self.clip = (usable && config.clip).then_some(area);

        // Radius, at most half the smaller side.
        let radius = config
            .corner_radius
            .min(area.size.w / 2.)
            .min(area.size.h / 2.)
            .max(0.);

        // Outline along the inside: a border-style focus ring around the area shrunk by its
        // width, so that its outer edge (radius `radius`) is the working area edge.
        let width = config
            .outline_width
            .min(area.size.w / 2.)
            .min(area.size.h / 2.);
        self.show_outline = usable && width > 0.;
        if self.show_outline {
            self.outline.update_config(outline_config(config));
            let inner: Size<f64, Logical> =
                Size::from((area.size.w - 2. * width, area.size.h - 2. * width));
            let inner_radius = (radius - width).max(0.) as f32;
            self.outline.update_render_elements(
                Size::from((inner.w.max(0.), inner.h.max(0.))),
                true,
                true,
                false,
                Rectangle::from_size(area.size),
                CornerRadius::from(inner_radius),
                scale,
                1.,
            );
            self.outline_loc = area.loc + Point::from((width, width));
        }

        // Corner masks: in each `radius`-sized corner square, the part outside the rounded
        // corner. Each is a border-shader quad of a band of width `radius` around the area with
        // an outer radius of `2 * radius` (so the inner edge is the area with `radius`).
        self.show_masks = self.clip.is_some() && radius > 0.;
        if self.show_masks {
            let r = radius;
            let size = Size::from((r, r));
            let corners = [
                area.loc,
                area.loc + Point::from((area.size.w - r, 0.)),
                area.loc + Point::from((0., area.size.h - r)),
                area.loc + Point::from((area.size.w - r, area.size.h - r)),
            ];
            let band = Rectangle::new(
                area.loc - Point::from((r, r)),
                area.size + Size::from((2. * r, 2. * r)),
            );
            for (mask, loc) in self.masks.iter_mut().zip(corners) {
                let geometry = Rectangle::new(band.loc - loc, band.size);
                mask.update(
                    size,
                    Rectangle::from_size(size),
                    GradientInterpolation::default(),
                    mask_color,
                    mask_color,
                    0.,
                    geometry,
                    r as f32,
                    CornerRadius::from((2. * r) as f32),
                    scale as f32,
                    1.,
                );
            }
            self.mask_locs = corners;
        }
    }

    pub fn update_shaders(&mut self) {
        self.outline.update_shaders();
        for mask in &mut self.masks {
            mask.damage_all();
        }
    }

    pub fn clip(&self) -> Option<Rectangle<f64, Logical>> {
        self.clip
    }

    /// Whether there is an outline or corner masks to draw.
    pub fn has_elements(&self) -> bool {
        self.show_outline || self.show_masks
    }

    pub fn render(
        &self,
        renderer: &mut impl NiriRenderer,
        push: &mut dyn FnMut(WorkAreaRenderElement),
    ) {
        if self.show_outline {
            self.outline
                .render(renderer, self.outline_loc, &mut |elem| push(elem.into()));
        }
        if self.show_masks && BorderRenderElement::has_shader(renderer) {
            for (mask, loc) in self.masks.iter().zip(self.mask_locs) {
                push(mask.clone().with_location(loc).into());
            }
        }
    }
}
