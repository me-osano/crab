# IPC

Crab exposes a Unix socket for querying state and dispatching actions. The protocol lives in [`crab-ipc`](../crab-ipc).

## Socket

- Path: `$XDG_RUNTIME_DIR/crab-<WAYLAND_DISPLAY>.sock` (or `$CRAB_SOCKET` to override)
- Permissions: `0600`
- Spawned/autostarted processes receive `CRAB_SOCKET` in the environment when IPC is enabled

## Protocol

- Transport: newline-delimited JSON
- Version: `PROTOCOL_VERSION = 2` (additive-only within a major version)
- A connection is either **request/response** or **event stream** (after `subscribe`). A subscribed connection is event-only; use a second connection to dispatch actions.

### Requests

| Request | Response | Notes |
| --- | --- | --- |
| `{"type":"version"}` | `{"type":"version","name":"crab","version":2}` | Handshake |
| `{"type":"outputs"}` | `{"type":"outputs","outputs":[...]}` | All outputs |
| `{"type":"spaces"}` | `{"type":"spaces","spaces":[...]}` | Existing spaces (occupied or visible), not all slots |
| `{"type":"windows"}` | `{"type":"windows","windows":[...]}` | Mapped windows |
| `{"type":"focused-window"}` | `{"type":"windows","windows":[...]}` | Filtered to focused window |
| `{"type":"layouts"}` | `{"type":"layouts","layouts":[...],"families":[...]}` | Modes and families; family names are aliases for their default mode |
| `{"type":"dispatch","action":{...}}` | `{"type":"ok"}` or `{"type":"error","message":"..."}` | Runs a canonical `Action` (same as keybinds). On success state may change. |
| `{"type":"subscribe"}` | First `{"event":"snapshot",...}` then deltas | Event-only stream. |
| `{"type":"config-reload"}` | `{"type":"ok"}` or `{"type":"error","message":"..."}` | Re-read and apply the active config file. Fails keep previous config. |
| `{"type":"config-validate"}` | `{"type":"ok"}` or `{"type":"error","message":"..."}` | Parse/validate without applying. |

### Actions

Actions use kebab-case and match [`crab_ipc::Action`](../crab-ipc/src/protocol.rs). Common examples:
- `{"quit":{}}`
- `{"spawn":{"command":["alacritty"]}}`
- `{"close-window":{}}`
- `{"focus-direction":{"direction":"left"}}`
- `{"move-window":{"direction":"up"}}`
- `{"focus-cycle":{"forward":true}}`
- `{"switch-keyboard-layout":{"forward":true}}`
- `{"view-space":{"index":1}}`
- `{"move-to-space":{"index":2}}`
- `{"toggle-space-view":{"index":3}}`
- `{"toggle-space":{"index":3}}`
- `{"view-space-step":{"step":1}}`
- `{"view-space-have-client":{"step":-1}}`
- `{"toggle-back":{}}`
- `{"cycle-layout":{}}`
- `{"cycle-layout-family":{}}`
- `{"set-layout":{"name":"fair"}}` (mode or family name)
- `{"toggle-floating":{}}`
- `{"toggle-fullscreen":{}}`
- `{"toggle-maximized":{}}`
- `{"master-ratio":{"grow":true}}`
- `{"master-count":{"add":true}}`
- `{"preset-width":{}}`
- `{"center-column":{}}`
- `{"move-column":{"direction":"left"}}`
- `{"move-window-to-column":{"direction":"right"}}`
- `{"consume-window-into-column":{}}`
- `{"expel-window-from-column":{}}`
- `{"expand-column-to-available-width":{}}`
- `{"set-column-width":{"width":"0.5"}}` (also `proportion 0.5`, `800px`)
- `{"reload-config":{}}`
- `{"enter-mode":{"name":"resize"}}`
- `{"leave-mode":{}}`
- `{"pop-mode":{}}`

Config parsing accepts some mango-style aliases but the wire/protocol uses the canonical forms above.

