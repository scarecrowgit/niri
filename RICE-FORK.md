# rice fork of niri

This branch (`rice`) is a fork of upstream niri (base: `73a04441`, v26.04+). It adds a
per-workspace **infinite canvas** window mode next to niri's scrolling strip. Everything else
behaves exactly like upstream: with the default config, and on any workspace that never enters
canvas mode, layout and rendering go through the same code paths with bit-identical numbers.

The fork is kept rebasable: new logic lives in new files, and edits in upstream files are short
and marked with a `// rice:` comment (or a `rice:` doc comment on new fields).

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
| `canvas-zoom-in` / `canvas-zoom-out` | `CanvasZoomIn {}` / `CanvasZoomOut {}` | next/previous zoom step |
| `canvas-set-zoom Z` | `CanvasSetZoom { zoom }` | clamped |
| `canvas-fit` | `CanvasFit {}` | |
| `canvas-home` | `CanvasHome {}` | |
| `canvas-fly-to-window` | `CanvasFlyToWindow { id }` | `--id` optional |
| `toggle-window-pinned` | `ToggleWindowPinned { id }` | `--id` optional |

All new variants are appended at the end of both `Action` enums.

IPC state: `niri_ipc::Workspace` has a new `mode: WorkspaceMode` field with `#[serde(default)]`
(old replies without it parse as `Strip`). A mode change currently triggers a full
`WorkspacesChanged` event.

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
| `niri-ipc/src/canvas.rs` | `WorkspaceMode` |

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

### Planned IPC shape (next agent)

- `WindowLayout.pos_in_canvas: Option<(f64, f64)>` – plane position of the tile (canvas windows),
  from `FloatingSpace::canvas_plane_rect`.
- `WindowLayout.is_pinned: bool` (serde default).
- `Event::WorkspaceModeChanged { id, mode }` (instead of the current full `WorkspacesChanged`).
- `Event::CanvasViewChanged { workspace_id, offset, zoom, target_offset, target_zoom, view_size }`
  emitted at most once per frame (compare against the last sent value after
  `advance_animations`).
- `Request::CanvasViews` → current/target transform and view size of every canvas workspace.

Note that `WindowLayout.tile_pos_in_workspace_view` of canvas windows is already in real view
coordinates (scaled by the zoom); `tile_size`/`window_size` stay logical, unscaled.

## Building and running nested

There is no global Rust on the dev machine; use the flake's dev shell:

```sh
nix develop               # or: nix print-dev-env > env.sh; source env.sh (bash)
export CARGO_TARGET_DIR=/mnt/storage/projects/rice/.cargo-target-niri
cargo build -j 6
cargo test -j 6 --workspace
RUN_SLOW_TESTS=1 cargo test -j 6 --release --lib layout::   # randomized + exhaustive layout tests
cargo clippy -j 6 --workspace --all-targets
cargo fmt --all -- --check
```

Never run a development build against the live session (no TTY/DRM, never with the inherited
`WAYLAND_DISPLAY`). Runtime checks use a nested session: a headless sway
(`WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1`) in a private `XDG_RUNTIME_DIR` with a
private D-Bus session, and the built `niri -c test.kdl` started inside it with
`WAYLAND_DISPLAY` pointing at sway (winit backend), the live `WAYLAND_DISPLAY`, `NIRI_SOCKET`,
`DISPLAY` and `DBUS_SESSION_BUS_ADDRESS` unset — the same technique as
`nix-new/rice/xtask/scripts/nested-session.sh`. Then drive it with
`NIRI_SOCKET=<nested socket> niri msg action ...` (e.g. `spawn -- foot`, `toggle-workspace-mode`,
`canvas-zoom-out`, `canvas-fit`, `focus-column-left`, `screenshot-screen` with a
`screenshot-path` pointing at a scratch directory).

## Known limitations and deviations

- Mod+drag of a canvas window: the dragged tile is semi-transparent (canvas windows count as
  tiled once they leave the workspace, so that moving them to a strip workspace tiles them).
  Pointer-input limitations under zoom are listed in "Canvas pointer input".
