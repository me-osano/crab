# ADR-0001 — License: GPL-3.0-or-later

**Status:** Accepted
**Date:** 2026-10-16

## Context

`crawm` reuses ideas — and wants to reuse *code* — from three compositors:

| Upstream | License |
|---|---|
| niri | GPL-3.0-or-later |
| cosmic-comp | GPL-3.0-only |
| mango (and its dwl base) | GPL-3.0-or-later (dwm / wlroots / sway / scenefx are MIT) |
| Smithay | MIT |

The roadmap deliberately borrows proven implementations (niri's scrollable tiling
and animation clock, mango's tag/layout logic, the cosmic+Smithay HDR reference).
Copying GPL-3.0 code into a permissively licensed project is not permitted.

`crawl-desktop` (the consuming shell) and `crawl` v1 are MIT, which raised the
question of whether copyleft here would be a problem.

## Decision

`crawm` is licensed **GPL-3.0-or-later**.

## Consequences

- Code from niri, cosmic-comp, and mango may be **copied and adapted with
  attribution**, instead of reimplemented from documentation. This is the single
  biggest developer-speed win available.
- Copyleft does **not** propagate into `crawl-desktop`/`crawl` (MIT): `crawm` is
  a **separate process**, communicating with the shell over Wayland protocols and a
  local socket. There is no linking, so the MIT crates stay MIT.
- Contributors must not paste GPL-incompatible code into the tree; reviewing
  provenance becomes part of code review.
- Any file adapted from an upstream must record its origin (file header comment or
  `NOTICE`), as required by GPL-3.0.

## Alternatives considered

- **MIT / Apache-2.0 / MPL-2.0** — rejected: would force clean-room reimplementation
  of exactly the hard parts (tiling math, animation, HDR), for no practical benefit
  given the process boundary above.
- **Permissive core + GPL binary** — viable (GPL the `crawm`/renderer crates, MIT
  the pure `-core`/`-layout`/`-anim`/`-config` crates) if permissive reusable
  libraries become desirable later. Not chosen now, to keep the tree uniform; can be
  revisited without breaking anything.
