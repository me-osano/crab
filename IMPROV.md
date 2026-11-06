# IMPROV — changes and implementations crab can borrow from niri

> Status snapshot, 2026-10-30.
>
> - crab: `3d63e05` plus the landed C3–C6 multi-output work; 21,620 Rust LOC
>   (20,130 in `src/`, 641 in `tests/`), 298 passing tests, clippy/fmt clean,
>   `scripts/check-boundaries.sh` green.
> - niri: `9d8f4f8` (`v26.x`, 2026-10-26); ~99,400 Rust LOC in the main
>   workspace (86,464 in `src/`, 10,304 in `niri-config`, 2,664 in `niri-ipc`);
>   ~101,500 including `niri-visual-tests`.
> - Both use Smithay `0.7.0`; niri pins the git revision
>   `8a270232f1bfc416c781273c98a3f52a812a0bba`, crab uses crates.io `0.7.0`.
> - Both are GPL-3.0-or-later, so code may be adapted with attribution
>   (ADR-0001, `NOTICE`, `CONTRIBUTING.md`).
>
> This document is ordered by *what to do*, not by *what niri has*. Each
> backlog item has an explicit `niri:` reference and a `Crab insertion point`;
> the comparison tables name both sides. Line numbers are current as of the
> snapshot and will drift; treat them as pointers to the surrounding code, not
> stable anchors.

---

## 1. Current comparison

### 1.1 At a glance

| Metric | crab | niri | Reading |
|---|---|---|---|
| Rust LOC | 21,620 (20,130 in `src/`) | ~99,400 main workspace; ~101,500 with visual tests | crab is ~1/5 the size |
| Compositor `src/` LOC | 20,130 | 86,464 | niri's `layout/` alone is 25,387 |
| Largest module | `src/compositor/state.rs`, 4,400 | `src/layout/scrolling.rs`, 5,752 | crab centralizes; niri specializes |
| Pure layout/config | 5,163 (`core` 3,572 + `layout` 1,591) + 1,776 config + 1,334 animation | 25,387 layout + 10,304 config | niri's layout is ~5× crab's pure core+layout |
| Tests | 298 (169 lib, 96 bin, 23 integration/doctest, 10 crab-ipc) | 247 `#[test]` in `src/` + 6,780 LOC `src/tests` + 2,058 LOC visual tests | crab is denser per line; niri is deeper per feature |
| Working backend | winit nested only | winit + DRM/KMS/libinput | the biggest practical gap |
| Protocol globals | core + xdg-shell/layer-shell/decoration/viewporter/fractional-scale/xdg-output | broad core set + capture, workspaces, session lock, IME | crab is at the “first windows” layer |
| Output model | per-output map, two-output harness, one live output | multi-output, hotplug, multi-GPU | crab's model is built and tested; only one output is live |
| Configure sync | per-window refusal state machine + `WindowFit` | transactions + `ConfigureIntent` | different philosophies |
| IPC | snapshot/diff JSON | typed niri-ipc + buffered event stream + output config + pick | crab is simpler; niri is richer |
| Docs | 4,716 lines of Markdown, excluding this file (1,786 in `docs/`) | extensive but less ADR-centric | crab is unusually explicit |

### 1.2 Capability comparison

Legend: **yes** = working; **partial** = implemented but limited/untested;
**no** = absent; **stub** = parses or plans but does not act.

| Area | crab | niri | Notes |
|---|---|---|---|
| Nested winit backend | yes | yes | crab's only usable path |
| DRM/KMS + libseat + udev | stub (`backend/tty.rs` probes, then bails) | yes | M4-sized work |
| libinput | no (winit events only) | yes | tty depends on it |
| Multi-GPU / multi-renderer | no | yes | niri `GpuManager`/`MultiRenderer` |
| Output hotplug | model only | yes | crab has `core/output_plan.rs` planning |
| Runtime output config | no | yes | niri output-management + IPC |
| Per-output workspaces/spaces | partial | yes | crab's focused-output assumptions are buggy |
| Space/workspace migration on unplug | lossy (`remove_output` dumps windows) | yes, identity-preserving | see B05 |
| Scrollable tiling | yes, pure | yes | crab's is much smaller |
| Stack/tile layouts | yes | no (scroll + floating + fair) | crab is richer here by design |
| Tags / multi-space view | yes | no | deliberate mango divergence |
| First-class columns + column actions | partial (columns exist, few operations) | yes | see B03 |
| Floating layout | yes, per-space + per-window | yes, separate `FloatingSpace` | see B04 |
| Configure transactions | no | yes, 300 ms blockers | see B01 |
| Configure throttling | partial (skip same size) | yes, `ConfigureIntent` | done: 16 ms, latest size wins (B02) |
| Animations | open/close/placement springs | full layout/scroll/overview animations | crab's core is solid |
| Live gestures | no (end-only, 30 px) | yes (3-finger view/space, 4-finger overview) | see B33 |
| Touch / tablet | no | yes | see B34 |
| Pointer constraints / relative pointer | no | yes | games/remote desktop |
| Cursor shape / themes | no (arrow fallback) | yes | see B17 |
| DMA-BUF | no | yes | all clients use shm today |
| Presentation-time | no | yes | needed for frame pacing |
| Screencopy / image capture | no | yes (+ PipeWire) | see B26 |
| Session lock | no | yes | P1 |
| Idle notify/inhibit | no | yes | P1 |
| Primary selection / data-control | no | yes | clipboard managers |
| Text input / IME | no | yes | CJK input |
| Keyboard shortcuts inhibit | flags parsed only | yes | remote desktop |
| Foreign-toplevel / ext-workspace | no | yes | bars/taskbars |
| XWayland | ADR-0006 only | yes | crab plans xwayland-satellite |
| Security context | no | yes | Flatpak portals |
| Config breadth | 1,776 LOC, many parsed-only fields | 10,304 LOC | niri's is much more complete |
| IPC request breadth | 10 request kinds + 29 actions, snapshot/diff events | ~18 request kinds, rich payloads, event-stream state | different shapes |
| `ext-workspace-v1` bar interface | no | yes | ADR-0005 declares it primary |
| Test harness depth | pure units + property tests | 6.8k integration + 2k visual tests | see B41/B42 |
| ADR discipline | strong (8 ADRs) | informal design docs | crab wins |

### 1.3 Where crab is already equal or better

These are worth keeping while borrowing:

1. **A hard pure-core boundary.** `src/lib.rs` exposes only
   `{core,config,layout,animation}`; `scripts/check-boundaries.sh` enforces it.
   niri's layout is generic over `LayoutElement`, but its config/layout/IPC code
   is not separated this aggressively.
2. **Small, focused IPC crate.** `crab-ipc` is a leaf with serde types and a
   blocking client; niri-ipc is 2.7k LOC. crab's versioned
   `PROTOCOL_VERSION = 2` and forward-compatible `Unknown` event are good.
3. **A custom ordered scene with damage tracking.** ADR-0008's ordered element
   list is simpler than adopting Smithay `Space`, and the damage tracker already
   gives idle-at-zero behavior.
4. **Config as a pure, reusable library.** `crab::config` can be tested without
   Wayland; `crab-cli config-validate` and `--validate` reuse it.
