//! ashd: edge layer surfaces revealed over a fullscreen window (`edge-reveal { on; }`).
//!
//! A fullscreen window is drawn above the top layer, so panels (top-layer layer-shell surfaces
//! anchored to one screen edge with an exclusive zone, e.g. a bar with a dock or a side rail) are
//! covered. With `edge-reveal`:
//!
//! - resting the pointer against an output edge for `dwell-ms` slides that edge's panels in above
//!   the fullscreen window, where they take pointer input as usual;
//! - they slide out `hide-delay-ms` after the pointer left them (and the edge), unless a layer
//!   surface has the keyboard focus or the pointer (a popup the panel opened, say);
//! - nothing triggers while the focused client holds an active pointer constraint (a game's locked
//!   or confined pointer), while a pointer button is held or a grab is active, or while the lock
//!   screen, the screenshot UI or the window switcher is up;
//! - the panels also slide out when a fullscreen window starts covering them and slide in when it
//!   stops (instead of popping), so entering and leaving fullscreen is one motion.
//!
//! Dwell was chosen over a pressure threshold: the pointer position is clamped to the output, so
//! "pushing past the edge" only shows up as relative motion that some devices (tablets, touch,
//! absolute pointers) don't produce; resting at the edge works for all of them. A deliberate push
//! reaches the edge and stays there; a graze leaves before the dwell is over.
//!
//! [`EdgeTrigger`] is the per-edge state machine as plain data; [`EdgeRevealState`] holds it per
//! output together with the slide animations and the per-frame placements that rendering and
//! hit-testing read.

use std::collections::HashMap;
use std::time::Duration;

use smithay::desktop::{layer_map_for_output, LayerSurface};
use smithay::output::Output;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::RegistrationToken;
use smithay::utils::{IsAlive, Logical, Point, Rectangle, Size};
use smithay::wayland::pointer_constraints::with_pointer_constraint;
use smithay::wayland::shell::wlr_layer::{Anchor, ExclusiveZone, Layer};

use crate::animation::{Animation, Clock};
use crate::niri::{KeyboardFocus, Niri, State};

/// A screen edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    pub const ALL: [Edge; 4] = [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom];

    fn idx(self) -> usize {
        self as usize
    }

    /// The edge a layer surface with these anchors belongs to: anchored to it and not to the
    /// opposite edge; corner-anchored surfaces (two candidate edges) belong to none.
    pub fn from_anchor(anchor: Anchor) -> Option<Edge> {
        let mut found = None;
        for (edge, this, opposite) in [
            (Edge::Left, Anchor::LEFT, Anchor::RIGHT),
            (Edge::Right, Anchor::RIGHT, Anchor::LEFT),
            (Edge::Top, Anchor::TOP, Anchor::BOTTOM),
            (Edge::Bottom, Anchor::BOTTOM, Anchor::TOP),
        ] {
            if anchor.contains(this) && !anchor.contains(opposite) {
                if found.is_some() {
                    return None;
                }
                found = Some(edge);
            }
        }
        found
    }

    /// Whether `pos` (output-local) rests against this edge of an output of `size`.
    pub fn touches(self, pos: Point<f64, Logical>, size: Size<f64, Logical>) -> bool {
        match self {
            Edge::Left => pos.x < 1.,
            Edge::Right => pos.x >= size.w - 1.,
            Edge::Top => pos.y < 1.,
            Edge::Bottom => pos.y >= size.h - 1.,
        }
    }

    /// Offset that moves a surface with geometry `geo` (output-local) fully past this edge,
    /// scaled by `hidden` (0: in place, 1: fully out).
    pub fn slide_offset(
        self,
        geo: Rectangle<f64, Logical>,
        output_size: Size<f64, Logical>,
        hidden: f64,
    ) -> Point<f64, Logical> {
        let (x, y) = match self {
            Edge::Left => (-(geo.loc.x + geo.size.w), 0.),
            Edge::Right => (output_size.w - geo.loc.x, 0.),
            Edge::Top => (0., -(geo.loc.y + geo.size.h)),
            Edge::Bottom => (0., output_size.h - geo.loc.y),
        };
        Point::from((x * hidden, y * hidden))
    }
}

