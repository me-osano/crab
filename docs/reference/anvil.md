# Smithay `anvil` v0.7.0 — walkthrough notes

[`anvil`] is Smithay's reference compositor. crab borrows its **Smithay wiring**
and lifecycle patterns, not its scene model or its X11 window manager. This note
records what is worth copying and what crab deliberately does differently
(ADR-0006, ADR-0008).

Source: `Smithay/smithay` tag `v0.7.0` (commit `a166cf4`), cloned to
`/tmp/opencode/smithay` for the walkthrough.

## Build and run status

- `cargo build -p anvil --no-default-features --features winit` succeeds
  (1m26s on the development host).
- `anvil --winit` starts under the host Wayland session: EGL 1.5 on
  `PLATFORM_WAYLAND_KHR`, a `wayland-1` socket, then `Initialization
  completed, starting the main loop`. The host logs one EGL `BAD_SURFACE`
  (`eglQuerySurface`) at startup; anvil still stays in the loop until killed.
- The default feature set (`egl, winit, x11, udev, xwayland`) does **not** build
  on this host. v0.7.0's `smithay-drm-extras` pulls `libdisplay-info-sys 0.2.2`,
  which requires `libdisplay-info < 0.3.0`, while the host has 0.3.0. Smithay
  master has since moved `smithay-drm-extras` to 0.3.0.
- The udev/DRM path below was therefore **read, not executed**. The failure is
  an anvil dependency pin, not a crab problem: crab's `backend-tty` feature does
  not pull `smithay-drm-extras`.

## File map

| File | Role |
|---|---|
| `main.rs` | Argument parsing and backend dispatch (`--winit`, `--x11`, `--tty-udev`). |
| `state.rs` | `AnvilState<BackendData>`, all protocol state, `init`, `pre_repaint`/`post_repaint`, and the `Backend` trait. |
| `winit.rs` | Nested-Wayland/X11-client backend and its manual render loop. |
| `x11.rs` | Nested backend that runs anvil as an X11 client. |
| `udev.rs` | Bare-metal DRM/KMS backend: libseat, libinput, udev hotplug, `DrmCompositor`, vblank throttling, direct scanout. |
| `shell/mod.rs` | `WindowElement`, `place_new_window`, initial configure, surface lookup. |
| `shell/element.rs` | `WindowElement(Window)` and its `SpaceElement` implementation. |
| `shell/xdg.rs` | `XdgShellHandler`: toplevel/popup lifecycle, move/resize/fullscreen/maximize, grab. |
| `shell/x11.rs` | In-process XWayland/X11 WM integration. |
| `shell/grabs.rs` | Interactive move/resize and popup grabs. |
| `shell/ssd.rs` | Server-side decorations. |
| `focus.rs` | `PointerFocusTarget` / `KeyboardFocusTarget` over windows, layers, and popups. |
| `input_handler.rs` | Generic `InputBackend` event handling; windowed and udev paths. |
| `render.rs` | Output render elements + damage tracker; direct-scanout helpers. |
| `drawing.rs` | Pointer, DnD, FPS custom elements. |

## State construction

`AnvilState<BackendData>` is one struct holding every protocol state plus
`Space<WindowElement>`, `PopupManager`, the seat/pointer, and a `Clock`.
`BackendData` is the per-backend data type implementing `Backend`.

`AnvilState::init` is the useful checklist:

1. Insert two calloop sources: `ListeningSocketSource` for new clients and a
   `Generic<Display>` for dispatching client requests.
2. Construct the globals:
   `compositor`, `data-device`, `layer-shell`, `output-manager`,
   `primary-selection`, `data-control`, `shm`, `viewporter`,
   `xdg-activation`, `xdg-decoration`, `xdg-shell`, `presentation`,
   `fractional-scale`, `xdg-foreign`, `single-pixel-buffer`, `fifo`,
   `commit-timing`, `text-input`, `input-method`, `virtual-keyboard`,
   `relative-pointer`, `pointer-constraints`, `pointer-gestures`, `tablet`,
   and `security-context`.
3. Create the seat and keyboard.
4. Return the state; the backend then maps its output and enters the loop.

Crab already splits this across `Crab::new_without_outputs` and
`handlers/<protocol>.rs`. Use anvil's list as the Phase-6 checklist rather than
copying the one-struct style.

The `Backend` trait supplies `seat_name`, `reset_buffers`, `early_import`,
`update_led_state`, and the `HAS_RELATIVE_MOTION`/`HAS_GESTURES` capability
constants. A small equivalent is a useful seam for crab's tty backend (seat
name, output reset, early dmabuf import, keyboard LEDs) without forcing all
handlers to be generic over the backend.

## Backend selection and lifecycle

`main.rs` lists the compiled-in backends and matches the first argument. Each
backend follows the same shape:

- create a `calloop::EventLoop` and a Wayland `Display`;
- create the backend window/device and its renderer;
- create and advertise an `Output` (winit has one; udev has one per connector);
- call `AnvilState::init`;
- map the output, start optional XWayland, and run.

### winit (`winit.rs`)

Creates the winit backend, a single `Output`, dmabuf state/global (with feedback
when a render node is available), an `OutputDamageTracker`, then
`AnvilState::init`. It runs a manual `while state.running` loop:

1. pump all pending winit events (`Resized`, `Input`, `CloseRequested`);
2. call `pre_repaint`;
3. build render elements (space, cursor, DnD, FPS) and call `render_output`;
4. `submit` the damage and report presentation feedback;
5. call `post_repaint` (frame callbacks, dmabuf feedback);
6. `event_loop.dispatch(Some(1ms))`, then `space.refresh()`,
   `popups.cleanup()`, and `flush_clients()`.

