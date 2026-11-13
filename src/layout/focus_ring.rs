use std::iter::zip;

use niri_config::{CornerRadius, Gradient, GradientRelativeTo};
use smithay::backend::renderer::element::{Element as _, Kind};
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::niri_render_elements;
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};

#[derive(Debug)]
pub struct FocusRing {
    buffers: [SolidColorBuffer; 8],
    locations: [Point<f64, Logical>; 8],
    sizes: [Size<f64, Logical>; 8],
    borders: [BorderRenderElement; 8],
    full_size: Size<f64, Logical>,
    is_border: bool,
    use_border_shader: bool,
    config: niri_config::FocusRing,
    thicken_corners: bool,
    /// rice: gap between the window and the ring used by the last update.
    gap: f64,
}

niri_render_elements! {
    FocusRingRenderElement => {
        SolidColor = SolidColorRenderElement,
        Gradient = BorderRenderElement,
    }
}

impl FocusRing {
    pub fn new(config: niri_config::FocusRing) -> Self {
        Self {
            buffers: Default::default(),
            locations: Default::default(),
            sizes: Default::default(),
            borders: Default::default(),
            full_size: Default::default(),
            is_border: false,
            use_border_shader: false,
            config,
            thicken_corners: true,
            gap: 0.,
        }
    }

    pub fn update_config(&mut self, config: niri_config::FocusRing) {
        self.config = config;
    }

