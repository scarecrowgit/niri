# ashd fork of niri

This branch (`ashd`) is a fork of upstream niri (base: `73a04441`, v26.04+). It adds a
per-workspace **infinite canvas** window mode next to niri's scrolling strip. Everything else
behaves exactly like upstream: with the default config, and on any workspace that never enters
canvas mode, layout and rendering go through the same code paths with bit-identical numbers.

The fork is kept rebasable: new logic lives in new files, and edits in upstream files are short
and marked with a `// ashd:` comment (or a `ashd:` doc comment on new fields).

## What canvas mode is

A workspace is in one of two modes (`niri_ipc::WorkspaceMode`):

- **Strip** – stock niri scrollable tiling.
- **Canvas** – all normal windows float on an unbounded 2D plane. The output shows a viewport
  onto the plane defined by an offset and a zoom:

  ```text
  view  = (plane - offset) * zoom
  plane = view / zoom + offset
  ```

Behaviour in canvas mode:

- Zoom steps `0.4 0.5 0.62 0.75 0.9 1.0` (configurable). A workspace gets `default-zoom` (0.62)
  the first time it enters canvas mode and remembers its zoom afterwards. Zoom in/out step through
  the list around an optional anchor (view point that stays fixed; the working area centre when
  not given). `canvas-set-zoom` clamps to the step range. Fit picks offset and zoom so that all
  (non-pinned) windows are visible with 48 px padding, clamped to the step range and never above
  1.0. Home pans to offset (0, 0) keeping the zoom.
- Pan by `pan-step` view pixels (divided by zoom on the plane) in four directions, pan to an
  absolute plane offset, fly to a window (centre it in the working area, keeping the zoom).
- Keyboard focus changes (directional focus actions, focus by id through IPC, focus-previous,
  alt-tab) recentre the view on the newly focused window with the pan spring ("reveal"). Pointer
  focus (click, focus-follows-mouse) never recentres.
- Moving a window with the keyboard (`move-column-left/right`, `move-window-up/down` and their
  variants, `move-floating-window`) past the viewport edge pushes the view along (minimal pan,
  16 px margin).
- A window can be pinned: it keeps its position on screen while the view pans (it scales with the
  canvas and its unscaled position is kept while zooming).
- Fullscreen and maximize keep niri semantics: the window leaves the plane for the scrolling
  space, covers the output at scale 1 regardless of the zoom (while it is focused the canvas is
  hidden; focusing a canvas window, e.g. `focus-floating`, shows the canvas on top again), and
  returns to its exact plane position afterwards.
- New windows open centred in the current viewport at their natural size; several in a row
  cascade by 32 plane px. Windows moved from another workspace land at the viewport centre
  (see limitations for strip-floating windows). Interactive drops land where they are dropped.
- Strip → canvas: the strip arrangement (column order, widths, full-width, tabbed display, tile
  order and heights, active tile/column, which windows are floating) is saved; each tile is put on
  the plane exactly at its current on-screen position (or its remembered plane position from the
  previous canvas session), then the view animates to the canvas zoom centred on the focused
  window.
- Canvas → strip: columns are rebuilt from the saved arrangement; windows unknown to it are
  appended ordered by plane x (windows with an `open-floating true` rule stay floating). Plane
  positions and the zoom are remembered so that toggling back restores the canvas layout.
  Windows that were floating in strip mode stay floating in strip mode after a round trip.
- Animations: pan/fly-to spring (damping 1.0, stiffness 500), zoom spring (1.0, 600), epsilon
  0.0001. No bounce.
- Borders and focus rings: once the zoom settles below 1, the canvas windows get an effective
  `border`/`focus-ring` width of `max(width, ceil(1 / (zoom * output_scale)))` so that the frame
  stays at least one physical pixel wide; the focused window keeps its focus ring / active colour.
- Off-screen canvas tiles are culled from rendering (with a margin of half the view size for
  shadows and popups).

## Config

```kdl
layout {
    // "strip" (default, stock niri) or "canvas": mode of newly created workspaces.
    default-workspace-mode "strip"

    canvas {
        default-zoom 0.62
        zoom-steps 0.4 0.5 0.62 0.75 0.9 1.0
        pan-step 120
    }
}

animations {
    canvas-pan {
        spring damping-ratio=1.0 stiffness=500 epsilon=0.0001
    }
    canvas-zoom {
        spring damping-ratio=1.0 stiffness=600 epsilon=0.0001
    }
}
```

`default-workspace-mode` and `canvas {}` are regular `layout` options, so they merge per output
(`output { layout { ... } }`) and per named workspace (`workspace "x" { layout { ... } }`) like
the other layout options. Zoom steps are sorted and deduplicated, values are clamped to
`0.05..=4`, and `default-zoom` is clamped into the step range. Canvas animations accept every
form the other animations accept (`off`, spring, easing).

Binds / actions (`niri msg action <name>`; canvas-only actions are no-ops on strip workspaces):

| Bind action | IPC action | Notes |
| --- | --- | --- |
| `toggle-workspace-mode` | `ToggleWorkspaceMode {}` | focused workspace |
| `set-workspace-mode "canvas"` | `SetWorkspaceMode { mode }` | `strip` / `canvas` |
| `canvas-pan-left` / `-right` / `-up` / `-down` | `CanvasPanLeft {}` … | one `pan-step` |
| `canvas-pan-to X Y` | `CanvasPanTo { x, y }` | offset becomes (X, Y): the view's top-left shows that plane point |
| `canvas-zoom-in` / `canvas-zoom-out` | `CanvasZoomIn { at_cursor }` / `CanvasZoomOut { at_cursor }` | next/previous zoom step (`at-cursor=true`: around the pointer) |
| `canvas-set-zoom Z` | `CanvasSetZoom { zoom }` | clamped |
| `canvas-fit` | `CanvasFit {}` | |
| `canvas-home` | `CanvasHome {}` | |
| `canvas-fly-to-window` | `CanvasFlyToWindow { id }` | `--id` optional |
| `toggle-window-pinned` | `ToggleWindowPinned { id }` | `--id` optional |
| `hide-window` | `HideWindow { id }` | `--id` optional; to the scratchpad (hidden workspace) |
| `unhide-window` | `UnhideWindow { id }` | `--id` optional, default most recently hidden |

The scratchpad is a named workspace with `hidden` (see "Hidden workspaces" below):

```kdl
workspace "scratch" {
    hidden
}
```

Pointer/gesture and decoration actions and options are listed in their own sections below.
All new variants are appended at the end of both `Action` enums.

IPC state: `niri_ipc::Workspace` has a new `mode: WorkspaceMode` field with `#[serde(default)]`
(old replies without it parse as `Strip`). A mode change sends `WorkspaceModeChanged`; see
"IPC for the shell" below for the canvas/strip view state and the scratchpad.

## Coordinate model and architecture

Three logical-pixel spaces exist on a canvas workspace (`src/layout/canvas.rs` module docs):

- **plane** – unbounded canvas coordinates; window positions live here.
- **unscaled view** (`u = plane - offset`) – what the floating space reports to the rest of the
  layout (`tiles_with_offsets()`, `tiles_with_render_positions()`, hit positions, popup target
  rects, interactive-move positions). At zoom 1 this *is* the view, so every stock code path
  (input, popups, interactive move/resize) works unchanged at 100%.
- **view** – `u * zoom`, what is on the output. Rendering wraps the floating space's elements in
  a `RescaleRenderElement` around the workspace origin (new `WorkspaceRenderElement::Canvas`
  variant); hit-testing divides by the zoom.

The canvas is the workspace's own `FloatingSpace` switched into *unbounded* mode
(`FloatingSpace::set_canvas_mode(Some(transform))`): per-tile `Data` keeps plane coordinates
without the Mutter-style on-screen clamping, size-fraction positions (`Tile::floating_pos`) are
relative to the visible part of the working area, and pinned tiles store unscaled view
coordinates. `Workspace` owns a `WorkspaceCanvas` (mode, `CanvasView`, saved
`StripArrangement`, memory of the last canvas session) and pushes the current transform into
the floating space every frame. In canvas mode the scrolling space only holds fullscreen or
maximized windows (all with `restore_to_floating`, so they return to the plane).

### New files

