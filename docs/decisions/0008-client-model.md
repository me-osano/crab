# ADR-0008 — Client model: a custom ordered element list, not Smithay `Space`

**Status:** Accepted
**Date:** 2026-10-23
**Depends on:** ADR-0002 (renderer), ADR-0004 (space model)
**Relates to:** ADR-0006 (XWayland), `COMPOSITOR.md` §6/§8.3, `TODO.md` Appendix A

## Context

Smithay offers a ready-made client scene: `Space` (with `DynamicScene` and
`WindowElement`) owns windows and outputs, tracks z-order, produces render
elements, and provides hit-testing. cosmic-comp builds on that model. niri, by
contrast, keeps a custom ordered element list.

The two models pull in different directions:

- `COMPOSITOR.md` §8.3 specifies an explicit bottom-to-top order: background
  layers, bottom layers, tiled windows (z-hint), floating windows (MRU,
  always-on-top last), fullscreen, top layers, popups/subsurfaces, overlay/lock,
  compositor UI, cursor.
- crab needs per-window fit/scale mapping (`src/compositor/fit.rs`), custom
  borders and later effects, custom damage identity, and compositor UI layers.
  Those do not fit `Space`'s element model without fighting it.
- §8.3's order is a product requirement, not an implementation detail; it should
  have one owner.

The current code already uses the custom model:

- the pure `Window` lives in `src/core/window.rs`;
- the compositor keeps a Smithay `ToplevelSurface` alongside it in
  `src/compositor/state.rs`;
- `src/compositor/render.rs` builds a `Vec<SceneElement>` in §8.3 order;
- hit-testing uses `under_from_surface_tree`;
- `LayerMap` is used only for layer-shell placement.

So this ADR **ratifies** the existing model and fixes its boundary. It is not a
migration.

XWayland does not change the answer: `xwayland-satellite` presents X11 windows as
ordinary `xdg_toplevel`s (ADR-0006), so they enter the same client model.

## Decision

### 1. The custom ordered element list is the scene model

The pure `Window` model plus a per-output ordered scene is the authority for what
is shown and in what order. Do **not** introduce `smithay::desktop::Space`,
`DynamicScene`, or `WindowElement` as the window store or scene. Do not run a
hybrid with two sources of truth. A future change to this decision requires a
superseding ADR.

### 2. Allowed Smithay building blocks

Use Smithay's lower-level pieces:

- `ToplevelSurface` and the xdg-shell handlers;
- `PopupManager` for unconstraining and popup placement;
- `LayerMap` for layer-shell placement and exclusive zones;
- `OutputDamageTracker` and the renderer's element types;
- surface-tree render helpers and `under_from_surface_tree` for hit-testing.

Forbidden as the scene owner: `Space`, `DynamicScene`, `WindowElement`.

### 3. One ordering function

A single function implements §8.3 and is called once per frame. The scene is
derived from the pure model plus per-frame state (animations, focus, effects); it
is never mutated by Smithay out of band. The order is:

1. background layer surfaces;
2. bottom layer surfaces;
3. tiled windows, in z-hint order, with borders/shadows;
4. floating windows, MRU with always-on-top last;
5. the fullscreen window;
6. top layer surfaces;
7. popups and subsurfaces of the above;
8. overlay layer surfaces and the session-lock surface;
9. compositor UI (overview, hotkey help, error banner, debug HUD);
10. the cursor.

Effects and decorations extend this list; they must not reorder the core layers.
Each element keeps a stable ID across frames so damage tracking can diff the
scene.

### 4. The compositor owns the rest

Because `Space` is not doing it, crab owns:

- window → output association and migration on hotplug;
- z-order and floating MRU;
- hit-testing (global → output → window → surface);
- popup unconstraining and placement;
- focus/activation updates through the single focus function;
- the surface-tree bookkeeping that maps surfaces to `WindowId`s.

### 5. Guardrail

Extend the boundary check so `smithay::desktop::Space`, `DynamicScene`, and
`WindowElement` cannot appear in `src/compositor/`. The decision says "do not
mix"; CI should enforce it.

### 6. Tests

- A unit test for the §8.3 z-order, including always-on-top, fullscreen, overlay,
  and lock.
- Element IDs remain stable across a frame; damage works across output moves.
- Popup placement/unconstraining and surface hit-testing.
- Output hotplug migration and per-output rendering.
- Fullscreen/overlay/lock interactions with top and overlay layers.

## Consequences

- crab keeps full control of ordering, per-window fit, custom elements, effects,
  and damage. This is the reason for the decision.
- crab also keeps the maintenance burden: hit-testing, popup handling, focus,
  activation, and output migration are ours. Most of this already exists.
- Some convenience helpers written against `Space` are unavailable; use the
  lower-level surface-tree helpers instead.
- The scene is rebuilt per frame. With a realistic window count this is cheap;
  the damage tracker does the expensive diffing. Avoid per-frame allocations
  where they show up in profiles.
- Smithay API churn touches more code, which is already mitigated by keeping all
  Smithay use under `src/compositor/` and `src/backend/`.
- XWayland/X11 windows need no special path: satellite delivers them as
  `xdg_toplevel`s.

## Alternatives considered

- **Smithay `Space`/`DynamicScene`.** Rejected: its z-order and element model do
  not express §8.3, per-window fit, borders/effects, or compositor UI without
  workarounds, and it would take ownership of a product requirement.
- **Hybrid (`Space` for storage, custom list for rendering).** Rejected: two
  sources of truth for order and window lifetime, and a constant source of
  desynchronisation bugs.
- **`WindowElement` without `Space`.** Rejected for the same reason as `Space`:
  crab's render path needs its own fit mapping, damage identity, and ordering
  semantics.
