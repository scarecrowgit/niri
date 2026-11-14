//! ashd: supersampled minification of textures drawn smaller than their size (zoomed-out canvas).
//!
//! The GLES renderer samples client buffers with plain bilinear filtering and there are no
//! mipmaps (client buffers are imported dmabufs/shm uploads that change every commit). Drawn at
//! zoom `z`, every output pixel covers `1/z` texels per axis, and bilinear filtering reads only a
//! 2x2 texel neighbourhood of it: below ~0.5 most texels are skipped, so text and 1 px lines
//! alias into a blocky, "low resolution" look that shimmers while panning.
//!
//! [`MinifyRenderElement`] wraps the canvas render elements and, for every texture drawn
//! minified by more than [`MIN_MINIFICATION`], overrides the texture shader with one that
//! averages `ceil(k)²` bilinear taps over the pixel's footprint (`minify.frag`). This is a
//! single pass with no offscreen buffers and no extra memory; the cost is `ceil(k)²` texture
//! reads per covered output pixel, while the covered area shrinks by `k²`, so the texel reads
//! per window stay roughly what drawing it at 100% costs.
//!
//! Window surfaces that niri clips to their geometry (`ClippedSurfaceRenderElement`, rounded
//! corners) use their own shader; while a `MinifyRenderElement` draws, [`clipped_program`] hands
//! them the minifying variant of it.

use std::cell::Cell;

use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, Uniform, UniformName, UniformType,
};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Physical, Rectangle, Scale, Transform};

use super::renderer::AsGlesFrame as _;
use super::shaders::Shaders;
use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};

/// Minification (texels per output pixel) below which plain bilinear filtering is used: up to
/// ~1.25 texels per pixel bilinear filtering does not skip texels and stays sharpest.
pub const MIN_MINIFICATION: f64 = 1.25;

/// Most taps per axis (must match `NIRI_MAX_TAPS` in `minify.frag`).
pub const MAX_TAPS: u32 = 8;

thread_local! {
    /// Whether a [`MinifyRenderElement`] is drawing right now (on this thread).
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
}

/// Shaders for minified drawing; `None` where compiling failed (e.g. no
/// `GL_OES_standard_derivatives`), in which case drawing falls back to bilinear filtering.
#[derive(Debug, Default)]
pub struct MinifyPrograms {
    /// Default texture shader with minification.
    pub texture: Option<GlesTexProgram>,
    /// `clipped_surface.frag` with minification.
    pub clipped_surface: Option<GlesTexProgram>,
}

impl MinifyPrograms {
    pub fn compile(renderer: &mut GlesRenderer) -> Self {
        let taps = UniformName::new("niri_taps", UniformType::_1f);

        let texture = renderer
            .compile_custom_texture_shader(
                concat!(
                    include_str!("shaders/minify_texture.frag"),
                    include_str!("shaders/minify.frag"),
                ),
                std::slice::from_ref(&taps),
            )
            .map_err(|err| warn!("error compiling minify texture shader: {err:?}"))
            .ok();

        let clipped_surface = clipped_surface_source().and_then(|src| {
            renderer
                .compile_custom_texture_shader(
                    src,
                    &[
                        UniformName::new("niri_scale", UniformType::_1f),
                        UniformName::new("geo_size", UniformType::_2f),
                        UniformName::new("corner_radius", UniformType::_4f),
                        UniformName::new("input_to_geo", UniformType::Matrix3x3),
                        taps,
                    ],
                )
                .map_err(|err| warn!("error compiling minify clipped surface shader: {err:?}"))
                .ok()
        });

        Self {
            texture,
            clipped_surface,
        }
    }
}