    pub fn update_shaders(&mut self) {
        for elem in &mut self.borders {
            elem.damage_all();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_render_elements(
        &mut self,
        win_size: Size<f64, Logical>,
        is_active: bool,
        is_border: bool,
        is_urgent: bool,
        view_rect: Rectangle<f64, Logical>,
        radius: CornerRadius,
        scale: f64,
        alpha: f32,
    ) {
        let width = self.config.width;

        // rice: with a gap, the ring goes around the window grown by the gap on every side,
        // with concentric corners, and is always drawn as a border (never as a background).
        let gap = self.config.gap.max(0.);
        self.gap = gap;
        let (win_size, view_rect, is_border, radius) = if gap > 0. {
            let grow = |r: f32| {
                if r > 0. {
                    r + gap as f32
                } else {
                    (gap + width) as f32
                }
            };
            let radius = CornerRadius {
                top_left: grow(radius.top_left),
                top_right: grow(radius.top_right),
                bottom_right: grow(radius.bottom_right),
                bottom_left: grow(radius.bottom_left),
            };
            let view_rect = Rectangle::new(view_rect.loc + Point::from((gap, gap)), view_rect.size);
            (
                win_size + Size::from((gap, gap)).upscale(2.),
                view_rect,
                true,
                radius,
            )
        } else {
            (win_size, view_rect, is_border, radius)
        };

        self.full_size = win_size + Size::from((width, width)).upscale(2.);
        self.is_border = is_border;

        let color = if is_urgent {
            self.config.urgent_color
        } else if is_active {
            self.config.active_color
        } else {
            self.config.inactive_color
        };

        for buf in &mut self.buffers {
            buf.set_color(color);
        }

        let radius = radius.fit_to(self.full_size.w as f32, self.full_size.h as f32);

        let gradient = if is_urgent {
            self.config.urgent_gradient
        } else if is_active {
            self.config.active_gradient
        } else {
            self.config.inactive_gradient
        };

        self.use_border_shader = radius != CornerRadius::default() || gradient.is_some();

        // Set the defaults for solid color + rounded corners.
        let gradient = gradient.unwrap_or_else(|| Gradient::from(color));

        let full_rect = Rectangle::new(Point::from((-width, -width)), self.full_size);
        let gradient_area = match gradient.relative_to {
            GradientRelativeTo::Window => full_rect,
            GradientRelativeTo::WorkspaceView => view_rect,
        };

        let rounded_corner_border_width = if is_border {
            // HACK: increase the border width used for the inner rounded corners a tiny bit to
            // reduce background bleed.
            let extra = if self.thicken_corners { 0.5 } else { 0. };
            width as f32 + extra
        } else {
            0.
        };

        let ceil = |logical: f64| (logical * scale).ceil() / scale;

        // All of this stuff should end up aligned to physical pixels because:
        // * Window size and border width are rounded to physical pixels before being passed to this
        //   function.
        // * We will ceil the corner radii below.
        // * We do not divide anything, only add, subtract and multiply by integers.
        // * At rendering time, tile positions are rounded to physical pixels.

        if is_border {
            let top_left = f64::max(width, ceil(f64::from(radius.top_left)));
            let top_right = f64::min(
                self.full_size.w - top_left,
                f64::max(width, ceil(f64::from(radius.top_right))),
            );
            let bottom_left = f64::min(
                self.full_size.h - top_left,
                f64::max(width, ceil(f64::from(radius.bottom_left))),
            );
            let bottom_right = f64::min(
                self.full_size.h - top_right,
                f64::min(
                    self.full_size.w - bottom_left,
                    f64::max(width, ceil(f64::from(radius.bottom_right))),
                ),
            );

            // Top edge.
            self.sizes[0] = Size::from((win_size.w + width * 2. - top_left - top_right, width));
            self.locations[0] = Point::from((-width + top_left, -width));

            // Bottom edge.
            self.sizes[1] =
                Size::from((win_size.w + width * 2. - bottom_left - bottom_right, width));
            self.locations[1] = Point::from((-width + bottom_left, win_size.h));

            // Left edge.
            self.sizes[2] = Size::from((width, win_size.h + width * 2. - top_left - bottom_left));
            self.locations[2] = Point::from((-width, -width + top_left));

            // Right edge.
            self.sizes[3] = Size::from((width, win_size.h + width * 2. - top_right - bottom_right));
            self.locations[3] = Point::from((win_size.w, -width + top_right));

            // Top-left corner.
            self.sizes[4] = Size::from((top_left, top_left));
            self.locations[4] = Point::from((-width, -width));

            // Top-right corner.
            self.sizes[5] = Size::from((top_right, top_right));
            self.locations[5] = Point::from((win_size.w + width - top_right, -width));

            // Bottom-right corner.
            self.sizes[6] = Size::from((bottom_right, bottom_right));
            self.locations[6] = Point::from((
                win_size.w + width - bottom_right,
                win_size.h + width - bottom_right,
            ));

            // Bottom-left corner.
            self.sizes[7] = Size::from((bottom_left, bottom_left));
            self.locations[7] = Point::from((-width, win_size.h + width - bottom_left));

            for (buf, size) in zip(&mut self.buffers, self.sizes) {
                buf.resize(size);
            }

            for (border, (loc, size)) in zip(&mut self.borders, zip(self.locations, self.sizes)) {
                border.update(
                    size,
                    Rectangle::new(gradient_area.loc - loc, gradient_area.size),
                    gradient.in_,
                    gradient.from,
                    gradient.to,
                    ((gradient.angle as f32) - 90.).to_radians(),
                    Rectangle::new(full_rect.loc - loc, full_rect.size),
                    rounded_corner_border_width,
                    radius,
                    scale as f32,
                    alpha,
                );
            }
        } else {
            self.sizes[0] = self.full_size;
            self.buffers[0].resize(self.sizes[0]);
            self.locations[0] = Point::from((-width, -width));

            self.borders[0].update(
                self.sizes[0],
                Rectangle::new(gradient_area.loc - self.locations[0], gradient_area.size),
                gradient.in_,
                gradient.from,
                gradient.to,
                ((gradient.angle as f32) - 90.).to_radians(),
                Rectangle::new(full_rect.loc - self.locations[0], full_rect.size),
                rounded_corner_border_width,
                radius,
                scale as f32,
                alpha,
            );
        }
    }

    pub fn render(
        &self,
        renderer: &mut impl NiriRenderer,
        location: Point<f64, Logical>,
        push: &mut dyn FnMut(FocusRingRenderElement),
    ) {
        if self.config.off {
            return;
        }

        let border_width = -self.locations[0].y;
        // rice: the elements are laid out around the window grown by the gap.
        let location = location - Point::from((self.gap, self.gap));

        // If drawing as a border with width = 0, then there's nothing to draw.
        if self.is_border && border_width == 0. {
            return;
        }

        let has_border_shader = BorderRenderElement::has_shader(renderer);

        let mut push = |buffer, border: &BorderRenderElement, location: Point<f64, Logical>| {
            let elem = if self.use_border_shader && has_border_shader {
                border.clone().with_location(location).into()
            } else {
                let alpha = border.alpha();
                SolidColorRenderElement::from_buffer(buffer, location, alpha, Kind::Unspecified)
                    .into()
            };
            push(elem);
        };

        if self.is_border {
            for ((buf, border), loc) in zip(zip(&self.buffers, &self.borders), self.locations) {
                push(buf, border, location + loc);
            }
        } else {
            push(
                &self.buffers[0],
                &self.borders[0],
                location + self.locations[0],
            );
        }
    }

    pub fn width(&self) -> f64 {
        self.config.width
    }

    pub fn is_off(&self) -> bool {
        self.config.off
    }

    pub fn set_thicken_corners(&mut self, value: bool) {
        self.thicken_corners = value;
    }

    pub fn config(&self) -> &niri_config::FocusRing {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(width: f64, gap: f64) -> FocusRing {
        FocusRing::new(niri_config::FocusRing {
            width,
            gap,
            ..Default::default()
        })
    }

    /// Outer rectangle of the drawn ring relative to the tile, and whether it is a border.
    fn outer(ring: &FocusRing) -> (Rectangle<f64, Logical>, bool) {
        let loc = ring.locations[4] - Point::from((ring.gap, ring.gap));
        (Rectangle::new(loc, ring.full_size), ring.is_border)
    }

    #[test]
    fn no_gap_is_upstream() {
        let mut ring = ring(4., 0.);
        let size = Size::from((100., 50.));
        let view = Rectangle::from_size(Size::from((1000., 1000.)));
        ring.update_render_elements(size, true, false, false, view, Default::default(), 1., 1.);
        assert_eq!(ring.gap, 0.);
        assert!(!ring.is_border);
        assert_eq!(ring.locations[0], Point::from((-4., -4.)));
        assert_eq!(ring.full_size, Size::from((108., 58.)));
    }

    #[test]
    fn gap_moves_ring_outwards() {
        let mut ring = ring(1., 4.);
        let size = Size::from((100., 50.));
        let view = Rectangle::from_size(Size::from((1000., 1000.)));
        // Even when asked to draw as a background, a ring with a gap is a border.
        ring.update_render_elements(size, true, false, false, view, Default::default(), 1., 1.);
        let (rect, is_border) = outer(&ring);
        assert!(is_border);
        assert_eq!(rect, Rectangle::new((-5., -5.).into(), (110., 60.).into()));

        // Edges: the top edge starts after the (gap + width = 5) corner and sits 5 px above.
        let top = Rectangle::new(
            ring.locations[0] - Point::from((ring.gap, ring.gap)),
            ring.sizes[0],
        );
        assert_eq!(top.loc.y, -5.);
        assert_eq!(top.size.h, 1.);
        // Square window corners get a concentric radius of gap + width.
        assert_eq!(ring.sizes[4], Size::from((5., 5.)));
    }

    #[test]
    fn gap_keeps_corners_concentric() {
        let mut ring = ring(2., 4.);
        let size = Size::from((100., 50.));
        let view = Rectangle::from_size(Size::from((1000., 1000.)));
        // The tile passes the window radius expanded by the ring width.
        let radius = CornerRadius::from(8.).expanded_by(2.);
        ring.update_render_elements(size, true, true, false, view, radius, 1., 1.);
        // Outer radius: window radius 8 + gap 4 + width 2.
        assert_eq!(ring.sizes[4], Size::from((14., 14.)));
        let (rect, _) = outer(&ring);
        assert_eq!(rect, Rectangle::new((-6., -6.).into(), (112., 62.).into()));
    }
}