Crab's winit backend instead registers the winit source with calloop and renders
on `WinitEvent::Redraw`. That is cleaner (no outer manual loop and no forced
1 ms dispatch) and should stay.

### x11 (`x11.rs`)

Same shape as winit, using `X11Backend`/`X11Surface` and an X11 window instead of
a winit window. Useful only as evidence that the state/loop split works across
nested backends.

### udev (`udev.rs`)

The M4 reference. `run_udev` does:

- `LibSeatSession::new()` and primary-GPU selection (`ANVIL_DRM_DEVICE`
  override);
- libinput backend plus udev monitor;
- `AnvilState::init` with `UdevData`;
- DRM device add/remove, connector connected/disconnected;
- `DrmOutputManager` per device, `DrmCompositor` per output;
- a vblank throttle timer, `frame_finish`, and `render_surface`;
- session pause/resume and VT handling through the libseat notifier.

`device_added` opens the DRM node through the session, creates `DrmDevice`/GBM,
and inserts the DRM notifier. `connector_connected` creates the output and maps
it in the scene; `connector_disconnected` unmaps it and migrates surfaces.
`render_surface` builds `DrmOutputRenderElements` and lets `DrmCompositor`
direct-scanout eligible windows; `frame_finish` advances the presentation
feedback and schedules the next frame.

Crab's Phase 4.1/4.2 items map directly onto these functions. Borrow the
lifecycle, not the `Space` calls.

## Handler wiring

- Protocol traits are implemented mostly in `state.rs`, with shell-specific
  handlers in `shell/xdg.rs` and `shell/x11.rs`. The matching `delegate_*!`
  macros register globals and dispatch. Crab's one-file-per-protocol split
  (`src/compositor/handlers/`) is better for growth; no need to copy anvil's
  mega-impl.
- `XdgShellHandler::new_toplevel` wraps `Window::new_wayland_window` in
  `WindowElement`, calls `place_new_window`, and installs a post-commit hook.
  The first commit is where `ensure_initial_configure` sends the initial
  configure; anvil intentionally does not configure before then.
- Popups go through `unconstrain_popup`, `PopupManager`, and `grab_popup`.
  Crab already has the equivalent flow in `handlers/xdg_shell.rs`.
- `CompositorHandler` calls `on_commit_buffer_handler` and shell-specific
  commit handling. anvil refreshes the space and cleans popups after each
  dispatch; crab does both in `Crab::prune_dead`, called from its display
  source. The tty loop must keep calling it.
- `focus.rs` implements `PointerTarget`/`KeyboardTarget` for windows, layer
  surfaces, and popups. Use it when filling gaps in crab's focus targets.
- `input_handler.rs` is generic over `InputBackend`, with separate
  `process_input_event` (udev) and `process_input_event_windowed` (winit/x11)
  paths. Its `Action` enum is backend-local; crab's pending canonical `Action`
  (TODO §5.5) should not copy that split.

## Render and frame lifecycle

- `pre_repaint` signals commit-timing barriers for windows, layer surfaces,
  cursor surfaces, and the DnD icon, then calls `blocker_cleared` for each
  affected client. Crab has no commit-timing global yet (Phase 6).
- `post_repaint` walks the same surfaces, checks the primary scanout output,
  sends frame callbacks (throttled to 1 s), reports presentation feedback for
  the output, and supplies dmabuf feedback. Crab already sends frame callbacks
  from `render.rs`; the presentation/dmabuf/primary-scanout pieces are M4.
- `take_presentation_feedback` and `update_primary_scanout_output` are the
  helpers to model crab's presentation/direct-scanout bookkeeping on.
- The renderer path is the familiar `OutputDamageTracker::render_output` plus
  `RenderElementStates`. In udev, `DrmCompositor` handles planes and direct
  scanout; `ANVIL_NO_DIRECT_SCANOUT` disables it for debugging.

## XWayland: do not copy

anvil starts XWayland in-process with `XWayland::spawn`, then
`X11Wm::start_wm`, adds an `X11Surface` variant to `WindowElement`, and
implements `XwmHandler` in `shell/x11.rs`. ADR-0006 rejects exactly this path:
crab uses `xwayland-satellite`, which re-exposes X11 toplevels as ordinary
`xdg_toplevel`/`xdg_popup` surfaces. There is no XWM, no `X11Surface` window
variant, and no X11 scene integration in crab. anvil's XWayland code is useful
only as a warning about how much WM code the in-process route owns.

## What to borrow, what to leave

Borrow:

- the `init` global checklist when adding Phase-6 protocols;
- the `Backend` capability seam and backend lifecycle shape;
- udev's session/hotplug/DRM/vblank/direct-scanout lifecycle for M4;
- `ensure_initial_configure` and popup unconstraining behavior;
- presentation-feedback and primary-scanout helpers for M4.

Leave:

- `Space`/`WindowElement`/`desktop::Window` as crab's scene
  (ADR-0008 keeps the custom ordered element list);
- the in-process X11 WM (ADR-0006);
- the one-struct/mega-impl handler style (crab already splits handlers);
- the manual winit loop (crab's calloop source is simpler and safer);
- anvil's unsafe `Generic<Display>` dispatch source (crab clones the display fd
  instead).

## Follow-ups recorded elsewhere

- M4.1 session pause/resume and udev hotplug: `TODO.md` §4.1.
- M4.2 `DrmCompositor`, vblank, connector hotplug: `TODO.md` §4.2.
- M4.4 presentation, dmabuf feedback, primary scanout: `TODO.md` §4.4.
- Phase-6 protocol checklist: `TODO.md` §6 and anvil's `init` list.
- XWayland: ADR-0006 (satellite), not anvil's in-process WM.

[`anvil`]: https://github.com/Smithay/smithay/tree/v0.7.0/anvil
