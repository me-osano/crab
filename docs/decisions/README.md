# Architecture Decision Records — crab

Short, immutable records of decisions that shape `crab`. Format: **context →
decision → consequences**. Once accepted, an ADR is not edited; it is superseded by
a new ADR.

| ADR | Title | Status | Date |
|---|---|---|---|
| [0001](0001-license.md) | License: GPL-3.0-or-later | Accepted | 2026-10-16 |
| [0002](0002-renderer-and-hdr.md) | Renderer (GLES) and HDR strategy | Accepted | 2026-10-16 |
| [0003](0003-config-format.md) | Configuration format: TOML | Accepted | 2026-10-16 |
| [0004](0004-space-model.md) | Space model: numbered spaces + layout families | Accepted | 2026-10-17 |
| [0005](0005-ipc-schema.md) | IPC schema: fresh, niri-shaped; `ext-workspace-v1` is the bar interface | Accepted | 2026-10-23 |
| [0006](0006-xwayland.md) | XWayland: `xwayland-satellite` with on-demand activation | Accepted | 2026-10-23 |
| [0007](0007-effects.md) | Effects: own GLES passes over the custom scene | Accepted | 2026-10-23 |
| [0008](0008-client-model.md) | Client model: custom ordered element list | Accepted | 2026-10-23 |

No decisions are currently open.

Note: the "ADR-0005" cited by `ROADMAP.md` §5 and `src/lib.rs` for the *modules
first* rule is `crawl-desktop`'s `docs/decisions/0005-crate-modularization.md` —
not this project's 0005 (IPC schema).

See `../../ROADMAP.md` §10 for the surrounding discussion.