/// `clipped_surface.frag` (as niri compiles it) with its texture read replaced by the
/// minifying one.
fn clipped_surface_source() -> Option<String> {
    let base = include_str!("shaders/clipped_surface.frag");
    let read = "vec4 color = texture2D(tex, v_coords);";
    let defines = "//_DEFINES_";
    let protos = "vec4 postprocess(vec4 color);";
    if !(base.contains(read) && base.contains(defines) && base.contains(protos)) {
        warn!("clipped_surface.frag changed, minified clipped surfaces fall back to bilinear");
        return None;
    }
    let mut src = base
        .replacen(
            defines,
            "//_DEFINES_\n#extension GL_OES_standard_derivatives : enable",
            1,
        )
        .replacen(
            protos,
            "vec4 postprocess(vec4 color);\nvec4 niri_minify_sample(vec2 coords);",
            1,
        )
        .replacen(read, "vec4 color = niri_minify_sample(v_coords);", 1);
    src.push_str(include_str!("shaders/rounding_alpha.frag"));
    src.push_str("\nvec4 postprocess(vec4 color) { return color; }\n");
    src.push_str(include_str!("shaders/minify.frag"));
    Some(src)
}

/// Taps per axis for drawing the `src` part of a buffer into `dst`: 1 (plain bilinear) up to
/// [`MIN_MINIFICATION`] texels per pixel, else `ceil(texels per pixel)`, at most [`MAX_TAPS`].
pub fn minify_taps(src: Rectangle<f64, Buffer>, dst: Rectangle<i32, Physical>) -> u32 {
    let dst_area = f64::from(dst.size.w) * f64::from(dst.size.h);
    let src_area = src.size.w * src.size.h;
    if !(dst_area > 0. && src_area > 0.) {
        return 1;
    }
    // Area ratio: independent of 90° buffer transforms.
    let k = (src_area / dst_area).sqrt();
    if !k.is_finite() || k <= MIN_MINIFICATION {
        return 1;
    }
    (k.ceil() as u32).clamp(2, MAX_TAPS)
}

/// Program and uniforms for a clipped surface: its own, or the minifying variant while a
/// [`MinifyRenderElement`] draws and the surface is drawn minified.
pub fn clipped_program(
    frame: &mut GlesFrame<'_, '_>,
    program: &GlesTexProgram,
    mut uniforms: Vec<Uniform<'static>>,
    src: Rectangle<f64, Buffer>,
    dst: Rectangle<i32, Physical>,
) -> (GlesTexProgram, Vec<Uniform<'static>>) {
    if !ACTIVE.get() {
        return (program.clone(), uniforms);
    }
    let taps = minify_taps(src, dst);
    if taps <= 1 {
        return (program.clone(), uniforms);
    }
    match &Shaders::get_from_frame(frame).minify.clipped_surface {
        Some(minify) => {
            uniforms.push(Uniform::new("niri_taps", taps as f32));
            (minify.clone(), uniforms)
        }
        None => (program.clone(), uniforms),
    }
}

/// Wrapper that draws textures of the inner element with supersampled minification when they
/// are drawn smaller than their size.
#[derive(Debug)]
pub struct MinifyRenderElement<E> {
    inner: E,
    enabled: bool,
}

impl<E> MinifyRenderElement<E> {
    /// Wraps `inner`; with `enabled == false` it draws exactly like `inner`.
    pub fn new(inner: E, enabled: bool) -> Self {
        Self { inner, enabled }
    }
}

/// Sets [`ACTIVE`] for the lifetime of the guard.
struct ActiveGuard(bool);

impl ActiveGuard {
    fn enter() -> Self {
        Self(ACTIVE.replace(true))
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        ACTIVE.set(self.0);
    }
}

impl<E> MinifyRenderElement<E> {
    /// Minifying program and taps for drawing `src` into `dst`, if minification applies.
    fn program(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
    ) -> Option<(GlesTexProgram, u32)> {
        if !self.enabled {
            return None;
        }
        let taps = minify_taps(src, dst);
        if taps <= 1 {
            return None;
        }
        let program = Shaders::get_from_frame(frame).minify.texture.clone()?;
        Some((program, taps))
    }
}

impl<E: Element> Element for MinifyRenderElement<E> {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.inner.opaque_regions(scale)
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }

    fn is_framebuffer_effect(&self) -> bool {
        self.inner.is_framebuffer_effect()
    }
}

