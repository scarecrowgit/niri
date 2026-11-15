//! ashd: rescale element for canvas windows that keeps buffers rendered for the zoom pixel-exact.
//!
//! Like smithay's `RescaleRenderElement` (geometry scaled around an origin and rounded to
//! physical pixels), with one addition: with `snap` (set while the canvas zoom is at rest), an
//! element whose source buffer is already (within one pixel) the size it is drawn at gets exactly
//! that size, so the buffer is composited 1:1 instead of being resampled by a fraction of a
//! pixel. That is the case for window buffers of clients that follow the zoomed fractional scale
//! (`crate::layout::client_scale`) and for compositor text rasterized at the settled zoom (title
//! strips). Without the snap, a client that rounds its buffer size differently from the
//! compositor (or the double rounding at fractional output scales) would be stretched by one
//! pixel over its width, which blurs every column where the sample phase crosses half a texel.
//!
//! Only elements whose source size differs from their unscaled size are snapped (textures that
//! are already resampled at the output scale): solid colours, shaders and buffers rendered for
//! the output scale keep the plain rescaled geometry, so frames and backgrounds line up as
//! before. The position is the rescaled, rounded position in either case.

use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::Renderer;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform};

/// Largest difference (physical pixels per axis) between the rescaled size and the source size
/// that is snapped to the source size.
pub const SNAP_TOLERANCE: f64 = 1.0;

#[derive(Debug)]
pub struct CanvasRescaleElement<E> {
    element: E,
    origin: Point<i32, Physical>,
    scale: f64,
    snap: bool,
}

impl<E: Element> CanvasRescaleElement<E> {
    /// Rescales `element` by `scale` around `origin`; `snap` lets buffers that match the rescaled
    /// size be drawn 1:1.
    pub fn from_element(element: E, origin: Point<i32, Physical>, scale: f64, snap: bool) -> Self {
        Self {
            element,
            origin,
            scale,
            snap,
        }
    }

    /// Unscaled geometry and drawn geometry.
    fn geometries(
        &self,
        scale: Scale<f64>,
    ) -> (Rectangle<i32, Physical>, Rectangle<i32, Physical>) {
        let inner = self.element.geometry(scale);
        let mut geo = inner;
        geo.loc -= self.origin;
        geo = geo.to_f64().upscale(self.scale).to_i32_round();
        geo.loc += self.origin;
        if self.snap {
            let src = self
                .element
                .transform()
                .transform_size(self.element.src().size);
            geo.size = snapped_size(inner.size, geo.size, src);
        }
        (inner, geo)
    }

    /// Per-axis factor from unscaled to drawn element-local coordinates.
    fn factors(&self, scale: Scale<f64>) -> (f64, f64, Size<i32, Physical>) {
        let (inner, geo) = self.geometries(scale);
        let f = |drawn: i32, unscaled: i32| {
            if unscaled > 0 {
                f64::from(drawn) / f64::from(unscaled)
            } else {
                self.scale
            }
        };
        (
            f(geo.size.w, inner.size.w),
            f(geo.size.h, inner.size.h),
            geo.size,
        )
    }
}

/// Drawn size of an element: the rescaled size, or per axis the source size when the element's
/// source is resampled at the unscaled size but matches the rescaled size within
/// [`SNAP_TOLERANCE`].
pub fn snapped_size(
    unscaled: Size<i32, Physical>,
    rescaled: Size<i32, Physical>,
    src: Size<f64, Buffer>,
) -> Size<i32, Physical> {
    let axis = |unscaled: i32, rescaled: i32, src: f64| {
        let integral = src.is_finite() && src > 0. && (src - src.round()).abs() < 1e-6;
        let resampled = (src - f64::from(unscaled)).abs() > 0.5;
        let matches = (src - f64::from(rescaled)).abs() <= SNAP_TOLERANCE + 1e-6;
        if integral && resampled && matches {
            src.round() as i32
        } else {
            rescaled
        }
    };
    Size::from((
        axis(unscaled.w, rescaled.w, src.w),
        axis(unscaled.h, rescaled.h, src.h),
    ))
}

