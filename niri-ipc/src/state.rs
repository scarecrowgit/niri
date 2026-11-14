//! Helpers for keeping track of the event stream state.
//!
//! 1. Create an [`EventStreamState`] using `Default::default()`, or any individual state part if
//!    you only care about part of the state.
//! 2. Connect to the niri socket and request an event stream.
//! 3. Pass every [`Event`] to [`EventStreamStatePart::apply`] on your state.
//! 4. Read the fields of the state as needed.

use std::collections::hash_map::Entry;
use std::collections::HashMap;

use crate::{CanvasViewState, Cast, Event, KeyboardLayouts, StripViewState, Window, Workspace};

/// Part of the state communicated via the event stream.
pub trait EventStreamStatePart {
    /// Returns a sequence of events that replicates this state from default initialization.
    fn replicate(&self) -> Vec<Event>;

    /// Applies the event to this state.
    ///
    /// Returns `None` after applying the event, and `Some(event)` if the event is ignored by this
    /// part of the state.
    fn apply(&mut self, event: Event) -> Option<Event>;
}

/// The full state communicated over the event stream.
///
/// Different parts of the state are not guaranteed to be consistent across every single event
/// sent by niri. For example, you may receive the first [`Event::WindowOpenedOrChanged`] for a
/// just-opened window *after* an [`Event::WorkspaceActiveWindowChanged`] for that window. Between
/// these two events, the workspace active window id refers to a window that does not yet exist in
/// the windows state part.
#[derive(Debug, Default)]
pub struct EventStreamState {
    /// State of workspaces.
    pub workspaces: WorkspacesState,

    /// State of windows.
    pub windows: WindowsState,

    /// State of the keyboard layouts.
    pub keyboard_layouts: KeyboardLayoutsState,

    /// State of the overview.
    pub overview: OverviewState,

    /// State of the config.
    pub config: ConfigState,

    /// State of screencasts.
    pub casts: CastsState,

    /// State of touchpads.
    pub touchpad: TouchpadState,

    /// State of the canvas views (ashd fork).
    pub canvas: CanvasState,

    /// State of the strip (scrolling layout) views (ashd fork).
    pub strip: StripState,
}

/// The workspaces state communicated over the event stream.
#[derive(Debug, Default)]
pub struct WorkspacesState {
    /// Map from a workspace id to the workspace.
    pub workspaces: HashMap<u64, Workspace>,
}

/// The windows state communicated over the event stream.
#[derive(Debug, Default)]
pub struct WindowsState {
    /// Map from a window id to the window.
    pub windows: HashMap<u64, Window>,
}

/// The keyboard layout state communicated over the event stream.
#[derive(Debug, Default)]
pub struct KeyboardLayoutsState {
    /// Configured keyboard layouts.
    pub keyboard_layouts: Option<KeyboardLayouts>,
}

/// The overview state communicated over the event stream.
#[derive(Debug, Default)]
pub struct OverviewState {
    /// Whether the overview is currently open.
    pub is_open: bool,
}

/// The config state communicated over the event stream.
#[derive(Debug, Default)]
pub struct ConfigState {
    /// Whether the last config load attempt had failed.
    pub failed: bool,
}

/// The casts state communicated over the event stream.
#[derive(Debug, Default)]
pub struct CastsState {
    /// Map from a stream id to the screencast.
    pub casts: HashMap<u64, Cast>,
}

/// The canvas view state communicated over the event stream (ashd fork).
#[derive(Debug, Default)]
pub struct CanvasState {
    /// Map from a workspace id to the view of that canvas-mode workspace.
    pub views: HashMap<u64, CanvasViewState>,
}

/// The strip view state communicated over the event stream (ashd fork).
#[derive(Debug, Default)]
pub struct StripState {
    /// Map from a workspace id to the scroll state of that strip-mode workspace.
    pub views: HashMap<u64, StripViewState>,
}

/// The touchpad state communicated over the event stream.
#[derive(Debug, Default)]
pub struct TouchpadState {
    /// Whether the touchpad is currently on.
    ///
    /// Note that this only indicates the [`Action::ToggleTouchpad`] state. The touchpad can be
    /// disabled in hardware, or via `input { touchpad { off; } }`—this is not reflected here.
    ///
    /// [`Action::ToggleTouchpad`]: crate::Action::ToggleTouchpad
    pub on: bool,
}

