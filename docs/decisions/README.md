# Architecture Decision Records — crawm

Short, immutable records of decisions that shape `crawm`. Format: **context →
decision → consequences**. Once accepted, an ADR is not edited; it is superseded by
a new ADR.

| ADR | Title | Status | Date |
|---|---|---|---|
| [0001](0001-license.md) | License: GPL-3.0-or-later | Accepted | 2026-10-16 |
| [0002](0002-renderer-and-hdr.md) | Renderer (GLES) and HDR strategy | Accepted | 2026-10-16 |
| [0003](0003-config-format.md) | Configuration format: TOML | Accepted | 2026-10-16 |
| [0004](0004-space-model.md) | Space model: numbered spaces + layout families | Accepted | 2026-10-17 |

**Open / next**

- 0005 — IPC schema (niri-style vs fresh; `ext-workspace` as primary bar interface)
- 0006 — XWayland (`xwayland-satellite` vs in-process)
- 0007 — Effects (own GLES implementation vs a scenefx-like crate)
- 0008 — Client model (custom element list vs Smithay `Space`)

Note: the "ADR-0005" cited by `ROADMAP.md` §5 and `src/lib.rs` for the *modules
first* rule is `crawl-desktop`'s `docs/decisions/0005-crate-modularization.md` —
not this project's 0005 (IPC schema).

See `../../ROADMAP.md` §10 for the surrounding discussion.
