# ADR-0007 — Effects: own GLES passes over the ADR-0008 scene

**Status:** Accepted
**Date:** 2026-10-23
**Depends on:** ADR-0001 (license), ADR-0002 (renderer and HDR), ADR-0008 (client model)
**Relates to:** `COMPOSITOR.md` §10/§20, `ROADMAP.md` §3H/§9/§10, `TODO.md` Phase 8

## Context

The effects set is P1 daily-driver polish: rounded corners, borders/focus rings,
shadows, dim-inactive and per-window opacity, backdrop blur, per-rule overrides,
a global kill switch, and a low-power mode. The target is mango/scenefx quality
on Smithay's **GLES** renderer (ADR-0002).

The options were:

1. implement the effect passes in crab on top of Smithay's renderer;
2. adopt a scenefx-like crate;
3. omit effects.

Relevant facts:

- **scenefx is a wlroots/C scene-graph extension** (MIT). It assumes wlroots
  renderers and a wlroots scene graph. crab is on Smithay and has its own ordered
  element list (ADR-0008), so scenefx is not a drop-in and would add the
  C/wlroots renderer dependency that `ROADMAP.md` §2 explicitly rejects.
- No maintained, general-purpose Smithay effects crate is known at the time of
  writing. niri, mango, and scenefx are **references**, not dependencies.
- crab is GPL-3.0-or-later (ADR-0001), so adapting niri/mango code is permitted;
  scenefx is MIT, so its ideas and shader maths can be ported with attribution.
- Effects interact with two existing guarantees: the **damage tracker** and
  **direct scanout**. A blur samples content *behind* a window, shadows and
  borders draw outside it, and any effect that modifies the scanned-out client
  makes direct scanout invalid.
- HDR is post-0.1 (ADR-0002). The M8 effects work targets the current SDR/GLES
  path and must leave the colour-pipeline seam intact for the later linear-F16
  path.

## Decision

### 1. Implement effects in crab on Smithay GLES

Do **not** take a scenefx/wlroots renderer dependency and do **not** introduce a
second effect scene graph. Effects are passes and decorated elements over the
ordered scene from ADR-0008.

- Smithay-facing effect code lives under `src/compositor/effects/`.
- Pure parameters, rule resolution, and geometry/radius maths live in the pure
  modules (`src/core`, `src/config`) where they can be tested without a renderer.
- Shaders live in `resources/shaders/` and are embedded at build time, as the
  existing README there specifies.
- Adapted shader maths or algorithms carry attribution per the project
  convention.

### 2. Pipeline model

The scene remains one bottom-to-top list. Each window contributes an ordered
group, and effects extend that group rather than reordering it:

- **Shadow** behind the surface (cached analytic-SDF or pre-blurred texture,
  keyed by size and radius).
- **Surface** with a rounded-corner SDF mask that clips the client texture,
  border, and shadow consistently.
- **Border / focus ring** drawn as a rounded-rect quad around the surface,
  with animated colour (gradients later).
- **Opacity / dim-inactive** applied in the same pass.
- **Backdrop blur** for windows that request it: render the content below the
  window's region into an offscreen target, run a dual-Kawase
  downsample/upsample chain, and composite the result behind the window.

Custom-shader open/close/resize transitions are a later polish item; the
animation system already produces the geometry they would consume.

Effects are **per-window and per-rule**, not a global post-process. A window
with no applicable effect takes the undecorated path.

### 3. Correctness rules

- **One logical→physical conversion.** Corner radii, border widths, shadow
  extents, and blur radii are configured in logical units and converted to
  physical pixels at the single existing conversion site, using the output's
  scale and transform. No effect rounds independently.
- **Premultiplied alpha** throughout; effects must not introduce fringes at the
  window edge or at fractional scale.
- **Rounded corners clip everything they decorate** — client content, border,
  and shadow — so nothing bleeds past the mask.
- Effects operate in the renderer's current SDR working space. Blur is not
  claimed to be linear-light until the HDR/linear-F16 path lands; effects must
  take the working space from the renderer rather than hard-coding sRGB, so
  that ADR-0002 Phase 2 does not become a rewrite.