impl EventStreamStatePart for EventStreamState {
    fn replicate(&self) -> Vec<Event> {
        let mut events = Vec::new();
        events.extend(self.workspaces.replicate());
        events.extend(self.windows.replicate());
        events.extend(self.keyboard_layouts.replicate());
        events.extend(self.overview.replicate());
        events.extend(self.config.replicate());
        events.extend(self.casts.replicate());
        events.extend(self.touchpad.replicate());
        events.extend(self.canvas.replicate());
        events.extend(self.strip.replicate());
        events
    }

    fn apply(&mut self, event: Event) -> Option<Event> {
        let event = self.workspaces.apply(event)?;
        let event = self.windows.apply(event)?;
        let event = self.keyboard_layouts.apply(event)?;
        let event = self.overview.apply(event)?;
        let event = self.config.apply(event)?;
        let event = self.casts.apply(event)?;
        let event = self.touchpad.apply(event)?;
        let event = self.canvas.apply(event)?;
        let event = self.strip.apply(event)?;
        Some(event)
    }
}

impl EventStreamStatePart for WorkspacesState {
    fn replicate(&self) -> Vec<Event> {
        let workspaces = self.workspaces.values().cloned().collect();
        vec![Event::WorkspacesChanged { workspaces }]
    }

    fn apply(&mut self, event: Event) -> Option<Event> {
        match event {
            Event::WorkspacesChanged { workspaces } => {
                self.workspaces = workspaces.into_iter().map(|ws| (ws.id, ws)).collect();
            }
            Event::WorkspaceUrgencyChanged { id, urgent } => {
                for ws in self.workspaces.values_mut() {
                    if ws.id == id {
                        ws.is_urgent = urgent;
                    }
                }
            }
            Event::WorkspaceActivated { id, focused } => {
                let ws = self.workspaces.get(&id);
                let ws = ws.expect("activated workspace was missing from the map");
                let output = ws.output.clone();

                for ws in self.workspaces.values_mut() {
                    let got_activated = ws.id == id;
                    if ws.output == output {
                        ws.is_active = got_activated;
                    }

                    if focused {
                        ws.is_focused = got_activated;
                    }
                }
            }
            Event::WorkspaceActiveWindowChanged {
                workspace_id,
                active_window_id,
            } => {
                let ws = self.workspaces.get_mut(&workspace_id);
                let ws = ws.expect("changed workspace was missing from the map");
                ws.active_window_id = active_window_id;
            }
            Event::WorkspaceModeChanged { id, mode } => {
                let ws = self.workspaces.get_mut(&id);
                let ws = ws.expect("changed workspace was missing from the map");
                ws.mode = mode;
            }
            event => return Some(event),
        }
        None
    }
}

impl EventStreamStatePart for WindowsState {
    fn replicate(&self) -> Vec<Event> {
        let windows = self.windows.values().cloned().collect();
        vec![Event::WindowsChanged { windows }]
    }

    fn apply(&mut self, event: Event) -> Option<Event> {
        match event {
            Event::WindowsChanged { windows } => {
                self.windows = windows.into_iter().map(|win| (win.id, win)).collect();
            }
            Event::WindowOpenedOrChanged { window } => {
                let (id, is_focused) = match self.windows.entry(window.id) {
                    Entry::Occupied(mut entry) => {
                        let entry = entry.get_mut();
                        *entry = window;
                        (entry.id, entry.is_focused)
                    }
                    Entry::Vacant(entry) => {
                        let entry = entry.insert(window);
                        (entry.id, entry.is_focused)
                    }
                };

                if is_focused {
                    for win in self.windows.values_mut() {
                        if win.id != id {
                            win.is_focused = false;
                        }
                    }
                }
            }
            Event::WindowClosed { id } => {
                let win = self.windows.remove(&id);
                win.expect("closed window was missing from the map");
            }
            Event::WindowFocusChanged { id } => {
                for win in self.windows.values_mut() {
                    win.is_focused = Some(win.id) == id;
                }
            }
            Event::WindowFocusTimestampChanged {
                id,
                focus_timestamp,
            } => {
                for win in self.windows.values_mut() {
                    if win.id == id {
                        win.focus_timestamp = focus_timestamp;
                        break;
                    }
                }
            }
            Event::WindowUrgencyChanged { id, urgent } => {
                for win in self.windows.values_mut() {
                    if win.id == id {
                        win.is_urgent = urgent;
                        break;
                    }
                }
            }
            Event::WindowLayoutsChanged { changes } => {
                for (id, update) in changes {
                    let win = self.windows.get_mut(&id);
                    let win = win.expect("changed window was missing from the map");
                    win.layout = update;
                }
            }
            event => return Some(event),
        }
        None
    }
}