- Deviation: a window that is floating on a *strip* workspace and moved by keyboard onto a
  canvas keeps its relative position in the viewport (stock floating semantics) instead of
  landing at the viewport centre; tiled windows and windows from other canvases land centred.
- Pinned windows scale with the canvas; their unscaled view position is kept, so zooming moves
  them towards the view origin.
- Frame width compensation adjusts the layout-level `border`/`focus-ring` widths; per-window-rule
  widths are not compensated. Compensation is applied when the zoom settles, so frames are thin
  during the zoom animation.
- `toggle-window-floating`, `move-window-to-floating/tiling` are no-ops for canvas windows. Column
  actions (consume/expel, column width presets, tabbed display) act on the floating window like
  on a stock floating window, or do nothing.
- Windows opened while in canvas mode become strip columns with a fixed width equal to their
  current width when going back to strip mode (unless `open-floating true`).
- The close animation of a canvas window stays at its unscaled position if the view pans during
  it; the move-to-workspace transition animation of a canvas window starts from its unscaled
  position. Both are short cosmetic glitches.
- In the overview, canvas windows outside the viewport show next to the workspace horizontally,
  like strip columns do (niri only crops vertically there).
- Render culling keeps tiles within half a view size of the viewport; popups extending further
  from an off-screen parent are not drawn.
- Changing `default-workspace-mode` affects workspaces created afterwards only.
- Performance was not measured with a profiler; in the nested debug build panning/zooming
  animations ran without visible artifacts. Every canvas frame re-renders all visible canvas
  tiles (positions change every frame), the same cost class as a strip view-offset animation.

## Follow-ups

- IPC events and requests listed above.
- Hidden workspace for a scratchpad.
- ~~Scaled pointer input, Mod+drag pan, Mod+wheel zoom-at-cursor~~: done, see "Canvas
  pointer input" below. Open: touch input and pointer constraints at zoom ≠ 1.
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
- The interactive-move tile switches between canvas zoom and full size abruptly when it crosses
  between a canvas and a strip workspace (other monitor or the overview); popups of the moved
  window are positioned for the unscaled window.
- Pinch zoom needs 3+ fingers, Mod, or the empty plane, to keep two-finger pinches for apps.
- `if-canvas` checks the focused workspace, also for pointer binds.
- Runtime-tested in the nested session (winit backend, virtual pointer/keyboard) with foot:
  clicks, typing, scroll, drag-selection at 0.62 and Mod+MMB / Mod+LMB pan, Mod+RMB resize,
  Mod+LMB move, Mod+wheel zoom at cursor, `if-canvas` fall-through, focus-follows-mouse. The
  glide was only covered by layout tests (the nested config has animations off); touchpad
  gestures were not tested on hardware.

## Decorations: title strips, focus ring gap, canvas grid and regions

Compositor-drawn window chrome and canvas plane decorations for the rice desktop design
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
  (dragging moves the window like Mod+drag; a click without drag just activates); a double
  click toggles `maximize-column` in strip mode, and on a canvas zooms to 100% centred on the
  window, or fits all windows when already at 100%. Other buttons do nothing. Mod+RMB resize
  edges are computed on the whole tile as before.
- **Focus ring gap**: the ring is drawn around the tile grown by `gap` on every side, always as
  a border (never as a background fill, which would paint the gap), with concentric corners
  (outer radius = window radius + gap + width; square windows get `gap + width`). Gap is
  rounded to physical pixels; under canvas zoom it scales with the tile.
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

- Pointer interaction with the strip (drag, double click) is implemented but was not
  exercised in the nested session (it has no input devices); it is covered only by the
  hit-test unit tests.
- The tab indicator of a tabbed column sits inside the focus ring gap.
- During a fullscreen/maximize animation the strip fades out at full height above the window
  instead of shrinking.
- Strip text is not re-rasterized for the overview zoom (scaled texture).
- `canvas-remove-region` of a config region lasts until niri restarts; regions are not in IPC
  state yet; region labels are not hit-testable.
- New-window size presets subtract the unrounded strip height (like niri does for borders),
  so at fractional scales a preset can be off by under one physical pixel.
