//! ashd: compositor-drawn title strip of a tile.
//!
//! The strip sits between the top border and the window: tile = border + strip + window + border
//! vertically. It shows the window title on the left (ellipsized) and a muted subtitle (the app
//! id) on the right, over a solid background with a thin line at the bottom.
//!
//! The text is rendered with pangocairo into one texture per (text, width, scale, style), cached
//! until any of those change. The scale is the output scale times the settled canvas zoom, so
//! that the text stays crisp on a zoomed canvas.
//!
//! The same struct also draws the *canvas title label* of the tile: below a configurable canvas
//! zoom, a small label with the window title at a constant on-screen size, above the tile's
//! top-left corner (see [`TitleBar::label_element`]).

use std::cell::{Cell, RefCell};

use anyhow::ensure;
use niri_config::{Color, CornerRadius, GradientInterpolation};
use pango::FontDescription;
use pangocairo::cairo::{self, ImageSurface};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::utils::{Logical, Point, Rectangle, Size, Transform};

use super::Options;
use crate::niri_render_elements;
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::utils::{round_logical_in_physical, round_logical_in_physical_max1};
use crate::window::ResolvedWindowRules;

/// Largest texture dimension we create for title text.
const MAX_TEXTURE_SIZE: i32 = 16383;

niri_render_elements! {
    TitleBarRenderElement => {
        SolidColor = SolidColorRenderElement,
        Rounded = BorderRenderElement,
        Text = PrimaryGpuTextureRenderElement,
    }
}

#[derive(Debug)]
pub struct TitleBar {
    config: niri_config::TitleBar,
    /// Whether the title bar is shown for this window (config + window rule).
    enabled: bool,
    /// Strip height rounded to physical pixels.
    height: f64,
    /// Bottom line width rounded to physical pixels.
    line_width: f64,
    /// Horizontal padding rounded to physical pixels.
    padding: f64,
    scale: f64,
    /// Scale the text is rendered at: output scale times the settled canvas zoom.
    text_scale: f64,

    title: Option<String>,
    subtitle: Option<String>,

    // Render state from the last update_render_elements().
    size: Size<f64, Logical>,
    alpha: f32,
    use_rounded: bool,
    background: SolidColorBuffer,
    rounded_background: BorderRenderElement,
    line: SolidColorBuffer,

    text: RefCell<TextCache>,
    /// Canvas title label textures.
    labels: RefCell<LabelCache<TextureBuffer<GlesTexture>>>,
    /// Number of label rasterizations (for tests and debugging).
    label_renders: Cell<u64>,
}

/// Canvas title label textures kept per tile (width buckets × focus colours).
const LABEL_CACHE_SIZE: usize = 6;
/// Narrowest on-screen tile (logical px) that still gets a label.
const LABEL_MIN_WIDTH: f64 = 24.;
/// Width buckets: labels are ellipsized to the largest `LABEL_BUCKET_BASE * LABEL_BUCKET_STEP^n`
/// physical px that fits the on-screen tile width, so a zoom animation re-rasterizes a label
/// only when it crosses a bucket (every 25% of width), and never once the bucket is cached.
const LABEL_BUCKET_BASE: f64 = 24.;
const LABEL_BUCKET_STEP: f64 = 1.25;
/// Label padding (logical px): horizontal, vertical; gap between the label and the tile.
const LABEL_PAD_X: f64 = 6.;
const LABEL_PAD_Y: f64 = 2.;
pub const LABEL_GAP: f64 = 4.;

/// Look of a canvas title label.
#[derive(Debug, Clone, Copy)]
pub struct LabelStyle {
    /// Text size in logical px.
    pub size: f64,
    pub text_color: Color,
    pub background_color: Color,
}

#[derive(Debug, Clone, PartialEq)]
struct LabelKey {
    text: String,
    font: String,
    size: f64,
    text_color: Color,
    background_color: Color,
    /// Largest texture width, physical px.
    bucket: i32,
    scale: f64,
}

impl LabelKey {
    /// Whether this is the key for these parameters (no allocations).
    fn matches(&self, text: &str, font: &str, style: LabelStyle, bucket: i32, scale: f64) -> bool {
        self.bucket == bucket
            && self.scale == scale
            && self.size == style.size
            && self.text_color == style.text_color
            && self.background_color == style.background_color
            && self.font == font
            && text
                .chars()
                .map(|c| if c == '\n' { ' ' } else { c })
                .eq(self.text.chars())
    }
}

