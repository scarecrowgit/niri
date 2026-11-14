use knuffel::errors::DecodeError;
use niri_ipc::{ColumnDisplay, SizeChange};

use crate::appearance::{
    Border, FocusRing, InsertHint, Shadow, TabIndicator, DEFAULT_BACKGROUND_COLOR,
};
use crate::canvas::{Canvas, CanvasPart, WorkspaceMode};
use crate::decorations::{TitleBar, TitleBarPart, WorkArea, WorkAreaPart};
use crate::utils::{expect_only_children, Flag, MergeWith};
use crate::{BorderRule, Color, FloatOrInt, InsertHintPart, ShadowRule, TabIndicatorPart};

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub focus_ring: FocusRing,
    pub border: Border,
    pub shadow: Shadow,
    pub tab_indicator: TabIndicator,
    pub insert_hint: InsertHint,
    pub preset_column_widths: Vec<PresetSize>,
    pub default_column_width: Option<PresetSize>,
    pub preset_window_heights: Vec<PresetSize>,
    pub center_focused_column: CenterFocusedColumn,
    pub always_center_single_column: bool,
    pub empty_workspace_above_first: bool,
    pub default_column_display: ColumnDisplay,
    pub gaps: f64,
    pub struts: Struts,
    pub background_color: Color,
    pub fullscreen_backdrop_color: Color,
    // ashd: infinite canvas workspace mode.
    pub default_workspace_mode: WorkspaceMode,
    pub canvas: Canvas,
    // ashd: compositor-drawn title strips.
    pub title_bar: TitleBar,
    // ashd: working area outline and clip.
    pub work_area: WorkArea,
    /// ashd: index references past the last workspace create workspaces up to that index, and
    /// empty workspaces below a used one are kept (only trailing empty ones are removed).
    pub create_workspaces_by_index: bool,
    /// ashd: a fullscreen window gets a workspace of its own, right after the one it came from.
    pub fullscreen_workspaces: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            focus_ring: FocusRing::default(),
            border: Border::default(),
            shadow: Shadow::default(),
            tab_indicator: TabIndicator::default(),
            insert_hint: InsertHint::default(),
            preset_column_widths: vec![
                PresetSize::Proportion(1. / 3.),
                PresetSize::Proportion(0.5),
                PresetSize::Proportion(2. / 3.),
            ],
            default_column_width: Some(PresetSize::Proportion(0.5)),
            center_focused_column: CenterFocusedColumn::Never,
            always_center_single_column: false,
            empty_workspace_above_first: false,
            default_column_display: ColumnDisplay::Normal,
            gaps: 16.,
            struts: Struts::default(),
            preset_window_heights: vec![
                PresetSize::Proportion(1. / 3.),
                PresetSize::Proportion(0.5),
                PresetSize::Proportion(2. / 3.),
            ],
            background_color: DEFAULT_BACKGROUND_COLOR,
            fullscreen_backdrop_color: Color::from_array_unpremul([0., 0., 0., 1.]),
            default_workspace_mode: WorkspaceMode::default(),
            canvas: Canvas::default(),
            title_bar: TitleBar::default(),
            work_area: WorkArea::default(),
            create_workspaces_by_index: false,
            fullscreen_workspaces: false,
        }
    }
}

impl MergeWith<LayoutPart> for Layout {
    fn merge_with(&mut self, part: &LayoutPart) {
        merge!(
            (self, part),
            focus_ring,
            border,
            shadow,
            tab_indicator,
            insert_hint,
            always_center_single_column,
            empty_workspace_above_first,
            gaps,
            canvas,
            title_bar, // ashd
            work_area, // ashd
            struts,    // ashd: per side
            // ashd: numbered workspaces created on demand.
            create_workspaces_by_index,
            // ashd: a workspace per fullscreen window.
            fullscreen_workspaces,
        );

        merge_clone!(
            (self, part),
            preset_column_widths,
            preset_window_heights,
            center_focused_column,
            default_column_display,
            background_color,
            fullscreen_backdrop_color,
            default_workspace_mode,
        );

        if let Some(x) = part.default_column_width {
            self.default_column_width = x.0;
        }

        if self.preset_column_widths.is_empty() {
            self.preset_column_widths = Layout::default().preset_column_widths;
        }

        if self.preset_window_heights.is_empty() {
            self.preset_window_heights = Layout::default().preset_window_heights;
        }
    }
}

