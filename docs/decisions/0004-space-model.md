# ADR-0004 — Space model: unified numbered spaces, with layout families

**Status:** Accepted
**Date:** 2026-10-17
**Depends on:** ADR-0002 (renderer), ADR-0003 (config)
**Relates to:** `COMPOSITOR.md` §6–7, `ROADMAP.md` §3A–3B

## Context

`COMPOSITOR.md` §6 and `ROADMAP.md` §3A propose three different mental models for
"where windows live", each borrowed from a different upstream:

- **mango / dwm tags** — each output owns a fixed set of numbered **tags**; a window
  may be on **several** tags; an output may **show several** tags at once; each tag
  remembers its own layout (`Pertag`).
- **niri named workspaces** — workspaces are **dynamic** and **named**; there is one
  visible workspace per output by default; `on-overflow` decides where new ones go.
- **cosmic `WorkspaceMode`** — workspaces are either output-bound or global, with a
  vertical/horizontal axis and wraparound.

The M2 core was built on the mango/dwm tag model (a 32-bit `TagMask` per output,
multi-view, multi-tag windows, per-tag layout state). Before committing M2 we must
fix the user-facing name and the growth story for layouts, because both are load
bearing for config (`COMPOSITOR.md` §13), IPC (`§14`), and the M7 layout set
(`ROADMAP.md` §3B).

Separately, `ROADMAP.md` §3B lists **14 flat layouts** (`Tile`, `Monocle`, `Deck`,
`Grid`, `Scroller`, `Dwindle`, `Fair`, `Vertical*`, …). As a flat enum with a single
`cycle-layout` action this does not scale: users cannot express "cycle within tiling"
or "switch family" and the enum becomes an undifferentiated list.

## Decision

### 1. One core concept: the **space**

The core model has exactly one spatial concept, the **space**:

- A space is a **numbered slot** per output (`1..=32`), held in a `SpaceSet`
  (a bitset, renamed from `TagMask`).
- A window belongs to a **set** of spaces (`Window.spaces: SpaceSet`); an output
  shows a **set** of spaces (`SpaceView { visible, previous }`).
- When several spaces are visible, the layout of the **lowest-numbered visible
  space** arranges the combined window set (the dwm rule), unchanged from
  `COMPOSITOR.md` §6.
- Per-space state (`SpaceState { layout, params, scroll, focus_history }`) is
  remembered for the life of the output (`Pertag`).

This is the mango/dwm model, kept because it is the only one of the three that can
express **multi-view** and **multi-space windows** — the two features `ROADMAP.md`
§3A marks P0. It is *named* "space" rather than "tag" so the same word covers the
model the docs already used ("one space", "sticks to all spaces") and leaves
"workspace" for the protocol surface.

### 2. "workspace" is a **projection**, not a core concept

Dynamic, named workspaces are not modeled in `core`. They are produced at the
edges:

- **Naming** a fixed slot (M3) is a label on a numbered space.
- **Persistence** across output changes (M3) is state on the space, not a new type.
- `ext-workspace-v1` (M6) and `Capability::Workspaces` in IPC project spaces onto
  the protocol's notion of "workspace"; a client never learns about `SpaceSet`.
- `WorkspaceMode`-style output-bound/global switches (M7) are space *flags*.

`ROADMAP.md` §3A's "named spaces / persistent workspaces (M3)" is therefore a
naming + persistence layer over numbered spaces, not a dynamic workspace manager.

### 3. Layouts are grouped into **families** with per-family **modes**

A space's layout is a **mode inside a family**. M2 ships:

| Family | Modes (M2) | Later modes |
|---|---|---|
| `Stack` | `floating`, `monocle` | `tabbed` (M7) |
| `Tile` | `tile` (master/stack) | `grid`, `deck`, `dwindle`, `fair`, `vertical-*` (M7) |
| `Scroll` (M3) | `scroll` (niri) | `vertical-scroll` |

Two cycle actions replace the single `cycle-layout`:

- `cycle-layout-mode` — next mode **within** the current family;
- `cycle-layout-family` — the next **family**, landing on its default mode.

Config and IPC accept a mode name; the family name is an accepted alias for that
family's default mode (so `default_layout = "tile"` and `default_layout = "stack"`
both work, and `free` is an alias for `floating`).

## Consequences

- **Terminology is unified.** `TagMask`/`TagView`/`TagState` become
  `SpaceSet`/`SpaceView`/`SpaceState`; actions become `view-space`,
  `toggle-space-view`, `move-to-space`, `toggle-space`. "Workspace" appears only at
  the protocol/IPC surface (`ext-workspace-v1`).
- **A bitset is a deliberate limit.** `SpaceSet` cannot express *unbounded*
  dynamic workspaces. This is accepted: 32 slots per output is far beyond any
  tiling workflow (mango uses 9), multi-view/multi-space windows are worth more
  than dynamic growth, and the M3 naming + M6 `ext-workspace-v1` projection covers
  the observable behaviour bars expect. If dynamic creation is ever needed, a
  follow-up ADR supersedes this one; it would be a pure-layer change
  (`SpaceSet` → `Vec`/`SlotMap` plus a `Window.space` singular).
- **The layout enum stays small per family.** M7 adds modes to a family instead of
  growing one flat list of 14; the cycle actions remain meaningful, and per-space
  memory restores the exact mode. Promoting modes to per-family enums later is a
  pure-layer change covered by the layout test suite.
- **The `floating` layout now floats.** Previously a space whose layout was
  `floating` produced no placements for its *tiled* windows, which stayed
  invisible. In the family model `floating` is the `Stack` family's free mode, so
  the compositor cascades every window on such a space (`compositor/layout.rs`).
- **Names are versionless and additive.** Config/IPC names (`floating`, `monocle`,
  `tile`, `stack`, `scroll`, …) are stable; new modes are added, never renamed.
- `COMPOSITOR.md` §6–7 and `ROADMAP.md` §3A–3B are updated to this vocabulary.

## Update (2026-10-20): dynamic numbering

The user asked for spaces to behave like niri workspaces: no per-space layout
configuration, and dynamic numbering that never leaves an empty space before the
focused one. The baseline above still holds — 32 slots per output and
multi-space windows remain possible — but the **numbering** is now dynamic:
`Output::compact` removes spaces that are now empty and followed by occupied
ones, renumbers the survivors densely from 1, moves each space's `SpaceState`
with it, and renumbers window membership through `SpaceRemap`. The view follows
its content: removing the focused space hands the view to the space that moved
into its slot. A newly created space starts from the `[layout]` defaults. An
`ext-workspace-v1` projection can expose this numbering as dynamic workspaces
without changing the pure model, and the `Vec`/`SlotMap` change rejected below
remains a follow-up only if more than 32 spaces are ever needed.

## Alternatives considered

- **Pure mango tags (keep the name).** Rejected: "tag" reads as metadata, not a
  place, and collides with the protocol's window "tags"; the project already
  spoke of "spaces" everywhere else.
- **Pure niri named workspaces (dynamic).** Rejected as the *core*: it cannot
  express multi-view or multi-space windows, and it forces per-output workspace
  lists into the hot path. Kept as the M6 projection instead.
- **A flat layout enum with a `cycle` over all modes.** Rejected: it cannot
  express "cycle within tiling" vs "switch family", and the M7 set turns it into
  an undifferentiated 14-variant list. Families were chosen instead.
- **Named `Space` objects with stable IDs (a `SlotMap`).** More flexible than a
  bitset but adds a lookup to every visibility check and multi-space membership
  becomes a `Vec` per window. Deferred as the superseding option above.