/// Inputs of one [`EdgeTrigger`] update.
#[derive(Debug, Clone, Copy, Default)]
pub struct TriggerInput {
    pub now: Duration,
    /// The edge's panels are covered (a fullscreen window is shown above the top layer).
    pub covered: bool,
    /// The pointer rests against this edge.
    pub at_edge: bool,
    /// Triggering is not allowed: pointer constraint, button held, grab.
    pub blocked: bool,
    /// Keep the panels shown: the pointer is over one of them, or a layer surface has the
    /// keyboard or the pointer.
    pub keep: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum TriggerState {
    #[default]
    Idle,
    Dwell {
        since: Duration,
    },
    Shown {
        left_at: Option<Duration>,
    },
}

/// The reveal state machine of one edge.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EdgeTrigger {
    state: TriggerState,
}

impl EdgeTrigger {
    /// Advances the state machine; returns whether the edge's panels are revealed.
    pub fn update(&mut self, input: TriggerInput, dwell: Duration, hide_delay: Duration) -> bool {
        if !input.covered {
            self.state = TriggerState::Idle;
            return false;
        }

        if let TriggerState::Idle = self.state {
            if input.at_edge && !input.blocked {
                self.state = TriggerState::Dwell { since: input.now };
            }
        }

        if let TriggerState::Dwell { since } = self.state {
            if !input.at_edge || input.blocked {
                self.state = TriggerState::Idle;
            } else if input.now.saturating_sub(since) >= dwell {
                self.state = TriggerState::Shown { left_at: None };
            }
        }

        if let TriggerState::Shown { left_at } = &mut self.state {
            if input.keep || input.at_edge {
                *left_at = None;
            } else {
                let left = *left_at.get_or_insert(input.now);
                if input.now.saturating_sub(left) >= hide_delay {
                    self.state = TriggerState::Idle;
                }
            }
        }

        self.is_shown()
    }

    pub fn is_shown(&self) -> bool {
        matches!(self.state, TriggerState::Shown { .. })
    }

    /// When the next update could change the state without any input changing.
    pub fn deadline(&self, dwell: Duration, hide_delay: Duration) -> Option<Duration> {
        match self.state {
            TriggerState::Idle => None,
            TriggerState::Dwell { since } => Some(since + dwell),
            TriggerState::Shown { left_at } => left_at.map(|left| left + hide_delay),
        }
    }
}

/// Visibility of one edge's panels: 1 shown, 0 slid out.
#[derive(Debug, Default)]
struct EdgeSlide {
    trigger: EdgeTrigger,
    target: Option<f64>,
    anim: Option<Animation>,
}

impl EdgeSlide {
    fn value(&self) -> f64 {
        match &self.anim {
            Some(anim) => anim.value().clamp(0., 1.),
            None => self.target.unwrap_or(1.),
        }
    }

    fn set_target(&mut self, clock: &Clock, target: f64, config: niri_config::Animation) -> bool {
        let Some(current_target) = self.target else {
            // First time: no animation.
            self.target = Some(target);
            return false;
        };
        if current_target == target {
            return false;
        }
        let from = self.value();
        self.target = Some(target);
        self.anim = Some(Animation::new(clock.clone(), from, target, 0., config));
        true
    }

    fn advance(&mut self) {
        if self.anim.as_ref().is_some_and(|anim| anim.is_done()) {
            self.anim = None;
        }
    }

    fn is_animating(&self) -> bool {
        self.anim.is_some()
    }
}

#[derive(Debug, Default)]
struct OutputReveal {
    edges: [EdgeSlide; 4],
}

/// Where an edge panel is drawn this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Added to the surface position (rendering and hit-testing).
    pub offset: Point<f64, Logical>,
    /// Drawn above the fullscreen window (it is covered otherwise).
    pub above: bool,
}

#[derive(Debug, Default)]
pub struct EdgeRevealState {
    outputs: HashMap<Output, OutputReveal>,
    placements: HashMap<LayerSurface, Placement>,
    timer: Option<(RegistrationToken, Duration)>,
}

impl EdgeRevealState {
    pub fn placement(&self, surface: &LayerSurface) -> Option<Placement> {
        self.placements.get(surface).copied()
    }
}

impl Niri {
    /// The edge a layer surface slides from, if it is an edge panel for `edge-reveal`.
    pub fn edge_panel_edge(&self, surface: &LayerSurface) -> Option<Edge> {
        let config = self.config.borrow();
        let reveal = &config.edge_reveal;
        if !reveal.on || surface.layer() != Layer::Top {
            return None;
        }
        let state = surface.cached_state();
        if !matches!(state.exclusive_zone, ExclusiveZone::Exclusive(zone) if zone > 0) {
            return None;
        }
        if let Some(ns) = &reveal.namespace {
            if !ns.0.is_match(surface.namespace()) {
                return None;
            }
        }
        Edge::from_anchor(state.anchor)
    }