#[derive(knuffel::Decode, Debug, Default, Clone, PartialEq)]
pub struct LayoutPart {
    #[knuffel(child)]
    pub focus_ring: Option<BorderRule>,
    #[knuffel(child)]
    pub border: Option<BorderRule>,
    #[knuffel(child)]
    pub shadow: Option<ShadowRule>,
    #[knuffel(child)]
    pub tab_indicator: Option<TabIndicatorPart>,
    #[knuffel(child)]
    pub insert_hint: Option<InsertHintPart>,
    #[knuffel(child, unwrap(children))]
    pub preset_column_widths: Option<Vec<PresetSize>>,
    #[knuffel(child)]
    pub default_column_width: Option<DefaultPresetSize>,
    #[knuffel(child, unwrap(children))]
    pub preset_window_heights: Option<Vec<PresetSize>>,
    #[knuffel(child, unwrap(argument))]
    pub center_focused_column: Option<CenterFocusedColumn>,
    #[knuffel(child)]
    pub always_center_single_column: Option<Flag>,
    #[knuffel(child)]
    pub empty_workspace_above_first: Option<Flag>,
    #[knuffel(child, unwrap(argument, str))]
    pub default_column_display: Option<ColumnDisplay>,
    #[knuffel(child, unwrap(argument))]
    pub gaps: Option<FloatOrInt<0, 65535>>,
    // ashd: merged per side.
    #[knuffel(child)]
    pub struts: Option<StrutsPart>,
    #[knuffel(child)]
    pub background_color: Option<Color>,
    #[knuffel(child)]
    pub fullscreen_backdrop_color: Option<Color>,
    // ashd: infinite canvas workspace mode.
    #[knuffel(child, unwrap(argument, str))]
    pub default_workspace_mode: Option<WorkspaceMode>,
    #[knuffel(child)]
    pub canvas: Option<CanvasPart>,
    // ashd: compositor-drawn title strips.
    #[knuffel(child)]
    pub title_bar: Option<TitleBarPart>,
    // ashd: working area outline and clip.
    #[knuffel(child)]
    pub work_area: Option<WorkAreaPart>,
    // ashd: numbered workspaces created on demand.
    #[knuffel(child)]
    pub create_workspaces_by_index: Option<Flag>,
    // ashd: a workspace per fullscreen window.
    #[knuffel(child)]
    pub fullscreen_workspaces: Option<Flag>,
}

