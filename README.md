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
crates/
  crawm-core/     pure data: ids, geometry, colors, tags, window model (no Smithay)
  crawm-layout/   Layout trait + implementations (pure)
  crawm-anim/     spring/easing curves + animation clock (pure)
  crawm-config/   TOML parsing, validation, defaults
  crawm-ipc/      IPC types + client (leaf; no other crawm-* deps)
  crawm/          the binary: Smithay glue, backends, handlers, render, input
  crawm-msg/      CLI client (`crawm-msg`)
resources/        default config, session units, shaders, portals
docs/             ADRs and (later) the manual
```

## Building

```sh
cargo build --workspace
cargo run -p crawm -- --backend winit     # nested window on an existing session
cargo run -p crawm -- --validate          # parse + validate the config, then exit
```

## License

GPL-3.0-or-later. See [`LICENSE`](./LICENSE) and [`NOTICE`](./NOTICE); code adapted
from niri / cosmic / mango is attributed per file (ADR-0001).

[smithay]: https://github.com/Smithay/smithay