5. **Documented tradeoffs.** `COMPOSITOR.md`, `ROADMAP.md`, `TODO.md`, and
   ADR-0004/0005/0007/0008 state what is deliberately different from niri. Do
   not erase that when porting.
6. **Test density per line.** 293 tests over 20k LOC is high; keep that ratio
   when adding niri-scale features instead of letting integration debt grow.
7. **Layout family breadth.** Stack/tile/fair/floating/scroll and 32-slot
   tag semantics are a deliberate superset of niri's model.

### 1.4 Where niri is ahead

Ordered by practical impact on “can this be a daily driver”:

1. **A real session backend** — DRM/KMS, GBM/EGL, libseat, udev, libinput,
   hotplug, session pause/resume, frame callbacks.
2. **DMA-BUF and presentation-time** — performance and correct frame pacing.
3. **Protocol breadth** — session lock, idle, clipboard, IME, cursor shape,
   constraints, screencopy, output management, foreign-toplevel/workspaces.
4. **Configure transactions** — atomic multi-window layout changes instead of
   stretching committed buffers.
5. **Workspace identity and output migration** — monitor unplug/replug does not
   destroy layout state.
6. **First-class columns and column actions** — move/consume/expel/resize
   columns, presets.
7. **Live gesture handling** — continuous view offset/workspace switch instead
   of a threshold at swipe end.
8. **IPC/bar integration** — typed event state, bounded channels, output
   configuration, `ext-workspace-v1`.
9. **Integration and visual tests** — niri can catch layout regressions that
   crab's pure tests cannot.
10. **Config surface** — 10k LOC of parsing/merging/validation for outputs,
    rules, binds, animations, gestures, and includes.

### 1.5 Architectural deltas that constrain borrowing

| crab choice | niri assumption | Consequence |
|---|---|---|
| 32 fixed slots per output, dynamic dense numbering (ADR-0004) | unbounded `Vec<Workspace>` | Do **not** transplant workspace storage; borrow migration *semantics* and adapt to slots. |
| Pure core forbids Smithay tokens | layout lives beside Smithay and is generic over `LayoutElement` | Borrow algorithms into `src/layout`; keep protocol/backend glue in `src/compositor`. |
| Ordered custom scene (ADR-0008) | niri's own render element tree + render helpers | Borrow effect/animation structure, not render code. |
| Smithay 0.7 crates.io | Smithay 0.7 git revision | Check every API against crab's pinned release; niri may use unreleased fixes. |
| `focused_output` + `fallback_output` (C5, done) | niri separates primary output (fallback) from focused output | Aligned; port multi-output logic directly. |
| IPC is snapshot/diff JSON | typed `EventStreamState` + per-entity deltas | Borrow the buffering/state pattern, keep crab's schema unless ADR-0005 is revised. |

### 1.6 Bottom line

crab is a well-structured, well-tested *model* of a compositor with a usable
nested session; niri is a complete *session*. The highest-leverage borrows, in
order, are:

1. **B09/B10** — KMS/libseat/udev/libinput. Nothing else makes crab a real
   compositor.
2. **B15/B16/B17** — DMA-BUF, presentation-time, cursor shape. These turn the
   existing renderer into a usable desktop.
3. **B01/B02** — transactions and configure throttling. These make resizing and
   scrolling stable instead of merely correct on paper.
4. **B05** — workspace migration. This is the difference between “multi-output”
   and “multi-output that survives a monitor unplug.”
5. **B18/B19/B14/B20** — `ext-workspace`, foreign-toplevel, session lock, idle.
   This is what makes bars, lockers, and idle daemons work.

Everything else is valuable but secondary. **C1–C6 are closed** (2026-10-30);
finish **B43** and the rest of **B41** so those borrows have a testable
foundation.

---

## 2. Ground rules for borrowing

1. **Borrow patterns, not dumps.** Port a state machine, data layout, or
   algorithm; rewrite it against crab's types and Smithay 0.7. A verbatim copy
   of a niri file almost always brings niri's workspace/render assumptions with
   it.
2. **Keep the boundary.** No `smithay`, `wayland`, `calloop`, `winit`,
   `libinput`, `drm`, `x11`, etc. in `src/{core,config,layout,animation}`.
   `scripts/check-boundaries.sh` must pass after every change.
3. **Attribute.** Add the niri file and commit to the file header and
   `NOTICE` per ADR-0001. niri is GPL-3.0-or-later, compatible with crab.
4. **One behavior per commit.** A transaction implementation, a protocol
   global, and a config wiring are three commits. Each needs tests and docs.
5. **Add the test first where possible.** Pure logic gets unit/property tests;
   compositor glue gets a headless/integration test or a documented manual
   reproduction.
6. **Measure hot paths.** For B01/B02/B44–B49, record before/after frame time or
   allocation counts, not just “looks faster.”
7. **Update the ADRs.** If borrowing changes a crab design decision (e.g. slots
   vs workspaces), write a superseding ADR rather than silently diverging from
   ADR-0004.

---

## 3. Prerequisites from the crab review

Fix these before layering more features on top; otherwise new work inherits
broken foundations.

| ID | Item | Evidence | Why first |
|---|---|---|---|
| C1 | Boundary check false positive on the `x11` config key | `src/config/mod.rs:439,656,963–964`; `scripts/check-boundaries.sh` exits 1 | **Fixed 2026-10-27:** the bare `\bx11` needle now matches only `x11rb`/`xcb`/`xwayland`; the check is green |
| C2 | `x11` window rules block the entire rule chain forever | `state.rs:2488` (`x11 = None`), `3653` (`Pending`) | **Fixed 2026-10-27:** protocol is a known `bool`; `pending` remains only for app-id/title/output, with a two-rule regression test |
| C3 | Layer commit loses the output origin in `usable_area` | `state.rs:1722–1727` vs `1617–1622` | **Fixed 2026-10-30:** `sync_layer_zone` returns the area at the output's global origin; covered by `an_exclusive_zone_on_a_secondary_output_keeps_its_global_origin` |
| C4 | Space compaction and rule application are focused-output-scoped | `state.rs:2016,2316,2455,2651–2688` | **Fixed 2026-10-30:** compaction, rules, focus, and activation read the window's output; covered by two harness scenarios |
| C5 | Rename/refactor `primary_output` → `focused_output` | `state.rs:1087–1089`, `2796–2800`, `2843–2847` | **Fixed 2026-10-30:** `focused_output` + `fallback_output`, `normalize_focus` keeps both valid, and `remove_output` migrates to the fallback |
| C6 | Add a headless two-output test harness | no multi-output test exists; winit makes one output | **Fixed 2026-10-30:** `src/compositor/state/multi_output.rs` drives a real `wayland-client`; six scenarios |

**Acceptance for this section:** `check-boundaries.sh`, clippy, fmt, and the
298 tests are green; a two-output test covers close-on-background-output, layer
zone updates, and rule application on a non-focused output.

---

## 4. Borrow backlog

Priority: **P0** = correctness/CI/blocker, **P1** = daily-driver capability,
**P2** = polish, **P3** = later. Effort: S (<1 day), M (1–4 days),
L (1–2 weeks), XL (multi-week).

### 4.1 Compositor core and layout

#### B01 — Transaction-based configure synchronization

**niri:** `src/utils/transaction.rs` (Transaction, TransactionBlocker,
`TIME_LIMIT = 300 ms`), used from `layout/mod.rs:3401`, `layout/monitor.rs:858`,
`layout/scrolling.rs:2109`, etc.