/// Width bucket (physical px) for a label at most `max_width` physical px wide.
fn label_bucket(max_width: f64) -> Option<i32> {
    if max_width.is_nan() || max_width < LABEL_BUCKET_BASE {
        return None;
    }
    let n = ((max_width / LABEL_BUCKET_BASE).ln() / LABEL_BUCKET_STEP.ln()).floor();
    let bucket = (LABEL_BUCKET_BASE * LABEL_BUCKET_STEP.powf(n)).floor();
    // Guard against rounding at exact bucket boundaries.
    let bucket = if bucket > max_width {
        bucket / LABEL_BUCKET_STEP
    } else {
        bucket
    };
    Some((bucket.floor() as i32).min(MAX_TEXTURE_SIZE))
}

/// Most-recently-used cache of label textures (`T` is generic for tests).
#[derive(Debug)]
struct LabelCache<T> {
    entries: Vec<(LabelKey, Option<T>)>,
}

impl<T> Default for LabelCache<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<T: Clone> LabelCache<T> {
    /// The cached texture for the key that `matches`, else `render(make_key())` (cached too,
    /// failures included, so a failing label is not retried every frame). Returns the texture
    /// and whether it was rendered now.
    fn get(
        &mut self,
        matches: impl Fn(&LabelKey) -> bool,
        make_key: impl FnOnce() -> LabelKey,
        render: impl FnOnce(&LabelKey) -> Option<T>,
    ) -> (Option<T>, bool) {
        if let Some(idx) = self.entries.iter().position(|(key, _)| matches(key)) {
            let entry = self.entries.remove(idx);
            let texture = entry.1.clone();
            self.entries.insert(0, entry);
            return (texture, false);
        }
        let key = make_key();
        let texture = render(&key);
        self.entries.insert(0, (key, texture.clone()));
        self.entries.truncate(LABEL_CACHE_SIZE);
        (texture, true)
    }
}

#[derive(Debug, Default)]
struct TextCache {
    key: Option<TextKey>,
    texture: Option<TextureBuffer<GlesTexture>>,
}

#[derive(Debug, Clone, PartialEq)]
struct TextKey {
    title: String,
    subtitle: String,
    font: String,
    text_color: Color,
    muted_color: Color,
    /// Physical size of the text area.
    width: i32,
    height: i32,
    gap: i32,
    scale: f64,
}

impl TitleBar {
    pub fn new(config: &niri_config::TitleBar, rule: Option<bool>, scale: f64) -> Self {
        let mut rv = Self {
            config: config.clone(),
            enabled: false,
            height: 0.,
            line_width: 0.,
            padding: 0.,
            scale,
            text_scale: scale,
            title: None,
            subtitle: None,
            size: Size::default(),
            alpha: 1.,
            use_rounded: false,
            background: SolidColorBuffer::default(),
            rounded_background: BorderRenderElement::default(),
            line: SolidColorBuffer::default(),
            text: RefCell::default(),
            labels: RefCell::default(),
            label_renders: Cell::new(0),
        };
        rv.update_config(config, rule, scale, None);
        rv
    }

    /// Applies the layout config, the window rule (`title-bar true/false`), the output scale and
    /// the settled canvas zoom (for tiles on a canvas).
    pub fn update_config(
        &mut self,
        config: &niri_config::TitleBar,
        rule: Option<bool>,
        scale: f64,
        canvas_zoom: Option<f64>,
    ) {
        if self.config != *config {
            self.config.clone_from(config);
        }

        self.scale = scale;
        let zoom = canvas_zoom.unwrap_or(1.).clamp(0.05, 4.);
        self.text_scale = scale * zoom;

        self.height = round_logical_in_physical(scale, self.config.height.max(0.));
        self.line_width = round_logical_in_physical_max1(scale, self.config.line_width.max(0.));
        self.line_width = self.line_width.min(self.height);
        self.padding = round_logical_in_physical(scale, self.config.padding.max(0.));
        self.enabled = rule.unwrap_or(!self.config.off) && self.height > 0.;

        self.background.set_color(self.config.background_color);
        self.line.set_color(self.config.line_color);
    }