impl<E: RenderElement<GlesRenderer>> RenderElement<GlesRenderer> for MinifyRenderElement<E> {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        let Some((program, taps)) = self.program(frame, src, dst) else {
            return self
                .inner
                .draw(frame, src, dst, damage, opaque_regions, cache);
        };

        frame.override_default_tex_program(program, vec![Uniform::new("niri_taps", taps as f32)]);
        let res = {
            let _guard = ActiveGuard::enter();
            self.inner
                .draw(frame, src, dst, damage, opaque_regions, cache)
        };
        frame.clear_tex_program_override();
        res
    }

    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        // A minified element must be composited (with the minifying shader), not scanned out.
        if self.enabled {
            None
        } else {
            self.inner.underlying_storage(renderer)
        }
    }

    fn capture_framebuffer(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), GlesError> {
        self.inner.capture_framebuffer(frame, src, dst, cache)
    }
}

impl<'render, E: RenderElement<TtyRenderer<'render>>> RenderElement<TtyRenderer<'render>>
    for MinifyRenderElement<E>
{
    fn draw(
        &self,
        frame: &mut TtyFrame<'render, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), TtyRendererError<'render>> {
        let Some((program, taps)) = self.program(frame.as_gles_frame(), src, dst) else {
            return self
                .inner
                .draw(frame, src, dst, damage, opaque_regions, cache);
        };

        frame
            .as_gles_frame()
            .override_default_tex_program(program, vec![Uniform::new("niri_taps", taps as f32)]);
        let res = {
            let _guard = ActiveGuard::enter();
            self.inner
                .draw(frame, src, dst, damage, opaque_regions, cache)
        };
        frame.as_gles_frame().clear_tex_program_override();
        res
    }

    fn underlying_storage(
        &self,
        renderer: &mut TtyRenderer<'render>,
    ) -> Option<UnderlyingStorage<'_>> {
        if self.enabled {
            None
        } else {
            self.inner.underlying_storage(renderer)
        }
    }

    fn capture_framebuffer(
        &self,
        frame: &mut TtyFrame<'render, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), TtyRendererError<'render>> {
        self.inner.capture_framebuffer(frame, src, dst, cache)
    }
}

#[cfg(test)]
mod tests {
    use smithay::utils::Size;

    use super::*;

    fn taps(src: (f64, f64), dst: (i32, i32)) -> u32 {
        minify_taps(
            Rectangle::from_size(Size::from(src)),
            Rectangle::from_size(Size::from(dst)),
        )
    }

    #[test]
    fn taps_follow_minification() {
        // 1:1, magnified and mildly minified: plain bilinear.
        assert_eq!(taps((800., 600.), (800, 600)), 1);
        assert_eq!(taps((400., 300.), (800, 600)), 1);
        assert_eq!(taps((800., 600.), (720, 540)), 1); // zoom 0.9
        assert_eq!(taps((800., 600.), (640, 480)), 1); // zoom 0.8 (k = 1.25)
                                                       // zoom 0.62, 0.5, 0.4, 0.25.
        assert_eq!(taps((800., 600.), (496, 372)), 2);
        assert_eq!(taps((800., 600.), (400, 300)), 2);
        assert_eq!(taps((800., 600.), (320, 240)), 3);
        assert_eq!(taps((800., 600.), (200, 150)), 4);
        // Capped.
        assert_eq!(taps((8000., 6000.), (100, 75)), MAX_TAPS);
        // A 90° buffer transform swaps src axes; the area ratio is the same.
        assert_eq!(taps((600., 800.), (320, 240)), 3);
        // Degenerate.
        assert_eq!(taps((800., 600.), (0, 0)), 1);
        assert_eq!(taps((0., 0.), (10, 10)), 1);
    }

    #[test]
    fn clipped_surface_source_patches() {
        let src = clipped_surface_source().expect("clipped_surface.frag markers");
        assert!(src.contains("niri_minify_sample(v_coords)"));
        assert!(!src.contains("texture2D(tex, v_coords)"));
        assert!(src.contains("GL_OES_standard_derivatives"));
    }
}
