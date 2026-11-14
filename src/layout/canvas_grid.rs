//! ashd: canvas plane decorations: the grid background and named regions.
//!
//! Both are drawn in workspace view coordinates under all windows of a canvas workspace, from
//! the current [`CanvasTransform`], so they pan and zoom exactly with the plane. Strip
//! workspaces draw nothing and pay nothing.
//!
//! - The grid is one shader element covering the view (`canvas_grid.frag`): pixel-snapped lines at
//!   least one physical pixel wide, or anti-aliased dots with a minimum diameter. It fades out when
//!   the zoom makes the spacing smaller than a few physical pixels. With a `background-color` it is
//!   opaque and hides the backdrop under it.
//! - Regions are named plane rectangles drawn as four dashed edge elements (same shader) plus a
//!   caps label texture at the top-left, rendered with pangocairo at the output scale times the
//!   settled zoom so that it is crisp once the zoom settles.
//!
//! Regions come from the config (`canvas { region "name" x= y= w= h= }`) plus the runtime
//! additions/removals of `canvas-add-region` / `canvas-remove-region` (kept per workspace).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use anyhow::ensure;
use niri_config::{CanvasDecor, CanvasGrid, CanvasRegion, Color};
use pango::FontDescription;
use pangocairo::cairo::{self, ImageSurface};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{GlesError, GlesFrame, GlesRenderer, GlesTexture, Uniform};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{
    Buffer, Logical, Physical, Point, Rectangle, Scale, Size, Transform as SmithayTransform,
};

use super::canvas::CanvasTransform;
use super::{Layout, LayoutElement};
use crate::backend::tty::{TtyFrame, TtyRenderer, TtyRendererError};
use crate::niri_render_elements;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::{AsGlesFrame as _, NiriRenderer};
use crate::render_helpers::shader_element::ShaderRenderElement;
use crate::render_helpers::shaders::{ProgramType, Shaders};
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};

/// Dash and gap of region outlines, in logical plane pixels.
const REGION_DASH: (f64, f64) = (6., 4.);
/// Width of region outlines, in logical plane pixels.
const REGION_LINE_WIDTH: f64 = 1.;
/// Offset of the region label from the region's top-left corner, in logical plane pixels.
const REGION_LABEL_OFFSET: (f64, f64) = (14., 8.);
/// Letter spacing of region labels, in em.
const REGION_LABEL_LETTER_SPACING: f64 = 0.08;
/// Smallest dot diameter of the dot grid, in physical pixels.
const MIN_DOT_DIAMETER: f32 = 2.;

niri_render_elements! {
    CanvasDecorRenderElement => {
        Shader = CanvasGridRenderElement,
        SolidColor = SolidColorRenderElement,
        Label = PrimaryGpuTextureRenderElement,
    }
}

/// Shader element of the grid or of one dashed region edge.
#[derive(Debug, Clone)]
pub struct CanvasGridRenderElement {
    inner: ShaderRenderElement,
    params: Option<GridParams>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct GridParams {
    size: Size<f64, Logical>,
    mode: f32,
    offset: [f32; 2],
    spacing: f32,
    line_width: f32,
    min_width: f32,
    dash: [f32; 2],
    color: Color,
    bg_color: Option<Color>,
}

impl Default for CanvasGridRenderElement {
    fn default() -> Self {
        Self {
            inner: ShaderRenderElement::empty(ProgramType::CanvasGrid, Kind::Unspecified),
            params: None,
        }
    }
}

impl CanvasGridRenderElement {
    fn update(&mut self, params: GridParams) {
        if self.params == Some(params) {
            return;
        }
        self.params = Some(params);

        let bg = params.bg_color.unwrap_or_default();
        let opaque = (bg.a >= 1.).then(|| vec![Rectangle::from_size(params.size)]);
        self.inner.update(
            params.size,
            opaque,
            1.,
            1.,
            Rc::new([
                Uniform::new("mode", params.mode),
                Uniform::new("elem_size", [params.size.w as f32, params.size.h as f32]),
                Uniform::new("grid_offset", params.offset),
                Uniform::new("spacing", params.spacing),
                Uniform::new("line_width", params.line_width),
                Uniform::new("min_width", params.min_width),
                Uniform::new("dash", params.dash),
                Uniform::new("color", params.color.to_array_premul()),
                Uniform::new("bg_color", bg.to_array_premul()),
            ]),
            HashMap::new(),
        );
    }