| File | Content |
| --- | --- |
| `src/layout/canvas.rs` | `CanvasTransform`, `CanvasView` (springs, retargeting), math helpers (`next_zoom_step`, `fit_transform`, `push_to_contain`, `compensated_frame_width`), `WorkspaceCanvas`, `CanvasMemory`, `CanvasPanDirection`, Layout-level canvas API, unit tests |
| `src/layout/workspace/canvas_ext.rs` | child module of `workspace`: all new `impl Workspace` methods (mode switching, view ops, hooks for add/remove/render/hit-test, invariants) |
| `src/layout/floating/canvas_mode.rs` | child module of `floating`: unbounded mode, plane accessors, pinning |
| `src/layout/scrolling/strip_snapshot.rs` | child module of `scrolling`: `StripArrangement`/`SavedColumn`, snapshot and rebuild of columns |
| `src/input/canvas_actions.rs` | `State::do_canvas_action`, `CanvasActionHook` (reveal / edge-push after keyboard actions) |
| `src/layout/tests/canvas.rs` | layout-level canvas tests |
| `niri-config/src/canvas.rs` | `Canvas`/`CanvasPart`, `CanvasPanAnim`/`CanvasZoomAnim`, parse tests |
| `niri-ipc/src/canvas.rs` | `WorkspaceMode`, `CanvasViewState`, `StripViewState` |
| `src/layout/scratch.rs` | hidden (scratchpad) workspaces: `ScratchOrigin`, Layout-level `hide_window`/`unhide_window`/`hidden_windows` |
| `src/layout/workspace/scratch_ext.rs` | child module of `workspace`: `is_hidden`/`set_hidden`, origin hooks for tiles entering/leaving a workspace |
| `src/input/scratch_actions.rs` | `State::do_scratch_action` (`hide-window`, `unhide-window`) |
| `src/layout/tests/scratch.rs` | hidden workspace tests |
| `src/layout/ws_by_index.rs` | `create-workspaces-by-index`: creating workspaces by index, trailing-only clean-up (see "Workspaces by index") |
| `src/layout/tests/ws_by_index.rs`, `src/tests/ashd_mru.rs` | by-index layout tests; recent-windows quick-tap tests |

### Upstream files touched

| File | Change | Why |
| --- | --- | --- |
| `src/layout/workspace.rs` | `canvas` field + init (2 constructors call `apply_default_mode()`); `mod canvas_ext`; `Canvas` render element variant; hooks in `advance_animations`, `are_animations_ongoing`, `are_transitions_ongoing`, `update_render_elements` (view rect), `update_config`/`set_view_size` (frame-compensated floating options, zoom re-clamp), `add_tile` (canvas placement), `add_tile_to_column`/`add_column` (spill onto plane), `remove_tile` (forget plane pos), `resolve_default_width/height`, `new_window_size`, `configure_new_window` (canvas windows open floating-sized), `toggle_window_floating` split into a guard + `_impl` (canvas windows cannot leave the plane; fullscreen uses `_impl`), `tiles_with_ipc_layouts` (view coords), `active_window_visual_rectangle`, `render_floating`, `window_under`, `resize_edges_under`, `scrolling_insert_position` (drops land on the plane), `remove_active_column` (forget plane pos), `set_fullscreen`/`set_maximized` (canvas windows always restore to the plane), `refresh` (windows un-maximized by a column action go back onto the plane), `is_floating_visible` (a focused fullscreen/maximized window hides the canvas), `floating_logical_to_size_frac` (view → unscaled for drops), `verify_invariants` | the workspace is where strip/canvas dispatch happens |
| `src/layout/floating.rs` | `canvas` field on the space and `canvas`/`pinned` on `Data`; unbounded branch in `recompute_logical_pos`/`set_logical_pos`; `tiles_with_offsets(_mut)` subtract the offset; size-fraction area, popup target rect, default floating position, centring and clamping use the visible area; add/remove keep exact plane positions; directional focus uses consistent centres; render culling; `mod canvas_mode`; invariants | unbounded plane mode of the floating space |
| `src/layout/tile.rs` | `canvas_plane_pos` field | exact plane position across fullscreen round trips |
| `src/layout/scrolling.rs` | `mod strip_snapshot` + re-export | arrangement snapshot needs private column fields |
| `src/layout/mod.rs` | `pub mod canvas`; interactive move begin/end multiply by the canvas zoom | Mod+drag at zoom ≠ 1 keeps the pointer ratio and animates from the right place |
| `src/layout/monitor.rs` | `move_column_to_workspace` tolerates the column not existing afterwards | a column moved onto a canvas is spilled onto the plane |
| `src/layout/tests.rs` | new `Op`s, strategies, `default_workspace_mode` in random layout parts, `mod canvas`, canvas ops in the exhaustive op lists | randomized coverage |
| `src/input/mod.rs` | `mod canvas_actions`; one match arm for the canvas actions; hook before/after the action match | actions + reveal / edge-push for keyboard actions |
| `src/niri.rs` | `State::focus_window` reveals | focus by id / previous / alt-tab |
| `src/ipc/server.rs` | `Workspace.mode`, change detection | IPC |
| `niri-ipc/src/lib.rs` | `mod canvas`, `Workspace.mode`, new `Action` variants | IPC |
| `src/layout/monitor.rs` (scratchpad) | visual workspace indices for switch animations/gestures/render geometry, hidden workspaces skipped by up/down navigation and filtered from render/hit-test iterators, `ensure_active_visible`, activation guard | hidden workspaces |
| `src/layout/mod.rs` (IPC/scratchpad) | `pub mod scratch`; `activate_window(_without_raising)` ignore hidden windows and compare gesture indices visually; `update_config` sets `hidden`; `add_output` keeps the active workspace visible | hidden workspaces |
| `src/layout/workspace.rs` (IPC/scratchpad) | `hidden` field + init, cleared by `unname`; `mod scratch_ext`; origin hooks in `add_tile`, `remove_tile`, `remove_active_column`, `add_column`; `tiles_with_ipc_layouts` uses `canvas_floating_ipc_layouts` | hidden workspaces, canvas IPC fields |
| `src/layout/tile.rs` | `scratch_origin` field; `ipc_layout_template` fills the new `WindowLayout` fields | scratchpad origin travels with the window |
| `src/layout/workspace/canvas_ext.rs`, `floating/canvas_mode.rs`, `scrolling/strip_snapshot.rs` | `canvas_floating_ipc_layouts`, `canvas_stored_positions`, `ipc_column_xs` (read-only) | IPC |
| `src/ipc/{server,client}.rs`, `src/cli.rs` | canvas/strip view diffing, `CanvasViews` request, `WorkspaceModeChanged`, new fields; `niri msg canvas-views` | IPC |
| `src/ui/mru.rs` | skip hidden workspaces | scratchpad |
| `src/input/mod.rs` (scratchpad) | `mod scratch_actions`; one match arm | actions |
| `niri-config/src/workspace.rs` | `hidden` flag + parse tests | config |
| `src/layout/{monitor,mod}.rs`, `niri-config/src/{layout,workspace,lib}.rs` (by index) | see "Workspaces by index" | `create-workspaces-by-index` |
| `niri-config/src/{lib,layout,animations,binds}.rs` | `mod canvas`, layout/animation fields, `Animation::decode_node` made `pub(crate)`, new bind actions; the big `parse` snapshot gained the new default fields | config |

## Public API for follow-up work

`CanvasTransform` (`crate::layout::canvas`): `offset`, `zoom`, `view_to_plane`, `plane_to_view`,
`plane_to_unscaled`, `unscaled_to_plane`, `view_rect_to_plane`, `plane_rect_to_view`,
`visible_plane_rect(view_size)`, `centered_on(plane, view_point, zoom)`,
`zoomed_around(anchor_view, zoom)`, `panned_by_view(delta)`.

`CanvasView`: `current()` (what is rendered/hit-tested), `target()` (where it settles),
`is_animating()`, `is_zoom_animating()`, `animate_to(clock, target, focus_view, pan_cfg,
zoom_cfg)` (retargets from the current state; the plane point under `focus_view` moves
linearly with the pan spring while the zoom follows the zoom spring), `jump_to(t)`,
`advance()`.

`Workspace` (all canvas-only operations are no-ops in strip mode):

- queries: `mode()`, `is_canvas()`, `canvas_view() -> Option<&CanvasView>`,
  `canvas_transform()` / `canvas_target_transform() -> Option<CanvasTransform>`,
  `canvas_zoom()` (1.0 in strip mode), `canvas_is_zoomed()`, `canvas_view_to_plane(view)`,
  `canvas_plane_to_view(plane)`, `canvas_window_plane_rect(id)`, `is_window_pinned(id)`,
  `view_size()`, `working_area()`;
- mode: `toggle_mode()`, `set_mode(mode)`;
- view: `canvas_pan(dir)`, `canvas_pan_to(plane_offset)`, `canvas_zoom_step(zoom_in,
  anchor_view)`, `canvas_set_zoom(zoom, anchor_view)`, `canvas_fit()`, `canvas_home()`,
  `canvas_fly_to_window(id)`, `canvas_reveal_window(id)`, `canvas_edge_push(id)`,
  `toggle_window_pinned(id)`.

