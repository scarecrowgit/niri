//! rice: compositor-drawn window decorations and canvas plane decorations.
//!
//! Everything here is off unless configured, so stock configs render exactly like upstream.
//!
//! ```kdl
//! layout {
//!     // Title strip above every window (off unless this block is present).
//!     title-bar {
//!         height 28
//!         font "JetBrains Mono 11"
//!         padding 12
//!         background-color "#1B1B19"
//!         text-color "#DAD7CF"
//!         muted-color "#96938B"
//!         line-color "#2A2A27"
//!         line-width 1
//!     }
//!
//!     focus-ring {
//!         // Space between the window and the ring.
//!         gap 4
//!         // On canvas workspaces: "ring" (default), "border" (no ring, the window border takes
//!         // the ring's active colour) or "off".
//!         canvas-style "border"
//!     }
//!
//!     canvas {
//!         grid "lines" // "lines", "dots" or "off" (default)
//!         grid-spacing 80
//!         grid-color "#161615"
//!         grid-line-width 1
//!         background-color "#111110"
//!         region-color "#2A2A27"
//!         region-label-color "#96938B"
//!         region-font "JetBrains Mono 16"
//!         region "dots" x=740 y=320 w=1740 h=1210
//!     }
//! }
//!
//! window-rule {
//!     match app-id="firefox"
//!     title-bar false
//! }
//! ```

use knuffel::errors::DecodeError;

use crate::appearance::Color;
use crate::utils::MergeWith;
use crate::FloatOrInt;

/// Final title bar config.
#[derive(Debug, Clone, PartialEq)]
pub struct TitleBar {
    pub off: bool,
    /// Height of the strip in logical pixels, including the bottom line.
    pub height: f64,
    /// Pango font description; the size is in logical pixels.
    pub font: String,
    /// Horizontal padding in logical pixels.
    pub padding: f64,
    /// Width of the bottom line in logical pixels (0 disables it).
    pub line_width: f64,
    pub background_color: Color,
    pub text_color: Color,
    pub muted_color: Color,
    pub line_color: Color,
}

impl Default for TitleBar {
    fn default() -> Self {
        Self {
            off: true,
            height: 28.,
            font: String::from("monospace 11"),
            padding: 12.,
            line_width: 1.,
            background_color: Color::from_rgba8_unpremul(0x1b, 0x1b, 0x19, 0xff),
            text_color: Color::from_rgba8_unpremul(0xda, 0xd7, 0xcf, 0xff),
            muted_color: Color::from_rgba8_unpremul(0x96, 0x93, 0x8b, 0xff),
            line_color: Color::from_rgba8_unpremul(0x2a, 0x2a, 0x27, 0xff),
        }
    }
}

impl TitleBar {
    /// Whether two configs give tiles different sizes.
    pub fn geometry_differs(&self, other: &Self) -> bool {
        self.off != other.off || self.height != other.height
    }
}

/// One `title-bar {}` section as parsed from the config.
#[derive(knuffel::Decode, Debug, Default, Clone, PartialEq)]
pub struct TitleBarPart {
    #[knuffel(child)]
    pub off: bool,
    #[knuffel(child)]
    pub on: bool,
    #[knuffel(child, unwrap(argument))]
    pub height: Option<FloatOrInt<0, 1024>>,
    #[knuffel(child, unwrap(argument))]
    pub font: Option<String>,
    #[knuffel(child, unwrap(argument))]
    pub padding: Option<FloatOrInt<0, 1024>>,
    #[knuffel(child, unwrap(argument))]
    pub line_width: Option<FloatOrInt<0, 1024>>,
    #[knuffel(child)]
    pub background_color: Option<Color>,
    #[knuffel(child)]
    pub text_color: Option<Color>,
    #[knuffel(child)]
    pub muted_color: Option<Color>,
    #[knuffel(child)]
    pub line_color: Option<Color>,
}

