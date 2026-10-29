# ADR-0005 — IPC schema: fresh and niri-shaped; `ext-workspace-v1` is the bar interface

**Status:** Accepted
**Date:** 2026-10-23
**Depends on:** ADR-0001 (license), ADR-0004 (space model)
**Relates to:** `COMPOSITOR.md` §14, `ROADMAP.md` §7/§10, `TODO.md` Phase 5

## Context

Two questions had to be settled together:

1. Should crab reuse niri's `niri-ipc` schema (or a wire-compatible subset), or
   define a fresh one?
2. Should bars use `ext-workspace-v1` as their primary interface, with IPC as the
   scripting/control channel — or should IPC be the primary interface?

The prior art differs:

- **niri** has a typed `Request`/`Reply`/`Event` schema, newline-delimited JSON, a
  version handshake, and an event stream. Its action vocabulary is shared with
  keybinds.
- **mango** exposes `mmsg` `get`/`watch`/`dispatch` over its own socket.
- `crab-ipc` is already niri-shaped (`Request`/`Response`/`Event`, kebab-case,
  newline JSON), but its vocabulary is stale: `Tags`/`TagInfo`/`ViewTag` predate
  ADR-0004's space rename, and its `Action` enum has drifted from the compositor's
  bind actions.

ADR-0004 makes the model concrete: up to 32 **numbered slots per output**, a
window may belong to several spaces, an output may show several spaces at once,
and dynamic compaction renumbers spaces and moves their state. niri's model is
named, dynamically created workspaces with (normally) one visible per output.
A wire-compatible niri schema would either force niri semantics onto crab or
expose crab-specific fields that break the compatibility claim.

`ext-workspace-v1` is the ecosystem's bar interface. It is in
**wayland-protocols staging**, but it is implemented by niri 26.04, COSMIC,
Hyprland, and labwc, and Waybar ships an `ext/workspaces` module. Its model fits
crab reasonably well:

- workspaces are grouped by output;
- `state` is a **bitfield** (`active`, `urgent`, `hidden`), so several workspaces
  can be active at once — exactly the multi-space view;
- `coordinates` provide a stable ordering within a group;
- `id` is explicitly meant for workspaces "likely stable across sessions", while
  workspaces without an id are temporary.

## Decision

### 1. A fresh schema, niri-shaped, not niri-compatible

`crab-ipc` defines crab's own `Request`, `Response`, `Event`, and `Action` types.
We borrow niri's *shape* — typed enums, kebab-case, newline-delimited JSON, a
version handshake, an event stream, and one action vocabulary shared with binds —
but we do **not** promise wire compatibility with niri, sway, or mango. Reusing
GPL-3.0 source with attribution remains permitted by ADR-0001; protocol
compatibility is not a goal.

Versioning:

- **0.x:** the schema may change; a breaking change bumps the version.
- **1.0:** the schema is frozen and only additive changes are allowed within a
  major version.
- `PROTOCOL_VERSION` is the handshake. Unknown events and unknown fields must be
  ignored by clients; malformed lines must never crash the compositor.

The canonical serde `Action` enum lives in `crab-ipc`. Keybind parsing produces
that type (directly or through a thin conversion), so the action names in config
and IPC cannot drift apart. Pointer-grab internals that cannot be expressed as a
command (interactive move/resize) stay compositor-side and are not serialized.

### 2. `ext-workspace-v1` is the primary bar interface

Bars discover spaces and windows through `ext-workspace-v1` and
`ext-foreign-toplevel-list`/`wlr-foreign-toplevel-management`. IPC is the
authoritative channel for names, urgency, richer window/output data, dispatch,
and configuration.

Mapping:

- **One workspace group per output.** The output's spaces are the group's
  workspaces.
- **Capabilities:** advertise `activate` and `deactivate` only. Do not advertise
  `create_workspace`, `remove`, or `assign`; dynamic compaction owns the
  lifecycle.
- **State:** set `active` for every space in `SpaceView::visible` (multiple active
  spaces are legal and represent a multi-space view), set `urgent` when any
  window on the space is urgent, and never set `hidden` for a space that exists.
- **Coordinates:** a one-dimensional `[index]` in current 1-based numbering.
  Bars sort by it; it changes when compaction renumbers.
