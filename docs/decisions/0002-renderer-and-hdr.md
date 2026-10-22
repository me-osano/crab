# ADR-0002 — Renderer (GLES) and HDR strategy

**Status:** Accepted
**Date:** 2026-10-16
**Depends on:** ADR-0001 (license)

## Context

`COMPOSITOR.md` §4 assumes Smithay's **GLES** renderer. mango implements HDR/colour
management only on a separate **Vulkan** branch, which raised the question: does
`crab` need Vulkan to support HDR?

Verified state of the stack (2026-10-16):

- **Protocols:** `wp_color_management_v1` landed in wayland-protocols **staging** on
  2025-02-13 and is still staging. `wp_color_representation_v1` (needed for
  Wine/Proton **scRGB** and BT.2020 passthrough) is the newer companion.
- **Kernel/DRM:** the **DRM Color Pipeline API** merged in Nov 2025 and shipped in
  **Linux 6.19-rc1** (1D LUTs, 17³ 3D LUT, matrices). Young, and only starting to be
  used by userspace.
- **Smithay:** colour management/HDR is **not merged**. Work lives in open PRs
  (Smithay/smithay#1143, #982); maintainers are not currently accepting external
  colour-management contributions. Any HDR work today means **vendoring a patched
  Smithay git revision**.
- **Every working HDR implementation on Smithay runs on GLES** — the validated
  `franzjeger/cosmic-hdr-reference` (NVIDIA RTX 5090 + AMD, 2026) and the niri HDR
  forks (`dividebysandwich`, `losnoco`) are all GLES/glow. Hyprland also does HDR on
  GLES (it *requires* GLES 3.0 as of 0.50 and dropped its legacy renderer).
- **niri upstream has no HDR** and no timeline; its maintainer's stated plan is to
  wait for cosmic/Smithay.
- mango's Vulkan requirement is an artifact of adding HDR to its scenefx/wlroots
  branch — a **mango-specific** constraint, not a property of Smithay or of HDR.

GLES 3.x is sufficient for HDR: FP16 (`RGBA16F`) render targets, 10-bit GBM scanout
(`XRGB2101010`/`ARGB2101010`), and custom shaders for the SDR→PQ/BT.2020 transform
and tone mapping.

## Decision

1. **Renderer: stay on Smithay's GLES renderer.** Do not adopt Vulkan for HDR.
2. **HDR is a post-0.1 feature**, delivered in phases, and must not block a usable
   `v0.1.0`. It depends on unmerged Smithay work; when implemented, **vendor a
   patched Smithay git revision**.
3. Ship two cheap prerequisites **early**: **10-bit SDR scanout** (`RGB10A2`,
   `max_bpc=10`) with a **per-format** 10-bit probe, and a clean renderer
   **colour-pipeline seam**.

### HDR phases (post-0.1)

- **Phase 1 — passthrough:** fullscreen/borderless HDR clients (games, video) get
  PQ/BT.2020 scanout plus `HDR_OUTPUT_METADATA`; the SDR desktop is tone-mapped
  around them. Implement `wp_color_management_v1` + `wp_color_representation_v1`
  behind a feature flag.
- **Phase 2 — desktop HDR:** composite all surfaces into a **linear F16**
  intermediate, apply per-surface transforms + tone mapping, encode only at the
  output boundary; teach effects (blur/shadows) the working space.
- **Phase 3 — colour management:** ICC / wide-gamut / per-output profiles via the
  DRM Color Pipeline API (1D/3D LUTs, matrices) once it is broadly available.

## Consequences

- No investment in an immature Vulkan path; we stay on the renderer the whole
  Smithay/niri/cosmic ecosystem uses.
- The renderer interface must expose a colour-pipeline abstraction so layout,
  animation, and effects never touch GLES specifics — this is required anyway
  (ROADMAP §5).
- HDR will initially look like the cosmic reference: the **cheap** path (8-bit
  intermediate + final PQ conversion) for Phase 1, moving to F16 linear compositing
  in Phase 2. We explicitly do **not** claim "correct" colour until Phase 2/3.
- Practical notes carried from upstream testing: probe 10-bit DRM formats
  **individually** (e.g. `XR30`/`XB30` can fail framebuffer binding where
  `AR30`/`AB30` succeed); NVIDIA deep colour may need
  `nvidia-modeset.hdmi_deepcolor=1`; direct/overlay scanout must be handled
  correctly while an HDR post-process is active; and the screenshot/screencast path
  needs an HDR→SDR conversion or captures look washed out.
- Recorded as the resolution of the "Renderer choice blocks HDR" risk in
  `ROADMAP.md` §10.