impl MergeWith<TitleBarPart> for TitleBar {
    fn merge_with(&mut self, part: &TitleBarPart) {
        // The presence of a `title-bar {}` block turns the title bar on, unless it says `off`.
        self.off = part.off && !part.on;

        if let Some(x) = part.height {
            self.height = x.0;
        }
        if let Some(x) = &part.font {
            self.font.clone_from(x);
        }
        if let Some(x) = part.padding {
            self.padding = x.0;
        }
        if let Some(x) = part.line_width {
            self.line_width = x.0;
        }
        if let Some(x) = part.background_color {
            self.background_color = x;
        }
        if let Some(x) = part.text_color {
            self.text_color = x;
        }
        if let Some(x) = part.muted_color {
            self.muted_color = x;
        }
        if let Some(x) = part.line_color {
            self.line_color = x;
        }
    }
}

/// How the focus is shown on canvas workspaces.
#[derive(knuffel::DecodeScalar, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum FocusRingCanvasStyle {
    /// Same focus ring as on strip workspaces.
    #[default]
    Ring,
    /// No focus ring; the window border takes the focus ring's active colour.
    Border,
    /// No focus ring.
    Off,
}

/// Background pattern of the canvas plane.
#[derive(knuffel::DecodeScalar, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CanvasGrid {
    #[default]
    Off,
    Lines,
    Dots,
}

/// A named rectangle on the canvas plane, in plane coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasRegion {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Final canvas plane decoration config (part of `layout { canvas {} }`).
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasDecor {
    pub grid: CanvasGrid,
    /// Distance between grid lines (or dots) on the plane, in logical plane pixels.
    pub grid_spacing: f64,
    pub grid_color: Color,
    /// Line width (lines) or dot diameter (dots) in logical view pixels.
    pub grid_line_width: f64,
    /// Fill colour of the plane under the grid; `None` leaves the backdrop visible.
    pub background_color: Option<Color>,
    /// Colour of the dashed region outlines.
    pub region_color: Color,
    pub region_label_color: Color,
    /// Pango font description of region labels; the size is in logical plane pixels.
    pub region_font: String,
    pub regions: Vec<CanvasRegion>,
}

impl Default for CanvasDecor {
    fn default() -> Self {
        Self {
            grid: CanvasGrid::Off,
            grid_spacing: 80.,
            grid_color: Color::from_rgba8_unpremul(0x16, 0x16, 0x15, 0xff),
            grid_line_width: 1.,
            background_color: None,
            region_color: Color::from_rgba8_unpremul(0x2a, 0x2a, 0x27, 0xff),
            region_label_color: Color::from_rgba8_unpremul(0x96, 0x93, 0x8b, 0xff),
            region_font: String::from("monospace 16"),
            regions: Vec::new(),
        }
    }
}

/// Canvas decoration keys as parsed from a `canvas {}` section (flattened into it).
#[derive(knuffel::Decode, Debug, Default, Clone, PartialEq)]
pub struct CanvasDecorPart {
    #[knuffel(child, unwrap(argument))]
    pub grid: Option<CanvasGrid>,
    #[knuffel(child, unwrap(argument))]
    pub grid_spacing: Option<FloatOrInt<1, 65535>>,
    #[knuffel(child)]
    pub grid_color: Option<Color>,
    #[knuffel(child, unwrap(argument))]
    pub grid_line_width: Option<FloatOrInt<0, 1024>>,
    #[knuffel(child)]
    pub background_color: Option<Color>,
    #[knuffel(child)]
    pub region_color: Option<Color>,
    #[knuffel(child)]
    pub region_label_color: Option<Color>,
    #[knuffel(child, unwrap(argument))]
    pub region_font: Option<String>,
    #[knuffel(flatten(child))]
    pub regions: CanvasRegionsPart,
}

impl MergeWith<CanvasDecorPart> for CanvasDecor {
    fn merge_with(&mut self, part: &CanvasDecorPart) {
        if let Some(x) = part.grid {
            self.grid = x;
        }
        if let Some(x) = part.grid_spacing {
            self.grid_spacing = x.0;
        }
        if let Some(x) = part.grid_color {
            self.grid_color = x;
        }
        if let Some(x) = part.grid_line_width {
            self.grid_line_width = x.0;
        }
        if part.background_color.is_some() {
            self.background_color = part.background_color;
        }
        if let Some(x) = part.region_color {
            self.region_color = x;
        }
        if let Some(x) = part.region_label_color {
            self.region_label_color = x;
        }
        if let Some(x) = &part.region_font {
            self.region_font.clone_from(x);
        }
        // Regions of a more specific section (output, named workspace) replace the others.
        if !part.regions.0.is_empty() {
            self.regions.clone_from(&part.regions.0);
        }
    }
}

