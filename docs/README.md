# crawm documentation

Index of the project's written material. Start with the roadmap.

| Document | What it is |
|---|---|
| [`../ROADMAP.md`](../ROADMAP.md) | **What to build and why.** Feature matrix with provenance, protocol matrix, milestones, decisions. |
| [`../TODO.md`](../TODO.md) | **How to build it.** Granular chronological task list (M0–M11 + HDR). |
| [`../COMPOSITOR.md`](../COMPOSITOR.md) | Design detail: data model, layout system, render pipeline, protocols. |
| [`decisions/README.md`](./decisions/README.md) | Architecture Decision Records (ADRs). |

## Architecture at a glance

crawm is a single-process Wayland compositor built on [Smithay]. The workspace is
split so that **all logic that can be pure, is pure**:

```
        ┌──────────────┐   ┌───────────────┐   ┌──────────────┐
        │  crawm-core  │◄──│ crawm-layout  │   │  crawm-anim  │
        │ (ids, geom,  │◄──│  (Layout +    │◄──│ (springs,    │
        │  tags, model)│   │   placements) │   │  clock)      │
        └──────┬───────┘   └───────────────┘   └──────────────┘
               │                    ▲
        ┌──────┴───────┐            │
        │ crawm-config │            │
        └──────┬───────┘            │
               │                    │
        ┌──────▼────────────────────┴───────────────┐
        │                 crawm (binary)             │
        │   Smithay glue · backends · handlers ·     │
        │   render · input · ipc server              │
        └──────┬───────────────────────────┬─────────┘
               │                           │
        ┌──────▼───────┐            ┌──────▼───────┐
        │  crawm-ipc   │◄───────────│  crawm-msg   │
        │  (leaf)      │            │  (CLI)       │
        └──────────────┘            └──────────────┘
```

**Boundaries (enforced by `scripts/check-boundaries.sh`):**

- `crawm-core`, `crawm-layout`, `crawm-anim`, `crawm-config` have **no** Smithay or
  Wayland dependencies — they compile and test in milliseconds.
- `crawm-ipc` is a **leaf**: no other `crawm-*` dependency, so third-party tools can
  depend on it alone.
- Only the `crawm` binary touches Smithay.

[Smithay]: https://github.com/Smithay/smithay