`Layout` (focused workspace unless an id is given): `toggle_workspace_mode()`,
`set_workspace_mode(mode)`, `canvas_pan(dir)`, `canvas_pan_to(p)`, `canvas_zoom_in(anchor)`,
`canvas_zoom_out(anchor)`, `canvas_set_zoom(z, anchor)`, `canvas_fit()`, `canvas_home()`,
`canvas_fly_to_window(Option<&id>)`, `canvas_reveal_window(&id)`, `canvas_edge_push(&id)`,
`toggle_window_pinned(Option<&id>)`, `is_window_pinned(&id)`. Anchors are view coordinates
relative to the workspace (for zoom-at-cursor: pointer position within the output minus the
workspace render geometry origin, divided by the overview zoom).

`FloatingSpace`: `canvas_transform()`, `set_canvas_mode(Option<CanvasTransform>)`,
`set_canvas_transform(t)`, `canvas_plane_rects()`, `canvas_plane_rect(id)`,
`set_canvas_plane_pos(id, plane, animate)`, `is_pinned(id)`, `set_pinned(id, bool)`.

### Pointer input under zoom

Implemented, see "Canvas pointer input" below. `Workspace::canvas_window_under()` returns
`HitType::ScaledInput { win_pos, zoom }` (view coordinates) for hits on canvas windows at
zoom ≠ 1, and `Niri::contents_under()` turns it into a pointer focus with scaled surface-local
coordinates.

### IPC

Implemented, see "IPC for the shell" below.

## IPC for the shell: canvas views, strip views, scratchpad

Everything here is append-only and backward compatible: new struct fields carry
`#[serde(default)]` (old payloads without them parse; a new client against an old compositor
sees the defaults), new `Request`/`Response`/`Event`/`Action` variants are appended at the end.
A vendored *old* `niri-ipc` fails to parse the new event variants, so the shell's copy must be
updated together with the compositor (unknown events can be skipped by parsing each line as
`serde_json::Value` first).

### New fields

| Type | Field | Meaning |
| --- | --- | --- |
| `Workspace` | `mode: WorkspaceMode` (`"Strip"` / `"Canvas"`) | window arrangement mode (existed) |
| `Workspace` | `is_hidden: bool` | hidden (scratchpad) workspace, see below |
| `Window` | `hidden_from_workspace_id: Option<u64>` | while the window is on a hidden workspace: the (still existing) workspace it was hidden from; `null` if unknown (opened there) or gone |
| `WindowLayout` | `pos_in_canvas: Option<(f64, f64)>` | top-left of the tile on the canvas plane, for windows on a canvas plane. Plane units = unscaled logical px; the size on the plane is `tile_size`. Does not change while the view pans/zooms |
| `WindowLayout` | `is_pinned: bool` | pinned canvas window (stays on screen while panning) |

`WindowLayout.tile_pos_in_workspace_view` of canvas windows is in real (zoomed) view
coordinates *at the settled view* (target transform), and `null` for tiles outside it — the same
"stationary" semantics as strip windows, so a pan or zoom animation produces one
`WindowLayoutsChanged` batch (when the target changes), not one per frame. For live positions
during an animation combine `pos_in_canvas` with `CanvasViewChanged`:
`view = (pos_in_canvas - offset) * zoom`. Fullscreen/maximized windows of a canvas workspace are
not on the plane (`pos_in_canvas: null`).

```json
{"id":7,"title":"foot","app_id":"foot","pid":4242,"workspace_id":3,"is_focused":true,
 "is_floating":true,"is_urgent":false,
 "layout":{"pos_in_scrolling_layout":null,"tile_size":[802.0,602.0],"window_size":[800,600],
           "tile_pos_in_workspace_view":[611.0,224.0],"window_offset_in_tile":[1.0,1.0],
           "pos_in_canvas":[559.0,239.0],"is_pinned":false},
 "focus_timestamp":{"secs":5321,"nanos":12000000},"hidden_from_workspace_id":null}
```

### Canvas views

`CanvasViewState` (`niri_ipc::CanvasViewState`, also in `niri_ipc::canvas`):

```json
{"workspace_id":3,"offset":[-411.5,-120.25],"zoom":0.62,
 "target_offset":[-300.0,-120.25],"target_zoom":0.62,
 "view_size":[1920.0,1080.0],"animating":true,
 "regions":[{"name":"notes","x":-400.0,"y":0.0,"w":800.0,"h":600.0}]}
```

`regions` (serde default `[]`) are the named plane rectangles of the workspace (config
`canvas { region ... }` plus `canvas-add-region` / `canvas-remove-region`), in plane units, for
the minimap. A region change is sent right away as a `CanvasViewChanged` for that workspace.

`view = (plane - offset) * zoom`, `plane = view / zoom + offset`; view coordinates are logical
px relative to the workspace's top-left on its output (= the output for the active workspace
outside the overview). `view_size` is the output size. `offset`/`zoom` are what is on screen
right now, `target_*` where the view settles.

- `Request::CanvasViews` → `Response::CanvasViews(Vec<CanvasViewState>)`: every canvas-mode
  workspace (hidden ones included), read live on the compositor thread.
  Request JSON: `"CanvasViews"`. CLI: `niri msg canvas-views` / `niri msg --json canvas-views`.
- `Event::CanvasViewsChanged { views }`: the full set; sent on event-stream connect and
  whenever a workspace enters/leaves canvas mode or a canvas workspace is created/removed.
- `Event::CanvasViewChanged { view }`: one workspace's view changed. While the view animates
  (pan, zoom, fit, reveal, edge-push, pan/zoom gestures), changes are sent **at most once per
  frame of the workspace's output** (compared against the last sent state in the IPC refresh,
  keyed on the output's frame-callback sequence). The state where the animation ends
  (`animating: false`) and view-size changes are always sent right away.

```json
{"CanvasViewsChanged":{"views":[{"workspace_id":3,"offset":[0.0,0.0],"zoom":1.0,"target_offset":[-588.4,-331.9],"target_zoom":0.62,"view_size":[1920.0,1080.0],"animating":true}]}}
{"CanvasViewChanged":{"view":{"workspace_id":3,"offset":[-300.1,-169.3],"zoom":0.71,"target_offset":[-588.4,-331.9],"target_zoom":0.62,"view_size":[1920.0,1080.0],"animating":true}}}
{"CanvasViewChanged":{"view":{"workspace_id":3,"offset":[-588.4,-331.9],"zoom":0.62,"target_offset":[-588.4,-331.9],"target_zoom":0.62,"view_size":[1920.0,1080.0],"animating":false}}}
```

- `Event::WorkspaceModeChanged { id, mode }` replaces the full `WorkspacesChanged` that a mode
  toggle used to send: `{"WorkspaceModeChanged":{"id":3,"mode":"Canvas"}}`. It is followed by
  `CanvasViewsChanged` / `StripViewsChanged` (the sets changed) and `WindowLayoutsChanged`
  (windows got or lost `pos_in_canvas`).

Reducer: `niri_ipc::state::EventStreamState` has two new parts, `canvas: CanvasState`
(`views: HashMap<workspace id, CanvasViewState>`) and `strip: StripState`
(`views: HashMap<workspace id, StripViewState>`); `WorkspacesState` applies
`WorkspaceModeChanged`.

### Strip header

Column order, widths and the focused column are derivable from the existing data:
`WindowLayout.pos_in_scrolling_layout` = (column, tile), 1-based; the column width is the
`tile_size.0` of its tiles; the focused column is the one of the workspace's
`active_window_id` (or of the window with `is_focused`). What was missing is the *live* scroll
position, now in `StripViewState`:

```json
{"workspace_id":1,"view_offset":812.3,"target_view_offset":976.0,
 "working_area_width":1920.0,"column_xs":[0.0,976.0,1952.0]}
```

