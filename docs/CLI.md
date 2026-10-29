# crab-cli

`crab-cli` talks to the compositor over the Unix socket and provides query/dispatch/watch utilities.

## Usage

```bash
crab-cli [--socket <path>] [--raw] <command> [args...]
```

- `--socket`: path to the IPC socket (defaults via `crab_ipc::socket_path_for` using `WAYLAND_DISPLAY` or `CRAB_SOCKET`)
- `--raw`: machine output (JSON/compact) for scripting

## Commands

| Command | Description |
| --- | --- |
| `version` | Show compositor name and protocol version |
| `outputs` | List outputs |
| `spaces` | List existing spaces (occupied or visible) |
| `windows` | List mapped windows |
| `focused-window` | Show the focused window (or nothing) |
| `layouts` | List layout modes and families |
| `dispatch <action-json>` or `dispatch <action>` | Run a canonical action (see [IPC](IPC.md)). Examples: `dispatch '{"view-space":{"index":1}}'` or `dispatch view-space 1` depending on CLI parsing. |
| `watch` | Subscribe to events and print them as they arrive (skips unknown events). Press Ctrl-C to exit. |
| `config-reload` | Re-read and apply the active config file. |
| `config-validate` | Parse and validate the active config file without applying. |

## Examples

```bash
# Query state
crab-cli version
crab-cli outputs
crab-cli spaces
crab-cli windows
crab-cli focused-window
crab-cli layouts

# Dispatch actions
crab-cli dispatch '{"set-layout":{"name":"monocle"}}'
crab-cli dispatch '{"view-space":{"index":2}}'
crab-cli dispatch '{"switch-keyboard-layout":{"forward":true}}'
crab-cli dispatch '{"spawn":{"command":["alacritty"]}}'

# Watch events
crab-cli watch

# Config
crab-cli config-reload
crab-cli config-validate
```

## Notes

- The CLI uses the same `crab-ipc` types as the compositor, so action/event shapes stay in sync.
- `watch` connects to the event stream and will not dispatch actions (use a second connection).
