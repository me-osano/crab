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
be pure **is** pure: the layout and animation engines live as Smithay-free
modules inside the compositor, and shared data/config types live in their own
crates:

```
        ┌──────────────────────── crawm (binary) ───────────────────────────┐
        │  Smithay glue · backends · handlers · render · input · ipc        │
        │  (the only crate allowed to touch Smithay/calloop/…)              │
        │                                                                   │
        │  pure modules — Smithay-free by rule, source-scanned in CI:       │
        │  ┌──────────────────┐  → crawm-core                               │
        │  │   src/layout     │                                             │
        │  ├──────────────────┤  → crawm-core                               │
        │  │  src/animation   │                                             │
        │  └──────────────────┘                                             │
        └──────────────────────────────┬─────────────────────────────────────┘
                                       │ depends on
                      ┌────────────────┼──────────────────┐
                      ▼                ▼                  ▼
               ┌──────────────┐ ◄── ┌───┴───────────┐ ┌───┴───────────┐
               │  crawm-core  │     │ crawm-config  │ │   crawm-ipc   │ ◄── crawm-msg
               │  (pure data) │     │    (pure)     │ │    (leaf)     │      (CLI)
               └──────────────┘     └───────────────┘ └───────────────┘
```

**Boundaries (enforced by `scripts/check-boundaries.sh`):**

- `crawm-core` and `crawm-config` have **no** Smithay or Wayland dependencies,
  and the `src/layout` / `src/animation` modules are Smithay/Wayland-free by
  rule (cargo-metadata check for the crates, source scan for the modules) —
  they compile and test in milliseconds.
- `crawm-ipc` is a **leaf**: no other `crawm-*` dependency, so third-party tools can
  depend on it alone.
- Only the `crawm` binary touches Smithay.

[Smithay]: https://github.com/Smithay/smithay
