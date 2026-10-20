# crawm

> A Wayland compositor + window manager in Rust on [Smithay][smithay], merging the
> best of **mango** (tags, many layouts, effects), **niri** (scrollable tiling,
> rigorous animation, typed IPC), and **cosmic** (mixed tiling/floating, groups,
> protocol breadth).

**Status: pre-alpha scaffold.** The workspace builds and `crawm` can open a nested
winit window; it does not yet run a real session.

## Documentation

| Doc | Purpose |
|---|---|
| [`ROADMAP.md`](./ROADMAP.md) | What to build, in what order, and where each feature comes from. |
| [`TODO.md`](./TODO.md) | Granular, chronological task list (M0–M11 + HDR). |
| [`COMPOSITOR.md`](./COMPOSITOR.md) | Design detail (data model, layouts, render, protocols). |
| [`docs/decisions/`](./docs/decisions/README.md) | Architecture Decision Records. |

## Workspace layout

```
src/              the compositor: Smithay glue, backends, handlers, render, input
  lib.rs          library surface: the pure modules below only (reusable, testable)
  core/           pure data: ids, geometry, colors, tags, window model (no Smithay)
  config/         TOML parsing, validation, defaults (no Smithay)
  layout/         pure layout engine: Layout trait + implementations (no Smithay)
  animation/      pure springs/easing curves + animated scalars (no Smithay)
tests/            headless integration tests (drive the library directly)
crawm-ipc/        IPC types + client (leaf; no other crawm-* deps)
crawm-cli/        CLI client (`crawm-cli`)
resources/        default config, session units, shaders, portals
docs/             ADRs and (later) the manual
```

The root `Cargo.toml` is both the `crawm` package and the workspace root
(niri-style), so plain `cargo run` starts the compositor. The package is
lib + bin: `src/lib.rs` exposes only the pure modules, and `src/main.rs` is the
thin Smithay entry point — so `tests/` can unit-drive layout/config/animation
without a GPU.

## Building

```sh
cargo build --workspace
cargo run -- --backend winit     # nested window on an existing session
cargo run -- --validate          # parse + validate the config, then exit
```

Configuration is TOML; the shipped example is
[`resources/default-config.toml`](resources/default-config.toml). Copy it to
`~/.config/crawm/config.toml` (or pass `--config <path>`). `[layout]` and
`[animations] enabled` apply today; the remaining mango-style sections parse and are
wired as their milestones land (`COMPOSITOR.md` §13).

Backends are cargo features: `backend-winit` is the default (nested development);
`--backend tty` needs `--features backend-tty` (bare-metal DRM/KMS, M4). The
default build skips the DRM/session/input stack; `cargo clippy --all-features`
builds the whole thing.

## License

GPL-3.0-or-later. See [`LICENSE`](./LICENSE) and [`NOTICE`](./NOTICE); code adapted
from niri / cosmic / mango is attributed per file (ADR-0001).

[smithay]: https://github.com/Smithay/smithay