### Events

After `subscribe` you receive:
- `{"event":"snapshot","outputs":[...],"spaces":[...],"windows":[...],"focused":<id|null>,"keyboard_layout":<string|null>,"config_error":<string|null>}`
- `{"event":"window-opened","window":{...}}`
- `{"event":"window-closed","id":<id>}`
- `{"event":"window-changed","window":{...}}`
- `{"event":"window-focused","id":<id|null>}`
- `{"event":"space-view-changed","output":<id>,"visible":[...]}` 
- `{"event":"layout-changed","output":<id>,"layout":"..."}` 
- `{"event":"output-added","output":{...}}`
- `{"event":"output-removed","id":<id>}`
- `{"event":"output-changed","output":{...}}`
- `{"event":"urgent","window":<id>,"urgent":<bool>}`
- `{"event":"config-reloaded"}` 
- `{"event":"config-error","message":"..."}` 
- `{"event":"keyboard-layout-changed","layout":"..."}`
- `{"event":"unknown"}` (for forward compatibility)

### Types (key fields)

**OutputInfo**
```json
{"id":1,"name":"DP-1","description":"DP-1","geometry":{"x":0,"y":0,"width":1920,"height":1080},"scale":1.0,"focused":true,"visible_spaces":[1],"layout":"tile"}
```

**SpaceInfo**
```json
{"output":1,"index":1,"name":null,"visible":true,"urgent":false}
```
`index` is 1-based. `spaces` only lists spaces that exist (occupied or visible).

**WindowInfo**
```json
{"id":42,"title":"Terminal","app_id":"foot","output":1,"spaces":[1],"focused":true,"urgent":false,"floating":false,"fullscreen":false,"maximized":false,"geometry":{"x":0,"y":0,"width":800,"height":600}}
```

**RectInfo**
```json
{"x":0.0,"y":0.0,"width":800.0,"height":800.0}
```

### Behavior notes

- Request lines are bounded (64 KiB). Invalid JSON/UTF-8 returns `error`.
- A client that stops reading is disconnected.
- On EOF, complete buffered requests are answered before closing.
- `spaces` reflects existing spaces (union of occupied and visible), not all 32 slots.
- `OutputChanged` excludes `visible_spaces` and `layout` (those have dedicated events).
- `KeyboardLayoutChanged` carries the xkb layout name reported by the keymap (for example `English (US)`), not the config spelling (`us`).
- Unknown event kinds deserialize as `unknown`.

## Bar integration

### Discovery
- Use `$CRAB_SOCKET` if set; otherwise derive from `WAYLAND_DISPLAY` as `$XDG_RUNTIME_DIR/crab-<display>.sock`. The compositor also exports `CRAB_SOCKET` to spawned/autostarted processes.
- Permissions are `0600`. If the socket is missing, the compositor may not be running or running under a different display.

### Connecting
- Use a library/client that speaks newline-delimited JSON. You can use [`crab-ipc`](../crab-ipc) (Rust) or any JSON socket client.
- For reading state + events: open one connection and send `{"type":"subscribe"}` to get an initial `snapshot` followed by deltas (event-only after subscribe). 
- For dispatching actions: use a **separate** connection and send `{"type":"dispatch","action":{...}}` (subscribed connections are event-only).
- Always handle reconnection/backoff and watch for disconnects.

### Typical flow
1. Connect and optionally call `version` to check `PROTOCOL_VERSION` (current: 2).
2. Subscribe to get full `snapshot` (outputs/spaces/windows/focused/keyboard_layout/config_error). Build your UI state from it.
3. Apply deltas (`window-opened/closed/changed`, `window-focused`, `space-view-changed`, `layout-changed`, `output-*`, `urgent`, `keyboard-layout-changed`, `config-reloaded`/`config-error`).
4. On user interaction, dispatch canonical actions over a separate request/response connection.

