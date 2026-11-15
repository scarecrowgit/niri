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
//!
//! [`MinifyMode::Sharp`] (used while the canvas zoom is at rest) replaces the box filter between
//! 1 and 2 texels per pixel (zoom 0.5 to 1) by a Catmull-Rom kernel scaled to the pixel
//! footprint: a box filter is visibly soft near ratio 1 (at 0.9 every glyph stem is smeared over
//! two pixels), plain bilinear filtering is soft *and* uneven (the blur depends on the sample
//! phase, so it changes across the window and while panning), while the Catmull-Rom kernel keeps
//! edges crisp with a slight overshoot. It reads up to `(2 · ceil(2k))²` texels per pixel (64 at
//! zoom 0.5), which is why it is only used at rest: windows whose client re-renders at the zoomed
//! scale (`crate::layout::client_scale`) are drawn 1:1 and need no filter at all, so at rest the
//! kernel only runs for clients that do not follow the scale.

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

/// Largest minification (texels per pixel) that the Catmull-Rom kernel handles: its support is
/// `2k` texels on either side, `2 · ceil(2k)` ≤ [`MAX_TAPS`] taps per axis.
pub const MAX_SHARP_MINIFICATION: f64 = 2.0;

/// Minification below which [`MinifyMode::Sharp`] draws with the default shader: a buffer drawn
/// 1:1 (or magnified) needs no filter.
pub const SHARP_THRESHOLD: f64 = 1.01;

/// How a [`MinifyRenderElement`] filters minified textures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinifyMode {
    /// Plain bilinear filtering (the renderer's default).
    Off,
    /// Box filter (supersampling) above [`MIN_MINIFICATION`]; cheap, used while zooming.
    Box,
    /// Catmull-Rom kernel from [`SHARP_THRESHOLD`] up to [`MAX_SHARP_MINIFICATION`], box filter
    /// above; used while the zoom is at rest.
    Sharp,
}

