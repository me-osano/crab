# ADR-0006 — XWayland: `xwayland-satellite` with on-demand activation

**Status:** Accepted
**Date:** 2026-10-23
**Depends on:** ADR-0001 (license)
**Relates to:** ADR-0008 (client model), `COMPOSITOR.md` §15, `ROADMAP.md` §4/§10,
`TODO.md` Phase 9

## Context

Running X11 applications requires an XWayland server **and**, for rootless
integration, an X11 window manager that maps X11 windows onto Wayland surfaces.
Two viable approaches:

1. **`xwayland-satellite`** — a separate process that is simultaneously an X11
   window manager, a Wayland client of crab, and a Wayland server for XWayland.
   It re-exposes X11 toplevels as ordinary `xdg_toplevel`/`xdg_popup` surfaces.
2. **In-process XWayland** — crab embeds Smithay's `xwayland` support and
   implements the X11 window-manager side itself (ICCCM/EWMH, override-redirect,
   focus and stacking quirks).

Facts that shaped the decision (verified 2026-10-23):

- Satellite is **MPL-2.0** (separate process; no linking or copyleft interaction
  with GPL-3.0 crab).
- The host compositor only needs core Wayland plus **`xdg_wm_base` and
  `wp_viewporter`**. `xdg-activation`, `xdg-decoration`, `fractional-scale`,
  dmabuf, primary selection, pointer constraints, tablet, and `xdg-foreign`
  improve quality but are not required.
- Satellite needs **Xwayland ≥ 23.1**, `xcb`, and `xcb-util-cursor`.
- Satellite supports passing Xwayland's **`-listenfd`** argument. That is what
  makes on-demand startup possible: the compositor can create the X11 sockets,
  listen for the first X client, and only then spawn satellite. niri does exactly
  this.
- In-process support in Smithay supplies XWayland plumbing, not an X11 window
  manager; option 2 still means writing the WM.

Known satellite limitations to document rather than solve: mixed-DPI setups pick
the **smallest** monitor's DPI; X11 applications cannot be absolutely positioned
(Steam notifications are the canonical example); some Java apps need
`_JAVA_AWT_WM_NONREPARENTING=1`; X11 clients can snoop each other, unlike
Wayland clients.

## Decision

### 1. Use `xwayland-satellite`; do not implement an in-process X11 WM

Satellite is the supported path. Smithay's `xwayland` feature stays **off**.
The in-process route is not a parallel implementation and must not be started
speculatively; it is only revisited if satellite blocks a real application (see
the trigger below).

### 2. The compositor owns the satellite lifecycle

"Start it when an X client connects" is crab code, not a satellite feature:

- On startup, when XWayland is enabled, create the X11 sockets and allocate a
  display number; export `DISPLAY` to spawned and autostarted children.
- Listen for X connections. On the first connection, spawn
  `xwayland-satellite <display> -listenfd <fd>`, passing the accepted socket.
- If the user configured an explicit `display`, honour it; otherwise pick a free
  `:N`.
- Restart satellite with bounded backoff after a crash; clear stale sockets and
  `DISPLAY` if it cannot be started.
- Terminate the child on compositor shutdown and surface its stderr in the log.
- Spawn and reap off the `calloop` loop (child watcher or a worker plus
  `calloop::channel`); never block the loop on process setup.

### 3. Configuration and feature gate

```toml
[xwayland]
enabled = true
satellite_path = "xwayland-satellite"
# display = ":0"          # optional; otherwise allocated on demand
```

The cargo feature is `xwayland`. When it is off, crab creates no X11 sockets and
exports no `DISPLAY`. This is unrelated to `backend-x11`, which makes crab a
nested **Wayland client** under an X11 session; keep the names distinct in docs.

### 4. Runtime dependency, not a bundled component

Document satellite and its dependencies as runtime requirements. Do not vendor,
link, or bundle the satellite binary. Distribution packages (AUR/Nix/Fedora)
should depend on the distro's `xwayland-satellite` package. Pin a minimum
satellite version and log the version at startup.

### 5. Protocol quality gates

Treat the optional host protocols as quality work, not prerequisites. XWayland
must not be declared done until at least `xdg-activation`, `xdg-decoration`,
`fractional-scale` + `viewporter`, dmabuf, and primary selection behave. Missing
pointer constraints or tablet support degrades gracefully.

### 6. Test matrix

- Steam/Proton, both native Wayland and X11.
- Wine/Proton and a plain X11 application.
- A Java application (with and without the non-reparenting workaround).
- Electron/Chromium and a Qt application under X11.
- `xclip`/`xsel` against `wl-clipboard`.
- Multi-monitor, including mixed DPI, and output hotplug.
- Satellite crash, manual kill, and restart; compositor shutdown while X11
  clients are running.

### 7. Revisit trigger

Start a superseding ADR only when a **real, reproducible application** cannot
work through satellite. "More control" or "purity" is not a trigger. If that
happens, the in-process WM becomes a scoped milestone with its own test matrix.

## Consequences

- Almost no X11 window-manager code lives in crab; the work is socket/display
  management, process lifecycle, `DISPLAY` propagation, and protocol quality.
- On-demand activation must be implemented carefully: the listener exists before
  any X client can connect, and spawning is asynchronous.
- Satellite updates require a compositor restart while integrated; the docs and
  packaging must say so.
- Mixed-DPI and absolute-positioning limitations are user-visible; document them
  instead of promising parity with a native X11 session.
- X11 applications are X11 clients for security purposes; this is inherent to
  running them, not a crab defect.

## Alternatives considered

- **In-process Smithay XWayland.** Rejected for the first implementation: it
  requires writing an X11 window manager and all of its quirks, owns far more
  code, and ties the feature to Smithay's XWayland surface. It remains the
  documented escape hatch.
- **Rootful XWayland** (a fullscreen X root window). Rejected as the normal
  path: it does not integrate X11 windows with crab's spaces, layout, or
  per-window rules. It remains useful for debugging.
- **Require a systemd user unit and always run satellite.** Rejected as the only
  option: it works, but on-demand startup is the better default and needs the
  same compositor integration anyway. An explicit `satellite_path` still allows
  a manual/externally managed instance.
- **Do nothing.** Rejected: Steam, older games, and a long tail of X11 tools are
  required for daily driving.
