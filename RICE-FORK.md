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

### Hook for real pointer input under zoom

`Workspace::window_under()` delegates to `Workspace::canvas_window_under()` (in
`canvas_ext.rs`). It hit-tests the floating tiles at `pos / zoom` (unscaled view) and today
returns the stock `HitType::Input { win_pos }` at zoom 1 and `hit.to_activate()` otherwise (like
the overview), so clicks on scaled windows only focus them; `resize_edges_under()` returns
`None` at zoom ≠ 1. To add scaled input as an additive change:

1. Append `HitType::ScaledInput { win_pos: Point<f64, Logical>, zoom: f64 }` at the end of
   `HitType` (`src/layout/mod.rs`), with `offset_win_pos` adding the offset to `win_pos` and
   `to_activate` downgrading it.
2. In `canvas_window_under`, for zoom ≠ 1 return `ScaledInput { win_pos: win_pos_u.upscale(zoom),
   zoom }` instead of `to_activate()` (`win_pos_u` is the `Input` hit's `win_pos`, the buffer
   origin in unscaled view coordinates). `Monitor::window_under` already shifts it by the
   workspace origin and already downgrades everything while the overview is open.
3. Where niri turns a hit into a pointer focus (`Niri::contents_under` / `window_under` in
   `src/niri.rs`), handle `ScaledInput`: surface-local position is `(pos - win_pos) / zoom`; look
   up the (sub)surface with that local position, and hand smithay a focus location of
   `pos - local` so that its `pos - focus_loc` arithmetic yields the scaled local position (the
   focus is recomputed on every motion event). Relative-pointer deltas and pointer constraints
   regions then need the same `/ zoom`.
4. Mod+drag pan grab and Mod+wheel zoom-at-cursor: use `Layout::canvas_pan_to` /
   `CanvasView::jump_to` for direct manipulation and `Layout::canvas_zoom_in(Some(anchor))` with
   the pointer position as the anchor.

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

- Pointer input to windows is activate-only while the zoom is not exactly 1.0; interactive
  resize is disabled at zoom ≠ 1 (see the hook above).
- Mod+drag of a canvas window: the dragged tile is drawn at full size (only the overview zoom
  applies to the dragged tile) and semi-transparent (canvas windows count as tiled once they
  leave the workspace, so that moving them to a strip workspace tiles them); it lands exactly
  where it is dropped (view → plane conversion).
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

- Scaled pointer input (hook above), including relative pointer and constraints.
- Mod+drag pan grab and Mod+wheel zoom-at-cursor (anchor = pointer).
- IPC events and requests listed above.
- Hidden workspace for a scratchpad.
- Plane grid / dot background under canvas workspaces.
- Named regions on the plane (bookmarked offsets/zooms).
- Title strips / decorations for canvas windows.