/// `region "name" x=.. y=.. w=.. h=..` nodes of a `canvas {}` section.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct CanvasRegionsPart(pub Vec<CanvasRegion>);

#[derive(knuffel::Decode)]
struct CanvasRegionNode {
    #[knuffel(argument)]
    name: String,
    #[knuffel(property)]
    x: FloatOrInt<-1000000000, 1000000000>,
    #[knuffel(property)]
    y: FloatOrInt<-1000000000, 1000000000>,
    #[knuffel(property)]
    w: FloatOrInt<1, 1000000000>,
    #[knuffel(property)]
    h: FloatOrInt<1, 1000000000>,
}

impl<S> knuffel::traits::DecodePartial<S> for CanvasRegionsPart
where
    S: knuffel::traits::ErrorSpan,
{
    fn insert_child(
        &mut self,
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<bool, DecodeError<S>> {
        if &**node.node_name != "region" {
            return Ok(false);
        }

        let region = <CanvasRegionNode as knuffel::Decode<S>>::decode_node(node, ctx)?;
        let region = CanvasRegion {
            name: region.name,
            x: region.x.0,
            y: region.y.0,
            w: region.w.0,
            h: region.h.0,
        };
        // A later region with the same name replaces an earlier one.
        self.0.retain(|r| r.name != region.name);
        self.0.push(region);
        Ok(true)
    }

    fn insert_property(
        &mut self,
        _name: &knuffel::span::Spanned<Box<str>, S>,
        _value: &knuffel::ast::Value<S>,
        _ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<bool, DecodeError<S>> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, FocusRing};

    #[test]
    fn decorations_off_by_default() {
        let config = Config::parse_mem("").unwrap();
        assert!(config.layout.title_bar.off);
        assert_eq!(config.layout.title_bar, TitleBar::default());
        assert_eq!(config.layout.focus_ring.gap, 0.);
        assert_eq!(
            config.layout.focus_ring.canvas_style,
            FocusRingCanvasStyle::Ring
        );
        assert_eq!(config.layout.canvas.decor, CanvasDecor::default());
        assert_eq!(config.layout.canvas.decor.grid, CanvasGrid::Off);
    }

    #[test]
    fn parse_title_bar() {
        let config = Config::parse_mem(
            r##"
            layout {
                title-bar {
                    height 30
                    font "JetBrains Mono 11"
                    padding 10
                    line-width 2
                    background-color "#1B1B19"
                    text-color "#DAD7CF"
                    muted-color "#96938B"
                    line-color "#2A2A27"
                }
            }
            "##,
        )
        .unwrap();

        let title_bar = &config.layout.title_bar;
        assert!(!title_bar.off);
        assert_eq!(title_bar.height, 30.);
        assert_eq!(title_bar.font, "JetBrains Mono 11");
        assert_eq!(title_bar.padding, 10.);
        assert_eq!(title_bar.line_width, 2.);
        assert_eq!(
            title_bar.background_color,
            Color::from_rgba8_unpremul(0x1b, 0x1b, 0x19, 0xff)
        );
        assert_eq!(
            title_bar.text_color,
            Color::from_rgba8_unpremul(0xda, 0xd7, 0xcf, 0xff)
        );
    }

    #[test]
    fn empty_title_bar_block_turns_it_on() {
        let config = Config::parse_mem("layout { title-bar; }").unwrap();
        assert!(!config.layout.title_bar.off);
        assert_eq!(config.layout.title_bar.height, 28.);

        let config = Config::parse_mem("layout { title-bar { off; }; }").unwrap();
        assert!(config.layout.title_bar.off);
    }

    #[test]
    fn title_bar_window_rule() {
        let config = Config::parse_mem(
            r#"
            window-rule {
                match app-id="firefox"
                title-bar false
            }
            "#,
        )
        .unwrap();
        assert_eq!(config.window_rules[0].title_bar, Some(false));
    }

    #[test]
    fn parse_focus_ring_gap() {
        let config = Config::parse_mem(
            r##"
            layout {
                focus-ring {
                    width 1
                    gap 4
                    canvas-style "border"
                    active-color "#E0A458"
                }
            }
            "##,
        )
        .unwrap();

        let ring = config.layout.focus_ring;
        assert_eq!(ring.width, 1.);
        assert_eq!(ring.gap, 4.);
        assert_eq!(ring.canvas_style, FocusRingCanvasStyle::Border);
        assert_eq!(
            ring.active_color,
            Color::from_rgba8_unpremul(0xe0, 0xa4, 0x58, 0xff)
        );

        // Merging a window rule keeps the gap and the canvas style.
        let mut merged = ring;
        merged.merge_with(&crate::BorderRule {
            width: Some(FloatOrInt(2.)),
            ..Default::default()
        });
        assert_eq!(merged, FocusRing { width: 2., ..ring });
        merged.merge_with(&crate::BorderRule {
            gap: Some(FloatOrInt(8.)),
            ..Default::default()
        });
        assert_eq!(merged.gap, 8.);
    }

    #[test]
    fn parse_canvas_decor() {
        let config = Config::parse_mem(
            r##"
            layout {
                canvas {
                    default-zoom 0.5
                    grid "dots"
                    grid-spacing 40
                    grid-color "#2A2A27"
                    grid-line-width 2
                    background-color "#111110"
                    region-color "#2A2A27"
                    region-label-color "#96938B"
                    region-font "JetBrains Mono 16"
                    region "dots" x=740 y=320 w=1740 h=1210
                    region "research" x=-140 y=260.5 w=600 h=860
                    region "dots" x=0 y=0 w=10 h=10
                }
            }
            "##,
        )
        .unwrap();

        let canvas = &config.layout.canvas;
        assert_eq!(canvas.default_zoom, 0.5);
        let decor = &canvas.decor;
        assert_eq!(decor.grid, CanvasGrid::Dots);
        assert_eq!(decor.grid_spacing, 40.);
        assert_eq!(decor.grid_line_width, 2.);
        assert_eq!(
            decor.background_color,
            Some(Color::from_rgba8_unpremul(0x11, 0x11, 0x10, 0xff))
        );
        assert_eq!(decor.region_font, "JetBrains Mono 16");
        assert_eq!(
            decor.regions,
            vec![
                CanvasRegion {
                    name: String::from("research"),
                    x: -140.,
                    y: 260.5,
                    w: 600.,
                    h: 860.,
                },
                CanvasRegion {
                    name: String::from("dots"),
                    x: 0.,
                    y: 0.,
                    w: 10.,
                    h: 10.,
                },
            ]
        );
    }

    #[test]
    fn canvas_region_requires_size() {
        assert!(
            Config::parse_mem(r#"layout { canvas { region "a" x=0 y=0 w=0 h=1; }; }"#).is_err()
        );
        assert!(Config::parse_mem(r#"layout { canvas { region "a" x=0 y=0; }; }"#).is_err());
        assert!(Config::parse_mem(r#"layout { canvas { grid "squares"; }; }"#).is_err());
    }

    #[test]
    fn named_workspace_regions_replace_global_ones() {
        let mut decor = CanvasDecor::default();
        decor.merge_with(&CanvasDecorPart {
            regions: CanvasRegionsPart(vec![CanvasRegion {
                name: String::from("a"),
                x: 0.,
                y: 0.,
                w: 1.,
                h: 1.,
            }]),
            ..Default::default()
        });
        decor.merge_with(&CanvasDecorPart {
            grid: Some(CanvasGrid::Lines),
            ..Default::default()
        });
        assert_eq!(decor.regions.len(), 1);
        decor.merge_with(&CanvasDecorPart {
            regions: CanvasRegionsPart(vec![CanvasRegion {
                name: String::from("b"),
                x: 0.,
                y: 0.,
                w: 1.,
                h: 1.,
            }]),
            ..Default::default()
        });
        assert_eq!(decor.regions.len(), 1);
        assert_eq!(decor.regions[0].name, "b");
        assert_eq!(decor.grid, CanvasGrid::Lines);
    }
}