    /// Height that the strip adds to the tile, if the title bar is enabled.
    ///
    /// This does not depend on the sizing mode; callers hide the strip for fullscreen and
    /// maximized windows.
    pub fn height(&self) -> Option<f64> {
        self.enabled.then_some(self.height)
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn update_shaders(&mut self) {
        self.rounded_background.damage_all();
    }

    pub fn config(&self) -> &niri_config::TitleBar {
        &self.config
    }

    /// Updates the title and subtitle; returns whether they changed.
    pub fn set_text(&mut self, title: Option<String>, subtitle: Option<String>) -> bool {
        if self.title == title && self.subtitle == subtitle {
            return false;
        }
        self.title = title;
        self.subtitle = subtitle;
        true
    }

    /// Prepares the background and line for a strip of `width` (the animated window width).
    ///
    /// `radius` is the radius of the strip's top corners (the window's top radius).
    pub fn update_render_elements(&mut self, width: f64, radius: CornerRadius, alpha: f32) {
        self.size = Size::from((width.max(0.), self.height));
        self.alpha = alpha;

        let radius = CornerRadius {
            top_left: radius.top_left,
            top_right: radius.top_right,
            bottom_right: 0.,
            bottom_left: 0.,
        }
        .fit_to(self.size.w as f32, self.size.h as f32);
        self.use_rounded = radius != CornerRadius::default();

        self.background.resize(self.size);
        let line_size = Size::from((self.size.w, self.line_width));
        self.line.resize(line_size);

        if self.use_rounded {
            let color = self.config.background_color;
            self.rounded_background.update(
                self.size,
                Rectangle::from_size(self.size),
                GradientInterpolation::default(),
                color,
                color,
                0.,
                Rectangle::from_size(self.size),
                0.,
                radius,
                self.scale as f32,
                alpha,
            );
        }
    }

    /// Text area in strip coordinates.
    fn text_area(&self) -> Rectangle<f64, Logical> {
        let w = (self.size.w - self.padding * 2.).max(0.);
        let h = (self.height - self.line_width).max(0.);
        Rectangle::new(Point::from((self.padding, 0.)), Size::from((w, h)))
    }

    /// Renders the strip with its top-left corner at `location`.
    ///
    /// `hide_text` hides the title text (blocked-out windows on screen captures).
    pub fn render<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        location: Point<f64, Logical>,
        hide_text: bool,
        push: &mut dyn FnMut(TitleBarRenderElement),
    ) {
        if !self.enabled || self.size.w <= 0. || self.alpha <= 0. {
            return;
        }

        if !hide_text {
            if let Some(elem) = self.text_element(renderer.as_gles_renderer(), location) {
                push(elem.into());
            }
        }

        if self.line_width > 0. {
            let loc = location + Point::from((0., self.height - self.line_width));
            let elem = SolidColorRenderElement::from_buffer(
                &self.line,
                loc,
                self.alpha,
                Kind::Unspecified,
            );
            push(elem.into());
        }

        if self.use_rounded && BorderRenderElement::has_shader(renderer) {
            push(
                self.rounded_background
                    .clone()
                    .with_location(location)
                    .into(),
            );
        } else {
            let elem = SolidColorRenderElement::from_buffer(
                &self.background,
                location,
                self.alpha,
                Kind::Unspecified,
            );
            push(elem.into());
        }
    }

    fn text_element(
        &self,
        renderer: &mut GlesRenderer,
        location: Point<f64, Logical>,
    ) -> Option<PrimaryGpuTextureRenderElement> {
        let area = self.text_area();

        let mut cache = self.text.borrow_mut();
        // Compare without building a key (no allocations in the per-frame path).
        if !cache
            .key
            .as_ref()
            .is_some_and(|key| self.text_key_matches(key, area))
        {
            let key = self.text_key(area)?;
            cache.texture = render_text(renderer, &key)
                .map_err(|err| trace!("error rendering title bar text: {err:?}"))
                .ok();
            cache.key = Some(key);
        }
        let texture = cache.texture.clone()?;

        // Crop to the current area (it can be narrower during a resize animation).
        let tex_size = texture.logical_size();
        let size = Size::from((tex_size.w.min(area.size.w), tex_size.h.min(area.size.h)));
        if size.w <= 0. || size.h <= 0. {
            return None;
        }
        let loc = location + area.loc;
        let loc = loc
            .to_physical_precise_round(self.scale)
            .to_logical(self.scale);
        let elem = TextureRenderElement::from_texture_buffer(
            texture,
            loc,
            self.alpha,
            Some(Rectangle::from_size(size)),
            None,
            Kind::Unspecified,
        );
        Some(PrimaryGpuTextureRenderElement(elem))
    }