- **Name:** the space's label once named (M3), otherwise its decimal index.
  Renaming emits `name`.
- **Id:** **omit it for ephemeral spaces.** The protocol expects ids for
  workspaces likely stable across sessions; a slot index renumbers under
  compaction, and a per-session counter is not stable. Emit an id only for a
  named/persistent space. A slot index must never masquerade as a stable id.
- **Activate/deactivate:** `activate` replaces the view with that space;
  `deactivate` removes it from the view without ever emptying it. Toggling a
  second space into a multi-space view is an IPC `Action`, not an `ext-workspace`
  request — the protocol cannot express "toggle" directly.
- **Batching:** send state changes followed by `done`. On compaction, send
  `workspace_leave`/`removed` and the survivors' updated coordinates in one
  `done` batch so bars reorder atomically.
- **Pin the wayland-protocols version.** `ext-workspace-v1` is staging and can
  still change.

Do **not** implement `wlr-workspaces` initially. It is a legacy surface; add it
only if a real bar cannot use `ext-workspace-v1`.

### 3. Transport and vocabulary

- Unix socket at `$XDG_RUNTIME_DIR/crab-$WAYLAND_DISPLAY.sock`, exported as
  `CRAB_SOCKET`. Sanitize `WAYLAND_DISPLAY` before using it in a path (it may be
  absolute), and create the socket with mode `0600`.
- Newline-delimited JSON, one in-flight request per connection. `subscribe`
  switches the connection to an **initial snapshot followed by deltas**; document
  whether a subscribed connection may still dispatch.
- All socket I/O runs on the `calloop` loop; parsing never blocks, and a line-size
  cap bounds memory.
- **"Space" is the internal and IPC name.** "Workspace" appears only at the
  protocol projection (ADR-0004). `Tags`/`TagInfo`/`ViewTag` are removed.

### 4. Minimum payloads

- `outputs`: id, name, description, logical geometry, scale, focus, visible
  spaces.
- `spaces`: index, name, visible, urgent, output.
- `windows`: id, title, app_id, output, spaces, focused, urgent, floating,
  fullscreen, geometry.
- `actions`: the same set as binds.
- Events: `WindowOpened`/`Closed`/`Changed`/`Focused`, `SpaceViewChanged`,
  `LayoutChanged`, `OutputAdded`/`Removed`/`Changed`, `KeyboardLayoutChanged`,
  `ConfigReloaded`/`ConfigError`, `Urgent`.

New fields are additive; consumers must ignore unknown ones.

## Consequences

- `crab-ipc` and `crab-cli` rename `tags` to `spaces` now, before the M5 server
  exists; the canonical `Action` replaces the divergent IPC enum.
- Bars that only need workspaces do not need crab-specific code. Bars that need
  names, urgency, a window list, or dispatch use IPC.
- `ext-workspace-v1` still represents a multi-space view correctly through
  multiple `active` bits, but it cannot toggle one space on and off; that stays an
  IPC action.
- Because ephemeral spaces have no `id`, bars cannot persist per-space
  preferences until spaces are named (M3). This is preferable to a fake id that
  changes on every compaction.
- niri IPC clients cannot talk to crab. Integration with `crawl-shell` uses the
  standard protocols first and the crab-specific IPC only where needed.
- Adapted niri/mango code carries attribution.

## Alternatives considered

- **Wire-compatible with niri.** Rejected: it would force named-workspace
  semantics onto ADR-0004, either lying about multi-space state or requiring
  crab-only extensions, and it would make every upstream schema change a
  compatibility obligation.
- **Depend on `niri-ipc` directly.** Rejected: licensing is fine (GPL-3.0), but
  it couples crab's public schema to another compositor's types and release
  cadence, and those types still cannot express crab's model.
- **IPC-only bars.** Rejected: every bar would need crab-specific integration,
  and `crawl-shell` already negotiates standard protocols.
- **`ext-workspace-v1` only, no IPC.** Rejected: it cannot dispatch actions,
  reload config, or carry the full window/output payload.
- **`wlr-workspaces` as the primary interface.** Rejected as the legacy surface;
  keep it as a possible compatibility add-on.