/// Maps an element-local rectangle by per-axis factors; `grow` rounds outwards (damage),
/// otherwise inwards (opaque regions). The result is clipped to `size`.
fn map_rect(
    rect: Rectangle<i32, Physical>,
    fx: f64,
    fy: f64,
    size: Size<i32, Physical>,
    grow: bool,
) -> Option<Rectangle<i32, Physical>> {
    let (x0, y0) = (f64::from(rect.loc.x) * fx, f64::from(rect.loc.y) * fy);
    let (x1, y1) = (
        f64::from(rect.loc.x + rect.size.w) * fx,
        f64::from(rect.loc.y + rect.size.h) * fy,
    );
    let (x0, y0, x1, y1) = if grow {
        (x0.floor(), y0.floor(), x1.ceil(), y1.ceil())
    } else {
        (x0.ceil(), y0.ceil(), x1.floor(), y1.floor())
    };
    let mapped = Rectangle::<i32, Physical>::from_extremities(
        (x0 as i32, y0 as i32),
        (x1 as i32, y1 as i32),
    );
    let mapped = mapped.intersection(Rectangle::from_size(size))?;
    (!mapped.is_empty()).then_some(mapped)
}

impl<E: Element> Element for CanvasRescaleElement<E> {
    fn id(&self) -> &Id {
        self.element.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.element.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.element.src()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.geometries(scale).1
    }

    fn transform(&self) -> Transform {
        self.element.transform()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        let (fx, fy, size) = self.factors(scale);
        self.element
            .damage_since(scale, commit)
            .into_iter()
            .filter_map(|rect| map_rect(rect, fx, fy, size, true))
            .collect()
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        let (fx, fy, size) = self.factors(scale);
        self.element
            .opaque_regions(scale)
            .into_iter()
            .filter_map(|rect| map_rect(rect, fx, fy, size, false))
            .collect()
    }

    fn alpha(&self) -> f32 {
        self.element.alpha()
    }

    fn kind(&self) -> Kind {
        self.element.kind()
    }

    fn is_framebuffer_effect(&self) -> bool {
        self.element.is_framebuffer_effect()
    }
}

impl<R: Renderer, E: RenderElement<R>> RenderElement<R> for CanvasRescaleElement<E> {
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), R::Error> {
        self.element
            .draw(frame, src, dst, damage, opaque_regions, cache)
    }

    fn underlying_storage(&self, renderer: &mut R) -> Option<UnderlyingStorage<'_>> {
        self.element.underlying_storage(renderer)
    }

    fn capture_framebuffer(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), R::Error> {
        self.element.capture_framebuffer(frame, src, dst, cache)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(w: i32, h: i32) -> Size<i32, Physical> {
        Size::from((w, h))
    }

    #[test]
    fn snaps_buffers_rendered_for_the_zoom() {
        // An 800x600 window at zoom 0.9 whose client rendered 720x540: drawn 1:1.
        assert_eq!(
            snapped_size(size(800, 600), size(720, 540), Size::from((720., 540.))),
            size(720, 540)
        );
        // A client that rounds up: 721 px for 720.4; drawn 1:1 at 721.
        assert_eq!(
            snapped_size(size(800, 600), size(720, 540), Size::from((721., 541.))),
            size(721, 541)
        );
    }

    #[test]
    fn keeps_rescaled_size_otherwise() {
        // A buffer rendered for the output scale is resampled as before.
        assert_eq!(
            snapped_size(size(800, 600), size(720, 540), Size::from((800., 600.))),
            size(720, 540)
        );
        // An old buffer (another zoom) is not snapped.
        assert_eq!(
            snapped_size(size(800, 600), size(600, 450), Size::from((720., 540.))),
            size(600, 450)
        );
        // Fractional sources (viewport crops) are not snapped.
        assert_eq!(
            snapped_size(size(800, 600), size(720, 540), Size::from((720.5, 540.))),
            size(720, 540)
        );
    }

    #[test]
    fn damage_and_opaque_mapping() {
        let r = Rectangle::new(Point::from((10, 10)), size(100, 50));
        let d = map_rect(r, 0.9, 0.9, size(720, 540), true).unwrap();
        assert_eq!(d, Rectangle::new(Point::from((9, 9)), size(90, 45)));
        let o = map_rect(r, 0.95, 0.95, size(720, 540), false).unwrap();
        assert!(o.loc.x >= 10 && o.loc.x + o.size.w <= 104);
        // Clipped to the element.
        let big = Rectangle::new(Point::from((0, 0)), size(1000, 1000));
        assert_eq!(
            map_rect(big, 0.9, 0.9, size(720, 540), true).unwrap().size,
            size(720, 540)
        );
    }
}