### Action mapping tips
- Use canonical action names from `crab_ipc::Action` (kebab-case). Config aliases are not required on the wire.
- Spaces use **1-based dynamic indices**. `spaces` only lists existing spaces (occupied or visible). To show a space, dispatch `{"view-space":{"index":n}}`.
- Move window: `{"move-to-space":{"index":n}}`. Toggle floating/fullscreen/maximized/close as needed.
- Layout: `{"cycle-layout":{}}`, `{"cycle-layout-family":{}}`, or `{"set-layout":{"name":"fair"}}` (mode or family name).
- Keyboard layout: `{"switch-keyboard-layout":{"forward":true}}`.
- Reload config: `{"reload-config":{}}` (or `crab-cli config-reload`).

### Event-to-UI guidance
| Event | What to update |
|---|---|
| `snapshot` | Replace entire state; note `keyboard_layout` and `config_error` |
| `window-opened`/`window-closed` | Add/remove window entries |
| `window-changed` | Update title/app_id/spaces/floating/fullscreen/maximized/geometry/urgent |
| `window-focused` | Update focus highlight |
| `space-view-changed` | Update which spaces are visible per output |
| `layout-changed` | Update layout label per output |
| `output-added/removed/changed` | Update outputs/geometry/scale/focus/description |
| `urgent` | Mark urgent (flash/highlight) |
| `keyboard-layout-changed` | Update layout indicator to reported xkb name (e.g. `English (US)`) |
| `config-reloaded` | Clear error state / toast success |
| `config-error` | Show error banner/info |

### Minimal examples

**Bash + socat + jq (read-only status):**
```bash
SOCK="${CRAB_SOCKET:-${XDG_RUNTIME_DIR}/crab-${WAYLAND_DISPLAY:-wayland-0}.sock}"
# Get spaces
echo '{"type":"spaces"}' | socat - UNIX-CONNECT:"$SOCK" | jq .
# Subscribe and print events
( echo '{"type":"subscribe"}'; cat ) | socat - UNIX-CONNECT:"$SOCK" | jq -c '.'
```
(Note: after `subscribe` the connection is event-only; do dispatches on another socket.)

**Waybar custom module (concept):**
- Use a long-running script that subscribes to events and writes JSON to stdout in waybar's expected format (or use `exec` with a script that polls/spawns).
- Example idea: maintain current workspace list (from `spaces` + `space-view-changed`) and focused window; emit JSON like `{"text": "...", "tooltip": "...", "class": [...]}`.
- Waybar's `custom` module can run a script; for real-time updates, have it read from a daemon that subscribes over IPC.

**eww (concept):**
- Define variables populated by a shell script that subscribes to `crab` events (via `socat`/`jq`/Python) and updates eww with `eww update var=...`. Dispatch on clicks with `crab-cli dispatch ...` or a second socket.

**Python (sketch):**
```python
import json, os, socket

sock = os.environ.get("CRAB_SOCKET") or os.path.expandvars("$XDG_RUNTIME_DIR/crab-$WAYLAND_DISPLAY.sock")
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(sock)
# subscribe
s.sendall(b'{"type":"subscribe"}\n')
f = s.makefile()
# read snapshot
snap = json.loads(f.readline())
# dispatch on separate conn needed for actions
```

### Tips & gotchas
- **Two connections**: subscribe on one, dispatch on another. Don't try to dispatch over event stream.
- **Spaces**: `index` is 1-based dynamic; `spaces` may not include unoccupied non-visible slots. Build workspace UI from existing spaces + visible sets.
- **Keyboard layout**: indicator should show `Snapshot.keyboard_layout` / `KeyboardLayoutChanged.layout` (xkb name, not config string).
- **Config errors**: watch `config-error`/`snapshot.config_error` to show a warning when reload fails (last good config remains).
- **Forward compatibility**: ignore unknown event types/fields; protocol is additive within major version (current v2).
- **Request sizing**: keep request lines reasonable; server bounds reads.