    /// Whether `key` is what `text_key(area)` would return.
    fn text_key_matches(&self, key: &TextKey, area: Rectangle<f64, Logical>) -> bool {
        let clean = |text: &Option<String>, cached: &str| {
            let text = text.as_deref().unwrap_or("");
            text.chars()
                .map(|c| if c == '\n' { ' ' } else { c })
                .eq(cached.chars())
        };
        let scale = self.text_scale;
        key.scale == scale
            && key.width == ((area.size.w * scale).round() as i32).min(MAX_TEXTURE_SIZE)
            && key.height == ((area.size.h * scale).round() as i32).min(MAX_TEXTURE_SIZE)
            && key.gap == (self.padding * scale).round() as i32
            && key.text_color == self.config.text_color
            && key.muted_color == self.config.muted_color
            && key.font == self.config.font
            && clean(&self.title, &key.title)
            && clean(&self.subtitle, &key.subtitle)
    }

    fn text_key(&self, area: Rectangle<f64, Logical>) -> Option<TextKey> {
        let title = self.title.as_deref().unwrap_or("");
        let subtitle = self.subtitle.as_deref().unwrap_or("");
        if title.is_empty() && subtitle.is_empty() {
            return None;
        }

        let scale = self.text_scale;
        let width = (area.size.w * scale).round() as i32;
        let height = (area.size.h * scale).round() as i32;
        if width <= 0 || height <= 0 {
            return None;
        }

        Some(TextKey {
            title: title.replace('\n', " "),
            subtitle: subtitle.replace('\n', " "),
            font: self.config.font.clone(),
            text_color: self.config.text_color,
            muted_color: self.config.muted_color,
            width: width.min(MAX_TEXTURE_SIZE),
            height: height.min(MAX_TEXTURE_SIZE),
            gap: (self.padding * scale).round() as i32,
            scale,
        })
    }

    /// Canvas title label: the title (or the subtitle if there is none) at `style.size` logical
    /// px regardless of the canvas zoom, ellipsized to `max_width` (the on-screen tile width,
    /// logical px), with its bottom-left corner `LABEL_GAP` above `tile_top_left` (view
    /// coordinates). `None` if there is no text or the tile is too narrow.
    ///
    /// Textures are cached per (text, font, size, colours, scale, width bucket), so changing the
    /// zoom re-rasterizes at most when the width crosses into a bucket not seen recently.
    pub fn label_element(
        &self,
        renderer: &mut GlesRenderer,
        style: LabelStyle,
        tile_top_left: Point<f64, Logical>,
        max_width: f64,
        alpha: f32,
    ) -> Option<PrimaryGpuTextureRenderElement> {
        if max_width < LABEL_MIN_WIDTH || alpha <= 0. {
            return None;
        }
        let text = self
            .title
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .or(self.subtitle.as_deref())
            .filter(|t| !t.trim().is_empty())?;
        let scale = self.scale;
        let bucket = label_bucket(max_width * scale)?;

        let font = &self.config.font;
        let matches = |key: &LabelKey| key.matches(text, font, style, bucket, scale);
        let make_key = || LabelKey {
            text: text.replace('\n', " "),
            font: font.clone(),
            size: style.size,
            text_color: style.text_color,
            background_color: style.background_color,
            bucket,
            scale,
        };
        let (texture, rendered) = self.labels.borrow_mut().get(matches, make_key, |key| {
            render_label(renderer, key)
                .map_err(|err| trace!("error rendering canvas title label: {err:?}"))
                .ok()
        });
        if rendered {
            self.label_renders.set(self.label_renders.get() + 1);
        }
        let texture = texture?;

        let size = texture.logical_size();
        let loc = tile_top_left - Point::from((0., LABEL_GAP + size.h));
        let loc = loc.to_physical_precise_round(scale).to_logical(scale);
        let elem = TextureRenderElement::from_texture_buffer(
            texture,
            loc,
            alpha,
            None,
            None,
            Kind::Unspecified,
        );
        Some(PrimaryGpuTextureRenderElement(elem))
    }

