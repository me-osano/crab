# crab — TODO

> Work top-to-bottom, chronologically. Each phase ends with something **runnable and
> demoable**, and each subsection has concrete exit criteria — do not move on until
> they are satisfied. Keep layout/animation/config logic **pure** (no Smithay/Wayland
> deps); only the binary crate touches Smithay. Cross-references: `ROADMAP.md` §N for
> *what/why* and the verified feature list; `COMPOSITOR.md` §N for *design detail*.
> Decisions live in `docs/decisions/`.

**Decided:** license **GPL-3.0-or-later** (ADR-0001) · renderer **Smithay GLES**,
HDR post-0.1 (ADR-0002) · config **TOML** (ADR-0003). These unlock lifting code
from niri/cosmic/mango with attribution (Appendix A). M0 foundation choices
(edition 2024 / `resolver = "3"`, Smithay `0.7.0` from crates.io) are recorded in
Phase 0 below.

**Timeline:** part-time solo, **~37–55 weeks** cumulative to `v0.1.0` (M0–M11). Order
matters more than dates. HDR/colour is a separate post-0.1 track.

**Name:** `crab`; binary `crab`, CLI `crab-cli`; env `CRAB_SOCKET`;
directory `crab/`.

---

## Phase 0 — Foundations (M0) *(~1–2 weeks)*

Goal: a buildable workspace with hard crate boundaries enforced, the toolchain pinned,
and a winit window showing a solid background. End with Smithay's `anvil` running
locally as the canonical reference.

