# crab documentation

Index of the project's written material. Start with the roadmap.

| Document | What it is |
|---|---|
| [`../ROADMAP.md`](../ROADMAP.md) | **What to build and why.** Feature matrix with provenance, protocol matrix, milestones, decisions. |
| [`../TODO.md`](../TODO.md) | **How to build it.** Granular chronological task list (M0–M11 + HDR). |
| [`../COMPOSITOR.md`](../COMPOSITOR.md) | Design detail: data model, layout system, render pipeline, protocols. |
| [`CONFIG.md`](./CONFIG.md) | **Configuration reference.** File locations, all sections, defaults, bind/action syntax, rules, hot reload, and status of applied/pending features. |
| [`IPC.md`](./IPC.md) | **IPC protocol and bar integration.** Socket, requests/responses/events, actions, bar author guidance with examples, and tips. |
| [`CLI.md`](./CLI.md) | **crab-cli usage.** Commands for querying state, dispatching actions, watching events, and config validation/reload. |
| [`decisions/README.md`](./decisions/README.md) | Architecture Decision Records (ADRs). |
| [`reference/anvil.md`](./reference/anvil.md) | Smithay `anvil` v0.7.0 walkthrough: patterns to borrow, and what crab deliberately does differently. |

## Architecture at a glance

crab is a single-process Wayland compositor built on [Smithay]. Logic that can
be pure **is** pure: the data model, config, layout, and animation engines all
live as Smithay-free modules inside the compositor's **library target**, and only
the IPC types stay a crate (shared with the CLI and with third-party tools):

```
        ┌──────────────────────── crab (package) ──────────────────────────┐
        │  bin: src/main.rs — Smithay glue, backends, handlers, render, input│
        │       (the only place allowed to touch Smithay/calloop/…)         │
        │                                                                   │
        │  lib: src/lib.rs — pure modules only, source-scanned in CI:       │
        │    src/config     → core, serde, toml, thiserror                  │
        │    src/layout     → core                                          │
        │    src/animation  → core                                          │
        │    src/core       → (ids, geometry, colors, spaces, window model) │
        └──────────────────────────────┬─────────────────────────────────────┘
                                       │ depends on
                                       ▼
                                ┌──────────────┐
                                │  crab-ipc   │ ◄──── crab-cli (CLI)
                                │    (leaf)    │
                                └──────────────┘
```

**Boundaries (enforced by `scripts/check-boundaries.sh`):**

- The `src/{core,config,layout,animation}` modules have **no** Smithay or Wayland
  dependencies — enforced by a source scan, since cargo metadata cannot isolate
  modules of one package — so they compile and test in milliseconds.
- Those pure modules depend **downward only** (`core` on nothing;
  `config`/`layout`/`animation` on `core`), and `src/lib.rs` exposes them and
  nothing else, so the reusable surface can never pull in graphics code.
- `crab-ipc` is a **leaf**: no other `crab-*` dependency, so third-party tools can
  depend on it alone.
- Only the `crab` binary (`src/main.rs`) touches Smithay.

[Smithay]: https://github.com/Smithay/smithay