    /// Whether an edge slide animation is running on `output`.
    pub fn edge_reveal_animating(&self, output: &Output) -> bool {
        self.edge_reveal
            .outputs
            .get(output)
            .is_some_and(|o| o.edges.iter().any(EdgeSlide::is_animating))
    }

    /// Recomputes where the edge panels of `output` (all outputs if `None`) are drawn this frame.
    pub fn update_edge_reveal_placements(&mut self, output: Option<&Output>) {
        self.edge_reveal
            .placements
            .retain(|surface, _| surface.alive());
        let outputs: Vec<Output> = match output {
            Some(output) => vec![output.clone()],
            None => self.global_space.outputs().cloned().collect(),
        };
        for output in outputs {
            let Some(mon) = self.layout.monitor_for_output(&output) else {
                continue;
            };
            let above = mon.render_above_top_layer();
            let output_size = mon.view_size();

            let layer_map = layer_map_for_output(&output);
            let surfaces: Vec<(LayerSurface, Rectangle<f64, Logical>)> = layer_map
                .layers_on(Layer::Top)
                .filter_map(|surface| {
                    let geo = layer_map.layer_geometry(surface)?.to_f64();
                    Some((surface.clone(), geo))
                })
                .collect();
            drop(layer_map);

            for (surface, geo) in surfaces {
                self.edge_reveal.placements.remove(&surface);
                let Some(edge) = self.edge_panel_edge(&surface) else {
                    continue;
                };
                let value = self
                    .edge_reveal
                    .outputs
                    .get_mut(&output)
                    .map(|o| {
                        let slide = &mut o.edges[edge.idx()];
                        slide.advance();
                        slide.value()
                    })
                    .unwrap_or(1.);
                if value >= 1. && !above {
                    // Exactly where upstream draws it.
                    continue;
                }
                if value <= 0. && above {
                    // Covered and fully slid out: not drawn.
                    continue;
                }
                self.edge_reveal.placements.insert(
                    surface,
                    Placement {
                        offset: edge.slide_offset(geo, output_size, 1. - value),
                        above,
                    },
                );
            }
        }
    }
}

