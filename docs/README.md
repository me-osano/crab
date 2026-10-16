# crawm documentation

Index of the project's written material. Start with the roadmap.

| Document | What it is |
|---|---|
| [`../ROADMAP.md`](../ROADMAP.md) | **What to build and why.** Feature matrix with provenance, protocol matrix, milestones, decisions. |
| [`../TODO.md`](../TODO.md) | **How to build it.** Granular chronological task list (M0–M11 + HDR). |
| [`../COMPOSITOR.md`](../COMPOSITOR.md) | Design detail: data model, layout system, render pipeline, protocols. |
| [`decisions/README.md`](./decisions/README.md) | Architecture Decision Records (ADRs). |

## Architecture at a glance

crawm is a single-process Wayland compositor built on [Smithay]. Logic that can
be pure **is** pure: the data model, config, layout, and animation engines all
live as Smithay-free modules inside the compositor, and only the IPC types stay
a crate (shared with the CLI and with third-party tools):

```
        ┌──────────────────────── crawm (binary) ───────────────────────────┐
        │  Smithay glue · backends · handlers · render · input · ipc        │
        │  (the only crate allowed to touch Smithay/calloop/…)              │
        │                                                                   │
        │  pure modules — Smithay-free by rule, source-scanned in CI:       │
        │    src/config     → core, serde, toml, thiserror                  │
        │    src/layout     → core                                          │
        │    src/animation  → core                                          │
        │    src/core       → (ids, geometry, colors, tags, window model)   │
        └──────────────────────────────┬─────────────────────────────────────┘
                                       │ depends on
                                       ▼
                                ┌──────────────┐
                                │  crawm-ipc   │ ◄──── crawm-cli (CLI)
                                │    (leaf)    │
                                └──────────────┘
```

**Boundaries (enforced by `scripts/check-boundaries.sh`):**

- The `src/{core,config,layout,animation}` modules have **no** Smithay or Wayland
  dependencies — enforced by a source scan, since cargo metadata cannot isolate
  modules of one package — so they compile and test in milliseconds.
- `crawm-ipc` is a **leaf**: no other `crawm-*` dependency, so third-party tools can
  depend on it alone.
- Only the `crawm` binary touches Smithay.

[Smithay]: https://github.com/Smithay/smithay