    pub fn damage_all(&mut self) {
        self.inner.damage_all();
    }

    fn with_location(mut self, location: Point<f64, Logical>) -> Self {
        self.inner = self.inner.with_location(location);
        self
    }

    fn with_alpha(mut self, alpha: f32) -> Self {
        if alpha < 1. {
            self.inner = self.inner.with_alpha(alpha);
        }
        self
    }

    pub fn has_shader(renderer: &mut impl NiriRenderer) -> bool {
        Shaders::get(renderer)
            .program(ProgramType::CanvasGrid)
            .is_some()
    }
}

impl Element for CanvasGridRenderElement {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn transform(&self) -> SmithayTransform {
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
}

impl RenderElement<GlesRenderer> for CanvasGridRenderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        RenderElement::<GlesRenderer>::draw(
            &self.inner,
            frame,
            src,
            dst,
            damage,
            opaque_regions,
            cache,
        )
    }

    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        self.inner.underlying_storage(renderer)
    }
}

impl<'render> RenderElement<TtyRenderer<'render>> for CanvasGridRenderElement {
    fn draw(
        &self,
        frame: &mut TtyFrame<'_, '_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), TtyRendererError<'render>> {
        let frame = frame.as_gles_frame();
        RenderElement::<GlesRenderer>::draw(self, frame, src, dst, damage, opaque_regions, cache)?;
        Ok(())
    }

    fn underlying_storage(
        &self,
        renderer: &mut TtyRenderer<'render>,
    ) -> Option<UnderlyingStorage<'_>> {
        self.inner.underlying_storage(renderer)
    }
}

/// Grid and region state of one canvas workspace.
#[derive(Debug, Default)]
pub struct CanvasDecorations {
    /// Regions added at runtime (`canvas-add-region`); they replace config regions of the same
    /// name.
    added: Vec<CanvasRegion>,
    /// Names of config regions removed at runtime (`canvas-remove-region`).
    removed: Vec<String>,

    /// Whether anything is drawn (canvas mode and something configured).
    visible: bool,
    grid: CanvasGridRenderElement,
    show_grid_shader: bool,
    background: SolidColorBuffer,
    show_background: bool,
    /// Top-left of the grid and background (the working area clip, or the view origin).
    area_loc: Point<f64, Logical>,
    regions: Vec<RegionDecor>,
}

#[derive(Debug)]
struct RegionDecor {
    name: String,
    visible: bool,
    edges: [CanvasGridRenderElement; 4],
    edge_locs: [Point<f64, Logical>; 4],
    label_loc: Point<f64, Logical>,
    /// Current zoom: the label texture is drawn at its plane size times this.
    zoom: f64,
    label: RefCell<LabelCache>,
    label_key: LabelKey,
}

#[derive(Debug, Default)]
struct LabelCache {
    key: Option<LabelKey>,
    texture: Option<TextureBuffer<GlesTexture>>,
}

#[derive(Debug, Clone, PartialEq)]
struct LabelKey {
    text: String,
    font: String,
    color: Color,
    scale: f64,
}

impl CanvasDecorations {
    /// Regions as currently in effect: config regions minus removed ones, plus runtime ones.
    pub fn effective_regions(&self, config: &CanvasDecor) -> Vec<CanvasRegion> {
        let mut regions: Vec<CanvasRegion> = config
            .regions
            .iter()
            .filter(|r| !self.removed.contains(&r.name))
            .filter(|r| !self.added.iter().any(|a| a.name == r.name))
            .cloned()
            .collect();
        regions.extend(self.added.iter().cloned());
        regions
    }