#[derive(knuffel::Decode, Debug, Clone, Copy, PartialEq)]
pub enum PresetSize {
    Proportion(#[knuffel(argument)] f64),
    Fixed(#[knuffel(argument)] i32),
}

impl From<PresetSize> for SizeChange {
    fn from(value: PresetSize) -> Self {
        match value {
            PresetSize::Proportion(prop) => SizeChange::SetProportion(prop * 100.),
            PresetSize::Fixed(fixed) => SizeChange::SetFixed(fixed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DefaultPresetSize(pub Option<PresetSize>);

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq)]
pub struct Struts {
    #[knuffel(child, unwrap(argument), default)]
    pub left: FloatOrInt<-65535, 65535>,
    #[knuffel(child, unwrap(argument), default)]
    pub right: FloatOrInt<-65535, 65535>,
    #[knuffel(child, unwrap(argument), default)]
    pub top: FloatOrInt<-65535, 65535>,
    #[knuffel(child, unwrap(argument), default)]
    pub bottom: FloatOrInt<-65535, 65535>,
}

/// ashd: one `struts {}` section as parsed from the config.
///
/// Struts merge per side: a later `struts {}` block (another included file, or a per-output or
/// per-workspace `layout {}`) only overrides the sides it names, so a theme file can set `top`
/// without losing a host-configured `left`.
#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq)]
pub struct StrutsPart {
    #[knuffel(child, unwrap(argument))]
    pub left: Option<FloatOrInt<-65535, 65535>>,
    #[knuffel(child, unwrap(argument))]
    pub right: Option<FloatOrInt<-65535, 65535>>,
    #[knuffel(child, unwrap(argument))]
    pub top: Option<FloatOrInt<-65535, 65535>>,
    #[knuffel(child, unwrap(argument))]
    pub bottom: Option<FloatOrInt<-65535, 65535>>,
}

impl From<Struts> for StrutsPart {
    fn from(struts: Struts) -> Self {
        Self {
            left: Some(struts.left),
            right: Some(struts.right),
            top: Some(struts.top),
            bottom: Some(struts.bottom),
        }
    }
}

impl MergeWith<StrutsPart> for Struts {
    fn merge_with(&mut self, part: &StrutsPart) {
        if let Some(x) = part.left {
            self.left = x;
        }
        if let Some(x) = part.right {
            self.right = x;
        }
        if let Some(x) = part.top {
            self.top = x;
        }
        if let Some(x) = part.bottom {
            self.bottom = x;
        }
    }
}

#[derive(knuffel::DecodeScalar, Debug, Default, PartialEq, Eq, Clone, Copy)]
pub enum CenterFocusedColumn {
    /// Focusing a column will not center the column.
    #[default]
    Never,
    /// The focused column will always be centered.
    Always,
    /// Focusing a column will center it if it doesn't fit on the screen together with the
    /// previously focused column.
    OnOverflow,
}

impl<S> knuffel::Decode<S> for DefaultPresetSize
where
    S: knuffel::traits::ErrorSpan,
{
    fn decode_node(
        node: &knuffel::ast::SpannedNode<S>,
        ctx: &mut knuffel::decode::Context<S>,
    ) -> Result<Self, DecodeError<S>> {
        expect_only_children(node, ctx);

        let mut children = node.children();

        if let Some(child) = children.next() {
            if let Some(unwanted_child) = children.next() {
                ctx.emit_error(DecodeError::unexpected(
                    unwanted_child,
                    "node",
                    "expected no more than one child",
                ));
            }
            PresetSize::decode_node(child, ctx).map(Some).map(Self)
        } else {
            Ok(Self(None))
        }
    }
}

// ashd: struts merge per side.
#[cfg(test)]
mod struts_tests {
    use std::path::PathBuf;

    use super::*;
    use crate::Config;

    fn struts(left: f64, right: f64, top: f64, bottom: f64) -> Struts {
        Struts {
            left: FloatOrInt(left),
            right: FloatOrInt(right),
            top: FloatOrInt(top),
            bottom: FloatOrInt(bottom),
        }
    }

    /// Writes `files` into a fresh directory and parses the first one.
    fn parse_files(name: &str, files: &[(&str, &str)]) -> Config {
        let dir: PathBuf =
            std::env::temp_dir().join(format!("niri-config-struts-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (file, text) in files {
            std::fs::write(dir.join(file), text).unwrap();
        }
        let path = dir.join(files[0].0);
        let text = std::fs::read_to_string(&path).unwrap();
        let config = Config::parse(&path, &text).config.unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        config
    }

    #[test]
    fn struts_merge_per_side_across_includes() {
        // The host sets left and top; the theme fragment, included after, sets only top.
        let config = parse_files(
            "include",
            &[
                (
                    "config.kdl",
                    r#"
                    layout {
                        struts {
                            left 64
                            top 46
                        }
                    }
                    include "theme.kdl"
                    "#,
                ),
                (
                    "theme.kdl",
                    r#"
                    layout {
                        struts {
                            top 58
                        }
                    }
                    "#,
                ),
            ],
        );
        assert_eq!(config.layout.struts, struts(64., 0., 58., 0.));
    }

    #[test]
    fn struts_in_one_block_and_defaults() {
        let config = Config::parse_mem("").unwrap();
        assert_eq!(config.layout.struts, Struts::default());

        let config = Config::parse_mem(
            r#"
            layout {
                struts {
                    left 1
                    right 2
                    top 3
                    bottom -4
                }
            }
            "#,
        )
        .unwrap();
        assert_eq!(config.layout.struts, struts(1., 2., 3., -4.));
    }

    #[test]
    fn struts_overrides_merge_per_side() {
        let mut layout = Layout::default();
        layout.merge_with(&LayoutPart {
            struts: Some(StrutsPart::from(struts(10., 20., 30., 40.))),
            ..Default::default()
        });
        // Like a per-output `layout { struts { top 0 } }`.
        layout.merge_with(&LayoutPart {
            struts: Some(StrutsPart {
                top: Some(FloatOrInt(0.)),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert_eq!(layout.struts, struts(10., 20., 0., 40.));
    }
}
