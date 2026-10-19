# crawm — Roadmap

> **Name:** `crawm` (binary `crawm`, CLI `crawm-cli`).
> **Companion docs:** [`COMPOSITOR.md`](./COMPOSITOR.md) is the *design* (what and
> why). This file is the *roadmap*: what to build, in what order, which verified
> upstream feature it comes from, and how we prove the feature is complete.
> Accepted decisions live in [`docs/decisions/`](./docs/decisions/README.md); the
> granular working task list is [`TODO.md`](./TODO.md).
> **Status:** planning. Upstream facts verified **2026-10-16** against
> niri `v26.04`, mango `0.18.1`, and `cosmic-comp` master (`1.0.0`).

---

## Table of contents

1. [What this project is](#1-what-this-project-is)
2. [Design synthesis — the best of niri, mango, cosmic](#2-design-synthesis)
3. [The completeness contract (feature target matrix)](#3-the-completeness-contract)
4. [Protocol support matrix](#4-protocol-support-matrix)
5. [Crate & module topology](#5-crate--module-topology)
6. [Milestones](#6-milestones)
7. [Integration with crawl-desktop](#7-integration-with-crawl-desktop)
8. [Testing & verification](#8-testing--verification)
9. [Performance targets](#9-performance-targets)
10. [Licensing, risks, and open decisions](#10-licensing-risks-and-open-decisions)
11. [Verified upstream references](#11-verified-upstream-references)

---

## 1. What this project is

An **independent Wayland compositor + window manager in Rust on Smithay** that
merges the three strongest compositor designs into one coherent whole:

- **mango** — dwm/dwl-style **spaces** with per-space layouts, many layouts, runtime
  plain-text config, scenefx-quality effects.
- **niri** — **scrollable tiling**, a rigorous **animation system**, typed IPC +
  event stream, typed text config, excellent protocol coverage.
- **cosmic** — **mixed tiling/floating** with groups and tabbed stacks, first-class
  config hot-reload, accessibility built into the compositor.

The thesis (from `COMPOSITOR.md` §1) is **layout as a swappable strategy**: tiling,
scrolling, floating, and stacked are per-space choices, not the compositor's
identity.

### Relationship to the other projects

| Project | Role | Relationship |
|---|---|---|
| `crawm` (this) | The compositor | Server: owns DRM/KMS, input, windows |
| `crawl-desktop` | The desktop shell | Client: its `crawl-compositor` adapter gains a **`crawm` backend** (§7) |
| `crawl` v1 | The service daemon | Unchanged; runs under any compositor |
| `amane` / `suzuha` | Reference immediate-mode shell | Unchanged; a model for how a shell consumes standard protocols |

**The strategic payoff:** `crawl-desktop` was designed to run on *other people's*
compositors (niri/mango/Hyprland/Sway). `crawm` makes the ecosystem
self-contained: the shell drives our compositor through the **same standard
protocols and the same `Compositor` trait**, so the shell never learns a
crawl-specific API. This is the original "from-scratch compositor" idea, but kept
as a **separate, independently-versioned project** so `crawl-desktop` ships on
niri today and on `crawm` later without a rewrite.

### Non-goals (initially)

- Being a full desktop environment (no built-in panel/bar/settings/file manager).
- An X11 window-manager mode (XWayland via `xwayland-satellite`).
- Matching every Hyprland effect. Effects stay modular and are cut first.
- Remote desktop / network transparency.

---

## 2. Design synthesis

Ordered by where the idea comes from, with the reason we want it.

### From niri (scrollable tiling + engineering rigor)

| Take | Why |
|---|---|
| **Scrollable tiling engine** (infinite horizontal strip of columns) | The best existing tiling UX for ultrawide/many-window workflows; a layout nobody else in the stack offers |
| **Stable layout on open** (a new window never reflows existing ones) | Avoids the classic "everything jumps" complaint; a deliberate niri trait worth preserving |
| **Synced animation clock + springs** (one clock per animation kind) | Interruptible, gesture-friendly motion without per-window drift |
| **Custom-shader open/close/resize animations** | High-quality polish with a small, contained surface (inline GLSL) |
| **TOML config + `validate` + includes + hot reload** | Typed, commentable, splittable config with a real validator |
| **Typed IPC crate + line-JSON + `event-stream`** | `niri-ipc` is a proven, documented protocol that bars/tools already consume |
| **`xwayland-satellite` integration** (auto-spawn, restart) | Avoids implementing an X11 WM; the modern, low-risk choice |
| **Built-in screenshot UI** (region/window/output) | Turns a missing feature into a first-class one |
| **`security-context-v1` + restricted-client model** | Gates privileged protocols (session lock, screencopy, output mgmt) for sandboxed clients |
| **Accessibility via AccessKit / Orca** | Screen-reader support is a real differentiator (2025-era niri) |
| **Named workspaces + `on-overflow` centering** | Persistent, scriptable spaces rather than only dynamic ones |
| **Fractional-layout discipline** (single logical→physical rounding path) | Prevents blurry/misaligned borders at fractional scale |

### From mango (spaces + layouts + effects)

| Take | Why |
|---|---|
| **Space model** (per-monitor, multi-space view, **multiple spaces per window**) | More expressive than workspaces; `comboview` (several spaces at once) is genuinely useful |
| **Per-space layout state** (`Pertag`: layout + master count/factor + scroller proportion + dwindle tree) | Each space remembers *how* it was arranged; rare and valuable |
| **Layout modes** incl. `dwindle`, `fair`, `center_tile`, `vertical_*`, `deck`, `grid`, `scroll` | Broad coverage, grouped into families (ADR-0004) |
| **Special workspace (space 0) as a tiling overlay** with background dim | A better scratchpad: a full tiled surface over the current space |
| **Runtime plain-text/TOML config** with variables, `source` includes, `auto_reload`, and an **error-nag bar** | Zero-recompile iteration; errors never kill the session |
| **Per-event animation config with cubic-bezier curves** | Fine-grained motion control (niri gives springs; mango gives per-event tuning) |
| **scenefx-quality effects** (blur, shadows, radius, opacity, dim) with **per-window overrides** and **wallpaper-cached blur** | The effects feature set users actually ask for, plus the performance trick that makes blur affordable |
| **Named scratchpads** + standard scratchpad + minimize/restore | Everyday workflow |
| **Window groups + auto tab bars** | Grouping without a separate compositor |
| **Rich window states** (`is_global` sticky, `is_overlay`, `fakefullscreen`, terminal **swallow**, `is_unmanaged_global`) | The long tail of real desktop behaviour |
| **Monitor rules** by name/make/model/serial regex; per-monitor spaces; primary-output flag | Flexible multi-monitor without config sprawl |
| **IPC `get` / `watch` / `dispatch`** + `ext-workspace` + first-party bar | Ergonomic scripting and bar integration |
| **Overview (hycov-style) with jump labels**, overcircle, thumbnail switcher | Fast navigation; labels rescue non-Latin keyboard layouts |
| **Virtual outputs** (headless) and **DRM lease** (VR) | Streaming (Sunshine/Moonlight) and VR support |
| **HDR / colour / tearing / syncobj** (mango `wl-only` + Vulkan branch) | Later differentiator. mango's Vulkan coupling is **mango-specific** — we do HDR on **GLES** (ADR-0002, §4) |

### From cosmic (mixed model + integration)

| Take | Why |
|---|---|
| **Mixed tiling/floating with groups and tabbed stacks** | Cosmic's `ToggleStacking` / groups are the cleanest expression of "windows inside a slot" |
| **Orientation + per-child size fractions + swap** | Predictable manual arrangements inside a layout |
| **Edge-snap for floating windows** (8 snap corners + threshold) | Expected floating UX |
| **`WorkspaceMode::{OutputBound, Global}` + vertical/horizontal layout + wraparound** | One switch covers single- and multi-monitor mental models |
| **Pinned workspaces matched by EDID** | Stable monitor→workspace assignment |
| **Config with per-key hot reload** (change one setting, no restart) | The right granularity; matches `crawl-desktop`'s live-config goal |
| **Compositor-rendered overview** | One coherent surface rather than a bolt-on |
| **Cursor shake-to-find** (magnify the pointer when shaken) | Small, delightful, universally useful |
| **Accessibility built into the compositor** (zoom, colour filters: greyscale/protanopia/deuteranopia/tritanopia) | Accessibility as a first-class compositor feature |
| **XWayland descaling + eavesdropping knobs** | Makes legacy/X11 apps behave under fractional scale |
| **Broad, modern protocol coverage** (`ext-image-copy-capture` instead of `wlr-screencopy`, `security-context`, `drm-lease`) | Fewer legacy paths |

### Deliberately **not** copied

- **Cosmic's non-configurable animations** (hardcoded 200 ms etc.) — we want
  niri/mango-level configurability.
- **mango's C/wlroots + scenefx coupling** — we are on Smithay; we re-implement the
  *effects ideas*, we do not take a wlroots renderer dependency.
- **niri's lack of output mirroring** — we will provide mirroring (cosmic has it).
- **Global-space coupling across monitors** as a default — we adopt mango's
  `synctag` *option*, not its assumption.
- **A built-in bar/panel** — that is `crawl-desktop`'s job.

---

## 3. The completeness contract

This matrix is the promise: **every row is a feature we intend to ship**, with its
provenance, priority, target milestone, and how to verify it. Priority:
**P0** = needed to boot a usable session, **P1** = needed for a daily driver,
**P2** = polish/later. Keep this table in the repo and tick rows as they land.

### A. Spaces & window model

| Feature | Best source | Prio | MS | Verify |
|---|---|---|---|---|
| Tiling windows, master/stack | mango/dwm | P0 | M2 | N windows tile without overlap |
| Per-window floating toggle; mixed tiled+floating on one space | mango + cosmic | P0 | M2 | Dialog floats over tiled windows |
| Spaces: per-output, multi-space **view**, multiple spaces per window | mango | P0 | M2 | `view 1+3`, `space` a window onto 2 spaces |
| Named spaces / persistent workspaces | niri | P1 | M3 | Named space survives output unplug |
| Special workspace (tiling overlay + dim) | mango | P1 | M7 | Overlay tiles over current space |
| Window groups + tabbed/stacked containers | cosmic | P1 | M7 | Two windows stack in one slot |
| Manual arrangements: orientation, swap, resize | cosmic | P1 | M7 | `Super+O` flips split orientation |
| Edge-snap for floating windows | cosmic | P1 | M7 | Drag to corner snaps |
| Sticky/`global` (all spaces), `overlay`, `fakefullscreen` | mango | P2 | M7 | Sticky window shows on every space |
| Terminal **swallow** | mango | P2 | M7 | Terminal swallows GUI launched from it |
| Minimize/restore + **named scratchpads** | mango | P2 | M7 | Named scratchpad launches/shows |
| `WorkspaceMode::{OutputBound,Global}`, vertical/horizontal, wraparound | cosmic | P2 | M7 | Mode switch changes nav axis |
| Pinned spaces by **EDID** | cosmic | P2 | M7 | Workspace follows the right monitor |
| Session-persistence hints (restore windows where clients allow) | niri | P2 | post | — |

### B. Layouts

Layouts are grouped into **families** with **modes** (ADR-0004). Family = broad
strategy; mode = concrete arrangement. Two cycle actions address the two axes
(`cycle-layout-mode`, `cycle-layout-family`).

| Family | Mode | Source | Prio | MS |
|---|---|---|---|---|
| `Stack` | `floating` (cascade, mixed) | classic | P0 | M2 |
| `Stack` | `monocle` | dwm/mango | P0 | M2 |
| `Tile` | `tile` (master/stack, ratio+count, left/right/top/bottom) | mango/dwm | P0 | M2 |
| `Scroll` | `scroll` (infinite columns, focus/center/overspread, presets) | niri + mango | P0 | M3 |
| `Tile` | `grid` | mango | P1 | M7 |
| `Tile` | `deck` (master + top-of-stack) | mango/dwl | P1 | M7 |
| `Tile` | `dwindle` (smart split, preserve, manual split) | mango/Hypr | P1 | M7 |
| `Tile` | `fair`, `vertical-fair` | mango | P1 | M7 |
| `Tile` | `center-tile`, `vertical-tile`, `right-tile` | mango | P2 | M7 |
| `Scroll` | `vertical-scroll` | mango/niri | P2 | M7 |
| `Stack` | `tabbed` (grouped slots) | cosmic | P1 | M7 |
| Per-space layout memory + params restore | mango (`Pertag`) | P0 | M2/M3 |
| Per-layout directional `neighbor()` | niri | P1 | M2 |

**Layout engine rule (from `COMPOSITOR.md` §7.1):** layouts are **pure functions**
in the `src/layout` module — no Wayland, no globals — so they are unit- and
property-testable.

### C. Animation

| Feature | Source | Prio | MS |
|---|---|---|---|
| Spring + easing + cubic-bezier curves | niri + mango | P0 | M3 |
| One synced clock per animation kind | niri | P0 | M3 |
| Interruptible retargeting (keeps value + velocity) | niri | P0 | M3 |
| Animate open/close/move/resize/space-switch/layout-switch | both | P0 | M3 |
| Per-event configurable curves/durations | mango | P1 | M5 |
| Gesture-driven values → spring handoff | niri + cosmic | P1 | M6 |
| Custom-shader open/close/resize animations | niri | P2 | M8 |
| Reduce-motion switch (config + IPC) | niri | P1 | M5 |
| Resize: crop/scale last buffer until client catches up | niri | P1 | M3 |
| Close: snapshot last buffer into a texture | niri | P1 | M3 |

### D. Effects

| Effect | Source | Prio | MS |
|---|---|---|---|
| Rounded corners (SDF, clips borders/shadows) | mango/scenefx | P1 | M8 |
| Borders / focus ring, animated colour, gradients | niri + mango | P1 | M8 |
| Shadows (cached, `shadow_only_floating` option) | mango/scenefx | P1 | M8 |
| Dim inactive / opacity, per-window rules | mango | P1 | M8 |
| Blur (dual-Kawase + **backdrop cache** + damage-aware) | mango/scenefx | P1 | M8 |
| `ext-background-effect-v1` (client-requested blur) | niri | P2 | M8 |
| Per-rule overrides (`no_blur`, `no_shadow`, …) + global kill switch | mango | P1 | M8 |
| Low-power mode (disable blur/shadows on battery) | new | P2 | M8 |

### E. Configuration

| Feature | Source | Prio | MS |
|---|---|---|---|
| TOML config, comments, repeated nodes | niri | P0 | M5 |
| `validate` subcommand + good error spans | niri | P0 | M5 |
| Atomic hot reload; bad config keeps last good + on-screen error | niri + mango | P0 | M5 |
| `include` / `source` (with `optional`) | niri + mango | P1 | M5 |
| User variables (`$name`) | mango | P2 | M5 |
| Per-key live reload where cheap | cosmic | P2 | M5 |
| Default config shipped + documented | all | P0 | M5 |
| Window / space / layer / monitor / device **rules** | mango | P0 | M5 |
| Dynamic rule re-evaluation on title/app-id change | mango | P2 | M5 |

### F. IPC

| Feature | Source | Prio | MS |
|---|---|---|---|
| Unix-socket, newline-JSON, typed Rust crate (`crawm-ipc`) | niri | P0 | M5 |
| Request/response + `version` handshake | niri | P0 | M5 |
| `event-stream` (open/close/focus/space/output/config) | niri | P0 | M5 |
| `get` / `watch` / `dispatch <action>` CLI | mango (`mmsg`) | P0 | M5 |
| `--json` machine output, `msg` CLI | both | P0 | M5 |
| `ext-workspace-v1` so bars work without our IPC | mango/niri | P0 | M6 |
| Action names identical between binds and IPC (one enum) | niri/mango | P0 | M5 |
| Versioned schema, additive changes only | niri | P1 | M5 |

### G. Input & binds

| Feature | Source | Prio | MS |
|---|---|---|---|
| libinput: keyboard/pointer/touchpad/touch/tablet | all | P0 | M4 |
| Per-device config (natural scroll, tap, dwt, accel, click method) | niri/mango | P0 | M4 |
| xkb layouts + runtime switch | all | P0 | M4 |
| Keyboard binds + modifiers + `Mod` remap | niri | P0 | M5 |
| Mouse binds, axis/scroll binds, gesture binds, switch (lid) binds | mango | P1 | M5 |
| Bind flags: repeat, `allow-when-locked`, `allow-inhibiting`, release, pass | niri + mango | P1 | M5 |
| Key modes / submaps (resize, passthrough) | mango | P1 | M5 |
| Touchpad gestures: 3-finger space, 4-finger overview, live preview | niri + mango | P1 | M6 |
| Click-to-focus; optional focus-follows-mouse (scroll-safe) | all | P0 | M3 |
| `xdg-activation` focus rules + `activation_bypass` | niri + mango | P1 | M6 |
| Hotkey overlay (`hotkey-overlay-title`) | niri | P2 | M8 |
| Cursor shake-to-find | cosmic | P2 | M8 |
| Per-device rules (`device_rule` by name/type) | mango | P2 | M5 |

### H. Outputs & display

| Feature | Source | Prio | MS |
|---|---|---|---|
| Hotplug via udev; per-output spaces | all | P0 | M4 |
| Modes `WxH@Hz`, custom modes, transform, position | all | P0 | M4 |
| **Fractional scaling** (viewporter + fractional-scale) | all | P0 | M4 |
| Mirroring / clone | cosmic (niri lacks) | P1 | M6 |
| Output management protocol (`wlr-output-management`) | all | P1 | M6 |
| Output power management (DPMS) | niri/mango | P1 | M6 |
| VRR / adaptive sync | all | P1 | M6 |
| Primary-output flag (for XWayland) | mango/cosmic | P1 | M6 |
| Virtual (headless) outputs | mango | P2 | post |
| DRM lease (VR) | mango/cosmic | P2 | post |
| Tearing control / `syncobj` | mango | P2 | post |
| **10-bit SDR scanout** (`RGB10A2`) + `max_bpc=10` | cosmic (`max_bpc`) + niri HDR forks | P1 | M4 |
| 10-bit probe **per format**, not all-or-nothing | niri fork (`XR30`/`XB30` can fail to bind where `AR30`/`AB30` succeed) | P1 | M4 |
| HDR passthrough for fullscreen clients + `HDR_OUTPUT_METADATA` | cosmic HDR reference | P3 | post-0.1 |
| Desktop HDR: linear **F16** intermediate + tone mapping + PQ encode | cosmic HDR reference (design goal) | P3 | post-0.1 |
| ICC / wide-gamut / per-output colour profiles | — | P3 | post-0.1 |

> **Renderer decision (ADR-0002):** stay on Smithay's **GLES** renderer. mango's HDR
> needs *its* Vulkan renderer — a mango-specific constraint, not a Smithay one. Every
> working HDR implementation on Smithay (the cosmic HDR reference, the niri HDR
> forks) runs on **GLES/glow**, which has FP16 render targets and 10-bit GBM scanout.
> HDR depends on **unmerged** Smithay colour-management work, so plan to vendor a
> patched Smithay rev. See §10 and `docs/decisions/0002-renderer-and-hdr.md`.

### I. Session & integration

| Feature | Source | Prio | MS |
|---|---|---|---|
| `.desktop` entry, session wrapper, env import (systemd + D-Bus) | all | P0 | M4 |
| `spawn-at-startup` / autostart | niri/mango | P0 | M4 |
| Portals config (screencast/screenshot/file chooser) | all | P1 | M9 |
| logind: VT switch, pause/resume, lid, inhibit | all | P0 | M4 |
| `ext-session-lock` + idle protocols | all | P1 | M6 |
| Built-in screenshot UI | niri | P1 | M9 |
| PipeWire screencast (monitor → window) + portal | niri/mango | P1 | M9 |
| `block-out-from` screencast privacy | niri/mango | P1 | M9 |
| XWayland via `xwayland-satellite` (auto-spawn/restart) | niri | P1 | M9 |
| Crash resilience: log panic + optional supervisor restart | niri | P2 | M11 |
| Accessibility: AccessKit/Orca | niri | P2 | post |
| Accessibility: zoom + colour filters | cosmic | P2 | post |

---

## 4. Protocol support matrix

Priority **P0** (usable session) / **P1** (daily driver) / **P2** (polish) /
**P3** (later). Verified names from niri `v26.04`, mango `0.18.1`, cosmic master.
This is the same list as `COMPOSITOR.md` §12, tightened with what upstream actually
ships.

| Protocol | Prio | Notes / source |
|---|---|---|
| `wl_compositor` (+sub), `wl_shm`, `wl_seat`, `wl_output`, `wl_data_device_manager` | P0 | Core |
| `xdg-shell` (toplevels + popups, `xdg_activation`) | P0 | Core |
| `linux-dmabuf` (+ feedback) | P0 | GPU buffers |
| `wlr-layer-shell` | P0 | Bars, launchers, wallpapers |
| `xdg-decoration` (+ KDE variant) | P0 | SSDs |
| `viewporter`, `fractional-scale` | P0 | HiDPI |
| `presentation-time` | P1 | Frame timing |
| `idle-notify`, `idle-inhibit` | P1 | swayidle |
| `ext-session-lock` | P1 | Lockers |
| `primary-selection`, `data-control` (`ext` + `wlr`) | P1 | Clipboard managers |
| `cursor-shape` | P1 | Client cursors |
| `pointer-constraints`, `relative-pointer` | P1 | Games |
| `keyboard-shortcuts-inhibit` | P1 | Remote desktop |
| `text-input-v3`, `input-method-v2` | P1 | CJK/IME |
| `wlr-output-management`, `xdg-output` | P1 | `wlr-randr`, kanshi |
| `wlr-output-power-management` | P1 | DPMS |
| `ext-foreign-toplevel-list`, `wlr-foreign-toplevel-management` | P1 | Taskbars (`ext-workspace` too) |
| `ext-workspace` | P1 | Workspace widgets |
| `ext-image-copy-capture` (+`ext-image-capture-source`) | P1 | Modern screenshots/record (niri/cosmic); mango ships both this **and** `wlr-screencopy` |
| `wlr-screencopy` | P1 | Legacy `grim`/`slurp` compat |
| `xdg-desktop-portal` ScreenCast/Screenshot | P1 | Browser/OBS sharing |
| `xdg-foreign`, `xdg-dialog`, `xdg-toplevel-drag` | P2 | Parent/child, modals, dnd |
| `single-pixel-buffer`, `alpha-modifier` | P2 | Cheap wins |
| `content-type`, `tearing-control` | P2 | Gaming/video |
| `tablet-v2` | P2 | Drawing tablets |
| `security-context-v1` | P2 | Flatpak/sandbox (niri restricted model) |
| `ext-background-effect` | P2 | Client-requested blur |
| `drm-lease` | P2 | VR |
| `wp_color_management_v1` (staging) + `wp_color_representation_v1` | P3 | HDR/colour (ADR-0002). Needs unmerged Smithay work (PR #1143); **GLES, not Vulkan**. The `representation` protocol is required for Wine/Proton scRGB + BT.2020 passthrough |
| `wp_commit_timing` / `wp_fifo` | P3 | Future frame pacing |

**Where we deliberately exceed upstream:** niri has **no** output mirroring, and niri
and cosmic ship **no** colour-management — **we add both**. cosmic has **no**
`wlr-screencopy` (only `ext-image-copy-capture`) and **no** `wlr-foreign-toplevel`
(uses `ext-*` + its own); we choose the ecosystem-friendly superset:
`ext-image-copy-capture` **and** legacy `wlr-screencopy`.

---

## 5. Crate & module topology

Apply the same split rule as `crawl-desktop` (their ADR-0005,
`docs/decisions/0005-crate-modularization.md`): **modules first**, promote
to a crate only when (1) ≥2 consumers with different dep trees, (2) a heavy/system
dep must be isolated, or (3) a genuinely stable public API earns independent
versioning.

```
crawm/
├── Cargo.toml                 # the `crawm` package (src/) AND the workspace root (niri-style)
├── COMPOSITOR.md              # design (existing)
├── ROADMAP.md                 # this file
├── src/                     # the compositor: Smithay glue, backends, handlers, render, input
│   ├── lib.rs                 # library surface: the pure modules only (reusable, testable)
│   ├── core/                  # pure data: ids, geometry, spaces/spaces, window model — no Smithay
│   ├── config/                # TOML parsing, validation, defaults, schema, rules (pure)
│   ├── layout/                # pure Layout trait + all layouts (unit/property-tested, no Smithay)
│   └── animation/             # springs/easing/curves + animated scalars (pure, no Smithay)
├── tests/                     # headless integration tests + transcripts (root package's tests/)
├── crawm-ipc/              # IPC types + client; shared by compositor and CLI (leaf)
├── crawm-protocols/        # generated Wayland protocol bindings (wayland-scanner) — only if >1 crate needs them
├── crawm-cli/              # CLI client (`crawm-cli`)
├── resources/                 # default-config.toml, session units/.desktop, shaders, portals
└── docs/                      # mdBook; ADRs
```

Rules:
- The root package is **lib + bin**: `src/lib.rs` exposes the pure modules below
  and `src/main.rs` is the thin Smithay entry point. The boundary script rejects
  any non-pure `mod` in `lib.rs`, so the reusable surface stays graphics-free and
  `tests/` can drive the pure modules directly (no shelling out to the binary).
- The `src/{core,config,layout,animation}` modules are Smithay/Wayland-free by
  rule → fast unit tests, and they are what a future `crawl-desktop` could reuse.
  Enforced by `scripts/check-boundaries.sh` (source scan, since cargo metadata
  cannot isolate modules of one package).
- The pure modules depend **downward only**: `core` on nothing, and
  `config`/`layout`/`animation` on `core`. The same script fails on any upward or
  sideways `crate::` edge (e.g. `core` reaching for `config` or `backend`).
- Backends are **cargo features** on the `crawm` package: `backend-winit`
  (default dev path), `backend-tty` (DRM/KMS, M4), `backend-x11`. A default build
  therefore skips the DRM/session/input stack; CI builds `--all-features` to keep
  proving it (TODO 0.4).
- `src/core` and `src/config` were folded in from the former `crawm-core` /
  `crawm-config` crates, and `src/layout` / `src/animation` from `crawm-layout` /
  `crawm-anim`: each has a single consumer, so the `crawl-desktop` ADR-0005
  "modules first" rule above applies. Promote a module back to a crate only when (1) fires — e.g.
  `crawl-desktop` needs it.
- `crawm-ipc` is a **leaf** (no other `crawm-*` deps) and stays a crate: it has
  two in-tree consumers plus third-party tools.
- Only `crawm` (and `crawm-cli` for IPC) touch the IPC socket; only `crawm`
  touches Smithay.
- Keep generated protocol code in `crawm-protocols` **only if ≥2 crates need it**;
  otherwise a `protocols` module inside `crawm`.
- **At M0** the workspace realizes `crawm` (root package) / `crawm-ipc` /
  `crawm-cli` plus `resources/`. `crawm-protocols` is deferred until a second
  crate needs the bindings, and integration tests live in the root package's
  `tests/` (the root `Cargo.toml` is both the `crawm` package and the workspace
  root, niri-style).

Inside `src/`: `lib.rs` (pure library surface), `main.rs`, `state.rs`,
`backend/{mod,winit,tty,headless}.rs`,
`handlers/` (one per protocol), `input/{mod,keyboard,pointer,gestures,binds}.rs`,
`render/{mod,elements/,shaders.rs,damage.rs}`, `core/` (pure data model),
`config/` (pure TOML parse/validate), `layout/` (pure), `layout_glue.rs`,
`animation/` (pure primitives + the frame-clock driver), `ipc.rs`,
`config_reload.rs`, `screencast/`, `xwayland.rs`. (See `COMPOSITOR.md` §5.)

---

## 6. Milestones

Each milestone ends with **something runnable and demoable**. Estimates assume
part-time solo work and are rough; **order matters more than dates**. The granular,
chronological task list lives in [`TODO.md`](./TODO.md); these milestone summaries
supersede `COMPOSITOR.md` §22 where they differ.

### M0 — Foundations *(1–2 weeks)*
- [x] Settle the **license** — **GPL-3.0-or-later** (ADR-0001, §10).
- [x] Create the workspace + the §5 layout (root `crawm` package + `crawm-ipc` /
      `crawm-cli`); `[workspace.dependencies]`.
- [x] Pin Smithay to a git rev (niri does this) or `0.7.x`+patch (cosmic does).
      *(Took `0.7.0` from crates.io — reproducible and matches cosmic; the vendored
      git rev arrives with HDR. See TODO 0.4.)*
- [x] CI: build, clippy `-D warnings`, fmt, tests, `cargo-deny`.
- [x] `tracing` + panic hook to `$XDG_STATE_HOME`.
- [ ] Get Smithay `anvil` building locally as a reference.
- [x] **Exit:** `cargo run` opens a winit window with a solid background.

### M1 — First windows *(2–3 weeks)*
- [x] Winit backend + `calloop` event loop (single-owner `State`).
- [x] Core globals: compositor, shm, seat, output, data-device.
- [x] `xdg-shell` toplevels + popups *(placed/configured, drawn from the
      `PopupManager` tree; grabs and `xdg_popup.reposition` landed with M3's
      protocol breadth, verified nested with GTK3 menus and GTK4 popovers)*.
- [x] Keyboard + pointer input; click-to-focus; software cursor *(wired; pointer
      motion, client cursor surfaces, and the named-icon fallback were exercised nested,
      keyboard routing still needs a real session).*
- [x] GLES render; frame callbacks; per-output redraw state machine
      (Idle→Queued→WaitingForVBlank→Idle). *(Transitions are pure and unit-tested in
      `src/core/output.rs`; the winit backend schedules them from `Redraw` events.
      Damage-aware partial redraws are still to come; the animation frame clock landed
      with M3 (re-timed at the frame boundary under winit; the real vblank timestamp
      arrives with the M4 backend).)*
- [x] Fixed cascade placement.
- [ ] **Exit:** `foot` + a GTK app run nested, accept input, resize. *(`foot` and GTK
      apps (`pavucontrol`, `nautilus`) run nested against the M1 globals; pointer input
      and cursor rendering were exercised, keyboard input and resize still need a real
      session.)*

### M2 — Spaces & layout core *(3–4 weeks)* — *the "best of mango+dwm" phase*
- [x] `src/core`: `WindowId`, `OutputId`, `SpaceSet`, `Rect`, `Window`, `Output`,
      `SpaceView`, `FocusTarget`.
- [x] `src/layout`: `Layout` trait, `LayoutInput`, `Placement`, `LayoutParams`.
- [x] Layout families: `Stack` (`floating`, `monocle`) + `Tile` (`tile`). Mixed
      tiled+floating on one space.
- [x] Spaces: `view`, `move-to-space`, `toggle-space`, **multi-space view**, multi-space windows.
- [x] **Per-space layout memory + params** (`Pertag` equivalent).
- [x] Cycle layout **family** and **mode** (two actions); master ratio/count; gaps/borders.
- [x] Interactive move/resize for floating; float-tiled conversion.
- [x] Unit + `proptest` invariants (no overlaps, in-area, deterministic).
- [ ] **Exit:** a full day of tiling + floating dialogs works nested, with per-space
      layouts remembered. *(Two `foot` windows and GTK apps verified tiled nested;
      floating dialogs and a full interactive day remain.)*

### M3 — Scroller + animation engine *(4–6 weeks)* — *the "best of niri+mango" phase*
- [x] `src/animation`: spring + easing + bezier; **one synced clock per kind**;
      interruptible retargeting; `is_animating()`.
- [ ] `Scroll` family `scroll` mode (columns, focus/center/overspread, proportion presets,
      per-column width, vertical variant).
- [ ] Named spaces + `on-overflow` centering; `default-column-display normal|tabbed`.
- [x] Animate move/resize/layout-switch: windows spring to their new rectangles, and
      borders, popups and hit-testing follow the drawn rectangle.
- [ ] Animate open/close/space-switch.
- [ ] Resize: configure immediately, draw last buffer scaled/cropped until commit.
      Close: snapshot last buffer.
- [x] Animations advance on frame time: the winit backend re-times the frame clock at the
      frame boundary and keeps requesting frames only while something is moving. The real
      vblank/`wp_presentation` timestamp arrives with the M4 backend.
- [ ] **Exit:** space/space switches and layout changes are smooth and interruptible,
      no pops.

### M4 — Bare-metal backend *(3–4 weeks)*
- [ ] libseat session, udev discovery.
- [ ] DRM/KMS + GBM + EGL; vblank-driven redraw.
- [ ] libinput devices + per-device config; xkb + runtime layout switch.
- [ ] Hardware cursor plane.
- [ ] VT switch + session pause/resume; logind integration.
- [ ] `linux-dmabuf`, `viewporter`, `presentation-time`, **fractional-scale**.
- [ ] **10-bit SDR scanout** (`RGB10A2`) + `max_bpc=10`, with a **per-format** probe
      (some formats fail to bind on some GPUs — ADR-0002, §3H).
- [ ] Output hotplug; per-output space views; transform.
- [ ] `.desktop` + session wrapper + env import.
- [ ] **Exit:** boot from a TTY and use `crawm` on real hardware.

### M5 — Config, rules, IPC *(4–6 weeks)*
- [ ] `src/config`: TOML parse + validate + `include`/`source` + variables;
      atomic hot reload + on-screen error; `crawm-cli validate`.
- [ ] Binds: keyboard (+`Mod` remap), mouse, axis, gesture, switch; flags
      (`repeat`, `allow-when-locked`, `allow-inhibiting`, release, pass);
      **key modes/submaps**.
- [ ] Rules: window, space, layer, monitor, device (match + actions from §3E).
- [ ] `crawm-ipc` + socket + `event-stream` + `version`; `crawm-cli`
      `get`/`watch`/`dispatch`; bind and IPC share **one `Action` enum**.
- [ ] Per-event animation curves/durations; reduce-motion switch.
- [ ] **Exit:** a bar shows spaces/windows over IPC; config edits apply live; bad
      config never crashes.

### M6 — Protocol breadth P0/P1 *(4–6 weeks)* — *the "cosmic breadth" phase*
- [ ] `wlr-layer-shell` with exclusive zones → `usable_area`.
- [ ] `xdg-decoration`, `cursor-shape`, `fractional-scale`, `viewporter`.
- [ ] `xdg-activation` (+`activation_bypass`), `primary-selection`, `data-control`.
- [ ] `idle-notify/inhibit`, `ext-session-lock`.
- [ ] `wlr-output-management`, `wlr-output-power-management`, `xdg-output`, mirroring.
- [ ] `ext-foreign-toplevel-list` / `wlr-foreign-toplevel-management`, `ext-workspace`.
- [ ] `pointer-constraints`, `relative-pointer`, `keyboard-shortcuts-inhibit`,
      `text-input-v3` / `input-method-v2`.
- [ ] Touchpad gestures → animation targets (live preview).
- [ ] Verify with waybar, fuzzel, swaybg, swaylock/gtklock, swayidle, wl-clipboard.
- [ ] **Exit:** replace your current compositor for a **full week** without fallback.

### M7 — Layout variety, grouping, window states *(4–6 weeks)* — *"all of mango + cosmic"*
- [ ] Layouts: `Grid`, `Deck`, `Dwindle`, `Fair`, `VerticalFair`, `CenterTile`,
      `VerticalTile`, `RightTile`, `Vertical*`; per-layout params + presets.
- [ ] **Special workspace** overlay with dim + per-app auto-assign.
- [ ] **Groups + tabbed/stacked** containers (cosmic); auto tab bar.
- [ ] Named scratchpads + standard scratchpad + minimize/restore.
- [ ] Window states: `global` (sticky), `overlay`, `fakefullscreen`, swallow,
      `unmanaged_global`.
- [ ] `WorkspaceMode::{OutputBound,Global}`, vertical/horizontal, wraparound,
      pinned spaces by EDID.
- [ ] **Exit:** every layout in §3B works; grouping and scratchpads feel native.

### M8 — Effects *(3–5 weeks)* — *"mango/scenefx quality on Smithay"*
- [ ] Rounded corners (SDF) clipping borders + shadows.
- [ ] Borders / focus ring with animated colour + gradients.
- [ ] Shadows (cached, `shadow_only_floating`).
- [ ] Dim inactive + per-window opacity rules.
- [ ] Dual-Kawase blur, **backdrop cache**, damage-aware; `ext-background-effect`.
- [ ] Per-rule overrides + global kill switch; low-power mode.
- [ ] Custom-shader open/close/resize (niri); hotkey overlay; cursor shake-to-find.
- [ ] **Exit:** effects on by default in a polished config, without regressing §9.

### M9 — Overview, capture, XWayland *(4–6 weeks)*
- [ ] Overview: zoom-out grid of spaces, live thumbnails, `jump labels`, gesture
      open/close, drag-and-drop.
- [ ] Screenshots: built-in UI (region/window/output) + `ext-image-copy-capture`
      + legacy `wlr-screencopy`.
- [ ] Capture path is colour-correct: once HDR exists, convert HDR→SDR for
      screenshots/screencast (otherwise captures look washed out).
- [ ] PipeWire screencast (monitor → window) + portal integration.
- [ ] `block-out-from "screencast"` privacy rules.
- [ ] XWayland via **`xwayland-satellite`** (auto-spawn, restart, `DISPLAY`).
- [ ] **Exit:** Firefox/Chromium screen share, OBS, and a Steam game under XWayland
      all work.

### M10 — crawl-desktop integration *(2–3 weeks, can overlap M6+)*
- [ ] Add a **`crawm` backend** to `crawl-desktop`'s `crawl-compositor`
      implementing the `Compositor` trait (§7).
- [ ] Ensure `ext-workspace`, `ext-foreign-toplevel-list`, `wlr-output-management`,
      `ext-image-copy-capture`, and IPC `dispatch` cover every `Capability`.
- [ ] Run `crawl-shell` (panel/launcher/notifd/osd/lock) unmodified on `crawm`.
- [ ] **Exit:** `crawl-desktop` boots on `crawm` with the same capability
      negotiation it uses on niri.

### M11 — Hardening & `v0.1.0` *(3–4 weeks)*
- [ ] Protocol conformance pass; fix the client compatibility matrix bugs.
- [ ] Multi-monitor + mixed-DPI + rotated outputs.
- [ ] Direct scanout for fullscreen; overlay-plane experiments.
- [ ] Crash handling + session restart story; log to `$XDG_STATE_HOME`.
- [ ] Packaging: `.desktop`, systemd units, portals, AUR/Nix/distro packages.
- [ ] Docs site: getting started, config reference, IPC reference, FAQ.
- [ ] Benchmarks + regression tracking.
- [ ] **Exit:** tagged **v0.1.0**, announced, issue templates + contribution guide.

### HDR & colour management *(post-0.1)*
> Begins after a usable `v0.1.0`. Depends on **unmerged** Smithay colour-management
> work (PR #1143) → plan to **vendor a patched Smithay git rev**. **GLES only; do not
> adopt Vulkan** (ADR-0002). See `docs/decisions/0002-renderer-and-hdr.md`.
- [ ] **Phase 1 — HDR passthrough:** fullscreen/borderless HDR clients (games, video)
      get PQ/BT.2020 scanout + `HDR_OUTPUT_METADATA`; the SDR desktop is tone-mapped
      around them. Implement `wp_color_management_v1` + `wp_color_representation_v1`
      behind a flag (the latter is required for Wine/Proton scRGB + BT.2020).
- [ ] **Phase 2 — desktop HDR:** composite into a **linear F16** intermediate,
      apply per-surface transforms + tone mapping, encode PQ at the output boundary;
      teach effects (blur/shadows) the working space.
- [ ] **Phase 3 — real colour management:** ICC / wide-gamut / per-output profiles via
      the **DRM Color Pipeline API** (Linux 6.19+; 1D/3D LUTs, matrices).
- [ ] Keep direct/overlay scanout correct while an HDR post-process is active (the
      cosmic reference disables them during HDR).
- [ ] **Exit:** native HDR games/video *and* HDR screen capture both look correct.

### Post-0.1 backlog (prioritise by user feedback)
Multi-GPU; tablet + touchscreen gestures; `security-context`, `xdg-foreign`,
`xdg-dialog`, `xdg-toplevel-drag`; DRM-lease/VR; virtual
outputs; session persistence; scripting layer (Rhai/Lua) over IPC; embedded
XWayland WM if satellite proves insufficient; **accessibility** (AccessKit/Orca,
high-contrast, magnifier); plugin API for custom layouts once the `Layout` trait is
stable.

### Versioning promises
- `0.x`: config + IPC may change; breaking changes documented in `CHANGELOG.md`.
- `1.0`: stable config + IPC schema with a documented deprecation policy.

---

## 7. Integration with crawl-desktop

`crawl-desktop` already defines the integration seam: the `Compositor` trait in
`crates/crawl-compositor` and the capability enum
`{ Workspaces, WindowList, FocusWindow, Screenshots, OutputManagement, Overview, Launch }`,
with `Event::{WorkspacesChanged, ActiveWorkspaceChanged, WindowsChanged, ActiveWindowChanged, OutputsChanged}`.

**Goal:** `crawm` is a first-class backend, discoverable by env var, with **no
crawl-specific API** — the shell uses standard protocols + a small IPC surface.

| `crawl-compositor` need | `crawm` provides |
|---|---|
| `Workspaces` / workspace events | `ext-workspace-v1` (bars) + IPC `get spaces`/`watch` for names/urgency |
| `WindowList`, `FocusWindow`, active window | `ext-foreign-toplevel-list` + `wlr-foreign-toplevel-management`; IPC `dispatch focus-window` |
| `Outputs`, `OutputManagement` | `wlr-output-management` + `xdg-output` (matches the existing matrix row) |
| `Screenshots` | `ext-image-copy-capture` (+ legacy `wlr-screencopy`) |
| `Overview` | IPC `dispatch overview` (toggle) |
| `Launch` | IPC `dispatch spawn <cmd>` |
| Detection | export `CRAWM_SOCKET`; `crawl-compositor::detect()` gains one arm (before niri) |
| Sessions | `crawl-session` either spawns `crawm` in its own `.desktop`, or attaches as a client when `CRAWM_SOCKET` is set |

Rules:
- **Never require a crawl-specific protocol.** If a shell feature can use a
  standard protocol, it must.
- Keep the `crawm-ipc` `Action` enum and `crawl-desktop`'s action names aligned
  so one keybind table can target both where possible.
- The `crawl-desktop` boundary lint (their ADR-0005,
  `docs/decisions/0005-crate-modularization.md`) still applies: the shell
  links `crawl-ui` + `crawl-ipc`, never the daemon — and never `crawm`'s internals.

---

## 8. Testing & verification

| Layer | Method |
|---|---|
| Layout math | Unit + `proptest` invariants in `src/layout` (no graphics) |
| Animation curves | Deterministic-clock unit tests |
| Config | Parse/validate tests, golden error messages, parser fuzzing |
| Protocol handlers | Headless backend + a `wayland-client` test harness |
| State machine | Scripted scenarios: open N windows, switch spaces, unplug output, assert focus/placements |
| Visual regression | Headless render → PNG, compare with tolerance (later) |
| **Real-world matrix** | Firefox, Chromium (Ozone), GTK3/4, Qt5/6, Electron, foot/alacritty/kitty, mpv, a Steam game via XWayland, OBS, a screen-share tab, a lock/unlock cycle |
| **Integration** | `crawl-desktop` shell on `crawm` (§7), plus the existing `docs/compositor-matrix.md` rows filled in |

Dev workflow: `cargo run -- --backend winit`; debug HUD (frame time, damage,
scanout, animation count); `RUST_LOG` per module; optional Tracy; `WAYLAND_DEBUG=1`;
`wayland-info`; CI runs build/clippy/fmt/tests/headless/`cargo-deny`.

---

## 9. Performance targets

| Metric | Target |
|---|---|
| Idle CPU (nothing changing) | ~0% (no unnecessary redraws) |
| Frame time, 4 windows, no effects, 1440p/144 | < 2 ms CPU, no missed vblanks |
| Frame time with blur | < 6 ms GPU on a mid-range iGPU at 1080p60 |
| Input-to-photon latency | one frame; direct scanout + cursor plane |
| Resident memory | < 100 MB (excludes client buffers) |
| Config reload | < 50 ms, no dropped frame |
| Startup → first frame | < 500 ms |

Regressions block merges once benchmarks exist.

---

## 10. Licensing, risks, and open decisions

### License — **DECIDED: `GPL-3.0-or-later`** (ADR-0001)

| Upstream | License |
|---|---|
| **niri** | **GPL-3.0-or-later** |
| **cosmic-comp** | **GPL-3.0-only** |
| **mango** | **GPL-3.0-or-later** (dwl base is also GPL-3.0-or-later; dwm/wlroots/sway/scenefx are MIT) |
| **Smithay** | MIT |

**Implication.**
- If you want to **copy code** from niri/cosmic/mango, `crawm` must be
  **GPL-3.0(-or-later)** and carry attribution.
- If you want a **permissive** license (MIT/MPL/Apache), you may **only
  re-implement from docs/specs**, never copy their source.

**Recommendation:** license `crawm` **GPL-3.0-or-later**. It is a *separate
process* talking to `crawl-desktop` (MIT) over Wayland/IPC, so copyleft does not
propagate into the MIT crates — and it lets you lift proven code (niri's scroller,
mango's layout math) with attribution instead of re-deriving it. If you prefer a
permissive core, split it: GPL-3.0 for the binary/renderer, MIT for the pure
`-core`/`-layout`/`-anim`/`-config` crates you write yourself.

**Decision (2026-10-16):** `crawm` is **`GPL-3.0-or-later`**. This permits
copying and adapting code from niri, cosmic-comp, and mango (all GPL-3.0) with
attribution — including the HDR/colour-management work — while keeping the
`crawl-desktop`/`crawl` MIT crates unaffected (separate process, no linking). See
`docs/decisions/0001-license.md`.

### Risks & mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Smithay API churn | rebase pain | Pin a git rev; isolate Smithay-facing code in `handlers/`+`backend/`; update in dedicated PRs |
| Scope creep (14 layouts + effects + HDR) | never daily-drivable | Hard milestone gating; M6 exit = one full week as primary |
| Renderer choice blocks HDR | rework | **Resolved (ADR-0002):** GLES, and HDR works on GLES. Residual: Smithay colour-management is **unmerged** → plan to vendor a patched Smithay rev |
| Effects ↔ damage bugs | frame drops | Budget + cache blur; keep effects behind flags; M8 exit checks §9 |
| Fractional-scale rounding | blurry borders | Single logical→physical conversion site + snapshot tests |
| NVIDIA quirks | unusable for some | Test early on proprietary + NVK; document a `Nvidia`-style page |
| Single-maintainer burnout | stall | Small milestones, docs from day one, early public releases |
| License mixups | legal friction | Settle §10 in M0; attribute any adapted code |

### Decisions (see `docs/decisions/`)

- **ADR-0001 — License:** `GPL-3.0-or-later`. ✅ *decided 2026-10-16*
- **ADR-0002 — Renderer & HDR:** Smithay **GLES**; HDR is a post-0.1, phased feature
  on GLES (never Vulkan). ✅ *decided 2026-10-16*
- **ADR-0003 — Config format:** **TOML** (via `serde` + the `toml` crate; keys are
  `snake_case`). ✅ *decided 2026-10-16*
- **ADR-0004 — Space model:** unified **numbered spaces** (a `SpaceSet` bitset per
  output; multi-space view and multi-space windows) with **layout families + modes**;
  "workspace" is a protocol/IPC projection (`ext-workspace-v1`), not a core type.
  ✅ *decided 2026-10-17*
- **ADR-0005 — IPC** *(open)*: reuse niri's `niri-ipc` schema shape, or a fresh
  schema; and whether to expose `ext-workspace` as the primary bar interface
  (recommended).
- **ADR-0006 — XWayland** *(open)*: `xwayland-satellite` (recommended) vs in-process
  (cosmic).
- **ADR-0007 — Effects** *(open)*: implement on Smithay GLES ourselves vs adopt a
  scenefx-like crate.

**M0 foundation choices** (recorded in `TODO.md` Phase 0; promote to ADRs if revisited):

- **Edition 2024 + `resolver = "3"`**, workspace `rust-version = "1.85"` — to match
  `crawl`/`crawl-desktop` and get MSRV-aware resolution.
- **Smithay `0.7.0` from crates.io** (not a git rev): no unreleased patches are
  needed yet, and a released space is reproducible; the vendored git rev arrives with
  HDR (ADR-0002). `backend_vulkan` is off (ADR-0002); `xwayland` is deferred to M9.

---

## 11. Verified upstream references

**Meta (verified 2026-10-16):**

| Project | Latest | Language / toolkit | License | Size |
|---|---|---|---|---|
| niri (`niri-wm/niri`) | v26.04 | Rust + Smithay (git pin) | GPL-3.0-or-later | ~97.6k LOC |
| mango (`mangowm/mango`) | 0.18.1 | C + wlroots 0.20 + scenefx 0.5 | GPL-3.0-or-later | ~30–36k LOC |
| cosmic-comp (`pop-os/cosmic-comp`) | 1.0.0 | Rust + Smithay 0.7 (upstream pin) | GPL-3.0-only | ~73k LOC |

**niri**
- https://github.com/niri-wm/niri · https://niri-wm.github.io/niri/
- IPC: `docs/wiki/IPC.md` · Animations: `Configuration:-Animations` · Layout:
  `Configuration:-Layout` · Window Rules · Input · Outputs · Gestures
- Overview: `docs/wiki/Overview.md` · Xwayland: `docs/wiki/Xwayland.md` ·
  Security: `docs/wiki/Security-Model.md` · Accessibility: `docs/wiki/Accessibility.md`

**mango**
- https://github.com/mangowm/mango · https://mangowm.github.io/docs/ ·
  https://mangowm.github.io/llms-full.txt
- Layouts · Rules · Effects · Animations · IPC · Monitors · Input · Keys/Mouse ·
  Overview · Scratchpad (docs pages) · `mangobar`
- https://github.com/mangowm/mangobar · https://github.com/DreamMaoMao/mango-config

**cosmic**
- https://github.com/pop-os/cosmic-comp · https://github.com/pop-os/cosmic-protocols
- https://github.com/pop-os/cosmic-epoch ·
  https://system76.com/blog/post/cosmic-de-tiling-redesign-and-libcosmic-rebasing
- HDR: issue #1384 · Animations: issue #376

**HDR / colour management (verified 2026-10-16)** — see ADR-0002
- `wp_color_management_v1` — wayland-protocols **staging**, landed 2025-02-13;
  `wp_color_representation_v1` for Wine/Proton scRGB + BT.2020 passthrough
- Smithay colour-management: PR `Smithay/smithay#1143` *(open)* · `#982`
- Cosmic HDR reference (validated, **GLES**):
  https://github.com/franzjeger/cosmic-hdr-reference
- niri HDR forks (GLES): https://github.com/dividebysandwich/niri
  (`hdr-smithay-master`) · https://github.com/losnoco/niri (`color-management-features`)
- Linux **DRM Color Pipeline API** (kernel 6.19+):
  https://docs.kernel.org/gpu/rfc/color_pipeline.html
- Hyprland HDR on GLES: https://wiki.hypr.land/Configuring/Monitors/

**Ecosystem / specs**
- Smithay + `anvil` example · `calloop` book · `wayland-protocols` (stable,
  staging, unstable) · `wlr-protocols` · `xdg-desktop-portal`
- Reference compositors to study: dwl/dwm (spaces), sway/i3 (containers + IPC),
  Hyprland (effects config ideas), labwc/Wayfire (stacking + plugins),
  river (out-of-process layouts), `xwayland-satellite`