    /// Adds (or replaces) a region.
    pub fn add_region(&mut self, region: CanvasRegion) {
        self.removed.retain(|name| *name != region.name);
        self.added.retain(|r| r.name != region.name);
        self.added.push(region);
    }

    /// Removes a region by name; returns whether one was known.
    pub fn remove_region(&mut self, name: &str, config: &CanvasDecor) -> bool {
        let existed = self
            .effective_regions(config)
            .iter()
            .any(|r| r.name == name);
        self.added.retain(|r| r.name != name);
        if config.regions.iter().any(|r| r.name == name) && !self.removed.iter().any(|n| n == name)
        {
            self.removed.push(name.to_owned());
        }
        existed
    }

    pub fn update_shaders(&mut self) {
        self.grid.damage_all();
        for region in &mut self.regions {
            for edge in &mut region.edges {
                edge.damage_all();
            }
        }
    }

    /// Recomputes the elements for the current view.
    ///
    /// `transform` is `None` on strip workspaces. `settled_zoom` is the zoom the region labels
    /// are rasterized for. With `clip` (the working area clip), the grid, the plane background
    /// and the region outlines are laid out within it (the shader elements can't be cropped
    /// afterwards: their patterns are computed from the element size).
    pub fn update_render_elements(
        &mut self,
        transform: Option<CanvasTransform>,
        view_size: Size<f64, Logical>,
        scale: f64,
        settled_zoom: f64,
        config: &CanvasDecor,
        clip: Option<Rectangle<f64, Logical>>,
    ) {
        let Some(transform) = transform else {
            self.visible = false;
            return;
        };
        self.visible = true;
        let zoom = transform.zoom;
        let area = clip.unwrap_or_else(|| Rectangle::from_size(view_size));
        self.area_loc = area.loc;

        // Grid and background.
        let grid_on = config.grid != CanvasGrid::Off && config.grid_spacing >= 1.;
        self.show_grid_shader = grid_on || config.background_color.is_some();
        self.show_background = config.background_color.is_some();
        if let Some(color) = config.background_color {
            self.background.resize(area.size);
            self.background.set_color(color);
        }
        if self.show_grid_shader {
            let spacing = config.grid_spacing * zoom;
            // View position of the plane origin relative to the element, folded into one period.
            let origin = transform.plane_to_view(Point::from((0., 0.))) - area.loc;
            let (mode, min_width) = match config.grid {
                CanvasGrid::Dots => (1., MIN_DOT_DIAMETER),
                _ => (0., 1.),
            };
            let (color, line_width) = if grid_on {
                (config.grid_color, config.grid_line_width * zoom)
            } else {
                (Color::default(), 0.)
            };
            self.grid.update(GridParams {
                size: area.size,
                mode,
                offset: [
                    origin.x.rem_euclid(spacing.max(1e-6)) as f32,
                    origin.y.rem_euclid(spacing.max(1e-6)) as f32,
                ],
                spacing: if grid_on { spacing as f32 } else { 1e6 },
                line_width: line_width as f32,
                min_width: if grid_on { min_width } else { 0. },
                dash: [0., 0.],
                color,
                bg_color: config.background_color,
            });
        }

        // Regions, reusing the elements of regions with the same name.
        let regions = self.effective_regions(config);
        let mut old = std::mem::take(&mut self.regions);
        let view_rect = area;
        let text_scale = scale * settled_zoom.clamp(0.05, 4.);
        for region in regions {
            let mut decor = match old.iter().position(|r| r.name == region.name) {
                Some(idx) => old.swap_remove(idx),
                None => RegionDecor::new(region.name.clone()),
            };

            let plane = Rectangle::new(
                Point::from((region.x, region.y)),
                Size::from((region.w, region.h)),
            );
            let rect = transform.plane_rect_to_view(plane);
            // Outlines are at least one physical pixel wide.
            let lw = f64::max(REGION_LINE_WIDTH * zoom, 1. / scale);
            decor.visible = rect.overlaps(view_rect)
                || rect.contains_rect(view_rect)
                || view_rect.contains_rect(rect);
            decor.zoom = zoom;

            let dash = [(REGION_DASH.0 * zoom) as f32, (REGION_DASH.1 * zoom) as f32];
            let edges = [
                (rect.loc, Size::from((rect.size.w, lw)), 2.),
                (
                    rect.loc + Point::from((0., rect.size.h - lw)),
                    Size::from((rect.size.w, lw)),
                    2.,
                ),
                (rect.loc, Size::from((lw, rect.size.h)), 3.),
                (
                    rect.loc + Point::from((rect.size.w - lw, 0.)),
                    Size::from((lw, rect.size.h)),
                    3.,
                ),
            ];
            for (i, (loc, size, mode)) in edges.into_iter().enumerate() {
                // Within the clip, with the dash phase of the part that was cut off.
                let edge = Rectangle::new(loc, size);
                let (loc, size, offset) = match clip.map(|clip| edge.intersection(clip)) {
                    None => (loc, size, [0., 0.]),
                    Some(Some(part)) => {
                        let cut = part.loc - loc;
                        (part.loc, part.size, [cut.x as f32, cut.y as f32])
                    }
                    Some(None) => (loc, Size::from((0., 0.)), [0., 0.]),
                };
                decor.edge_locs[i] = loc;
                decor.edges[i].update(GridParams {
                    size,
                    mode,
                    offset,
                    spacing: 1.,
                    line_width: 0.,
                    min_width: 0.,
                    dash,
                    color: config.region_color,
                    bg_color: None,
                });
            }

            decor.label_loc = transform.plane_to_view(
                plane.loc + Point::from((REGION_LABEL_OFFSET.0, REGION_LABEL_OFFSET.1)),
            );
            decor.label_key = LabelKey {
                text: region.name.to_uppercase(),
                font: config.region_font.clone(),
                color: config.region_label_color,
                scale: text_scale,
            };

            self.regions.push(decor);
        }
    }