impl EventStreamStatePart for KeyboardLayoutsState {
    fn replicate(&self) -> Vec<Event> {
        if let Some(keyboard_layouts) = self.keyboard_layouts.clone() {
            vec![Event::KeyboardLayoutsChanged { keyboard_layouts }]
        } else {
            vec![]
        }
    }

    fn apply(&mut self, event: Event) -> Option<Event> {
        match event {
            Event::KeyboardLayoutsChanged { keyboard_layouts } => {
                self.keyboard_layouts = Some(keyboard_layouts);
            }
            Event::KeyboardLayoutSwitched { idx } => {
                let kb = self.keyboard_layouts.as_mut();
                let kb = kb.expect("keyboard layouts must be set before a layout can be switched");
                kb.current_idx = idx;
            }
            event => return Some(event),
        }
        None
    }
}

impl EventStreamStatePart for OverviewState {
    fn replicate(&self) -> Vec<Event> {
        vec![Event::OverviewOpenedOrClosed {
            is_open: self.is_open,
        }]
    }

    fn apply(&mut self, event: Event) -> Option<Event> {
        match event {
            Event::OverviewOpenedOrClosed { is_open } => {
                self.is_open = is_open;
            }
            event => return Some(event),
        }
        None
    }
}

impl EventStreamStatePart for ConfigState {
    fn replicate(&self) -> Vec<Event> {
        vec![Event::ConfigLoaded {
            failed: self.failed,
        }]
    }

    fn apply(&mut self, event: Event) -> Option<Event> {
        match event {
            Event::ConfigLoaded { failed } => {
                self.failed = failed;
            }
            event => return Some(event),
        }
        None
    }
}

impl EventStreamStatePart for CastsState {
    fn replicate(&self) -> Vec<Event> {
        let casts = self.casts.values().cloned().collect();
        vec![Event::CastsChanged { casts }]
    }

    fn apply(&mut self, event: Event) -> Option<Event> {
        match event {
            Event::CastsChanged { casts } => {
                self.casts = casts.into_iter().map(|c| (c.stream_id, c)).collect();
            }
            Event::CastStartedOrChanged { cast } => {
                self.casts.insert(cast.stream_id, cast);
            }
            Event::CastStopped { stream_id } => {
                let cast = self.casts.remove(&stream_id);
                cast.expect("stopped cast was missing from the map");
            }
            event => return Some(event),
        }
        None
    }
}

impl EventStreamStatePart for TouchpadState {
    fn replicate(&self) -> Vec<Event> {
        vec![Event::TouchpadToggled { on: self.on }]
    }

    fn apply(&mut self, event: Event) -> Option<Event> {
        match event {
            Event::TouchpadToggled { on } => {
                self.on = on;
            }
            event => return Some(event),
        }
        None
    }
}

impl EventStreamStatePart for CanvasState {
    fn replicate(&self) -> Vec<Event> {
        let mut views: Vec<_> = self.views.values().cloned().collect();
        views.sort_by_key(|view| view.workspace_id);
        vec![Event::CanvasViewsChanged { views }]
    }

    fn apply(&mut self, event: Event) -> Option<Event> {
        match event {
            Event::CanvasViewsChanged { views } => {
                self.views = views
                    .into_iter()
                    .map(|view| (view.workspace_id, view))
                    .collect();
            }
            Event::CanvasViewChanged { view } => {
                self.views.insert(view.workspace_id, view);
            }
            event => return Some(event),
        }
        None
    }
}

impl EventStreamStatePart for StripState {
    fn replicate(&self) -> Vec<Event> {
        let mut views: Vec<_> = self.views.values().cloned().collect();
        views.sort_by_key(|view| view.workspace_id);
        vec![Event::StripViewsChanged { views }]
    }