**What niri does:** before a layout change that sizes/moves multiple windows,
create a `Transaction`; configure every affected client; register each commit as
a blocker on its surface; block the pending commit until all clients have
committed the new state or the deadline expires; then unblock and redraw.

**Why crab needs it:** crab currently configures each window independently and
hides the gap with `WindowFit` stretching. A column resize or scroll can show
different windows at different commit generations, and input mapping must undo
the stretch. Transactions eliminate most of that complexity.

**Crab insertion point:**
- New `src/compositor/transaction.rs` (Smithay-side; do not put it in `lib.rs`).
- `layout::arrange` should return the set of windows whose size changed, or
  write placements into a `LayoutOutcome`; the compositor-side caller creates
  one transaction per arrange and attaches blockers in the surface `commit`
  handler.
- Keep the existing `Configure`/`WindowFit` path as a fallback for deadlines and
  clients that refuse size, so this can land incrementally.
- Register a calloop timer for the deadline; on expiry, complete the
  transaction so blockers release and the best committed state is presented.
  niri's transaction does not apply layout on expiry; that ordering is
  crab-specific and must not block other clients.

**Acceptance:** resizing a scroll column or moving a window between columns
never presents a mixed-generation frame; a client that ignores configure does
not stall other windows past 300 ms; a test with two fake clients shows both
commits releasing the frame.

**Status:** landed. `src/compositor/transaction.rs` owns a `TransactionQueue`;
`layout::arrange` returns the windows still owing a commit, `Crab::relayout`
starts a transaction, and `on_commit` releases a window once its `Configure` is
no longer awaiting. `Crab::can_present_frame` gates rendering in the winit
backend, and `advance_frame` expires the 300 ms deadline. The deadline is
checked at the frame boundary — frames keep being requested while a transaction
blocks through `is_animating` — rather than by a calloop timer, so the same rule
works for the future tty backend. A real two-client resize test commits both
buffers and asserts that the frame is held until the second commit.

**Priority:** P1 · **Effort:** L · **Depends on:** C6.

---

#### B02 — `ConfigureIntent` and configure throttling

**niri:** `src/layout/mod.rs:486` —

```rust
pub enum ConfigureIntent {
    NotNeeded,
    Throttled,
    CanSend,
    ShouldSend,
}
```

**What niri does:** layout changes classify what changed. Size-only changes may
be throttled (e.g. during fast resize); non-size changes bypass throttling.
This prevents a configure storm while a client commits slower than the pointer
moves.

**Why crab needs it:** `Configure::ask` already skips asking for the same size
(`state.rs:813–825`), but every different size is sent immediately. During a
continuous resize or scroll, that can queue many configures and cause
ping-ponging with clients that answer slowly.

**Crab insertion point:** extend `Configure` with
`last_sent: Option<Instant>`, `pending: ConfigureIntent`, and a
`MIN_CONFIGURE_INTERVAL` (start with one frame, ~16 ms). In
`ManagedWindow::configure_to`, compute intent from what changed (size vs
position/visibility/state) and coalesce to the latest size. Keep `ShouldSend`
for focus/fullscreen/state changes so they are never delayed.

**Acceptance:** a synthetic resize loop with a 30 ms client produces bounded
configures (e.g. ≤ 2 per 30 ms) and converges to the final size; existing
resize/refusal tests still pass.

**Status:** landed. `Configure` carries `last_sent`/`pending`, `configure_to`
plans a `ConfigureIntent` and coalesces to the latest size, and the backends
flush at the frame boundary. `MIN_CONFIGURE_INTERVAL` is one frame (16 ms); the
resize-loop test asserts at most four sends over 60 ms and convergence to the
final size.

**Priority:** P1 · **Effort:** M · **Depends on:** B01 (or lands first as a
smaller step).

---

#### B03 — First-class columns and column actions

**niri:** `src/layout/scrolling.rs` — `ColumnWidth::{Proportion, Fixed}`,
`WindowHeight::{Auto { weight }, Fixed, Preset}`, `ScrollDirection`, column
move/consume/expel/resize methods; preset logic around `scrolling.rs:4006`.

**What niri does:** columns are first-class containers. Windows move between
columns, columns move in the strip, widths can be proportional/fixed/preset, and
window heights can be auto/fixed/preset. Actions target columns, not just
windows.

**Why crab needs it:** crab's `layout/scroll.rs` already has a `Column` and a
`ScrollState` with widths/presets, but the action surface is thin:
`preset_width`, `center_column`, master ratio/count. There is no
move-column-left/right, consume/expel, set-column-width, or set-window-height.
A scrollable tiling WM without those is frustrating in daily use.

**Crab insertion point:** pure algorithm in `src/layout/scroll.rs` (add
operations + property tests); new `crab_ipc::Action` variants; binds/aliases in
`binds.rs`; config parsing if a configurable width list is desired.

**Suggested action set:** `move-column-left/right`, `move-window-to-column`,
`consume-window-into-column`, `expel-window-from-column`,
`expand-column-to-available-width`, `switch-preset-column-width`,
`switch-preset-window-height`, `set-column-width`, `set-window-height`,
`center-column`, and focus-window-within-column.

**Status:** columns now carry the move/window-move/consume/expel/expand and
set-width operations as pure `ScrollState` functions in `src/layout/scroll.rs`,
with unit tests, a randomized invariant (no window is lost, duplicated, or
invented), `crab_ipc::Action` variants, bind aliases, and a real-client
compositor scenario. Window heights are stored per window
(`ScrollState::heights`), with `set-window-height` and
`switch-preset-window-height`; fixed heights claim their pixels and the rest
share what remains. Focus-within-column remains.

**Acceptance:** property tests preserve window set and ordering; IPC actions are
idempotent where expected; a nested client session exercises a column move and a
width preset.

**Priority:** P1 · **Effort:** L · **Depends on:** none (pure), but B01 improves
the visual result.

---

#### B04 — Extract a `FloatingSpace` from the layout bridge

**niri:** `src/layout/floating.rs` (1,430 LOC), alongside `scrolling.rs`.

**What niri does:** floating windows have their own placement/z-order/geometry
space, separate from the scrolling strip. Moving between tiled and floating is an
explicit transition, not a flag checked inside every tiled operation.

**Why crab needs it:** floating handling is currently spread through
`layout.rs` (`place` passes, `floating` checks) and `ManagedWindow`. Crab already
supports per-space `LayoutKind::Floating` and per-window floating, but the
separation is ad hoc. Extracting a pure `FloatingSpace` would make placement,
z-order, and tests much clearer.

**Crab insertion point:** `src/layout/floating.rs` (pure), with a
`FloatingSpace` that owns `Vec<FloatingWindow>` plus cascade placement and
z-order; `layout.rs` calls it for the floating pass. Keep `always_on_top`,
fullscreen/maximized cover rules, and rules' `open_floating` behavior.

**Acceptance:** floating behavior is unchanged in existing tests; a property
test covers non-overlap of cascaded new windows and stable z-order across
focus.

**Priority:** P2 · **Effort:** M · **Depends on:** none.

---

#### B05 — Workspace identity and output migration (adapted to 32 slots)