    pub fn render<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        scale: f64,
        alpha: f32,
        push: &mut dyn FnMut(CanvasDecorRenderElement),
    ) {
        if alpha <= 0. {
            return;
        }
        if !self.visible {
            return;
        }

        let has_shader = CanvasGridRenderElement::has_shader(renderer);

        for region in self.regions.iter().filter(|r| r.visible) {
            if let Some(elem) = region.label_element(renderer.as_gles_renderer(), scale, alpha) {
                push(elem.into());
            }
            if has_shader {
                for (edge, loc) in region.edges.iter().zip(region.edge_locs) {
                    // Edges cut away by the working area clip are empty.
                    if edge.params.is_some_and(|p| p.size.w > 0. && p.size.h > 0.) {
                        push(edge.clone().with_location(loc).with_alpha(alpha).into());
                    }
                }
            }
        }

        if has_shader && self.show_grid_shader {
            push(
                self.grid
                    .clone()
                    .with_location(self.area_loc)
                    .with_alpha(alpha)
                    .into(),
            );
        } else if self.show_background {
            let elem = SolidColorRenderElement::from_buffer(
                &self.background,
                self.area_loc,
                alpha,
                Kind::Unspecified,
            );
            push(elem.into());
        }
    }

    #[cfg(test)]
    pub fn visible_region_names(&self) -> Vec<String> {
        self.regions
            .iter()
            .filter(|r| r.visible)
            .map(|r| r.name.clone())
            .collect()
    }
}