    fn apply(&mut self, event: Event) -> Option<Event> {
        match event {
            Event::StripViewsChanged { views } => {
                self.views = views
                    .into_iter()
                    .map(|view| (view.workspace_id, view))
                    .collect();
            }
            Event::StripViewChanged {
                workspace_id,
                view_offset,
                target_view_offset,
                working_area_width,
                column_xs,
            } => {
                let view = StripViewState {
                    workspace_id,
                    view_offset,
                    target_view_offset,
                    working_area_width,
                    column_xs,
                };
                self.views.insert(workspace_id, view);
            }
            event => return Some(event),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WindowLayout, WorkspaceMode};

    fn workspace(id: u64, mode: WorkspaceMode) -> Workspace {
        Workspace {
            id,
            idx: id as u8,
            name: None,
            output: Some(String::from("out")),
            is_urgent: false,
            is_active: id == 1,
            is_focused: id == 1,
            active_window_id: None,
            mode,
            is_hidden: false,
            fullscreen_window_id: None,
            fullscreen_origin_id: None,
        }
    }

    fn canvas_view(workspace_id: u64, x: f64, animating: bool) -> CanvasViewState {
        CanvasViewState {
            workspace_id,
            offset: (x, 0.),
            zoom: 0.62,
            target_offset: (100., 0.),
            target_zoom: 0.62,
            view_size: (1920., 1080.),
            animating,
            regions: Vec::new(),
        }
    }

    fn strip_view(workspace_id: u64, x: f64) -> StripViewState {
        StripViewState {
            workspace_id,
            view_offset: x,
            target_view_offset: 200.,
            working_area_width: 1920.,
            column_xs: vec![0., 1000.],
        }
    }

    /// Applies events to a fresh state, then checks that replicating it gives the same state.
    fn apply_all(events: impl IntoIterator<Item = Event>) -> EventStreamState {
        let mut state = EventStreamState::default();
        for event in events {
            assert!(state.apply(event).is_none());
        }

        let mut copy = EventStreamState::default();
        for event in state.replicate() {
            assert!(copy.apply(event).is_none());
        }
        assert_eq!(copy.workspaces.workspaces, state.workspaces.workspaces);
        assert_eq!(copy.canvas.views, state.canvas.views);
        assert_eq!(copy.strip.views, state.strip.views);

        state
    }

    #[test]
    fn workspace_mode_changed() {
        let state = apply_all([
            Event::WorkspacesChanged {
                workspaces: vec![
                    workspace(1, WorkspaceMode::Strip),
                    workspace(2, WorkspaceMode::Strip),
                ],
            },
            Event::WorkspaceModeChanged {
                id: 2,
                mode: WorkspaceMode::Canvas,
            },
        ]);
        let ws = &state.workspaces.workspaces;
        assert_eq!(ws[&1].mode, WorkspaceMode::Strip);
        assert_eq!(ws[&2].mode, WorkspaceMode::Canvas);
    }

    #[test]
    fn canvas_views() {
        let state = apply_all([
            Event::CanvasViewsChanged {
                views: vec![canvas_view(1, 0., false), canvas_view(2, 0., false)],
            },
            Event::CanvasViewChanged {
                view: canvas_view(2, 50., true),
            },
            Event::CanvasViewChanged {
                view: canvas_view(2, 100., false),
            },
        ]);
        assert_eq!(state.canvas.views.len(), 2);
        assert_eq!(state.canvas.views[&2], canvas_view(2, 100., false));

        // A full update replaces everything.
        let mut state = state;
        state.apply(Event::CanvasViewsChanged {
            views: vec![canvas_view(3, 0., false)],
        });
        assert_eq!(state.canvas.views.keys().collect::<Vec<_>>(), [&3]);
    }

    #[test]
    fn strip_views() {
        let state = apply_all([
            Event::StripViewsChanged {
                views: vec![strip_view(1, 0.), strip_view(2, 0.)],
            },
            strip_view(1, 150.).into_event(),
        ]);
        assert_eq!(state.strip.views.len(), 2);
        assert_eq!(state.strip.views[&1], strip_view(1, 150.));
    }

    #[test]
    fn window_layouts_carry_canvas_fields() {
        let window = Window {
            id: 5,
            title: None,
            app_id: None,
            pid: None,
            workspace_id: Some(2),
            is_focused: false,
            is_floating: true,
            is_urgent: false,
            layout: WindowLayout {
                pos_in_scrolling_layout: None,
                tile_size: (100., 100.),
                window_size: (100, 100),
                tile_pos_in_workspace_view: None,
                window_offset_in_tile: (0., 0.),
                pos_in_canvas: Some((1., 2.)),
                is_pinned: false,
            },
            focus_timestamp: None,
            hidden_from_workspace_id: Some(1),
        };
        let mut moved = window.layout.clone();
        moved.pos_in_canvas = Some((300., -40.));
        moved.is_pinned = true;

        let mut state = EventStreamState::default();
        state.apply(Event::WindowsChanged {
            windows: vec![window],
        });
        state.apply(Event::WindowLayoutsChanged {
            changes: vec![(5, moved.clone())],
        });
        let win = &state.windows.windows[&5];
        assert_eq!(win.layout, moved);
        assert_eq!(win.hidden_from_workspace_id, Some(1));
    }
}
