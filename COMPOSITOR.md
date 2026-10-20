# crawm — Compositor design

> Design document and roadmap for a Rust Wayland compositor + window manager that mixes **space-based layouts** (mango), **scrollable tiling and engineering quality** (niri), and **broad protocol coverage and floating/tiling mixing** (COSMIC).
>
> Name: `crawm`. Status: **pre-alpha design**.
> Notes on upstream projects are from memory; verify against current repos before copying any design decision.

---

## Table of contents

1. [Vision](#1-vision)
2. [Design principles](#2-design-principles)
3. [Architecture overview](#3-architecture-overview)
4. [Tech stack](#4-tech-stack)
5. [Repository layout](#5-repository-layout)
6. [Core data model](#6-core-data-model)
7. [Layout system](#7-layout-system)
8. [Event loop and render pipeline](#8-event-loop-and-render-pipeline)
9. [Animation system](#9-animation-system)
10. [Visual effects](#10-visual-effects)
11. [Input, focus, and keybindings](#11-input-focus-and-keybindings)
12. [Protocol support matrix](#12-protocol-support-matrix)
13. [Configuration](#13-configuration)
14. [IPC](#14-ipc)
15. [XWayland](#15-xwayland)
16. [Outputs, scaling, and color](#16-outputs-scaling-and-color)
17. [Screencast and screenshots](#17-screencast-and-screenshots)
18. [Session and desktop integration](#18-session-and-desktop-integration)
19. [Testing and debugging](#19-testing-and-debugging)
20. [Performance targets](#20-performance-targets)
21. [Risks and mitigations](#21-risks-and-mitigations)
22. [Roadmap](#22-roadmap)
23. [References](#23-references)

---

## 1. Vision

A compositor that treats **layout as a swappable strategy**, not an identity. Users should be able to:

- Tile or stack (master/stack, grid, deck, monocle) on one space.
- Scroll (niri-style infinite strip) on another.
- Float freely on a third, or **mix floating and tiled windows on the same space**.
- Switch layout per space at runtime with a keybind or IPC call, with smooth animation between states.

### Goals

- Correct, fast, tear-free Wayland compositor on bare metal (DRM/KMS) and nested (winit).
- Multiple layouts behind one trait; per-space layout state.
- Modern UX: spring animations, gestures, blur/shadow/rounded corners as optional passes.
- First-class scripting surface: hot-reloaded config, JSON IPC, event stream.
- Works with the ecosystem: bars (waybar, ironbar, eww), launchers (fuzzel, rofi-wayland), lockers, screen sharing via portals, notifications.

### Non-goals (initially)

- Being a full desktop environment (no built-in panel, settings app, or file manager).
- X11 window manager mode. XWayland support comes via `xwayland-satellite` or a minimal embedded approach.
- Matching every effect of Hyprland/mango. Effects stay modular and are cut first when they threaten stability.
- Network transparency, remote desktop (maybe later through PipeWire/portal).

---

## 2. Design principles

1. **Correctness before polish.** Protocol compliance and frame pacing come before blur.
2. **Layouts are pure functions.** A layout takes windows plus an area and returns placements. No hidden global state, so layouts are unit-testable without a Wayland connection.
3. **State is single-owner.** One `State` struct driven by `calloop`; no `Arc<Mutex<_>>` soup. Cross-thread work (screencast, config parsing, asset loading) communicates through channels.
4. **Everything animatable is a value with a target.** Do not mutate positions directly; set targets and let the animation system interpolate (interruptibly).
5. **Config is validated, never trusted.** A bad config reload must never crash or lose the current config. Show an error overlay/notification and keep the last good config.
6. **Make invariants unrepresentable.** Use newtypes (`SpaceSet`, `OutputId`, `WindowId`), enums for focus targets, and exhaustive matches.
7. **Observable by default.** `tracing` spans around frame, layout, and protocol handlers; an optional debug overlay (frame time, damage, direct-scanout status).
8. **Modular effects.** Each effect is a render pass behind a feature flag and a config switch, and can be disabled per window rule.

---

## 3. Architecture overview

```
┌──────────────────────────────────────────────────────────────────────┐
│                              main / CLI                              │
│        arg parsing · config load · backend selection · logging       │
└───────────────┬──────────────────────────────────────────────────────┘
                │
        ┌───────▼────────┐            ┌─────────────────────────────┐
        │   calloop      │◄───────────┤ Event sources               │
        │  event loop    │            │  · Wayland display fd       │
        └───────┬────────┘            │  · libinput                 │
                │                     │  · DRM vblank / winit       │
                │                     │  · timers (animations)      │
                │                     │  · config file watcher      │
                │                     │  · IPC socket               │
                │                     │  · D-Bus (logind/portals)   │
                │                     │  · PipeWire channel         │
                │                     └─────────────────────────────┘
        ┌───────▼──────────────────────────────────────────────────┐
        │                         State                            │
        │  ┌────────────┐ ┌────────────┐ ┌──────────┐ ┌─────────┐  │
        │  │  Outputs   │ │ Spaces │ │ Windows  │ │  Input  │  │
        │  └─────┬──────┘ └─────┬──────┘ └────┬─────┘ └────┬────┘  │
        │        │              │             │            │       │
        │  ┌─────▼──────────────▼─────────────▼────────────▼────┐  │
        │  │   Layout engine (trait) + Animation system         │  │
        │  └───────────────────────┬────────────────────────────┘  │
        │                          │ placements                    │
        │  ┌───────────────────────▼────────────────────────────┐  │
        │  │            Render element builder                  │  │
        │  │  (windows, layer surfaces, cursor, effects, UI)    │  │
        │  └───────────────────────┬────────────────────────────┘  │
        └──────────────────────────┼───────────────────────────────┘
                                   │
                 ┌─────────────────▼──────────────────┐
                 │   Backend: DRM/GBM/EGL │ winit     │
                 └────────────────────────────────────┘
```

### Data flow per frame (simplified)

1. Event arrives (input, client commit, timer, IPC).
2. Handler mutates `State` (focus, window set, space membership, layout params).
3. If layout inputs changed, mark the space **layout-dirty**.
4. On the next frame callback for an output: run layout for dirty spaces, retarget animations, advance animations to the frame's presentation time, build render elements, submit via backend, then send `frame` callbacks and `wp_presentation` feedback.

---

## 4. Tech stack

| Concern | Choice | Notes |
|---|---|---|
| Compositor toolkit | **Smithay** (git) | Track main; pin a commit in `Cargo.toml` and bump deliberately |
| Event loop | **calloop** | Used by Smithay; all sources are calloop sources |
| Backends | Smithay `winit`, `drm` + `gbm` + `egl` + `udev` + `libinput` + `libseat` | Winit for dev, DRM for real use |
| Renderer | Smithay GLES renderer | Custom shaders for blur, rounded corners, shadows |
| Config format | **TOML** (`serde` + `toml` crate) | See [section 13](#13-configuration) |
| Config watching | `notify` crate | Debounced |
| IPC | Unix socket + JSON (`serde_json`) | Event stream for bars/scripts |
| D-Bus | `zbus` | logind, portals, screen-saver inhibit, accessibility later |
| Screencast | `pipewire` crate | Dedicated thread; frames passed via dma-buf |
| XWayland | `xwayland-satellite` (external) first | Avoids implementing an X11 WM |
| Text rendering (UI overlays) | `cosmic-text` or `pango`/`cairo` | For error overlay, hotkey overlay, debug HUD |
| Logging | `tracing` + `tracing-subscriber` | Optional `tracy-client` feature |
| Errors | `anyhow` (binary), `thiserror` (libs) | |
| Testing | `cargo test`, `proptest` for layouts, headless backend | See [section 19](#19-testing-and-debugging) |

### Rust toolchain

- Stable Rust, `edition = "2021"` (move to 2024 once dependencies settle).
- `clippy` and `rustfmt` in CI; `#![deny(unsafe_op_in_unsafe_fn)]`.
- Cargo features: `xwayland`, `screencast`, `effects`, `dbus`, `profile-with-tracy`.

---

## 5. Repository layout

Use a workspace so layout logic and config can be tested without graphics dependencies.
The root `Cargo.toml` is both the `crawm` package and the workspace root (niri-style),
so the compositor lives in a top-level `src/` and the library crates sit beside it.

```
crawm/
├── Cargo.toml                  # `crawm` package + workspace
├── COMPOSITOR.md               # this file
├── src/                        # the compositor: Smithay glue, backends, render, handlers
│   ├── lib.rs                  # library surface: the pure modules only (reusable, testable)
│   ├── core/                   # pure data: ids, geometry, spaces, window model, no Smithay
│   ├── config/                 # parsing, validation, defaults, schema (pure)
│   ├── layout/                 # Layout trait + implementations (pure, unit-tested)
│   └── animation/              # spring/easing curves, animated scalars (pure)
├── tests/                      # integration tests using the headless backend
├── crawm-ipc/                  # IPC types shared by compositor and CLI client
├── crawm-cli/                  # CLI client: `crawm-cli ...`
├── resources/
│   ├── default-config.toml
│   ├── session/                # .desktop file, systemd units
│   └── shaders/                # GLSL for effects
└── docs/                       # mdBook; ADRs
```

The package is **lib + bin**: `src/lib.rs` exposes only the pure
`src/{core,config,layout,animation}` modules, and `src/main.rs` is the thin
Smithay entry point. Everything binary-side that knows Smithay lives in
`src/compositor/` (state, handlers, input, render, the layout bridge), so the
shared code is one directory and the platform code another. `scripts/check-boundaries.sh`
rejects any non-pure `mod` in `lib.rs`, so the reusable surface stays graphics-free.

### Inside `src/`

```
src/
├── lib.rs                      # library surface: pure modules only
├── main.rs                     # bin entry point: backend selection, logging, panic hook
├── compositor/                 # everything that knows Smithay (binary-side)
│   ├── mod.rs
│   ├── state.rs                # Crawm: state + top-level event dispatch
│   ├── layout.rs               # bridges Smithay windows to the pure layout module
│   ├── input.rs                # seat events + binds; splits per device class when it earns it
│   ├── render.rs               # frame composition; splits into render/ when effects need it
│   └── handlers/               # one file per protocol (xdg_shell.rs, layer_shell.rs, ...)
├── backend/
│   ├── mod.rs                  # Backend trait
│   ├── winit.rs
│   ├── tty.rs                  # DRM/KMS, multi-GPU later
│   └── headless.rs             # for tests
├── core/                       # pure data: ids, geometry, spaces, window model, no Smithay
├── config/                     # parsing, validation, defaults, schema (pure)
├── layout/                     # pure layout engine (Layout trait, layouts, placement)
├── animation/                  # pure primitives; driver advances them off the frame clock
├── ipc.rs
├── config_reload.rs
├── screencast/
└── xwayland.rs
```

Backends are compiled per cargo feature on the `crawm` package: `backend-winit`
(nested window, the default dev path) and `backend-tty` (bare-metal DRM/KMS, M4);
`headless` is always present so tests can run the compositor without graphics.

---

## 6. Core data model

Names are suggestions. Keep this part in `src/core` with no Smithay dependency.

### Identifiers and geometry

```rust
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct WindowId(pub u64);

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct OutputId(pub u32);

/// Bitmask of spaces. Up to 32 spaces is plenty (mango uses 9).
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Default)]
pub struct SpaceSet(pub u32);

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rect { pub x: f64, pub y: f64, pub w: f64, pub h: f64 }
```

Use logical `f64` coordinates in the layout layer and convert to physical pixels (with rounding rules) only at render time. Fractional scaling makes integer logical coordinates a trap.

### Window model

```rust
pub struct Window {
    pub id: WindowId,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub spaces: SpaceSet,            // can belong to multiple spaces
    pub floating: bool,           // per-window override
    pub fullscreen: bool,
    pub maximized: bool,
    pub urgent: bool,
    pub float_geometry: Option<Rect>, // remembered when floating
    pub min_size: Option<Size>,
    pub max_size: Option<Size>,
    pub rules: AppliedRules,      // result of matching window rules
}
```

### Output and space state

Two viable models. **Recommendation: spaces per output** (each monitor has its own space view), as in mango/dwl.

```rust
pub struct Output {
    pub id: OutputId,
    pub name: String,             // "DP-1"
    pub geometry: Rect,           // logical, after scale/transform
    pub scale: f64,
    pub usable_area: Rect,        // after layer-shell exclusive zones
    pub view: SpaceView,            // which spaces are visible and how
    pub space_states: [SpaceState; 32],
    pub default_space: SpaceState,   // state a newly created space starts with
}

pub struct SpaceView {
    pub visible: SpaceSet,         // can show multiple spaces at once (union)
    pub previous: SpaceSet,        // for "toggle back"
}

pub struct SpaceState {
    pub layout: LayoutKind,       // a family + a mode (see §7.2)
    pub params: LayoutParams,     // master ratio, master count, gaps, etc.
    pub scroll: ScrollState,      // for scroller: its columns and the view offset
    pub focus_history: Vec<WindowId>,
}
```

A space's layout is a **mode inside a family** (ADR-0004): `Stack` (`floating`,
`monocle`, later `tabbed`), `Tile` (M2 `tile`; later `grid`, `deck`, `dwindle`,
…), and `Scroll` (M3 `scroll`, `vertical-scroll`). Two cycle actions address the two axes:
`cycle-layout-mode` (within the family) and `cycle-layout-family` (to the next
family's default mode). Config and IPC name a mode; a family name is an alias for
that family's default mode.

Rule for **multi-space view**: when several spaces are visible, the layout of the *lowest-numbered visible space* is used for the combined window set (dwm behavior). Document this clearly; it is the one place where space models confuse people.

Spaces are **dynamic** (niri-style): a space exists while it holds a window or is
the empty space past the last occupied one (where a `view` past the end lands).
After a window closes or moves, `Output::compact` removes the spaces that are now
empty and followed by occupied ones, renumbers the survivors densely from 1,
moves each space's `SpaceState` (layout memory, scroll columns, focus history)
with it, and resets freed slots to `default_space`. The returned `SpaceRemap`
renumbers window membership, and the view follows its content: if the focused
space is removed, it shows the space that moved into its slot. There is
therefore never an empty space before the focused one: `MOD+5` on a two-space
desktop shows the empty space 3, closing the last window of space 2 makes space 3
become space 2, and with no windows at all every index collapses to space 1. The
fixed 32-slot `SpaceSet` is unchanged; this is a renumbering policy on top of it.
There is **no per-space configuration**: a new space starts from the `[layout]`
defaults, and cycling the layout changes only the current space at runtime.

### Focus model

```rust
pub enum FocusTarget {
    Window(WindowId),
    LayerSurface(LayerId),   // e.g. launcher with keyboard-interactivity
    LockSurface(OutputId),
    None,
}
```

Keep a per-output MRU stack and a global "last focused output". Focus change must go through one function that also updates keyboard focus, activation state (`xdg_toplevel` activated), border color targets, and IPC events.

---

## 7. Layout system

### 7.1 The trait

```rust
pub struct LayoutInput<'a> {
    pub area: Rect,                       // usable area of the output
    pub windows: &'a [LayoutWindow],      // tiled windows in order (floating excluded)
    pub focused: Option<usize>,
    pub params: &'a LayoutParams,
    pub scroll: &'a mut ScrollState,      // layouts that need persistent state
}

pub struct Placement {
    pub window: WindowId,
    pub rect: Rect,
    pub visible: bool,                    // monocle/deck hide non-top windows
    pub z_hint: i32,
}

pub trait Layout {
    fn kind(&self) -> LayoutKind;
    fn arrange(&self, input: LayoutInput<'_>) -> Vec<Placement>;

    /// Optional: directional focus within this layout (left/right/up/down).
    fn neighbor(&self, input: &LayoutInput<'_>, from: usize, dir: Direction) -> Option<usize>;

    /// Optional: layout-specific commands (grow master, scroll column, etc.).
    /// The layout's own persistent state comes along, so a family can edit what
    /// only it understands — the scroller's columns, for instance.
    fn command(&self, _cmd: LayoutCommand, _params: &mut LayoutParams, _scroll: &mut ScrollState) {}
}
```

Floating windows are **not** passed to `arrange`; they keep their `float_geometry` and are drawn above tiled windows (with per-window raise on focus). This is what lets one space mix both.

### 7.2 Built-in layouts: families and modes

Layouts are grouped into **families**, and each family has **modes** (ADR-0004).
The family is the broad strategy; the mode is a concrete arrangement. A family's
first mode is its default, and the family name is accepted as an alias for it.

| Family | Mode | Description | Source | MS |
|---|---|---|---|---|
| `Stack` | `floating` | No automatic placement; cascading initial position | classic stacking | M2 |
| `Stack` | `monocle` | One window fills the area; others hidden | dwm | M2 |
| `Stack` | `tabbed` | Grouped windows share a rect with a tab bar | COSMIC/i3 | M7 |
| `Tile` | `tile` | Master/stack, master count+ratio, left/right/top/bottom | dwm/mango | M2 |
| `Tile` | `grid` | Even grid | mango | M7 |
| `Tile` | `deck` | Master + stack where only the top of the stack is visible | mango/dwl | M7 |
| `Tile` | `dwindle` | Recursive splits (smart/manual), with preserve | bspwm/Hyprland | M7 |
| `Tile` | `fair`, `vertical-*` | Fair splits and vertical variants | mango | M7 |
| `Scroll` | `scroll` | Infinite horizontal strip of columns | niri | M3 |
| `Scroll` | `vertical-scroll` | Same strip, run down the vertical axis | niri/mango | M3 |

### 7.3 Mixing tiling and floating

- Each window has `floating: bool`; toggling moves it between the layout's input set and the free set.
- A space whose layout is the `Stack` family's `floating` mode has **no tiled input** at all: every window on it is placed as floating, whether or not it is individually floated.
- When un-floating, the window re-enters the layout at a sensible position (after the focused window).
- Window rules can force floating (dialogs, `app_id` matches, windows with fixed min/max size, transient parents).
- Floating windows can be **pinned** (visible on all spaces) and **always-on-top**.
- Interactive move/resize with the mouse on a tiled window can either auto-float it (default) or reorder in the layout (config option).

### 7.4 Layout switching

- Switching the layout of a space does **not** change window state; it only changes how `arrange` is computed.
- Two actions: `cycle-layout-mode` walks the modes **within** the current family; `cycle-layout-family` jumps to the next family's default mode.
- Animate each window's rect from old to new placement (see [section 9](#9-animation-system)).
- Remember per-mode params so switching back restores master ratio, scroll offset, etc.

### 7.5 Fullscreen, maximized, and special cases

- Fullscreen windows are removed from layout input and placed at the full output geometry above layer-shell `Top` (but below `Overlay`), per protocol expectations.
- Maximize in a tiled layout means "fill the usable area" without leaving the space's window ordering.
- Respect client min/max size hints; layouts treat them as soft constraints and clamp placements. Clients that ignore configure sizes need a "clip to rect" fallback.
- Windows are placed by their **window geometry** (`xdg_surface.set_window_geometry`), not by their buffer: client-side decorations draw their shadow margins outside the geometry, so the geometry is what the layout arranges, what the border frames, and what popups anchor to. The surface is drawn from its own origin, which is the geometry origin pulled back out by whatever inset the client reported; a client that never sets a geometry falls back to its surface bounding box.

### 7.6 Testing layouts

Because layouts are pure:

- Unit tests on fixed inputs (golden rect lists).
- `proptest` invariants: no overlaps in non-floating layouts, all rects inside area, total area coverage within gaps, deterministic output.
- Snapshot tests for each layout with 1..12 windows.
- Scroller-specific invariants: every window is placed, the focused column is always in view, columns never overlap, and opening a window leaves every column up to the focused one — and the view — exactly where it was.

### 7.7 The scroller (`Scroll` family)

niri's model, and the reason it feels calm: **the columns are the layout's state,
not a function of the window list**. `arrange` only *reconciles* them against the
windows it is handed.

- A **column** is a width plus an ordered list of windows plus a display mode
  (`normal`, and `tabbed` from M7). Columns live in `ScrollState`, which every
  space owns, so a column keeps its width and its neighbours across any number
  of arranges.
- A window that is not in a column yet becomes a new column **immediately after
  the focused one** (or after the column that held the focus when the focused
  window is itself new). Columns whose windows are all gone are dropped. This is
  what gives the two properties everything else follows from:
  - opening a window cannot resize or move *any* existing window — the new
    column simply takes space where it is inserted;
  - the focused window never drifts on its own — the view offset is re-derived
    each arrange from the focused column's place in the strip and where it was
    asked to sit on screen, so columns opening or closing before it move the
    strip but not the window.
- A column's main-axis span is its configured width — a `proportion` of the view
  or a `fixed` size — widened to fit its widest window's minimum size. Whether a
  column wider than the view *spills past both edges* (`overspread = true`) or is
  clamped to the view (`overspread = false`) is a separate question from where
  the view sits.
- **Where the view sits** — `center-focused-column` — is one of `never` (scroll
  as little as it takes to keep the focused column visible; the default),
  `always` (keep it in the middle), or `on-overflow` (centre it only when it
  cannot sit beside the previously focused column). A too-wide column always
  aligns its leading edge with the view's.
- **Inside** a column the windows split the cross axis with the usual gaps; in
  `tabbed` they all take the column's rectangle and only the focused one is
  drawn, so a tab switch moves nothing.
- `switch-preset-width` cycles the focused column through `preset-column-widths`
  (a third, a half, two thirds by default); `center-column` centres it once.
  Both commands are the scroller's own — they edit `ScrollState`, not
  `LayoutParams`, which is why the trait's `command` receives both.
- Movement along the strip is animated window-by-window like any other relayout
  (each window's target rect is where its column lands), so a scroll is a
  `move` animation. Animating the *offset* itself — free scrolling, gestures,
  a rubber band at the ends — is [section 9](#9-animation-system) work.
- Off-screen columns are still given an explicit rectangle and drawn (clipped by
  the output); culling them from the render pass is a damage-tracking concern.

`scroll` and `vertical-scroll` are one implementation on two axes: every
computation is expressed in terms of the *main* axis (which the strip grows
along) and the *cross* axis, and `ScrollAxis` turns those into coordinates.

---

## 8. Event loop and render pipeline

### 8.1 Event loop

- One `calloop::EventLoop<State>` on the main thread.
- Wayland display is inserted as a calloop source; client dispatch happens in the loop.
- Worker threads (config parse, PipeWire, screenshot encode) send results through `calloop::channel`.
- **Never block** the loop: no synchronous file IO in handlers, no waiting on D-Bus.

### 8.2 Per-output redraw state machine

Follow the well-trodden approach (used by niri and Smithay's anvil):

```
Idle ──queue_redraw──► Queued ──render──► WaitingForVBlank ──vblank──► Idle
                          ▲                       │
                          └──── queue_redraw ─────┘ (EstimatedVBlank timer when nothing to show)
```

- Only render when something requested a redraw (damage, animation tick, cursor move).
- When an animation is active, keep requesting redraws until all animations settle.
- Use the vblank timestamp (or `wp_presentation` clock) as the animation time, not `Instant::now()`, so motion is smooth.

### 8.3 Render element ordering (bottom to top)

1. Background layer-shell surfaces (`Background`)
2. Bottom layer-shell surfaces (`Bottom`)
3. Tiled windows (z-hint order) with borders/shadows
4. Floating windows (MRU order; always-on-top last)
5. Fullscreen window (if any)
6. Top layer-shell surfaces (`Top`) (hidden under fullscreen unless configured)
7. Popups (`xdg_popup`) and subsurfaces of the above
8. Overlay layer-shell surfaces (`Overlay`), session-lock surface when locked
9. Compositor UI: overview, hotkey help, error banner, debug HUD
10. Cursor (hardware plane if possible, otherwise composited)

### 8.4 Damage tracking and scanout

- Use Smithay's `OutputDamageTracker` / `DrmCompositor` so only changed regions are repainted.
- **Direct scanout**: if a single fullscreen window's buffer is scanout-capable (dma-buf, right modifiers), submit it directly to the primary plane. Disable on effect use that touches that window.
- **Overlay planes**: use for cursor first; video/window overlays are a later optimization.
- Send `wp_presentation_feedback` and frame callbacks only for surfaces actually shown on that output.

### 8.5 Multi-GPU (later)

Support primary GPU rendering with copy to secondary GPU outputs (dma-buf import/export). Treat it as a late milestone; plan the `Backend` trait so a renderer per device is possible, but ship single-GPU first.

---

## 9. Animation system

Goal: niri-quality motion with mango-level configurability.

### Primitives

Implemented in `src/animation` (TODO.md §3.1); the shapes below are the real
API, not a sketch.

```rust
pub enum Curve {
    Spring(SpringParams),                        // { stiffness, damping, epsilon }
    Easing { duration_ms: u32, kind: Easing },
    Bezier { duration_ms: u32, control: CubicBezier },
}

pub struct Animated<T: Animatable> {
    from: T,             // the current segment's start
    to: T,               // its target: the value at rest
    velocity: T,         // handed over on retarget (springs only)
    start: f64,          // frame time the segment began at
    curve: Curve,
    kind: Kind,          // the event that last moved it
}
```

`Animated<T>` is **sampled, not stepped**: its value is a pure function of the
frame time `Clocks` reports, so reading it never changes it and two values moved
by one event stay in phase however many frames each of them missed. Springs are
evaluated analytically (the closed-form damped oscillator, expressed as scalar
weights so a rectangle springs as happily as a scalar); `is_animating()` is
then just "is the segment younger than its settle time".

The clock is **one per animation `Kind`** — `WindowOpen`, `WindowClose`,
`WindowMove`, `WindowResize`, `SpaceSwitch`, `LayoutSwitch`, `Scroll`, `Border`
— all re-timed from the same frame instant, so no window can drift from another.
Each kind carries its own reduce-motion flag, which is what per-event config and
the global reduce-motion switch both act on. A kind flagged
`complete_instantly` (or a zero-duration curve) reports an infinite age, so it
jumps straight to its target without any special-casing downstream.

### Requirements

- **Interruptible:** retargeting mid-animation keeps current value and velocity (springs make this natural).
- **Gesture-driven:** touchpad swipe controls the value 1:1 while fingers are down, then hands velocity to a spring on release.
- **Clock-driven:** animations advance using the frame time passed in; the system reports `is_animating()` so the redraw machine knows whether to keep going.
- **Per-event curves** configurable: window open/close, window move/resize, space switch, layout switch, workspace overview, focus ring/border color.
- **Reduce-motion switch:** one config flag and one IPC toggle that sets all durations to zero.

### What gets animated

| Event | Animated properties |
|---|---|
| Window open | alpha, scale, offset |
| Window close | alpha, scale (render a snapshot texture after the client unmaps) |
| Move/resize in layout | x, y, w, h (render at animated rect; for resize, crop/scale the last buffer until the client catches up) |
| Space switch | per-output horizontal/vertical slide of window groups |
| Layout switch | per-window rect |
| Scroller | view offset (today a scroll lands as per-window moves; animating the offset itself — free scroll, gestures, the ends of the strip — is open) |
| Border/focus ring | color blend |

Landed in M3: **the per-window rectangle** — move/resize in layout, and with it layout
switches and scrolling, which are the same thing seen from different angles (a scroll
retargets every window to the rectangle its column now occupies). The window, its border
and its popups are all drawn at the animated rectangle, and hit-testing, drag origins and
popup placement read the same rectangle, so nothing lags behind the picture while it
moves. The space-switch *slide*, the scroller's *own* offset (for free scrolling and
gestures), open/close and border colour blends still land with the rest of M3 (their
properties need the snapshot and crop/scale machinery described below).

### Resize handling

Clients commit new buffers after being configured, not instantly. For smooth visuals:

- Configure the client with the target size, clamped to the `min_size`/`max_size` hints it
  set, and book what was asked so a later commit can be compared against it.
- Draw the committed buffer **into** the animated rect until the new buffer arrives, per
  axis: a rect that has grown past what the client committed is filled by scaling its
  pixels over it, anchored on the rect's origin so the window's top-left corner stays put
  and a client's decoration shadows spill past the far edge as they do at rest; a rect the
  client has overfilled is cut back to it pixel for pixel, so nothing spills into a
  neighbour. A rect the client has filled exactly is drawn 1:1, shadows included.
- Ask again only for a *new* wrong size, and only a bounded number of times
  (`MAX_SIZE_REFUSALS`); then the window is marked as refusing, left at the size it chose,
  and drawn cut back to its rect. A client that keeps repeating itself is never configured
  again, so this cannot loop.
- Pointer input is mapped back through the same fit: the seat is given an origin that makes
  `position - origin` land on the surface-local point *drawn* there, not the one the
  placement puts there (`WindowFit::input_origin`). A window that is cut back is drawn 1:1
  and reports its plain surface origin. Popups are never stretched and keep their placement.
- For close animations, capture the last buffer into an offscreen texture before the surface goes away.

---

## 10. Visual effects

All effects are optional render passes with a performance budget and a kill switch.

| Effect | Approach | Notes |
|---|---|---|
| Rounded corners | Fragment shader with SDF mask on window texture | Must also clip borders and shadows |
| Borders / focus ring | Drawn as rounded-rect shader quads around the window | Animate color; support gradients later |
| Shadows | Pre-blurred rounded-rect shadow texture or analytic SDF shadow | Cache by size and radius |
| Blur | Dual Kawase blur of the content *behind* a window | Needs the framebuffer behind the window; cache and update only on damage; respect `ext-background-effect` if/when stabilized |
| Dim inactive | Alpha multiply | Cheap |
| Opacity rules | Per-window rule (`opacity 0.9`) | |

### Rules of the road

- **Corner radius first**, then borders, then shadows. Blur last; it is expensive and interacts with damage tracking.
- Effects must not break direct scanout when no effect applies to the scanned-out window.
- Provide `effects { enabled false }` globally and per-window rule overrides (`blur false`).
- Expose a "low power mode" that disables blur and shadows on battery.

---

## 11. Input, focus, and keybindings

### Devices

- libinput via Smithay: keyboard, pointer, touchpad, touchscreen, tablet (later), switches (lid).
- Pointer events are forwarded as `wl_pointer`, and every motion/button/axis batch ends with
  `wl_pointer.frame`: clients that group events (SCTK, GTK, Qt) only act once the frame
  arrives.
- Button and enter events carry a real serial (`SERIAL_COUNTER`), and the last press a client
  saw is remembered: `xdg_toplevel.move`/`resize` name a press, and the compositor checks it.
- Per-device config: natural scroll, tap, accel profile, click method, disable-while-typing.
- XKB layouts configurable per keyboard; runtime layout switching via bind and IPC.

### Keybindings

- Declarative binds in config with **modifiers + key**, **mouse binds**, **gesture binds**, and **switch events**.
- Bind flags: `repeat`, `allow-when-locked`, `allow-inhibiting=false` (so critical binds work even when a client inhibits shortcuts).
- Optional **modes/submaps** (resize mode, passthrough mode).
- A **hotkey overlay** listing active binds (niri has one; it is a great onboarding feature).

### Focus policy

- Click-to-focus default; optional focus-follows-mouse (with scroll-safe thresholds).
- Focus-on-new-window with rules (`no-focus`, `focus-on-activation` via `xdg_activation`).
- Respect `xdg_activation_v1` tokens to avoid focus stealing.
- Pointer constraints and relative pointer for games; keyboard shortcuts inhibit protocol.

### Gestures

- Touchpad: 3-finger horizontal swipe for space switch, 4-finger for overview (configurable).
- Touchscreen: edge swipes (later).
- Gestures write into animation targets directly (see section 9).

### Interactive move/resize

- Super+drag to move, Super+right-drag to resize, as in classic stacking WMs.
- For tiled windows: dragging converts to floating or reorders, per config.
- A client may also ask (`xdg_toplevel.move`/`resize`, usually a CSD titlebar drag). The
  request is accepted only while its serial is the last press crawm sent and the window is
  still under the pointer; fullscreen and maximized windows ignore it, tiled ones float
  first. Because the press reached the client, its release is forwarded as well — a
  compositor-started drag swallowed the press and swallows the release, but a client that
  never sees its release keeps the titlebar pressed.
- A resize drags the requested edge(s) and pins the opposite ones, so pulling the left or
  top edge moves the origin too. It stops at one pixel rather than crossing the fixed edge.
- Snap-to-edges and tile-by-drag-to-edge can come later as stacking-mode polish.

---

## 12. Protocol support matrix

Priority: **P0** (needed to boot a usable session), **P1** (needed for a good daily driver), **P2** (nice to have), **P3** (later / evaluate).

| Protocol | Priority | Why |
|---|---|---|
| `wl_compositor`, `wl_subcompositor`, `wl_shm`, `wl_seat`, `wl_output`, `wl_data_device_manager` | P0 | Core |
| `xdg-shell` | P0 | Normal application windows and popups |
| `linux-dmabuf` | P0 | GPU buffers; needed for performance and most clients |
| `wlr-layer-shell` | P0 | Bars, launchers, wallpapers, notifications |
| `xdg-decoration` | P0 | Server-side decoration negotiation |
| `viewporter`, `fractional-scale` | P0/P1 | Correct HiDPI behavior |
| `presentation-time` | P1 | Accurate frame timing for clients |
| `idle-notify`, `idle-inhibit` | P1 | Idle management (swayidle etc.) |
| `ext-session-lock` | P1 | Screen lockers |
| `xdg-activation` | P1 | Focus stealing prevention, app-to-app activation |
| `primary-selection`, `data-control` | P1 | Clipboard managers, middle-click paste |
| `cursor-shape` | P1 | Client-requested cursors without shipping themes |
| `pointer-constraints`, `relative-pointer` | P1 | Games |
| `keyboard-shortcuts-inhibit` | P1 | Remote desktop, VMs |
| `text-input` / `input-method` | P1 | CJK and on-screen keyboards |
| `wlr-output-management` | P1 | `wlr-randr`, kanshi |
| `wlr-output-power-management` | P1 | DPMS control |
| `ext-foreign-toplevel-list` / `wlr-foreign-toplevel-management` | P1 | Taskbars |
| `ext-workspace` | P1 | Workspace/space widgets in bars |
| `xdg-output` | P1 | Output names and logical geometry |
| `wlr-screencopy` / `ext-image-copy-capture` | P1 | Screenshots, recording |
| `xdg-desktop-portal` (ScreenCast, Screenshot) | P1 | Browser and OBS screen sharing via PipeWire |
| `xdg-foreign` | P2 | Parent/child across processes |
| `tablet-v2` | P2 | Drawing tablets |
| `security-context` | P2 | Sandboxed apps (Flatpak) |
| `single-pixel-buffer`, `alpha-modifier` | P2 | Cheap efficiency wins |
| `content-type`, `tearing-control` | P2 | Gaming/video hints |
| `ext-background-effect` | P2/P3 | Standardized blur request from clients |
| `color-management` | P3 | HDR/ICC; track upstream |
| `xdg-toplevel-drag`, `xdg-dialog` | P2 | Better drag/drop and modal dialog behavior |

Keep this table in the repo and update it as protocols land. Each protocol gets its own handler file and, where possible, an integration test.

---

## 13. Configuration

### 13.1 Format choice

| Option | Pros | Cons |
|---|---|---|
| **TOML** *(chosen — ADR-0003)* | Ubiquitous, `serde` support, first-class tooling (`taplo`), familiar to everyone | Deeply-nested/repeated structures need arrays of tables |
| **KDL** | Nested, readable, comments, good for repeated nodes (binds, rules); niri precedent | Fewer Rust crates and far less familiarity; kdl-rs now defaults to KDL v2 (`#true`/`#false`) |
| Plain `key = value` (mango) | Easy to hand-edit; simple parsing | Weak structure; harder to validate |
| Lua/Rhai scripting | Maximum power | Complexity, startup cost, safety |

**Recommendation:** **TOML** for the main config (ADR-0003), with `snake_case` keys,
mango-style sections (`[layout]`, `[input]`, `[binds.default]`, …), arrays of tables
where a section repeats (`[[window_rule]]`; a single `[window_rule]` table is
accepted too), and a flat `chord = action` map for binds. Add an optional scripting
layer (Rhai or Lua) only after IPC is solid; most "scripting" needs are met by IPC
plus the event stream.

### 13.2 Behavior

- **Hot reload** via file watcher with debounce.
- **Atomic apply:** parse and validate into a new `Config`; only swap if valid.
- **Errors surface visibly:** on-screen banner plus `crawm-cli validate` for CI/pre-flight.
- **Default path:** `$XDG_CONFIG_HOME/crawm/config.toml` (falling back to
  `$HOME/.config/crawm/config.toml`) is read when `--config` is absent. The shipped
  example is `resources/default-config.toml`.
- **Toggles accept both spellings:** TOML `true`/`false` and mango's `0`/`1`.
- **Sections land as their features do:** the current schema is the full example
  (see §13.3), but only `[layout]` and `[animations] enabled` are applied so far. A
  section whose feature is still missing parses and validates, so a config can be
  written once; unknown keys are ignored and a present-but-wrongly-typed key is an
  error. The pre-`[layout]` flat form is rejected with a message rather than silently reset.
- `include = ["other.toml"]` support for splitting configs.
- Provide `crawm-cli config schema` or documented defaults so editors can offer completion later.

### 13.3 Example

The authoritative example is [`resources/default-config.toml`](resources/default-config.toml);
`crawm --validate --config <path>` checks a copy. Its shape:

```toml
[layout]
gaps = 8
border_width = 2
border_color = "#7aa2f7"
unfocused_border_color = "#3b4261"
client_side_decorations = 0
focus_follows_cursor = 1
default_layout = "tile"

[layout.stack]
next_space_direction = "horizontal"

[layout.tile]
master_count = 1
master_ratio = 0.6
next_space_direction = "horizontal"

[layout.scroll]
center_focused_column = "never"        # never | always | on-overflow
always_center_single_column = 1
default_column_ratio = 0.5
next_space_direction = "vertical"

[animations]
enabled = 1                                   # 0/1 or false/true
animation_type_open = "slide"                 # slide | zoom
animation_type_close = "slide"

[input.keyboard]
repeat_rate = 25
repeat_delay = 600
numlock_on = 0
xkb_rules_layout = "us"

[input.touchpad]
disable = 0
tap_to_click = 1
tap_and_drag = 1
drag_lock = 1
natural_scrolling = 1
disable_while_typing = 1
swipe_min_threshold = 1

[input.mouse]
natural_scrolling = 0

[output.monitor]                              # one table per connector, e.g. eDP-1
mode = "1920x1080@120.030"
scale = 1.0
transform = "normal"
position = { x = 1280, y = 0 }

[autostart]
exec_once = ["waybar"]

[[window_rule]]                               # `[window_rule]` also works for one
app_id = "Nautilus"                           # a string or a list of app_ids
open_floating = 1
size = [800, 500]
offset_x = -90
offset_y = -90

[layer_rule]
animation_type_open = "zoom"
layer_name = ["rofi", "alacritty"]

[binds.default]
"SUPER+q" = "killclient"
"SUPER+Return" = "spawn:alacritty"
"ALT+Left" = "focusdir:left"
"SUPER+1" = "view_space:1"
"Alt+1" = "move_client_space:1"

[binds.locked]
"XF86AudioMute" = "spawn:wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"

[mousebinds]
"btn_left" = "moveresize,curmove"
"btn_right" = "moveresize,curresize"

[gesturebinds]
"left+3" = "focusdir:left"

[axisbinds]
"SUPER+UP" = "viewtoleft_have_client"
```

**Status:** `[layout]` and `[animations] enabled` are applied at startup; the
open/close styles are read when those animations land (M3). The other sections parse
and validate so the file can be written once, but their behavior lands with M4
(`[input]`, `[output]`), M5.3/M5.4 (`[binds.*]`, `[mousebinds]`, `[autostart]`, rules)
and M6 (gestures, layer rules). The built-in key table is still what runs.

### 13.4 Window rules

Rules live in `[window_rule]` (one table) or `[[window_rule]]` (several). The
current schema matches `app_id` (a string or a list of app_ids) and sets
`open_floating`, `size = [width, height]`, `offset_x` and `offset_y`; M5.4 expands
it to match `title`, `is-floating`,
`is-fullscreen`, `output`, `space` and X11-vs-Wayland, with actions for initial
space, size/position, opacity, blur/shadow/corner overrides, `no-focus`, pinned,
always-on-top, `block-out-from` (screencast privacy) and default column width
(scroller).

Apply rules **at map time** and **re-evaluate on title/app-id change** for rules marked `dynamic`.

---

## 14. IPC

### Transport

- Unix domain socket at `$XDG_RUNTIME_DIR/crawm-$WAYLAND_DISPLAY.sock`; path exported as `CRAWM_SOCKET`.
- Newline-delimited JSON, request/response plus an **event stream** subscription.
- A `crawm-cli` CLI wraps it (`--json` for machine output).

### Requests (initial set)

```
version | outputs | spaces | windows | focused-window | layouts
action <Action>                  # same actions as keybinds
set-layout --space N --output X <layout>
config reload | validate
event-stream
```

### Events

`WindowOpened/Closed/Changed/Focused`, `SpaceViewChanged`, `LayoutChanged`, `OutputAdded/Removed/Changed`, `KeyboardLayoutChanged`, `ConfigReloaded`, `ConfigError`, `Urgent`.

### Rules

- **Schema is versioned.** Additive changes only within a major version; include a version request.
- IPC types live in `crawm-ipc` so third-party Rust tools can depend on it.
- Keep action names identical between config binds and IPC (one `Action` enum, serialized both ways).
- Document the protocol so bar authors can integrate (waybar modules, eww, custom scripts).

---

## 15. XWayland

Options:

1. **`xwayland-satellite` (recommended first):** an external process that acts as an X11 WM and presents X windows as normal Wayland toplevels. Far less compositor code; some edge-case limitations (certain X11-specific window behaviors).
2. **Embedded X11 WM via Smithay's XWayland support:** full control, more work (ICCCM/EWMH, override-redirect, focus quirks).

Plan: ship option 1 with a clear config hook (`xwayland { satellite-path "..." }`), spawn on demand when an X client connects, and set `DISPLAY` automatically. Revisit option 2 only if satellite limitations block real users.

---

## 16. Outputs, scaling, and color

- Hotplug via udev; output add/remove updates space views and migrates windows from removed outputs to a fallback output.
- Modes: parse `WxH@Hz`, support custom modes later, VRR (adaptive sync) toggle.
- **Fractional scaling:** use `wp_fractional_scale` + `wp_viewporter`; layout in logical coordinates; round buffer positions to physical pixels consistently to avoid blurry borders.
- Transform (rotation/flip) support early, since it affects the coordinate pipeline everywhere.
- Mirroring and extended layouts via output management.
- **Color/HDR:** out of scope for v1; keep the render path structured (linear-light option later) so color-management protocol work is possible.

---

## 17. Screencast and screenshots

### Screenshots

- Built-in UI (niri style): select region/window/screen, copy to clipboard, save to file.
- Also support `wlr-screencopy`/`ext-image-copy-capture` so `grim`, `slurp`, etc. work.

### Screencast

- Implement the portal backend path: `xdg-desktop-portal` → (your portal impl or a compatible one) → PipeWire stream fed by compositor frames.
- Prefer **dma-buf** sharing to avoid CPU copies; fall back to shm.
- Support **monitor** casting first, **window** casting second (render the window offscreen).
- **Privacy:** `block-out-from "screencast"` window rule renders a blank/solid rect in cast output while visible locally.
- Run the PipeWire loop on its own thread; communicate through channels.

---

## 18. Session and desktop integration

- **Session start:** `.desktop` entry for display managers; wrapper script sets `XDG_CURRENT_DESKTOP`, `XDG_SESSION_TYPE=wayland`, imports env into systemd/D-Bus (`dbus-update-activation-environment`).
- **systemd:** optional `crawm-session.target` and units for bars/wallpaper (`graphical-session.target` integration).
- **Autostart:** `spawn-at-startup` entries in config.
- **Portals:** ship `crawm-portals.conf` choosing `gtk`/`wlr`/own backends for FileChooser, Screenshot, ScreenCast.
- **Notifications, bar, launcher, wallpaper:** explicitly **not built in**; document recommended pairings (e.g., waybar/ironbar, fuzzel, mako/swaync, swaybg/swww).
- **Lock/idle:** support `ext-session-lock` and idle protocols; document `swayidle` + `swaylock`/`gtklock` setups.
- **logind:** handle VT switching, session pause/resume, lid switch, suspend inhibit.
- **Accessibility:** keep an eye on AT-SPI/screen reader support; cursor size/theme options; reduce-motion.
- **Crash resilience:** log panics with backtrace to `$XDG_STATE_HOME`; consider a supervisor that restarts the compositor (clients will die, but the session does not drop to the DM).

---

## 19. Testing and debugging

### Layers of testing

| Layer | Method |
|---|---|
| Layout math | Unit + property tests in `src/layout` (no graphics) |
| Animation curves | Unit tests with deterministic clocks |
| Config | Parse/validate tests, golden error messages, fuzzing the parser |
| Protocol handlers | Headless backend plus a test Wayland client (e.g., `wayland-client` based harness) |
| State machine | Scripted scenarios: open N windows, switch spaces, unplug output, assert focus and placements |
| Visual regression | Headless render to PNG; compare with tolerance (later) |
| Real hardware | Manual matrix: AMD, Intel, NVIDIA; multi-monitor; mixed DPI |

### Dev workflow

- `cargo run -- --backend winit` for nested iteration.
- Debug HUD: frame time, damage regions, scanout state, animation count.
- `tracing` filters per module (`RUST_LOG=crawm::render=debug`).
- Optional Tracy profiling feature for frame hitches.
- `WAYLAND_DEBUG=1` on test clients; `wayland-info` / `weston-info` for capability checks.
- CI: build, clippy, fmt, unit tests, headless integration tests, `cargo deny` for dependency audit.

### Compatibility test apps

Firefox, Chromium (Ozone Wayland), GTK3/4 apps, Qt5/6 apps, Electron, foot/alacritty/kitty, mpv, a Steam game via Xwayland, OBS, a screen sharing browser tab, a login/lock cycle.

---

## 20. Performance targets

| Metric | Target |
|---|---|
| Idle CPU (nothing changing) | ~0% (no unnecessary redraws) |
| Frame time, 4 windows, no effects, 1440p/144 | < 2 ms CPU, no missed vblanks |
| Frame time with blur on | < 6 ms GPU on mid-range iGPU at 1080p60 |
| Input-to-photon latency | One frame; use direct scanout and cursor plane where possible |
| Memory | < 100 MB resident with a typical session (excludes client buffers) |
| Config reload | < 50 ms, no dropped frame |
| Startup to first frame | < 500 ms |

Measure continuously; regressions block merges once benchmarks exist.

---

## 21. Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Smithay API churn | Rebase pain | Pin commit; update in dedicated PRs; keep Smithay-facing code isolated in `compositor/` and `backend/` |
| Scope creep (effects, every protocol) | Never reaching daily-drivable | Strict milestone gating (section 22); P0/P1 first; effects behind features |
| Layout/animation coupling bugs | Glitchy resize | Pure layout module; explicit animation retargeting tests |
| NVIDIA quirks | Unusable for part of audience | Test early on NVIDIA proprietary + nouveau/NVK; document known issues |
| Fractional scaling rounding | Blurry or misaligned borders | Single place that converts logical → physical; snapshot tests |
| Blur performance and damage interaction | Frame drops | Budgeted, cached, optional; disable under low power |
| Focus-handling bugs | Frustrating UX | One focus function; scenario tests; follow xdg-activation rules |
| Single-maintainer burnout | Stalled project | Small milestones, docs, tests, early public releases to attract contributors |
| Licensing mixups when borrowing ideas/code | Legal friction | niri is GPL-3.0, COSMIC components are GPL-3.0, mango/dwl derive from dwl/dwm (GPL/MIT mix). Re-implement ideas rather than copying code unless your license is compatible; check each source's license before reuse |

---

## 22. Roadmap

> `ROADMAP.md` §6 is the **authoritative** milestone plan (M0–M9). The sketches below
> are kept for the design narrative and may lag; where they disagree, `ROADMAP.md`
> wins.

Each milestone ends with something **runnable and demoable**. Checkboxes are the working task list. Time estimates assume part-time solo work and are rough; treat order as more important than dates.

### Milestone 0: Foundations (1-2 weeks)

- [ ] Create workspace and crates (`core`, `layout`, `config`, `anim`, `ipc`, `compositor`, `msg`)
- [ ] CI: build, clippy, fmt, tests
- [ ] Pin Smithay; get Anvil building as a reference
- [ ] Logging with `tracing`; panic hook
- [ ] Decide license; add `LICENSE`, `CONTRIBUTING.md`
- **Exit criteria:** `cargo run` opens a winit window with a solid background

### Milestone 1: First windows (2-3 weeks)

- [ ] Winit backend with calloop loop
- [ ] Core globals: compositor, shm, seat, output, data device
- [ ] `xdg-shell`: toplevels and popups
- [ ] Keyboard and pointer input; click-to-focus
- [ ] Render windows with GLES; frame callbacks
- [ ] Simple fixed placement (cascade)
- **Exit criteria:** `foot` and a GTK app run inside the nested compositor, accept input, resize

### Milestone 2: Spaces & layout core (3-4 weeks)

- [ ] `Layout` trait, `Placement`, pure `src/layout` module
- [ ] Layout families `Stack` (`floating`, `monocle`) + `Tile` (`tile`)
- [ ] Per-window `floating` flag; mixed tiled+floating on one space
- [ ] Spaces: `SpaceSet`, view space, move-to-space, toggle-space
- [ ] Layout params: master ratio/count, gaps; `cycle-layout-mode` / `cycle-layout-family`
- [ ] Interactive move/resize for floating windows
- [ ] Unit + property tests for layouts
- **Exit criteria:** daily workflow of tiling plus floating dialogs works nested

### Milestone 3: Bare-metal backend (3-4 weeks)

- [ ] libseat session, udev device discovery
- [ ] DRM/KMS + GBM + EGL rendering, vblank-driven redraw state machine
- [ ] libinput devices and per-device config
- [ ] Hardware cursor plane
- [ ] VT switching and session pause/resume
- [ ] `linux-dmabuf`, `viewporter`, `presentation-time`
- [ ] Output hotplug basics; per-output space views
- **Exit criteria:** boot into the compositor from a TTY and use it for real on one machine

### Milestone 4: Desktop essentials, P0/P1 protocols (4-6 weeks)

- [ ] `wlr-layer-shell` with exclusive zones feeding `usable_area`
- [ ] `xdg-decoration`, `cursor-shape`, `fractional-scale`
- [ ] `xdg-activation`, `primary-selection`, `data-control`
- [ ] `idle-notify/inhibit`, `ext-session-lock`
- [ ] `wlr-output-management`, `output-power-management`, `xdg-output`
- [ ] `ext-foreign-toplevel-list` / foreign-toplevel-management, `ext-workspace`
- [ ] `pointer-constraints`, `relative-pointer`, `keyboard-shortcuts-inhibit`, `text-input/input-method`
- [ ] Verified with waybar, fuzzel, swaybg, swaylock/gtklock, swayidle, wl-clipboard
- **Exit criteria:** replace your current compositor for a full week without fallback

### Milestone 5: Config, rules, and IPC (3-4 weeks)

- [ ] TOML config parser with validation and good error messages
- [ ] Hot reload with atomic apply and error banner
- [ ] Keybinds (keyboard, mouse, modes), hotkey overlay
- [ ] Window rules (app-id/title match, floating, space, size, opacity, focus)
- [ ] IPC socket, JSON protocol, event stream, `crawm-cli`
- [ ] Documentation of config and IPC; default config shipped
- **Exit criteria:** bars can show spaces/windows via IPC; config edits apply live

### Milestone 6: Layout variety, scroller, and animation (4-6 weeks)

- [ ] `src/animation`: spring + easing, interruptible, clock-driven
- [ ] Animate window open/close/move/resize, space switch, layout switch
- [ ] Layouts: `Scroller` (niri-style), `Grid`, `Deck`, `Spiral`
- [ ] Per-space layout memory and params restore
- [ ] Touchpad gestures (space switch, scroll) bound to animation targets
- [ ] Reduce-motion switch
- **Exit criteria:** switching layouts and spaces feels smooth and interruptible, with no visual pops

### Milestone 7: Effects (3-5 weeks)

- [ ] Rounded corners shader (windows + borders)
- [ ] Borders / focus ring with animated color
- [ ] Shadows
- [ ] Dim inactive, per-window opacity
- [ ] Dual-Kawase blur with caching and damage awareness
- [ ] Per-rule overrides and global kill switch; low-power mode
- **Exit criteria:** effects on by default in a polished config without regressing frame-time targets

### Milestone 8: Screen sharing, screenshots, XWayland (3-5 weeks)

- [ ] `wlr-screencopy` / `ext-image-copy-capture`
- [ ] Built-in screenshot UI (region/window/screen)
- [ ] PipeWire screencast (monitor, then window) + portal integration
- [ ] `block-out-from` privacy rules
- [ ] XWayland via `xwayland-satellite` with auto-spawn
- **Exit criteria:** Firefox/Chromium screen sharing, OBS capture, and a Steam game under XWayland work

### Milestone 9: Hardening and 0.1 release (3-4 weeks)

- [ ] Protocol conformance pass; fix client compatibility bugs from the test app matrix
- [ ] Multi-monitor and mixed-DPI testing; rotated outputs
- [ ] Direct scanout for fullscreen; overlay-plane experiments
- [ ] Crash handling and session restart story
- [ ] Packaging: `.desktop`, systemd units, portals config, AUR/Nix/Flatpak-less distro packages
- [ ] Docs site: getting started, config reference, IPC reference, FAQ
- [ ] Performance benchmarks and regression tracking
- **Exit criteria:** tagged **v0.1.0**; announced; issue templates and contribution guide ready

### Post-0.1 backlog (prioritize by user feedback)

- Tabbed/stacked window groups (COSMIC/i3 style)
- Overview mode (niri/GNOME style) with live thumbnails
- Multi-GPU support
- Tablet protocol, touchscreen gestures
- `security-context`, `xdg-foreign`, `xdg-dialog`, `xdg-toplevel-drag`
- `ext-background-effect` and client-requested blur
- Color management / HDR (track upstream protocol and Smithay support)
- Window-snap/edge-tiling UX for stacking users
- Scripting layer (Rhai/Lua) on top of IPC
- Embedded XWayland WM if satellite proves insufficient
- Accessibility: screen reader hooks, high-contrast borders, magnifier
- Session persistence (restore windows to spaces after compositor restart where clients allow it)
- Plugin API for custom layouts (WASM or dynamic loading) once the `Layout` trait is stable

### Versioning and stability promises

- `0.x`: config and IPC may change; breaking changes documented in `CHANGELOG.md`.
- `1.0`: stable config schema, stable IPC schema, documented deprecation policy.

---

## 23. References

**Projects to study**

- **niri**: scrollable tiling, animation system, rendering, IPC, screencast. Rust + Smithay.
- **MangoWC / mango (`mangowm`)**: spaces, many layouts, scenefx effects, plain-text config. C + wlroots.
- **COSMIC (`cosmic-comp`)**: mixed tiling/floating, protocol breadth, desktop integration. Rust + Smithay.
- **Smithay `anvil`**: canonical Smithay usage.
- **dwl / dwm**: space semantics and minimal layout model.
- **Sway / i3**: containers and IPC conventions.
- **Hyprland**: effects and animation configuration ideas (C++).
- **labwc / Wayfire**: stacking behavior and plugin architecture references.
- **river**: layout-as-external-process idea (`river-layout-v3`) if you ever want out-of-process layouts.

**Specs and docs**

- Wayland protocols repository (stable, staging, unstable) and `wlr-protocols`
- Smithay documentation and examples
- `calloop` book
- freedesktop.org: XDG specs, portals, PipeWire docs
- DRM/KMS docs (kernel.org) for atomic modesetting and planes

**Licensing note:** verify each project's license before adapting code (niri and COSMIC are GPL-3.0; wlroots-based projects vary). Reimplementing documented *ideas* is generally fine; copying *code* requires compatible licensing.