    /// How many canvas title labels this strip has rasterized.
    pub fn label_renders(&self) -> u64 {
        self.label_renders.get()
    }

    #[cfg(test)]
    pub fn has_text_texture(&self) -> bool {
        self.text.borrow().texture.is_some()
    }
}

/// Height that the title strip adds to the tile of a window with these rules, for computing
/// window sizes from tile sizes before the tile exists (like the border width there, this is not
/// rounded to physical pixels).
pub fn title_bar_height_for(options: &Options, rules: &ResolvedWindowRules) -> f64 {
    let config = &options.layout.title_bar;
    if rules.title_bar.unwrap_or(!config.off) && config.height > 0. {
        config.height
    } else {
        0.
    }
}

fn set_source(cr: &cairo::Context, color: Color) {
    let [r, g, b, a] = color.to_array_unpremul();
    cr.set_source_rgba(r.into(), g.into(), b.into(), a.into());
}

/// Renders title (left, ellipsized) and subtitle (right, at most half the width) into a texture
/// of exactly the key's physical size.
fn render_text(
    renderer: &mut GlesRenderer,
    key: &TextKey,
) -> anyhow::Result<TextureBuffer<GlesTexture>> {
    let _span = tracy_client::span!("title_bar::render_text");

    let mut font = FontDescription::from_string(&key.font);
    let size = font.size();
    ensure!(size > 0);
    // Font sizes are in logical pixels, like in the rest of niri's UI.
    font.set_absolute_size((f64::from(size) * key.scale).round());

    let (width, height) = (key.width, key.height);
    let surface = ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    let cr = cairo::Context::new(&surface)?;

    let make_layout = |text: &str, max_width: i32| {
        let layout = pangocairo::functions::create_layout(&cr);
        layout.context().set_round_glyph_positions(false);
        layout.set_single_paragraph_mode(true);
        layout.set_font_description(Some(&font));
        layout.set_ellipsize(pango::EllipsizeMode::End);
        layout.set_width(max_width.max(1) * pango::SCALE);
        layout.set_text(text);
        layout
    };

    let center_y = |layout: &pango::Layout| {
        let (_, h) = layout.pixel_size();
        f64::from(((height - h) / 2).max(0))
    };

    let mut subtitle_w = 0;
    if !key.subtitle.is_empty() {
        let layout = make_layout(&key.subtitle, width / 2);
        let (w, _) = layout.pixel_size();
        subtitle_w = w.min(width);
        cr.move_to(f64::from(width - subtitle_w), center_y(&layout));
        set_source(&cr, key.muted_color);
        pangocairo::functions::show_layout(&cr, &layout);
    }

    let title_w = if subtitle_w > 0 {
        width - subtitle_w - key.gap
    } else {
        width
    };
    if !key.title.is_empty() && title_w > 0 {
        let layout = make_layout(&key.title, title_w);
        cr.move_to(0., center_y(&layout));
        set_source(&cr, key.text_color);
        pangocairo::functions::show_layout(&cr, &layout);
    }

    drop(cr);
    let data = surface.take_data().unwrap();
    let buffer = TextureBuffer::from_memory(
        renderer,
        &data,
        Fourcc::Argb8888,
        (width, height),
        false,
        key.scale,
        Transform::Normal,
        Vec::new(),
    )?;

    Ok(buffer)
}

/// Renders a canvas title label: the text on the strip background, padded, ellipsized so that
/// the whole label is at most `key.bucket` physical px wide.
fn render_label(
    renderer: &mut GlesRenderer,
    key: &LabelKey,
) -> anyhow::Result<TextureBuffer<GlesTexture>> {
    let _span = tracy_client::span!("title_bar::render_label");

    let mut font = FontDescription::from_string(&key.font);
    font.set_absolute_size((key.size * key.scale * f64::from(pango::SCALE)).round());

    let pad_x = (LABEL_PAD_X * key.scale).round() as i32;
    let pad_y = (LABEL_PAD_Y * key.scale).round() as i32;
    let max_text_w = key.bucket - 2 * pad_x;
    ensure!(max_text_w > 0);

    // Measure on a scratch surface, then draw at the exact size.
    let measure = ImageSurface::create(cairo::Format::ARgb32, 1, 1)?;
    let cr = cairo::Context::new(&measure)?;
    let layout = pangocairo::functions::create_layout(&cr);
    layout.context().set_round_glyph_positions(false);
    layout.set_single_paragraph_mode(true);
    layout.set_font_description(Some(&font));
    layout.set_ellipsize(pango::EllipsizeMode::End);
    layout.set_width(max_text_w * pango::SCALE);
    layout.set_text(&key.text);
    let (text_w, text_h) = layout.pixel_size();
    drop(cr);

    let width = (text_w.min(max_text_w) + 2 * pad_x).clamp(1, MAX_TEXTURE_SIZE);
    let height = (text_h + 2 * pad_y).clamp(1, MAX_TEXTURE_SIZE);
    let surface = ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    let cr = cairo::Context::new(&surface)?;
    set_source(&cr, key.background_color);
    cr.paint()?;
    pangocairo::functions::update_layout(&cr, &layout);
    cr.move_to(f64::from(pad_x), f64::from(pad_y));
    set_source(&cr, key.text_color);
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
        Transform::Normal,
        Vec::new(),
    )?;
    Ok(buffer)
}