**niri:** `src/layout/mod.rs:1–40` (design invariants), `layout/monitor.rs`
(`Monitor.workspaces: Vec<Workspace>`, `original_outputs`,
`InsertWorkspace`, `InsertHint`, `InsertPosition`, `MonitorAddWindowTarget`).

**What niri does:** every workspace remembers its ordered output candidates.
On output disconnect, its workspaces move to the first connected candidate (or
the primary output); on reconnect, they move back. Creating/moving windows on a
workspace forgets its original output so it stays where the user is working.

**Why crab needs it:** `remove_output` currently reassigns every window to the
lowest-id remaining output and drops per-space layout state
(`state.rs:1418–1435`). Unplug/replug destroys scroll positions, column widths,
visibility, and focus history. This is the largest architectural difference
between crab and niri.

**Crab insertion point (minimal, slot-preserving):**
- Add `original_output: Option<OutputId>` (or an ordered candidate list) to
  `SpaceState` / `OutputModel`.
- On `remove_output`, move each `SpaceState` and its windows as a unit to a
  surviving output, remapping space indices explicitly; do not reset to
  `default_space`.
- Preserve `view.visible`, scroll offset, and layout params.
- On `add_output`, if a space's candidate matches the new connector, move it
  back and re-run `relayout` for both outputs.
- Add a headless test with two outputs and a fake unplug/replug.

**Long-term option:** if unbounded named workspaces become a requirement,
supersede ADR-0004 with a niri-style `Vec<Workspace>` model. Do not do both at
once; the fixed-slot model is a valid product choice.

**Acceptance:** unplugging an output preserves every window's space identity and
scroll state; replugging restores it; no window is silently moved to the focused
output.

**Status (2026-11-06): done.** `SpaceState.home: Option<SpaceHome { output, slot }>`
records where a parked space came from. `remove_output` moves each occupied
space and its windows as a unit to the fallback (or the lowest survivor when the
fallback is leaving), keeps the slot number when the target does not use it and
falls through to the next free slot otherwise, and saves the removed view in
`Crab.parked_views`. `add_output` moves spaces whose home matches the new output
back to their recorded slots, restores the parked visible/previous view, and
compacts afterwards. A window that also belongs to a space staying behind cannot
be split across outputs: it remains on the source and loses the departing bits.
Covered by `unplugging_an_output_parks_its_spaces_and_replug_restores_them`.

**Priority:** P1 · **Effort:** L · **Depends on:** C5, C6.

---

#### B06 — Per-output focus ring / MRU

**niri:** `src/ui/mru.rs` (1,947 LOC), `src/layout/focus_ring.rs`.

**What niri does:** maintains a most-recently-used order per output for
`focus-previous`, alt-tab-style switching, and focus restoration after a window
closes.

**Why crab needs it:** crab has `core/focus.rs` (134 LOC) and a focus stack with
`toggle_back`, but no per-output MRU and no “focus last window” semantic beyond
the view history. As windows move across spaces, the simple stack can point at a
hidden window.

**Crab insertion point:** `src/core/focus.rs` + state-side activation hooks.
Keep it pure and small: `FocusRing` with `touch`, `remove`, `previous_on(output)`,
bounded to windows that are visible on the target output. Add `focus-previous`
(alias `focus_last`) to IPC/binds.

**Acceptance:** closing the focused window returns focus to the most recently
focused visible window on the same output; property tests cover removal and
duplicate touches.

**Priority:** P2 · **Effort:** M · **Depends on:** C5.

---

#### B07 — Opening/closing window animation architecture

**niri:** `src/layout/opening_window.rs`, `closing_window.rs`, `shadow.rs`,
`insert_hint_element.rs`, `tab_indicator.rs`; animation code under
`src/animation/`.

**What niri does:** opening/closing windows are first-class scene elements with
their own lifecycle, buffers, geometry, and animation state, rather than a flag
on a live window.

**Why crab needs it:** crab already keeps a `ClosingWindow` snapshot with the
last buffer (`state.rs:2248–2316`), which is the right idea, but the types are
embedded in `state.rs` and only support open/close fade/slide/zoom. As effects
and workspace animations land, a separate lifecycle type will pay off.

**Crab insertion point:** split `OpeningWindow`/`ClosingWindow` into
`src/compositor/scene.rs` (or `src/core` where pure), with a small trait for
“sample visual state at time t.” Keep ADR-0008's ordered element list.

**Acceptance:** no behavior change; open/close tests still pass; the close
snapshot survives client unmap/destroy without leaking the buffer.

**Priority:** P2 · **Effort:** M · **Depends on:** none.

---

#### B08 — Insert hints and tab indicators

**niri:** `src/layout/insert_hint_element.rs`, `tab_indicator.rs`; overview UI
under `src/ui/`.

**What niri does:** while dragging/moving a window, draws an insertion hint;
in tabbed columns, draws a tab indicator.

**Why crab needs it:** crab has interactive move/resize but no drag preview or
insertion target. This is polish, not a blocker.

**Crab insertion point:** pure geometry in `src/layout` + scene elements in
`render.rs`; reuse the existing `SceneElement::Solid`/`Surface` list.

**Priority:** P3 · **Effort:** S/M · **Depends on:** B03.

---

### 4.2 Backend, session, and frame pacing

#### B09 — A real TTY/DRM/KMS backend

**niri:** `src/backend/tty.rs` (3,780 LOC): `LibSeatSession`, `UdevBackend`,
`Libinput`, `GbmAllocator`, `GlesRenderer`, `DrmCompositor`, output hotplug,
session pause/resume, gamma, multi-GPU (`GpuManager`, `MultiRenderer`).

**What niri does:** owns the whole bare-metal path: session activation,
device enumeration, connector/mode selection, GBM buffer allocation, EGL/GLES
rendering, DRM plane composition, libinput dispatch, frame callbacks, and
hotplug.

**Why crab needs it:** without KMS there is no compositor a user can log into.
`backend/tty.rs` currently probes connectors and then bails
(`backend/tty.rs:106–110`); `backend-x11` is compiled but unreachable.

**Crab insertion point:** `src/backend/tty.rs` already has the right seams:
`run`, `probe_device`, `output_plan`, `apply_configured_position`. Implement in
stages:

1. session + udev enumeration + DRM device open (extend current probe);
2. GBM allocator + EGL/GLES renderer + `DrmCompositor`;
3. output model registration via `Crab::add_output`/`set_output_size`;
4. libinput backend feeding the existing `input::handle`;
5. frame clock + `Output::finish_frame`/presentation feedback;
6. hotplug and session pause/resume;
7. multi-GPU (B11).

**Acceptance:** crab starts on a free VT, shows the configured outputs, runs a
nested-equivalent client set, survives VT switch, and hotplugs a connector
without losing windows.

**Priority:** P1 (the top item) · **Effort:** XL · **Depends on:** B12 for
frame pacing; B10 lands alongside it.

---

#### B10 — libinput and session lifecycle

**niri:** `src/input/mod.rs` (6,182 LOC) and `backend/tty.rs` session handling.

**What niri does:** libinput device configuration (tap, natural scroll, disable
while typing), per-device event routing, seat focus, touch/tablet/gesture
events, and session pause/resume (suspend libinput, reapply output config on
resume).