Strip coordinates: x = 0 at the left edge of the first column, columns separated by the gaps;
`column_xs[i]` is the strip x of column `i + 1`. Column `i + 1` is drawn at workspace view x
`column_xs[i] - view_offset` right now (the tile's own x offset inside its column aside).
`target_view_offset` is where the view settles.

- `Event::StripViewsChanged { views }`: full set (all strip-mode workspaces); sent on connect and
  whenever the set changes.
- `Event::StripViewChanged { workspace_id, view_offset, target_view_offset, working_area_width,
  column_xs }`: one workspace changed. Scrolling animations and gestures are limited to one event
  per output frame; the settled position and column-layout/working-area changes are sent
  immediately. `{"StripViewChanged":{"workspace_id":1,"view_offset":900.0,"target_view_offset":976.0,"working_area_width":1920.0,"column_xs":[0.0,976.0,1952.0]}}`

### Hidden workspaces (scratchpad)

```kdl
workspace "scratch" {
    hidden
}
```

A named workspace with `hidden` keeps its place in its monitor's workspace list, but it does
not count for index references: `focus-workspace N`, `move-window/column-to-workspace N`,
`move-workspace-to-index N`, `set/unset-workspace-name` with an index and IPC `Workspace.idx` use
1-based *visual* indices over the visible workspaces of the monitor (see "Workspace indices"
below). Hidden workspaces have IPC `idx` 0 and are addressed by name or id. Moreover:

- it never becomes the active workspace: `focus-workspace "scratch"` (by name or id) is a
  **no-op** (it may still move the focus to the monitor the scratch workspace is on);
  `focus-workspace-up/down`, `focus-window-or-workspace-up/down`, `focus-workspace-previous`,
  `move-window/column-to-workspace-up/down`, `move-workspace-up/down`, workspace-switch
  gestures (touchpad, mouse, DnD edge scroll in the overview) and their bounds skip it;
- it is not rendered and takes no space, normally and in the overview (switch animations and
  gestures run in "visual" indices that count only visible workspaces);
- its windows are not in the recent-windows switcher; activation (`focus-window --id`,
  xdg-activation / urgency, focus-follows-mouse) does not show or focus them;
- it is never removed while it is named (its config entry keeps it); unnaming it (config entry
  removed, `unset-workspace-name`) makes it a normal workspace. Adding `hidden` to the config entry
  of the currently active workspace moves the monitor to the nearest visible workspace.

Hiding and showing:

| Bind action | IPC action | Behaviour |
| --- | --- | --- |
| `hide-window` | `HideWindow { id }` (`--id` optional, default focused) | to the first hidden workspace of the window's monitor (else of any monitor), without following it |
| `unhide-window` | `UnhideWindow { id }` (`--id` optional, default *most recently hidden*) | back to the workspace it was hidden from if that still exists (else the focused workspace), then focused like `focus-window --id` |

The generic actions work too and remember the origin the same way, e.g.
`niri msg action move-window-to-workspace --window-id 7 --focus false scratch` to hide and
`niri msg action move-window-to-workspace --window-id 7 3` + `niri msg action focus-window --id 7`
to show. Every window carries where it was last removed from (workspace and, on a canvas, its
plane position): returning from a hidden workspace to *that* canvas puts it back at its plane
position; any other canvas gets it at the viewport centre; a strip workspace gets it as a new
column right of the focused one.

For the scratchpad list: `Windows` / `WindowOpenedOrChanged` report hidden windows with the
hidden workspace's `workspace_id` and `hidden_from_workspace_id`; `Workspace.is_hidden` tells
which workspaces are hidden. `niri msg workspaces` lists them last, with `-` instead of an
index, marked `(hidden)` (and canvas ones `(canvas)`).

The ext-workspace protocol (`ext_workspace.rs`) does not announce hidden workspaces; its
coordinates and the default names of unnamed workspaces use the visual index.

## Building and running nested

There is no global Rust on the dev machine; use the flake's dev shell:

```sh
nix develop               # or: nix print-dev-env > env.sh; source env.sh (bash)
export CARGO_TARGET_DIR=$HOME/.cache/niri-target   # optional: keep build output out of the tree
cargo build -j 6
cargo test -j 6 --workspace
RUN_SLOW_TESTS=1 cargo test -j 6 --release --lib layout::   # randomized + exhaustive layout tests
cargo clippy -j 6 --workspace --all-targets
cargo fmt --all -- --check
```

Never run a development build against the live session (no TTY/DRM, never with the inherited
`WAYLAND_DISPLAY` or `NIRI_SOCKET`). Runtime checks use the nested session of the ashd test
harness (`nix-new/ashd/xtask`, see its README): `NIRI=<debug niri> SWAY=… cargo xtask nested
start --niri-config test.kdl` prints a root `/tmp/ashd-nested.XXXXXX`; `cargo xtask nested env
<root>` gives the scrubbed environment (headless sway, niri on winit inside it, private
`XDG_RUNTIME_DIR`). Before every `niri msg …` check that `NIRI_SOCKET` lies under that root.
Pointer and keyboard input can be driven with any `zwlr_virtual_pointer_v1` /
`zwp_virtual_keyboard_v1` client connected to the nested `WAYLAND_DISPLAY` (in the nested
session Mod is Alt). `niri msg action screenshot-screen --path <file> --copy-to-clipboard false
--notify false` takes stills.

## Known limitations and deviations

- Mod+drag of a canvas window: the dragged tile is semi-transparent (canvas windows count as
  tiled once they leave the workspace, so that moving them to a strip workspace tiles them).
  Pointer-input limitations under zoom are listed in "Canvas pointer input".
- Deviation: a window that is floating on a *strip* workspace and moved by keyboard onto a
  canvas keeps its relative position in the viewport (stock floating semantics) instead of
  landing at the viewport centre; tiled windows and windows from other canvases land centred.
- Pinned windows scale with the canvas; their unscaled view position is kept, so zooming moves
  them towards the view origin.
- Frame width compensation applies to the layout-level and the window-rule `border`/`focus-ring`
  widths. It is applied when the zoom settles, so frames are thin during the zoom animation.
- `toggle-window-floating`, `move-window-to-floating/tiling` are no-ops for canvas windows. Column
  actions (consume/expel, column width presets, tabbed display) act on the floating window like
  on a stock floating window, or do nothing.
- Windows opened while in canvas mode become strip columns with a fixed width equal to their
  current width when going back to strip mode (unless `open-floating true`).
- The close animation of a *pinned* canvas window moves with the plane if the view pans during
  it (unpinned windows stay on their plane position, as they should).
- In the overview, canvas windows outside the viewport show next to the workspace horizontally,
  like strip columns do (niri only crops vertically there).
- Render culling keeps tiles within half a view size of the viewport; popups extending further
  from an off-screen parent are not drawn.
- Changing `default-workspace-mode` affects workspaces created afterwards only.
- Performance was not measured with a profiler; see "Hot path" under "Fork polish" for CPU
  numbers of the nested debug build. Every canvas frame re-renders all visible canvas tiles
  (positions change every frame), the same cost class as a strip view-offset animation.

## Follow-ups

- ~~Scaled pointer input, Mod+drag pan, Mod+wheel zoom-at-cursor~~: done, see "Canvas
  pointer input" below. Open: touch input and pointer constraints at zoom ≠ 1.
- ~~IPC events and requests; hidden scratchpad workspace~~: done, see "IPC for the shell" above.
- ~~Plane grid / dot background, named regions, title strips~~: done, see "Decorations" below.

## Canvas pointer input

Real pointer input to windows at any canvas zoom, mouse/touchpad pan and zoom.

### Behaviour

- **Scaled input.** Clicks, motion, scroll, enter/leave and drag-and-drop reach canvas windows
  at any zoom with surface-local coordinates `(pos − win_view_pos) / zoom`. Popups and
  subsurfaces are hit-tested in the same unscaled window coordinates (`Window::surface_under`
  with `WindowSurfaceType::ALL`), so they are consistent with what is drawn. While a button is
  held (implicit grab, e.g. selecting text by dragging), motion keeps being scaled, also outside
  the window. Relative-pointer deltas are divided by the zoom. The cursor image is not scaled.
  At zoom 1 hits are the stock `HitType::Input`, bit-identical to upstream.
- **Drag-and-drop** onto scaled windows targets the right surface with scaled coordinates
  (smithay's DnD grab uses the per-motion focus location, see the mechanism below).
- **Interactive move** (Mod+LMB, or a client move request): over a canvas workspace the dragged
  tile is drawn at that canvas's zoom (and scaled input hit-tests it), so the grabbed point stays
  under the pointer and the window lands exactly where it is drawn. Over strip workspaces or the
  overview it is drawn as before.
- **Interactive resize** (Mod+RMB, double-click gestures, and client resize requests from CSD
  edges): resize edges are found through the zoom, and pointer deltas are divided by the zoom
  for canvas windows (pinned ones too).
- **Mouse pan** (`src/input/canvas_pan_grab.rs`, `CanvasPanGrab`): Mod+MMB anywhere on a canvas
  workspace (instead of the stock view-offset / workspace-switch gesture) and Mod+LMB on the
  empty plane (also over background/bottom layer surfaces, not over top/overlay ones) pan the
  canvas 1:1 with the pointer (plane delta = view delta / zoom, f64, no rounding). Relative
  motion is preferred and the pointer wraps around the output edges in both axes, like the stock
  Mod+MMB view-offset gesture. On release the view glides with the release velocity: two
  `SwipeTracker`s (x, y) measure it, a zero-delta sample at the release time drops stale history
  (holding still before releasing does not glide), the glide distance is the tracker's projected
  end position (touchpad deceleration), and the pan spring starts with the release velocity
  (`CanvasView::glide`). No glide while a zoom animation runs, or with the pan animation `off`.
- **Touchpad**: on a canvas workspace the horizontal 3-finger swipe (the one that scrolls the
  strip) pans the canvas in 2D (both axes, `working-area width / 1200` view px per unit like the
  strip gesture), with the same glide on release (none if libinput cancels the gesture). The
  vertical 3-finger swipe still switches workspaces; 4 fingers still open the overview.
- **Pinch zoom**: a touchpad pinch over a canvas zooms continuously around the cursor (zoom =
  start zoom × pinch scale, with `pinch-sensitivity`, clamped to `zoom-steps`) when it has 3+
  fingers, or Mod is held, or it starts over the empty plane. Two-finger pinches over windows go
  to the window as before. Frame-width compensation is applied when the pinch ends.
- **Zoom at cursor**: `canvas-zoom-in at-cursor=true` / `canvas-zoom-out at-cursor=true`
  (IPC: `CanvasZoomIn { at_cursor }`, CLI `--at-cursor`, `serde(default)`) step the zoom of the
  canvas workspace under the cursor keeping the plane point under the cursor fixed; without the
  flag, or when the cursor is not over a canvas, they act on the focused workspace around the
  working area centre as before.
- **`if-canvas`** (config only): `if-canvas { <canvas action>; <strip action>; }` runs the
  first child if the focused workspace is a canvas and the second (optional) one otherwise, so
  one key does different things per mode. One or two children, otherwise a config error.
- **Focus-follows-mouse and warp**: FFM targets the window drawn under the pointer at any zoom
  and never recentres the view (pointer focus never reveals). warp-mouse-to-focus (and the
  tablet's map-to-focused-window) use the active canvas window's rectangle at the *target* view,
  so after a keyboard reveal the pointer lands where the window ends up.

Recommended binds (not in the default config; wheel binds are user config):

```kdl
binds {
    // Mod+wheel: zoom at the cursor on canvas workspaces, switch workspaces on strip ones.
    Mod+WheelScrollUp   cooldown-ms=150 { if-canvas { canvas-zoom-in  at-cursor=true; focus-workspace-up;   }; }
    Mod+WheelScrollDown cooldown-ms=150 { if-canvas { canvas-zoom-out at-cursor=true; focus-workspace-down; }; }
}
```

### Mechanism

- `HitType::ScaledInput { win_pos, zoom }` (appended to the enum; `win_pos` is the buffer
  origin as drawn, `offset_win_pos` shifts it, `to_activate` downgrades it, `HitType::scaled(z)`
  converts an unscaled `Input` hit). `Monitor::window_under` downgrades it in the overview like
  every other hit.
- `niri::scaled_surface_under()`: `local = (pos − win_pos)/zoom − subsurface_offset`, and the
  focus location handed to smithay is `pos − local`, so smithay's `pos − location` yields the
  scaled local position. It depends on `pos`; this works because the pointer focus is
  recomputed on every motion event (`contents_under()`), and smithay's DnD grab and xdg popup
  grab use that per-motion focus too. `ClickGrab` (implicit grab while a button is held) gets
  the zoom from `PointContents::scaled_input_zoom()` (`SeatHandler::click_grab` in
  `src/handlers/mod.rs`) and recomputes the location per motion from the surface origin at the
  press.
- Gesture state lives in `WorkspaceCanvas` (`pan_gesture: Option<CanvasPanGesture>`,
  `zoom_gesture: Option<f64>`); `CanvasView::pan_by_plane()` shifts the current and target
  transforms and a running animation's plane points together, so a zoom animation continues
  while dragging.

### API

`Layout`: `canvas_workspace_under(output, pos) -> Option<(WorkspaceId, anchor)>`,
`canvas_zoom_step_at(ws_id, zoom_in, anchor)`, `canvas_pan_gesture_begin(ws_id, is_touchpad)`,
`canvas_pan_gesture_update(delta, timestamp, is_touchpad)`,
`canvas_pan_gesture_end(Option<release_time>, Option<is_touchpad>)`,
`canvas_zoom_gesture_begin(ws_id)`, `canvas_zoom_gesture_update(scale, Option<anchor>)`,
`canvas_zoom_gesture_end()` (updates/ends return `None` when no such gesture is ongoing, else the
output to redraw). `Workspace`: the same per workspace plus `has_canvas_pan_gesture`,
`has_canvas_zoom_gesture`. `CanvasView`: `glide(clock, plane_delta, plane_speed, focus, cfg)`,
`pan_by_plane(delta)`. `Niri::canvas_under_cursor()`, `Niri::pointer_over_top_layer()`,
`PointContents::{scaled_input_zoom, without_scaled_input}`.

### Files

| File | Change |
| --- | --- |
| `src/input/canvas_pan_grab.rs` (new) | `CanvasPanGrab` |
| `src/tests/canvas_input.rs` (new) | client-level tests: click/motion/leave at zoom 0.5, drag while a button is held, zoom 1 unchanged, scroll at 0.62 |
| `src/input/canvas_actions.rs` | at-cursor zoom, pinch helpers, `Niri::canvas_under_cursor`, `pointer_over_top_layer` |
| `src/input/click_grab.rs` | scaled focus location while a button is held |
| `src/input/mod.rs` | Mod+MMB / Mod+LMB canvas pan, pointer wrap, relative-motion scaling, touch fallback, touchpad 3-finger pan, pinch, `if-canvas` |
| `src/niri.rs` | `ScaledInput` in `contents_under`, `scaled_surface_under`, `PointContents` helpers, no constraint activation on scaled windows, window-cast cursor position |
| `src/handlers/mod.rs` | `click_grab` passes the zoom (3 lines) |
| `src/layout/mod.rs` | `HitType::ScaledInput`, `HitType::scaled`; interactive move drawn/hit/dropped at the canvas zoom |
| `src/layout/canvas.rs` | `CanvasView::{glide, pan_by_plane}`, gesture state, Layout API above |
| `src/layout/workspace/canvas_ext.rs` | scaled hits, `canvas_resize_edges_under`, `canvas_resize_delta`, gestures, warp rectangle at the target view |
| `src/layout/workspace.rs` | `resize_edges_under` and `interactive_resize_update` hooks (2 lines) |
| `niri-config/src/binds.rs`, `niri-ipc/src/lib.rs` | `at-cursor` flag, `if-canvas` |
| `src/layout/tests.rs`, `src/layout/tests/canvas.rs`, `src/tests/client.rs`, `niri-config/src/canvas.rs` | gesture ops in the randomized tests, layout tests, `wl_pointer` recording in the test client, config parse tests |

### Limitations

- **Touch** at zoom ≠ 1 is activate-only (tapping focuses the window; no touch events reach it):
  smithay keeps the touch-down focus location for the whole touch sequence, so scaled
  coordinates cannot be delivered. Touch at zoom 1 is unchanged.
- **Pointer constraints** (lock/confine) are not activated on windows drawn scaled. A
  constraint that was already active when the zoom changed keeps working, with confinement
  regions checked against unscaled motion.
- **Tablet** tools get scaled coordinates (their focus is recomputed per event); not tested on
  hardware.
- The interactive-move tile animates its size (canvas-zoom animation) when it crosses between
  workspaces with different zooms; popups of the moved window are positioned for the unscaled
  window.
- Pinch zoom needs 3+ fingers, Mod, or the empty plane, to keep two-finger pinches for apps.
- `if-canvas` checks the focused workspace, also for pointer binds.
- Runtime-tested in the nested session (winit backend, virtual pointer/keyboard) with foot:
  clicks, typing, scroll, drag-selection at 0.62 and Mod+MMB / Mod+LMB pan, Mod+RMB resize,
  Mod+LMB move, Mod+wheel zoom at cursor, `if-canvas` fall-through, focus-follows-mouse. The
  glide was only covered by layout tests (the nested config has animations off); touchpad
  gestures were not tested on hardware.

## Decorations: title strips, focus ring gap, canvas grid and regions

Compositor-drawn window chrome and canvas plane decorations for the ashd desktop design
(`docs/design/pages/desk-05-*`, `sys-04-*`). Everything is **off by default**: without the
options below, tiles, focus rings and canvas workspaces render exactly like before (no strip,
gap 0, no grid, no regions, no extra render elements on strip workspaces).

### Config reference

```kdl
layout {
    // Title strip above every window. Off unless this block is present (`off` inside turns it
    // off again, e.g. in a per-output or per-workspace layout override).
    title-bar {
        height 28                 // logical px, includes the bottom line
        font "JetBrains Mono 11"  // Pango font description; the size is in logical px
        padding 12                // left/right padding, also the title/subtitle gap
        background-color "#1B1B19"
        text-color "#DAD7CF"      // window title, left, ellipsized
        muted-color "#96938B"     // subtitle (app id), right, at most half the width
        line-color "#2A2A27"      // bottom line
        line-width 1              // 0 hides the line
    }

    focus-ring {
        gap 4                     // space between the window and the ring (default 0)
        // How focus is shown on canvas workspaces:
        //   "ring"   (default) same ring as on strip workspaces,
        //   "border" no ring; the window border takes the ring's active colour/gradient,
        //   "off"    no ring.
        canvas-style "border"
    }

    canvas {
        grid "lines"              // "lines", "dots" or "off" (default)
        grid-spacing 80           // plane px between lines / dots
        grid-color "#161615"
        grid-line-width 1         // line width, or dot diameter for "dots" (min 2 physical px)
        background-color "#111110" // plane fill under the grid (opaque: hides the backdrop)
        region-color "#2A2A27"    // dashed region outlines
        region-label-color "#96938B"
        region-font "JetBrains Mono 16" // caps label, size in plane px
        region "dots" x=740 y=320 w=1740 h=1210 // plane coordinates
    }
}

window-rule {
    match app-id="firefox"
    title-bar false               // apps that draw their own header; `true` forces a strip
}
```

`title-bar`, `focus-ring` and `canvas` are layout options: they merge per output and per named
workspace (`workspace "dots" { layout { canvas { region ... } } }`). Regions of a more specific
section replace the less specific ones; a later `region` with the same name replaces an earlier
one. `gap` and `canvas-style` are also accepted in window-rule `focus-ring {}` blocks.

Actions (`niri msg action …`, binds), acting on the focused workspace; appended at the end of
both `Action` enums:

| Bind action | IPC action | Notes |
| --- | --- | --- |
| `canvas-add-region "name" X Y W H` | `CanvasAddRegion { name, x, y, w, h }` | adds, or replaces by name (config regions too) |
| `canvas-remove-region "name"` | `CanvasRemoveRegion { name }` | also hides a config region until the next restart |

Regions are not exposed in IPC state yet (follow-up for the IPC work:
`Workspace::canvas_regions()` returns the effective list).

### Recommended config (Ash), both modes

The design wants a 1 px line-coloured frame on every window, a 1 px accent ring 4 px outside
the focused window in strip mode, and the window's own frame turning accent on canvases:

```kdl
layout {
    gaps 12
    background-color "#111110"
    shadow { off; }
    border {
        width 1
        active-color "#2A2A27"
        inactive-color "#2A2A27"
    }
    focus-ring {
        width 1
        gap 4
        active-color "#E0A458"
        inactive-color "#2A2A27"
        canvas-style "border"
    }
    title-bar {
        height 28
        font "JetBrains Mono 11"
        padding 12
        background-color "#1B1B19"
        text-color "#DAD7CF"
        muted-color "#96938B"
        line-color "#2A2A27"
    }
    canvas {
        grid "lines"            // dot themes: grid "dots"; grid-spacing 40; grid-color <line>; grid-line-width 2
        grid-spacing 80
        grid-color "#161615"
        grid-line-width 1
        background-color "#111110"
        region-color "#2A2A27"
        region-label-color "#96938B"
        region-font "JetBrains Mono 16"
    }
}
```

A canvas-only setup without `canvas-style` works too: `border { width 1; active-color
"#E0A458"; inactive-color "#2A2A27"; }` with `focus-ring { off; }` gives the "own border turns
accent" look everywhere. Theme switching: put the block above in an included KDL file that
the shell rewrites; niri reloads it and every colour (strip, text, ring, border, grid,
regions) updates live (text textures are keyed by their colours and re-rendered).

### Behaviour

- **Geometry**: tile = border + strip + window + border (vertically). Every tile ↔ window
  conversion accounts for the strip: `tile_size`, `tile_expected_or_current_size`,
  `window_loc`, `request_tile_size`, `tile/window_height_for_*`, min/max size, the resize
  animation start size, and new-window sizes in the scrolling and floating spaces. Fullscreen
  and maximized-to-edges tiles have no strip (it shrinks away with the expand animation);
  `maximize-column` keeps it. Floating and canvas windows have it; in tabbed columns every
  tile has its strip and only the visible tile is drawn. Heights are rounded to physical
  pixels (`28 * 1.5 = 42`, `27 * 1.5 = 40.5 → 41` physical px).
- **Rendering**: background (rounded with the window's top radius via the border shader when
  the window has one; the window content then gets square top corners), the bottom line, and
  one text texture per (title, subtitle, width, scale, font, colours), re-rendered only when
  one of those changes (title changes queue a redraw). On a canvas the text is rasterized at
  output scale × the settled zoom, so it is crisp once the zoom settles (scaled during the
  zoom animation). Blocked-out windows hide the text in screencasts. Open/close/resize/alpha
  animations and output screenshots include the strip; window-only screenshots/casts don't
  (like borders).
- **Input**: the strip is part of the tile for hit-testing (`HitType::TitleBar`, kept through
  canvas zoom and the overview downgrade). Left press activates and starts a move grab
  (dragging moves the window like Mod+drag, keeping the grab point on the strip; a click without
  drag just activates); a double click toggles `maximize-column` in strip mode, and on a canvas
  zooms to 100% centred on the window, or fits all windows when already at 100%. Other buttons
  do nothing (they don't activate the window either). Mod+RMB resize edges are computed on the
  whole tile as before, so Mod+RMB on the strip resizes from the top edge.
- **Focus ring gap**: the ring is drawn around the tile grown by `gap` on every side, always as
  a border (never as a background fill, which would paint the gap), with concentric corners
  (outer radius = window radius + gap + width; square windows get `gap + width`). Gap is
  rounded to physical pixels; under canvas zoom it scales with the tile. The tab indicator of a
  tabbed column is moved out by the ring's gap and width (and gets that much more room with
  `place-within-column`), unless its own `gap` is negative.
- **Canvas grid**: one shader element (`canvas_grid.frag`) covering the workspace view under
  all windows, computed from the current canvas transform: pixel-snapped lines at least one
  physical pixel wide, or anti-aliased dots (min 2 physical px), fading out when the spacing
  drops below ~12 physical px (gone at 4). With `background-color` it is opaque. Its uniforms
  only change when the view pans or zooms, so a static canvas causes no grid damage.
- **Regions**: four dashed edge elements (same shader) plus a caps label (`+0.08em` letter
  spacing) 14/8 plane px inside the top-left corner, all scaled with the plane; culled when
  off screen.

### Files

| File | Change |
| --- | --- |
| `niri-config/src/decorations.rs` (new) | `TitleBar(Part)`, `FocusRingCanvasStyle`, `CanvasGrid`, `CanvasDecor(Part)`, `CanvasRegion`, region node parsing (`DecodePartial`), parse tests |
| `niri-config/src/appearance.rs` | `FocusRing.gap/canvas_style`, `BorderRule.gap/canvas_style`, merging keeps them |
| `niri-config/src/{layout,window_rule,lib}.rs` | `layout.title_bar`, window rule `title-bar`, module + re-exports, parse snapshot |
| `niri-config/src/canvas.rs` (input agent's) | one flattened `decor` field in `CanvasPart`/`Canvas` + merge + 2 test literals |
| `niri-config/src/binds.rs` (input agent's), `niri-ipc/src/lib.rs` (IPC agent's) | `CanvasAddRegion`, `CanvasRemoveRegion` appended + conversion |
| `src/layout/title_bar.rs` (new) | strip state, render elements, text texture cache, `title_bar_height_for` |
| `src/layout/canvas_grid.rs` (new) | grid/edge shader element, `CanvasDecorations`, labels, `Layout::canvas_add/remove_region` |
| `src/layout/workspace/canvas_decor.rs` (new) | `Workspace` hooks: update/render decorations, region API |
| `src/render_helpers/shaders/canvas_grid.frag` (new), `shaders/mod.rs` | shader + `ProgramType::CanvasGrid` |
| `src/layout/tile.rs` | strip geometry, rendering, hit-testing, canvas focus style, ring gap rounding |
| `src/layout/focus_ring.rs` | gap |
| `src/layout/{scrolling,floating}.rs` | new-window heights minus the strip; column resizes on strip changes; `FloatingSpace::options()` no longer test-only |
| `src/layout/workspace.rs`, `workspace/canvas_ext.rs`, `canvas.rs` | `mod canvas_decor`, render element variant, update/render/shader hooks; canvas tiles get `Options::canvas_tile_zoom`; `WorkspaceCanvas.decor` |
| `src/layout/mod.rs` (input agent's) | `mod title_bar/canvas_grid`, `LayoutElement::title_bar_text()` (default `None`), `HitType::TitleBar`, `Options::canvas_tile_zoom` |
| `src/input/title_bar_click.rs`, `src/input/decor_actions.rs` (new), `src/input/mod.rs` | title bar press/drag/double click, region actions |
| `src/window/{mapped,mod}.rs`, `src/handlers/xdg_shell.rs`, `src/layout/insert_hint_element.rs` | title text, rule, double-click time, redraw on title change, literal fix |
| `src/layout/tests/decorations.rs` (new), `src/layout/tests.rs` | geometry/hit/canvas/region tests; title bar, ring gap and canvas style in the randomized configs |

### Limitations

- Pointer interaction with the strip was verified in the nested session with a virtual pointer
  and keyboard (see "Fork polish" below) and is covered by client-level tests.
- During a fullscreen/maximize animation the strip fades out at full height above the window
  instead of shrinking.
- Strip text is not re-rasterized for the overview zoom (scaled texture).
- `canvas-remove-region` of a config region lasts until niri restarts; regions are not in IPC
  state yet; region labels are not hit-testable.
- New-window size presets subtract the unrounded strip height (like niri does for borders),
  so at fractional scales a preset can be off by under one physical pixel.

## Fork polish: indices, focus, quiet screenshots, work area, struts, strip input

Fixes for rough edges reported by the shell and config authors. Everything that changes
behaviour is opt-in, except the workspace indices with hidden workspaces (no change without
hidden workspaces), the per-side `struts` merge, the focus fixes and the cosmetic fixes.

### Workspace indices

With hidden workspaces, index references count the *visible* workspaces only:

- `focus-workspace N`, `move-window-to-workspace N`, `move-column-to-workspace N`,
  `move-workspace-to-index N` (both the target index and `--reference N`), the workspace reference
  of `move-workspace-to-monitor`, `set-workspace-name --workspace N`, `unset-workspace-name N`: N
  is 1…n over the visible
  workspaces of the active monitor (of the window's monitor for
  `move-window-to-workspace --window-id`). Indices past the end still clamp to the last
  workspace like upstream, unless `create-workspaces-by-index` is on (see "Workspaces by
  index" below).
- IPC `Workspace.idx`: the same visual index; **0 for hidden workspaces** (address them by
  name or id). `niri msg workspaces` lists hidden workspaces last, with `-` as index.
- ext-workspace: hidden workspaces are not announced; coordinates/default names are visual.
- Switch animations, gestures, the hot corner and the overview already used visual indices.

So the NixOS config binds `Super+N` to `focus-workspace N` again (drop the `N+1` workaround),
on every monitor.

Code: `Layout::resolve_workspace_index`, `Layout::ipc_workspace_idx`,
`scratch::{visual_ws_idx, real_ws_idx}` (`src/layout/scratch.rs`),
`Niri::find_output_and_workspace_index_for` (`src/niri.rs`). The layout keeps working with real
indices; conversion happens where references are resolved.

### On-demand layer focus

A layer surface with on-demand keyboard interactivity keeps the keyboard after it was clicked.
Explicit window focus now takes it back, like upstream's directional and workspace focus
actions already did: `focus-window`, `focus-window-previous`, `unhide-window`, the
recent-windows switcher (all through `State::focus_window()`), `focus-floating`,
`focus-tiling`, `switch-focus-between-floating-and-tiling`, and any action after which the
focused workspace is a different one (e.g. `move-window-to-workspace` with focus). xdg-activation,
foreign-toplevel activation and focus-follows-mouse into a window already cleared it (now covered
by tests, including FFM back into the window that kept the layout focus). Clicks keep upstream
behaviour; plain pointer motion without focus-follows-mouse leaves the layer focus alone.
Code: `src/input/layer_focus_hook.rs`; tests: `src/tests/ashd_focus.rs`.

### Quiet screenshots

| IPC action | New fields (appended, `#[serde(default = true)]`) | CLI | bind property |
| --- | --- | --- | --- |
| `Screenshot { show_pointer, path, .. }` | `copy_to_clipboard: bool`, `notify: bool` | `--copy-to-clipboard <bool>`, `--notify <bool>` | `copy-to-clipboard=false`, `notify=false` |
| `ScreenshotScreen { write_to_disk, show_pointer, path, .. }` | same | same | same |
| `ScreenshotWindow { id, write_to_disk, show_pointer, path, .. }` | same | same | same |

`copy_to_clipboard: false` skips setting the clipboard selection. `notify: false` skips niri's
"Screenshot captured" desktop notification (the `org.freedesktop.Notifications.Notify` D-Bus call
in `utils::show_screenshot_notification`, the only user-visible notification); the
`ScreenshotCaptured` IPC event is still sent. The CLI flags follow niri's convention for
`--write-to-disk` (explicit boolean values). For the interactive screenshot UI the choice is kept
until it is confirmed. Old requests and existing binds behave as before.

```sh
niri msg action screenshot-screen --write-to-disk true --show-pointer false \
    --path /run/user/1000/ashd/still.png --copy-to-clipboard false --notify false
```
`{"Action":{"ScreenshotScreen":{"write_to_disk":true,"show_pointer":false,"path":"/run/user/1000/ashd/still.png","copy_to_clipboard":false,"notify":false}}}`

### Work area outline and clip

```kdl
layout {
    work-area {
        outline-width 1          // 0 (default): no outline
        outline-color "#2A2A27"
        corner-radius 12         // outline and clip
        clip true                // default false
    }
}
```

Off by default; a layout option, so it merges per output and per named workspace. The working
area is the output minus layer-shell exclusive zones **minus the struts** (floating themes inset
the main area with `struts`).

- `clip true`: strip tiles (with title strips, borders, focus rings) and the canvas plane
  (windows, pinned windows, grid, background, regions and labels) are cropped to the working
  area, per workspace in workspace-local coordinates, so the clip follows workspace switch
  animations and the overview zoom. Not clipped: floating windows on strip workspaces, windows
  moving between workspaces, the interactive-move tile, and fullscreen or maximized-to-edges
  windows — while one of those is in view the workspace has no clip and no outline.
- `corner-radius`: content is cropped to the rectangle (smithay `CropRenderElement`); each
  corner square then gets a mask in the layout `background-color` outside the rounded corner
  (border shader). With a wallpaper (background layer surface) these few corner pixels show the
  background colour instead of the wallpaper.
- `outline-width`/`outline-color`: drawn along the inside of the working area above the
  content, like a border-style focus ring (4 edges + 4 corner pieces).

Why no offscreen pass: an offscreen render of the workspace would allow an exact rounded clip
over any background, but it costs a full extra pass over the workspace every frame it changes.
Cropping adds no pass, keeps per-element damage, and the outline/masks are static elements that
only change with the working area or the config, so a static workspace gets no extra damage.
The grid and region-outline shaders compute their pattern from the element size, so on a
clipped canvas they are laid out inside the clip instead of being cropped.

Code: `src/layout/work_area.rs` (`WorkAreaDecor`, `WorkAreaRenderElement`),
`src/layout/workspace/work_area_ext.rs`, `Monitor::render_workspaces` (per-element crop),
`CanvasDecorations::update_render_elements(.., clip)`, `ScrollingSpace::working_area()`,
config `niri-config/src/decorations.rs` (`WorkArea`, `WorkAreaPart`). Tests:
`src/layout/tests/work_area.rs`, randomized configs (`arbitrary_work_area`,
`Op::UpdateRenderElements`).

### Struts merge per side

`layout { struts { … } }` merges per side: a later `struts {}` block — in an included file, or in
a per-output / per-workspace `layout {}` override — only overrides the sides it names. A theme
fragment can set `struts { top 58; }` and keep the host's `left`. (Upstream replaced the whole
block; a single `struts {}` block parses as before.) Config type: `LayoutPart.struts:
Option<StrutsPart>` with `Option` sides; `Layout.struts: Struts` is unchanged.

### Title strip pointer behaviour (verified)

Verified in the nested session with a virtual pointer and keyboard (`zwlr_virtual_pointer_v1`,
`zwp_virtual_keyboard_v1`; Mod = Alt nested):

| Check | Result |
| --- | --- |
| press on the strip of an unfocused window | activates it |
| strip of an unfocused tile of a stacked column | activates that tile |
| drag in strip mode | interactive move like Mod+drag (dropped into another column) |
| drag on a canvas at 0.62 and 0.4 | moves on the plane by exactly pointer delta / zoom |
| double click, strip mode | toggles `maximize-column` (width 1896 ↔ 751) |
| double click, canvas | 0.62 → 100% centred on the window; at 100% → fit |
| right click | nothing (**fixed**: it used to activate the window) |
| Mod+RMB on the strip | resize from the top edge wins over the strip |

Fixed as well: a drag that started on the strip made the window jump up by the distance between
the pointer and the window top (the interactive-move grab point was clamped into the window);
tiles with a strip now keep the grab point on the strip. Tests: `src/tests/title_bar_input.rs`.

### Canvas cosmetics

- The tab indicator no longer sits inside the focus ring gap (see "Focus ring gap").
- Closing windows stay on their plane position while the view pans.
- Moving a canvas window to another workspace animates from where it was drawn.
- The interactive-move tile animates between the zooms of the workspaces under the pointer
  (`MoveZoom` in `src/layout/canvas.rs`, canvas-zoom animation config).
- Window-rule border / focus-ring widths are compensated under zoom like the layout ones.

### Hot path

Nested session (debug build, winit inside headless sway, 1920x1080), 12 foot windows, title
strips, borders, ring gap, line grid with background, one region, `work-area` clip on; CPU time
of the niri process over 10 s of continuous Mod+MMB dragging (a virtual pointer moving every
8 ms):

| Scenario | niri CPU |
| --- | --- |
| idle | 0% |
| strip workspace, view-offset scroll | 35–40% of one core |
| canvas at 0.62 (all 12 windows visible), pan | 37% |
| canvas zoom in/out animations, back to back | 35% |

niri has no frame-timing output without tracy, so CPU time is the measure; canvas panning costs
the same as strip scrolling. Strip text is rasterized only for the settled zoom (tiles see
`canvas_tile_zoom` = the settled zoom; during a zoom animation the texture is scaled), checked by
a test. Fixed waste: every rendered strip built its text cache key (three `String`s) per frame.

### IPC changes (for the vendored niri-ipc)

- `Workspace.idx`: semantics only — visual index among visible workspaces, `0` for hidden
  workspaces (doc comment updated; no type change).
- `Action::Screenshot`, `Action::ScreenshotScreen`, `Action::ScreenshotWindow`: appended fields
  `copy_to_clipboard: bool` and `notify: bool`, both `#[serde(default = "default_true")]`, clap
  `--copy-to-clipboard <bool>` / `--notify <bool>` (default true).
- No new requests, responses or events.

### Recommended config (Ash, floating theme)

```kdl
layout {
    gaps 12
    background-color "#111110"
    // Host part (main config); the theme fragment may add `struts { top 58; }` and keeps `left`.
    struts {
        left 64
        top 46
    }
    shadow { off; }
    border {
        width 1
        active-color "#2A2A27"
        inactive-color "#2A2A27"
    }
    focus-ring {
        width 1
        gap 4
        active-color "#E0A458"
        inactive-color "#2A2A27"
        canvas-style "border"
    }
    title-bar {
        height 28
        font "JetBrains Mono 11"
        padding 12
        background-color "#1B1B19"
        text-color "#DAD7CF"
        muted-color "#96938B"
        line-color "#2A2A27"
    }
    // Floating themes only (separate rounded boxes with a gap):
    work-area {
        outline-width 1
        outline-color "#2A2A27"
        corner-radius 12
        clip true
    }
    canvas {
        grid "lines"
        grid-spacing 80
        grid-color "#161615"
        grid-line-width 1
        background-color "#111110"
        region-color "#2A2A27"
        region-label-color "#96938B"
        region-font "JetBrains Mono 16"
    }
}

workspace "scratch" {
    hidden
}

binds {
    Super+1 { focus-workspace 1; }          // no more `focus-workspace 2` workaround
    Super+Shift+1 { move-column-to-workspace 1; }
    Mod+WheelScrollUp   cooldown-ms=150 { if-canvas { canvas-zoom-in  at-cursor=true; focus-workspace-up;   }; }
    Mod+WheelScrollDown cooldown-ms=150 { if-canvas { canvas-zoom-out at-cursor=true; focus-workspace-down; }; }
}
```

### Files

| File | Change |
| --- | --- |
| `src/layout/work_area.rs` (new) | `WorkAreaDecor`, outline / corner masks / clip rect |
| `src/layout/workspace/work_area_ext.rs` (new) | `Workspace` hooks: update, clip classification, render |
| `src/input/layer_focus_hook.rs` (new) | `LayerFocusHook` around `do_action()` |
| `src/tests/ashd_focus.rs`, `src/tests/title_bar_input.rs`, `src/layout/tests/work_area.rs` (new) | tests |
| `src/layout/scratch.rs` | visual index helpers, `resolve_workspace_index`, `ipc_workspace_idx` |
| `src/layout/mod.rs` | `find_workspace_by_ref` / `move_workspace_to_idx` use visual indices; `mod work_area`; strip grab point; `MoveZoom` in `InteractiveMoveData` |
| `src/layout/monitor.rs` | per-workspace work-area crop and outline pass; move-to-workspace animation origin under zoom |
| `src/layout/canvas.rs`, `canvas_grid.rs`, `workspace/canvas_decor.rs` | `MoveZoom`; decorations laid out inside the clip; `WorkspaceCanvas.work_area` |
| `src/layout/floating/canvas_mode.rs`, `closing_window.rs` | closing windows follow the pan |
| `src/layout/scrolling.rs` | `working_area()`, tab indicator past the ring gap |
| `src/layout/tile.rs` | rule frame widths compensated under zoom |
| `src/layout/title_bar.rs` | allocation-free text key check |
| `src/layout/workspace.rs` | `mod work_area_ext`, `WorkArea` render element variant, update/shader hooks |
| `src/niri.rs` | `find_output_and_workspace_index_for`, `focus_window` clears on-demand focus, `ScreenshotDelivery` |
| `src/input/mod.rs`, `src/input/title_bar_click.rs` | screenshot delivery, by-id index resolution, hook, strip button filter |
| `src/ipc/{server,client}.rs`, `src/protocols/ext_workspace.rs`, `src/cli.rs` | visual `idx`, listing, ext-workspace, CLI test |
| `niri-ipc/src/lib.rs` | screenshot fields, `idx` docs |
| `niri-config/src/{binds,decorations,layout,lib}.rs` | screenshot bind properties, `WorkArea`, `StrutsPart`, tests, parse snapshot |

## Workspaces by index

Opt-in layout option (off by default, so upstream behaviour and upstream tests are unchanged):

```kdl
layout {
    create-workspaces-by-index
}
```

Upstream keeps exactly one empty workspace at the end of each monitor and removes every other
empty unnamed workspace, so `focus-workspace 5` with two workspaces lands on the third (empty)
one. With the option:

- `focus-workspace N` (also with `workspace-auto-back-and-forth`), `move-window-to-workspace N`
  (also `--window-id`, counted on the window's monitor) and `move-column-to-workspace N` with N
  past the last workspace first append empty workspaces up to index N, then go / move there. N
  counts the visible workspaces like every index reference in the fork (the hidden scratchpad
  is skipped). A move with nothing to move creates nothing.
- An empty unnamed workspace survives while a workspace *after* it is in use: has windows, is
  named (and visible), or is the active workspace of the monitor. Only the trailing empty
  workspaces are removed, back to the single empty workspace niri always keeps at the end.
  Closing the last window of the highest workspace (while another one is focused), or leaving
  a high empty workspace, trims back to the last used one.
- Hidden workspaces are named, so they are never removed; they don't count as "in use" either,
  so they don't keep the empty workspaces before them alive.
- Workspace numbers therefore don't shift: closing every window of workspace 2 while workspace
  3 has windows leaves workspace 2 empty in place.
- Name-, id- and up/down references, `move-workspace-to-index N` and output moves are unchanged
  (they still clamp). When a monitor is removed, its empty workspaces are dropped like upstream
  (only workspaces with windows or a name move to another monitor).
- A layout option like `empty-workspace-above-first`: it merges per output
  (`output { layout { … } }`) and is rejected in `workspace "x" { layout { … } }`. Turning it
  off at runtime removes the empty workspaces in the middle right away (after a running
  workspace switch).

IPC: no type changes. The `Workspaces` list / `WorkspacesChanged` simply contain empty
workspaces in the middle now (`active_window_id: null`); `idx` stays the visual index.

Code: `src/layout/ws_by_index.rs` (`Monitor::ensure_workspace_for_index`,
`ensure_workspace_for_move`, `clean_up_workspaces_by_index`, `by_index_trim_start`,
invariants). Upstream files: `src/layout/monitor.rs` (`clean_up_workspaces` dispatches to the
by-index variant; `switch_workspace`, `switch_workspace_auto_back_and_forth`,
`move_to_workspace`, `move_column_to_workspace` create the target first; `update_config`
cleans up when the option changes; `verify_invariants` checks the relaxed rule when the option
is on), `src/layout/mod.rs` (`mod ws_by_index`; `remove_window` runs the by-index clean-up
instead of removing the emptied workspace), `niri-config/src/{layout,workspace,lib}.rs`
(option, per-workspace rejection, parse test, parse snapshot). Tests:
`src/layout/tests/ws_by_index.rs` (focused tests and `WsIndexOp`, index references resolved
like the input code), `src/layout/tests.rs` (`Op::WsIndex`, the option in
`arbitrary_layout_part`, so it is randomized for the layout, per output and through
`Op::UpdateConfig`; an `UpdateConfig` turning it on and `WsIndex(Focus(4))` in both exhaustive
op lists).

## Recent-windows switcher (no code change)

Checked for the report "quick Super+Tab does nothing": a quick tap (key events well inside the
150 ms open delay and the 750 ms focus debounce) switches to the previous window, repeated taps
toggle between the two most recent windows, and holding Mod while tapping Tab walks the list,
on strip and canvas workspaces (`src/tests/ashd_mru.rs`, also checked in the nested session
with a virtual keyboard). The debounce does not lose quick back-and-forth: opening the switcher
commits the pending focus first (`mru_apply_keyboard_commit`). What does break quick taps is
upstream's sticky scope: W / O / S pressed while the switcher is open (e.g. Super+W with Super
still held) set the scope for every later open by a bind without `scope=`; on a workspace with
one window a quick Super+Tab then has nothing to switch to. Configs should give the binds an
explicit `scope="all"` (the ashd NixOS config does); the test shows both behaviours.