impl RegionDecor {
    fn new(name: String) -> Self {
        Self {
            name,
            visible: false,
            edges: Default::default(),
            edge_locs: Default::default(),
            label_loc: Point::default(),
            zoom: 1.,
            label: RefCell::default(),
            label_key: LabelKey {
                text: String::new(),
                font: String::new(),
                color: Color::default(),
                scale: 1.,
            },
        }
    }

    fn label_element(
        &self,
        renderer: &mut GlesRenderer,
        scale: f64,
        alpha: f32,
    ) -> Option<PrimaryGpuTextureRenderElement> {
        if self.label_key.text.is_empty() {
            return None;
        }

        let mut cache = self.label.borrow_mut();
        if cache.key.as_ref() != Some(&self.label_key) {
            cache.texture = render_label(renderer, &self.label_key)
                .map_err(|err| trace!("error rendering canvas region label: {err:?}"))
                .ok();
            cache.key = Some(self.label_key.clone());
        }
        let texture = cache.texture.clone()?;

        // The texture is rasterized for the settled zoom; draw it at the current one.
        let size = texture.logical_size().upscale(self.zoom);
        let loc = self
            .label_loc
            .to_physical_precise_round(scale)
            .to_logical(scale);
        let elem = TextureRenderElement::from_texture_buffer(
            texture,
            loc,
            alpha,
            None,
            Some(size),
            Kind::Unspecified,
        );
        Some(PrimaryGpuTextureRenderElement(elem))
    }
}

fn render_label(
    renderer: &mut GlesRenderer,
    key: &LabelKey,
) -> anyhow::Result<TextureBuffer<GlesTexture>> {
    let _span = tracy_client::span!("canvas_grid::render_label");

    let mut font = FontDescription::from_string(&key.font);
    let size = font.size();
    ensure!(size > 0);
    // Font sizes are in logical plane pixels.
    let px = f64::from(size) * key.scale;
    font.set_absolute_size(px.round());

    let attrs = pango::AttrList::new();
    let spacing = (REGION_LABEL_LETTER_SPACING * px).round() as i32;
    attrs.insert(pango::AttrInt::new_letter_spacing(spacing));

    let surface = ImageSurface::create(cairo::Format::ARgb32, 0, 0)?;
    let cr = cairo::Context::new(&surface)?;
    let layout = pangocairo::functions::create_layout(&cr);
    layout.context().set_round_glyph_positions(false);
    layout.set_single_paragraph_mode(true);
    layout.set_font_description(Some(&font));
    layout.set_attributes(Some(&attrs));
    layout.set_text(&key.text);

    let (width, height) = layout.pixel_size();
    ensure!(width > 0 && height > 0);
    let width = width.min(16383);
    let height = height.min(16383);

    let surface = ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    let cr = cairo::Context::new(&surface)?;
    let [r, g, b, a] = key.color.to_array_unpremul();
    cr.set_source_rgba(r.into(), g.into(), b.into(), a.into());
    pangocairo::functions::show_layout(&cr, &layout);

    drop(cr);
    let data = surface.take_data().unwrap();
    let buffer = TextureBuffer::from_memory(
        renderer,
        &data,
        Fourcc::Argb8888,
        (width, height),
        false,
        key.scale,
        SmithayTransform::Normal,
        Vec::new(),
    )?;
    Ok(buffer)
}

/// Layout-level region API, operating on the focused workspace.
impl<W: LayoutElement> Layout<W> {
    /// Adds (or replaces) a named region on the focused workspace's canvas plane.
    pub fn canvas_add_region(&mut self, region: CanvasRegion) {
        if let Some(ws) = self.active_workspace_mut() {
            ws.canvas_add_region(region);
        }
    }

    /// Removes a named region from the focused workspace.
    pub fn canvas_remove_region(&mut self, name: &str) {
        if let Some(ws) = self.active_workspace_mut() {
            ws.canvas_remove_region(name);
        }
    }
}