**Why crab needs it:** crab's input layer only speaks Smithay backend events;
touchpad/mouse config fields are parsed but unused. Session lifecycle is absent
because KMS is absent.

**Crab insertion point:** `src/compositor/input.rs` already routes keyboard,
pointer, axis, and swipe events. Add libinput-specific config application in the
tty backend (device added/removed), and a `session` module for pause/resume.
Do not put libinput types in the pure modules.

**Acceptance:** tap-to-click, natural scrolling, and disable-while-typing
actually change behavior on a real touchpad; VT switch suspends input and
rendering and resumes cleanly.

**Priority:** P1 · **Effort:** L (after B09) · **Depends on:** B09.

---

#### B11 — Multi-GPU and multi-renderer

**niri:** `src/backend/tty.rs` uses `GpuManager`, `GbmGlesBackend`,
`MultiRenderer`, `MultiFrame`.

**What niri does:** renders on whichever GPU owns an output, imports buffers
across GPUs when needed.

**Why crab needs it:** laptops with hybrid graphics and multi-GPU desktops will
otherwise fail or fall back poorly. This is not needed for a first usable
release.

**Crab insertion point:** backend-only; the rest of crab already addresses
outputs by `OutputId`. Defer until B09 is stable.

**Priority:** P3 · **Effort:** XL · **Depends on:** B09.

---

#### B12 — Frame clock and presentation feedback

**niri:** `src/frame_clock.rs` (98 LOC) plus presentation-time handling.

**What niri does:** one frame clock per output, advanced from the backend's
presentation timestamp; animations and frame callbacks sample it. This avoids
the “loop clock under winit” jitter crab documents.

**Why crab needs it:** crab's winit backend uses `start_time.elapsed()`
(`winit.rs:156`), which jitters with event-loop scheduling. KMS needs vblank
timing anyway, and `wp_presentation` clients need accurate feedback.

**Crab insertion point:** `src/animation/clock.rs` already has a `Clocks`
abstraction; add a per-output frame clock in the backend, pass its time to
`advance_frame`, and register `PresentationState` in `state.rs`.

**Acceptance:** animations sample presentation time, not loop time; a
`wp_presentation` client receives `presented`/`discarded` feedback; no visual
regression under winit.

**Priority:** P1 · **Effort:** S/M · **Depends on:** none for clock
correctness; B16 consumes it for presentation feedback.

---

#### B13 — Output hotplug and runtime output configuration

**niri:** `src/protocols/output_management.rs` (862 LOC),
`src/backend/tty.rs` udev handling, `niri-config` output parsing.

**What niri does:** applies connector modes/positions/scales/transforms from
config, reacts to hotplug, and lets `wlr-randr`/kanshi change outputs at runtime
via `zwlr_output_manager_v1` + IPC `Output`.

**Why crab needs it:** crab has `core/output_plan.rs` (653 LOC) and config
parsing for modes/transforms, but no runtime application path. The model is
ready; the protocol/backend glue is not.

**Crab insertion point:** finish `output_plan` → tty backend wiring; then add
the output-management global and IPC `Output` request. Keep `OutputPreference`
as the pure representation.

**Acceptance:** `wlr-randr` can list and change mode/scale/transform/position;
changes survive config reload semantics; invalid modes are rejected without
killing the session.

**Priority:** P1/P2 · **Effort:** L · **Depends on:** B09.

---

#### B14 — Session lock

**niri:** `SessionLockManagerState` registration (`niri.rs:2558`), lock
handling in `handlers/`.

**What niri does:** advertises `ext-session-lock-v1`, blanks/hides the scene
while locked, restores on unlock, and filters input.

**Why crab needs it:** a session compositor needs a locker. crab's bind flags
`allow-when-locked`/`locked` are parsed but reserved (`docs/CONFIG.md`).

**Crab insertion point:** global in `state.rs`; a `locked` flag in `Crab`;
render a lock surface layer; route keys only to the lock client; wire the
existing bind flags.

**Acceptance:** `swaylock`/`gtklock` locks the session; no normal window input
leaks; unlock restores focus.

**Priority:** P1 · **Effort:** M · **Depends on:** B09 for a real session.

---

### 4.3 Protocol breadth

niri's registrations are in `src/niri.rs:2530–2685`. Add globals in roughly
this order. None of this belongs in the pure modules.

| ID | Protocol | niri reference | Crab insertion point | Priority | Effort |
|---|---|---|---|---|---|
| B15 | `linux-dmabuf` + feedback | `niri.rs:2565`, handlers | `state.rs` global + `delegate_dmabuf!`; shm fallback stays | P1 | M |
| B16 | `wp_presentation` | `niri.rs:2594` | `state.rs` global; feed from B12 | P1 | S |
| B17 | `wp_cursor_shape` + cursor themes | `niri.rs:2681`, `cursor.rs` (318 LOC) | `render.rs:343` note; add manager + theme loader | P1 | M |
| B18 | `ext-workspace-v1` | `protocols/ext_workspace.rs` (689 LOC) | new `src/compositor/protocols/ext_workspace.rs`; map 32 slots to workspace groups | P1 | L |
| B19 | `wlr-foreign-toplevel` / `ext-foreign-toplevel-list` | `protocols/foreign_toplevel.rs` (624 LOC) | new protocol module; window events already exist | P1 | M |
| B20 | `idle-notify` + `idle-inhibit` | `niri.rs:2573–2574` | `state.rs`; timer + inhibitor list | P1 | S/M |
| B21 | primary selection + `wlr`/`ext` data-control | `niri.rs:2577–2592` | `state.rs` globals + handlers | P1 | M |
| B22 | pointer constraints + relative pointer + pointer gestures | `niri.rs:2570–2572` | `input.rs` + handlers | P1 | M |
| B23 | `text-input-v3` + `input-method-v2` | `niri.rs:2598–2600` | handlers + focus/popup integration | P1 | L |
| B24 | `keyboard-shortcuts-inhibit` | `niri.rs:2602` | seat handler; gate bind dispatch | P1 | S |
| B25 | `wlr-output-management` + `gamma-control` | `protocols/output_management.rs`, `gamma_control.rs` | B13 | P1 | L |
| B26 | `wlr-screencopy` + `ext-image-copy-capture` | `protocols/screencopy.rs` (720 LOC), `handlers/image_copy_capture.rs` (458) | new protocol modules; damage-tracked scene helps | P1/P2 | L |
| B27 | XWayland via `xwayland-satellite` | ADR-0006; niri has in-process X11 | follow crab's ADR, not niri's | P2 | L |
| B28 | tablet-v2 + touch | `niri.rs:2569`, `input/mod.rs` | `input.rs` + handlers | P2 | L |
| B29 | security-context + virtual keyboard/pointer | `niri.rs:2596–2606` | `state.rs`; restrict by client | P2 | M |
| B30 | `single-pixel-buffer`, `xdg-foreign` | niri registration | cheap wins; add as globals/handlers | P2 | S each |
| B31 | `ext-background-effect` | `handlers/background_effect.rs` (121) | after ADR-0007 effects | P3 | M |
| B32 | `drm-lease` | niri support | tty backend; hand off DRM fds | P3 | M |

**Notes**

- **B15 is the most valuable single protocol.** Without it every client uses
  `wl_shm`; the GLES renderer only helps compositing. Add `DmabufState` and
  feedback, keep shm as fallback, and test with a GL client (`foot`, GTK4).