#[cfg(test)]
mod label_tests {
    use super::*;

    #[test]
    fn label_buckets() {
        assert_eq!(label_bucket(10.), None);
        assert_eq!(label_bucket(23.9), None);
        assert_eq!(label_bucket(24.), Some(24));
        assert_eq!(label_bucket(29.), Some(24));
        assert_eq!(label_bucket(30.), Some(30));
        // Monotonic, never wider than the space, at most 25% narrower.
        let mut last = 0;
        for w in 24..4000 {
            let b = label_bucket(f64::from(w)).unwrap();
            assert!(b >= last);
            assert!(b <= w);
            assert!(f64::from(b) * LABEL_BUCKET_STEP >= f64::from(w) - 1.);
            last = b;
        }
        // A zoom animation from 0.7 to 0.4 of an 800 px wide tile crosses few buckets.
        let buckets: std::collections::BTreeSet<_> = (0..=300)
            .map(|i| label_bucket(800. * (0.7 - 0.3 * f64::from(i) / 300.)).unwrap())
            .collect();
        assert!(buckets.len() <= 4, "{buckets:?}");
    }

    fn style() -> LabelStyle {
        LabelStyle {
            size: 12.,
            text_color: Color::from_rgba8_unpremul(255, 255, 255, 255),
            background_color: Color::from_rgba8_unpremul(0, 0, 0, 255),
        }
    }

    /// Looks up the label of an 800 px wide tile at `zoom` the way `label_element` does;
    /// returns whether it was rasterized.
    fn frame(cache: &mut LabelCache<()>, text: &str, zoom: f64, style: LabelStyle) -> bool {
        let bucket = label_bucket(800. * zoom).unwrap();
        let font = "sans 11";
        let (_, rendered) = cache.get(
            |key| key.matches(text, font, style, bucket, 1.),
            || LabelKey {
                text: text.to_owned(),
                font: font.to_owned(),
                size: style.size,
                text_color: style.text_color,
                background_color: style.background_color,
                bucket,
                scale: 1.,
            },
            |_| Some(()),
        );
        rendered
    }

    #[test]
    fn zoom_animation_does_not_rasterize_per_frame() {
        let mut cache = LabelCache::default();
        // A 180 Hz zoom animation from 0.7 to 0.4 (half a second).
        let zooms: Vec<f64> = (0..=90).map(|i| 0.7 - 0.3 * f64::from(i) / 90.).collect();
        let first: usize = zooms
            .iter()
            .filter(|z| frame(&mut cache, "foot", **z, style()))
            .count();
        assert!(first <= 4, "{first} rasterizations");
        // Back and forth again: everything is cached.
        let again: usize = zooms
            .iter()
            .rev()
            .chain(zooms.iter())
            .filter(|z| frame(&mut cache, "foot", **z, style()))
            .count();
        assert_eq!(again, 0);
        // A settled zoom costs nothing per frame.
        assert!(!(0..100).any(|_| frame(&mut cache, "foot", 0.4, style())));
        // Focus colour and title changes do rasterize.
        let accent = LabelStyle {
            text_color: Color::from_rgba8_unpremul(224, 164, 88, 255),
            ..style()
        };
        assert!(frame(&mut cache, "foot", 0.4, accent));
        assert!(frame(&mut cache, "vim", 0.4, accent));
        assert!(!frame(&mut cache, "vim", 0.4, accent));
    }
}