### 4. Damage and direct scanout

- Effects **expand** the damage rectangle by their shadow/blur extent.
- The backdrop cache is per output and invalidated by: damage in the backdrop
  region, window move/resize, space/view or layout change, animation frames,
  and output changes. It is never sampled across an output boundary.
- **Direct scanout is allowed only when the scanned-out client and everything
  composited over or under it are unmodified.** If any effect applies to the
  scanned window (corner mask, border, shadow, opacity, or blur), scanout is
  disabled for that frame. When effects apply only to windows that are not part
  of the scanout decision, scanout remains available.
- A fullscreen window with effects disabled/unmodified can still scan out; a
  translucent or blurred fullscreen window cannot.

### 5. Controls and failure behaviour

- Cargo feature `effects` gates the implementation.
- Runtime kill switch: `[effects] enabled = false` in TOML (ADR-0003) disables
  all effect passes.
- Per-window rule overrides: `opacity`, `blur`, `shadow`, `corner_radius`
  (names finalised with the config surface). Rules can also force an effect off
  for one window only.
- Low-power mode disables blur and shadows (and may reduce shadow radius).
- **Fail safe:** a shader compile/link failure logs the error, disables that
  effect, and surfaces the normal on-screen config-error path. Radii, passes,
  and cache sizes are clamped. Effects never crash the compositor and never
  make a window unrenderable.

### 6. Performance gate

The M8 exit gate is `COMPOSITOR.md` §20:

- 4 windows, no effects, 1440p/144: **< 2 ms CPU**, no missed vblanks.
- Blur: **< 6 ms GPU** at 1080p60 on a mid-range integrated GPU.
- Effects default-on in the polished config; regressions block merges once
  benchmarks exist.

Measure each pass, cache aggressively, and skip work when nothing requests it.
Effects remain behind the feature flag until the M8 gate passes.

### 7. Build order

1. Rounded corners (with border/shadow clipping).
2. Borders / focus ring and animated colour.
3. Shadows; dim-inactive and per-window opacity.
4. Blur: dual-Kawase, backdrop cache, damage and scanout handling.
5. `ext-background-effect-v1` (P2, only when negotiated) and low-power polish.
6. Custom-shader transitions, cursor shake-to-find, debug HUD.

This order follows `COMPOSITOR.md` §10: the cheap clips and colour work first,
blur last because it is the expensive, damage-sensitive pass.

## Consequences

- No new renderer dependency and no wlroots coupling; crab keeps full control of
  ordering, colour, and damage.
- All shader, pass, cache, and fallback work is ours. This is the main cost of
  the decision.
- Effects force explicit tests for damage expansion, cache invalidation, and
  direct-scanout eligibility. Those tests are part of M8, not optional polish.
- The config and window-rule surface grows; docs and `--validate` must cover the
  new keys, and per-rule overrides must remain additive.
- Embedded GLSL must be maintained and compiled by the build; a broken shader
  must degrade to "effect off", never to a crash.
- The HDR work inherits a seam (working space passed in) instead of a rewrite.
- Adapted ideas/code from niri, mango, and scenefx carry attribution.

## Alternatives considered

- **Adopt scenefx (or a scenefx-like crate).** Rejected: scenefx is a wlroots/C
  scene-graph extension, not a Smithay component; adopting it would add a
  wlroots renderer dependency and a second scene model, contradicting ADR-0008
  and `ROADMAP.md` §2.
- **Wait for a Smithay effects crate.** Rejected for M8: none is mature enough
  to plan around, and the integration (ordering, damage, scanout, config) would
  still be crab-specific. Revisit if a maintained crate appears.
- **Client-side effects / protocol-only (`ext-background-effect-v1`).** Rejected
  as the whole answer: client-requested blur is P2, optional, and cannot provide
  compositor-wide corners, shadows, dim, or a consistent look. Implement it in
  addition to compositor effects, not instead of them.
- **No effects.** Rejected: the effect set is P1 daily-driver polish and a
  stated project goal. The runtime kill switch covers users who do not want
  them.