### 0.1 Repo & tooling
- [x] `Cargo.toml` workspace, `[workspace.dependencies]`, `workspace = true` everywhere. *(Used `resolver = "3"`, the edition-2024 default — see 0.1 toolchain note — rather than `"2"`.)*
- [x] `rust-toolchain.toml` — pinned to **stable** (Smithay tracks stable). **Edition 2024** chosen to match `crawl`/`crawl-desktop` (`COMPOSITOR.md`'s 2021 assumption superseded); workspace `rust-version = "1.85"`.
- [x] `.gitignore` (`target/`, `*.log`, `perf.data*`, `.idea/`, `.vscode/`), `rustfmt.toml`, `deny.toml`, workspace lints incl. `unsafe_op_in_unsafe_fn = "deny"` (all crates opt in via `[lints] workspace = true`).
- [x] `LICENSE` (**GPL-3.0-or-later**, full text), `CONTRIBUTING.md`, `README.md` (status: pre-alpha). *(`COPYING` dropped — `LICENSE` alone is unambiguous and non-duplicative.)*
- [x] `NOTICE` + attribution convention (file header + `CONTRIBUTING.md` + `NOTICE`).

### 0.2 Decisions & governance
- [x] `docs/decisions/0001-license.md` — **GPL-3.0-or-later**.
- [x] `docs/decisions/0002-renderer-and-hdr.md` — **GLES**, phased HDR.
- [x] `docs/decisions/0003-config-format.md` — **TOML** (accepted; via `serde` + the `toml` crate, `snake_case` keys).
- [x] `docs/decisions/0004-space-model.md` — **unified numbered spaces** + **layout families** (accepted 2026-10-17).
- [ ] `docs/decisions/0005-ipc-schema.md` — niri-`ipc` shape vs fresh; `ext-workspace` as primary bar interface.
- [ ] `docs/decisions/0006-xwayland.md` — **`xwayland-satellite`** (recommended) vs in-process.
- [ ] `docs/decisions/0007-effects.md` — own GLES implementation vs a scenefx-like crate.
- [ ] `docs/decisions/0008-client-model.md` — custom element list (niri-style) vs Smithay `Space` (see Appendix A pitfall).

**Exit criteria:** `docs/decisions/README.md` index current; every framework-level choice has an ADR or a written rationale.

### 0.3 Workspace & crate skeleton (per `ROADMAP.md` §5)
- [x] `src/core/` — pure data: ids, logical geometry, colors, spaces (no Smithay; folded in from the former `crab-core` crate).
- [x] `src/layout/` — `Layout` trait + `Monocle`/`Floating` impls (pure; folded in from the former `crab-layout` crate).
- [x] `src/animation/` — spring/easing curves + `Animated` scalar (pure; folded in from the former `crab-anim` crate).
- [x] `src/config/` — TOML parse/validate/defaults (pure; folded in from the former `crab-config` crate).
- [x] `crab-ipc/` — IPC types + blocking client (leaf; no other `crab-*` deps).
- [x] `src/` — the compositor binary (backend selection, logging, panic hook, winit backend); root `Cargo.toml` is both the `crab` package and the workspace root (niri-style). Smithay-facing code lives under `src/compositor/` (state, handlers, input, render, the layout bridge), backends under `src/backend/`.
- [x] `src/lib.rs` — the package is lib + bin: the pure modules are the reusable library surface (so doctests run and `tests/` can drive them directly); the boundary script rejects any non-pure `mod` here.
- [x] Backend cargo features — `backend-winit` (default), `backend-tty`, `backend-x11` gate Smithay's backend stacks, so a default build skips DRM/session/input; CI builds `--all-features` to keep proving the stack.
- [x] `crab-cli/` — CLI client (renamed from `crab-msg`).
- [x] `resources/` — `default-config.toml`, `session/` (`.desktop`, systemd `.target`/`.service`), `shaders/`, `crab-portals.conf`.
- [x] Headless/integration tests live in `tests/` (the root package's integration tests).
- [x] **Boundary lint**: `scripts/check-boundaries.sh` fails if the `src/{core,config,layout,animation}` modules gain Smithay/Wayland code or an upward/sideways `crate::` dependency, if `src/lib.rs` exposes a non-pure module, or if `-ipc` gains `crab-*` deps; wired into CI and `just check`.

### 0.4 Smithay baseline
- [x] Smithay pinned to **`0.7.0` (crates.io)** — a deliberate deviation from "a git revision": we need no unreleased patches yet, and a released space is reproducible and matches cosmic-comp. The vendored git revision arrives with HDR (ADR-0002, post-0.1).
- [ ] Clone and build `anvil` locally; walk its `state.rs`, backend selection, and handler wiring.
- [x] Confirmed the toolchain builds `smithay` + `calloop` + `input` + `drm`/`gbm`/`egl`/`udev`/`libseat` on the target distro: the backends are cargo features (`backend-tty` pulls the DRM/session/input stack) and CI runs `cargo clippy --all-features`, so the whole stack keeps building. `renderer_multi` on (under `backend-tty`), `backend_vulkan` off (ADR-0002), `xwayland` deferred to M9.

### 0.5 CI, logging, errors
- [x] CI (`.github/workflows/ci.yml`): `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --workspace`, `scripts/check-boundaries.sh`, `cargo deny check`. *(Uses `cargo test`, not `cargo nextest`, to avoid an extra tool dependency for now.)*
- [x] `tracing` + `tracing-subscriber` (env-filter), panic hook writing a backtrace to `$XDG_STATE_HOME/crab/crash.log`.
- [x] `main.rs`: arg parsing (`--backend {winit,tty,headless}`, `--config`, `--validate`, `--log-filter`), config load, backend select, logging init.

**Exit criteria (phase):** `cargo run -- --backend winit` opens a window with a solid background; empty-but-valid crate graph builds; boundary lint passes. *(All met: the winit backend launches and renders; `tty` fails explicitly until M4; `headless` validates and returns.)*

---

## Phase 1 — First windows, nested (M1) *(~2–3 weeks)*

Goal: a nested compositor under an existing Wayland session that runs real apps, accepts
input, and resizes them. (Design: `COMPOSITOR.md` §3, §8.)

### 1.1 Event loop & state
- [x] One `calloop::EventLoop<State>` on the main thread; `State` is single-owner (principle §2.3).
- [x] Insert the **Wayland display fd** as a calloop source; client dispatch inside the loop.
- [ ] `calloop::channel` for worker results (config parse, later: PipeWire, screenshot encode).
- [x] **Never block the loop**: no synchronous file IO, no D-Bus waits in handlers.

### 1.2 Core globals (`handlers/`)
- [x] `wl_compositor` + `wl_subcompositor`, `wl_shm`, `wl_seat`, `wl_output`, `wl_data_device_manager`.
- [x] `xdg-shell` — toplevels + popups (`XdgShellHandler`).
- [x] Client `commit`/`destroy` bookkeeping; map/unmap lifecycle; initial `configure`.

### 1.3 Input basics
- [x] Keyboard (`xkbcommon`, keymap, modifiers, key repeat) — via `KeyboardHandle::input` with the default keymap (a `KeyboardHandler` impl lands when keybind interception does).
- [x] Pointer (motion, buttons, axis/scroll) — `PointerHandle` events, each batch closed
  with `wl_pointer.frame` so clients that group events (SCTK, GTK, Qt) act on them.
- [x] **Click-to-focus**; keyboard focus follows the focused surface.
- [x] Cursor rendering (software cursor element first) — a client's `wl_pointer.set_cursor`
  surface, the built-in arrow for named icons, and nothing for a hidden cursor; the backends
  hide the host pointer so this is the only one drawn. *(The arrow is a fixed classic-pointer
  outline — black rim, white core, tip at the hotspot — until
  icon-theme loading lands; it also freezes at the last position when the host
  pointer leaves the nested window, since winit does not forward `CursorLeft`.)*

### 1.4 Render + frame pacing
- [x] GLES renderer (`GlesRenderer`) via the winit backend.
- [x] Per-output **redraw state machine**: `Idle → Queued → WaitingForVBlank → Idle`
  (`COMPOSITOR.md` §8.2) — the transitions live on the pure `Output` (unit-tested) and
  the winit backend drives them from `Redraw` events; a request that lands while a frame
  is presented queues the next one.
- [x] Frame callbacks sent only for surfaces the damage tracker actually presented,
  after `submit` succeeds (M2's frame clock will move them to the presentation time).
- [x] Smithay `OutputDamageTracker` drives repaints: elements are collected once,
  opaque content is accounted for, and a frame with no damage is not submitted.
- [x] Redraw only when something requested it — dispatch, input, resize, and the frame
  event itself; repeated requests coalesce into one frame per loop iteration.

### 1.5 Placement
- [x] Fixed cascade placement for toplevels + popups positioned relative to parents.
- [x] Popup grabs + `xdg_popup.reposition`: Smithay's `PopupManager` tracks the
  popup trees (so submenus resolve), `xdg_popup.grab` is answered with the
  `PopupPointerGrab`/`PopupKeyboardGrab` defaults (a grab from a client without
  the keyboard is refused, so popups cannot steal focus), and a reposition
  request is re-placed against the output and acknowledged. *Verified nested:
  GTK3 menus and GTK4 popovers configure, grab, re-place on resize, and an
  unfocused client's grab is denied. Dismissal on an outside click and menu
  keyboard navigation still need a hand (no input injection here).*
- [x] Respect `min_size`/`max_size` hints — read from the surface's cached state on
  commit, clamped in the layout, and clamped again in every configure the compositor
  sends (a request the client cannot honour only earns a refusal).
- [x] Place windows by their **window geometry** (`xdg_surface.set_window_geometry`)
  rather than by their buffer. Client-side decorations draw their shadow margins
  outside the geometry, so with the buffer's origin on the tile the content sat inset
  from its placement with the shadows spilling past the border, and popups anchored off
  it. `ManagedWindow` now carries the geometry, the surface is drawn at its origin minus
  the client's inset, and popups offset from the geometry origin. *Verified nested:
  `nautilus` reports `geometry=(25,25)` on a 412×1030 tile with a 462×1080 buffer, and
  `foot` `geometry=(0,-26)`; both content rects land on their placement.*
- [x] Handle clients that ignore `configure`. What was asked and what was committed are
  kept side by side (`Configure` in `src/compositor/state.rs`), and only a *new* wrong
  size earns another configure — at most `MAX_SIZE_REFUSALS` of them, after which the
  window is marked as refusing and left alone, so a client that repeats itself is never
  configured again. Its pixels are then cut back to its rectangle instead of being scaled
  into it, which is also what keeps pointer input on the drawn content. *Pure logic, unit
  tested; no live client here that ignores configure outright.*

**Exit criteria (phase):** `foot` and a GTK app run nested, accept keyboard/pointer input, and resize; idle CPU ~0% when nothing changes.
*(`foot` and GTK apps (`pavucontrol`, `nautilus`) run nested and connect against the M1 globals; pointer motion, client cursor surfaces, and `Named(Default)` fallback were exercised. Keyboard/pointer injection still has no test harness. Idle CPU is ~0% because redraws are on demand.)*

**Known M1 gaps** (from `foot`'s startup warnings — all later-milestone breadth, not bugs):
`wp_primary_selection` (M6), `xdg_activation` (M6; basic activation landed, `activation_bypass` open),
fractional scaling (M4), `cursor_shape` (not advertised yet; every named icon draws as the built-in
arrow until icon-theme loading lands), `xdg_toplevel_icon` (M6), `text-input`/IME (M6),
`xdg_decoration` (client-side decorations work). Popup grabs and `xdg_popup.reposition`
landed in `handlers/xdg_shell.rs` (M3 scope, done early) and render from the
`PopupManager` tree in `src/compositor/render.rs`.

---

## Phase 2 — Spaces & layout core (M2) *(~3–4 weeks)* — *"best of mango + dwm"*

Goal: a real tiling model with per-output spaces, per-space layout, mixed tiling/floating,
and per-space layout memory. Everything here is pure and unit-testable.

### 2.1 `src/core` data model (`COMPOSITOR.md` §6)
- [x] Newtypes `WindowId`, `OutputId`, `SpaceSet(u32)`, `LayerId`; logical `f64` `Rect`/`Point`/`Size`.
- [x] `Window` (id, app_id, title, `output: OutputId`, `spaces: SpaceSet`, floating, fullscreen, maximized, urgent, float_geometry, min/max, applied_rules).
- [x] `Output` (name, geometry, scale, `usable_area`, `view: SpaceView`, `space_states: [SpaceState; 32]`, `default_space`).
- [x] `SpaceView { visible, previous }`; `SpaceState { layout, params, scroll, focus_history }`.
- [x] `SpaceRemap` — dense renumbering used by dynamic spaces (niri-style): see §2.5.
- [x] `FocusTarget { Window | LayerSurface | LockSurface | None }`; per-output MRU stack + `last_focused_output`.
- [x] **One** focus function that updates keyboard focus, activation (`xdg_toplevel`), border-color targets, and IPC events.
- [x] Document the **multi-space-view rule**: combined set uses the lowest-numbered visible space's layout (dwm behavior).

### 2.2 `src/layout` engine (`COMPOSITOR.md` §7.1)
- [x] `LayoutInput`, `Placement { window, rect, visible, z_hint }`, `LayoutParams`, `ScrollState`.
- [x] `trait Layout { kind; arrange; neighbor?; command? }` — **pure functions**, no globals.
- [x] `compositor/layout.rs` in the binary: bridge Smithay windows → layout input; map `Placement` back.
- [x] **Floating windows are excluded from `arrange`** and drawn above tiled windows with per-window raise.

### 2.3 Layout families (ADR-0004)
- [x] `LayoutFamily` (`Stack`, `Tile`) + `LayoutKind` modes; `family()`, `modes_in()`, `is_free()`.
- [x] `Tile` — master/stack, `master_count`, `master_ratio`, left/right/top/bottom variants.
- [x] `Monocle` — focused fills area; others `visible=false`.
- [x] `Floating` — no automatic placement; a `floating`-mode space floats every window.
- [x] `neighbor()` directional focus for each.

### 2.4 Mixing tiling & floating (`COMPOSITOR.md` §7.3)
- [x] Per-window `floating` toggle; un-float re-enters layout after the focused window.
- [x] Rules force floating (dialogs, fixed min/max size, transient parent) — pure model now, wired to config in §5.
- [x] Floating windows: **pin** (all spaces) and **always-on-top**.

### 2.5 Spaces (`dwm`/mango semantics)
- [x] `view-space`, `toggle-space-view`, `view-spaces 1+3` (multi-view), `move-to-space`, `toggle-space` (on window).
- [x] Multiple spaces **per window**; `toggle-back` via `SpaceView.previous`.
- [x] Per-output space views; moving windows/output focus between spaces.
- [x] Windows are arranged on their owning output; layout, focus cycling, activation, and dynamic-space compaction are scoped to it.
- [x] Output registry keyed by `OutputId`; adding an output makes the first one primary, and removing one migrates its windows and layer entries to a fallback output.
- [x] **Dynamic numbering** (niri-style): a space exists while it holds a window or is the empty space past the last occupied one; spaces that are now empty and followed by occupied ones are removed and the survivors compacted from 1, with their layout state following and window membership renumbered through `SpaceRemap` — so there is never an empty space before the focused one, and removing the focused space hands the view to the space that moves up.
- [x] `move-to-space` shows the target space, so focus follows the moved window.
- [x] **No per-space configuration**: every space starts from `[layout]` defaults; cycling the layout changes only the current space at runtime.

### 2.6 Layout params, switching, memory
- [x] `cycle-layout-mode` + `cycle-layout-family` (two actions); set layout per space; `Pertag`-style **per-space layout memory + params restore** (§7.4).
- [x] Gaps (inner/outer), border width, focus ring color (`border_color` / `unfocused_border_color`).
- [x] Switching layout animates rects (real in Phase 3: windows spring to their new rectangles).

### 2.7 Interactive move/resize
- [x] `Super+drag` move, `Super+right-drag` resize for floating windows.
- [x] Tiled window drag → auto-float (default); reorder-in-layout is the config option (§7.3).
- [x] **Client-initiated** move/resize (`xdg_toplevel.move`/`resize`, e.g. a CSD titlebar):
  the request's serial must be the last press crab sent and the window must still be under
  the pointer; tiled windows float first, fullscreen and maximized ones refuse. The press was
  forwarded, so its release is too — a client that never sees it keeps its titlebar stuck.
  A resize follows the requested edge(s), holding the opposite edge and stopping at one
  pixel instead of inverting. Checked nested with a probe client that asks on press: press
  and release report the same surface-local point while the pointer travels (the window
  followed it), no motion is forwarded during the grab, and a left-edge drag configures the
  client to the new width; a forged serial is ignored and motion keeps reaching the client.
- [ ] Snap-to-edge for floating (basic).

### 2.8 Tests
- [x] Unit tests: golden `Placement` lists per layout for 1..12 windows.
- [x] `proptest` invariants: no overlaps (non-floating), all rects inside area, deterministic.
- [x] Snapshot-style golden rect tests per layout.

**Exit criteria (phase):** a full day of tiling + floating dialogs works nested; per-space layouts and params are remembered; `cargo test -p crab layout` green. *(Two `foot` windows verified tiled nested with per-space `tile` layout; a GTK app and a full interactive day remain before the phase is closed.)*

---

## Phase 3 — Scroller + animation engine (M3) *(~4–6 weeks)* — *"best of niri + mango"*

Goal: niri-quality motion with mango-level configurability. Switches and layout changes
are smooth, interruptible, and free of pops.

### 3.1 `src/animation` (`COMPOSITOR.md` §9)
- [x] `Curve::{ Spring, Easing, Bezier }`; `Animated<T>` (`from`/`to`/`velocity`/`start`/`curve`/`kind`).
- [x] Interruptible retargeting (keeps value + velocity); `is_animating()`.
- [x] **One synced clock per animation kind** (no per-window drift) — `Clocks`/`Kind`.
- [x] Zero-duration fast path + `reduce-motion` flag hook.
- Note: values are *sampled* from frame time rather than stepped (COMPOSITOR.md §9);
  springs are evaluated in closed form, so missed frames cannot drift them.

### 3.2 Drive animations off the frame clock
- [x] Advance animations using **frame time**, not wall-clock reads inside a redraw — the
  winit backend re-times `Crab::frame_clock` from its loop clock at the frame boundary
  (winit reports no presentation time); the tty backend passes the real vblank /
  `wp_presentation` time in M4.
- [x] Redraw machine keeps requesting frames while `is_animating()`; stops when settled
  (`WinitEvent::Redraw` re-queues while anything moves, so idle CPU stays at zero).
- [x] The frame-clock driver bridges the primitives ↔ placements/elements:
  `Crab::advance_frame` / `Crab::is_animating`, per-window `Animated<Rect>` written by
  `compositor::layout::place`, sampled by `render`. The bridge is glue, so it lives in the binary
  — `src/animation` stays pure.

### 3.3 `Scroll` family: `scroll` mode (niri)
- [x] Infinite horizontal strip of columns; focused column edge-aligned / centered
  (`center = never | always | on-overflow`), and `overspread` deciding whether a
  column wider than the view spills past both edges or is clamped to it.
- [x] Per-column width (proportion/fixed), presets cycled by `switch-preset-width`;
  column display `normal | tabbed`. *(The `preset-column-widths` /
  `default-column-display` config keys ride with M5's schema; the `tabbed` tab bar
  itself is M7.)*
- [x] Vertical variant (`vertical-scroll`, the same code on the other axis);
  `switch-preset-width` / `center-column` commands; `neighbor()` for left/right/up/down.
- [x] **Stable layout on open** — a new window opens beside the focused column and
  reflows nothing: existing columns keep their width and their place, and the
  focused window does not move even when a column before it closes.

### 3.4 Named spaces + overflow
- [ ] Named spaces / persistent workspaces; survive output unplug (§3A).
- [x] `on-overflow` centering for full-width columns. *(The policy landed with the
  scroller in 3.3; a full-width column simply degenerates to the leading edge.)*
- [x] `default-column-display normal|tabbed`.

### 3.5 Animate the events that matter
- [x] Move/resize: x/y/w/h — the window, its border and its popups all render at the
  animated rectangle, and hit-testing reads it too.
- [x] Move/resize: **crop/scale the last buffer** until the client commits the new size.
  Every surface is drawn *into* its animated rectangle rather than at its own size: a
  rectangle that has grown past what the client committed is filled by scaling its pixels
  over it, one that it has overfilled is cut back to it (`WindowFit` in
  `src/compositor/fit.rs`), so the content is never a frame behind the border and never
  spills into a neighbour. At rest the mapping is 1:1 and a client draws exactly as before,
  decoration shadows included.
- [x] Pointer input through a **stretched** window: while a rectangle is bigger than what
  its client committed, the seat is handed an origin that makes `position - origin` land on
  the surface-local point *drawn* there, not the one the placement puts there — the inverse
  of the fit, `WindowFit::input_origin` in `src/compositor/fit.rs`, which the renderer and
  the input path now both read from one place (niri's `View::local()` does the same). A
  window that is *cut back* is unaffected: it is drawn pixel for pixel and reports its
  plain surface origin. Checked end-to-end against a probe client that commits 200×100 and
  ignores every configure: drawn in 1904×1064, a pointer on the rectangle's centre reaches
  it as the buffer's centre (100, 50); without the inverse it arrives as (952, 532), far
  outside the surface. The same run turned up a missing `wl_pointer.frame` after motion and
  buttons, without which clients that batch events (`smithay-client-toolkit`, GTK, Qt) see
  none at all — every batch now ends with one.
- [x] Open: alpha, scale, offset. The first mapped frame starts a fixed-duration
  ease-out ([`Kind::WindowOpen`]): `slide` starts the rectangle a little below its
  resting place and rises into it, `zoom` starts it smaller around its centre. The
  surface, border and popups all fade with the same progress, and the rectangle is
  the one hit-testing and popup placement read, so nothing is drawn somewhere a click
  would not land.
- [x] Close: alpha, scale — **snapshot the last buffer** after unmap
  (`COMPOSITOR.md` §9 resize handling). The last buffer committed with a real
  geometry is kept alive (which keeps its cached `wl_buffer` data, and the shm pool or
  dmabuf, alive too), then imported into a texture on the fade's frames and drawn
  through the window's last fit. `slide` falls and `zoom` shrinks around the window's
  centre while the alpha goes to zero; the client can detach its buffer and destroy
  the surface immediately without the animation losing its pixels.
- [x] Layout switch: per-window rect — windows spring to their new tile.
- [ ] Space switch: per-output slide of window groups.
- [ ] Scroll view offset; border/focus-ring color blend. *(A scroll already slides, because
  every window retargets to its new column rect; animating the offset *itself* — free
  scroll, gestures, a rubber band at the ends — is what remains.)*

### 3.6 Gesture plumbing (touchpad gestures land in M6)
- [ ] Gesture values write directly into animation targets 1:1 while fingers are down; hand velocity to a spring on release (§9).

**Exit criteria (phase):** space/space switches and layout changes are smooth and interruptible with no pops; opening a window in the `scroll` mode does not disturb existing columns.

---

## Phase 4 — Bare-metal backend (M4) *(~3–4 weeks)*

Goal: boot `crab` from a TTY and use it as the real session on one machine.
(Design: `COMPOSITOR.md` §4, §8, §16.)

### 4.1 Session & device discovery
- [ ] `libseat` session (session switching, pause/resume); `udev` device enumeration.
- [ ] DRM device selection (primary vs render node) with a clear log line.
- [ ] logind integration (`zbus`): VT switch, session pause/resume, lid, suspend inhibit.

### 4.2 DRM/KMS + GBM + EGL
- [ ] `DrmDevice` + atomic modesetting; `GbmBufferedSurface` + EGL for rendering.
- [ ] `DrmCompositor` for planes, reusing the `OutputDamageTracker` that already
  drives the winit backend.
- [ ] vblank-driven redraw wired to the Phase-1 state machine (one per output).
- [ ] Handle connector hotplug (add/remove), mode selection, and window migration off removed outputs.

### 4.3 Input devices
- [ ] `libinput` backend: keyboard, pointer, touchpad, touch, switches.
- [ ] Per-device config: natural scroll, tap, dwt, accel profile, click method.
- [ ] XKB layouts per keyboard + **runtime layout switch** (bind + IPC).

### 4.4 Presentation
- [ ] Hardware **cursor plane** (fall back to composited cursor).
- [ ] `linux-dmabuf` (+ feedback), `viewporter`, `presentation-time`, **`fractional-scale`**.
- [ ] Single logical→physical conversion site with explicit rounding (avoid blurry borders).
- [ ] Transform (rotation/flip) — affects the coordinate pipeline everywhere, so do it early.

### 4.5 10-bit SDR scanout (ADR-0002)
- [ ] Offer `RGB10A2` scanout formats + set `max_bpc=10` where supported.
- [ ] **Per-format** 10-bit probe (some formats fail framebuffer binding where others succeed — do not treat "10-bit" as all-or-nothing).

### 4.6 Session integration (basic)
- [ ] `.desktop` entry + session wrapper: set `XDG_CURRENT_DESKTOP`, `XDG_SESSION_TYPE=wayland`, `dbus-update-activation-environment`.
- [ ] `spawn-at-startup` entries in config (`[autostart] exec_once` parses; the spawner is open).

**Exit criteria (phase):** log in from a display manager / boot from a TTY, run a full app set, switch VT and resume cleanly; mixed-DPI outputs are crisp.

---

## Phase 5 — Config, rules, IPC (M5) *(~4–6 weeks)*

Goal: a bar can display spaces/windows via IPC, config edits apply live, and a bad config
never crashes. (Design: `COMPOSITOR.md` §13, §14.)

### 5.1 `src/config`
- [x] TOML parser (per ADR-0003) → typed `Config`; the mango-style section layout
      (`[input]`, `[output.<name>]`, `[layout]`, `[animations]`, `[autostart]`,
      `[window_rule]`, `[layer_rule]`, `[binds.*]`, `[mousebinds]`,
      `[gesturebinds]`, `[axisbinds]`) is `resources/default-config.toml`, and a
      test parses/validates it. `[layout]` and `[animations] enabled` are applied;
      the other sections are typed, validated and kept for their milestones.
- [x] Default config path (`$XDG_CONFIG_HOME/crab/config.toml`, then
      `$HOME/.config/crab/config.toml`) when `--config` is absent.
- [x] `crab --validate` for CI/pre-flight (syntax/type errors already carry the
      `toml` crate's line/column).
- [ ] Validation with **good error spans** for semantic errors (range/choice
      failures name the key but carry no span yet).
- [ ] File includes (`include = [...]`, with optional entries); user variables.

### 5.2 Hot reload
- [ ] `notify` file watcher with debounce; **atomic apply** — parse+validate into a new `Config`, swap only if valid.
- [ ] On error: keep last good config, show an **on-screen error banner** + log; never crash or lose state.

### 5.3 Binds & input config
- [x] Parse `[binds.default]`/`[binds.locked]` (and the mouse/gesture/axis tables)
      as `chord = action` strings, kept verbatim.
- [x] Keyboard binds (modifiers + key) compiled at startup and layered over the
      built-ins; config chords override a built-in with the same chord. Keys are
      matched by base keysym, so Shift still selects the chord's modifiers.
- [x] `spawn:<command>` actions (shell-style quoting, no shell) and
      `[autostart] exec_once`, with `WAYLAND_DISPLAY` pointed at crab's socket.
- [x] Mouse binds (`[mousebinds]`, compiled over the built-in Super+drag
      chords; bare left/right `moveresize` is refused) and axis binds
      (`[axisbinds]`, with `viewtoleft/right_have_client` skipping empty spaces).
- [ ] `Mod` remap; gesture binds; switch (lid) binds.
- [ ] Bind flags: `repeat`, `allow-when-locked`, `allow-inhibiting`, release, pass-through.
- [ ] **Key modes / submaps** (resize, passthrough).
- [ ] Per-device `device_rule` (by name/type); xkb/input blocks from §5.1.
- [ ] **Hotkey overlay** listing active binds.

### 5.4 Rules (`COMPOSITOR.md` §13.4)
- [x] Parse `[window_rule]` / `[[window_rule]]`: `app_id`, `open_floating`, `width`,
      `height`, `offset_x`, `offset_y`; parse `[layer_rule]` / `[[layer_rule]]`.
- [x] Match and apply the current rule subset at map time: the first matching
      rule floats it, sizes it (clamped to its hints) and centres it at the
      rule's offset. Rules can match `app_id` exactly, title fragments
      case-insensitively, and the current floating/fullscreen state.
- [x] Actions: initial dynamic space, `no-focus`, `pinned` and `always-on-top`.
- [ ] Window rules: match `output` and X11-vs-Wayland.
- [ ] Actions: opacity, blur/shadow/corner overrides, `block-out-from`, default column width.
- [x] Apply `[layer_rule]` open animations by layer namespace.
- [ ] Space rules, monitor rules (name/make/model/serial regex), device rules.
- [ ] Re-evaluate rules on title/app-id change for rules marked `dynamic`.

### 5.5 `crab-ipc` + socket (`COMPOSITOR.md` §14)
- [ ] Unix socket at `$XDG_RUNTIME_DIR/crab-$WAYLAND_DISPLAY.sock`; export `CRAB_SOCKET`.
- [ ] Newline-delimited JSON; request/response + `version` handshake; **versioned schema, additive-only**.
- [ ] Requests: `version`, `outputs`, `spaces`, `windows`, `focused-window`, `layouts`, `action <A>`, `set-layout`, `config reload|validate`, `event-stream`.
- [ ] `set-layout` takes a **mode** name; a **family** name is accepted as an alias for the family's default mode (ADR-0004). `layouts` reports both families and modes.
- [ ] **`ext-workspace-v1` projects spaces as "workspaces"**; the socket and protocol never expose `SpaceSet` directly (ADR-0004).
- [ ] **One `Action` enum** shared by binds and IPC (serialized both ways).
- [ ] Event stream: `WindowOpened/Closed/Changed/Focused`, `SpaceViewChanged`, `LayoutChanged`, `OutputAdded/Removed/Changed`, `KeyboardLayoutChanged`, `ConfigReloaded`, `ConfigError`, `Urgent`.
- [ ] Robust parsing: unknown event names/fields are skipped, never fatal.

### 5.6 `crab-cli`
- [ ] `get <query>`, `watch <event-stream>`, `dispatch <action>`, `validate`; `--json` machine output.

### 5.7 Animation config surface
- [ ] Per-event curves/durations (open, close, move/resize, space switch, layout switch, overview, focus ring).
- [ ] `reduce-motion` config flag **and** IPC toggle (sets all durations to zero).

### 5.8 Docs
- [ ] Config reference, IPC reference (for bar authors: waybar/eww modules), default config explained.

**Exit criteria (phase):** a bar shows spaces/windows over IPC; config edits (including error cases) apply live without crashes; `Action` names identical between binds and IPC.

---

## Phase 6 — Protocol breadth P0/P1 (M6) *(~4–6 weeks)* — *"cosmic breadth"*

Goal: replace your current compositor for a **full week** with no fallback. Verify each
protocol with real clients. (Matrix: `ROADMAP.md` §4, `COMPOSITOR.md` §12.)

### 6.1 Shell & decoration
- [x] `wlr-layer-shell` with **exclusive zones → `usable_area`**; anchor, margin, layer, namespace, keyboard-interactivity.
- [x] `xdg-decoration` (+ KDE variant) — prefers client-side until crab draws a titlebar; `fractional-scale` sends a preferred scale; `viewporter` hardening.
- [ ] `cursor-shape` — needs icon-theme loading; not advertised until every shape renders (clients keep using `wl_pointer.set_cursor`).

### 6.2 Activation & selection
- [ ] `xdg-activation` (+ `activation_bypass`); `primary-selection`; `data-control` (`ext` + `wlr`).

### 6.3 Idle, lock, inhibit
- [ ] `idle-notify`, `idle-inhibit`; `ext-session-lock`; `keyboard-shortcuts-inhibit`.

### 6.4 Outputs
- [x] `xdg-output` (names and logical geometry for bars).
- [ ] `wlr-output-management`, `wlr-output-power-management` (DPMS).
- [ ] **Mirroring / clone** (explicitly chosen over upstream niri).

### 6.5 Window/space visibility to bars
- [ ] `ext-foreign-toplevel-list` / `wlr-foreign-toplevel-management`; `ext-workspace`.

### 6.6 Games & IME
- [ ] `pointer-constraints`, `relative-pointer`; `text-input-v3` / `input-method-v2`.

### 6.7 Touchpad gestures
- [ ] 3-finger horizontal → space/space switch; 4-finger → overview; **live preview** driven by the animation targets from Phase 3.

### 6.8 Ecosystem verification
- [ ] waybar, fuzzel, swaybg, swaylock/gtklock, swayidle, wl-clipboard, a Qt app, an Electron app.
- [ ] Update `ROADMAP.md` §4 rows as each lands; add a repo protocol table.

**Exit criteria (phase):** a **full week** as the primary compositor on real hardware, no fallback.

---

## Phase 7 — Layout variety, grouping, window states (M7) *(~4–6 weeks)* — *"all of mango + cosmic"*

Goal: every layout in `ROADMAP.md` §3B works; grouping and scratchpads feel native.

### 7.1 More layout modes
Modes slot into the M2 families (ADR-0004), not a flat list.
- [ ] `Tile` family: `Grid`, `Deck`, `Dwindle`/spiral (smart split, preserve, manual split), `Fair` + `VerticalFair`, `CenterTile`, `VerticalTile`, `RightTile`.
- [ ] `Scroll` family: `VerticalGrid`, `VerticalDeck`. *(`VerticalScroller` shipped with `scroll`/`vertical-scroll` in M3.)*
- [ ] `Stack` family: `Tabbed`.
- [ ] Per-mode params + presets; per-space memory for every mode.

### 7.2 Special workspace (mango space 0)
- [ ] Full **tiling overlay** over the current space with background dim; per-app auto-assign.

### 7.3 Groups & stacks (cosmic)
- [ ] Window groups + **tabbed/stacked containers**; auto tab bar; orientation, per-child size fractions, swap.

### 7.4 Scratchpads
- [ ] Named scratchpads + standard scratchpad; minimize/restore.

### 7.5 Window states (mango)
- [ ] `global` (sticky across spaces), `overlay`, `fakefullscreen`, terminal **swallow**, `unmanaged_global`.

### 7.6 Space modes
- [ ] `WorkspaceMode::{OutputBound, Global}`, vertical/horizontal nav, **wraparound**.
- [ ] Pinned spaces matched by **EDID**.

**Exit criteria (phase):** all layouts work; grouping, special workspace, and scratchpads behave like natives; per-space layout memory verified for each.

---

## Phase 8 — Effects (M8) *(~3–5 weeks)* — *"mango/scenefx quality on GLES"*

Goal: effects on by default in a polished config **without** regressing the perf targets
(`COMPOSITOR.md` §20). Corner radius → borders → shadows → blur (order matters, §10).

### 8.1 Corners & borders
- [ ] Rounded corners (SDF shader) that **also clip borders and shadows**.
- [ ] Borders / focus ring as rounded-rect shader quads; **animated color**, gradients later.

### 8.2 Shadows & dim
- [ ] Shadows: cached by size/radius (analytic SDF or pre-blurred texture); `shadow_only_floating` option.
- [ ] Dim inactive + per-window `opacity` rules.

### 8.3 Blur
- [ ] Dual-Kawase blur of content **behind** a window; **backdrop cache**; update only on damage.
- [ ] `ext-background-effect` (client-requested blur) when negotiated.
- [ ] Interaction with damage tracking + direct scanout handled explicitly.

### 8.4 Controls & safety
- [ ] Per-rule overrides (`blur false`, `no_shadow`, …) + global `effects { enabled false }` kill switch.
- [ ] **Low-power mode**: disable blur + shadows on battery.
- [ ] Custom-shader open/close/resize animations (niri); cursor shake-to-find; debug HUD.

### 8.5 Perf gate
- [ ] Benchmarks: 4 windows @1440p/144 < 2 ms CPU; blur < 6 ms GPU @1080p60 on a mid-range iGPU.

**Exit criteria (phase):** effects default-on in a polished config; frame-time targets hold; direct scanout still works when no effect touches the scanned-out window.

---

## Phase 9 — Overview, capture, XWayland (M9) *(~4–6 weeks)*

Goal: screen sharing, screenshots, and a Steam game under XWayland all work.
(Design: `COMPOSITOR.md` §15, §17.)

### 9.1 Overview
- [ ] Zoom-out grid of spaces with **live thumbnails**; `jump labels`; gesture open/close; drag-and-drop; compositor-rendered.

### 9.2 Screenshots
- [ ] Built-in UI (region/window/output) → clipboard + file.
- [ ] `ext-image-copy-capture` (+ source) **and** legacy `wlr-screencopy` (so `grim`/`slurp` work).
- [ ] **Capture colour path**: once HDR exists, convert HDR→SDR for capture (otherwise washed out).

### 9.3 Screencast
- [ ] PipeWire stream (monitor first, then window) fed by dma-buf (shm fallback); own thread + channels.
- [ ] `xdg-desktop-portal` backend: `resources/crab-portals.conf`; permission flow.
- [ ] `block-out-from "screencast"` privacy rules.

### 9.4 XWayland
- [ ] `xwayland-satellite` (per ADR-0006): config hook `xwayland { satellite-path }`, spawn on demand, set `DISPLAY`, restart.
- [ ] Verify: X11 apps map as normal toplevels; Steam game works.

**Exit criteria (phase):** Firefox/Chromium screen share, OBS capture, screenshots, and a Steam game under XWayland all work.

---

## Phase 10 — crawl-desktop integration (M10) *(~2–3 weeks; can overlap M6+)*

Goal: `crawl-desktop` runs on `crab` through the **same standard protocols** and the
same `Compositor` trait — no crawl-specific API. (Crosswalk: `ROADMAP.md` §7.)

- [ ] Add a **`crab` backend** to `crawl-desktop`'s `crates/crawl-compositor`:
  - [ ] `detect()` gains an arm on `CRAB_SOCKET` (before niri).
  - [ ] `Capability` coverage: `Workspaces` (`ext-workspace` + IPC), `WindowList`/`FocusWindow` (`ext-foreign-toplevel-list` + IPC `dispatch focus-window`), `OutputManagement` (`wlr-output-management` + `xdg-output`), `Screenshots` (`ext-image-copy-capture`/`wlr-screencopy`), `Overview` (`dispatch overview`), `Launch` (`dispatch spawn`).
  - [ ] `Event` mapping → `WorkspacesChanged`/`ActiveWorkspaceChanged`/`WindowsChanged`/`ActiveWindowChanged`/`OutputsChanged`.
- [ ] Run `crawl-shell` (panel/launcher/notifd/osd/lock) **unmodified** on `crab`.
- [ ] Align `crab-ipc` `Action` names with `crawl-desktop`’s action vocabulary (one keybind table where possible).
- [ ] Session story: `crawl-session` spawns `crab` in its own `.desktop`, or attaches when `CRAB_SOCKET` is set.
- [ ] Fill `crawl-desktop/docs/compositor-matrix.md` rows for `crab`.

**Exit criteria (phase):** `crawl-desktop` boots on `crab` with the same capability negotiation it uses on niri.

---

## Phase 11 — Hardening & `v0.1.0` (M11) *(~3–4 weeks)*

- [ ] Protocol conformance pass; fix client-compat bugs from the full test-app matrix (`COMPOSITOR.md` §19).
- [ ] Multi-monitor + **mixed-DPI** + rotated outputs.
- [ ] **Direct scanout** for fullscreen; overlay-plane experiments (cursor first).
- [ ] Crash handling: panic → log + backtrace; optional supervisor restart (session does not drop to DM).
- [ ] Packaging: `.desktop`, systemd user units + target, portals config, distro packaging (AUR/Nix/Fedora/Arch).
- [ ] Docs site: getting started, config reference, IPC reference, keybind reference, FAQ.
- [ ] Benchmarks + regression tracking in CI.
- [ ] Issue templates, contribution guide, `CHANGELOG.md` (0.x: breaking changes documented).

**Exit criteria (phase):** tagged **`v0.1.0`**, announced, installable on at least two distros.

---

## Phase 12 — HDR & colour management *(post-0.1)* — ADR-0002

Goal: native HDR games/video **and** HDR screen capture look correct. GLES only.

- [ ] **Vendor a patched Smithay git rev** carrying colour-management (upstream PR #1143 is unmerged as of 2026-10).
- [ ] Phase 1 — **passthrough**: fullscreen/borderless HDR clients get PQ/BT.2020 scanout + `HDR_OUTPUT_METADATA`; SDR desktop tone-mapped around them.
- [ ] Protocols: `wp_color_management_v1` + `wp_color_representation_v1` (the latter for Wine/Proton scRGB + BT.2020), behind a feature flag.
- [ ] Phase 2 — **desktop HDR**: composite into a **linear F16** intermediate, per-surface transforms + tone mapping, encode PQ at the output boundary; teach effects the working space.
- [ ] Phase 3 — **colour management**: ICC / wide-gamut / per-output profiles via the **DRM Color Pipeline API** (Linux 6.19+, 1D/3D LUTs, matrices).
- [ ] Keep direct/overlay scanout correct while an HDR post-process is active.
- [ ] Optional: configurable `peak-luminance` / `reference-luminance` overrides (EDID often lies).

**Exit criteria (phase):** HDR video/games and HDR→SDR capture are both correct on validated AMD + NVIDIA hardware.

---

## Phase 13 — Maturity & extensions *(aspirational)*

- [ ] Multi-GPU (render on primary, copy to secondary outputs); tablet + touchscreen gestures.
- [ ] `security-context`, `xdg-foreign`, `xdg-dialog`, `xdg-toplevel-drag`; `drm-lease` (VR); virtual/headless outputs; tearing control / `syncobj`.
- [ ] Accessibility: AccessKit/Orca bridge, zoom, colour filters, high-contrast borders, magnifier.
- [ ] Session persistence (restore windows to spaces where clients allow); scripting layer (Rhai/Lua) over IPC.
- [ ] Embedded XWayland WM if `xwayland-satellite` proves insufficient; plugin API for custom layouts once `Layout` is stable (WASM or dylib).

**Exit criteria:** extensions remain opt-in; accessibility bridge works on core flows without regressing perf.

---

## Definition of Done (Global)
- [ ] Every phase ends with something **runnable** (nested at first; on bare metal from M4).
- [ ] `cargo fmt --check`, `cargo clippy -D warnings`, `cargo nextest run`, `cargo deny check` pass in CI.
- [ ] **Boundary lint** holds: `-core`/`-layout`/`-anim`/`-config` have no Smithay/Wayland deps; `-ipc` is a leaf; no cycles; only the binary touches Smithay.
- [ ] Layouts are pure and property-tested; single logical→physical rounding site.
- [ ] Config is validated and never trusted: bad reload keeps last good config + visible error.
- [ ] One focus function updates keyboard focus, activation, borders, and IPC together.
- [ ] Effects are modular, feature-gated, per-rule overridable, and never break direct scanout.
- [ ] Protocol handlers degrade gracefully; malformed client input never panics.
- [ ] IPC schema is versioned and additive-only within a major; `Action` names identical between binds and IPC.
- [ ] Perf targets in `COMPOSITOR.md` §20 / `ROADMAP.md` §9 hold; regressions block merges once benchmarks exist.
- [ ] Docs current: ADRs, config/IPC references, protocol table, `crawl-desktop` integration matrix.
- [ ] Adapted code carries attribution headers (GPL-3.0).

---

## Appendix A — Where to lift code from (GPL-3.0 unlocked by ADR-0001)

Read these before the matching phase. Because `crab` is **GPL-3.0-or-later**, you may
**copy and adapt** GPL-3.0 sources with attribution — not just reimplement ideas.

| Concern | Source | What to take / watch |
|---|---|---|
| Canonical Smithay wiring | `smithay/anvil` (MIT) | state, backend selection, `DrmCompositor`, handler ergonomics |
| State / event model | **niri** `src/niri.rs` (GPL-3.0+) | single-owner `State`, per-output redraw machine |
| Scrollable layout | **niri** `src/layout/` | scroller column model, `neighbor()`, stable-on-open |
| Animation | **niri** `src/animation.rs` | spring/clock, interruptible retarget, synced clock |
| Render elements | **niri** `src/render_helpers/`, `src/backend/` | element ordering, damage, custom shaders |
| IPC | **niri** `niri-ipc/` (GPL-3.0+) | typed request/event schema, event stream |
| Config | **niri** `src/config/` | validation, `validate`, error spans |
| Overview / screencast / gestures | **niri** | zoom grid, PipeWire path, gesture→animation |
| Mixed tiling/floating + groups | **cosmic-comp** `src/shell/`, `src/state.rs` | id-tree, groups/stacks, orientation, edge-snap |
| XWayland knobs | **cosmic-comp** | descaling, eavesdropping |
| Spaces + layout families + per-space params | **mango** (C) | **port the algorithms** (Pertag, special workspace, `mmsg`/`get`/`watch`/`dispatch`) |
| Effects feature set + wallpaper-cached blur | **mango** / `scenefx` (C) | reimplement ideas on GLES |
| HDR | `franzjeger/cosmic-hdr-reference` (glue MIT; cosmic GPL, smithay MIT) + niri HDR forks (`dividebysandwich`, `losnoco`, GPL-3.0) | atomic KMS colour state, F16 compositing, format-aware 10-bit probe |
| Space semantics | **dwl/dwm** (GPL/MIT) | view/toggle/multi-space rules |
| Layout-as-process | **river** (GPL) | only if you ever want out-of-process layouts |

**Pitfalls to avoid**
- **Smithay API churn**: pin a git rev; keep every Smithay touch inside `handlers/` + `backend/`.
- **Client model**: `COMPOSITOR.md` §8.3 specifies explicit element ordering → prefer a **custom element list (niri-style)** over Smithay `Space` (cosmic-style). Decide in ADR-0008 and do not mix.
- **Don't hold locks across frames**: never hold a Smithay surface-state lock or your own registry lock across `arrange`/draw.
- **Never block `calloop`**: no sync IO / D-Bus in handlers; use channels + worker threads.
- **Animation clock**: vblank/`wp_presentation`, not `Instant::now()`, or motion stutters.
- **Fractional scale**: one rounding site; integer logical coords are a trap.
- **Direct scanout vs effects**: an effect touching the scanned-out window must disable it.
- **10-bit probing**: test each DRM format individually.
- **GPL diligence**: all sources above are GPL-3.0(-compatible); still record origin per file. Do not paste non-GPL-compatible code.

## Appendix B — Crate & dependency map

```
crab          → crab-ipc, [crab-protocols]     + Smithay/calloop/…
                 · lib target (src/lib.rs) exposes the pure modules only:
                   src/core       → (ids, geometry, colors, spaces)
                   src/config     → core, serde, toml, thiserror
                   src/layout     → core
                   src/animation  → core            (curves; geometry via core)
                 · bin target (src/main.rs) + Smithay glue (src/compositor/, src/backend/)
                 · pure modules are Smithay-free and depend downward only (source-scanned in CI)
crab-ipc      → (leaf; serde only)
crab-cli      → ipc                        (CLI)
```
- `crab-protocols` exists **only if ≥2 crates need** generated bindings; otherwise a `protocols` module inside the binary.
- Backend features: `backend-winit` (default), `backend-tty`, `backend-x11`.
- Feature flags: `xwayland`, `screencast`, `effects`, `dbus`, `profile-with-tracy`, and (later) `hdr`.

## Appendix C — Doc crosswalk

| Phase (this file) | ROADMAP milestone | COMPOSITOR.md |
|---|---|---|
| 0 Foundations | M0 | §4–5, §22 M0 |
| 1 First windows | M1 | §3, §8 |
| 2 Spaces & layout core | M2 | §6, §7.1–7.3 |
| 3 Scroller + animation | M3 | §7.4, §9 |
| 4 Bare-metal backend | M4 | §8, §16 |
| 5 Config, rules, IPC | M5 | §13, §14 |
| 6 Protocol breadth | M6 | §11, §12 |
| 7 Layout variety, grouping | M7 | §7.2, §7.5 |
| 8 Effects | M8 | §10 |
| 9 Overview, capture, XWayland | M9 | §15, §17 |
| 10 crawl-desktop integration | M10 | (ROADMAP §7) |
| 11 Hardening / v0.1.0 | M11 | §19, §20, §21 |
| 12 HDR & colour | post-0.1 | §16 (colour) |
| 13 Maturity | post-0.1 backlog | §22 |