impl State {
    /// Updates the edge triggers and slide targets of all outputs (once per event loop
    /// iteration, after the pointer contents are known).
    pub fn refresh_edge_reveal(&mut self) {
        let niri = &mut self.niri;
        let config = niri.config.borrow();
        let enabled = config.edge_reveal.on;
        let dwell = Duration::from_millis(u64::from(config.edge_reveal.dwell_ms));
        let hide_delay = Duration::from_millis(u64::from(config.edge_reveal.hide_delay_ms));
        let anim_config = config.animations.edge_reveal.0;
        drop(config);

        if !enabled {
            if !niri.edge_reveal.outputs.is_empty() || !niri.edge_reveal.placements.is_empty() {
                niri.edge_reveal = EdgeRevealState::default();
                niri.queue_redraw_all();
            }
            return;
        }

        let now = niri.clock.now_unadjusted();
        let pointer = niri.seat.get_pointer().unwrap();
        let pointer_pos = pointer.current_location();

        // Triggering is blocked by an active pointer constraint, held buttons and grabs.
        let constrained = niri
            .pointer_contents
            .surface
            .as_ref()
            .is_some_and(|(surface, _)| {
                with_pointer_constraint(surface, &pointer, |constraint| {
                    constraint.is_some_and(|c| c.is_active())
                })
            });
        let blocked = constrained
            || pointer.is_grabbed()
            || niri.popup_grab.is_some()
            || niri.is_locked()
            || niri.screenshot_ui.is_open()
            || niri.window_mru_ui.is_open()
            || niri.layout.is_overview_open();

        // A layer surface with the keyboard (an overlay popup) or the pointer keeps panels shown.
        let layer_keyboard = matches!(niri.keyboard_focus, KeyboardFocus::LayerShell { .. })
            || niri.layer_shell_on_demand_focus.is_some()
            || niri.popup_grab.is_some();
        let pointer_layer = niri.pointer_contents.layer.clone();

        let outputs: Vec<Output> = niri.global_space.outputs().cloned().collect();
        niri.edge_reveal.outputs.retain(|o, _| outputs.contains(o));
        let mut deadline: Option<Duration> = None;

        for output in outputs {
            let Some(mon) = niri.layout.monitor_for_output(&output) else {
                continue;
            };
            let covered = mon.edge_panels_covered();
            let Some(output_geo) = niri.global_space.output_geometry(&output) else {
                continue;
            };
            let output_size = output_geo.size.to_f64();
            let pos = pointer_pos - output_geo.loc.to_f64();
            let pointer_here = output_geo.to_f64().contains(pointer_pos);

            // Which edges have panels, and which panel has the pointer.
            let layer_map = layer_map_for_output(&output);
            let panels: Vec<(LayerSurface, Edge)> = layer_map
                .layers_on(Layer::Top)
                .filter_map(|s| Some((s.clone(), niri.edge_panel_edge(s)?)))
                .collect();
            let pointer_on_other_layer = pointer_here
                && pointer_layer
                    .as_ref()
                    .is_some_and(|l| layer_map.layers().any(|s| s == l));
            drop(layer_map);

            let state = niri.edge_reveal.outputs.entry(output.clone()).or_default();
            let mut changed = false;
            for edge in Edge::ALL {
                let has_panel = panels.iter().any(|(_, e)| *e == edge);
                let over_panel = pointer_layer
                    .as_ref()
                    .is_some_and(|l| panels.iter().any(|(s, e)| s == l && *e == edge));
                let input = TriggerInput {
                    now,
                    covered,
                    at_edge: has_panel && pointer_here && edge.touches(pos, output_size),
                    blocked,
                    keep: over_panel || layer_keyboard || pointer_on_other_layer,
                };
                let slide = &mut state.edges[edge.idx()];
                let shown = slide.trigger.update(input, dwell, hide_delay);
                if let Some(d) = slide.trigger.deadline(dwell, hide_delay) {
                    deadline = Some(deadline.map_or(d, |cur| cur.min(d)));
                }
                let target = if !covered || shown { 1. } else { 0. };
                changed |= slide.set_target(&niri.clock, target, anim_config);
            }
            if changed {
                niri.queue_redraw(&output);
            }
        }

        // Wake up for dwell / hide deadlines while the pointer rests.
        match (deadline, niri.edge_reveal.timer) {
            (Some(d), Some((_, at))) if at == d => {}
            (deadline, timer) => {
                if let Some((token, _)) = timer {
                    niri.event_loop.remove(token);
                }
                niri.edge_reveal.timer = deadline.map(|d| {
                    let timer =
                        Timer::from_duration(d.saturating_sub(now) + Duration::from_millis(1));
                    let token = niri
                        .event_loop
                        .insert_source(timer, |_, _, state| {
                            state.niri.edge_reveal.timer = None;
                            // The event loop iteration refreshes the triggers.
                            state.niri.queue_redraw_all();
                            TimeoutAction::Drop
                        })
                        .unwrap();
                    (token, d)
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DWELL: Duration = Duration::from_millis(150);
    const HIDE: Duration = Duration::from_millis(400);

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    fn at(now: u64, at_edge: bool) -> TriggerInput {
        TriggerInput {
            now: ms(now),
            covered: true,
            at_edge,
            blocked: false,
            keep: false,
        }
    }

    #[test]
    fn dwell_then_reveal() {
        let mut t = EdgeTrigger::default();
        assert!(!t.update(at(0, true), DWELL, HIDE));
        assert_eq!(t.deadline(DWELL, HIDE), Some(ms(150)));
        assert!(!t.update(at(100, true), DWELL, HIDE));
        assert!(t.update(at(150, true), DWELL, HIDE));
    }

    #[test]
    fn graze_does_not_reveal() {
        let mut t = EdgeTrigger::default();
        assert!(!t.update(at(0, true), DWELL, HIDE));
        assert!(!t.update(at(50, false), DWELL, HIDE));
        assert!(!t.update(at(200, false), DWELL, HIDE));
        // Coming back restarts the dwell.
        assert!(!t.update(at(210, true), DWELL, HIDE));
        assert!(!t.update(at(300, true), DWELL, HIDE));
        assert!(t.update(at(360, true), DWELL, HIDE));
    }

    #[test]
    fn hides_after_leaving() {
        let mut t = EdgeTrigger::default();
        t.update(at(0, true), DWELL, HIDE);
        assert!(t.update(at(150, true), DWELL, HIDE));
        // Over the panel: stays.
        let over = TriggerInput {
            keep: true,
            ..at(500, false)
        };
        assert!(t.update(over, DWELL, HIDE));
        assert_eq!(t.deadline(DWELL, HIDE), None);
        // Left the panel at 600: hidden at 1000.
        assert!(t.update(at(600, false), DWELL, HIDE));
        assert_eq!(t.deadline(DWELL, HIDE), Some(ms(1000)));
        assert!(t.update(at(999, false), DWELL, HIDE));
        assert!(!t.update(at(1000, false), DWELL, HIDE));
    }

    #[test]
    fn coming_back_cancels_hiding() {
        let mut t = EdgeTrigger::default();
        t.update(at(0, true), DWELL, HIDE);
        t.update(at(150, true), DWELL, HIDE);
        assert!(t.update(at(200, false), DWELL, HIDE));
        let over = TriggerInput {
            keep: true,
            ..at(300, false)
        };
        assert!(t.update(over, DWELL, HIDE));
        assert!(t.update(at(650, false), DWELL, HIDE));
        assert!(t.update(at(1000, false), DWELL, HIDE));
        assert!(!t.update(at(1050, false), DWELL, HIDE));
    }

    #[test]
    fn constraint_or_button_blocks_triggering() {
        let mut t = EdgeTrigger::default();
        let blocked = |now| TriggerInput {
            blocked: true,
            ..at(now, true)
        };
        assert!(!t.update(blocked(0), DWELL, HIDE));
        assert!(!t.update(blocked(500), DWELL, HIDE));
        assert_eq!(t.deadline(DWELL, HIDE), None);

        // Blocked in the middle of the dwell: starts over.
        assert!(!t.update(at(600, true), DWELL, HIDE));
        assert!(!t.update(blocked(700), DWELL, HIDE));
        assert!(!t.update(at(760, true), DWELL, HIDE));
        assert!(!t.update(at(800, true), DWELL, HIDE));
        assert!(t.update(at(910, true), DWELL, HIDE));

        // Once shown, a held button (a click on the panel) doesn't hide it.
        let click = TriggerInput {
            keep: true,
            ..blocked(1000)
        };
        assert!(t.update(click, DWELL, HIDE));
    }

    #[test]
    fn not_covered_resets() {
        let mut t = EdgeTrigger::default();
        t.update(at(0, true), DWELL, HIDE);
        t.update(at(150, true), DWELL, HIDE);
        let uncovered = TriggerInput {
            covered: false,
            ..at(200, true)
        };
        assert!(!t.update(uncovered, DWELL, HIDE));
        assert_eq!(t, EdgeTrigger::default());
    }

    #[test]
    fn zero_dwell_reveals_right_away() {
        let mut t = EdgeTrigger::default();
        assert!(t.update(at(0, true), Duration::ZERO, HIDE));
    }

    #[test]
    fn edges_from_anchors() {
        let rail = Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT;
        let bar = Anchor::LEFT | Anchor::RIGHT | Anchor::BOTTOM;
        assert_eq!(Edge::from_anchor(rail), Some(Edge::Left));
        assert_eq!(Edge::from_anchor(bar), Some(Edge::Bottom));
        assert_eq!(Edge::from_anchor(Anchor::TOP), Some(Edge::Top));
        assert_eq!(Edge::from_anchor(Anchor::TOP | Anchor::RIGHT), None);
        assert_eq!(Edge::from_anchor(Anchor::all()), None);
        assert_eq!(Edge::from_anchor(Anchor::empty()), None);
    }

    #[test]
    fn touching_and_sliding() {
        let size = Size::from((1920., 1080.));
        assert!(Edge::Left.touches(Point::from((0., 500.)), size));
        assert!(!Edge::Left.touches(Point::from((2., 500.)), size));
        assert!(Edge::Bottom.touches(Point::from((800., 1079.5)), size));
        assert!(!Edge::Bottom.touches(Point::from((800., 1070.)), size));

        let rail = Rectangle::new(Point::from((12., 12.)), Size::from((64., 1000.)));
        assert_eq!(
            Edge::Left.slide_offset(rail, size, 1.),
            Point::from((-76., 0.))
        );
        assert_eq!(
            Edge::Left.slide_offset(rail, size, 0.),
            Point::from((0., 0.))
        );
        let bar = Rectangle::new(Point::from((0., 1034.)), Size::from((1920., 46.)));
        assert_eq!(
            Edge::Bottom.slide_offset(bar, size, 0.5),
            Point::from((0., 23.))
        );
    }
}