- **B18 is a strategic ADR commitment.** ADR-0005 names `ext-workspace-v1` the
  primary bar interface, but the global does not exist. Either implement it or
  revise the ADR; do not leave the contradiction.
- **Within B26:** implement `wlr-screencopy` first for `grim`/`slurp`; then
  `ext-image-copy-capture`; PipeWire/portal casting is a separate project
  (niri's `screencasting/` is ~2.8k LOC).
- **B23** is large because IME needs popup positioning, focus transfer, and
  preedit rendering; schedule it after B17 and B35.
- **Not in this niri snapshot:** `alpha-modifier`, `content-type`,
  `tearing-control`, and `xdg-toplevel-drag` are still worth adding, but take
  their reference from Smithay or another compositor rather than niri.

---

### 4.4 Input

#### B33 — Live gesture state machine

**niri:** `src/input/mod.rs:4107–4292`, `src/input/scroll_swipe_gesture.rs`
(69 LOC), layout methods `view_offset_gesture_*`,
`workspace_switch_gesture_*`, `overview_gesture_*`.

**What niri does:** 3-finger horizontal swipes move the view, vertical swipes
switch workspace, and 4-finger swipes drive the overview; updates are fed into
the layout every frame, and end settles or cancels based on velocity/progress.
Natural-scroll and unaccelerated deltas are handled per device.

**Why crab needs it:** crab accumulates deltas in thread-locals and only fires a
bind at swipe end if the dominant axis exceeds a hardcoded 30 px
(`input.rs:32–36, 383–409`). That is not a gesture system; it is a threshold
shortcut.

**Crab insertion point:**
- Move gesture state into `Crab` (not `thread_local!`) keyed by seat/device.
- Add `GestureState` in `src/compositor/input.rs` with begin/update/end and
  finger count.
- Add pure layout entry points (`view_offset_gesture_*`,
  `workspace_switch_gesture_*`) in `src/layout`; feed `ScrollState.offset` for
  live movement.
- Apply the configured `swipe_min_threshold` (default 1.0) consistently, or
  remove the field.
- Handle device natural-scroll and unaccelerated deltas in the tty backend.

**Acceptance:** a 3-finger horizontal swipe moves the view smoothly and settles
on a column/space; a short swipe does nothing; canceling mid-swipe restores the
original offset; no thread-local state remains.

**Priority:** P1 · **Effort:** M · **Depends on:** B03/B09 for real devices.

---

#### B34 — Touch and tablet input

**niri:** `src/input/mod.rs` (touch/tablet sections), `niri.rs:2569`.

**What niri does:** tracks touch points, maps them to outputs, forwards touch
events to clients, handles tablet tools/pads and mapping.

**Why crab needs it:** no touch/tablet support exists; `src/input/` is an empty
placeholder directory in crab.

**Crab insertion point:** `src/compositor/input.rs` + Smithay handlers; touch
mapping needs per-output absolute coordinates (and exposes the current
`PointerMotionAbsolute` focused-output assumption, `input.rs:47–56`).

**Priority:** P2 · **Effort:** L · **Depends on:** B09.

---

#### B35 — Seat focus and bind-state robustness

**niri:** input focus handling in `input/mod.rs`; explicit cancellation of
interrupted release binds (e.g. `cancel_interrupted_release_binds`, line 4108).

**What crab needs:** `bound_keys` (`input.rs:114–175`) is keyed only by raw
keycode and is never cleared on seat/output focus loss. If a release is lost,
the next press is treated as a repeat and swallowed with stale flags.

**Crab insertion point:** clear `bound_keys`/`bound_buttons` on keyboard focus
change, grab begin/end, output focus change, and session pause; add a
`cancel_interrupted_release_binds` equivalent.

**Priority:** P2 · **Effort:** S · **Depends on:** none.

---

### 4.5 IPC

#### B36 — Bounded event channels and atomic event state

**niri:** `src/ipc/server.rs:43` (`EVENT_STREAM_BUFFER_SIZE = 64`),
`niri-ipc/src/state.rs` (`EventStreamState`, `EventStreamStatePart`), per-client
`async_channel::bounded`.

**What niri does:** fans out events through bounded per-client channels, keeps
per-domain state so the first event of each kind is a full snapshot, and writes
whole events. A client that falls too far behind is disconnected deliberately
at an event boundary with a warning; it is never left with a half-written line.

**Why crab needs it:** crab writes directly to a nonblocking `UnixStream` and
drops a client on `WouldBlock` (`ipc/mod.rs:790–800`). That can happen after a
partial line has been written, and the only trace is a debug log. A busy bar can
be disconnected during a window burst; there is no per-domain
snapshot/epoch state for a coherent reconnect.

**Crab insertion point:** keep the JSON schema and `Event` enum, but add:
- a per-client bounded queue (or an internal buffer + readable/writable calloop
  source) instead of dropping on `WouldBlock`;
- `Snapshot` state per client so a reconnecting client gets a consistent first
  event;
- backpressure metrics/logs.

**Acceptance:** a burst of 1,000 window events uses bounded per-client memory;
a deliberately slow reader is disconnected only at an event boundary with a
warning; a reconnecting subscriber gets a coherent initial snapshot; no
truncated JSON line is ever observed.

**Priority:** P1 · **Effort:** M · **Depends on:** none.

---

#### B37 — Richer query surface

**niri:** `niri-ipc/src/lib.rs:67–122` — `Layers`, `KeyboardLayouts`,
`FocusedOutput`, `PickWindow`, `PickColor`, `Output { output, action }`,
`OverviewState`, `Casts`.

**What crab has:** ten request kinds — `Version`, `Outputs`, `Spaces`, `Windows`,
`FocusedWindow`, `Layouts`, `Dispatch`, `Subscribe`, `ConfigReload`,
`ConfigValidate` — plus 29 actions (including `Quit` and `Spawn`), and a
snapshot/diff event stream.

**Borrow:** `Layers` (bar/debugging), `KeyboardLayouts` (status), `FocusedOutput`,
and eventually `Output` (B13). `PickWindow`/`PickColor` require a compositor
interaction mode; defer to P2.

**Crab insertion point:** `crab-ipc/src/protocol.rs` + `src/ipc/mod.rs`
handlers + `crab-cli` subcommands + `docs/IPC.md`.

**Acceptance:** each new request has a round-trip test and a CLI command.

**Priority:** P2 · **Effort:** S/M · **Depends on:** none.

---

#### B38 — Output configuration over IPC

**niri:** `Request::Output { output, action }`; `OutputAction` variants.

**What crab needs:** `wlr-randr`-style runtime changes without restarting, and
a way for `kanshi`/`niri msg`-style scripts to apply temporary config.

**Crab insertion point:** after B13, add `Action::Output` to `crab-ipc` and a
`Crab::apply_output_config` that updates `OutputModel` + backend. Validate
against `OutputPreference` before applying.

**Priority:** P2 · **Effort:** M · **Depends on:** B13.

---

### 4.6 Config

#### B39 — Borrow niri-config's validation/merge patterns

**niri:** `niri-config` (10,304 LOC): `MergeWith`, per-output config, bind
parsing, window rules, animations, gestures, validation.

**What to borrow (patterns, not schema):**
- explicit `MergeWith` for layered config (defaults + user config + runtime
  overrides) so reload cannot half-apply;
- per-section validation with path-qualified errors (crab already does a good
  job here);
- output config as a reusable pure type (`OutputPreference` is already close);
- action parsing from config strings with structured errors (crab's
  `BindIssue` is a good start).

**Why:** crab's config is already solid, but as `[input]`, `[output]`, rules,
gestures, and runtime output config grow, ad-hoc merge logic will drift. niri's
separation of parse → merge → validate → apply is worth copying.

**Crab insertion point:** `src/config/mod.rs`; keep it pure and under the
boundary. Add config-version/migration notes to `docs/CONFIG.md`.

**Priority:** P2 · **Effort:** M · **Depends on:** none.

---

#### B40 — Wire or explicitly reject parsed-only fields

Not strictly a niri borrow, but niri demonstrates the alternative: every config
field either works or is absent. Crab currently parses:

- `layout.client_side_decorations`
- `layout.{stack,tile,scroll}.next_space_direction`
- `layout.scroll.always_center_single_column`
- all `[input.touchpad]` fields, `[input.mouse].natural_scrolling`,
  `[input.keyboard].numlock_on`
- `swipe_min_threshold` (hardcoded 30 in code)

Either implement them (most belong to B09/B10/B33) or remove them until their
milestone, and make `pending_sections()` cover per-rule fields such as `x11`.

**Priority:** P1 for `x11` semantics (C2 in §3); P2 for the rest.

---

### 4.7 Testing and tooling

#### B41 — niri-style layout integration tests

**niri:** `src/layout/tests.rs` (4,050 LOC) and `src/tests/` (6,780 LOC).

**What niri does:** constructs a fake `Layout` with fake windows/outputs and
drives operations (add, move, resize, close, output remove) through assertions
on the resulting layout, without a compositor or GPU.

**Why crab needs it:** crab's 293 tests are mostly pure units; there is no
multi-window/multi-output scenario suite. B01–B05 will be hard to trust without
one.

**Crab insertion point:** `tests/` + a small `tests/common/` harness that builds
`OutputModel`, `SpaceState`, and `LayoutWindow` values. Cover:
- two outputs with windows on each;
- close a background-output window (C4);
- layer exclusive-zone change on a secondary output (C3);
- unplug/replug migration (B05);
- column move/consume/expel (B03);
- transaction deadline with a fake refusing client (B01).

**Status:** `tests/layout_scenarios.rs` covers the pure half — independent
two-output arrangement, move-to-space and its source reflow, cross-output
migration, swap order, close reflow, and a scroll column closing before the
focused one. `src/compositor/state/multi_output.rs` covers the protocol half —
move-drag edge snapping without changing output, resize-drag pinned opposite
edges, focused-close focus handoff on the owning output, move-to-space leaving
the peer output untouched, plus the existing C3/C4 and unplug/replug cases.
Column move/consume/expel waits on B03 and the transaction deadline on B01.

**Priority:** P1 · **Effort:** M/L.

---

#### B42 — Visual/snapshot tests

**niri:** `niri-visual-tests` (2,058 LOC).

**What niri does:** renders scripted scenes and compares against reference
images/parameters, catching layout/animation regressions that unit tests miss.

**Why crab needs it:** once animations/effects land, pixel-level regressions are
easy. crab already has a deterministic frame clock and damage tracker, which
makes this feasible.

**Crab insertion point:** a `visual-tests/` crate using headless GLES or a
software renderer; start with layout geometry snapshots (JSON) rather than
pixels, then add image comparison.

**Priority:** P2 · **Effort:** L · **Depends on:** stable B09/renderer.

---

#### B43 — CI hardening

Borrow niri's habit of testing the real feature set:

- run `cargo test --workspace --all-features` (crab currently tests without
  `--all-features`);
- enforce MSRV (`cargo +1.85 check`) instead of `channel = "stable"` drifting;
- run `cargo doc --all-features` with `-D warnings` (already in CI);
- add a headless smoke test that starts the compositor's state without a GPU;
- keep `check-boundaries.sh` green (C1).

**Priority:** P1 · **Effort:** S.

---

### 4.8 Internal performance items

These are not niri ports, but niri's scale makes the patterns worth adopting.

| ID | Item | Crab evidence | Change |
|---|---|---|---|
| B44 | Stop cloning `ScrollState` per arrange | `layout.rs:210–230` clones `Vec<Column>` on every relayout | `mem::take` + write back, or an `Arc`/`&mut` split |
| B45 | Stop deep-cloning 32 `SpaceState`s on compact | `core/output.rs:394–403` | move states + `default_space` only for freed slots (done 2026-11-06) |
| B46 | Render the config banner as a texture | `render.rs:285–330` creates up to ~16k solid elements | rasterize the glyph run once into a buffer/texture |
| B47 | Index windows per output | `arrange` filters all windows for each output; `occupied_spaces` scans all | `HashMap<OutputId, Vec<WindowId>>` maintained on add/remove/migrate |
| B48 | Avoid full-snapshot diff allocation on every IPC event | `ipc/mod.rs:637–745` builds full snapshots + maps | per-domain dirty flags or incremental event state (B36) |
| B49 | Content-type/opaque region correctness | render path already uses damage tracker | ensure `wp_content_type` and opaque regions match once dmabuf lands |

**Priority:** P2/P3, except B44/B45 which are cheap and can land with B01.

**B45 landed (2026-11-06):** `Output::compact` moves each kept `SpaceState`
into its new slot with `mem::take` and clones `default_space` only for freed
slots, so closing or migrating a window no longer deep-clones all 32 states
(including their column lists and focus histories). The compaction unit test
checks that a moved focus-history allocation keeps its address.

---

## 5. Recommended sequencing

### Phase A — Stabilize the current tree (1–2 weeks) — *C1–C6 done 2026-10-30*

1. ~~C1 boundary check.~~ ✅
2. ~~C2 `x11` rule semantics.~~ ✅
3. ~~C3 layer origin.~~ ✅
4. ~~C4/C5 focused-output refactor + two-output tests.~~ ✅
5. ~~C6 two-output harness.~~ ✅
6. B43 CI (`--all-features`, MSRV) — next.
7. B41 remaining scenarios as B01–B05 land.
8. B44/B45 cheap allocation fixes.

**Exit:** green CI; multi-output invariants tested; no known rule/view bugs.

### Phase B — Make it a real session (4–6 weeks)

1. B12 frame clock (winit first).
2. B15 dmabuf.
3. B09 KMS/session in stages.
4. B10 libinput + session lifecycle.
5. B16 presentation-time.
6. B17 cursor shape.
7. B14 session lock.
8. B20 idle inhibit/notify.

**Exit:** crab runs on a VT, with GPU buffers, a cursor, a locker, and idle
management. This is the “daily driver beta” line.

### Phase C — Layout and interaction quality (4–6 weeks)

1. B01 transactions.
2. B02 configure throttling.
3. B03 column actions.
4. B05 workspace migration.
5. B33 live gestures.
6. B34 touch/tablet (if hardware).
7. B18 ext-workspace + B19 foreign-toplevel.

**Exit:** multi-window resize/scroll feels stable; bars work; hotplug does not
destroy layout state.

### Phase D — Parity and polish (ongoing)

1. B21 clipboard, B22 constraints, B23 IME, B24 inhibit.
2. B25 output management + B38 IPC output config.
3. B26 screencopy/capture.
4. B27 XWayland satellite.
5. B04 floating extraction, B06 MRU, B07/B08 scene polish.
6. B39/B40 config completeness.
7. B41/B42 test harnesses.
8. B11 multi-GPU, B31 effects, HDR via ADR-0002.

---

## 6. Reference map: niri file → crab target

| niri | LOC | Borrow for | Crab target |
|---|---|---|---|
| `src/utils/transaction.rs` | 193 | commit blockers, 300 ms deadline | `src/compositor/transaction.rs` (new) |
| `src/layout/mod.rs` | 5,055 | `ConfigureIntent`, workspace invariants | `src/layout`, `src/compositor/state.rs` |
| `src/layout/monitor.rs` | 2,234 | workspaces per monitor, insert hints, migration | `src/core/output.rs`, `state.rs` |
| `src/layout/workspace.rs` | 2,126 | workspace behavior, focus/activation | `src/core/spaces.rs`, `state.rs` |
| `src/layout/scrolling.rs` | 5,752 | columns, widths/heights, presets, scroll ops | `src/layout/scroll.rs` |
| `src/layout/floating.rs` | 1,430 | floating space/z-order/cascade | `src/layout/floating.rs` (new) |
| `src/layout/opening_window.rs` / `closing_window.rs` | — | window lifecycle scenes | `src/compositor/scene.rs` |
| `src/layout/focus_ring.rs`, `src/ui/mru.rs` | 2,223 (276 + 1,947) | MRU/per-output focus | `src/core/focus.rs` |
| `src/backend/tty.rs` | 3,780 | KMS/GBM/libseat/udev/libinput | `src/backend/tty.rs` |
| `src/backend/mod.rs` | — | backend selection, multi-GPU | `src/backend/mod.rs` |
| `src/frame_clock.rs` | 98 | presentation-timed frames | `src/animation/clock.rs`, backend |
| `src/input/mod.rs` | 6,182 | device config, touch/tablet/gestures | `src/compositor/input.rs` |
| `src/input/scroll_swipe_gesture.rs` | 69 | swipe direction/state | `src/compositor/input.rs`, `src/layout` |
| `src/cursor.rs` | 318 | cursor shape/themes | `src/compositor/cursor.rs` (new) |
| `src/protocols/ext_workspace.rs` | 689 | ext-workspace-v1 | `src/compositor/protocols/` |
| `src/protocols/foreign_toplevel.rs` | 624 | taskbars | same |
| `src/protocols/output_management.rs` | 862 | wlr-output-management | same |
| `src/protocols/gamma_control.rs` | 219 | gamma | same |
| `src/protocols/screencopy.rs` | 720 | wlr-screencopy | same |
| `src/handlers/image_copy_capture.rs` | 458 | ext-image-copy-capture | `src/compositor/handlers/` |
| `src/handlers/background_effect.rs` | 121 | client blur | after ADR-0007 |
| `src/ipc/server.rs` | 1,004 | bounded event streams | `src/ipc/mod.rs` |
| `niri-ipc` (`lib.rs`, `state.rs`) | 2,664 (whole crate) | request/event/state shapes | `crab-ipc`, `src/ipc` |
| `niri-config` | 10,304 | merge/validate/output config patterns | `src/config` |
| `src/layout/tests.rs`, `src/tests/` | 10,830 | layout/integration harness | `tests/` |
| `niri-visual-tests` | 2,058 | snapshot/visual regression | `visual-tests/` (new) |

---

## 7. What not to borrow

1. **Unbounded `Vec<Workspace>` storage.** crab's ADR-0004 chose 32 unified
   numbered slots. Borrow niri's *migration semantics*, not its storage, unless
   a superseding ADR is accepted.
2. **niri's config schema.** crab has its own TOML and mango-style aliases;
   copying niri's `niri-config` types would break `docs/CONFIG.md` and
   `resources/default-config.toml`. Borrow validation/merge *patterns*.
3. **niri's render element tree and effects code verbatim.** crab has ADR-0008
   (ordered scene) and ADR-0007 (own GLES passes). Borrow lifecycle and
   animation structure.
4. **niri's XWayland integration.** crab's ADR-0006 explicitly chooses
   `xwayland-satellite` with `-listenfd`; follow that instead.
5. **niri's overview/UI/a11y/D-Bus modules** until crab has a usable session.
   These are large and not on the critical path.
6. **PipeWire screencasting.** Start with `wlr-screencopy`; niri's
   `screencasting/` is a separate multi-thousand-line project.
7. **Smithay git-only APIs.** Check every call against crab's crates.io 0.7.0.
   If a needed fix is only on niri's git revision, either vendor/patch Smithay
   deliberately (ADR-0002 already anticipates this for HDR) or find a 0.7
   workaround.
8. **niri's test helpers expecting niri types.** Port the *scenario shape*, not
   the harness; crab's `LayoutWindow`/`OutputModel` differ.

---

## 8. Acceptance checklist for every borrowed item

- [ ] Pure logic lives in `src/{core,config,layout,animation}` and does not
      mention Smithay/Wayland/system APIs.
- [ ] `scripts/check-boundaries.sh` passes.
- [ ] New unit/property tests cover the pure logic; a scenario test covers the
      compositor behavior.
- [ ] Existing 293 tests still pass; clippy/fmt/doc clean.
- [ ] `docs/CONFIG.md` / `docs/IPC.md` / `COMPOSITOR.md` updated where behavior
      changes.
- [ ] ADR updated or added if a design decision changes.
- [ ] niri source/commit attributed in the file header and `NOTICE` if code was
      adapted.
- [ ] No config field is silently accepted-but-ignored; either wire it or
      remove it and report it via `pending_sections()`.
- [ ] Hot-path changes have a before/after measurement.
- [ ] Manual reproduction recorded for backend/protocol changes (VT, hotplug,
      touchpad, locker, bar).

---

## Appendix A — niri landmarks worth reading first

If you only read five things in niri before starting:

1. `src/layout/mod.rs:1–40` — the output/workspace invariants.
2. `src/utils/transaction.rs` — the commit-blocker model (300 ms).
3. `src/layout/scrolling.rs:262–300` — `ColumnWidth`/`WindowHeight`, the core
   abstraction crab should grow toward.
4. `src/ipc/server.rs:200–260` — per-client bounded event streams.
5. `src/backend/tty.rs:440–520` — session + udev + libinput startup, the
   missing half of crab.

## Appendix B — crab landmarks to touch

| Concern | File |
|---|---|
| Central state / focus / rules / configure / layers | `src/compositor/state.rs` (4,235) |
| Layout ↔ Smithay bridge | `src/compositor/layout.rs` |
| Damage-tracked scene | `src/compositor/render.rs` |
| Input routing | `src/compositor/input.rs` |
| IPC server | `src/ipc/mod.rs` |
| Per-output model / spaces | `src/core/output.rs`, `src/core/spaces.rs` |
| Scroll layout | `src/layout/scroll.rs` |
| Config | `src/config/mod.rs` |
| Backends | `src/backend/{winit,tty,headless}.rs` |
| IPC schema | `crab-ipc/src/protocol.rs` |
| Boundary guard | `scripts/check-boundaries.sh` |