thread_local! {
    /// Mode of the [`MinifyRenderElement`] drawing right now (on this thread).
    static ACTIVE: Cell<MinifyMode> = const { Cell::new(MinifyMode::Off) };
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
        let taps = [
            UniformName::new("niri_taps", UniformType::_1f),
            UniformName::new("niri_mode", UniformType::_1f),
            UniformName::new("niri_k", UniformType::_2f),
        ];

        let texture = renderer
            .compile_custom_texture_shader(
                concat!(
                    include_str!("shaders/minify_texture.frag"),
                    include_str!("shaders/minify.frag"),
                ),
                &taps,
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
                        taps[0].clone(),
                        taps[1].clone(),
                        taps[2].clone(),
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

/// Filter for drawing the `src` part of a buffer with `transform` into `dst` in `mode`: `None`
/// for the default shader, else the uniforms of the minifying shaders.
pub fn filter_uniforms(
    mode: MinifyMode,
    src: Rectangle<f64, Buffer>,
    dst: Rectangle<i32, Physical>,
    transform: Transform,
) -> Option<Vec<Uniform<'static>>> {
    let uniforms = |taps: u32, sharp: bool, k: (f64, f64)| {
        vec![
            Uniform::new("niri_taps", taps as f32),
            Uniform::new("niri_mode", if sharp { 1f32 } else { 0f32 }),
            Uniform::new("niri_k", (k.0 as f32, k.1 as f32)),
        ]
    };
    match mode {
        MinifyMode::Off => None,
        MinifyMode::Box => {
            let taps = minify_taps(src, dst);
            (taps > 1).then(|| uniforms(taps, false, (1., 1.)))
        }
        MinifyMode::Sharp => {
            let (kx, ky) = minification(src, dst, transform)?;
            let k = kx.max(ky);
            if k <= SHARP_THRESHOLD {
                None
            } else if k <= MAX_SHARP_MINIFICATION {
                Some(uniforms(1, true, (kx.max(1.), ky.max(1.))))
            } else {
                let taps = minify_taps(src, dst);
                (taps > 1).then(|| uniforms(taps, false, (1., 1.)))
            }
        }
    }
}

/// Texels per output pixel along the buffer's x and y axes.
pub fn minification(
    src: Rectangle<f64, Buffer>,
    dst: Rectangle<i32, Physical>,
    transform: Transform,
) -> Option<(f64, f64)> {
    // The destination size along the buffer axes (90° transforms swap them).
    let dst = transform.transform_size(dst.size);
    if dst.w <= 0 || dst.h <= 0 || !(src.size.w > 0. && src.size.h > 0.) {
        return None;
    }
    let kx = src.size.w / f64::from(dst.w);
    let ky = src.size.h / f64::from(dst.h);
    (kx.is_finite() && ky.is_finite()).then_some((kx, ky))
}

/// Program and uniforms for a clipped surface: its own, or the minifying variant while a
/// [`MinifyRenderElement`] draws and the surface is drawn minified.
pub fn clipped_program(
    frame: &mut GlesFrame<'_, '_>,
    program: &GlesTexProgram,
    mut uniforms: Vec<Uniform<'static>>,
    src: Rectangle<f64, Buffer>,
    dst: Rectangle<i32, Physical>,
    transform: Transform,
) -> (GlesTexProgram, Vec<Uniform<'static>>) {
    let Some(extra) = filter_uniforms(ACTIVE.get(), src, dst, transform) else {
        return (program.clone(), uniforms);
    };
    match &Shaders::get_from_frame(frame).minify.clipped_surface {
        Some(minify) => {
            uniforms.extend(extra);
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
    mode: MinifyMode,
}

impl<E> MinifyRenderElement<E> {
    /// Wraps `inner` with the box filter; with `enabled == false` it draws exactly like `inner`.
    pub fn new(inner: E, enabled: bool) -> Self {
        let mode = if enabled {
            MinifyMode::Box
        } else {
            MinifyMode::Off
        };
        Self { inner, mode }
    }

    /// Wraps `inner` with the given filter mode.
    pub fn with_mode(inner: E, mode: MinifyMode) -> Self {
        Self { inner, mode }
    }
}

/// Sets [`ACTIVE`] for the lifetime of the guard.
struct ActiveGuard(MinifyMode);

impl ActiveGuard {
    fn enter(mode: MinifyMode) -> Self {
        Self(ACTIVE.replace(mode))
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        ACTIVE.set(self.0);
    }
}

impl<E: Element> MinifyRenderElement<E> {
    /// Minifying program and uniforms for drawing `src` into `dst`, if minification applies.
    fn program(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
    ) -> Option<(GlesTexProgram, Vec<Uniform<'static>>)> {
        let uniforms = filter_uniforms(self.mode, src, dst, self.inner.transform())?;
        let program = Shaders::get_from_frame(frame).minify.texture.clone()?;
        Some((program, uniforms))
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
        let Some((program, uniforms)) = self.program(frame, src, dst) else {
            // Nested clipped surfaces still pick their filter from the mode.
            let _guard = ActiveGuard::enter(self.mode);
            return self
                .inner
                .draw(frame, src, dst, damage, opaque_regions, cache);
        };

        frame.override_default_tex_program(program, uniforms);
        let res = {
            let _guard = ActiveGuard::enter(self.mode);
            self.inner
                .draw(frame, src, dst, damage, opaque_regions, cache)
        };
        frame.clear_tex_program_override();
        res
    }

    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        // A minified element must be composited (with the minifying shader), not scanned out.
        if self.mode != MinifyMode::Off {
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
        let Some((program, uniforms)) = self.program(frame.as_gles_frame(), src, dst) else {
            let _guard = ActiveGuard::enter(self.mode);
            return self
                .inner
                .draw(frame, src, dst, damage, opaque_regions, cache);
        };

        frame
            .as_gles_frame()
            .override_default_tex_program(program, uniforms);
        let res = {
            let _guard = ActiveGuard::enter(self.mode);
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
        if self.mode != MinifyMode::Off {
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
